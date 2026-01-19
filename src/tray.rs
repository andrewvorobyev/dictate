use crate::audio::AudioDevice;
use anyhow::{Context, Result};
use std::collections::HashMap;
use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

const ICON_SIZE: usize = 44;

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
    SelectMic(Option<String>),
    ToggleRecording,
}

pub struct TrayController {
    tray: TrayIcon,
    status_item: MenuItem,
    start_stop_item: MenuItem,
    default_mic_item: CheckMenuItem,
    mic_items: HashMap<MenuId, (String, CheckMenuItem)>,
    quit_id: MenuId,
    icons: TrayIcons,
}

struct TrayIcons {
    idle: Icon,
    recording: Icon,
    downloading: Icon,
}

impl TrayController {
    pub fn new(
        devices: &[AudioDevice],
        current_mic: Option<&str>,
        default_mic_label: Option<&str>,
    ) -> Result<Self> {
        let status_item = MenuItem::new("Status: Idle", false, None);
        let start_stop_item = MenuItem::new("Start Recording (Option+Space)", true, None);
        let quit_item = PredefinedMenuItem::quit(None);
        let quit_id = quit_item.id().clone();

        let menu = Menu::new();
        menu.append(&status_item)?;
        menu.append(&start_stop_item)?;
        menu.append(&PredefinedMenuItem::separator())?;
        let mic_header = MenuItem::new("Microphones", false, None);
        menu.append(&mic_header)?;
        let default_label = match default_mic_label {
            Some(name) => format!("System Default ({name})"),
            None => "System Default".to_string(),
        };
        let default_mic_item =
            CheckMenuItem::new(default_label, true, current_mic.is_none(), None);
        menu.append(&default_mic_item)?;
        let mut mic_items = HashMap::new();
        for dev in devices {
            let checked = current_mic.map(|m| m == dev.name).unwrap_or(false);
            let item = CheckMenuItem::new(dev.name.clone(), true, checked, None);
            menu.append(&item)?;
            mic_items.insert(item.id().clone(), (dev.name.clone(), item.clone()));
        }
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
            start_stop_item,
            default_mic_item,
            mic_items,
            quit_id,
            icons,
        })
    }

    pub fn action_for_menu(&self, id: MenuId) -> Option<TrayAction> {
        if id == self.start_stop_item.id().clone() {
            return Some(TrayAction::ToggleRecording);
        }
        if id == self.quit_id {
            return Some(TrayAction::Quit);
        }
        if id == self.default_mic_item.id().clone() {
            return Some(TrayAction::SelectMic(None));
        }
        self.mic_items
            .get(&id)
            .map(|(name, _)| TrayAction::SelectMic(Some(name.clone())))
    }

    pub fn set_selected_mic(&self, name: Option<&str>) {
        self.default_mic_item.set_checked(name.is_none());
        for (_id, (mic_name, item)) in &self.mic_items {
            item.set_checked(name == Some(mic_name.as_str()));
        }
    }

    pub fn set_default_mic_label(&self, name: Option<&str>) {
        let label = match name {
            Some(name) => format!("System Default ({name})"),
            None => "System Default".to_string(),
        };
        self.default_mic_item.set_text(label);
    }

    pub fn set_state(&self, state: TrayState) -> Result<()> {
        match state {
            TrayState::Idle => {
                self.tray.set_icon(Some(self.icons.idle.clone()))?;
                self.status_item.set_text("Status: Idle");
                self.start_stop_item
                    .set_text("Start Recording (Option+Space)");
            }
            TrayState::Recording => {
                self.tray.set_icon(Some(self.icons.recording.clone()))?;
                self.status_item.set_text("Status: Recording");
                self.start_stop_item
                    .set_text("Stop Recording (Option+Space)");
            }
            TrayState::Transcribing { progress } => {
                let icon = icon_transcribing(progress)?;
                self.tray.set_icon(Some(icon))?;
                let label = match progress {
                    Some(p) => format!("Status: Transcribing {p}%"),
                    None => "Status: Transcribing".to_string(),
                };
                self.status_item.set_text(&label);
                self.start_stop_item
                    .set_text("Start Recording (Option+Space)");
            }
            TrayState::Downloading { progress } => {
                self.tray
                    .set_icon(Some(self.icons.downloading.clone()))?;
                let label = match progress {
                    Some(p) => format!("Status: Loading model {p}%"),
                    None => "Status: Loading model".to_string(),
                };
                self.status_item.set_text(&label);
                self.start_stop_item
                    .set_text("Start Recording (Option+Space)");
            }
        }
        Ok(())
    }
}

impl TrayIcons {
    fn new() -> Result<Self> {
        Ok(Self {
            idle: icon_idle_mic()?,
            recording: icon_recording()?,
            downloading: icon_downloading()?,
        })
    }
}

fn icon_idle_mic() -> Result<Icon> {
    let mut canvas = empty_canvas();
    let black = [0, 0, 0, 255];
    let cx = (ICON_SIZE / 2) as i32;
    let top = 2;
    draw_capsule(&mut canvas, cx, top, 18, 26, black);
    draw_rect(&mut canvas, cx - 1, top + 26, 2, 13, black);
    draw_rect(&mut canvas, cx - 9, 41, 18, 2, black);
    draw_rect(&mut canvas, cx - 6, top + 6, 12, 1, [0, 0, 0, 180]);
    draw_rect(&mut canvas, cx - 6, top + 9, 12, 1, [0, 0, 0, 180]);
    draw_rect(&mut canvas, cx - 6, top + 12, 12, 1, [0, 0, 0, 180]);
    Icon::from_rgba(canvas, ICON_SIZE as u32, ICON_SIZE as u32).context("build idle icon")
}

fn icon_recording() -> Result<Icon> {
    let mut canvas = empty_canvas();
    let red = [220, 24, 32, 255];
    let cx = (ICON_SIZE / 2) as i32;
    draw_circle(&mut canvas, cx, cx, 21, red);
    Icon::from_rgba(canvas, ICON_SIZE as u32, ICON_SIZE as u32).context("build recording icon")
}

fn icon_transcribing(progress: Option<u8>) -> Result<Icon> {
    let mut canvas = empty_canvas();
    let base = [240, 200, 40, 255];
    let fill = [0, 0, 0, 255];
    let cx = (ICON_SIZE / 2) as i32;
    draw_circle(&mut canvas, cx, cx, 21, base);
    if let Some(pct) = progress {
        let angle = (pct.min(100) as f32) / 100.0 * std::f32::consts::TAU;
        draw_wedge(&mut canvas, cx, cx, 21, angle, fill);
    }
    Icon::from_rgba(canvas, ICON_SIZE as u32, ICON_SIZE as u32).context("build transcribing icon")
}

fn icon_downloading() -> Result<Icon> {
    let mut canvas = empty_canvas();
    let gray = [120, 120, 120, 255];
    let cx = (ICON_SIZE / 2) as i32;
    draw_ring(&mut canvas, cx, cx, 21, 15, gray);
    Icon::from_rgba(canvas, ICON_SIZE as u32, ICON_SIZE as u32).context("build downloading icon")
}

fn empty_canvas() -> Vec<u8> {
    vec![0u8; ICON_SIZE * ICON_SIZE * 4]
}

fn set_pixel(canvas: &mut [u8], x: i32, y: i32, color: [u8; 4]) {
    if x < 0 || y < 0 || x >= ICON_SIZE as i32 || y >= ICON_SIZE as i32 {
        return;
    }
    let idx = ((y as usize) * ICON_SIZE + (x as usize)) * 4;
    canvas[idx..idx + 4].copy_from_slice(&color);
}

fn draw_rect(canvas: &mut [u8], x: i32, y: i32, w: i32, h: i32, color: [u8; 4]) {
    for yy in y..(y + h) {
        for xx in x..(x + w) {
            set_pixel(canvas, xx, yy, color);
        }
    }
}

fn draw_circle(canvas: &mut [u8], cx: i32, cy: i32, r: i32, color: [u8; 4]) {
    let r2 = r * r;
    for y in (cy - r)..=(cy + r) {
        for x in (cx - r)..=(cx + r) {
            let dx = x - cx;
            let dy = y - cy;
            if dx * dx + dy * dy <= r2 {
                set_pixel(canvas, x, y, color);
            }
        }
    }
}

fn draw_capsule(canvas: &mut [u8], cx: i32, y: i32, w: i32, h: i32, color: [u8; 4]) {
    let r = w / 2;
    let mid_h = h - w;
    draw_circle(canvas, cx, y + r, r, color);
    draw_circle(canvas, cx, y + r + mid_h, r, color);
    draw_rect(canvas, cx - r, y + r, w, mid_h, color);
}

fn draw_ring(canvas: &mut [u8], cx: i32, cy: i32, r_outer: i32, r_inner: i32, color: [u8; 4]) {
    let r_outer2 = r_outer * r_outer;
    let r_inner2 = r_inner * r_inner;
    for y in (cy - r_outer)..=(cy + r_outer) {
        for x in (cx - r_outer)..=(cx + r_outer) {
            let dx = x - cx;
            let dy = y - cy;
            let dist2 = dx * dx + dy * dy;
            if dist2 <= r_outer2 && dist2 >= r_inner2 {
                set_pixel(canvas, x, y, color);
            }
        }
    }
}

fn draw_wedge(canvas: &mut [u8], cx: i32, cy: i32, r: i32, angle: f32, color: [u8; 4]) {
    let r2 = r * r;
    for y in (cy - r)..=(cy + r) {
        for x in (cx - r)..=(cx + r) {
            let dx = x - cx;
            let dy = y - cy;
            let dist2 = dx * dx + dy * dy;
            if dist2 <= r2 {
                let ang = (dy as f32).atan2(dx as f32);
                let ang = if ang < -std::f32::consts::FRAC_PI_2 {
                    ang + std::f32::consts::TAU
                } else {
                    ang
                };
                let ang = ang + std::f32::consts::FRAC_PI_2;
                if ang <= angle {
                    set_pixel(canvas, x, y, color);
                }
            }
        }
    }
}
