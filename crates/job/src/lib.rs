//! Shared domain types: prepared designs, projects and sheets.
//!
//! This crate must remain independent of the UI, VectorCraft, planner and driver.

pub mod prepared;
pub mod project;
pub mod project_file;

pub use prepared::{
    Change, Colour, Contour, CutLine, Decision, Diagnostic, DxfUnit, FontUse, ImportRoute, Page,
    PendingDecision, Piece, PreparedDesign, Recipe, Severity, StrokeRole, TraceColours,
    TraceSettings,
};
pub use project::{
    CompletedSheet, CutRecord, GroupBy, MachineProfile, Mat, MatProfile, Material, OffsetMethod,
    Placement, Project, RowKey, RulerUnit, Sheet, Status,
};
pub use project_file::{EmbeddedSource, ProjectFile};
