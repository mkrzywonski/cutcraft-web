//! Durable queue snapshots and accepted Queue receipts, separate from the library and sessions.
//! A dedicated publication transaction rejects late snapshots from the same boot. It never holds
//! a cutter lock during I/O, and Stop never takes this transaction.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use cameo_pi_core::cutter::QueueState;
use cameo_pi_core::journal::write_atomic;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::service::{ApiError, Result};

#[derive(Serialize, Deserialize)]
struct Snapshot {
    schema_version: u32,
    cutter: String,
    boot: String,
    state: QueueState,
}

pub struct Store {
    dir: PathBuf,
    published: Mutex<HashMap<String, u64>>,
}

fn storage(e: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        503,
        "queue-unconfirmed",
        format!("The queue could not be recorded safely: {e}"),
    )
}

impl Store {
    pub fn open(dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir).map_err(storage)?;
        if let Some(parent) = dir.parent() {
            std::fs::File::open(parent)
                .and_then(|f| f.sync_all())
                .map_err(storage)?;
        }
        Ok(Self {
            dir,
            published: Mutex::new(HashMap::new()),
        })
    }

    fn path(&self, cutter: &str) -> PathBuf {
        self.dir
            .join(format!("{:x}.json", Sha256::digest(cutter.as_bytes())))
    }

    pub fn load(&self, cutter: &str) -> Result<Option<QueueState>> {
        let path = self.path(cutter);
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(storage(e)),
        };
        let record: Snapshot = serde_json::from_slice(&bytes).map_err(storage)?;
        if record.schema_version != 1 || record.cutter != cutter {
            return Err(storage("unsupported or damaged queue snapshot"));
        }
        Ok(Some(record.state))
    }

    pub fn save(&self, cutter: &str, boot: &str, state: QueueState) -> Result<()> {
        let mut published = self.published.lock().unwrap_or_else(|p| p.into_inner());
        if published
            .get(cutter)
            .is_some_and(|sequence| *sequence >= state.sequence)
        {
            return Ok(());
        }
        let sequence = state.sequence;
        let bytes = serde_json::to_vec(&Snapshot {
            schema_version: 1,
            cutter: cutter.into(),
            boot: boot.into(),
            state,
        })
        .map_err(storage)?;
        write_atomic(&self.path(cutter), &bytes).map_err(storage)?;
        published.insert(cutter.into(), sequence);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cameo_pi_core::cutter::{Actor, Cutter, JobRef};

    #[test]
    fn late_snapshots_cannot_overwrite_a_newer_queue() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("queues")).unwrap();
        let mut cutter = Cutter::new("test", "Test cutter", "boot");
        let actor = Actor {
            user: None,
            client: "test".into(),
        };
        cutter
            .queue(
                JobRef {
                    job_id: "job".into(),
                    revision: 1,
                },
                actor.clone(),
                "one",
            )
            .unwrap();
        let old = cutter.queue_state();
        cutter
            .queue(
                JobRef {
                    job_id: "job".into(),
                    revision: 1,
                },
                actor,
                "two",
            )
            .unwrap();
        store.save("test", "boot", cutter.queue_state()).unwrap();
        store.save("test", "boot", old).unwrap();
        assert_eq!(store.load("test").unwrap().unwrap().waiting.len(), 1);
    }
}
