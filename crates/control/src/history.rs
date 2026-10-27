//! Undo history and the session journal (spec §6.2.1). Every edit that changes the open design
//! is one undo step labelled with its command and origin; an atomic batch is one step. The
//! journal records each command's id, parameters, origin, revisions and outcome, without
//! artwork and with file paths reduced to their names.

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;

use crate::{OpenDesign, Origin};

/// Steps kept for undo.
pub const MAX_STEPS: usize = 100;
/// Journal entries kept.
pub const MAX_JOURNAL: usize = 1000;

#[derive(Clone, Serialize)]
pub struct HistoryStep {
    pub label: String,
    pub command: String,
    pub origin: Origin,
    /// The design to restore when this step is undone (or redone).
    #[serde(skip)]
    pub design: Arc<OpenDesign>,
}

#[derive(Clone, Default, Serialize)]
pub struct History {
    pub undo: Vec<HistoryStep>,
    pub redo: Vec<HistoryStep>,
}

impl History {
    pub(crate) fn push(&mut self, step: HistoryStep) {
        self.undo.push(step);
        if self.undo.len() > MAX_STEPS {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub(crate) fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct JournalEntry {
    pub seq: u64,
    pub id: String,
    pub origin: Origin,
    pub params: Value,
    pub revision_before: u64,
    pub revision_after: u64,
    pub ok: bool,
    pub code: Option<&'static str>,
}

/// Parameters as journalled: paths become file names (spec §6.2.1 keeps paths out).
pub(crate) fn redact(params: &Value) -> Value {
    match params {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let value = match (key.as_str(), value) {
                        ("path" | "save_to", Value::String(path)) => Value::String(
                            std::path::Path::new(path)
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        ),
                        _ => redact(value),
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        other => other.clone(),
    }
}
