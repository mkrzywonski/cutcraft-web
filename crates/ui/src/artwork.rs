//! Visible-region artwork requests, adapted from the reference desktop viewport.
use crate::Desktop;
use cutcraft_web_protocol::{ArtView, PictureRequest};
use egui::{Pos2, Rect, Ui};

fn region_request(art: &ArtView, visible: [f64; 4], pixels_per_mm: f64) -> PictureRequest {
    let [x0, y0, x1, y1] = visible;
    let (w, h) = ((x1 - x0).max(0.001), (y1 - y0).max(0.001));
    let snap = |v: f64, step: f64, up: bool| {
        if up {
            (v / step).ceil() * step
        } else {
            (v / step).floor() * step
        }
    };
    let region = [
        snap(x0 - w * 0.25, w / 4., false),
        snap(y0 - h * 0.25, h / 4., false),
        snap(x1 + w * 0.25, w / 4., true),
        snap(y1 + h * 0.25, h / 4., true),
    ]
    .map(|v| v / art.scale);
    let density = pixels_per_mm * art.scale;
    let width = (region[2] - region[0]) * density;
    let height = (region[3] - region[1]) * density;
    let shrink = (f64::from(PictureRequest::MAX_SIDE) / width.max(height)).min(1.);
    PictureRequest {
        art: art.id.clone(),
        region,
        zoom_step: (pixels_per_mm.ln() / 1.15f64.ln()).round() as i32,
        width: (width * shrink).round().clamp(1., 4096.) as u32,
        height: (height * shrink).round().clamp(1., 4096.) as u32,
    }
}
impl Desktop {
    pub(crate) fn request_artwork(
        &mut self,
        ui: &Ui,
        viewport: Rect,
        origin: Pos2,
        bounds: [f64; 4],
        scale: f32,
    ) {
        let Some(art) = self
            .remote
            .snapshot
            .design
            .as_ref()
            .and_then(|d| d.art.as_ref())
        else {
            return;
        };
        if scale <= 0. || !scale.is_finite() || art.scale <= 0. {
            return;
        }
        let mm = |p: Pos2| {
            [
                bounds[0] + f64::from((p.x - origin.x) / scale),
                bounds[1] + f64::from((p.y - origin.y) / scale),
            ]
        };
        let [x0, y0] = mm(viewport.min);
        let [x1, y1] = mm(viewport.max);
        let request = region_request(
            art,
            [x0, y0, x1, y1],
            f64::from(scale * ui.ctx().pixels_per_point()),
        );
        let now = ui.input(|i| i.time);
        if self
            .art_requested
            .as_ref()
            .is_none_or(|(previous, _)| previous != &request)
        {
            self.art_requested = Some((request.clone(), now));
        }
        let settled = self
            .art_requested
            .as_ref()
            .is_some_and(|(_, since)| now - since >= 0.12);
        if self.art_sent.as_ref() != Some(&request) && (settled || self.art_sent.is_none()) {
            self.send(
                "artwork",
                serde_json::to_value(&request).unwrap_or_default(),
            );
            self.art_sent = Some(request);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_moving_view_settles_before_requesting_another_region() {
        let mut desktop = Desktop::default();
        desktop.remote.snapshot.design = Some(cutcraft_web_protocol::DesignView {
            art: Some(ArtView {
                id: "art".into(),
                design_revision: 1,
                bounds: [0., 0., 100., 100.],
                scale: 1.,
            }),
            ..Default::default()
        });
        let ctx = egui::Context::default();
        for (time, pan, expected) in [
            (0., 0., 1),
            (1., 50., 1),
            (1.1, 100., 1),
            (1.21, 100., 1),
            (1.23, 100., 2),
        ] {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ui| {
                    desktop.request_artwork(
                        ui,
                        Rect::from_min_size(Pos2::ZERO, egui::vec2(400., 400.)),
                        Pos2::new(pan, 0.),
                        [0., 0., 100., 100.],
                        4.,
                    );
                },
            );
            output.textures_delta.clear();
            assert_eq!(desktop.intents.len(), expected, "time {time}");
        }
    }

    #[test]
    fn region_tracks_zoom_pan_scale_and_screen_density_with_bounded_pixels() {
        let art = ArtView {
            id: "art".into(),
            design_revision: 1,
            bounds: [0., 0., 200., 100.],
            scale: 2.,
        };
        let first = region_request(&art, [0., 0., 100., 100.], 2.);
        assert_eq!(first.region, [-12.5, -12.5, 62.5, 62.5]);
        assert_eq!((first.width, first.height), (300, 300));
        let zoomed = region_request(&art, [25., 25., 75., 75.], 4.);
        assert!(zoomed.region[2] - zoomed.region[0] < first.region[2] - first.region[0]);
        assert_eq!(zoomed.width, first.width);
        let panned = region_request(&art, [75., 25., 125., 75.], 4.);
        assert_eq!(panned.region[0] - zoomed.region[0], 25.);
        let retina = region_request(&art, [25., 25., 75., 75.], 8.);
        assert_eq!(retina.width, zoomed.width * 2);
        let huge = region_request(&art, [0., 0., 10000., 5000.], 20.);
        assert_eq!((huge.width, huge.height), (4096, 2048));
    }
}
