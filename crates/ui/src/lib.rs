//! The desktop layout's client half (spec §9.2, §13). This crate is growing toward the egui
//! window compiled to WebAssembly; it starts with the typed session model every version of that
//! window needs — the half whose absence replayed accepted keys and lost refused answers in the
//! first M4 attempt.

pub mod session;

pub use session::{Checkpoint, KeyCollision, Reconciled, Session};
