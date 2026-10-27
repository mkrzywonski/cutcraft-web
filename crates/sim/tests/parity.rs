//! Preview/send parity (spec §9.1): a planned sheet, encoded and run through the simulated Cameo,
//! leaves the same cuts on the virtual mat that the Cut Preview showed.

use std::time::Duration;

use cutcraft_device::gpgl::{self, CAMEO_1};
mod common;

use common::project;
use cutcraft_job::{Mat, Material, Placement};
use cutcraft_path::{CutPlan, plan_sheet};
use cutcraft_sim::{Clock, Profile, Scenario, Simulator, harness, parity};

/// Encode a plan, send it through the simulator and compare the virtual mat with the plan.
fn run(plan: &CutPlan, against: &CutPlan, scenario: &str) -> parity::ParityReport {
    let encoded = gpgl::encode(plan, &CAMEO_1).expect("plan encodes");
    assert!(encoded.max_quantization_error_mm <= encoded.quantization_budget_mm);
    let scenario = Scenario::named(scenario, encoded.bytes.len()).unwrap();
    let sim = Simulator::new(Profile::cameo_1(), Clock::Virtual, &scenario);
    let mut transport = sim.transport();
    harness::send(&mut transport, &encoded.bytes, 64, Duration::from_secs(120));
    assert!(sim.run_until_idle(Duration::from_secs(3600)));
    sim.with_machine(|m| parity::check(against, m, CAMEO_1.quantization_budget_mm()))
}

#[test]
fn the_virtual_mat_matches_the_previewed_plan() {
    let project = project();
    let sheet = project.sheets[0].id.clone();
    let plan = plan_sheet(&project, &sheet).unwrap();
    assert!(plan.valid, "{:?}", plan.diagnostics);
    let report = run(&plan, &plan, "normal");
    assert!(report.matches, "{:#?}", report);
    // Holes and the island, the outline, the square, the line and the weeding box.
    assert_eq!(report.cuts_found, plan.operations.len());
    assert!(report.max_deviation_mm <= report.budget_mm);
}

#[test]
fn mirror_rotation_repeats_and_a_long_mat_survive_encoding() {
    let mut project = project();
    let sheet = project.sheets[0].id.clone();
    let mut material = Material::defaults().into_iter().find(|m| m.mirror).unwrap();
    material.passes = 2;
    project.set_material(&sheet, material).unwrap();
    project
        .set_placement(
            &sheet,
            Placement {
                offset_mm: [80.0, 400.0],
                rotation_deg: 30.0,
            },
            Mat::Mat12x24,
        )
        .unwrap();
    let plan = plan_sheet(&project, &sheet).unwrap();
    assert!(plan.valid && plan.mirror, "{:?}", plan.diagnostics);
    let encoded = gpgl::encode(&plan, &CAMEO_1).unwrap();
    let scenario = Scenario {
        media: Some(cutcraft_sim::Media::Mat12x24),
        ..Scenario::default()
    };
    let sim = Simulator::new(Profile::cameo_1(), Clock::Virtual, &scenario);
    let mut transport = sim.transport();
    harness::send(&mut transport, &encoded.bytes, 64, Duration::from_secs(120));
    assert!(sim.run_until_idle(Duration::from_secs(3600)));
    let report = sim.with_machine(|m| parity::check(&plan, m, CAMEO_1.quantization_budget_mm()));
    assert!(report.matches, "{:#?}", report);
    // Every artwork contour twice; the weeding box once.
    assert_eq!(report.cuts_found, 2 * plan.operations.len() - 1);
}

#[test]
fn an_edited_sheet_no_longer_matches_what_was_sent() {
    let mut project = project();
    let sheet = project.sheets[0].id.clone();
    let sent = plan_sheet(&project, &sheet).unwrap();
    project
        .set_placement(
            &sheet,
            Placement {
                offset_mm: [20.0, 12.0],
                rotation_deg: 0.0,
            },
            Mat::Mat12x12,
        )
        .unwrap();
    let edited = plan_sheet(&project, &sheet).unwrap();
    assert_ne!(sent.plan_id, edited.plan_id);
    let report = run(&sent, &edited, "normal");
    assert!(!report.matches);
    assert!(
        report
            .problems
            .iter()
            .any(|p| p.contains("does not follow"))
    );
}

#[test]
fn a_cut_interrupted_by_power_loss_is_reported_incomplete() {
    let project = project();
    let plan = plan_sheet(&project, &project.sheets[0].id).unwrap();
    let report = run(&plan, &plan, "power-off-mid-cut");
    assert!(!report.matches);
    assert!(report.cuts_found < report.cuts_expected, "{report:#?}");
}

#[test]
fn the_pen_calibration_sheet_encodes_and_matches_on_the_simulator() {
    let mut pen = Material::defaults().remove(0);
    pen.force = 8;
    pen.speed = 3;
    let plan =
        cutcraft_path::calibration_plan(&pen, &cutcraft_job::MachineProfile::cameo_1()).unwrap();
    let report = run(&plan, &plan, "normal");
    assert!(report.matches, "{report:#?}");
    assert_eq!(report.cuts_found, plan.operations.len());
}

#[test]
fn the_letter_calibration_sheet_fits_the_paper_below_the_tape_and_matches() {
    let mut blade = Material::defaults().remove(0);
    blade.force = 1;
    blade.speed = 1;
    let plan =
        cutcraft_path::letter_calibration_plan(&blade, &cutcraft_job::MachineProfile::cameo_1())
            .unwrap();
    // Every point 15 mm inside 8.5 x 11 in paper and 60 mm or more below its top edge.
    for op in &plan.operations {
        for p in &op.points {
            assert!((15.0..=215.9 - 15.0).contains(&p[0]), "{p:?}");
            assert!((60.0..=279.4 - 15.0).contains(&p[1]), "{p:?}");
        }
    }
    let report = run(&plan, &plan, "normal");
    assert!(report.matches, "{report:#?}");
    assert_eq!(report.cuts_found, plan.operations.len());
}

#[test]
fn weed_lines_and_layering_marks_encode_and_match() {
    let mut project = project();
    let sheet = project.sheets[0].id.clone();
    project.set_weed_lines(&sheet, true).unwrap();
    project.set_layering_marks(&sheet, true).unwrap();
    let plan = plan_sheet(&project, &sheet).unwrap();
    assert!(plan.valid, "{:?}", plan.diagnostics);
    let aids = |kind| plan.operations.iter().filter(|o| o.kind == kind).count();
    assert_eq!(
        aids(cutcraft_path::OperationKind::Mark),
        4,
        "four triangles"
    );
    assert!(aids(cutcraft_path::OperationKind::WeedLine) > 0);
    let report = run(&plan, &plan, "normal");
    assert!(report.matches, "{report:#?}");
    assert_eq!(report.cuts_found, plan.operations.len());
}

#[test]
fn a_roll_sheet_cuts_and_matches_with_roll_loaded() {
    let mut project = project();
    let sheet = project.sheets[0].id.clone();
    project
        .set_placement(
            &sheet,
            Placement {
                offset_mm: [20.0, 900.0],
                rotation_deg: 0.0,
            },
            Mat::Roll,
        )
        .unwrap();
    let plan = plan_sheet(&project, &sheet).unwrap();
    assert!(plan.valid, "{:?}", plan.diagnostics);
    let encoded = gpgl::encode(&plan, &CAMEO_1).unwrap();
    let scenario = Scenario {
        media: Some(cutcraft_sim::Media::Roll),
        ..Scenario::named("normal", encoded.bytes.len()).unwrap()
    };
    let sim = Simulator::new(Profile::cameo_1(), Clock::Virtual, &scenario);
    harness::send(
        &mut sim.transport(),
        &encoded.bytes,
        64,
        Duration::from_secs(120),
    );
    assert!(sim.run_until_idle(Duration::from_secs(3600)));
    let report = sim.with_machine(|m| parity::check(&plan, m, CAMEO_1.quantization_budget_mm()));
    assert!(report.matches, "{report:#?}");
}

#[test]
fn overcut_and_software_compensation_match_what_the_simulator_traces() {
    let mut project = project();
    let sheet = project.sheets[0].id.clone();
    let mut material = project.sheets[0].material.clone();
    material.overcut_mm = 0.5;
    material.offset_method = cutcraft_job::OffsetMethod::Software;
    material.offset_mm = 0.25;
    project.set_material(&sheet, material).unwrap();
    let plan = plan_sheet(&project, &sheet).unwrap();
    assert!(plan.valid, "{:?}", plan.diagnostics);
    assert!(plan.operations.iter().all(|o| o.tool_path.is_some()));
    let report = run(&plan, &plan, "normal");
    assert!(report.matches, "{report:#?}");
    assert_eq!(report.cuts_found, plan.operations.len());
}
