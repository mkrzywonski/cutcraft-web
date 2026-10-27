//! A minimal sender for exercising the simulator from tests and the CLI.
//!
//! This is **not** the driver. It writes a stream in chunks and polls status; it has none of the
//! driver's plan validation, checkpoints, Pause/Stop or recovery (spec §5.5, §7.3).

use std::time::Duration;

use cutcraft_device::transport::{Transport, TransportError};
use serde::Serialize;

use crate::machine::Status;
use crate::protocol::{ESC, ETX};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SendOutcome {
    /// Bytes the transport confirmed as accepted. After an error more may have arrived.
    pub confirmed: usize,
    pub writes: usize,
    pub error: Option<String>,
}

/// Writes `data` in chunks of at most `chunk` bytes, stopping at the first error.
pub fn send(
    transport: &mut impl Transport,
    data: &[u8],
    chunk: usize,
    timeout: Duration,
) -> SendOutcome {
    let chunk = chunk.max(1);
    let mut outcome = SendOutcome {
        confirmed: 0,
        writes: 0,
        error: None,
    };
    while outcome.confirmed < data.len() {
        let end = (outcome.confirmed + chunk).min(data.len());
        outcome.writes += 1;
        match transport.write(&data[outcome.confirmed..end], timeout) {
            Ok(0) => {
                outcome.error = Some("the device accepted nothing".to_owned());
                break;
            }
            Ok(count) => outcome.confirmed += count,
            Err(error) => {
                outcome.error = Some(error.to_string());
                break;
            }
        }
    }
    outcome
}

/// Sends ESC ENQ and reads one reply.
pub fn query_status(
    transport: &mut impl Transport,
    timeout: Duration,
) -> Result<Status, TransportError> {
    transport.write(&[ESC, 0x05], timeout)?;
    let reply = read_reply(transport, timeout)?;
    match reply.as_slice() {
        [byte] => Status::from_reply_byte(*byte),
        _ => None,
    }
    .ok_or_else(|| TransportError::Other(format!("unexpected status reply {reply:?}")))
}

/// Reads bytes up to and excluding ETX.
pub fn read_reply(
    transport: &mut impl Transport,
    timeout: Duration,
) -> Result<Vec<u8>, TransportError> {
    let mut reply = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        transport.read(&mut byte, timeout)?;
        if byte[0] == ETX {
            return Ok(reply);
        }
        if reply.len() >= 256 {
            return Err(TransportError::Other("reply without ETX".to_owned()));
        }
        reply.push(byte[0]);
    }
}

/// Polls status every `poll` until it is no longer "moving", for at most `limit`. Ready must
/// hold for the driver's `READY_HOLD_S`, as the Cameo also answers Ready while starting up.
/// `wait` passes time between polls (advance a virtual clock, or sleep).
pub fn wait_until_stopped(
    transport: &mut impl Transport,
    poll: Duration,
    limit: Duration,
    mut wait: impl FnMut(Duration),
) -> Result<Status, TransportError> {
    let hold = Duration::from_secs_f64(cutcraft_device::driver::READY_HOLD_S);
    let mut waited = Duration::ZERO;
    let mut still = Duration::ZERO;
    loop {
        let status = query_status(transport, poll.max(Duration::from_millis(100)))?;
        match status {
            Status::Moving => still = Duration::ZERO,
            Status::Ready if still < hold => {}
            _ => return Ok(status),
        }
        if waited >= limit {
            return Err(TransportError::Timeout);
        }
        wait(poll);
        waited += poll;
        if status != Status::Moving {
            still += poll;
        }
    }
}
