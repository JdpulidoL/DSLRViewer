//! The camera thread: a single-owner event loop.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use super::files::{next_free_dir, next_free_path, save_bytes};
use super::{BracketPlan, CaptureRecord, Change, Command, Event, Level, Thumb};
use crate::camera::{Camera, Capabilities, CaptureOutcome, LiveImage, Setting, Shot};
use crate::ptp::codes::prop;
use crate::imaging::stack::{self, Options as StackOptions};
use crate::imaging::{self, Peaking};
use crate::link::{self, DeviceEntry};
use crate::ptp::PtpError;

const FRAME_INTERVAL: Duration = Duration::from_millis(12);
const SETTLE_AFTER_CONNECT: Duration = Duration::from_secs(4);
const STALL_RESET_AFTER: Duration = Duration::from_secs(8);
const SWITCH_COOLDOWN: Duration = Duration::from_secs(8);
const STALL_AFTER: Duration = Duration::from_millis(1500);
const RETRY_INTERVAL: Duration = Duration::from_millis(4);
const SETTINGS_POLL: Duration = Duration::from_millis(400);
const IDLE_POLL: Duration = Duration::from_millis(40);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);
const THUMB_EDGE: u32 = 220;
const PREVIEW_EDGE: u32 = 1400;

pub fn run(rx: Receiver<Command>, tx: Sender<Event>, wake: Arc<dyn Fn() + Send + Sync>) {
    #[cfg(windows)]
    let _com = crate::link::wpd::ComGuard::new();
    Worker { camera: None, rx, tx, wake, pending: VecDeque::new(), peaking: None, last_frame: Instant::now(), last_poll: Instant::now(), recording: false, last_image: None, output: None, last_warn: None, known: RefCell::new(Vec::new()), caps: None, last_good: Instant::now(), last_revive: Instant::now(), last_kind: None, connected_at: Instant::now(), rec_changed_at: Instant::now() - Duration::from_secs(60), pending_check: false, want_live: false, last_switch: Instant::now() - SWITCH_COOLDOWN }.event_loop();
}

struct Worker {
    camera: Option<Camera>,
    rx: Receiver<Command>,
    tx: Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
    /// Commands received while a long operation was running.
    pending: VecDeque<Command>,
    peaking: Option<Peaking>,
    last_frame: Instant,
    last_poll: Instant,
    recording: bool,
    /// Most recent live-view JPEG; used for stills while the camera is in video mode.
    last_image: Option<LiveImage>,
    /// Where pictures taken with the camera's own shutter button are saved.
    output: Option<(PathBuf, String)>,
    last_warn: Option<Instant>,
    /// Settings as last shown to the UI, to tell hand changes on the body from our own writes.
    known: RefCell<Vec<Setting>>,
    last_good: Instant,
    last_revive: Instant,
    /// Kind of the last live-view frame (true = raw video frame), to notice dial changes.
    last_kind: Option<bool>,
    connected_at: Instant,
    rec_changed_at: Instant,
    /// A dial move was suspected during the cooldown; check again once it ends.
    pending_check: bool,
    /// The UI asked for live view; used to bring it back if the body drops it.
    want_live: bool,
    last_switch: Instant,
    caps: Option<Capabilities>,
}

/// Settings whose value differs from `before` (battery drift is not a user action).
fn diff_settings(before: &[Setting], after: &[Setting]) -> Vec<Change> {
    after
        .iter()
        .filter(|s| s.prop != prop::BATTERY_LEVEL)
        .filter_map(|new| {
            let old = before.iter().find(|o| o.prop == new.prop)?;
            (old.current != new.current).then(|| Change { prop: new.prop, label: new.label, from: old.text.clone(), to: new.text.clone() })
        })
        .collect()
}

impl Worker {
    fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
        (self.wake)();
    }

    fn log(&self, level: Level, msg: impl Into<String>) {
        let msg = msg.into();
        crate::logfile::line(match level { Level::Info => "info", Level::Success => "ok", Level::Warn => "warn", Level::Error => "error" }, &msg);
        self.emit(Event::Log(level, msg));
    }

    /// Warn about a recurring problem at most once every few seconds.
    fn log_throttled(&mut self, msg: String) {
        if self.last_warn.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
            self.last_warn = Some(Instant::now());
            self.log(Level::Warn, msg);
        }
    }

    fn live(&self) -> bool {
        self.camera.as_ref().is_some_and(Camera::live_view_active)
    }

    fn event_loop(mut self) {
        loop {
            let next = self.pending.pop_front().map(Ok).unwrap_or_else(|| {
                let wait = if self.live() { Duration::ZERO } else { IDLE_POLL };
                self.rx.recv_timeout(wait)
            });
            match next {
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if self.live() {
                self.pump_live_frame();
            }
            self.poll_camera_settings();
        }
        self.drop_camera(None);
    }

    /// Tell the UI when the camera starts or stops recording (including via its own button).
    fn sync_recording(&mut self) {
        let now = self.camera.as_ref().is_some_and(Camera::recording);
        if now != self.recording {
            self.recording = now;
            self.rec_changed_at = Instant::now();
            self.log(if now { Level::Warn } else { Level::Info }, if now { "Recording started" } else { "Recording stopped" });
            self.emit(Event::Recording(now));
        }
    }

    /// Pick up settings the user changed on the camera body itself.
    fn poll_camera_settings(&mut self) {
        if self.last_poll.elapsed() < SETTINGS_POLL {
            return;
        }
        self.last_poll = Instant::now();
        if let Some(settings) = self.camera.as_mut().and_then(Camera::poll_settings) {
            // The body completes its property dump shortly after connecting; that is not a hand change.
            let changes = if self.connected_at.elapsed() < SETTLE_AFTER_CONNECT { Vec::new() } else { diff_settings(&self.known.borrow(), &settings) };
            self.known.replace(settings.clone());
            self.emit(Event::Settings(settings));
            if !changes.is_empty() {
                let text: Vec<String> = changes.iter().map(|c| format!("{} {} → {}", c.label, c.from, c.to)).collect();
                self.log(Level::Info, format!("Changed on the camera: {}", text.join(", ")));
                self.emit(Event::CameraChanged(changes));
            }
        }
        self.sync_mode();
        self.resume_live_view();
        self.sync_recording();
        self.collect_body_shots();
    }

    /// The body announces a mode-dial move with a whole-state property dump. Only a fresh handshake tells
    /// reliably which position it is in (the record status reads 3 in both modes once live view runs), so a
    /// dump - or a change of the record capability - triggers a verified re-initialisation.
    fn sync_mode(&mut self) {
        let Some(cam) = self.camera.as_mut() else { return };
        let dump = cam.take_big_dump();
        let caps = cam.refresh_capabilities();
        let settled = self.connected_at.elapsed() > SETTLE_AFTER_CONNECT;
        if self.caps.is_none() || !settled {
            if self.caps != Some(caps) {
                self.caps = Some(caps);
                self.emit(Event::Capabilities(caps));
            }
            return;
        }
        let own_change = self.rec_changed_at.elapsed() < Duration::from_secs(5);
        let wanted = (dump && !own_change) || self.caps != Some(caps) || self.pending_check;
        if !wanted || self.recording {
            return;
        }
        if self.last_switch.elapsed() < SWITCH_COOLDOWN {
            crate::logfile::line("mode", "dial move suspected but a switch just ran; waiting");
            if dump {
                self.pending_check = true;
            }
            return;
        }
        self.pending_check = false;
        self.switch_mode();
    }

    /// The dial moved between photo and video: show the loading screen while the camera is re-initialised,
    /// then publish the settings and abilities of the new mode.
    fn switch_mode(&mut self) {
        self.last_switch = Instant::now();
        let before = self.caps;
        crate::logfile::line("mode", "re-initialising the camera to read the mode dial");
        self.emit(Event::ModeSwitching(Some(false)));
        let result = self.camera.as_mut().map(Camera::reinit_remote);
        if let Some(Err(e)) = result {
            self.report("Switching camera mode", e);
        }
        if let Some(cam) = self.camera.as_mut() {
            let caps = cam.refresh_capabilities();
            let settings = cam.read_settings();
            let live = cam.live_view_active();
            self.caps = Some(caps);
            self.known.replace(settings.clone());
            self.emit(Event::Settings(settings));
            self.emit(Event::Capabilities(caps));
            self.emit(Event::LiveView(live));
            let moved = before.is_some_and(|b| b.record != caps.record);
            self.log(Level::Info, format!("Camera is in {} mode{}", if caps.record { "video" } else { "photo" }, if moved { " (dial moved)" } else { "" }));
        }
        self.last_good = Instant::now();
        self.last_switch = Instant::now();
        self.emit(Event::ModeSwitching(None));
        crate::logfile::line("mode", "switch finished");
    }
    /// The user wants live view but the body dropped it (e.g. after a mode change): start it again, quietly.
    fn resume_live_view(&mut self) {
        if !self.want_live || self.last_revive.elapsed() < Duration::from_secs(3) {
            return;
        }
        let Some(cam) = self.camera.as_mut().filter(|c| !c.live_view_active()) else { return };
        self.last_revive = Instant::now();
        let result = cam.start_live_view();
        crate::logfile::line("live", format!("resuming live view: {result:?}"));
        if result.is_ok() {
            self.last_good = Instant::now();
            self.emit(Event::LiveView(true));
        }
    }

    /// Pictures taken with the shutter button on the camera itself arrive as events; download and save them.
    fn collect_body_shots(&mut self) {
        let Some((dir, prefix)) = self.output.clone() else { return };
        let Some(cam) = self.camera.as_mut() else { return };
        match cam.take_new_images() {
            Ok(shots) if !shots.is_empty() => {
                if let Some(record) = self.save_shots(shots, &dir, &prefix) {
                    self.log(Level::Success, format!("Photo taken on the camera: {}", record.label));
                    self.emit(Event::Captured(record));
                }
            }
            Ok(_) => {}
            Err(e) => self.report("Receiving a photo from the camera", e),
        }
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Scan => {
                let devices = link::scan();
                self.emit(Event::Devices(devices));
            }
            Command::Connect(entry) => self.connect(entry),
            Command::Disconnect => self.drop_camera(None),
            Command::SetSetting { prop, value } => self.with_camera("Changing setting", |w, cam| {
                cam.set_setting(prop, value)?;
                let settings = cam.read_settings();
                w.known.replace(settings.clone());
                w.emit(Event::Settings(settings));
                Ok(())
            }),
            Command::LiveView(on) => {
                self.want_live = on;
                self.set_live_view(on);
            }
            Command::SetPeaking(p) => self.peaking = p,
            Command::SetOutput { dir, prefix } => self.output = Some((dir, prefix)),
            Command::Capture { dir, prefix } => self.capture_single(&dir, &prefix),
            Command::Focus { dir, steps } => self.with_camera("Focus drive", |_, cam| cam.focus_drive(dir, steps)),
            Command::Record(on) => {
                self.with_camera("Video recording", |_, cam| cam.set_recording(on));
                self.sync_recording();
            }
            Command::Autofocus => self.with_camera("Autofocus", |w, cam| {
                cam.autofocus()?;
                w.log(Level::Info, "Autofocus triggered");
                Ok(())
            }),
            Command::Bracket(plan) => self.bracket(plan),
            Command::CancelBracket => {} // only meaningful while a bracket runs
            Command::Stack { paths, options } => self.spawn_stack(paths, options),
            Command::Shutdown => unreachable!("handled by the event loop"),
        }
    }

    // ------------------------------------------------------------ connection

    fn connect(&mut self, entry: DeviceEntry) {
        self.drop_camera(None);
        self.log(Level::Info, format!("Connecting to {}…", entry.name));
        let mut entry = entry;
        let mut result = link::open(&entry).and_then(Camera::open);
        // A camera that was power-cycled or replugged gets a new WPD device id, so the listed one is
        // gone ("file not found"). Rescan and retry with the entry that now carries the same name.
        #[cfg(windows)]
        let is_wpd = matches!(entry.kind, link::DeviceKind::Wpd { .. });
        #[cfg(not(windows))]
        let is_wpd = false;
        if result.is_err() && is_wpd {
            let devices = link::scan();
            let fresh = devices.iter().find(|d| d.name == entry.name && d.kind != entry.kind).cloned();
            self.emit(Event::Devices(devices));
            if let Some(fresh) = fresh {
                self.log(Level::Info, "The camera was re-enumerated by Windows; retrying with its new address");
                entry = fresh;
                result = link::open(&entry).and_then(Camera::open);
            }
        }
        match result {
            Ok(mut camera) => {
                let identity = camera.identity();
                let caps = camera.capabilities();
                let settings = camera.read_settings();
                self.log(
                    Level::Success,
                    format!("Connected to {} through {} ({})", identity.title(), identity.transport, identity.vendor.label()),
                );
                self.camera = Some(camera);
                self.known.replace(settings.clone());
                self.caps = Some(caps);
                self.connected_at = Instant::now();
                self.emit(Event::Connected { identity, caps, settings });
            }
            Err(e) => {
                let hint = if e.to_string().contains("0x80070002") { " Turn the camera off and on (or replug the USB cable), then press Scan." } else { "" };
                self.log(Level::Error, format!("Could not connect to {}: {e}.{hint}", entry.name));
            }
        }
    }

    fn drop_camera(&mut self, reason: Option<String>) {
        if let Some(mut cam) = self.camera.take() {
            self.known.get_mut().clear();
            self.last_kind = None;
            self.caps = None;
            if reason.is_none() {
                cam.close();
            }
            self.emit(Event::LiveView(false));
            if std::mem::take(&mut self.recording) {
                self.emit(Event::Recording(false));
            }
            self.emit(Event::Disconnected { reason });
        }
    }

    /// Run `f` against the connected camera, turning errors into log lines and
    /// dropping the camera if the device vanished.
    fn with_camera(&mut self, what: &str, f: impl FnOnce(&Self, &mut Camera) -> Result<(), PtpError>) {
        let Some(mut cam) = self.camera.take() else {
            self.log(Level::Warn, format!("{what}: no camera connected"));
            return;
        };
        let result = f(self, &mut cam);
        self.camera = Some(cam);
        if let Err(e) = result {
            self.report(what, e);
        }
    }

    fn report(&mut self, what: &str, e: PtpError) {
        if matches!(e, PtpError::Disconnected) {
            self.log(Level::Error, "Camera disconnected");
            self.drop_camera(Some("The camera was unplugged or stopped responding".into()));
        } else {
            self.log(Level::Error, format!("{what} failed: {e}"));
        }
    }

    // -------------------------------------------------------------- live view

    fn set_live_view(&mut self, on: bool) {
        let Some(cam) = self.camera.as_mut() else {
            self.log(Level::Warn, "Live view: no camera connected");
            return;
        };
        let result = if on { cam.start_live_view() } else { cam.stop_live_view() };
        let active = cam.live_view_active();
        match result {
            Ok(()) => self.emit(Event::LiveView(active)),
            Err(e) => self.report("Live view", e),
        }
    }

    fn pump_live_frame(&mut self) {
        let since = self.last_frame.elapsed();
        if since < FRAME_INTERVAL {
            std::thread::sleep(FRAME_INTERVAL - since);
        }
        self.last_frame = Instant::now();
        let Some(cam) = self.camera.as_mut() else { return };
        match cam.live_view_frame() {
            Ok(Some(image)) => {
                // Photo mode serves JPEG frames, video mode raw YUV: a change of kind means the dial moved.
                let video = matches!(image, LiveImage::Rgb { .. });
                if self.last_kind != Some(video) {
                    crate::logfile::line("live", format!("frame kind is now {}", if video { "raw video" } else { "jpeg" }));
                    self.last_kind = Some(video);
                }
                match imaging::decode_live(&image, self.peaking) {
                    Ok(frame) => self.emit(Event::Frame(frame)),
                    Err(e) => self.log_throttled(format!("Dropped a live view frame: {e:#}")),
                }
                self.last_image = Some(image);
                self.last_good = Instant::now();
            }
            // "No frame yet" (every other call while recording): ask again soon instead of waiting a full interval.
            Ok(None) => {
                self.last_frame = Instant::now() - (FRAME_INTERVAL - RETRY_INTERVAL);
                self.revive_if_stalled();
            }
            Err(e) => {
                let _ = self.camera.as_mut().map(|c| c.stop_live_view());
                self.emit(Event::LiveView(false));
                self.report("Live view", e);
            }
        }
    }

    /// Live view is on but no picture arrives. Only logged: acting on it (re-initialising) looped when the body
    /// legitimately answers "not ready" for a while, and real mode changes are caught from the property events.
    fn revive_if_stalled(&mut self) {
        if self.last_good.elapsed() < STALL_AFTER || self.last_revive.elapsed() < STALL_AFTER {
            return;
        }
        self.last_revive = Instant::now();
        let silent = self.last_good.elapsed();
        crate::logfile::line("live", format!("no live view frame for {:.1} s", silent.as_secs_f32()));
        // A long silence (the body took the viewfinder away after a mode change we could not see) is
        // cured by redoing the handshake; the cooldown keeps this from looping.
        let settled = self.connected_at.elapsed() > STALL_RESET_AFTER && self.last_switch.elapsed() > STALL_RESET_AFTER;
        if silent >= STALL_RESET_AFTER && settled && !self.recording {
            self.switch_mode();
        }
    }

    // ---------------------------------------------------------------- capture

    fn capture_once(&mut self, dir: &Path, prefix: &str) -> Result<Option<CaptureRecord>, PtpError> {
        let cam = self.camera.as_mut().ok_or(PtpError::Disconnected)?;
        if cam.video_mode() {
            return self.live_snapshot(dir, prefix);
        }
        match cam.capture(CAPTURE_TIMEOUT)? {
            CaptureOutcome::StoredOnCamera => Ok(Some(CaptureRecord { path: None, label: "Stored on camera card".into(), thumb: None })),
            CaptureOutcome::Downloaded(shots) => Ok(self.save_shots(shots, dir, prefix)),
        }
    }

    /// Write downloaded images to disk. A RAW with no ordinary decoder also gets its embedded
    /// full-size JPEG preview saved next to it, so it can be viewed, shown in the gallery and stacked.
    /// Returns the record to show: the viewable file when there is one.
    fn save_shots(&mut self, shots: Vec<Shot>, dir: &Path, prefix: &str) -> Option<CaptureRecord> {
        let mut record: Option<CaptureRecord> = None;
        for shot in shots {
            let path = match save_bytes(dir, prefix, &shot.name, &shot.bytes) {
                Ok(p) => p,
                Err(e) => {
                    self.log(Level::Error, format!("Could not save {}: {e:#}", shot.name));
                    continue;
                }
            };
            let mut shown = (path.clone(), shot.bytes.as_slice());
            let preview = imaging::raw::embedded_jpeg(&shot.bytes).filter(|_| imaging::thumbnail(&shot.bytes, 8).is_err());
            if let Some(jpeg) = preview {
                let jpg_path = path.with_extension("jpg");
                if !jpg_path.exists() && std::fs::write(&jpg_path, jpeg).is_ok() {
                    shown = (jpg_path, jpeg);
                }
            }
            let thumb = imaging::thumbnail(shown.1, THUMB_EDGE).ok().map(|(width, height, rgba)| Thumb { width, height, rgba });
            let label = shown.0.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // Prefer the viewable image as the record when several files came back.
            if record.is_none() || thumb.is_some() && record.as_ref().is_some_and(|r| r.thumb.is_none()) {
                record = Some(CaptureRecord { path: Some(shown.0), label, thumb });
            }
        }
        record
    }

    /// Video mode cannot take still photos; save the current live-view frame instead.
    fn live_snapshot(&mut self, dir: &Path, prefix: &str) -> Result<Option<CaptureRecord>, PtpError> {
        let Some(image) = self.last_image.clone() else {
            return Err(PtpError::Protocol("the camera is in video mode: start live view so a snapshot can be saved".into()));
        };
        let jpeg = match image {
            LiveImage::Jpeg(bytes) => bytes,
            LiveImage::Rgb { width, height, pixels } => {
                let mut out = Vec::new();
                let encoded = image::RgbImage::from_raw(width as u32, height as u32, pixels)
                    .map(|img| img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg));
                if !matches!(encoded, Some(Ok(()))) {
                    return Err(PtpError::Protocol("could not encode the live view snapshot".into()));
                }
                out
            }
        };
        match save_bytes(dir, prefix, "snapshot.jpg", &jpeg) {
            Ok(path) => {
                let thumb = imaging::thumbnail(&jpeg, THUMB_EDGE).ok().map(|(width, height, rgba)| Thumb { width, height, rgba });
                let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.log(Level::Info, "Video mode: saved a snapshot of the live view (the camera cannot take stills in this mode)");
                Ok(Some(CaptureRecord { path: Some(path), label, thumb }))
            }
            Err(e) => {
                self.log(Level::Error, format!("Could not save the snapshot: {e:#}"));
                Ok(None)
            }
        }
    }

    fn capture_single(&mut self, dir: &Path, prefix: &str) {
        if self.camera.is_none() {
            self.log(Level::Warn, "Capture: no camera connected");
            return;
        }
        match self.capture_once(dir, prefix) {
            Ok(Some(record)) => {
                self.log(Level::Success, format!("Captured {}", record.label));
                self.emit(Event::Captured(record));
            }
            Ok(None) => {}
            Err(e) => self.report("Capture", e),
        }
    }

    // --------------------------------------------------------------- bracket

    /// True if the user asked to cancel; other commands are kept for later.
    fn cancel_requested(&mut self) -> bool {
        let mut cancel = false;
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                Command::CancelBracket => cancel = true,
                Command::Shutdown => {
                    self.pending.push_back(Command::Shutdown);
                    cancel = true;
                }
                other => self.pending.push_back(other),
            }
        }
        cancel
    }

    fn bracket(&mut self, plan: BracketPlan) {
        if self.camera.is_none() {
            self.log(Level::Warn, "Focus bracketing: no camera connected");
            return;
        }
        let caps = self.camera.as_ref().map(Camera::capabilities).unwrap_or_default();
        if !caps.focus_drive {
            self.log(Level::Error, "Focus bracketing needs a camera that supports manual focus drive over USB");
            self.emit(Event::BracketFinished { folder: plan.output_dir.clone(), paths: Vec::new(), cancelled: true });
            return;
        }
        let folder = next_free_dir(&plan.output_dir, &plan.prefix);
        self.log(Level::Info, format!("Focus bracket: {} shots, step {} → {}", plan.shots, plan.step, folder.display()));
        let mut paths = Vec::new();
        let mut cancelled = false;
        for i in 0..plan.shots {
            if self.cancel_requested() {
                cancelled = true;
                break;
            }
            match self.capture_once(&folder, &plan.prefix) {
                Ok(Some(record)) => {
                    if let Some(p) = record.path.clone() {
                        if is_stackable(&p) {
                            paths.push(p);
                        } else {
                            self.log(Level::Warn, format!("{} is not a stackable image format; set the camera to JPEG to stack", p.display()));
                        }
                    }
                    self.emit(Event::Captured(record));
                }
                Ok(None) => {
                    cancelled = true;
                    break;
                }
                Err(e) => {
                    self.report("Focus bracket capture", e);
                    cancelled = true;
                    break;
                }
            }
            self.emit(Event::BracketProgress { done: i + 1, total: plan.shots });
            if i + 1 < plan.shots {
                let moved = self.camera.as_mut().map(|c| c.focus_drive(plan.direction, plan.step));
                if let Some(Err(e)) = moved {
                    self.report("Focus drive", e);
                    cancelled = true;
                    break;
                }
                self.settle(Duration::from_millis(plan.settle_ms));
            }
        }
        let verb = if cancelled { "stopped" } else { "finished" };
        self.log(if cancelled { Level::Warn } else { Level::Success }, format!("Focus bracket {verb}: {} images saved", paths.len()));
        self.emit(Event::BracketFinished { folder, paths, cancelled });
    }

    /// Wait for the lens to settle while keeping the live view moving.
    fn settle(&mut self, wait: Duration) {
        let until = Instant::now() + wait;
        while Instant::now() < until {
            if self.live() {
                self.pump_live_frame();
            } else {
                std::thread::sleep(Duration::from_millis(10).min(until - Instant::now()));
            }
        }
    }

    // ----------------------------------------------------------------- stack

    fn spawn_stack(&self, paths: Vec<PathBuf>, options: StackOptions) {
        let (tx, wake) = (self.tx.clone(), self.wake.clone());
        self.log(Level::Info, format!("Stacking {} images…", paths.len()));
        let spawned = std::thread::Builder::new().name("focus-stack".into()).spawn(move || {
            let send = |e: Event| {
                let _ = tx.send(e);
                wake();
            };
            match run_stack(&paths, options, &|p| send(Event::StackProgress(p))) {
                Ok((path, preview)) => send(Event::StackFinished { path, preview }),
                Err(e) => send(Event::StackFailed(format!("{e:#}"))),
            }
        });
        if let Err(e) = spawned {
            self.emit(Event::StackFailed(e.to_string()));
        }
    }
}

/// Formats the stacker can decode.
fn is_stackable(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png" | "tif" | "tiff"))
}

fn run_stack(paths: &[PathBuf], options: StackOptions, progress: &(dyn Fn(f32) + Sync)) -> anyhow::Result<(PathBuf, Option<Thumb>)> {
    let frames = stack::load_sequence(paths)?;
    let merged = stack::merge(&frames, options, progress)?;
    let dir = paths.first().and_then(|p| p.parent()).unwrap_or_else(|| Path::new("."));
    let out = next_free_path(dir, "stack", "jpg");
    merged.save_with_format(&out, image::ImageFormat::Jpeg)?;
    let preview = image::DynamicImage::ImageRgb8(merged).thumbnail(PREVIEW_EDGE, PREVIEW_EDGE).to_rgba8();
    let preview = Thumb { width: preview.width() as usize, height: preview.height() as usize, rgba: preview.into_raw() };
    Ok((out, Some(preview)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(prop: u16, current: i64, text: &str) -> Setting {
        Setting { prop, label: "X", writable: true, current, text: text.into(), choices: Vec::new() }
    }

    #[test]
    fn hand_changes_are_reported_but_battery_drift_is_not() {
        let before = [setting(prop::EXPOSURE_INDEX, 100, "100"), setting(prop::BATTERY_LEVEL, 100, "100%")];
        let after = [setting(prop::EXPOSURE_INDEX, 400, "400"), setting(prop::BATTERY_LEVEL, 50, "50%")];
        let changes = diff_settings(&before, &after);
        assert_eq!(changes.len(), 1);
        assert_eq!((changes[0].from.as_str(), changes[0].to.as_str()), ("100", "400"));
        assert!(diff_settings(&after, &after).is_empty());
    }
}