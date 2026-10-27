//! A preparation worker (spec §5): CutCraft's controller for one person's design session. In
//! production it's a child process with its memory capped, so a hostile or huge design takes down
//! only that process ([`run_process`]); the server's tests run it on a thread.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cutcraft_control::job::ProjectFile;

use cutcraft_control::{Controller, Origin, Roots};
use cutcraft_web_protocol::{
    ArtView, CommandReply, CommandRequest, DesignView, Geometry, GeometryRef, KeyLookup,
    MatGeometry, PROTOCOL_VERSION, PictureRequest, SessionSnapshot, SheetView, Stroke, WorkerReply,
    WorkerRequest, codes,
};
use serde_json::{Value, json};

/// Spec §18, provisional.
const MEMORY_LIMIT: u64 = 1536 * 1024 * 1024;
/// Spec §7, provisional: a draft is written this long after the last change…
const QUIET: Duration = Duration::from_secs(5);
/// …and at least this often while changes keep coming.
const AT_MOST: Duration = Duration::from_secs(30);
/// How many change records a draft keeps (the most recent).
const KEPT_KEYS: usize = 1000;
/// The person's one working project (spec §7), with the record of its changes.
const DRAFT: &str = "drafts/session.json";

/// A draft: the open project and the change records it contains, written together in one
/// atomic file so they can never disagree after a crash (spec §7).
#[derive(serde::Serialize, serde::Deserialize)]
struct Draft {
    schema: u32,
    name: Option<String>,
    project: ProjectFile,
    /// Oldest first.
    keys: Vec<(CommandRequest, CommandReply)>,
}

/// The registry commands a browser may send in milestone 0. Everything else is refused:
/// commands that take paths (`design.open`, `project.save`, `export`…) are reached only through
/// the server, which resolves its own identifiers (spec §6).
///
/// Every reply here is checked for server paths by `protocol-check.sh`. `app.status` and
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

pub struct Worker {
    controller: Controller,
    area: PathBuf,
    incarnation: String,
    /// Accepted keys: the request as received and its answer (spec §7). They become durable
    /// with the draft checkpoint that contains their change, not before.
    keys: HashMap<String, (CommandRequest, CommandReply)>,
    /// The keys in the order they were accepted, for keeping the most recent.
    order: VecDeque<String>,
    /// Changes not yet in the draft: when the first and the latest were made.
    dirty: Option<(Instant, Instant)>,
    name: Option<String>,
    /// The registry's read commands: only these may come without a revision.
    reads: Vec<&'static str>,
}

/// The worker as a child process (`cameo-pi worker --area DIR`): its memory capped, one JSON
/// request per line in, one answer per line out.
pub fn run_process(area: &Path) -> Result<(), String> {
    // CUTCRAFT_WEB_WORKER_MB lowers the cap, for the isolation checks.
    let limit = std::env::var("CUTCRAFT_WEB_WORKER_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(MEMORY_LIMIT, |mb| mb * 1024 * 1024);
    rlimit::setrlimit(rlimit::Resource::AS, limit, limit)
        .map_err(|e| format!("setting the memory limit: {e}"))?;
    let mut worker = Worker::new(area)?;
    // Lines come from a reader thread, so the draft timer runs while no request arrives.
    let (send, lines) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if send.send(line).is_err() {
                break;
            }
        }
    });
    let mut stdout = std::io::stdout().lock();
    loop {
        let line = match lines.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => line,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                worker.tick(Instant::now());
                continue;
            }
            // The server closed the connection (it stopped, or dropped the session): keep the
            // work, then leave.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return worker.checkpoint();
            }
        };
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
        worker.tick(Instant::now());
    }
}

fn refused(code: &str, message: impl Into<String>) -> CommandReply {
    CommandReply::Refused {
        code: code.to_owned(),
        message: message.into(),
    }
}

impl Worker {
    /// A worker for the person whose private area this is: it reads only inside it.
    pub fn new(area: &Path) -> Result<Self, String> {
        let area = std::fs::canonicalize(area).map_err(|e| format!("{}: {e}", area.display()))?;
        let roots = Roots::new(Some(&area), None)?;
        let controller = Controller::headless(roots);
        let reads = controller
            .commands()
            .into_iter()
            .filter(|c| c.authority == cutcraft_control::Authority::Read)
            .map(|c| c.id)
            .collect();
        let mut worker = Self {
            reads,
            controller,
            area,
            incarnation: crate::keys::random_hex(8),
            keys: HashMap::new(),
            order: VecDeque::new(),
            dirty: None,
            name: None,
        };
        worker.restore();
        Ok(worker)
    }

    /// Picks up where the last load left off: the draft's project and its change records. A
    /// draft that can't be read is set aside, never lost, and the session starts empty.
    fn restore(&mut self) {
        let path = self.area.join(DRAFT);
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        let opened = serde_json::from_slice::<Draft>(&bytes)
            .map_err(|e| e.to_string())
            .and_then(|draft| {
                let file = self.area.join("drafts/restoring.cutcraft");
                draft.project.save(&file)?;
                let opened = self
                    .controller
                    .execute(
                        Origin::User,
                        "project.open",
                        json!({"path": file.to_string_lossy()}),
                        None,
                    )
                    .map_err(|e| e.message);
                let _ = std::fs::remove_file(&file);
                opened.map(|_| draft)
            });
        match opened {
            Ok(draft) => {
                self.name = draft.name;
                for (request, reply) in draft.keys {
                    self.order.push_back(request.key.clone());
                    self.keys.insert(request.key.clone(), (request, reply));
                }
            }
            Err(e) => {
                eprintln!("cameo-pi worker: the draft couldn't be restored ({e}); kept aside");
                let _ = std::fs::rename(
                    &path,
                    path.with_extension(format!("damaged-{}", crate::keys::random_hex(4))),
                );
            }
        }
    }

    /// Writes the draft when it's due (spec §7): `QUIET` after the last change, or `AT_MOST`
    /// after the first one not yet written.
    pub fn tick(&mut self, now: Instant) {
        if let Some((first, latest)) = self.dirty
            && (now.duration_since(latest) >= QUIET || now.duration_since(first) >= AT_MOST)
            && let Err(e) = self.checkpoint()
        {
            eprintln!("cameo-pi worker: the draft couldn't be written: {e}");
        }
    }

    fn changed(&mut self) {
        let now = Instant::now();
        self.dirty = Some(self.dirty.map_or((now, now), |(first, _)| (first, now)));
    }

    /// Writes the draft now, if anything changed: the project and its change records, in one
    /// atomic file.
    pub fn checkpoint(&mut self) -> Result<(), String> {
        if self.dirty.is_none() {
            return Ok(());
        }
        let snapshot = self.controller.snapshot()?;
        let path = self.area.join(DRAFT);
        match &snapshot.design {
            None => {
                let _ = std::fs::remove_file(&path);
            }
            Some(open) => {
                while self.order.len() > KEPT_KEYS {
                    if let Some(old) = self.order.pop_front() {
                        self.keys.remove(&old);
                    }
                }
                let draft = Draft {
                    schema: 1,
                    name: self.name.clone(),
                    project: ProjectFile::new(
                        open.project.clone(),
                        Path::new(&open.path),
                        &open.bytes,
                        cutcraft_control::APP_BUILD,
                    ),
                    keys: self
                        .order
                        .iter()
                        .filter_map(|k| self.keys.get(k).cloned())
                        .collect(),
                };
                let bytes = serde_json::to_vec(&draft).map_err(|e| e.to_string())?;
                cameo_pi_core::journal::write_atomic(&path, &bytes).map_err(|e| e.to_string())?;
            }
        }
        self.dirty = None;
        Ok(())
    }

    pub fn answer(&mut self, request: WorkerRequest) -> WorkerReply {
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
        if matches!(reply, CommandReply::Applied { .. }) && !self.reads.contains(&c.id.as_str()) {
            self.changed();
        }
        self.order.push_back(c.key.clone());
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
        // A newly opened design is written at once.
        self.changed();
        self.checkpoint()?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use cutcraft_web_protocol::KeyLookup;

    const STAR: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="60mm" viewBox="0 0 80 60"><path fill="#d22" d="M20 5 L24.7 17.6 L38 17.6 L27.2 25.4 L31.4 38 L20 30.2 L8.6 38 L12.8 25.4 L2 17.6 L15.3 17.6 Z"/></svg>"##;

    fn area() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for folder in ["designs", "drafts"] {
            std::fs::create_dir_all(dir.path().join(folder)).unwrap();
        }
        std::fs::write(dir.path().join("designs/star.svg"), STAR).unwrap();
        dir
    }

    fn snapshot(w: &mut Worker) -> SessionSnapshot {
        match w.answer(WorkerRequest::Snapshot) {
            WorkerReply::Snapshot(s) => s,
            other => panic!("{other:?}"),
        }
    }

    fn open(w: &mut Worker) -> SessionSnapshot {
        match w.answer(WorkerRequest::Open {
            file: "star.svg".into(),
            name: "star.svg".into(),
        }) {
            WorkerReply::Snapshot(s) => s,
            other => panic!("{other:?}"),
        }
    }

    fn material(w: &mut Worker, key: &str, name: &str) -> CommandReply {
        let s = snapshot(w);
        match w.answer(WorkerRequest::Command(CommandRequest {
            id: "sheet.set".into(),
            params: json!({"material": name}),
            key: key.into(),
            incarnation: s.incarnation,
            revision: Some(s.revision),
        })) {
            WorkerReply::Command(r) => r,
            other => panic!("{other:?}"),
        }
    }

    fn lookup(w: &mut Worker, key: &str) -> KeyLookup {
        match w.answer(WorkerRequest::Lookup { key: key.into() }) {
            WorkerReply::Lookup(l) => l,
            other => panic!("{other:?}"),
        }
    }

    fn sheet_material(w: &mut Worker) -> String {
        snapshot(w).design.unwrap().sheet.unwrap().material
    }

    /// Spec §7: after the quiet period the draft holds the change and its record, and a new load
    /// picks both up.
    #[test]
    fn a_draft_after_the_quiet_period_brings_the_change_and_its_record_back() {
        let dir = area();
        let mut w = Worker::new(dir.path()).unwrap();
        open(&mut w);
        assert!(matches!(
            material(&mut w, "k1", "Removable"),
            CommandReply::Applied { .. }
        ));
        let before = w.incarnation.clone();
        w.tick(Instant::now() + QUIET + Duration::from_millis(10));
        assert!(w.dirty.is_none(), "the draft was written");
        drop(w);

        let mut w = Worker::new(dir.path()).unwrap();
        assert!(sheet_material(&mut w).starts_with("Removable"));
        assert!(matches!(lookup(&mut w, "k1"), KeyLookup::Applied { .. }));
        assert_ne!(w.incarnation, before, "a new load has a new incarnation");
    }

    /// Spec §7: a crash before the draft loses the change, and the record says so: never
    /// "applied" for a change the recovered project doesn't contain.
    #[test]
    fn a_change_lost_in_a_crash_is_looked_up_as_not_applied() {
        let dir = area();
        let mut w = Worker::new(dir.path()).unwrap();
        open(&mut w);
        let original = sheet_material(&mut w);
        assert!(matches!(
            material(&mut w, "k2", "Removable"),
            CommandReply::Applied { .. }
        ));
        // The worker dies before its draft is due: no tick, no checkpoint.
        drop(w);

        let mut w = Worker::new(dir.path()).unwrap();
        assert_eq!(
            sheet_material(&mut w),
            original,
            "the opened design came back, unchanged"
        );
        assert_eq!(lookup(&mut w, "k2"), KeyLookup::NotApplied);
    }

    /// Spec §7: changes that keep coming are written at least every `AT_MOST`.
    #[test]
    fn steady_changes_are_written_at_least_every_thirty_seconds() {
        let dir = area();
        let mut w = Worker::new(dir.path()).unwrap();
        open(&mut w);
        let base = Instant::now();
        // The first unwritten change was at `base`, the latest 2 s ago: not quiet yet.
        w.dirty = Some((base, base + AT_MOST - Duration::from_secs(2)));
        w.tick(base + AT_MOST - Duration::from_secs(1));
        assert!(w.dirty.is_some(), "neither rule is due yet");
        w.tick(base + AT_MOST);
        assert!(
            w.dirty.is_none(),
            "written at thirty seconds, though it was never quiet"
        );
    }

    #[test]
    fn a_refused_change_doesnt_make_a_draft_due() {
        let dir = area();
        let mut w = Worker::new(dir.path()).unwrap();
        open(&mut w);
        assert!(w.dirty.is_none());
        let s = snapshot(&mut w);
        let reply = w.answer(WorkerRequest::Command(CommandRequest {
            id: "sheet.set".into(),
            params: json!({"material": "Removable"}),
            key: "stale".into(),
            incarnation: s.incarnation,
            revision: Some(s.revision + 7),
        }));
        assert!(matches!(
            reply,
            WorkerReply::Command(CommandReply::Refused { .. })
        ));
        assert!(w.dirty.is_none());
    }

    #[test]
    fn a_damaged_draft_is_set_aside_not_lost() {
        let dir = area();
        std::fs::write(dir.path().join(DRAFT), b"{not a draft").unwrap();
        let mut w = Worker::new(dir.path()).unwrap();
        assert!(snapshot(&mut w).design.is_none());
        let kept = std::fs::read_dir(dir.path().join("drafts"))
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains("damaged"));
        assert!(kept);
    }
}
