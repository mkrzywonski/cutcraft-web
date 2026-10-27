//! Every action cameo-pi offers, in one place (spec §5): the web interface, the protocol and MCP
//! all call these, so they share one validated path and one state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cameo_pi_core::cutter::{
    Actor, Answer, Finished, JobRef, Kind, Refusal, StartAction, StartRequest,
};
use cameo_pi_core::library::{
    HistoryEntry, JobDetails, Library, LibraryError, Listing, REVISION_SCHEMA, RevisionFiles,
    RevisionMeta,
};
use cameo_pi_core::runnable::Runnable;
use cameo_pi_core::validate;
use cameo_pi_core::worker::{Shared, lock};
use cutcraft_device::gpgl::CAMEO_1;
use cutcraft_job::{MachineProfile, ProjectFile};
use cutcraft_path::CutPlan;
use cutcraft_sim::{Media, Simulator};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::keys::Keys;
use crate::users::Users;

/// An error a client sees: an HTTP status, a stable code, and a plain message.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApiError {
    #[serde(skip)]
    pub status: u16,
    pub error: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

impl ApiError {
    pub fn new(status: u16, error: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            error: error.into(),
            message: message.into(),
            detail: None,
        }
    }
    pub fn not_found() -> Self {
        Self::new(404, "not-found", "Not found.")
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(400, "bad-request", message)
    }
}

impl From<Refusal> for ApiError {
    fn from(refusal: Refusal) -> Self {
        let message = refusal.message();
        let detail = serde_json::to_value(&refusal).ok();
        let code = detail
            .as_ref()
            .and_then(|d| d.get("refusal"))
            .and_then(|c| c.as_str())
            .unwrap_or("refused")
            .to_owned();
        Self {
            status: 409,
            error: code,
            message,
            detail,
        }
    }
}

impl From<LibraryError> for ApiError {
    fn from(error: LibraryError) -> Self {
        let message = error.to_string();
        match error {
            LibraryError::NotFound => Self::new(404, "not-found", message),
            LibraryError::BadName(_) => Self::new(400, "bad-name", message),
            LibraryError::Exists => Self::new(409, "exists", message),
            LibraryError::FolderNotEmpty => Self::new(409, "folder-not-empty", message),
            LibraryError::IntoItself => Self::new(409, "into-itself", message),
            LibraryError::Conflict(current) => Self {
                status: 409,
                error: "conflict".into(),
                message,
                detail: serde_json::to_value(&*current).ok(),
            },
            LibraryError::Io(_) => Self::new(500, "storage", message),
        }
    }
}

pub type Result<T> = std::result::Result<T, ApiError>;

/// One cutter the server knows.
pub struct CutterEntry {
    pub id: String,
    pub shared: Shared,
    /// Set for the practice cutter, so its virtual mat can be loaded from the web interface.
    pub simulator: Option<Simulator>,
}

/// A Start, as a client sends it (spec §9.2).
#[derive(Debug, Deserialize)]
pub struct StartBody {
    /// The `boot` of the state the page showed.
    pub boot: String,
    pub kind: Kind,
    pub turn: Option<String>,
    pub job_id: String,
    pub revision: u32,
    pub generation: u64,
    pub action_epoch: u64,
    pub key: String,
}

/// An upload, after the HTTP layer has read its parts.
pub struct Upload {
    pub folder: Vec<String>,
    pub name: String,
    /// Set to publish a new revision of an existing job instead of a new job.
    pub job_id: Option<String>,
    pub planner: String,
    pub project: Vec<u8>,
    pub plan: Vec<u8>,
    pub key: String,
}

/// An upload key's state: being validated now, or done with its result.
enum UploadKey {
    InFlight(String),
    Done(String, JobDetails),
}

pub struct Service {
    pub boot: String,
    pub library: Library,
    pub cutters: Vec<CutterEntry>,
    pub keys: Keys,
    pub users: Users,
    staging: PathBuf,
    /// Upload idempotency keys (spec §9.1).
    uploads: Mutex<HashMap<String, UploadKey>>,
    upload_done: std::sync::Condvar,
    /// The Pi's own description of the cutter: bounds and pacing never come from uploads.
    pub machine: MachineProfile,
    /// Held while a job's current revision is read and acted on, and while a new revision is
    /// published and handed to the cutters, so no turn is left holding a superseded revision.
    pub(crate) revisions: Mutex<()>,
    /// The backup's progress and a fetched restore waiting for Replace (spec §15).
    pub(crate) backup: crate::backups::Shared,
    /// The Pi's private state folder (`data/private`): the scheduled backup's key lives there.
    pub(crate) private: PathBuf,
    /// Each person's private area lives here (`data/people`, spec §6).
    pub(crate) people: PathBuf,
    /// The household's material and mat lists (`data/catalog`, spec §8).
    pub(crate) lists: PathBuf,
}

fn job_ref(id: &str, revision: u32) -> JobRef {
    JobRef {
        job_id: id.to_owned(),
        revision,
    }
}

impl Service {
    pub fn new(boot: String, data: &Path, cutters: Vec<CutterEntry>) -> Result<Self> {
        let library = Library::open(data)?;
        let keys = Keys::open(&data.join("private"))
            .map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
        let users = Users::open(&data.join("private"))
            .map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
        Ok(Self {
            boot,
            library,
            cutters,
            keys,
            users,
            staging: data.join(".staging").join("uploads"),
            uploads: Mutex::new(HashMap::new()),
            upload_done: std::sync::Condvar::new(),
            machine: MachineProfile::cameo_1(),
            revisions: Mutex::new(()),
            backup: Mutex::default(),
            private: data.join("private"),
            people: data.join("people"),
            lists: data.join("catalog"),
        })
    }

    pub fn cutter(&self, id: &str) -> Result<&CutterEntry> {
        self.cutters
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(ApiError::not_found)
    }

    /// Every cutter's state, as the event stream and snapshots carry it.
    pub fn cutters_view(&self) -> Vec<Value> {
        self.cutters
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "simulated": c.simulator.is_some(),
                    "view": lock(&c.shared).view(),
                })
            })
            .collect()
    }

    // --- Cutting -------------------------------------------------------------------------------

    /// Queues a job's current revision. `seen` is the revision the person was looking at: if
    /// the job has changed since, nothing is queued (spec §9.2).
    pub fn queue(
        &self,
        cutter: &str,
        job_id: &str,
        seen: Option<u32>,
        actor: Actor,
        key: &str,
    ) -> Result<Answer> {
        let entry = self.cutter(cutter)?;
        let _revisions = self.revisions.lock().unwrap_or_else(|p| p.into_inner());
        let details = self.library.details(job_id)?;
        if seen.is_some_and(|r| r != details.current_revision) {
            return Err(Refusal::StaleRevision.into());
        }
        let job = job_ref(job_id, details.current_revision);
        Ok(lock(&entry.shared).queue(job, actor, key)?)
    }

    /// The test cut or main cut for a stored revision, encoded for the cutter (spec §5.5).
    fn runnable(
        &self,
        entry: &CutterEntry,
        kind: Kind,
        job_id: &str,
        revision: u32,
    ) -> Result<Runnable> {
        let project_path = self
            .library
            .revision_file(job_id, revision, "project.cutcraft")?;
        let plan_path = self.library.revision_file(job_id, revision, "plan.json")?;
        let project = ProjectFile::open(&project_path).map_err(ApiError::bad_request)?;
        let text =
            std::fs::read(&plan_path).map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
        let plan: CutPlan = serde_json::from_slice(&text)
            .map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
        // Pacing from the Pi's own profile, never the uploaded project's.
        let machine = &self.machine;
        let plan = match kind {
            Kind::Main => plan,
            Kind::Test => {
                let taken = lock(&entry.shared)
                    .view()
                    .turn
                    .map(|t| t.taken)
                    .unwrap_or_default();
                let mut occupied: Vec<[f64; 4]> = plan.bounds_mm.into_iter().collect();
                occupied.extend(taken);
                let area = [0.0, 0.0, plan.cutting_area_mm[0], plan.cutting_area_mm[1]];
                let at = plan
                    .test_spot_mm
                    .filter(|_| occupied.len() == 1)
                    .or_else(|| cutcraft_path::test_cut_spot(area, &occupied))
                    .ok_or_else(|| {
                        ApiError::new(
                            409,
                            "no-room",
                            "There's no room left on this mat for a test cut.",
                        )
                    })?;
                cutcraft_path::test_cut_plan(&project.project, &plan.sheet, at)
                    .map_err(ApiError::bad_request)?
            }
        };
        Runnable::new(&plan, &CAMEO_1, machine).map_err(|problems| {
            let lines: Vec<String> = problems.iter().map(|d| d.message.clone()).collect();
            ApiError::bad_request(lines.join("; "))
        })
    }

    pub fn start(&self, cutter: &str, body: StartBody, actor: Actor) -> Result<Answer> {
        let entry = self.cutter(cutter)?;
        let _revisions = self.revisions.lock().unwrap_or_else(|p| p.into_inner());
        let action = StartAction {
            boot: body.boot.clone(),
            kind: body.kind,
            turn: body.turn.clone(),
            job: job_ref(&body.job_id, body.revision),
            generation: body.generation,
            action_epoch: body.action_epoch,
        };
        if let Some(answer) = lock(&entry.shared).replay_start(&body.key, &action) {
            return Ok(answer?);
        }
        // A page showing an older revision than the job's current one starts nothing (spec §9.2).
        if self.library.details(&body.job_id)?.current_revision != body.revision {
            return Err(Refusal::StaleRevision.into());
        }
        let run = self.runnable(entry, body.kind, &body.job_id, body.revision)?;
        let request = StartRequest {
            boot: body.boot,
            kind: body.kind,
            turn: body.turn,
            job: job_ref(&body.job_id, body.revision),
            generation: body.generation,
            action_epoch: body.action_epoch,
            run: Arc::new(run),
            key: body.key,
            actor,
        };
        Ok(lock(&entry.shared).start(request)?)
    }

    pub fn start_result(&self, cutter: &str, key: &str) -> Result<Answer> {
        lock(&self.cutter(cutter)?.shared)
            .start_result(key)
            .ok_or_else(|| {
                ApiError::new(
                    404,
                    "start-not-found",
                    "No accepted Start with this key is recorded.",
                )
            })
    }

    /// A person confirms the loaded mat has fresh material, though it wasn't seen loading.
    pub fn confirm_fresh(&self, cutter: &str, actor: Actor) -> Result<Answer> {
        Ok(lock(&self.cutter(cutter)?.shared).confirm_fresh(actor)?)
    }

    pub fn stop(&self, cutter: &str) -> Result<Answer> {
        Ok(lock(&self.cutter(cutter)?.shared).stop()?)
    }
    pub fn pause(&self, cutter: &str) -> Result<Answer> {
        Ok(lock(&self.cutter(cutter)?.shared).pause()?)
    }
    pub fn resume(&self, cutter: &str) -> Result<Answer> {
        Ok(lock(&self.cutter(cutter)?.shared).resume()?)
    }
    pub fn release(&self, cutter: &str, turn: &str) -> Result<Answer> {
        Ok(lock(&self.cutter(cutter)?.shared).release(turn)?)
    }
    pub fn skip(&self, cutter: &str, turn: &str) -> Result<Answer> {
        Ok(lock(&self.cutter(cutter)?.shared).skip(turn)?)
    }
    pub fn remove(&self, cutter: &str, turn: &str) -> Result<Answer> {
        Ok(lock(&self.cutter(cutter)?.shared).remove(turn)?)
    }

    /// The practice cutter's virtual mat (spec §6).
    pub fn practice(&self, cutter: &str, load: bool) -> Result<()> {
        let sim =
            self.cutter(cutter)?.simulator.as_ref().ok_or_else(|| {
                ApiError::bad_request("Only the practice cutter has a virtual mat.")
            })?;
        // One mat at a time, as on the real Cameo: a used mat must come out before a fresh one.
        let loaded = sim.snapshot().media.is_some();
        match (load, loaded) {
            (true, true) => Err(ApiError::new(
                409,
                "mat-loaded",
                "A mat is loaded already. Unload it first.",
            )),
            (false, false) => Err(ApiError::new(409, "no-mat", "No mat is loaded.")),
            (true, false) => {
                sim.load(Media::Mat12x12);
                Ok(())
            }
            (false, true) => {
                sim.unload();
                Ok(())
            }
        }
    }

    /// Records an ended execution in its job's history (the job may have been deleted since).
    pub fn record_finished(&self, cutter_name: &str, finished: &Finished, by: &str) {
        let ended = match &finished.ended {
            cameo_pi_core::cutter::Ended::Finished => "finished",
            cameo_pi_core::cutter::Ended::Stopped => "stopped",
            cameo_pi_core::cutter::Ended::Interrupted { .. } => "interrupted",
            cameo_pi_core::cutter::Ended::NotRun { .. } => "not-run",
        };
        let entry = HistoryEntry {
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            execution: finished.execution.clone(),
            cutter: cutter_name.to_owned(),
            revision: finished.job.revision,
            kind: match finished.kind {
                Kind::Test => "test".into(),
                Kind::Main => "main".into(),
            },
            by: by.to_owned(),
            ended: ended.into(),
        };
        let _ = self.library.record(&finished.job.job_id, entry);
    }

    /// Whether a job is queued or on a cutter: it can't be moved or deleted then (spec §7.4).
    fn job_in_use(&self, job_id: &str) -> bool {
        self.cutters.iter().any(|c| {
            let view = lock(&c.shared).view();
            view.turn.is_some_and(|t| t.job.job_id == job_id)
                || view.queue.iter().any(|q| q.job.job_id == job_id)
        })
    }

    // --- The library ---------------------------------------------------------------------------

    pub fn list(&self, path: &[String]) -> Result<Listing> {
        Ok(self.library.list(path)?)
    }

    pub fn create_folder(&self, parent: &[String], name: &str) -> Result<()> {
        Ok(self.library.create_folder(parent, name)?)
    }

    pub fn delete_folder(&self, path: &[String]) -> Result<()> {
        Ok(self.library.delete_folder(path)?)
    }

    /// Renames a folder. Not while one of its jobs is queued or on a cutter, as for moving it.
    pub fn rename_folder(&self, path: &[String], name: &str) -> Result<Vec<String>> {
        if self
            .library
            .jobs_under(path)?
            .iter()
            .any(|id| self.job_in_use(id))
        {
            return Err(ApiError::new(
                409,
                "in-use",
                "A job in this folder is queued or on a cutter; the folder can't be renamed now.",
            ));
        }
        Ok(self.library.rename_folder(path, name.trim())?)
    }

    /// What's in Recently deleted (spec §7.4).
    pub fn deleted(&self) -> Result<Vec<cameo_pi_core::library::DeletedItem>> {
        Ok(self.library.deleted()?)
    }

    /// Puts a deleted job back where it was. Returns its id and folder.
    pub fn restore_deleted(&self, entry: &str) -> Result<Value> {
        let (id, folder) = self.library.restore_deleted(entry)?;
        Ok(json!({ "id": id, "folder": folder }))
    }

    pub fn purge_deleted(&self, entry: &str) -> Result<()> {
        Ok(self.library.purge_deleted(entry)?)
    }

    /// A deleted job's thumbnail, drawn like a library job's.
    pub fn deleted_thumb(&self, entry: &str) -> Result<Vec<u8>> {
        let read = |file: &str| -> Result<Vec<u8>> {
            let path = self.library.deleted_file(entry, file)?;
            std::fs::read(path).map_err(|e| ApiError::new(500, "storage", e.to_string()))
        };
        let plan: CutPlan = serde_json::from_slice(&read("plan.json")?)
            .map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
        let svg = String::from_utf8_lossy(&read("preview.svg")?).into_owned();
        Ok(thumbnail(&svg, plan.bounds_mm).into_bytes())
    }

    /// Moves a folder and everything in it. Not while one of its jobs is queued or on a cutter,
    /// the same as moving the job itself. Returns the folder's new path.
    pub fn move_folder(&self, path: &[String], to: &[String]) -> Result<Vec<String>> {
        if self
            .library
            .jobs_under(path)?
            .iter()
            .any(|id| self.job_in_use(id))
        {
            return Err(ApiError::new(
                409,
                "in-use",
                "A job in this folder is queued or on a cutter; the folder can't be moved now.",
            ));
        }
        Ok(self.library.move_folder(path, to)?)
    }

    pub(crate) fn stored_plan(&self, id: &str, revision: u32) -> Result<CutPlan> {
        let path = self.library.revision_file(id, revision, "plan.json")?;
        let bytes =
            std::fs::read(path).map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| ApiError::new(500, "storage", e.to_string()))
    }

    /// A job's details, and one revision with its loading steps: the current revision unless
    /// another is asked for, so a cutter page describes exactly the revision its turn cuts.
    pub fn job(&self, id: &str, revision: Option<u32>) -> Result<Value> {
        let details = self.library.details(id)?;
        let number = revision.unwrap_or(details.current_revision);
        let revision = self.library.revision(id, number)?;
        let plan = self.stored_plan(id, number)?;
        // The settings Adjust starts from, and any drift from CutCraft's current material.
        let (settings, drift) = self.settings_of(id, number).unwrap_or((Value::Null, None));
        Ok(json!({
            "details": details,
            "folder": self.library.folder_of(id)?,
            "revision": revision,
            "settings": settings,
            "material_drift": drift,
            "steps": loading_steps(&plan),
            "loaded_steps": setup_steps(&plan),
            "mirror": plan.mirror,
            "blade": plan.material.blade,
        }))
    }

    /// What the live viewer draws (spec §12): the mat, the cutting area and every tool path in
    /// cut order, in mm.
    pub fn geometry(&self, id: &str, revision: u32) -> Result<Value> {
        Ok(crate::adjusting::geometry_of(
            &self.stored_plan(id, revision)?,
        ))
    }

    pub fn edit(
        &self,
        id: &str,
        based_on: u64,
        name: Option<&str>,
        notes: Option<&str>,
        by: &str,
    ) -> Result<JobDetails> {
        Ok(self.library.edit(id, based_on, name, notes, by)?)
    }

    pub fn move_job(&self, id: &str, to: &[String]) -> Result<()> {
        if self.job_in_use(id) {
            return Err(ApiError::new(
                409,
                "in-use",
                "This job is queued or on a cutter; it can't be moved now.",
            ));
        }
        Ok(self.library.move_job(id, to)?)
    }

    pub fn delete_job(&self, id: &str) -> Result<()> {
        if self.job_in_use(id) {
            return Err(ApiError::new(
                409,
                "in-use",
                "This job is queued or on a cutter; it can't be deleted now.",
            ));
        }
        Ok(self.library.delete_job(id)?)
    }

    /// Validates an upload (spec §10) and stores it as a new job or a new revision. Uploads are
    /// idempotent by key (spec §9.1): a retry gets the first result, and a retry that arrives
    /// while the first is still running waits for it rather than storing a second job.
    pub fn upload(&self, upload: Upload, by: &str) -> Result<JobDetails> {
        self.upload_from(upload, by, None)
    }

    /// An upload, or (with `adjusted_from`) a revision made on cameo-pi by Adjust.
    pub(crate) fn upload_from(
        &self,
        upload: Upload,
        by: &str,
        adjusted_from: Option<u32>,
    ) -> Result<JobDetails> {
        let fingerprint = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(upload.folder.join("/").as_bytes());
            h.update([0]);
            h.update(upload.name.as_bytes());
            h.update([0]);
            h.update(upload.job_id.as_deref().unwrap_or("").as_bytes());
            h.update([0]);
            h.update(&upload.project);
            h.update(&upload.plan);
            format!("{:x}", h.finalize())
        };
        let key = upload.key.clone();
        {
            let mut uploads = self.uploads.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                match uploads.get(&key) {
                    None => {
                        uploads.insert(key.clone(), UploadKey::InFlight(fingerprint.clone()));
                        break;
                    }
                    Some(UploadKey::InFlight(seen) | UploadKey::Done(seen, _))
                        if *seen != fingerprint =>
                    {
                        return Err(Refusal::KeyReused.into());
                    }
                    Some(UploadKey::Done(_, details)) => return Ok(details.clone()),
                    Some(UploadKey::InFlight(_)) => {
                        uploads = self
                            .upload_done
                            .wait(uploads)
                            .unwrap_or_else(|p| p.into_inner());
                    }
                }
            }
        }
        let result = self.upload_checked(upload, by, adjusted_from);
        let mut uploads = self.uploads.lock().unwrap_or_else(|p| p.into_inner());
        match &result {
            Ok(details) => {
                uploads.insert(key, UploadKey::Done(fingerprint, details.clone()));
            }
            // A failed upload may be tried again with the same key.
            Err(_) => {
                uploads.remove(&key);
            }
        }
        self.upload_done.notify_all();
        result
    }

    fn upload_checked(
        &self,
        upload: Upload,
        by: &str,
        adjusted_from: Option<u32>,
    ) -> Result<JobDetails> {
        // A folder of its own, so simultaneous uploads never touch each other's files.
        let stage = self.staging.join(crate::keys::random_hex(12));
        let checked = validate::validate(
            &upload.project,
            &upload.plan,
            &upload.planner,
            &CAMEO_1,
            &self.machine,
            &stage,
        )
        .map_err(|message| ApiError::new(422, "invalid-upload", message));
        let _ = std::fs::remove_dir_all(&stage);
        let checked = checked?;

        let plan_json = serde_json::to_vec_pretty(&checked.plan)
            .map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
        let preview = cutcraft_path::preview_svg(&checked.plan);
        let files = RevisionFiles {
            project: &upload.project,
            plan: &plan_json,
            preview_svg: preview.as_bytes(),
        };
        let plan = &checked.plan;
        let meta = RevisionMeta {
            schema_version: REVISION_SCHEMA,
            revision: 0,
            created_at: 0,
            created_by: by.to_owned(),
            model: CAMEO_1.name.into(),
            material: plan.material.name.clone(),
            mat: serde_json::to_value(plan.mat)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            size_mm: plan.design_size_mm,
            estimate_s: plan.estimate_s,
            digest: checked.runnable.digest.clone(),
            planner: if checked.derivation_verified {
                upload.planner.clone()
            } else {
                format!("{} (derivation not independently verified)", upload.planner)
            },
            adjusted_from,
        };
        match &upload.job_id {
            None => self
                .library
                .add_job(&upload.folder, &upload.name, &files, meta, by)
                .map_err(Into::into),
            Some(id) => {
                let _revisions = self.revisions.lock().unwrap_or_else(|p| p.into_inner());
                let details = self.library.add_revision(id, &files, meta, by)?;
                for entry in &self.cutters {
                    lock(&entry.shared).job_updated(id, details.current_revision);
                }
                Ok(details)
            }
        }
    }

    /// A revision's file for download: the project, the original design, or the cut lines.
    pub fn download(
        &self,
        id: &str,
        revision: u32,
        what: &str,
    ) -> Result<(String, String, Vec<u8>)> {
        let read = |file: &str| -> Result<Vec<u8>> {
            let path = self.library.revision_file(id, revision, file)?;
            std::fs::read(path).map_err(|e| ApiError::new(500, "storage", e.to_string()))
        };
        let name = self.library.details(id)?.name;
        match what {
            "project" => Ok((
                format!("{name}.cutcraft"),
                "application/json".into(),
                read("project.cutcraft")?,
            )),
            "preview" => Ok((
                format!("{name}.svg"),
                "image/svg+xml".into(),
                read("preview.svg")?,
            )),
            "thumb" => {
                let plan: CutPlan = serde_json::from_slice(&read("plan.json")?)
                    .map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
                let svg = String::from_utf8_lossy(&read("preview.svg")?).into_owned();
                Ok((
                    format!("{name} thumbnail.svg"),
                    "image/svg+xml".into(),
                    thumbnail(&svg, plan.bounds_mm).into_bytes(),
                ))
            }
            "cut-lines" => {
                let plan: CutPlan = serde_json::from_slice(&read("plan.json")?)
                    .map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
                Ok((
                    format!("{name} cut lines.svg"),
                    "image/svg+xml".into(),
                    cutcraft_path::preview_svg(&plan).into_bytes(),
                ))
            }
            "design" => {
                let path = self
                    .library
                    .revision_file(id, revision, "project.cutcraft")?;
                let project = ProjectFile::open(&path).map_err(ApiError::bad_request)?;
                let bytes = project.source.bytes().map_err(ApiError::bad_request)?;
                Ok((
                    project.source.file_name.clone(),
                    "application/octet-stream".into(),
                    bytes,
                ))
            }
            _ => Err(ApiError::not_found()),
        }
    }
}

fn inches(mm: f64) -> String {
    let inches = mm / 25.4;
    format!("{:.1} in", inches)
}

/// The loading steps for a plan, in plain words, as the web interface shows them at the cutter
/// (spec §12). Cameo-pi's own wording; the same facts CutCraft's Cut screen gives.
pub fn loading_steps(plan: &CutPlan) -> Vec<String> {
    let mut steps = setup_steps(plan);
    let load = "Line the mat up with the guide and press Load Mat on the Cameo, top edge first.";
    let at = steps.len() - usize::from(plan.mirror);
    steps.insert(at, load.into());
    steps
}

/// The steps that still apply once a mat is loaded: the material and its placement, the blade,
/// the material's notes and mirroring. Only the loading itself is left out.
pub fn setup_steps(plan: &CutPlan) -> Vec<String> {
    let material = &plan.material;
    let side = if material.mirror {
        "shiny side down"
    } else {
        "backing paper down"
    };
    let mat = match plan.mat {
        cutcraft_job::Mat::Mat12x12 => "12 × 12 in mat",
        cutcraft_job::Mat::Mat12x24 => "12 × 24 in mat",
        cutcraft_job::Mat::Roll => "roll",
    };
    let mut steps = Vec::new();
    match (plan.piece_mm, plan.bounds_mm) {
        (Some([w, h]), _) => steps.push(format!(
            "Put your {} × {} piece of {}, {side}, in the top-left corner of the {mat}.",
            inches(w),
            inches(h),
            material.name
        )),
        (None, Some([x0, y0, x1, y1])) => steps.push(format!(
            "Put {}, {side}, on the {mat}, covering at least {} × {} from the top-left corner \
             (the cut sits {} from the left, {} from the top).",
            material.name,
            inches(x1),
            inches(y1),
            inches(x0),
            inches(y0)
        )),
        (None, None) => steps.push(format!("Put {}, {side}, on the {mat}.", material.name)),
    }
    steps.push(format!("Set the blade to {}.", material.blade));
    if !material.notes.trim().is_empty() {
        steps.push(material.notes.trim().to_owned());
    }
    if plan.mirror {
        steps.push("This cut is mirrored, as iron-on needs.".into());
    }
    steps
}

/// The preview cropped to the design, for library lists: the whole mat would leave a small
/// design a speck. Only the outer `<svg>` tag is replaced; the drawing is the Pi's own.
pub fn thumbnail(svg: &str, bounds: Option<[f64; 4]>) -> String {
    let (Some([x0, y0, x1, y1]), Some(start)) = (bounds, svg.find("<svg")) else {
        return svg.to_owned();
    };
    let Some(end) = svg[start..].find('>').map(|i| start + i + 1) else {
        return svg.to_owned();
    };
    let margin = 4.0;
    format!(
        r#"{}<svg xmlns="http://www.w3.org/2000/svg" viewBox="{} {} {} {}" font-family="sans-serif">{}"#,
        &svg[..start],
        x0 - margin,
        y0 - margin,
        x1 - x0 + 2.0 * margin,
        y1 - y0 + 2.0 * margin,
        &svg[end..]
    )
}
