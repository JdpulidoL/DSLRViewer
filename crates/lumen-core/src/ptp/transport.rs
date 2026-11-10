//! Transport abstractions.
//!
//! * [`PtpTransport`] executes whole PTP transactions and is what the rest of
//!   the application talks to.
//! * [`WireLink`] is a raw container pipe (USB bulk endpoints or an in-process
//!   simulator). [`ContainerTransport`] turns any `WireLink` into a
//!   `PtpTransport` by framing commands, data and responses.

use std::time::Duration;

use super::PtpError;
use super::codec::{Container, ContainerKind, HEADER_LEN};
use super::codes::{op, rc};

/// Result of one PTP transaction.
#[derive(Debug, Clone, Default)]
pub struct Reply {
    pub code: u16,
    /// Response parameters; part of the transaction model even though no current operation reads them.
    #[allow(dead_code)]
    pub params: Vec<u32>,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtpEvent {
    pub code: u16,
    pub params: Vec<u32>,
}

pub trait PtpTransport: Send {
    /// Run a full transaction: command, optional outbound data, inbound data
    /// and response. A non-OK response code is returned in `Reply`, not as an
    /// error; errors are reserved for transport failures.
    fn transact(&mut self, op: u16, params: &[u32], send: Option<&[u8]>) -> Result<Reply, PtpError>;

    /// Wait up to `timeout` for an asynchronous event.
    fn poll_event(&mut self, timeout: Duration) -> Result<Option<PtpEvent>, PtpError>;

    /// True if the lower layer already opens/closes the PTP session itself.
    fn manages_session(&self) -> bool {
        false
    }

    /// False when the lower layer cannot deliver PTP events; callers then
    /// discover new objects by listing object handles instead.
    fn has_events(&self) -> bool {
        true
    }

    fn describe(&self) -> String;
}

/// Raw container-level pipe to a device.
pub trait WireLink: Send {
    fn write(&mut self, data: &[u8]) -> Result<(), PtpError>;
    /// Read the next chunk from the bulk-in pipe. A chunk may hold less than a
    /// whole container; `ContainerTransport` reassembles.
    fn read(&mut self, timeout: Duration) -> Result<Vec<u8>, PtpError>;
    fn read_event(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, PtpError>;
    fn describe(&self) -> String;
}

/// Lets a boxed link (e.g. one a host platform registered) be used wherever a concrete one is.
impl WireLink for Box<dyn WireLink> {
    fn write(&mut self, data: &[u8]) -> Result<(), PtpError> {
        (**self).write(data)
    }
    fn read(&mut self, timeout: Duration) -> Result<Vec<u8>, PtpError> {
        (**self).read(timeout)
    }
    fn read_event(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, PtpError> {
        (**self).read_event(timeout)
    }
    fn describe(&self) -> String {
        (**self).describe()
    }
}

pub struct ContainerTransport<L: WireLink> {
    link: L,
    transaction: u32,
    io_timeout: Duration,
}

impl<L: WireLink> ContainerTransport<L> {
    pub fn new(link: L) -> Self {
        Self { link, transaction: 0, io_timeout: Duration::from_secs(5) }
    }

    fn read_container(&mut self) -> Result<Container, PtpError> {
        let mut buf = self.link.read(self.io_timeout)?;
        while buf.len() < HEADER_LEN {
            buf.extend(self.link.read(self.io_timeout)?);
        }
        let declared = Container::declared_len(&buf).unwrap_or(0);
        if declared != u32::MAX {
            let want = declared as usize;
            while buf.len() < want {
                buf.extend(self.link.read(self.io_timeout)?);
            }
            buf.truncate(want.max(HEADER_LEN));
        }
        Container::decode(&buf)
    }

    fn next_transaction(&mut self, opcode: u16) -> u32 {
        if opcode == op::OPEN_SESSION {
            // The spec requires transaction id 0 for OpenSession; the first
            // transaction inside the session is 1.
            self.transaction = 0;
        } else {
            self.transaction = self.transaction.wrapping_add(1).max(1);
        }
        self.transaction
    }
}

impl<L: WireLink> PtpTransport for ContainerTransport<L> {
    fn transact(&mut self, opcode: u16, params: &[u32], send: Option<&[u8]>) -> Result<Reply, PtpError> {
        let tid = self.next_transaction(opcode);
        self.link.write(&Container::command(opcode, tid, params).encode())?;
        if let Some(data) = send {
            self.link.write(&Container::new(ContainerKind::Data, opcode, tid, data.to_vec()).encode())?;
        }
        let mut data = Vec::new();
        loop {
            let c = self.read_container()?;
            if c.transaction != tid {
                return Err(PtpError::Protocol(format!(
                    "transaction mismatch: expected {tid}, got {} ({:?})",
                    c.transaction, c.kind
                )));
            }
            match c.kind {
                ContainerKind::Data => data.extend_from_slice(&c.payload),
                ContainerKind::Response => {
                    return Ok(Reply { code: c.code, params: c.params(), data });
                }
                ContainerKind::Event | ContainerKind::Command => {
                    return Err(PtpError::Protocol(format!("unexpected {:?} container in transaction", c.kind)));
                }
            }
        }
    }

    fn poll_event(&mut self, timeout: Duration) -> Result<Option<PtpEvent>, PtpError> {
        match self.link.read_event(timeout)? {
            Some(bytes) => {
                let c = Container::decode(&bytes)?;
                Ok(Some(PtpEvent { code: c.code, params: c.params() }))
            }
            None => Ok(None),
        }
    }

    fn describe(&self) -> String {
        self.link.describe()
    }
}

/// Convert a non-OK reply into a typed error.
pub fn expect_ok(op: u16, reply: Reply) -> Result<Reply, PtpError> {
    if reply.code == rc::OK { Ok(reply) } else { Err(PtpError::Response { op, code: reply.code }) }
}
