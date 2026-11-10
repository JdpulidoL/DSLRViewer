//! Picture Transfer Protocol (ISO 15740) implementation.

pub mod codec;
pub mod codes;
pub mod dataset;
pub mod transport;

use std::time::Duration;

pub use dataset::{DeviceInfo, PropDesc, Value};
pub use transport::{PtpEvent, PtpTransport, Reply};

use codes::{op, rc};

#[derive(Debug, thiserror::Error)]
pub enum PtpError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("device returned {} ({code:#06x}) for operation {op:#06x}", codes::rc::name(*code))]
    Response { op: u16, code: u16 },
    #[error("timed out waiting for the device")]
    Timeout,
    #[error("device disconnected")]
    Disconnected,
}

impl PtpError {
    /// Busy/not-ready style responses that callers may retry or skip.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Timeout)
            || matches!(self, Self::Response { code, .. } if [rc::DEVICE_BUSY, rc::NIKON_NOT_LIVE_VIEW, rc::CANON_NOT_READY].contains(code))
    }
}

/// An open PTP session over any transport.
pub struct Session {
    transport: Box<dyn PtpTransport>,
    pub info: DeviceInfo,
}

impl Session {
    /// Open a session and read the device information.
    pub fn open(mut transport: Box<dyn PtpTransport>) -> Result<Self, PtpError> {
        if !transport.manages_session() {
            let reply = transport.transact(op::OPEN_SESSION, &[1], None)?;
            if reply.code != rc::OK && reply.code != rc::SESSION_ALREADY_OPEN {
                return Err(PtpError::Response { op: op::OPEN_SESSION, code: reply.code });
            }
        }
        let reply = transport::expect_ok(op::GET_DEVICE_INFO, transport.transact(op::GET_DEVICE_INFO, &[], None)?)?;
        let info = DeviceInfo::parse(&reply.data)?;
        Ok(Self { transport, info })
    }

    pub fn describe(&self) -> String {
        self.transport.describe()
    }

    /// Execute an operation expecting `OK`.
    pub fn call(&mut self, opcode: u16, params: &[u32]) -> Result<Reply, PtpError> {
        transport::expect_ok(opcode, self.transport.transact(opcode, params, None)?)
    }

    pub fn call_with_data(&mut self, opcode: u16, params: &[u32], data: &[u8]) -> Result<Reply, PtpError> {
        transport::expect_ok(opcode, self.transport.transact(opcode, params, Some(data))?)
    }

    pub fn prop_desc(&mut self, prop: u16) -> Result<PropDesc, PtpError> {
        let reply = self.call(op::GET_DEVICE_PROP_DESC, &[prop as u32])?;
        PropDesc::parse(&reply.data)
    }

    pub fn set_prop(&mut self, prop: u16, value: &Value) -> Result<(), PtpError> {
        self.call_with_data(op::SET_DEVICE_PROP_VALUE, &[prop as u32], &value.to_bytes())?;
        Ok(())
    }

    pub fn poll_event(&mut self, timeout: Duration) -> Result<Option<PtpEvent>, PtpError> {
        self.transport.poll_event(timeout)
    }

    pub fn has_events(&self) -> bool {
        self.transport.has_events()
    }

    /// Every object handle on every store.
    pub fn object_handles(&mut self) -> Result<Vec<u32>, PtpError> {
        let reply = self.call(op::GET_OBJECT_HANDLES, &[0xFFFF_FFFF, 0, 0])?;
        codec::Reader::new(&reply.data).u32_array()
    }

    pub fn close(&mut self) {
        if !self.transport.manages_session() {
            let _ = self.transport.transact(op::CLOSE_SESSION, &[], None);
        }
    }
}
