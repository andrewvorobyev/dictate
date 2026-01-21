use crate::audio::AudioDevice;
use anyhow::{Context, Result};
use std::collections::HashMap;
use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

const ICON_SIZE: usize = 44;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Theme {
    Light,
    Dark,
}

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
    menu: Menu,
    status_item: MenuItem,
    start_stop_item: MenuItem,
    default_mic_item: CheckMenuItem,
    mic_items: HashMap<MenuId, (String, CheckMenuItem)>,
    mic_separator: PredefinedMenuItem,
    quit_id: MenuId,
    icons: TrayIcons,
    idle_theme: Theme,
}

struct TrayIcons {
    idle_light: Icon,
    idle_dark: Icon,
    recording: Icon,
    downloading: Icon,
}

impl TrayController {
    pub fn new(
        devices: &[AudioDevice],
        current_mic: Option<&str>,
        default_mic_label: Option<&str>,
    ) -> Result<Self> {
        let menu_parts = Self::build_menu(
            devices,
            current_mic,
            default_mic_label,
            "Status: Idle",
            "Start Recording (Option+Space)",
        )?;

        let icons = TrayIcons::new()?;
        let idle_theme = current_theme();
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu_parts.menu.clone()))
            .with_tooltip("Dictate")
            .with_icon(icons.idle_for_theme(idle_theme))
            .with_icon_as_template(true)
            .build()
            .context("create tray icon")?;

        Ok(Self {
            tray,
            menu: menu_parts.menu,
            status_item: menu_parts.status_item,
            start_stop_item: menu_parts.start_stop_item,
            default_mic_item: menu_parts.default_mic_item,
            mic_items: menu_parts.mic_items,
            mic_separator: menu_parts.mic_separator,
            quit_id: menu_parts.quit_id,
            icons,
            idle_theme,
        })
    }

    pub fn refresh_microphones(
        &mut self,
        devices: &[AudioDevice],
        current_mic: Option<&str>,
        default_mic_label: Option<&str>,
    ) -> Result<()> {
        let default_label = match default_mic_label {
            Some(name) => format!("System Default ({name})"),
            None => "System Default".to_string(),
        };
        self.default_mic_item.set_text(default_label);
        self.default_mic_item.set_checked(current_mic.is_none());

        let old_items = std::mem::take(&mut self.mic_items);
        for (_, (_, item)) in old_items {
            self.menu.remove(&item)?;
        }

        let insert_pos = self
            .menu
            .items()
            .iter()
            .position(|item| item.id() == self.mic_separator.id())
            .unwrap_or_else(|| self.menu.items().len());
        let mut new_items = HashMap::new();
        for (idx, dev) in devices.iter().enumerate() {
            let checked = current_mic.map(|m| m == dev.name).unwrap_or(false);
            let item = CheckMenuItem::new(dev.name.clone(), true, checked, None);
            self.menu.insert(&item, insert_pos + idx)?;
            new_items.insert(item.id().clone(), (dev.name.clone(), item.clone()));
        }
        self.mic_items = new_items;
        Ok(())
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
                self.apply_icon(self.icons.idle_for_theme(self.idle_theme), true)?;
                self.status_item.set_text("Status: Idle");
                self.start_stop_item
                    .set_text("Start Recording (Option+Space)");
            }
            TrayState::Recording => {
                self.apply_icon(self.icons.recording.clone(), false)?;
                self.status_item.set_text("Status: Recording");
                self.start_stop_item
                    .set_text("Stop Recording (Option+Space)");
            }
            TrayState::Transcribing { progress } => {
                let icon = icon_transcribing(progress)?;
                self.apply_icon(icon, false)?;
                let label = match progress {
                    Some(p) => format!("Status: Transcribing {p}%"),
                    None => "Status: Transcribing".to_string(),
                };
                self.status_item.set_text(&label);
                self.start_stop_item
                    .set_text("Start Recording (Option+Space)");
            }
            TrayState::Downloading { progress } => {
                self.apply_icon(self.icons.downloading.clone(), false)?;
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

    pub fn sync_idle_theme(&mut self) -> Result<()> {
        let theme = current_theme();
        if theme != self.idle_theme {
            self.idle_theme = theme;
            self.apply_icon(self.icons.idle_for_theme(self.idle_theme), true)?;
        }
        Ok(())
    }

    fn apply_icon(&self, icon: Icon, is_template: bool) -> Result<()> {
        self.tray.set_icon(Some(icon))?;
        self.tray.set_icon_as_template(is_template);
        Ok(())
    }
}

impl TrayIcons {
    fn new() -> Result<Self> {
        Ok(Self {
            idle_light: icon_idle_mic(IdlePalette::light())?,
            idle_dark: icon_idle_mic(IdlePalette::dark())?,
            recording: icon_recording()?,
            downloading: icon_downloading()?,
        })
    }

    fn idle_for_theme(&self, theme: Theme) -> Icon {
        match theme {
            Theme::Light => self.idle_dark.clone(),
            Theme::Dark => self.idle_light.clone(),
        }
    }
}

struct MenuParts {
    menu: Menu,
    status_item: MenuItem,
    start_stop_item: MenuItem,
    default_mic_item: CheckMenuItem,
    mic_items: HashMap<MenuId, (String, CheckMenuItem)>,
    mic_separator: PredefinedMenuItem,
    quit_id: MenuId,
}

impl TrayController {
    fn build_menu(
        devices: &[AudioDevice],
        current_mic: Option<&str>,
        default_mic_label: Option<&str>,
        status_label: &str,
        start_stop_label: &str,
    ) -> Result<MenuParts> {
        let status_item = MenuItem::new(status_label, false, None);
        let start_stop_item = MenuItem::new(start_stop_label, true, None);
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
        let mic_separator = PredefinedMenuItem::separator();
        menu.append(&mic_separator)?;
        menu.append(&quit_item)?;

        Ok(MenuParts {
            menu,
            status_item,
            start_stop_item,
            default_mic_item,
            mic_items,
            mic_separator,
            quit_id,
        })
    }
}

struct IdlePalette {
    body: [u8; 4],
    arm: [u8; 4],
    highlight: [u8; 4],
    grille: [u8; 4],
}

impl IdlePalette {
    fn light() -> Self {
        Self {
            body: [255, 255, 255, 255],
            arm: [220, 220, 220, 255],
            highlight: [0, 0, 0, 25],
            grille: [0, 0, 0, 80],
        }
    }

    fn dark() -> Self {
        Self {
            body: [0, 0, 0, 255],
            arm: [40, 40, 40, 255],
            highlight: [255, 255, 255, 35],
            grille: [255, 255, 255, 90],
        }
    }
}

fn icon_idle_mic(palette: IdlePalette) -> Result<Icon> {
    let mut canvas = empty_canvas();
    let cx = ICON_SIZE as f32 / 2.0;

    // Microphone body - elegant capsule shape
    draw_capsule_aa(&mut canvas, cx, 4.0, 16.0, 24.0, palette.body);

    // Subtle highlight on left side of mic body for depth
    draw_capsule_aa(
        &mut canvas,
        cx - 3.0,
        6.0,
        3.0,
        18.0,
        palette.highlight,
    );

    // Microphone grille lines - delicate horizontal lines
    for i in 0..4 {
        let y = 10.0 + i as f32 * 4.0;
        draw_line_h_aa(&mut canvas, cx - 5.0, y, 10.0, palette.grille);
    }

    // Stand/stem - tapered elegant stem
    draw_rect_aa(&mut canvas, cx - 1.5, 28.0, 3.0, 8.0, palette.body);

    // Curved arm holding the mic
    draw_arc_aa(
        &mut canvas,
        cx,
        28.0,
        10.0,
        0.0,
        std::f32::consts::PI,
        2.5,
        palette.arm,
    );

    // Base - solid horizontal bar
    draw_rect_aa(&mut canvas, cx - 10.0, 40.0, 20.0, 2.5, palette.body);

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

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn current_theme() -> Theme {
    use objc::{class, msg_send, sel, sel_impl};
    use objc::runtime::Object;
    use std::ffi::{CStr, CString};
    use std::os::raw::c_char;

    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        if !app.is_null() {
            let appearance: *mut Object = msg_send![app, effectiveAppearance];
            if !appearance.is_null() {
                let name: *mut Object = msg_send![appearance, name];
                if !name.is_null() {
                    let name_ptr: *const c_char = msg_send![name, UTF8String];
                    if !name_ptr.is_null() {
                        let name = CStr::from_ptr(name_ptr).to_string_lossy();
                        if name.contains("Dark") {
                            return Theme::Dark;
                        }
                        return Theme::Light;
                    }
                }
            }
        }

        let defaults: *mut Object = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let key = CString::new("AppleInterfaceStyle").expect("cstring");
        let key_ns: *mut Object =
            msg_send![class!(NSString), stringWithUTF8String: key.as_ptr()];
        let style: *mut Object = msg_send![defaults, stringForKey: key_ns];
        if style.is_null() {
            return Theme::Light;
        }
        let style_ptr: *const c_char = msg_send![style, UTF8String];
        if style_ptr.is_null() {
            return Theme::Light;
        }
        let style = CStr::from_ptr(style_ptr).to_string_lossy();
        if style.contains("Dark") {
            Theme::Dark
        } else {
            Theme::Light
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn current_theme() -> Theme {
    Theme::Light
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

// Alpha-blend a color onto the canvas at (x, y)
fn blend_pixel(canvas: &mut [u8], x: i32, y: i32, color: [u8; 4], alpha: f32) {
    if x < 0 || y < 0 || x >= ICON_SIZE as i32 || y >= ICON_SIZE as i32 {
        return;
    }
    let idx = ((y as usize) * ICON_SIZE + (x as usize)) * 4;
    let a = (color[3] as f32 / 255.0) * alpha;
    if a <= 0.0 {
        return;
    }

    let dst_r = canvas[idx] as f32;
    let dst_g = canvas[idx + 1] as f32;
    let dst_b = canvas[idx + 2] as f32;
    let dst_a = canvas[idx + 3] as f32 / 255.0;

    let src_r = color[0] as f32;
    let src_g = color[1] as f32;
    let src_b = color[2] as f32;

    let out_a = a + dst_a * (1.0 - a);
    if out_a > 0.0 {
        canvas[idx] = ((src_r * a + dst_r * dst_a * (1.0 - a)) / out_a) as u8;
        canvas[idx + 1] = ((src_g * a + dst_g * dst_a * (1.0 - a)) / out_a) as u8;
        canvas[idx + 2] = ((src_b * a + dst_b * dst_a * (1.0 - a)) / out_a) as u8;
        canvas[idx + 3] = (out_a * 255.0) as u8;
    }
}

// Anti-aliased circle
fn draw_circle_aa(canvas: &mut [u8], cx: f32, cy: f32, r: f32, color: [u8; 4]) {
    let x_min = (cx - r - 1.0).floor() as i32;
    let x_max = (cx + r + 1.0).ceil() as i32;
    let y_min = (cy - r - 1.0).floor() as i32;
    let y_max = (cy + r + 1.0).ceil() as i32;

    for y in y_min..=y_max {
        for x in x_min..=x_max {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let alpha = (r - dist + 0.5).clamp(0.0, 1.0);
            if alpha > 0.0 {
                blend_pixel(canvas, x, y, color, alpha);
            }
        }
    }
}

// Anti-aliased rectangle
fn draw_rect_aa(canvas: &mut [u8], x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
    let x_min = (x - 0.5).floor() as i32;
    let x_max = (x + w + 0.5).ceil() as i32;
    let y_min = (y - 0.5).floor() as i32;
    let y_max = (y + h + 0.5).ceil() as i32;

    for py in y_min..=y_max {
        for px in x_min..=x_max {
            let px_f = px as f32;
            let py_f = py as f32;

            // Calculate coverage
            let left = (px_f + 1.0 - x).clamp(0.0, 1.0);
            let right = (x + w - px_f).clamp(0.0, 1.0);
            let top = (py_f + 1.0 - y).clamp(0.0, 1.0);
            let bottom = (y + h - py_f).clamp(0.0, 1.0);

            let alpha = left * right * top * bottom;
            if alpha > 0.0 {
                blend_pixel(canvas, px, py, color, alpha);
            }
        }
    }
}

// Anti-aliased capsule (rounded rectangle for microphone body)
fn draw_capsule_aa(canvas: &mut [u8], cx: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
    let r = w / 2.0;
    let mid_h = h - w;

    // Top circle
    draw_circle_aa(canvas, cx, y + r, r, color);
    // Bottom circle
    draw_circle_aa(canvas, cx, y + r + mid_h, r, color);
    // Middle rectangle
    draw_rect_aa(canvas, cx - r, y + r, w, mid_h, color);
}

// Anti-aliased horizontal line
fn draw_line_h_aa(canvas: &mut [u8], x: f32, y: f32, w: f32, color: [u8; 4]) {
    draw_rect_aa(canvas, x, y, w, 1.0, color);
}

// Anti-aliased arc (stroke only)
fn draw_arc_aa(
    canvas: &mut [u8],
    cx: f32,
    cy: f32,
    r: f32,
    start_angle: f32,
    end_angle: f32,
    thickness: f32,
    color: [u8; 4],
) {
    let r_outer = r + thickness / 2.0;
    let r_inner = r - thickness / 2.0;
    let x_min = (cx - r_outer - 1.0).floor() as i32;
    let x_max = (cx + r_outer + 1.0).ceil() as i32;
    let y_min = (cy - r_outer - 1.0).floor() as i32;
    let y_max = (cy + r_outer + 1.0).ceil() as i32;

    for y in y_min..=y_max {
        for x in x_min..=x_max {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();

            // Check if within ring
            let outer_alpha = (r_outer - dist + 0.5).clamp(0.0, 1.0);
            let inner_alpha = (dist - r_inner + 0.5).clamp(0.0, 1.0);
            let ring_alpha = outer_alpha * inner_alpha;

            if ring_alpha > 0.0 {
                // Check angle
                let angle = dy.atan2(dx);
                let mut a = angle;
                if a < start_angle {
                    a += std::f32::consts::TAU;
                }
                if a >= start_angle && a <= end_angle + start_angle {
                    blend_pixel(canvas, x, y, color, ring_alpha);
                }
            }
        }
    }
}
