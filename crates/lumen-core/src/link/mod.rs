//! Device discovery and transport construction.

pub mod sim;
pub mod usb;
#[cfg(windows)]
pub mod wpd;

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ptp::PtpError;
use crate::ptp::transport::{ContainerTransport, PtpTransport, WireLink};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceKind {
    Simulated,
    Usb { bus: u8, address: u8 },
    /// A pipe the platform opened for us (Android: Kotlin does the USB I/O). Claimed with [`take_bridge`].
    Bridge { id: u64 },
    #[cfg(windows)]
    Wpd { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceEntry {
    pub kind: DeviceKind,
    pub name: String,
    pub detail: String,
}

/// Links handed over by the host platform, waiting for the engine thread to connect them.
static BRIDGES: Mutex<Vec<(u64, Box<dyn WireLink>)>> = Mutex::new(Vec::new());
static NEXT_BRIDGE: AtomicU64 = AtomicU64::new(1);

/// Park a platform-provided link and get the [`DeviceKind::Bridge`] id to connect to it.
pub fn register_bridge(link: Box<dyn WireLink>) -> u64 {
    let id = NEXT_BRIDGE.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut list) = BRIDGES.lock() {
        list.push((id, link));
    }
    id
}

fn take_bridge(id: u64) -> Option<Box<dyn WireLink>> {
    let mut list = BRIDGES.lock().ok()?;
    let at = list.iter().position(|(i, _)| *i == id)?;
    Some(list.swap_remove(at).1)
}

/// Enumerate every camera reachable right now. The virtual camera is always listed last.
pub fn scan() -> Vec<DeviceEntry> {
    let mut out = Vec::new();
    #[cfg(windows)]
    out.extend(wpd::scan());
    // A camera bound to the stock Windows driver shows up in WPD; its libusb twin could never be opened.
    #[cfg(windows)]
    let wpd_ids: Vec<String> = out.iter().filter_map(|d| match &d.kind { DeviceKind::Wpd { id } => Some(id.to_ascii_lowercase()), _ => None }).collect();
    #[cfg(not(windows))]
    let wpd_ids: Vec<String> = Vec::new();
    out.extend(usb::scan().into_iter().filter(|d| !wpd_ids.iter().any(|id| id.contains(&d.vid_pid().to_ascii_lowercase()))));
    out.push(DeviceEntry {
        kind: DeviceKind::Simulated,
        name: "Virtual DSLR".into(),
        detail: "Built-in simulator (full PTP protocol)".into(),
    });
    out
}

impl DeviceEntry {
    /// `vid_xxxx&pid_xxxx`, the form Windows uses inside device ids; empty if unknown.
    fn vid_pid(&self) -> String {
        let hex: Vec<&str> = self.detail.split_whitespace().nth(1).map(|s| s.split(':').collect()).unwrap_or_default();
        match hex.as_slice() {
            [v, p] => format!("vid_{v}&pid_{p}"),
            _ => "\u{0}".into(),
        }
    }
}

pub fn open(entry: &DeviceEntry) -> Result<Box<dyn PtpTransport>, PtpError> {
    match &entry.kind {
        DeviceKind::Simulated => Ok(Box::new(ContainerTransport::new(sim::SimLink::new().0))),
        DeviceKind::Usb { bus, address } => Ok(Box::new(ContainerTransport::new(usb::UsbLink::open(*bus, *address)?))),
        DeviceKind::Bridge { id } => {
            let link = take_bridge(*id).ok_or_else(|| PtpError::Transport("the platform USB pipe is no longer available".into()))?;
            Ok(Box::new(ContainerTransport::new(link)))
        }
        #[cfg(windows)]
        DeviceKind::Wpd { id } => Ok(Box::new(wpd::WpdTransport::open(id)?)),
    }
}
