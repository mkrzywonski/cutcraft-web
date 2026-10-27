//! Cut planning (spec §5.4, §7.2): turn one sheet of a project into an immutable, ordered plan of
//! blade paths in mat millimetres, with bounds, estimates and diagnostics. No device units or
//! commands here; encoding belongs to `cutcraft-device`.

mod preview;
mod raster;

pub use preview::preview_svg;
pub use raster::{RasterOptions, RasterPreview, RasterView, assembly_png, preview_png};

use cutcraft_job::{Contour, Diagnostic, Mat, Material, PreparedDesign, Project, Severity, Sheet};
use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PLAN_SCHEMA_VERSION: u32 = 1;
/// Largest allowed deviation of a flattened curve from the true curve (spec §7.2).
pub const FLATTEN_TOLERANCE_MM: f64 = 0.025;
/// Output coordinates are rounded to this many mm so plans compare across runs.
const QUANTUM_MM: f64 = 1e-4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    /// A hole or other contour inside a piece, cut before what encloses it.
    Hole,
    /// A piece's outer contour, which releases it.
    Outline,
    /// A cut line: cut once along its centre.
    Line,
    /// The weeding box around the artwork, cut last.
    WeedingBox,
    /// A line through the waste vinyl, clear of the artwork, cut after it.
    WeedLine,
    /// A layering mark: a small triangle on the design's centre line, pointing at it, at the same
    /// place in assembly coordinates in every cut that has it.
    Mark,
}

impl OperationKind {
    /// Generated cutting aids, cut once whatever the material's passes.
    pub fn is_aid(self) -> bool {
        matches!(self, Self::WeedingBox | Self::WeedLine | Self::Mark)
    }
}

/// Spacing of weed lines across the waste (spec §5.4).
pub const WEED_LINE_SPACING_MM: f64 = 25.4;
/// Weed lines keep this far from any artwork.
pub const WEED_LINE_CLEARANCE_MM: f64 = 1.5;
/// Shorter pieces of weed line are left out.
const WEED_LINE_MIN_MM: f64 = 3.0;
/// Each layering mark is a triangle this wide and this deep.
pub const MARK_MM: f64 = 6.0;
/// Space between the design and a layering mark's tip.
const MARK_GAP_MM: f64 = 3.0;

/// One blade-down path.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Operation {
    pub kind: OperationKind,
    /// The prepared piece or cut line it comes from (none for the weeding box).
    pub item: Option<String>,
    pub colour: Option<String>,
    /// Polyline in mat mm; a closed path ends where it starts.
    pub points: Vec<[f64; 2]>,
    pub closed: bool,
    pub passes: u8,
    /// The path the tool follows when it differs from `points`: overcut and software blade
    /// compensation (spec §7.2). The preview draws `points`; the sender and bounds use this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_path: Option<Vec<[f64; 2]>>,
}

impl Operation {
    /// What the tool actually follows.
    pub fn path(&self) -> &[[f64; 2]] {
        self.tool_path.as_deref().unwrap_or(&self.points)
    }
}

/// Blade compensation in use (spec §7.2): none until calibration test cuts choose a method.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Compensation {
    /// None: the blade follows the drawn path (the material's method is off).
    PendingCalibration,
    /// CutCraft swings the holder round corners; the cutter's own compensation is turned off.
    Software { offset_mm: f64 },
    /// The cutter's own blade-offset setting; its clearance is included in the bounds.
    Machine { offset_mm: f64 },
}

impl Compensation {
    pub fn for_material(material: &Material) -> Self {
        match material.offset_method {
            cutcraft_job::OffsetMethod::Off => Self::PendingCalibration,
            cutcraft_job::OffsetMethod::Software => Self::Software {
                offset_mm: material.offset_mm,
            },
            cutcraft_job::OffsetMethod::Machine => Self::Machine {
                offset_mm: material.offset_mm,
            },
        }
    }
}

/// An immutable, validated cut plan for one sheet (spec §6). The preview and the sender consume
/// the same plan; any change to its inputs makes a new plan with a new id.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct CutPlan {
    pub schema_version: u32,
    /// SHA-256 of everything that determines the cut.
    pub plan_id: String,
    pub sheet: String,
    pub sheet_name: String,
    pub project_revision: u64,
    pub source_sha256: String,
    pub material: Material,
    pub mirror: bool,
    pub mat: Mat,
    /// Mat size and usable cutting area, in mm.
    pub mat_mm: [f64; 2],
    pub cutting_area_mm: [f64; 2],
    pub operations: Vec<Operation>,
    /// Bounds of all blade-down paths, `[x0, y0, x1, y1]`.
    pub bounds_mm: Option<[f64; 4]>,
    /// Design size after mirror and rotation, before aids.
    pub design_size_mm: Option<[f64; 2]>,
    pub cut_length_mm: f64,
    pub travel_length_mm: f64,
    /// An estimate from the measured timing model (`estimate`), not a measured time.
    pub estimate_s: f64,
    pub compensation: Compensation,
    pub diagnostics: Vec<Diagnostic>,
    /// True when nothing blocks cutting this plan.
    pub valid: bool,
    /// Their piece of material at the start corner, when they gave its size (spec §5.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub piece_mm: Option<[f64; 2]>,
    /// Where they put the test cut, when they placed it themselves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_spot_mm: Option<[f64; 2]>,
    /// Where their piece's top-left corner sits, from the start corner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub piece_at_mm: Option<[f64; 2]>,
}

impl CutPlan {
    /// Whether `area` (`[x0, y0, x1, y1]` mm) touches the artwork: crosses or comes within
    /// [`TEST_CUT_CLEARANCE_MM`] of a cut, or lies inside a piece. Cutting aids don't count, so
    /// a test cut may sit in a gap inside the design's box.
    pub fn overlaps_art(&self, area: [f64; 4]) -> bool {
        let c = TEST_CUT_CLEARANCE_MM.min(1.0);
        let r = Rect::new(area[0] - c, area[1] - c, area[2] + c, area[3] + c);
        let centre = r.center();
        self.operations
            .iter()
            .filter(|o| !o.kind.is_aid())
            .any(|op| {
                let points: Vec<Point> = op.path().iter().map(|p| Point::new(p[0], p[1])).collect();
                let crosses = points.windows(2).any(|w| segment_meets_rect(w[0], w[1], r));
                let inside = op.closed
                    && op.kind == OperationKind::Outline
                    && polygon(&points).winding(centre) != 0;
                crosses || inside
            })
    }

    /// Their piece, `[x0, y0, x1, y1]` from the start corner, when they gave its size.
    pub fn piece_rect_mm(&self) -> Option<[f64; 4]> {
        let [w, h] = self.piece_mm?;
        let [x, y] = self.piece_at_mm.unwrap_or([0.0, 0.0]);
        Some([x, y, x + w, y + h])
    }

    /// Where cuts may go, `[x0, y0, x1, y1]`: their piece when they gave its size, within the
    /// cutting area.
    pub fn usable_area_mm(&self) -> [f64; 4] {
        let [aw, ah] = self.cutting_area_mm;
        match self.piece_rect_mm() {
            Some([x0, y0, x1, y1]) => [x0.max(0.0), y0.max(0.0), x1.min(aw), y1.min(ah)],
            None => [0.0, 0.0, aw, ah],
        }
    }
}

/// A contour ready for ordering: flattened, placed and with its containment known.
#[derive(Clone)]
struct Item {
    kind: OperationKind,
    id: Option<String>,
    colour: Option<String>,
    points: Vec<Point>,
    closed: bool,
}

/// Plan one sheet of a project.
pub fn plan_sheet(project: &Project, sheet_id: &str) -> Result<CutPlan, String> {
    let sheet = project
        .sheets
        .iter()
        .find(|s| s.id == sheet_id)
        .ok_or_else(|| format!("There is no {sheet_id}."))?;
    let design = &project.prepared;
    let mut diagnostics = Vec::new();
    if !design.cut_ready {
        diagnostics.push(Diagnostic::blocking(
            "plan.not_prepared",
            "The design still has unanswered questions or blockers in Make cut-ready.",
            vec![],
        ));
    }
    let mut items = sheet_items(design, sheet, &project.excluded_rows, project.scale);
    // Layering marks sit outside the whole design in assembly coordinates, so every sheet with
    // them places its art and marks alike (spec §5.4).
    let mut marks = match (sheet.layering_marks, whole_design_bounds(project)) {
        (true, Some(whole)) if !items.is_empty() => layering_marks(whole),
        _ => Vec::new(),
    };
    let design_bounds = bounds_of(
        items
            .iter()
            .chain(&marks)
            .flat_map(|i| i.points.iter().copied()),
    );
    let transform = design_bounds.map(|b| placement_transform(sheet, b));
    if let Some(t) = transform {
        for item in items.iter_mut().chain(&mut marks) {
            for p in &mut item.points {
                *p = t * *p;
            }
        }
    }
    // Spec §7.2: features that vanish at the cutter's resolution are found here, before the
    // preview, and listed, rather than refused at send time.
    let machine = &project.machine;
    let mut specks: Vec<String> = Vec::new();
    items.retain(|item| {
        // Checked on the points the plan will hold.
        let rounded: Vec<Point> = item
            .points
            .iter()
            .map(|p| {
                let [x, y] = quantize(*p);
                Point::new(x, y)
            })
            .collect();
        let gone = collapses(&rounded, item.closed, machine.step_mm);
        if gone
            && let Some(id) = &item.id
            && !specks.contains(id)
        {
            specks.push(id.clone());
        }
        !gone
    });
    if !specks.is_empty() {
        diagnostics.push(Diagnostic::info(
            "plan.specks_skipped",
            format!(
                "Left out {} speck(s) smaller than the cutter's {} mm step; they are too small to cut.",
                specks.len(),
                machine.step_mm
            ),
            specks,
        ));
    }
    let art_placed = !items.is_empty();
    let art_bounds = bounds_of(items.iter().flat_map(|i| i.points.iter().copied()));
    // The weeding box and weed lines go round the marks too: they are weeded like the art.
    let placed = bounds_of(
        items
            .iter()
            .chain(&marks)
            .flat_map(|i| i.points.iter().copied()),
    );
    if !art_placed {
        diagnostics.push(Diagnostic::blocking(
            "plan.empty",
            "This sheet has nothing to cut.",
            vec![],
        ));
    }
    let mut ordered = order(items, sheet.mat == Mat::Roll);
    ordered.extend(marks);
    if sheet.weed_lines
        && art_placed
        && let Some(b) = placed
    {
        let m = sheet.weeding_margin_mm;
        let region = Rect::new(b.x0 - m, b.y0 - m, b.x1 + m, b.y1 + m);
        let lines = weed_lines(&ordered, region, sheet.weed_inverted);
        ordered.extend(lines);
    }
    if sheet.weeding_box
        && art_placed
        && let Some(b) = placed
    {
        let m = sheet.weeding_margin_mm;
        let r = Rect::new(b.x0 - m, b.y0 - m, b.x1 + m, b.y1 + m);
        ordered.push(Item {
            kind: OperationKind::WeedingBox,
            id: None,
            colour: None,
            points: vec![
                Point::new(r.x0, r.y0),
                Point::new(r.x1, r.y0),
                Point::new(r.x1, r.y1),
                Point::new(r.x0, r.y1),
                Point::new(r.x0, r.y0),
            ],
            closed: true,
        });
    }
    let mut operations: Vec<Operation> = ordered
        .into_iter()
        .map(|item| Operation {
            kind: item.kind,
            item: item.id,
            colour: item.colour,
            points: item.points.iter().map(|p| quantize(*p)).collect(),
            closed: item.closed,
            passes: if item.kind.is_aid() {
                1
            } else {
                sheet.material.passes
            },
            tool_path: None,
        })
        .collect();
    let area = [machine.cutting_width_mm, machine.mat_length_mm(sheet.mat)];
    let compensation = tool_paths(&mut operations, &sheet.material);
    let blade = blade_bounds(&operations, compensation);
    // Spec §5.4: never clamp, crop or shrink; anything outside the area blocks.
    if let Some(b) = blade
        && (b.x0 < 0.0 || b.y0 < 0.0 || b.x1 > area[0] || b.y1 > area[1])
    {
        diagnostics.push(Diagnostic::blocking(
            "plan.out_of_bounds",
            format!(
                "The cut reaches outside the {:.0} × {:.0} mm cutting area. Move or shrink the design, or use a longer mat.",
                area[0], area[1]
            ),
            vec![],
        ));
    }
    let piece_at = sheet.piece_at_mm.unwrap_or([0.0, 0.0]);
    if let (Some(b), Some([w, h])) = (blade, sheet.piece_mm)
        && (b.x0 < piece_at[0] - 1e-6
            || b.y0 < piece_at[1] - 1e-6
            || b.x1 > piece_at[0] + w + 1e-6
            || b.y1 > piece_at[1] + h + 1e-6)
    {
        diagnostics.push(Diagnostic::blocking(
            "plan.outside_piece",
            format!(
                "The cut reaches off your {:.2} × {:.2} in piece. Move the cut lines onto the piece, shrink them, turn off the weeding box, or use a bigger piece.",
                w / 25.4,
                h / 25.4
            ),
            vec![],
        ));
    }
    // Cutting past the mat's sticky area cuts into the mat's edge (a warning: where the
    // cutter's corner falls on their mat is measured, not known).
    if sheet.mat != Mat::Roll
        && let Some(b) = blade
    {
        let [sx0, sy0, sx1, sy1] = sheet.mat_profile().sticky_in_cut();
        if b.x0 < sx0 - 1e-6 || b.y0 < sy0 - 1e-6 || b.x1 > sx1 + 1e-6 || b.y1 > sy1 + 1e-6 {
            diagnostics.push(Diagnostic::info(
                "plan.off_mat_grid",
                "Part of the cut is outside the mat's sticky grid, where nothing holds the material.",
                vec![],
            ));
        }
    }
    if sheet.mat == Mat::Roll && !sheet.material.roll {
        diagnostics.push(Diagnostic::info(
            "plan.material_not_for_roll",
            format!(
                "{} isn't marked as suitable for a roll; use a mat, or check the material's settings.",
                sheet.material.name
            ),
            vec![],
        ));
    }
    for d in &design.diagnostics {
        if d.severity == Severity::Info && d.code.starts_with("piece.") {
            diagnostics.push(d.clone());
        }
    }
    let (cut, travel) = lengths(&operations);
    let estimate_s = estimate(&operations, sheet.material.speed, machine);
    // A roll is drawn as long as the cut needs, in whole inches, at least a square foot.
    let mat_mm = match (sheet.mat, blade) {
        (Mat::Roll, Some(b)) => [304.8, ((b.y1 + 25.4) / 25.4).ceil().max(12.0) * 25.4],
        (Mat::Roll, None) => [304.8, 304.8],
        (mat, _) => [304.8, machine.mat_length_mm(mat)],
    };
    let mut plan = CutPlan {
        schema_version: PLAN_SCHEMA_VERSION,
        plan_id: String::new(),
        sheet: sheet.id.clone(),
        sheet_name: sheet.name.clone(),
        project_revision: project.revision,
        source_sha256: design.source_sha256.clone(),
        material: sheet.material.clone(),
        mirror: sheet.mirror,
        mat: sheet.mat,
        mat_mm,
        cutting_area_mm: area,
        bounds_mm: blade.map(|b| [b.x0, b.y0, b.x1, b.y1].map(round)),
        design_size_mm: art_bounds.map(|b| [round(b.width()), round(b.height())]),
        operations,
        cut_length_mm: round(cut),
        travel_length_mm: round(travel),
        estimate_s: (estimate_s * 10.0).round() / 10.0,
        compensation,
        valid: !diagnostics.iter().any(|d| d.severity == Severity::Blocking),
        diagnostics,
        piece_mm: sheet.piece_mm,
        test_spot_mm: sheet.test_spot_mm,
        piece_at_mm: sheet.piece_at_mm,
    };
    plan.plan_id = plan_id(&plan)?;
    Ok(plan)
}

/// Every piece and cut line of the design at its current size, in assembly (design) mm, with
/// whether it is cut: the shared assembly view (spec §5.3) and the main window's cut lines.
pub fn assembly(project: &Project) -> Vec<(Operation, bool)> {
    let design = &project.prepared;
    let cut = |row_has: &dyn Fn(&cutcraft_job::RowKey) -> bool| {
        project
            .sheets
            .iter()
            .flat_map(|s| &s.rows)
            .any(|r| row_has(r) && !project.excluded_rows.contains(r))
    };
    let op = |kind, id: &str, hex: &str, c: &Contour| Operation {
        kind,
        item: Some(id.to_owned()),
        colour: Some(hex.to_owned()),
        points: flatten(c, project.scale)
            .into_iter()
            .map(quantize)
            .collect(),
        closed: c.closed,
        passes: 1,
        tool_path: None,
    };
    let mut out = Vec::new();
    for piece in &design.pieces {
        let included = cut(&|r| r.has_piece(&piece.colour, &piece.layer));
        out.push((
            op(
                OperationKind::Outline,
                &piece.id,
                &piece.colour.hex,
                &piece.outer,
            ),
            included,
        ));
        for hole in &piece.holes {
            out.push((
                op(OperationKind::Hole, &piece.id, &piece.colour.hex, hole),
                included,
            ));
        }
    }
    for line in &design.cut_lines {
        let included = cut(&|r| r.has_line(&line.colour, &line.layer));
        out.push((
            op(
                OperationKind::Line,
                &line.id,
                &line.colour.hex,
                &line.contour,
            ),
            included,
        ));
    }
    out.retain(|(o, _)| o.points.len() >= 2);
    out
}

/// The whole design's width and height at its current size, every row included: what the
/// size fields show (spec §5.1.1).
pub fn design_size_mm(project: &Project) -> Option<[f64; 2]> {
    let design = &project.prepared;
    let contours = design
        .pieces
        .iter()
        .map(|p| &p.outer)
        .chain(design.cut_lines.iter().map(|l| &l.contour));
    bounds_of(contours.flat_map(|c| flatten(c, project.scale)))
        .map(|b| [round(b.width()), round(b.height())])
}

/// The whole design's bounds at its current size, every row included, in assembly mm.
fn whole_design_bounds(project: &Project) -> Option<Rect> {
    let design = &project.prepared;
    let contours = design
        .pieces
        .iter()
        .map(|p| &p.outer)
        .chain(design.cut_lines.iter().map(|l| &l.contour));
    bounds_of(contours.flat_map(|c| flatten(c, project.scale)))
}

/// Four solid triangles on the design's centre lines, left, right, above and below it, each
/// pointing at the artwork. They are weeded like the art and stay on the carrier, so each layer
/// is placed by lining their tips up with centre lines marked on the garment.
fn layering_marks(whole: Rect) -> Vec<Item> {
    let mid = whole.center();
    let (half, depth) = (MARK_MM / 2.0, MARK_MM);
    let near = MARK_GAP_MM;
    // (tip, the direction from the tip away from the art).
    let marks = [
        (Point::new(whole.x0 - near, mid.y), (-1.0, 0.0)),
        (Point::new(whole.x1 + near, mid.y), (1.0, 0.0)),
        (Point::new(mid.x, whole.y0 - near), (0.0, -1.0)),
        (Point::new(mid.x, whole.y1 + near), (0.0, 1.0)),
    ];
    marks
        .into_iter()
        .map(|(tip, (dx, dy))| {
            let base = Point::new(tip.x + dx * depth, tip.y + dy * depth);
            // Across the direction of the point.
            let (ax, ay) = (-dy * half, dx * half);
            Item {
                kind: OperationKind::Mark,
                id: None,
                colour: None,
                points: vec![
                    tip,
                    Point::new(base.x + ax, base.y + ay),
                    Point::new(base.x - ax, base.y - ay),
                    tip,
                ],
                closed: true,
            }
        })
        .collect()
}

/// Weed lines (spec §5.4): horizontal and vertical lines about [`WEED_LINE_SPACING_MM`] apart
/// across `region`, kept only in the waste and never within [`WEED_LINE_CLEARANCE_MM`] of any
/// edge, so they never cross kept vinyl. The waste is outside the pieces, or, `inverted` (a
/// stencil), inside them but outside their holes and the layering marks. Checked on the exact
/// polylines, not on bounding boxes.
fn weed_lines(art: &[Item], region: Rect, inverted: bool) -> Vec<Item> {
    let cl = WEED_LINE_CLEARANCE_MM;
    let mut out = Vec::new();
    for horizontal in [true, false] {
        // `u` runs along the line, `v` across it.
        let (lo, hi, from, to) = if horizontal {
            (region.y0, region.y1, region.x0, region.x1)
        } else {
            (region.x0, region.x1, region.y0, region.y1)
        };
        let uv = |p: Point| if horizontal { (p.x, p.y) } else { (p.y, p.x) };
        let n = ((hi - lo) / WEED_LINE_SPACING_MM).floor() as usize;
        for k in 1..=n {
            let c = lo + (hi - lo) * k as f64 / (n + 1) as f64;
            let mut blocked: Vec<(f64, f64)> = Vec::new();
            // Inverted: where the line runs inside a piece's outline, before the rest is taken away.
            let mut waste: Vec<(f64, f64)> = Vec::new();
            for item in art {
                let points: Vec<(f64, f64)> = item.points.iter().map(|p| uv(*p)).collect();
                let mut segments: Vec<_> = points.windows(2).map(|w| (w[0], w[1])).collect();
                if item.closed
                    && let (Some(&first), Some(&last)) = (points.first(), points.last())
                    && first != last
                {
                    segments.push((last, first));
                }
                // Near an edge: the part of each segment within the clearance band.
                for &((u0, v0), (u1, v1)) in &segments {
                    let (t0, t1) = if (v1 - v0).abs() < 1e-12 {
                        if (v0 - c).abs() > cl {
                            continue;
                        }
                        (0.0, 1.0)
                    } else {
                        let ta = (c - cl - v0) / (v1 - v0);
                        let tb = (c + cl - v0) / (v1 - v0);
                        let (t0, t1) = (ta.min(tb).max(0.0), ta.max(tb).min(1.0));
                        if t0 > t1 {
                            continue;
                        }
                        (t0, t1)
                    };
                    let (ua, ub) = (u0 + (u1 - u0) * t0, u0 + (u1 - u0) * t1);
                    blocked.push((ua.min(ub) - cl, ua.max(ub) + cl));
                }
                // Inside a closed contour: where the line crosses its interior.
                if item.closed {
                    let mut crossings: Vec<f64> = segments
                        .iter()
                        .filter(|((_, v0), (_, v1))| (*v0 <= c) != (*v1 <= c))
                        .map(|((u0, v0), (u1, v1))| u0 + (c - v0) * (u1 - u0) / (v1 - v0))
                        .collect();
                    crossings.sort_by(f64::total_cmp);
                    for pair in crossings.chunks_exact(2) {
                        let inside = (pair[0] - cl, pair[1] + cl);
                        match (inverted, item.kind) {
                            // A hole lies inside its piece, already blocked.
                            (false, OperationKind::Hole) => {}
                            (false, _) => blocked.push(inside),
                            (true, OperationKind::Outline) => waste.push((pair[0], pair[1])),
                            // Holes are kept vinyl; marks are too small to split.
                            (true, _) => blocked.push(inside),
                        }
                    }
                }
            }
            let candidates = if inverted {
                waste
                    .into_iter()
                    .map(|(a, b)| (a.max(from), b.min(to)))
                    .filter(|(a, b)| a < b)
                    .collect()
            } else {
                vec![(from, to)]
            };
            blocked.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut free = Vec::new();
            for (from, to) in candidates {
                let mut at = from;
                for &(start, end) in &blocked {
                    if end <= at {
                        continue;
                    }
                    if start >= to {
                        break;
                    }
                    if start > at {
                        free.push((at, start));
                    }
                    at = at.max(end);
                    if at >= to {
                        break;
                    }
                }
                if at < to {
                    free.push((at, to));
                }
            }
            for (start, end) in free {
                if end - start < WEED_LINE_MIN_MM {
                    continue;
                }
                let point = |u: f64| {
                    if horizontal {
                        Point::new(u, c)
                    } else {
                        Point::new(c, u)
                    }
                };
                out.push(Item {
                    kind: OperationKind::WeedLine,
                    id: None,
                    colour: None,
                    points: vec![point(start), point(end)],
                    closed: false,
                });
            }
        }
    }
    out
}

/// Whether the segment `a`–`b` touches `r` (an end inside, or crossing an edge).
fn segment_meets_rect(a: Point, b: Point, r: Rect) -> bool {
    if r.contains(a) || r.contains(b) {
        return true;
    }
    let corners = [
        Point::new(r.x0, r.y0),
        Point::new(r.x1, r.y0),
        Point::new(r.x1, r.y1),
        Point::new(r.x0, r.y1),
    ];
    let cross =
        |o: Point, p: Point, q: Point| (p.x - o.x) * (q.y - o.y) - (p.y - o.y) * (q.x - o.x);
    (0..4).any(|i| {
        let (c, d) = (corners[i], corners[(i + 1) % 4]);
        let (d1, d2) = (cross(c, d, a), cross(c, d, b));
        let (d3, d4) = (cross(a, b, c), cross(a, b, d));
        (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0)
    })
}

/// Overcut is at most this share of a contour's length (spec §7.2: bounded on small shapes).
const OVERCUT_MAX_SHARE: f64 = 0.1;
/// Corners turn the holder through arcs in steps no larger than this.
const ARC_STEP_RAD: f64 = 10.0 * std::f64::consts::PI / 180.0;

/// Give each operation the path the tool follows, from the material's overcut and blade
/// compensation (spec §7.2), and say which compensation is in use. Overcut applies to closed
/// contours before compensation; software compensation then swings the holder round every
/// corner. Machine compensation leaves the path to the cutter.
fn tool_paths(operations: &mut [Operation], material: &Material) -> Compensation {
    let compensation = Compensation::for_material(material);
    let overcut = material.overcut_mm.max(0.0);
    for op in operations.iter_mut() {
        let points: Vec<Point> = op.points.iter().map(|p| Point::new(p[0], p[1])).collect();
        let mut path = points.clone();
        if op.closed && overcut > 0.0 {
            path = extend_closed(&path, overcut);
        }
        if let Compensation::Software { offset_mm } = compensation {
            path = drag_knife(&path, offset_mm);
        }
        if path != points {
            op.tool_path = Some(path.into_iter().map(quantize).collect());
        }
    }
    compensation
}

/// Bounds of everything the blade does: tool paths, and the cutter's own compensation clearance.
fn blade_bounds(operations: &[Operation], compensation: Compensation) -> Option<Rect> {
    let b = bounds_of(
        operations
            .iter()
            .flat_map(|o| o.path().iter().chain(&o.points))
            .map(|p| Point::new(p[0], p[1])),
    )?;
    Some(match compensation {
        Compensation::Machine { offset_mm } => b.inflate(offset_mm, offset_mm),
        _ => b,
    })
}

/// A closed contour cut on past its start by `length` along itself, at most
/// [`OVERCUT_MAX_SHARE`] of its perimeter.
fn extend_closed(points: &[Point], length: f64) -> Vec<Point> {
    let perimeter: f64 = points.windows(2).map(|w| w[0].distance(w[1])).sum();
    let mut left = length.min(perimeter * OVERCUT_MAX_SHARE);
    let mut out = points.to_vec();
    for w in points.windows(2) {
        if left <= 1e-9 {
            break;
        }
        let step = w[0].distance(w[1]);
        if step <= left {
            out.push(w[1]);
            left -= step;
        } else {
            out.push(w[0].lerp(w[1], left / step));
            left = 0.0;
        }
    }
    out
}

/// Drag-knife compensation: the blade tip trails the holder's axis by `offset`, so the holder
/// runs `offset` past each vertex along the segment it finishes, then swings round the vertex on
/// an arc of that radius to the next direction. The tip then follows the drawn corners. The
/// first `offset` of a cut depends on how the blade starts out; overcut covers it on closed shapes.
fn drag_knife(points: &[Point], offset: f64) -> Vec<Point> {
    let mut pts: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        if pts.last().is_none_or(|q: &Point| q.distance(*p) > 1e-9) {
            pts.push(*p);
        }
    }
    if pts.len() < 2 || offset <= 0.0 {
        return pts;
    }
    let dirs: Vec<kurbo::Vec2> = pts.windows(2).map(|w| (w[1] - w[0]).normalize()).collect();
    let mut out = vec![pts[0] + dirs[0] * offset];
    for (i, d) in dirs.iter().enumerate() {
        let corner = pts[i + 1];
        out.push(corner + *d * offset);
        if let Some(next) = dirs.get(i + 1) {
            let mut turn = next.atan2() - d.atan2();
            while turn > std::f64::consts::PI {
                turn -= std::f64::consts::TAU;
            }
            while turn <= -std::f64::consts::PI {
                turn += std::f64::consts::TAU;
            }
            let steps = (turn.abs() / ARC_STEP_RAD).ceil() as usize;
            for k in 1..=steps {
                let a = d.atan2() + turn * k as f64 / steps as f64;
                out.push(corner + kurbo::Vec2::from_angle(a) * offset);
            }
        }
    }
    out
}

/// SHA-256 of everything that determines the cut.
fn plan_id(plan: &CutPlan) -> Result<String, String> {
    let identity = serde_json::to_vec(&(
        &plan.operations,
        &plan.material,
        plan.mirror,
        plan.mat,
        &plan.source_sha256,
        &plan.compensation,
        plan.piece_mm,
        plan.piece_at_mm,
        plan.cutting_area_mm,
        plan.mat_mm,
    ))
    .map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(identity)))
}

type Mark = (OperationKind, bool, Vec<[f64; 2]>);

fn calibration_line(points: Vec<[f64; 2]>) -> Mark {
    (OperationKind::Line, false, points)
}

fn calibration_square(x: f64, y: f64, s: f64) -> Mark {
    let points = vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s], [x, y]];
    (OperationKind::Outline, true, points)
}

/// A +x arrow with one head, ending at `(x, y)`, `length` long.
fn x_arrow(x: f64, y: f64, length: f64) -> Mark {
    calibration_line(vec![
        [x - length, y],
        [x, y],
        [x - 6.0, y - 4.0],
        [x, y],
        [x - 6.0, y + 4.0],
    ])
}

/// A +y arrow with two heads, ending at `(x, y)`, `length` long.
fn y_arrow(x: f64, y: f64, length: f64) -> [Mark; 2] {
    [
        calibration_line(vec![
            [x, y - length],
            [x, y],
            [x - 4.0, y - 6.0],
            [x, y],
            [x + 4.0, y - 6.0],
        ]),
        calibration_line(vec![[x - 4.0, y - 12.0], [x, y - 6.0], [x + 4.0, y - 12.0]]),
    ]
}

/// The pen calibration sheet for the hardware procedure (spec §9: origin, axes, dimensions,
/// small detail and travel): a crosshair 10 mm from the start corner, a 100 mm square, a 1 in
/// square, a +x arrow (one head) and a +y arrow (two heads), a 3 mm circle, and a 10 mm square
/// near the far corner. Every element is drawn, open lines included; no weeding box.
pub fn calibration_plan(
    material: &Material,
    machine: &cutcraft_job::MachineProfile,
) -> Result<CutPlan, String> {
    let circle: Vec<[f64; 2]> = (0..=48)
        .map(|k| {
            let a = std::f64::consts::TAU * f64::from(k % 48) / 48.0;
            [170.0 + 1.5 * a.cos(), 80.0 + 1.5 * a.sin()]
        })
        .collect();
    let mut items = vec![
        calibration_line(vec![[5.0, 10.0], [15.0, 10.0]]),
        calibration_line(vec![[10.0, 5.0], [10.0, 15.0]]),
        calibration_square(20.0, 20.0, 100.0),
        calibration_square(140.0, 20.0, 25.4),
        x_arrow(80.0, 140.0, 60.0),
    ];
    items.extend(y_arrow(20.0, 200.0, 60.0));
    items.push((OperationKind::Outline, true, circle));
    items.push(calibration_square(284.0, 284.0, 10.0));
    calibration(
        items,
        "Pen calibration",
        [5.0, 5.0, 294.0, 294.0],
        material,
        machine,
    )
}

/// The calibration sheet scored with the blade on Letter (8.5 × 11 in) paper or cardstock at
/// the mat's start corner: everything keeps 15 mm inside the paper and at least 60 mm below its
/// top edge, where tape often holds it. A 100 mm square and a 1 in square, a +x arrow (one head),
/// a +y arrow (two heads) and a 10 mm square near the far corner. Origin offset comes from the
/// big square's corner once its size has been checked; no crosshair is needed at the corner.
pub fn letter_calibration_plan(
    material: &Material,
    machine: &cutcraft_job::MachineProfile,
) -> Result<CutPlan, String> {
    let mut items = vec![
        calibration_square(20.0, 60.0, 100.0),
        calibration_square(140.0, 60.0, 25.4),
        x_arrow(190.0, 110.0, 50.0),
    ];
    items.extend(y_arrow(20.0, 240.0, 60.0));
    items.push(calibration_square(190.0, 250.0, 10.0));
    calibration(
        items,
        "Letter calibration",
        [16.0, 60.0, 200.0, 260.0],
        material,
        machine,
    )
}

fn calibration(
    items: Vec<Mark>,
    name: &str,
    bounds: [f64; 4],
    material: &Material,
    machine: &cutcraft_job::MachineProfile,
) -> Result<CutPlan, String> {
    let operations: Vec<Operation> = items
        .into_iter()
        .map(|(kind, closed, points)| Operation {
            kind,
            item: Some("calibration".into()),
            colour: None,
            points: points
                .into_iter()
                .map(|[x, y]| quantize(Point::new(x, y)))
                .collect(),
            closed,
            passes: 1,
            tool_path: None,
        })
        .collect();
    let (cut, travel) = lengths(&operations);
    let estimate_s = estimate(&operations, material.speed, machine);
    let mut plan = CutPlan {
        schema_version: PLAN_SCHEMA_VERSION,
        plan_id: String::new(),
        sheet: "calibration".into(),
        sheet_name: name.into(),
        project_revision: 0,
        source_sha256: String::new(),
        material: material.clone(),
        mirror: false,
        mat: Mat::Mat12x12,
        mat_mm: [304.8, machine.mat_length_mm(Mat::Mat12x12)],
        cutting_area_mm: [
            machine.cutting_width_mm,
            machine.mat_length_mm(Mat::Mat12x12),
        ],
        bounds_mm: Some(bounds),
        design_size_mm: Some([bounds[2] - bounds[0], bounds[3] - bounds[1]]),
        estimate_s: (estimate_s * 10.0).round() / 10.0,
        operations,
        cut_length_mm: round(cut),
        travel_length_mm: round(travel),
        compensation: Compensation::PendingCalibration,
        diagnostics: Vec::new(),
        valid: true,
        piece_mm: None,
        test_spot_mm: None,
        piece_at_mm: None,
    };
    plan.plan_id = plan_id(&plan)?;
    Ok(plan)
}

/// The test cut's square side (spec §5.5: a triangle inside a square), 1 in.
pub const TEST_CUT_MM: f64 = 25.4;
/// Clear space kept around the test cut, from artwork and the area's edge.
const TEST_CUT_CLEARANCE_MM: f64 = 5.0;

/// Where the test cut goes on a sheet's mat: the first spot, nearest the start corner, that
/// keeps clear of everything `occupied` (`[x0, y0, x1, y1]` each: the design, earlier test
/// cuts) and inside the cutting area. None when nothing fits: then they need a separate scrap.
pub fn test_cut_spot(area_mm: [f64; 4], occupied: &[[f64; 4]]) -> Option<[f64; 2]> {
    let c = TEST_CUT_CLEARANCE_MM;
    let size = TEST_CUT_MM;
    let step = 5.0;
    let [ax0, ay0, ax1, ay1] = area_mm;
    let mut y = ay0 + c;
    while y + size + c <= ay1 {
        let mut x = ax0 + c;
        while x + size + c <= ax1 {
            let clear = occupied.iter().all(|&[x0, y0, x1, y1]| {
                x + size + c <= x0 || x >= x1 + c || y + size + c <= y0 || y >= y1 + c
            });
            if clear {
                return Some([x, y]);
            }
            x += step;
        }
        y += step;
    }
    None
}

/// The test cut for a sheet's material (spec §5.5): a triangle cut first, then the square
/// around it, with its top-left corner at `at`. It uses the sheet's material and mat, is never
/// mirrored and has no weeding box.
pub fn test_cut_plan(project: &Project, sheet_id: &str, at: [f64; 2]) -> Result<CutPlan, String> {
    let sheet = project
        .sheets
        .iter()
        .find(|s| s.id == sheet_id)
        .ok_or_else(|| format!("There is no {sheet_id}."))?;
    let machine = &project.machine;
    let [x, y] = at;
    let s = TEST_CUT_MM;
    let inset = (s - 15.4) / 2.0;
    let triangle = vec![
        [x + inset, y + s - inset],
        [x + s / 2.0, y + inset],
        [x + s - inset, y + s - inset],
        [x + inset, y + s - inset],
    ];
    let square = vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s], [x, y]];
    let operations: Vec<Operation> = [
        (OperationKind::Hole, triangle),
        (OperationKind::Outline, square),
    ]
    .into_iter()
    .map(|(kind, points)| Operation {
        kind,
        item: Some("test-cut".into()),
        colour: None,
        points: points
            .into_iter()
            .map(|[px, py]| quantize(Point::new(px, py)))
            .collect(),
        closed: true,
        passes: sheet.material.passes,
        tool_path: None,
    })
    .collect();
    // The test cut tries the material's whole setting, compensation and overcut included.
    let mut operations = operations;
    let compensation = tool_paths(&mut operations, &sheet.material);
    let area = [machine.cutting_width_mm, machine.mat_length_mm(sheet.mat)];
    let mut diagnostics = Vec::new();
    let reach = blade_bounds(&operations, compensation);
    if x < 0.0
        || y < 0.0
        || x + s > area[0]
        || y + s > area[1]
        || reach.is_some_and(|b| b.x0 < 0.0 || b.y0 < 0.0 || b.x1 > area[0] || b.y1 > area[1])
    {
        diagnostics.push(Diagnostic::blocking(
            "plan.out_of_bounds",
            "The test cut reaches outside the cutting area.",
            vec![],
        ));
    }
    if let Some([w, h]) = sheet.piece_mm {
        let [left, top] = sheet.piece_at_mm.unwrap_or([0.0, 0.0]);
        if reach.is_some_and(|b| b.x0 < left || b.y0 < top || b.x1 > left + w || b.y1 > top + h) {
            diagnostics.push(Diagnostic::blocking(
                "plan.off_piece",
                "The test pattern reaches outside the material scrap.",
                vec![],
            ));
        }
    }
    let (cut, travel) = lengths(&operations);
    let estimate_s = estimate(&operations, sheet.material.speed, machine);
    let mut plan = CutPlan {
        schema_version: PLAN_SCHEMA_VERSION,
        plan_id: String::new(),
        sheet: sheet.id.clone(),
        sheet_name: format!("Test cut: {}", sheet.material.name),
        project_revision: project.revision,
        source_sha256: String::new(),
        material: sheet.material.clone(),
        mirror: false,
        mat: sheet.mat,
        mat_mm: [304.8, machine.mat_length_mm(sheet.mat)],
        cutting_area_mm: area,
        bounds_mm: reach.map(|b| [b.x0, b.y0, b.x1, b.y1].map(round)),
        design_size_mm: Some([s, s]),
        operations,
        cut_length_mm: round(cut),
        travel_length_mm: round(travel),
        estimate_s: (estimate_s * 10.0).round() / 10.0,
        compensation,
        valid: diagnostics.is_empty(),
        diagnostics,
        piece_mm: sheet.piece_mm,
        test_spot_mm: None,
        piece_at_mm: sheet.piece_at_mm,
    };
    plan.plan_id = plan_id(&plan)?;
    Ok(plan)
}

/// The sheet's included contours, flattened in design (page) mm.
fn sheet_items(
    design: &PreparedDesign,
    sheet: &Sheet,
    excluded: &[cutcraft_job::RowKey],
    scale: f64,
) -> Vec<Item> {
    let included = |pick: &dyn Fn(&cutcraft_job::RowKey) -> bool| {
        sheet.rows.iter().any(|r| pick(r) && !excluded.contains(r))
    };
    let mut items = Vec::new();
    for piece in &design.pieces {
        if !included(&|r| r.has_piece(&piece.colour, &piece.layer)) {
            continue;
        }
        for hole in &piece.holes {
            items.push(Item {
                kind: OperationKind::Hole,
                id: Some(piece.id.clone()),
                colour: Some(piece.colour.hex.clone()),
                points: flatten(hole, scale),
                closed: true,
            });
        }
        items.push(Item {
            kind: OperationKind::Outline,
            id: Some(piece.id.clone()),
            colour: Some(piece.colour.hex.clone()),
            points: flatten(&piece.outer, scale),
            closed: true,
        });
    }
    for line in &design.cut_lines {
        if !included(&|r| r.has_line(&line.colour, &line.layer)) {
            continue;
        }
        items.push(Item {
            kind: OperationKind::Line,
            id: Some(line.id.clone()),
            colour: Some(line.colour.hex.clone()),
            points: flatten(&line.contour, scale),
            closed: line.contour.closed,
        });
    }
    items.retain(|i| i.points.len() >= 2);
    items
}

/// Whether a polyline vanishes when rounded to the cutter's step: a closed contour that encloses
/// no area, or an open one that becomes a point.
fn collapses(points: &[Point], closed: bool, step: f64) -> bool {
    if step <= 0.0 {
        return false;
    }
    // The same arithmetic as the encoder (multiply by units per mm), so both agree at half steps.
    let per_mm = 1.0 / step;
    let mut grid: Vec<(i64, i64)> = Vec::new();
    for p in points {
        let q = ((p.x * per_mm).round() as i64, (p.y * per_mm).round() as i64);
        if grid.last() != Some(&q) {
            grid.push(q);
        }
    }
    if !closed {
        return grid.len() < 2;
    }
    let area: i64 = grid
        .windows(2)
        .map(|w| w[0].0 * w[1].1 - w[1].0 * w[0].1)
        .sum();
    grid.len() < 4 || area == 0
}

fn contour_path(c: &Contour) -> BezPath {
    let mut bp = BezPath::new();
    bp.move_to((c.start[0], c.start[1]));
    for [a, b, e] in &c.segments {
        bp.curve_to((a[0], a[1]), (b[0], b[1]), (e[0], e[1]));
    }
    if c.closed {
        bp.close_path();
    }
    bp
}

/// A polyline within [`FLATTEN_TOLERANCE_MM`] of the contour at its cut size (spec §7.2: size
/// first, so the tolerance is in cut-space mm); closed ones end at their start.
fn flatten(c: &Contour, scale: f64) -> Vec<Point> {
    let mut points: Vec<Point> = Vec::new();
    let path = Affine::scale(scale) * contour_path(c);
    kurbo::flatten(path, FLATTEN_TOLERANCE_MM, |el| match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) if points.last() != Some(&p) => points.push(p),
        PathEl::ClosePath => {
            if let Some(&first) = points.first()
                && points.last() != Some(&first)
            {
                points.push(first);
            }
        }
        _ => {}
    });
    points
}

fn bounds_of(points: impl Iterator<Item = Point>) -> Option<Rect> {
    points.fold(None, |acc: Option<Rect>, p| {
        Some(acc.map_or(Rect::from_points(p, p), |r| r.union_pt(p)))
    })
}

/// Mirror (across the design's vertical centre line) and rotate about the design's centre, then
/// move the result's top-left corner to the sheet's offset.
fn placement_transform(sheet: &Sheet, design: Rect) -> Affine {
    let centre = design.center().to_vec2();
    let mut t = Affine::translate(-centre);
    if sheet.mirror {
        t = Affine::FLIP_X * t;
    }
    t = Affine::rotate(sheet.placement.rotation_deg.to_radians()) * t;
    let turned = t.transform_rect_bbox(design);
    Affine::translate((
        sheet.placement.offset_mm[0] - turned.x0,
        sheet.placement.offset_mm[1] - turned.y0,
    )) * t
}

fn polygon(points: &[Point]) -> BezPath {
    let mut bp = BezPath::new();
    if let Some((first, rest)) = points.split_first() {
        bp.move_to(*first);
        for p in rest {
            bp.line_to(*p);
        }
        bp.close_path();
    }
    bp
}

/// Spec §7.2: a contour is cut only after everything inside it (holes and islands before the
/// cut that releases them; cut lines inside a piece before its outline). Among the contours
/// ready to cut, the nearest start comes next, ties broken by input order. Closed contours start
/// at their vertex nearest the blade; open lines run from their nearer end.
/// On a roll, each mm of feeding back counts as this much travel: the roll is cut top to bottom
/// as far as containment allows (spec §7.2).
const ROLL_REVERSAL_WEIGHT: f64 = 10.0;

fn order(items: Vec<Item>, roll: bool) -> Vec<Item> {
    let shapes: Vec<Option<BezPath>> = items
        .iter()
        .map(|i| i.closed.then(|| polygon(&i.points)))
        .collect();
    // Each item's area, bounding box and top, worked out once: comparing every item with every
    // other is cheap with them, and the slow tests (winding, every vertex) run only when a box
    // says they could matter. The order is exactly what testing everything would give.
    let area: Vec<f64> = shapes
        .iter()
        .map(|s| s.as_ref().map_or(0.0, |s| s.area().abs()))
        .collect();
    let boxes: Vec<[f64; 4]> = items
        .iter()
        .map(|i| {
            i.points.iter().fold(
                [
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ],
                |[x0, y0, x1, y1], p| [x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)],
            )
        })
        .collect();
    // inside[k]: the closed contours that contain item k (by a point on it, and smaller area).
    // A point outside a contour's box has winding 0, so the box rules most pairs out.
    let inside: Vec<Vec<usize>> = (0..items.len())
        .map(|k| {
            let probe = items[k].points[items[k].points.len() / 2];
            (0..items.len())
                .filter(|&j| {
                    let [x0, y0, x1, y1] = boxes[j];
                    j != k
                        && area[j] > area[k]
                        && (x0..=x1).contains(&probe.x)
                        && (y0..=y1).contains(&probe.y)
                        && shapes[j].as_ref().is_some_and(|s| s.winding(probe) != 0)
                })
                .collect()
        })
        .collect();
    let mut waiting: Vec<usize> = vec![0; items.len()];
    for containers in &inside {
        for &j in containers {
            waiting[j] += 1;
        }
    }
    let mut items: Vec<Option<Item>> = items.into_iter().map(Some).collect();
    let mut done = vec![false; items.len()];
    let mut at = Point::ORIGIN;
    let mut out = Vec::with_capacity(items.len());
    while out.len() < done.len() {
        let mut best: Option<(f64, usize, usize, bool)> = None;
        for k in 0..items.len() {
            if done[k] || waiting[k] > 0 {
                continue;
            }
            let item = items[k].as_ref().expect("not yet taken");
            let reversal = if roll {
                ROLL_REVERSAL_WEIGHT * (at.y - boxes[k][1]).max(0.0)
            } else {
                0.0
            };
            // No vertex is nearer than the box: skip what can't beat the best so far.
            if let Some((d, ..)) = best {
                let [x0, y0, x1, y1] = boxes[k];
                let dx = (x0 - at.x).max(at.x - x1).max(0.0);
                let dy = (y0 - at.y).max(at.y - y1).max(0.0);
                if (dx * dx + dy * dy).sqrt() + reversal >= d - 1e-9 {
                    continue;
                }
            }
            let (start, reverse, mut distance) = entry(item, at);
            distance += reversal;
            if best.is_none_or(|(d, ..)| distance < d - 1e-9) {
                best = Some((distance, k, start, reverse));
            }
        }
        let Some((_, k, start, reverse)) = best else {
            // Containment cycles cannot happen with strict area ordering; take what remains.
            let k = (0..done.len()).find(|&k| !done[k]).expect("items remain");
            waiting[k] = 0;
            continue;
        };
        let mut item = items[k].take().expect("not yet taken");
        if item.closed {
            // Rotate the loop to begin at the chosen vertex.
            let mut ring: Vec<Point> = item.points[..item.points.len() - 1].to_vec();
            ring.rotate_left(start);
            ring.push(ring[0]);
            item.points = ring;
        } else if reverse {
            item.points.reverse();
        }
        at = *item.points.last().expect("at least two points");
        done[k] = true;
        for &j in &inside[k] {
            waiting[j] -= 1;
        }
        out.push(item);
    }
    out
}

/// Where to enter an item from `at`: start vertex, whether to reverse, and travel distance.
fn entry(item: &Item, at: Point) -> (usize, bool, f64) {
    if item.closed {
        let ring = &item.points[..item.points.len() - 1];
        let (i, d) = ring
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p.distance(at)))
            .fold((0, f64::INFINITY), |best, c| {
                if c.1 < best.1 - 1e-9 { c } else { best }
            });
        (i, false, d)
    } else {
        let first = item.points[0].distance(at);
        let last = item.points[item.points.len() - 1].distance(at);
        if last < first - 1e-9 {
            (0, true, last)
        } else {
            (0, false, first)
        }
    }
}

fn lengths(operations: &[Operation]) -> (f64, f64) {
    let mut cut = 0.0;
    let mut travel = 0.0;
    let mut at = [0.0, 0.0];
    let dist = |a: [f64; 2], b: [f64; 2]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
    for op in operations {
        let path = op.path();
        travel += dist(at, path[0]);
        let length: f64 = path.windows(2).map(|w| dist(w[0], w[1])).sum();
        cut += length * f64::from(op.passes);
        at = *path.last().expect("at least two points");
    }
    travel += dist(at, [0.0, 0.0]);
    (cut, travel)
}

/// Estimated motion time from the measured timing model: cuts at the material's speed with a
/// slowdown for each change of direction, tool-up travel at the machine's travel speed, a lift
/// per cut and pass, and a cost per point. Each pass starts with a tool-up move to the cut's
/// start and the job ends with one back to the origin, as the encoder sends them on a mat.
pub fn estimate(
    operations: &[Operation],
    speed: u8,
    machine: &cutcraft_job::MachineProfile,
) -> f64 {
    let draw = f64::from(speed.max(1)) * machine.speed_step_mm_s;
    let dist = |a: [f64; 2], b: [f64; 2]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
    let mut at = [0.0, 0.0];
    let mut total = 0.0;
    for op in operations {
        let path = op.path();
        let length: f64 = path.windows(2).map(|w| dist(w[0], w[1])).sum();
        let turned: f64 = path
            .windows(3)
            .map(|w| {
                let a = (w[1][1] - w[0][1]).atan2(w[1][0] - w[0][0]);
                let b = (w[2][1] - w[1][1]).atan2(w[2][0] - w[1][0]);
                let d = (b - a).rem_euclid(std::f64::consts::TAU);
                d.min(std::f64::consts::TAU - d)
            })
            .sum();
        for _ in 0..op.passes.max(1) {
            total += dist(at, path[0]) / machine.travel_mm_s
                + machine.tool_change_s
                + length / draw
                + turned * machine.turn_s_per_rad
                + path.len() as f64 * machine.point_s;
            at = *path.last().expect("at least two points");
        }
    }
    if !operations.is_empty() {
        total += dist(at, [0.0, 0.0]) / machine.travel_mm_s;
    }
    total
}

fn round(v: f64) -> f64 {
    (v * 1e3).round() / 1e3
}

fn quantize(p: Point) -> [f64; 2] {
    let q = |v: f64| (v / QUANTUM_MM).round() * QUANTUM_MM;
    [q(p.x), q(p.y)]
}

#[cfg(test)]
mod tests {
    /// The ordering as first written, testing every pair and every vertex: [`order`] must give
    /// exactly the same result, only faster.
    #[allow(dead_code)]
    fn order_reference(items: Vec<Item>, roll: bool) -> Vec<Item> {
        let shapes: Vec<Option<BezPath>> = items
            .iter()
            .map(|i| i.closed.then(|| polygon(&i.points)))
            .collect();
        let area = |k: usize| shapes[k].as_ref().map_or(0.0, |s| s.area().abs());
        // inside[k]: the closed contours that contain item k (by a point on it, and smaller area).
        let inside: Vec<Vec<usize>> = (0..items.len())
            .map(|k| {
                let probe = items[k].points[items[k].points.len() / 2];
                (0..items.len())
                    .filter(|&j| {
                        j != k
                            && shapes[j]
                                .as_ref()
                                .is_some_and(|s| s.winding(probe) != 0 && area(j) > area(k))
                    })
                    .collect()
            })
            .collect();
        let mut waiting: Vec<usize> = vec![0; items.len()];
        for containers in &inside {
            for &j in containers {
                waiting[j] += 1;
            }
        }
        let mut items: Vec<Option<Item>> = items.into_iter().map(Some).collect();
        let mut done = vec![false; items.len()];
        let mut at = Point::ORIGIN;
        let mut out = Vec::with_capacity(items.len());
        while out.len() < done.len() {
            let mut best: Option<(f64, usize, usize, bool)> = None;
            for k in 0..items.len() {
                if done[k] || waiting[k] > 0 {
                    continue;
                }
                let item = items[k].as_ref().expect("not yet taken");
                let (start, reverse, mut distance) = entry(item, at);
                if roll {
                    let top = item
                        .points
                        .iter()
                        .map(|p| p.y)
                        .fold(f64::INFINITY, f64::min);
                    distance += ROLL_REVERSAL_WEIGHT * (at.y - top).max(0.0);
                }
                if best.is_none_or(|(d, ..)| distance < d - 1e-9) {
                    best = Some((distance, k, start, reverse));
                }
            }
            let Some((_, k, start, reverse)) = best else {
                // Containment cycles cannot happen with strict area ordering; take what remains.
                let k = (0..done.len()).find(|&k| !done[k]).expect("items remain");
                waiting[k] = 0;
                continue;
            };
            let mut item = items[k].take().expect("not yet taken");
            if item.closed {
                // Rotate the loop to begin at the chosen vertex.
                let mut ring: Vec<Point> = item.points[..item.points.len() - 1].to_vec();
                ring.rotate_left(start);
                ring.push(ring[0]);
                item.points = ring;
            } else if reverse {
                item.points.reverse();
            }
            at = *item.points.last().expect("at least two points");
            done[k] = true;
            for &j in &inside[k] {
                waiting[j] -= 1;
            }
            out.push(item);
        }
        out
    }

    fn random_items(seed: u64, n: usize) -> Vec<Item> {
        // A small, fixed generator: the same items on every run.
        let mut state = seed;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % 10_000) as f64 / 10_000.0
        };
        (0..n)
            .map(|i| {
                let (cx, cy) = (next() * 280.0, next() * 280.0);
                // Rings of all sizes, so many sit inside others; every fifth is an open line.
                let r = 1.0 + next() * if i % 7 == 0 { 120.0 } else { 15.0 };
                let sides = 3 + (next() * 9.0) as usize;
                let mut points: Vec<Point> = (0..sides)
                    .map(|s| {
                        let a = std::f64::consts::TAU * s as f64 / sides as f64 + next();
                        Point::new(cx + r * a.cos(), cy + r * a.sin())
                    })
                    .collect();
                let closed = i % 5 != 0;
                if closed {
                    points.push(points[0]);
                }
                Item {
                    kind: OperationKind::Line,
                    id: Some(format!("item{i}")),
                    colour: None,
                    points,
                    closed,
                }
            })
            .collect()
    }

    #[test]
    fn the_faster_ordering_is_exactly_the_original() {
        for seed in 1..=40u64 {
            for roll in [false, true] {
                let items = random_items(seed * 7919, 30 + (seed as usize % 7) * 40);
                let fast = order(items.clone(), roll);
                let slow = order_reference(items, roll);
                let shape = |v: &[Item]| -> Vec<(Option<String>, Vec<Point>)> {
                    v.iter().map(|i| (i.id.clone(), i.points.clone())).collect()
                };
                assert_eq!(shape(&fast), shape(&slow), "seed {seed}, roll {roll}");
            }
        }
    }

    use super::*;
    use cutcraft_job::{Colour, CutLine, MachineProfile, Piece, Recipe};

    fn square(x: f64, y: f64, size: f64) -> Contour {
        let p = |x: f64, y: f64| [x, y];
        let line = |a: [f64; 2], b: [f64; 2]| [a, b, b];
        Contour {
            start: p(x, y),
            segments: vec![
                line(p(x, y), p(x + size, y)),
                line(p(x + size, y), p(x + size, y + size)),
                line(p(x + size, y + size), p(x, y + size)),
                line(p(x, y + size), p(x, y)),
            ],
            closed: true,
        }
    }

    fn colour(hex: &str) -> Colour {
        Colour {
            hex: hex.into(),
            swatch: None,
        }
    }

    fn piece(id: &str, hex: &str, outer: Contour, holes: Vec<Contour>) -> Piece {
        Piece {
            id: id.into(),
            layer: "source/0".into(),
            colour: colour(hex),
            source_ids: vec![],
            outer,
            holes,
        }
    }

    fn design(pieces: Vec<Piece>, cut_lines: Vec<CutLine>) -> cutcraft_job::PreparedDesign {
        cutcraft_job::PreparedDesign {
            schema_version: 1,
            stage: "preparation".into(),
            cut_ready: true,
            engine_revision: "test".into(),
            source_sha256: "0".repeat(64),
            recipe: Recipe::new(),
            fonts: vec![],
            page: None,
            pieces,
            cut_lines,
            changes: vec![],
            pending: vec![],
            diagnostics: vec![],
            stroked_fills: 0,
        }
    }

    /// A ring (40 mm square with a 20 mm hole), an island in its hole, and a cut line on the ring.
    fn ring_design() -> cutcraft_job::PreparedDesign {
        let line = CutLine {
            id: "line/0".into(),
            layer: "source/0".into(),
            colour: colour("#000000"),
            source_ids: vec![],
            contour: Contour {
                start: [2.0, 20.0],
                segments: vec![[[2.0, 20.0], [8.0, 20.0], [8.0, 20.0]]],
                closed: false,
            },
        };
        design(
            vec![
                piece(
                    "piece/0",
                    "#000000",
                    square(0.0, 0.0, 40.0),
                    vec![square(10.0, 10.0, 20.0)],
                ),
                piece("piece/1", "#000000", square(15.0, 15.0, 10.0), vec![]),
            ],
            vec![line],
        )
    }

    fn one_sheet(design: cutcraft_job::PreparedDesign) -> Project {
        let mut project = Project::new(design, MachineProfile::cameo_1());
        // Put every row on the first sheet.
        let rows: Vec<_> = project.sheets.iter().flat_map(|s| s.rows.clone()).collect();
        project.sheets.truncate(1);
        project.sheets[0].rows = rows;
        project
    }

    /// Two letters-like pieces with a gap, and a cut line: room for weed lines between them.
    fn spaced_design() -> cutcraft_job::PreparedDesign {
        design(
            vec![
                piece(
                    "piece/0",
                    "#ff0000",
                    square(0.0, 0.0, 30.0),
                    vec![square(10.0, 10.0, 10.0)],
                ),
                piece("piece/1", "#0000ff", square(60.0, 20.0, 40.0), vec![]),
                piece("piece/2", "#ff0000", square(0.0, 70.0, 100.0), vec![]),
            ],
            vec![],
        )
    }

    /// Distance from `p` to the segment `a`–`b`.
    fn to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = dx * dx + dy * dy;
        let t = if len == 0.0 {
            0.0
        } else {
            (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len).clamp(0.0, 1.0)
        };
        (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy)
    }

    fn inside(p: [f64; 2], ring: &[[f64; 2]]) -> bool {
        let mut inside = false;
        for w in ring.windows(2) {
            let (a, b) = (w[0], w[1]);
            if (a[1] > p[1]) != (b[1] > p[1])
                && p[0] < a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
            {
                inside = !inside;
            }
        }
        inside
    }

    #[test]
    fn inverted_weed_lines_split_the_shapes_and_keep_the_background() {
        // A stencil: the shapes are peeled out, the vinyl around them and in their holes is kept.
        let mut project = one_sheet(spaced_design());
        project.sheets[0].weed_lines = true;
        project.sheets[0].weed_inverted = true;
        let plan = plan_sheet(&project, "sheet/1").unwrap();
        assert!(plan.valid, "{:?}", plan.diagnostics);
        let outlines: Vec<&Operation> = plan
            .operations
            .iter()
            .filter(|o| o.kind == OperationKind::Outline)
            .collect();
        let holes: Vec<&Operation> = plan
            .operations
            .iter()
            .filter(|o| o.kind == OperationKind::Hole)
            .collect();
        let lines: Vec<&Operation> = plan
            .operations
            .iter()
            .filter(|o| o.kind == OperationKind::WeedLine)
            .collect();
        assert!(lines.len() >= 4, "{} weed lines", lines.len());
        for line in &lines {
            let (a, b) = (line.points[0], line.points[1]);
            for i in 0..=200 {
                let t = f64::from(i) / 200.0;
                let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                assert!(
                    outlines.iter().any(|o| inside(p, &o.points)),
                    "{p:?} is in the kept background"
                );
                assert!(
                    holes.iter().all(|h| !inside(p, &h.points)),
                    "{p:?} is in a kept hole"
                );
                for op in outlines.iter().chain(&holes) {
                    for w in op.points.windows(2) {
                        let d = to_segment(p, w[0], w[1]);
                        assert!(
                            d >= WEED_LINE_CLEARANCE_MM - 1e-3,
                            "{p:?} is {d:.3} mm from an edge"
                        );
                    }
                }
            }
        }
        // Inverting changes only the weed lines.
        project.sheets[0].weed_inverted = false;
        let normal = plan_sheet(&project, "sheet/1").unwrap();
        let others = |p: &CutPlan| -> Vec<Operation> {
            p.operations
                .iter()
                .filter(|o| o.kind != OperationKind::WeedLine)
                .cloned()
                .collect()
        };
        assert_eq!(others(&normal), others(&plan));
        assert_ne!(normal.operations, plan.operations);
    }

    #[test]
    fn weed_lines_split_the_waste_and_never_touch_the_artwork() {
        let mut project = one_sheet(spaced_design());
        project.sheets[0].weed_lines = true;
        let plan = plan_sheet(&project, "sheet/1").unwrap();
        assert!(plan.valid, "{:?}", plan.diagnostics);
        let art: Vec<&Operation> = plan
            .operations
            .iter()
            .filter(|o| matches!(o.kind, OperationKind::Outline | OperationKind::Hole))
            .collect();
        let lines: Vec<&Operation> = plan
            .operations
            .iter()
            .filter(|o| o.kind == OperationKind::WeedLine)
            .collect();
        assert!(lines.len() >= 4, "{} weed lines", lines.len());
        // After the artwork, before the weeding box, cut once.
        let first_line = plan
            .operations
            .iter()
            .position(|o| o.kind == OperationKind::WeedLine);
        let last_art = plan
            .operations
            .iter()
            .rposition(|o| o.kind == OperationKind::Outline);
        assert!(first_line > last_art);
        assert_eq!(
            plan.operations.last().unwrap().kind,
            OperationKind::WeedingBox
        );
        let [bx0, by0, bx1, by1] = plan.bounds_mm.unwrap();
        for line in &lines {
            assert_eq!(line.passes, 1);
            let (a, b) = (line.points[0], line.points[1]);
            assert!(a[0] == b[0] || a[1] == b[1], "straight across or down");
            for i in 0..=200 {
                let t = f64::from(i) / 200.0;
                let p = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                assert!(
                    p[0] >= bx0 - 1e-6
                        && p[0] <= bx1 + 1e-6
                        && p[1] >= by0 - 1e-6
                        && p[1] <= by1 + 1e-6
                );
                for op in &art {
                    if op.kind == OperationKind::Outline {
                        assert!(!inside(p, &op.points), "{p:?} inside a piece");
                    }
                    for w in op.points.windows(2) {
                        let d = to_segment(p, w[0], w[1]);
                        assert!(
                            d >= WEED_LINE_CLEARANCE_MM - 1e-3,
                            "{p:?} is {d:.3} mm from art"
                        );
                    }
                }
            }
        }
        // Toggling them changes only the aids: the artwork cuts are the same.
        project.sheets[0].weed_lines = false;
        let plain = plan_sheet(&project, "sheet/1").unwrap();
        let cut = |p: &CutPlan| -> Vec<Operation> {
            p.operations
                .iter()
                .filter(|o| !o.kind.is_aid())
                .cloned()
                .collect()
        };
        assert_eq!(cut(&plain), cut(&plan));
    }

    #[test]
    fn layering_marks_land_in_the_same_place_on_every_cut_with_them() {
        let mut project = Project::new(spaced_design(), MachineProfile::cameo_1());
        assert_eq!(project.sheets[0].rows.len(), 2, "red and blue");
        project.sheets[0].layering_marks = true;
        // Red is cut first, then blue on another piece of vinyl.
        let only = |project: &Project, hex: &str| {
            let mut one = project.clone();
            one.excluded_rows = one.sheets[0]
                .rows
                .iter()
                .filter(|r| !r.label().starts_with(hex))
                .cloned()
                .collect();
            one
        };
        let marks = |project: &Project, hex: &str| -> Vec<Vec<[f64; 2]>> {
            let plan = plan_sheet(&only(project, hex), "sheet/1").unwrap();
            assert!(plan.valid, "{:?}", plan.diagnostics);
            plan.operations
                .iter()
                .filter(|o| o.kind == OperationKind::Mark)
                .map(|o| o.points.clone())
                .collect()
        };
        let (red, blue) = (marks(&project, "#ff0000"), marks(&project, "#0000ff"));
        assert_eq!(red.len(), 4, "left, right, above and below");
        assert_eq!(red, blue);
        // Closed triangles whose tips lie on the design's centre lines, pointing inward.
        let tip = |m: &Vec<[f64; 2]>| m[0];
        assert!(red.iter().all(|m| m.len() == 4 && m[0] == m[3]));
        assert!(
            (tip(&red[0])[1] - tip(&red[1])[1]).abs() < 0.01,
            "left and right"
        );
        assert!(
            (tip(&red[2])[0] - tip(&red[3])[0]).abs() < 0.01,
            "above and below"
        );
        assert!(red[0][1][0] < tip(&red[0])[0], "the left one points right");
        assert!(red[2][1][1] < tip(&red[2])[1], "the top one points down");
        // Inside the weeding box, so they stay on the carrier; weed lines keep clear of them.
        let mut lined = only(&project, "#0000ff");
        lined.sheets[0].weed_lines = true;
        let plan = plan_sheet(&lined, "sheet/1").unwrap();
        let weeding = plan
            .operations
            .iter()
            .find(|o| o.kind == OperationKind::WeedingBox)
            .unwrap();
        let [b0, _, b2, ..] = [
            weeding.points[0],
            weeding.points[1],
            weeding.points[2],
            weeding.points[3],
        ];
        let inside = |p: [f64; 2]| p[0] > b0[0] && p[0] < b2[0] && p[1] > b0[1] && p[1] < b2[1];
        let placed: Vec<_> = plan
            .operations
            .iter()
            .filter(|o| o.kind == OperationKind::Mark)
            .collect();
        assert!(placed.iter().all(|m| m.points.iter().all(|p| inside(*p))));
        for line in plan
            .operations
            .iter()
            .filter(|o| o.kind == OperationKind::WeedLine)
        {
            for mark in &placed {
                let (lo, hi) =
                    mark.points
                        .iter()
                        .fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| {
                            (
                                [lo[0].min(p[0]), lo[1].min(p[1])],
                                [hi[0].max(p[0]), hi[1].max(p[1])],
                            )
                        });
                for p in &line.points {
                    assert!(
                        !(p[0] > lo[0] && p[0] < hi[0] && p[1] > lo[1] && p[1] < hi[1]),
                        "a weed line ends inside a mark"
                    );
                }
            }
        }
        // Mirrored together, they still match each other.
        project.sheets[0].mirror = true;
        assert_eq!(marks(&project, "#ff0000"), marks(&project, "#0000ff"));
    }

    #[test]
    fn a_roll_takes_long_designs_and_is_cut_top_to_bottom() {
        // Squares scattered down a long strip, listed out of order.
        let pieces: Vec<Piece> = [3usize, 0, 4, 1, 5, 2]
            .iter()
            .map(|&i| {
                let x = if i % 2 == 0 { 0.0 } else { 200.0 };
                piece(
                    &format!("piece/{i}"),
                    "#000000",
                    square(x, i as f64 * 150.0, 20.0),
                    vec![],
                )
            })
            .collect();
        let mut project = one_sheet(design(pieces, vec![]));
        project.sheets[0].mat = Mat::Mat12x24;
        assert!(
            !plan_sheet(&project, "sheet/1").unwrap().valid,
            "too long for a mat"
        );
        project.sheets[0].mat = Mat::Roll;
        let plan = plan_sheet(&project, "sheet/1").unwrap();
        assert!(plan.valid, "{:?}", plan.diagnostics);
        assert!(
            plan.mat_mm[1] >= plan.bounds_mm.unwrap()[3],
            "the roll is drawn as long as the cut"
        );
        assert_eq!(plan.mat_mm[1] % 25.4, 0.0);
        let tops: Vec<f64> = plan
            .operations
            .iter()
            .filter(|o| o.kind == OperationKind::Outline)
            .map(|o| o.points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min))
            .collect();
        assert!(
            tops.windows(2).all(|w| w[1] >= w[0]),
            "no feeding back: {tops:?}"
        );
    }

    #[test]
    fn overcut_and_software_compensation_shape_the_tool_path_only() {
        let mut project = one_sheet(design(
            vec![piece("piece/0", "#000000", square(0.0, 0.0, 20.0), vec![])],
            vec![],
        ));
        project.sheets[0].weeding_box = false;
        let plain = plan_sheet(&project, "sheet/1").unwrap();
        assert!(plain.operations.iter().all(|o| o.tool_path.is_none()));
        assert_eq!(plain.compensation, Compensation::PendingCalibration);

        // Overcut: the contour is cut on past its start, along its first edge.
        project.sheets[0].material.overcut_mm = 0.5;
        let over = plan_sheet(&project, "sheet/1").unwrap();
        let op = &over.operations[0];
        assert_eq!(
            op.points, plain.operations[0].points,
            "the preview is unchanged"
        );
        let path = op.tool_path.as_ref().unwrap();
        assert_eq!(&path[..op.points.len()], op.points.as_slice());
        let extra: f64 = path[op.points.len() - 1..]
            .windows(2)
            .map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]))
            .sum();
        assert!((extra - 0.5).abs() < 1e-3, "{extra}");
        assert_ne!(
            over.plan_id, plain.plan_id,
            "the tool path is part of the cut"
        );

        // Bounded on tiny shapes: at most a tenth of the perimeter.
        let tiny = extend_closed(
            &[
                Point::new(0.0, 0.0),
                Point::new(1.0, 0.0),
                Point::new(1.0, 1.0),
                Point::new(0.0, 1.0),
                Point::new(0.0, 0.0),
            ],
            0.5,
        );
        assert_eq!(*tiny.last().unwrap(), Point::new(0.4, 0.0));

        // Software compensation: the holder runs the offset past each corner and swings round it.
        project.sheets[0].material.overcut_mm = 0.0;
        project.sheets[0].material.offset_method = cutcraft_job::OffsetMethod::Software;
        project.sheets[0].material.offset_mm = 0.25;
        let soft = plan_sheet(&project, "sheet/1").unwrap();
        assert_eq!(
            soft.compensation,
            Compensation::Software { offset_mm: 0.25 }
        );
        let op = &soft.operations[0];
        let path = op.tool_path.as_ref().unwrap();
        let ring = &op.points[..op.points.len() - 1];
        for p in path {
            // On an edge's line, or on an arc of the offset round a corner.
            let near_corner = ring
                .iter()
                .any(|c| ((p[0] - c[0]).hypot(p[1] - c[1]) - 0.25).abs() < 1e-3);
            let on_edge = op.points.windows(2).any(|w| {
                let (a, b) = (w[0], w[1]);
                let cross = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
                cross.abs() / (b[0] - a[0]).hypot(b[1] - a[1]) < 1e-3
            });
            assert!(near_corner || on_edge, "{p:?} is off the compensated path");
        }
        let [x0, y0, x1, y1] = soft.bounds_mm.unwrap();
        let [px0, py0, px1, py1] = plain.bounds_mm.unwrap();
        assert!(
            x1 > px1 && y1 > py1 && x0 >= px0 - 0.26 && y0 >= py0 - 0.26,
            "bounds include the swing"
        );

        // Machine compensation: the path is the cutter's business, the clearance is in the bounds.
        project.sheets[0].material.offset_method = cutcraft_job::OffsetMethod::Machine;
        let machine = plan_sheet(&project, "sheet/1").unwrap();
        assert!(machine.operations[0].tool_path.is_none());
        let [mx0, ..] = machine.bounds_mm.unwrap();
        assert!((px0 - mx0 - 0.25).abs() < 1e-6);
    }

    #[test]
    fn a_test_cut_may_sit_in_a_gap_but_never_on_the_artwork() {
        let mut project = one_sheet(spaced_design());
        project.sheets[0].weed_lines = true;
        let plan = plan_sheet(&project, "sheet/1").unwrap();
        let place = |plan: &CutPlan, x: f64, y: f64| {
            let s = TEST_CUT_MM;
            let o = project.sheets[0].placement.offset_mm;
            plan.overlaps_art([o[0] + x, o[1] + y, o[0] + x + s, o[1] + y + s])
        };
        // spaced_design: a 30 mm square at (0, 0), a 40 mm square at (60, 20), a bar from y = 70.
        // The gap right of the first square, above the second, is clear (weed lines don't count).
        assert!(
            !place(&plan, 32.0, 32.0 - TEST_CUT_MM),
            "a gap inside the box is fine"
        );
        assert!(place(&plan, 10.0, 10.0), "on top of a piece");
        assert!(place(&plan, 70.0, 30.0), "inside a piece");
        assert!(place(&plan, 25.0, 0.0), "crossing an edge");
    }

    #[test]
    fn her_material_can_sit_anywhere_and_cuts_off_the_mat_grid_are_flagged() {
        let mut project = one_sheet(ring_design());
        let plan = plan_sheet(&project, "sheet/1").unwrap();
        let [x0, y0, x1, y1] = plan.bounds_mm.unwrap();
        // A piece starting beyond the design's left edge no longer holds it.
        project.sheets[0].piece_mm = Some([x1 - x0 + 2.0, y1 - y0 + 2.0]);
        project.sheets[0].piece_at_mm = Some([x0 - 1.0, y0 - 1.0]);
        let on = plan_sheet(&project, "sheet/1").unwrap();
        assert!(on.valid, "{:?}", on.diagnostics);
        assert_eq!(
            on.usable_area_mm(),
            [x0 - 1.0, y0 - 1.0, x1 + 1.0, y1 + 1.0]
        );
        project.sheets[0].piece_at_mm = Some([x0 + 1.0, y0 - 1.0]);
        let off = plan_sheet(&project, "sheet/1").unwrap();
        assert!(
            off.diagnostics
                .iter()
                .any(|d| d.code == "plan.outside_piece")
        );
        // The test cut is looked for inside the moved piece.
        project.sheets[0].piece_mm = Some([200.0, 100.0]);
        project.sheets[0].piece_at_mm = Some([100.0, 150.0]);
        let roomy = plan_sheet(&project, "sheet/1").unwrap();
        let [tx, ty] = test_cut_spot(roomy.usable_area_mm(), roomy.bounds_mm.as_slice()).unwrap();
        assert!(tx >= 100.0 && ty >= 150.0);
        // A grid whose corner sits right of the cutter's corner leaves the design off it.
        project.sheets[0].piece_mm = None;
        let mut mat = cutcraft_job::MatProfile::for_class(Mat::Mat12x12);
        mat.cut_origin_mm = [0.0, 0.0];
        mat.sticky_mm[0] = 200.0;
        project.sheets[0].mat_profile = Some(mat);
        let warned = plan_sheet(&project, "sheet/1").unwrap();
        assert!(warned.valid, "a warning, not a block");
        assert!(
            warned
                .diagnostics
                .iter()
                .any(|d| d.code == "plan.off_mat_grid")
        );
    }

    #[test]
    fn inner_cuts_come_before_what_releases_them_and_the_weeding_box_is_last() {
        let plan = plan_sheet(&one_sheet(ring_design()), "sheet/1").unwrap();
        assert!(plan.valid, "{:?}", plan.diagnostics);
        let order: Vec<(OperationKind, Option<&str>)> = plan
            .operations
            .iter()
            .map(|o| (o.kind, o.item.as_deref()))
            .collect();
        let at = |kind, item: Option<&str>| order.iter().position(|o| *o == (kind, item)).unwrap();
        let island = at(OperationKind::Outline, Some("piece/1"));
        let hole = at(OperationKind::Hole, Some("piece/0"));
        let line = at(OperationKind::Line, Some("line/0"));
        let ring = at(OperationKind::Outline, Some("piece/0"));
        assert!(island < hole && hole < ring && line < ring, "{order:?}");
        let weeding = plan.operations.last().unwrap();
        assert_eq!(weeding.kind, OperationKind::WeedingBox);
        let b = plan.bounds_mm.unwrap();
        // The design starts at the default offset; the box adds the margin around it.
        let start = cutcraft_job::project::WEEDING_MARGIN_MM
            + cutcraft_job::project::PLACEMENT_CLEARANCE_MM;
        assert!(
            (b[0] - (start - 6.35)).abs() < 1e-6 && (b[2] - (start + 40.0 + 6.35)).abs() < 1e-6
        );
        assert_eq!(plan.design_size_mm, Some([40.0, 40.0]));
        assert!(plan.cut_length_mm > 4.0 * (40.0 + 20.0 + 10.0));
        assert!(plan.estimate_s > 0.0);
    }

    #[test]
    fn mirroring_flips_the_design_and_iron_on_mirrors_automatically() {
        let design = design(
            vec![
                piece("piece/0", "#000000", square(0.0, 0.0, 10.0), vec![]),
                piece("piece/1", "#000000", square(30.0, 0.0, 10.0), vec![]),
            ],
            vec![],
        );
        let mut project = one_sheet(design);
        project.sheets[0].weeding_box = false;
        let plain = plan_sheet(&project, "sheet/1").unwrap();
        let htv = cutcraft_job::Material::defaults()
            .into_iter()
            .find(|m| m.mirror)
            .unwrap();
        project.set_material("sheet/1", htv).unwrap();
        assert!(project.sheets[0].mirror);
        let mirrored = plan_sheet(&project, "sheet/1").unwrap();
        let first_x = |plan: &CutPlan, item: &str| {
            plan.operations
                .iter()
                .find(|o| o.item.as_deref() == Some(item))
                .unwrap()
                .points
                .iter()
                .map(|p| p[0])
                .fold(f64::MAX, f64::min)
        };
        // piece/0 was on the left; mirrored, it is on the right, and the design stays in place.
        assert!(first_x(&plain, "piece/0") < first_x(&plain, "piece/1"));
        assert!(first_x(&mirrored, "piece/0") > first_x(&mirrored, "piece/1"));
        assert_eq!(plain.bounds_mm, mirrored.bounds_mm);
        assert_ne!(plain.plan_id, mirrored.plan_id);
        assert!(preview_svg(&mirrored).contains("MIRRORED"));
    }

    #[test]
    fn plans_that_leave_the_cutting_area_are_blocked_never_clamped() {
        let mut project = one_sheet(ring_design());
        project.sheets[0].placement.offset_mm = [280.0, 10.0];
        let plan = plan_sheet(&project, "sheet/1").unwrap();
        assert!(!plan.valid);
        assert!(
            plan.diagnostics
                .iter()
                .any(|d| d.code == "plan.out_of_bounds")
        );
        assert!(
            plan.bounds_mm.unwrap()[2] > 304.0,
            "the plan is reported as placed, not moved"
        );
        project.sheets[0].placement.offset_mm = [10.0, 280.0];
        project.sheets[0].mat = Mat::Mat12x24;
        assert!(
            plan_sheet(&project, "sheet/1").unwrap().valid,
            "a longer mat fits it"
        );
    }

    #[test]
    fn a_cut_off_her_piece_blocks_the_sheet_and_the_test_cut_stays_on_it() {
        let mut project = one_sheet(ring_design());
        let plan = plan_sheet(&project, "sheet/1").unwrap();
        let [_, _, x1, y1] = plan.bounds_mm.unwrap();
        project
            .set_piece("sheet/1", Some([x1 + 1.0, y1 + 1.0]))
            .unwrap();
        let fits = plan_sheet(&project, "sheet/1").unwrap();
        assert!(fits.valid, "{:?}", fits.diagnostics);
        assert_eq!(fits.piece_mm, Some([x1 + 1.0, y1 + 1.0]));
        assert_eq!(
            fits.operations, plan.operations,
            "the piece does not change the geometry"
        );
        assert_ne!(
            fits.plan_id, plan.plan_id,
            "material bounds change the reviewed setup"
        );
        project
            .set_piece("sheet/1", Some([x1 - 1.0, y1 + 1.0]))
            .unwrap();
        let off = plan_sheet(&project, "sheet/1").unwrap();
        assert!(!off.valid);
        assert!(
            off.diagnostics
                .iter()
                .any(|d| d.code == "plan.outside_piece")
        );
        // No room beside the artwork on that piece: they need a scrap for the test cut.
        assert_eq!(
            test_cut_spot(off.usable_area_mm(), off.bounds_mm.as_slice()),
            None
        );
        project
            .set_piece("sheet/1", Some([x1 + 40.0, y1 + 1.0]))
            .unwrap();
        let roomy = plan_sheet(&project, "sheet/1").unwrap();
        let [tx, ty] = test_cut_spot(roomy.usable_area_mm(), roomy.bounds_mm.as_slice()).unwrap();
        assert!(tx + TEST_CUT_MM <= x1 + 40.0 && ty + TEST_CUT_MM <= y1 + 1.0);
        assert!(project.set_piece("sheet/1", Some([400.0, 100.0])).is_err());
        assert!(project.set_piece("sheet/1", Some([0.0, 100.0])).is_err());
        project.set_piece("sheet/1", None).unwrap();
        assert_eq!(plan_sheet(&project, "sheet/1").unwrap().piece_mm, None);
    }

    #[test]
    fn curves_are_flattened_within_tolerance_and_plans_are_deterministic() {
        // A quarter-circle-like cubic from (0,0) to (50,50).
        let curve = Contour {
            start: [0.0, 0.0],
            segments: vec![[[27.6, 0.0], [50.0, 22.4], [50.0, 50.0]]],
            closed: false,
        };
        let points = flatten(&curve, 1.0);
        let cubic = kurbo::CubicBez::new((0.0, 0.0), (27.6, 0.0), (50.0, 22.4), (50.0, 50.0));
        for w in points.windows(2) {
            let mid = w[0].midpoint(w[1]);
            let nearest = kurbo::ParamCurveNearest::nearest(&cubic, mid, 1e-6)
                .distance_sq
                .sqrt();
            assert!(nearest <= FLATTEN_TOLERANCE_MM + 1e-9, "{nearest}");
        }
        let project = one_sheet(ring_design());
        let a = plan_sheet(&project, "sheet/1").unwrap();
        let b = plan_sheet(&project, "sheet/1").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn empty_and_unprepared_sheets_are_blocked() {
        let mut project = one_sheet(ring_design());
        project.excluded_rows = project.sheets[0].rows.clone();
        let empty = plan_sheet(&project, "sheet/1").unwrap();
        assert!(empty.diagnostics.iter().any(|d| d.code == "plan.empty") && !empty.valid);
        let mut unready = ring_design();
        unready.cut_ready = false;
        let plan = plan_sheet(&one_sheet(unready), "sheet/1").unwrap();
        assert!(
            plan.diagnostics
                .iter()
                .any(|d| d.code == "plan.not_prepared")
        );
        assert!(plan_sheet(&project, "sheet/9").is_err());
    }

    #[test]
    fn a_new_project_cuts_every_row_on_one_sheet() {
        let mut d = ring_design();
        d.pieces
            .push(piece("piece/2", "#ff0000", square(50.0, 0.0, 10.0), vec![]));
        let project = Project::new(d, MachineProfile::cameo_1());
        assert_eq!(project.sheets.len(), 1);
        let rows: Vec<_> = project.sheets[0].rows.iter().map(|r| r.label()).collect();
        assert_eq!(rows, ["#000000", "#ff0000", "#000000 cut lines"]);
        assert!(project.excluded_rows.is_empty());
        assert!(project.sheets[0].weeding_box && !project.sheets[0].mirror);
        let svg = preview_svg(&plan_sheet(&project, "sheet/1").unwrap());
        assert!(svg.contains("#ff0000") && svg.contains("#000000") && svg.contains("<line"));
    }

    #[test]
    fn specks_below_the_cutter_step_are_left_out_and_listed() {
        let mut d = ring_design();
        d.pieces
            .push(piece("piece/2", "#000000", square(5.0, 5.0, 0.02), vec![]));
        let project = one_sheet(d);
        let plan = plan_sheet(&project, "sheet/1").unwrap();
        assert!(plan.valid, "{:?}", plan.diagnostics);
        assert!(
            plan.operations
                .iter()
                .all(|o| o.item.as_deref() != Some("piece/2"))
        );
        let note = plan
            .diagnostics
            .iter()
            .find(|d| d.code == "plan.specks_skipped")
            .unwrap();
        assert_eq!(note.source_ids, ["piece/2"]);
        assert!(!collapses(
            &[Point::new(0.0, 0.0), Point::new(0.06, 0.0)],
            false,
            0.05
        ));
        assert!(collapses(
            &[Point::new(0.0, 0.0), Point::new(0.02, 0.0)],
            false,
            0.05
        ));
    }

    #[test]
    fn the_png_preview_draws_the_plan_on_the_mat() {
        let mut d = ring_design();
        d.pieces
            .push(piece("piece/2", "#ff0000", square(50.0, 0.0, 10.0), vec![]));
        let mut project = Project::new(d, MachineProfile::cameo_1());
        // Only the red square is cut.
        project.excluded_rows = project.sheets[0]
            .rows
            .iter()
            .filter(|r| !r.label().starts_with("#ff0000"))
            .cloned()
            .collect();
        let plan = plan_sheet(&project, "sheet/1").unwrap();
        let options = RasterOptions {
            max_side: 400,
            ..Default::default()
        };
        let preview = preview_png(&plan, &options).unwrap();
        assert_eq!(preview.width.max(preview.height), 400);
        let image = image::load_from_memory(&preview.png).unwrap().to_rgba8();
        assert_eq!(
            (image.width(), image.height()),
            (preview.width, preview.height)
        );
        // The red square's centre, placed on the mat.
        let b = plan.bounds_mm.unwrap();
        let m = cutcraft_job::project::WEEDING_MARGIN_MM;
        let centre = [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0];
        assert!(b[2] - b[0] > 2.0 * m);
        let at = |mm: [f64; 2]| {
            let p = [
                preview.origin_px[0] + mm[0] * preview.px_per_mm,
                preview.origin_px[1] + mm[1] * preview.px_per_mm,
            ];
            image.get_pixel(p[0] as u32, p[1] as u32).0
        };
        assert_eq!(at(centre), [255, 0, 0, 255]);
        // Far corner of the mat: the mat colour, not the piece.
        assert_eq!(at([300.0, 300.0]), [244, 244, 240, 255]);
        let ordered = preview_png(
            &plan,
            &RasterOptions {
                max_side: 200,
                order: Some(0.0),
                view: RasterView::ToolMotion,
                highlight: vec!["piece/2".into()],
                test_cut: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(ordered.cuts_shown, 0);
        assert_eq!(ordered.highlighted, ["piece/2"]);
    }

    #[test]
    fn sizing_scales_every_cut_and_the_test_cut_keeps_clear() {
        let mut project = one_sheet(design(
            vec![piece("piece/0", "#ff0000", square(0.0, 0.0, 20.0), vec![])],
            vec![],
        ));
        let sheet = project.sheets[0].id.clone();
        let before = plan_sheet(&project, &sheet).unwrap();
        project.set_scale(2.5).unwrap();
        let after = plan_sheet(&project, &sheet).unwrap();
        assert_eq!(before.design_size_mm, Some([20.0, 20.0]));
        assert_eq!(after.design_size_mm, Some([50.0, 50.0]));
        assert_ne!(before.plan_id, after.plan_id);

        let area = after.usable_area_mm();
        let occupied = after.bounds_mm.unwrap();
        let spot = test_cut_spot(area, &[occupied]).unwrap();
        let test = test_cut_plan(&project, &sheet, spot).unwrap();
        assert!(test.valid);
        let [x0, y0, x1, y1] = test.bounds_mm.unwrap();
        let apart =
            x0 >= occupied[2] || x1 <= occupied[0] || y0 >= occupied[3] || y1 <= occupied[1];
        assert!(apart, "{:?} overlaps {occupied:?}", test.bounds_mm);
        assert_eq!(
            test.operations[0].kind,
            OperationKind::Hole,
            "triangle first"
        );
        let triangle = 15.4 + 2.0 * 7.7_f64.hypot(15.4);
        assert!((test.cut_length_mm - (4.0 * TEST_CUT_MM + triangle)).abs() < 0.01);
        assert_eq!(
            test_cut_spot([0.0, 0.0, 30.0, 30.0], &[]),
            None,
            "too small for a test cut"
        );
    }
}
