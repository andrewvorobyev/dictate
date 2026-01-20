use crate::audio::{encode_m4a, CpalRecorder, RecordingHandle};
use crate::beep;
use crate::cli::{Cli, Commands, RunArgs, TranscribeArgs};
use crate::clipboard::Clipboard;
use crate::config::{AutoTranscribeConfig, Config, ConfigStore, WatchPair};
use crate::logging;
use crate::model;
use crate::queue::{AutoJob, Job, JobKind, JobQueue, HotkeyJob};
use crate::storage;
use crate::transcriber::WhisperTranscriber;
use crate::tray::{TrayAction, TrayController, TrayState};
use anyhow::{Context, Result};
use clap::Parser;
use crossbeam_channel::{unbounded, Receiver, Sender};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use indicatif::{ProgressBar, ProgressStyle};
use notify::{Event as NotifyEvent, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoop};
#[cfg(target_os = "macos")]
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use tray_icon::menu::MenuEvent;
use tray_icon::{MouseButtonState, TrayIconEvent};

#[derive(Debug)]
enum WorkerEvent {
    ModelReady(PathBuf),
    ModelProgress(u8),
    ModelError(String),
    HotkeyRecordingReady(HotkeyJob),
    HotkeyRecordingError(String),
    AutoFileDetected(AutoJobSpec),
    TranscriptionProgress(u8),
    HotkeyTranscriptionDone { text: String },
    HotkeyTranscriptionError(String),
    AutoTranscriptionDone { input_path: PathBuf },
    AutoTranscriptionError { input_path: PathBuf, error: String },
    Error(String),
}

#[derive(Debug, Clone)]
struct AutoJobSpec {
    input_path: PathBuf,
    output_dir: PathBuf,
    processed_dir: PathBuf,
}

pub fn run() -> Result<()> {
    logging::init();
    let cli = Cli::parse();
    match cli.command.unwrap_or(Commands::Run(RunArgs::default())) {
        Commands::Run(args) => run_daemon(args),
        Commands::Transcribe(args) => run_transcribe(args),
        Commands::Models => list_models(),
    }
}

fn run_transcribe(args: TranscribeArgs) -> Result<()> {
    tracing::info!(input = %args.input.display(), "transcribe file");
    let store = ConfigStore::new()?;
    let config = store.load()?;
    let model = args
        .model
        .as_deref()
        .unwrap_or(config.model.as_str())
        .to_string();
    let vocabulary_prompt = vocabulary_prompt(&config.vocabulary);
    let models_dir = default_models_dir()?;
    let model_path = model::ensure_model(&models_dir, &model)?;
    let transcriber = WhisperTranscriber::new(model_path)?;
    let pb = ProgressBar::new(100);
    let style = ProgressStyle::with_template("{spinner} {bar:40} {pos}% {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("=>-")
        .tick_strings(&["-", "\\", "|", "/"]);
    pb.set_style(style);
    pb.set_message("transcribing");
    pb.enable_steady_tick(Duration::from_millis(120));
    let pb_ref = pb.clone();
    let text = match transcriber.transcribe_file_with_progress_and_prompt(
        &args.input,
        Some(move |pct| {
            let pct = if pct < 0 {
                0
            } else if pct > 100 {
                100
            } else {
                pct
            };
            pb_ref.set_position(pct as u64);
        }),
        vocabulary_prompt.as_deref(),
        args.language.as_deref(),
    ) {
        Ok(text) => text,
        Err(err) => {
            pb.finish_and_clear();
            return Err(err);
        }
    };
    pb.finish_and_clear();
    let output = storage::transcript_path_for_input(&args.input)?;
    fs::write(&output, &text)
        .with_context(|| format!("write transcript {}", output.display()))?;
    println!("{text}");
    tracing::info!(output = %output.display(), "transcription complete");
    Ok(())
}

fn list_models() -> Result<()> {
    let models = model::available_models();
    if models.is_empty() {
        println!("No models available.");
        return Ok(());
    }

    let size_labels: Vec<String> = models
        .iter()
        .map(|info| model::format_size(info.size_bytes))
        .collect();
    let name_width = models
        .iter()
        .map(|info| info.name.len())
        .max()
        .unwrap_or(0)
        .max("model".len());
    let size_width = size_labels
        .iter()
        .map(|size| size.len())
        .max()
        .unwrap_or(0)
        .max("size".len());
    let languages_width = models
        .iter()
        .map(|info| info.languages.label().len())
        .max()
        .unwrap_or(0)
        .max("languages".len());

    println!("Available models:");
    println!(
        "  {name:<name_width$}  {size:>size_width$}  {langs:<languages_width$}  {desc}",
        name = "model",
        size = "size",
        langs = "languages",
        desc = "description",
    );
    for (info, size_label) in models.iter().zip(size_labels.iter()) {
        println!(
            "  {name:<name_width$}  {size:>size_width$}  {langs:<languages_width$}  {desc}",
            name = info.name,
            size = size_label,
            langs = info.languages.label(),
            desc = info.description,
        );
    }
    Ok(())
}

fn run_daemon(args: RunArgs) -> Result<()> {
    tracing::info!("starting app");
    let store = ConfigStore::new()?;
    let mut config = store.load()?;
    if let Some(model) = args.model.clone() {
        config.model = model;
    }
    config.recordings_dir = args.recordings_dir.clone();
    store.save(&config)?;
    let vocabulary_prompt = vocabulary_prompt(&config.vocabulary);

    storage::ensure_dir(&config.recordings_dir)?;
    let devices = CpalRecorder::list_devices()?;
    let default_mic = CpalRecorder::default_device_name()?;
    let tray = TrayController::new(
        &devices,
        config.selected_mic.as_deref(),
        default_mic.as_deref(),
    )?;
    tray.set_state(TrayState::Downloading { progress: None })?;
    let beep = match beep::BeepPlayer::new() {
        Ok(player) => Some(player),
        Err(err) => {
            tracing::warn!(error = %err, "beep output unavailable");
            None
        }
    };

    let (worker_tx, worker_rx) = unbounded();
    if let Some(auto_cfg) = config.auto_transcribe.clone() {
        spawn_auto_transcribe_watchers(auto_cfg, worker_tx.clone())?;
    }
    let models_dir = default_models_dir()?;
    spawn_model_download(models_dir.clone(), config.model.clone(), worker_tx.clone());

        let app = App {
            config,
            store,
            tray,
            beep,
            downloading_model: true,
            model_download_progress: None,
            model_path: None,
            recordings_dir: args.recordings_dir,
            worker_rx,
            worker_tx,
            recording: None,
            queue: JobQueue::new(),
            transcription_progress: None,
            auto_inflight: HashSet::new(),
            vocabulary_prompt,
            last_theme_check: Instant::now(),
            hotkey_pending: false,
        };

    app.event_loop()
}

struct App {
    config: Config,
    store: ConfigStore,
    tray: TrayController,
    beep: Option<beep::BeepPlayer>,
    downloading_model: bool,
    model_download_progress: Option<u8>,
    model_path: Option<PathBuf>,
    recordings_dir: PathBuf,
    worker_rx: Receiver<WorkerEvent>,
    worker_tx: Sender<WorkerEvent>,
    recording: Option<RecordingHandle>,
    queue: JobQueue,
    transcription_progress: Option<u8>,
    auto_inflight: HashSet<PathBuf>,
    vocabulary_prompt: Option<String>,
    last_theme_check: Instant,
    hotkey_pending: bool,
}

impl App {
    fn event_loop(mut self) -> Result<()> {
        let mut event_loop = EventLoop::<()>::new();
        #[cfg(target_os = "macos")]
        {
            event_loop.set_activation_policy(ActivationPolicy::Accessory);
        }
        let hotkey_manager = GlobalHotKeyManager::new().context("init hotkey manager")?;
        let hotkey = HotKey::new(Some(Modifiers::ALT), Code::Space);
        hotkey_manager
            .register(hotkey)
            .context("register Command+Space")?;
        let hotkey_rx = GlobalHotKeyEvent::receiver();
        let menu_rx = MenuEvent::receiver();
        let tray_rx = TrayIconEvent::receiver();

        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(50));
            match event {
                Event::NewEvents(StartCause::Init) => {
                    tracing::info!("event loop started");
                }
                Event::MainEventsCleared => {
                    while let Ok(ev) = hotkey_rx.try_recv() {
                        if ev.id == hotkey.id() && ev.state == HotKeyState::Pressed {
                            if let Err(err) = self.handle_hotkey() {
                                tracing::error!(error = %err, "hotkey handler failed");
                            }
                        }
                    }
                    while let Ok(tray_event) = tray_rx.try_recv() {
                        if let TrayIconEvent::Click { button_state, .. } = tray_event {
                            if button_state == MouseButtonState::Down {
                                if let Err(err) = self.refresh_mic_menu() {
                                    tracing::error!(error = %err, "refresh mic menu failed");
                                }
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
                    if let Err(err) = self.maybe_refresh_idle_icon() {
                        tracing::error!(error = %err, "idle icon refresh failed");
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
                match name {
                    Some(name) => {
                        tracing::info!(mic = %name, "select microphone");
                        self.config.selected_mic = Some(name);
                    }
                    None => {
                        tracing::info!("select microphone: system default");
                        self.config.selected_mic = None;
                    }
                }
                self.tray
                    .set_selected_mic(self.config.selected_mic.as_deref());
                if self.config.selected_mic.is_none() {
                    let current_default = CpalRecorder::default_device_name()?;
                    self.tray.set_default_mic_label(current_default.as_deref());
                }
                self.store.save(&self.config)?;
            }
            TrayAction::ToggleRecording => {
                self.handle_hotkey()?;
            }
        }
        Ok(())
    }

    fn refresh_mic_menu(&mut self) -> Result<()> {
        let devices = CpalRecorder::list_devices()?;
        let default_mic = CpalRecorder::default_device_name()?;
        self.tray.refresh_microphones(
            &devices,
            self.config.selected_mic.as_deref(),
            default_mic.as_deref(),
        )?;
        self.update_tray_state()?;
        Ok(())
    }

    fn handle_worker(&mut self, event: WorkerEvent) -> Result<()> {
        match event {
            WorkerEvent::ModelReady(path) => {
                tracing::info!(path = %path.display(), "model ready");
                self.model_path = Some(path);
                self.downloading_model = false;
                self.model_download_progress = None;
                self.update_tray_state()?;
                self.maybe_start_transcription()?;
            }
            WorkerEvent::ModelProgress(pct) => {
                self.model_download_progress = Some(pct);
                if self.downloading_model {
                    self.update_tray_state()?;
                }
            }
            WorkerEvent::ModelError(err) => {
                tracing::error!(error = %err, "model download failed");
                self.downloading_model = false;
                self.model_download_progress = None;
                self.update_tray_state()?;
            }
            WorkerEvent::HotkeyRecordingReady(job) => {
                if !self.queue.enqueue_hotkey(job) {
                    tracing::warn!("hotkey recording already queued");
                }
                self.maybe_start_transcription()?;
            }
            WorkerEvent::HotkeyRecordingError(err) => {
                tracing::error!(error = %err, "recording failed");
                self.queue.cancel_hotkey_session();
                self.hotkey_pending = false;
                self.update_tray_state()?;
            }
            WorkerEvent::AutoFileDetected(spec) => {
                if let Err(err) = self.enqueue_auto_job(spec) {
                    tracing::error!(error = %err, "failed to enqueue auto transcription");
                }
            }
            WorkerEvent::TranscriptionProgress(pct) => {
                self.transcription_progress = Some(pct);
                self.update_tray_state()?;
            }
            WorkerEvent::HotkeyTranscriptionDone { text } => {
                tracing::info!("transcription done");
                println!("{text}");
                let mut clipboard = Clipboard::new()?;
                clipboard.set_text(&text)?;
                self.transcription_progress = None;
                self.queue.complete_active(JobKind::Hotkey);
                self.update_tray_state()?;
                self.maybe_start_transcription()?;
            }
            WorkerEvent::HotkeyTranscriptionError(err) => {
                tracing::error!(error = %err, "transcription failed");
                self.transcription_progress = None;
                self.queue.complete_active(JobKind::Hotkey);
                self.update_tray_state()?;
                self.maybe_start_transcription()?;
            }
            WorkerEvent::AutoTranscriptionDone { input_path } => {
                tracing::info!(path = %input_path.display(), "auto transcription done");
                self.auto_inflight.remove(&input_path);
                self.transcription_progress = None;
                self.queue.complete_active(JobKind::Auto);
                self.update_tray_state()?;
                self.maybe_start_transcription()?;
            }
            WorkerEvent::AutoTranscriptionError { input_path, error } => {
                tracing::error!(path = %input_path.display(), error = %error, "auto transcription failed");
                self.auto_inflight.remove(&input_path);
                self.transcription_progress = None;
                self.queue.complete_active(JobKind::Auto);
                self.update_tray_state()?;
                self.maybe_start_transcription()?;
            }
            WorkerEvent::Error(err) => {
                tracing::error!(error = %err, "worker error");
                self.update_tray_state()?;
            }
        }
        Ok(())
    }

    fn handle_hotkey(&mut self) -> Result<()> {
        if self.recording.is_some() {
            return self.stop_recording();
        }
        if self.downloading_model {
            tracing::info!("hotkey ignored while model is downloading");
            return Ok(());
        }
        self.start_recording()
    }

    fn start_recording(&mut self) -> Result<()> {
        if !self.queue.begin_hotkey_session() {
            tracing::info!("hotkey ignored while busy");
            return Ok(());
        }
        tracing::info!("start recording");
        self.play_beep();
        if self.config.selected_mic.is_none() {
            let current_default = CpalRecorder::default_device_name()?;
            self.tray.set_default_mic_label(current_default.as_deref());
        }
        match CpalRecorder::start_recording(self.config.selected_mic.as_deref()) {
            Ok(handle) => {
                self.recording = Some(handle);
                self.update_tray_state()?;
                Ok(())
            }
            Err(err) => {
                self.queue.cancel_hotkey_session();
                Err(err)
            }
        }
    }

    fn stop_recording(&mut self) -> Result<()> {
        tracing::info!("stop recording");
        let handle = self.recording.take().context("no recording in progress")?;
        let recordings_dir = self.recordings_dir.clone();
        let worker_tx = self.worker_tx.clone();
        self.hotkey_pending = true;
        self.transcription_progress = None;
        self.update_tray_state()?;
        tracing::info!("finalizing recording");
        self.play_beep();

        thread::spawn(move || {
            let result: Result<HotkeyJob> = (|| {
                let recorded = handle.stop()?;
                let (audio_path, text_path) = storage::next_recording_paths(&recordings_dir)?;
                encode_m4a(&recorded, &audio_path)?;
                Ok(HotkeyJob {
                    audio_path,
                    text_path,
                })
            })();
            match result {
                Ok(job) => {
                    let _ = worker_tx.send(WorkerEvent::HotkeyRecordingReady(job));
                }
                Err(err) => {
                    let _ = worker_tx.send(WorkerEvent::HotkeyRecordingError(err.to_string()));
                }
            }
        });
        Ok(())
    }

    fn play_beep(&mut self) {
        if let Some(player) = self.beep.as_mut() {
            if let Err(err) = player.play() {
                tracing::warn!(error = %err, "beep failed");
            }
        }
    }

    fn enqueue_auto_job(&mut self, spec: AutoJobSpec) -> Result<()> {
        if !is_m4a(&spec.input_path) {
            return Ok(());
        }
        if self.auto_inflight.contains(&spec.input_path) {
            return Ok(());
        }
        let output_path =
            storage::transcript_path_for_output_dir(&spec.input_path, &spec.output_dir)?;
        let processed_path =
            storage::processed_path_for_input(&spec.input_path, &spec.processed_dir)?;
        let job = AutoJob {
            input_path: spec.input_path.clone(),
            output_path,
            processed_path,
        };
        self.auto_inflight.insert(spec.input_path);
        self.queue.enqueue_auto(job);
        self.maybe_start_transcription()?;
        Ok(())
    }

    fn maybe_start_transcription(&mut self) -> Result<()> {
        let model_path = match self.model_path.clone() {
            Some(path) => path,
            None => return Ok(()),
        };
        let job = match self.queue.next_job() {
            Some(job) => job,
            None => return Ok(()),
        };
        if matches!(job, Job::Hotkey(_)) {
            self.hotkey_pending = false;
        }
        if let Job::Auto(_) = job {
            let total = self.queue.auto_queue_len() + 1;
            tracing::info!("auto transcription: processing 1 of {total}");
        }
        self.transcription_progress = None;
        self.update_tray_state()?;
        spawn_transcription(job, model_path, self.vocabulary_prompt.clone(), self.worker_tx.clone());
        Ok(())
    }

    fn update_tray_state(&mut self) -> Result<()> {
        if self.recording.is_some() {
            self.tray.set_state(TrayState::Recording)?;
            return Ok(());
        }
        if self.hotkey_pending {
            self.tray.set_state(TrayState::Transcribing {
                progress: self.transcription_progress,
            })?;
            return Ok(());
        }
        if self.queue.active_kind().is_some() {
            self.tray.set_state(TrayState::Transcribing {
                progress: self.transcription_progress,
            })?;
            return Ok(());
        }
        if self.downloading_model {
            self.tray.set_state(TrayState::Downloading {
                progress: self.model_download_progress,
            })?;
            return Ok(());
        }
        self.tray.sync_idle_theme()?;
        self.tray.set_state(TrayState::Idle)?;
        Ok(())
    }

    fn maybe_refresh_idle_icon(&mut self) -> Result<()> {
        if !self.is_idle() {
            return Ok(());
        }
        let now = Instant::now();
        if now.duration_since(self.last_theme_check) < Duration::from_secs(1) {
            return Ok(());
        }
        self.last_theme_check = now;
        self.tray.sync_idle_theme()?;
        Ok(())
    }

    fn is_idle(&self) -> bool {
        self.recording.is_none()
            && !self.hotkey_pending
            && self.queue.active_kind().is_none()
            && !self.downloading_model
    }
}

fn spawn_transcription(
    job: Job,
    model_path: PathBuf,
    prompt: Option<String>,
    tx: Sender<WorkerEvent>,
) {
    thread::spawn(move || match job {
        Job::Hotkey(job) => {
            if let Err(err) = transcribe_hotkey(&job, model_path, prompt.as_deref(), tx.clone()) {
                let _ = tx.send(WorkerEvent::HotkeyTranscriptionError(err.to_string()));
            }
        }
        Job::Auto(job) => {
            if let Err(err) = transcribe_auto(&job, model_path, prompt.as_deref(), tx.clone()) {
                let _ = tx.send(WorkerEvent::AutoTranscriptionError {
                    input_path: job.input_path.clone(),
                    error: err.to_string(),
                });
            }
        }
    });
}

fn transcribe_hotkey(
    job: &HotkeyJob,
    model_path: PathBuf,
    prompt: Option<&str>,
    tx: Sender<WorkerEvent>,
) -> Result<()> {
    let transcriber = WhisperTranscriber::new(model_path)?;
    let worker_progress = tx.clone();
    let mut last_pct: Option<i32> = None;
    let text = transcriber.transcribe_file_with_progress_and_prompt(
        &job.audio_path,
        Some(move |pct| {
            if last_pct == Some(pct) {
                return;
            }
            last_pct = Some(pct);
            let _ = worker_progress.send(WorkerEvent::TranscriptionProgress(
                pct.clamp(0, 100) as u8,
            ));
        }),
        prompt,
        None,
    )?;
    fs::write(&job.text_path, &text)
        .with_context(|| format!("write transcript {}", job.text_path.display()))?;
    tx.send(WorkerEvent::HotkeyTranscriptionDone { text })
        .context("send transcription event")?;
    Ok(())
}

fn transcribe_auto(
    job: &AutoJob,
    model_path: PathBuf,
    prompt: Option<&str>,
    tx: Sender<WorkerEvent>,
) -> Result<()> {
    let transcriber = WhisperTranscriber::new(model_path)?;
    let worker_progress = tx.clone();
    let mut last_pct: Option<i32> = None;
    let text = transcriber.transcribe_file_with_progress_and_prompt(
        &job.input_path,
        Some(move |pct| {
            if last_pct == Some(pct) {
                return;
            }
            last_pct = Some(pct);
            let _ = worker_progress.send(WorkerEvent::TranscriptionProgress(
                pct.clamp(0, 100) as u8,
            ));
        }),
        prompt,
        None,
    )?;
    if let Some(parent) = job.output_path.parent() {
        storage::ensure_dir(parent)?;
    }
    if let Some(parent) = job.processed_path.parent() {
        storage::ensure_dir(parent)?;
    }
    fs::write(&job.output_path, &text)
        .with_context(|| format!("write transcript {}", job.output_path.display()))?;
    fs::rename(&job.input_path, &job.processed_path).with_context(|| {
        format!(
            "move processed file {} -> {}",
            job.input_path.display(),
            job.processed_path.display()
        )
    })?;
    tx.send(WorkerEvent::AutoTranscriptionDone {
        input_path: job.input_path.clone(),
    })
    .context("send auto transcription event")?;
    Ok(())
}

fn spawn_auto_transcribe_watchers(config: AutoTranscribeConfig, tx: Sender<WorkerEvent>) -> Result<()> {
    if config.watches.is_empty() {
        return Ok(());
    }
    thread::spawn(move || {
        if let Err(err) = run_auto_transcribe_watcher(config, tx.clone()) {
            let _ = tx.send(WorkerEvent::Error(format!(
                "auto-transcribe watcher failed: {err}"
            )));
        }
    });
    Ok(())
}

fn run_auto_transcribe_watcher(
    config: AutoTranscribeConfig,
    tx: Sender<WorkerEvent>,
) -> Result<()> {
    storage::ensure_dir(&config.processed_dir)?;
    for watch in &config.watches {
        storage::ensure_dir(&watch.input_dir)?;
        storage::ensure_dir(&watch.output_dir)?;
    }

    enqueue_existing_files(&config, &tx)?;

    let (event_tx, event_rx) = unbounded();
    let mut watcher: RecommendedWatcher =
        notify::recommended_watcher(move |res| {
            let _ = event_tx.send(res);
        })
        .context("init watcher")?;
    for watch in &config.watches {
        watcher
            .watch(&watch.input_dir, RecursiveMode::NonRecursive)
            .with_context(|| format!("watch {}", watch.input_dir.display()))?;
    }

    for res in event_rx {
        match res {
            Ok(event) => handle_auto_event(event, &config, &tx),
            Err(err) => {
                let _ = tx.send(WorkerEvent::Error(format!(
                    "auto-transcribe watcher error: {err}"
                )));
            }
        }
    }
    Ok(())
}

fn enqueue_existing_files(config: &AutoTranscribeConfig, tx: &Sender<WorkerEvent>) -> Result<()> {
    for watch in &config.watches {
        let entries = fs::read_dir(&watch.input_dir)
            .with_context(|| format!("read dir {}", watch.input_dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            enqueue_auto_path(&path, watch, &config.processed_dir, tx);
        }
    }
    Ok(())
}

fn handle_auto_event(event: NotifyEvent, config: &AutoTranscribeConfig, tx: &Sender<WorkerEvent>) {
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any => {}
        _ => return,
    }
    for path in event.paths {
        for watch in &config.watches {
            if path.starts_with(&watch.input_dir) {
                enqueue_auto_path(&path, watch, &config.processed_dir, tx);
                break;
            }
        }
    }
}

fn enqueue_auto_path(path: &Path, watch: &WatchPair, processed_dir: &Path, tx: &Sender<WorkerEvent>) {
    if !is_m4a(path) {
        return;
    }
    if !wait_for_stable_file(path) {
        return;
    }
    let spec = AutoJobSpec {
        input_path: path.to_path_buf(),
        output_dir: watch.output_dir.clone(),
        processed_dir: processed_dir.to_path_buf(),
    };
    let _ = tx.send(WorkerEvent::AutoFileDetected(spec));
}

fn wait_for_stable_file(path: &Path) -> bool {
    let mut last_size = None;
    for _ in 0..3 {
        let size = match fs::metadata(path) {
            Ok(meta) => meta.len(),
            Err(_) => return false,
        };
        if Some(size) == last_size {
            return true;
        }
        last_size = Some(size);
        thread::sleep(Duration::from_millis(200));
    }
    false
}

fn is_m4a(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("m4a"))
        .unwrap_or(false)
}

fn vocabulary_prompt(vocabulary: &[String]) -> Option<String> {
    let mut words = Vec::new();
    for word in vocabulary {
        let trimmed = word.trim();
        if !trimmed.is_empty() {
            words.push(trimmed.to_string());
        }
    }
    if words.is_empty() {
        None
    } else {
        Some(format!("Vocabulary: {}", words.join(", ")))
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
                let _ = tx.send(WorkerEvent::ModelError(err.to_string()));
            }
        }
    });
}

fn default_models_dir() -> Result<PathBuf> {
    Ok(PathBuf::from(".models"))
}
