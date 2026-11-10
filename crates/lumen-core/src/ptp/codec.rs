//! Little-endian wire codec for PTP containers and datasets.

use super::PtpError;

pub const HEADER_LEN: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    Command = 1,
    Data = 2,
    Response = 3,
    Event = 4,
}

impl ContainerKind {
    fn from_u16(v: u16) -> Result<Self, PtpError> {
        Ok(match v {
            1 => Self::Command,
            2 => Self::Data,
            3 => Self::Response,
            4 => Self::Event,
            other => return Err(PtpError::Protocol(format!("unknown container type {other}"))),
        })
    }
}

/// A fully reassembled PTP container (header + payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    pub kind: ContainerKind,
    pub code: u16,
    pub transaction: u32,
    pub payload: Vec<u8>,
}

impl Container {
    pub fn new(kind: ContainerKind, code: u16, transaction: u32, payload: Vec<u8>) -> Self {
        Self { kind, code, transaction, payload }
    }

    pub fn command(code: u16, transaction: u32, params: &[u32]) -> Self {
        let mut w = Writer::default();
        for p in params {
            w.u32(*p);
        }
        Self::new(ContainerKind::Command, code, transaction, w.finish())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        out.extend_from_slice(&((HEADER_LEN + self.payload.len()) as u32).to_le_bytes());
        out.extend_from_slice(&(self.kind as u16).to_le_bytes());
        out.extend_from_slice(&self.code.to_le_bytes());
        out.extend_from_slice(&self.transaction.to_le_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// Total container length declared by a (possibly partial) header.
    pub fn declared_len(header: &[u8]) -> Option<u32> {
        header.get(..4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PtpError> {
        if bytes.len() < HEADER_LEN {
            return Err(PtpError::Protocol(format!("container too short: {} bytes", bytes.len())));
        }
        let mut r = Reader::new(bytes);
        let len = r.u32()? as usize;
        let kind = ContainerKind::from_u16(r.u16()?)?;
        let code = r.u16()?;
        let transaction = r.u32()?;
        let end = if len == u32::MAX as usize { bytes.len() } else { len.min(bytes.len()) };
        Ok(Self { kind, code, transaction, payload: bytes[HEADER_LEN..end.max(HEADER_LEN)].to_vec() })
    }

    /// Interpret the payload as a list of 32-bit parameters.
    pub fn params(&self) -> Vec<u32> {
        self.payload.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
    }
}

#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(v);
        self
    }
    /// PTP string: character count (including terminator) followed by UTF-16LE.
    pub fn string(&mut self, s: &str) -> &mut Self {
        if s.is_empty() {
            return self.u8(0);
        }
        let units: Vec<u16> = s.encode_utf16().take(254).collect();
        self.u8((units.len() + 1) as u8);
        for u in units {
            self.u16(u);
        }
        self.u16(0)
    }
    pub fn u16_array(&mut self, values: &[u16]) -> &mut Self {
        self.u32(values.len() as u32);
        for v in values {
            self.u16(*v);
        }
        self
    }
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], PtpError> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.data.len());
        match end {
            Some(end) => {
                let s = &self.data[self.pos..end];
                self.pos = end;
                Ok(s)
            }
            None => Err(PtpError::Protocol(format!(
                "truncated dataset: need {n} bytes at offset {}, have {}",
                self.pos,
                self.remaining()
            ))),
        }
    }

    pub fn u8(&mut self) -> Result<u8, PtpError> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, PtpError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, PtpError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, PtpError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    pub fn string(&mut self) -> Result<String, PtpError> {
        let n = self.u8()? as usize;
        if n == 0 {
            return Ok(String::new());
        }
        let raw = self.take(n * 2)?;
        let units: Vec<u16> =
            raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|u| *u != 0).collect();
        Ok(String::from_utf16_lossy(&units))
    }

    pub fn u32_array(&mut self) -> Result<Vec<u32>, PtpError> {
        let n = self.u32()? as usize;
        if n > self.remaining() / 4 {
            return Err(PtpError::Protocol(format!("array of {n} elements exceeds dataset")));
        }
        (0..n).map(|_| self.u32()).collect()
    }

    pub fn u16_array(&mut self) -> Result<Vec<u16>, PtpError> {
        let n = self.u32()? as usize;
        if n > self.remaining() / 2 {
            return Err(PtpError::Protocol(format!("array of {n} elements exceeds dataset")));
        }
        (0..n).map(|_| self.u16()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_roundtrip() {
        let c = Container::command(0x1001, 7, &[1, 2, 3]);
        let bytes = c.encode();
        assert_eq!(bytes.len(), 12 + 12);
        assert_eq!(Container::decode(&bytes).unwrap(), c);
        assert_eq!(c.params(), vec![1, 2, 3]);
    }

    #[test]
    fn string_roundtrip() {
        for s in ["", "Nikon", "Ünïcode ✓"] {
            let mut w = Writer::default();
            w.string(s);
            let b = w.finish();
            assert_eq!(Reader::new(&b).string().unwrap(), s);
        }
    }

    #[test]
    fn truncated_dataset_is_an_error() {
        assert!(Reader::new(&[5, 0]).string().is_err());
        assert!(Reader::new(&[255, 255, 255, 255]).u16_array().is_err());
    }
}
