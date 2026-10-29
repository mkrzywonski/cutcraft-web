use crate::{Desktop, number};
use base64::Engine;
use egui::{Color32, Pos2, Rect, Stroke, StrokeKind, Ui, Vec2};
use serde_json::json;
#[derive(Clone)]
pub struct CanvasDrag {
    kind: String,
    target: String,
    bounds: [f64; 4],
    offset: [f64; 2],
    size: [f64; 2],
    angle: f64,
    scale: f32,
    incarnation: String,
    revision: u64,
}
impl Desktop {
    fn media_bounds(&self) -> Option<[f64; 4]> {
        let s = &self.remote.snapshot.ui.sheet;
        let w = s["piece_mm"][0].as_f64()?;
        let h = s["piece_mm"][1].as_f64()?;
        let origin = self.remote.geometry.mat.as_ref()?.origin_mm;
        let x = origin[0] + number(&s["piece_at_mm"][0]);
        let y = origin[1] + number(&s["piece_at_mm"][1]);
        Some([x, y, x + w, y + h])
    }
    pub fn canvas(&mut self, ui: &mut Ui) {
        if !self.has_design() {
            ui.vertical_centered(|ui| {
                ui.add_space(100.);
                ui.heading("Open a design to begin");
                ui.label("SVG, PDF, Illustrator, DXF, pictures or a CutCraft project");
                if self.button(ui, "Choose a file…", true) {
                    self.send("upload", json!({}));
                }
                ui.weak("You can also drop a file into this window.");
            });
            return;
        }
        let current_art = self
            .remote
            .snapshot
            .design
            .as_ref()
            .and_then(|d| d.art.as_ref())
            .map(|a| a.id.as_str());
        if current_art != Some(self.texture_id.as_str()) {
            self.texture = None;
        }
        if self.texture_id != self.remote.picture_id
            && !self.remote.picture.is_empty()
            && let Ok(bytes) =
                base64::engine::general_purpose::STANDARD.decode(&self.remote.picture)
            && let Ok(image) = image::load_from_memory(&bytes)
        {
            let image = image.to_rgba8();
            let size = [image.width() as usize, image.height() as usize];
            self.texture = Some(ui.ctx().load_texture(
                "source",
                egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
                Default::default(),
            ));
            self.texture_id = self.remote.picture_id.clone();
        }
        let mat = matches!(self.screen(), "mat" | "cut" | "test-cut");
        if self.screen() == "mat" {
            ui.horizontal_wrapped(|ui| {
                ui.label("Move:");
                for target in ["design", "media", "test spot"] {
                    ui.selectable_value(&mut self.target, target.into(), target);
                }
                ui.weak("Top edge enters cutter ↑");
            });
        }
        let available = ui.available_size().max(Vec2::splat(50.));
        let (rect, response) = ui.allocate_exact_size(available, egui::Sense::click_and_drag());
        self.record("Preview canvas", &response);
        let painter = ui.painter_at(rect);
        painter.rect_filled(
            rect,
            0,
            if self.dark {
                Color32::from_gray(35)
            } else {
                Color32::from_rgb(236, 240, 238)
            },
        );
        if !mat {
            if response.dragged() {
                self.pan += response.drag_delta();
            }
            if response.hovered() {
                let delta = ui.input(|i| i.smooth_scroll_delta.y);
                if delta != 0. {
                    self.zoom = (self.zoom * (delta * 0.002).exp()).clamp(0.1, 50.);
                }
            }
        }
        let mut bounds = if mat {
            self.remote
                .geometry
                .mat
                .as_ref()
                .map(|m| [0., 0., m.outer_mm[0], m.outer_mm[1]])
                .unwrap_or([0., 0., 330., 350.])
        } else {
            self.remote
                .snapshot
                .design
                .as_ref()
                .and_then(|d| d.art.as_ref())
                .map(|a| a.bounds)
                .unwrap_or([0., 0., 100., 100.])
        };
        if !mat && !self.highlight.is_empty() {
            let points = self
                .remote
                .geometry
                .lines
                .iter()
                .filter(|l| l.source_ids.iter().any(|id| self.highlight.contains(id)))
                .flat_map(|l| l.points.iter());
            let mut found = None;
            for p in points {
                let b = found.get_or_insert([p[0] as f64, p[1] as f64, p[0] as f64, p[1] as f64]);
                b[0] = b[0].min(p[0] as f64);
                b[1] = b[1].min(p[1] as f64);
                b[2] = b[2].max(p[0] as f64);
                b[3] = b[3].max(p[1] as f64);
            }
            if let Some(b) = found {
                bounds = [b[0] - 3., b[1] - 3., b[2] + 3., b[3] + 3.];
            }
        }
        let panes = if self.screen() == "side-by-side" {
            2
        } else {
            1
        };
        let pane_width = rect.width() / panes as f32;
        let mut scale = 1.;
        let mut screen_origin = Pos2::ZERO;
        for pane in 0..panes {
            let painter = ui.painter_at(Rect::from_min_size(
                rect.min + Vec2::new(pane_width * pane as f32, 0.),
                Vec2::new(pane_width, rect.height()),
            ));
            let area = Rect::from_min_size(
                rect.min + Vec2::new(pane_width * pane as f32, 0.),
                Vec2::new(pane_width, rect.height()),
            )
            .shrink(22.);
            scale = ((area.width() - 20.) / (bounds[2] - bounds[0]).max(1.) as f32)
                .min((area.height() - 30.) / (bounds[3] - bounds[1]).max(1.) as f32)
                * if mat { 1. } else { self.zoom };
            let origin = area.center()
                - Vec2::new(
                    (bounds[2] - bounds[0]) as f32 * scale,
                    (bounds[3] - bounds[1]) as f32 * scale,
                ) * 0.5
                + if mat { Vec2::ZERO } else { self.pan };
            screen_origin = origin;
            let point = |x: f64, y: f64| {
                Pos2::new(
                    origin.x + (x - bounds[0]) as f32 * scale,
                    origin.y + (y - bounds[1]) as f32 * scale,
                )
            };
            let page = Rect::from_min_max(point(bounds[0], bounds[1]), point(bounds[2], bounds[3]));
            painter.rect_filled(page, 2, Color32::WHITE);
            if !mat {
                let clipped = page.intersect(rect);
                let checker = ui.painter_at(clipped);
                for y in 0..(clipped.height() / 12.).ceil() as i32 {
                    for x in 0..(clipped.width() / 12.).ceil() as i32 {
                        if (x + y) % 2 == 0 {
                            checker.rect_filled(
                                Rect::from_min_size(
                                    clipped.min + Vec2::new(x as f32 * 12., y as f32 * 12.),
                                    Vec2::splat(12.),
                                ),
                                0,
                                Color32::from_gray(225),
                            );
                        }
                    }
                }
            }

            if mat {
                if let Some(m) = &self.remote.geometry.mat {
                    let [x0, y0, x1, y1] = m.sticky_mm;
                    let grid = Rect::from_min_max(point(x0, y0), point(x1, y1));
                    painter.rect_filled(grid, 0, Color32::from_rgb(223, 237, 226));
                    for x in 0..=((x1 - x0) / 25.4) as usize {
                        let x = x0 + x as f64 * 25.4;
                        painter.line_segment(
                            [point(x, y0), point(x, y1)],
                            Stroke::new(0.6, Color32::from_gray(160)),
                        );
                    }
                    for y in 0..=((y1 - y0) / 25.4) as usize {
                        let y = y0 + y as f64 * 25.4;
                        painter.line_segment(
                            [point(x0, y), point(x1, y)],
                            Stroke::new(0.6, Color32::from_gray(160)),
                        );
                    }
                    let s = &self.remote.snapshot.ui.sheet;
                    if let (Some(w), Some(h)) =
                        (s["piece_mm"][0].as_f64(), s["piece_mm"][1].as_f64())
                    {
                        let x = m.origin_mm[0] + number(&s["piece_at_mm"][0]);
                        let y = m.origin_mm[1] + number(&s["piece_at_mm"][1]);
                        painter.rect_filled(
                            Rect::from_min_max(point(x, y), point(x + w, y + h)),
                            0,
                            Color32::from_rgba_unmultiplied(250, 240, 206, 180),
                        );
                    }
                }
            } else if pane == 0 {
                if let Some(texture) = &self.texture {
                    let art = self
                        .remote
                        .snapshot
                        .design
                        .as_ref()
                        .and_then(|d| d.art.as_ref())
                        .map(|a| a.bounds)
                        .unwrap_or(bounds);
                    let artwork = Rect::from_min_max(point(art[0], art[1]), point(art[2], art[3]));
                    painter.image(
                        texture.id(),
                        artwork,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                        Color32::WHITE,
                    );
                } else {
                    painter.text(
                        page.center(),
                        egui::Align2::CENTER_CENTER,
                        "Rendering artwork…",
                        egui::FontId::proportional(14.),
                        Color32::GRAY,
                    );
                }
            }
            let lines = if mat {
                &self.remote.geometry.sheet
            } else {
                &self.remote.geometry.lines
            };
            if mat || pane == 1 || self.screen() == "overlay" {
                let mut last = None;
                let limit = (lines.len() as f32 * self.order) as usize;
                for (i, line) in lines.iter().enumerate() {
                    if !line.included
                        && !line.source_ids.iter().any(|id| self.highlight.contains(id))
                        && !line.item.as_ref().is_some_and(|id| self.peek.contains(id))
                    {
                        continue;
                    }
                    let points: Vec<_> = line
                        .points
                        .iter()
                        .map(|p| point(p[0] as f64, p[1] as f64))
                        .collect();
                    if self.tool_motion
                        && mat
                        && let (Some(a), Some(b)) = (last, points.first())
                    {
                        painter.line_segment([a, *b], Stroke::new(0.6, Color32::LIGHT_BLUE));
                    }
                    last = points.last().copied();
                    let colour = if line.source_ids.iter().any(|id| self.highlight.contains(id)) {
                        Color32::from_rgb(220, 110, 20)
                    } else if i >= limit {
                        Color32::from_gray(200)
                    } else {
                        colour(&line.colour)
                    };
                    if points.len() > 1 {
                        if colour.r() > 220 && colour.g() > 220 && colour.b() > 220 {
                            painter.add(egui::Shape::line(
                                points.clone(),
                                Stroke::new(2.4, Color32::from_gray(120)),
                            ));
                        }
                        painter.add(egui::Shape::line(
                            points,
                            Stroke::new(if line.aid { 0.7 } else { 1.4 }, colour),
                        ));
                    }
                }
            }
            if mat {
                let media = self.media_bounds();
                let selected = match self.target.as_str() {
                    "media" => media,
                    "test spot" => self.remote.geometry.test_spot_mm,
                    _ => self.remote.geometry.art_box_mm,
                };
                if let Some(b) = self.remote.geometry.test_spot_mm.filter(|_| self.show_test) {
                    let r = Rect::from_min_max(point(b[0], b[1]), point(b[2], b[3]));
                    painter.rect_stroke(
                        r,
                        0,
                        Stroke::new(1., Color32::DARK_GRAY),
                        StrokeKind::Inside,
                    );
                    painter.add(egui::Shape::closed_line(
                        vec![
                            r.center_top() + Vec2::new(0., 4.),
                            r.right_bottom() - Vec2::splat(4.),
                            r.left_bottom() + Vec2::new(4., -4.),
                        ],
                        Stroke::new(1., Color32::DARK_GRAY),
                    ));
                }
                if self.screen() == "mat"
                    && let Some(mut b) = selected
                {
                    if let Some(drag) = &self.drag_start {
                        b = drag.preview(self.drag_delta);
                    }
                    let r = Rect::from_min_max(point(b[0], b[1]), point(b[2], b[3]));
                    let ink = Color32::from_rgb(0, 110, 100);
                    painter.rect_stroke(r, 0, Stroke::new(1., ink), StrokeKind::Outside);
                    self.controls.insert(
                        "Selected bounds".into(),
                        [r.min.x, r.min.y, r.max.x, r.max.y],
                    );
                    if self.target != "test spot" {
                        let handles = if self.target == "media" {
                            vec![
                                ("nw", r.left_top()),
                                ("ne", r.right_top()),
                                ("sw", r.left_bottom()),
                                ("se", r.right_bottom()),
                            ]
                        } else {
                            vec![("se", r.right_bottom())]
                        };
                        for (name, p) in handles {
                            painter.rect_filled(Rect::from_center_size(p, Vec2::splat(9.)), 2, ink);
                            self.controls.insert(
                                format!("Resize {name}"),
                                [p.x - 6., p.y - 6., p.x + 6., p.y + 6.],
                            );
                        }
                    }
                    if self.target == "design" {
                        let p = r.center_top() - Vec2::new(0., 22.);
                        painter.line_segment([r.center_top(), p], Stroke::new(1., ink));
                        painter.circle_stroke(p, 5., Stroke::new(2., ink));
                        self.controls.insert(
                            "Rotate design".into(),
                            [p.x - 7., p.y - 7., p.x + 7., p.y + 7.],
                        );
                    }
                    painter.text(
                        r.left_bottom() + Vec2::new(0., 12.),
                        egui::Align2::LEFT_TOP,
                        format!("{:.1} × {:.1} mm", b[2] - b[0], b[3] - b[1]),
                        egui::FontId::proportional(12.),
                        ink,
                    );
                }
            }
            if panes == 2 {
                painter.text(
                    Pos2::new(area.center().x, rect.top() + 9.),
                    egui::Align2::CENTER_TOP,
                    if pane == 0 { "Artwork" } else { "Cut lines" },
                    egui::FontId::proportional(14.),
                    Color32::DARK_GRAY,
                );
            }
        }
        if self.screen() == "mat"
            && !self.remote.pending
            && self.remote.connected
            && response.drag_started()
        {
            let pointer = ui.input(|i| i.pointer.press_origin());
            let selected = match self.target.as_str() {
                "media" => self.media_bounds(),
                "test spot" => self.remote.geometry.test_spot_mm,
                _ => self.remote.geometry.art_box_mm,
            };
            if let (Some(p), Some(b)) = (pointer, selected) {
                let screen = |x: f64, y: f64| {
                    Pos2::new(
                        screen_origin.x + (x - bounds[0]) as f32 * scale,
                        screen_origin.y + (y - bounds[1]) as f32 * scale,
                    )
                };
                let r = Rect::from_min_max(screen(b[0], b[1]), screen(b[2], b[3]));
                let mut kind = None;
                if self.target != "test spot" {
                    let handles = if self.target == "media" {
                        vec![
                            ("nw", r.left_top()),
                            ("ne", r.right_top()),
                            ("sw", r.left_bottom()),
                            ("se", r.right_bottom()),
                        ]
                    } else {
                        vec![("se", r.right_bottom())]
                    };
                    for (name, pos) in handles {
                        if p.distance(pos) < 10. {
                            kind = Some(name);
                        }
                    }
                }
                if self.target == "design" && p.distance(r.center_top() - Vec2::new(0., 22.)) < 10.
                {
                    kind = Some("rotate");
                }
                if kind.is_none() && r.contains(p) {
                    kind = Some("move");
                }
                if let Some(kind) = kind {
                    let sheet = &self.remote.snapshot.ui.sheet;
                    let origin = self
                        .remote
                        .geometry
                        .mat
                        .as_ref()
                        .map(|m| m.origin_mm)
                        .unwrap_or([0., 0.]);
                    let offset = match self.target.as_str() {
                        "media" => [
                            number(&sheet["piece_at_mm"][0]),
                            number(&sheet["piece_at_mm"][1]),
                        ],
                        "test spot" => [b[0] - origin[0], b[1] - origin[1]],
                        _ => [
                            number(&sheet["placement"]["offset_mm"][0]),
                            number(&sheet["placement"]["offset_mm"][1]),
                        ],
                    };
                    self.drag_delta = Vec2::ZERO;
                    self.drag_start = Some(CanvasDrag {
                        kind: kind.into(),
                        target: self.target.clone(),
                        bounds: b,
                        offset,
                        size: self.remote.snapshot.ui.size_mm.unwrap_or([1., 1.]),
                        angle: number(&sheet["placement"]["rotation_deg"]),
                        scale,
                        incarnation: self.remote.snapshot.incarnation.clone(),
                        revision: self.remote.snapshot.revision,
                    });
                }
            }
        }
        if self.drag_start.is_some() && response.dragged() {
            self.drag_delta += response.drag_delta();
        }
        if response.drag_stopped() {
            if let Some(drag) = self.drag_start.take() {
                let snap = if self.remote.snapshot.ui.snap["enabled"] == true {
                    Some(number(&self.remote.snapshot.ui.snap["step_mm"]).max(0.1))
                } else {
                    None
                };
                let (id, params) = drag.command(self.drag_delta, snap);
                self.send("command",json!({"id":id,"params":params,"incarnation":drag.incarnation,"revision":drag.revision}));
            }
            self.drag_delta = Vec2::ZERO;
        }
        if self.playing {
            self.order += ui.input(|i| i.stable_dt) * 0.1;
            if self.order >= 1. {
                self.order = 1.;
                self.playing = false;
            }
            ui.ctx().request_repaint();
        }
    }
}
pub(crate) fn colour(hex: &str) -> Color32 {
    let h = hex.trim_start_matches('#');
    if h.len() == 6
        && let Ok(n) = u32::from_str_radix(h, 16)
    {
        return Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8);
    }
    Color32::BLACK
}

impl CanvasDrag {
    fn preview(&self, delta: Vec2) -> [f64; 4] {
        let [dx, dy] = [(delta.x / self.scale) as f64, (delta.y / self.scale) as f64];
        let mut b = self.bounds;
        match self.kind.as_str() {
            "move" => {
                b[0] += dx;
                b[2] += dx;
                b[1] += dy;
                b[3] += dy;
            }
            "rotate" => {}
            corner => {
                if self.target == "design" {
                    let factor = ((b[2] - b[0] + dx) / (b[2] - b[0]).max(0.001)).max(0.01);
                    b[2] = b[0] + (b[2] - b[0]) * factor;
                    b[3] = b[1] + (b[3] - b[1]) * factor;
                } else {
                    if corner.contains('w') {
                        b[0] = (b[0] + dx).min(b[2] - 1.);
                    } else {
                        b[2] = (b[2] + dx).max(b[0] + 1.);
                    }
                    if corner.contains('n') {
                        b[1] = (b[1] + dy).min(b[3] - 1.);
                    } else {
                        b[3] = (b[3] + dy).max(b[1] + 1.);
                    }
                }
            }
        }
        b
    }
    fn command(&self, delta: Vec2, snap: Option<f64>) -> (&'static str, serde_json::Value) {
        let b = self.preview(delta);
        let round = |x: f64| snap.map_or(x, |step| (x / step).round() * step);
        let x = round(self.offset[0] + b[0] - self.bounds[0]);
        let y = round(self.offset[1] + b[1] - self.bounds[1]);
        if self.target == "media" {
            (
                "sheet.piece",
                json!({"x_mm":x,"y_mm":y,"width_mm":round(b[2]-b[0]).max(1.),"height_mm":round(b[3]-b[1]).max(1.)}),
            )
        } else if self.target == "test spot" {
            ("sheet.test_spot", json!({"x_mm":x,"y_mm":y}))
        } else if self.kind == "rotate" {
            (
                "sheet.place",
                json!({"rotate":(self.angle+delta.x as f64*0.5).rem_euclid(360.)}),
            )
        } else if self.kind == "move" {
            ("sheet.place", json!({"x":x,"y":y}))
        } else {
            (
                "design.size",
                json!({"width_mm":self.size[0]*(b[2]-b[0])/(self.bounds[2]-self.bounds[0]).max(0.001)}),
            )
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn drag(target: &str, kind: &str) -> CanvasDrag {
        CanvasDrag {
            kind: kind.into(),
            target: target.into(),
            bounds: [10., 20., 110., 70.],
            offset: [2., 3.],
            size: [100., 50.],
            angle: 0.,
            scale: 2.,
            incarnation: "i".into(),
            revision: 4,
        }
    }
    #[test]
    fn media_resize_preserves_opposite_corner_and_minimum_size() {
        let d = drag("media", "nw");
        let (id, p) = d.command(Vec2::new(20., 10.), None);
        assert_eq!(id, "sheet.piece");
        assert_eq!(
            p,
            json!({"x_mm":12.,"y_mm":8.,"width_mm":90.,"height_mm":45.})
        );
        let p = d.command(Vec2::splat(1000.), None).1;
        assert_eq!(p["width_mm"], 1.);
        assert_eq!(p["height_mm"], 1.);
    }
    #[test]
    fn moving_test_spot_uses_machine_coordinates_and_snap() {
        let d = drag("test spot", "move");
        assert_eq!(
            d.command(Vec2::new(3., 3.), Some(1.)).1,
            json!({"x_mm":4.,"y_mm":5.})
        );
    }
    #[test]
    fn design_resize_preserves_proportions() {
        let d = drag("design", "se");
        assert_eq!(d.preview(Vec2::new(100., 0.)), [10., 20., 160., 95.]);
        assert_eq!(
            d.command(Vec2::new(100., 0.), None).1,
            json!({"width_mm":150.})
        );
    }
}
