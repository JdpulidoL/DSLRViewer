//! Canon EOS property model.
//!
//! EOS bodies do not expose the standard PTP property descriptors. Instead,
//! `GetEvent` (0x9116) returns a stream of records describing property values
//! and their allowed lists; this module folds that stream into state and turns
//! it into the generic [`Setting`] list the UI renders.

use std::collections::BTreeMap;

use super::settings::Setting;

pub const AV: u16 = 0xD101;
pub const TV: u16 = 0xD102;
pub const ISO: u16 = 0xD103;
pub const EXP_COMP: u16 = 0xD104;
pub const DRIVE_MODE: u16 = 0xD106;
pub const AE_MODE: u16 = 0xD105;
pub const METERING: u16 = 0xD107;
pub const FOCUS_MODE: u16 = 0xD108;
pub const WHITE_BALANCE: u16 = 0xD109;

/// Where new images go: 2 = card, 4 = host, 6 = both.
pub const SAVE_TO: u16 = 0xD11C;

/// Drive mode 0 = single shot. Anything else (continuous) makes one release deliver several frames.
pub const DRIVE_SINGLE: u32 = 0;

/// Movie recording state (EDSDK "Record"): 0 = stopped, 3 = ready, 4 = recording.
pub const RECORD_STATUS: u16 = 0xD1B8;
pub const EVF_MODE: u16 = 0xD1B1;
pub const RECORD_START: u32 = 4;
pub const RECORD_STOP: u32 = 0;

const EVENT_OBJECT_ADDED: u32 = 0xC181;
const EVENT_PROP_VALUE: u32 = 0xC189;
const EVENT_AVAIL_LIST: u32 = 0xC18A;

/// Display order of the properties shown in the UI.
const SHOWN: [u16; 8] = [AE_MODE, ISO, AV, TV, EXP_COMP, WHITE_BALANCE, METERING, FOCUS_MODE];

#[derive(Default, Clone, PartialEq, Eq)]
struct Prop {
    value: u32,
    avail: Vec<u32>,
}

#[derive(Default)]
pub struct EosState {
    props: BTreeMap<u16, Prop>,
}

fn le32(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

/// One short line per `GetEvent` record, for the diagnostic log (property changes, object events, unknown records).
pub fn summarize(data: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0;
    while let (Some(size), Some(kind)) = (le32(data, at), le32(data, at + 4)) {
        let size = size as usize;
        if size < 8 || at + size > data.len() {
            break;
        }
        let rec = &data[at..at + size];
        if kind == 0 {
            break; // end-of-list marker
        }
        out.push(match kind {
            EVENT_PROP_VALUE => format!("prop {:04X} = {:X}", le32(rec, 8).unwrap_or(0), le32(rec, 12).unwrap_or(0)),
            EVENT_AVAIL_LIST => format!("list {:04X} count {}", le32(rec, 8).unwrap_or(0), le32(rec, 16).unwrap_or(0)),
            EVENT_OBJECT_ADDED => format!("object added {:02X?}", &rec[8..rec.len().min(28)]),
            _ => format!("record {kind:04X} size {size} {:02X?}", &rec[8..rec.len().min(32)]),
        });
        at += size;
    }
    out
}

impl EosState {
    /// Fold one `GetEvent` payload into the state; true if anything changed.
    pub fn apply(&mut self, data: &[u8]) -> bool {
        let before = self.props.clone();
        let mut at = 0;
        while let (Some(size), Some(kind)) = (le32(data, at), le32(data, at + 4)) {
            let size = size as usize;
            if size < 8 || at + size > data.len() {
                break;
            }
            let rec = &data[at..at + size];
            match kind {
                EVENT_PROP_VALUE => {
                    if let (Some(code), Some(value)) = (le32(rec, 8), le32(rec, 12)) {
                        self.props.entry(code as u16).or_default().value = value;
                    }
                }
                EVENT_AVAIL_LIST => {
                    if let (Some(code), Some(count)) = (le32(rec, 8), le32(rec, 16)) {
                        let body = rec.get(20..).unwrap_or_default();
                        let width = if count == 0 { 0 } else { body.len() / count as usize };
                        let values = (0..count as usize)
                            .filter_map(|i| {
                                let b = body.get(i * width..(i + 1) * width)?;
                                Some(match width {
                                    1 => b[0] as u32,
                                    2 => u16::from_le_bytes([b[0], b[1]]) as u32,
                                    4 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                                    _ => return None,
                                })
                            })
                            .collect();
                        self.props.entry(code as u16).or_default().avail = values;
                    }
                }
                _ => {}
            }
            at += size;
        }
        self.props != before
    }

    /// True while the body reports an active movie recording.
    pub fn recording(&self) -> bool {
        self.props.get(&RECORD_STATUS).is_some_and(|p| p.value == RECORD_START)
    }

    /// Video mode, judged from the viewfinder mode (2 = movie, 1 = still), which the body reports consistently
    /// in both dial positions. The record status alone is ambiguous: it reads 0 in photo mode right after
    /// opening a session and 3 in video mode once live view runs; there the allowed list (it contains 4 only in
    /// video) is the fallback.
    pub fn video_mode(&self) -> bool {
        match self.props.get(&EVF_MODE).map(|p| p.value) {
            Some(2) => true,
            Some(1) => false,
            _ => self.props.get(&RECORD_STATUS).is_some_and(|p| p.avail.contains(&RECORD_START)),
        }
    }
    /// Raw current value and allowed list of any property.
    pub fn raw(&self, code: u16) -> Option<(u32, &[u32])> {
        self.props.get(&code).map(|p| (p.value, p.avail.as_slice()))
    }

    pub fn settings(&self) -> Vec<Setting> {
        SHOWN
            .iter()
            .filter_map(|code| {
                let p = self.props.get(code)?;
                let label = label(*code);
                let choices: Vec<(i64, String)> = p.avail.iter().map(|v| (*v as i64, describe(*code, *v))).collect();
                let writable = *code != AE_MODE && *code != FOCUS_MODE && choices.len() > 1;
                Some(Setting { prop: *code, label, writable, current: p.value as i64, text: describe(*code, p.value), choices })
            })
            .collect()
    }
}

/// An image the camera announced as ready for download.
#[derive(Debug, PartialEq, Eq)]
pub struct Added {
    pub handle: u32,
    pub name: Option<String>,
}

/// Extract `ObjectAddedEx` announcements from a `GetEvent` payload.
pub fn added_objects(data: &[u8]) -> Vec<Added> {
    let mut out = Vec::new();
    let mut at = 0;
    while let (Some(size), Some(kind)) = (le32(data, at), le32(data, at + 4)) {
        let size = size as usize;
        if size < 8 || at + size > data.len() {
            break;
        }
        if kind == EVENT_OBJECT_ADDED {
            let rec = &data[at..at + size];
            if let Some(handle) = le32(rec, 8) {
                out.push(Added { handle, name: filename_in(rec.get(12..).unwrap_or_default()) });
            }
        }
        at += size;
    }
    out
}

/// The record embeds the file name as a NUL-terminated ASCII string; find the longest
/// printable run that looks like `NAME.EXT`.
fn filename_in(bytes: &[u8]) -> Option<String> {
    bytes
        .split(|b| !(b.is_ascii_graphic()))
        .filter(|run| run.len() >= 5 && run.contains(&b'.'))
        .max_by_key(|run| run.len())
        .and_then(|run| String::from_utf8(run.to_vec()).ok())
}

fn label(code: u16) -> &'static str {
    match code {
        AV => "Aperture",
        TV => "Shutter",
        ISO => "ISO",
        EXP_COMP => "Exposure comp.",
        AE_MODE => "Exposure mode",
        METERING => "Metering",
        FOCUS_MODE => "Focus mode",
        WHITE_BALANCE => "White balance",
        _ => "Setting",
    }
}

pub fn describe(code: u16, v: u32) -> String {
    match code {
        ISO => iso(v),
        AV => aperture(v),
        TV => shutter(v),
        EXP_COMP => format!("{:+.1} EV", ((v as u8 as i8) as f32 * 3.0 / 8.0).round() / 3.0),
        AE_MODE => named(v, &[(0, "Program"), (1, "Shutter priority"), (2, "Aperture priority"), (3, "Manual"), (4, "Bulb"), (5, "A-DEP"), (6, "DEP"), (0x13, "Creative Auto"), (0x16, "Scene Intelligent Auto")]),
        METERING => named(v, &[(3, "Evaluative"), (4, "Partial"), (5, "Spot"), (7, "Centre-weighted")]),
        FOCUS_MODE => named(v, &[(0, "One-Shot AF"), (1, "AI Servo"), (2, "AI Focus"), (3, "Manual")]),
        WHITE_BALANCE => named(v, &[(0, "Auto"), (1, "Daylight"), (2, "Cloudy"), (3, "Tungsten"), (4, "Fluorescent"), (5, "Flash"), (6, "Custom"), (8, "Shade"), (9, "Colour temp."), (0x17, "Auto (white priority)")]),
        _ => format!("{v:#x}"),
    }
}

fn named(v: u32, table: &[(u32, &str)]) -> String {
    table.iter().find(|(k, _)| *k == v).map_or_else(|| format!("#{v:#x}"), |(_, n)| (*n).to_string())
}

/// ISO codes advance 8 per stop from 0x48 (= ISO 100); 0 means Auto.
fn iso(v: u32) -> String {
    if v == 0 {
        return "Auto".into();
    }
    let raw = 100.0 * 2f64.powf((v as f64 - 72.0) / 8.0);
    const NOMINAL: [u32; 17] = [50, 100, 125, 160, 200, 250, 320, 400, 500, 640, 800, 1000, 1250, 1600, 3200, 6400, 12800];
    let nearest = NOMINAL.iter().min_by(|a, b| (raw.ln() - (**a as f64).ln()).abs().total_cmp(&(raw.ln() - (**b as f64).ln()).abs())).unwrap();
    format!("ISO {nearest}")
}

/// Aperture codes advance 16 per doubling of the f-number from 0x08 (= f/1.0), i.e. 8 per stop of light.
fn aperture(v: u32) -> String {
    let raw = 2f64.powf((v as f64 - 8.0) / 16.0);
    const NOMINAL: [f64; 31] = [
        1.0, 1.1, 1.2, 1.4, 1.6, 1.8, 2.0, 2.2, 2.5, 2.8, 3.2, 3.5, 4.0, 4.5, 5.0, 5.6, 6.3, 7.1, 8.0, 9.0, 10.0, 11.0, 13.0, 14.0, 16.0, 18.0, 20.0, 22.0, 25.0, 29.0, 32.0,
    ];
    let nearest = NOMINAL.iter().min_by(|a, b| (raw.ln() - a.ln()).abs().total_cmp(&(raw.ln() - b.ln()).abs())).unwrap();
    if *nearest >= 10.0 { format!("f/{nearest:.0}") } else { format!("f/{nearest:.1}") }
}

/// Shutter codes advance 8 per stop from 0x10 (= 30 s).
fn shutter(v: u32) -> String {
    if v == 0x0C {
        return "Bulb".into();
    }
    let secs = 30.0 * 2f64.powf(-((v as f64) - 16.0) / 8.0);
    if secs >= 0.3 {
        let nominal: [f64; 21] = [30.0, 25.0, 20.0, 15.0, 13.0, 10.0, 8.0, 6.0, 5.0, 4.0, 3.2, 2.5, 2.0, 1.6, 1.3, 1.0, 0.8, 0.6, 0.5, 0.4, 0.3];
        let n = nominal.iter().min_by(|a, b| (secs.ln() - a.ln()).abs().total_cmp(&(secs.ln() - b.ln()).abs())).unwrap();
        return format!("{n}s");
    }
    const DENOMS: [u32; 31] = [4, 5, 6, 8, 10, 13, 15, 20, 25, 30, 40, 50, 60, 80, 100, 125, 160, 200, 250, 320, 400, 500, 640, 800, 1000, 1250, 1600, 2000, 2500, 3200, 4000];
    let want = (1.0 / secs).ln();
    let d = DENOMS.iter().min_by(|a, b| (want - (**a as f64).ln()).abs().total_cmp(&(want - (**b as f64).ln()).abs())).unwrap();
    format!("1/{d}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: u32, words: &[u32]) -> Vec<u8> {
        let mut r = Vec::new();
        r.extend_from_slice(&((8 + words.len() * 4) as u32).to_le_bytes());
        r.extend_from_slice(&kind.to_le_bytes());
        for w in words {
            r.extend_from_slice(&w.to_le_bytes());
        }
        r
    }

    #[test]
    fn labels_match_known_codes() {
        assert_eq!(describe(ISO, 0x48), "ISO 100");
        assert_eq!(describe(ISO, 0x68), "ISO 1600");
        assert_eq!(describe(AV, 0x10), "f/1.4");
        assert_eq!(describe(AV, 0x30), "f/5.6");
        assert_eq!(describe(AV, 0x58), "f/32");
        assert_eq!(describe(TV, 0x70), "1/125");
        assert_eq!(describe(TV, 0x98), "1/4000");
        assert_eq!(describe(TV, 0x38), "1s");
    }

    #[test]
    fn tracks_recording_state() {
        let mut s = EosState::default();
        assert!(!s.video_mode() && !s.recording());
        s.apply(&record(EVENT_PROP_VALUE, &[RECORD_STATUS as u32, 3]));
        assert!(!s.video_mode() && !s.recording());
        s.apply(&record(EVENT_PROP_VALUE, &[RECORD_STATUS as u32, RECORD_START]));
        assert!(s.recording());
        s.apply(&record(EVENT_PROP_VALUE, &[RECORD_STATUS as u32, RECORD_STOP]));
        assert!(!s.recording());
    }

    #[test]
    fn video_mode_follows_the_viewfinder_mode() {
        let mut s = EosState::default();
        s.apply(&record(EVENT_PROP_VALUE, &[RECORD_STATUS as u32, 0]));
        assert!(!s.video_mode(), "status 0 alone (photo right after open) is not video");
        s.apply(&record(EVENT_PROP_VALUE, &[EVF_MODE as u32, 1]));
        assert!(!s.video_mode());
        s.apply(&record(EVENT_PROP_VALUE, &[EVF_MODE as u32, 2]));
        s.apply(&record(EVENT_PROP_VALUE, &[RECORD_STATUS as u32, 3]));
        assert!(s.video_mode(), "status 3 with the movie viewfinder is video");
    }

    #[test]
    fn video_mode_falls_back_to_the_record_list() {
        let mut s = EosState::default();
        let mut list = record(EVENT_AVAIL_LIST, &[RECORD_STATUS as u32, 3, 2]);
        list.extend_from_slice(&[4, 0, 0, 0]);
        let len = list.len() as u32;
        list[..4].copy_from_slice(&len.to_le_bytes());
        s.apply(&list);
        assert!(s.video_mode());
    }
    #[test]
    fn finds_added_images_and_names() {
        let mut rec = record(EVENT_OBJECT_ADDED, &[0x80001234, 0x10001, 0xB103, 0, 5_000_000, 0]);
        rec.extend_from_slice(b"IMG_0042.CR2\0\0\0\0");
        let len = rec.len() as u32;
        rec[..4].copy_from_slice(&len.to_le_bytes());
        let mut data = record(EVENT_PROP_VALUE, &[ISO as u32, 0x58]);
        data.extend(rec);
        assert_eq!(added_objects(&data), vec![Added { handle: 0x80001234, name: Some("IMG_0042.CR2".into()) }]);
    }

    #[test]
    fn folds_value_and_list_events() {
        let mut data = record(EVENT_PROP_VALUE, &[ISO as u32, 0x58]);
        // List of three u16-width values: record carries datatype, count, then 3 * 2 bytes.
        let mut list = record(EVENT_AVAIL_LIST, &[ISO as u32, 3, 3]);
        list.extend_from_slice(&[0x48, 0, 0x50, 0, 0x58, 0]);
        let len = list.len() as u32;
        list[..4].copy_from_slice(&len.to_le_bytes());
        data.extend(list);
        data.extend(record(0, &[])); // terminator
        let mut s = EosState::default();
        assert!(s.apply(&data));
        assert!(!s.apply(&data), "re-applying identical data changes nothing");
        let iso = s.settings().into_iter().find(|x| x.prop == ISO).unwrap();
        assert_eq!(iso.text, "ISO 400");
        assert_eq!(iso.choices.iter().map(|c| c.1.as_str()).collect::<Vec<_>>(), ["ISO 100", "ISO 200", "ISO 400"]);
        assert!(iso.writable);
    }
}
