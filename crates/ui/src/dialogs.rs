use crate::{Desktop, Dialog, list, number, text};
use egui::{Context, Ui};
use serde_json::{Value, json};
impl Desktop {
    pub fn dialogs(&mut self, ctx: &Context) {
        let Some(mut d) = self.dialog.take() else {
            return;
        };
        let mut open = true;
        let mut done = false;
        let modal=egui::Modal::new(egui::Id::new("dialog"))
            .frame(egui::Frame::window(&ctx.style_of(ctx.theme())).inner_margin(20)).show(ctx,|ui|{
                ui.set_width(620_f32.min(ctx.content_rect().width()-60.));
                ui.horizontal(|ui| {ui.heading(&d.title);if self.button(ui,"Close dialog",true){open=false;}});
                ui.separator();
                egui::ScrollArea::vertical().max_height((ctx.content_rect().height()-150.).max(200.)).show(ui,|ui|{
                    match d.kind.as_str(){
                        "materials"|"mats"=>self.library_dialog(ui,&mut d),
                        "material-edit"|"mat-edit"=>{self.editor(ui,&mut d);},
                        "save"=>{string_field(ui,"Name",&mut d.value["name"]);string_field(ui,"Folder (optional, / separated)",&mut d.value["folder"]);ui.horizontal(|ui|{if self.button(ui,"Save project",!self.remote.pending){let folder:Vec<_>=text(&d.value["folder"]).split('/').filter(|s|!s.is_empty()).collect();self.send("save",json!({"as":{"name":d.value["name"],"folder":folder},"close_after":self.close_after_save}));}
if self.button(ui,"Cancel",true){done=true;}});},
                        "trace"=>{
                            ui.label("Adjust the trace, then apply it to preview the resulting cut lines. Undo restores the previous trace.");
                            let mut colours=text(&d.value["colours"]).to_owned();
                            ui.horizontal(|ui|{ui.radio_value(&mut colours,"one".into(),"One colour");ui.radio_value(&mut colours,"several".into(),"Several colours");});
                            d.value["colours"]=json!(colours);
                            numeric_field(ui,"Detail",&mut d.value["detail"],0.,100.,true);
                            numeric_field(ui,"Smoothness",&mut d.value["smoothness"],0.,100.,true);
                            if colours=="one"{numeric_field(ui,"Darkness threshold",&mut d.value["threshold"],0.,255.,true);}
                            let mut white=d.value["ignore_white"]==true;if ui.checkbox(&mut white,"Ignore white").changed(){d.value["ignore_white"]=json!(white);}
                            if self.button(ui,"Apply trace and preview",!self.remote.pending){self.command("prepare.decide",json!({"decision":d.value}));}
                        },
                        "material-save"=>{string_field(ui,"Material name",&mut d.value["name"]);if self.button(ui,"Save material",!self.remote.pending){self.command("materials.save_current",json!({"name":d.value["name"]}));}},
                        "close"=>{ui.label("Close this design? Unsaved work is kept as a session draft when replaced; save a library copy to keep it permanently.");ui.horizontal(|ui|{if self.button(ui,"Save first",true){self.close_after_save=true;self.save(false);done=true;}
if self.button(ui,"Close design",!self.remote.pending){self.command("design.close",json!({}));done=true;}
if self.button(ui,"Cancel",true){done=true;}});},
                        "save-conflict"=>{
                            ui.label("Another device saved a newer library revision. Your current work is still here.");
                            if self.button(ui,"Reload saved project",!self.remote.pending) && let Some(saved)=self.remote.snapshot.design.as_ref().and_then(|d|d.saved.as_ref()){
                                self.send("open",json!({"project":{"job":saved.job},"replace_unsaved":true}));
                            }
                            if self.button(ui,"Save As a new project",!self.remote.pending){self.save(true);}
                        },
                        "delete"=>{
                            ui.label(format!("Delete {} from the library? Open designs keep their current settings.",text(&d.value["name"])));
                            if self.button(ui,"Delete preset",!self.remote.pending){self.command(if d.value["material"]==true{"materials.delete"}else{"mats.delete"},json!({"name":d.value["name"]}));done=true;}
                            if self.button(ui,"Cancel deletion",true){self.open_dialog(if d.value["material"]==true{"materials"}else{"mats"},"Library");}
                        },
                        "history"=>{for kind in ["undo","redo"]{ui.heading(kind);for step in list(&self.remote.snapshot.ui.history[kind]).iter().rev(){ui.label(format!("{} — {}",text(&step["label"]),text(&step["origin"])));}}},
                        "preferences"=>{
                            ui.heading("Interface");if ui.checkbox(&mut self.dark,"Dark appearance").changed(){self.send("preference",json!({"dark":self.dark}));}
                            self.value_field(ui,"Text size (10–24 points)",self.remote.snapshot.ui.font_size as f64,"settings.font_size","points",None);
                            for units in ["millimetres","inches"]{if ui.radio(self.remote.snapshot.ui.units==units,units).clicked(){self.command("settings.units",json!({"units":units}));}}
                            if self.button(ui,"Restore default text size",true){self.command("settings.font_size",json!({"points":14}));}
                        },
                        "library"|"uploads"=>{
                            if d.kind=="library" {
                                let path=text(&d.value["path"]).to_owned();
                                ui.label(if path.is_empty(){"Library"}else{&path});
                                if !path.is_empty() && self.button(ui,"Parent folder",!self.remote.pending) {
                                    let parent=path.rsplit_once('/').map(|v|v.0).unwrap_or("");
                                    d.value["path"]=json!(parent);self.send("library",json!({"path":parent}));
                                }
                                for folder in list(&self.remote.library["folders"]) {
                                    let name=text(&folder);
                                    if self.button(ui,&format!("Folder: {name}"),!self.remote.pending){
                                        let next=if path.is_empty(){name.to_owned()}else{format!("{path}/{name}")};
                                        d.value["path"]=json!(next);self.send("library",json!({"path":next}));
                                    }
                                }
                            }
                            let items=if d.kind=="uploads"{list(&self.remote.uploads["designs"])}else{list(&self.remote.library["jobs"])};
                            ui.add(egui::TextEdit::singleline(&mut d.search).hint_text("Search"));
                            for item in items {let name=text(&item["name"]);if !name.to_lowercase().contains(&d.search.to_lowercase()){continue;}
                                if self.button(ui,name,!self.remote.pending){let body=if d.kind=="uploads"{json!({"design":item["id"]})}else{json!({"project":{"job":item["id"]}})};self.send("open",body);}
                            }

                        },
                        "about"=>{ui.heading("CutCraft");ui.label(format!("Version {}",env!("CARGO_PKG_VERSION")));ui.label("Prepare, arrange and cut your designs. Your files stay on your CutCraft server.");if self.button(ui,"Font licences",true){self.open_dialog("font-licences","Bundled font licences");}
if self.button(ui,"User manual",true){self.send("link",json!({"url":"https://github.com/mkrzywonski/cutcraft-web/blob/main/docs/help/design-guide.md"}));}},
                        "font-licences"=>help(ui,include_str!("../../../docs/help/font-licences.md")),
                        "troubleshooting"=>help(ui,include_str!("../../../docs/help/troubleshooting.md")),
                        "ai-help"=>help(ui,include_str!("../../../docs/help/mcp.md")),
                        "design-help"=>help(ui,include_str!("../../../docs/help/design-guide.md")),
                        _=>{}
                    }
                });
            });
        if modal.should_close() {
            open = false;
        }
        if !open || (done && self.dialog.is_none()) {
            self.close_after_save = false;
        }
        if open && !done && self.dialog.is_none() {
            self.dialog = Some(d);
        }
    }
    fn library_dialog(&mut self, ui: &mut Ui, d: &mut Dialog) {
        let material = d.kind == "materials";
        let entries = if material {
            self.remote.snapshot.ui.materials.clone()
        } else {
            self.remote.snapshot.ui.mats.clone()
        };
        ui.add(egui::TextEdit::singleline(&mut d.search).hint_text("Search by name"));
        ui.horizontal_wrapped(|ui| {
            if self.button(ui, "New…", true) {
                let mut value = entries.first().cloned().unwrap_or(json!({}));
                value["name"] = json!("");
                self.edit_dialog(material, value, None);
            }
            if self.button(ui, "Edit…", d.selected.is_some())
                && let Some(value) = entries
                    .iter()
                    .find(|v| Some(text(&v["name"]).to_string()) == d.selected)
            {
                self.edit_dialog(material, value.clone(), d.selected.clone());
            }
            if self.button(ui, "Duplicate", d.selected.is_some())
                && let Some(value) = entries
                    .iter()
                    .find(|v| Some(text(&v["name"]).to_string()) == d.selected)
            {
                let mut value = value.clone();
                value["name"] = json!(format!("{} copy", text(&value["name"])));
                self.edit_dialog(material, value, None);
            }
            if self.button(ui, "Delete", d.selected.is_some() && !self.remote.pending) {
                self.dialog = Some(Dialog {
                    kind: "delete".into(),
                    title: "Delete preset?".into(),
                    value: json!({"name":d.selected,"material":material}),
                    ..Default::default()
                });
            }
            if self.button(ui, "Use as default", d.selected.is_some()) {
                self.command(
                    "settings.new_design",
                    if material {
                        json!({"material":d.selected})
                    } else {
                        json!({"mat":d.selected})
                    },
                );
            }
        });
        ui.separator();
        for value in entries {
            let name = text(&value["name"]);
            if !name.to_lowercase().contains(&d.search.to_lowercase()) {
                continue;
            }
            egui::Frame::group(ui.style())
                .inner_margin(12)
                .show(ui, |ui| {
                    let r = ui.selectable_label(
                        d.selected.as_deref() == Some(name),
                        egui::RichText::new(name).strong(),
                    );
                    if r.clicked() {
                        d.selected = Some(name.into());
                    }
                    if r.double_clicked() {
                        self.edit_dialog(material, value.clone(), Some(name.into()));
                    }
                    if material {
                        ui.weak(format!(
                            "Blade {} · Force {} · Speed {} · {} pass(es)",
                            value["blade"], value["force"], value["speed"], value["passes"]
                        ));
                    } else {
                        ui.weak(format!(
                            "{:.1} × {:.1} mm",
                            number(&value["outer_mm"][0]),
                            number(&value["outer_mm"][1])
                        ));
                    }
                });
            ui.add_space(6.);
        }
    }
    fn edit_dialog(&mut self, material: bool, value: Value, original: Option<String>) {
        self.dialog = Some(Dialog {
            kind: if material {
                "material-edit"
            } else {
                "mat-edit"
            }
            .into(),
            title: if material {
                "Edit material"
            } else {
                "Edit mat"
            }
            .into(),
            value,
            original,
            ..Default::default()
        });
    }
    fn editor(&mut self, ui: &mut Ui, d: &mut Dialog) {
        let material = d.kind == "material-edit";
        string_field(ui, "Name", &mut d.value["name"]);
        if material {
            for (key, label, min, max) in [
                ("blade", "Blade depth", 1., 10.),
                ("force", "Force", 1., 33.),
                ("speed", "Speed", 1., 10.),
                ("passes", "Passes", 1., 3.),
            ] {
                numeric_field(ui, label, &mut d.value[key], min, max, true);
            }
            for (key, label) in [
                ("mirror", "Mirrored"),
                ("roll", "Suitable for roll cutting"),
            ] {
                let mut v = d.value[key] == true;
                if ui.checkbox(&mut v, label).changed() {
                    d.value[key] = json!(v);
                }
            }
            ui.label("Loading notes");
            let mut notes = text(&d.value["notes"]).to_owned();
            if ui.text_edit_multiline(&mut notes).changed() {
                d.value["notes"] = json!(notes);
            }
            egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
                let mut method = text(&d.value["offset_method"]).to_owned();
                egui::ComboBox::from_label("Blade compensation")
                    .selected_text(&method)
                    .show_ui(ui, |ui| {
                        for m in ["off", "software", "machine"] {
                            ui.selectable_value(&mut method, m.into(), m);
                        }
                    });
                d.value["offset_method"] = json!(method);
                numeric_field(
                    ui,
                    "Blade offset (mm)",
                    &mut d.value["offset_mm"],
                    0.,
                    1.,
                    false,
                );
                numeric_field(
                    ui,
                    "Overcut (mm)",
                    &mut d.value["overcut_mm"],
                    0.,
                    2.,
                    false,
                );
            });
        } else {
            for (key, label, n) in [
                ("outer_mm", "Outer size (width / height)", 2),
                ("sticky_mm", "Sticky grid (left / top / right / bottom)", 4),
                ("grid_origin_mm", "Ruler origin (left / top)", 2),
                ("cut_origin_mm", "Cutter start corner (left / top)", 2),
            ] {
                ui.label(label);
                for i in 0..n {
                    numeric_field(
                        ui,
                        &format!("{label} {} (mm)", i + 1),
                        &mut d.value[key][i],
                        0.,
                        2000.,
                        false,
                    );
                }
            }
            for i in 0..4 {
                let mut unit = text(&d.value["rulers"][i]).to_owned();
                egui::ComboBox::from_id_salt(("ruler", i))
                    .selected_text(&unit)
                    .show_ui(ui, |ui| {
                        for u in ["inches", "centimetres", "none"] {
                            ui.selectable_value(&mut unit, u.into(), u);
                        }
                    });
                d.value["rulers"][i] = json!(unit);
            }
            let mut class = text(&d.value["class"]).to_owned();
            egui::ComboBox::from_label("Load length")
                .selected_text(&class)
                .show_ui(ui, |ui| {
                    for c in ["mat12x12", "mat12x24"] {
                        ui.selectable_value(&mut class, c.into(), c);
                    }
                });
            d.value["class"] = json!(class);
            let mut approximate = d.value["approximate"] == true;
            if ui
                .checkbox(&mut approximate, "Approximate measurements")
                .changed()
            {
                d.value["approximate"] = json!(approximate);
            }
        }
        ui.separator();
        ui.horizontal(|ui| {
            if self.button(ui, "Save changes", !self.remote.pending) {
                let params = if material {
                    json!({"material":d.value,"rename_from":d.original})
                } else {
                    json!({"mat":d.value,"rename_from":d.original})
                };
                self.command(
                    if material {
                        "materials.save"
                    } else {
                        "mats.save"
                    },
                    params,
                );
            }
            if self.button(ui, "Cancel edit", true) {
                self.open_dialog(
                    if material { "materials" } else { "mats" },
                    if material { "Materials" } else { "Mats" },
                );
            }
        });
    }
}
fn string_field(ui: &mut Ui, label: &str, v: &mut Value) {
    ui.label(label);
    let mut s = text(v).to_owned();
    if ui.text_edit_singleline(&mut s).changed() {
        *v = json!(s);
    }
}
fn numeric_field(ui: &mut Ui, label: &str, v: &mut Value, min: f64, max: f64, integer: bool) {
    ui.horizontal(|ui| {
        ui.label(label);
        let mut n = number(v);
        if ui
            .add(
                egui::DragValue::new(&mut n)
                    .range(min..=max)
                    .speed(if integer { 1. } else { 0.1 }),
            )
            .changed()
        {
            *v = if integer { json!(n as u64) } else { json!(n) };
        }
    });
}
fn help(ui: &mut Ui, source: &str) {
    for line in source.lines() {
        if line.starts_with('#') {
            ui.heading(line.trim_start_matches('#').trim());
        } else if !line.is_empty() {
            let mut rest = line;
            ui.horizontal_wrapped(|ui| {
                while let Some(start) = rest.find('[') {
                    let Some(mid) = rest[start..].find("](").map(|n| n + start) else {
                        break;
                    };
                    let Some(end) = rest[mid + 2..].find(')').map(|n| n + mid + 2) else {
                        break;
                    };
                    ui.label(&rest[..start]);
                    ui.hyperlink_to(&rest[start + 1..mid], &rest[mid + 2..end]);
                    rest = &rest[end + 1..];
                }
                ui.label(rest);
            });
        } else {
            ui.add_space(6.);
        }
    }
}
