//! A PNG Cut Preview of one plan (spec §6.2.4 `render_preview`), drawn from the same plan as the
//! SVG preview: the mat with an inch grid, the cutting area, pieces in their colours, cut lines,
//! the weeding box, and optionally the cut order so far, travel moves or highlighted items.

use serde::Serialize;
use vello_cpu::kurbo::{Affine, BezPath, Rect, Shape, Stroke};
use vello_cpu::peniko::{Color, Fill};
use vello_cpu::{Pixmap, RenderContext, Resources};

use cutcraft_job::Project;

use crate::{CutPlan, OperationKind};

const INCH: f64 = 25.4;
/// Margin around the mat, in mm.
const MARGIN_MM: f64 = 6.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RasterView {
    /// The sheet as it will be cut.
    #[default]
    Sheet,
    /// The sheet with the blade's tool-up travel between cuts drawn over it.
    ToolMotion,
}

#[derive(Clone, Debug, Default)]
pub struct RasterOptions {
    pub view: RasterView,
    /// Longest side in pixels, 64 to 2048.
    pub max_side: u32,
    /// Show the cut order up to this fraction (0 to 1): cuts so far solid, the rest faint.
    pub order: Option<f64>,
    /// Item ids (pieces or cut lines) to draw highlighted.
    pub highlight: Vec<String>,
    /// Test cuts, `[x0, y0, x1, y1]`: areas finished ones used (solid, `true`) and where the
    /// next goes (dashed, `false`).
    pub test_cut: Vec<([f64; 4], bool)>,
}

/// The legend that goes with the picture.
#[derive(Clone, Debug, Serialize)]
pub struct RasterPreview {
    #[serde(skip)]
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub px_per_mm: f64,
    /// Where mat (0, 0) is, in pixels.
    pub origin_px: [f64; 2],
    pub plan_id: String,
    pub mirrored: bool,
    /// Operations drawn as already cut (all of them without `order`).
    pub cuts_shown: usize,
    pub cuts_total: usize,
    pub highlighted: Vec<String>,
    pub legend: Vec<(&'static str, &'static str)>,
}

fn colour(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    let byte = |i: usize| {
        hex.get(i..i + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
            .unwrap_or(0)
    };
    Color::from_rgba8(byte(0), byte(2), byte(4), 255)
}

fn polyline(points: &[[f64; 2]], closed: bool) -> BezPath {
    let mut path = BezPath::new();
    for (i, p) in points.iter().enumerate() {
        if i == 0 {
            path.move_to((p[0], p[1]));
        } else {
            path.line_to((p[0], p[1]));
        }
    }
    if closed && points.len() > 2 {
        path.close_path();
    }
    path
}

pub fn preview_png(plan: &CutPlan, options: &RasterOptions) -> Result<RasterPreview, String> {
    let max_side = options.max_side.clamp(64, 2048);
    let [mat_w, mat_h] = plan.mat_mm;
    let span = [mat_w + 2.0 * MARGIN_MM, mat_h + 2.0 * MARGIN_MM];
    let scale = f64::from(max_side) / span[0].max(span[1]);
    let width = ((span[0] * scale).round() as u16).max(1);
    let height = ((span[1] * scale).round() as u16).max(1);
    let view = Affine::scale(scale) * Affine::translate((MARGIN_MM, MARGIN_MM));
    let px = 1.0 / scale;

    let mut ctx = RenderContext::new(width, height);
    ctx.set_paint(Color::from_rgba8(255, 255, 255, 255));
    ctx.fill_rect(&Rect::new(0.0, 0.0, f64::from(width), f64::from(height)));
    ctx.set_transform(view);
    // The mat, its inch grid and the cutting area.
    ctx.set_paint(Color::from_rgba8(244, 244, 240, 255));
    ctx.fill_rect(&Rect::new(0.0, 0.0, mat_w, mat_h));
    ctx.set_stroke(Stroke::new(px));
    ctx.set_paint(Color::from_rgba8(221, 221, 221, 255));
    let mut grid = BezPath::new();
    let mut x = INCH;
    while x < mat_w {
        grid.move_to((x, 0.0));
        grid.line_to((x, mat_h));
        x += INCH;
    }
    let mut y = INCH;
    while y < mat_h {
        grid.move_to((0.0, y));
        grid.line_to((mat_w, y));
        y += INCH;
    }
    ctx.stroke_path(&grid);
    ctx.set_paint(Color::from_rgba8(153, 153, 153, 255));
    ctx.stroke_path(&Rect::new(0.0, 0.0, mat_w, mat_h).to_path(0.1));
    let [area_w, area_h] = plan.cutting_area_mm;
    ctx.set_stroke(Stroke::new(1.5 * px).with_dashes(0.0, [4.0 * px, 3.0 * px]));
    ctx.set_paint(Color::from_rgba8(119, 170, 153, 255));
    ctx.stroke_path(&Rect::new(0.0, 0.0, area_w, area_h).to_path(0.1));
    // Their piece of material, at the start corner.
    if let Some([px0, py0, px1, py1]) = plan.piece_rect_mm() {
        let piece = Rect::new(px0, py0, px1, py1);
        ctx.set_paint(Color::from_rgba8(255, 236, 179, 160));
        ctx.fill_rect(&piece);
        ctx.set_stroke(Stroke::new(2.0 * px));
        ctx.set_paint(Color::from_rgba8(176, 120, 0, 255));
        ctx.stroke_path(&piece.to_path(0.1));
    }

    for &([x0, y0, x1, y1], done) in &options.test_cut {
        let square = Rect::new(x0, y0, x1, y1);
        let stroke = if done {
            Stroke::new(2.0 * px)
        } else {
            Stroke::new(1.5 * px).with_dashes(0.0, [5.0 * px, 3.0 * px])
        };
        ctx.set_stroke(stroke);
        ctx.set_paint(Color::from_rgba8(230, 120, 0, 255));
        ctx.stroke_path(&square.to_path(0.1));
        // A faint triangle inside, as the test cut is shaped.
        let mut triangle = BezPath::new();
        let inset = (x1 - x0) * 0.2;
        triangle.move_to((x0 + inset, y1 - inset));
        triangle.line_to(((x0 + x1) / 2.0, y0 + inset));
        triangle.line_to((x1 - inset, y1 - inset));
        triangle.close_path();
        ctx.set_stroke(Stroke::new(px));
        ctx.stroke_path(&triangle);
    }

    let total = plan.operations.len();
    let shown = options
        .order
        .map(|o| ((o.clamp(0.0, 1.0) * total as f64).round() as usize).min(total))
        .unwrap_or(total);
    let faint = Color::from_rgba8(190, 190, 190, 255);

    // Pieces filled with their holes, in their colours, when the whole sheet is shown.
    if options.order.is_none() {
        let mut pieces: Vec<(&str, &str, BezPath)> = Vec::new();
        for op in &plan.operations {
            if !matches!(op.kind, OperationKind::Outline | OperationKind::Hole) {
                continue;
            }
            let id = op.item.as_deref().unwrap_or("");
            let path = polyline(&op.points, true);
            match pieces.iter_mut().find(|p| p.0 == id) {
                Some(piece) => piece.2.extend(path),
                None => pieces.push((id, op.colour.as_deref().unwrap_or("#000000"), path)),
            }
        }
        ctx.set_fill_rule(Fill::EvenOdd);
        for (_, hex, path) in &pieces {
            ctx.set_paint(colour(hex));
            ctx.fill_path(path);
        }
    }
    // Blade paths.
    for (i, op) in plan.operations.iter().enumerate() {
        let path = polyline(&op.points, op.closed);
        let done = i < shown;
        let (paint, stroke) = match op.kind {
            _ if !done => (faint, Stroke::new(px)),
            OperationKind::Line => (
                colour(op.colour.as_deref().unwrap_or("#000000")),
                Stroke::new(2.0 * px).with_dashes(0.0, [6.0 * px, 3.0 * px]),
            ),
            OperationKind::WeedingBox | OperationKind::WeedLine => (
                Color::from_rgba8(136, 136, 136, 255),
                Stroke::new(1.5 * px).with_dashes(0.0, [8.0 * px, 4.0 * px]),
            ),
            OperationKind::Mark => (Color::from_rgba8(0, 120, 200, 255), Stroke::new(2.0 * px)),
            _ if options.order.is_some() => {
                (Color::from_rgba8(204, 0, 204, 255), Stroke::new(1.5 * px))
            }
            _ => (Color::from_rgba8(204, 0, 204, 255), Stroke::new(px)),
        };
        ctx.set_paint(paint);
        ctx.set_stroke(stroke);
        ctx.stroke_path(&path);
    }
    // Travel with the blade up, from the start corner through each cut, and the tool's own
    // path where overcut or compensation make it differ from the drawn cut (spec §5.4).
    if options.view == RasterView::ToolMotion {
        ctx.set_paint(Color::from_rgba8(0, 150, 140, 255));
        ctx.set_stroke(Stroke::new(px));
        for op in plan.operations.iter().take(shown) {
            if let Some(tool) = &op.tool_path {
                ctx.stroke_path(&polyline(tool, false));
            }
        }
        let mut travel = BezPath::new();
        let mut at = [0.0, 0.0];
        for op in plan.operations.iter().take(shown) {
            if let (Some(first), Some(last)) = (op.path().first(), op.path().last()) {
                travel.move_to((at[0], at[1]));
                travel.line_to((first[0], first[1]));
                at = *last;
            }
        }
        ctx.set_paint(Color::from_rgba8(30, 120, 220, 255));
        ctx.set_stroke(Stroke::new(px).with_dashes(0.0, [3.0 * px, 3.0 * px]));
        ctx.stroke_path(&travel);
    }
    let mut highlighted = Vec::new();
    for op in &plan.operations {
        if let Some(id) = &op.item
            && options.highlight.contains(id)
        {
            ctx.set_paint(Color::from_rgba8(255, 140, 0, 255));
            ctx.set_stroke(Stroke::new(3.0 * px));
            ctx.stroke_path(&polyline(&op.points, op.closed));
            if !highlighted.contains(id) {
                highlighted.push(id.clone());
            }
        }
    }

    let mut pixmap = Pixmap::new(width, height);
    let mut resources = Resources::new();
    ctx.flush();
    ctx.render(&mut pixmap, &mut resources);
    let png = pixmap.into_png().map_err(|e| e.to_string())?;
    let mut legend = vec![
        ("mat", "#f4f4f0 with a 1 in grid"),
        ("cutting area", "dashed green"),
        ("piece outlines", "magenta, filled in the piece colour"),
        ("cut lines", "dashed, in their colour"),
        ("weeding box", "dashed grey"),
    ];
    if options.order.is_some() {
        legend.push(("not yet cut", "light grey"));
    }
    if options.view == RasterView::ToolMotion {
        legend.push(("travel, blade up", "dashed blue"));
    }
    if !options.highlight.is_empty() {
        legend.push(("highlighted", "orange"));
    }
    Ok(RasterPreview {
        png,
        width: u32::from(width),
        height: u32::from(height),
        px_per_mm: scale,
        origin_px: [MARGIN_MM * scale, MARGIN_MM * scale],
        plan_id: plan.plan_id.clone(),
        mirrored: plan.mirror,
        cuts_shown: shown,
        cuts_total: total,
        highlighted,
        legend,
    })
}

/// The shared assembly view (spec §5.3): every colour of the design together at its cut size,
/// in assembly coordinates, with rows that are not cut drawn faint. No mat or placement.
pub fn assembly_png(project: &Project, max_side: u32) -> Result<RasterPreview, String> {
    let ops = crate::assembly(project);
    let mut bounds: Option<[f64; 4]> = None;
    for p in ops.iter().flat_map(|(o, _)| &o.points) {
        let b = bounds.get_or_insert([p[0], p[1], p[0], p[1]]);
        *b = [
            b[0].min(p[0]),
            b[1].min(p[1]),
            b[2].max(p[0]),
            b[3].max(p[1]),
        ];
    }
    let [x0, y0, x1, y1] = bounds.ok_or("The design has nothing to show.")?;
    let max_side = max_side.clamp(64, 2048);
    let span = [x1 - x0 + 2.0 * MARGIN_MM, y1 - y0 + 2.0 * MARGIN_MM];
    let scale = f64::from(max_side) / span[0].max(span[1]);
    let width = ((span[0] * scale).round() as u16).max(1);
    let height = ((span[1] * scale).round() as u16).max(1);
    let origin = [MARGIN_MM - x0, MARGIN_MM - y0];
    let px = 1.0 / scale;
    let mut ctx = RenderContext::new(width, height);
    ctx.set_paint(Color::from_rgba8(255, 255, 255, 255));
    ctx.fill_rect(&Rect::new(0.0, 0.0, f64::from(width), f64::from(height)));
    ctx.set_transform(Affine::scale(scale) * Affine::translate((origin[0], origin[1])));
    let faint = Color::from_rgba8(215, 215, 215, 255);
    // Pieces with their holes, then cut lines on top.
    let mut pieces: Vec<(&str, Color, BezPath)> = Vec::new();
    for (op, included) in &ops {
        if op.kind == OperationKind::Line {
            continue;
        }
        let id = op.item.as_deref().unwrap_or("");
        let paint = if *included {
            colour(op.colour.as_deref().unwrap_or("#000000"))
        } else {
            faint
        };
        let path = polyline(&op.points, true);
        match pieces.iter_mut().find(|p| p.0 == id) {
            Some(piece) => piece.2.extend(path),
            None => pieces.push((id, paint, path)),
        }
    }
    ctx.set_fill_rule(Fill::EvenOdd);
    for (_, paint, path) in &pieces {
        ctx.set_paint(*paint);
        ctx.fill_path(path);
    }
    for (op, included) in ops.iter().filter(|(o, _)| o.kind == OperationKind::Line) {
        ctx.set_paint(if *included {
            colour(op.colour.as_deref().unwrap_or("#000000"))
        } else {
            faint
        });
        ctx.set_stroke(Stroke::new(2.0 * px).with_dashes(0.0, [6.0 * px, 3.0 * px]));
        ctx.stroke_path(&polyline(&op.points, op.closed));
    }
    let mut pixmap = Pixmap::new(width, height);
    let mut resources = Resources::new();
    ctx.flush();
    ctx.render(&mut pixmap, &mut resources);
    let png = pixmap.into_png().map_err(|e| e.to_string())?;
    Ok(RasterPreview {
        png,
        width: u32::from(width),
        height: u32::from(height),
        px_per_mm: scale,
        origin_px: [origin[0] * scale, origin[1] * scale],
        plan_id: "assembly".into(),
        mirrored: false,
        cuts_shown: ops.len(),
        cuts_total: ops.len(),
        highlighted: Vec::new(),
        legend: vec![
            ("pieces", "filled in their colour, as they fit together"),
            ("cut lines", "dashed, in their colour"),
            ("not cut", "light grey"),
        ],
    })
}
