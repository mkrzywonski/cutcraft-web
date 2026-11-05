use crate::{Desktop, list, text};
use egui::{Key, Modifiers, Ui};
use serde_json::json;
impl Desktop {
    pub fn menus(&mut self, ui: &mut Ui) {
        egui::MenuBar::new().ui(ui,|ui| {
            let menu = ui.menu_button("File",|ui|{
                if self.button(ui,"Open…",true) {self.send("upload",json!({}));ui.close();}
                if self.button(ui,"Open from library…",true) {self.send("library",json!({}));self.open_dialog("library","Open from library");ui.close();}
                if self.button(ui,"Recent designs…",true) {self.send("uploads",json!({}));self.open_dialog("uploads","Recent designs");ui.close();}
                ui.separator();
                if self.button(ui,"Save",self.has_design()) {self.save(false);ui.close();}
                if self.button(ui,"Save As…",self.has_design()) {self.save(true);ui.close();}
                if self.button(ui,"Download project…",self.has_design()) {self.send("download",json!({"kind":"project"}));ui.close();}
                if self.button(ui,"Update from Design…",self.has_design()) {self.send("upload",json!({"update":true}));ui.close();}
                ui.menu_button("Export",|ui| {for (kind,label) in [("preview-svg","Cut Preview SVG"),("preview-png","Cut Preview PNG"),("cutter-file","Cutter file (inspection only)"),("plan","Cut plan")] {if self.button(ui,label,self.has_design()){self.send("download",json!({"kind":kind}));ui.close();}}});
                ui.separator();
                if self.button(ui,"Close design",self.has_design()) {self.open_dialog("close","Close design");ui.close();}
            });
            self.record("Menu File",&menu.response);
            let menu = ui.menu_button("Edit",|ui|{
                self.action_button(ui,"Undo","history.undo",json!({}));self.action_button(ui,"Redo","history.redo",json!({}));
                if self.button(ui,"History…",true){self.open_dialog("history","History");ui.close();}
                ui.separator();
                for (kind,label) in [("materials","Materials…"),("mats","Mats…"),("preferences","Preferences…")] {if self.button(ui,label,true){self.open_dialog(kind,label);ui.close();}}
                if self.button(ui,"AI connection…",true){self.send("settings",json!({"section":"keys"}));ui.close();}
            });
            self.record("Menu Edit",&menu.response);
            let menu = ui.menu_button("View",|ui|{
                for (screen,label) in [("side-by-side","Side by side"),("overlay","Overlay"),("mat","Cut Preview")] {
                    let selected=self.screen()==screen;
                    if ui.add_enabled(!self.cut_busy(),egui::Button::selectable(selected,label)).clicked(){self.command("view.show",json!({"screen":screen}));ui.close();}
                }
                ui.separator();let zoom=self.view_controls_enabled();let mat=self.screen()=="mat";
                if self.button(ui,if mat{"Make design bigger"}else{"Zoom in"},zoom){self.zoom_view(true);}
                if self.button(ui,if mat{"Make design smaller"}else{"Zoom out"},zoom){self.zoom_view(false);}
                if self.button(ui,if mat{"Fit design to mat"}else{"Fit in window"},zoom){self.fit_view();}
                ui.checkbox(&mut self.tool_motion,"Show moves between cuts");
                ui.checkbox(&mut self.show_test,"Show test-cut spot");
                ui.separator();
                for units in ["inches","millimetres"] {if ui.radio(self.remote.snapshot.ui.units==units,units).clicked(){self.command("settings.units",json!({"units":units}));}}
                if self.button(ui,"Interface font size and theme…",true){self.open_dialog("preferences","Preferences");ui.close();}
                if self.button(ui,"Reset panel layout",true){self.command("settings.panel",json!({"order":[],"locked":[]}));self.send("reset-folds",json!({}));}
                if self.button(ui,"Phone layout",true){self.send("layout",json!({"layout":"compact"}));}

            });
            self.record("Menu View",&menu.response);
            let menu = ui.menu_button("Cut",|ui| {
                if self.button(ui,"Cut Preview",!self.cut_busy()){self.command("view.show",json!({"screen":"mat"}));ui.close();}
                if self.button(ui,"Stop now",self.cut_busy()){self.send("cutter",json!({"id":self.cutter,"action":"stop"}));ui.close();}

                if self.button(ui,"Cut…",self.ready() && !self.cut_busy()){self.enter_cut(false);ui.close();}
                if self.button(ui,"Test Cut…",self.has_design() && !self.cut_busy()){self.enter_cut(true);ui.close();}
                if self.button(ui,"Cutters and queue…",true){self.send("settings",json!({"section":"cutters"}));ui.close();}
            });
            self.record("Menu Cut",&menu.response);
            let menu = ui.menu_button("Help",|ui| {
                for (kind,label) in [("design-help","How to save designs for cutting"),("troubleshooting","Troubleshooting"),("ai-help","Set up AI tools"),("about","About CutCraft")] {if self.button(ui,label,true){self.open_dialog(kind,label);ui.close();}}
                if self.button(ui,"Manual",true){self.send("link",json!({"url":"https://github.com/mkrzywonski/cutcraft-web/blob/main/docs/help/design-guide.md"}));ui.close();}
            });
            self.record("Menu Help",&menu.response);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| {
                if self.button(ui,"Account",true){self.send("settings",json!({"section":"settings"}));}
                ui.weak(self.remote.snapshot.design.as_ref().map(|d|format!("{}{}",d.name,if d.unsaved {" •"}else{""})).unwrap_or_else(||"CutCraft".into()));
            });
        });
    }
    fn tool(
        &mut self,
        ui: &mut Ui,
        icon: crate::chrome::Icon,
        label: &str,
        tip: &str,
        selected: bool,
        enabled: bool,
    ) -> bool {
        let r = crate::chrome::tool(ui, icon, label, tip, selected, enabled);
        self.record(tip, &r);
        r.clicked()
    }
    pub fn toolbar(&mut self, ui: &mut Ui) {
        use crate::chrome::Icon;
        let compact = ui.available_width() < 1150.;
        ui.horizontal(|ui| {
            if self.tool(
                ui,
                Icon::Open,
                if compact { "" } else { "Open" },
                "Open",
                false,
                true,
            ) {
                self.send("upload", json!({}));
            }
            if self.tool(ui, Icon::Save, "", "Save", false, self.has_design()) {
                self.save(false);
            }
            crate::chrome::divider(ui);
            if self.tool(
                ui,
                Icon::Undo,
                "",
                "Undo",
                false,
                self.enabled("history.undo"),
            ) {
                self.command("history.undo", json!({}));
            }
            if self.tool(
                ui,
                Icon::Redo,
                "",
                "Redo",
                false,
                self.enabled("history.redo"),
            ) {
                self.command("history.redo", json!({}));
            }
            crate::chrome::divider(ui);
            for (screen, icon, label) in [
                ("side-by-side", Icon::SideBySide, "Side by side"),
                ("overlay", Icon::Overlay, "Overlay"),
                ("mat", Icon::Mat, "Cut Preview"),
            ] {
                if self.tool(
                    ui,
                    icon,
                    if compact { "" } else { label },
                    label,
                    self.screen() == screen,
                    !self.cut_busy(),
                ) {
                    self.command("view.show", json!({"screen":screen}));
                }
            }
            crate::chrome::divider(ui);
            let zoom = self.view_controls_enabled();
            if self.tool(
                ui,
                Icon::Minus,
                "",
                if self.screen() == "mat" {
                    "Make design smaller"
                } else {
                    "Zoom out"
                },
                false,
                zoom,
            ) {
                self.zoom_view(false);
            }
            if self.tool(
                ui,
                Icon::Plus,
                "",
                if self.screen() == "mat" {
                    "Make design bigger"
                } else {
                    "Zoom in"
                },
                false,
                zoom,
            ) {
                self.zoom_view(true);
            }
            if self.tool(
                ui,
                Icon::Fit,
                "",
                if self.screen() == "mat" {
                    "Fit design to mat"
                } else {
                    "Fit in window"
                },
                false,
                zoom,
            ) {
                self.fit_view();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let cut = self.screen() == "cut";
                let test = self.screen() == "test-cut";
                if self.cut_busy() {
                    if self.tool(ui, Icon::Stop, "Stop", "Stop now", false, true) {
                        self.send("cutter", json!({"id":self.cutter,"action":"stop"}));
                    }
                    return;
                }
                if self.tool(
                    ui,
                    if cut { Icon::Exit } else { Icon::Cut },
                    if cut { "Exit" } else { "Cut" },
                    "Cut",
                    cut,
                    self.ready() || cut,
                ) {
                    if cut {
                        self.command("view.show", json!({"screen":"mat"}));
                    } else {
                        self.enter_cut(false);
                    }
                }
                if self.tool(
                    ui,
                    if test { Icon::Exit } else { Icon::TestCut },
                    if test { "Exit" } else { "Test Cut" },
                    "Test Cut",
                    test,
                    self.has_design(),
                ) {
                    if test {
                        self.command("view.show", json!({"screen":"mat"}));
                    } else {
                        self.enter_cut(true);
                    }
                }
            });
        });
    }
    pub fn save(&mut self, as_new: bool) {
        if as_new
            || self
                .remote
                .snapshot
                .design
                .as_ref()
                .is_none_or(|d| d.saved.is_none())
        {
            self.open_dialog("save", "Save As");
            if let Some(d) = self.dialog.as_mut() {
                d.value = json!({"name":self.remote.snapshot.design.as_ref().map(|d|d.name.clone()).unwrap_or_default(),"folder":""});
            }
        } else {
            self.send("save", json!({"close_after":self.close_after_save}));
        }
    }
    pub fn enter_cut(&mut self, test: bool) {
        if self.cutter.is_empty()
            && let Some(c) = list(&self.remote.state["cutters"]).first()
        {
            self.cutter = text(&c["id"]).into();
        }
        self.command(
            "view.show",
            json!({"screen":if test{"test-cut"}else{"cut"}}),
        );

        self.send("select-cutter", json!({"cutter":self.cutter}));
    }
    pub fn cut_busy(&self) -> bool {
        list(&self.remote.state["cutters"]).iter().any(|c| {
            c["id"] == self.cutter
                && matches!(
                    text(&c["view"]["activity"]),
                    "testing" | "cutting" | "paused" | "stopping" | "confirming-stop"
                )
        })
    }
    pub fn keyboard(&mut self, ctx: &egui::Context) {
        let pressed = |modifiers, key| {
            ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(modifiers, key)))
        };
        // Stop remains available through modal dialogs, text editing and pending network work.
        if self.cut_busy() && pressed(Modifiers::NONE, Key::Escape) {
            self.send("cutter", json!({"id":self.cutter,"action":"stop"}));
            return;
        }
        if self.dialog.is_some() || ctx.text_edit_focused() {
            return;
        }
        let shift = Modifiers::COMMAND | Modifiers::SHIFT;
        if pressed(Modifiers::COMMAND, Key::O) {
            self.send("upload", json!({}));
        }
        if pressed(shift, Key::S) && self.has_design() {
            self.save(true);
        } else if pressed(Modifiers::COMMAND, Key::S) && self.has_design() {
            self.save(false);
        }
        if (pressed(shift, Key::Z) || pressed(Modifiers::COMMAND, Key::Y))
            && self.enabled("history.redo")
        {
            self.command("history.redo", json!({}));
        } else if pressed(Modifiers::COMMAND, Key::Z) && self.enabled("history.undo") {
            self.command("history.undo", json!({}));
        }
        if self.has_design() && !self.cut_busy() {
            for (key, screen) in [
                (Key::Num1, "side-by-side"),
                (Key::Num2, "overlay"),
                (Key::Num3, "mat"),
            ] {
                if pressed(Modifiers::COMMAND, key) {
                    self.command("view.show", json!({"screen":screen}));
                }
            }
        }
        if self.view_controls_enabled() {
            if pressed(Modifiers::COMMAND, Key::Num0) {
                self.fit_view();
            }
            if pressed(Modifiers::COMMAND, Key::Equals) || pressed(Modifiers::COMMAND, Key::Plus) {
                self.zoom_view(true);
            }
            if pressed(Modifiers::COMMAND, Key::Minus) {
                self.zoom_view(false);
            }
        }
    }
    fn view_controls_enabled(&self) -> bool {
        self.has_design()
            && !matches!(self.screen(), "cut" | "test-cut")
            && (self.screen() != "mat" || self.enabled("design.size"))
    }
    fn zoom_view(&mut self, bigger: bool) {
        if !self.view_controls_enabled() {
            return;
        }
        if self.screen() == "mat" {
            self.resize_step(if bigger { 1.1 } else { 1. / 1.1 });
        } else {
            self.zoom = (self.zoom * if bigger { 1.25 } else { 1. / 1.25 }).clamp(0.1, 50.);
        }
    }
    fn fit_view(&mut self) {
        if !self.view_controls_enabled() {
            return;
        }
        if self.screen() == "mat" {
            self.command("design.size", json!({"fit":true}));
        } else {
            self.zoom = 1.;
            self.pan = egui::Vec2::ZERO;
        }
    }
    fn resize_step(&mut self, factor: f64) {
        if let Some([w, _]) = self.remote.snapshot.ui.size_mm {
            self.command("design.size", json!({"width_mm":w*factor}));
        }
    }
    pub fn cutting(&mut self, ui: &mut Ui) {
        let test = self.screen() == "test-cut";
        egui::Panel::right("loading-controls").default_size(370.).min_size(310.).show(ui,|ui|{
            ui.heading(if test{"Test Cut"}else{"Cut"});
            let cutters=list(&self.remote.state["cutters"]);
            if self.cutter.is_empty(){self.cutter=text(&self.remote.preferences["cutter"]).into();if self.cutter.is_empty()&& let Some(c)=cutters.first(){self.cutter=text(&c["id"]).into();}}
            let before=self.cutter.clone();
            ui.add_enabled_ui(!self.cut_busy(),|ui|{egui::ComboBox::from_label("Cutter").selected_text(&self.cutter).show_ui(ui,|ui|{for c in &cutters{let id=text(&c["id"]);ui.selectable_value(&mut self.cutter,id.into(),text(&c["view"]["name"]));}});});
            if self.cutter!=before{self.send("select-cutter",json!({"cutter":self.cutter}));}
            let cutter=cutters.iter().find(|c|c["id"]==self.cutter).cloned().unwrap_or_default();let v=cutter["view"].clone();
            ui.separator();
            let preview=self.remote.cut_preview.clone();let busy=self.cut_busy();
            let copy = &preview["copy"];
            let changed = self.remote.snapshot.design.as_ref().is_some_and(|design|
                !copy.is_null() && (copy["project_revision"] != design.project_revision
                    || copy["source_sha256"] != design.source_sha256
                    || copy["incarnation"] != self.remote.snapshot.incarnation));
            if changed && !busy {
                ui.strong("Settings changed since this cut was prepared");
                ui.label("Prepare the current settings to review the updated cut. Earlier test areas stay reserved on the same material.");
                if self.button(ui,"Prepare updated cut",!self.remote.pending){self.send("prepare-cut",json!({"cutter":self.cutter,"test":test}));}
            }
            let material=&preview["material"];
            ui.strong(text(&material["name"]));
            ui.label(format!("Blade {} · force {} · speed {} · {} pass(es)",material["blade"],material["force"],material["speed"],material["passes"]));
            for (n,step) in list(&preview["steps"]).iter().enumerate(){ui.label(format!("{}. {}",n+1,text(step)));}
            let waiting=preview["copy"]["id"].as_str().is_some_and(|id|list(&v["queue"]).iter().any(|q|q["job"]["job_id"]==id));
            if waiting{ui.strong("Waiting for this cutter");ui.label("Your prepared cut is in the queue. Start will be available when it is your turn and the mat is ready.");}
            if !waiting && (preview.is_null() || preview["geometry"].is_null()){ui.label("Choose a cutter to prepare the loading instructions.");if self.button(ui,if test{"Cut pattern"}else{"Cut this sheet"},!self.remote.pending && !self.cutter.is_empty()){self.send("prepare-cut",json!({"cutter":self.cutter,"test":test}));}}
            ui.separator();
            let state=text(&v["activity"]);ui.strong(match state{"testing"=>"Test cut in progress","cutting"=>"Cutting","paused"=>"Paused","stopping"=>"Stopping — waiting for confirmation","confirming-stop"=>"Checking that the cutter stopped","check-cutter"=>"Check the cutter",_=>if v["mat_loaded"]==true{"Mat loaded"}else{"Load the mat"}});
            if busy {
                let fraction=crate::number(&v["progress"]["fraction"]) as f32;ui.add(egui::ProgressBar::new(fraction).show_percentage());
                ui.label(format!("About {:.0} seconds remaining",crate::number(&v["progress"]["remaining_s"])));
                if self.button(ui,if state=="paused"{"Resume"}else{"Pause"},matches!(state,"testing"|"cutting"|"paused")){self.send("cutter",json!({"id":self.cutter,"action":if state=="paused"{"resume"}else{"pause"}}));}
                if self.button(ui,"Stop now",true){self.send("cutter",json!({"id":self.cutter,"action":"stop"}));}
            }else{
                if v["mat"]["state"]=="unseen" && v["mat_loaded"]==true{
                    ui.label("CutCraft did not see this mat load. Confirm only if it has fresh material where this cut will go.");
                    if self.button(ui,"This mat has fresh material",!self.remote.pending){self.send("cutter",json!({"id":self.cutter,"action":"mat-fresh"}));}
                }
                let tests=v["turn"]["tests"].as_u64().unwrap_or(0);
                if test && tests>self.reviewed_tests && v["mat"]["state"]=="fresh" {
                    ui.strong("Test cut finished");
                    ui.label("Check how the pattern weeds. Adjust material settings if needed, or try another pattern.");
                    if self.button(ui,"Try another pattern",!self.remote.pending){self.reviewed_tests=tests;}
                }
                let own_copy=preview.get("copy").filter(|c|!c.is_null());
                let ready=!changed && (!test || tests<=self.reviewed_tests) && self.remote.connected && !self.remote.pending && v["online"]==true && v["activity"]=="idle" && v["mat"]["state"]=="fresh" && v["mat_loaded"]==true && own_copy.is_some_and(|c|c["id"]==v["turn"]["job"]["job_id"]) && !preview["geometry"].is_null();
                if self.button(ui,"Start",ready){let start=json!({"boot":self.remote.state["boot"],"kind":if test{"test"}else{"main"},"turn":v["turn"]["id"],"job_id":v["turn"]["job"]["job_id"],"revision":v["turn"]["job"]["revision"],"generation":v["mat"]["generation"],"action_epoch":v["action_epoch"]});self.send("start",json!({"cutter":self.cutter,"start":start}));}
                if v["mat"]["state"]=="used" {ui.strong("Fresh material needed");ui.label("Unload and load fresh material for another cut.");}
                if tests>0{ui.label(format!("{tests} test cut(s) on this mat. The next pattern stays clear of earlier tests."));}
                if self.button(ui,"Troubleshooting",true){self.open_dialog("troubleshooting","Troubleshooting");}
            }
            if cutter["simulated"]==true{ui.separator();ui.label("Simulated Cameo — no physical motion");ui.horizontal(|ui|{for (load,label) in [(true,"Load virtual mat"),(false,"Unload virtual mat")]{if self.button(ui,label,!busy && if load {v["mat"]["state"]=="none"}else{v["mat_loaded"]==true}){self.send("cutter",json!({"id":self.cutter,"action":"practice","body":{"load":load}}));}}});}
        });
        let saved = self.remote.geometry.clone();
        if let Ok(geometry) = serde_json::from_value(self.remote.cut_preview["geometry"].clone()) {
            self.remote.geometry = geometry;
        }
        self.canvas(ui);
        self.remote.geometry = saved;
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;

    fn key_input(key: Key, modifiers: Modifiers) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280., 900.),
            )),
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn escape_stops_through_a_modal_and_pending_request() {
        let mut desktop = Desktop {
            cutter: "practice".into(),
            ..Default::default()
        };
        desktop.remote.pending = true;
        desktop.remote.snapshot.ui.screen = "cut".into();
        desktop.remote.state = json!({"cutters":[{"id":"practice","view":{"activity":"cutting"}}]});
        desktop.open_dialog("troubleshooting", "Troubleshooting");
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(key_input(Key::Escape, Modifiers::NONE), |ui| {
            desktop.frame(ui)
        });
        output.textures_delta.clear();
        assert!(
            desktop
                .intents
                .iter()
                .any(|i| i.action == "cutter"
                    && i.params == json!({"id":"practice","action":"stop"}))
        );
        assert!(
            desktop.controls.contains_key("Stop now"),
            "a visible Stop is also available in the modal"
        );
    }

    #[test]
    fn undo_in_a_text_field_does_not_undo_the_project() {
        let mut desktop = Desktop::default();
        desktop.remote.connected = true;
        desktop.remote.snapshot.ui.history = json!({"undo":[{"label":"project edit"}]});
        desktop.remote.snapshot.ui.commands = vec![json!({"id":"history.undo","enabled":true})];
        let ctx = egui::Context::default();
        let mut draft = "A material name".to_owned();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            ui.add(egui::TextEdit::singleline(&mut draft).id(egui::Id::new("draft")))
                .request_focus();
        });
        output.textures_delta.clear();
        assert!(ctx.text_edit_focused());
        let mut output = ctx.run_ui(key_input(Key::Z, Modifiers::COMMAND), |ui| {
            desktop.keyboard(ui.ctx());
            ui.add(egui::TextEdit::singleline(&mut draft).id(egui::Id::new("draft")));
        });
        output.textures_delta.clear();
        assert!(desktop.intents.is_empty());
    }

    #[test]
    fn save_as_and_view_shortcuts_work_but_cut_screens_cannot_resize() {
        let mut desktop = Desktop::default();
        desktop.remote.snapshot.design = Some(Default::default());
        desktop.remote.snapshot.ui.screen = "side-by-side".into();
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(
            key_input(Key::S, Modifiers::COMMAND | Modifiers::SHIFT),
            |ui| desktop.keyboard(ui.ctx()),
        );
        output.textures_delta.clear();
        assert_eq!(desktop.dialog.as_ref().unwrap().kind, "save");
        desktop.dialog = None;
        let mut output = ctx.run_ui(key_input(Key::Num2, Modifiers::COMMAND), |ui| {
            desktop.keyboard(ui.ctx())
        });
        output.textures_delta.clear();
        assert!(desktop.intents.iter().any(|i|i.params["id"]=="view.show" && i.params["params"]["screen"]=="overlay"));
        desktop.intents.clear();
        for screen in ["cut", "test-cut"] {
            desktop.remote.snapshot.ui.screen = screen.into();
            let before = desktop.zoom;
            desktop.zoom_view(true);
            desktop.fit_view();
            assert_eq!(desktop.zoom, before);
            assert!(desktop.intents.is_empty());
        }
    }
}
