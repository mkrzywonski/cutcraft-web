//! A preparation worker (spec §5): CutCraft's controller for one person's design session. In
//! production it's a child process with its memory capped, so a hostile or huge design takes down
//! only that process ([`run_process`]); the server's tests run it on a thread.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cutcraft_control::job::ProjectFile;

use cutcraft_control::{Controller, Origin, Roots};
use cutcraft_web_protocol::worker::{OpenRequest, OpenSource, WorkerReply, WorkerRequest};
use cutcraft_web_protocol::{
    ArtView, CommandReply, CommandRequest, DesignView, Geometry, GeometryRef, KeyLookup,
    MatGeometry, PROTOCOL_VERSION, PictureRequest, SavedRef, SessionSnapshot, SheetView, Stroke,
    codes,
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
    project: Option<ProjectFile>,
    /// Oldest first.
    keys: Vec<(CommandRequest, CommandReply)>,
    /// Changes since it was opened or saved.
    #[serde(default)]
    unsaved: bool,
    /// The library revision it came from or was saved as.
    #[serde(default)]
    saved: Option<SavedRef>,
}

/// How many replaced projects are kept as drafts (spec §7: opening another project keeps the
/// previous one).
const KEPT_PREVIOUS: usize = 10;

/// Registry commands refused by name, though their class would allow them (spec §6): their
/// replies can carry server paths, they belong to the desktop app or the whole server, or they
/// drive a cutter, which goes through the cutter service from milestone 3.
const REFUSED: &[&str] = &[
    "cut.copy",
    "design.cancel",
    // Replies with server paths: the snapshot gives a browser what it needs of them.
    "app.status",
    "app.engine_check",
    "prepare.report",
    "project.inspect",
    "design.inspection",
    "journal.list",
    // The desktop app's own AI connection and paired stations.
    "settings.mcp",
    "settings.mcp_key_create",
    "settings.mcp_key_delete",
    "settings.mcp_port",
    "station.forget",
    // The cutter and the cut screen: milestone 3, through the cutter service.
    "device.discover",
    "device.status",
    "device.wait",
    "cut.exit",
    "cut.dismiss",
    "cut.reset",
];

/// The authority policy for one command (spec §6): who may run it from a session. Reads and
/// edits of the person's own project are allowed, except those refused by name; commands that
/// name files only through the server's own routes, which name files by owned ids; cutting not
/// from a session.
fn policy(id: &str, authority: cutcraft_control::Authority) -> Result<(), (&'static str, String)> {
    use cutcraft_control::Authority;
    if REFUSED.contains(&id) {
        return Err((codes::FORBIDDEN, format!("{id} isn't available here.")));
    }
    match authority {
        Authority::Read | Authority::Edit => Ok(()),
        Authority::FileRead | Authority::FileWrite => Err((
            codes::FORBIDDEN,
            format!("{id} names files: use the server's own routes for designs and projects."),
        )),
        Authority::DeviceQuery | Authority::DeviceStop | Authority::Motion => Err((
            codes::FORBIDDEN,
            format!("{id} drives a cutter: cutting goes through the cutter service."),
        )),
    }
}

pub struct Worker {
    controller: Controller,
    control_state: std::sync::Arc<std::sync::Mutex<CallState>>,
    area: PathBuf,
    incarnation: String,
    /// Accepted keys: the request as received and its answer (spec §7). They become durable
    /// with the draft checkpoint that contains their change, not before.
    keys: HashMap<String, (CommandRequest, CommandReply)>,
    /// The keys in the order they were accepted, for keeping the most recent.
    order: VecDeque<String>,
    /// Changes not yet in the draft: when the first and the latest were made.
    dirty: Option<(Instant, Instant)>,
    /// Edits since the project was opened or saved (spec §7, §11).
    unsaved: bool,
    /// The library revision the project was opened from or last saved as.
    saved: Option<SavedRef>,
    name: Option<String>,
    /// The registry's read commands: only these may come without a revision.
    reads: Vec<&'static str>,
    /// Every registry command's class, for the authority policy.
    authority: HashMap<&'static str, cutcraft_control::Authority>,
}

#[derive(Clone)]
pub(super) struct Control {
    controller: Controller,
    state: std::sync::Arc<std::sync::Mutex<CallState>>,
}
#[derive(Default)]
struct CallState {
    id: u64,
    active: bool,
    cancelled: bool,
    import: bool,
    cancellable: bool,
}
impl Control {
    pub fn queued(&self, id: u64, request: &WorkerRequest) {
        let import = matches!(request, WorkerRequest::Open(_))
            || matches!(request,WorkerRequest::Command(c) if matches!(c.id.as_str(),"prepare.decide"|"prepare.reset"));
        *self.state.lock().unwrap_or_else(|p| p.into_inner()) = CallState {
            id,
            active: false,
            cancelled: false,
            import,
            cancellable: import || matches!(request, WorkerRequest::Picture(_)),
        };
    }
    pub fn begin(&self, id: u64) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.id != id || state.cancelled {
            return false;
        }
        state.active = true;
        true
    }
    pub fn finish(&self, id: u64, reply: &mut WorkerReply) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.id == id {
            if state.cancelled
                && matches!(reply, WorkerReply::Command(CommandReply::Refused { .. }))
            {
                *reply = WorkerReply::Command(refused("cancelled", "Preparation was cancelled."));
            }
            if state.cancelled && matches!(reply, WorkerReply::Picture { .. }) {
                *reply = WorkerReply::Failed {
                    message: "The preview was superseded.".into(),
                };
            }
            state.id = 0;
            state.active = false;
        }
    }
    pub fn cancel(&self, id: u64) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.id != id || !state.cancellable {
            return false;
        }
        if state.cancelled {
            return true;
        }
        state.cancelled = true;
        if state.active && state.import {
            let _ = self
                .controller
                .execute(Origin::User, "design.cancel", json!({}), None);
        }
        true
    }
}

/// The worker as a child process (`cameo-pi worker --area DIR`): its memory capped, one JSON
/// request per line in, one answer per line out.
pub fn run_process(area: &Path, lists: &Path) -> Result<(), String> {
    let limit = std::env::var("CUTCRAFT_WEB_WORKER_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(MEMORY_LIMIT, |mb| {
            mb.saturating_mul(1024 * 1024).min(MEMORY_LIMIT)
        });
    rlimit::setrlimit(rlimit::Resource::AS, limit, limit)
        .map_err(|e| format!("setting the memory limit: {e}"))?;
    #[cfg(target_os = "linux")]
    if std::fs::write("/proc/self/oom_score_adj", b"500").is_err() {
        eprintln!("cutcraft-web worker: could not lower OOM priority");
    }
    let mut worker = Worker::new(area, lists)?;
    let control = worker.control();
    let (send, lines) = std::sync::mpsc::sync_channel::<cutcraft_web_protocol::worker::Call>(1);
    let (output, responses) =
        std::sync::mpsc::sync_channel::<cutcraft_web_protocol::worker::Response>(8);
    let reply = output.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };
            let Ok(call) = serde_json::from_str::<cutcraft_web_protocol::worker::Call>(&line)
            else {
                break;
            };
            if let WorkerRequest::Cancel { target } = call.request {
                let requested = control.cancel(target);
                if reply
                    .send(cutcraft_web_protocol::worker::Response {
                        id: call.id,
                        reply: WorkerReply::Cancel { requested },
                    })
                    .is_err()
                {
                    break;
                }
            } else {
                control.queued(call.id, &call.request);
                if send.send(call).is_err() {
                    break;
                }
            }
        }
    });
    std::thread::spawn(move || {
        let mut stdout = std::io::stdout().lock();
        for response in responses {
            let Ok(mut bytes) = serde_json::to_vec(&response) else {
                break;
            };
            bytes.push(b'\n');
            if stdout
                .write_all(&bytes)
                .and_then(|()| stdout.flush())
                .is_err()
            {
                break;
            }
        }
    });
    let control = worker.control();
    loop {
        let call = match lines.recv_timeout(Duration::from_millis(500)) {
            Ok(call) => call,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                worker.tick(Instant::now());
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return worker.checkpoint(),
        };
        let mut answer = if control.begin(call.id) {
            worker.answer(call.request)
        } else {
            WorkerReply::Command(refused(
                "cancelled",
                "Preparation was cancelled before it began.",
            ))
        };
        control.finish(call.id, &mut answer);
        if output
            .send(cutcraft_web_protocol::worker::Response {
                id: call.id,
                reply: answer,
            })
            .is_err()
        {
            return worker.checkpoint();
        }
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
    pub(super) fn control(&self) -> Control {
        Control {
            controller: self.controller.clone(),
            state: self.control_state.clone(),
        }
    }

    /// A worker for the person whose private area this is: it reads only inside it. Their own
    /// settings live in the area; the material and mat lists are the household's, in `lists`
    /// (spec §8).
    pub fn new(area: &Path, lists: &Path) -> Result<Self, String> {
        let area = std::fs::canonicalize(area).map_err(|e| format!("{}: {e}", area.display()))?;
        let roots = Roots::new(Some(&area), None)?;
        let store = cutcraft_control::SettingsStore::new(area.join("settings"))
            .with_shared_lists(lists.to_path_buf());
        let controller = Controller::headless_with(store, roots);
        let commands = controller.commands();
        let reads = commands
            .iter()
            .filter(|c| c.authority == cutcraft_control::Authority::Read)
            .map(|c| c.id)
            .collect();
        let authority = commands.iter().map(|c| (c.id, c.authority)).collect();
        let mut worker = Self {
            reads,
            authority,
            controller,
            control_state: std::sync::Arc::new(std::sync::Mutex::new(CallState::default())),
            area,
            incarnation: crate::keys::random_hex(8),
            keys: HashMap::new(),
            order: VecDeque::new(),
            dirty: None,
            unsaved: false,
            saved: None,
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
                if !matches!(draft.schema, 1 | 2) {
                    return Err("unsupported draft schema".into());
                }
                let Some(project) = &draft.project else {
                    return Ok(draft);
                };
                let file = self.area.join("drafts/restoring.cutcraft");
                project.save(&file)?;
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
                self.unsaved = draft.unsaved;
                self.saved = draft.saved;
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
        while self.order.len() > KEPT_KEYS {
            if let Some(old) = self.order.pop_front() {
                self.keys.remove(&old);
            }
        }
        let draft = Draft {
            schema: 2,
            name: self.name.clone(),
            project: snapshot.design.as_ref().map(|open| {
                ProjectFile::new(
                    open.project.clone(),
                    Path::new(&open.path),
                    &open.bytes,
                    cutcraft_control::APP_BUILD,
                )
            }),
            unsaved: self.unsaved,
            saved: self.saved.clone(),
            keys: self
                .order
                .iter()
                .filter_map(|k| self.keys.get(k).cloned())
                .collect(),
        };
        let bytes = serde_json::to_vec(&draft).map_err(|e| e.to_string())?;
        cameo_pi_core::journal::write_atomic(&path, &bytes).map_err(|e| e.to_string())?;
        self.dirty = None;
        Ok(())
    }

    pub fn answer(&mut self, request: WorkerRequest) -> WorkerReply {
        match request {
            WorkerRequest::Cancel { .. } => WorkerReply::Cancel { requested: false },
            WorkerRequest::Checkpoint => match self.checkpoint() {
                Ok(()) => WorkerReply::Replay { result: None },
                Err(message) => WorkerReply::Failed { message },
            },
            WorkerRequest::Remember { request, answer } => {
                if let Some(reply) = self.replay(&request) {
                    return WorkerReply::Command(reply);
                }
                if request.incarnation != self.incarnation
                    || request.revision
                        != Some(self.controller.snapshot().map_or(0, |s| s.revision))
                {
                    return WorkerReply::Command(refused(
                        codes::STALE_REVISION,
                        "The project changed before the action finished.",
                    ));
                }
                self.record(request, &answer);
                self.changed();
                if let Err(e) = self.checkpoint() {
                    eprintln!("cameo-pi worker: the action's draft couldn't be written: {e}");
                }
                WorkerReply::Command(answer)
            }
            WorkerRequest::Replay(c) => WorkerReply::Replay {
                result: self.replay(&c),
            },
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
            WorkerRequest::Open(o) => WorkerReply::Command(self.open(o)),
            WorkerRequest::Export => match self.export() {
                Ok(e) => e,
                Err(message) => WorkerReply::Failed { message },
            },
            WorkerRequest::Saved { request, saved } => {
                WorkerReply::Command(self.saved(request, saved))
            }
            WorkerRequest::ReloadLists => match self
                .controller
                .reload_lists()
                .and_then(|()| self.snapshot())
            {
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
        let Some(&authority) = self.authority.get(c.id.as_str()) else {
            return refused("unknown-command", format!("There is no command {}.", c.id));
        };
        if let Err((code, message)) = policy(&c.id, authority) {
            return refused(code, message);
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
            self.unsaved = self.controller.snapshot().is_ok_and(|s| s.design.is_some());
            if !self.unsaved {
                self.saved = None;
                self.name = None;
            }
            self.changed();
        }
        self.order.push_back(c.key.clone());
        self.keys.insert(c.key.clone(), (c, reply.clone()));
        reply
    }

    /// A change recorded under its key, or the earlier answer for that key (spec §7): the same
    /// rules as a registry command, for the session's own changes (open, saved).
    fn replay(&self, request: &CommandRequest) -> Option<CommandReply> {
        let (first, reply) = self.keys.get(&request.key)?;
        Some(if first == request {
            reply.clone()
        } else {
            refused(
                codes::KEY_REUSED,
                "That change was already made differently.",
            )
        })
    }

    fn record(&mut self, request: CommandRequest, reply: &CommandReply) {
        self.order.push_back(request.key.clone());
        self.keys
            .insert(request.key.clone(), (request, reply.clone()));
    }

    /// Opens a design or a library project in place of the session's project (spec §7):
    /// refused if the project open now has unsaved changes, unless the person chose to open
    /// anyway. The replaced project is kept as a draft.
    fn open(&mut self, o: OpenRequest) -> CommandReply {
        let request = o.request.clone().unwrap_or_else(|| CommandRequest {
            id: "session.open".into(),
            params: json!({"source": o.source, "replace_unsaved": o.replace_unsaved}),
            key: o.key.clone(),
            incarnation: o.incarnation.clone(),
            revision: Some(o.revision),
        });
        if let Some(reply) = self.replay(&request) {
            return reply;
        }
        if o.incarnation != self.incarnation {
            return refused(
                codes::STALE_INCARNATION,
                "This project was reopened since. Here it is now.",
            );
        }
        let current = self.controller.snapshot().map_or(0, |s| s.revision);
        if o.revision != current {
            return refused(
                codes::STALE_REVISION,
                "This project changed since. Here it is now.",
            );
        }
        if self.unsaved && !o.replace_unsaved {
            return refused(
                codes::UNSAVED,
                "The project open now has changes that aren't saved. Save it first, or open the other one anyway (the changes are kept as a draft).",
            );
        }
        let reply = match self.replace(&o.source) {
            Ok(revision) => CommandReply::Applied {
                revision,
                value: json!({"opened": true}),
            },
            Err(message) => refused("failed", message),
        };
        self.record(request, &reply);
        if matches!(reply, CommandReply::Applied { .. }) {
            // A newly opened project is written at once.
            self.changed();
            if let Err(e) = self.checkpoint() {
                eprintln!("cameo-pi worker: the draft couldn't be written: {e}");
            }
        }
        reply
    }

    /// Keeps the open project's draft aside, then opens the source. Returns the new revision.
    fn replace(&mut self, source: &OpenSource) -> Result<u64, String> {
        let (file, folder) = match source {
            OpenSource::Upload { file, .. } => (file, "designs"),
            OpenSource::Project { file, .. } => (file, "drafts"),
        };
        if file.contains('/') || file.starts_with('.') || file.is_empty() {
            return Err("unsafe file name".into());
        }
        self.keep_previous()?;
        let path = self.area.join(folder).join(file);
        let command = match source {
            OpenSource::Upload { .. } => "design.open",
            OpenSource::Project { .. } => "project.open",
        };
        self.controller
            .execute(
                Origin::User,
                command,
                json!({"path": path.to_string_lossy()}),
                None,
            )
            .map_err(|e| e.message)?;
        if let OpenSource::Project { .. } = source {
            let _ = std::fs::remove_file(&path);
        }
        let (name, saved) = match source {
            OpenSource::Upload { name, .. } => (name.clone(), None),
            OpenSource::Project { name, saved, .. } => (name.clone(), Some(saved.clone())),
        };
        self.name = Some(name);
        self.saved = saved;
        self.unsaved = false;
        Ok(self.controller.snapshot()?.revision)
    }

    /// The open project's draft, kept as `drafts/previous-<time>.json` before it's replaced.
    fn keep_previous(&mut self) -> Result<(), String> {
        if self.controller.snapshot()?.design.is_none() {
            return Ok(());
        }
        self.changed();
        self.checkpoint()?;
        let drafts = self.area.join("drafts");
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let kept = drafts.join(format!(
            "previous-{stamp:015}-{}.json",
            crate::keys::random_hex(4)
        ));
        let bytes = std::fs::read(self.area.join(DRAFT)).map_err(|e| e.to_string())?;
        cameo_pi_core::journal::write_atomic(&kept, &bytes).map_err(|e| e.to_string())?;
        let mut previous: Vec<_> = std::fs::read_dir(&drafts)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("previous-"))
            })
            .collect();
        previous.sort();
        while previous.len() > KEPT_PREVIOUS {
            let _ = std::fs::remove_file(previous.remove(0));
        }
        Ok(())
    }

    /// The open project as a `.cutcraft` file and its first sheet's plan, for the server to save
    /// in the library (spec §11).
    fn export(&self) -> Result<WorkerReply, String> {
        let output = self
            .controller
            .execute(Origin::User, "cut.copy", json!({}), None)
            .map_err(|e| e.message)?;
        Ok(WorkerReply::Export {
            name: self.name.clone().unwrap_or_else(|| "Design".into()),
            project: serde_json::to_string(&output.value["project"]).map_err(|e| e.to_string())?,
            plan: serde_json::to_string(&output.value["plan"]).map_err(|e| e.to_string())?,
            saved: self.saved.clone(),
        })
    }

    /// The server saved the project: it's now that library revision, with nothing unsaved.
    fn saved(&mut self, request: CommandRequest, saved: SavedRef) -> CommandReply {
        if let Some(reply) = self.replay(&request) {
            return reply;
        }
        if request.incarnation != self.incarnation {
            return refused(
                codes::STALE_INCARNATION,
                "The project was reopened before saving finished.",
            );
        }
        if request.revision != Some(self.controller.snapshot().map_or(0, |s| s.revision)) {
            return refused(
                codes::STALE_REVISION,
                "The project changed before saving finished.",
            );
        }
        self.saved = Some(saved);
        self.unsaved = false;
        self.changed();
        let reply = CommandReply::Applied {
            revision: self.controller.snapshot().map_or(0, |s| s.revision),
            value: json!({"saved": true}),
        };
        self.record(request, &reply);
        if let Err(message) = self.checkpoint() {
            return refused("storage", message);
        }

        reply
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
                    unsaved: self.unsaved,
                    saved: self.saved.clone(),
                })
            }
        };
        Ok(SessionSnapshot {
            busy_key: None,
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

    #[test]
    fn cancellation_is_fenced_to_the_active_call_and_never_rewrites_an_applied_edit() {
        let dir = area();
        let worker = Worker::new(dir.path(), &dir.path().join("lists")).unwrap();
        let control = worker.control();
        let picture = WorkerRequest::Open(OpenRequest {
            request: None,
            source: OpenSource::Upload {
                file: "star.svg".into(),
                name: "star.svg".into(),
            },
            key: "open".into(),
            incarnation: "test".into(),
            revision: 0,
            replace_unsaved: false,
        });
        control.queued(1, &picture);
        assert!(control.cancel(1));
        assert!(!control.begin(1), "queued cancellation must prevent work");
        control.queued(2, &picture);
        assert!(control.begin(2));
        assert!(
            !control.cancel(1),
            "late cancellation cannot reach the next call"
        );
        assert!(control.cancel(2));
        let mut applied = WorkerReply::Command(CommandReply::Applied {
            revision: 42,
            value: json!({}),
        });
        control.finish(2, &mut applied);
        assert!(matches!(
            applied,
            WorkerReply::Command(CommandReply::Applied { revision: 42, .. })
        ));
        assert!(!control.cancel(2));
    }

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

    fn open_as(w: &mut Worker, key: &str, replace_unsaved: bool) -> CommandReply {
        let s = snapshot(w);
        match w.answer(WorkerRequest::Open(OpenRequest {
            request: None,
            source: OpenSource::Upload {
                file: "star.svg".into(),
                name: "star.svg".into(),
            },
            key: key.into(),
            incarnation: s.incarnation,
            revision: s.revision,
            replace_unsaved,
        })) {
            WorkerReply::Command(r) => r,
            other => panic!("{other:?}"),
        }
    }

    fn open(w: &mut Worker) -> SessionSnapshot {
        let key = format!("open-{}", crate::keys::random_hex(4));
        assert!(matches!(
            open_as(w, &key, false),
            CommandReply::Applied { .. }
        ));
        snapshot(w)
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
        let mut w = Worker::new(dir.path(), &dir.path().join("lists")).unwrap();
        open(&mut w);
        assert!(matches!(
            material(&mut w, "k1", "Removable"),
            CommandReply::Applied { .. }
        ));
        let before = w.incarnation.clone();
        w.tick(Instant::now() + QUIET + Duration::from_millis(10));
        assert!(w.dirty.is_none(), "the draft was written");
        // The original schema used a non-optional project; keep that format readable.
        let path = dir.path().join(DRAFT);
        let mut legacy: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        legacy["schema"] = json!(1);
        std::fs::write(path, serde_json::to_vec(&legacy).unwrap()).unwrap();

        drop(w);

        let mut w = Worker::new(dir.path(), &dir.path().join("lists")).unwrap();
        assert!(sheet_material(&mut w).starts_with("Removable"));
        assert!(matches!(lookup(&mut w, "k1"), KeyLookup::Applied { .. }));
        assert_ne!(w.incarnation, before, "a new load has a new incarnation");
    }

    /// Spec §7: a crash before the draft loses the change, and the record says so: never
    /// "applied" for a change the recovered project doesn't contain.
    #[test]
    fn a_change_lost_in_a_crash_is_looked_up_as_not_applied() {
        let dir = area();
        let mut w = Worker::new(dir.path(), &dir.path().join("lists")).unwrap();
        open(&mut w);
        let original = sheet_material(&mut w);
        assert!(matches!(
            material(&mut w, "k2", "Removable"),
            CommandReply::Applied { .. }
        ));
        // The worker dies before its draft is due: no tick, no checkpoint.
        drop(w);

        let mut w = Worker::new(dir.path(), &dir.path().join("lists")).unwrap();
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
        let mut w = Worker::new(dir.path(), &dir.path().join("lists")).unwrap();
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
        let mut w = Worker::new(dir.path(), &dir.path().join("lists")).unwrap();
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

    /// Spec §6 and Codex's review of milestone 0: whatever the policy lets a browser run, its
    /// reply (answer or refusal) never shows a server path. New registry commands are covered
    /// automatically, or refused.
    #[test]
    fn no_command_the_policy_allows_shows_a_server_path() {
        let dir = area();
        let mut w = Worker::new(dir.path(), &dir.path().join("lists")).unwrap();
        open(&mut w);
        let server = std::fs::canonicalize(dir.path()).unwrap();
        let server = server.to_string_lossy();
        let mut allowed = 0;
        // Closing the design goes last, so every other command meets an open design.
        let mut commands = w.controller.commands();
        commands.sort_by_key(|c| c.id == "design.close");
        for command in commands {
            if policy(command.id, command.authority).is_err() {
                continue;
            }
            allowed += 1;
            // Real answers, not only refusals, for the edits a page uses most.
            let params = match command.id {
                "sheet.set" => json!({"material": "Removable"}),
                "sheet.place" => json!({"x": 20.0, "y": 20.0}),
                "sheet.piece" => json!({"width_mm": 200.0, "height_mm": 200.0}),
                "sheet.test_spot" => json!({"x_mm": 250.0, "y_mm": 250.0}),
                "materials.save_current" => json!({"name": "Scan test"}),
                _ => json!({}),
            };
            let s = snapshot(&mut w);
            let reply = w.answer(WorkerRequest::Command(CommandRequest {
                id: command.id.into(),
                params,
                key: format!("scan-{}", command.id),
                incarnation: s.incarnation,
                revision: Some(s.revision),
            }));
            let text = serde_json::to_string(&reply).unwrap();
            assert!(
                !text.contains(server.as_ref()),
                "{} shows a server path: {text}",
                command.id
            );
        }
        assert!(
            allowed >= 20,
            "the policy allows the design's reads and edits ({allowed})"
        );
        let applied = w
            .keys
            .values()
            .filter(|(_, r)| matches!(r, CommandReply::Applied { .. }))
            .count();
        assert!(
            applied >= 8,
            "the scan saw real answers, not only refusals ({applied})"
        );
    }

    #[test]
    fn files_and_cutters_are_refused_from_a_session() {
        use cutcraft_control::Authority;
        for (id, authority) in [
            ("design.open", Authority::FileRead),
            ("project.save", Authority::FileWrite),
            ("cut.start", Authority::Motion),
            ("cut.stop", Authority::DeviceStop),
            ("device.connect", Authority::DeviceQuery),
            ("app.status", Authority::Read),
        ] {
            assert!(policy(id, authority).is_err(), "{id}");
        }
        assert!(policy("sheet.place", Authority::Edit).is_ok());
        assert!(policy("plan.sheet", Authority::Read).is_ok());
    }

    /// Spec §7: opening another project over unsaved changes is refused until the person
    /// chooses to; then the replaced project is kept as a draft.
    #[test]
    fn opening_over_unsaved_changes_asks_first_and_keeps_the_old_project() {
        let dir = area();
        let mut w = Worker::new(dir.path(), &dir.path().join("lists")).unwrap();
        open(&mut w);
        assert!(!snapshot(&mut w).design.unwrap().unsaved);
        assert!(matches!(
            material(&mut w, "edit", "Removable"),
            CommandReply::Applied { .. }
        ));
        assert!(snapshot(&mut w).design.unwrap().unsaved);

        match open_as(&mut w, "again", false) {
            CommandReply::Refused { code, .. } => assert_eq!(code, codes::UNSAVED),
            other => panic!("{other:?}"),
        }
        assert!(
            sheet_material(&mut w).starts_with("Removable"),
            "nothing was replaced"
        );

        assert!(matches!(
            open_as(&mut w, "anyway", true),
            CommandReply::Applied { .. }
        ));
        assert!(!snapshot(&mut w).design.unwrap().unsaved);
        let kept: Vec<_> = std::fs::read_dir(dir.path().join("drafts"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("previous-"))
            .collect();
        assert_eq!(kept.len(), 1, "the replaced project is kept");
        let text = std::fs::read_to_string(kept[0].path()).unwrap();
        assert!(text.contains("Removable"), "with its unsaved change");
    }

    #[test]
    fn an_open_repeated_with_its_key_answers_the_same_and_changes_nothing_more() {
        let dir = area();
        let mut w = Worker::new(dir.path(), &dir.path().join("lists")).unwrap();
        let s = snapshot(&mut w);
        let request = OpenRequest {
            request: None,
            source: OpenSource::Upload {
                file: "star.svg".into(),
                name: "star.svg".into(),
            },
            key: "o1".into(),
            incarnation: s.incarnation.clone(),
            revision: s.revision,
            replace_unsaved: false,
        };
        let first = w.answer(WorkerRequest::Open(request.clone()));
        let revision = snapshot(&mut w).revision;
        let again = w.answer(WorkerRequest::Open(request));
        assert_eq!(first, again);
        assert_eq!(snapshot(&mut w).revision, revision);
    }

    #[test]
    fn a_saved_project_has_nothing_unsaved_and_names_its_library_revision() {
        let dir = area();
        let mut w = Worker::new(dir.path(), &dir.path().join("lists")).unwrap();
        open(&mut w);
        material(&mut w, "edit", "Removable");
        let WorkerReply::Export {
            project,
            plan,
            saved,
            ..
        } = w.answer(WorkerRequest::Export)
        else {
            panic!("no export");
        };
        assert!(saved.is_none());
        assert!(project.contains("cutcraft-project") && plan.contains("plan_id"));
        let saved = SavedRef {
            job: "j1".into(),
            revision: 1,
        };
        w.answer(WorkerRequest::Saved {
            request: CommandRequest {
                id: "session.save".into(),
                params: json!({}),
                key: "s1".into(),
                incarnation: w.incarnation.clone(),
                revision: Some(w.controller.snapshot().unwrap().revision),
            },
            saved: saved.clone(),
        });
        let design = snapshot(&mut w).design.unwrap();
        assert!(!design.unsaved);
        assert_eq!(design.saved, Some(saved));
    }

    /// Spec §8: the material and mat lists are the household's. Two people's workers edit them
    /// in turn, and neither edit undoes the other.
    #[test]
    fn two_peoples_edits_to_the_shared_lists_both_stay() {
        let (one, two) = (area(), area());
        let lists = tempfile::tempdir().unwrap();
        let mut sam = Worker::new(one.path(), lists.path()).unwrap();
        let mut alex = Worker::new(two.path(), lists.path()).unwrap();
        open(&mut sam);
        open(&mut alex);
        let save = |w: &mut Worker, key: &str, name: &str| {
            let s = snapshot(w);
            let reply = w.answer(WorkerRequest::Command(CommandRequest {
                id: "materials.save_current".into(),
                params: json!({"name": name}),
                key: key.into(),
                incarnation: s.incarnation,
                revision: Some(s.revision),
            }));
            assert!(
                matches!(reply, WorkerReply::Command(CommandReply::Applied { .. })),
                "{reply:?}"
            );
        };
        save(&mut sam, "s1", "Sam's vinyl");
        // Alex's worker still holds the old list in memory; its edit reads the shared one again.
        save(&mut alex, "a1", "Alex's vinyl");
        let names = |w: &mut Worker| {
            w.answer(WorkerRequest::ReloadLists);
            snapshot(w).materials
        };
        for list in [names(&mut sam), names(&mut alex)] {
            assert!(list.iter().any(|m| m == "Sam's vinyl"), "{list:?}");
            assert!(list.iter().any(|m| m == "Alex's vinyl"), "{list:?}");
        }
        assert!(lists.path().join("materials.json").is_file());
    }

    #[test]
    fn a_damaged_draft_is_set_aside_not_lost() {
        let dir = area();
        std::fs::write(dir.path().join(DRAFT), b"{not a draft").unwrap();
        let mut w = Worker::new(dir.path(), &dir.path().join("lists")).unwrap();
        assert!(snapshot(&mut w).design.is_none());
        let kept = std::fs::read_dir(dir.path().join("drafts"))
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains("damaged"));
        assert!(kept);
    }
    #[test]
    fn closing_checkpoints_the_empty_project_and_its_key_records() {
        let dir = area();
        let lists = dir.path().join("lists");
        let mut w = Worker::new(dir.path(), &lists).unwrap();
        let s = open(&mut w);
        let reply = w.answer(WorkerRequest::Command(CommandRequest {
            id: "design.close".into(),
            params: json!({}),
            key: "close".into(),
            incarnation: s.incarnation,
            revision: Some(s.revision),
        }));
        assert!(
            matches!(reply, WorkerReply::Command(CommandReply::Applied { .. })),
            "{reply:?}"
        );
        w.checkpoint().unwrap();
        drop(w);
        let mut recovered = Worker::new(dir.path(), &lists).unwrap();
        assert!(snapshot(&mut recovered).design.is_none());
        assert!(matches!(
            open_as(&mut recovered, "reopen", false),
            CommandReply::Applied { .. }
        ));

        assert!(matches!(
            lookup(&mut recovered, "close"),
            KeyLookup::Applied { .. }
        ));
    }

    #[test]
    fn finishing_an_older_save_cannot_clear_newer_edits() {
        let dir = area();
        let mut w = Worker::new(dir.path(), &dir.path().join("lists")).unwrap();
        let s = open(&mut w);
        material(&mut w, "edit", "Removable");
        let reply = w.answer(WorkerRequest::Saved {
            request: CommandRequest {
                id: "session.save".into(),
                params: json!({}),
                key: "save".into(),
                incarnation: s.incarnation,
                revision: Some(s.revision),
            },
            saved: SavedRef {
                job: "job".into(),
                revision: 1,
            },
        });
        assert!(
            matches!(reply, WorkerReply::Command(CommandReply::Refused { code, .. }) if code == codes::STALE_REVISION)
        );
        assert!(snapshot(&mut w).design.unwrap().unsaved);
    }
}
