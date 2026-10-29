use crate::{Desktop, list, number, text};
use egui::Ui;
use serde_json::{Value, json};
pub const GROUPS: [&str; 10] = [
    "Issues",
    "Info",
    "Artwork size",
    "What to cut",
    "Material",
    "Mat",
    "Media",
    "Cut lines",
    "Cutting aids",
    "Estimate",
];
impl Desktop {
    pub fn panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.strong(if self.ready() {
                "Ready to cut"
            } else {
                "Review the design"
            });
        });
        egui::ComboBox::from_id_salt("selected-cutter")
            .width(ui.available_width() - 15.)
            .selected_text(if self.cutter.is_empty() {
                "Choose cutter"
            } else {
                &self.cutter
            })
            .show_ui(ui, |ui| {
                for c in list(&self.remote.state["cutters"]) {
                    ui.selectable_value(
                        &mut self.cutter,
                        text(&c["id"]).into(),
                        text(&c["view"]["name"]),
                    );
                }
            });
        ui.separator();
        let mut groups: Vec<String> = list(&self.remote.snapshot.ui.panel["order"])
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        for group in GROUPS {
            if !groups.iter().any(|g| g == group) {
                groups.push(group.into());
            }
        }
        let locked = list(&self.remote.snapshot.ui.panel["locked"]);
        if !locked.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("locked-groups")
                .max_height(ui.available_height() * 0.5)
                .show(ui, |ui| {
                    for group in &groups {
                        if locked.iter().any(|g| g == group) {
                            self.group(ui, group);
                        }
                    }
                });
        }
        egui::ScrollArea::vertical()
            .id_salt("panel-scroll")
            .show(ui, |ui| {
                for group in &groups {
                    if !locked.iter().any(|g| g == group) {
                        self.group(ui, group);
                    }
                }
            });
    }
    fn group(&mut self, ui: &mut Ui, group: &str) {
        let Some(group) = GROUPS.iter().copied().find(|g| *g == group) else {
            return;
        };
        let locked = list(&self.remote.snapshot.ui.panel["locked"])
            .iter()
            .any(|v| v == group);
        crate::chrome::set_panel_pass(
            ui,
            Some(crate::chrome::PanelPass {
                group,
                locked,
                open: self.remote.preferences["folds"][group].as_bool(),
            }),
        );
        let issues = self.remote.snapshot.design.as_ref().map_or(0, |d| d.issues);
        let title = if group == "Issues" {
            format!(
                "Issues · {}",
                if issues == 0 {
                    "Ready".into()
                } else {
                    format!("{issues} to resolve")
                }
            )
        } else {
            group.into()
        };
        crate::chrome::flagged_section(
            ui,
            &title,
            !matches!(group, "Info" | "Cutting aids"),
            if group == "Issues" && issues > 0 {
                Some(crate::chrome::CAUTION)
            } else {
                None
            },
            |ui| self.group_body(ui, group),
        );
        crate::chrome::set_panel_pass(ui, None);
        for event in crate::chrome::take_panel_events(ui) {
            match event {
                crate::chrome::PanelEvent::Open(group, open) => {
                    self.send("fold", json!({"group":group,"open":open}))
                }
                crate::chrome::PanelEvent::ToggleLock(group) => {
                    let mut locked = list(&self.remote.snapshot.ui.panel["locked"]);
                    if locked.iter().any(|g| g == &group) {
                        locked.retain(|g| g != &group);
                    } else {
                        locked.push(json!(group));
                    }
                    self.command("settings.panel", json!({"locked":locked}));
                }
                crate::chrome::PanelEvent::Move {
                    group,
                    target,
                    after,
                } => {
                    let mut order: Vec<_> = list(&self.remote.snapshot.ui.panel["order"])
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect();
                    for g in GROUPS {
                        if !order.iter().any(|v| v == g) {
                            order.push(g.into());
                        }
                    }
                    order.retain(|g| g != &group);
                    let at = order.iter().position(|g| g == &target).unwrap_or(0);
                    order.insert(at + usize::from(after), group);
                    self.command("settings.panel", json!({"order":order}));
                }
            }
        }
    }
    fn group_body(&mut self, ui: &mut Ui, group: &str) {
        let data = self.remote.snapshot.ui.clone();
        let sheet = data.sheet.clone();
        match group {
            "Issues" | "Info" => {
                let mut entries = Vec::new();
                if group == "Issues" {
                    for item in list(&data.report["pending"]) {
                        entries.push((question_title(&item), item, 0));
                    }
                }
                for item in list(&data.report["changes"]) {
                    let resolved = resolved_title(&item);
                    if (group == "Issues") == resolved.is_some() {
                        entries.push((
                            resolved.unwrap_or_else(|| info_title(&item)),
                            item,
                            if group == "Issues" { 2 } else { 3 },
                        ));
                    }
                }
                for item in list(&data.report["diagnostics"])
                    .into_iter()
                    .chain(list(&data.plan["diagnostics"]))
                {
                    let blocking = item["severity"] == "blocking";
                    if blocking == (group == "Issues") {
                        entries.push((
                            if blocking {
                                problem_title(&item)
                            } else {
                                info_title(&item)
                            },
                            item,
                            if blocking { 1 } else { 3 },
                        ));
                    }
                }
                if entries.is_empty() {
                    ui.weak(if group == "Issues" {
                        "Nothing to resolve."
                    } else {
                        "No preparation notes."
                    });
                }
                for (index, (title, item, kind)) in entries.into_iter().enumerate() {
                    let colour = match kind {
                        0 => egui::Color32::from_rgb(160, 110, 20),
                        1 => egui::Color32::from_rgb(180, 50, 40),
                        2 => egui::Color32::from_rgb(25, 120, 75),
                        _ => egui::Color32::from_rgb(40, 100, 160),
                    };
                    egui::Frame::new()
                        .stroke(egui::Stroke::new(1., colour.gamma_multiply(0.5)))
                        .inner_margin(8)
                        .corner_radius(5)
                        .show(ui, |ui| {
                            egui::CollapsingHeader::new(egui::RichText::new(title).color(colour))
                                .id_salt((group, index))
                                .default_open(kind < 2)
                                .show(ui, |ui| {
                                    ui.label(text(&item["message"]));
                                    ui.horizontal_wrapped(|ui| {
                                        for choice in list(&item["choices"]) {
                                            if self.button(
                                                ui,
                                                &choice_label(&choice),
                                                !self.remote.pending,
                                            ) {
                                                self.command(
                                                    "prepare.decide",
                                                    json!({"decision":choice}),
                                                );
                                            }
                                        }
                                    });
                                    if item["code"]=="picture.traced" && self.button(ui,"Trace settings…",!self.remote.pending) {
                                let value=list(&data.report["recipe"]["decisions"]).into_iter().find(|d|d["decision"]=="trace").unwrap_or(json!({"decision":"trace","colours":"one","threshold":128,"detail":95,"smoothness":20,"ignore_white":true}));
                                self.open_dialog("trace","Trace picture");
                                if let Some(d)=&mut self.dialog {d.value=value;}
                            }
                            let ids: Vec<String> = list(&item["source_ids"])
                                        .iter()
                                        .filter_map(|v| v.as_str().map(str::to_owned))
                                        .collect();
                                    if !ids.is_empty()
                                        && self.button(
                                            ui,
                                            if self.highlight == ids {
                                                "Stop showing"
                                            } else {
                                                "Show me"
                                            },
                                            true,
                                        )
                                    {
                                        if self.highlight == ids {
                                            self.highlight.clear();
                                        } else {
                                            self.highlight = ids;
                                            self.zoom = 1.;
                                            self.pan = egui::Vec2::ZERO;
                                            if self.screen() == "mat" {
                                                self.command(
                                                    "view.show",
                                                    json!({"screen":"side-by-side"}),
                                                );
                                            }
                                        }
                                    }
                                });
                        });
                }
                if group == "Issues" {
                    ui.horizontal_wrapped(|ui| {
                        self.action_button(ui, "Run again", "prepare.run", json!({}));
                        self.action_button(ui, "Clear answers", "prepare.reset", json!({}));
                        if self.button(ui, "Design help", true) {
                            self.open_dialog("design-help", "Preparing designs");
                        }
                    });
                }
            }
            "Artwork size" => {
                if let Some([w, h]) = data.size_mm {
                    ui.label("Linked proportions");
                    self.length_field(ui, "Width", w, "design.size", "width_mm");
                    self.length_field(ui, "Height", h, "design.size", "height_mm");
                    ui.horizontal_wrapped(|ui| {
                        self.action_button(
                            ui,
                            "Fit design to mat",
                            "design.size",
                            json!({"fit":true}),
                        );
                        self.action_button(ui, "Original size", "design.size", json!({"scale":1}));
                    });
                }
            }
            "What to cut" => {
                self.peek.clear();
                ui.horizontal(|ui| {
                    for (by, label) in [("colour", "Colour"), ("layer", "Layer")] {
                        if ui.selectable_label(data.group_by == by, label).clicked() {
                            self.command("rows.group", json!({"by":by}));
                        }
                    }
                });
                for row in data.rows {
                    let mut included = row["included"] == true;
                    let r = ui.horizontal(|ui| {
                        if ui.checkbox(&mut included, "").changed() {
                            self.command(
                                "cut.include",
                                json!({"row":row["row"],"include":included}),
                            );
                        }
                        let hex = text(&row["row"]["colour"]["hex"]);
                        if !hex.is_empty() {
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(14., 14.), egui::Sense::hover());
                            ui.painter()
                                .rect_filled(rect, 2, crate::canvas::colour(hex));
                            ui.painter().rect_stroke(
                                rect,
                                2,
                                egui::Stroke::new(0.5, egui::Color32::GRAY),
                                egui::StrokeKind::Inside,
                            );
                        }
                        ui.label(text(&row["label"]));
                        ui.weak(format!(
                            "{} pieces, {} lines",
                            row["pieces"], row["cut_lines"]
                        ));
                    });
                    if r.response.hovered() {
                        self.peek = list(&row["items"])
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect();
                    }
                }
                if number(&data.report["stroked_fills"]) > 0. {
                    ui.label("Outlines on filled shapes");
                    for (role, label) in [
                        ("part_of_shape", "Part of the shape"),
                        ("shape", "Cut separately"),
                    ] {
                        if self.button(ui, label, !self.remote.pending) {
                            self.command(
                                "prepare.decide",
                                json!({"decision":{"decision":"outlines_on_fills","role":role}}),
                            );
                        }
                    }
                }
            }
            "Material" => {
                egui::ComboBox::from_id_salt("material")
                    .width(ui.available_width() - 15.)
                    .selected_text(text(&sheet["material"]["name"]))
                    .show_ui(ui, |ui| {
                        for m in &data.materials {
                            let name = text(&m["name"]);
                            if ui
                                .selectable_label(sheet["material"]["name"] == name, name)
                                .clicked()
                            {
                                self.command("sheet.set", json!({"material":name}));
                            }
                        }
                    });
                for (key, label) in [
                    ("blade", "Blade depth"),
                    ("force", "Force"),
                    ("speed", "Speed"),
                    ("passes", "Passes"),
                ] {
                    self.value_field(
                        ui,
                        label,
                        number(&sheet["material"][key]),
                        "sheet.adjust",
                        key,
                        None,
                    );
                }
                let mut mirrored = sheet["mirror"] == true;
                if ui.checkbox(&mut mirrored, "Mirrored").changed() {
                    self.command("sheet.adjust", json!({"mirror":mirrored}));
                }
                self.length_field(
                    ui,
                    "Overcut",
                    number(&sheet["material"]["overcut_mm"]),
                    "sheet.adjust",
                    "overcut_mm",
                );
                egui::CollapsingHeader::new("Blade compensation").show(ui, |ui| {
                    for method in ["off", "software", "machine"] {
                        if ui
                            .selectable_label(sheet["material"]["offset_method"] == method, method)
                            .clicked()
                        {
                            self.command("sheet.adjust", json!({"offset_method":method}));
                        }
                    }
                    self.length_field(
                        ui,
                        "Blade offset",
                        number(&sheet["material"]["offset_mm"]),
                        "sheet.adjust",
                        "offset_mm",
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    self.action_button(ui, "Save settings", "materials.save_current", json!({}));
                    if self.button(ui, "Save settings as…", self.has_design()) {
                        self.open_dialog("material-save", "Save material as");
                        if let Some(d) = &mut self.dialog {
                            d.value = sheet["material"].clone();
                        }
                    }
                    if self.button(ui, "Edit materials…", true) {
                        self.open_dialog("materials", "Materials");
                    }
                });
            }
            "Mat" => {
                egui::ComboBox::from_id_salt("mat")
                    .width(ui.available_width() - 15.)
                    .selected_text(text(&sheet["mat_profile"]["name"]))
                    .show_ui(ui, |ui| {
                        for mat in &data.mats {
                            let name = text(&mat["name"]);
                            if ui
                                .selectable_label(sheet["mat_profile"]["name"] == name, name)
                                .clicked()
                            {
                                self.command("sheet.set", json!({"mat_profile":name}));
                            }
                        }
                    });
                if self.button(ui, "Edit mats…", true) {
                    self.open_dialog("mats", "Mats");
                }
                let mut snap = data.snap["enabled"] == true;
                if ui.checkbox(&mut snap, "Snap to grid").changed() {
                    self.command("settings.snap", json!({"enabled":snap}));
                }
                self.length_field(
                    ui,
                    "Grid step",
                    number(&data.snap["step_mm"]),
                    "settings.snap",
                    "step_mm",
                );
            }
            "Media" => {
                let mut piece = sheet["piece_mm"].is_array();
                if ui
                    .checkbox(&mut piece, "Cutting a piece of material")
                    .changed()
                {
                    self.command(
                        "sheet.piece",
                        if piece {
                            json!({"width_mm":250.,"height_mm":250.})
                        } else {
                            json!({})
                        },
                    );
                }
                if piece {
                    for (key, label, index, field) in [
                        ("width_mm", "Media width", 0, "piece_mm"),
                        ("height_mm", "Media height", 1, "piece_mm"),
                        ("x_mm", "Media left", 0, "piece_at_mm"),
                        ("y_mm", "Media top", 1, "piece_at_mm"),
                    ] {
                        self.length_field(
                            ui,
                            label,
                            number(&sheet[field][index]),
                            "sheet.piece",
                            key,
                        );
                    }
                    if self.button(ui, "Media to grid corner", !self.remote.pending) {
                        let m = &sheet["mat_profile"];
                        self.command("sheet.piece",json!({"x_mm":number(&m["sticky_mm"][0])-number(&m["cut_origin_mm"][0]),"y_mm":number(&m["sticky_mm"][1])-number(&m["cut_origin_mm"][1])}));
                    }
                } else {
                    ui.weak("The whole usable mat area is available.");
                }
            }
            "Cut lines" => {
                self.length_field(
                    ui,
                    "Left",
                    number(&sheet["placement"]["offset_mm"][0]),
                    "sheet.place",
                    "x",
                );
                self.length_field(
                    ui,
                    "Top",
                    number(&sheet["placement"]["offset_mm"][1]),
                    "sheet.place",
                    "y",
                );
                let angle = number(&sheet["placement"]["rotation_deg"]);
                self.value_field(ui, "Rotation °", angle, "sheet.place", "rotate", None);
                ui.horizontal_wrapped(|ui| {
                    self.action_button(ui, "Start corner", "sheet.place", json!({"corner":true}));
                    if self.button(ui, "Turn 90°", !self.remote.pending) {
                        self.command("sheet.place", json!({"rotate":(angle+90.)%360.}));
                    }
                    if self.button(ui, "Centre on material", !self.remote.pending)
                        && let Some(b) = self.remote.geometry.art_box_mm
                    {
                        let m = self.remote.geometry.mat.clone().unwrap_or_default();
                        let r = if let (Some(w), Some(h)) =
                            (sheet["piece_mm"][0].as_f64(), sheet["piece_mm"][1].as_f64())
                        {
                            let x = number(&sheet["piece_at_mm"][0]) + m.origin_mm[0];
                            let y = number(&sheet["piece_at_mm"][1]) + m.origin_mm[1];
                            [x, y, x + w, y + h]
                        } else {
                            m.sticky_mm
                        };
                        let x = number(&sheet["placement"]["offset_mm"][0])
                            + (r[0] + r[2] - b[0] - b[2]) / 2.;
                        let y = number(&sheet["placement"]["offset_mm"][1])
                            + (r[1] + r[3] - b[1] - b[3]) / 2.;
                        self.command("sheet.place", json!({"x":x,"y":y}));
                    }
                    self.action_button(ui, "Onto grid", "sheet.place", json!({"onto_grid":true}));
                });
                ui.checkbox(&mut self.tool_motion, "Show blade-up travel");
                ui.checkbox(&mut self.show_test, "Show test spot");
                ui.add(egui::Slider::new(&mut self.order, 0.0..=1.0).text("Cut order"));
                if self.button(
                    ui,
                    if self.playing {
                        "Pause preview"
                    } else {
                        "Play preview"
                    },
                    true,
                ) {
                    self.playing = !self.playing;
                }
            }
            "Cutting aids" => {
                for (key, label) in [
                    ("weeding_box", "Weeding box"),
                    ("weed_lines", "Weed lines"),
                    ("weed_inverted", "Invert (stencil)"),
                    ("layering_marks", "Layering marks"),
                ] {
                    let mut value = sheet[key] == true;
                    if ui.checkbox(&mut value, label).changed() {
                        self.command("sheet.set", json!({key:value}));
                    }
                }
                self.length_field(
                    ui,
                    "Weeding margin",
                    number(&sheet["weeding_margin_mm"]),
                    "sheet.set",
                    "weeding_margin_mm",
                );
                self.action_button(
                    ui,
                    "Place test spot automatically",
                    "sheet.test_spot",
                    json!({}),
                );
            }
            "Estimate" => {
                if let Some(s) = self
                    .remote
                    .snapshot
                    .design
                    .as_ref()
                    .and_then(|d| d.sheet.as_ref())
                {
                    ui.label(format!(
                        "Estimated time: {:.0} sec",
                        s.estimate_s.unwrap_or(0.)
                    ));
                    ui.label(if s.plan_valid {
                        "Fits the cutting area"
                    } else {
                        "Review placement before cutting"
                    });
                } else {
                    ui.weak("Open a design to see an estimate.");
                }
                egui::ComboBox::from_id_salt("panel-cutter")
                    .selected_text(if self.cutter.is_empty() {
                        "Choose cutter"
                    } else {
                        &self.cutter
                    })
                    .show_ui(ui, |ui| {
                        for c in list(&self.remote.state["cutters"]) {
                            let id = text(&c["id"]);
                            ui.selectable_value(&mut self.cutter, id.into(), text(&c["name"]));
                        }
                    });
            }
            _ => {}
        }
    }
    pub fn length_field(&mut self, ui: &mut Ui, label: &str, value: f64, command: &str, key: &str) {
        let inches = self.remote.snapshot.ui.units == "inches";
        self.value_field(ui, label, value, command, key, Some(inches));
    }

    pub(crate) fn value_field(
        &mut self,
        ui: &mut Ui,
        label: &str,
        value: f64,
        command: &str,
        key: &str,
        length: Option<bool>,
    ) {
        let factor = if length == Some(true) { 25.4 } else { 1. };
        let field = format!("{command}/{label}");
        ui.horizontal(|ui| {
            ui.label(label);
            let edit = self.fields.entry(field.clone()).or_insert_with(||format!("{:.*}", if length.is_none(){0}else if length==Some(true){2}else{3}, value/factor));
            let r = ui.add_enabled(!self.remote.pending && self.remote.connected, egui::TextEdit::singleline(edit).desired_width(82.));
            let parsed = if let Some(inches) = length {parse_length(edit, inches)} else {edit.trim().parse::<f64>().ok().filter(|n|n.is_finite())};
            let submit = r.lost_focus() || (r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            if !r.has_focus() && !submit && !self.edit_fences.contains_key(&field) { *edit = format!("{:.*}", if length.is_none(){0}else if length==Some(true){2}else{3}, value/factor); }
            if r.gained_focus() { self.edit_fences.insert(field.clone(), (self.remote.snapshot.incarnation.clone(), self.remote.snapshot.revision, edit.clone())); }
            let changed=self.edit_fences.get(&field).is_some_and(|(_,_,original)|original!=edit);
            self.record(label, &r);
            if submit && self.edit_fences.contains_key(&field) {
                if !changed {self.edit_fences.remove(&field);r.surrender_focus();}
                else if let Some(mut n) = parsed {
                    r.surrender_focus();
                    if length.is_none() && key != "rotate" { n = n.round(); }
                    if (n-value).abs()>0.0001 {
                        let mut params = json!({key:n});
                        if length.is_none() && key != "rotate" {params = json!({key:n as i64});}
                        if command=="sheet.piece" && matches!(key,"width_mm"|"height_mm") {
                            let piece=&self.remote.snapshot.ui.sheet["piece_mm"];
                            params=json!({"width_mm":if key=="width_mm"{n}else{piece[0].as_f64().unwrap_or(250.)},"height_mm":if key=="height_mm"{n}else{piece[1].as_f64().unwrap_or(250.)}});
                        }
                        let (incarnation,revision,_)=self.edit_fences.remove(&field).unwrap_or((self.remote.snapshot.incarnation.clone(),self.remote.snapshot.revision,String::new()));
                        self.send("command",json!({"id":command,"params":params,"incarnation":incarnation,"revision":revision}));
                    }
                } else { ui.colored_label(egui::Color32::RED,"Enter a valid number"); }
            }
            if let Some(inches) = length { ui.label(if inches {"in"} else {"mm"}); }
        });
    }
}
pub fn parse_length(input: &str, inches: bool) -> Option<f64> {
    let t = input.trim().to_lowercase();
    let (t, factor) = if let Some(s) = t.strip_suffix("mm") {
        (s, 1.)
    } else if let Some(s) = t.strip_suffix("cm") {
        (s, 10.)
    } else if let Some(s) = t.strip_suffix("in") {
        (s, 25.4)
    } else if let Some(s) = t.strip_suffix('"') {
        (s, 25.4)
    } else {
        (t.as_str(), if inches { 25.4 } else { 1. })
    };
    let t = t.trim();
    let negative = t.starts_with('-');
    let t = t.strip_prefix(['-', '+']).unwrap_or(t);
    let parts: Vec<_> = t.split_whitespace().collect();
    if parts.is_empty() || parts.len() > 2 {
        return None;
    }
    let mut total = 0.;
    for (index, part) in parts.iter().enumerate() {
        if part.starts_with(['-', '+']) {
            return None;
        }
        let n = if let Some((a, b)) = part.split_once('/') {
            if parts.len() == 2 && index == 0 {
                return None;
            }
            let a = a.parse::<f64>().ok()?;
            let b = b.parse::<f64>().ok()?;
            if a < 0. || b <= 0. {
                return None;
            }
            a / b
        } else {
            if parts.len() == 2 && index == 1 {
                return None;
            }
            part.parse::<f64>().ok()?
        };
        total += n;
    }
    let result = total * factor * if negative { -1. } else { 1. };
    result.is_finite().then_some(result)
}

pub fn choice_label(choice: &serde_json::Value) -> String {
    match choice["decision"].as_str() {
        Some("dxf_units") => match choice["unit"].as_str() {
            Some("in") => "Inches",
            Some("mm") => "Millimetres",
            Some("cm") => "Centimetres",
            Some("ft") => "Feet",
            Some("m") => "Metres",
            _ => "Choose units",
        }
        .into(),
        Some("background") if choice["cut"] == true => "Cut it".into(),
        Some("background") => "Don't cut".into(),
        Some("page") => {
            let page = choice["page"].as_str().unwrap_or_default();
            let number = page
                .strip_prefix("page/")
                .and_then(|n| n.parse::<usize>().ok());
            number.map_or_else(|| page.into(), |n| format!("Page {}", n + 1))
        }
        Some("font_substitute") => "Use this substitute".into(),
        Some("font_from_file") => "Use the letter shapes saved in the file".into(),
        Some("exclude_picture") | Some("exclude_object") => "Don't cut".into(),
        Some("fit_artwork") => "Use the artwork's area".into(),
        Some("keep_page") => "Trim the art to the page".into(),
        Some("accept_text_overflow") => "Leave out the text that doesn't fit".into(),
        Some("cut_solid") => match choice["colour"].as_str() {
            Some(hex) => format!("Cut in {hex}"),
            None => "Cut as solid".into(),
        },
        Some("picture_size") => format!("Use {} mm", choice["width_mm"]),
        Some("picture_frame") => format!("Frame {}", choice["frame"].as_u64().unwrap_or(0) + 1),
        Some("trace") if choice["colours"] == "several" => "Trace in several colours".into(),
        Some("trace") => "Trace in one colour".into(),
        Some("trace_photo") => "Trace it anyway".into(),
        Some("accept_warning") if choice["warning"] == "duplicate.near" => "Keep both".into(),
        Some("accept_warning") => "Cut them anyway".into(),
        Some("import_route") if choice["route"] == "pdf_appearance" => {
            "Use the page appearance".into()
        }
        Some("import_route") => "Use the editable layers".into(),
        Some("stroke_colour_role") if choice["role"] == "cut_line" => {
            "Cut these along their centre".into()
        }
        Some("stroke_colour_role") if choice["role"] == "part_of_shape" => {
            "Make them part of their shapes".into()
        }
        Some("stroke_colour_role") => "Cut around these as shapes".into(),
        Some("stroke_role") if choice["role"] == "cut_line" => "Cut along line".into(),
        Some("stroke_role") if choice["role"] == "part_of_shape" => "Part of the shape".into(),
        Some("stroke_role") => "Cut around shape".into(),
        Some("confirm_size") => format!(
            "Confirm {} × {} mm",
            choice["width_mm"], choice["height_mm"]
        ),
        _ => "Use this choice".into(),
    }
}

/// A short title for a question Make cut-ready asks.
fn question_title(question: &Value) -> String {
    let family = question["choices"]
        .as_array()
        .and_then(|choices| choices.iter().find_map(|c| c["family"].as_str()));
    let title = match question["decision"].as_str().unwrap_or_default() {
        "font_substitute" => {
            return family.map_or("Missing font".into(), |f| format!("Missing font: {f}"));
        }
        "background" => "Background box?",
        "appearance" => "Effect that can't be cut",
        "confirm_size" => "Check the size",
        "dxf_units" => "Which units does this drawing use?",
        "exclude_picture" => "Picture inside the design",
        "fit_artwork" => "Art outside the page",
        "import_route" => "Which version to open",
        "page" => "Which page to cut",
        "picture_frame" => "Which frame to cut",
        "picture_size" => "How big is the picture?",
        "trace_photo" => "This looks like a photo",
        "small_pieces" => "Very small pieces",
        "thin_pieces" => "Very thin pieces",
        "near_duplicates" => "Outlines that nearly repeat",
        "text_overflow" => "Text doesn't fit",
        _ => "Question",
    };
    title.into()
}

/// A short title for something that stops the design being cut.
pub(crate) fn problem_title(problem: &Value) -> String {
    let code = problem["code"].as_str().unwrap_or_default();
    match code {
        "plan.out_of_bounds" => return "Outside the cutting area".into(),
        "plan.outside_piece" => return "Outside your material".into(),
        "plan.empty" => return "Nothing to cut on this sheet".into(),
        _ => {}
    }
    let title = match code.split('.').next().unwrap_or_default() {
        "font" | "text" => "Text problem",
        "picture" | "trace" => "Picture problem",
        "import" => "Part of the file couldn't be read",
        "size" | "page" => "Size or page problem",
        "appearance" | "effect" => "Effect that can't be cut",
        _ => "Can't cut yet",
    };
    title.into()
}

/// A resolved question's title, or none for a routine change (text turned into shapes, …).
fn resolved_title(change: &Value) -> Option<String> {
    // "Using … for Imaginary Brush Script Regular (your choice)": the font it was about.
    let named = change["message"].as_str().and_then(|m| {
        let after = m.rsplit_once(" for ")?.1;
        Some(after.trim_end_matches(" (your choice)").to_owned())
    });
    let family = change["choices"]
        .as_array()
        .and_then(|choices| {
            choices
                .iter()
                .find_map(|c| c["family"].as_str().map(str::to_owned))
        })
        .or(named);
    let font = |what: &str| match &family {
        Some(f) => format!("Missing font {f}: {what}"),
        None => format!("Missing font: {what}"),
    };
    Some(match change["code"].as_str().unwrap_or_default() {
        "font.substituted" => font("replaced"),
        "font.from_file" => font("using the file's letter shapes"),
        "background.excluded" => "Background box: not cut".into(),
        "background.kept" => "Background box: cut".into(),
        "picture.sized" => "Picture size set".into(),
        "picture.photo_traced" => "Photo: traced anyway".into(),
        "piece.tiny.accepted" => "Very small pieces: cutting anyway".into(),
        "piece.thin.accepted" => "Very thin pieces: cutting anyway".into(),
        "duplicate.near.accepted" => "Nearly repeating outlines: keeping both".into(),
        "picture.frame" => "Frame chosen".into(),
        "page.chosen" => "Page chosen".into(),
        "page.fitted" => "Page fitted to the art".into(),
        "page.kept" => "Page kept: art trimmed to it".into(),
        "size.confirmed" => "Size confirmed".into(),
        "import.route" => "Version chosen".into(),
        "appearance.solid" => "Cut as solid".into(),
        "object.excluded" => "Left out".into(),
        "picture.excluded" => "Picture left out".into(),
        "text.overflow" => "Overflowing text left out".into(),
        _ => return None,
    })
}

/// A short title for something Make cut-ready did on its own, or a note: its kind, else the
/// start of its message.
pub(crate) fn info_title(item: &Value) -> String {
    let title = match item["code"].as_str().unwrap_or_default() {
        "text.outlined" => "Text turned into shapes",
        "text.empty" => "Empty text left out",
        "stroke.outlined" => "Lines turned into shapes",
        "stroke.cut_along" => "Thick lines cut along the middle",
        "stroke.shape_chosen" => "Lines cut as shapes of their own",
        "stroke.part_of_shape" | "stroke.joined_chosen" => "Outlines made part of their shapes",
        "union.joined" => "Overlapping shapes joined",
        "duplicate.removed" => "Duplicate outlines removed",
        "clip.trimmed" => "Hidden parts trimmed",
        "clip.hidden" => "Clipped-away art left out",
        "empty.excluded" => "Empty objects left out",
        "speck.dropped" | "plan.specks_skipped" => "Tiny specks left out",
        "page.outside" => "Art off the page left out",
        "page.trimmed" => "Art trimmed to the page",
        "page.art_bounds" => "Page set to the whole art",
        "ignored.hidden" => "Hidden art left out",
        "picture.traced" => "Picture traced",
        "picture.low_resolution" => "Low-resolution picture",
        "plan.off_mat_grid" => "Off the sticky grid",
        "plan.material_not_for_roll" => "Material not marked for a roll",
        "plan.outside_piece" => "Outside your material",
        "size.css_pixels_normalized" => "Pixel sizes read at 96 per inch",
        "svg.hidden_dropped" => "Hidden SVG parts skipped",
        "import.off_artboard_skipped" => "Art off the artboard skipped",
        "import.clipped_to_page" => "Art clipped to the page",
        "import.hidden_art_skipped" => "Hidden art skipped",
        "import.fonts_unavailable" => "Fonts not available",
        "import.text_outlines_from_file" => "Letter shapes taken from the file",
        "import.text_regrouped" => "Text regrouped",
        "import.layers_from_pdf" => "Layers read from the PDF",
        "import.eps_one_layer" => "EPS read as one layer",
        "import.eps_note" => "EPS note",
        "import.eps_preview" => "Only the EPS preview could be read",
        "import.appearance_expanded" => "Effects expanded",
        _ => return message_start(item["message"].as_str().unwrap_or("Note")),
    };
    title.into()
}

/// The first few words of a message, for a title.
fn message_start(message: &str) -> String {
    let first = message
        .split(['.', ';', ':', '('])
        .next()
        .unwrap_or(message)
        .trim();
    if first.chars().count() <= 40 {
        return first.to_owned();
    }
    let mut title = String::new();
    for word in first.split_whitespace() {
        if title.chars().count() + word.chars().count() > 36 {
            break;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(word);
    }
    title.push('…');
    title
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_units_and_fractional_lengths() {
        assert_eq!(parse_length("1 1/2 in", false), Some(38.099999999999994));
        assert_eq!(parse_length("2 cm", true), Some(20.));
        assert_eq!(parse_length("1/0", false), None);
        assert_eq!(parse_length("NaN", false), None);
        assert_eq!(parse_length("-1 1/2 mm", false), Some(-1.5));
        assert_eq!(parse_length("1 2", false), None);
        assert_eq!(parse_length("1/-2", false), None);
    }
}
