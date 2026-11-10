//! High-level camera control on top of a PTP [`Session`].
//!
//! Vendor differences (live view, manual focus drive, capture) are isolated in
//! this module; everything above it only sees [`Camera`].

pub mod eos;
pub mod evf;
pub mod settings;

use std::time::{Duration, Instant};

use crate::ptp::codec::Reader;
use crate::ptp::codes::{ev, op, prop, vendor};
use crate::ptp::{PtpError, Session, Value};
pub use evf::LiveImage;
pub use settings::Setting;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Nikon,
    Canon,
    Generic,
}

impl Vendor {
    pub fn label(self) -> &'static str {
        match self {
            Self::Nikon => "Nikon PTP extensions",
            Self::Canon => "Canon EOS extensions",
            Self::Generic => "Standard PTP",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub live_view: bool,
    pub focus_drive: bool,
    pub autofocus: bool,
    /// Movie recording can be started and stopped over USB right now (Canon: the dial is on video).
    pub record: bool,
}

#[derive(Debug, Clone)]
pub struct Identity {
    pub manufacturer: String,
    pub model: String,
    pub serial: String,
    pub vendor: Vendor,
    pub transport: String,
}

impl Identity {
    pub fn title(&self) -> String {
        format!("{} {}", self.manufacturer, self.model).trim().to_string()
    }
}

/// A downloaded image.
#[derive(Debug, Clone)]
pub struct Shot {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub enum CaptureOutcome {
    /// One or more files (e.g. RAW + JPEG) transferred to the PC.
    Downloaded(Vec<Shot>),
    /// The shutter fired but the image stayed on the camera's own card.
    StoredOnCamera,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    Near,
    Far,
}

pub struct Camera {
    session: Session,
    vendor: Vendor,
    live: bool,
    caps: Capabilities,
    /// Canon: the SaveTo destination the camera had before we switched it to include the PC.
    canon_saved_dest: Option<u32>,
    /// Images the body announced (shutter button or our own release) that are not downloaded yet.
    pending_added: Vec<eos::Added>,
    /// New pictures are being sent to the PC (SaveTo includes it and the capacity was announced).
    pc_routed: bool,
    /// Mode dial position as judged from a fresh handshake (record status 0/4 = video, 3 = photo). The status
    /// reads 3 in video mode too once live view runs, so it cannot be judged from the live stream of events.
    mode_video: Option<bool>,
    /// A large mid-session property dump arrived: the body sends one when the mode dial moves.
    big_dump: bool,
    /// Canon: the drive mode the body had before we forced single-shot for tethered captures.
    saved_drive: Option<u32>,
    battery: Option<u8>,
    battery_checked: Option<Instant>,
    /// Canon EOS property state, fed by GetEvent.
    eos: eos::EosState,
}

/// GetEvent payloads with at least this many records are whole-state dumps (mode dial moved).
const BIG_DUMP: usize = 80;

impl Camera {
    pub fn open(transport: Box<dyn crate::ptp::PtpTransport>) -> Result<Self, PtpError> {
        let mut session = Session::open(transport)?;
        let vendor = detect_vendor(&session.info);
        if vendor == Vendor::Canon {
            // Canon EOS bodies must be switched to remote/PC mode first.
            session.call(op::canon::SET_REMOTE_MODE, &[1])?;
            session.call(op::canon::SET_EVENT_MODE, &[1])?;
        }
        let mut eos = eos::EosState::default();
        if vendor == Vendor::Canon
            && let Ok(r) = session.call(op::canon::GET_EVENT, &[])
        {
            eos.apply(&r.data);
        }
        let info = &session.info;
        let caps = match vendor {
            Vendor::Nikon => Capabilities {
                live_view: info.supports_op(op::nikon::START_LIVE_VIEW) && info.supports_op(op::nikon::GET_LIVE_VIEW_IMG),
                focus_drive: info.supports_op(op::nikon::MF_DRIVE),
                autofocus: info.supports_op(op::nikon::AF_DRIVE),
                record: false,
            },
            Vendor::Canon => Capabilities {
                live_view: info.supports_op(op::canon::GET_VIEW_FINDER_DATA),
                focus_drive: info.supports_op(op::canon::DRIVE_LENS),
                autofocus: false,
                record: eos.video_mode() || eos.recording(),
            },
            Vendor::Generic => Capabilities::default(),
        };
        let mut camera = Self { session, vendor, live: false, caps, canon_saved_dest: None, pending_added: Vec::new(), pc_routed: false, mode_video: None, big_dump: false, saved_drive: None, battery: None, battery_checked: None, eos };
        if vendor == Vendor::Canon {
            camera.ensure_pc_routing();
        }
        Ok(camera)
    }

    pub fn identity(&self) -> Identity {
        let i = &self.session.info;
        Identity {
            manufacturer: i.manufacturer.clone(),
            model: i.model.clone(),
            serial: i.serial.clone(),
            vendor: self.vendor,
            transport: self.session.describe(),
        }
    }

    #[cfg(test)]
    pub fn eos_raw(&self, code: u16) -> Option<(u32, Vec<u32>)> {
        self.eos.raw(code).map(|(v, a)| (v, a.to_vec()))
    }

    /// Raw device information, for diagnostics.
    #[cfg(test)]
    pub fn device_info(&self) -> &crate::ptp::DeviceInfo {
        &self.session.info
    }

    pub fn capabilities(&self) -> Capabilities {
        self.caps
    }

    /// Capabilities that depend on the body's mode dial (Canon: recording only in video mode).
    pub fn refresh_capabilities(&mut self) -> Capabilities {
        if self.vendor == Vendor::Canon {
            self.caps.record = self.in_video();
        }
        self.caps
    }

    pub fn live_view_active(&self) -> bool {
        self.live
    }

    /// Canon: drain pending `GetEvent` records into the property state.
    fn poll_eos(&mut self) -> Result<bool, PtpError> {
        let reply = self.session.call(op::canon::GET_EVENT, &[])?;
        let records = eos::summarize(&reply.data);
        if records.len() >= BIG_DUMP {
            self.big_dump = true;
        }
        if records.len() > 40 {
            crate::logfile::line("event", format!("initial property dump: {} records", records.len()));
        } else {
            for r in records {
                crate::logfile::line("event", r);
            }
        }
        self.pending_added.extend(eos::added_objects(&reply.data));
        Ok(self.eos.apply(&reply.data))
    }

    /// Re-read settings the camera changed by itself (dials, menus, battery); `None` if nothing changed.
    pub fn poll_settings(&mut self) -> Option<Vec<Setting>> {
        if self.vendor != Vendor::Canon {
            return None;
        }
        let eos_changed = self.poll_eos().ok()?;
        self.ensure_pc_routing();
        let battery_changed = self.refresh_battery();
        (eos_changed || battery_changed).then(|| self.canon_settings())
    }

    /// In a still mode, make sure pictures also go to the PC so shots taken with the camera's own
    /// button reach the app. Video mode keeps its own card-only recording.
    fn ensure_pc_routing(&mut self) {
        if self.in_video() {
            // Movies go to the card only: with PC delivery still selected the body answers "busy" to
            // both the app's and its own record button.
            if self.pc_routed && self.canon_set_dest(&[2]).unwrap_or(false) {
                self.pc_routed = false;
            }
            return;
        }
        if !self.pc_routed && self.canon_set_dest(&[6, 4]).unwrap_or(false) {
            self.canon_announce_pc_capacity();
            self.pc_routed = true;
        }
    }

    /// Download pictures announced since the last call (taken with the camera's shutter button).
    pub fn take_new_images(&mut self) -> Result<Vec<Shot>, PtpError> {
        if self.vendor != Vendor::Canon || self.pending_added.is_empty() {
            return Ok(Vec::new());
        }
        let announced = std::mem::take(&mut self.pending_added);
        if self.in_video() || !self.pc_routed {
            return Ok(Vec::new()); // card-only or movie files: nothing to fetch
        }
        announced.into_iter().map(|a| self.download_eos(a)).collect()
    }

    /// A continuous drive mode would give several pictures per release - fatal for focus bracketing.
    fn force_single_shot(&mut self) -> Result<(), PtpError> {
        let Some((current, avail)) = self.eos.raw(eos::DRIVE_MODE).map(|(c, a)| (c, a.to_vec())) else { return Ok(()) };
        if current != eos::DRIVE_SINGLE && avail.contains(&eos::DRIVE_SINGLE) {
            self.saved_drive.get_or_insert(current);
            self.canon_set_prop_retry(eos::DRIVE_MODE as u32, eos::DRIVE_SINGLE)?;
        }
        Ok(())
    }

    /// Canon: the EOS property list plus the standard battery level.
    fn canon_settings(&self) -> Vec<Setting> {
        let mut list = self.eos.settings();
        if let Some(level) = self.battery {
            list.push(Setting {
                prop: prop::BATTERY_LEVEL,
                label: "Battery",
                writable: false,
                current: level as i64,
                text: format!("{level}%"),
                choices: Vec::new(),
            });
        }
        list
    }

    /// Re-read the battery level at most every 10 s; true if it changed.
    fn refresh_battery(&mut self) -> bool {
        if self.battery_checked.is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
            return false;
        }
        self.battery_checked = Some(Instant::now());
        let level = self
            .session
            .prop_desc(prop::BATTERY_LEVEL)
            .ok()
            .and_then(|d| d.current.as_i64())
            .map(|v| v.clamp(0, 100) as u8);
        let changed = level != self.battery;
        if level.is_some() {
            self.battery = level;
        }
        changed
    }

    pub fn read_settings(&mut self) -> Vec<Setting> {
        if self.vendor == Vendor::Canon {
            let _ = self.poll_eos();
            self.battery_checked = None;
            self.refresh_battery();
            return self.canon_settings();
        }
        let supported: Vec<u16> =
            settings::STANDARD_PROPS.iter().copied().filter(|p| self.session.info.supports_prop(*p)).collect();
        supported
            .into_iter()
            .filter_map(|p| self.session.prop_desc(p).ok())
            .filter_map(|d| Setting::from_desc(&d))
            .collect()
    }

    pub fn set_setting(&mut self, prop_code: u16, value: i64) -> Result<(), PtpError> {
        if self.vendor == Vendor::Canon {
            self.canon_set_prop(prop_code as u32, value as u32)?;
            // The new value is confirmed through the event stream; give the body a moment.
            for _ in 0..5 {
                std::thread::sleep(Duration::from_millis(60));
                if self.poll_eos()? {
                    break;
                }
            }
            return Ok(());
        }
        let desc = self.session.prop_desc(prop_code)?;
        self.session.set_prop(prop_code, &Value::int(value, desc.datatype))
    }

    // ------------------------------------------------------------- recording

    /// True when a Canon body is on its video mode (still photos are unavailable).
    pub fn video_mode(&self) -> bool {
        self.vendor == Vendor::Canon && self.in_video()
    }

    fn in_video(&self) -> bool {
        self.mode_video.unwrap_or_else(|| self.eos.video_mode()) || self.eos.recording()
    }

    /// True once after a large property dump arrived (the sign that the mode dial moved).
    pub fn take_big_dump(&mut self) -> bool {
        std::mem::take(&mut self.big_dump)
    }

    /// True while the camera reports an active movie recording.
    pub fn recording(&self) -> bool {
        self.eos.recording()
    }

    /// Start or stop movie recording. The body must be in video mode (mode dial).
    pub fn set_recording(&mut self, on: bool) -> Result<(), PtpError> {
        if !self.caps.record {
            return Err(PtpError::Protocol("the camera is in photo mode; turn the mode dial to video to record".into()));
        }
        if self.recording() == on {
            return Ok(());
        }
        let value = if on { eos::RECORD_START } else { eos::RECORD_STOP };
        if on {
            self.ensure_pc_routing();
        }
        let mut started = self.canon_set_prop_retry(eos::RECORD_STATUS as u32, value);
        if on && started.as_ref().is_err_and(PtpError::is_transient) {
            // After the mode dial moved the body can sit in a half-switched viewfinder state: cycle the
            // viewfinder output once and ask again.
            let _ = self.canon_set_prop_retry(prop::CANON_EVF_OUTPUT_DEVICE, 0);
            std::thread::sleep(Duration::from_millis(400));
            let _ = self.poll_eos();
            let _ = self.canon_start_evf();
            std::thread::sleep(Duration::from_millis(300));
            started = self.canon_set_prop_retry(eos::RECORD_STATUS as u32, value);
        }
        match started {
            Err(e) if on && e.is_transient() => {
                let _ = self.poll_eos();
                let raw = |c: u16| self.eos.raw(c).map_or_else(|| "?".to_string(), |(v, _)| format!("{v:X}"));
                return Err(PtpError::Protocol(format!(
                    "the camera is busy and will not start recording (SaveTo {}, EVF output {}, EVF mode {}); \
                     turn the mode dial to video and make sure a memory card is inserted",
                    raw(eos::SAVE_TO),
                    raw(prop::CANON_EVF_OUTPUT_DEVICE as u16),
                    raw(prop::CANON_EVF_MODE as u16),
                )));
            }
            other => other?,
        }
        for _ in 0..25 {
            std::thread::sleep(Duration::from_millis(100));
            self.poll_eos()?;
            if self.recording() == on {
                return Ok(());
            }
        }
        Err(PtpError::Protocol(if on {
            "the camera did not start recording; turn the mode dial to video and make sure a memory card is inserted".into()
        } else {
            "the camera did not confirm that recording stopped".into()
        }))
    }

    // ----------------------------------------------------------- live view

    pub fn start_live_view(&mut self) -> Result<(), PtpError> {
        if !self.caps.live_view {
            return Err(PtpError::Protocol("this camera does not expose live view over PTP".into()));
        }
        match self.vendor {
            Vendor::Nikon => {
                self.session.call(op::nikon::START_LIVE_VIEW, &[])?;
            }
            Vendor::Canon => {
                self.canon_start_evf()?;
            }
            Vendor::Generic => unreachable!("generic cameras report no live view capability"),
        }
        self.live = true;
        Ok(())
    }

    pub fn stop_live_view(&mut self) -> Result<(), PtpError> {
        if !self.live {
            return Ok(());
        }
        self.live = false;
        match self.vendor {
            Vendor::Nikon => self.session.call(op::nikon::END_LIVE_VIEW, &[]).map(drop),
            Vendor::Canon => {
                // While recording the camera keeps its own viewfinder; do not blank it.
                if self.recording() { Ok(()) } else { self.canon_set_prop_retry(prop::CANON_EVF_OUTPUT_DEVICE, 0) }
            }
            Vendor::Generic => Ok(()),
        }
    }

    /// Canon: redo the remote-mode handshake after the mode dial moved between photo and video.
    /// The body does not reliably refresh its allowed values or viewfinder routing mid-session (record
    /// and live view then stay "busy"), whereas a fresh handshake always lands in the right state.
    pub fn reinit_remote(&mut self) -> Result<(), PtpError> {
        if self.vendor != Vendor::Canon || self.recording() {
            return Ok(());
        }
        let was_live = std::mem::take(&mut self.live);
        let _ = self.canon_set_prop(prop::CANON_EVF_OUTPUT_DEVICE, 0);
        let _ = self.session.call(op::canon::SET_EVENT_MODE, &[0]);
        let _ = self.session.call(op::canon::SET_REMOTE_MODE, &[0]);
        std::thread::sleep(Duration::from_millis(500));
        self.session.call(op::canon::SET_REMOTE_MODE, &[1])?;
        self.session.call(op::canon::SET_EVENT_MODE, &[1])?;
        self.eos = eos::EosState::default();
        // The property dump can lag the handshake; without the record-status property we would misread the mode.
        for _ in 0..10 {
            self.poll_eos()?;
            if self.eos.raw(eos::RECORD_STATUS).is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        // The first dump after a dial move still carries the old record status; let the body settle, then read it
        // with live view off, where it is unambiguous: 0 = video, 3 = photo.
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(250));
            self.poll_eos()?;
        }
        self.mode_video = Some(self.eos.video_mode());
        self.big_dump = false;
        crate::logfile::line("mode", format!("handshake says: {}", match self.mode_video { Some(true) => "video", Some(false) => "photo", None => "unknown" }));
        self.refresh_capabilities();
        self.pc_routed = false;
        self.ensure_pc_routing();
        if was_live && self.caps.live_view {
            for attempt in 0..6 {
                match self.start_live_view() {
                    Ok(()) => break,
                    Err(_) if attempt < 5 => {
                        std::thread::sleep(Duration::from_millis(300));
                    }
                    Err(e) => return Err(e),
                }
            }
            // Second opinion once the viewfinder runs: its mode is 2 for movies and 1 for stills.
            std::thread::sleep(Duration::from_millis(600));
            self.poll_eos()?;
            if let Some((evf, _)) = self.eos.raw(eos::EVF_MODE)
                && matches!(evf, 1 | 2)
            {
                let video = evf == 2;
                if self.mode_video != Some(video) {
                    crate::logfile::line("mode", format!("viewfinder mode says {}; correcting", if video { "video" } else { "photo" }));
                    self.mode_video = Some(video);
                    self.refresh_capabilities();
                    self.ensure_pc_routing();
                }
            }
        }
        Ok(())
    }

    /// Fetch the next live-view frame as JPEG bytes. `Ok(None)` means the
    /// camera is not ready yet and the caller should simply try again.
    pub fn live_view_frame(&mut self) -> Result<Option<LiveImage>, PtpError> {
        let reply = match self.vendor {
            Vendor::Nikon => self.session.call(op::nikon::GET_LIVE_VIEW_IMG, &[]),
            Vendor::Canon => self.session.call(op::canon::GET_VIEW_FINDER_DATA, &[0x0010_0000]),
            Vendor::Generic => return Ok(None),
        };
        match reply {
            Ok(r) => Ok(match self.vendor {
                Vendor::Canon => evf::parse_canon(&r.data),
                _ => evf::extract_jpeg(&r.data).map(|j| LiveImage::Jpeg(j.to_vec())),
            }),
            Err(e) if e.is_transient() => Ok(None),
            Err(e) => Err(e),
        }
    }

    // --------------------------------------------------------------- focus

    /// Move the focus ring by `steps` in the given direction.
    pub fn focus_drive(&mut self, dir: FocusDirection, steps: u32) -> Result<(), PtpError> {
        match self.vendor {
            Vendor::Nikon => {
                let d = if dir == FocusDirection::Near { 1 } else { 2 };
                self.session.call(op::nikon::MF_DRIVE, &[d, steps.max(1)]).map(drop)
            }
            Vendor::Canon => {
                // Canon accepts three coarse speeds per direction.
                let level = match steps {
                    0..=4 => 1,
                    5..=19 => 2,
                    _ => 3,
                };
                let p = if dir == FocusDirection::Near { level } else { 0x8000 | level };
                self.session.call(op::canon::DRIVE_LENS, &[p]).map(drop)
            }
            Vendor::Generic => Err(PtpError::Protocol("focus drive is not supported by this camera".into())),
        }
    }

    pub fn autofocus(&mut self) -> Result<(), PtpError> {
        match self.vendor {
            Vendor::Nikon => self.session.call(op::nikon::AF_DRIVE, &[]).map(drop),
            _ => Err(PtpError::Protocol("autofocus trigger is not supported by this camera".into())),
        }
    }

    // ------------------------------------------------------------- capture

    pub fn capture(&mut self, timeout: Duration) -> Result<CaptureOutcome, PtpError> {
        // Discard stale events so we only react to this shot's ObjectAdded.
        while self.session.poll_event(Duration::ZERO)?.is_some() {}
        let use_events = self.session.has_events();
        let before = if use_events { Vec::new() } else { self.session.object_handles().unwrap_or_default() };
        match self.vendor {
            Vendor::Canon => return self.canon_capture(timeout),
            _ => {
                self.session.call(op::INITIATE_CAPTURE, &[0, 0])?;
            }
        }
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(PtpError::Timeout);
            }
            let slice = remaining.min(Duration::from_millis(250));
            let added = if use_events {
                self.session.poll_event(slice)?.filter(|e| e.code == ev::OBJECT_ADDED).and_then(|e| e.params.first().copied())
            } else {
                std::thread::sleep(slice);
                self.session.object_handles()?.into_iter().find(|h| !before.contains(h))
            };
            if let Some(handle) = added {
                return self.download(handle).map(|s| CaptureOutcome::Downloaded(vec![s]));
            }
        }
    }

    fn download(&mut self, handle: u32) -> Result<Shot, PtpError> {
        let info = self.session.call(op::GET_OBJECT_INFO, &[handle])?;
        let name = parse_object_name(&info.data).unwrap_or_else(|| format!("object_{handle}.jpg"));
        let bytes = self.session.call(op::GET_OBJECT, &[handle])?.data;
        Ok(Shot { name, bytes })
    }

    /// Canon EOS: fire the shutter and fetch every image the body announces.
    fn canon_capture(&mut self, timeout: Duration) -> Result<CaptureOutcome, PtpError> {
        let to_host = self.canon_set_dest(&[6, 4])?;
        self.pc_routed = to_host;
        self.force_single_shot()?;
        self.poll_eos()?;
        if to_host {
            self.canon_announce_pc_capacity();
        }
        let mut status = self.canon_trigger()?;
        if status != 0 && to_host {
            // A non-zero status usually means the card is missing or full while saving to
            // "card + PC"; retry sending the image to the PC only.
            if self.canon_set_dest(&[4])? {
                self.canon_announce_pc_capacity();
                status = self.canon_trigger()?;
            }
        }
        if status != 0 {
            return Err(PtpError::Protocol(format!(
                "the camera refused to take the picture (status {status:#x}); still photos are unavailable while the mode dial is on video, and check focus, lens switch and memory card"
            )));
        }
        if !to_host {
            return Ok(CaptureOutcome::StoredOnCamera);
        }
        let deadline = Instant::now() + timeout;
        let mut announced: Vec<eos::Added> = Vec::new();
        let mut last_new = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(120));
            self.poll_eos()?;
            let fresh = std::mem::take(&mut self.pending_added);
            if !fresh.is_empty() {
                announced.extend(fresh);
                last_new = Instant::now();
            }
            // RAW+JPEG announces two files; wait briefly for stragglers after the first.
            if !announced.is_empty() && last_new.elapsed() > Duration::from_millis(1200) {
                break;
            }
            if Instant::now() > deadline {
                if announced.is_empty() {
                    return Err(PtpError::Timeout);
                }
                break;
            }
        }
        let shots = announced.into_iter().map(|a| self.download_eos(a)).collect::<Result<Vec<_>, _>>()?;
        Ok(CaptureOutcome::Downloaded(shots))
    }

    /// A body sending images to the PC refuses the shutter (status 0x7) until it has been told
    /// how much space the PC has (EDSDK `SetCapacity`).
    fn canon_announce_pc_capacity(&mut self) {
        if self.session.info.supports_op(op::canon::PC_HDD_CAPACITY) {
            let _ = self.session.call(op::canon::PC_HDD_CAPACITY, &[0x0FFF_FFFF, 0x1000, 1]);
        }
    }

    /// Send the shutter command; returns the camera's status word (0 = shot taken).
    fn canon_trigger(&mut self) -> Result<u32, PtpError> {
        if self.session.info.supports_op(op::canon::REMOTE_RELEASE) {
            let reply = self.session.call(op::canon::REMOTE_RELEASE, &[])?;
            return Ok(reply.params.first().copied().unwrap_or(0));
        }
        // Older bodies: explicit half press (focus) then full press.
        self.session.call(op::canon::REMOTE_RELEASE_ON, &[1, 0])?;
        std::thread::sleep(Duration::from_millis(400));
        let full = self.session.call(op::canon::REMOTE_RELEASE_ON, &[2, 0]);
        std::thread::sleep(Duration::from_millis(150));
        // Always release, even if the press reported a problem.
        let _ = self.session.call(op::canon::REMOTE_RELEASE_OFF, &[2]);
        let _ = self.session.call(op::canon::REMOTE_RELEASE_OFF, &[1]);
        full.map(|_| 0)
    }

    /// Choose where new images go (2 = card, 4 = PC, 6 = both), taking the first of
    /// `wanted` the body allows. Returns false if none is available.
    fn canon_set_dest(&mut self, wanted: &[u32]) -> Result<bool, PtpError> {
        let Some((current, avail)) = self.eos.raw(eos::SAVE_TO).map(|(c, a)| (c, a.to_vec())) else { return Ok(false) };
        let Some(target) = wanted.iter().copied().find(|t| avail.contains(t)) else { return Ok(false) };
        if current != target {
            self.canon_saved_dest.get_or_insert(current);
            self.canon_set_prop(eos::SAVE_TO as u32, target)?;
            self.poll_eos()?;
        }
        Ok(true)
    }

    fn download_eos(&mut self, added: eos::Added) -> Result<Shot, PtpError> {
        const CHUNK: u32 = 0x0010_0000;
        let mut bytes = Vec::new();
        loop {
            let part = self.session.call(op::canon::GET_PARTIAL_OBJECT, &[added.handle, bytes.len() as u32, CHUNK])?;
            let got = part.data.len();
            bytes.extend_from_slice(&part.data);
            if got < CHUNK as usize {
                break;
            }
        }
        let _ = self.session.call(op::canon::TRANSFER_COMPLETE, &[added.handle]);
        let name = added.name.unwrap_or_else(|| format!("IMG_{:08X}.{}", added.handle, guess_extension(&bytes)));
        Ok(Shot { name, bytes })
    }

    /// Test hook: leave the body routed to card+PC as photo mode does, before the dial moves to video.
    #[cfg(test)]
    pub fn route_to_pc_for_test(&mut self) -> Result<bool, PtpError> {
        let ok = self.canon_set_dest(&[6, 4])?;
        self.canon_announce_pc_capacity();
        self.pc_routed = ok;
        Ok(ok)
    }

    /// Test hook: storage ids and the raw StorageInfo of each (to see whether the body reports a card).
    #[cfg(test)]
    pub fn storage_for_test(&mut self) -> String {
        let ids = match self.session.call(0x1004, &[]) {
            Ok(r) => r.data,
            Err(e) => return format!("GetStorageIDs failed: {e}"),
        };
        let count = u32::from_le_bytes(ids.get(0..4).and_then(|b| b.try_into().ok()).unwrap_or([0; 4])) as usize;
        let mut out = format!("{count} storage(s)");
        for i in 0..count {
            let Some(id) = ids.get(4 + i * 4..8 + i * 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes) else { break };
            match self.session.call(0x1005, &[id]) {
                Ok(r) => out += &format!("; {id:08X}: {:02X?}", &r.data[..r.data.len().min(40)]),
                Err(e) => out += &format!("; {id:08X}: {e}"),
            }
        }
        out
    }

    /// Test hook: can EOS properties be read directly (GetDevicePropValue / GetDevicePropDesc)?
    #[cfg(test)]
    pub fn direct_read_for_test(&mut self, code: u16) -> String {
        let value = self.session.call(0x1015, &[code as u32]).map(|r| format!("{:02X?}", r.data)).unwrap_or_else(|e| e.to_string());
        let desc = self.session.call(0x1014, &[code as u32]).map(|r| format!("{} bytes {:02X?}", r.data.len(), &r.data[..r.data.len().min(24)])).unwrap_or_else(|e| e.to_string());
        format!("GetDevicePropValue: {value}; GetDevicePropDesc: {desc}")
    }

    /// Test hook: SaveTo = card only.
    #[cfg(test)]
    pub fn card_only_for_test(&mut self) -> Result<bool, PtpError> {
        self.pc_routed = false;
        let ok = self.canon_set_dest(&[2])?;
        std::thread::sleep(Duration::from_millis(500));
        self.poll_eos()?;
        Ok(ok)
    }

    /// Test hook: fire the shutter exactly like the camera's own button would (no waiting, no download).
    #[cfg(test)]
    pub fn release_only_for_test(&mut self) -> Result<u32, PtpError> {
        self.canon_trigger()
    }

    /// Route the electronic viewfinder to the PC. In video mode the body already runs the
    /// viewfinder in its movie mode (EVF mode 2) and refuses to change it, so only touch what is needed.
    fn canon_start_evf(&mut self) -> Result<(), PtpError> {
        let raw = |s: &Self, code: u32| s.eos.raw(code as u16).map(|(v, _)| v);
        if raw(self, prop::CANON_EVF_MODE) == Some(0) {
            self.canon_set_prop_retry(prop::CANON_EVF_MODE, 1)?;
        }
        let output = raw(self, prop::CANON_EVF_OUTPUT_DEVICE).unwrap_or(0);
        if output & 2 == 0 {
            self.canon_set_prop_retry(prop::CANON_EVF_OUTPUT_DEVICE, output | 2)?;
        }
        Ok(())
    }

    /// `canon_set_prop` that waits out transient "device busy" answers.
    fn canon_set_prop_retry(&mut self, prop: u32, value: u32) -> Result<(), PtpError> {
        let mut last = None;
        for _ in 0..12 {
            match self.canon_set_prop(prop, value) {
                Err(e) if e.is_transient() => {
                    last = Some(e);
                    std::thread::sleep(Duration::from_millis(150));
                }
                other => {
                    let _ = self.poll_eos();
                    return other;
                }
            }
        }
        Err(last.unwrap_or(PtpError::Timeout))
    }

    fn canon_set_prop(&mut self, prop: u32, value: u32) -> Result<(), PtpError> {
        // Canon "SetDevicePropValueEx": size, prop, value.
        let mut payload = Vec::with_capacity(12);
        payload.extend_from_slice(&12u32.to_le_bytes());
        payload.extend_from_slice(&prop.to_le_bytes());
        payload.extend_from_slice(&value.to_le_bytes());
        self.session.call_with_data(op::canon::SET_DEVICE_PROP_VALUE_EX, &[], &payload).map(drop)
    }

    pub fn close(&mut self) {
        if self.recording() {
            let _ = self.set_recording(false);
        }
        let _ = self.stop_live_view();
        if self.vendor == Vendor::Canon {
            if let Some(drive) = self.saved_drive.take() {
                let _ = self.canon_set_prop_retry(eos::DRIVE_MODE as u32, drive);
            }
            if let Some(mut dest) = self.canon_saved_dest.take() {
                // A leftover PC route (e.g. from a session that was killed) must not become the 'original' value.
                if dest != 2 && self.eos.raw(eos::SAVE_TO).is_some_and(|(_, a)| a.contains(&2)) {
                    dest = 2;
                }
                let _ = self.canon_set_prop(eos::SAVE_TO as u32, dest);
            }
            // Leave the body in normal mode so the next session gets a fresh property dump.
            let _ = self.session.call(op::canon::SET_EVENT_MODE, &[0]);
            let _ = self.session.call(op::canon::SET_REMOTE_MODE, &[0]);
        }
        self.session.close();
    }
}

/// File extension from the image magic bytes.
fn guess_extension(bytes: &[u8]) -> &'static str {
    match bytes {
        [0xFF, 0xD8, ..] => "jpg",
        [b'I', b'I', 0x2A, 0, _, _, _, _, b'C', b'R', ..] => "cr2",
        _ => "bin",
    }
}

/// Canon EOS bodies often report the Microsoft MTP extension id (6) instead of 0x0B,
/// so fall back to the maker name plus the presence of EOS-only operations.
fn detect_vendor(info: &crate::ptp::DeviceInfo) -> Vendor {
    let maker = info.manufacturer.to_ascii_lowercase();
    if info.vendor_extension_id == vendor::NIKON || (maker.contains("nikon") && info.supports_op(op::nikon::GET_LIVE_VIEW_IMG)) {
        Vendor::Nikon
    } else if info.vendor_extension_id == vendor::CANON
        || (maker.contains("canon") && info.supports_op(op::canon::SET_REMOTE_MODE) && info.supports_op(op::canon::GET_EVENT))
    {
        Vendor::Canon
    } else {
        Vendor::Generic
    }
}

/// Filename from an ObjectInfo dataset.
fn parse_object_name(data: &[u8]) -> Option<String> {
    let mut r = Reader::new(data);
    r.u32().ok()?; // storage id
    r.u16().ok()?; // format
    r.u16().ok()?; // protection
    r.u32().ok()?; // compressed size
    r.u16().ok()?; // thumb format
    for _ in 0..7 {
        r.u32().ok()?; // thumb size, thumb w/h, image w/h, bit depth, parent
    }
    r.u16().ok()?; // association type
    r.u32().ok()?; // association desc
    r.u32().ok()?; // sequence number
    r.string().ok().filter(|s| !s.is_empty())
}
