// Adapted from CutCraft 1.0.5's physical mat drawing (see docs/provenance.md).
// Coordinates and per-edge ruler units come from the shared geometry response.
pub(crate) fn draw_mat(
    painter: &egui::Painter,
    mat: &cutcraft_web_protocol::MatGeometry,
    zoom: f64,
    at: impl Fn([f64; 2]) -> egui::Pos2,
) {
    use cutcraft_web_protocol::RulerUnit;
    let outer = egui::Rect::from_two_pos(at([0.0, 0.0]), at(mat.outer_mm));
    painter.rect_filled(outer, 6.0, egui::Color32::from_rgb(150, 190, 215));
    painter.rect_stroke(
        outer,
        6.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(90, 130, 160)),
        egui::StrokeKind::Middle,
    );
    let [sx0, sy0, sx1, sy1] = mat.sticky_mm;
    let sticky = egui::Rect::from_two_pos(at([sx0, sy0]), at([sx1, sy1]));
    painter.rect_filled(sticky, 0.0, egui::Color32::from_rgb(240, 246, 250));
    let [gx, gy] = mat.grid_origin_mm;
    let unit = |u: RulerUnit| match u {
        RulerUnit::Inches => Some((25.4, 8u32, "")),
        RulerUnit::Centimetres => Some((10.0, 10u32, "")),
        RulerUnit::None => None,
    };
    // Grid lines every unit of the top ruler.
    if let Some((step, ..)) = unit(mat.rulers[0]).or(unit(mat.rulers[3])) {
        let grid = egui::Stroke::new(1.0, egui::Color32::from_rgb(200, 215, 228));
        let mut x = gx + ((sx0 - gx) / step).ceil() * step;
        while x <= sx1 + 1e-6 {
            painter.line_segment([at([x, sy0]), at([x, sy1])], grid);
            x += step;
        }
        let mut y = gy + ((sy0 - gy) / step).ceil() * step;
        while y <= sy1 + 1e-6 {
            painter.line_segment([at([sx0, y]), at([sx1, y])], grid);
            y += step;
        }
    }
    painter.rect_stroke(
        sticky,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(90, 130, 160)),
        egui::StrokeKind::Middle,
    );
    let ink = egui::Color32::from_rgb(40, 60, 80);
    let font = egui::FontId::proportional(9.0);
    // Rulers: top, right, bottom, left.
    for (edge, u) in mat.rulers.iter().enumerate() {
        let Some((step, parts, _)) = unit(*u) else {
            continue;
        };
        let fine = step / f64::from(parts);
        let horizontal = edge % 2 == 0;
        let (from, to, origin) = if horizontal {
            (sx0, sx1, gx)
        } else {
            (sy0, sy1, gy)
        };
        let mut k = ((from - origin) / fine).ceil() as i64;
        loop {
            let v = origin + k as f64 * fine;
            if v > to + 1e-6 {
                break;
            }
            let whole = k.rem_euclid(i64::from(parts)) == 0;
            let half = parts % 2 == 0 && k.rem_euclid(i64::from(parts)) == i64::from(parts / 2);
            let length = if whole {
                6.0
            } else if half {
                4.0
            } else {
                2.0
            };
            let (p, q, text_at, align) = match edge {
                0 => (
                    at([v, sy0]),
                    at([v, sy0]) + egui::vec2(0.0, length),
                    at([v, sy0]) - egui::vec2(0.0, 2.0),
                    egui::Align2::CENTER_BOTTOM,
                ),
                1 => (
                    at([sx1, v]),
                    at([sx1, v]) - egui::vec2(length, 0.0),
                    at([sx1, v]) + egui::vec2(3.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                ),
                2 => (
                    at([v, sy1]),
                    at([v, sy1]) - egui::vec2(0.0, length),
                    at([v, sy1]) + egui::vec2(0.0, 2.0),
                    egui::Align2::CENTER_TOP,
                ),
                _ => (
                    at([sx0, v]),
                    at([sx0, v]) + egui::vec2(length, 0.0),
                    at([sx0, v]) - egui::vec2(3.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                ),
            };
            // Ticks too close to read are left out; whole units always show.
            if whole || fine * zoom >= 3.0 {
                painter.line_segment([p, q], egui::Stroke::new(1.0, ink));
            }
            if whole && step * zoom >= 14.0 {
                let n = (k / i64::from(parts)).to_string();
                painter.text(text_at, align, n, font.clone(), ink);
            }
            k += 1;
        }
    }
    if mat.approximate {
        painter.text(
            outer.left_bottom() + egui::vec2(6.0, -4.0),
            egui::Align2::LEFT_BOTTOM,
            "Approximate mat measurements — measure your mat with Edit mats… (Mat group)",
            egui::FontId::proportional(10.0),
            ink,
        );
    }
}

/// Physical loading direction and the cutter coordinate origin, distinct from printed zero.
pub(crate) fn draw_orientation(
    painter: &egui::Painter,
    mat: &cutcraft_web_protocol::MatGeometry,
    at: impl Fn([f64; 2]) -> egui::Pos2,
) {
    let ink = egui::Color32::from_rgb(20, 90, 120);
    let top = at([mat.outer_mm[0] / 2., 0.]);
    painter.arrow(
        top + egui::vec2(0., 25.),
        egui::vec2(0., -19.),
        egui::Stroke::new(2., ink),
    );
    painter.text(
        top + egui::vec2(8., 16.),
        egui::Align2::LEFT_CENTER,
        "Load this edge first",
        egui::FontId::proportional(11.),
        ink,
    );
    let start = at(mat.origin_mm);
    painter.circle_filled(start, 4., ink);
    painter.circle_stroke(start, 4., egui::Stroke::new(1.5, egui::Color32::WHITE));
    painter.text(
        start + egui::vec2(7., 7.),
        egui::Align2::LEFT_TOP,
        "Start corner",
        egui::FontId::proportional(11.),
        ink,
    );
}

/// A transient editor diagram. Invalid in-progress numbers never drive ruler loops.
pub(crate) fn editor_diagram(ui: &mut egui::Ui, value: &serde_json::Value) {
    let mut geometry = value.clone();
    geometry["origin_mm"] = value["cut_origin_mm"].clone();
    let Ok(mat) = serde_json::from_value::<cutcraft_web_protocol::MatGeometry>(geometry) else {
        return;
    };
    let [w, h] = mat.outer_mm;
    let [x0, y0, x1, y1] = mat.sticky_mm;
    let finite = mat
        .outer_mm
        .iter()
        .chain(&mat.sticky_mm)
        .chain(&mat.grid_origin_mm)
        .chain(&mat.origin_mm)
        .all(|v| v.is_finite());
    if !finite
        || w <= 0.
        || h <= 0.
        || w > 2000.
        || h > 2000.
        || x0 < 0.
        || y0 < 0.
        || x1 <= x0
        || y1 <= y0
        || x1 > w
        || y1 > h
    {
        ui.weak("Enter an outer size and a sticky grid inside it to see the mat.");
        return;
    }
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 220.), egui::Sense::hover());
    let scale = (f64::from(rect.width() - 32.) / w)
        .min(f64::from(rect.height() - 32.) / h)
        .max(0.001);
    let origin = rect.center() - egui::vec2((w * scale) as f32, (h * scale) as f32) * 0.5;
    let at = |[x, y]: [f64; 2]| origin + egui::vec2((x * scale) as f32, (y * scale) as f32);
    let painter = ui.painter_at(rect);
    // The full-size workspace has room for the approximate-measurement note.
    let mut diagram = mat.clone();
    diagram.approximate = false;
    draw_mat(&painter, &diagram, scale, at);
    draw_orientation(&painter, &diagram, at);
    ui.weak("Blue is the outer mat; the pale area is the sticky grid. Rulers start at the ruler origin. The dot marks the cutter’s start corner.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use cutcraft_web_protocol::{MatGeometry, RulerUnit};

    #[test]
    fn editor_diagram_updates_from_draft_measurements_and_tolerates_incomplete_input() {
        let ctx = egui::Context::default();
        for (width, drawn) in [(330., true), (0., false), (1e12, false)] {
            let value = serde_json::json!({"outer_mm":[width,350.],"sticky_mm":[10.,20.,300.,330.],"grid_origin_mm":[10.,20.],"cut_origin_mm":[12.,22.],"rulers":["inches","none","centimetres","none"]});
            let mut output = ctx.run_ui(Default::default(), |ui| editor_diagram(ui, &value));
            output.textures_delta.clear();
            let has_start = output.shapes.iter().any(
                |s| matches!(&s.shape,egui::Shape::Text(t) if t.galley.text()=="Start corner"),
            );
            assert_eq!(has_start, drawn);
        }
    }

    #[test]
    fn orientation_uses_cut_origin_and_points_into_the_cutter() {
        let mat = MatGeometry {
            outer_mm: [150., 160.],
            origin_mm: [22., 33.],
            grid_origin_mm: [25., 40.],
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            draw_orientation(ui.painter(), &mat, |[x, y]| {
                egui::pos2(x as f32 * 2., y as f32 * 2.)
            })
        });
        output.textures_delta.clear();
        assert!(
            output.shapes.iter().any(
                |s| matches!(&s.shape,egui::Shape::Circle(c) if c.center==egui::pos2(44.,66.))
            )
        );
        assert!(output.shapes.iter().any(|s|matches!(&s.shape,egui::Shape::LineSegment {points,..} if points[0]==egui::pos2(150.,25.) && points[1]==egui::pos2(150.,6.))));
        for label in ["Start corner", "Load this edge first"] {
            assert!(
                output
                    .shapes
                    .iter()
                    .any(|s| matches!(&s.shape,egui::Shape::Text(t) if t.galley.text()==label))
            );
        }
    }

    #[test]
    fn mixed_rulers_and_grid_use_the_printed_zero_not_the_cut_origin() {
        let mat = MatGeometry {
            outer_mm: [150., 160.],
            sticky_mm: [20., 30., 120., 130.],
            grid_origin_mm: [25., 40.],
            origin_mm: [22., 33.],
            rulers: [
                RulerUnit::Centimetres,
                RulerUnit::None,
                RulerUnit::Inches,
                RulerUnit::None,
            ],
            approximate: false,
        };
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            draw_mat(ui.painter(), &mat, 2., |[x, y]| {
                egui::pos2(x as f32 * 2., y as f32 * 2.)
            });
        });
        output.textures_delta.clear();
        let mut ones = Vec::new();
        let mut grid_x = Vec::new();
        for shape in output.shapes {
            match shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "1" => {
                    ones.push(text.pos.x + text.galley.size().x / 2.)
                }
                egui::Shape::LineSegment { points, stroke }
                    if stroke.color == egui::Color32::from_rgb(200, 215, 228)
                        && points[0].x == points[1].x =>
                {
                    grid_x.push(points[0].x)
                }
                _ => {}
            }
        }
        ones.sort_by(f32::total_cmp);
        assert_eq!(ones.len(), 2, "only the top and bottom rulers have labels");
        assert!(
            (ones[0] - 70.).abs() < 0.01,
            "top 1 cm is 10 mm from the printed zero"
        );
        assert!(
            (ones[1] - 100.8).abs() < 0.01,
            "bottom 1 inch is 25.4 mm from the same zero"
        );
        assert_eq!(grid_x.first(), Some(&50.));
        assert_eq!(grid_x.get(1), Some(&70.));
    }
}
