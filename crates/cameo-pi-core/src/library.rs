//! The job library (spec §7): plain folders on disk, one folder per job, with immutable
//! revisions and changeable details.
//!
//! ```text
//! library/<folders…>/<job name>/job.json
//!                               revisions/<n>/revision.json, project.cutcraft, plan.json, preview.svg
//! ```
//!
//! Writes are staged outside the library, synced, and published with a rename (spec §7.2), so a
//! crash never leaves half a job. Whether a job may be moved or deleted while it is queued or
//! cutting is the server's decision: it knows the queues.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::journal::write_atomic;

pub const DETAILS_SCHEMA: u32 = 1;
pub const REVISION_SCHEMA: u32 = 1;
/// History entries kept in `job.json`; the full record is the cut log (spec §7.3).
pub const HISTORY_KEPT: usize = 50;
const MAX_NAME: usize = 100;

#[derive(Debug, PartialEq)]
pub enum LibraryError {
    BadName(String),
    NotFound,
    /// Someone else changed the job meanwhile: here is the current version (spec §7.4).
    Conflict(Box<JobDetails>),
    FolderNotEmpty,
    Exists,
    /// A folder can't go inside itself or a folder within it.
    IntoItself,
    Io(String),
}

impl std::fmt::Display for LibraryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadName(why) => write!(f, "That name can't be used: {why}."),
            Self::NotFound => f.write_str("Not found in the library."),
            Self::Conflict(current) => write!(
                f,
                "This job was changed by {} meanwhile; here is the current version.",
                current.changed_by
            ),
            Self::FolderNotEmpty => f.write_str("Only an empty folder can be deleted."),
            Self::Exists => f.write_str("Something with that name is already there."),
            Self::IntoItself => f.write_str("A folder can't go inside itself."),
            Self::Io(e) => write!(f, "The library couldn't be changed: {e}"),
        }
    }
}

impl From<std::io::Error> for LibraryError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

type Result<T> = std::result::Result<T, LibraryError>;

/// A folder or job name a person typed (spec §14.3): no separators, no `..`, no control
/// characters, no leading dot, not too long.
/// Whether a top-level name is the one kept for Recently deleted.
fn reserved(parent: &[String], name: &str) -> bool {
    parent.is_empty() && name.eq_ignore_ascii_case(RECENTLY_DELETED)
}

pub fn check_name(name: &str) -> Result<()> {
    let bad = |why: &str| Err(LibraryError::BadName(why.into()));
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return bad("it is empty");
    }
    if trimmed != name {
        return bad("it starts or ends with a space");
    }
    if name.chars().count() > MAX_NAME {
        return bad("it is too long");
    }
    if name.starts_with('.') {
        return bad("it starts with a dot");
    }
    if name.ends_with('.') {
        return bad("it ends with a dot");
    }
    if name.chars().any(|c| {
        c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
    }) {
        return bad("it contains / \\ : * ? \" < > | or a control character");
    }
    Ok(())
}

/// One cut or test cut, as a job remembers it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub at: u64,
    pub execution: String,
    pub cutter: String,
    pub revision: u32,
    pub kind: String,
    pub by: String,
    /// "finished", "stopped", "interrupted" or "not-run".
    pub ended: String,
}

/// A job's changeable details: `job.json` (spec §7.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobDetails {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub notes: String,
    pub current_revision: u32,
    pub created_at: u64,
    pub created_by: String,
    pub last_cut: Option<u64>,
    pub times_cut: u32,
    pub history: Vec<HistoryEntry>,
    /// Increases with every change, for conflict checks.
    pub details_revision: u64,
    pub changed_by: String,
}

/// What was sent, for one revision: `revision.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RevisionMeta {
    pub schema_version: u32,
    pub revision: u32,
    pub created_at: u64,
    pub created_by: String,
    pub model: String,
    pub material: String,
    pub mat: String,
    pub size_mm: Option<[f64; 2]>,
    pub estimate_s: f64,
    /// The execution digest computed on the Pi (spec §10).
    pub digest: String,
    /// The CutCraft build that planned it.
    pub planner: String,
    /// Made on cameo-pi by adjusting this revision (Adjust, spec §12.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjusted_from: Option<u32>,
}

/// The immutable files of one revision.
pub struct RevisionFiles<'a> {
    pub project: &'a [u8],
    pub plan: &'a [u8],
    pub preview_svg: &'a [u8],
}

/// A job in a listing.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JobSummary {
    pub id: String,
    pub name: String,
    pub notes: String,
    pub last_cut: Option<u64>,
    pub times_cut: u32,
    pub current_revision: u32,
    pub created_at: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Listing {
    pub folders: Vec<String>,
    pub jobs: Vec<JobSummary>,
}

/// How long deleted jobs are kept (spec §7.4).
pub const KEEP_DELETED_S: u64 = 30 * 86_400;
/// The top-level folder that shows deleted jobs. It isn't a real folder, and the name is kept
/// for it.
pub const RECENTLY_DELETED: &str = "Recently deleted";

/// `deleted.json` in a deleted job: where it was and when it went.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct DeletedFrom {
    folder: Vec<String>,
    at: u64,
}

/// Something in Recently deleted: a job, or the whole library a restore replaced.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DeletedItem {
    /// Its name in Recently deleted, for restoring or deleting it.
    pub entry: String,
    /// `job` or `library`.
    pub kind: &'static str,
    pub id: Option<String>,
    pub name: String,
    pub notes: String,
    /// Where it was (empty: the top, or not known).
    pub folder: Vec<String>,
    pub current_revision: Option<u32>,
    pub deleted_at: u64,
    /// When it's deleted for good.
    pub expires_at: u64,
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub(crate) fn sync_dir(dir: &Path) -> std::io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

pub(crate) fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

pub(crate) fn json<T: Serialize>(value: &T) -> std::io::Result<Vec<u8>> {
    serde_json::to_vec_pretty(value).map_err(std::io::Error::other)
}

pub struct Library {
    pub(crate) root: PathBuf,
    pub(crate) staging: PathBuf,
    pub(crate) deleted: PathBuf,
    /// Job id → its folder. Rebuilt at start-up.
    pub(crate) index: Mutex<HashMap<String, PathBuf>>,
    /// The library write lock (spec §15): one change at a time. Never held by cutter workers.
    pub(crate) write: Mutex<u64>,
    /// How many changes so far, and when the last one began (seconds): a backup on every change
    /// watches them.
    changes: std::sync::atomic::AtomicU64,
    changed_at: std::sync::atomic::AtomicU64,
}

impl Library {
    /// Opens the library under `data`: `data/library`, with `data/.staging` and
    /// `data/deleted` beside it. Leftover staging is removed, and a revision that `job.json`
    /// doesn't mention yet is completed if whole, otherwise removed (spec §7.2).
    pub fn open(data: &Path) -> Result<Self> {
        let library = Self {
            root: data.join("library"),
            staging: data.join(".staging"),
            deleted: data.join("deleted"),
            index: Mutex::new(HashMap::new()),
            write: Mutex::new(0),
            changes: std::sync::atomic::AtomicU64::new(0),
            changed_at: std::sync::atomic::AtomicU64::new(0),
        };
        fs::create_dir_all(&library.root)?;
        fs::create_dir_all(&library.deleted)?;
        if library.staging.exists() {
            fs::remove_dir_all(&library.staging)?;
        }
        fs::create_dir_all(&library.staging)?;
        let mut index = HashMap::new();
        library.scan(&library.root.clone(), &mut index)?;
        *library.index.lock().unwrap_or_else(|p| p.into_inner()) = index;
        Ok(library)
    }

    pub(crate) fn scan(&self, dir: &Path, index: &mut HashMap<String, PathBuf>) -> Result<()> {
        if dir.join("job.json").is_file() {
            if let Ok(mut details) = read_details(dir) {
                self.repair(dir, &mut details)?;
                index.insert(details.id.clone(), dir.to_path_buf());
            }
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            // Never follow links out of the library (spec §14.3).
            if entry.file_type()?.is_dir() {
                self.scan(&entry.path(), index)?;
            }
        }
        Ok(())
    }

    /// Completes or removes revisions newer than `job.json` knows about (a crash between
    /// publishing a revision and updating the details).
    fn repair(&self, job: &Path, details: &mut JobDetails) -> Result<()> {
        let revisions = job.join("revisions");
        let Ok(entries) = fs::read_dir(&revisions) else {
            return Ok(());
        };
        let mut changed = false;
        for entry in entries.flatten() {
            let Some(n) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            if n <= details.current_revision {
                continue;
            }
            if revision_complete(&entry.path()) && n == details.current_revision + 1 {
                details.current_revision = n;
                changed = true;
            } else {
                fs::remove_dir_all(entry.path())?;
            }
        }
        if changed {
            write_atomic(&job.join("job.json"), &json(details)?)?;
        }
        Ok(())
    }

    /// The write lock, for a change: every change counts (see [`Library::changes`]).
    pub(crate) fn lock_write(&self) -> std::sync::MutexGuard<'_, u64> {
        let guard = self.write.lock().unwrap_or_else(|p| p.into_inner());
        self.changes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.changed_at
            .store(now(), std::sync::atomic::Ordering::SeqCst);
        guard
    }

    /// How many changes have begun since start-up, and when the last one did (seconds).
    pub fn changes(&self) -> (u64, u64) {
        (
            self.changes.load(std::sync::atomic::Ordering::SeqCst),
            self.changed_at.load(std::sync::atomic::Ordering::SeqCst),
        )
    }

    fn job_dir(&self, id: &str) -> Result<PathBuf> {
        self.index
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
            .ok_or(LibraryError::NotFound)
    }

    /// The folder a job is in, as names from the library's top (empty at the top).
    pub fn folder_of(&self, id: &str) -> Result<Vec<String>> {
        let dir = self.job_dir(id)?;
        let parent = dir.parent().and_then(|p| p.strip_prefix(&self.root).ok());
        Ok(parent
            .map(|p| p.iter().map(|n| n.to_string_lossy().into_owned()).collect())
            .unwrap_or_default())
    }

    /// The folder for a path of names inside the library, each checked.
    pub(crate) fn folder(&self, path: &[String]) -> Result<PathBuf> {
        let mut dir = self.root.clone();
        for name in path {
            check_name(name)?;
            dir.push(name);
        }
        Ok(dir)
    }

    /// A free display name in `dir`: `name`, then `name (2)`, `name (3)`…
    pub(crate) fn free_name(dir: &Path, name: &str) -> String {
        if !dir.join(name).exists() {
            return name.to_owned();
        }
        (2..)
            .map(|n| format!("{name} ({n})"))
            .find(|candidate| !dir.join(candidate).exists())
            .expect("some suffix is free")
    }

    fn new_id(counter: &mut u64) -> String {
        *counter += 1;
        let seed = format!(
            "{:?}|{}|{}",
            std::time::SystemTime::now(),
            std::process::id(),
            counter
        );
        Sha256::digest(seed.as_bytes())
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    pub fn list(&self, path: &[String]) -> Result<Listing> {
        let dir = self.folder(path)?;
        if dir.join("job.json").exists() || !dir.is_dir() {
            return Err(LibraryError::NotFound);
        }
        let mut listing = Listing {
            folders: Vec::new(),
            jobs: Vec::new(),
        };
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let path = entry.path();
            if path.join("job.json").is_file() {
                if let Ok(d) = read_details(&path) {
                    listing.jobs.push(JobSummary {
                        id: d.id,
                        name: d.name,
                        notes: d.notes,
                        last_cut: d.last_cut,
                        times_cut: d.times_cut,
                        current_revision: d.current_revision,
                        created_at: d.created_at,
                    });
                }
            } else if let Some(name) = entry.file_name().to_str() {
                listing.folders.push(name.to_owned());
            }
        }
        listing.folders.sort();
        listing.jobs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(listing)
    }

    pub fn create_folder(&self, parent: &[String], name: &str) -> Result<()> {
        check_name(name)?;
        if reserved(parent, name) {
            return Err(LibraryError::Exists);
        }
        let _guard = self.lock_write();
        let parent = self.folder(parent)?;
        if !parent.is_dir() || parent.join("job.json").exists() {
            return Err(LibraryError::NotFound);
        }
        let dir = parent.join(name);
        if dir.exists() {
            return Err(LibraryError::Exists);
        }
        fs::create_dir(&dir)?;
        sync_dir(&parent)?;
        Ok(())
    }

    /// Writes a revision's files and its `revision.json` into `dir`, each synced.
    fn write_revision(dir: &Path, files: &RevisionFiles, meta: &RevisionMeta) -> Result<()> {
        fs::create_dir_all(dir)?;
        write_synced(&dir.join("project.cutcraft"), files.project)?;
        write_synced(&dir.join("plan.json"), files.plan)?;
        write_synced(&dir.join("preview.svg"), files.preview_svg)?;
        write_synced(&dir.join("revision.json"), &json(meta)?)?;
        sync_dir(dir)?;
        Ok(())
    }

    /// Adds a new job with its first revision (spec §7.2). The whole folder is built in staging,
    /// synced, then renamed into place; only then is it in the library.
    pub fn add_job(
        &self,
        folder: &[String],
        name: &str,
        files: &RevisionFiles,
        mut meta: RevisionMeta,
        by: &str,
    ) -> Result<JobDetails> {
        check_name(name)?;
        let mut counter = self.lock_write();
        let parent = self.folder(folder)?;
        if !parent.is_dir() || parent.join("job.json").exists() {
            return Err(LibraryError::NotFound);
        }
        let id = Self::new_id(&mut counter);
        let at = now();
        meta.revision = 1;
        meta.created_at = at;
        let details = JobDetails {
            schema_version: DETAILS_SCHEMA,
            id: id.clone(),
            name: String::new(),
            notes: String::new(),
            current_revision: 1,
            created_at: at,
            created_by: by.to_owned(),
            last_cut: None,
            times_cut: 0,
            history: Vec::new(),
            details_revision: 1,
            changed_by: by.to_owned(),
        };
        let stage = self.staging.join(&id);
        let result = (|| {
            Self::write_revision(&stage.join("revisions").join("1"), files, &meta)?;
            sync_dir(&stage.join("revisions"))?;
            let display = Self::free_name(&parent, name);
            let details = JobDetails {
                name: display.clone(),
                ..details
            };
            write_synced(&stage.join("job.json"), &json(&details)?)?;
            sync_dir(&stage)?;
            let target = parent.join(&display);
            fs::rename(&stage, &target)?;
            sync_dir(&parent)?;
            Ok::<_, LibraryError>((details, target))
        })();
        match result {
            Ok((details, target)) => {
                self.index
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(id, target);
                Ok(details)
            }
            Err(e) => {
                let _ = fs::remove_dir_all(&stage);
                Err(e)
            }
        }
    }

    /// Publishes a new revision of a job; older revisions are kept (spec §7.1).
    pub fn add_revision(
        &self,
        id: &str,
        files: &RevisionFiles,
        mut meta: RevisionMeta,
        by: &str,
    ) -> Result<JobDetails> {
        let mut counter = self.lock_write();
        let job = self.job_dir(id)?;
        let mut details = read_details(&job)?;
        let n = details.current_revision + 1;
        meta.revision = n;
        meta.created_at = now();
        let stage = self.staging.join(Self::new_id(&mut counter));
        let result = (|| {
            Self::write_revision(&stage, files, &meta)?;
            let revisions = job.join("revisions");
            fs::rename(&stage, revisions.join(n.to_string()))?;
            sync_dir(&revisions)?;
            details.current_revision = n;
            details.details_revision += 1;
            details.changed_by = by.to_owned();
            write_atomic(&job.join("job.json"), &json(&details)?)?;
            Ok::<_, LibraryError>(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&stage);
        }
        result.map(|()| details)
    }

    /// Jobs whose name or notes contain `text` (any case), with their folders, by name.
    pub fn search(&self, text: &str) -> Vec<(Vec<String>, JobSummary)> {
        let needle = text.to_lowercase();
        let index = self.index.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let mut found: Vec<(Vec<String>, JobSummary)> = index
            .values()
            .filter_map(|dir| {
                let d = read_details(dir).ok()?;
                if !d.name.to_lowercase().contains(&needle)
                    && !d.notes.to_lowercase().contains(&needle)
                {
                    return None;
                }
                let folder = dir
                    .parent()?
                    .strip_prefix(&self.root)
                    .ok()?
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                Some((
                    folder,
                    JobSummary {
                        id: d.id,
                        name: d.name,
                        notes: d.notes,
                        last_cut: d.last_cut,
                        times_cut: d.times_cut,
                        current_revision: d.current_revision,
                        created_at: d.created_at,
                    },
                ))
            })
            .collect();
        found.sort_by_key(|(_, job)| job.name.to_lowercase());
        found
    }

    pub fn details(&self, id: &str) -> Result<JobDetails> {
        read_details(&self.job_dir(id)?)
    }

    pub fn revision(&self, id: &str, n: u32) -> Result<RevisionMeta> {
        let dir = self.job_dir(id)?.join("revisions").join(n.to_string());
        let text = fs::read_to_string(dir.join("revision.json"))?;
        serde_json::from_str(&text).map_err(|e| LibraryError::Io(e.to_string()))
    }

    /// The path of one of a revision's files (`project.cutcraft`, `plan.json`, `preview.svg`).
    pub fn revision_file(&self, id: &str, n: u32, file: &str) -> Result<PathBuf> {
        if !matches!(file, "project.cutcraft" | "plan.json" | "preview.svg") {
            return Err(LibraryError::NotFound);
        }
        let path = self
            .job_dir(id)?
            .join("revisions")
            .join(n.to_string())
            .join(file);
        if path.is_file() {
            Ok(path)
        } else {
            Err(LibraryError::NotFound)
        }
    }

    /// Changes a job's name and notes, refusing if someone else changed it since `based_on`
    /// (spec §7.4). Renaming renames its folder.
    pub fn edit(
        &self,
        id: &str,
        based_on: u64,
        name: Option<&str>,
        notes: Option<&str>,
        by: &str,
    ) -> Result<JobDetails> {
        if let Some(name) = name {
            check_name(name)?;
        }
        let _guard = self.lock_write();
        let mut job = self.job_dir(id)?;
        let mut details = read_details(&job)?;
        if details.details_revision != based_on {
            return Err(LibraryError::Conflict(Box::new(details)));
        }
        if let Some(name) = name
            && name != details.name
        {
            let parent = job.parent().ok_or(LibraryError::NotFound)?.to_path_buf();
            let target = parent.join(name);
            if target.exists() {
                return Err(LibraryError::Exists);
            }
            fs::rename(&job, &target)?;
            sync_dir(&parent)?;
            job = target;
            details.name = name.to_owned();
            self.index
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(id.to_owned(), job.clone());
        }
        if let Some(notes) = notes {
            details.notes = notes.to_owned();
        }
        details.details_revision += 1;
        details.changed_by = by.to_owned();
        write_atomic(&job.join("job.json"), &json(&details)?)?;
        Ok(details)
    }

    /// Records a cut in the job's history. Only a main cut that finished or was stopped counts
    /// as "cut" for last cut and times cut.
    pub fn record(&self, id: &str, entry: HistoryEntry) -> Result<()> {
        let _guard = self.lock_write();
        let job = self.job_dir(id)?;
        let mut details = read_details(&job)?;
        if entry.kind == "main" && matches!(entry.ended.as_str(), "finished" | "stopped") {
            details.last_cut = Some(entry.at);
            details.times_cut += 1;
        }
        details.history.push(entry);
        let excess = details.history.len().saturating_sub(HISTORY_KEPT);
        details.history.drain(..excess);
        details.details_revision += 1;
        write_atomic(&job.join("job.json"), &json(&details)?)?;
        Ok(())
    }

    /// Moves a job to another folder (a single rename).
    pub fn move_job(&self, id: &str, to: &[String]) -> Result<()> {
        let _guard = self.lock_write();
        let job = self.job_dir(id)?;
        let target_dir = self.folder(to)?;
        if !target_dir.is_dir() || target_dir.join("job.json").exists() {
            return Err(LibraryError::NotFound);
        }
        let name = job
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(LibraryError::NotFound)?
            .to_owned();
        let target = target_dir.join(Self::free_name(&target_dir, &name));
        let old_parent = job.parent().map(Path::to_path_buf);
        fs::rename(&job, &target)?;
        sync_dir(&target_dir)?;
        if let Some(parent) = old_parent {
            sync_dir(&parent)?;
        }
        self.index
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id.to_owned(), target);
        Ok(())
    }

    /// Moves a job to Recently deleted (kept 30 days, outside the library and its backups).
    pub fn delete_job(&self, id: &str) -> Result<()> {
        let folder = self.folder_of(id)?;
        let _guard = self.lock_write();
        let job = self.job_dir(id)?;
        let at = now();
        let target = self.deleted.join(format!("{at}-{id}"));
        let parent = job.parent().map(Path::to_path_buf);
        fs::rename(&job, &target)?;
        write_synced(
            &target.join("deleted.json"),
            &json(&DeletedFrom { folder, at })?,
        )?;
        sync_dir(&self.deleted)?;
        if let Some(parent) = parent {
            sync_dir(&parent)?;
        }
        self.index
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        Ok(())
    }

    /// The ids of every job in a folder and the folders within it.
    pub fn jobs_under(&self, path: &[String]) -> Result<Vec<String>> {
        let dir = self.folder(path)?;
        Ok(self
            .index
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|(_, job)| job.starts_with(&dir))
            .map(|(id, _)| id.clone())
            .collect())
    }

    /// Moves a folder, with everything in it, into another folder (a single rename). A folder of
    /// the same name there makes this one `name (2)`. Returns the folder's new path.
    pub fn move_folder(&self, path: &[String], to: &[String]) -> Result<Vec<String>> {
        if path.is_empty() {
            return Err(LibraryError::NotFound);
        }
        if to.starts_with(path) {
            return Err(LibraryError::IntoItself);
        }
        let _guard = self.lock_write();
        let dir = self.folder(path)?;
        let target_dir = self.folder(to)?;
        for d in [&dir, &target_dir] {
            if !d.is_dir() || d.join("job.json").exists() {
                return Err(LibraryError::NotFound);
            }
        }
        let name = path.last().expect("not the top").clone();
        if to == &path[..path.len() - 1] {
            return Ok(path.to_vec());
        }
        let name = if reserved(to, &name) {
            Self::free_name(&target_dir, &format!("{name} (2)"))
        } else {
            Self::free_name(&target_dir, &name)
        };
        let target = target_dir.join(&name);
        self.rename_folder_dir(&dir, &target)?;
        Ok(to.iter().cloned().chain(std::iter::once(name)).collect())
    }

    /// Renames a folder. Returns its new path.
    pub fn rename_folder(&self, path: &[String], name: &str) -> Result<Vec<String>> {
        check_name(name)?;
        let Some((old, parent)) = path.split_last() else {
            return Err(LibraryError::NotFound);
        };
        if reserved(parent, name) {
            return Err(LibraryError::Exists);
        }
        let _guard = self.lock_write();
        let dir = self.folder(path)?;
        if !dir.is_dir() || dir.join("job.json").exists() {
            return Err(LibraryError::NotFound);
        }
        let renamed: Vec<String> = parent
            .iter()
            .cloned()
            .chain(std::iter::once(name.to_owned()))
            .collect();
        if old == name {
            return Ok(renamed);
        }
        let target = self.folder(&renamed)?;
        // Only the case changing is the same folder on some disks: allowed.
        if target.exists() && !old.eq_ignore_ascii_case(name) {
            return Err(LibraryError::Exists);
        }
        self.rename_folder_dir(&dir, &target)?;
        Ok(renamed)
    }

    /// Renames a folder's directory, and points the jobs inside at their new place.
    fn rename_folder_dir(&self, dir: &Path, target: &Path) -> Result<()> {
        fs::rename(dir, target)?;
        if let Some(parent) = target.parent() {
            sync_dir(parent)?;
        }
        if let Some(parent) = dir.parent() {
            sync_dir(parent)?;
        }
        for job in self
            .index
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values_mut()
        {
            if let Ok(rest) = job.strip_prefix(dir) {
                *job = target.join(rest);
            }
        }
        Ok(())
    }

    /// A Recently deleted entry by its name, checked to be one.
    fn deleted_entry(&self, entry: &str) -> Result<PathBuf> {
        if entry.is_empty() || entry.starts_with('.') || entry.contains(['/', '\\']) {
            return Err(LibraryError::NotFound);
        }
        let dir = self.deleted.join(entry);
        if dir.is_dir() {
            Ok(dir)
        } else {
            Err(LibraryError::NotFound)
        }
    }

    /// What's in Recently deleted, newest first.
    pub fn deleted(&self) -> Result<Vec<DeletedItem>> {
        let mut items = Vec::new();
        for e in fs::read_dir(&self.deleted)?.flatten() {
            let Some(entry) = e.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let dir = e.path();
            if !dir.is_dir() {
                continue;
            }
            let stamp: u64 = entry
                .split('-')
                .next()
                .and_then(|t| t.parse().ok())
                .unwrap_or(0);
            let from: Option<DeletedFrom> = fs::read(dir.join("deleted.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
            let deleted_at = from.as_ref().map_or(stamp, |f| f.at);
            let item = if let Ok(details) = read_details(&dir) {
                DeletedItem {
                    entry,
                    kind: "job",
                    id: Some(details.id),
                    name: details.name,
                    notes: details.notes,
                    folder: from.map(|f| f.folder).unwrap_or_default(),
                    current_revision: Some(details.current_revision),
                    deleted_at,
                    expires_at: deleted_at + KEEP_DELETED_S,
                }
            } else if entry.ends_with("-library-before-restore") {
                DeletedItem {
                    entry,
                    kind: "library",
                    id: None,
                    name: "The library before a restore".into(),
                    notes: String::new(),
                    folder: Vec::new(),
                    current_revision: None,
                    deleted_at,
                    expires_at: deleted_at + KEEP_DELETED_S,
                }
            } else {
                continue;
            };
            items.push(item);
        }
        items.sort_by_key(|i| std::cmp::Reverse(i.deleted_at));
        Ok(items)
    }

    /// A file of a deleted job's current revision (for its thumbnail).
    pub fn deleted_file(&self, entry: &str, file: &str) -> Result<PathBuf> {
        if !matches!(file, "plan.json" | "preview.svg") {
            return Err(LibraryError::NotFound);
        }
        let dir = self.deleted_entry(entry)?;
        let details = read_details(&dir)?;
        let path = dir
            .join("revisions")
            .join(details.current_revision.to_string())
            .join(file);
        if path.is_file() {
            Ok(path)
        } else {
            Err(LibraryError::NotFound)
        }
    }

    /// Puts a deleted job back: in the folder it was in, or at the top when that folder is gone.
    /// A job of the same name there makes it `name (2)`. Returns the job's id and folder.
    pub fn restore_deleted(&self, entry: &str) -> Result<(String, Vec<String>)> {
        let _guard = self.lock_write();
        let dir = self.deleted_entry(entry)?;
        let details = read_details(&dir).map_err(|_| LibraryError::NotFound)?;
        if self.job_dir(&details.id).is_ok() {
            return Err(LibraryError::Exists);
        }
        let from: Option<DeletedFrom> = fs::read(dir.join("deleted.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let wanted = from.map(|f| f.folder).unwrap_or_default();
        let folder = match self.folder(&wanted) {
            Ok(d) if d.is_dir() && !d.join("job.json").exists() => wanted,
            _ => Vec::new(),
        };
        let parent = self.folder(&folder)?;
        let target = parent.join(Self::free_name(&parent, &details.name));
        let _ = fs::remove_file(dir.join("deleted.json"));
        fs::rename(&dir, &target)?;
        sync_dir(&parent)?;
        sync_dir(&self.deleted)?;
        self.index
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(details.id.clone(), target);
        Ok((details.id, folder))
    }

    /// Deletes something in Recently deleted for good.
    pub fn purge_deleted(&self, entry: &str) -> Result<()> {
        let dir = self.deleted_entry(entry)?;
        fs::remove_dir_all(&dir)?;
        sync_dir(&self.deleted)?;
        Ok(())
    }

    /// Deletes what has been in Recently deleted longer than 30 days. Returns how many.
    pub fn expire_deleted(&self, at: u64) -> Result<usize> {
        let mut gone = 0;
        for item in self.deleted()? {
            if item.expires_at <= at {
                self.purge_deleted(&item.entry)?;
                gone += 1;
            }
        }
        Ok(gone)
    }

    pub fn delete_folder(&self, path: &[String]) -> Result<()> {
        if path.is_empty() {
            return Err(LibraryError::NotFound);
        }
        let _guard = self.lock_write();
        let dir = self.folder(path)?;
        if !dir.is_dir() || dir.join("job.json").exists() {
            return Err(LibraryError::NotFound);
        }
        if fs::read_dir(&dir)?.next().is_some() {
            return Err(LibraryError::FolderNotEmpty);
        }
        fs::remove_dir(&dir)?;
        if let Some(parent) = dir.parent() {
            sync_dir(parent)?;
        }
        Ok(())
    }
}

pub(crate) fn read_details(job: &Path) -> Result<JobDetails> {
    let text = fs::read_to_string(job.join("job.json"))?;
    serde_json::from_str(&text).map_err(|e| LibraryError::Io(e.to_string()))
}

pub(crate) fn revision_complete(dir: &Path) -> bool {
    [
        "revision.json",
        "project.cutcraft",
        "plan.json",
        "preview.svg",
    ]
    .iter()
    .all(|f| dir.join(f).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> RevisionMeta {
        RevisionMeta {
            schema_version: REVISION_SCHEMA,
            revision: 0,
            created_at: 0,
            created_by: "kitchen MacBook".into(),
            model: "Silhouette Cameo (original)".into(),
            material: "Vinyl".into(),
            mat: "mat12x12".into(),
            size_mm: Some([100.0, 80.0]),
            estimate_s: 30.0,
            digest: "d".into(),
            planner: "1.2.1 (b6aad51)".into(),
            adjusted_from: None,
        }
    }

    fn files() -> RevisionFiles<'static> {
        RevisionFiles {
            project: b"{}",
            plan: b"{}",
            preview_svg: b"<svg/>",
        }
    }

    fn path(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn names_with_separators_dots_or_control_characters_are_refused() {
        for bad in [
            "",
            " x",
            "a/b",
            "a\\b",
            "..",
            ".hidden",
            "x.",
            "tab\there",
            "a:b",
        ] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
        for good in [
            "Summer 2026",
            "Planisphere (2)",
            "T-shirt — front",
            "Ünïcødé",
        ] {
            check_name(good).unwrap();
        }
        assert!(check_name(&"x".repeat(101)).is_err());
    }

    #[test]
    fn jobs_are_added_listed_and_survive_a_reopen() {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(data.path()).unwrap();
        library.create_folder(&[], "Shirts").unwrap();
        let job = library
            .add_job(&path(&["Shirts"]), "Planisphere", &files(), meta(), "sam")
            .unwrap();
        let again = library
            .add_job(&path(&["Shirts"]), "Planisphere", &files(), meta(), "sam")
            .unwrap();
        assert_eq!(again.name, "Planisphere (2)");

        let listing = library.list(&path(&["Shirts"])).unwrap();
        assert_eq!(listing.jobs.len(), 2);
        assert_eq!(library.list(&[]).unwrap().folders, vec!["Shirts"]);

        drop(library);
        let library = Library::open(data.path()).unwrap();
        assert_eq!(library.details(&job.id).unwrap().name, "Planisphere");
        assert_eq!(library.revision(&job.id, 1).unwrap().revision, 1);
        assert!(library.revision_file(&job.id, 1, "preview.svg").is_ok());
        assert!(library.revision_file(&job.id, 1, "../job.json").is_err());
    }

    #[test]
    fn a_new_revision_keeps_the_old_one() {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(data.path()).unwrap();
        let job = library
            .add_job(&[], "Logo", &files(), meta(), "sam")
            .unwrap();
        let updated = library
            .add_revision(&job.id, &files(), meta(), "alex")
            .unwrap();
        assert_eq!(updated.current_revision, 2);
        assert!(library.revision(&job.id, 1).is_ok());
        assert_eq!(library.revision(&job.id, 2).unwrap().revision, 2);
    }

    #[test]
    fn an_edit_based_on_an_old_version_is_refused_with_the_current_one() {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(data.path()).unwrap();
        let job = library
            .add_job(&[], "Logo", &files(), meta(), "sam")
            .unwrap();
        let first = library
            .edit(
                &job.id,
                job.details_revision,
                None,
                Some("blue vinyl"),
                "sam",
            )
            .unwrap();
        let Err(LibraryError::Conflict(current)) =
            library.edit(&job.id, job.details_revision, None, Some("red"), "alex")
        else {
            panic!("expected a conflict")
        };
        assert_eq!(current.notes, "blue vinyl");
        assert_eq!(current.details_revision, first.details_revision);

        let renamed = library
            .edit(
                &job.id,
                first.details_revision,
                Some("Logo v2"),
                None,
                "sam",
            )
            .unwrap();
        assert_eq!(renamed.name, "Logo v2");
        assert!(data.path().join("library/Logo v2/job.json").is_file());
    }

    #[test]
    fn history_counts_only_main_cuts_and_is_bounded() {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(data.path()).unwrap();
        let job = library
            .add_job(&[], "Logo", &files(), meta(), "sam")
            .unwrap();
        let entry = |kind: &str, ended: &str, at: u64| HistoryEntry {
            at,
            execution: format!("e{at}"),
            cutter: "Workshop".into(),
            revision: 1,
            kind: kind.into(),
            by: "sam".into(),
            ended: ended.into(),
        };
        library
            .record(&job.id, entry("test", "finished", 1))
            .unwrap();
        library
            .record(&job.id, entry("main", "interrupted", 2))
            .unwrap();
        library
            .record(&job.id, entry("main", "finished", 3))
            .unwrap();
        let d = library.details(&job.id).unwrap();
        assert_eq!((d.times_cut, d.last_cut), (1, Some(3)));
        for at in 10..80 {
            library
                .record(&job.id, entry("test", "finished", at))
                .unwrap();
        }
        let d = library.details(&job.id).unwrap();
        assert_eq!(d.history.len(), HISTORY_KEPT);
        assert_eq!(d.history.last().unwrap().at, 79);
    }

    #[test]
    fn moving_deleting_and_folders() {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(data.path()).unwrap();
        library.create_folder(&[], "A").unwrap();
        library.create_folder(&[], "B").unwrap();
        assert_eq!(library.create_folder(&[], "A"), Err(LibraryError::Exists));
        let job = library
            .add_job(&path(&["A"]), "Logo", &files(), meta(), "s")
            .unwrap();
        assert_eq!(
            library.delete_folder(&path(&["A"])),
            Err(LibraryError::FolderNotEmpty)
        );
        assert_eq!(library.folder_of(&job.id).unwrap(), path(&["A"]));
        library.move_job(&job.id, &path(&["B"])).unwrap();
        assert_eq!(library.list(&path(&["B"])).unwrap().jobs.len(), 1);
        assert_eq!(library.folder_of(&job.id).unwrap(), path(&["B"]));
        // A folder moves with its jobs; it can't go inside itself; a clash gets "(2)".
        library.create_folder(&path(&["B"]), "Inner").unwrap();
        assert_eq!(
            library.move_folder(&path(&["B"]), &path(&["B", "Inner"])),
            Err(LibraryError::IntoItself)
        );
        library.create_folder(&[], "C").unwrap();
        library.create_folder(&path(&["C"]), "B").unwrap();
        assert_eq!(
            library.move_folder(&path(&["B"]), &path(&["C"])).unwrap(),
            path(&["C", "B (2)"])
        );
        assert_eq!(library.folder_of(&job.id).unwrap(), path(&["C", "B (2)"]));
        assert_eq!(
            library.jobs_under(&path(&["C"])).unwrap(),
            vec![job.id.clone()]
        );
        assert_eq!(library.details(&job.id).unwrap().name, "Logo");
        library.delete_folder(&path(&["A"])).unwrap();
        // A folder renames with its jobs; the name kept for Recently deleted can't be taken.
        assert_eq!(
            library
                .rename_folder(&path(&["C", "B (2)"]), "Logos")
                .unwrap(),
            path(&["C", "Logos"])
        );
        assert_eq!(library.folder_of(&job.id).unwrap(), path(&["C", "Logos"]));
        assert_eq!(
            library.rename_folder(&path(&["C"]), "recently deleted"),
            Err(LibraryError::Exists)
        );
        assert_eq!(
            library.create_folder(&[], RECENTLY_DELETED),
            Err(LibraryError::Exists)
        );
        library.delete_job(&job.id).unwrap();
        assert_eq!(library.details(&job.id), Err(LibraryError::NotFound));
        // Recently deleted lists it with where it was, and restores it there.
        let deleted = library.deleted().unwrap();
        assert_eq!(deleted.len(), 1);
        assert_eq!(
            (
                deleted[0].kind,
                deleted[0].name.as_str(),
                deleted[0].folder.clone()
            ),
            ("job", "Logo", path(&["C", "Logos"]))
        );
        assert_eq!(
            deleted[0].expires_at,
            deleted[0].deleted_at + KEEP_DELETED_S
        );
        assert!(
            library
                .deleted_file(&deleted[0].entry, "preview.svg")
                .is_ok()
        );
        assert_eq!(
            library.restore_deleted(&deleted[0].entry).unwrap(),
            (job.id.clone(), path(&["C", "Logos"]))
        );
        assert_eq!(library.details(&job.id).unwrap().name, "Logo");
        assert!(library.deleted().unwrap().is_empty());
        // Its folder gone, a restore puts it at the top; expiry and Delete now remove it for good.
        library.delete_job(&job.id).unwrap();
        library
            .delete_folder(&path(&["C", "Logos", "Inner"]))
            .unwrap();
        library.delete_folder(&path(&["C", "Logos"])).unwrap();
        let entry = library.deleted().unwrap()[0].entry.clone();
        assert_eq!(
            library.restore_deleted(&entry).unwrap().1,
            Vec::<String>::new()
        );
        library.delete_job(&job.id).unwrap();
        assert_eq!(library.expire_deleted(now()).unwrap(), 0);
        assert_eq!(
            library.expire_deleted(now() + KEEP_DELETED_S + 1).unwrap(),
            1
        );
        assert!(library.deleted().unwrap().is_empty());
        assert_eq!(
            library.restore_deleted("../library"),
            Err(LibraryError::NotFound)
        );
        assert!(library.list(&path(&[".."])).is_err());
    }

    #[test]
    fn a_crash_between_publishing_a_revision_and_updating_details_is_repaired() {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(data.path()).unwrap();
        let job = library.add_job(&[], "Logo", &files(), meta(), "s").unwrap();
        let dir = data.path().join("library/Logo/revisions");
        // A whole revision 2 that job.json doesn't know about, and a broken revision 5.
        Library::write_revision(&dir.join("2"), &files(), &meta()).unwrap();
        fs::create_dir_all(dir.join("5")).unwrap();
        fs::write(data.path().join(".staging/leftover"), b"x").unwrap();
        drop(library);

        let library = Library::open(data.path()).unwrap();
        assert_eq!(library.details(&job.id).unwrap().current_revision, 2);
        assert!(!dir.join("5").exists());
        assert_eq!(
            fs::read_dir(data.path().join(".staging")).unwrap().count(),
            0
        );
    }
}
