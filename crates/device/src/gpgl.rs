//! GP-GL encoding of a cut plan (spec §7.2, §7.3): plan millimetres become device units and
//! ETX-framed commands. Nothing here talks to a device; the sender and the simulator consume the
//! encoded bytes.
//!
//! Clean room: written from the public command descriptions summarised in spec §7.3 (`M`/`D`
//! with the feed axis first, `FX` force, `!` speed, ETX framing). Every one of them is a
//! working hypothesis until the hardware tests confirm it.

use cutcraft_job::{Diagnostic, Mat, Severity};
use cutcraft_path::{Compensation, CutPlan};
use serde::Serialize;

mod stream;
pub use stream::{Experiment, StreamError, validate, validate_experiment};

const ETX: u8 = 0x03;

/// What the encoder needs to know about a cutter model.
#[derive(Clone, Debug, Serialize)]
pub struct Model {
    pub name: &'static str,
    pub vendor_id: u16,
    pub product_id: u16,
    /// Device units per mm (hypothesis: 20).
    pub units_per_mm: f64,
    /// Usable width across the mat.
    pub width_mm: f64,
    pub force_range: (u8, u8),
    pub speed_range: (u8, u8),
    /// Conservative validation ceilings, not measured firmware limits.
    pub max_offset_units: i32,
    pub max_length_mm: f64,
    /// Points per `M`/`D` command, keeping each command short and chunk boundaries frequent.
    pub max_points_per_command: usize,
    /// Tool speed per speed step, in mm/s (hypothesis: 10).
    pub speed_step_mm_s: f64,
    /// Longest estimated motion in one command. Long moves are split, so what is queued can be
    /// bounded by time and Stop is never stuck behind one long move (spec §7.3).
    pub max_command_s: f64,
}

/// The original Cameo, from the same hypotheses as the simulator's profile.
pub const CAMEO_1: Model = Model {
    name: "Silhouette Cameo (original)",
    vendor_id: 0x0b4d,
    product_id: 0x1121,
    units_per_mm: 20.0,
    width_mm: 304.0,
    force_range: (1, 33),
    speed_range: (1, 10),
    max_offset_units: 200,
    max_length_mm: 3000.0,
    max_points_per_command: 32,
    speed_step_mm_s: 10.0,
    max_command_s: 0.5,
};

impl Model {
    /// The largest distance a quantized point can be from its plan point.
    pub fn quantization_budget_mm(&self) -> f64 {
        0.5 * std::f64::consts::SQRT_2 / self.units_per_mm
    }
}

/// One pass over one plan operation, as sent.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EncodedPass {
    /// Index into the plan's operations.
    pub operation: usize,
    /// 1-based.
    pub pass: u8,
    /// Byte range of its commands in [`EncodedPlan::bytes`], for progress and checkpoints.
    pub bytes: [usize; 2],
    /// Blade-down polyline in device units, `[x, y]` (x across, y along the feed).
    pub points: Vec<[i32; 2]>,
}

/// A plan encoded for one model. It names the plan it came from, so a sender can refuse it once
/// the plan is stale.
#[derive(Clone, Debug, Serialize)]
pub struct EncodedPlan {
    pub plan_id: String,
    pub model: &'static str,
    #[serde(skip)]
    pub bytes: Vec<u8>,
    pub byte_count: usize,
    /// Authoritative software cutting area, x across and y along the feed, in mm.
    pub cutting_area_mm: [f64; 2],
    pub commands: usize,
    pub passes: Vec<EncodedPass>,
    pub quantization_budget_mm: f64,
    /// Largest distance any sent point moved from its plan point.
    pub max_quantization_error_mm: f64,
}

/// Encode a validated plan. Refuses (with blocking diagnostics) an invalid plan, settings outside
/// the model's ranges, and contours that collapse, duplicate or leave the area at device
/// resolution (spec §7.2).
pub fn encode(plan: &CutPlan, model: &Model) -> Result<EncodedPlan, Vec<Diagnostic>> {
    let mut problems = Vec::new();
    if !plan.valid {
        problems.push(Diagnostic::blocking(
            "encode.plan_invalid",
            "The plan has blocking problems; resolve them before sending.",
            vec![],
        ));
    }
    let material = &plan.material;
    let in_range = |v: u8, (low, high): (u8, u8)| (low..=high).contains(&v);
    if !in_range(material.force, model.force_range) {
        problems.push(Diagnostic::blocking(
            "encode.force_out_of_range",
            format!(
                "Force {} is outside the {}'s range {}-{}.",
                material.force, model.name, model.force_range.0, model.force_range.1
            ),
            vec![],
        ));
    }
    if !in_range(material.speed, model.speed_range) {
        problems.push(Diagnostic::blocking(
            "encode.speed_out_of_range",
            format!(
                "Speed {} is outside the {}'s range {}-{}.",
                material.speed, model.name, model.speed_range.0, model.speed_range.1
            ),
            vec![],
        ));
    }
    // Spec §7.2/§7.3: one compensation method per material. The cutter keeps the last `FC` it
    // received, even across a power cycle, so Off and software compensation both send `FC0`
    // (verified with a pen on the original Cameo, 2026-10-23). Machine compensation sends the
    // offset in device units; its blade behaviour is not yet verified.
    let offset_setting = match plan.compensation {
        Compensation::PendingCalibration | Compensation::Software { .. } => Some("FC0".to_owned()),
        Compensation::Machine { offset_mm } => Some(format!(
            "FC{}",
            (offset_mm * model.units_per_mm).round() as i64
        )),
    };

    let units = model.units_per_mm;
    let limit = [
        (model.width_mm.min(plan.cutting_area_mm[0]) * units).floor() as i64,
        (plan.cutting_area_mm[1] * units).floor() as i64,
    ];
    let mut max_error = 0.0f64;
    let mut quantized: Vec<Vec<[i32; 2]>> = Vec::with_capacity(plan.operations.len());
    for (index, op) in plan.operations.iter().enumerate() {
        let ids = op.item.clone().into_iter().collect::<Vec<_>>();
        let mut points: Vec<[i32; 2]> = Vec::with_capacity(op.points.len());
        let mut outside = op.path().iter().any(|p| {
            let q = [(p[0] * units).round(), (p[1] * units).round()];
            !(q[0].is_finite() && q[1].is_finite())
                || q[0] < 0.0
                || q[1] < 0.0
                || q[0] as i64 > limit[0]
                || q[1] as i64 > limit[1]
        });
        for p in &op.points {
            if !p[0].is_finite() || !p[1].is_finite() {
                outside = true;
                continue;
            }
            let q = [(p[0] * units).round(), (p[1] * units).round()];
            max_error = max_error.max((q[0] / units - p[0]).hypot(q[1] / units - p[1]));
            if q[0] < 0.0 || q[1] < 0.0 || q[0] as i64 > limit[0] || q[1] as i64 > limit[1] {
                outside = true;
            }
            let q = [q[0] as i32, q[1] as i32];
            // Quantization removes zero-length moves.
            if points.last() != Some(&q) {
                points.push(q);
            }
        }
        if outside {
            problems.push(Diagnostic::blocking(
                "encode.out_of_bounds",
                format!("Cut {} reaches outside the cutting area.", index + 1),
                ids.clone(),
            ));
        }
        let collapsed = if op.closed {
            points.len() < 4 || twice_area(&points) == 0
        } else {
            points.len() < 2
        };
        if collapsed {
            problems.push(Diagnostic::blocking(
                "encode.collapsed",
                format!(
                    "Cut {} is too small for the cutter's {:.2} mm step and would collapse. Make the design larger or remove the tiny feature.",
                    index + 1,
                    1.0 / units
                ),
                ids.clone(),
            ));
        }
        if let Some(other) = quantized.iter().position(|q| same_contour(q, &points)) {
            problems.push(Diagnostic::blocking(
                "encode.duplicate",
                format!(
                    "Cuts {} and {} become the same cut at the cutter's resolution, so the blade would cut it twice.",
                    other + 1,
                    index + 1
                ),
                ids,
            ));
        }
        quantized.push(points);
    }
    if problems.iter().any(|d| d.severity == Severity::Blocking) {
        return Err(problems);
    }

    let mut out = Writer::default();
    out.command(&format!("FX{}", material.force));
    out.command(&format!("!{}", material.speed));
    if let Some(setting) = &offset_setting {
        out.command(setting);
    }
    let mut passes = Vec::new();
    let max_units =
        (f64::from(material.speed) * model.speed_step_mm_s * model.max_command_s * units).max(1.0);
    let mut at = [0, 0];
    for (index, op) in plan.operations.iter().enumerate() {
        // Long segments are split in plan millimetres and each new point rounded on its own, so
        // every sent point stays within the quantization budget of the plan's path.
        // The tool's path: overcut and software compensation included.
        let path = op.path();
        let cut = densify(path, max_units / units, units);
        for q in &cut {
            let q = [f64::from(q[0]) / units, f64::from(q[1]) / units];
            max_error = max_error.max(nearest_on(path, q));
        }
        for pass in 1..=op.passes.max(1) {
            let start = out.bytes.len();
            // Every pass starts with a tool-up move, even in place, so the blade lifts between
            // cuts and each cut is one blade-down run.
            let travel = subdivide(&[at, cut[0]], max_units);
            out.motion(
                "M",
                &travel[1..],
                model.max_points_per_command,
                max_units,
                at,
            );
            out.motion(
                "D",
                &cut[1..],
                model.max_points_per_command,
                max_units,
                cut[0],
            );
            at = *cut.last().expect("a cut has points");
            passes.push(EncodedPass {
                operation: index,
                pass,
                bytes: [start, out.bytes.len()],
                points: cut.clone(),
            });
        }
    }
    // A mat job ends with a tool-up move back to where it started, so the head is clear of the
    // work and every job starts from the same place. A plain `M` in the job's own frame: no
    // homing or origin command. A roll stays where it finished, since returning would feed the
    // roll back by the whole job's length.
    if !passes.is_empty() {
        let end = if plan.mat == Mat::Roll { at } else { [0, 0] };
        let travel = subdivide(&[at, end], max_units);
        out.motion("M", &travel[1..], 1, max_units, at);
    }
    validate(&out.bytes, model, plan.cutting_area_mm).map_err(|error| {
        vec![Diagnostic::blocking(
            "encode.stream_invalid",
            error.to_string(),
            vec![],
        )]
    })?;
    Ok(EncodedPlan {
        plan_id: plan.plan_id.clone(),
        model: model.name,
        byte_count: out.bytes.len(),
        cutting_area_mm: plan.cutting_area_mm,
        commands: out.commands,
        bytes: out.bytes,
        passes,
        quantization_budget_mm: model.quantization_budget_mm(),
        max_quantization_error_mm: max_error,
    })
}

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
    commands: usize,
}

impl Writer {
    fn command(&mut self, text: &str) {
        self.bytes.extend_from_slice(text.as_bytes());
        self.bytes.push(ETX);
        self.commands += 1;
    }

    /// `M`/`D` commands, feed axis (y) first, each with at most `per_command` points and
    /// `max_units` of travel from `from`. A move in place is still one command.
    fn motion(
        &mut self,
        code: &str,
        points: &[[i32; 2]],
        per_command: usize,
        max_units: f64,
        from: [i32; 2],
    ) {
        if points.is_empty() {
            // A pass that starts where the last one ended still lifts the blade there.
            self.command(&format!("{code}{},{}", from[1], from[0]));
            return;
        }
        let mut args: Vec<String> = Vec::new();
        let mut length = 0.0;
        let mut at = from;
        for p in points {
            let step = distance(at, *p);
            if !args.is_empty() && (args.len() >= per_command.max(1) || length + step > max_units) {
                self.command(&format!("{code}{}", args.join(",")));
                args.clear();
                length = 0.0;
            }
            args.push(format!("{},{}", p[1], p[0]));
            length += step;
            at = *p;
        }
        self.command(&format!("{code}{}", args.join(",")));
    }
}

fn distance(a: [i32; 2], b: [i32; 2]) -> f64 {
    f64::from(b[0] - a[0]).hypot(f64::from(b[1] - a[1]))
}

/// Plan points (mm) with points added along any segment longer than `max_mm`, each rounded
/// to device units, without zero-length moves.
fn densify(points: &[[f64; 2]], max_mm: f64, units: f64) -> Vec<[i32; 2]> {
    let round = |p: [f64; 2]| [(p[0] * units).round() as i32, (p[1] * units).round() as i32];
    let mut out: Vec<[i32; 2]> = Vec::with_capacity(points.len());
    let mut push = |q: [i32; 2]| {
        if out.last() != Some(&q) {
            out.push(q);
        }
    };
    for (i, p) in points.iter().enumerate() {
        if i > 0 {
            let from = points[i - 1];
            let length = (p[0] - from[0]).hypot(p[1] - from[1]);
            let pieces = (length / max_mm).ceil() as usize;
            for k in 1..pieces {
                let t = k as f64 / pieces as f64;
                push(round([
                    from[0] + t * (p[0] - from[0]),
                    from[1] + t * (p[1] - from[1]),
                ]));
            }
        }
        push(round(*p));
    }
    out
}

/// Distance from `p` to the nearest point of the polyline, in mm.
fn nearest_on(polyline: &[[f64; 2]], p: [f64; 2]) -> f64 {
    polyline
        .windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let d = [b[0] - a[0], b[1] - a[1]];
            let len2 = d[0] * d[0] + d[1] * d[1];
            let t = if len2 > 0.0 {
                (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (a[0] + t * d[0] - p[0]).hypot(a[1] + t * d[1] - p[1])
        })
        .fold(f64::INFINITY, f64::min)
        .min(
            polyline
                .first()
                .map_or(f64::INFINITY, |a| (a[0] - p[0]).hypot(a[1] - p[1])),
        )
}

/// The polyline with points added along any segment longer than `max_units`, so no single
/// move outlasts one command's time budget.
fn subdivide(points: &[[i32; 2]], max_units: f64) -> Vec<[i32; 2]> {
    let mut out: Vec<[i32; 2]> = Vec::with_capacity(points.len());
    for p in points {
        if let Some(&from) = out.last() {
            let pieces = (distance(from, *p) / max_units).ceil() as i32;
            for k in 1..pieces {
                let t = f64::from(k) / f64::from(pieces);
                out.push([
                    (f64::from(from[0]) + t * f64::from(p[0] - from[0])).round() as i32,
                    (f64::from(from[1]) + t * f64::from(p[1] - from[1])).round() as i32,
                ]);
            }
        }
        if out.last() != Some(p) {
            out.push(*p);
        }
    }
    out
}

fn twice_area(points: &[[i32; 2]]) -> i64 {
    points
        .windows(2)
        .map(|w| i64::from(w[0][0]) * i64::from(w[1][1]) - i64::from(w[1][0]) * i64::from(w[0][1]))
        .sum::<i64>()
        .abs()
}

/// The same track: identical points, or a closed contour started elsewhere or run backwards.
fn same_contour(a: &[[i32; 2]], b: &[[i32; 2]]) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let closed = a.first() == a.last() && b.first() == b.last();
    if !closed {
        return a.iter().eq(b.iter().rev());
    }
    let ring_a = &a[..a.len() - 1];
    let mut ring_b = b[..b.len() - 1].to_vec();
    for _ in 0..2 {
        if let Some(offset) = ring_b.iter().position(|p| *p == ring_a[0])
            && ring_a
                .iter()
                .enumerate()
                .all(|(i, p)| *p == ring_b[(offset + i) % ring_b.len()])
        {
            return true;
        }
        ring_b.reverse();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutcraft_job::{Mat, Material};
    use cutcraft_path::{Operation, OperationKind};

    fn plan(operations: Vec<Operation>) -> CutPlan {
        CutPlan {
            schema_version: 1,
            plan_id: "test".into(),
            sheet: "sheet-1".into(),
            sheet_name: "Test".into(),
            project_revision: 0,
            source_sha256: String::new(),
            material: Material::defaults().remove(0),
            mirror: false,
            mat: Mat::Mat12x12,
            mat_mm: [304.8, 304.8],
            cutting_area_mm: [304.0, 304.8],
            operations,
            bounds_mm: None,
            design_size_mm: None,
            cut_length_mm: 0.0,
            travel_length_mm: 0.0,
            estimate_s: 0.0,
            compensation: Compensation::PendingCalibration,
            diagnostics: vec![],
            valid: true,
            piece_mm: None,
            test_spot_mm: None,
            piece_at_mm: None,
        }
    }

    fn op(kind: OperationKind, points: &[[f64; 2]], closed: bool) -> Operation {
        Operation {
            kind,
            item: Some("piece".into()),
            colour: None,
            points: points.to_vec(),
            closed,
            passes: 1,
            tool_path: None,
        }
    }

    fn square(x: f64, y: f64, size: f64) -> Vec<[f64; 2]> {
        vec![
            [x, y],
            [x + size, y],
            [x + size, y + size],
            [x, y + size],
            [x, y],
        ]
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8(bytes.to_vec())
            .unwrap()
            .replace('\x03', "|")
    }

    #[test]
    fn encodes_settings_then_feed_first_moves_and_returns_to_the_start() {
        let mut p = plan(vec![
            op(OperationKind::Outline, &square(10.0, 20.0, 5.0), true),
            op(OperationKind::Line, &[[1.0, 2.0], [3.0, 2.0]], false),
        ]);
        p.operations[1].passes = 2;
        let encoded = encode(&p, &CAMEO_1).unwrap();
        assert_eq!(
            text(&encoded.bytes),
            "FX10|!5|FC0|M400,200|D400,300,500,300,500,200,400,200|M40,20|D40,60|M40,20|D40,60|M0,0|"
        );
        assert_eq!(encoded.passes.len(), 3);
        assert_eq!(encoded.passes[2].pass, 2);
        assert_eq!(&encoded.bytes[encoded.passes[0].bytes[0]..][..4], b"M400");
        assert_eq!(encoded.max_quantization_error_mm, 0.0);
        // A roll lifts where it finishes instead of feeding back.
        p.mat = Mat::Roll;
        assert!(text(&encode(&p, &CAMEO_1).unwrap().bytes).ends_with("|D40,60|M40,60|"));
    }

    #[test]
    fn compensation_settings_and_tool_paths_are_sent() {
        let mut p = plan(vec![op(
            OperationKind::Outline,
            &square(10.0, 20.0, 5.0),
            true,
        )]);
        p.compensation = Compensation::Software { offset_mm: 0.25 };
        p.operations[0].tool_path = Some(vec![[10.0, 20.0], [15.25, 20.0], [15.0, 25.0]]);
        let encoded = encode(&p, &CAMEO_1).unwrap();
        assert!(text(&encoded.bytes).starts_with("FX10|!5|FC0|M400,200|D400,305,500,300|"));
        p.compensation = Compensation::Machine { offset_mm: 0.25 };
        p.operations[0].tool_path = None;
        let encoded = encode(&p, &CAMEO_1).unwrap();
        assert!(text(&encoded.bytes).starts_with("FX10|!5|FC5|M400,200|"));
        // A tool path outside the area blocks, even when the drawn contour fits.
        p.operations[0].tool_path = Some(vec![[10.0, 20.0], [-1.0, 20.0]]);
        let codes: Vec<String> = encode(&p, &CAMEO_1)
            .unwrap_err()
            .into_iter()
            .map(|d| d.code)
            .collect();
        assert_eq!(codes, ["encode.out_of_bounds"]);
    }

    #[test]
    fn rounds_absolute_positions_and_drops_zero_length_moves() {
        let p = plan(vec![op(
            OperationKind::Line,
            &[[0.0, 0.0], [0.01, 0.0], [0.024, 0.0], [10.026, 0.0]],
            false,
        )]);
        let encoded = encode(&p, &CAMEO_1).unwrap();
        assert_eq!(encoded.passes[0].points, vec![[0, 0], [201, 0]]);
        assert!(encoded.max_quantization_error_mm <= CAMEO_1.quantization_budget_mm());
        assert!((encoded.max_quantization_error_mm - 0.024).abs() < 1e-9);
    }

    #[test]
    fn long_contours_are_split_into_short_commands() {
        let points: Vec<[f64; 2]> = (0..=100).map(|i| [f64::from(i), 1.0]).collect();
        let encoded = encode(
            &plan(vec![op(OperationKind::Line, &points, false)]),
            &CAMEO_1,
        )
        .unwrap();
        let draws = text(&encoded.bytes).matches("|D").count();
        assert_eq!(draws, 100usize.div_ceil(32));
        assert!(encoded.bytes.split(|&b| b == ETX).all(|c| c.len() < 512));
    }

    #[test]
    fn refuses_collapsed_duplicate_and_out_of_area_contours_and_bad_settings() {
        let mut p = plan(vec![
            op(OperationKind::Hole, &square(10.0, 10.0, 0.02), true),
            op(OperationKind::Outline, &square(20.0, 20.0, 5.0), true),
            op(OperationKind::Outline, &square(20.01, 20.01, 5.0), true),
            op(OperationKind::Line, &[[300.0, 1.0], [305.0, 1.0]], false),
        ]);
        p.material.force = 40;
        let codes: Vec<String> = encode(&p, &CAMEO_1)
            .unwrap_err()
            .into_iter()
            .map(|d| d.code)
            .collect();
        assert_eq!(
            codes,
            [
                "encode.force_out_of_range",
                "encode.collapsed",
                "encode.duplicate",
                "encode.out_of_bounds"
            ]
        );
        let mut invalid = plan(vec![op(
            OperationKind::Line,
            &[[1.0, 1.0], [2.0, 1.0]],
            false,
        )]);
        invalid.valid = false;
        assert_eq!(
            encode(&invalid, &CAMEO_1).unwrap_err()[0].code,
            "encode.plan_invalid"
        );
    }

    #[test]
    fn a_closed_contour_run_backwards_from_another_start_is_a_duplicate() {
        let a = [[0, 0], [10, 0], [10, 10], [0, 0]];
        let b = [[10, 10], [10, 0], [0, 0], [10, 10]];
        assert!(same_contour(&a, &b));
        assert!(!same_contour(&a, &[[0, 0], [10, 0], [10, 11], [0, 0]]));
    }
}
