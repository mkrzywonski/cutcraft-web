//! A preparation worker (spec §5): one child process per loaded session, running CutCraft's
//! controller. It reads one JSON request per line on standard input and answers one JSON line on
//! standard output. Its memory is capped, so a hostile or huge design takes down only this
//! process.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use cutcraft_control::{Controller, Origin, Roots};
use cutcraft_web_protocol::{
    ArtView, CommandReply, CommandRequest, DesignView, Geometry, GeometryRef, KeyLookup,
    MatGeometry, PROTOCOL_VERSION, PictureRequest, SessionSnapshot, SheetView, Stroke, WorkerReply,
    WorkerRequest, codes,
};
use serde_json::{Value, json};

/// Spec §18, provisional.
const MEMORY_LIMIT: u64 = 1536 * 1024 * 1024;

/// The registry commands a browser may send in milestone 0. Everything else is refused:
/// commands that take paths (`design.open`, `project.save`, `export`…) are reached only through
/// the server, which resolves its own identifiers (spec §6).
///
/// `protocol-check.sh` scans the replies of the reads, `sheet.set`, `sheet.place` and
/// `history.undo` for server paths. `app.status` and
/// `prepare.report` are left out: their replies carry server file paths, and the snapshot gives
/// what a browser needs of them (Codex's review of milestone 0).
const ALLOWED: &[&str] = &[
    "prepare.decide",
    "rows.list",
    "cut.include",
    "sheet.set",
    "sheet.place",
    "sheet.test_spot",
    "sheet.piece",
    "plan.sheet",
    "materials.list",
    "mats.list",
    "history.undo",
    "history.redo",
];

struct Worker {
    controller: Controller,
    area: PathBuf,
    incarnation: String,
    /// Accepted keys: the request as received and its answer (spec §7). In memory for the
    /// slice; durable with the draft checkpoint in milestone 2.
    keys: HashMap<String, (CommandRequest, CommandReply)>,
    name: Option<String>,
    /// The registry's read commands: only these may come without a revision.
    reads: Vec<&'static str>,
}

pub fn run(area: &Path) -> Result<(), String> {
    // CUTCRAFT_WEB_WORKER_MB lowers the cap, for milestone 0's isolation test.
    let limit = std::env::var("CUTCRAFT_WEB_WORKER_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(MEMORY_LIMIT, |mb| mb * 1024 * 1024);
    rlimit::setrlimit(rlimit::Resource::AS, limit, limit)
        .map_err(|e| format!("setting the memory limit: {e}"))?;
    let area = std::fs::canonicalize(area).map_err(|e| format!("{}: {e}", area.display()))?;
    let roots = Roots::new(Some(&area), None)?;
    let controller = Controller::headless(roots);
    let reads = controller
        .commands()
        .into_iter()
        .filter(|c| c.authority == cutcraft_control::Authority::Read)
        .map(|c| c.id)
        .collect();
    let mut worker = Worker {
        reads,
        controller,
        area,
        incarnation: crate::random_hex(8),
        keys: HashMap::new(),
        name: None,
    };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        let reply = match serde_json::from_str::<WorkerRequest>(&line) {
            Ok(request) => worker.answer(request),
            Err(e) => WorkerReply::Failed {
                message: format!("unreadable request: {e}"),
            },
        };
        let mut text = serde_json::to_string(&reply).map_err(|e| e.to_string())?;
        text.push('\n');
        stdout
            .write_all(text.as_bytes())
            .and_then(|()| stdout.flush())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn refused(code: &str, message: impl Into<String>) -> CommandReply {
    CommandReply::Refused {
        code: code.to_owned(),
        message: message.into(),
    }
}

impl Worker {
    fn answer(&mut self, request: WorkerRequest) -> WorkerReply {
        match request {
            WorkerRequest::Command(c) => WorkerReply::Command(self.command(c)),
            WorkerRequest::Lookup { key } => WorkerReply::Lookup(match self.keys.get(&key) {
                Some((_, CommandReply::Applied { revision, value })) => KeyLookup::Applied {
                    revision: *revision,
                    value: value.clone(),
                },
                Some((_, CommandReply::Refused { code, message })) => KeyLookup::Refused {
                    code: code.clone(),
                    message: message.clone(),
                },
                None => KeyLookup::NotApplied,
            }),
            WorkerRequest::Snapshot => match self.snapshot() {
                Ok(s) => WorkerReply::Snapshot(s),
                Err(message) => WorkerReply::Failed { message },
            },
            WorkerRequest::Geometry => match self.geometry() {
                Ok(g) => WorkerReply::Geometry(g),
                Err(message) => WorkerReply::Failed { message },
            },
            WorkerRequest::Open { file, name } => match self.open(&file, name) {
                Ok(s) => WorkerReply::Snapshot(s),
                Err(message) => WorkerReply::Failed { message },
            },
            WorkerRequest::Picture(p) => match self.picture(&p) {
                Ok(png) => {
                    use base64::Engine;
                    WorkerReply::Picture {
                        png_base64: base64::engine::general_purpose::STANDARD.encode(png),
                    }
                }
                Err(message) => WorkerReply::Failed { message },
            },
        }
    }

    /// One change, at most once per key (spec §7): a known key answers as before, before any
    /// stale check; the same key with another body is refused.
    fn command(&mut self, c: CommandRequest) -> CommandReply {
        if let Some((first, reply)) = self.keys.get(&c.key) {
            return if *first == c {
                reply.clone()
            } else {
                refused(
                    codes::KEY_REUSED,
                    "That change was already made differently.",
                )
            };
        }
        if c.incarnation != self.incarnation {
            return refused(
                codes::STALE_INCARNATION,
                "This project was reopened since. Here it is now.",
            );
        }
        if !ALLOWED.contains(&c.id.as_str()) {
            return refused(codes::FORBIDDEN, format!("{} isn't available here.", c.id));
        }
        // Every change names the revision it was made against (spec §7); only reads may not.
        if c.revision.is_none() && !self.reads.contains(&c.id.as_str()) {
            return refused(
                codes::REVISION_REQUIRED,
                "This change didn't say which version of the project it was made on.",
            );
        }
        let params = if c.params.is_null() {
            json!({})
        } else {
            c.params.clone()
        };
        let reply = match self
            .controller
            .execute(Origin::User, &c.id, params, c.revision)
        {
            Ok(output) => CommandReply::Applied {
                revision: output.revision,
                value: output.value,
            },
            Err(e) if e.code == "revision-mismatch" => refused(
                codes::STALE_REVISION,
                "This project changed since. Here it is now.",
            ),
            Err(e) => refused(e.code, e.message),
        };
        self.keys.insert(c.key.clone(), (c, reply.clone()));
        reply
    }

    /// An upload the server stored in this person's area, named by the server.
    fn open(&mut self, file: &str, name: String) -> Result<SessionSnapshot, String> {
        if file.contains('/') || file.starts_with('.') {
            return Err("unsafe file name".into());
        }
        let path = self.area.join("designs").join(file);
        self.controller
            .execute(
                Origin::User,
                "design.open",
                json!({"path": path.to_string_lossy()}),
                None,
            )
            .map_err(|e| e.message)?;
        self.name = Some(name);
        self.snapshot()
    }

    fn snapshot(&self) -> Result<SessionSnapshot, String> {
        let s = self.controller.snapshot()?;
        let materials = s.materials.iter().map(|m| m.name.clone()).collect();
        let design = match &s.design {
            None => None,
            Some(open) => {
                let project = &open.project;
                let report = self
                    .controller
                    .execute(Origin::User, "prepare.report", json!({}), None)
                    .map(|o| o.value)
                    .unwrap_or(Value::Null);
                let sheet = project.sheets.first();
                let plan = sheet.and_then(|sh| cutcraft_path::plan_sheet(project, &sh.id).ok());
                Some(DesignView {
                    name: self.name.clone().unwrap_or_else(|| "Design".into()),
                    project_revision: project.revision,
                    cut_ready: report["cut_ready"].as_bool().unwrap_or(false),
                    issues: report["pending"].as_array().map_or(0, |p| p.len() as u32),
                    geometry: Some(GeometryRef {
                        id: geometry_id(project.revision, plan.as_ref()),
                    }),
                    art: art_view(open, &self.incarnation),
                    sheet: sheet.map(|sh| SheetView {
                        material: sh.material.name.clone(),
                        mat: sh.mat_profile().name.clone(),
                        plan_valid: plan.as_ref().is_some_and(|p| p.valid),
                        estimate_s: plan.as_ref().map(|p| p.estimate_s),
                    }),
                })
            }
        };
        Ok(SessionSnapshot {
            protocol: PROTOCOL_VERSION,
            incarnation: self.incarnation.clone(),
            revision: s.revision,
            busy: s
                .loading_path
                .as_ref()
                .map(|_| "Opening the design…".into()),
            error: s.last_error.clone(),
            design,
            materials,
        })
    }

    fn geometry(&self) -> Result<Geometry, String> {
        let s = self.controller.snapshot()?;
        let open = s.design.as_ref().ok_or("No design is open.")?;
        let project = &open.project;
        let lines = cutcraft_path::assembly(project)
            .into_iter()
            .filter(|(_, included)| *included)
            .map(|(op, _)| stroke(&op, [0.0, 0.0]))
            .collect();
        let Some(sheet) = project.sheets.first() else {
            return Ok(Geometry {
                id: geometry_id(project.revision, None),
                lines,
                ..Geometry::default()
            });
        };
        let plan = cutcraft_path::plan_sheet(project, &sheet.id)?;
        let roll = sheet.mat == cutcraft_control::job::Mat::Roll;
        let profile = sheet.mat_profile();
        let origin = if roll {
            [0.0, 0.0]
        } else {
            profile.cut_origin_mm
        };
        let outer = if roll { plan.mat_mm } else { profile.outer_mm };
        let shift = |b: [f64; 4]| {
            [
                b[0] + origin[0],
                b[1] + origin[1],
                b[2] + origin[0],
                b[3] + origin[1],
            ]
        };
        let art = plan
            .operations
            .iter()
            .filter(|o| !o.kind.is_aid())
            .flat_map(|o| o.points.iter())
            .fold(None, |b: Option<[f64; 4]>, p| {
                Some(b.map_or([p[0], p[1], p[0], p[1]], |b| {
                    [
                        b[0].min(p[0]),
                        b[1].min(p[1]),
                        b[2].max(p[0]),
                        b[3].max(p[1]),
                    ]
                }))
            });
        let spot = cutcraft_control::test_cut_markers(&plan, &Default::default())
            .into_iter()
            .find(|(_, done)| !done)
            .map(|(r, _)| shift(r));
        Ok(Geometry {
            id: geometry_id(project.revision, Some(&plan)),
            mat: Some(MatGeometry {
                outer_mm: outer,
                sticky_mm: if roll {
                    [0.0, 0.0, outer[0], outer[1]]
                } else {
                    profile.sticky_mm
                },
                origin_mm: origin,
            }),
            sheet: plan.operations.iter().map(|o| stroke(o, origin)).collect(),
            art_box_mm: art.map(shift),
            test_spot_mm: spot,
            lines,
        })
    }

    fn picture(&self, p: &PictureRequest) -> Result<Vec<u8>, String> {
        if p.width == 0
            || p.height == 0
            || p.width > PictureRequest::MAX_SIDE
            || p.height > PictureRequest::MAX_SIDE
        {
            return Err("picture size out of range".into());
        }
        let s = self.controller.snapshot()?;
        let open = s.design.as_ref().ok_or("No design is open.")?;
        if art_view(open, &self.incarnation).map(|a| a.id) != Some(p.art.clone()) {
            return Err("that picture isn't of the design open now".into());
        }
        let view = cutcraft_control::source_view(open)?;
        let rgba = view.render(p.region, p.width, p.height)?;
        let image = image::RgbaImage::from_raw(p.width, p.height, rgba)
            .ok_or("the picture has the wrong size")?;
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        Ok(png)
    }
}

fn stroke(op: &cutcraft_path::Operation, origin: [f64; 2]) -> Stroke {
    Stroke {
        colour: op.colour.clone().unwrap_or_else(|| "#000000".into()),
        aid: op.kind.is_aid(),
        points: op
            .points
            .iter()
            .map(|p| [(p[0] + origin[0]) as f32, (p[1] + origin[1]) as f32])
            .collect(),
    }
}

/// Changes whenever the vectors or the plan change.
fn geometry_id(revision: u64, plan: Option<&cutcraft_path::CutPlan>) -> String {
    match plan {
        Some(p) => format!("{revision}-{}", p.plan_id),
        None => format!("{revision}"),
    }
}

/// Where to show the artwork, as the desktop fits it: the page, grown to any art outside it,
/// in millimetres as resized.
fn art_view(open: &cutcraft_control::OpenDesign, incarnation: &str) -> Option<ArtView> {
    let project = &open.project;
    let lines = cutcraft_path::assembly(project)
        .into_iter()
        .flat_map(|(op, _)| op.points)
        .fold(None, |b: Option<[f64; 4]>, p| {
            Some(b.map_or([p[0], p[1], p[0], p[1]], |b| {
                [
                    b[0].min(p[0]),
                    b[1].min(p[1]),
                    b[2].max(p[0]),
                    b[3].max(p[1]),
                ]
            }))
        });
    let scale = project.scale;
    let bounds = match &project.prepared.page {
        Some(page) => {
            let page = [0.0, 0.0, page.width_mm * scale, page.height_mm * scale];
            lines.map_or(page, |b| {
                [
                    page[0].min(b[0]),
                    page[1].min(b[1]),
                    page[2].max(b[2]),
                    page[3].max(b[3]),
                ]
            })
        }
        None => lines?,
    };
    let source = &project.prepared.source_sha256;
    Some(ArtView {
        id: format!(
            "{incarnation}-{}-{}-{scale}",
            &source[..source.len().min(16)],
            project.revision
        ),
        design_revision: project.revision,
        bounds,
        scale,
    })
}
