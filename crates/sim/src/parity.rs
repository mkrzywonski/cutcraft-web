//! Preview/send parity (spec §9.1): does the virtual mat show the plan the Cut Preview showed?
//! The machine's blade-down runs are compared with the plan's operations, in order, pass by
//! pass, within the quantization budget, along with force, speed and faults.

use cutcraft_path::CutPlan;
use serde::Serialize;

use crate::machine::Machine;

#[derive(Debug, Clone, Serialize)]
pub struct ParityReport {
    pub plan_id: String,
    pub matches: bool,
    pub cuts_expected: usize,
    pub cuts_found: usize,
    /// Largest distance between a cut vertex and the planned path, or the reverse.
    pub max_deviation_mm: f64,
    pub budget_mm: f64,
    pub problems: Vec<String>,
}

/// One blade-down run on the virtual mat, in mm (`[x, y]`).
struct Run {
    points: Vec<[f64; 2]>,
    settings: Vec<(i32, i32)>,
}

/// Compare everything the machine cut with `plan`.
pub fn check(plan: &CutPlan, machine: &Machine, budget_mm: f64) -> ParityReport {
    let mut problems: Vec<String> = machine
        .faults()
        .iter()
        .map(|f| format!("fault at {:.2} s: {}", f.at_s, f.detail))
        .collect();
    let runs = runs(machine);
    let expected: Vec<(usize, &[[f64; 2]])> = plan
        .operations
        .iter()
        .enumerate()
        // What the tool follows: overcut and software compensation included.
        .flat_map(|(i, op)| (0..op.passes.max(1)).map(move |_| (i, op.path())))
        .collect();
    if runs.len() != expected.len() {
        problems.push(format!(
            "the plan has {} cuts (with repeats) but the machine made {}",
            expected.len(),
            runs.len()
        ));
    }
    let want = (
        i32::from(plan.material.force),
        i32::from(plan.material.speed),
    );
    let mut max_deviation = 0.0f64;
    for (n, ((op, planned), run)) in expected.iter().zip(&runs).enumerate() {
        let cut = n + 1;
        if let Some(&(force, speed)) = run.settings.iter().find(|s| **s != want) {
            problems.push(format!(
                "cut {cut} ran at force {force}, speed {speed}; the plan says {}, {}",
                want.0, want.1
            ));
        }
        // An overcut or compensated path ends past its start by design.
        let closed = plan.operations[*op].closed && plan.operations[*op].tool_path.is_none();
        if closed && distance(run.points[0], *run.points.last().unwrap()) > budget_mm {
            problems.push(format!(
                "cut {cut} should be closed but does not end where it starts"
            ));
        }
        match (
            follows(&run.points, planned, budget_mm),
            follows(planned, &run.points, budget_mm),
        ) {
            (Some(a), Some(b)) => max_deviation = max_deviation.max(a).max(b),
            _ => problems.push(format!(
                "cut {cut} does not follow operation {} of the plan within {budget_mm:.3} mm",
                op + 1
            )),
        }
    }
    ParityReport {
        plan_id: plan.plan_id.clone(),
        matches: problems.is_empty(),
        cuts_expected: expected.len(),
        cuts_found: runs.len(),
        max_deviation_mm: max_deviation,
        budget_mm,
        problems,
    }
}

/// Blade-down runs: consecutive tool-down segments, split wherever the tool rose or jumped.
fn runs(machine: &Machine) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let mut open = false;
    for segment in machine.trace() {
        if !segment.tool_down {
            open = false;
            continue;
        }
        match runs.last_mut() {
            Some(run) if open && *run.points.last().unwrap() == segment.from => {
                run.points.push(segment.to);
                run.settings.push((segment.force, segment.speed));
            }
            _ => runs.push(Run {
                points: vec![segment.from, segment.to],
                settings: vec![(segment.force, segment.speed)],
            }),
        }
        open = true;
    }
    runs
}

/// Whether every vertex of `a`, in order, lies within `budget` of the polyline `b`, walking `b`
/// forwards only (so direction and order count). Returns the largest distance found.
fn follows(a: &[[f64; 2]], b: &[[f64; 2]], budget: f64) -> Option<f64> {
    if b.len() == 1 {
        return a
            .iter()
            .map(|p| distance(*p, b[0]))
            .try_fold(0.0f64, |m, d| (d <= budget).then_some(m.max(d)));
    }
    let mut segment = 0;
    let mut worst = 0.0f64;
    for p in a {
        let mut found = None;
        for (i, w) in b.windows(2).enumerate().skip(segment) {
            let d = to_segment(*p, w[0], w[1]);
            if d <= budget {
                found = Some((i, d));
                break;
            }
        }
        let (i, d) = found?;
        segment = i;
        worst = worst.max(d);
    }
    Some(worst)
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len2 = d[0] * d[0] + d[1] * d[1];
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0)
    };
    distance(p, [a[0] + t * d[0], a[1] + t * d[1]])
}
