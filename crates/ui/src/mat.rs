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

#[cfg(test)]
mod tests {
    use super::*;
    use cutcraft_web_protocol::{MatGeometry, RulerUnit};

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
