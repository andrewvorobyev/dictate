use crate::audio::{encode_m4a, CpalRecorder, RecordingHandle};
use crate::beep;
use crate::cli::{Cli, Commands, RunArgs, TranscribeArgs};
use crate::clipboard::Clipboard;
use crate::config::{Config, ConfigStore};
use crate::logging;
use crate::model;
use crate::storage;
use crate::transcriber::WhisperTranscriber;
use crate::tray::{TrayAction, TrayController, TrayState};
use anyhow::{Context, Result};
use clap::Parser;
use crossbeam_channel::{unbounded, Receiver, Sender};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager};
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoop};
use tray_icon::menu::MenuEvent;

#[derive(Debug, Clone)]
enum AppState {
    Idle,
    Recording,
    Transcribing,
    DownloadingModel,
}

#[derive(Debug)]
enum WorkerEvent {
    ModelReady(PathBuf),
    ModelProgress(u8),
    TranscriptionDone { text: String },
    Error(String),
}

pub fn run() -> Result<()> {
    logging::init();
    let cli = Cli::parse();
    match cli.command.unwrap_or(Commands::Run(RunArgs::default())) {
        Commands::Run(args) => run_daemon(args),
        Commands::Transcribe(args) => run_transcribe(args),
    }
}

fn run_transcribe(args: TranscribeArgs) -> Result<()> {
    tracing::info!(input = %args.input.display(), "transcribe file");
    let models_dir = default_models_dir()?;
    let model_path = model::ensure_model(&models_dir, &args.model)?;
    let transcriber = WhisperTranscriber::new(model_path)?;
    let text = transcriber.transcribe_file(&args.input)?;
    let output = storage::transcript_path_for_input(&args.input)?;
    fs::write(&output, &text)
        .with_context(|| format!("write transcript {}", output.display()))?;
    tracing::info!(output = %output.display(), "transcription complete");
    Ok(())
}

fn run_daemon(args: RunArgs) -> Result<()> {
    tracing::info!("starting app");
    let store = ConfigStore::new()?;
    let mut config = store.load()?;
    config.model = args.model.clone();
    config.recordings_dir = args.recordings_dir.clone();
    store.save(&config)?;

    storage::ensure_dir(&config.recordings_dir)?;
    let devices = CpalRecorder::list_devices()?;
    let tray = TrayController::new(&devices, config.selected_mic.as_deref())?;
    tray.set_state(TrayState::Downloading { progress: None })?;

    let (worker_tx, worker_rx) = unbounded();
    let models_dir = default_models_dir()?;
    spawn_model_download(models_dir.clone(), config.model.clone(), worker_tx.clone());

    let app = App {
        config,
        store,
        tray,
        state: AppState::DownloadingModel,
        model_path: None,
        recordings_dir: args.recordings_dir,
        worker_rx,
        worker_tx,
        recording: None,
    };

    app.event_loop()
}

struct App {
    config: Config,
    store: ConfigStore,
    tray: TrayController,
    state: AppState,
    model_path: Option<PathBuf>,
    recordings_dir: PathBuf,
    worker_rx: Receiver<WorkerEvent>,
    worker_tx: Sender<WorkerEvent>,
    recording: Option<RecordingHandle>,
}

impl App {
    fn event_loop(mut self) -> Result<()> {
        let event_loop = EventLoop::<()>::new();
        let hotkey_manager = GlobalHotKeyManager::new().context("init hotkey manager")?;
        let hotkey = HotKey::new(Some(Modifiers::META), Code::Space);
        hotkey_manager
            .register(hotkey)
            .context("register Command+Space")?;
        let hotkey_rx = GlobalHotKeyEvent::receiver();
        let menu_rx = MenuEvent::receiver();

        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(50));
            match event {
                Event::NewEvents(StartCause::Init) => {
                    tracing::info!("event loop started");
                }
                Event::MainEventsCleared => {
                    while let Ok(ev) = hotkey_rx.try_recv() {
                        if ev.id == hotkey.id() {
                            if let Err(err) = self.handle_hotkey() {
                                tracing::error!(error = %err, "hotkey handler failed");
                            }
                        }
                    }
                    while let Ok(menu_event) = menu_rx.try_recv() {
                        if let Some(action) = self.tray.action_for_menu(menu_event.id) {
                            if let Err(err) = self.handle_menu(action) {
                                tracing::error!(error = %err, "menu handler failed");
                            }
                        }
                    }
                    while let Ok(worker_event) = self.worker_rx.try_recv() {
                        if let Err(err) = self.handle_worker(worker_event) {
                            tracing::error!(error = %err, "worker handler failed");
                        }
                    }
                }
                _ => {}
            }
        });
    }

    fn handle_menu(&mut self, action: TrayAction) -> Result<()> {
        match action {
            TrayAction::Quit => {
                tracing::info!("quitting");
                std::process::exit(0);
            }
            TrayAction::SelectMic(name) => {
                tracing::info!(mic = %name, "select microphone");
                self.config.selected_mic = Some(name);
                if let Some(selected) = self.config.selected_mic.as_deref() {
                    self.tray.set_selected_mic(selected);
                }
                self.store.save(&self.config)?;
            }
        }
        Ok(())
    }

    fn handle_worker(&mut self, event: WorkerEvent) -> Result<()> {
        match event {
            WorkerEvent::ModelReady(path) => {
                tracing::info!(path = %path.display(), "model ready");
                self.model_path = Some(path);
                self.state = AppState::Idle;
                self.tray.set_state(TrayState::Idle)?;
            }
            WorkerEvent::ModelProgress(pct) => {
                if matches!(self.state, AppState::DownloadingModel) {
                    self.tray
                        .set_state(TrayState::Downloading { progress: Some(pct) })?;
                }
            }
            WorkerEvent::TranscriptionDone { text } => {
                tracing::info!("transcription done");
                let mut clipboard = Clipboard::new()?;
                clipboard.set_text(&text)?;
                self.state = AppState::Idle;
                self.tray.set_state(TrayState::Idle)?;
            }
            WorkerEvent::Error(err) => {
                tracing::error!(error = %err, "worker error");
                self.state = AppState::Idle;
                self.tray.set_state(TrayState::Idle)?;
            }
        }
        Ok(())
    }

    fn handle_hotkey(&mut self) -> Result<()> {
        match self.state {
            AppState::Idle => self.start_recording(),
            AppState::Recording => self.stop_recording(),
            AppState::Transcribing | AppState::DownloadingModel => {
                tracing::info!("hotkey ignored while busy");
                Ok(())
            }
        }
    }

    fn start_recording(&mut self) -> Result<()> {
        tracing::info!("start recording");
        beep::play().ok();
        let handle = CpalRecorder::start_recording(self.config.selected_mic.as_deref())?;
        self.recording = Some(handle);
        self.state = AppState::Recording;
        self.tray.set_state(TrayState::Recording)?;
        Ok(())
    }

    fn stop_recording(&mut self) -> Result<()> {
        tracing::info!("stop recording");
        beep::play().ok();
        let handle = self.recording.take().context("no recording in progress")?;
        let recordings_dir = self.recordings_dir.clone();
        let model_path = self.model_path.clone().context("model not ready")?;
        let worker_tx = self.worker_tx.clone();
        self.state = AppState::Transcribing;
        self.tray
            .set_state(TrayState::Transcribing { progress: None })?;
        tracing::info!("starting transcription");

        thread::spawn(move || {
            let result: Result<()> = (|| {
                let recorded = handle.stop()?;
                let (audio_path, text_path) = storage::next_recording_paths(&recordings_dir)?;
                encode_m4a(&recorded, &audio_path)?;
                let transcriber = WhisperTranscriber::new(model_path)?;
                let text = transcriber.transcribe_file(&audio_path)?;
                fs::write(&text_path, &text).with_context(|| {
                    format!("write transcript {}", text_path.display())
                })?;
                worker_tx
                    .send(WorkerEvent::TranscriptionDone { text })
                    .context("send transcription event")?;
                Ok(())
            })();
            if let Err(err) = result {
                let _ = worker_tx.send(WorkerEvent::Error(err.to_string()));
            }
        });
        Ok(())
    }
}

fn spawn_model_download(models_dir: PathBuf, model: String, tx: Sender<WorkerEvent>) {
    thread::spawn(move || {
        tracing::info!(model = %model, "ensuring model");
        let result = model::ensure_model_with_progress(&models_dir, &model, |pct| {
            let _ = tx.send(WorkerEvent::ModelProgress(pct));
        });
        match result {
            Ok(path) => {
                let _ = tx.send(WorkerEvent::ModelReady(path));
            }
            Err(err) => {
                let _ = tx.send(WorkerEvent::Error(err.to_string()));
            }
        }
    });
}

fn default_models_dir() -> Result<PathBuf> {
    let proj = directories::ProjectDirs::from("com", "dictate", "dictate-2")
        .context("resolve data dir")?;
    Ok(proj.data_dir().join("models"))
}
