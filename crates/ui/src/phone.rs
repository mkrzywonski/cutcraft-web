//! Phone arrangement (M5 proof of concept). A narrow window shows one full-screen view at a time:
//! the artwork, the cut lines, Overlay, Cut Preview or the Controls panel, chosen with
//! thumb-sized tabs at the bottom. The menus fold into a menu row, and Stop is always a large red
//! button at the top while a cut runs. Every view and control is the desktop's own; only the
//! arrangement changes, so the phone keeps every capability.
use crate::Desktop;
use egui::{Color32, RichText, Ui, vec2};
use serde_json::json;

/// Narrower than this (in points) uses the phone arrangement.
pub const PHONE_WIDTH: f32 = 600.;
const TAB_HEIGHT: f32 = 52.;
const TOP_HEIGHT: f32 = 44.;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PhoneTab {
    #[default]
    Artwork,
    Lines,
    Overlay,
    Preview,
    Controls,
}

impl Desktop {
    pub fn phone_frame(&mut self, root: &mut Ui) {
        let cutting = matches!(self.remote.snapshot.ui.screen.as_str(), "cut" | "test-cut");
        if cutting && !matches!(self.phone_tab, PhoneTab::Preview | PhoneTab::Controls) {
            self.phone_tab = PhoneTab::Controls;
        }
        egui::Panel::top("phone-top").show(root, |ui| self.phone_top(ui, cutting));
        if cutting || self.has_design() {
            egui::Panel::bottom("phone-tabs").show(root, |ui| self.phone_tabs(ui, cutting));
        }
        egui::Panel::bottom("phone-status").show(root, |ui| self.status(ui));
        egui::CentralPanel::default().show(root, |ui| {
            if cutting {
                self.cutting(ui);
            } else if self.has_design() && self.phone_tab == PhoneTab::Controls {
                self.panel(ui);
            } else {
                self.canvas(ui);
            }
        });
    }

    fn phone_button(&mut self, ui: &mut Ui, label: &str, id: &str, enabled: bool) -> bool {
        let r = ui.add_enabled(
            enabled,
            egui::Button::new(label).min_size(vec2(TOP_HEIGHT, TOP_HEIGHT)),
        );
        self.record(id, &r);
        r.clicked()
    }

    fn phone_top(&mut self, ui: &mut Ui, cutting: bool) {
        ui.horizontal(|ui| {
            let menu = ui.add(
                egui::Button::selectable(self.phone_menu, "Menu")
                    .min_size(vec2(TOP_HEIGHT * 1.4, TOP_HEIGHT)),
            );
            self.record("Phone menu", &menu);
            if menu.clicked() {
                self.phone_menu = !self.phone_menu;
            }
            if !cutting {
                let undo = self.enabled("history.undo");
                if self.phone_button(ui, "Undo", "Phone undo", undo) {
                    self.command("history.undo", json!({}));
                }
                let redo = self.enabled("history.redo");
                if self.phone_button(ui, "Redo", "Phone redo", redo) {
                    self.command("history.redo", json!({}));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.cut_busy() {
                    // Stop is never folded away and never waits behind other work.
                    let stop = ui.add(
                        egui::Button::new(
                            RichText::new("STOP")
                                .strong()
                                .size(18.)
                                .color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(200, 30, 30))
                        .min_size(vec2(120., TOP_HEIGHT)),
                    );
                    self.record("Phone stop", &stop);
                    if stop.clicked() {
                        self.send("cutter", json!({"id":self.cutter,"action":"stop"}));
                    }
                } else if cutting {
                    if self.phone_button(ui, "Exit", "Phone exit", true) {
                        self.command("view.show", json!({"screen":"mat"}));
                        self.phone_tab = PhoneTab::Preview;
                    }
                } else {
                    let ready = self.ready();
                    if self.phone_button(ui, "Cut", "Phone cut", ready) {
                        self.enter_cut(false);
                        self.phone_tab = PhoneTab::Controls;
                    }
                    let design = self.has_design();
                    if self.phone_button(ui, "Test Cut", "Phone test cut", design) {
                        self.enter_cut(true);
                        self.phone_tab = PhoneTab::Controls;
                    }
                }
            });
        });
        if self.phone_menu {
            // The desktop's own menus, every item, on a row of thumb-sized entries.
            ui.separator();
            self.menus(ui);
        }
        if let Some(design) = self.remote.snapshot.design.as_ref() {
            let status = if design.cut_ready {
                "ready to cut".to_owned()
            } else if design.issues > 0 {
                format!("{} to resolve", design.issues)
            } else {
                "not ready".to_owned()
            };
            ui.add(
                egui::Label::new(RichText::new(format!("{} · {status}", design.name)).weak())
                    .truncate(),
            );
        }
    }

    fn phone_tabs(&mut self, ui: &mut Ui, cutting: bool) {
        let issues = self.remote.snapshot.design.as_ref().map_or(0, |d| d.issues);
        let controls = if issues > 0 {
            format!("Controls ({issues})")
        } else {
            "Controls".to_owned()
        };
        let tabs: Vec<(PhoneTab, String)> = if cutting {
            vec![
                (PhoneTab::Preview, "Mat".into()),
                (
                    PhoneTab::Controls,
                    if self.screen() == "test-cut" {
                        "Test Cut".into()
                    } else {
                        "Cut".into()
                    },
                ),
            ]
        } else {
            vec![
                (PhoneTab::Artwork, "Artwork".into()),
                (PhoneTab::Lines, "Cut lines".into()),
                (PhoneTab::Overlay, "Overlay".into()),
                (PhoneTab::Preview, "Preview".into()),
                (PhoneTab::Controls, controls),
            ]
        };
        let gap = 4.;
        ui.spacing_mut().item_spacing.x = gap;
        // The tab, not its text, sets the width: five tabs must fit a 360-point phone.
        ui.spacing_mut().button_padding = vec2(2., 6.);
        let width = (ui.available_width() - gap * (tabs.len() as f32 - 1.)) / tabs.len() as f32;
        ui.horizontal(|ui| {
            for (tab, label) in tabs {
                let r = ui.add_sized(
                    [width, TAB_HEIGHT],
                    egui::Button::selectable(
                        self.phone_tab == tab,
                        RichText::new(&label).size(13.),
                    ),
                );
                self.record(&format!("Phone tab {label}"), &r);
                if r.clicked() {
                    self.phone_tab = tab;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controls_at(width: f32) -> Vec<String> {
        let mut desktop = Desktop::default();
        desktop.remote.connected = true;
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 844.),
                )),
                ..Default::default()
            },
            |root| desktop.frame(root),
        );
        output.textures_delta.clear();
        desktop.controls.keys().cloned().collect()
    }

    #[test]
    fn narrow_windows_use_the_phone_arrangement_and_wide_ones_the_desktop() {
        let phone = controls_at(390.);
        assert!(phone.iter().any(|c| c == "Phone menu"));
        assert!(
            !phone.iter().any(|c| c == "Menu File"),
            "desktop menu bar hidden until Menu is tapped"
        );
        let desktop = controls_at(1280.);
        assert!(!desktop.iter().any(|c| c == "Phone menu"));
    }
}
