//! The slice's screen: sign in, open a design, the artwork with its cut lines (a server picture
//! under locally drawn vectors), the Cut preview with a draggable design, the material, and a
//! lost reply recovered by its key.

use std::cell::RefCell;
use std::rc::Rc;

use cutcraft_web_protocol::{
    CommandReply, CommandRequest, Geometry, KeyLookup, PictureRequest, SessionSnapshot,
};
use eframe::egui;
use serde_json::{Value, json};

use crate::net;
use crate::state;
use crate::view::{self, Viewport};

/// How long a dropped design waits for its new plan before snapping back (the desktop's rule).
const HOLD_MS: f64 = 2000.0;
/// A view must rest this long before a new picture is asked for (the desktop's SETTLE).
const SETTLE_MS: f64 = 120.0;

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Overlay,
    Mat,
}

struct Picture {
    key: String,
    /// The artwork it shows ([`ArtView::id`](cutcraft_web_protocol::ArtView)).
    art: String,
    /// In the design's own mm.
    region: [f64; 4],
    texture: egui::TextureHandle,
}

#[derive(Default)]
struct Shared {
    signed_in: Option<bool>,
    login_error: Option<String>,
    live: bool,
    snapshot: Option<SessionSnapshot>,
    geometry: Option<Geometry>,
    geometry_loading: Option<String>,
    picture: Option<Picture>,
    picture_loading: Option<String>,
    /// The last few things that happened, newest last.
    log: Vec<String>,
    command_ms: Option<f64>,
    picture_ms: Option<f64>,
    geometry_ms: Option<f64>,
    geometry_bytes: usize,
    upload_ms: Option<f64>,
    /// When the latest geometry arrived, for timing a drop to its new plan.
    geometry_at: Option<f64>,
    /// When the latest snapshot arrived.
    snapshot_at: Option<f64>,
    lost_reply: Option<String>,
    busy: Option<String>,
    /// The server speaks another protocol version: the page must reload (spec §13).
    reload: bool,
}

impl Shared {
    fn note(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > 8 {
            self.log.remove(0);
        }
    }
}

pub struct App {
    shared: Rc<RefCell<Shared>>,
    ctx: egui::Context,
    password: String,
    screen: Screen,
    overlay: Viewport,
    fitted: Option<String>,
    changed_at: f64,
    /// The design dragged on the mat, in mm, until dropped.
    dragging: Option<[f64; 2]>,
    /// A dropped drag shown until the new plan arrives: the move, when, and the plan it was on.
    held: Option<([f64; 2], f64, String)>,
    material: String,
    /// Where the views were last drawn, for the browser checks to aim at.
    mat_view: Option<(view::Rect, Viewport)>,
    overlay_view: Option<(view::Rect, Viewport)>,
    /// When the app first drew, in ms since the page started loading.
    ready_at: Option<f64>,
    /// The last drop: press and release points, and the move sent, for the browser checks.
    last_drop: Option<Value>,
    drag_start: Option<Value>,
    drop_at: Option<f64>,
    /// When the plan was last asked for again ([`App::retry_geometry`]).
    geometry_tried: f64,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let app = Self {
            shared: Rc::default(),
            ctx: cc.egui_ctx.clone(),
            password: String::new(),
            screen: Screen::Mat,
            overlay: Viewport::default(),
            fitted: None,
            changed_at: 0.0,
            dragging: None,
            held: None,
            material: String::new(),
            mat_view: None,
            overlay_view: None,
            ready_at: None,
            last_drop: None,
            drag_start: None,
            drop_at: None,
            geometry_tried: 0.0,
        };
        app.check_signed_in();
        app
    }

    fn spawn(&self, task: impl std::future::Future<Output = ()> + 'static) {
        wasm_bindgen_futures::spawn_local(task);
    }

    fn check_signed_in(&self) {
        let (shared, ctx) = (self.shared.clone(), self.ctx.clone());
        self.spawn(async move {
            match net::request("GET", "/api/snapshot", None, None, &[]).await {
                Ok(a) if a.status == 401 => shared.borrow_mut().signed_in = Some(false),
                Ok(a) if state::protocol_refused(a.status, &a.body) => {
                    shared.borrow_mut().reload = true
                }
                Ok(a) if a.status == 200 => {
                    shared.borrow_mut().signed_in = Some(true);
                    if let Ok(text) = String::from_utf8(a.body) {
                        apply_snapshot(&shared, &ctx, &text);
                    }
                    follow(shared.clone(), ctx.clone());
                }
                Ok(a) => shared
                    .borrow_mut()
                    .note(format!("snapshot: HTTP {}", a.status)),
                Err(e) => shared.borrow_mut().note(format!("snapshot: {e}")),
            }
            ctx.request_repaint();
        });
    }

    fn login(&self) {
        let (shared, ctx) = (self.shared.clone(), self.ctx.clone());
        let body = json!({"password": self.password}).to_string();
        let again = Self::checker(shared.clone(), ctx.clone());
        self.spawn(async move {
            match net::request(
                "POST",
                "/api/login",
                Some(body.as_bytes()),
                Some("application/json"),
                &[],
            )
            .await
            {
                Ok(a) if a.status == 200 => again(),
                Ok(a) => {
                    shared.borrow_mut().login_error =
                        Some(String::from_utf8_lossy(&a.body).into_owned())
                }
                Err(e) => shared.borrow_mut().login_error = Some(e),
            }
            ctx.request_repaint();
        });
    }

    fn checker(shared: Rc<RefCell<Shared>>, ctx: egui::Context) -> impl Fn() + 'static {
        move || {
            let (shared, ctx) = (shared.clone(), ctx.clone());
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(a) = net::request("GET", "/api/snapshot", None, None, &[]).await
                    && a.status == 200
                {
                    shared.borrow_mut().signed_in = Some(true);
                    if let Ok(text) = String::from_utf8(a.body) {
                        apply_snapshot(&shared, &ctx, &text);
                    }
                    follow(shared.clone(), ctx.clone());
                }
                ctx.request_repaint();
            });
        }
    }

    fn open_file(&self) {
        let (shared, ctx) = (self.shared.clone(), self.ctx.clone());
        net::pick_file(Rc::new(move |name, bytes| {
            upload(shared.clone(), ctx.clone(), name, bytes)
        }));
    }

    /// One change: a fresh key, the incarnation and the revision it was made against.
    fn command(&self, id: &str, params: Value, lose_reply: bool) {
        let Some(s) = self.shared.borrow().snapshot.clone() else {
            return;
        };
        let request = CommandRequest {
            id: id.into(),
            params,
            key: net::new_key(),
            incarnation: s.incarnation.clone(),
            revision: Some(s.revision),
        };
        let (shared, ctx) = (self.shared.clone(), self.ctx.clone());
        self.spawn(async move {
            let body = serde_json::to_vec(&request).unwrap_or_default();
            let extra: &[(&str, &str)] = if lose_reply {
                &[("x-m0-drop-reply", "1")]
            } else {
                &[]
            };
            let answer = net::request(
                "POST",
                "/api/command",
                Some(&body),
                Some("application/json"),
                extra,
            )
            .await;
            match answer {
                Ok(a) if a.status == 200 => {
                    let mut sh = shared.borrow_mut();
                    sh.command_ms = Some(a.ms);
                    match serde_json::from_slice::<CommandReply>(&a.body) {
                        Ok(CommandReply::Applied { revision, .. }) => sh.note(format!(
                            "{} applied (revision {revision}, {:.0} ms)",
                            request.id, a.ms
                        )),
                        Ok(CommandReply::Refused { code, message }) => {
                            sh.note(format!("{} refused: {code}: {message}", request.id))
                        }
                        Err(e) => sh.note(format!("{}: unreadable answer {e}", request.id)),
                    }
                }
                // Another protocol version: the page must reload (Grok's re-review).
                Ok(a) if state::protocol_refused(a.status, &a.body) => {
                    shared.borrow_mut().reload = true;
                }
                // The answer was lost (or never came): ask by key, never send it again blindly.
                other => {
                    let why = match other {
                        Ok(a) => format!("HTTP {}", a.status),
                        Err(e) => e,
                    };
                    shared
                        .borrow_mut()
                        .note(format!("{}: no answer ({why}); asking by key", request.id));
                    let url = format!("/api/lookup/{}", request.key);
                    let found = net::request("GET", &url, None, None, &[]).await;
                    let text = match found
                        .ok()
                        .and_then(|a| serde_json::from_slice::<KeyLookup>(&a.body).ok())
                    {
                        Some(KeyLookup::Applied { revision, .. }) => {
                            format!("found by key: applied at revision {revision}")
                        }
                        Some(KeyLookup::Refused { code, .. }) => {
                            format!("found by key: refused ({code})")
                        }
                        Some(KeyLookup::NotApplied) => {
                            "found by key: not applied, safe to send again".into()
                        }
                        None => "the lookup failed too".into(),
                    };
                    shared.borrow_mut().note(text.clone());
                    // The page shows what the server has now, not only a note: fetch the
                    // snapshot (and so the plan) rather than trusting the live push.
                    if let Ok(a) = net::request("GET", "/api/snapshot", None, None, &[]).await
                        && a.status == 200
                        && let Ok(snap) = String::from_utf8(a.body)
                    {
                        apply_snapshot(&shared, &ctx, &snap);
                    }
                    shared.borrow_mut().lost_reply = Some(text);
                }
            }
            ctx.request_repaint();
        });
    }

    fn report(&self) {
        let sh = self.shared.borrow();
        let s = sh.snapshot.as_ref();
        let design = s.and_then(|s| s.design.as_ref());
        let state = json!({
            "signed_in": sh.signed_in,
            "live": sh.live,
            "revision": s.map(|s| s.revision),
            "design": design.map(|d| d.name.clone()),
            "cut_ready": design.map(|d| d.cut_ready),
            "material": design.and_then(|d| d.sheet.as_ref()).map(|s| s.material.clone()),
            "geometry": sh.geometry.as_ref().map(|g| g.id.clone()),
            // What the mat may draw: only the plan the latest snapshot asks for.
            "drawn_geometry": sh
                .geometry
                .as_ref()
                .filter(|g| state::geometry_is_current(sh.snapshot.as_ref(), g))
                .map(|g| g.id.clone()),
            "art_box_mm": sh.geometry.as_ref().and_then(|g| g.art_box_mm),
            "picture": sh.picture.as_ref().map(|p| p.key.clone()),
            "lost_reply": sh.lost_reply,
            "command_ms": sh.command_ms,
            "picture_ms": sh.picture_ms,
            "geometry_ms": sh.geometry_ms,
            "geometry_bytes": sh.geometry_bytes,
            "upload_ms": sh.upload_ms,
            "log": sh.log,
            "ready_at_ms": self.ready_at,
            "screen": if self.screen == Screen::Mat { "mat" } else { "overlay" },
            "mat_view": self.mat_view.map(|(r, v)| json!({"rect": r, "center": v.center, "zoom": v.zoom})),
            "overlay_view": self.overlay_view.map(|(r, v)| json!({"rect": r, "center": v.center, "zoom": v.zoom})),
            "picture_region": sh.picture.as_ref().map(|p| p.region),
            "materials": s.map(|s| s.materials.len()),
            "last_drop": self.last_drop,
            "drag_start": self.drag_start,
            "drop_to_snapshot_ms": self.drop_at.zip(sh.snapshot_at).filter(|(d, s)| s > d).map(|(d, s)| s - d),
            "drop_to_plan_ms": self.drop_at.zip(sh.geometry_at).filter(|(d, g)| g > d).map(|(d, g)| g - d),
            "pixels_per_point": self.ctx.pixels_per_point(),
            "reload": sh.reload,
        });
        net::report(&state.to_string());
    }
}

fn apply_snapshot(shared: &Rc<RefCell<Shared>>, ctx: &egui::Context, text: &str) {
    let Ok(snapshot) = serde_json::from_str::<SessionSnapshot>(text) else {
        return;
    };
    let mut sh = shared.borrow_mut();
    if !state::snapshot_is_newer(sh.snapshot.as_ref(), &snapshot) {
        return;
    }
    if state::needs_reload(&snapshot) {
        sh.reload = true;
    }
    let wanted = state::wanted_geometry(&snapshot).map(str::to_owned);
    if wanted.is_none() {
        // No design (or a fresh worker): nothing of the old plan stays on the mat.
        sh.geometry = None;
    }
    sh.snapshot = Some(snapshot);
    sh.snapshot_at = Some(net::now_ms());
    let have = sh.geometry.as_ref().map(|g| g.id.clone());
    if let Some(id) = wanted
        && have.as_ref() != Some(&id)
        && sh.geometry_loading.as_ref() != Some(&id)
    {
        sh.geometry_loading = Some(id.clone());
        drop(sh);
        let (shared, ctx) = (shared.clone(), ctx.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let answer = net::request("GET", "/api/geometry", None, None, &[]).await;
            if let Ok(a) = &answer
                && state::protocol_refused(a.status, &a.body)
            {
                shared.borrow_mut().reload = true;
            }
            if let Ok(a) = answer
                && a.status == 200
                && let Ok(g) = serde_json::from_slice::<Geometry>(&a.body)
            {
                let mut sh = shared.borrow_mut();
                // Only the plan the latest snapshot asks for: a late answer about an older
                // one is dropped (Grok's review of milestone 0).
                if state::geometry_is_current(sh.snapshot.as_ref(), &g) {
                    sh.geometry_ms = Some(a.ms);
                    sh.geometry_bytes = a.body.len();
                    sh.geometry = Some(g);
                    sh.geometry_at = Some(net::now_ms());
                }
            }
            let mut sh = shared.borrow_mut();
            if sh.geometry_loading.as_deref() == Some(id.as_str()) {
                sh.geometry_loading = None;
            }
            // If the snapshot moved on meanwhile, fetch what it asks for now.
            let again = sh
                .snapshot
                .as_ref()
                .and_then(state::wanted_geometry)
                .map(str::to_owned);
            let stale = again.is_some()
                && again.as_deref() != Some(id.as_str())
                && sh.geometry_loading.is_none()
                && sh.geometry.as_ref().map(|g| &g.id) != again.as_ref();
            let text = stale
                .then(|| {
                    sh.snapshot
                        .as_ref()
                        .and_then(|s| serde_json::to_string(s).ok())
                })
                .flatten();
            drop(sh);
            if let Some(text) = text {
                apply_snapshot(&shared, &ctx, &text);
            }
            ctx.request_repaint();
        });
    }
}

fn follow(shared: Rc<RefCell<Shared>>, ctx: egui::Context) {
    let (s2, c2) = (shared.clone(), ctx.clone());
    net::follow(
        Rc::new(move |text| {
            apply_snapshot(&shared, &ctx, &text);
            ctx.request_repaint();
        }),
        Rc::new(move |up| {
            s2.borrow_mut().live = up;
            c2.request_repaint();
        }),
    );
}

fn upload(shared: Rc<RefCell<Shared>>, ctx: egui::Context, name: String, bytes: Vec<u8>) {
    shared.borrow_mut().busy = Some(format!("Opening {name}…"));
    ctx.request_repaint();
    wasm_bindgen_futures::spawn_local(async move {
        let encoded: String = js_sys::encode_uri_component(&name).into();
        let url = format!("/api/upload?name={encoded}");
        let answer = net::request(
            "POST",
            &url,
            Some(&bytes),
            Some("application/octet-stream"),
            &[],
        )
        .await;
        let mut sh = shared.borrow_mut();
        sh.busy = None;
        match answer {
            Ok(a) if a.status == 200 => {
                sh.upload_ms = Some(a.ms);
                sh.note(format!(
                    "opened {name} ({} KB, {:.0} ms)",
                    bytes.len() / 1024,
                    a.ms
                ));
                drop(sh);
                if let Ok(text) = String::from_utf8(a.body) {
                    apply_snapshot(&shared, &ctx, &text);
                }
            }
            Ok(a) => sh.note(format!("{name}: {}", String::from_utf8_lossy(&a.body))),
            Err(e) => sh.note(format!("{name}: {e}")),
        }
        ctx.request_repaint();
    });
}

fn colour(hex: &str) -> egui::Color32 {
    let h = hex.trim_start_matches('#');
    let byte = |i: usize| {
        h.get(i..i + 2)
            .and_then(|s| u8::from_str_radix(s, 16).ok())
            .unwrap_or(0)
    };
    if h.len() >= 6 {
        egui::Color32::from_rgb(byte(0), byte(2), byte(4))
    } else {
        egui::Color32::BLACK
    }
}

fn rect_of(r: egui::Rect) -> view::Rect {
    [r.min.x, r.min.y, r.max.x, r.max.y]
}

fn pos(p: [f32; 2]) -> egui::Pos2 {
    egui::pos2(p[0], p[1])
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.ready_at.get_or_insert_with(net::now_ms);
        self.report();
        if self.shared.borrow().reload {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.heading("CutCraft was updated. Reload this page.");
                });
            });
            return;
        }
        let signed_in = self.shared.borrow().signed_in;
        if signed_in != Some(true) {
            egui::CentralPanel::default().show(ui, |ui| self.sign_in(ui, signed_in));
            return;
        }
        // A design dropped onto the window opens like a picked one.
        let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
        for file in dropped {
            let (shared, ctx) = (self.shared.clone(), self.ctx.clone());
            self.spawn(async move {
                let name = file
                    .path()
                    .file_name()
                    .map_or("design".into(), |n| n.to_string_lossy().into_owned());
                match file.bytes_async().await {
                    Ok(bytes) => upload(shared, ctx, name, bytes),
                    Err(e) => shared.borrow_mut().note(format!("{name}: {e}")),
                }
            });
        }
        self.keys(ui);
        self.retry_geometry();
        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).inner_margin(0.0))
            .show(ui, |ui| match self.screen {
                Screen::Overlay => self.overlay(ui),
                Screen::Mat => self.mat(ui),
            });
    }
}

impl App {
    /// Milestone 0's keys, so the browser checks can drive what the canvas hides: O and C switch
    /// views, M picks the next material, L runs the lost-reply test.
    fn keys(&mut self, ui: &mut egui::Ui) {
        let pressed = |k| ui.input(|i| i.key_pressed(k));
        if pressed(egui::Key::O) {
            self.screen = Screen::Overlay;
        }
        if pressed(egui::Key::C) {
            self.screen = Screen::Mat;
        }
        if pressed(egui::Key::M) {
            let next = {
                let sh = self.shared.borrow();
                sh.snapshot.as_ref().and_then(|s| {
                    let current = s.design.as_ref()?.sheet.as_ref()?.material.clone();
                    let i = s.materials.iter().position(|m| *m == current)?;
                    s.materials.get((i + 1) % s.materials.len()).cloned()
                })
            };
            if let Some(m) = next {
                self.command("sheet.set", json!({"material": m}), false);
            }
        }
        if pressed(egui::Key::L) {
            self.lose_a_reply();
        }
    }

    fn lose_a_reply(&self) {
        let geometry = self.shared.borrow().geometry.clone();
        if let Some(g) = geometry
            && let (Some(b), Some(m)) = (g.art_box_mm, g.mat.as_ref())
        {
            let [x, y] = view::place_params(b, m.origin_mm, [5.0, 0.0]);
            self.command("sheet.place", json!({"x": x, "y": y}), true);
        }
    }

    fn sign_in(&mut self, ui: &mut egui::Ui, state: Option<bool>) {
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);
            ui.heading("CutCraft-Web · milestone 0");
            ui.add_space(16.0);
            if state.is_none() {
                ui.label("Connecting…");
                return;
            }
            ui.label("Password");
            let field = ui.add(egui::TextEdit::singleline(&mut self.password).password(true));
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Sign in").clicked() || enter {
                self.login();
            }
            if let Some(e) = &self.shared.borrow().login_error {
                ui.colored_label(egui::Color32::RED, e);
            }
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Open a design…").clicked() {
                self.open_file();
            }
            ui.separator();
            ui.selectable_value(&mut self.screen, Screen::Overlay, "Overlay");
            ui.selectable_value(&mut self.screen, Screen::Mat, "Cut preview");
            ui.separator();
            let (materials, current) = {
                let sh = self.shared.borrow();
                let s = sh.snapshot.as_ref();
                (
                    s.map(|s| s.materials.clone()).unwrap_or_default(),
                    s.and_then(|s| s.design.as_ref())
                        .and_then(|d| d.sheet.as_ref())
                        .map(|s| s.material.clone()),
                )
            };
            if let Some(current) = current {
                if self.material.is_empty() {
                    self.material = current.clone();
                }
                let mut chosen = current.clone();
                egui::ComboBox::from_id_salt("material")
                    .selected_text(&chosen)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        for m in &materials {
                            ui.selectable_value(&mut chosen, m.clone(), m);
                        }
                    });
                if chosen != current {
                    self.command("sheet.set", json!({"material": chosen}), false);
                }
                ui.separator();
                if ui
                    .button("Test a lost reply")
                    .on_hover_text("Moves the design 5 mm right; the server makes the change but drops the answer, and the page asks by key.")
                    .clicked()
                {
                    self.lose_a_reply();
                }
            }
        });
    }

    fn status(&mut self, ui: &mut egui::Ui) {
        let sh = self.shared.borrow();
        ui.horizontal_wrapped(|ui| {
            ui.label(if sh.live {
                "● live"
            } else {
                "○ reconnecting"
            });
            if let Some(s) = &sh.snapshot {
                ui.label(format!("revision {}", s.revision));
                if let Some(d) = &s.design {
                    ui.label(format!(
                        "{} · {}",
                        d.name,
                        if d.cut_ready {
                            "cut-ready"
                        } else {
                            "needs answers"
                        }
                    ));
                    if let Some(sheet) = &d.sheet {
                        ui.label(format!(
                            "{} · {}",
                            sheet.mat,
                            sheet
                                .estimate_s
                                .map_or(String::new(), |e| format!("about {e:.0} s"))
                        ));
                    }
                }
            }
            let ms = |v: Option<f64>| v.map_or("–".into(), |v| format!("{v:.0} ms"));
            ui.label(format!(
                "command {} · picture {} · geometry {} ({} KB) · upload {}",
                ms(sh.command_ms),
                ms(sh.picture_ms),
                ms(sh.geometry_ms),
                sh.geometry_bytes / 1024,
                ms(sh.upload_ms)
            ));
        });
        if let Some(last) = sh.log.last() {
            ui.label(egui::RichText::new(last).weak());
        }
    }

    /// A plan the snapshot wants but the page doesn't have, and isn't fetching (a failed
    /// fetch): asked for again, at most once a second (Grok's re-review).
    fn retry_geometry(&mut self) {
        let now = net::now_ms();
        let text = {
            let sh = self.shared.borrow();
            let wanted = sh.snapshot.as_ref().and_then(state::wanted_geometry);
            let missing = wanted.is_some()
                && sh.geometry_loading.is_none()
                && sh.geometry.as_ref().map(|g| g.id.as_str()) != wanted;
            if !missing || now - self.geometry_tried < 1000.0 {
                return;
            }
            sh.snapshot
                .as_ref()
                .and_then(|s| serde_json::to_string(s).ok())
        };
        self.geometry_tried = now;
        if let Some(text) = text {
            // The snapshot isn't newer than itself, so this only starts the fetch.
            apply_snapshot(&self.shared, &self.ctx, &text);
        }
        self.ctx
            .request_repaint_after(std::time::Duration::from_millis(1000));
    }

    fn placeholder(&self, ui: &mut egui::Ui, rect: egui::Rect) -> bool {
        let sh = self.shared.borrow();
        let text = if let Some(b) = &sh.busy {
            b.clone()
        } else if sh.snapshot.as_ref().is_none_or(|s| s.design.is_none()) {
            "Open a design, or drop one here.".into()
        } else if !sh
            .geometry
            .as_ref()
            .is_some_and(|g| state::geometry_is_current(sh.snapshot.as_ref(), g))
        {
            // Not drawn: no plan yet, or one for an older revision (Grok's re-review).
            "Planning…".into()
        } else {
            return false;
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(16.0),
            egui::Color32::GRAY,
        );
        true
    }

    /// The artwork as a server picture, with the cut lines drawn here every frame.
    fn overlay(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        ui.painter().rect_filled(rect, 0.0, egui::Color32::WHITE);
        if self.placeholder(ui, rect) {
            return;
        }
        let (art, geometry) = {
            let sh = self.shared.borrow();
            let art = sh
                .snapshot
                .as_ref()
                .and_then(|s| s.design.as_ref())
                .and_then(|d| d.art.clone());
            let current = sh
                .geometry
                .clone()
                .filter(|g| state::geometry_is_current(sh.snapshot.as_ref(), g));
            (art, current)
        };
        let (Some(art), Some(geometry)) = (art, geometry) else {
            return;
        };
        let r = rect_of(rect);
        let now = net::now_ms();
        // Fit once per design; then pan by dragging and zoom by wheel or pinch about the pointer.
        let fit_key = format!("{}:{:?}", art.design_revision, art.bounds);
        if self.fitted.as_ref() != Some(&fit_key) {
            self.overlay.fit(art.bounds, r);
            self.fitted = Some(fit_key);
            self.changed_at = now;
        }
        if response.dragged() {
            let d = response.drag_delta();
            self.overlay.center[0] -= f64::from(d.x) / self.overlay.zoom;
            self.overlay.center[1] -= f64::from(d.y) / self.overlay.zoom;
            self.changed_at = now;
        }
        if let Some(p) = response.hover_pos() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
            let factor = f64::from(zoom) * (f64::from(scroll.y) / 300.0).exp();
            if (factor - 1.0).abs() > 1e-4 {
                self.overlay.zoom_about(r, [p.x, p.y], factor);
                self.changed_at = now;
            }
        }
        let view = self.overlay;
        self.overlay_view = Some((r, view));
        // The picture: whatever we have, scaled to where it belongs, until a new one arrives.
        let painter = ui.painter_at(rect);
        if let Some(p) = self
            .shared
            .borrow()
            .picture
            .as_ref()
            .filter(|p| p.art == art.id)
        {
            let s = art.scale;
            let a = view.to_screen(r, [p.region[0] * s, p.region[1] * s]);
            let b = view.to_screen(r, [p.region[2] * s, p.region[3] * s]);
            painter.image(
                p.texture.id(),
                egui::Rect::from_two_pos(pos(a), pos(b)),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        for stroke in &geometry.lines {
            let points: Vec<egui::Pos2> = stroke
                .points
                .iter()
                .map(|p| pos(view.to_screen(r, [f64::from(p[0]), f64::from(p[1])])))
                .collect();
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(1.5, egui::Color32::from_rgb(230, 0, 120)),
            ));
        }
        if now - self.changed_at < SETTLE_MS {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(SETTLE_MS as u64));
            return;
        }
        let (region, width, height) =
            view::picture_for(view, r, ui.ctx().pixels_per_point(), art.scale);
        let request = PictureRequest {
            art: art.id.clone(),
            region,
            zoom_step: view.zoom_step(),
            width,
            height,
        };
        let key = request.query();
        let wanted = {
            let sh = self.shared.borrow();
            sh.picture.as_ref().map(|p| &p.key) != Some(&key) && sh.picture_loading.is_none()
        };
        if wanted {
            self.shared.borrow_mut().picture_loading = Some(key.clone());
            let (shared, ctx) = (self.shared.clone(), self.ctx.clone());
            self.spawn(async move {
                let url = format!("/api/picture?{key}");
                if let Ok(a) = net::request("GET", &url, None, None, &[]).await
                    && a.status == 200
                    && let Ok(img) =
                        image::load_from_memory_with_format(&a.body, image::ImageFormat::Png)
                {
                    let rgba = img.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let colour = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                    let texture = ctx.load_texture("artwork", colour, egui::TextureOptions::LINEAR);
                    let mut sh = shared.borrow_mut();
                    // Only for the artwork shown now: a late picture of another design, or of
                    // an older revision, is dropped (Codex's review of milestone 0).
                    if state::picture_is_current(sh.snapshot.as_ref(), &request.art) {
                        sh.picture_ms = Some(a.ms);
                        sh.picture = Some(Picture {
                            key,
                            art: request.art.clone(),
                            region: request.region,
                            texture,
                        });
                    }
                }
                shared.borrow_mut().picture_loading = None;
                ctx.request_repaint();
            });
        }
    }

    /// The sheet on its mat, drawn here from the geometry; dragging moves the design locally, and
    /// the drop sends one `sheet.place` (spec §12).
    fn mat(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        ui.painter()
            .rect_filled(rect, 0.0, egui::Color32::from_gray(235));
        if self.placeholder(ui, rect) {
            return;
        }
        let current = {
            let sh = self.shared.borrow();
            sh.geometry
                .clone()
                .filter(|g| state::geometry_is_current(sh.snapshot.as_ref(), g))
        };
        let Some(geometry) = current else {
            return;
        };
        let Some(mat) = geometry.mat.clone() else {
            return;
        };
        let r = rect_of(rect);
        let mut view = Viewport::default();
        view.fit([0.0, 0.0, mat.outer_mm[0], mat.outer_mm[1]], r);
        self.mat_view = Some((r, view));
        let now = net::now_ms();
        // A held drop lasts until a new plan arrives, or two seconds.
        if let Some((_, at, plan)) = &self.held
            && (*plan != geometry.id || now - at > HOLD_MS)
        {
            self.held = None;
        }
        let on_art = |p: egui::Pos2| {
            geometry.art_box_mm.is_some_and(|b| {
                let m = view.to_mm(r, [p.x, p.y]);
                m[0] >= b[0] && m[0] <= b[2] && m[1] >= b[1] && m[1] <= b[3]
            })
        };
        // Measured from where the button went down: egui only calls it a drag after a few
        // points, and adding up per-frame deltas would lose those.
        let press = ui.input(|i| i.pointer.press_origin());
        if response.drag_started() && press.is_some_and(on_art) {
            self.dragging = Some([0.0, 0.0]);
            self.drag_start = Some(json!({
                "press": press.map(|p| [p.x, p.y]),
                "zoom": view.zoom,
                "rect": r,
                "pixels_per_point": ui.ctx().pixels_per_point(),
                "screen": [ui.ctx().content_rect().width(), ui.ctx().content_rect().height()],
            }));
        }
        if let (Some(moved), Some(press), Some(now_at)) = (
            self.dragging.as_mut(),
            press,
            ui.input(|i| i.pointer.latest_pos()),
        ) {
            moved[0] = f64::from(now_at.x - press.x) / view.zoom;
            moved[1] = f64::from(now_at.y - press.y) / view.zoom;
        }
        if response.drag_stopped()
            && let Some(moved) = self.dragging.take()
            && let Some(b) = geometry.art_box_mm
        {
            let [x, y] = view::place_params(b, mat.origin_mm, moved);
            self.last_drop = Some(json!({
                "press": press.map(|p| [p.x, p.y]),
                "release": ui.input(|i| i.pointer.latest_pos()).map(|p| [p.x, p.y]),
                "moved_mm": moved,
                "zoom": view.zoom,
            }));
            self.drop_at = Some(now);
            self.command("sheet.place", json!({"x": x, "y": y}), false);
            self.held = Some((moved, now, geometry.id.clone()));
        }
        let shift = self
            .dragging
            .or(self.held.as_ref().map(|h| h.0))
            .unwrap_or([0.0, 0.0]);
        let painter = ui.painter_at(rect);
        let at = |p: [f64; 2]| pos(view.to_screen(r, p));
        let outer = egui::Rect::from_two_pos(at([0.0, 0.0]), at(mat.outer_mm));
        painter.rect_filled(outer, 6.0, egui::Color32::from_rgb(150, 190, 215));
        let [sx0, sy0, sx1, sy1] = mat.sticky_mm;
        painter.rect_filled(
            egui::Rect::from_two_pos(at([sx0, sy0]), at([sx1, sy1])),
            0.0,
            egui::Color32::from_rgb(240, 246, 250),
        );
        for stroke in &geometry.sheet {
            let c = colour(&stroke.colour);
            let c = if stroke.aid { c.gamma_multiply(0.4) } else { c };
            let points: Vec<egui::Pos2> = stroke
                .points
                .iter()
                .map(|p| at([f64::from(p[0]) + shift[0], f64::from(p[1]) + shift[1]]))
                .collect();
            painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, c)));
        }
        if let Some(s) = geometry.test_spot_mm {
            painter.rect_stroke(
                egui::Rect::from_two_pos(at([s[0], s[1]]), at([s[2], s[3]])),
                0.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgb(200, 120, 0)),
                egui::StrokeKind::Middle,
            );
        }
        if let Some(b) = geometry.art_box_mm {
            painter.rect_stroke(
                egui::Rect::from_two_pos(
                    at([b[0] + shift[0], b[1] + shift[1]]),
                    at([b[2] + shift[0], b[3] + shift[1]]),
                ),
                0.0,
                egui::Stroke::new(1.0, egui::Color32::from_gray(120)),
                egui::StrokeKind::Middle,
            );
        }
        if self.held.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}
