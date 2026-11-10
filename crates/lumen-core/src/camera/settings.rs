//! Human-facing description of PTP device properties.

use crate::ptp::codes::prop;
use crate::ptp::dataset::{Form, PropDesc, Value};

/// Properties exposed in the UI, in display order.
pub const STANDARD_PROPS: &[u16] = &[
    prop::EXPOSURE_PROGRAM,
    prop::EXPOSURE_INDEX,
    prop::F_NUMBER,
    prop::EXPOSURE_TIME,
    prop::EXPOSURE_BIAS,
    prop::WHITE_BALANCE,
    prop::FOCUS_MODE,
    prop::METERING_MODE,
    prop::BATTERY_LEVEL,
];

#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub prop: u16,
    pub label: &'static str,
    pub writable: bool,
    pub current: i64,
    pub text: String,
    pub choices: Vec<(i64, String)>,
}

impl Setting {
    pub fn from_desc(desc: &PropDesc) -> Option<Self> {
        let current = desc.current.as_i64()?;
        let label = label(desc.code)?;
        let choices = match &desc.form {
            Form::Enumeration(values) => values.iter().filter_map(Value::as_i64).map(|v| (v, format_value(desc.code, v))).collect(),
            Form::Range { min, max, step } => {
                let (lo, hi, st) = (min.as_i64()?, max.as_i64()?, step.as_i64()?.max(1));
                if (hi - lo) / st <= 64 { (lo..=hi).step_by(st as usize).map(|v| (v, format_value(desc.code, v))).collect() } else { Vec::new() }
            }
            Form::None => Vec::new(),
        };
        Some(Self { prop: desc.code, label, writable: desc.writable && !choices.is_empty(), current, text: format_value(desc.code, current), choices })
    }

    /// Index of the current value inside `choices`.
    pub fn current_index(&self) -> Option<usize> {
        self.choices.iter().position(|(v, _)| *v == self.current)
    }

    /// Neighbouring choice for stepper buttons.
    pub fn neighbour(&self, delta: isize) -> Option<i64> {
        let i = self.current_index()? as isize + delta;
        usize::try_from(i).ok().and_then(|i| self.choices.get(i)).map(|(v, _)| *v)
    }
}

pub fn label(code: u16) -> Option<&'static str> {
    Some(match code {
        prop::EXPOSURE_PROGRAM => "Exposure mode",
        prop::EXPOSURE_INDEX => "ISO",
        prop::F_NUMBER => "Aperture",
        prop::EXPOSURE_TIME => "Shutter",
        prop::EXPOSURE_BIAS => "Exposure comp.",
        prop::WHITE_BALANCE => "White balance",
        prop::FOCUS_MODE => "Focus mode",
        prop::METERING_MODE => "Metering",
        prop::BATTERY_LEVEL => "Battery",
        _ => return None,
    })
}

pub fn format_value(code: u16, v: i64) -> String {
    match code {
        prop::EXPOSURE_INDEX => format!("ISO {v}"),
        prop::F_NUMBER => format!("f/{:.1}", v as f64 / 100.0),
        prop::EXPOSURE_TIME => format_shutter(v),
        prop::EXPOSURE_BIAS => format!("{:+.1} EV", v as f64 / 1000.0),
        prop::BATTERY_LEVEL => format!("{v}%"),
        prop::EXPOSURE_PROGRAM => named(v, &[(1, "Manual"), (2, "Program"), (3, "Aperture priority"), (4, "Shutter priority")]),
        prop::WHITE_BALANCE => named(
            v,
            &[(1, "Manual"), (2, "Auto"), (3, "One push"), (4, "Daylight"), (5, "Fluorescent"), (6, "Tungsten"), (7, "Flash")],
        ),
        prop::FOCUS_MODE => named(v, &[(1, "Manual"), (2, "Autofocus"), (3, "Macro AF")]),
        prop::METERING_MODE => named(v, &[(1, "Average"), (2, "Centre-weighted"), (3, "Matrix"), (4, "Spot"), (5, "Multi-spot")]),
        _ => v.to_string(),
    }
}

fn named(v: i64, table: &[(i64, &str)]) -> String {
    table.iter().find(|(k, _)| *k == v).map_or_else(|| format!("#{v}"), |(_, n)| (*n).to_string())
}

/// PTP exposure time is expressed in units of 0.1 ms.
fn format_shutter(v: i64) -> String {
    if v <= 0 {
        return "-".into();
    }
    let seconds = v as f64 / 10_000.0;
    if seconds >= 0.4 { format!("{seconds:.1}s") } else { format!("1/{}", (1.0 / seconds).round() as i64) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_exposure_values() {
        assert_eq!(format_value(prop::EXPOSURE_TIME, 40), "1/250");
        assert_eq!(format_value(prop::EXPOSURE_TIME, 10000), "1.0s");
        assert_eq!(format_value(prop::F_NUMBER, 560), "f/5.6");
        assert_eq!(format_value(prop::EXPOSURE_BIAS, -667), "-0.7 EV");
        assert_eq!(format_value(prop::EXPOSURE_INDEX, 3200), "ISO 3200");
    }
}
