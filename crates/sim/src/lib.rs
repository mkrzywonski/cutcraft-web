//! A simulated original Silhouette Cameo (spec §7.4).
//!
//! The simulator sits behind [`cutcraft_device::transport::Transport`], so a driver cannot tell
//! it from the real machine. It parses the GP-GL a driver sends, models loaded media, tool
//! position, settings, a bounded buffer and timed execution, scripts faults, and records every
//! move for the virtual mat.
//!
//! Its behaviour is a model of public descriptions (spec §7.3), not of the machine itself:
//! [`profile::CAMEO_1_ASSUMPTIONS`] lists each belief and whether it is a hypothesis, observed
//! or measured. A passing simulation shows that CutCraft sends what it intends; it does not show
//! that the Cameo behaves as modelled.
//!
//! Clean room: written from the public behaviour summarised in the spec. No code from GPL
//! projects (inkscape-silhouette and others) is used.

pub mod harness;
pub mod link;
pub mod machine;
pub mod mat;
pub mod parity;
pub mod profile;
pub mod protocol;

pub use link::{Clock, Scenario, SimTransport, Simulator, Trigger};
pub use machine::{Fault, FaultKind, Machine, Segment, Snapshot, Status};
pub use mat::{Report, SvgOptions};
pub use parity::ParityReport;
pub use profile::{Media, Profile, Source};
