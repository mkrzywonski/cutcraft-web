//! The one validation path for every uploaded job (spec §10): CutCraft, the web interface and MCP
//! all come through [`validate`] before anything is stored as cuttable.
//!
//! It guarantees **physical validation of the reviewed plan**: the uploaded plan is what gets
//! cut, after checks that it is safe for the actual cutter. It does not prove the plan was
//! correctly derived from its project, except when the uploader used this exact planner, in
//! which case the plan is regenerated and must match exactly.

use std::path::Path;

use cutcraft_device::gpgl::Model;
use cutcraft_job::{MachineProfile, Mat, ProjectFile};
use cutcraft_path::{CutPlan, PLAN_SCHEMA_VERSION};

use crate::runnable::Runnable;

/// The CutCraft build whose planner this links (recorded in `docs/provenance.md`).
pub const PLANNER: &str = "1.2.1 (b6aad51)";
/// The full CutCraft commit the planner crates came from.
pub const CUTCRAFT_REV: &str = "b6aad512f19217677c9675def794087e972fa89d";

/// Upload limits, checked before parsing more than needed.
pub const MAX_PROJECT_BYTES: usize = 48 * 1024 * 1024;
pub const MAX_PLAN_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_OPERATIONS: usize = 20_000;
pub const MAX_POINTS: usize = 1_000_000;

/// An upload that passed every check.
#[derive(Debug)]
pub struct Validated {
    pub project: ProjectFile,
    pub plan: CutPlan,
    pub runnable: Runnable,
    /// True when the plan was regenerated here and matched exactly; false when it came from
    /// another, compatible planner ("derivation not independently verified").
    pub derivation_verified: bool,
}

fn refuse<T>(why: impl Into<String>) -> Result<T, String> {
    Err(why.into())
}

/// Validates an upload: the `.cutcraft` project bytes, the plan bytes, and the CutCraft build
/// that says it made them. `model` and `machine` are the Pi's own, trusted description of the
/// cutter: the project's machine profile is uploaded data and never widens the area or sets the
/// pacing. `staging` is a private folder, unique to this request, to open the project from.
pub fn validate(
    project_bytes: &[u8],
    plan_bytes: &[u8],
    planner: &str,
    model: &Model,
    machine: &MachineProfile,
    staging: &Path,
) -> Result<Validated, String> {
    if project_bytes.len() > MAX_PROJECT_BYTES {
        return refuse("The project is too big to upload.");
    }
    if plan_bytes.len() > MAX_PLAN_BYTES {
        return refuse("The cut plan is too big to upload.");
    }

    // The project, through CutCraft's own checks: format, schema (newer ones refused) and the
    // embedded design's fingerprint.
    std::fs::create_dir_all(staging).map_err(|e| e.to_string())?;
    let file = staging.join("upload.cutcraft");
    std::fs::write(&file, project_bytes).map_err(|e| e.to_string())?;
    let project = ProjectFile::open(&file);
    let _ = std::fs::remove_file(&file);
    let project = project?;

    let schema = serde_json::from_slice::<serde_json::Value>(plan_bytes)
        .ok()
        .and_then(|v| v.get("schema_version").and_then(|s| s.as_u64()));
    if schema != Some(u64::from(PLAN_SCHEMA_VERSION)) {
        return refuse(format!(
            "This cut plan's format ({}) isn't one cameo-pi supports. Update cameo-pi, or send \
             it again from CutCraft.",
            schema.map_or("unknown".into(), |s| s.to_string())
        ));
    }
    let mut plan: CutPlan =
        serde_json::from_slice(plan_bytes).map_err(|e| format!("The cut plan is damaged: {e}"))?;

    // Limits on size before any geometry work.
    let points: usize = plan.operations.iter().map(|o| o.path().len()).sum();
    if plan.operations.len() > MAX_OPERATIONS || points > MAX_POINTS {
        return refuse("The cut plan is too large for cameo-pi.");
    }

    // The plan belongs to this project.
    let Some(sheet) = project.project.sheets.iter().find(|s| s.id == plan.sheet) else {
        return refuse("The cut plan is for a sheet that isn't in the project.");
    };
    if plan.project_revision != project.project.revision
        || plan.source_sha256 != project.project.prepared.source_sha256
    {
        return refuse("The cut plan doesn't belong to this version of the project.");
    }

    // What the person is told (material, mat, mirror) agrees with what is cut.
    if plan.material != sheet.material || plan.mat != sheet.mat || plan.mirror != sheet.mirror {
        return refuse("The cut plan's settings don't match its project.");
    }
    if plan.mat == Mat::Roll {
        return refuse("Cutting from a roll isn't supported on cameo-pi yet.");
    }
    let material = &plan.material;
    if !(model.force_range.0..=model.force_range.1).contains(&material.force)
        || !(model.speed_range.0..=model.speed_range.1).contains(&material.speed)
        || !(1..=4).contains(&material.passes)
    {
        return refuse("The material settings are outside what this cutter accepts.");
    }

    // The project's profile may describe a smaller cutter, never a bigger one.
    let claimed = &project.project.machine;
    if claimed.cutting_width_mm > machine.cutting_width_mm + 1e-6
        || claimed.mat_12x12_mm > machine.mat_12x12_mm + 1e-6
        || claimed.mat_12x24_mm > machine.mat_12x24_mm + 1e-6
    {
        return refuse("The project describes a bigger cutter or mat than this one.");
    }

    // Every point the blade goes to is finite and inside this cutter's area on this mat, by the
    // Pi's own profile.
    let area = [
        model.width_mm.min(machine.cutting_width_mm),
        machine.mat_length_mm(plan.mat),
    ];
    let mut bounds: Option<[f64; 4]> = None;
    for operation in &plan.operations {
        if !(1..=4).contains(&operation.passes) {
            return refuse("The cut plan has an operation with an unsupported number of passes.");
        }
        for &[x, y] in operation.path() {
            if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 || x > area[0] || y > area[1]
            {
                return refuse("Part of the cut is outside the cutter's area on this mat.");
            }
            let b = bounds.get_or_insert([x, y, x, y]);
            *b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
    }

    // Regenerate with this planner when the uploader says it used the same one; a mismatch is
    // refused, never excused by the physical checks.
    let derivation_verified = if planner == PLANNER {
        // Everything but the bounds and area, which are the Pi's own (set below): a plan stored
        // here already carries them.
        let mut regenerated = cutcraft_path::plan_sheet(&project.project, &plan.sheet)?;
        let mut submitted = plan.clone();
        for p in [&mut regenerated, &mut submitted] {
            p.bounds_mm = bounds;
            p.cutting_area_mm = area;
        }
        let same = serde_json::to_value(&regenerated).ok() == serde_json::to_value(&submitted).ok();
        if !same {
            return refuse(
                "The cut plan doesn't match its project as this version of CutCraft plans it.",
            );
        }
        true
    } else if planner.starts_with("1.") {
        // The compatibility policy: CutCraft 1.x plans with the supported plan schema.
        false
    } else {
        return refuse(format!(
            "Plans from CutCraft {planner:?} aren't supported. Update cameo-pi."
        ));
    };

    // The Pi's own values, never the client's.
    plan.bounds_mm = bounds;
    plan.cutting_area_mm = area;
    let runnable = Runnable::new(&plan, model, machine).map_err(|problems| {
        let lines: Vec<String> = problems.iter().map(|d| d.message.clone()).collect();
        format!("The cut can't be sent to this cutter: {}", lines.join("; "))
    })?;
    if !plan.valid {
        return refuse("CutCraft marked this plan as not ready to cut.");
    }
    Ok(Validated {
        project,
        plan,
        runnable,
        derivation_verified,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use cutcraft_device::gpgl::CAMEO_1;

    use super::*;
    use crate::fixtures::upload;

    fn check(project: &[u8], plan: &[u8], planner: &str) -> Result<Validated, String> {
        let dir = tempfile::tempdir().unwrap();
        validate(
            project,
            plan,
            planner,
            &CAMEO_1,
            &cutcraft_job::MachineProfile::cameo_1(),
            dir.path(),
        )
    }

    fn edit_plan(plan: &[u8], f: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
        let mut value: serde_json::Value = serde_json::from_slice(plan).unwrap();
        f(&mut value);
        serde_json::to_vec(&value).unwrap()
    }

    #[test]
    fn a_plan_from_this_planner_is_regenerated_and_verified() {
        let (project, plan) = upload();
        let ok = check(&project, &plan, PLANNER).unwrap();
        assert!(ok.derivation_verified);
        assert!(ok.runnable.encoded.byte_count > 0);
        // Computed here from the tool paths; for a verified plan they agree with CutCraft's
        // (the square plus its weeding box).
        let theirs: CutPlan = serde_json::from_slice(&plan).unwrap();
        let (ours, theirs) = (ok.plan.bounds_mm.unwrap(), theirs.bounds_mm.unwrap());
        for i in 0..4 {
            assert!((ours[i] - theirs[i]).abs() < 1e-6, "{ours:?} vs {theirs:?}");
        }
    }

    #[test]
    fn a_plan_from_another_1x_planner_is_accepted_but_not_verified() {
        let (project, plan) = upload();
        let ok = check(&project, &plan, "1.0.3 (5e621e1)").unwrap();
        assert!(!ok.derivation_verified);
        assert!(check(&project, &plan, "2.0.0 (abc)").is_err());
    }

    #[test]
    fn a_tampered_plan_from_this_planner_is_refused_even_if_physically_fine() {
        let (project, plan) = upload();
        let moved = edit_plan(&plan, |v| {
            let p = &mut v["operations"][0]["points"][0];
            p[0] = serde_json::json!(p[0].as_f64().unwrap() + 1.0);
        });
        let error = check(&project, &moved, PLANNER).unwrap_err();
        assert!(error.contains("doesn't match its project"), "{error}");
    }

    #[test]
    fn a_point_outside_the_cutters_area_is_refused() {
        let (project, plan) = upload();
        let outside = edit_plan(&plan, |v| {
            v["operations"][0]["points"][0] = serde_json::json!([900.0, 10.0]);
        });
        let error = check(&project, &outside, "1.0.3 (x)").unwrap_err();
        assert!(error.contains("outside the cutter's area"), "{error}");
        let nan = edit_plan(&plan, |v| {
            v["operations"][0]["points"][0] = serde_json::json!([-1.0, 10.0]);
        });
        assert!(check(&project, &nan, "1.0.3 (x)").is_err());
    }

    #[test]
    fn settings_that_disagree_with_the_project_are_refused() {
        let (project, plan) = upload();
        let force = edit_plan(&plan, |v| v["material"]["force"] = serde_json::json!(33));
        let error = check(&project, &force, "1.0.3 (x)").unwrap_err();
        assert!(error.contains("don't match its project"), "{error}");
    }

    #[test]
    fn a_plan_for_another_project_version_or_sheet_is_refused() {
        let (project, plan) = upload();
        let stale = edit_plan(&plan, |v| v["project_revision"] = serde_json::json!(999));
        assert!(check(&project, &stale, "1.0.3 (x)").is_err());
        let other = edit_plan(&plan, |v| v["sheet"] = serde_json::json!("sheet/none"));
        assert!(check(&project, &other, "1.0.3 (x)").is_err());
    }

    #[test]
    fn unsupported_schemas_damaged_projects_and_raw_streams_are_refused() {
        let (project, plan) = upload();
        let newer = edit_plan(&plan, |v| v["schema_version"] = serde_json::json!(99));
        assert!(
            check(&project, &newer, PLANNER)
                .unwrap_err()
                .contains("format (99)")
        );
        assert!(check(b"not json", &plan, PLANNER).is_err());
        // Raw GP-GL is never a job format.
        assert!(check(&project, b"\x1b\x05TT\x03FX10\x03", PLANNER).is_err());
    }

    #[test]
    fn numbers_read_back_exactly() {
        // Without serde_json's float_roundtrip these come back a step off, a real design then plans
        // differently here, and its upload is refused as not matching.
        for x in [
            47.495_200_000_000_004_f64,
            215.899_999_999_999_98,
            56.086_800_000_000_004,
        ] {
            let text = serde_json::to_string(&x).unwrap();
            assert_eq!(
                serde_json::from_str::<f64>(&text).unwrap().to_bits(),
                x.to_bits(),
                "{text}"
            );
        }
    }

    #[test]
    fn a_stored_plan_validates_again_as_planned_here() {
        // The library keeps the plan with the Pi's own bounds and area; a restore checks it again.
        let (project, plan) = upload();
        let stage = tempfile::tempdir().unwrap();
        let first = validate(
            &project,
            &plan,
            PLANNER,
            &CAMEO_1,
            &MachineProfile::cameo_1(),
            stage.path(),
        )
        .unwrap();
        assert!(first.derivation_verified);
        let mut stored_plan = first.plan.clone();
        // Bounds as the planner rounds them differ from the Pi's own; only the Pi's count.
        stored_plan.bounds_mm = stored_plan
            .bounds_mm
            .map(|[a, b, c, d]| [a, b, c + 0.0004, d]);
        let stored = serde_json::to_vec_pretty(&stored_plan).unwrap();
        let again = validate(
            &project,
            &stored,
            PLANNER,
            &CAMEO_1,
            &MachineProfile::cameo_1(),
            stage.path(),
        )
        .unwrap();
        assert!(again.derivation_verified);
        assert_eq!(again.runnable.digest, first.runnable.digest);
    }

    #[test]
    fn the_planner_origin_is_recorded() {
        // The planner crates were imported unchanged from this commit (docs/provenance.md).
        let provenance = include_str!("../../../docs/provenance.md");
        assert!(provenance.contains(&format!("`{}`", &CUTCRAFT_REV[..7])));
        assert!(provenance.contains(PLANNER));
        assert!(PLANNER.contains(&CUTCRAFT_REV[..7]));
    }

    #[test]
    fn review_uploaded_profile_must_not_enlarge_physical_mat() {
        let (project, plan) = upload();
        let mut p: serde_json::Value = serde_json::from_slice(&project).unwrap();
        p["project"]["machine"]["mat_12x12_mm"] = serde_json::json!(2000.0);
        let plan = edit_plan(&plan, |v| {
            for op in v["operations"].as_array_mut().unwrap() {
                for point in op["points"].as_array_mut().unwrap() {
                    point[1] = serde_json::json!(point[1].as_f64().unwrap() + 500.0);
                }
            }
        });
        let result = check(&serde_json::to_vec(&p).unwrap(), &plan, "1.0.3");
        assert!(
            result.is_err(),
            "accepted paths 500mm off a 12x12 mat using a client-supplied machine profile"
        );
    }

    #[test]
    fn a_forged_profile_for_either_mat_or_its_pacing_changes_nothing_sent() {
        let (project, plan) = upload();
        let forge = |field: &str, value: f64| {
            let mut p: serde_json::Value = serde_json::from_slice(&project).unwrap();
            p["project"]["machine"][field] = serde_json::json!(value);
            serde_json::to_vec(&p).unwrap()
        };
        for field in ["mat_12x12_mm", "mat_12x24_mm", "cutting_width_mm"] {
            let error = check(&forge(field, 5000.0), &plan, "1.0.3 (x)").unwrap_err();
            assert!(error.contains("bigger cutter"), "{field}: {error}");
        }
        // Pacing comes from the Pi's own profile: a forged speed changes neither the bytes nor
        // the timing the driver uses.
        let honest = check(&project, &plan, "1.0.3 (x)").unwrap();
        let forged = check(&forge("travel_mm_s", 1.0e9), &plan, "1.0.3 (x)").unwrap();
        assert_eq!(honest.runnable.digest, forged.runnable.digest);
        assert_eq!(
            forged.runnable.travel_mm_s,
            MachineProfile::cameo_1().travel_mm_s
        );
    }
}
