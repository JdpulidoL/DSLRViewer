//! Raw USB still-image-class (PTP) link built on libusb.
//!
//! On Windows this path works for cameras whose interface is bound to a
//! WinUSB/libusb driver; cameras using the stock Windows MTP/PTP driver are
//! reached through the WPD link instead.

use std::time::Duration;

use rusb::{Context, Device, DeviceHandle, Direction, TransferType, UsbContext};

use super::{DeviceKind, DeviceEntry};
use crate::ptp::PtpError;
use crate::ptp::transport::WireLink;

const CLASS_STILL_IMAGE: u8 = 0x06;
const READ_BUF: usize = 64 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

struct Endpoints {
    iface: u8,
    bulk_in: u8,
    bulk_out: u8,
    int_in: Option<u8>,
    max_packet: usize,
}

fn find_endpoints(device: &Device<Context>) -> Option<Endpoints> {
    let config = device.active_config_descriptor().ok()?;
    for iface in config.interfaces() {
        for desc in iface.descriptors() {
            if desc.class_code() != CLASS_STILL_IMAGE || desc.sub_class_code() != 1 {
                continue;
            }
            let (mut bulk_in, mut bulk_out, mut int_in, mut max_packet) = (None, None, None, 512);
            for ep in desc.endpoint_descriptors() {
                match (ep.transfer_type(), ep.direction()) {
                    (TransferType::Bulk, Direction::In) => {
                        bulk_in = Some(ep.address());
                        max_packet = ep.max_packet_size() as usize;
                    }
                    (TransferType::Bulk, Direction::Out) => bulk_out = Some(ep.address()),
                    (TransferType::Interrupt, Direction::In) => int_in = Some(ep.address()),
                    _ => {}
                }
            }
            if let (Some(bulk_in), Some(bulk_out)) = (bulk_in, bulk_out) {
                return Some(Endpoints { iface: desc.interface_number(), bulk_in, bulk_out, int_in, max_packet });
            }
        }
    }
    None
}

pub fn scan() -> Vec<DeviceEntry> {
    // Android hides /dev/bus/usb from apps; Kotlin enumerates with `UsbManager` and registers a bridge (see `link::register_bridge`).
    if cfg!(target_os = "android") {
        return Vec::new();
    }
    let Ok(ctx) = Context::new() else { return Vec::new() };
    let Ok(devices) = ctx.devices() else { return Vec::new() };
    devices
        .iter()
        .filter(|d| find_endpoints(d).is_some())
        .filter_map(|d| {
            let desc = d.device_descriptor().ok()?;
            let name = d
                .open()
                .ok()
                .and_then(|h| h.read_product_string_ascii(&desc).ok())
                .unwrap_or_else(|| format!("USB camera {:04x}:{:04x}", desc.vendor_id(), desc.product_id()));
            Some(DeviceEntry {
                kind: DeviceKind::Usb { bus: d.bus_number(), address: d.address() },
                name,
                detail: format!("USB {:04x}:{:04x} (libusb)", desc.vendor_id(), desc.product_id()),
            })
        })
        .collect()
}

pub struct UsbLink {
    handle: DeviceHandle<Context>,
    ep: Endpoints,
    label: String,
}

impl UsbLink {
    pub fn open(bus: u8, address: u8) -> Result<Self, PtpError> {
        let ctx = Context::new().map_err(map_err)?;
        let device = ctx
            .devices()
            .map_err(map_err)?
            .iter()
            .find(|d| d.bus_number() == bus && d.address() == address)
            .ok_or(PtpError::Disconnected)?;
        let ep = find_endpoints(&device).ok_or_else(|| PtpError::Transport("no PTP interface on device".into()))?;
        let handle = device.open().map_err(|e| {
            PtpError::Transport(format!(
                "cannot open the USB device ({e}). On Windows the camera must use a WinUSB driver for this path; \
                 otherwise use the WPD entry for the same camera"
            ))
        })?;
        let _ = handle.set_auto_detach_kernel_driver(true);
        handle.claim_interface(ep.iface).map_err(map_err)?;
        let desc = device.device_descriptor().map_err(map_err)?;
        let label = format!("USB {:04x}:{:04x} via libusb", desc.vendor_id(), desc.product_id());
        Ok(Self { handle, ep, label })
    }
}

fn map_err(e: rusb::Error) -> PtpError {
    match e {
        rusb::Error::Timeout => PtpError::Timeout,
        rusb::Error::NoDevice | rusb::Error::Io | rusb::Error::Pipe => PtpError::Disconnected,
        other => PtpError::Transport(other.to_string()),
    }
}

impl WireLink for UsbLink {
    fn write(&mut self, data: &[u8]) -> Result<(), PtpError> {
        let mut sent = 0;
        while sent < data.len() {
            sent += self.handle.write_bulk(self.ep.bulk_out, &data[sent..], WRITE_TIMEOUT).map_err(map_err)?;
        }
        // A transfer that ends exactly on a packet boundary needs a zero-length packet.
        if !data.is_empty() && data.len().is_multiple_of(self.ep.max_packet) {
            self.handle.write_bulk(self.ep.bulk_out, &[], WRITE_TIMEOUT).map_err(map_err)?;
        }
        Ok(())
    }

    fn read(&mut self, timeout: Duration) -> Result<Vec<u8>, PtpError> {
        let mut buf = vec![0u8; READ_BUF];
        let n = self.handle.read_bulk(self.ep.bulk_in, &mut buf, timeout).map_err(map_err)?;
        buf.truncate(n);
        Ok(buf)
    }

    fn read_event(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, PtpError> {
        let Some(ep) = self.ep.int_in else { return Ok(None) };
        let mut buf = [0u8; 64];
        match self.handle.read_interrupt(ep, &mut buf, timeout) {
            Ok(n) => Ok(Some(buf[..n].to_vec())),
            Err(rusb::Error::Timeout) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }

    fn describe(&self) -> String {
        self.label.clone()
    }
}

impl Drop for UsbLink {
    fn drop(&mut self) {
        let _ = self.handle.release_interface(self.ep.iface);
    }
}
