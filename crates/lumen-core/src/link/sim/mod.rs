//! A virtual DSLR that speaks the real PTP wire protocol (Nikon-style
//! extension set). It lets the whole stack - containers, sessions, vendor
//! drivers, live view, capture and focus bracketing - run without hardware.

mod scene;

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::ptp::PtpError;
use crate::ptp::codec::{Container, ContainerKind, Writer};
use crate::ptp::codes::{ev, op, prop, rc, vendor, FORMAT_EXIF_JPEG};
use crate::ptp::dataset::{DeviceInfo, Form, PropDesc, Value, dt};
use crate::ptp::transport::WireLink;
use scene::{Exposure, FOCUS_MAX, Scene};

/// Size of the chunks the virtual bulk-in pipe returns, to exercise reassembly.
const BULK_CHUNK: usize = 4096;
const RC_INVALID_PROP_VALUE: u16 = 0x201B;

struct StoredObject {
    name: String,
    jpeg: Vec<u8>,
    size: (u32, u32),
}

pub struct SimCamera {
    props: BTreeMap<u16, PropDesc>,
    session: bool,
    live_view: bool,
    focus: i32,
    objects: Vec<StoredObject>,
    events: VecDeque<Container>,
    scene: Scene,
    shots: u32,
}

fn enum_prop(code: u16, datatype: u16, current: i64, values: &[i64]) -> PropDesc {
    let v = |x: i64| Value::int(x, datatype);
    PropDesc {
        code,
        datatype,
        writable: true,
        default: v(current),
        current: v(current),
        form: Form::Enumeration(values.iter().map(|x| v(*x)).collect()),
    }
}

impl SimCamera {
    pub fn new() -> Self {
        let mut props = BTreeMap::new();
        let mut add = |p: PropDesc| {
            props.insert(p.code, p);
        };
        add(enum_prop(prop::EXPOSURE_INDEX, dt::U16, 400, &[100, 200, 400, 800, 1600, 3200, 6400, 12800]));
        add(enum_prop(prop::F_NUMBER, dt::U16, 560, &[280, 400, 560, 800, 1100, 1600, 2200]));
        add(enum_prop(
            prop::EXPOSURE_TIME,
            dt::U32,
            80,
            &[5, 10, 20, 40, 80, 167, 333, 667, 1250, 2500, 5000, 10000],
        ));
        add(enum_prop(prop::EXPOSURE_BIAS, dt::I16, 0, &[-2000, -1667, -1333, -1000, -667, -333, 0, 333, 667, 1000, 1333, 1667, 2000]));
        add(enum_prop(prop::WHITE_BALANCE, dt::U16, 2, &[2, 4, 5, 6, 7]));
        add(enum_prop(prop::FOCUS_MODE, dt::U16, 1, &[1, 2]));
        add(enum_prop(prop::METERING_MODE, dt::U16, 3, &[2, 3, 4]));
        add(enum_prop(prop::EXPOSURE_PROGRAM, dt::U16, 1, &[1, 3, 4]));
        add(PropDesc {
            code: prop::BATTERY_LEVEL,
            datatype: dt::U8,
            writable: false,
            default: Value::int(100, dt::U8),
            current: Value::int(87, dt::U8),
            form: Form::Range { min: Value::int(0, dt::U8), max: Value::int(100, dt::U8), step: Value::int(1, dt::U8) },
        });
        Self {
            props,
            session: false,
            live_view: false,
            focus: FOCUS_MAX / 2,
            objects: Vec::new(),
            events: VecDeque::new(),
            scene: Scene::new((720, 480), (1800, 1200)),
            shots: 0,
        }
    }

    fn device_info(&self) -> DeviceInfo {
        use op::nikon::*;
        DeviceInfo {
            standard_version: 100,
            vendor_extension_id: vendor::NIKON,
            vendor_extension_version: 100,
            vendor_extension_desc: "Lumen virtual camera".into(),
            functional_mode: 0,
            operations: vec![
                op::GET_DEVICE_INFO,
                op::OPEN_SESSION,
                op::CLOSE_SESSION,
                op::GET_OBJECT_HANDLES,
                op::GET_OBJECT_INFO,
                op::GET_OBJECT,
                op::DELETE_OBJECT,
                op::INITIATE_CAPTURE,
                op::GET_DEVICE_PROP_DESC,
                op::GET_DEVICE_PROP_VALUE,
                op::SET_DEVICE_PROP_VALUE,
                AF_DRIVE,
                START_LIVE_VIEW,
                END_LIVE_VIEW,
                GET_LIVE_VIEW_IMG,
                MF_DRIVE,
            ],
            events: vec![ev::OBJECT_ADDED, ev::DEVICE_PROP_CHANGED, ev::CAPTURE_COMPLETE],
            properties: self.props.keys().copied().collect(),
            capture_formats: vec![FORMAT_EXIF_JPEG],
            image_formats: vec![FORMAT_EXIF_JPEG],
            manufacturer: "Lumen Labs".into(),
            model: "Virtual DSLR V1".into(),
            device_version: "1.0".into(),
            serial: "SIM-0001".into(),
        }
    }

    fn int_prop(&self, code: u16) -> i64 {
        self.props.get(&code).and_then(|p| p.current.as_i64()).unwrap_or_default()
    }

    fn exposure(&self) -> Exposure {
        let wb = match self.int_prop(prop::WHITE_BALANCE) {
            4 => [1.0, 1.0, 1.0],
            5 => [0.92, 1.0, 1.12],
            6 => [0.78, 1.0, 1.38],
            7 => [1.1, 1.0, 0.9],
            _ => [1.0, 1.0, 1.0],
        };
        Exposure {
            iso: self.int_prop(prop::EXPOSURE_INDEX) as f32,
            shutter_s: self.int_prop(prop::EXPOSURE_TIME) as f32 / 10_000.0,
            f_number: self.int_prop(prop::F_NUMBER) as f32 / 100.0,
            bias_ev: self.int_prop(prop::EXPOSURE_BIAS) as f32 / 1000.0,
            wb,
        }
    }

    fn object_info(&self, o: &StoredObject) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(0x0001_0001) // storage id
            .u16(FORMAT_EXIF_JPEG)
            .u16(0) // protection status
            .u32(o.jpeg.len() as u32)
            .u16(0) // thumb format
            .u32(0) // thumb size
            .u32(0) // thumb width
            .u32(0) // thumb height
            .u32(o.size.0)
            .u32(o.size.1)
            .u32(24) // bit depth
            .u32(0) // parent object
            .u16(0) // association type
            .u32(0) // association description
            .u32(0) // sequence number
            .string(&o.name)
            .string("20260101T000000")
            .string("20260101T000000")
            .string("");
        w.finish()
    }

    /// Process one command (plus optional inbound data) into response containers.
    fn handle(&mut self, opcode: u16, tid: u32, params: &[u32], data: &[u8]) -> Vec<Container> {
        let ok = |p: &[u32]| Container::new(ContainerKind::Response, rc::OK, tid, p.iter().flat_map(|v| v.to_le_bytes()).collect());
        let fail = |code: u16| Container::new(ContainerKind::Response, code, tid, Vec::new());
        let with_data = |payload: Vec<u8>, opcode: u16| {
            vec![Container::new(ContainerKind::Data, opcode, tid, payload), Container::new(ContainerKind::Response, rc::OK, tid, Vec::new())]
        };

        if opcode == op::GET_DEVICE_INFO {
            return with_data(self.device_info().encode(), opcode);
        }
        if opcode == op::OPEN_SESSION {
            if self.session {
                return vec![fail(rc::SESSION_ALREADY_OPEN)];
            }
            self.session = true;
            return vec![ok(&[])];
        }
        if !self.session {
            return vec![fail(rc::SESSION_NOT_OPEN)];
        }
        match opcode {
            op::CLOSE_SESSION => {
                self.session = false;
                self.live_view = false;
                vec![ok(&[])]
            }
            op::GET_DEVICE_PROP_DESC => match params.first().and_then(|p| self.props.get(&(*p as u16))) {
                Some(d) => with_data(d.encode(), opcode),
                None => vec![fail(rc::DEVICE_PROP_NOT_SUPPORTED)],
            },
            op::GET_DEVICE_PROP_VALUE => match params.first().and_then(|p| self.props.get(&(*p as u16))) {
                Some(d) => with_data(d.current.to_bytes(), opcode),
                None => vec![fail(rc::DEVICE_PROP_NOT_SUPPORTED)],
            },
            op::SET_DEVICE_PROP_VALUE => {
                let code = params.first().copied().unwrap_or_default() as u16;
                let Some(desc) = self.props.get_mut(&code) else { return vec![fail(rc::DEVICE_PROP_NOT_SUPPORTED)] };
                if !desc.writable {
                    return vec![fail(rc::GENERAL_ERROR)];
                }
                let Ok(value) = Value::decode(&mut crate::ptp::codec::Reader::new(data), desc.datatype) else {
                    return vec![fail(RC_INVALID_PROP_VALUE)];
                };
                let allowed = match &desc.form {
                    Form::Enumeration(vs) => vs.contains(&value),
                    Form::Range { min, max, .. } => matches!((value.as_i64(), min.as_i64(), max.as_i64()), (Some(v), Some(lo), Some(hi)) if (lo..=hi).contains(&v)),
                    Form::None => true,
                };
                if !allowed {
                    return vec![fail(RC_INVALID_PROP_VALUE)];
                }
                desc.current = value;
                self.events.push_back(Container::new(ContainerKind::Event, ev::DEVICE_PROP_CHANGED, 0, code.to_le_bytes().iter().copied().chain([0, 0]).collect()));
                vec![ok(&[])]
            }
            op::nikon::START_LIVE_VIEW => {
                self.live_view = true;
                vec![ok(&[])]
            }
            op::nikon::END_LIVE_VIEW => {
                self.live_view = false;
                vec![ok(&[])]
            }
            op::nikon::GET_LIVE_VIEW_IMG => {
                if !self.live_view {
                    return vec![fail(rc::NIKON_NOT_LIVE_VIEW)];
                }
                let jpeg = self.scene.render_live(self.focus, &self.exposure(), 82);
                // Real Nikon bodies prefix the JPEG with a fixed-size info header.
                let mut payload = vec![0u8; 128];
                payload[0..2].copy_from_slice(&720u16.to_be_bytes());
                payload[2..4].copy_from_slice(&480u16.to_be_bytes());
                payload.extend_from_slice(&jpeg);
                with_data(payload, opcode)
            }
            op::nikon::MF_DRIVE => {
                let (dir, steps) = (params.first().copied().unwrap_or(0), params.get(1).copied().unwrap_or(0) as i32);
                let delta = match dir {
                    1 => -steps,
                    2 => steps,
                    _ => return vec![fail(rc::PARAMETER_NOT_SUPPORTED)],
                };
                self.focus = (self.focus + delta).clamp(0, FOCUS_MAX);
                vec![ok(&[])]
            }
            op::nikon::AF_DRIVE => {
                // Contrast AF locks onto the centre chart.
                self.focus = FOCUS_MAX / 2;
                vec![ok(&[])]
            }
            op::INITIATE_CAPTURE => {
                self.shots += 1;
                let (jpeg, size) = self.scene.render_full(self.focus, &self.exposure(), 92);
                let name = format!("DSC_{:04}.JPG", self.shots);
                self.objects.push(StoredObject { name, jpeg, size });
                let handle = self.objects.len() as u32;
                self.events.push_back(Container::new(ContainerKind::Event, ev::OBJECT_ADDED, 0, handle.to_le_bytes().to_vec()));
                self.events.push_back(Container::new(ContainerKind::Event, ev::CAPTURE_COMPLETE, 0, Vec::new()));
                vec![ok(&[])]
            }
            op::GET_OBJECT_HANDLES => {
                let mut w = Writer::default();
                let live: Vec<u32> = self.objects.iter().enumerate().filter(|(_, o)| !o.jpeg.is_empty()).map(|(i, _)| i as u32 + 1).collect();
                w.u32(live.len() as u32);
                live.iter().for_each(|h| {
                    w.u32(*h);
                });
                with_data(w.finish(), opcode)
            }
            op::GET_OBJECT_INFO | op::GET_OBJECT | op::DELETE_OBJECT => {
                let idx = params.first().copied().unwrap_or(0) as usize;
                let Some(o) = idx.checked_sub(1).and_then(|i| self.objects.get(i)) else {
                    return vec![fail(rc::INVALID_OBJECT_HANDLE)];
                };
                match opcode {
                    op::GET_OBJECT_INFO => with_data(self.object_info(o), opcode),
                    op::GET_OBJECT => with_data(o.jpeg.clone(), opcode),
                    _ => {
                        self.objects[idx - 1].jpeg.clear();
                        vec![ok(&[])]
                    }
                }
            }
            _ => vec![fail(rc::OPERATION_NOT_SUPPORTED)],
        }
    }
}

impl Default for SimCamera {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle used to unplug the virtual camera in tests.
#[derive(Clone, Default)]
pub struct Unplug(Arc<AtomicBool>);

impl Unplug {
    #[cfg(test)]
    pub fn pull(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// `WireLink` backed by an in-process [`SimCamera`].
pub struct SimLink {
    camera: SimCamera,
    pending: Option<(u16, u32, Vec<u32>)>,
    rx: VecDeque<Vec<u8>>,
    unplug: Unplug,
}

impl SimLink {
    pub fn new() -> (Self, Unplug) {
        let unplug = Unplug::default();
        (Self { camera: SimCamera::new(), pending: None, rx: VecDeque::new(), unplug: unplug.clone() }, unplug)
    }

    fn respond(&mut self, containers: Vec<Container>) {
        for c in containers {
            for chunk in c.encode().chunks(BULK_CHUNK) {
                self.rx.push_back(chunk.to_vec());
            }
        }
    }

    fn check(&self) -> Result<(), PtpError> {
        if self.unplug.0.load(Ordering::SeqCst) { Err(PtpError::Disconnected) } else { Ok(()) }
    }
}

impl WireLink for SimLink {
    fn write(&mut self, data: &[u8]) -> Result<(), PtpError> {
        self.check()?;
        let c = Container::decode(data)?;
        match c.kind {
            ContainerKind::Command => {
                let params = c.params();
                if c.code == op::SET_DEVICE_PROP_VALUE {
                    self.pending = Some((c.code, c.transaction, params));
                } else {
                    let out = self.camera.handle(c.code, c.transaction, &params, &[]);
                    self.respond(out);
                }
            }
            ContainerKind::Data => {
                let Some((code, tid, params)) = self.pending.take() else {
                    return Err(PtpError::Protocol("data phase without command".into()));
                };
                let out = self.camera.handle(code, tid, &params, &c.payload);
                self.respond(out);
            }
            _ => return Err(PtpError::Protocol("host sent a device-only container".into())),
        }
        Ok(())
    }

    fn read(&mut self, _timeout: Duration) -> Result<Vec<u8>, PtpError> {
        self.check()?;
        self.rx.pop_front().ok_or(PtpError::Timeout)
    }

    fn read_event(&mut self, _timeout: Duration) -> Result<Option<Vec<u8>>, PtpError> {
        self.check()?;
        Ok(self.camera.events.pop_front().map(|c| c.encode()))
    }

    fn describe(&self) -> String {
        "Virtual camera (in-process PTP)".into()
    }
}
