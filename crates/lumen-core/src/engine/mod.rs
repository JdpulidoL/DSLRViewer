//! Background engine: owns the camera on a dedicated thread and talks to the
//! UI through typed commands and events.

mod files;
mod worker;

use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender};

use crate::camera::{Capabilities, FocusDirection, Identity, Setting};
use crate::imaging::stack::Options as StackOptions;
use crate::imaging::{Frame, Peaking};
use crate::link::DeviceEntry;

#[derive(Clone, Debug)]
pub struct BracketPlan {
    pub shots: u32,
    /// Focus-drive steps between consecutive shots.
    pub step: u32,
    pub direction: FocusDirection,
    /// Wait after each focus move before shooting, in milliseconds.
    pub settle_ms: u64,
    pub output_dir: PathBuf,
    pub prefix: String,
}

#[derive(Debug)]
pub enum Command {
    Scan,
    Connect(DeviceEntry),
    Disconnect,
    SetSetting { prop: u16, value: i64 },
    LiveView(bool),
    /// Start (true) or stop (false) movie recording.
    Record(bool),
    SetPeaking(Option<Peaking>),
    /// Folder and file prefix for pictures taken with the camera's own shutter button.
    SetOutput { dir: PathBuf, prefix: String },
    Capture { dir: PathBuf, prefix: String },
    Focus { dir: FocusDirection, steps: u32 },
    Autofocus,
    Bracket(BracketPlan),
    CancelBracket,
    Stack { paths: Vec<PathBuf>, options: StackOptions },
    Shutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Success,
    Warn,
    Error,
}

pub struct Thumb {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

pub struct CaptureRecord {
    /// `None` when the image stayed on the camera card.
    pub path: Option<PathBuf>,
    pub label: String,
    pub thumb: Option<Thumb>,
}

/// A setting that was changed on the camera body (dial, menu, button) rather than from the app.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub prop: u16,
    pub label: &'static str,
    pub from: String,
    pub to: String,
}

pub enum Event {
    Devices(Vec<DeviceEntry>),
    Connected { identity: Identity, caps: Capabilities, settings: Vec<Setting> },
    Disconnected { reason: Option<String> },
    Settings(Vec<Setting>),
    /// What the user just changed by hand on the camera; follows the matching `Settings` event.
    CameraChanged(Vec<Change>),
    /// The camera's abilities changed (e.g. the mode dial moved between photo and video).
    Capabilities(Capabilities),
    /// The mode dial moved and the camera is re-initialising: `Some(true)` = to video, `None` = done.
    ModeSwitching(Option<bool>),
    LiveView(bool),
    Recording(bool),
    Frame(Frame),
    Captured(CaptureRecord),
    BracketProgress { done: u32, total: u32 },
    BracketFinished { folder: PathBuf, paths: Vec<PathBuf>, cancelled: bool },
    StackProgress(f32),
    StackFinished { path: PathBuf, preview: Option<Thumb> },
    StackFailed(String),
    Log(Level, String),
}

pub struct Engine {
    tx: Sender<Command>,
    pub events: Receiver<Event>,
    handle: Option<JoinHandle<()>>,
}

impl Engine {
    /// `wake` is called whenever an event is queued so the UI can repaint.
    pub fn spawn(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let (evt_tx, evt_rx) = crossbeam_channel::unbounded();
        let handle = std::thread::Builder::new()
            .name("camera-engine".into())
            .spawn(move || worker::run(cmd_rx, evt_tx, wake))
            .expect("failed to start the camera engine thread");
        Self { tx: cmd_tx, events: evt_rx, handle: Some(handle) }
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
