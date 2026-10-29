//! Original vector-drawn application chrome. No icon fonts or bitmap assets are required.
use egui::{self, Color32, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2, pos2, vec2};

#[derive(Clone, Copy)]
pub enum Icon {
    Open,
    Save,
    Undo,
    Redo,
    SideBySide,
    Overlay,
    Mat,
    Minus,
    Plus,
    Fit,
    TestCut,
    Exit,
    Cut,
    Stop,
    More,
}

pub const ACCENT: Color32 = Color32::from_rgb(29, 139, 139);

/// A flat toolbar control with a vector icon and an optional visible label.
pub fn tool(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    tip: &str,
    selected: bool,
    enabled: bool,
) -> Response {
    ui.add_enabled(
        enabled,
        Tool {
            icon,
            label,
            tip,
            selected,
        },
    )
    .on_hover_text(tip)
}

struct Tool<'a> {
    icon: Icon,
    label: &'a str,
    tip: &'a str,
    selected: bool,
}
impl egui::Widget for Tool<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let font = egui::TextStyle::Button.resolve(ui.style());
        let pitch = font.size + 18.0;
        let galley = ui
            .painter()
            .layout_no_wrap(self.label.into(), font, Color32::PLACEHOLDER);
        let width = pitch
            + if self.label.is_empty() {
                0.0
            } else {
                galley.size().x + 8.0
            };
        let (rect, response) = ui.allocate_exact_size(vec2(width, pitch), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Button,
                ui.is_enabled(),
                self.selected,
                self.tip,
            )
        });
        let visuals = ui.style().interact_selectable(&response, self.selected);
        if self.selected
            || response.hovered()
            || response.has_focus()
            || response.is_pointer_button_down_on()
        {
            ui.painter().rect_filled(
                rect.shrink(1.0),
                5.0,
                if self.selected {
                    ACCENT.gamma_multiply(0.38)
                } else {
                    visuals.bg_fill
                },
            );
        }
        let color = if ui.is_enabled() && self.selected {
            if ui.visuals().dark_mode {
                Color32::from_rgb(140, 235, 222)
            } else {
                Color32::from_rgb(0, 90, 90)
            }
        } else {
            visuals.text_color()
        };
        let side = pitch * 0.57;
        paint(
            ui,
            self.icon,
            Rect::from_center_size(
                pos2(rect.left() + pitch / 2.0, rect.center().y),
                Vec2::splat(side),
            ),
            color,
        );
        if !self.label.is_empty() {
            ui.painter().galley(
                pos2(rect.left() + pitch, rect.center().y - galley.size().y / 2.0),
                galley,
                color,
            );
        }
        response
    }
}

pub fn paint(ui: &Ui, icon: Icon, rect: Rect, color: Color32) {
    let p = |x: f32, y: f32| rect.min + vec2(x, y) * rect.size() / 24.0;
    let stroke = Stroke::new((rect.width() / 14.0).max(1.2), color);
    let line = |points: &[[f32; 2]]| {
        ui.painter().add(egui::Shape::line(
            points.iter().map(|[x, y]| p(*x, *y)).collect(),
            stroke,
        ));
    };
    let box_at = |x: f32, y: f32, w: f32, h: f32| {
        ui.painter().rect_stroke(
            Rect::from_min_max(p(x, y), p(x + w, y + h)),
            1.5,
            stroke,
            egui::StrokeKind::Inside,
        );
    };
    match icon {
        Icon::Open => {
            line(&[
                [3., 20.],
                [3., 5.],
                [10., 5.],
                [13., 8.],
                [21., 8.],
                [21., 11.],
            ]);
            line(&[[3., 20.], [7., 11.], [23., 11.], [19., 20.], [3., 20.]]);
        }
        Icon::Save => {
            line(&[
                [4., 3.],
                [17., 3.],
                [21., 7.],
                [21., 21.],
                [3., 21.],
                [3., 3.],
                [4., 3.],
            ]);
            box_at(7., 3., 9., 6.);
            box_at(7., 14., 10., 7.);
        }
        Icon::Undo | Icon::Redo => {
            let flip = matches!(icon, Icon::Redo);
            let q = |x| if flip { 24. - x } else { x };
            line(&[[q(8.), 5.], [q(3.), 10.], [q(8.), 15.]]);
            line(&[
                [q(3.), 10.],
                [q(14.), 10.],
                [q(19.), 12.],
                [q(21.), 16.],
                [q(20.), 20.],
            ]);
        }
        Icon::SideBySide => {
            box_at(2., 4., 20., 16.);
            line(&[[12., 4.], [12., 20.]]);
        }
        Icon::Overlay => {
            box_at(3., 3., 13., 13.);
            box_at(9., 9., 13., 13.);
        }
        Icon::Mat => {
            box_at(3., 2., 18., 20.);
            for x in [9., 15.] {
                line(&[[x, 5.], [x, 19.]]);
            }
            for y in [8., 14.] {
                line(&[[6., y], [18., y]]);
            }
        }
        Icon::Minus | Icon::Plus => {
            line(&[[5., 12.], [19., 12.]]);
            if matches!(icon, Icon::Plus) {
                line(&[[12., 5.], [12., 19.]]);
            }
        }
        Icon::Fit => {
            for (x, y, sx, sy) in [
                (3., 3., 1., 1.),
                (21., 3., -1., 1.),
                (3., 21., 1., -1.),
                (21., 21., -1., -1.),
            ] {
                line(&[[x, y + 6. * sy], [x, y], [x + 6. * sx, y]]);
            }
        }
        Icon::Exit => {
            box_at(14., 3., 7., 18.);
            line(&[[16., 12.], [3., 12.]]);
            line(&[[7., 8.], [3., 12.], [7., 16.]]);
        }
        Icon::TestCut => {
            box_at(3., 3., 18., 18.);
            line(&[[7., 17.], [12., 7.], [17., 17.], [7., 17.]]);
        }
        Icon::Cut => {
            for y in [6., 18.] {
                ui.painter()
                    .circle_stroke(p(5., y), rect.width() * 0.14, stroke);
            }
            line(&[[8., 8.], [21., 20.]]);
            line(&[[8., 16.], [21., 4.]]);
        }
        Icon::Stop => {
            ui.painter()
                .rect_filled(Rect::from_min_max(p(5., 5.), p(19., 19.)), 2.0, color);
        }
        Icon::More => {
            for x in [5., 12., 19.] {
                ui.painter()
                    .circle_filled(p(x, 12.), rect.width() * 0.07, color);
            }
        }
    }
}

/// The right panel draws one group at a time: while this names a group, every other section
/// is skipped, and the drawn one gets a lock and a drag handle on its header.
#[derive(Clone, Copy)]
pub struct PanelPass {
    pub group: &'static str,
    pub locked: bool,
    /// Whether they left it open or closed; none keeps the group's usual state.
    pub open: Option<bool>,
}

/// What they did to a group's header this frame, for the panel to act on.
#[derive(Clone, Debug)]
pub enum PanelEvent {
    ToggleLock(String),
    /// They opened or closed a group.
    Open(String, bool),
    /// Drop `group` before or after `target`.
    Move {
        group: String,
        target: String,
        after: bool,
    },
}

fn pass_id() -> egui::Id {
    egui::Id::new("panel-pass")
}

fn events_id() -> egui::Id {
    egui::Id::new("panel-events")
}

pub fn set_panel_pass(ui: &Ui, pass: Option<PanelPass>) {
    ui.data_mut(|d| match pass {
        Some(pass) => {
            d.insert_temp(pass_id(), pass);
        }
        None => d.remove::<PanelPass>(pass_id()),
    });
}

pub fn take_panel_events(ui: &Ui) -> Vec<PanelEvent> {
    ui.data_mut(|d| d.remove_temp::<Vec<PanelEvent>>(events_id()))
        .unwrap_or_default()
}

fn push_panel_event(ui: &Ui, event: PanelEvent) {
    ui.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<PanelEvent>>(events_id())
            .push(event)
    });
}

/// A filled warning triangle with an exclamation mark, in `rect`.
pub fn alert_triangle(ui: &Ui, rect: Rect, color: Color32) {
    let r = rect.shrink(1.0);
    let points = vec![
        pos2(r.center().x, r.top()),
        pos2(r.right(), r.bottom()),
        pos2(r.left(), r.bottom()),
    ];
    ui.painter()
        .add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
    let x = r.center().x;
    let mark = Stroke::new(r.width() * 0.12, Color32::WHITE);
    ui.painter().line_segment(
        [
            pos2(x, r.top() + r.height() * 0.35),
            pos2(x, r.top() + r.height() * 0.68),
        ],
        mark,
    );
    ui.painter().circle_filled(
        pos2(x, r.top() + r.height() * 0.83),
        r.width() * 0.07,
        Color32::WHITE,
    );
}

/// A padlock, closed or open, in `rect`.
fn padlock(ui: &Ui, rect: Rect, closed: bool, color: Color32) {
    let c = rect.center();
    let body = Rect::from_center_size(c + vec2(0.0, 3.0), vec2(10.0, 7.0));
    ui.painter().rect_filled(body, 1.5, color);
    let lift = if closed { 0.0 } else { 3.0 };
    let shackle: Vec<Pos2> = (0..=12)
        .map(|i| {
            let a = std::f32::consts::PI * (1.0 + i as f32 / 12.0);
            c + vec2(3.2 * a.cos(), -0.5 - lift + 3.5 * a.sin())
        })
        .collect();
    let stroke = Stroke::new(1.5, color);
    ui.painter().add(egui::Shape::line(shackle, stroke));
    ui.painter()
        .line_segment([c + vec2(3.2, -0.5 - lift), c + vec2(3.2, 0.0)], stroke);
    if closed {
        ui.painter()
            .line_segment([c + vec2(-3.2, -0.5), c + vec2(-3.2, 0.0)], stroke);
    }
}

/// Header colours for a closed group that needs their: something blocks the cut, or asks.
pub const ALERT: Color32 = Color32::from_rgb(200, 60, 50);
pub const CAUTION: Color32 = Color32::from_rgb(214, 160, 30);

/// Panel sections have a full-width hit target and a padded body, without tree guide lines.
pub fn section(ui: &mut Ui, title: &str, default_open: bool, body: impl FnOnce(&mut Ui)) {
    flagged_section(ui, title, default_open, None, body);
}

/// A section whose header turns `attention`'s colour while it is closed.
pub fn flagged_section(
    ui: &mut Ui,
    title: &str,
    default_open: bool,
    attention: Option<Color32>,
    body: impl FnOnce(&mut Ui),
) {
    let key = title.split('·').next().unwrap_or(title).trim().to_owned();
    let pass = ui.data(|d| d.get_temp::<PanelPass>(pass_id()));
    if pass.is_some_and(|p| p.group != key) {
        return;
    }
    ui.add_space(6.0);
    let dark = ui.visuals().dark_mode;
    let framed = egui::Frame::new()
        .fill(if dark {
            Color32::from_rgb(30, 32, 34)
        } else {
            Color32::from_rgb(248, 249, 250)
        })
        .stroke(Stroke::new(
            1.0,
            if dark {
                Color32::from_rgb(47, 50, 53)
            } else {
                Color32::from_rgb(215, 219, 222)
            },
        ))
        .corner_radius(5.0)
        .show(ui, |ui| {
            let id = ui.make_persistent_id(("section", key.as_str()));
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                default_open,
            );
            // Their remembered choice, applied when it changes (or the group moved and its
            // state was made afresh), so their click this frame isn't undone before it's saved.
            let applied = id.with("applied");
            if let Some(pass) = pass
                && ui.data(|d| d.get_temp::<Option<bool>>(applied)) != Some(pass.open)
            {
                if let Some(open) = pass.open {
                    state.set_open(open);
                }
                ui.data_mut(|d| d.insert_temp(applied, pass.open));
            }
            let font = egui::TextStyle::Button.resolve(ui.style());
            let height = font.size + 18.0;
            let (rect, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::CollapsingHeader,
                    ui.is_enabled(),
                    state.is_open(),
                    title,
                )
            });
            // The lock and drag handle sit on top of the header, so they take its clicks.
            let controls = pass.map(|pass| {
                let handle_rect = Rect::from_center_size(
                    pos2(rect.right() - 16.0, rect.center().y),
                    vec2(22.0, height),
                );
                let lock_rect = Rect::from_center_size(
                    pos2(rect.right() - 46.0, rect.center().y),
                    vec2(30.0, height),
                );
                let lock = ui
                    .interact(lock_rect, id.with("lock"), Sense::click())
                    .on_hover_text(if pass.locked {
                        "Unlock: let this group scroll with the others"
                    } else {
                        "Lock: keep this group at the top, out of the scrolling"
                    });
                let handle = ui
                    .interact(handle_rect, id.with("drag"), Sense::drag())
                    .on_hover_cursor(egui::CursorIcon::Grab)
                    .on_hover_text("Drag to move this group");
                handle.dnd_set_drag_payload(key.clone());
                if handle.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
                (pass, lock, lock_rect, handle, handle_rect)
            });
            if response.clicked() {
                state.toggle(ui);
                if pass.is_some() {
                    let open = state.is_open();
                    ui.data_mut(|d| d.insert_temp(applied, Some(open)));
                    push_panel_event(ui, PanelEvent::Open(key.clone(), open));
                }
            }
            let flag = attention.filter(|_| !state.is_open());
            let fill = if let Some(colour) = flag {
                colour.gamma_multiply(if response.hovered() { 0.75 } else { 0.55 })
            } else if response.hovered() {
                ui.visuals().widgets.hovered.weak_bg_fill
            } else if ui.visuals().dark_mode {
                Color32::from_rgb(43, 46, 49)
            } else {
                Color32::from_rgb(230, 233, 235)
            };
            ui.painter().rect_filled(rect, 4.0, fill);
            let center = pos2(rect.left() + 14.0, rect.center().y);
            let points: Vec<Pos2> = if state.is_open() {
                vec![
                    center + vec2(-4., -2.),
                    center + vec2(0., 2.),
                    center + vec2(4., -2.),
                ]
            } else {
                vec![
                    center + vec2(-2., -4.),
                    center + vec2(2., 0.),
                    center + vec2(-2., 4.),
                ]
            };
            ui.painter().add(egui::Shape::line(
                points,
                Stroke::new(1.5, ui.visuals().text_color()),
            ));
            ui.painter().text(
                pos2(rect.left() + 29.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                title,
                font,
                ui.visuals().strong_text_color(),
            );
            if let Some((pass, lock, lock_rect, handle, handle_rect)) = controls {
                let weak = ui.visuals().weak_text_color();
                let strong = ui.visuals().strong_text_color();
                let lock_color = if pass.locked || lock.hovered() {
                    strong
                } else {
                    weak
                };
                // An up-and-down arrow beside the padlock: it locks the scrolling, not the settings.
                padlock(
                    ui,
                    lock_rect.translate(vec2(5.0, 0.0)),
                    pass.locked,
                    lock_color,
                );
                let x = lock_rect.center().x - 8.0;
                let y = lock_rect.center().y;
                let stroke = Stroke::new(1.3, lock_color);
                ui.painter()
                    .line_segment([pos2(x, y - 6.0), pos2(x, y + 6.0)], stroke);
                for (tip, dir) in [(y - 6.0, 1.0), (y + 6.0, -1.0)] {
                    ui.painter().add(egui::Shape::line(
                        vec![
                            pos2(x - 2.5, tip + 2.5 * dir),
                            pos2(x, tip),
                            pos2(x + 2.5, tip + 2.5 * dir),
                        ],
                        stroke,
                    ));
                }
                if lock.clicked() {
                    push_panel_event(ui, PanelEvent::ToggleLock(key.clone()));
                }
                let dots = if handle.hovered() || handle.dragged() {
                    strong
                } else {
                    weak
                };
                for dx in [-2.5, 2.5] {
                    for dy in [-5.0, 0.0, 5.0] {
                        ui.painter()
                            .circle_filled(handle_rect.center() + vec2(dx, dy), 1.4, dots);
                    }
                }
            }
            state.show_body_unindented(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(10, 10))
                    .show(ui, body);
            });
        });
    // Another group dragged over this one: show where it lands, and move it there on release.
    if pass.is_some() {
        let area = framed.response.rect;
        let after = ui
            .ctx()
            .pointer_interact_pos()
            .is_some_and(|p| p.y > area.center().y);
        if let Some(group) = framed.response.dnd_hover_payload::<String>()
            && *group != key
        {
            let y = if after {
                area.bottom() + 3.0
            } else {
                area.top() - 3.0
            };
            ui.painter()
                .hline(area.x_range(), y, Stroke::new(2.0, ACCENT));
        }
        if let Some(group) = framed.response.dnd_release_payload::<String>()
            && *group != key
        {
            push_panel_event(
                ui,
                PanelEvent::Move {
                    group: (*group).clone(),
                    target: key,
                    after,
                },
            );
        }
    }
}

/// Width-based toolbar decisions keep the bar on one line; hidden controls live in More.
#[derive(Debug, PartialEq)]
pub struct ToolbarLayout {
    pub compact: bool,
    pub history: bool,
    pub zoom: bool,
    pub labels: bool,
}
impl ToolbarLayout {
    pub fn new(width: f32, font_size: f32) -> Self {
        let pitch = font_size + 18.0;
        // Keep room for the labelled Test Cut control beside the primary Cut button.
        let width = width - (pitch + font_size * 4.5 + 13.0);
        Self {
            compact: width < pitch * 9.0 + 100.0,
            history: width >= pitch * 11.0 + 80.0,
            zoom: width >= pitch * 14.0 + 80.0,
            labels: width >= pitch * 14.0 + font_size * 19.0 + 80.0,
        }
    }
}

pub fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(9.0, egui::TextStyle::Button.resolve(ui.style()).size + 10.0),
        Sense::hover(),
    );
    ui.painter().vline(
        rect.center().x,
        rect.y_range().shrink(3.0),
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
}

/// Modal dialogs use window padding, rather than egui's compact popup/menu frame.
pub fn dialog_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::window(&ctx.global_style())
        .inner_margin(ctx.global_style().spacing.window_margin)
        .corner_radius(8.0)
}
