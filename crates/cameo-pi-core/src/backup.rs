//! Library backups (spec §15): consistent hard-link snapshots of the library, the manifest that
//! describes a backup on another computer, working out what changed since the last one, and
//! checking a library fetched back for a restore. The transfer itself (SFTP, optional encryption)
//! is the server's. Backups hold the library only: never users, keys, settings or anything about
//! the cutters.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use cutcraft_device::gpgl::Model;
use cutcraft_job::MachineProfile;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::library::{
    JobDetails, Library, LibraryError, RevisionMeta, check_name, json, now, read_details, sync_dir,
    write_synced,
};
use crate::validate;

pub const FORMAT: &str = "cameo-pi-backup";
pub const SCHEMA: u32 = 2;
/// The marker file at the top of a backup folder: which Pi, and whether it's encrypted.
pub const MARKER: &str = "cameo-pi-backup.json";
/// How long what disappeared from the Pi is kept on the backup computer.
pub const KEEP_REMOVED_S: u64 = 90 * 86_400;
const REVISION_FILES: [&str; 4] = [
    "revision.json",
    "project.cutcraft",
    "plan.json",
    "preview.svg",
];

/// The marker file. It is never encrypted: it says what the folder is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub format: String,
    pub schema_version: u32,
    /// The Pi that backs up here (a random id kept with its private state).
    pub pi_id: String,
    /// Its name, for people.
    pub pi_name: String,
    pub encrypted: bool,
    /// Encrypted: the public key the files are encrypted to (`age1…`), so a new key (a new
    /// password) is noticed and everything is encrypted again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<String>,
    /// When the last backup finished, in seconds since 1970.
    pub updated: u64,
    /// The cameo-pi version that wrote it.
    pub made_by: String,
    /// Version 2: an append-only commit naming an immutable manifest and encrypted key.
    /// None on the ownership marker, which is made before the first generation is built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// One library file in a backup.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub size: u64,
    pub sha256: String,
    /// Modified time on the Pi, seconds: with the size, it says whether the file needs hashing.
    pub mtime: u64,
    /// An immutable stored file (plain or encrypted). Legacy plain entries have no object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
}

/// A file that disappeared from the Pi, kept on the backup computer until [`KEEP_REMOVED_S`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Removed {
    /// Its library path.
    pub path: String,
    /// Where it is now on the backup computer: `removed/<date>/library/…`, or its object.
    pub stored: String,
    pub at: u64,
    /// Encrypted retained files can belong to an earlier key after a password change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Everything a backup holds: library paths (`Shirts/Planisphere/job.json`) to their files.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub files: BTreeMap<String, Entry>,
    #[serde(default)]
    pub removed: Vec<Removed>,
    /// Includes empty directories, independently of any files they contain.
    #[serde(default)]
    pub directories: Vec<String>,
}

/// Directories in a snapshot, including empty folders. Links are never followed.
pub fn directories(snapshot: &Path) -> io::Result<Vec<String>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                out.push(
                    entry
                        .path()
                        .strip_prefix(root)
                        .expect("inside snapshot")
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
                walk(root, &entry.path(), out)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(snapshot, snapshot, &mut out)?;
    out.sort();
    Ok(out)
}

/// A library file as it is now on the Pi.
#[derive(Clone, Debug, PartialEq)]
pub struct Local {
    pub size: u64,
    pub sha256: String,
    pub mtime: u64,
}

/// What a backup run does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Diff {
    /// New or changed: upload these.
    pub upload: Vec<String>,
    /// Gone from the Pi: move these to the removed area.
    pub retire: Vec<String>,
    /// Removed longer ago than [`KEEP_REMOVED_S`]: delete these for good.
    pub expired: Vec<Removed>,
    /// Bytes to upload.
    pub bytes: u64,
}

fn unique() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}-{}", NEXT.fetch_add(1, Ordering::SeqCst))
}

/// A copy of `from` with every file hard-linked, so it costs almost no space. Links are skipped.
fn link_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            link_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::hard_link(entry.path(), target)?;
        }
    }
    Ok(())
}

impl Library {
    /// A consistent copy of the whole library, taken under the write lock (spec §15). Revisions
    /// never change and `job.json` is replaced by rename, so the hard links keep exactly the files
    /// of that moment. The caller removes it when done.
    pub fn snapshot(&self) -> Result<PathBuf, LibraryError> {
        let _guard = self.write.lock().unwrap_or_else(|p| p.into_inner());
        let into = self.scratch("snapshot");
        link_tree(&self.root, &into)?;
        Ok(into)
    }

    /// A fresh path in staging for temporary files; staging is emptied at start-up.
    pub fn scratch(&self, what: &str) -> PathBuf {
        self.staging.join(format!("{what}-{}", unique()))
    }

    fn reindex(&self) -> Result<(), LibraryError> {
        let mut index = std::collections::HashMap::new();
        self.scan(&self.root.clone(), &mut index)?;
        *self.index.lock().unwrap_or_else(|p| p.into_inner()) = index;
        Ok(())
    }

    /// Replaces the library with a staged one (checked by [`inspect`]). The old library is kept
    /// in Recently deleted. The caller makes sure no cutter has a turn.
    pub fn replace_with(&self, staged: &Path) -> Result<(), LibraryError> {
        let _guard = self.lock_write();
        let old = self
            .deleted
            .join(format!("{}-library-before-restore", now()));
        fs::rename(&self.root, &old)?;
        if let Err(error) = fs::rename(staged, &self.root) {
            fs::rename(&old, &self.root)?;
            return Err(error.into());
        }
        sync_dir(&self.deleted)?;
        if let Some(parent) = self.root.parent() {
            sync_dir(parent)?;
        }
        self.reindex()
    }
}

/// A library path from a manifest, checked as a name a person could have typed (spec §14.3):
/// no `..`, no absolute paths, no hidden names. Restores write only these.
pub fn safe_path(path: &str) -> Result<PathBuf, String> {
    let unsafe_name = || Err(format!("The backup has an unsafe file name: {path:?}."));
    if path.contains('\\') || path.starts_with('/') || path.is_empty() {
        return unsafe_name();
    }
    let mut parts = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => match part.to_str() {
                Some(part) if check_name(part).is_ok() => parts.push(part),
                _ => return unsafe_name(),
            },
            _ => return unsafe_name(),
        }
    }
    Ok(parts)
}

/// Every file of a snapshot, by library path. Immutable revision files whose size and modified
/// time match keep their hashes. Mutable job details are always hashed: several equal-size
/// notes/history updates can share one filesystem timestamp.
pub fn scan(snapshot: &Path, previous: &Manifest) -> io::Result<BTreeMap<String, Local>> {
    fn walk(
        dir: &Path,
        prefix: &str,
        previous: &Manifest,
        out: &mut BTreeMap<String, Local>,
    ) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let kind = entry.file_type()?;
            if kind.is_dir() {
                walk(&entry.path(), &path, previous, out)?;
            } else if kind.is_file() {
                let meta = entry.metadata()?;
                let size = meta.len();
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                let sha256 = match previous.files.get(&path) {
                    Some(e)
                        if REVISION_FILES.contains(&entry.file_name().to_str().unwrap_or(""))
                            && e.size == size
                            && e.mtime == mtime =>
                    {
                        e.sha256.clone()
                    }
                    _ => format!("{:x}", Sha256::digest(fs::read(entry.path())?)),
                };
                out.insert(
                    path,
                    Local {
                        size,
                        sha256,
                        mtime,
                    },
                );
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(snapshot, "", previous, &mut out)?;
    Ok(out)
}

/// What a run must do to bring the backup from `previous` to `now`, at `at` (seconds).
pub fn diff(previous: &Manifest, current: &BTreeMap<String, Local>, at: u64) -> Diff {
    let mut d = Diff::default();
    for (path, local) in current {
        if previous
            .files
            .get(path)
            .is_none_or(|e| e.sha256 != local.sha256)
        {
            d.bytes += local.size;
            d.upload.push(path.clone());
        }
    }
    d.retire = previous
        .files
        .keys()
        .filter(|p| !current.contains_key(*p))
        .cloned()
        .collect();
    d.expired = previous
        .removed
        .iter()
        .filter(|r| at.saturating_sub(r.at) >= KEEP_REMOVED_S)
        .cloned()
        .collect();
    d
}

/// A random name for an encrypted object: nothing about the file shows in it.
pub fn object_name() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system's random number source works");
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("objects/{}/{hex}", &hex[..2])
}

/// A job in a fetched backup, checked.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StagedJob {
    pub id: String,
    /// Its folder's name (its display name).
    pub name: String,
    pub folder: Vec<String>,
    pub revisions: u32,
}

/// What a fetched backup holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Staged {
    pub jobs: Vec<StagedJob>,
    /// Every folder, jobs' folders included, from the top.
    pub folders: Vec<Vec<String>>,
}

/// Checks a staged `library/` folder: every job, and every revision of it validated as an upload
/// would be (spec §10). The Pi's own values replace the archive's: plans are rewritten as
/// validated, previews drawn again and digests recomputed.
pub fn inspect(
    library: &Path,
    model: &Model,
    machine: &MachineProfile,
    scratch: &Path,
) -> Result<Staged, String> {
    let mut staged = Staged::default();
    let mut ids = HashSet::new();
    walk(
        library,
        &mut Vec::new(),
        model,
        machine,
        scratch,
        &mut staged,
        &mut ids,
    )?;
    Ok(staged)
}

fn walk(
    dir: &Path,
    path: &mut Vec<String>,
    model: &Model,
    machine: &MachineProfile,
    scratch: &Path,
    staged: &mut Staged,
    ids: &mut HashSet<String>,
) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .and_then(|d| d.collect::<io::Result<Vec<_>>>())
        .map_err(|e| e.to_string())?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.path().is_dir() {
            return Err(format!(
                "The backup has a stray file {:?} among the folders.",
                [path.as_slice(), &[name]].concat().join("/")
            ));
        }
        path.push(name.clone());
        if entry.path().join("job.json").exists() {
            let job = check_job(&entry.path(), path, model, machine, scratch)?;
            if !ids.insert(job.id.clone()) {
                return Err(format!("The backup has job {:?} twice.", job.name));
            }
            staged.jobs.push(job);
        } else {
            staged.folders.push(path.clone());
            walk(&entry.path(), path, model, machine, scratch, staged, ids)?;
        }
        path.pop();
    }
    Ok(())
}

fn check_job(
    dir: &Path,
    path: &[String],
    model: &Model,
    machine: &MachineProfile,
    scratch: &Path,
) -> Result<StagedJob, String> {
    let shown = path.join("/");
    let bad = |why: String| format!("Job {shown:?} in the backup can't be restored: {why}");
    let folder = path[..path.len() - 1].to_vec();
    let name = path[path.len() - 1].clone();
    let mut details: JobDetails = read_details(dir).map_err(|e| bad(e.to_string()))?;
    if details.id.is_empty() || !details.id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(bad("its id is damaged.".into()));
    }
    let mut names: Vec<String> = fs::read_dir(dir)
        .map_err(|e| bad(e.to_string()))?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    names.sort();
    if names != ["job.json", "revisions"] {
        return Err(bad(format!(
            "it holds unexpected files ({}).",
            names.join(", ")
        )));
    }
    let revisions = dir.join("revisions");
    let mut numbers: Vec<u32> = fs::read_dir(&revisions)
        .map_err(|e| bad(e.to_string()))?
        .map(|e| {
            e.ok()
                .and_then(|e| e.file_name().to_str().and_then(|n| n.parse().ok()))
                .ok_or_else(|| bad("a revision folder has a strange name.".into()))
        })
        .collect::<Result<_, _>>()?;
    numbers.sort_unstable();
    if numbers != (1..=details.current_revision).collect::<Vec<_>>() || numbers.is_empty() {
        return Err(bad("its revisions don't match its details.".into()));
    }
    for n in numbers {
        let rev = revisions.join(n.to_string());
        let mut files: Vec<String> = fs::read_dir(&rev)
            .map_err(|e| bad(e.to_string()))?
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect();
        files.sort();
        let mut expected = REVISION_FILES.map(str::to_owned).to_vec();
        expected.sort();
        if files != expected {
            return Err(bad(format!("revision {n} isn't complete.")));
        }
        let read = |file: &str| fs::read(rev.join(file)).map_err(|e| bad(e.to_string()));
        let mut meta: RevisionMeta = serde_json::from_slice(&read("revision.json")?)
            .map_err(|_| bad(format!("revision {n}'s details are damaged.")))?;
        // As recorded: a revision stored as not independently verified stays so, even when its
        // planner is this one (it was saved before the Pi could check it exactly).
        let planner = meta.planner.clone();
        let checked = validate::validate(
            &read("project.cutcraft")?,
            &read("plan.json")?,
            &planner,
            model,
            machine,
            scratch,
        )
        .map_err(|why| bad(format!("revision {n}: {why}")))?;
        let _ = fs::remove_dir_all(scratch);
        let plan = serde_json::to_vec_pretty(&checked.plan).map_err(|e| bad(e.to_string()))?;
        meta.revision = n;
        meta.digest = checked.runnable.digest.clone();
        let write = |file: &str, bytes: &[u8]| {
            write_synced(&rev.join(file), bytes).map_err(|e| bad(e.to_string()))
        };
        write("plan.json", &plan)?;
        write(
            "preview.svg",
            cutcraft_path::preview_svg(&checked.plan).as_bytes(),
        )?;
        write(
            "revision.json",
            &json(&meta).map_err(|e| bad(e.to_string()))?,
        )?;
    }
    details.name.clone_from(&name);
    write_synced(
        &dir.join("job.json"),
        &json(&details).map_err(|e| bad(e.to_string()))?,
    )
    .map_err(|e| bad(e.to_string()))?;
    Ok(StagedJob {
        id: details.id,
        name,
        folder,
        revisions: details.current_revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{REVISION_SCHEMA, RevisionFiles};
    use cutcraft_device::gpgl::CAMEO_1;

    fn job(library: &Library, folder: &[String], name: &str) -> String {
        let (project, plan) = crate::fixtures::upload();
        let scratch = library.scratch("validate");
        let checked = validate::validate(
            &project,
            &plan,
            validate::PLANNER,
            &CAMEO_1,
            &MachineProfile::cameo_1(),
            &scratch,
        )
        .unwrap();
        let plan = serde_json::to_vec_pretty(&checked.plan).unwrap();
        let preview = cutcraft_path::preview_svg(&checked.plan);
        let meta = RevisionMeta {
            schema_version: REVISION_SCHEMA,
            revision: 0,
            created_at: 0,
            created_by: "test".into(),
            model: "Cameo".into(),
            material: "vinyl".into(),
            mat: "mat12x12".into(),
            size_mm: None,
            estimate_s: 1.0,
            digest: checked.runnable.digest.clone(),
            planner: validate::PLANNER.into(),
            adjusted_from: None,
        };
        let files = RevisionFiles {
            project: &project,
            plan: &plan,
            preview_svg: preview.as_bytes(),
        };
        library
            .add_job(folder, name, &files, meta, "test")
            .unwrap()
            .id
    }

    #[test]
    fn a_run_uploads_what_changed_retires_what_went_and_expires_old_removals() {
        let a = tempfile::tempdir().unwrap();
        let library = Library::open(a.path()).unwrap();
        library.create_folder(&[], "Shirts").unwrap();
        let first = job(&library, &["Shirts".into()], "Square");
        let snapshot = library.snapshot().unwrap();
        let now_files = scan(&snapshot, &Manifest::default()).unwrap();
        let d = diff(&Manifest::default(), &now_files, 1000);
        assert_eq!(
            d.upload.len(),
            5,
            "job.json and the revision's four files: {:?}",
            d.upload
        );
        assert!(d.retire.is_empty() && d.bytes > 0);
        let manifest = Manifest {
            files: now_files
                .iter()
                .map(|(p, l)| {
                    (
                        p.clone(),
                        Entry {
                            size: l.size,
                            sha256: l.sha256.clone(),
                            mtime: l.mtime,
                            object: None,
                        },
                    )
                })
                .collect(),
            directories: vec![],
            removed: vec![Removed {
                path: "Old/job.json".into(),
                stored: "removed/x".into(),
                at: 0,
                key: None,
            }],
        };
        fs::remove_dir_all(&snapshot).unwrap();

        // Nothing changed: nothing to upload; the old removal has expired.
        let snapshot = library.snapshot().unwrap();
        let again = scan(&snapshot, &manifest).unwrap();
        let d = diff(&manifest, &again, KEEP_REMOVED_S + 1);
        assert!(d.upload.is_empty() && d.retire.is_empty());
        assert_eq!(d.expired.len(), 1);
        fs::remove_dir_all(&snapshot).unwrap();

        // A note changes one file; a deleted job's files are retired.
        library
            .edit(&first, 1, None, Some("new note"), "test")
            .unwrap();
        let second = job(&library, &[], "Top");
        let snapshot = library.snapshot().unwrap();
        let changed = scan(&snapshot, &manifest).unwrap();
        let d = diff(&manifest, &changed, 2000);
        assert!(d.upload.contains(&"Shirts/Square/job.json".to_owned()));
        assert!(d.upload.iter().any(|p| p.starts_with("Top/")));
        assert!(
            !d.upload
                .iter()
                .any(|p| p.starts_with("Shirts/Square/revisions")),
            "revisions never change"
        );
        fs::remove_dir_all(&snapshot).unwrap();
        library.delete_job(&second).unwrap();
        library.delete_job(&first).unwrap();
        let snapshot = library.snapshot().unwrap();
        let gone = scan(&snapshot, &manifest).unwrap();
        assert_eq!(diff(&manifest, &gone, 3000).retire.len(), 5);
    }

    #[test]
    fn restored_paths_are_safe() {
        assert!(safe_path("Shirts/Square/job.json").is_ok());
        for bad in [
            "../escape",
            "/etc/passwd",
            "Shirts/.hidden/x",
            "a\\b",
            "",
            "Shirts//x/../y",
        ] {
            assert!(safe_path(bad).is_err(), "{bad}");
        }
        assert_ne!(object_name(), object_name());
        assert!(object_name().starts_with("objects/"));
    }

    #[test]
    fn a_tampered_job_is_refused_on_inspection_and_replace_keeps_the_old_library() {
        let a = tempfile::tempdir().unwrap();
        let library = Library::open(a.path()).unwrap();
        let id = job(&library, &[], "Square");
        let snapshot = library.snapshot().unwrap();
        let plan_path = snapshot.join("Square/revisions/1/plan.json");
        let mut plan: serde_json::Value =
            serde_json::from_slice(&fs::read(&plan_path).unwrap()).unwrap();
        plan["operations"][0]["points"][0] = serde_json::json!([5000.0, 5000.0]);
        fs::remove_file(&plan_path).unwrap();
        fs::write(&plan_path, serde_json::to_vec(&plan).unwrap()).unwrap();
        let error = inspect(
            &snapshot,
            &CAMEO_1,
            &MachineProfile::cameo_1(),
            &library.scratch("v"),
        )
        .unwrap_err();
        assert!(error.contains("Square"), "{error}");
        assert!(
            library.details(&id).is_ok(),
            "the library itself is untouched"
        );

        // A good copy replaces the library; the old one goes to Recently deleted.
        let b = tempfile::tempdir().unwrap();
        let other = Library::open(b.path()).unwrap();
        let mine = job(&other, &[], "Mine");
        let good = library.snapshot().unwrap();
        let staged = inspect(
            &good,
            &CAMEO_1,
            &MachineProfile::cameo_1(),
            &other.scratch("v"),
        )
        .unwrap();
        assert_eq!(staged.jobs.len(), 1);
        let moved = other.scratch("restore");
        fs::rename(&good, &moved).unwrap();
        other.replace_with(&moved).unwrap();
        assert!(other.details(&id).is_ok() && other.details(&mine).is_err());
        assert_eq!(fs::read_dir(b.path().join("deleted")).unwrap().count(), 1);
    }
}

#[cfg(test)]
mod review_regressions {
    use super::*;
    #[test]
    fn review_equal_size_edits_in_one_second_must_reach_the_backup() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("job.json");
        fs::write(&file, b"old notes").unwrap();
        let stamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1700000000);
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(stamp + std::time::Duration::from_millis(100)),
            )
            .unwrap();
        let first = scan(dir.path(), &Manifest::default()).unwrap();
        let previous = Manifest {
            files: first
                .iter()
                .map(|(p, l)| {
                    (
                        p.clone(),
                        Entry {
                            size: l.size,
                            sha256: l.sha256.clone(),
                            mtime: l.mtime,
                            object: None,
                        },
                    )
                })
                .collect(),
            removed: vec![],
            directories: vec![],
        };
        fs::write(&file, b"new notes").unwrap();
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(stamp + std::time::Duration::from_millis(900)),
            )
            .unwrap();
        let second = scan(dir.path(), &previous).unwrap();
        assert_ne!(
            first["job.json"].sha256, second["job.json"].sha256,
            "a changed file inherited its old backup hash"
        );
        assert_eq!(
            diff(&previous, &second, 1700000001).upload,
            vec!["job.json"]
        );
    }
}
