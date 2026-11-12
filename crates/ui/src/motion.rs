//! The reference cut display: byte ranges from the sender, never inferred from line counts.
use cutcraft_web_protocol::CutMotion;
use egui::{Color32, Painter, Pos2, Stroke, Vec2};
use serde::Deserialize;

pub const DONE: Color32 = Color32::from_rgb(20, 20, 20);
pub const TODO: Color32 = Color32::from_rgb(190, 40, 40);
pub const QUEUED: Color32 = Color32::from_rgb(240, 150, 30);

#[derive(Default, Deserialize)]
#[serde(default)]
pub struct Estimate {
    pub done_bytes: usize,
    pub at: [i32; 2],
    pub down: bool,
    pub ahead: Vec<[i32; 2]>,
}

pub fn draw(
    painter: &Painter,
    motion: &CutMotion,
    estimate: Option<&Estimate>,
    origin: [f64; 2],
    point: impl Fn(f64, f64) -> Pos2,
) {
    if !motion.units_per_mm.is_finite() || motion.units_per_mm <= 0. {
        return;
    }
    let at = |p: [i32; 2]| {
        point(
            origin[0] + f64::from(p[0]) / motion.units_per_mm,
            origin[1] + f64::from(p[1]) / motion.units_per_mm,
        )
    };
    let done_bytes = estimate.map_or(0, |e| e.done_bytes);
    for pass in &motion.passes {
        if pass.points.len() < 2 {
            continue;
        }
        let points: Vec<_> = pass.points.iter().copied().map(at).collect();
        if pass.bytes[1] <= done_bytes {
            painter.add(egui::Shape::line(points, Stroke::new(2., DONE)));
        } else {
            painter.add(egui::Shape::line(points.clone(), Stroke::new(1.5, TODO)));
            if let Some(e) = estimate.filter(|e| pass.bytes[0] <= e.done_bytes && e.down) {
                let nearest = pass
                    .points
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, p)| {
                        let dx = i64::from(p[0]) - i64::from(e.at[0]);
                        let dy = i64::from(p[1]) - i64::from(e.at[1]);
                        i128::from(dx) * i128::from(dx) + i128::from(dy) * i128::from(dy)
                    })
                    .map_or(0, |(i, _)| i);
                let mut cut = points[..=nearest].to_vec();
                cut.push(at(e.at));
                painter.add(egui::Shape::line(cut, Stroke::new(2., DONE)));
            }
        }
    }
    if let Some(e) = estimate {
        if e.ahead.len() > 1 {
            painter.add(egui::Shape::line(
                e.ahead.iter().copied().map(at).collect(),
                Stroke::new(6., Color32::from_rgba_unmultiplied(240, 150, 30, 110)),
            ));
        }
        let blade = at(e.at);
        painter.circle_stroke(blade, 7., Stroke::new(2., QUEUED));
        for delta in [Vec2::new(11., 0.), Vec2::new(0., 11.)] {
            painter.line_segment([blade - delta, blade + delta], Stroke::new(1., QUEUED));
        }
        painter.text(
            blade + Vec2::new(10., -10.),
            egui::Align2::LEFT_BOTTOM,
            "≈ blade (estimated)",
            egui::FontId::proportional(11.),
            Color32::from_rgb(170, 95, 0),
        );
    }
}

pub fn length(points: &[[f32; 2]]) -> f64 {
    points
        .windows(2)
        .map(|p| f64::from((p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1])))
        .sum()
}

/// A distance budget can stop between vertices, without favouring short contours.
pub fn prefix(points: &[[f32; 2]], mut distance: f64) -> Vec<[f32; 2]> {
    let Some(first) = points.first() else {
        return Vec::new();
    };
    if distance <= 0. {
        return Vec::new();
    }
    let mut result = vec![*first];
    for pair in points.windows(2) {
        let length = f64::from((pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]));
        if distance < length {
            let t = (distance / length) as f32;
            result.push([
                pair[0][0] + t * (pair[1][0] - pair[0][0]),
                pair[0][1] + t * (pair[1][1] - pair[0][1]),
            ]);
            break;
        }
        result.push(pair[1]);
        distance -= length;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn playback_uses_distance_and_interpolates_between_vertices() {
        let path = [[0., 0.], [100., 0.], [100., 10.]];
        assert_eq!(length(&path), 110.);
        assert_eq!(prefix(&path, 55.), vec![[0., 0.], [55., 0.]]);
        assert_eq!(prefix(&path, 105.), vec![[0., 0.], [100., 0.], [100., 5.]]);
        assert_eq!(prefix(&path, 120.), path);
        assert!(prefix(&path, 0.).is_empty());
    }
    #[test]
    fn byte_progress_draws_done_remaining_queue_and_estimated_blade() {
        let ctx = egui::Context::default();
        let motion = CutMotion {
            units_per_mm: 20.,
            passes: vec![
                cutcraft_web_protocol::MotionPass {
                    bytes: [10, 20],
                    points: vec![[0, 0], [200, 0]],
                },
                cutcraft_web_protocol::MotionPass {
                    bytes: [30, 50],
                    points: vec![[0, 200], [200, 200]],
                },
            ],
        };
        let estimate = Estimate {
            done_bytes: 25,
            at: [100, 100],
            ahead: vec![[100, 100], [200, 200]],
            ..Default::default()
        };
        let mut output = ctx.run_ui(Default::default(), |ui| {
            draw(
                ui.painter(),
                &motion,
                Some(&estimate),
                [12., 18.],
                |x, y| Pos2::new(x as f32, y as f32),
            )
        });
        output.textures_delta.clear();
        let paths: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Path(p) => Some(p),
                _ => None,
            })
            .collect();
        assert!(
            paths
                .iter()
                .any(|p| p.points == [Pos2::new(12., 18.), Pos2::new(22., 18.)]
                    && p.stroke.color == egui::epaint::ColorMode::Solid(DONE))
        );
        assert!(
            paths
                .iter()
                .any(|p| p.stroke.color == egui::epaint::ColorMode::Solid(TODO))
        );
        assert!(output.shapes.iter().any(
            |s| matches!(&s.shape,egui::Shape::Text(t) if t.galley.text()=="≈ blade (estimated)")
        ));
    }
}
