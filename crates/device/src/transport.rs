//! The byte transport between the driver and a cutter.
//!
//! The USB transport (`nusb`, not implemented yet) and the Cameo simulator (`cutcraft-sim`) both
//! implement [`Transport`], so the driver cannot tell them apart. Calls block for at most their
//! timeout. A write may deliver part of its data: the returned count is what the device accepted.

use std::fmt;
use std::time::Duration;

/// Why a transfer failed. Delivery of the data involved is unknown unless stated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// Nothing completed within the timeout. For a write, some or all of the data may still have
    /// been delivered: the stream position is uncertain.
    Timeout,
    /// The device is gone (unplugged, powered off, or this handle predates a reconnection).
    Disconnected,
    /// The endpoint is halted; nothing more transfers until [`Transport::clear_halt`].
    Stalled,
    /// Any other transport failure.
    Other(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("transfer timed out; delivery is uncertain"),
            Self::Disconnected => f.write_str("the cutter is disconnected"),
            Self::Stalled => f.write_str("the endpoint is stalled"),
            Self::Other(detail) => write!(f, "transport error: {detail}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// A bidirectional byte stream to one cutter.
pub trait Transport {
    /// Writes from the start of `data`, returning how many bytes the device accepted (at least
    /// one). Waits up to `timeout` for buffer space.
    fn write(&mut self, data: &[u8], timeout: Duration) -> Result<usize, TransportError>;

    /// Reads available reply bytes into `buf`, returning how many (at least one). Waits up to
    /// `timeout` for a reply.
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, TransportError>;

    /// Clears a halted endpoint after [`TransportError::Stalled`].
    fn clear_halt(&mut self) -> Result<(), TransportError>;
}
