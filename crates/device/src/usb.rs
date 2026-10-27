//! The USB transport (spec §7.3): the real cutter behind [`Transport`], over `nusb`. Opening
//! claims the printer-class interface and finds its bulk endpoints from the descriptors. On Linux
//! it first detaches the kernel printer driver (`usblp`) from that interface, which most
//! distributions bind to the Cameo; it is reattached when the transport is dropped. Every
//! transfer has a timeout; a timed-out write may have been partly delivered, so it is reported
//! as [`TransportError::Timeout`] (uncertain), never retried here.

use std::time::Duration;

use nusb::MaybeFuture;
use nusb::descriptors::TransferType;
use nusb::transfer::{Buffer, Bulk, Direction, In, Out, TransferError};
use serde::Serialize;

use crate::gpgl::Model;
use crate::transport::{Transport, TransportError};

/// Largest single OUT transfer; the driver writes whole commands, which are shorter.
const MAX_WRITE: usize = 4096;

/// Why the cutter could not be opened, in words they can act on (spec §5.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum OpenError {
    NotFound,
    Several { count: usize },
    PermissionDenied { detail: String },
    Busy { detail: String },
    NoBulkEndpoints,
    Other { detail: String },
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => {
                f.write_str("Can't see the Cameo. Check it's on and the USB cable is in.")
            }
            Self::Several { count } => write!(
                f,
                "{count} supported cutters are connected; connect only one for now."
            ),
            Self::PermissionDenied { detail } => write!(
                f,
                "This user may not open the cutter ({detail}). On Linux, install the udev rule that gives the logged-in user access to 0b4d:1121, then reconnect the cable."
            ),
            Self::Busy { detail } => write!(
                f,
                "Another app or a system driver may be using the cutter ({detail}). Close Silhouette Studio or Silhouette Connect, then try again."
            ),
            Self::NoBulkEndpoints => f.write_str(
                "The cutter's USB interface has no bulk endpoints; this is not a supported model.",
            ),
            Self::Other { detail } => write!(f, "The cutter could not be opened: {detail}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// What was found when opening: for the probe report and problem reports.
#[derive(Debug, Clone, Serialize)]
pub struct UsbInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial_number: Option<String>,
    pub speed: Option<String>,
    pub interface: u8,
    pub bulk_out: u8,
    pub bulk_in: u8,
    pub max_packet_out: usize,
    pub max_packet_in: usize,
    /// Other endpoints on the interface, by address and type, which CutCraft does not use.
    pub unused_endpoints: Vec<(u8, String)>,
}

pub struct UsbTransport {
    out: nusb::Endpoint<Bulk, Out>,
    input: nusb::Endpoint<Bulk, In>,
    /// Reply bytes received but not read yet.
    unread: Vec<u8>,
    pub info: UsbInfo,
}

fn open_error(error: nusb::Error) -> OpenError {
    let detail = error.to_string();
    match error.kind() {
        nusb::ErrorKind::PermissionDenied => OpenError::PermissionDenied { detail },
        nusb::ErrorKind::Busy => OpenError::Busy { detail },
        nusb::ErrorKind::NotFound | nusb::ErrorKind::Disconnected => OpenError::NotFound,
        _ => OpenError::Other { detail },
    }
}

fn transfer_error(error: TransferError) -> TransportError {
    match error {
        TransferError::Cancelled => TransportError::Timeout,
        TransferError::Stall => TransportError::Stalled,
        TransferError::Disconnected => TransportError::Disconnected,
        other => TransportError::Other(other.to_string()),
    }
}

impl UsbTransport {
    /// Open the one connected cutter of `model`. Never resets or reconfigures the device; on
    /// Linux it takes the interface from `usblp` for as long as the transport lives.
    pub fn open(model: &Model) -> Result<Self, OpenError> {
        let matching: Vec<_> = nusb::list_devices()
            .wait()
            .map_err(open_error)?
            .filter(|d| d.vendor_id() == model.vendor_id && d.product_id() == model.product_id)
            .collect();
        let info = match matching.as_slice() {
            [] => return Err(OpenError::NotFound),
            [one] => one.clone(),
            many => return Err(OpenError::Several { count: many.len() }),
        };
        let device = info.open().wait().map_err(open_error)?;
        // The printer-class interface with a bulk OUT and a bulk IN endpoint.
        let config = device
            .active_configuration()
            .map_err(|e| OpenError::Other {
                detail: e.to_string(),
            })?;
        let mut found = None;
        for interface in config.interfaces() {
            let Some(alt) = interface.alt_settings().next() else {
                continue;
            };
            let bulk = |dir: Direction| {
                alt.endpoints()
                    .find(|e| e.transfer_type() == TransferType::Bulk && e.direction() == dir)
                    .map(|e| (e.address(), e.max_packet_size()))
            };
            if let (Some(out), Some(input)) = (bulk(Direction::Out), bulk(Direction::In)) {
                let unused = alt
                    .endpoints()
                    .filter(|e| e.address() != out.0 && e.address() != input.0)
                    .map(|e| (e.address(), format!("{:?}", e.transfer_type())))
                    .collect();
                found = Some((alt.interface_number(), out, input, unused));
                break;
            }
        }
        let (number, (out_address, out_packet), (in_address, in_packet), unused) =
            found.ok_or(OpenError::NoBulkEndpoints)?;
        // Linux only detaches; elsewhere this is a plain claim.
        let interface = device
            .detach_and_claim_interface(number)
            .wait()
            .map_err(open_error)?;
        let out = interface
            .endpoint::<Bulk, Out>(out_address)
            .map_err(open_error)?;
        let input = interface
            .endpoint::<Bulk, In>(in_address)
            .map_err(open_error)?;
        Ok(Self {
            out,
            input,
            unread: Vec::new(),
            info: UsbInfo {
                vendor_id: info.vendor_id(),
                product_id: info.product_id(),
                serial_number: info.serial_number().map(str::to_owned),
                speed: device.speed().map(|s| format!("{s:?}")),
                interface: number,
                bulk_out: out_address,
                bulk_in: in_address,
                max_packet_out: out_packet,
                max_packet_in: in_packet,
                unused_endpoints: unused,
            },
        })
    }
}

impl Transport for UsbTransport {
    fn write(&mut self, data: &[u8], timeout: Duration) -> Result<usize, TransportError> {
        if data.is_empty() {
            return Ok(0);
        }
        let chunk = &data[..data.len().min(MAX_WRITE)];
        let completion = self
            .out
            .transfer_blocking(Buffer::from(chunk.to_vec()), timeout);
        match completion.status {
            Ok(()) => Ok(completion.actual_len),
            // A cancelled (timed-out) or failed write may have delivered some bytes: uncertain.
            Err(error) => Err(transfer_error(error)),
        }
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, TransportError> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.unread.is_empty() {
            let packet = self.input.max_packet_size().max(1);
            let completion = self.input.transfer_blocking(Buffer::new(packet), timeout);
            completion.status.map_err(transfer_error)?;
            let received = completion.buffer.into_vec();
            if received.is_empty() {
                return Err(TransportError::Timeout);
            }
            self.unread = received;
        }
        let n = buf.len().min(self.unread.len());
        buf[..n].copy_from_slice(&self.unread[..n]);
        self.unread.drain(..n);
        Ok(n)
    }

    fn clear_halt(&mut self) -> Result<(), TransportError> {
        self.out
            .clear_halt()
            .wait()
            .map_err(|e| TransportError::Other(e.to_string()))?;
        self.input
            .clear_halt()
            .wait()
            .map_err(|e| TransportError::Other(e.to_string()))
    }
}
