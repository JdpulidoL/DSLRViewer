//! Windows Portable Devices (WPD) link.
//!
//! Cameras that use the stock Windows MTP/PTP driver are not reachable through
//! raw USB. WPD exposes the driver's "MTP extension" commands, which tunnel
//! complete PTP transactions (operation code, parameters, data phases) to the
//! device. Session management stays with the driver.

// windows-rs declares COM out-parameters as *const T; passing &mut documents that the callee writes.
#![allow(clippy::unnecessary_mut_passed)]

use std::ffi::c_void;
use std::time::Duration;

use windows::Win32::Devices::PortableDevices::*;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize};
use windows::core::{HRESULT, PCWSTR, PWSTR};

use super::{DeviceEntry, DeviceKind};
use crate::ptp::PtpError;
use crate::ptp::codec::Writer;
use crate::ptp::codes::{op, rc, vendor};
use crate::ptp::dataset::DeviceInfo;
use crate::ptp::transport::{PtpEvent, PtpTransport, Reply};

/// HRESULTs that mean the camera went away.
const GONE: [u32; 3] = [0x8007_048F, 0x8007_001F, 0x8007_0015];
const DEFAULT_CHUNK: u32 = 256 * 1024;

/// Initialises COM (MTA) for the calling thread for as long as it lives.
pub struct ComGuard(bool);

impl ComGuard {
    pub fn new() -> Self {
        // S_OK and S_FALSE both require a matching CoUninitialize.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Default for ComGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

fn werr(context: &str, e: windows::core::Error) -> PtpError {
    if GONE.contains(&(e.code().0 as u32)) {
        PtpError::Disconnected
    } else {
        PtpError::Transport(format!("{context}: {e}"))
    }
}

fn pwstr_to_string(p: PWSTR) -> String {
    unsafe { p.to_string().unwrap_or_default() }
}

pub fn scan() -> Vec<DeviceEntry> {
    let _com = ComGuard::new();
    let Ok(manager) = (unsafe { CoCreateInstance::<_, IPortableDeviceManager>(&PortableDeviceManager, None, CLSCTX_INPROC_SERVER) }) else {
        return Vec::new();
    };
    let _ = unsafe { manager.RefreshDeviceList() };
    let mut count = 0u32;
    if unsafe { manager.GetDevices(std::ptr::null_mut(), &mut count) }.is_err() || count == 0 {
        return Vec::new();
    }
    let mut ids = vec![PWSTR::null(); count as usize];
    if unsafe { manager.GetDevices(ids.as_mut_ptr(), &mut count) }.is_err() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for id in ids.into_iter().take(count as usize) {
        let id_str = pwstr_to_string(id);
        let name = string_query(|buf, len| unsafe { manager.GetDeviceFriendlyName(PCWSTR(id.0), buf, len) });
        let maker = string_query(|buf, len| unsafe { manager.GetDeviceManufacturer(PCWSTR(id.0), buf, len) });
        unsafe { CoTaskMemFree(Some(id.0 as *const c_void)) };
        // Many camera drivers (e.g. Canon EOS) only advertise a storage category, so
        // fall back to the manufacturer / model name.
        let label = format!("{} {}", maker.as_deref().unwrap_or_default(), name.as_deref().unwrap_or_default());
        if !(is_camera(&id_str) || looks_like_camera(&label)) {
            continue;
        }
        out.push(DeviceEntry {
            name: name.filter(|n| !n.is_empty()).or_else(|| maker.clone()).unwrap_or_else(|| "Portable device".into()),
            detail: format!("{} (Windows WPD)", maker.unwrap_or_default()).trim().to_string(),
            kind: DeviceKind::Wpd { id: id_str },
        });
    }
    out
}

/// True if the portable device advertises the still-image-capture functional
/// category (cameras), as opposed to phones, drives and media players.
fn is_camera(id: &str) -> bool {
    let probe = || -> windows::core::Result<bool> {
        let device: IPortableDevice = unsafe { CoCreateInstance(&PortableDeviceFTM, None, CLSCTX_INPROC_SERVER) }?;
        let client: IPortableDeviceValues = unsafe { CoCreateInstance(&PortableDeviceValues, None, CLSCTX_INPROC_SERVER) }?;
        unsafe { client.SetStringValue(&WPD_CLIENT_NAME, windows::core::w!("Lumen Tether (probe)")) }?;
        let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
        unsafe { device.Open(PCWSTR(wide.as_ptr()), &client) }?;
        let categories = unsafe { device.Capabilities()?.GetFunctionalCategories() }?;
        let mut n = 0u32;
        unsafe { categories.GetCount(&mut n) }?;
        let mut found = false;
        for i in 0..n {
            let mut pv = PROPVARIANT::default();
            unsafe { categories.GetAt(i, &mut pv) }?;
            // SAFETY: a functional-category entry is a VT_CLSID variant holding a GUID pointer.
            let guid = unsafe { pv.Anonymous.Anonymous.Anonymous.puuid };
            if !guid.is_null() && unsafe { *guid } == WPD_FUNCTIONAL_CATEGORY_STILL_IMAGE_CAPTURE {
                found = true;
            }
        }
        let _ = unsafe { device.Close() };
        Ok(found)
    };
    probe().unwrap_or(false)
}

/// Camera makers whose devices are treated as cameras even without a still-image category.
fn looks_like_camera(label: &str) -> bool {
    const MAKERS: [&str; 12] =
        ["canon", "nikon", "sony", "fuji", "olympus", "om digital", "panasonic", "pentax", "ricoh", "leica", "sigma", "kodak"];
    let label = label.to_ascii_lowercase();
    MAKERS.iter().any(|m| label.contains(m))
}

/// Two-call pattern used by WPD for variable-length strings.
fn string_query(call: impl Fn(PWSTR, *mut u32) -> windows::core::Result<()>) -> Option<String> {
    let mut len = 0u32;
    call(PWSTR::null(), &mut len).ok()?;
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    call(PWSTR(buf.as_mut_ptr()), &mut len).ok()?;
    Some(String::from_utf16_lossy(&buf).trim_end_matches('\0').to_string())
}

pub struct WpdTransport {
    device: IPortableDevice,
    name: String,
    _com: ComGuard,
}

// SAFETY: the transport is created and used exclusively by the camera worker
// thread, which owns an MTA COM apartment (`_com`); MTA objects are free-threaded.
unsafe impl Send for WpdTransport {}

impl WpdTransport {
    pub fn open(id: &str) -> Result<Self, PtpError> {
        let com = ComGuard::new();
        let device: IPortableDevice = unsafe { CoCreateInstance(&PortableDeviceFTM, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| werr("create portable device", e))?;
        let client: IPortableDeviceValues = new_values()?;
        unsafe {
            let _ = client.SetStringValue(&WPD_CLIENT_NAME, windows::core::w!("Lumen Tether"));
            let _ = client.SetUnsignedIntegerValue(&WPD_CLIENT_MAJOR_VERSION, 1);
            let _ = client.SetUnsignedIntegerValue(&WPD_CLIENT_MINOR_VERSION, 0);
            let _ = client.SetUnsignedIntegerValue(&WPD_CLIENT_REVISION, 0);
        }
        let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
        unsafe { device.Open(PCWSTR(wide.as_ptr()), &client) }.map_err(|e| werr("open device", e))?;
        Ok(Self { device, name: id.to_string(), _com: com })
    }

    fn send(&self, command: PROPERTYKEY, fill: impl FnOnce(&IPortableDeviceValues) -> Result<(), PtpError>) -> Result<IPortableDeviceValues, PtpError> {
        let values = new_values()?;
        unsafe {
            values.SetGuidValue(&WPD_PROPERTY_COMMON_COMMAND_CATEGORY, &command.fmtid).map_err(|e| werr("command", e))?;
            values.SetUnsignedIntegerValue(&WPD_PROPERTY_COMMON_COMMAND_ID, command.pid).map_err(|e| werr("command", e))?;
        }
        fill(&values)?;
        let results = unsafe { self.device.SendCommand(0, &values) }.map_err(|e| werr("send command", e))?;
        let hr = unsafe { results.GetErrorValue(&WPD_PROPERTY_COMMON_HRESULT) }.unwrap_or(HRESULT(0));
        if hr.is_err() {
            return Err(werr(&format!("device rejected WPD command #{}", command.pid), windows::core::Error::from(hr)));
        }
        Ok(results)
    }

    fn operation_values(opcode: u16, params: &[u32]) -> impl FnOnce(&IPortableDeviceValues) -> Result<(), PtpError> + '_ {
        move |v| {
            let coll: IPortableDevicePropVariantCollection =
                unsafe { CoCreateInstance(&PortableDevicePropVariantCollection, None, CLSCTX_INPROC_SERVER) }
                    .map_err(|e| werr("create parameter list", e))?;
            for p in params {
                unsafe { coll.Add(&PROPVARIANT::from(*p)) }.map_err(|e| werr("add parameter", e))?;
            }
            unsafe {
                v.SetUnsignedIntegerValue(&WPD_PROPERTY_MTP_EXT_OPERATION_CODE, opcode as u32).map_err(|e| werr("opcode", e))?;
                v.SetIPortableDevicePropVariantCollectionValue(&WPD_PROPERTY_MTP_EXT_OPERATION_PARAMS, &coll).map_err(|e| werr("params", e))
            }
        }
    }

    fn response(results: &IPortableDeviceValues) -> (u16, Vec<u32>) {
        let code = unsafe { results.GetUnsignedIntegerValue(&WPD_PROPERTY_MTP_EXT_RESPONSE_CODE) }.map(|c| c as u16).unwrap_or(rc::OK);
        let mut params = Vec::new();
        if let Ok(coll) = unsafe { results.GetIPortableDevicePropVariantCollectionValue(&WPD_PROPERTY_MTP_EXT_RESPONSE_PARAMS) } {
            let mut n = 0u32;
            if unsafe { coll.GetCount(&mut n) }.is_ok() {
                for i in 0..n {
                    let mut pv = PROPVARIANT::default();
                    if unsafe { coll.GetAt(i, &mut pv) }.is_ok()
                        && let Ok(v) = u32::try_from(&pv)
                    {
                        params.push(v);
                    }
                }
            }
        }
        (code, params)
    }

    fn end_transfer(&self, context: PCWSTR) -> Result<(u16, Vec<u32>), PtpError> {
        let end = self.send(WPD_COMMAND_MTP_EXT_END_DATA_TRANSFER, |v| unsafe {
            v.SetStringValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_CONTEXT, context).map_err(|e| werr("end transfer", e))
        })?;
        Ok(Self::response(&end))
    }

    fn read_phase(&self, opcode: u16, params: &[u32]) -> Result<Reply, PtpError> {
        let start = self.send(WPD_COMMAND_MTP_EXT_EXECUTE_COMMAND_WITH_DATA_TO_READ, Self::operation_values(opcode, params))?;
        let context = unsafe { start.GetStringValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_CONTEXT) }.map_err(|e| werr("transfer context", e))?;
        let total = unsafe { start.GetUnsignedLargeIntegerValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_TOTAL_DATA_SIZE) }
            .or_else(|_| unsafe { start.GetUnsignedIntegerValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_TOTAL_DATA_SIZE) }.map(u64::from))
            .unwrap_or(0) as usize;
        let chunk = unsafe { start.GetUnsignedIntegerValue(&WPD_PROPERTY_MTP_EXT_OPTIMAL_TRANSFER_BUFFER_SIZE) }.unwrap_or(DEFAULT_CHUNK).max(4096);

        let mut data = Vec::with_capacity(total);
        let mut failure = None;
        while data.len() < total {
            let want = chunk.min((total - data.len()) as u32);
            let step = self.send(WPD_COMMAND_MTP_EXT_READ_DATA, |v| unsafe {
                v.SetStringValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_CONTEXT, PCWSTR(context.0)).map_err(|e| werr("read", e))?;
                v.SetUnsignedIntegerValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_NUM_BYTES_TO_READ, want).map_err(|e| werr("read", e))?;
                v.SetBufferValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_DATA, &vec![0u8; want as usize]).map_err(|e| werr("read", e))
            });
            match step.and_then(|r| read_buffer(&r, &WPD_PROPERTY_MTP_EXT_TRANSFER_DATA)) {
                Ok(bytes) if !bytes.is_empty() => data.extend_from_slice(&bytes),
                Ok(_) => break,
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
        }
        let end = self.end_transfer(PCWSTR(context.0));
        unsafe { CoTaskMemFree(Some(context.0 as *const c_void)) };
        if let Some(e) = failure {
            return Err(e);
        }
        let (code, params) = end?;
        Ok(Reply { code, params, data })
    }

    fn write_phase(&self, opcode: u16, params: &[u32], payload: &[u8]) -> Result<Reply, PtpError> {
        let start = self.send(WPD_COMMAND_MTP_EXT_EXECUTE_COMMAND_WITH_DATA_TO_WRITE, |v| {
            Self::operation_values(opcode, params)(v)?;
            // The driver rejects the command (ELEMENT_NOT_FOUND) unless both size properties are present.
            let size = payload.len() as u32;
            unsafe {
                v.SetUnsignedIntegerValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_NUM_BYTES_TO_WRITE, size)
                    .and_then(|_| v.SetUnsignedIntegerValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_TOTAL_DATA_SIZE, size))
            }
            .map_err(|e| werr("write size", e))
        })?;
        let context = unsafe { start.GetStringValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_CONTEXT) }.map_err(|e| werr("transfer context", e))?;
        let mut failure = None;
        for part in payload.chunks(DEFAULT_CHUNK as usize) {
            let step = self.send(WPD_COMMAND_MTP_EXT_WRITE_DATA, |v| unsafe {
                v.SetStringValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_CONTEXT, PCWSTR(context.0)).map_err(|e| werr("write", e))?;
                v.SetUnsignedIntegerValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_NUM_BYTES_TO_WRITE, part.len() as u32).map_err(|e| werr("write", e))?;
                v.SetBufferValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_DATA, part).map_err(|e| werr("write", e))
            });
            if let Err(e) = step {
                failure = Some(e);
                break;
            }
        }
        let end = self.end_transfer(PCWSTR(context.0));
        unsafe { CoTaskMemFree(Some(context.0 as *const c_void)) };
        if let Some(e) = failure {
            return Err(e);
        }
        let (code, params) = end?;
        Ok(Reply { code, params, data: Vec::new() })
    }

    /// Fallback when the driver refuses to forward `GetDeviceInfo`: describe the
    /// device from WPD's vendor-opcode query and the maker name.
    fn synthesize_device_info(&self) -> Result<Vec<u8>, PtpError> {
        let mut operations = vec![
            op::GET_DEVICE_INFO,
            op::GET_OBJECT_INFO,
            op::GET_OBJECT,
            op::INITIATE_CAPTURE,
            op::GET_DEVICE_PROP_DESC,
            op::GET_DEVICE_PROP_VALUE,
            op::SET_DEVICE_PROP_VALUE,
        ];
        let results = self.send(WPD_COMMAND_MTP_EXT_GET_SUPPORTED_VENDOR_OPCODES, |_| Ok(()))?;
        if let Ok(coll) = unsafe { results.GetIPortableDevicePropVariantCollectionValue(&WPD_PROPERTY_MTP_EXT_VENDOR_OPERATION_CODES) } {
            let mut n = 0u32;
            if unsafe { coll.GetCount(&mut n) }.is_ok() {
                for i in 0..n {
                    let mut pv = PROPVARIANT::default();
                    if unsafe { coll.GetAt(i, &mut pv) }.is_ok()
                        && let Ok(v) = u32::try_from(&pv)
                    {
                        operations.push(v as u16);
                    }
                }
            }
        }
        let name = self.name.to_ascii_lowercase();
        let vendor_extension_id = if name.contains("nikon") || operations.contains(&op::nikon::GET_LIVE_VIEW_IMG) {
            vendor::NIKON
        } else if name.contains("canon") || operations.contains(&op::canon::GET_VIEW_FINDER_DATA) {
            vendor::CANON
        } else {
            0
        };
        let info = DeviceInfo {
            standard_version: 100,
            vendor_extension_id,
            operations,
            properties: crate::camera::settings::STANDARD_PROPS.to_vec(),
            manufacturer: String::new(),
            model: "Camera (WPD)".into(),
            ..Default::default()
        };
        let mut w = Writer::default();
        w.bytes(&info.encode());
        Ok(w.finish())
    }
}

fn read_buffer(values: &IPortableDeviceValues, key: &PROPERTYKEY) -> Result<Vec<u8>, PtpError> {
    let (mut ptr, mut len) = (std::ptr::null_mut::<u8>(), 0u32);
    unsafe { values.GetBufferValue(key, &mut ptr, &mut len) }.map_err(|e| werr("read buffer", e))?;
    let bytes = if ptr.is_null() { Vec::new() } else { unsafe { std::slice::from_raw_parts(ptr, len as usize) }.to_vec() };
    unsafe { CoTaskMemFree(Some(ptr as *const c_void)) };
    // The driver reports how many bytes were actually read.
    let read = unsafe { values.GetUnsignedIntegerValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_NUM_BYTES_READ) }.unwrap_or(len) as usize;
    Ok(bytes[..read.min(bytes.len())].to_vec())
}

fn new_values() -> Result<IPortableDeviceValues, PtpError> {
    unsafe { CoCreateInstance(&PortableDeviceValues, None, CLSCTX_INPROC_SERVER) }.map_err(|e| werr("create values", e))
}

/// Operations whose response carries a data phase from device to host.
fn returns_data(opcode: u16) -> bool {
    use op::{canon, nikon};
    matches!(
        opcode,
        op::GET_DEVICE_INFO
            | op::GET_OBJECT_INFO
            | op::GET_OBJECT
            | op::GET_THUMB
            | op::GET_OBJECT_HANDLES
            | op::GET_DEVICE_PROP_DESC
            | op::GET_DEVICE_PROP_VALUE
            | nikon::GET_LIVE_VIEW_IMG
            | canon::GET_VIEW_FINDER_DATA
            | canon::GET_EVENT
            | canon::GET_PARTIAL_OBJECT
    )
}

impl PtpTransport for WpdTransport {
    fn transact(&mut self, opcode: u16, params: &[u32], send: Option<&[u8]>) -> Result<Reply, PtpError> {
        if let Some(payload) = send {
            return self.write_phase(opcode, params, payload);
        }
        if returns_data(opcode) {
            let result = self.read_phase(opcode, params);
            if opcode == op::GET_DEVICE_INFO && result.is_err() {
                return Ok(Reply { code: rc::OK, params: Vec::new(), data: self.synthesize_device_info()? });
            }
            return result;
        }
        let results = self.send(WPD_COMMAND_MTP_EXT_EXECUTE_COMMAND_WITHOUT_DATA_PHASE, Self::operation_values(opcode, params))?;
        let (code, params) = Self::response(&results);
        Ok(Reply { code, params, data: Vec::new() })
    }

    fn poll_event(&mut self, timeout: Duration) -> Result<Option<PtpEvent>, PtpError> {
        std::thread::sleep(timeout.min(Duration::from_millis(30)));
        Ok(None)
    }

    fn manages_session(&self) -> bool {
        true
    }

    fn has_events(&self) -> bool {
        false
    }

    fn describe(&self) -> String {
        "Windows Portable Devices (MTP/PTP driver)".into()
    }
}

impl Drop for WpdTransport {
    fn drop(&mut self) {
        let _ = unsafe { self.device.Close() };
    }
}
