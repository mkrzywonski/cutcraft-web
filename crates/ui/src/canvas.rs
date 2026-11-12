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
    mat_origin: [f64; 2],
    ruler_offset: [f64; 2],
    size: [f64; 2],
    angle: f64,
    scale: f32,
    incarnation: String,
    revision: u64,
}
pub struct Settling {
    target: String,
    original: [f64; 4],
    bounds: [f64; 4],
    geometry: String,
    result: u64,
    until: f64,
}
impl Settling {
    fn finished(
        &self,
        geometry: &str,
        result: u64,
        refused: bool,
        pending: bool,
        now: f64,
    ) -> bool {
        now >= self.until
            || (result != self.result && refused)
            || (!pending && geometry != self.geometry)
    }
}
impl Desktop {
    fn preview_change(&self) -> Option<(String, [f64; 4], [f64; 4])> {
        if self.screen() != "mat" {
            return None;
        }
        self.drag_start
            .as_ref()
            .map(|d| (d.target.clone(), d.bounds, d.preview(self.drag_delta)))
            .or_else(|| {
                self.drag_settling
                    .as_ref()
                    .map(|d| (d.target.clone(), d.original, d.bounds))
            })
    }
    fn canvas_sheet(&self) -> &serde_json::Value {
        if matches!(self.screen(), "cut" | "test-cut") && !self.unprepared_cut_preview {
            &self.remote.cut_preview["sheet"]
        } else {
            &self.remote.snapshot.ui.sheet
        }
    }
    fn occupied_test_bounds(&self) -> Vec<[f64; 4]> {
        if !self.show_test {
            return Vec::new();
        }
        let Some(mat) = &self.remote.geometry.mat else {
            return Vec::new();
        };
        let Ok(preview_mat) = serde_json::from_value::<cutcraft_web_protocol::MatGeometry>(
            self.remote.cut_preview["geometry"]["mat"].clone(),
        ) else {
            return Vec::new();
        };
        if *mat != preview_mat {
            return Vec::new();
        }
        let areas: Vec<[f64; 4]> =
            serde_json::from_value(self.remote.cut_preview["occupied"].clone()).unwrap_or_default();
        areas
            .into_iter()
            .map(|[x0, y0, x1, y1]| {
                [
                    x0 + mat.origin_mm[0],
                    y0 + mat.origin_mm[1],
                    x1 + mat.origin_mm[0],
                    y1 + mat.origin_mm[1],
                ]
            })
            .collect()
    }
    fn media_bounds(&self) -> Option<[f64; 4]> {
        let s = self.canvas_sheet();
        let w = s["piece_mm"][0].as_f64()?;
        let h = s["piece_mm"][1].as_f64()?;
        let origin = self.remote.geometry.mat.as_ref()?.origin_mm;
        let x = origin[0] + number(&s["piece_at_mm"][0]);
        let y = origin[1] + number(&s["piece_at_mm"][1]);
        Some(
            self.preview_change()
                .filter(|(target, _, _)| target == "media")
                .map_or([x, y, x + w, y + h], |(_, _, b)| b),
        )
    }
    pub fn canvas(&mut self, ui: &mut Ui) {
        if self.drag_settling.as_ref().is_some_and(|d| {
            d.finished(
                &self.remote.geometry.id,
                self.remote.result_id,
                self.remote.result.get("error_code").is_some(),
                self.remote.pending,
                ui.input(|i| i.time),
            )
        }) {
            self.drag_settling = None;
        }
        if self.drag_settling.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(30));
        }
        let change = self.preview_change();
        if matches!(self.screen(), "cut" | "test-cut") && self.remote.geometry.mat.is_none() {
            ui.centered_and_justified(|ui| {
                ui.label(if self.cut_busy() {
                    "Recovering the cutter's preview…"
                } else {
                    "Prepare a cut to see its preview."
                });
            });
            return;
        }
        if !self.has_design() && !matches!(self.screen(), "cut" | "test-cut") {
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
        if self.remote.picture.is_empty() || self.texture_source != self.remote.picture_source {
            self.texture = None;
            self.texture_id.clear();
            self.texture_source.clear();
            self.texture_region = None;
        }
        let picture_key = if self.remote.picture_key.is_empty() {
            &self.remote.picture_id
        } else {
            &self.remote.picture_key
        };
        if self.texture_id != *picture_key
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
            self.texture_id = picture_key.clone();
            self.texture_source = self.remote.picture_source.clone();
            self.texture_region = self.remote.picture_region;
        }
        let mat = matches!(self.screen(), "mat" | "cut" | "test-cut");
        if self.screen() == "mat" {
            ui.horizontal_wrapped(|ui| {
                ui.label("Move:");
                for target in ["design", "media", "test spot"] {
                    let response = ui.selectable_value(&mut self.target, target.into(), target);
                    self.record(&format!("Move {target}"), &response);
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
            let middle = response.dragged_by(egui::PointerButton::Middle);
            if middle || response.dragged_by(egui::PointerButton::Primary) {
                self.pan += ui.input(|i| i.pointer.delta());
            }
            if response.hovered() {
                let (mut scroll, shift, zoom, pointer) = ui.input(|i| {
                    (
                        i.smooth_scroll_delta,
                        i.modifiers.shift,
                        i.zoom_delta(),
                        i.pointer.hover_pos(),
                    )
                });
                if zoom != 1. {
                    if let Some(pointer) = pointer {
                        let panes = if self.screen() == "side-by-side" {
                            2.
                        } else {
                            1.
                        };
                        let width = rect.width() / panes;
                        let pane = ((pointer.x - rect.left()) / width)
                            .floor()
                            .clamp(0., panes - 1.);
                        let centre = Pos2::new(rect.left() + (pane + 0.5) * width, rect.center().y);
                        zoom_at(&mut self.zoom, &mut self.pan, pointer - centre, zoom);
                    }
                } else {
                    if shift && scroll.x == 0. {
                        scroll = Vec2::new(scroll.y, 0.);
                    }
                    self.pan += scroll;
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
            if !mat && pane == 0 {
                self.request_artwork(ui, painter.clip_rect(), origin, bounds, scale);
            }
            let page = Rect::from_min_max(point(bounds[0], bounds[1]), point(bounds[2], bounds[3]));
            painter.rect_filled(page, 2, Color32::WHITE);
            if !mat {
                let clipped = page.intersect(painter.clip_rect());
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
                    crate::mat::draw_mat(&painter, m, f64::from(scale), |[x, y]| point(x, y));
                    if let Some([x0, y0, x1, y1]) = self.media_bounds() {
                        painter.rect_filled(
                            Rect::from_min_max(point(x0, y0), point(x1, y1)),
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
                    let factor = self
                        .remote
                        .snapshot
                        .design
                        .as_ref()
                        .and_then(|d| d.art.as_ref())
                        .map(|a| a.scale)
                        .unwrap_or(1.);
                    let art = self
                        .texture_region
                        .map(|region| region.map(|v| v * factor))
                        .unwrap_or(art);
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
            if mat {
                for (index, [x0, y0, x1, y1]) in self.occupied_test_bounds().into_iter().enumerate()
                {
                    let area = Rect::from_min_max(point(x0, y0), point(x1, y1));
                    painter.rect_filled(area, 0, Color32::from_rgba_unmultiplied(128, 110, 90, 35));
                    painter.rect_stroke(
                        area,
                        0,
                        Stroke::new(1.5, Color32::from_rgb(100, 80, 65)),
                        StrokeKind::Inside,
                    );
                    painter.text(
                        area.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("Test {}", index + 1),
                        egui::FontId::proportional(11.),
                        Color32::from_rgb(85, 65, 50),
                    );
                }
            }
            let lines = if mat {
                &self.remote.geometry.sheet
            } else {
                &self.remote.geometry.lines
            };
            let motion = matches!(self.screen(), "cut" | "test-cut")
                .then(|| {
                    serde_json::from_value::<cutcraft_web_protocol::CutMotion>(
                        self.remote.cut_preview["motion"].clone(),
                    )
                    .ok()
                })
                .flatten();
            if let Some(motion) = motion.as_ref().filter(|_| !self.unprepared_cut_preview) {
                let cutter = crate::list(&self.remote.state["cutters"])
                    .into_iter()
                    .find(|c| c["id"] == self.cutter)
                    .unwrap_or_default();
                let estimate = (self.cut_busy()
                    && cutter["view"]["execution"] == self.remote.cut_preview["execution"])
                    .then(|| {
                        serde_json::from_value::<crate::motion::Estimate>(
                            cutter["view"]["progress"]["estimate"].clone(),
                        )
                        .ok()
                    })
                    .flatten();
                let origin = self
                    .remote
                    .geometry
                    .mat
                    .as_ref()
                    .map_or([0., 0.], |m| m.origin_mm);
                crate::motion::draw(&painter, motion, estimate.as_ref(), origin, point);
            } else if mat || pane == 1 || self.screen() == "overlay" {
                let mut last = None;
                let mut distance = lines
                    .iter()
                    .filter(|line| line.included)
                    .map(|line| crate::motion::length(&line.points))
                    .sum::<f64>()
                    * f64::from(self.order);
                for line in lines {
                    if !line.included
                        && !line.source_ids.iter().any(|id| self.highlight.contains(id))
                        && !line.item.as_ref().is_some_and(|id| self.peek.contains(id))
                    {
                        continue;
                    }
                    let points: Vec<_> = line
                        .points
                        .iter()
                        .map(|p| {
                            let q = if mat {
                                change
                                    .as_ref()
                                    .filter(|(target, _, _)| target == "design")
                                    .map(|(_, old, new)| {
                                        map_bounds([f64::from(p[0]), f64::from(p[1])], *old, *new)
                                    })
                                    .unwrap_or([f64::from(p[0]), f64::from(p[1])])
                            } else {
                                [f64::from(p[0]), f64::from(p[1])]
                            };
                            point(q[0], q[1])
                        })
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
                    } else {
                        colour(&line.colour)
                    };
                    let points = if mat && self.order < 1. {
                        painter.add(egui::Shape::line(
                            points,
                            Stroke::new(1., Color32::from_gray(190)),
                        ));
                        let prefix = crate::motion::prefix(&line.points, distance);
                        distance = (distance - crate::motion::length(&line.points)).max(0.);
                        prefix
                            .iter()
                            .map(|p| point(f64::from(p[0]), f64::from(p[1])))
                            .collect::<Vec<_>>()
                    } else {
                        points
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
                if let Some(m) = &self.remote.geometry.mat {
                    crate::mat::draw_orientation(&painter, m, |[x, y]| point(x, y));
                }
                let media = self.media_bounds();
                let selected = match self.target.as_str() {
                    "media" => media,
                    "test spot" => self.remote.geometry.test_spot_mm,
                    _ => self.remote.geometry.art_box_mm,
                };
                if let Some(mut b) = self.remote.geometry.test_spot_mm.filter(|_| self.show_test) {
                    if let Some((target, _, next)) = &change
                        && target == "test spot"
                    {
                        b = *next;
                    }
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
                    if let Some((target, _, next)) = &change
                        && target == &self.target
                    {
                        b = *next;
                    }
                    let r = Rect::from_min_max(point(b[0], b[1]), point(b[2], b[3]));
                    let ink = if self.target == "media" {
                        Color32::from_rgb(170, 110, 20)
                    } else {
                        Color32::from_rgb(35, 105, 180)
                    };
                    painter.rect_stroke(r, 0, Stroke::new(1., ink), StrokeKind::Outside);
                    self.controls.insert(
                        "Selected bounds".into(),
                        [r.min.x, r.min.y, r.max.x, r.max.y],
                    );
                    if self.target != "test spot" {
                        let handles = if self.target == "media" {
                            vec![
                                ("nw", r.left_top()),
                                ("n", r.center_top()),
                                ("ne", r.right_top()),
                                ("e", r.right_center()),
                                ("se", r.right_bottom()),
                                ("s", r.center_bottom()),
                                ("sw", r.left_bottom()),
                                ("w", r.left_center()),
                            ]
                        } else {
                            vec![
                                ("nw", r.left_top()),
                                ("ne", r.right_top()),
                                ("sw", r.left_bottom()),
                                ("se", r.right_bottom()),
                            ]
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
                    for (axis, label, pos, align) in [
                        (
                            0,
                            "Width dimension",
                            r.center_bottom() + Vec2::new(0., 12.),
                            egui::Align2::CENTER_TOP,
                        ),
                        (
                            1,
                            "Height dimension",
                            r.left_center() - Vec2::new(12., 0.),
                            egui::Align2::RIGHT_CENTER,
                        ),
                    ] {
                        let value = b[axis + 2] - b[axis];
                        let inches = self.remote.snapshot.ui.units == "inches";
                        let caption = if inches {
                            format!("{:.2} in", value / 25.4)
                        } else {
                            format!("{value:.1} mm")
                        };
                        let text_rect =
                            painter.text(pos, align, caption, egui::FontId::proportional(12.), ink);
                        let response = ui.interact(
                            text_rect.expand(4.),
                            ui.id().with(label),
                            egui::Sense::click(),
                        );
                        self.record(label, &response);
                        if self.target == "design"
                            && value > 0.
                            && response.double_clicked()
                            && !self.remote.pending
                        {
                            self.open_dialog(
                                "dimension",
                                if axis == 0 {
                                    "Design width"
                                } else {
                                    "Design height"
                                },
                            );
                            if let Some(d) = &mut self.dialog {
                                d.value = json!({"length":format!("{:.4}",value/if inches{25.4}else{1.}),"inches":inches,"original_mm":value,"source_length":self.remote.snapshot.ui.size_mm.map_or(1.,|s|if s[0]>0.{s[0]}else{s[1]}),"size_key":if self.remote.snapshot.ui.size_mm.is_some_and(|s|s[0]>0.){"width_mm"}else{"height_mm"},"incarnation":self.remote.snapshot.incarnation,"revision":self.remote.snapshot.revision});
                            }
                        }
                    }
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
            self.playing = false;
            self.order = 1.;
            self.drag_settling = None;
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
                            ("n", r.center_top()),
                            ("ne", r.right_top()),
                            ("e", r.right_center()),
                            ("se", r.right_bottom()),
                            ("s", r.center_bottom()),
                            ("sw", r.left_bottom()),
                            ("w", r.left_center()),
                        ]
                    } else {
                        vec![
                            ("nw", r.left_top()),
                            ("ne", r.right_top()),
                            ("sw", r.left_bottom()),
                            ("se", r.right_bottom()),
                        ]
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
                        mat_origin: origin,
                        ruler_offset: self
                            .remote
                            .geometry
                            .mat
                            .as_ref()
                            .map(|m| {
                                [
                                    m.origin_mm[0] - m.grid_origin_mm[0],
                                    m.origin_mm[1] - m.grid_origin_mm[1],
                                ]
                            })
                            .unwrap_or([0., 0.]),
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
                self.drag_settling = Some(Settling {
                    target: drag.target.clone(),
                    original: drag.bounds,
                    bounds: drag.dropped_bounds(self.drag_delta, snap),
                    geometry: self.remote.geometry.id.clone(),
                    result: self.remote.result_id,
                    until: ui.input(|i| i.time) + 2.,
                });
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
    fn fixed_corner(&self) -> &'static str {
        match self.kind.as_str() {
            "nw" => "se",
            "ne" => "sw",
            "sw" => "ne",
            _ => "nw",
        }
    }
    fn scaled_bounds(&self, factor: f64) -> [f64; 4] {
        let fixed = [
            self.bounds[if self.kind.contains('w') { 2 } else { 0 }],
            self.bounds[if self.kind.contains('n') { 3 } else { 1 }],
        ];
        [
            fixed[0] + (self.bounds[0] - fixed[0]) * factor,
            fixed[1] + (self.bounds[1] - fixed[1]) * factor,
            fixed[0] + (self.bounds[2] - fixed[0]) * factor,
            fixed[1] + (self.bounds[3] - fixed[1]) * factor,
        ]
    }
    fn dropped_bounds(&self, delta: Vec2, snap: Option<f64>) -> [f64; 4] {
        let (id, p) = self.command(delta, snap);
        match id {
            "design.size" => self.scaled_bounds(if self.size[0] > 0. {
                number(&p["width_mm"]) / self.size[0]
            } else {
                number(&p["height_mm"]) / self.size[1]
            }),
            "sheet.piece" => {
                let x = number(&p["x_mm"]) + self.mat_origin[0];
                let y = number(&p["y_mm"]) + self.mat_origin[1];
                [
                    x,
                    y,
                    x + number(&p["width_mm"]),
                    y + number(&p["height_mm"]),
                ]
            }
            "sheet.place" if self.kind == "move" => {
                let dx = number(&p["x"]) - self.offset[0];
                let dy = number(&p["y"]) - self.offset[1];
                [
                    self.bounds[0] + dx,
                    self.bounds[1] + dy,
                    self.bounds[2] + dx,
                    self.bounds[3] + dy,
                ]
            }
            "sheet.test_spot" => {
                let x = number(&p["x_mm"]) + self.mat_origin[0];
                let y = number(&p["y_mm"]) + self.mat_origin[1];
                [
                    x,
                    y,
                    x + self.bounds[2] - self.bounds[0],
                    y + self.bounds[3] - self.bounds[1],
                ]
            }
            _ => self.preview(delta),
        }
    }
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
                    let fx =
                        1. + if corner.contains('w') { -dx } else { dx } / (b[2] - b[0]).max(0.001);
                    let fy =
                        1. + if corner.contains('n') { -dy } else { dy } / (b[3] - b[1]).max(0.001);
                    let factor = if b[2] - b[0] < 1e-6 {
                        fy
                    } else if b[3] - b[1] < 1e-6 {
                        fx
                    } else {
                        (fx + fy) / 2.
                    };
                    b = self.scaled_bounds(factor.clamp(0.02, 50.));
                } else {
                    if corner.contains('w') {
                        b[0] = (b[0] + dx).min(b[2] - 1.);
                    } else if corner.contains('e') {
                        b[2] = (b[2] + dx).max(b[0] + 1.);
                    }
                    if corner.contains('n') {
                        b[1] = (b[1] + dy).min(b[3] - 1.);
                    } else if corner.contains('s') {
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
        let snapped_offset = |axis: usize| {
            let corner = self.bounds[axis] - self.mat_origin[axis];
            let next = corner + b[axis] - self.bounds[axis];
            let snapped = round(next + self.ruler_offset[axis]) - self.ruler_offset[axis];
            self.offset[axis] + snapped - corner
        };
        let (x, y) = (snapped_offset(0), snapped_offset(1));
        if self.target == "media" {
            if self.kind == "move" {
                return (
                    "sheet.piece",
                    json!({"x_mm":x,"y_mm":y,"width_mm":self.bounds[2]-self.bounds[0],"height_mm":self.bounds[3]-self.bounds[1]}),
                );
            }
            // Reference resize contract: snap the changed size, then derive its moving edge
            // from the fixed opposite edge. Unchanged axes retain their exact dimensions.
            let mut size = [
                self.bounds[2] - self.bounds[0],
                self.bounds[3] - self.bounds[1],
            ];
            let mut offset = self.offset;
            for (axis, low, high) in [(0, 'w', 'e'), (1, 'n', 's')] {
                if self.kind.contains(low) || self.kind.contains(high) {
                    size[axis] = round(b[axis + 2] - b[axis]).max(1.);
                    if self.kind.contains(low) {
                        offset[axis] += self.bounds[axis + 2] - self.bounds[axis] - size[axis];
                    }
                }
            }
            (
                "sheet.piece",
                json!({"x_mm":offset[0],"y_mm":offset[1],"width_mm":size[0],"height_mm":size[1]}),
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
            let axis = usize::from(self.bounds[2] - self.bounds[0] < 1e-6);
            let factor = round(b[axis + 2] - b[axis]).max(0.1)
                / (self.bounds[axis + 2] - self.bounds[axis]).max(0.001);
            let (key, size) = if self.size[0] > 0. {
                ("width_mm", self.size[0])
            } else {
                ("height_mm", self.size[1])
            };
            (
                "design.size",
                json!({key:size*factor,"fixed_corner":self.fixed_corner()}),
            )
        }
    }
}
fn map_bounds(p: [f64; 2], old: [f64; 4], new: [f64; 4]) -> [f64; 2] {
    [
        new[0] + (p[0] - old[0]) * (new[2] - new[0]) / (old[2] - old[0]).max(1e-9),
        new[1] + (p[1] - old[1]) * (new[3] - new[1]) / (old[3] - old[1]).max(1e-9),
    ]
}

/// Keep the source coordinate under the pointer fixed, including at zoom limits.
fn zoom_at(zoom: &mut f32, pan: &mut Vec2, anchor: Vec2, factor: f32) {
    let next = (*zoom * factor).clamp(0.1, 50.);
    *pan = anchor + (*pan - anchor) * (next / *zoom);
    *zoom = next;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zoom_keeps_the_point_under_the_pointer_in_both_panes() {
        for anchor in [Vec2::new(-70., 35.), Vec2::new(120., -80.)] {
            let mut zoom = 1.;
            let mut pan = Vec2::new(42., -19.);
            let source = (anchor - pan) / zoom;
            for factor in [2., 0.5, 1000., 0.00001] {
                zoom_at(&mut zoom, &mut pan, anchor, factor);
                assert!((pan + source * zoom - anchor).length() < 0.001);
            }
        }
    }

    #[test]
    fn source_texture_survives_a_new_render_but_never_a_new_source() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([10, 20, 30, 255]),
        ))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
        let mut desktop = Desktop::default();
        desktop.remote.snapshot.design = Some(cutcraft_web_protocol::DesignView {
            art: Some(cutcraft_web_protocol::ArtView {
                id: "old-render".into(),
                design_revision: 1,
                bounds: [0., 0., 2., 2.],
                scale: 1.,
            }),
            ..Default::default()
        });
        desktop.remote.snapshot.ui.screen = "side-by-side".into();
        desktop.remote.picture = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
        desktop.remote.picture_id = "old-render".into();
        desktop.remote.picture_source = "session|source".into();
        desktop.remote.picture_region = Some([0., 0., 2., 2.]);
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ui| desktop.canvas(ui));
        output.textures_delta.clear();
        let texture = desktop.texture.as_ref().unwrap().id();
        desktop
            .remote
            .snapshot
            .design
            .as_mut()
            .unwrap()
            .art
            .as_mut()
            .unwrap()
            .id = "new-render".into();
        let mut output = ctx.run_ui(Default::default(), |ui| desktop.canvas(ui));
        output.textures_delta.clear();
        assert_eq!(desktop.texture.as_ref().unwrap().id(), texture);
        assert_eq!(desktop.texture_region, Some([0., 0., 2., 2.]));
        desktop.remote.picture_key = "same-art-different-region".into();
        desktop.remote.picture_region = Some([1., 1., 2., 2.]);
        let mut output = ctx.run_ui(Default::default(), |ui| desktop.canvas(ui));
        output.textures_delta.clear();
        assert_ne!(desktop.texture.as_ref().unwrap().id(), texture);
        assert_eq!(desktop.texture_region, Some([1., 1., 2., 2.]));
        desktop.remote.picture_source = "different-session|source".into();
        desktop.remote.picture = "invalid image".into();
        let mut output = ctx.run_ui(Default::default(), |ui| desktop.canvas(ui));
        output.textures_delta.clear();
        assert!(
            desktop.texture.is_none(),
            "a bad new image cannot leave another source's texture visible"
        );
    }

    #[test]
    fn cut_canvas_uses_frozen_material_instead_of_the_editable_draft() {
        let mut desktop = Desktop::default();
        desktop.remote.geometry.mat = Some(cutcraft_web_protocol::MatGeometry {
            origin_mm: [10., 20.],
            ..Default::default()
        });
        desktop.remote.snapshot.ui.sheet = json!({"piece_mm":[300,300],"piece_at_mm":[90,90]});
        desktop.remote.cut_preview = json!({"sheet":{"piece_mm":[25,50],"piece_at_mm":[2,3]}});
        for screen in ["cut", "test-cut"] {
            desktop.remote.snapshot.ui.screen = screen.into();
            assert_eq!(desktop.media_bounds(), Some([12., 23., 37., 73.]));
        }
        desktop.remote.cut_preview = json!({});
        assert_eq!(
            desktop.media_bounds(),
            None,
            "missing preview must not borrow draft material"
        );
        desktop.remote.snapshot.ui.screen = "mat".into();
        assert_eq!(desktop.media_bounds(), Some([100., 110., 400., 410.]));
    }

    #[test]
    fn occupied_tests_follow_the_mat_origin_and_hiding_does_not_free_them() {
        let mut desktop = Desktop {
            show_test: true,
            ..Default::default()
        };
        let mat = cutcraft_web_protocol::MatGeometry {
            origin_mm: [12., 18.],
            ..Default::default()
        };
        desktop.remote.geometry.mat = Some(mat.clone());
        desktop.remote.cut_preview = json!({"geometry":{"mat":mat},"occupied":[[1,2,26.4,27.4]]});
        assert_eq!(desktop.occupied_test_bounds(), vec![[13., 20., 38.4, 45.4]]);
        desktop.show_test = false;
        assert!(desktop.occupied_test_bounds().is_empty());
        assert_eq!(
            desktop.remote.cut_preview["occupied"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        desktop.show_test = true;
        desktop.remote.geometry.mat.as_mut().unwrap().origin_mm = [0., 0.];
        assert!(
            desktop.occupied_test_bounds().is_empty(),
            "another mat setup must not inherit the overlay"
        );
    }

    #[test]
    fn panned_comparison_checkerboard_stays_inside_its_pane() {
        // Regression for the review's pane-divider bleed: inspect actual paint clips,
        // with each page crossing its pane edge in either direction.
        for pan_x in [-230., 230.] {
            let mut desktop = Desktop::default();
            desktop.remote.snapshot.design = Some(Default::default());
            desktop.remote.snapshot.ui.screen = "side-by-side".into();
            desktop.pan = Vec2::new(pan_x, 0.);
            desktop.zoom = 2.;
            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800., 600.))),
                    ..Default::default()
                },
                |ui| desktop.canvas(ui),
            );
            output.textures_delta.clear();
            let canvas = desktop.controls["Preview canvas"];
            let divider = (canvas[0] + canvas[2]) * 0.5;
            let mut checks = 0;
            for shape in output.shapes {
                if let egui::epaint::Shape::Rect(rect) = shape.shape
                    && rect.fill == Color32::from_gray(225)
                {
                    checks += 1;
                    assert!(
                        shape.clip_rect.right() <= divider || shape.clip_rect.left() >= divider,
                        "checkerboard crossed the pane divider: {:?}",
                        shape.clip_rect
                    );
                }
            }
            assert!(checks > 0);
        }
    }

    fn drag(target: &str, kind: &str) -> CanvasDrag {
        CanvasDrag {
            kind: kind.into(),
            target: target.into(),
            bounds: [10., 20., 110., 70.],
            offset: [2., 3.],
            mat_origin: [8., 17.],
            ruler_offset: [0., 0.],
            size: [100., 50.],
            angle: 0.,
            scale: 2.,
            incarnation: "i".into(),
            revision: 4,
        }
    }
    #[test]
    fn media_edge_handles_change_one_axis_and_keep_the_opposite_edge_fixed() {
        for (edge, expected) in [
            ("w", [12., 3., 90., 50.]),
            ("e", [2., 3., 110., 50.]),
            ("n", [2., 8., 100., 45.]),
            ("s", [2., 3., 100., 55.]),
        ] {
            let d = drag("media", edge);
            let (id, p) = d.command(Vec2::new(20., 10.), None);
            assert_eq!(id, "sheet.piece");
            assert_eq!(
                p,
                json!({"x_mm":expected[0],"y_mm":expected[1],"width_mm":expected[2],"height_mm":expected[3]})
            );
        }
        let d = drag("media", "w");
        let p = d.command(Vec2::new(23., 80.), Some(5.)).1;
        assert_eq!(
            p,
            json!({"x_mm":12.,"y_mm":3.,"width_mm":90.,"height_mm":50.})
        );
    }
    #[test]
    fn moving_media_snaps_to_printed_zero_without_resizing_it() {
        let mut d = drag("media", "move");
        d.bounds = [10., 20., 112., 73.];
        d.ruler_offset = [-1., 2.];
        let p = d.command(Vec2::new(3., 3.), Some(5.)).1;
        assert_eq!(
            p,
            json!({"x_mm":6.,"y_mm":3.,"width_mm":102.,"height_mm":53.})
        );
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
    fn every_design_corner_keeps_its_opposite_fixed_and_snaps_linked_size() {
        for (handle, fixed, delta, index) in [
            ("nw", "se", [-20., -10.], [2, 3]),
            ("ne", "sw", [20., -10.], [0, 3]),
            ("sw", "ne", [-20., 10.], [2, 1]),
            ("se", "nw", [20., 10.], [0, 1]),
        ] {
            let d = drag("design", handle);
            let delta = Vec2::from(delta);
            let b = d.dropped_bounds(delta, Some(5.));
            assert_eq!(b[index[0]], d.bounds[index[0]]);
            assert_eq!(b[index[1]], d.bounds[index[1]]);
            assert!(((b[2] - b[0]) / (b[3] - b[1]) - 2.).abs() < 1e-9);
            let p = d.command(delta, Some(5.)).1;
            assert_eq!(p["fixed_corner"], fixed);
            assert!((p["width_mm"].as_f64().unwrap() - 110.).abs() < 1e-9);
        }
    }
    #[test]
    fn dropped_geometry_holds_until_a_new_plan_refusal_or_timeout() {
        let d = Settling {
            target: "design".into(),
            original: [0.; 4],
            bounds: [1.; 4],
            geometry: "old".into(),
            result: 4,
            until: 2.,
        };
        assert!(!d.finished("old", 4, false, false, 1.));
        assert!(!d.finished("new", 5, false, true, 1.));
        assert!(d.finished("new", 5, false, false, 1.));
        assert!(d.finished("old", 5, true, false, 1.));
        assert!(d.finished("old", 4, false, true, 2.));
        assert_eq!(
            map_bounds([20., 30.], [10., 20., 110., 70.], [5., 10., 205., 110.]),
            [25., 30.]
        );
    }

    #[test]
    fn vertical_lines_resize_by_the_nonzero_dimension() {
        let mut d = drag("design", "se");
        d.bounds = [10., 20., 10., 70.];
        d.size = [0., 50.];
        let delta = Vec2::new(100., 20.);
        assert_eq!(
            d.command(delta, None).1,
            json!({"height_mm":60.,"fixed_corner":"nw"})
        );
        assert_eq!(d.dropped_bounds(delta, None), [10., 20., 10., 80.]);
    }
    #[test]
    fn design_resize_preserves_proportions() {
        let d = drag("design", "se");
        assert_eq!(d.preview(Vec2::new(100., 50.)), [10., 20., 160., 95.]);
        assert_eq!(
            d.command(Vec2::new(100., 50.), None).1,
            json!({"width_mm":150.,"fixed_corner":"nw"})
        );
    }
}
