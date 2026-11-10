//! Kotlin-facing API. The Android UI never touches the camera: it sends calls into a [`LumenSession`]
//! (which forwards them to the engine thread) and receives results through a [`LumenListener`].
//! Same split as the desktop app (CLAUDE.md rule 1), only the UI is native Compose instead of egui.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use lumen_core::camera::{Capabilities, FocusDirection};
use lumen_core::config;
use lumen_core::engine::{Command, Engine, Event, Level as CoreLevel};
use lumen_core::link::{DeviceEntry, DeviceKind};
use lumen_core::logfile;
use lumen_core::ptp::PtpError;
use lumen_core::ptp::transport::WireLink;

uniffi::setup_scaffolding!();

#[derive(uniffi::Enum)]
pub enum LogLevel {
    Info,
    Success,
    Warn,
    Error,
}

#[derive(uniffi::Record)]
pub struct DeviceInfo {
    pub name: String,
    pub detail: String,
    pub simulated: bool,
}

#[derive(uniffi::Record)]
pub struct CameraInfo {
    pub manufacturer: String,
    pub model: String,
    pub serial: String,
    pub transport: String,
}

#[derive(uniffi::Record)]
pub struct CameraCaps {
    pub live_view: bool,
    pub focus_drive: bool,
    pub autofocus: bool,
    pub record: bool,
}

#[derive(uniffi::Record)]
pub struct Choice {
    pub value: i64,
    pub text: String,
}

#[derive(uniffi::Record)]
pub struct CameraSetting {
    pub prop: u16,
    pub label: String,
    pub writable: bool,
    pub current: i64,
    pub text: String,
    pub choices: Vec<Choice>,
}

#[derive(uniffi::Record)]
pub struct LiveFrame {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA8, ready for `Bitmap.copyPixelsFromBuffer`.
    pub rgba: Vec<u8>,
    /// R, G, B, luma; 256 bins each.
    pub histogram: Vec<u32>,
    pub sharpness: f32,
}

#[derive(Debug, uniffi::Error)]
pub enum UsbError {
    Timeout,
    Disconnected,
    Failed,
}

impl std::fmt::Display for UsbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Raw USB pipe implemented in Kotlin with `UsbDeviceConnection.bulkTransfer`.
/// libusb cannot drive the camera through an Android-provided descriptor (every transfer timed out on a
/// Canon T5i), while the platform API answers instantly, so Kotlin owns the I/O and Rust only frames PTP.
#[uniffi::export(with_foreign)]
pub trait UsbTransport: Send + Sync {
    /// Send all of `data` to the bulk-out endpoint (Kotlin adds the zero-length packet when needed).
    fn write(&self, data: Vec<u8>) -> Result<(), UsbError>;
    /// Next chunk from bulk-in; `Timeout` when nothing arrived within `timeout_ms`.
    fn read(&self, timeout_ms: u32) -> Result<Vec<u8>, UsbError>;
    /// Next event from the interrupt endpoint; empty when none arrived.
    fn read_event(&self, timeout_ms: u32) -> Result<Vec<u8>, UsbError>;
    fn describe(&self) -> String;
}

struct KotlinLink(Arc<dyn UsbTransport>);

fn wire_err(e: UsbError) -> PtpError {
    logfile::line("usb", format!("transfer error: {e}"));
    match e {
        UsbError::Timeout => PtpError::Timeout,
        UsbError::Disconnected => PtpError::Disconnected,
        UsbError::Failed => PtpError::Transport("USB transfer failed".into()),
    }
}

impl WireLink for KotlinLink {
    fn write(&mut self, data: &[u8]) -> Result<(), PtpError> {
        self.0.write(data.to_vec()).map_err(wire_err)
    }
    fn read(&mut self, timeout: Duration) -> Result<Vec<u8>, PtpError> {
        self.0.read(timeout.as_millis() as u32).map_err(wire_err)
    }
    fn read_event(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, PtpError> {
        match self.0.read_event(timeout.as_millis() as u32) {
            Ok(bytes) if bytes.is_empty() => Ok(None),
            Ok(bytes) => Ok(Some(bytes)),
            Err(UsbError::Timeout) => Ok(None),
            Err(e) => Err(wire_err(e)),
        }
    }
    fn describe(&self) -> String {
        self.0.describe()
    }
}

/// Implemented in Kotlin. Called from a background thread: post to the main thread before touching UI state.
#[uniffi::export(with_foreign)]
pub trait LumenListener: Send + Sync {
    fn on_devices(&self, devices: Vec<DeviceInfo>);
    fn on_connected(&self, camera: CameraInfo, caps: CameraCaps, settings: Vec<CameraSetting>);
    fn on_disconnected(&self, reason: Option<String>);
    fn on_settings(&self, settings: Vec<CameraSetting>);
    fn on_capabilities(&self, caps: CameraCaps);
    fn on_live_view(&self, running: bool);
    fn on_recording(&self, recording: bool);
    fn on_frame(&self, frame: LiveFrame);
    /// `path` is `None` when the image stayed on the camera card.
    fn on_captured(&self, path: Option<String>, label: String);
    fn on_log(&self, level: LogLevel, message: String);
}

fn to_caps(c: &Capabilities) -> CameraCaps {
    CameraCaps { live_view: c.live_view, focus_drive: c.focus_drive, autofocus: c.autofocus, record: c.record }
}

fn to_settings(list: &[lumen_core::camera::Setting]) -> Vec<CameraSetting> {
    list.iter()
        .map(|s| CameraSetting {
            prop: s.prop,
            label: s.label.to_string(),
            writable: s.writable,
            current: s.current,
            text: s.text.clone(),
            choices: s.choices.iter().map(|(value, text)| Choice { value: *value, text: text.clone() }).collect(),
        })
        .collect()
}

fn dispatch(listener: &dyn LumenListener, event: Event) {
    match event {
        Event::Devices(list) => listener.on_devices(
            list.into_iter().map(|d| DeviceInfo { simulated: d.kind == DeviceKind::Simulated, name: d.name, detail: d.detail }).collect(),
        ),
        Event::Connected { identity, caps, settings } => listener.on_connected(
            CameraInfo { manufacturer: identity.manufacturer, model: identity.model, serial: identity.serial, transport: identity.transport },
            to_caps(&caps),
            to_settings(&settings),
        ),
        Event::Disconnected { reason } => listener.on_disconnected(reason),
        Event::Settings(s) => listener.on_settings(to_settings(&s)),
        Event::Capabilities(c) => listener.on_capabilities(to_caps(&c)),
        Event::LiveView(on) => listener.on_live_view(on),
        Event::Recording(on) => listener.on_recording(on),
        Event::Frame(f) => listener.on_frame(LiveFrame {
            width: f.width as u32,
            height: f.height as u32,
            rgba: f.rgba,
            histogram: f.histogram,
            sharpness: f.sharpness,
        }),
        Event::Captured(rec) => listener.on_captured(rec.path.map(|p| p.to_string_lossy().into_owned()), rec.label),
        Event::Log(level, msg) => {
            let level = match level {
                CoreLevel::Info => LogLevel::Info,
                CoreLevel::Success => LogLevel::Success,
                CoreLevel::Warn => LogLevel::Warn,
                CoreLevel::Error => LogLevel::Error,
            };
            listener.on_log(level, msg);
        }
        // Bracketing, stacking and mode-switch overlays arrive with the matching UI in a later milestone.
        _ => {}
    }
}

#[derive(uniffi::Object)]
pub struct LumenSession {
    engine: Engine,
    pictures: PathBuf,
}

#[uniffi::export]
impl LumenSession {
    /// `data_dir` = `Context.getFilesDir()`; config, logs and photos live below it.
    #[uniffi::constructor]
    pub fn new(data_dir: String, listener: Arc<dyn LumenListener>) -> Arc<Self> {
        let data_dir = PathBuf::from(data_dir);
        config::set_data_dir(data_dir.clone());
        logfile::init_at(data_dir.join("logs").join("lumen.log"));

        let engine = Engine::spawn(Arc::new(|| {}));
        let events = engine.events.clone();
        std::thread::Builder::new()
            .name("lumen-events".into())
            .spawn(move || {
                // Ends when the engine (and with it the sender) is dropped.
                while let Ok(event) = events.recv() {
                    dispatch(listener.as_ref(), event);
                }
            })
            .expect("failed to start the event pump");
        let pictures = config::Config::default().output_dir;
        Arc::new(Self { engine, pictures })
    }

    /// Lists the virtual camera; real cameras are announced by Kotlin through [`Self::connect_usb`].
    pub fn scan(&self) {
        self.engine.send(Command::Scan);
    }

    pub fn connect_simulator(&self) {
        self.engine.send(Command::Connect(DeviceEntry {
            kind: DeviceKind::Simulated,
            name: "Virtual DSLR".into(),
            detail: "Built-in simulator (full PTP protocol)".into(),
        }));
    }

    /// Connect a camera whose USB pipe is driven from Kotlin. Keep the connection open until `disconnect`.
    pub fn connect_usb(&self, io: Arc<dyn UsbTransport>, name: String) {
        let id = lumen_core::link::register_bridge(Box::new(KotlinLink(io)));
        self.engine.send(Command::Connect(DeviceEntry { kind: DeviceKind::Bridge { id }, name, detail: "USB (Android host)".into() }));
    }

    pub fn disconnect(&self) {
        self.engine.send(Command::Disconnect);
    }

    pub fn set_setting(&self, prop: u16, value: i64) {
        self.engine.send(Command::SetSetting { prop, value });
    }

    pub fn set_live_view(&self, on: bool) {
        self.engine.send(Command::LiveView(on));
    }

    pub fn record(&self, on: bool) {
        self.engine.send(Command::Record(on));
    }

    /// Fires the shutter. Only call it from an explicit user action.
    pub fn capture(&self) {
        logfile::line("android", "shutter requested from the UI");
        self.engine.send(Command::Capture { dir: self.pictures.clone(), prefix: "capture".into() });
    }

    pub fn focus(&self, far: bool, steps: u32) {
        let dir = if far { FocusDirection::Far } else { FocusDirection::Near };
        self.engine.send(Command::Focus { dir, steps });
    }

    pub fn autofocus(&self) {
        self.engine.send(Command::Autofocus);
    }

    pub fn pictures_dir(&self) -> String {
        self.pictures.to_string_lossy().into_owned()
    }
}
