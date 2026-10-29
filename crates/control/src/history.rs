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

/// Durable session history. Source bytes are deduplicated by fingerprint; these records are
/// private worker checkpoints, never part of the browser snapshot or a library project.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct HistoryArchive {
    pub sources: std::collections::BTreeMap<String, crate::job::EmbeddedSource>,
    pub undo: Vec<ArchivedStep>,
    pub redo: Vec<ArchivedStep>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ArchivedStep {
    pub label: String,
    pub command: String,
    pub origin: Origin,
    pub source: String,
    pub project: crate::job::Project,
    pub inspection: Value,
    pub answered_by: Vec<crate::Answered>,
}
impl crate::Controller {
    pub fn archive_history(&self) -> Result<HistoryArchive, String> {
        let s = self.snapshot()?;
        let mut archive = HistoryArchive::default();
        let mut pack = |steps: Vec<HistoryStep>| {
            steps
                .into_iter()
                .map(|step| {
                    let source = crate::job::EmbeddedSource::new(
                        std::path::Path::new(&step.design.path),
                        &step.design.bytes,
                    );
                    let hash = source.sha256.clone();
                    archive.sources.insert(hash.clone(), source);
                    ArchivedStep {
                        label: step.label,
                        command: step.command,
                        origin: step.origin,
                        source: hash,
                        project: step.design.project.clone(),
                        inspection: step.design.inspection.clone(),
                        answered_by: step.design.answered_by.clone(),
                    }
                })
                .collect()
        };
        archive.undo = pack(s.history.undo);
        archive.redo = pack(s.history.redo);
        Ok(archive)
    }
    pub fn restore_history(&self, archive: HistoryArchive) -> Result<(), String> {
        let unpack = |steps: Vec<ArchivedStep>| -> Result<Vec<HistoryStep>, String> {
            if steps.len() > MAX_STEPS {
                return Err("Session history exceeds its limit.".into());
            }
            steps
                .into_iter()
                .map(|step| {
                    let source = archive
                        .sources
                        .get(&step.source)
                        .ok_or("History source is missing.")?;
                    let bytes = source.bytes()?;
                    let prepared =
                        serde_json::to_value(&step.project.prepared).map_err(|e| e.to_string())?;
                    Ok(HistoryStep {
                        label: step.label,
                        command: step.command,
                        origin: step.origin,
                        design: Arc::new(OpenDesign {
                            path: source.original_path.clone(),
                            project_path: None,
                            inspection: step.inspection,
                            recipe: step.project.prepared.recipe.clone(),
                            prepared,
                            project: step.project,
                            bytes: Arc::new(bytes),
                            answered_by: step.answered_by,
                        }),
                    })
                })
                .collect()
        };
        let history = History {
            undo: unpack(archive.undo)?,
            redo: unpack(archive.redo)?,
        };
        self.change(|s| s.history = history)
    }
}
