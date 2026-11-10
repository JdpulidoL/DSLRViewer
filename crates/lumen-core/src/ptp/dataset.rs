//! Typed PTP datasets: `DeviceInfo`, property values and property descriptors.

use super::PtpError;
use super::codec::{Reader, Writer};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    pub standard_version: u16,
    pub vendor_extension_id: u32,
    pub vendor_extension_version: u16,
    pub vendor_extension_desc: String,
    pub functional_mode: u16,
    pub operations: Vec<u16>,
    pub events: Vec<u16>,
    pub properties: Vec<u16>,
    pub capture_formats: Vec<u16>,
    pub image_formats: Vec<u16>,
    pub manufacturer: String,
    pub model: String,
    pub device_version: String,
    pub serial: String,
}

impl DeviceInfo {
    pub fn parse(data: &[u8]) -> Result<Self, PtpError> {
        let mut r = Reader::new(data);
        Ok(Self {
            standard_version: r.u16()?,
            vendor_extension_id: r.u32()?,
            vendor_extension_version: r.u16()?,
            vendor_extension_desc: r.string()?,
            functional_mode: r.u16()?,
            operations: r.u16_array()?,
            events: r.u16_array()?,
            properties: r.u16_array()?,
            capture_formats: r.u16_array()?,
            image_formats: r.u16_array()?,
            manufacturer: r.string()?,
            model: r.string()?,
            device_version: r.string()?,
            serial: r.string()?,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.u16(self.standard_version)
            .u32(self.vendor_extension_id)
            .u16(self.vendor_extension_version)
            .string(&self.vendor_extension_desc)
            .u16(self.functional_mode)
            .u16_array(&self.operations)
            .u16_array(&self.events)
            .u16_array(&self.properties)
            .u16_array(&self.capture_formats)
            .u16_array(&self.image_formats)
            .string(&self.manufacturer)
            .string(&self.model)
            .string(&self.device_version)
            .string(&self.serial);
        w.finish()
    }

    pub fn supports_op(&self, op: u16) -> bool {
        self.operations.contains(&op)
    }

    pub fn supports_prop(&self, prop: u16) -> bool {
        self.properties.contains(&prop)
    }
}

/// PTP data type codes used by device properties.
pub mod dt {
    pub const I8: u16 = 0x0001;
    pub const U8: u16 = 0x0002;
    pub const I16: u16 = 0x0003;
    pub const U16: u16 = 0x0004;
    pub const I32: u16 = 0x0005;
    pub const U32: u16 = 0x0006;
    pub const I64: u16 = 0x0007;
    pub const U64: u16 = 0x0008;
    pub const STR: u16 = 0xFFFF;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64, u16),
    Str(String),
}

impl Value {
    pub fn int(v: i64, datatype: u16) -> Self {
        Self::Int(v, datatype)
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(v, _) => Some(*v),
            Self::Str(_) => None,
        }
    }

    pub fn decode(r: &mut Reader<'_>, datatype: u16) -> Result<Self, PtpError> {
        Ok(match datatype {
            dt::I8 => Self::Int(r.u8()? as i8 as i64, datatype),
            dt::U8 => Self::Int(r.u8()? as i64, datatype),
            dt::I16 => Self::Int(r.u16()? as i16 as i64, datatype),
            dt::U16 => Self::Int(r.u16()? as i64, datatype),
            dt::I32 => Self::Int(r.u32()? as i32 as i64, datatype),
            dt::U32 => Self::Int(r.u32()? as i64, datatype),
            dt::I64 => Self::Int(r.u64()? as i64, datatype),
            dt::U64 => Self::Int(r.u64()? as i64, datatype),
            dt::STR => Self::Str(r.string()?),
            other => return Err(PtpError::Protocol(format!("unsupported data type {other:#06x}"))),
        })
    }

    pub fn encode(&self, w: &mut Writer) {
        match self {
            Self::Int(v, t) => match *t {
                dt::I8 | dt::U8 => {
                    w.u8(*v as u8);
                }
                dt::I16 | dt::U16 => {
                    w.u16(*v as u16);
                }
                dt::I32 | dt::U32 => {
                    w.u32(*v as u32);
                }
                _ => {
                    w.u64(*v as u64);
                }
            },
            Self::Str(s) => {
                w.string(s);
            }
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::default();
        self.encode(&mut w);
        w.finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Form {
    None,
    Range { min: Value, max: Value, step: Value },
    Enumeration(Vec<Value>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PropDesc {
    pub code: u16,
    pub datatype: u16,
    pub writable: bool,
    pub default: Value,
    pub current: Value,
    pub form: Form,
}

impl PropDesc {
    pub fn parse(data: &[u8]) -> Result<Self, PtpError> {
        let mut r = Reader::new(data);
        let code = r.u16()?;
        let datatype = r.u16()?;
        let writable = r.u8()? != 0;
        let default = Value::decode(&mut r, datatype)?;
        let current = Value::decode(&mut r, datatype)?;
        let form = match r.u8()? {
            0 => Form::None,
            1 => Form::Range {
                min: Value::decode(&mut r, datatype)?,
                max: Value::decode(&mut r, datatype)?,
                step: Value::decode(&mut r, datatype)?,
            },
            2 => {
                let n = r.u16()? as usize;
                Form::Enumeration((0..n).map(|_| Value::decode(&mut r, datatype)).collect::<Result<_, _>>()?)
            }
            other => return Err(PtpError::Protocol(format!("unknown property form {other}"))),
        };
        Ok(Self { code, datatype, writable, default, current, form })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.u16(self.code).u16(self.datatype).u8(self.writable as u8);
        self.default.encode(&mut w);
        self.current.encode(&mut w);
        match &self.form {
            Form::None => {
                w.u8(0);
            }
            Form::Range { min, max, step } => {
                w.u8(1);
                min.encode(&mut w);
                max.encode(&mut w);
                step.encode(&mut w);
            }
            Form::Enumeration(values) => {
                w.u8(2).u16(values.len() as u16);
                for v in values {
                    v.encode(&mut w);
                }
            }
        }
        w.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_info_roundtrip() {
        let info = DeviceInfo {
            standard_version: 100,
            vendor_extension_id: 0xA,
            manufacturer: "Acme".into(),
            model: "X1".into(),
            operations: vec![0x1001, 0x1002],
            properties: vec![0x500F],
            ..Default::default()
        };
        assert_eq!(DeviceInfo::parse(&info.encode()).unwrap(), info);
    }

    #[test]
    fn prop_desc_roundtrip_enum_and_range() {
        let e = PropDesc {
            code: 0x500F,
            datatype: dt::U16,
            writable: true,
            default: Value::int(100, dt::U16),
            current: Value::int(400, dt::U16),
            form: Form::Enumeration(vec![Value::int(100, dt::U16), Value::int(400, dt::U16)]),
        };
        assert_eq!(PropDesc::parse(&e.encode()).unwrap(), e);
        let r = PropDesc {
            code: 0x5010,
            datatype: dt::I16,
            writable: true,
            default: Value::int(0, dt::I16),
            current: Value::int(-333, dt::I16),
            form: Form::Range {
                min: Value::int(-3000, dt::I16),
                max: Value::int(3000, dt::I16),
                step: Value::int(333, dt::I16),
            },
        };
        assert_eq!(PropDesc::parse(&r.encode()).unwrap(), r);
    }
}
