//! Immutable, validated cut copies (spec §14). Kept apart from sessions and the editable
//! library. A copy and its request receipt are one synced atomic record; retries never rewrite
//! it. This store does not queue or start anything.

use std::path::PathBuf;
use std::sync::Mutex;

use cameo_pi_core::{journal::write_atomic, validate};
use cutcraft_device::gpgl::CAMEO_1;
use cutcraft_job::{MachineProfile, ProjectFile};
use cutcraft_path::CutPlan;
use cutcraft_web_protocol::{CommandReply, CommandRequest, CutCopy, codes};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::service::{ApiError, Result};

const PREFIX: &str = "copy-";
const MAX_RECORD: u64 = 128 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
pub(crate) struct Stored {
    schema_version: u32,
    owner: String,
    pub request: CommandRequest,
    pub view: CutCopy,
    pub project: ProjectFile,
    pub plan: CutPlan,
}

impl Stored {
    pub fn reply(&self) -> CommandReply {
        CommandReply::Applied {
            revision: self
                .request
                .revision
                .expect("copy requests name a revision"),
            value: serde_json::json!({"copy": self.view}),
        }
    }
}

pub struct Store {
    dir: PathBuf,
    /// Only storage publication: validation runs before this lock, and Stop never takes it.
    publish: Mutex<()>,
}

fn storage(e: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        500,
        "copy-storage",
        format!("The cut copy couldn't be read or written: {e}"),
    )
}

fn id_for(owner: &str, key: &str) -> String {
    let encoded = serde_json::to_vec(&(owner, key)).expect("strings serialize");
    format!("{PREFIX}{:x}", Sha256::digest(encoded))
}

pub fn is_copy(id: &str) -> bool {
    id.starts_with(PREFIX)
}

impl Store {
    pub fn open(dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir).map_err(storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .map_err(storage)?;
        }
        if let Some(parent) = dir.parent() {
            std::fs::File::open(parent)
                .and_then(|f| f.sync_all())
                .map_err(storage)?;
        }
        Ok(Self {
            dir,
            publish: Mutex::new(()),
        })
    }

    fn path(&self, id: &str) -> Result<PathBuf> {
        let hex = id.strip_prefix(PREFIX).ok_or_else(ApiError::not_found)?;
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ApiError::not_found());
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    pub fn read(&self, id: &str) -> Result<Option<Stored>> {
        let path = self.path(id)?;
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(storage(e)),
        };
        if !meta.is_file() || meta.len() > MAX_RECORD {
            return Err(storage("invalid cut copy"));
        }
        let bytes = std::fs::read(path).map_err(storage)?;
        let stored: Stored = serde_json::from_slice(&bytes).map_err(storage)?;
        if stored.schema_version != 1
            || stored.view.id != id
            || id_for(&stored.owner, &stored.request.key) != id
            || stored.request.revision.is_none()
            || stored.view.revision != 1
        {
            return Err(storage("unsupported or damaged cut copy"));
        }
        Ok(Some(stored))
    }

    pub fn lookup(&self, owner: &str, key: &str) -> Result<Option<Stored>> {
        self.read(&id_for(owner, key))
    }

    pub fn prepare(
        &self,
        owner: &str,
        request: CommandRequest,
        name: String,
        project: &[u8],
        plan: &[u8],
        machine: &MachineProfile,
    ) -> Result<CommandReply> {
        if request.id != "cut.copy" || request.revision.is_none() {
            return Err(ApiError::bad_request(
                "Give a keyed cut.copy request with a revision.",
            ));
        }
        // A unique scratch directory, never a path supplied by a client.
        let stage = self
            .dir
            .join(format!(".validate-{}", crate::keys::random_hex(12)));
        let checked =
            validate::validate(project, plan, validate::PLANNER, &CAMEO_1, machine, &stage);
        let _ = std::fs::remove_dir_all(&stage);
        let (project, plan, digest, test_only) = match checked {
            Ok(checked) => (
                checked.project,
                checked.plan,
                checked.runnable.digest,
                false,
            ),
            Err(_) if request.params["test"] == true => {
                let project: ProjectFile = serde_json::from_slice(project)
                    .map_err(|e| ApiError::bad_request(e.to_string()))?;
                project.source.bytes().map_err(ApiError::bad_request)?;
                let sheet = project
                    .project
                    .sheets
                    .first()
                    .ok_or_else(|| ApiError::bad_request("Open a design first."))?;
                let plan = cutcraft_path::plan_sheet(&project.project, &sheet.id)
                    .map_err(ApiError::bad_request)?;
                let at = cutcraft_control::next_test_spot(&plan, &[]).ok_or_else(|| {
                    ApiError::new(
                        422,
                        "no-room",
                        "There is no room for a test pattern on this material.",
                    )
                })?;
                let test = cutcraft_path::test_cut_plan(&project.project, &sheet.id, at)
                    .map_err(ApiError::bad_request)?;
                cameo_pi_core::runnable::Runnable::new(&test, &CAMEO_1, machine).map_err(|ds| {
                    ApiError::new(
                        422,
                        "not-cut-ready",
                        ds.iter()
                            .map(|d| d.message.clone())
                            .collect::<Vec<_>>()
                            .join(" "),
                    )
                })?;
                let digest = format!(
                    "{:x}",
                    Sha256::digest(serde_json::to_vec(&plan).map_err(storage)?)
                );
                (project, plan, digest, true)
            }
            Err(e) => return Err(ApiError::new(422, "not-cut-ready", e)),
        };
        let id = id_for(owner, &request.key);
        let view = CutCopy {
            test_only,
            id: id.clone(),
            name,
            revision: 1,
            project_revision: plan.project_revision,
            sheet: plan.sheet.clone(),
            material: plan.material.name.clone(),
            mirrored: plan.mirror,
            digest,
        };
        let stored = Stored {
            schema_version: 1,
            owner: owner.into(),
            request,
            view,
            project,
            plan,
        };
        let _publication = self.publish.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(first) = self.read(&id)? {
            return if first.request == stored.request {
                Ok(first.reply())
            } else {
                Ok(CommandReply::Refused {
                    code: codes::KEY_REUSED.into(),
                    message: "That cut copy was already prepared differently.".into(),
                })
            };
        }
        let bytes = serde_json::to_vec(&stored).map_err(storage)?;
        if bytes.len() as u64 > MAX_RECORD {
            return Err(ApiError::new(
                422,
                "copy-too-large",
                "The cut copy is too large.",
            ));
        }
        write_atomic(&self.path(&id)?, &bytes).map_err(storage)?;
        Ok(stored.reply())
    }

    pub fn get(&self, id: &str, revision: u32) -> Result<Stored> {
        if revision != 1 {
            return Err(ApiError::not_found());
        }
        self.read(id)?.ok_or_else(ApiError::not_found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_ids_cannot_escape_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("copies")).unwrap();
        for id in ["copy-../../secret", "copy-/tmp/file", "copy-abc", "job-1"] {
            assert_eq!(store.read(id).err().unwrap().status, 404);
        }
    }
    #[test]
    fn copy_receipts_survive_a_store_restart_and_remain_scoped_to_the_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("copies");
        let store = Store::open(path.clone()).unwrap();
        let (project, plan) = cameo_pi_core::fixtures::upload();
        let request = CommandRequest {
            id: "cut.copy".into(),
            params: serde_json::json!({}),
            key: "copy".into(),
            incarnation: "first".into(),
            revision: Some(1),
        };
        let first = store
            .prepare(
                "owner-a",
                request.clone(),
                "Square".into(),
                &project,
                &plan,
                &MachineProfile::cameo_1(),
            )
            .unwrap();
        drop(store);
        let store = Store::open(path).unwrap();
        assert_eq!(
            store.lookup("owner-a", "copy").unwrap().unwrap().reply(),
            first
        );
        assert!(store.lookup("owner-b", "copy").unwrap().is_none());
        let mut changed = request;
        changed.revision = Some(2);
        assert!(
            matches!(store.prepare("owner-a", changed, "Different".into(), &project, &plan, &MachineProfile::cameo_1()).unwrap(),
            CommandReply::Refused { code, .. } if code == codes::KEY_REUSED)
        );
        assert_eq!(
            store.lookup("owner-a", "copy").unwrap().unwrap().reply(),
            first
        );
    }
}
