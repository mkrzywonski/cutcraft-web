//! Adjust (spec §12.1): changing a library job's cut settings on the Pi, without CutCraft. The
//! design and how it was made cut-ready never change: only the sheet's settings, from which the
//! Pi's own planner (`plan_sheet`, CutCraft's) makes the cut again. A saved adjustment is a new
//! revision, checked like any upload.
//!
//! The material and mat lists come from CutCraft ([`Catalog`]); a job always carries its own
//! material, so Adjust works without them.

use cutcraft_job::project::PLACEMENT_CLEARANCE_MM;
use cutcraft_job::{Mat, MatProfile, Material, Placement, Project};
use cutcraft_path::CutPlan;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// CutCraft's material and mat lists, as last sent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub materials: Vec<Material>,
    pub mats: Vec<MatProfile>,
    /// Which CutCraft sent them ("CutCraft on the kitchen MacBook").
    #[serde(default)]
    pub from: String,
    /// When, in seconds since 1970.
    #[serde(default)]
    pub at: u64,
    /// In merged lists: which CutCraft each material came from, by name.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub material_from: std::collections::BTreeMap<String, String>,
    /// In merged lists: every CutCraft whose lists are in, with when each sent them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub senders: Vec<Sender>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sender {
    pub from: String,
    pub at: u64,
}

impl Catalog {
    /// Several CutCraft computers' lists as one (spec §12.1): everything any of them has. When two
    /// have a material or mat of the same name, the one sent most recently wins.
    pub fn merge(lists: &[Catalog]) -> Catalog {
        use std::collections::BTreeMap;
        let mut ordered: Vec<&Catalog> = lists.iter().collect();
        ordered.sort_by_key(|c| c.at);
        let mut materials: BTreeMap<String, (Material, String)> = BTreeMap::new();
        let mut mats: BTreeMap<String, MatProfile> = BTreeMap::new();
        for list in &ordered {
            for m in &list.materials {
                materials.insert(m.name.clone(), (m.clone(), list.from.clone()));
            }
            for m in &list.mats {
                mats.insert(m.name.clone(), m.clone());
            }
        }
        let latest = ordered.last();
        Catalog {
            material_from: materials
                .iter()
                .map(|(n, (_, from))| (n.clone(), from.clone()))
                .collect(),
            materials: materials.into_values().map(|(m, _)| m).collect(),
            mats: mats.into_values().collect(),
            from: latest.map(|c| c.from.clone()).unwrap_or_default(),
            at: latest.map_or(0, |c| c.at),
            senders: ordered
                .iter()
                .map(|c| Sender {
                    from: c.from.clone(),
                    at: c.at,
                })
                .collect(),
        }
    }
}

pub const MAX_MATERIALS: usize = 500;
pub const MAX_MATS: usize = 100;

impl Catalog {
    pub fn check(&self) -> Result<(), String> {
        if self.materials.len() > MAX_MATERIALS || self.mats.len() > MAX_MATS {
            return Err("The material or mat list is longer than cameo-pi keeps.".into());
        }
        if self.materials.iter().any(|m| m.name.trim().is_empty())
            || self.mats.iter().any(|m| m.name.trim().is_empty())
        {
            return Err("Every material and mat needs a name.".into());
        }
        Ok(())
    }

    /// A mat by name: CutCraft's list first, then the standard presets.
    pub fn mat(&self, name: &str) -> Option<MatProfile> {
        self.mats
            .iter()
            .find(|m| m.name == name)
            .cloned()
            .or_else(|| MatProfile::presets().into_iter().find(|m| m.name == name))
    }
}

/// Where a corner preset puts the cut, inside the piece or the mat's sticky area.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Anchor {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Centre,
}

/// What to change; everything left out stays as the revision has it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Adjustments {
    /// The design's size as a factor of its original size (1 = as imported).
    pub scale: Option<f64>,
    /// The design's width in mm (instead of `scale`); the height follows.
    pub width_mm: Option<f64>,
    /// Where the design's top-left corner goes, from the cutter's start corner.
    pub offset_mm: Option<[f64; 2]>,
    /// A corner (or the middle) of the piece or the mat, after everything else.
    pub anchor: Option<Anchor>,
    /// Degrees, clockwise.
    pub rotation_deg: Option<f64>,
    pub mirror: Option<bool>,
    /// A material from CutCraft's list, by name: its settings, and its mirroring.
    pub material: Option<String>,
    pub blade: Option<u8>,
    pub force: Option<u8>,
    pub speed: Option<u8>,
    pub passes: Option<u8>,
    pub overcut_mm: Option<f64>,
    /// A mat by name: from CutCraft's list, or a standard one.
    pub mat: Option<String>,
    /// The piece of material's size, or `null` for none (the whole mat).
    #[serde(default, with = "nullable")]
    pub piece_mm: Option<Option<[f64; 2]>>,
    /// Where the piece's top-left corner sits, or `null` for the start corner.
    #[serde(default, with = "nullable")]
    pub piece_at_mm: Option<Option<[f64; 2]>>,
    pub weeding_box: Option<bool>,
    pub weeding_margin_mm: Option<f64>,
    /// Registration marks (layering marks), for lining up colours cut on separate sheets.
    pub layering_marks: Option<bool>,
}

/// Tells "left out" (keep) from `null` (clear) for the optional fields.
mod nullable {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<Option<[f64; 2]>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        value.flatten().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Option<[f64; 2]>>, D::Error> {
        Ok(Some(Option::deserialize(d)?))
    }
}

fn check_range<T: PartialOrd + std::fmt::Display + Copy>(
    name: &str,
    value: T,
    low: T,
    high: T,
) -> Result<T, String> {
    if value < low || value > high {
        return Err(format!("{name} must be between {low} and {high}."));
    }
    Ok(value)
}

fn finite(name: &str, values: &[f64]) -> Result<(), String> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        Err(format!("{name} must be a number."))
    }
}

/// Applies `a` to the project's first sheet and plans it with the Pi's planner. The plan may be
/// invalid (the design no longer fits, say): its diagnostics say why, and saving is refused.
pub fn apply(project: &mut Project, a: &Adjustments, catalog: &Catalog) -> Result<CutPlan, String> {
    let id = project
        .sheets
        .first()
        .ok_or("The job has no sheet.")?
        .id
        .clone();
    {
        let sheet = &mut project.sheets[0];
        if let Some(name) = &a.material {
            let material = catalog
                .materials
                .iter()
                .find(|m| &m.name == name)
                .ok_or_else(|| format!("{name:?} isn't in the material list from CutCraft."))?;
            sheet.material = material.clone();
            sheet.mirror = material.mirror;
        }
        let m = &mut sheet.material;
        if let Some(v) = a.blade {
            m.blade = check_range("The blade setting", v, 1, 10)?;
        }
        // The cutter's own ranges are checked again with the plan (spec §10).
        if let Some(v) = a.force {
            m.force = check_range("Force", v, 1, 33)?;
        }
        if let Some(v) = a.speed {
            m.speed = check_range("Speed", v, 1, 10)?;
        }
        if let Some(v) = a.passes {
            m.passes = check_range("Passes", v, 1, 4)?;
        }
        if let Some(v) = a.overcut_mm {
            finite("Overcut", &[v])?;
            m.overcut_mm = check_range("Overcut (mm)", v, 0.0, 5.0)?;
        }
        if let Some(v) = a.mirror {
            sheet.mirror = v;
        }
        if let Some(name) = &a.mat {
            let mat = catalog
                .mat(name)
                .ok_or_else(|| format!("There's no mat called {name:?}."))?;
            if mat.class == Mat::Roll {
                return Err("Cutting from a roll isn't supported on cameo-pi yet.".into());
            }
            sheet.mat = mat.class;
            sheet.mat_profile = Some(mat);
        }
        if let Some(piece) = a.piece_mm {
            if let Some([w, h]) = piece {
                finite("The piece's size", &[w, h])?;
                if w <= 0.0 || h <= 0.0 {
                    return Err("The piece needs a width and a height.".into());
                }
            }
            sheet.piece_mm = piece;
        }
        if let Some(at) = a.piece_at_mm {
            if let Some([x, y]) = at {
                finite("The piece's position", &[x, y])?;
            }
            sheet.piece_at_mm = at;
        }
        if let Some(v) = a.weeding_box {
            sheet.weeding_box = v;
        }
        if let Some(v) = a.weeding_margin_mm {
            finite("The weeding margin", &[v])?;
            sheet.weeding_margin_mm = check_range("The weeding margin (mm)", v, 0.0, 50.0)?;
        }
        if let Some(v) = a.layering_marks {
            sheet.layering_marks = v;
        }
    }
    // Size: a factor, or a width worked out from the design's size now.
    let scale = match (a.scale, a.width_mm) {
        (Some(_), Some(_)) => return Err("Give the size as a scale or a width, not both.".into()),
        (Some(scale), None) => Some(scale),
        (None, Some(width)) => {
            finite("The width", &[width])?;
            let plan = cutcraft_path::plan_sheet(project, &id)?;
            let [w, _] = plan.design_size_mm.ok_or("The design has no size.")?;
            if w <= 0.0 || width <= 0.0 {
                return Err("The width must be more than 0.".into());
            }
            Some(project.scale * width / w)
        }
        (None, None) => None,
    };
    if let Some(scale) = scale {
        project.set_scale(scale)?;
    }
    if a.offset_mm.is_some() || a.rotation_deg.is_some() {
        let current = &project.sheets[0];
        let offset = a.offset_mm.unwrap_or(current.placement.offset_mm);
        let rotation = a
            .rotation_deg
            .unwrap_or(current.placement.rotation_deg)
            .rem_euclid(360.0);
        let mat = current.mat;
        project.set_placement(
            &id,
            Placement {
                offset_mm: offset,
                rotation_deg: rotation,
            },
            mat,
        )?;
    }
    let mut plan = cutcraft_path::plan_sheet(project, &id)?;
    if let Some(anchor) = a.anchor
        && let Some([bx0, by0, bx1, by1]) = plan.bounds_mm
    {
        let [x0, y0, x1, y1] = fit_area(project, &plan);
        let (w, h) = (bx1 - bx0, by1 - by0);
        let (tx, ty) = match anchor {
            Anchor::TopLeft => (x0, y0),
            Anchor::TopRight => (x1 - w, y0),
            Anchor::BottomLeft => (x0, y1 - h),
            Anchor::BottomRight => (x1 - w, y1 - h),
            Anchor::Centre => ((x0 + x1 - w) / 2.0, (y0 + y1 - h) / 2.0),
        };
        let current = &project.sheets[0];
        let [ox, oy] = current.placement.offset_mm;
        let placement = Placement {
            offset_mm: [ox + tx - bx0, oy + ty - by0],
            rotation_deg: current.placement.rotation_deg,
        };
        let mat = current.mat;
        project.set_placement(&id, placement, mat)?;
        plan = cutcraft_path::plan_sheet(project, &id)?;
    }
    Ok(plan)
}

/// Where cuts may go, as CutCraft's Fit does: the piece when one is given, otherwise the mat's
/// sticky area, both within the cutting area and less the placement clearance.
fn fit_area(project: &Project, plan: &CutPlan) -> [f64; 4] {
    let sheet = &project.sheets[0];
    let [aw, ah] = plan.cutting_area_mm;
    let [x0, y0, x1, y1] = if plan.piece_mm.is_some() {
        plan.usable_area_mm()
    } else {
        let [x0, y0, x1, y1] = sheet.mat_profile().sticky_in_cut();
        [x0.max(0.0), y0.max(0.0), x1.min(aw), y1.min(ah)]
    };
    let c = PLACEMENT_CLEARANCE_MM;
    [x0 + c, y0 + c, x1 - c, y1 - c]
}

/// The first sheet's adjustable settings, as the Adjust screen and MCP show them.
pub fn settings(project: &Project, plan: &CutPlan) -> Value {
    let Some(sheet) = project.sheets.first() else {
        return json!({});
    };
    json!({
        "scale": project.scale,
        "design_size_mm": plan.design_size_mm,
        "offset_mm": sheet.placement.offset_mm,
        "rotation_deg": sheet.placement.rotation_deg,
        "mirror": sheet.mirror,
        "material": sheet.material,
        "mat": sheet.mat_profile().name,
        "mat_size": sheet.mat,
        "piece_mm": sheet.piece_mm,
        "piece_at_mm": sheet.piece_at_mm,
        "weeding_box": sheet.weeding_box,
        "weeding_margin_mm": sheet.weeding_margin_mm,
        "layering_marks": sheet.layering_marks,
    })
}

/// Whether CutCraft's list now has different settings for the job's material (spec §11,
/// "settings drift"): the list's version, when it does.
pub fn drift<'a>(project: &Project, catalog: &'a Catalog) -> Option<&'a Material> {
    let mine = &project.sheets.first()?.material;
    let theirs = catalog.materials.iter().find(|m| m.name == mine.name)?;
    let same = mine.blade == theirs.blade
        && mine.force == theirs.force
        && mine.speed == theirs.speed
        && mine.passes == theirs.passes
        && mine.mirror == theirs.mirror
        && mine.offset_method == theirs.offset_method
        && mine.offset_mm == theirs.offset_mm
        && mine.overcut_mm == theirs.overcut_mm;
    (!same).then_some(theirs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutcraft_job::ProjectFile;

    fn project() -> Project {
        let (bytes, _) = crate::fixtures::upload();
        serde_json::from_slice::<ProjectFile>(&bytes)
            .unwrap()
            .project
    }

    #[test]
    fn nothing_changed_plans_as_before() {
        let mut p = project();
        let before = cutcraft_path::plan_sheet(&p, &p.sheets[0].id.clone()).unwrap();
        let after = apply(&mut p, &Adjustments::default(), &Catalog::default()).unwrap();
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after).unwrap()
        );
    }

    #[test]
    fn size_position_rotation_and_mirror_change_the_cut() {
        let mut p = project();
        let plan = apply(
            &mut p,
            &Adjustments {
                width_mm: Some(80.0),
                ..Default::default()
            },
            &Catalog::default(),
        )
        .unwrap();
        let [w, _] = plan.design_size_mm.unwrap();
        assert!((w - 80.0).abs() < 0.01, "{w}");
        let plan = apply(
            &mut p,
            &Adjustments {
                anchor: Some(Anchor::BottomRight),
                ..Default::default()
            },
            &Catalog::default(),
        )
        .unwrap();
        let [_, _, x1, y1] = plan.bounds_mm.unwrap();
        let area = fit_area(&p, &plan);
        assert!(
            (x1 - area[2]).abs() < 0.01 && (y1 - area[3]).abs() < 0.01,
            "{:?} {area:?}",
            plan.bounds_mm
        );
        assert!(plan.valid, "{:?}", plan.diagnostics);
        let plan = apply(
            &mut p,
            &Adjustments {
                rotation_deg: Some(-90.0),
                mirror: Some(true),
                ..Default::default()
            },
            &Catalog::default(),
        )
        .unwrap();
        assert_eq!(p.sheets[0].placement.rotation_deg, 270.0);
        assert!(p.sheets[0].mirror && plan.mirror);
    }

    #[test]
    fn a_material_from_the_list_brings_its_settings_and_overrides_follow() {
        let mut p = project();
        let mut iron_on = Material::defaults().into_iter().find(|m| m.mirror).unwrap();
        iron_on.force = 20;
        let catalog = Catalog {
            materials: vec![iron_on.clone()],
            ..Default::default()
        };
        let a = Adjustments {
            material: Some(iron_on.name.clone()),
            passes: Some(2),
            ..Default::default()
        };
        let plan = apply(&mut p, &a, &catalog).unwrap();
        assert_eq!((plan.material.force, plan.material.passes), (20, 2));
        assert!(plan.mirror, "the material's mirroring comes with it");
        assert!(
            drift(&p, &catalog).is_some(),
            "two passes isn't the list's setting"
        );
        let error = apply(
            &mut p,
            &Adjustments {
                material: Some("Unknown".into()),
                ..Default::default()
            },
            &catalog,
        )
        .unwrap_err();
        assert!(error.contains("material list"));
        assert!(
            apply(
                &mut p,
                &Adjustments {
                    force: Some(99),
                    ..Default::default()
                },
                &catalog
            )
            .is_err()
        );
    }

    #[test]
    fn a_design_too_big_for_its_piece_plans_invalid_with_a_reason() {
        let mut p = project();
        let plan = apply(
            &mut p,
            &Adjustments {
                piece_mm: Some(Some([20.0, 20.0])),
                ..Default::default()
            },
            &Catalog::default(),
        )
        .unwrap();
        assert!(!plan.valid);
        assert!(!plan.diagnostics.is_empty());
        let cleared: Adjustments = serde_json::from_str(r#"{"piece_mm": null}"#).unwrap();
        assert_eq!(cleared.piece_mm, Some(None));
        let plan = apply(&mut p, &cleared, &Catalog::default()).unwrap();
        assert!(plan.valid);
    }

    #[test]
    fn several_computers_lists_merge_and_the_newest_wins_a_name() {
        let defaults = Material::defaults();
        let (mut a, mut b) = (defaults[0].clone(), defaults[0].clone());
        a.force = 10;
        b.force = 14;
        let only_b = defaults[1].clone();
        let first = Catalog {
            materials: vec![a],
            from: "CutCraft on A".into(),
            at: 100,
            ..Default::default()
        };
        let second = Catalog {
            materials: vec![b, only_b.clone()],
            from: "CutCraft on B".into(),
            at: 200,
            ..Default::default()
        };
        let merged = Catalog::merge(&[second.clone(), first.clone()]);
        assert_eq!(merged.materials.len(), 2);
        let shared = merged
            .materials
            .iter()
            .find(|m| m.name == defaults[0].name)
            .unwrap();
        assert_eq!(shared.force, 14, "B sent later");
        assert_eq!(merged.material_from[&only_b.name], "CutCraft on B");
        assert_eq!((merged.senders.len(), merged.at), (2, 200));
        // A sends again, later: its version of the shared name wins now.
        let again = Catalog { at: 300, ..first };
        let merged = Catalog::merge(&[second, again]);
        assert_eq!(
            merged
                .materials
                .iter()
                .find(|m| m.name == defaults[0].name)
                .unwrap()
                .force,
            10
        );
    }

    #[test]
    fn mats_come_from_the_list_or_the_standard_ones() {
        let mut p = project();
        let long = MatProfile::presets()
            .into_iter()
            .find(|m| m.class == Mat::Mat12x24)
            .unwrap();
        let plan = apply(
            &mut p,
            &Adjustments {
                mat: Some(long.name.clone()),
                ..Default::default()
            },
            &Catalog::default(),
        )
        .unwrap();
        assert_eq!(plan.mat, Mat::Mat12x24);
        assert!(
            apply(
                &mut p,
                &Adjustments {
                    mat: Some("No such mat".into()),
                    ..Default::default()
                },
                &Catalog::default()
            )
            .is_err()
        );
    }
}
