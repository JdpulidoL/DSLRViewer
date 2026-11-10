//! Application shell: state, engine event handling and layout.

mod camera_panel;
mod dock;
mod header;
mod meters;
mod selftest;
mod theme;
mod tools_panel;
mod viewer;
mod widgets;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Color32, ColorImage, Context, TextureHandle, TextureOptions};

use crate::audio::{self, AudioMonitor};
use crate::camera::{Capabilities, FocusDirection, Identity, Setting};
use crate::config::Config;
use crate::engine::{BracketPlan, Command, Engine, Event, Level, Thumb};
use crate::imaging::stack::{Method, Options as StackOptions};
use crate::imaging::{Frame, Peaking};
use crate::link::DeviceEntry;

/// Focus-drive step presets: label and drive steps.
pub const FOCUS_STEPS: [(&str, u32); 3] = [("Fine", 5), ("Medium", 25), ("Coarse", 100)];

pub enum Connection {
    Disconnected,
    Connecting,
    Connected { identity: Identity, caps: Capabilities },
}

pub struct CaptureItem {
    pub label: String,
    pub path: Option<PathBuf>,
    pub texture: Option<TextureHandle>,
}

pub struct LogLine {
    pub level: Level,
    pub at: Duration,
    pub text: String,
}

pub struct Toast {
    pub level: Level,
    pub text: String,
    pub born: Instant,
}

#[derive(Default)]
pub struct StackState {
    pub running: bool,
    pub progress: f32,
    pub result: Option<(PathBuf, Option<TextureHandle>)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DockTab {
    Captures,
    Activity,
}

/// `HH:MM:SS` (or `MM:SS` under an hour).
pub fn format_clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{:02}:{:02}", s / 60, s % 60) }
}

pub struct LumenApp {
    pub engine: Engine,
    pub audio: AudioMonitor,
    pub audio_devices: Vec<String>,
    pub audio_on: bool,
    pub audio_info: Option<String>,
    pub audio_channels: usize,
    pub audio_clip: bool,
    /// When the input last had no audible signal (None while sound is present).
    audio_quiet_since: Option<Instant>,
    pub meters: [meters::Meter; 2],
    pub cfg: Config,
    started: Instant,

    pub devices: Vec<DeviceEntry>,
    pub selected_device: usize,
    pub connection: Connection,
    pub settings: Vec<Setting>,

    pub live: bool,
    pub frame_tex: Option<TextureHandle>,
    pub frame_size: (usize, usize),
    pub histogram: Vec<u32>,
    pub sharpness: VecDeque<f32>,
    pub sharp_peak: f32,
    pub fps: f32,
    pub last_frame_at: Option<Instant>,
    /// Settings just changed by hand on the camera body, for a short highlight in the panel and OSD.
    pub hand_changed: std::collections::HashMap<u16, Instant>,
    /// `(to_video, since)` while the camera re-initialises after the mode dial moved.
    pub mode_switch: Option<(bool, Instant)>,
    /// Live-view frames received since start (used by the self-test).
    pub frames_seen: u64,
    /// Last (folder, prefix) sent to the engine for camera-button shots.
    output_sent: Option<(PathBuf, String)>,
    pub zoom: f32,
    pub pan: egui::Vec2,

    pub captures: Vec<CaptureItem>,
    pub selected_capture: Option<usize>,
    pub capture_pending: bool,
    pub bracket: Option<(u32, u32)>,
    pub last_sequence: Vec<PathBuf>,
    pub stack: StackState,
    pub focus_hold: Option<(FocusDirection, Instant)>,
    pub recording: bool,
    pub rec_started: Option<Instant>,
    pub fullscreen: bool,
    pub last_pointer_move: Instant,

    pub logs: VecDeque<LogLine>,
    pub toasts: Vec<Toast>,
    pub dock_tab: DockTab,
    pub dock_open: bool,
    selftest: Option<selftest::SelfTest>,
}

impl LumenApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let audio_ctx = cc.egui_ctx.clone();
        let engine = Engine::spawn(std::sync::Arc::new(move || ctx.request_repaint()));
        let audio = AudioMonitor::spawn(std::sync::Arc::new(move || audio_ctx.request_repaint()));
        let cfg = Config::load();
        let mut app = Self {
            engine,
            audio,
            audio_devices: crate::audio::input_devices(),
            audio_on: false,
            audio_info: None,
            audio_channels: 0,
            audio_clip: false,
            audio_quiet_since: None,
            meters: Default::default(),
            cfg,
            started: Instant::now(),
            devices: Vec::new(),
            selected_device: 0,
            connection: Connection::Disconnected,
            settings: Vec::new(),
            live: false,
            frame_tex: None,
            frame_size: (0, 0),
            histogram: Vec::new(),
            sharpness: VecDeque::new(),
            sharp_peak: 1.0,
            fps: 0.0,
            last_frame_at: None,
            hand_changed: Default::default(),
            mode_switch: None,
            frames_seen: 0,
            output_sent: None,
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            captures: Vec::new(),
            selected_capture: None,
            capture_pending: false,
            bracket: None,
            last_sequence: Vec::new(),
            stack: StackState::default(),
            focus_hold: None,
            recording: false,
            rec_started: None,
            fullscreen: false,
            last_pointer_move: Instant::now(),
            logs: VecDeque::new(),
            toasts: Vec::new(),
            dock_tab: DockTab::Captures,
            dock_open: true,
            selftest: selftest::SelfTest::from_env(),
        };
        app.push_peaking();
        app.engine.send(Command::Scan);
        app.audio.send(audio::Command::Volume(app.cfg.audio_volume));
        if app.cfg.audio_auto_start {
            app.start_audio();
        }
        app
    }

    // ------------------------------------------------------------ commands

    pub fn is_connected(&self) -> bool {
        matches!(self.connection, Connection::Connected { .. })
    }

    pub fn caps(&self) -> Capabilities {
        match &self.connection {
            Connection::Connected { caps, .. } => *caps,
            _ => Capabilities::default(),
        }
    }

    /// File prefix from the settings, never empty or containing path separators.
    fn file_prefix(&self) -> String {
        let cleaned: String = self.cfg.prefix.trim().chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')).collect();
        if cleaned.is_empty() { "capture".into() } else { cleaned }
    }

    pub fn scan(&self) {
        self.engine.send(Command::Scan);
    }

    pub fn connect_selected(&mut self) {
        if let Some(entry) = self.devices.get(self.selected_device).cloned() {
            self.cfg.last_device = Some(entry.name.clone());
            self.connection = Connection::Connecting;
            self.engine.send(Command::Connect(entry));
        }
    }

    pub fn toggle_live(&self) {
        self.engine.send(Command::LiveView(!self.live));
    }

    pub fn capture(&mut self) {
        if !self.is_connected() || self.capture_pending || self.bracket.is_some() {
            return;
        }
        self.capture_pending = true;
        self.engine.send(Command::Capture { dir: self.cfg.output_dir.clone(), prefix: self.file_prefix() });
    }

    /// Keep the engine's save location in step with the settings.
    fn sync_output(&mut self) {
        let wanted = (self.cfg.output_dir.clone(), self.file_prefix());
        if self.output_sent.as_ref() != Some(&wanted) {
            self.engine.send(Command::SetOutput { dir: wanted.0.clone(), prefix: wanted.1.clone() });
            self.output_sent = Some(wanted);
        }
    }

    /// The monitor is running but the input has delivered silence for a while.
    pub fn audio_silent(&self) -> bool {
        self.audio_on && self.audio_quiet_since.is_some_and(|t| t.elapsed() > Duration::from_secs(4))
    }

    pub fn refresh_audio_devices(&mut self) {
        self.audio_devices = audio::input_devices();
    }

    pub fn start_audio(&mut self) {
        self.audio.send(audio::Command::Start { device: self.cfg.audio_device.clone() });
    }

    pub fn toggle_audio(&mut self) {
        if self.audio_on {
            self.audio.send(audio::Command::Stop);
        } else {
            self.start_audio();
        }
    }

    pub fn set_listen(&mut self, on: bool) {
        self.cfg.audio_listen = on;
        self.audio.send(audio::Command::Listen(on));
    }

    /// Fold audio notices and fresh levels into the UI state.
    fn update_audio(&mut self, ctx: &Context) {
        while let Ok(n) = self.audio.notices.try_recv() {
            match n {
                audio::Notice::Started { device, channels, sample_rate } => {
                    self.audio_on = true;
                    self.audio_channels = channels;
                    self.audio_clip = false;
                    self.audio_quiet_since = Some(Instant::now());
                    self.audio_info = Some(format!("{device} · {channels} ch · {} kHz", sample_rate as f32 / 1000.0));
                    self.log(Level::Success, format!("Audio monitor started on {device}"));
                    if self.cfg.audio_listen {
                        self.audio.send(audio::Command::Listen(true));
                    }
                }
                audio::Notice::Stopped => {
                    self.audio_on = false;
                    self.audio_info = None;
                    self.meters.iter_mut().for_each(meters::Meter::reset);
                }
                audio::Notice::Error(e) => {
                    self.audio_on = false;
                    self.toast(Level::Error, format!("Audio: {e}"));
                    self.log(Level::Error, format!("Audio monitor failed: {e}"));
                }
                audio::Notice::ListenUnavailable(e) => {
                    self.cfg.audio_listen = false;
                    self.toast(Level::Warn, format!("Cannot play the monitor sound: {e}"));
                }
            }
        }
        if self.audio_on {
            let snap = self.audio.levels.take();
            let dt = ctx.input(|i| i.stable_dt).min(0.1);
            for c in 0..2 {
                self.meters[c].feed(snap.peak_db[c], dt);
            }
            self.audio_clip |= snap.clipped;
            let loudest = snap.peak_db[0].max(snap.peak_db[1]);
            if loudest > -75.0 {
                self.audio_quiet_since = None;
            } else if self.audio_quiet_since.is_none() {
                self.audio_quiet_since = Some(Instant::now());
            }
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    pub fn toggle_record(&mut self) {
        if self.is_connected() && self.caps().record {
            self.engine.send(Command::Record(!self.recording));
        }
    }

    pub fn toggle_fullscreen(&mut self, ctx: &Context) {
        self.fullscreen = !self.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
    }

    /// Elapsed recording time, if recording.
    pub fn rec_elapsed(&self) -> Option<Duration> {
        self.rec_started.map(|t| t.elapsed())
    }

    pub fn battery(&self) -> Option<u8> {
        self.settings.iter().find(|s| s.prop == crate::ptp::codes::prop::BATTERY_LEVEL).map(|s| s.current.clamp(0, 100) as u8)
    }

    pub fn drive_focus(&self, dir: FocusDirection) {
        let steps = FOCUS_STEPS[self.cfg.focus_step_index.min(FOCUS_STEPS.len() - 1)].1;
        self.engine.send(Command::Focus { dir, steps });
    }

    pub fn push_peaking(&self) {
        let p = self.cfg.show_peaking.then_some(Peaking { threshold: self.cfg.peaking_threshold, color: [61, 255, 178] });
        self.engine.send(Command::SetPeaking(p));
    }

    pub fn start_bracket(&mut self) {
        if !self.is_connected() || self.bracket.is_some() {
            return;
        }
        let direction = if self.cfg.bracket_far_first { FocusDirection::Near } else { FocusDirection::Far };
        self.bracket = Some((0, self.cfg.bracket_shots));
        self.engine.send(Command::Bracket(BracketPlan {
            shots: self.cfg.bracket_shots,
            step: self.cfg.bracket_step,
            direction,
            settle_ms: self.cfg.settle_ms,
            output_dir: self.cfg.output_dir.clone(),
            prefix: self.file_prefix(),
        }));
    }

    pub fn stack_options(&self) -> StackOptions {
        StackOptions {
            method: if self.cfg.stack_sharpest { Method::Sharpest } else { Method::Weighted },
            smoothing: self.cfg.stack_smoothing,
            ..StackOptions::default()
        }
    }

    pub fn start_stack(&mut self, paths: Vec<PathBuf>) {
        if self.stack.running || paths.len() < 2 {
            return;
        }
        self.stack = StackState { running: true, progress: 0.0, result: None };
        self.engine.send(Command::Stack { paths, options: self.stack_options() });
    }

    pub fn toast(&mut self, level: Level, text: impl Into<String>) {
        self.toasts.push(Toast { level, text: text.into(), born: Instant::now() });
    }

    pub fn open_in_explorer(path: &Path) {
        let arg = if path.is_file() { format!("/select,{}", path.display()) } else { path.display().to_string() };
        let _ = std::process::Command::new("explorer").arg(arg).spawn();
    }

    pub fn open_file(path: &Path) {
        let _ = std::process::Command::new("cmd").args(["/C", "start", ""]).arg(path).spawn();
    }

    // -------------------------------------------------------------- events

    fn drain_events(&mut self, ctx: &Context) {
        let mut latest_frame = None;
        while let Ok(event) = self.engine.events.try_recv() {
            match event {
                Event::Frame(f) => latest_frame = Some(f),
                other => self.apply_event(ctx, other),
            }
        }
        if let Some(frame) = latest_frame {
            self.apply_frame(ctx, frame);
        }
    }

    fn apply_frame(&mut self, ctx: &Context, frame: Frame) {
        let image = ColorImage::from_rgba_unmultiplied([frame.width, frame.height], &frame.rgba);
        match &mut self.frame_tex {
            Some(t) => t.set(image, TextureOptions::LINEAR),
            None => self.frame_tex = Some(ctx.load_texture("live-view", image, TextureOptions::LINEAR)),
        }
        self.frames_seen += 1;
        self.frame_size = (frame.width, frame.height);
        self.histogram = frame.histogram;
        self.sharpness.push_back(frame.sharpness);
        while self.sharpness.len() > 120 {
            self.sharpness.pop_front();
        }
        self.sharp_peak = self.sharpness.iter().copied().fold(1.0, f32::max);
        let now = Instant::now();
        if let Some(prev) = self.last_frame_at {
            let inst = 1.0 / now.duration_since(prev).as_secs_f32().max(1e-3);
            self.fps = if self.fps == 0.0 { inst } else { self.fps * 0.9 + inst * 0.1 };
        }
        self.last_frame_at = Some(now);
    }

    fn texture_from(ctx: &Context, name: &str, t: &Thumb) -> TextureHandle {
        ctx.load_texture(name, ColorImage::from_rgba_unmultiplied([t.width, t.height], &t.rgba), TextureOptions::LINEAR)
    }

    fn apply_event(&mut self, ctx: &Context, event: Event) {
        match event {
            Event::Devices(devices) => {
                let preferred = self.cfg.last_device.as_ref().and_then(|n| devices.iter().position(|d| &d.name == n));
                let current = self.devices.get(self.selected_device).and_then(|c| devices.iter().position(|d| d == c));
                self.selected_device = current.or(preferred).unwrap_or(0);
                self.devices = devices;
            }
            Event::Connected { identity, caps, settings } => {
                self.toast(Level::Success, format!("Connected to {}", identity.title()));
                self.connection = Connection::Connected { identity, caps };
                self.settings = settings;
                if caps.live_view {
                    self.engine.send(Command::LiveView(true));
                }
            }
            Event::Disconnected { reason } => {
                self.connection = Connection::Disconnected;
                self.settings.clear();
                self.live = false;
                self.capture_pending = false;
                self.bracket = None;
                self.recording = false;
                self.rec_started = None;
                if let Some(r) = reason {
                    self.toast(Level::Error, r);
                }
            }
            Event::Settings(s) => self.settings = s,
            Event::CameraChanged(changes) => {
                let now = Instant::now();
                for c in &changes {
                    self.hand_changed.insert(c.prop, now);
                }
                let text: Vec<String> = changes.iter().map(|c| format!("{} {}", c.label, c.to)).collect();
                self.toast(Level::Info, format!("Camera: {}", text.join(" · ")));
            }
            Event::Capabilities(new) => {
                if let Connection::Connected { caps, .. } = &mut self.connection {
                    *caps = new;
                }
                self.toast(Level::Info, if new.record { "Camera is in video mode" } else { "Camera is in photo mode" });
            }
            Event::ModeSwitching(to) => {
                self.mode_switch = to.map(|video| (video, Instant::now()));
                if to.is_some() {
                    self.frame_tex = None; // the old mode's last picture must not linger under the overlay
                }
            }
            Event::LiveView(on) => {
                self.live = on;
                if !on {
                    self.last_frame_at = None;
                    self.fps = 0.0;
                }
            }
            Event::Recording(on) => {
                self.recording = on;
                self.rec_started = on.then(Instant::now);
                self.toast(if on { Level::Warn } else { Level::Info }, if on { "Recording started" } else { "Recording stopped" });
            }
            Event::Frame(_) => unreachable!("frames are coalesced in drain_events"),
            Event::Captured(rec) => {
                self.capture_pending = false;
                let texture = rec.thumb.as_ref().map(|t| Self::texture_from(ctx, &rec.label, t));
                self.captures.push(CaptureItem { label: rec.label, path: rec.path, texture });
                self.selected_capture = Some(self.captures.len() - 1);
            }
            Event::BracketProgress { done, total } => self.bracket = Some((done, total)),
            Event::BracketFinished { folder, paths, cancelled } => {
                self.bracket = None;
                self.capture_pending = false;
                if paths.len() >= 2 {
                    self.last_sequence = paths.clone();
                }
                if cancelled {
                    self.toast(Level::Warn, format!("Bracket stopped after {} shots", paths.len()));
                } else {
                    let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.toast(Level::Success, format!("Bracket complete: {} images in {name}", paths.len()));
                    if self.cfg.auto_stack {
                        self.start_stack(paths);
                    }
                }
            }
            Event::StackProgress(p) => self.stack.progress = p,
            Event::StackFinished { path, preview } => {
                let texture = preview.as_ref().map(|t| Self::texture_from(ctx, "stack-result", t));
                self.stack = StackState { running: false, progress: 1.0, result: Some((path.clone(), texture)) };
                self.toast(Level::Success, format!("Stack saved: {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
                self.log(Level::Success, format!("Stack saved to {}", path.display()));
            }
            Event::StackFailed(msg) => {
                self.stack.running = false;
                self.toast(Level::Error, format!("Stacking failed: {msg}"));
                self.log(Level::Error, format!("Stacking failed: {msg}"));
            }
            Event::Log(level, text) => {
                if matches!(level, Level::Error | Level::Warn) {
                    self.capture_pending = false;
                    self.toast(level, text.clone());
                }
                self.log(level, text);
            }
        }
    }

    fn log(&mut self, level: Level, text: String) {
        self.logs.push_back(LogLine { level, at: self.started.elapsed(), text });
        while self.logs.len() > 500 {
            self.logs.pop_front();
        }
    }

    // ------------------------------------------------------------ shortcuts

    fn shortcuts(&mut self, ctx: &Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        use egui::Key;
        let (space, l, a, left, right, g, p, r, f, esc, info) = ctx.input(|i| {
            (
                i.key_pressed(Key::Space),
                i.key_pressed(Key::L),
                i.key_pressed(Key::A),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::G),
                i.key_pressed(Key::P),
                i.key_pressed(Key::R),
                i.key_pressed(Key::F) || i.key_pressed(Key::F11),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::I),
            )
        });
        if r {
            self.toggle_record();
        }
        if f || (esc && self.fullscreen) {
            self.toggle_fullscreen(ctx);
        }
        if info {
            self.cfg.show_osd = !self.cfg.show_osd;
        }
        if space {
            self.capture();
        }
        if l && self.is_connected() {
            self.toggle_live();
        }
        if a && self.caps().autofocus {
            self.engine.send(Command::Autofocus);
        }
        if left && self.caps().focus_drive {
            self.drive_focus(FocusDirection::Near);
        }
        if right && self.caps().focus_drive {
            self.drive_focus(FocusDirection::Far);
        }
        if g {
            self.cfg.show_grid = !self.cfg.show_grid;
        }
        if p {
            self.cfg.show_peaking = !self.cfg.show_peaking;
            self.push_peaking();
        }
    }

    fn drive_held_focus(&mut self, ctx: &Context) {
        if let Some((dir, due)) = self.focus_hold {
            if Instant::now() >= due {
                self.drive_focus(dir);
                self.focus_hold = Some((dir, Instant::now() + Duration::from_millis(140)));
            }
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }
}

impl eframe::App for LumenApp {
    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.drain_events(ctx);
        self.shortcuts(ctx);
        self.drive_held_focus(ctx);
        self.update_audio(ctx);
        self.sync_output();
        self.selftest_tick(ctx);
        self.toasts.retain(|t| t.born.elapsed() < Duration::from_secs(4));
        if !self.toasts.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Release the held focus button once the pointer is up.
        if !ui.input(|i| i.pointer.any_down()) {
            self.focus_hold = None;
        }
        if ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_down()) {
            self.last_pointer_move = Instant::now();
        }
        if self.fullscreen {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| viewer::show_fullscreen(self, ui));
        } else {
            egui::Panel::top("header")
                .exact_size(58.0)
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(18, 10)))
                .show(ui, |ui| header::show(self, ui));

            let dock_height = if self.dock_open { 190.0 } else { 40.0 };
            egui::Panel::bottom("dock")
                .exact_size(dock_height)
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin { left: 18, right: 18, top: 0, bottom: 14 }))
                .show(ui, |ui| dock::show(self, ui));

            egui::Panel::left("camera")
                .exact_size(318.0)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin { left: 18, right: 9, top: 4, bottom: 10 }))
                .show(ui, |ui| camera_panel::show(self, ui));

            egui::Panel::right("tools")
                .exact_size(336.0)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin { left: 9, right: 18, top: 4, bottom: 10 }))
                .show(ui, |ui| tools_panel::show(self, ui));

            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin { left: 9, right: 9, top: 4, bottom: 10 }))
                .show(ui, |ui| viewer::show(self, ui));

        }

        dock::toasts(self, ui.ctx());
    }

    fn on_exit(&mut self) {
        self.cfg.save();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Color32::from_rgb(11, 13, 17).to_normalized_gamma_f32()
    }
}

#[cfg(test)]
mod clock_tests {
    use super::format_clock;
    use std::time::Duration;

    #[test]
    fn formats_minutes_and_hours() {
        assert_eq!(format_clock(Duration::from_secs(5)), "00:05");
        assert_eq!(format_clock(Duration::from_secs(754)), "12:34");
        assert_eq!(format_clock(Duration::from_secs(3 * 3600 + 61)), "03:01:01");
    }
}
