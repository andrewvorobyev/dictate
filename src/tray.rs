use crate::audio::AudioDevice;
use anyhow::{Context, Result};
use std::collections::HashMap;
use tray_icon::menu::{Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

#[derive(Debug, Clone)]
pub enum TrayState {
    Idle,
    Recording,
    Transcribing { progress: Option<u8> },
    Downloading { progress: Option<u8> },
}

#[derive(Debug, Clone)]
pub enum TrayAction {
    Quit,
    SelectMic(String),
}

pub struct TrayController {
    tray: TrayIcon,
    status_item: MenuItem,
    mic_items: HashMap<MenuId, (String, MenuItem)>,
    quit_id: MenuId,
    icons: TrayIcons,
}

struct TrayIcons {
    idle: Icon,
    recording: Icon,
    transcribing: Icon,
    downloading: Icon,
}

impl TrayController {
    pub fn new(devices: &[AudioDevice], current_mic: Option<&str>) -> Result<Self> {
        let status_item = MenuItem::new("Status: Idle", false, None);
        let quit_item = PredefinedMenuItem::quit(None);
        let quit_id = quit_item.id();

        let mic_menu = Menu::new();
        let mut mic_items = HashMap::new();
        for dev in devices {
            let checked = current_mic.map(|m| m == dev.name).unwrap_or(false);
            let item = MenuItem::new(dev.name.clone(), true, None);
            item.set_checked(checked);
            mic_items.insert(item.id(), (dev.name.clone(), item.clone()));
            mic_menu.append(&item)?;
        }
        let mic_submenu = Submenu::new("Microphone", true, mic_menu);

        let menu = Menu::new();
        menu.append(&status_item)?;
        menu.append(&mic_submenu)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&quit_item)?;

        let icons = TrayIcons::new()?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Dictate 2")
            .with_icon(icons.idle.clone())
            .build()
            .context("create tray icon")?;

        Ok(Self {
            tray,
            status_item,
            mic_items,
            quit_id,
            icons,
        })
    }

    pub fn action_for_menu(&self, id: MenuId) -> Option<TrayAction> {
        if id == self.quit_id {
            return Some(TrayAction::Quit);
        }
        self.mic_items
            .get(&id)
            .map(|(name, _)| name.clone())
            .map(TrayAction::SelectMic)
    }

    pub fn set_selected_mic(&self, name: &str) {
        for (_id, (mic_name, item)) in &self.mic_items {
            item.set_checked(mic_name == name);
        }
    }

    pub fn set_state(&self, state: TrayState) -> Result<()> {
        match state {
            TrayState::Idle => {
                self.tray.set_icon(self.icons.idle.clone())?;
                self.status_item.set_title("Status: Idle");
            }
            TrayState::Recording => {
                self.tray.set_icon(self.icons.recording.clone())?;
                self.status_item.set_title("Status: Recording");
            }
            TrayState::Transcribing { progress } => {
                self.tray.set_icon(self.icons.transcribing.clone())?;
                let label = match progress {
                    Some(p) => format!("Status: Transcribing {p}%"),
                    None => "Status: Transcribing".to_string(),
                };
                self.status_item.set_title(&label);
            }
            TrayState::Downloading { progress } => {
                self.tray.set_icon(self.icons.downloading.clone())?;
                let label = match progress {
                    Some(p) => format!("Status: Loading model {p}%"),
                    None => "Status: Loading model".to_string(),
                };
                self.status_item.set_title(&label);
            }
        }
        Ok(())
    }
}

impl TrayIcons {
    fn new() -> Result<Self> {
        Ok(Self {
            idle: solid_icon([60, 120, 255, 255])?,
            recording: solid_icon([220, 40, 40, 255])?,
            transcribing: solid_icon([240, 200, 40, 255])?,
            downloading: solid_icon([120, 120, 120, 255])?,
        })
    }
}

fn solid_icon(color: [u8; 4]) -> Result<Icon> {
    let width = 16;
    let height = 16;
    let mut rgba = vec![0u8; width * height * 4];
    for chunk in rgba.chunks_exact_mut(4) {
        chunk.copy_from_slice(&color);
    }
    Icon::from_rgba(rgba, width as u32, height as u32).context("build icon")
}
