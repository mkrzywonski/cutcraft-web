//! Adjust on the Pi (spec §12.1), and the material and mat lists CutCraft sends (spec §11). The
//! changes themselves are `cameo_pi_core::adjust`; this reads and stores them.

use cameo_pi_core::adjust::{self, Adjustments, Catalog};
use cameo_pi_core::library::JobDetails;
use cameo_pi_core::validate::PLANNER;
use cutcraft_device::gpgl::CAMEO_1;
use cutcraft_job::ProjectFile;
use cutcraft_path::CutPlan;
use serde_json::{Value, json};

use crate::service::{ApiError, Service, Upload};

type Result<T> = std::result::Result<T, ApiError>;

/// Serialises reading, changing and writing `catalog.json` (one server per data folder).
static CATALOG: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `private/catalog.json`: each CutCraft's lists, by the key that sent them.
#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    senders: Vec<Sent>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Sent {
    key: String,
    catalog: Catalog,
}

fn storage(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(500, "storage", error.to_string())
}

/// What the viewer draws: the mat, the cutting area and every tool path in cut order, in mm.
pub fn geometry_of(plan: &CutPlan) -> Value {
    let operations: Vec<Value> = plan
        .operations
        .iter()
        .map(|o| {
            json!({
                "kind": o.kind,
                "colour": o.colour,
                "passes": o.passes,
                "points": o.path(),
            })
        })
        .collect();
    json!({
        "mat_mm": plan.mat_mm,
        "cutting_area_mm": plan.cutting_area_mm,
        "bounds_mm": plan.bounds_mm,
        "operations": operations,
        "units_per_mm": CAMEO_1.units_per_mm,
    })
}

fn problems(plan: &CutPlan) -> Vec<String> {
    plan.diagnostics
        .iter()
        .filter(|d| d.severity == cutcraft_job::Severity::Blocking)
        .map(|d| d.message.clone())
        .collect()
}

impl Service {
    fn catalog_path(&self) -> std::path::PathBuf {
        self.private.join("catalog.json")
    }

    /// Every CutCraft's lists, by the access key that sent them ("" for lists from before
    /// they were kept apart, "page" for lists sent from a browser).
    fn catalogs(&self) -> Vec<Sent> {
        let Ok(bytes) = std::fs::read(self.catalog_path()) else {
            return Vec::new();
        };
        if let Ok(stored) = serde_json::from_slice::<Stored>(&bytes) {
            return stored.senders;
        }
        // One list, as 0.14.0 kept it.
        serde_json::from_slice::<Catalog>(&bytes)
            .map(|catalog| {
                vec![Sent {
                    key: String::new(),
                    catalog,
                }]
            })
            .unwrap_or_default()
    }

    /// The lists of every CutCraft still paired, as one (spec §12.1): a computer's list goes
    /// when its key is removed.
    pub fn catalog(&self) -> Catalog {
        let keys: Vec<String> = self.keys.list().into_iter().map(|k| k.id).collect();
        let senders = self.catalogs();
        let keyed = senders.iter().any(|s| keys.contains(&s.key));
        let lists: Vec<Catalog> = senders
            .into_iter()
            .filter(|s| match s.key.as_str() {
                // Older lists count until a paired CutCraft sends its own.
                "" => !keyed,
                "page" => true,
                key => keys.iter().any(|k| k == key),
            })
            .map(|s| s.catalog)
            .collect();
        Catalog::merge(&lists)
    }

    /// Keeps the lists a CutCraft sent, replacing only that computer's earlier ones. Two
    /// computers syncing at once are taken one at a time, so neither's lists are lost.
    pub fn set_catalog(&self, mut catalog: Catalog, key: &str) -> Result<Value> {
        catalog
            .check()
            .map_err(|why| ApiError::new(400, "bad-catalog", why))?;
        catalog.from = catalog.from.chars().take(80).collect();
        catalog.at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        catalog.material_from.clear();
        catalog.senders.clear();
        let (materials, mats, at) = (catalog.materials.len(), catalog.mats.len(), catalog.at);
        let _one_at_a_time = CATALOG.lock().unwrap_or_else(|p| p.into_inner());
        let mut senders = self.catalogs();
        senders.retain(|s| s.key != key);
        senders.push(Sent {
            key: key.to_owned(),
            catalog,
        });
        let bytes = serde_json::to_vec_pretty(&Stored { senders }).map_err(storage)?;
        std::fs::create_dir_all(&self.private).map_err(storage)?;
        cameo_pi_core::journal::write_atomic(&self.catalog_path(), &bytes).map_err(storage)?;
        Ok(json!({ "materials": materials, "mats": mats, "at": at }))
    }

    pub(crate) fn project_of(&self, id: &str, revision: u32) -> Result<ProjectFile> {
        let path = self
            .library
            .revision_file(id, revision, "project.cutcraft")?;
        let bytes = std::fs::read(path).map_err(storage)?;
        serde_json::from_slice(&bytes).map_err(storage)
    }

    /// The revision's adjustable settings, and whether its material has changed in CutCraft's
    /// list since (spec §11, settings drift).
    pub(crate) fn settings_of(&self, id: &str, revision: u32) -> Result<(Value, Option<Value>)> {
        let file = self.project_of(id, revision)?;
        let plan = self.stored_plan(id, revision)?;
        let catalog = self.catalog();
        let drift = adjust::drift(&file.project, &catalog).map(|m| json!(m));
        Ok((adjust::settings(&file.project, &plan), drift))
    }

    /// What `adjustments` would make of a revision: the new cut and its problems. Nothing is
    /// stored.
    pub fn adjust_preview(
        &self,
        id: &str,
        revision: u32,
        adjustments: &Adjustments,
    ) -> Result<Value> {
        let mut file = self.project_of(id, revision)?;
        let plan = adjust::apply(&mut file.project, adjustments, &self.catalog())
            .map_err(|why| ApiError::new(422, "bad-adjustment", why))?;
        Ok(json!({
            "valid": plan.valid,
            "problems": problems(&plan),
            "estimate_s": plan.estimate_s,
            "bounds_mm": plan.bounds_mm,
            "settings": adjust::settings(&file.project, &plan),
            "geometry": geometry_of(&plan),
        }))
    }

    /// Saves an adjustment as the job's new revision: re-planned here, then checked like any
    /// upload (spec §10). Refused when the job has a newer revision than the one adjusted, or when
    /// the result can't be cut.
    pub fn adjust_save(
        &self,
        id: &str,
        revision: u32,
        adjustments: &Adjustments,
        key: String,
        by: &str,
    ) -> Result<JobDetails> {
        let details = self.library.details(id)?;
        if details.current_revision != revision {
            return Err(ApiError::new(
                409,
                "stale-revision",
                "The job has a newer revision since this one was opened. Open it again.",
            ));
        }
        let mut file = self.project_of(id, revision)?;
        let plan = adjust::apply(&mut file.project, adjustments, &self.catalog())
            .map_err(|why| ApiError::new(422, "bad-adjustment", why))?;
        if !plan.valid {
            return Err(ApiError::new(
                422,
                "cannot-cut",
                format!("This can't be cut: {}", problems(&plan).join(" ")),
            ));
        }
        let upload = Upload {
            folder: Vec::new(),
            name: details.name,
            job_id: Some(id.to_owned()),
            planner: PLANNER.into(),
            project: serde_json::to_vec(&file).map_err(storage)?,
            plan: serde_json::to_vec(&plan).map_err(storage)?,
            key,
        };
        self.upload_from(upload, by, Some(revision))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two computers syncing at once lose neither's lists (from Codex's review, finding 6).
    #[test]
    fn simultaneous_catalogs_keep_every_sender() {
        let dir = tempfile::tempdir().unwrap();
        let service = std::sync::Arc::new(Service::new("boot".into(), dir.path(), vec![]).unwrap());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(16));
        let workers: Vec<_> = (0..16)
            .map(|i| {
                let (service, barrier) = (service.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    let catalog = Catalog {
                        from: format!("computer-{i}"),
                        ..Catalog::default()
                    };
                    service.set_catalog(catalog, &format!("key-{i}"))
                })
            })
            .collect();
        let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        let failed = results.iter().filter(|r| r.is_err()).count();
        assert_eq!((failed, service.catalogs().len()), (0, 16), "{results:?}");
    }
}
