//! The view arithmetic, apart from egui so it's tested natively: pan and zoom in millimetres, which
//! artwork picture to ask for, and what a drop on the mat sends.

/// A screen rectangle in points: [x0, y0, x1, y1].
pub type Rect = [f32; 4];

/// Zoom limits, in points per mm, as the desktop app's.
const ZOOM_RANGE: (f64, f64) = (0.05, 400.0);
/// The picture covers the visible region plus this much on each side, so small pans reuse it.
pub const ART_MARGIN: f64 = 0.25;
/// The largest picture side asked for, in pixels.
pub const ART_MAX_PX: f64 = 4096.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// The millimetre point at the centre of the pane.
    pub center: [f64; 2],
    /// Points per millimetre.
    pub zoom: f64,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            center: [0.0, 0.0],
            zoom: 2.0,
        }
    }
}

fn centre(r: Rect) -> [f64; 2] {
    [f64::from(r[0] + r[2]) / 2.0, f64::from(r[1] + r[3]) / 2.0]
}

impl Viewport {
    /// Show `bounds` (mm) whole in `rect`, with a margin.
    pub fn fit(&mut self, bounds: [f64; 4], rect: Rect) {
        let [x0, y0, x1, y1] = bounds;
        let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
        let (rw, rh) = (f64::from(rect[2] - rect[0]), f64::from(rect[3] - rect[1]));
        self.zoom = ((rw / w).min(rh / h) * 0.92).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
        self.center = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
    }

    pub fn to_screen(self, rect: Rect, p: [f64; 2]) -> [f32; 2] {
        let c = centre(rect);
        [
            (c[0] + (p[0] - self.center[0]) * self.zoom) as f32,
            (c[1] + (p[1] - self.center[1]) * self.zoom) as f32,
        ]
    }

    pub fn to_mm(self, rect: Rect, pos: [f32; 2]) -> [f64; 2] {
        let c = centre(rect);
        [
            self.center[0] + (f64::from(pos[0]) - c[0]) / self.zoom,
            self.center[1] + (f64::from(pos[1]) - c[1]) / self.zoom,
        ]
    }

    pub fn visible(self, rect: Rect) -> [f64; 4] {
        let a = self.to_mm(rect, [rect[0], rect[1]]);
        let b = self.to_mm(rect, [rect[2], rect[3]]);
        [a[0], a[1], b[0], b[1]]
    }

    /// Zoom by `factor` keeping the millimetre point under `pos` still.
    pub fn zoom_about(&mut self, rect: Rect, pos: [f32; 2], factor: f64) {
        let before = self.to_mm(rect, pos);
        self.zoom = (self.zoom * factor).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
        let after = self.to_mm(rect, pos);
        self.center = [
            self.center[0] + before[0] - after[0],
            self.center[1] + before[1] - after[1],
        ];
    }

    /// The zoom rounded to a step, as the desktop rounds it: pictures are cached per step.
    pub fn zoom_step(self) -> i32 {
        (self.zoom.ln() / 1.15f64.ln()).round() as i32
    }
}

/// Which artwork picture to ask for (spec §12), as the desktop app chooses one: the visible
/// region plus a margin, snapped to a quarter of the view so small pans reuse it, at the screen's
/// pixel density. `scale` converts the view's resized mm to the design's own mm. Returns the
/// region in the design's mm and the size in pixels.
pub fn picture_for(
    view: Viewport,
    rect: Rect,
    pixels_per_point: f32,
    scale: f64,
) -> ([f64; 4], u32, u32) {
    let [vx0, vy0, vx1, vy1] = view.visible(rect);
    let (vw, vh) = (vx1 - vx0, vy1 - vy0);
    let snap = |v: f64, step: f64, up: bool| {
        if step <= 0.0 {
            v
        } else if up {
            (v / step).ceil() * step
        } else {
            (v / step).floor() * step
        }
    };
    let (sx, sy) = (vw / 4.0, vh / 4.0);
    let region = [
        snap(vx0 - vw * ART_MARGIN, sx, false) / scale,
        snap(vy0 - vh * ART_MARGIN, sy, false) / scale,
        snap(vx1 + vw * ART_MARGIN, sx, true) / scale,
        snap(vy1 + vh * ART_MARGIN, sy, true) / scale,
    ];
    let density = view.zoom * f64::from(pixels_per_point) * scale;
    let mut px = [
        (region[2] - region[0]) * density,
        (region[3] - region[1]) * density,
    ];
    let shrink = (ART_MAX_PX / px[0].max(px[1])).min(1.0);
    px = [px[0] * shrink, px[1] * shrink];
    (
        region,
        px[0].round().max(1.0) as u32,
        px[1].round().max(1.0) as u32,
    )
}

/// What a drop on the mat sends: `sheet.place` measures the design's left and top edges from the
/// cutter's start corner, in mm. `art_box` and `origin` are in mat mm; `moved` is the drag.
pub fn place_params(art_box: [f64; 4], origin: [f64; 2], moved: [f64; 2]) -> [f64; 2] {
    let round = |v: f64| (v * 100.0).round() / 100.0;
    [
        round(art_box[0] + moved[0] - origin[0]),
        round(art_box[1] + moved[1] - origin[1]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const PANE: Rect = [0.0, 0.0, 800.0, 600.0];

    #[test]
    fn a_fitted_view_shows_the_whole_bounds() {
        let mut view = Viewport::default();
        view.fit([0.0, 0.0, 80.0, 60.0], PANE);
        let [x0, y0, x1, y1] = view.visible(PANE);
        assert!(x0 <= 0.0 && y0 <= 0.0 && x1 >= 80.0 && y1 >= 60.0);
        let back = view.to_mm(PANE, view.to_screen(PANE, [12.5, 40.0]));
        assert!((back[0] - 12.5).abs() < 1e-3 && (back[1] - 40.0).abs() < 1e-3);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_pointer() {
        let mut view = Viewport::default();
        view.fit([0.0, 0.0, 80.0, 60.0], PANE);
        let pointer = [200.0, 150.0];
        let before = view.to_mm(PANE, pointer);
        view.zoom_about(PANE, pointer, 1.5);
        let after = view.to_mm(PANE, pointer);
        assert!((before[0] - after[0]).abs() < 1e-6 && (before[1] - after[1]).abs() < 1e-6);
    }

    #[test]
    fn a_small_pan_asks_for_the_same_picture() {
        let mut view = Viewport::default();
        view.fit([0.0, 0.0, 80.0, 60.0], PANE);
        let first = picture_for(view, PANE, 2.0, 1.0);
        view.center[0] += 0.5;
        assert_eq!(picture_for(view, PANE, 2.0, 1.0).0, first.0);
        assert!(first.1 <= ART_MAX_PX as u32 && first.2 <= ART_MAX_PX as u32);
    }

    #[test]
    fn pictures_are_asked_for_in_the_designs_own_millimetres() {
        let mut view = Viewport::default();
        view.fit([0.0, 0.0, 160.0, 120.0], PANE);
        let resized = picture_for(view, PANE, 1.0, 2.0).0;
        let mut same = view;
        same.center = [view.center[0] / 2.0, view.center[1] / 2.0];
        same.zoom *= 2.0;
        let original = picture_for(same, PANE, 1.0, 1.0).0;
        for i in 0..4 {
            assert!(
                (resized[i] - original[i]).abs() < 1e-6,
                "{resized:?} vs {original:?}"
            );
        }
    }

    #[test]
    fn a_drop_places_from_the_start_corner() {
        let box_mm = [22.05, 28.4, 100.05, 73.4];
        let origin = [12.7, 19.05];
        assert_eq!(place_params(box_mm, origin, [40.0, 30.0]), [49.35, 39.35]);
        assert_eq!(place_params(box_mm, origin, [0.0, 0.0]), [9.35, 9.35]);
    }
}
