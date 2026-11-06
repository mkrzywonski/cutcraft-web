//! CutCraft's desktop presentation. The browser owns only interaction drafts and cached drawing;
//! all project changes are revision-checked commands sent to the existing controller.
mod canvas;
pub mod chrome;
mod dialogs;
mod mat;
mod panels;
mod toolbar;
#[cfg(target_arch = "wasm32")]
mod web;

use cutcraft_web_protocol::{Geometry, SessionSnapshot};
use egui::{Color32, Response, Ui};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct Remote {
    pub snapshot: SessionSnapshot,
    pub geometry: Geometry,
    pub state: Value,
    pub library: Value,
    pub uploads: Value,
    pub me: Value,
    pub cut_preview: Value,
    pub preferences: Value,
    pub connected: bool,
    pub pending: bool,
    pub message: String,
    pub result: Value,
    pub result_id: u64,
    pub picture: String,
    pub picture_id: String,
    pub picture_source: String,
    pub picture_region: Option<[f64; 4]>,
}
#[derive(Clone, Serialize)]
pub struct Intent {
    pub action: String,
    pub params: Value,
}
#[derive(Clone, Default)]
pub struct Dialog {
    pub kind: String,
    pub title: String,
    pub value: Value,
    pub original: Option<String>,
    pub search: String,
    pub selected: Option<String>,
}
pub struct Desktop {
    presentation_identity: Option<(String, String, String, String)>,
    pub remote: Remote,
    pub intents: Vec<Intent>,
    pub dialog: Option<Dialog>,
    pub fields: BTreeMap<String, String>,
    pub edit_fences: BTreeMap<String, (String, u64, String)>,
    pub controls: BTreeMap<String, [f32; 4]>,
    pub texture: Option<egui::TextureHandle>,
    pub texture_id: String,
    pub texture_source: String,
    pub texture_region: Option<[f64; 4]>,
    pub highlight: Vec<String>,
    pub peek: Vec<String>,
    pub zoom: f32,
    pub pan: egui::Vec2,
    pub drag_start: Option<canvas::CanvasDrag>,
    pub drag_delta: egui::Vec2,
    pub target: String,
    pub show_test: bool,
    pub tool_motion: bool,
    pub order: f32,
    pub playing: bool,
    pub cutter: String,
    pub last_result: u64,
    pub close_after_save: bool,
    pub reviewed_tests: u64,
    pub dark: bool,
}
impl Default for Desktop {
    fn default() -> Self {
        Self {
            presentation_identity: None,
            remote: Remote::default(),
            intents: vec![],
            dialog: None,
            fields: BTreeMap::new(),
            edit_fences: BTreeMap::new(),
            controls: BTreeMap::new(),
            texture: None,
            texture_id: String::new(),
            texture_source: String::new(),
            texture_region: None,
            highlight: vec![],
            peek: vec![],
            zoom: 1.,
            pan: egui::Vec2::ZERO,
            drag_start: None,
            drag_delta: egui::Vec2::ZERO,
            target: "design".into(),
            show_test: true,
            tool_motion: false,
            order: 1.,
            playing: false,
            cutter: String::new(),
            last_result: 0,
            close_after_save: false,
            reviewed_tests: 0,
            dark: false,
        }
    }
}
impl Desktop {
    fn sync_presentation_identity(&mut self) {
        let source = self
            .remote
            .snapshot
            .design
            .as_ref()
            .map(|d| {
                if d.opened_source_sha256.is_empty() {
                    d.source_sha256.clone()
                } else {
                    d.opened_source_sha256.clone()
                }
            })
            .unwrap_or_default();
        let identity = (
            text(&self.remote.me["mode"]).to_owned(),
            text(&self.remote.me["principal"]).to_owned(),
            self.remote.snapshot.incarnation.clone(),
            source,
        );
        if self
            .presentation_identity
            .as_ref()
            .is_some_and(|old| old != &identity)
        {
            self.dialog = None;
            self.fields.clear();
            self.edit_fences.clear();
            self.intents.clear();
            self.highlight.clear();
            self.peek.clear();
            self.texture = None;
            self.texture_id.clear();
            self.texture_source.clear();
            self.texture_region = None;
            self.drag_start = None;
            self.drag_delta = egui::Vec2::ZERO;
            self.pan = egui::Vec2::ZERO;
            self.zoom = 1.;
            self.playing = false;
            self.order = 1.;
            self.close_after_save = false;
            self.reviewed_tests = 0;
            self.last_result = self.remote.result_id;
        }
        self.presentation_identity = Some(identity);
    }

    pub fn send(&mut self, action: &str, params: Value) {
        self.intents.push(Intent {
            action: action.into(),
            params,
        });
    }
    pub fn command(&mut self, id: &str, params: Value) {
        self.send("command", json!({"id": id, "params": params,"incarnation":self.remote.snapshot.incarnation,"revision":self.remote.snapshot.revision}));
    }
    pub fn screen(&self) -> &str {
        &self.remote.snapshot.ui.screen
    }
    pub fn has_design(&self) -> bool {
        self.remote.snapshot.design.is_some()
    }
    pub fn ready(&self) -> bool {
        self.remote
            .snapshot
            .design
            .as_ref()
            .is_some_and(|d| d.cut_ready && d.sheet.as_ref().is_some_and(|s| s.plan_valid))
    }
    pub fn enabled(&self, id: &str) -> bool {
        if id == "history.undo" && list(&self.remote.snapshot.ui.history["undo"]).is_empty() {
            return false;
        }
        if id == "history.redo" && list(&self.remote.snapshot.ui.history["redo"]).is_empty() {
            return false;
        }
        !self.remote.pending
            && self.remote.connected
            && self
                .remote
                .snapshot
                .ui
                .commands
                .iter()
                .any(|c| c["id"] == id && c["enabled"] == true)
    }
    pub fn record(&mut self, label: &str, r: &Response) {
        self.controls.insert(
            label.into(),
            [r.rect.min.x, r.rect.min.y, r.rect.max.x, r.rect.max.y],
        );
    }
    pub fn button(&mut self, ui: &mut Ui, label: &str, enabled: bool) -> bool {
        let r = ui.add_enabled(enabled, egui::Button::new(label));
        self.record(label, &r);
        r.clicked()
    }
    pub fn action_button(&mut self, ui: &mut Ui, label: &str, id: &str, params: Value) {
        if self.button(ui, label, self.enabled(id)) {
            self.command(id, params);
        }
    }
    pub fn open_dialog(&mut self, kind: &str, title: &str) {
        self.dialog = Some(Dialog {
            kind: kind.into(),
            title: title.into(),
            ..Default::default()
        });
    }
    pub fn frame(&mut self, root: &mut Ui) {
        let ctx = root.ctx().clone();
        self.sync_presentation_identity();
        self.controls.clear();
        self.dark = self.remote.preferences["dark"].as_bool().unwrap_or(false);
        if self.remote.result_id != self.last_result {
            self.last_result = self.remote.result_id;
            if self.remote.result["error_code"] == "saved-elsewhere" {
                self.open_dialog("save-conflict", "This project was saved elsewhere");
            }
            if self.remote.result.get("close_dialog") == Some(&Value::Bool(true)) {
                self.dialog = None;
            }
        }
        ctx.set_visuals(if self.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        let mut style = (**root.style()).clone();
        let size = self.remote.snapshot.ui.font_size;
        let size = if size >= 10. { size } else { 14. };
        style.text_styles = egui::Style::default().text_styles;
        for font in style.text_styles.values_mut() {
            font.size *= size / 14.;
        }
        style.spacing.item_spacing = egui::vec2(8., 7.);
        style.spacing.button_padding = egui::vec2(10., 6.);
        style.visuals.selection.bg_fill = Color32::from_rgb(24, 112, 96);
        style.visuals.selection.stroke.color = Color32::WHITE;
        ctx.set_global_style(style.clone());
        root.set_style(style);
        if self.remote.me["login_required"] == true
            || self.remote.me["must_change"] == true
            || self.remote.me["mode"] == "locked"
        {
            egui::CentralPanel::default().show(root, |ui| self.login(ui));
            return;
        }
        self.keyboard(&ctx);
        egui::Panel::top("chrome").show(root, |ui| {
            self.menus(ui);
            ui.separator();
            self.toolbar(ui);
        });
        egui::Panel::bottom("status").show(root, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(
                    if self.remote.connected {
                        Color32::from_rgb(30, 115, 80)
                    } else {
                        Color32::from_rgb(160, 60, 40)
                    },
                    if self.remote.connected {
                        "● Connected"
                    } else {
                        "● Reconnecting — edits unavailable"
                    },
                );
                if self.remote.pending || self.remote.snapshot.busy.is_some() {
                    ui.spinner();
                    ui.label(
                        self.remote
                            .snapshot
                            .busy
                            .as_deref()
                            .unwrap_or("Applying change…"),
                    );
                }
                if !self.remote.message.is_empty() {
                    ui.label(&self.remote.message);
                }
            });
        });
        if matches!(self.screen(), "cut" | "test-cut") {
            egui::CentralPanel::default().show(root, |ui| self.cutting(ui));
        } else {
            if self.has_design() {
                egui::Panel::right("controls")
                    .default_size(340.)
                    .min_size(270.)
                    .resizable(true)
                    .show(root, |ui| self.panel(ui));
            }
            egui::CentralPanel::default().show(root, |ui| self.canvas(ui));
        }
        self.dialogs(&ctx);
    }
    fn login(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui|{
            ui.add_space(70.);
            if self.remote.me["mode"]=="locked" {ui.heading("Account recovery required");ui.label("The server's account file could not be read. Ask the administrator to restore it or reset the administrator account from the command line.");return;}
            let change=self.remote.me["must_change"]==true;
            ui.heading(if change {"Choose a new password"} else {"Sign in to CutCraft"});
            if !change {
                let r=ui.add(egui::TextEdit::singleline(self.fields.entry("username".into()).or_default()).hint_text("Username"));self.record("Username",&r);
            }
            let r=ui.add(egui::TextEdit::singleline(self.fields.entry("password".into()).or_default()).password(true).hint_text(if change {"Current password"}else{"Password"}));self.record("Password",&r);
            if change {
                let r=ui.add(egui::TextEdit::singleline(self.fields.entry("new-password".into()).or_default()).password(true).hint_text("New password"));self.record("New password",&r);
                if self.button(ui,"Change password",!self.remote.pending) {
                    let old=self.fields.remove("password").unwrap_or_default();let new=self.fields.remove("new-password").unwrap_or_default();
                    self.send("password",json!({"old":old,"new":new}));
                }
                if self.button(ui,"Sign out",!self.remote.pending){self.send("logout",json!({}));}
            } else if self.button(ui,"Sign in",!self.remote.pending) {
                let password=self.fields.remove("password").unwrap_or_default();self.send("login",json!({"username":self.fields.get("username"),"password":password}));
            }
            ui.label(&self.remote.message);
        });
    }
}
pub fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
pub fn number(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.)
}
pub fn list(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn another_account_cannot_inherit_dialog_drafts_or_pending_actions() {
        let mut desktop = Desktop::default();
        desktop.remote.me = json!({"mode":"users","principal":"first-account"});
        desktop.remote.snapshot.incarnation = "session".into();
        desktop.remote.snapshot.design = Some(cutcraft_web_protocol::DesignView {
            opened_source_sha256: "original-file".into(),
            ..Default::default()
        });
        desktop.sync_presentation_identity();
        desktop.open_dialog("save", "Save private project");
        desktop.fields.insert("name".into(), "Private name".into());
        desktop.reviewed_tests = 3;
        desktop.send("save", json!({}));
        desktop.remote.snapshot.revision += 1;
        desktop
            .remote
            .snapshot
            .design
            .as_mut()
            .unwrap()
            .source_sha256 = "another-page".into();
        desktop.sync_presentation_identity();
        assert!(
            desktop.dialog.is_some(),
            "an answer in the same source retains interaction drafts"
        );
        desktop.remote.me["principal"] = json!("second-account");
        desktop.sync_presentation_identity();
        assert!(desktop.dialog.is_none());
        assert!(desktop.fields.is_empty());
        assert!(desktop.intents.is_empty());
        assert_eq!(desktop.reviewed_tests, 0);
    }
}
