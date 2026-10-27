//! An SVG Cut Preview of one plan (spec §5.4): the mat to scale with an inch grid, the cutting
//! area, pieces in their colours, cut lines dashed and the weeding box. It draws the plan itself,
//! so the preview shows exactly what would be sent.

use std::fmt::Write;

use crate::{CutPlan, OperationKind};

const INCH: f64 = 25.4;

pub fn preview_svg(plan: &CutPlan) -> String {
    let [w, h] = plan.mat_mm;
    let mut s = String::new();
    let _ = write!(
        s,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}mm" height="{h}mm" viewBox="-8 -14 {} {}" font-family="sans-serif">"##,
        w + 16.0,
        h + 22.0
    );
    let _ = write!(
        s,
        r##"<rect x="0" y="0" width="{w}" height="{h}" fill="#f4f4f0" stroke="#999" stroke-width="0.4"/>"##
    );
    // Inch grid.
    let mut x = INCH;
    while x < w {
        let _ = write!(
            s,
            r##"<line x1="{x}" y1="0" x2="{x}" y2="{h}" stroke="#ddd" stroke-width="0.2"/>"##
        );
        x += INCH;
    }
    let mut y = INCH;
    while y < h {
        let _ = write!(
            s,
            r##"<line x1="0" y1="{y}" x2="{w}" y2="{y}" stroke="#ddd" stroke-width="0.2"/>"##
        );
        y += INCH;
    }
    let [aw, ah] = plan.cutting_area_mm;
    let _ = write!(
        s,
        r##"<rect x="0" y="0" width="{aw}" height="{ah}" fill="none" stroke="#7a9" stroke-width="0.3" stroke-dasharray="2 1"/>"##
    );
    if let Some([px, py, px1, py1]) = plan.piece_rect_mm() {
        let (pw, ph) = (px1 - px, py1 - py);
        let _ = write!(
            s,
            r##"<rect x="{px}" y="{py}" width="{pw}" height="{ph}" fill="#ffecb3" fill-opacity="0.6" stroke="#b07800" stroke-width="0.5"/><text x="{}" y="{}" font-size="3" fill="#b07800">your piece {:.2} × {:.2} in</text>"##,
            px + 1.5,
            py1 - 1.5,
            pw / INCH,
            ph / INCH
        );
    }
    // Pieces: each outline filled with its holes, in its colour.
    let mut pieces: Vec<(&str, &str, String)> = Vec::new();
    for op in &plan.operations {
        if !matches!(op.kind, OperationKind::Outline | OperationKind::Hole) {
            continue;
        }
        let id = op.item.as_deref().unwrap_or("");
        let d = path_d(&op.points, true);
        match pieces.iter_mut().find(|p| p.0 == id) {
            Some(p) => p.2.push_str(&d),
            None => pieces.push((id, op.colour.as_deref().unwrap_or("#000000"), d)),
        }
    }
    for (_, colour, d) in &pieces {
        let _ = write!(
            s,
            r##"<path d="{d}" fill="{colour}" fill-rule="evenodd" stroke="#c0c" stroke-width="0.15"/>"##
        );
    }
    for op in &plan.operations {
        let d = path_d(&op.points, op.closed);
        match op.kind {
            OperationKind::Line => {
                let colour = op.colour.as_deref().unwrap_or("#000000");
                let _ = write!(
                    s,
                    r##"<path d="{d}" fill="none" stroke="{colour}" stroke-width="0.4" stroke-dasharray="1.5 0.8"/>"##
                );
            }
            OperationKind::Mark => {
                let _ = write!(
                    s,
                    r##"<path d="{d}" fill="#0078c8" fill-opacity="0.35" stroke="#0078c8" stroke-width="0.5"/>"##
                );
            }
            OperationKind::WeedingBox | OperationKind::WeedLine => {
                let _ = write!(
                    s,
                    r##"<path d="{d}" fill="none" stroke="#888" stroke-width="0.3" stroke-dasharray="3 1.5"/>"##
                );
            }
            _ => {}
        }
    }
    let size = plan
        .design_size_mm
        .map(|[dw, dh]| format!("{:.2} × {:.2} in", dw / INCH, dh / INCH))
        .unwrap_or_else(|| "empty".into());
    let mirrored = if plan.mirror { " · MIRRORED" } else { "" };
    let _ = write!(
        s,
        r##"<text x="0" y="-8" font-size="5">{} · {}{mirrored}</text><text x="0" y="-2.5" font-size="3.2">Design {size} · about {:.0} s · top goes in first ↑</text>"##,
        escape(&plan.sheet_name),
        escape(&plan.material.name),
        plan.estimate_s
    );
    s.push_str("</svg>");
    s
}

fn path_d(points: &[[f64; 2]], closed: bool) -> String {
    let mut d = String::new();
    for (i, p) in points.iter().enumerate() {
        let _ = write!(
            d,
            "{}{:.3} {:.3}",
            if i == 0 { "M" } else { "L" },
            p[0],
            p[1]
        );
    }
    if closed {
        d.push('Z');
    }
    d
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
