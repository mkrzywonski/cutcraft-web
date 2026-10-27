//! Execution records (spec §8.7): written and synced before the first byte reaches a cutter, and
//! completed with the outcome afterwards. At start-up any record without an outcome is marked
//! interrupted and never replayed, so a crash can turn a finished cut into "uncertain" but never
//! into a second cut.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cutter::{Answer, Ended, Execution, JobRef, Kind};

/// One execution, as kept on disk.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// Earlier imported records had no version field; their fields are schema 1.
    #[serde(default = "record_schema")]
    pub schema_version: u32,
    pub execution: String,
    /// The cutter's stable id.
    pub cutter: String,
    pub turn: String,
    pub job_id: String,
    pub revision: u32,
    pub kind: String,
    pub key: String,
    pub fingerprint: String,
    pub user: Option<String>,
    pub client: String,
    pub digest: String,
    /// Seconds since the Unix epoch.
    pub started: u64,
    /// `None` until the execution ends.
    pub outcome: Option<serde_json::Value>,
}

fn record_schema() -> u32 {
    1
}

impl Record {
    pub fn job(&self) -> JobRef {
        JobRef {
            job_id: self.job_id.clone(),
            revision: self.revision,
        }
    }

    /// The answer the original Start got, for its idempotency key.
    pub fn answer(&self) -> Answer {
        Answer::Started {
            execution: self.execution.clone(),
            turn: self.turn.clone(),
        }
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Numbers each temporary file this process makes.
static TEMPORARY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Writes `bytes` to `path` so that a crash leaves either the old file or the new one: a
/// temporary file beside it, synced, renamed over, then the folder synced (spec §7.2).
///
/// Each write has its own temporary file, so two writes to one path at once can't trample each
/// other's: the last rename wins whole. That makes the write atomic, not a read-modify-write
/// safe: callers that read, change and write a file serialise that themselves.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::sync::atomic::Ordering;
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    // A fresh name each time; one left by an earlier process (same pid after a restart) is
    // skipped, never written over or removed.
    let (temporary, mut file) = loop {
        let n = TEMPORARY.fetch_add(1, Ordering::Relaxed);
        let temporary = dir.join(format!(".{name}.{}.{n}.tmp", std::process::id()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => break (temporary, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    // From here the temporary file is this write's own, removed if anything fails.
    let written = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written?;
    fs::File::open(dir)?.sync_all()
}

/// The folder of execution records.
pub struct Journal {
    dir: PathBuf,
    transaction: std::sync::Mutex<()>,
}

impl Journal {
    /// Opens (creating) the folder. Returns the journal and every record found; records left
    /// without an outcome by a crash or power cut are completed as interrupted first.
    pub fn open(dir: impl Into<PathBuf>) -> std::io::Result<(Self, Vec<Record>)> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let journal = Self {
            dir,
            transaction: std::sync::Mutex::new(()),
        };
        let mut records = Vec::new();
        for entry in fs::read_dir(&journal.dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".tmp"))
                {
                    fs::remove_file(&path)?;
                }
                continue;
            }
            let text = fs::read_to_string(&path)?;
            let mut record: Record = serde_json::from_str(&text).map_err(std::io::Error::other)?;
            if journal.path(&record.execution) != path {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "execution record identity does not match its file",
                ));
            }
            if record.schema_version != record_schema() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unsupported execution record schema",
                ));
            }
            if record.outcome.is_none() {
                let ended = Ended::Interrupted {
                    reason: "cameo-pi stopped while this was running; what was cut is uncertain"
                        .into(),
                };
                record.outcome = Some(serde_json::to_value(&ended).unwrap_or_default());
                journal.write(&record)?;
            }
            records.push(record);
        }
        records.sort_by_key(|r| r.started);
        Ok((journal, records))
    }

    /// Deterministic storage-stall seam for simulator fixtures, absent from production builds.
    #[cfg(feature = "fixtures")]
    pub fn hold_writes_for_fixture(&self) -> std::sync::MutexGuard<'_, ()> {
        self.transaction.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn path(&self, execution: &str) -> PathBuf {
        // Ids are generated here ("exec-<boot>-<n>"), never taken from a client.
        let safe: String = execution
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        self.dir.join(format!("{safe}.json"))
    }

    fn write(&self, record: &Record) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(record).map_err(std::io::Error::other)?;
        write_atomic(&self.path(&record.execution), &bytes)
    }

    /// Records an execution before anything is sent. If this fails, nothing may be sent.
    pub fn started(&self, cutter: &str, execution: &Execution) -> std::io::Result<()> {
        let _transaction = self.transaction.lock().unwrap_or_else(|p| p.into_inner());
        match fs::read(self.path(&execution.id)) {
            Ok(bytes) => {
                let first: Record =
                    serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
                if first.cutter == cutter
                    && first.execution == execution.id
                    && first.key == execution.key
                    && first.fingerprint == execution.fingerprint
                    && first.digest == execution.run.digest
                    && first.turn == execution.turn
                    && first.job_id == execution.job.job_id
                    && first.revision == execution.job.revision
                    && first.user == execution.actor.user
                    && first.client == execution.actor.client
                    && first.schema_version == record_schema()
                    && first.outcome.is_none()
                {
                    return Ok(());
                }
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "an execution with this id was already recorded differently or ended",
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        self.write(&Record {
            schema_version: record_schema(),
            execution: execution.id.clone(),
            cutter: cutter.to_owned(),
            turn: execution.turn.clone(),
            job_id: execution.job.job_id.clone(),
            revision: execution.job.revision,
            kind: match execution.kind {
                Kind::Test => "test".into(),
                Kind::Main => "main".into(),
            },
            key: execution.key.clone(),
            fingerprint: execution.fingerprint.clone(),
            user: execution.actor.user.clone(),
            client: execution.actor.client.clone(),
            digest: execution.run.digest.clone(),
            started: now(),
            outcome: None,
        })
    }

    /// Completes a record with how the execution ended.
    pub fn ended(&self, execution: &str, ended: &Ended) -> std::io::Result<()> {
        let _transaction = self.transaction.lock().unwrap_or_else(|p| p.into_inner());
        let text = fs::read_to_string(self.path(execution))?;
        let mut record: Record = serde_json::from_str(&text).map_err(std::io::Error::other)?;
        if record.outcome.is_some() {
            return Ok(());
        }
        record.outcome = Some(serde_json::to_value(ended).map_err(std::io::Error::other)?);
        self.write(&record)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_stale_temporary_file_is_skipped_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.json");
        // Names a previous process with this pid could have left: the next 256 this one would use
        // (other tests write meanwhile, so more than one).
        let next = TEMPORARY.load(std::sync::atomic::Ordering::Relaxed);
        for k in next..next + 256 {
            let stale = format!(".file.json.{}.{k}.tmp", std::process::id());
            std::fs::write(dir.path().join(stale), b"stale").unwrap();
        }
        write_atomic(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let stale = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(stale, 256, "stale files are skipped and left alone");
    }

    #[test]
    fn simultaneous_writes_to_one_path_each_land_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.json");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(16));
        let writers: Vec<_> = (0..16)
            .map(|i| {
                let (path, barrier) = (path.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    write_atomic(&path, format!("writer {i:02}").as_bytes())
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap().expect("every write succeeds");
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("writer ") && text.len() == 9, "{text}");
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left.len(), 1, "no temporary files left: {left:?}");
    }

    use std::sync::Arc;

    use cutcraft_device::driver::Controls;

    use super::*;
    use crate::cutter::Actor;
    use crate::runnable::tests::runnable_at;

    fn execution(id: &str) -> Execution {
        Execution {
            id: id.into(),
            kind: Kind::Main,
            turn: "turn-b-1".into(),
            job: JobRef {
                job_id: "job".into(),
                revision: 2,
            },
            key: "key-1".into(),
            fingerprint: "fp".into(),
            actor: Actor {
                user: Some("sam".into()),
                client: "phone".into(),
            },
            run: runnable_at(0.0),
            controls: Arc::new(Controls::default()),
        }
    }

    #[test]
    fn a_record_without_an_outcome_is_interrupted_at_start_up_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (journal, records) = Journal::open(dir.path()).unwrap();
        assert!(records.is_empty());
        journal
            .started("Workshop Cameo", &execution("exec-b-1"))
            .unwrap();
        journal
            .started("Workshop Cameo", &execution("exec-b-2"))
            .unwrap();
        journal.ended("exec-b-2", &Ended::Finished).unwrap();
        drop(journal);

        // "Restart".
        let (_, records) = Journal::open(dir.path()).unwrap();
        assert_eq!(records.len(), 2);
        let crashed = records.iter().find(|r| r.execution == "exec-b-1").unwrap();
        assert_eq!(crashed.outcome.as_ref().unwrap()["ended"], "interrupted");
        let done = records.iter().find(|r| r.execution == "exec-b-2").unwrap();
        assert_eq!(done.outcome.as_ref().unwrap()["ended"], "finished");
        assert_eq!(crashed.key, "key-1");
        assert_eq!(
            crashed.answer(),
            Answer::Started {
                execution: "exec-b-1".into(),
                turn: "turn-b-1".into()
            }
        );
    }

    #[test]
    fn leftover_temporary_files_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".exec-x.json.1.tmp"), b"partial").unwrap();
        let (_, records) = Journal::open(dir.path()).unwrap();
        assert!(records.is_empty());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
    #[test]
    fn recording_twice_is_idempotent_and_never_resurrects_an_ended_execution() {
        let dir = tempfile::tempdir().unwrap();
        let (journal, _) = Journal::open(dir.path()).unwrap();
        let execution = execution("exec-once");
        journal.started("cutter", &execution).unwrap();
        let first = fs::read(journal.path(&execution.id)).unwrap();
        journal.started("cutter", &execution).unwrap();
        assert_eq!(fs::read(journal.path(&execution.id)).unwrap(), first);
        journal.ended(&execution.id, &Ended::Finished).unwrap();
        assert!(journal.started("cutter", &execution).is_err());
        journal.ended(&execution.id, &Ended::Stopped).unwrap();
        let (_, records) = Journal::open(dir.path()).unwrap();
        assert_eq!(records[0].outcome.as_ref().unwrap()["ended"], "finished");
    }

    #[test]
    fn legacy_execution_records_remain_readable_and_damaged_records_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let (journal, _) = Journal::open(dir.path()).unwrap();
        let execution = execution("exec-legacy");
        journal.started("cutter", &execution).unwrap();
        let path = journal.path(&execution.id);
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        legacy.as_object_mut().unwrap().remove("schema_version");
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let (_, records) = Journal::open(dir.path()).unwrap();
        assert_eq!(records[0].schema_version, 1);
        assert_eq!(records[0].outcome.as_ref().unwrap()["ended"], "interrupted");
        fs::write(&path, b"damaged").unwrap();
        assert!(Journal::open(dir.path()).is_err());
        assert_eq!(fs::read(path).unwrap(), b"damaged");
    }
}
