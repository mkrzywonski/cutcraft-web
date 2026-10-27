//! cameo-pi's core: cutters, their mats, turns and queues, and the workers that own them.
//! Everything that decides whether a cutter may move lives here, shared by the web interface,
//! the protocol and MCP (spec §5).

pub mod adjust;
pub mod backup;
pub mod cutter;
#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;
pub mod journal;
pub mod library;
pub mod mat;
pub mod runnable;
pub mod update;
pub mod validate;
pub mod worker;
