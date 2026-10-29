//! Design sessions (spec §7): one per principal, each with its own preparation worker running
//! CutCraft's controller, behind the server's guard (login, CSRF, Host). The browser speaks
//! `cutcraft-web-protocol` here; every request is checked for the protocol version first.

mod ipc;
pub mod worker;

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, Uri, header};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use cutcraft_web_protocol::worker::{OpenRequest, OpenSource, WorkerReply, WorkerRequest};
use cutcraft_web_protocol::{
    CommandReply, CommandRequest, PROTOCOL_VERSION, PictureRequest, SavedRef, codes,
};
use futures_util::Stream;
use tokio::sync::watch;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::WatchStream;

use crate::http::{App, Caller};
use crate::people::{self, Principal};
use crate::service::ApiError;

/// Spec §18, provisional.
pub const UPLOAD_LIMIT: usize = 64 * 1024 * 1024;
/// Commands that change the household's material or mat list (spec §8).
const LIST_EDITS: &[&str] = &[
    "materials.save",
    "materials.save_current",
    "materials.delete",
    "mats.save",
    "mats.delete",
];
/// Designs CutCraft can open.
const DESIGNS: &[&str] = &[
    "svg",
    "svgz",
    "pdf",
    "ai",
    "eps",
    "dxf",
    "png",
    "jpg",
    "jpeg",
    "gif",
    "bmp",
    "psd",
    "psb",
    "vectorcraft",
    "drawcraft",
    "cutcraft",
];
/// The index of a person's uploads, in their area.
const INDEX: &str = "designs/index.json";

/// One upload: its owned id (spec §6) is all a browser ever names it by.
#[derive(Clone, serde::Serialize)]
pub struct Upload {
    pub id: String,
    /// The file name as uploaded, for showing.
    pub name: String,
    /// Seconds since 1970.
    pub uploaded: u64,
    pub bytes: u64,
}

/// How an upload is kept in the index file, with where it's stored (`<id>.<ext>`), which a
/// browser is never sent.
#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    id: String,
    name: String,
    uploaded: u64,
    bytes: u64,
    ext: String,
}

fn read_index(area: &Path) -> Vec<Stored> {
    std::fs::read(area.join(INDEX))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// How workers run.
#[derive(Clone)]
pub enum Mode {
    /// A child process of this program (`cameo-pi worker`), memory capped: the product.
    Process(PathBuf),
    /// A thread in this process: the server's own tests.
    #[cfg_attr(not(test), allow(dead_code))]
    Thread,
}

type Ask = (
    u64,
    WorkerRequest,
    tokio::sync::oneshot::Sender<WorkerReply>,
);

/// The connection to one session's worker. Requests go one at a time.
enum Link {
    Process(Box<ProcessLink>),
    Thread(ThreadLink),
}

struct ProcessLink {
    _child: Arc<tokio::sync::Mutex<tokio::process::Child>>,
    pipe: ipc::Pipe,
}
struct ThreadLink {
    send: std::sync::mpsc::Sender<Ask>,
    control: Option<worker::Control>,
    next: u64,
}
#[derive(Clone)]
enum Control {
    Process {
        pipe: ipc::Pipe,
        child: Arc<tokio::sync::Mutex<tokio::process::Child>>,
    },
    Thread(worker::Control),
}
impl Control {
    async fn cancel(&self, id: u64) -> bool {
        match self {
            Self::Process { pipe, .. } => matches!(
                pipe.ask(WorkerRequest::Cancel { target: id }).await,
                Ok(WorkerReply::Cancel { requested: true })
            ),
            Self::Thread(control) => control.cancel(id),
        }
    }
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Process { child: a, .. }, Self::Process { child: b, .. }) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
    async fn kill(&self) {
        if let Self::Process { child, .. } = self {
            let _ = child.lock().await.start_kill();
        }
    }
}
#[derive(Clone)]
struct Active {
    id: u64,
    key: Option<String>,
    picture: bool,
    control: Control,
}
fn request_key(request: &WorkerRequest) -> Option<String> {
    match request {
        WorkerRequest::Open(open) => Some(open.key.clone()),
        WorkerRequest::Command(command)
            if matches!(command.id.as_str(), "prepare.decide" | "prepare.reset") =>
        {
            Some(command.key.clone())
        }
        _ => None,
    }
}
fn cancellable(request: &WorkerRequest) -> bool {
    request_key(request).is_some() || matches!(request, WorkerRequest::Picture(_))
}

impl Link {
    fn start(mode: &Mode, area: &Path, lists: &Path) -> Result<Self, String> {
        match mode {
            Mode::Process(program) => {
                let mut child = tokio::process::Command::new(program)
                    .args(["worker", "--area"])
                    .arg(area)
                    .arg("--lists")
                    .arg(lists)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    // Not killed when the session lets go: its input closes, and it writes its
                    // draft before it leaves.
                    .kill_on_drop(false)
                    .spawn()
                    .map_err(|e| format!("starting a worker: {e}"))?;
                let stdin = child.stdin.take().ok_or("no worker stdin")?;
                let stdout = child.stdout.take().ok_or("no worker stdout")?;
                Ok(Self::Process(Box::new(ProcessLink {
                    _child: Arc::new(tokio::sync::Mutex::new(child)),
                    pipe: ipc::Pipe::new(stdin, stdout),
                })))
            }
            Mode::Thread => {
                let (send, receive) = std::sync::mpsc::channel::<Ask>();
                let mut worker = worker::Worker::new(area, lists)?;
                let control = worker.control();
                let calls = control.clone();
                std::thread::spawn(move || {
                    loop {
                        match receive.recv_timeout(Duration::from_millis(500)) {
                            Ok((id, request, reply)) => {
                                let mut answer = if calls.begin(id) {
                                    worker.answer(request)
                                } else {
                                    WorkerReply::Failed {
                                        message: "Preparation was cancelled.".into(),
                                    }
                                };
                                calls.finish(id, &mut answer);
                                let _ = reply.send(answer);
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                                let _ = worker.checkpoint();
                                return;
                            }
                        }
                        worker.tick(Instant::now());
                    }
                });
                Ok(Self::Thread(ThreadLink {
                    send,
                    control: Some(control),
                    next: 1,
                }))
            }
        }
    }

    async fn close(self) {
        if let Self::Process(p) = self {
            let ProcessLink { mut _child, pipe } = *p;
            let _ = tokio::time::timeout(Duration::from_secs(1), pipe.close()).await;
            drop(pipe);
            let mut _child = _child.lock().await;
            if tokio::time::timeout(Duration::from_secs(3), _child.wait())
                .await
                .is_err()
            {
                let _ = _child.kill().await;
            }
        }
    }
    async fn ask(&mut self, request: &WorkerRequest) -> Result<WorkerReply, String> {
        self.ask_inner(request, None).await
    }
    async fn ask_inner(
        &mut self,
        request: &WorkerRequest,
        session: Option<&Session>,
    ) -> Result<WorkerReply, String> {
        let (id, answer, control) = match self {
            Self::Process(p) => {
                let (id, answer) = p.pipe.begin(request.clone()).await?;
                (
                    id,
                    answer,
                    Some(Control::Process {
                        pipe: p.pipe.clone(),
                        child: p._child.clone(),
                    }),
                )
            }
            Self::Thread(thread) => {
                let id = thread.next;
                thread.next += 1;
                if let Some(control) = &thread.control {
                    control.queued(id, request);
                }
                let (send, answer) = tokio::sync::oneshot::channel();
                thread
                    .send
                    .send((id, request.clone(), send))
                    .map_err(|_| "the worker stopped".to_owned())?;
                (id, answer, thread.control.clone().map(Control::Thread))
            }
        };
        if let (Some(session), Some(control)) = (session, control) {
            *session.active.lock().unwrap_or_else(|p| p.into_inner()) = Some(Active {
                id,
                key: request_key(request),
                picture: matches!(request, WorkerRequest::Picture(_)),
                control: control.clone(),
            });
            let cancelled = request_key(request).is_some_and(|key| {
                session
                    .cancelled
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .contains(&key)
            });
            if cancelled {
                control.cancel(id).await;
            }
        }
        let reply = answer.await.map_err(|_| "the worker stopped".to_owned());
        if let Some(session) = session {
            session
                .active
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take();
        }
        reply
    }
}

const MAX_WORKERS: usize = 2;
const MAX_PENDING: usize = 32;
const IDLE: Duration = Duration::from_secs(60 * 60);

struct Pool {
    slots: Arc<tokio::sync::Semaphore>,
    requests: Arc<tokio::sync::Semaphore>,
    uploads: Arc<tokio::sync::Semaphore>,
    responses: Arc<tokio::sync::Semaphore>,
    loading: tokio::sync::Mutex<()>,
    stopping: std::sync::atomic::AtomicBool,
}

struct Session {
    area: PathBuf,
    active: Mutex<Option<Active>>,
    known: Mutex<std::collections::HashSet<String>>,
    cancelled: Mutex<std::collections::HashSet<String>>,
    current_key: Mutex<Option<String>>,
    pictures: std::sync::atomic::AtomicU64,
    initializing: std::sync::atomic::AtomicBool,
    last_used: Mutex<Instant>,
    cached: Mutex<Option<cutcraft_web_protocol::SessionSnapshot>>,
    phase: Mutex<Option<String>>,
    permit: Mutex<Option<tokio::sync::OwnedSemaphorePermit>>,
    /// Held while the upload index is read and rewritten.
    index: Mutex<()>,
    operations: tokio::sync::Mutex<()>,
    retired: std::sync::atomic::AtomicBool,
    link: tokio::sync::Mutex<Option<Link>>,
    /// Snapshots as they change, for this principal's pages.
    live: watch::Sender<String>,
}

struct Activity {
    session: Arc<Session>,
    key: Option<String>,
}
impl Drop for Activity {
    fn drop(&mut self) {
        self.session
            .current_key
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(key) = &self.key {
            self.session
                .known
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(key);
            self.session
                .cancelled
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(key);
        }
        *self
            .session
            .last_used
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Instant::now();
        set_phase(&self.session, None);
    }
}

/// Every principal's design session.
pub struct Sessions {
    mode: Mode,
    people: PathBuf,
    /// The household's material and mat lists (spec §8).
    lists: PathBuf,
    sessions: Arc<Mutex<HashMap<String, Arc<Session>>>>,
    pool: Arc<Pool>,
}

fn storage(e: impl std::fmt::Display) -> ApiError {
    ApiError::new(500, "storage", e.to_string())
}

impl Sessions {
    pub fn new(mode: Mode, people: PathBuf, lists: PathBuf) -> Self {
        Self {
            mode,
            people,
            lists,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            pool: Arc::new(Pool {
                slots: Arc::new(tokio::sync::Semaphore::new(MAX_WORKERS)),
                requests: Arc::new(tokio::sync::Semaphore::new(MAX_PENDING)),
                uploads: Arc::new(tokio::sync::Semaphore::new(2)),
                responses: Arc::new(tokio::sync::Semaphore::new(4)),
                loading: tokio::sync::Mutex::new(()),
                stopping: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }

    fn session(&self, principal: &Principal) -> Result<Arc<Session>, ApiError> {
        if self.pool.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(ApiError::new(
                503,
                "server-stopping",
                "The server is restarting. Reconnect shortly.",
            ));
        }
        let mut sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(s) = sessions.get(&principal.id) {
            if s.retired.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(ApiError::new(401, "login", "Log in first."));
            }
            *s.last_used.lock().unwrap_or_else(|p| p.into_inner()) = Instant::now();
            return Ok(s.clone());
        }
        let area = people::area(&self.people, principal).map_err(storage)?;
        let session = Arc::new(Session {
            area,
            active: Mutex::new(None),
            known: Mutex::new(std::collections::HashSet::new()),
            cancelled: Mutex::new(std::collections::HashSet::new()),
            current_key: Mutex::new(None),
            pictures: std::sync::atomic::AtomicU64::new(0),
            initializing: std::sync::atomic::AtomicBool::new(false),
            last_used: Mutex::new(Instant::now()),
            cached: Mutex::new(None),
            phase: Mutex::new(None),
            permit: Mutex::new(None),
            index: Mutex::new(()),
            operations: tokio::sync::Mutex::new(()),
            retired: std::sync::atomic::AtomicBool::new(false),
            link: tokio::sync::Mutex::new(None),
            live: watch::channel(String::new()).0,
        });
        sessions.insert(principal.id.clone(), session.clone());
        Ok(session)
    }

    /// Asks this principal's worker, starting it if needed. A worker that stopped is replaced,
    /// and the request fails once, plainly.
    async fn ask(
        &self,
        principal: &Principal,
        request: WorkerRequest,
    ) -> Result<WorkerReply, ApiError> {
        let session = self.session(principal)?;
        let mode = self.mode.clone();
        let lists = self.lists.clone();
        let pool = self.pool.clone();
        let sessions = self.sessions.clone();
        let capacity = pool.requests.clone().try_acquire_owned().map_err(|_| {
            ApiError::new(
                503,
                "preparation-busy",
                "Preparation is busy. Wait for the current requests, then retry.",
            )
        })?;
        let key = request_key(&request);
        if let Some(key) = &key {
            session
                .known
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(key.clone());
        }
        let picture = if matches!(request, WorkerRequest::Picture(_)) {
            let generation = session
                .pictures
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;
            let old = session
                .active
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
                .filter(|active| active.picture);
            if let Some(old) = old {
                old.control.cancel(old.id).await;
            }
            Some(generation)
        } else {
            None
        };
        // A disconnected HTTP client must not leave an unread worker reply in the pipe.
        tokio::spawn(async move {
            let _capacity = capacity;
            let mut link = session.link.lock().await;
            let _activity=Activity { session:session.clone(), key:key.clone() };
            *session.current_key.lock().unwrap_or_else(|p|p.into_inner())=key.clone();
            if pool.stopping.load(std::sync::atomic::Ordering::SeqCst) { return Err(ApiError::new(503,"server-stopping","The server is restarting.")); }
            if session.retired.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(ApiError::new(401, "login", "Log in first."));
            }
            if link.is_none() {
                set_phase(&session, Some("Waiting for a preparation slot…"));
                let _loading = pool.loading.lock().await;
                loop {
                    if pool.stopping.load(std::sync::atomic::Ordering::SeqCst) {
                        set_phase(&session,None);
                        return Err(ApiError::new(503,"server-stopping","The server is restarting."));
                    }
                    if let Ok(permit) = pool.slots.clone().try_acquire_owned() {
                        match Link::start(&mode, &session.area, &lists) {
                            Ok(mut worker) => {
                                // A reload changes incarnation. Publish it even if the triggering
                                // request is a stale command or a replay, rather than a Snapshot.
                                if !matches!(request, WorkerRequest::Snapshot) {
                                    match worker.ask(&WorkerRequest::Snapshot).await {
                                        Ok(WorkerReply::Snapshot(snapshot)) => cache_snapshot(&session, *snapshot),
                                        _ => { worker.close().await; return Err(ApiError::new(503,"worker-stopped","The design worker could not recover its draft.")); }
                                    }
                                }
                                *link = Some(worker); *session.permit.lock().unwrap_or_else(|p| p.into_inner()) = Some(permit); break;
                            },
                            Err(e) => { set_phase(&session,None); return Err(storage(e)); },
                        }
                    }
                    if unload_oldest(&sessions, Some(&session), None).await { continue; }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
            set_phase(&session,match &request { WorkerRequest::Open(_)=>Some("Opening and making cut-ready…"),WorkerRequest::Picture(_)=>Some("Rendering preview…"),WorkerRequest::Command(_)|WorkerRequest::Export=>Some("Preparing…"),_=>None });
            *session.current_key.lock().unwrap_or_else(|p|p.into_inner())=key.clone();
            let worker=link.as_mut().expect("just started");
            let obsolete=picture.is_some_and(|generation|generation!=session.pictures.load(std::sync::atomic::Ordering::SeqCst))
                || key.as_ref().is_some_and(|key|session.cancelled.lock().unwrap_or_else(|p|p.into_inner()).contains(key));
            if obsolete { return Err(ApiError::new(409,"cancelled","Preparation was superseded or cancelled.")); }
            if cancellable(&request) {
                match worker.ask(&WorkerRequest::Checkpoint).await {
                    Ok(WorkerReply::Replay { result:None })=>{},
                    _=>return Err(ApiError::new(503,"checkpoint-failed","The project could not be checkpointed before preparation.")),
                }
            }
            let result = match worker.ask_inner(&request,Some(&session)).await {
                Ok(reply) => {
                    if let WorkerReply::Snapshot(snapshot) = &reply { cache_snapshot(&session,*snapshot.clone()); }
                    Ok(reply)
                },
                Err(e) => {
                    if let Some(worker) = link.take() { worker.close().await; }
                    session.permit.lock().unwrap_or_else(|p| p.into_inner()).take();
                    Err(ApiError::new(503,"worker-stopped",format!("The design worker stopped ({e}). CutCraft will recover the last draft on the next request.")))
                }
            };
            if picture.is_some_and(|generation|generation!=session.pictures.load(std::sync::atomic::Ordering::SeqCst)) { return Err(ApiError::new(409,"superseded","A newer preview was requested.")); }
            result
        })
        .await
        .map_err(|e| ApiError::new(503, "worker-stopped", e.to_string()))?
    }

    /// Checkpoint and detach open-mode work before its directory moves to the first account.
    pub(crate) async fn retire_household(&self) -> Result<(), ApiError> {
        let principal = Principal {
            id: people::HOUSEHOLD.into(),
            name: "the household".into(),
        };
        let session = self.session(&principal)?;
        let _operation = session.operations.lock().await;
        let mut link = session.link.lock().await;
        if let Some(worker) = link.as_mut()
            && let WorkerReply::Failed { message } = worker
                .ask(&WorkerRequest::Checkpoint)
                .await
                .map_err(storage)?
        {
            return Err(storage(message));
        }
        session
            .retired
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(worker) = link.take() {
            worker.close().await;
        }
        session
            .permit
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        Ok(())
    }

    pub(crate) fn resume_household(&self) {
        if let Some(session) = self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(people::HOUSEHOLD)
        {
            session
                .retired
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// Uses an injected instant for deterministic eviction tests; no wall-clock waiting.
    pub(crate) async fn unload_idle(&self, now: Instant) {
        while unload_oldest(&self.sessions, None, Some(now)).await {}
    }

    pub(crate) fn is_stopping(&self) -> bool {
        self.pool.stopping.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) async fn shutdown(&self) -> Result<(), ApiError> {
        self.pool
            .stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let sessions: Vec<_> = self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect();
        for session in &sessions {
            let active = session
                .active
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            if let Some(active) = active {
                request_cancel(session.clone(), active).await;
            }
        }
        for session in sessions {
            let _operation = session.operations.lock().await;
            let mut link = session.link.lock().await;
            if let Some(worker) = link.as_mut()
                && let WorkerReply::Failed { message } = worker
                    .ask(&WorkerRequest::Checkpoint)
                    .await
                    .map_err(storage)?
            {
                return Err(storage(message));
            }
            if let Some(worker) = link.take() {
                worker.close().await;
            }
            session
                .permit
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take();
        }
        Ok(())
    }

    /// Another person changed the household's material or mat list (spec §8): every other loaded
    /// session reads them again, and its pages get the new snapshot. Sessions with no worker
    /// read them when they next start.
    async fn reload_lists_except(&self, changed_by: &Principal) {
        let others: Vec<(String, Arc<Session>)> = self
            .sessions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|(id, _)| **id != changed_by.id)
            .map(|(id, s)| (id.clone(), s.clone()))
            .collect();
        for (id, session) in others {
            let _operation = session.operations.lock().await;
            if session.link.lock().await.is_none() {
                continue;
            }
            let principal = Principal {
                id,
                name: String::new(),
            };
            if let Ok(WorkerReply::Snapshot(s)) =
                self.ask(&principal, WorkerRequest::ReloadLists).await
                && let Ok(text) = serde_json::to_string(&s)
            {
                session.live.send_replace(text);
            }
        }
    }

    /// Asks for the current snapshot; ask publishes it while still holding the worker link.
    async fn publish(&self, principal: &Principal) {
        let _ = self.ask(principal, WorkerRequest::Snapshot).await;
    }
}

fn cache_snapshot(session: &Session, mut snapshot: cutcraft_web_protocol::SessionSnapshot) {
    let mut cached = session.cached.lock().unwrap_or_else(|p| p.into_inner());
    snapshot.busy = session
        .phase
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    snapshot.busy_key = if snapshot.busy.is_some() {
        session
            .current_key
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    } else {
        None
    };
    let text = serde_json::to_string(&snapshot).unwrap_or_default();
    *cached = Some(snapshot);
    session.live.send_replace(text);
}

fn set_phase(session: &Session, phase: Option<&str>) {
    *session.phase.lock().unwrap_or_else(|p| p.into_inner()) = phase.map(str::to_owned);
    let mut cached = session.cached.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(snapshot) = cached.as_mut() {
        snapshot.busy = phase.map(str::to_owned);
        snapshot.busy_key = if phase.is_some() {
            session
                .current_key
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
        } else {
            None
        };
        session
            .live
            .send_replace(serde_json::to_string(snapshot).unwrap_or_default());
    }
}

async fn unload_oldest(
    sessions: &Mutex<HashMap<String, Arc<Session>>>,
    except: Option<&Arc<Session>>,
    idle: Option<Instant>,
) -> bool {
    let mut candidates: Vec<_> = sessions
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .values()
        .filter(|session| except.is_none_or(|except| !Arc::ptr_eq(session, except)))
        .cloned()
        .collect();
    candidates.sort_by_key(|session| *session.last_used.lock().unwrap_or_else(|p| p.into_inner()));
    for session in candidates {
        let Ok(_operation) = session.operations.try_lock() else {
            continue;
        };
        let Ok(mut link) = session.link.try_lock() else {
            continue;
        };
        let Some(worker) = link.as_mut() else {
            continue;
        };
        if idle.is_some_and(|now| {
            now.saturating_duration_since(
                *session.last_used.lock().unwrap_or_else(|p| p.into_inner()),
            ) < IDLE
        }) {
            continue;
        }
        match worker.ask(&WorkerRequest::Checkpoint).await {
            Ok(WorkerReply::Replay { result: None }) => {}
            Err(_) => {
                if let Some(worker) = link.take() {
                    worker.close().await;
                }
                session
                    .permit
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take();
                return true;
            }
            _ => continue,
        }
        if let Some(worker) = link.take() {
            worker.close().await;
        }
        session
            .permit
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        return true;
    }
    false
}

/// Durable copy receipts also fence keys after a worker/session loses its checkpoint.
async fn replay(
    app: &App,
    p: &Principal,
    request: &CommandRequest,
) -> Result<Option<CommandReply>, ApiError> {
    if let WorkerReply::Replay {
        result: Some(reply),
    } = app
        .design
        .ask(p, WorkerRequest::Replay(request.clone()))
        .await?
    {
        return Ok(Some(reply));
    }
    let (service, owner, key) = (app.service.clone(), p.id.clone(), request.key.clone());
    let stored = tokio::task::spawn_blocking(move || service.copies.lookup(&owner, &key))
        .await
        .map_err(storage)??;
    Ok(stored.map(|first| {
        if first.request == *request {
            first.reply()
        } else {
            CommandReply::Refused {
                code: codes::KEY_REUSED.into(),
                message: "That change was already made differently.".into(),
            }
        }
    }))
}

/// The shared action behind the browser command and MCP. No queue or motion is implied.
pub(crate) async fn prepare_copy(
    app: App,
    p: Principal,
    body: cutcraft_web_protocol::CopyRequest,
) -> Result<CommandReply, ApiError> {
    tokio::spawn(async move {
        if body.key.is_empty() || body.key.len() > 256 || body.incarnation.len() > 128 {
            return Err(ApiError::bad_request(
                "Give a valid request key and project incarnation.",
            ));
        }
        let session = app.design.session(&p)?;
        let _operation = session.operations.lock().await;
        let request = CommandRequest {
            id: "cut.copy".into(),
            params: if body.test {
                serde_json::json!({"test":true})
            } else {
                serde_json::json!({})
            },
            key: body.key,
            incarnation: body.incarnation,
            revision: Some(body.revision),
        };
        if let Some(reply) = replay(&app, &p, &request).await? {
            return Ok(reply);
        }
        let WorkerReply::Snapshot(now) = app.design.ask(&p, WorkerRequest::Snapshot).await? else {
            return Err(unexpected());
        };
        let refuse = |code: &str, message: &str| CommandReply::Refused {
            code: code.into(),
            message: message.into(),
        };
        if now.incarnation != request.incarnation {
            return Ok(refuse(
                codes::STALE_INCARNATION,
                "This project was reopened. Refresh it before cutting.",
            ));
        }
        if now.revision != body.revision {
            return Ok(refuse(
                codes::STALE_REVISION,
                "This project changed. Review it before cutting.",
            ));
        }
        let (name, project, plan) = match app.design.ask(&p, WorkerRequest::Export).await? {
            WorkerReply::Export {
                name,
                project,
                plan,
                ..
            } => (name, project, plan),
            WorkerReply::Failed { message } => {
                return Ok(CommandReply::Refused {
                    code: "not-cut-ready".into(),
                    message,
                });
            }
            _ => return Err(unexpected()),
        };
        let (service, owner, fingerprint) = (app.service.clone(), p.id.clone(), request.clone());
        let reply = tokio::task::spawn_blocking(move || {
            service.copies.prepare(
                &owner,
                fingerprint,
                name,
                project.as_bytes(),
                plan.as_bytes(),
                &service.machine,
            )
        })
        .await
        .map_err(storage)?;
        let reply = match reply {
            Ok(reply) => reply,
            Err(error) if error.error == "not-cut-ready" => CommandReply::Refused {
                code: error.error,
                message: error.message,
            },
            Err(error) => return Err(error),
        };
        if matches!(reply, CommandReply::Applied { .. }) {
            // The copy's own receipt remains authoritative even if the worker dies here.
            let _ = app
                .design
                .ask(
                    &p,
                    WorkerRequest::Remember {
                        request,
                        answer: reply.clone(),
                    },
                )
                .await;
        }
        Ok(reply)
    })
    .await
    .map_err(storage)?
}

pub(crate) async fn session_status(
    app: &App,
    p: &Principal,
) -> Result<cutcraft_web_protocol::SessionSnapshot, ApiError> {
    let session = app.design.session(p)?;
    let busy = session
        .phase
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_some();
    if busy
        && let Some(snapshot) = session
            .cached
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    {
        return Ok(snapshot);
    }
    let pressured = app.design.pool.slots.available_permits() == 0
        && session
            .permit
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_none();
    if pressured {
        let snapshot = session
            .cached
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .unwrap_or_else(|| cutcraft_web_protocol::SessionSnapshot {
                protocol: PROTOCOL_VERSION,
                incarnation: "pending".into(),
                revision: 0,
                busy: Some("Waiting for a preparation slot…".into()),
                ..Default::default()
            });
        set_phase(&session, Some("Waiting for a preparation slot…"));
        cache_snapshot(&session, snapshot.clone());
        if session
            .initializing
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
        {
            let (design, principal, session) = (app.design.clone(), p.clone(), session.clone());
            tokio::spawn(async move {
                let _ = design.ask(&principal, WorkerRequest::Snapshot).await;
                session
                    .initializing
                    .store(false, std::sync::atomic::Ordering::SeqCst);
            });
        }
        return Ok(snapshot);
    }
    match app.design.ask(p, WorkerRequest::Snapshot).await? {
        WorkerReply::Snapshot(snapshot) => Ok(*snapshot),
        _ => Err(unexpected()),
    }
}

pub(crate) async fn cancel_preparation(
    app: &App,
    p: &Principal,
    key: &str,
) -> Result<serde_json::Value, ApiError> {
    let session = app.design.session(p)?;
    let known = session
        .known
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .contains(key);
    if !known {
        return Ok(serde_json::json!({"cancel_requested":false}));
    }
    session
        .cancelled
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key.into());
    let active = session
        .active
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .filter(|active| active.key.as_deref() == Some(key));
    let requested = match active {
        Some(active) => {
            request_cancel(session.clone(), active).await;
            true
        }
        None => true,
    };
    Ok(serde_json::json!({"cancel_requested":requested}))
}

async fn request_cancel(session: Arc<Session>, active: Active) {
    let _ =
        tokio::time::timeout(Duration::from_millis(500), active.control.cancel(active.id)).await;
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let current = session
            .active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if current
            .is_some_and(|current| current.id == active.id && current.control.same(&active.control))
        {
            active.control.kill().await;
        }
    });
}

/// Spec §13: the browser and the server must speak the same protocol; another version is told
/// to reload, before anything reaches a worker.
fn same_protocol(headers: &HeaderMap, uri: &Uri) -> Result<(), ApiError> {
    let wanted = PROTOCOL_VERSION.to_string();
    let from_header = headers
        .get("x-cutcraft-protocol")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let from_query = uri.query().and_then(|q| {
        q.split('&')
            .find_map(|kv| kv.strip_prefix("protocol=").map(str::to_owned))
    });
    if from_header.or(from_query).as_deref() == Some(wanted.as_str()) {
        Ok(())
    } else {
        Err(ApiError::new(
            409,
            codes::PROTOCOL,
            "CutCraft was updated. Reload this page.",
        ))
    }
}

/// The principal, and the protocol checked.
fn who(caller: &Caller, headers: &HeaderMap, uri: &Uri) -> Result<Principal, ApiError> {
    same_protocol(headers, uri)?;
    caller
        .principal()
        .ok_or_else(|| ApiError::new(401, "login", "Log in first."))
}

fn unexpected() -> ApiError {
    ApiError::new(
        500,
        "internal",
        "The design worker gave an unexpected answer.",
    )
}

pub async fn snapshot(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, ApiError> {
    let p = who(&caller, &headers, &uri)?;
    Ok(Json(session_status(&app, &p).await?).into_response())
}

pub async fn command(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    headers: HeaderMap,
    uri: Uri,
    Json(body): Json<CommandRequest>,
) -> Result<Response, ApiError> {
    let p = who(&caller, &headers, &uri)?;
    if body.id == "cut.copy" {
        if body.params != serde_json::json!({})
            && body.params != serde_json::json!({"test":true})
            && body.params != serde_json::json!({"test":false})
            && !body.params.is_null()
        {
            return Err(ApiError::bad_request(
                "Prepare cut copy accepts only the optional test flag.",
            ));
        }
        let Some(revision) = body.revision else {
            return Ok(refusal(
                codes::REVISION_REQUIRED,
                "Name the project revision before cutting.",
            ));
        };
        return Ok(Json(
            prepare_copy(
                app,
                p,
                cutcraft_web_protocol::CopyRequest {
                    test: body.params["test"] == true,
                    key: body.key,
                    incarnation: body.incarnation,
                    revision,
                },
            )
            .await?,
        )
        .into_response());
    }
    if body.id == "design.cancel" {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Target {
            key: Option<String>,
        }
        let params: Target = serde_json::from_value(body.params)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        let current = session_status(&app, &p).await?;
        if body.incarnation != current.incarnation {
            return Ok(refusal(
                codes::STALE_INCARNATION,
                "This preparation belongs to an earlier session.",
            ));
        }
        let key = params
            .key
            .or(current.busy_key)
            .ok_or_else(|| ApiError::bad_request("Name the preparation request to cancel."))?;
        let value = cancel_preparation(&app, &p, &key).await?;
        return Ok(Json(CommandReply::Applied {
            revision: current.revision,
            value,
        })
        .into_response());
    }
    let session = app.design.session(&p)?;
    let _operation = session.operations.lock().await;
    if let Some(reply) = replay(&app, &p, &body).await? {
        return Ok(Json(reply).into_response());
    }
    let lists = LIST_EDITS.contains(&body.id.as_str());
    let reply = app.design.ask(&p, WorkerRequest::Command(body)).await?;
    if matches!(reply, WorkerReply::Command(CommandReply::Applied { .. })) {
        app.design.publish(&p).await;
        // Never hold this session while acquiring another session's operation lock.
        drop(_operation);
        if lists {
            app.design.reload_lists_except(&p).await;
        }
    }
    match reply {
        WorkerReply::Command(r) => Ok(Json(r).into_response()),
        WorkerReply::Failed { message } => Err(ApiError::new(500, "worker", message)),
        _ => Err(unexpected()),
    }
}

pub async fn lookup(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    headers: HeaderMap,
    uri: Uri,
    UrlPath(key): UrlPath<String>,
) -> Result<Response, ApiError> {
    let p = who(&caller, &headers, &uri)?;
    match app
        .design
        .ask(&p, WorkerRequest::Lookup { key: key.clone() })
        .await?
    {
        WorkerReply::Lookup(cutcraft_web_protocol::KeyLookup::NotApplied) => {
            let service = app.service.clone();
            let save_key = format!("design-save:{}:{}", p.id, key);
            let receipt = tokio::task::spawn_blocking(move || service.library.receipt(&save_key))
                .await
                .map_err(storage)?
                .map_err(storage)?;
            let result = match receipt {
                Some((_, _, receipt)) => {
                    let reply: CommandReply =
                        serde_json::from_value(receipt["reply"].clone()).map_err(storage)?;
                    match reply {
                        CommandReply::Applied { revision, value } => {
                            cutcraft_web_protocol::KeyLookup::Applied { revision, value }
                        }
                        CommandReply::Refused { code, message } => {
                            cutcraft_web_protocol::KeyLookup::Refused { code, message }
                        }
                    }
                }
                None => {
                    let (service, owner, key) = (app.service.clone(), p.id.clone(), key);
                    match tokio::task::spawn_blocking(move || service.copies.lookup(&owner, &key))
                        .await
                        .map_err(storage)??
                    {
                        Some(copy) => match copy.reply() {
                            CommandReply::Applied { revision, value } => {
                                cutcraft_web_protocol::KeyLookup::Applied { revision, value }
                            }
                            _ => unreachable!(),
                        },
                        None => cutcraft_web_protocol::KeyLookup::NotApplied,
                    }
                }
            };
            Ok(Json(result).into_response())
        }
        WorkerReply::Lookup(l) => Ok(Json(l).into_response()),
        WorkerReply::Failed { message } => Err(ApiError::new(500, "worker", message)),
        _ => Err(unexpected()),
    }
}

pub async fn geometry(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, ApiError> {
    let p = who(&caller, &headers, &uri)?;
    match app.design.ask(&p, WorkerRequest::Geometry).await? {
        WorkerReply::Geometry(g) => Ok(Json(g).into_response()),
        WorkerReply::Failed { message } => Err(ApiError::new(422, "plan-unavailable", message)),
        _ => Err(unexpected()),
    }
}

#[derive(serde::Deserialize)]
pub struct PictureQuery {
    art: String,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    zoom_step: i32,
    width: u32,
    height: u32,
}

pub async fn picture(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<PictureQuery>,
) -> Result<Response, ApiError> {
    use base64::Engine;
    let p = who(&caller, &headers, &uri)?;
    let request = PictureRequest {
        art: q.art,
        region: [q.x0, q.y0, q.x1, q.y1],
        zoom_step: q.zoom_step,
        width: q.width,
        height: q.height,
    };
    match app.design.ask(&p, WorkerRequest::Picture(request)).await? {
        WorkerReply::Picture { png_base64 } => {
            let png = base64::engine::general_purpose::STANDARD
                .decode(png_base64)
                .map_err(|_| unexpected())?;
            // Named by everything that affects it (the artwork's identity): the browser may
            // keep it, but only for this person.
            Ok((
                [
                    (header::CONTENT_TYPE, "image/png"),
                    (header::CACHE_CONTROL, "private, max-age=86400"),
                ],
                png,
            )
                .into_response())
        }
        WorkerReply::Failed { message } => Err(ApiError::new(404, "not-found", message)),
        _ => Err(unexpected()),
    }
}

#[derive(serde::Deserialize)]
pub struct UploadQuery {
    name: String,
}

/// Keep the permit until the last response byte is consumed or the reader disconnects.
/// A slow preview reader can occupy one bounded slot, never the cutter's control path.
pub(crate) async fn preview_capacity(
    State(app): State<App>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let Ok(permit) = app.design.pool.responses.clone().try_acquire_owned() else {
        return ApiError::new(
            503,
            "preview-busy",
            "Preview downloads are busy. Retry shortly.",
        )
        .into_response();
    };
    let response = next.run(request).await;
    let (parts, body) = response.into_parts();
    let stream = futures_util::stream::unfold(
        (body.into_data_stream(), permit),
        |(mut stream, permit)| async move { stream.next().await.map(|chunk| (chunk, (stream, permit))) },
    );
    Response::from_parts(parts, axum::body::Body::from_stream(stream))
}

/// Admit upload bodies before buffering them. Cutter controls never acquire this permit.
pub(crate) async fn upload_capacity(
    State(app): State<App>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let Ok(_permit) = app.design.pool.uploads.clone().try_acquire_owned() else {
        return ApiError::new(
            503,
            "upload-busy",
            "Two uploads are already in progress. Retry shortly.",
        )
        .into_response();
    };
    match tokio::time::timeout(Duration::from_secs(120), next.run(request)).await {
        Ok(response) => response,
        Err(_) => ApiError::new(408, "upload-timeout", "The upload took too long. Retry it.")
            .into_response(),
    }
}

/// Spec §10: an upload becomes a server-owned copy in the person's own area, named by the
/// server. Opening it is a change of its own ([`open`]).
pub async fn upload(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let p = who(&caller, &headers, &uri)?;
    let ext = q.name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if !DESIGNS.contains(&ext.as_str()) || body.is_empty() {
        return Err(ApiError::bad_request(
            "That isn't a design CutCraft can open.",
        ));
    }
    let session = app.design.session(&p)?;
    let _operation = session.operations.lock().await;
    app.design.session(&p)?;
    let id = crate::keys::random_hex(12);
    let file = format!("{id}.{ext}");
    let path = session.area.join("designs").join(&file);
    tokio::fs::write(&path, &body).await.map_err(storage)?;
    let name: String = q
        .name
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect();
    {
        let _index = session.index.lock().unwrap_or_else(|p| p.into_inner());
        let mut index = read_index(&session.area);
        index.push(Stored {
            id,
            name: name.clone(),
            uploaded: now_s(),
            bytes: body.len() as u64,
            ext,
        });
        let bytes = serde_json::to_vec_pretty(&index).map_err(storage)?;
        cameo_pi_core::journal::write_atomic(&session.area.join(INDEX), &bytes).map_err(storage)?;
    }
    let upload = Upload {
        id: file.split('.').next().unwrap_or_default().to_owned(),
        name,
        uploaded: now_s(),
        bytes: body.len() as u64,
    };
    Ok(Json(serde_json::json!({ "design": upload })).into_response())
}

/// This principal's snapshots as they change (server-sent events), the current one first.
pub async fn live(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let p = who(&caller, &headers, &uri)?;
    let session = app.design.session(&p)?;
    let latest = session_status(&app, &p).await?;
    let cached = session.cached.lock().unwrap_or_else(|p| p.into_inner());
    let first = serde_json::to_string(cached.as_ref().unwrap_or(&latest)).map_err(storage)?;
    let changes = WatchStream::from_changes(session.live.subscribe());
    drop(cached);
    let initial = tokio_stream::once(Ok(Event::default().event("snapshot").data(first)));
    // A slow page retains only the latest full snapshot, regardless of how many edits occur.
    let rest = changes.map(|text| Ok(Event::default().event("snapshot").data(text)));
    let checks =
        tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(Duration::from_secs(1)))
            .map(|_| None);
    let stream = initial
        .map(Some)
        .chain(rest.map(Some).merge(checks))
        .take_while(move |_| {
            !app.design.is_stopping()
                && crate::http::identify(&app, caller.key.as_deref(), caller.session.as_deref())
                    .ok()
                    .and_then(|identity| {
                        let current = Caller {
                            identity,
                            ..caller.clone()
                        };
                        current.principal()
                    })
                    .is_some_and(|current| current.id == p.id)
        })
        .filter_map(|event| event);
    Ok(Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(15))))
}

/// The person's uploads, newest first: their owned ids and names, never where they're stored.
pub async fn designs(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Response, ApiError> {
    let p = who(&caller, &headers, &uri)?;
    let session = app.design.session(&p)?;
    let index: Vec<Upload> = read_index(&session.area)
        .into_iter()
        .rev()
        .map(|s| Upload {
            id: s.id,
            name: s.name,
            uploaded: s.uploaded,
            bytes: s.bytes,
        })
        .collect();
    Ok(Json(serde_json::json!({ "designs": index })).into_response())
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct OpenBody {
    /// One of the person's uploads, by its owned id ([`designs`]).
    #[serde(default)]
    design: Option<String>,
    /// Or a library project ([`ProjectRef`]).
    #[serde(default)]
    project: Option<ProjectRef>,
    key: String,
    incarnation: String,
    revision: u64,
    /// The person was told about unsaved changes and chose to open anyway.
    #[serde(default)]
    replace_unsaved: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ProjectRef {
    job: String,
    /// The library revision; the current one if absent.
    #[serde(default)]
    revision: Option<u32>,
}

/// Opens one of the person's uploads, or a library project, in their session (spec §7, §16):
/// a change with its key, incarnation and revision. Someone else's upload id is simply not
/// found; a library project is copied into the person's own area first.
pub async fn open(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    headers: HeaderMap,
    uri: Uri,
    Json(body): Json<OpenBody>,
) -> Result<Response, ApiError> {
    let p = who(&caller, &headers, &uri)?;
    tokio::spawn(async move { open_inner(app, p, body).await })
        .await
        .map_err(storage)?
}

async fn open_inner(app: App, p: Principal, body: OpenBody) -> Result<Response, ApiError> {
    let session = app.design.session(&p)?;
    let _operation = session.operations.lock().await;
    let fingerprint = CommandRequest {
        id: "session.open".into(),
        params: serde_json::to_value(&body).map_err(storage)?,
        key: body.key.clone(),
        incarnation: body.incarnation.clone(),
        revision: Some(body.revision),
    };
    if let Some(reply) = replay(&app, &p, &fingerprint).await? {
        return Ok(Json(reply).into_response());
    }
    let source = match (body.design, body.project) {
        (Some(id), None) => {
            let Some(upload) = read_index(&session.area).into_iter().find(|u| u.id == id) else {
                return Err(ApiError::not_found());
            };
            OpenSource::Upload {
                file: format!("{}.{}", upload.id, upload.ext),
                name: upload.name,
            }
        }
        (None, Some(project)) => {
            let service = app.service.clone();
            let area = session.area.clone();
            let (file, name, saved) = tokio::task::spawn_blocking(move || {
                let details = service.library.details(&project.job)?;
                let revision = project.revision.unwrap_or(details.current_revision);
                let (_, _, bytes) = service.download(&project.job, revision, "project")?;
                let file = format!("open-{}.cutcraft", crate::keys::random_hex(8));
                std::fs::write(area.join("drafts").join(&file), bytes).map_err(storage)?;
                Ok::<_, ApiError>((
                    file,
                    details.name,
                    SavedRef {
                        job: project.job,
                        revision,
                    },
                ))
            })
            .await
            .map_err(|e| ApiError::new(500, "internal", e.to_string()))??;
            OpenSource::Project { file, name, saved }
        }
        _ => {
            return Err(ApiError::bad_request(
                "Name one design or one project to open.",
            ));
        }
    };
    let request = OpenRequest {
        source,
        request: Some(fingerprint),
        key: body.key,
        incarnation: body.incarnation,
        revision: body.revision,
        replace_unsaved: body.replace_unsaved,
    };
    let reply = app.design.ask(&p, WorkerRequest::Open(request)).await?;
    app.design.publish(&p).await;
    match reply {
        WorkerReply::Command(r) => Ok(Json(r).into_response()),
        WorkerReply::Failed { message } => Err(ApiError::new(500, "worker", message)),
        _ => Err(unexpected()),
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct SaveBody {
    key: String,
    incarnation: String,
    /// The session revision being saved.
    revision: u64,
    /// Save As: a new library project in this folder, under this name.
    #[serde(default, rename = "as")]
    save_as: Option<SaveAs>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct SaveAs {
    #[serde(default)]
    folder: Vec<String>,
    name: String,
}

fn refusal(code: &str, message: impl Into<String>) -> Response {
    (
        axum::http::StatusCode::OK,
        Json(CommandReply::Refused {
            code: code.into(),
            message: message.into(),
        }),
    )
        .into_response()
}

/// Saves the session's project in the library (spec §11): a new revision of the project it
/// was opened from or last saved as, or with `as` a new project. Save names its base: if
/// someone saved that project since, it's refused with `saved-elsewhere` so the person can
/// Reload or Save As. The library validates and publishes it as it does any upload.
pub async fn save(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    headers: HeaderMap,
    uri: Uri,
    Json(body): Json<SaveBody>,
) -> Result<Response, ApiError> {
    let p = who(&caller, &headers, &uri)?;
    tokio::spawn(async move { save_inner(app, p, body).await })
        .await
        .map_err(storage)?
}

async fn save_inner(app: App, p: Principal, body: SaveBody) -> Result<Response, ApiError> {
    let session = app.design.session(&p)?;
    let _operation = session.operations.lock().await;
    let fingerprint = CommandRequest {
        id: "session.save".into(),
        params: serde_json::to_value(&body).map_err(storage)?,
        key: body.key.clone(),
        incarnation: body.incarnation.clone(),
        revision: Some(body.revision),
    };
    if let Some(reply) = replay(&app, &p, &fingerprint).await? {
        return Ok(Json(reply).into_response());
    }
    let receipt_key = format!("design-save:{}:{}", p.id, body.key);
    let stored = {
        let service = app.service.clone();
        let key = receipt_key.clone();
        tokio::task::spawn_blocking(move || service.library.receipt(&key))
            .await
            .map_err(storage)?
            .map_err(storage)?
    };
    if let Some((_, _, receipt)) = stored {
        let recorded: CommandRequest =
            serde_json::from_value(receipt["request"].clone()).map_err(storage)?;
        if recorded != fingerprint {
            return Ok(refusal(
                codes::KEY_REUSED,
                "That save was already made differently.",
            ));
        }
        let reply: CommandReply =
            serde_json::from_value(receipt["reply"].clone()).map_err(storage)?;
        return Ok(Json(reply).into_response());
    }
    let WorkerReply::Snapshot(now) = app.design.ask(&p, WorkerRequest::Snapshot).await? else {
        return Err(unexpected());
    };
    if body.incarnation != now.incarnation {
        return Ok(refusal(
            codes::STALE_INCARNATION,
            "This project was reopened since. Here it is now.",
        ));
    }
    if body.revision != now.revision {
        return Ok(refusal(
            codes::STALE_REVISION,
            "This project changed since. Here it is now.",
        ));
    }
    let (name, project, plan, saved) = match app.design.ask(&p, WorkerRequest::Export).await? {
        WorkerReply::Export {
            name,
            project,
            plan,
            saved,
        } => (name, project, plan, saved),
        WorkerReply::Failed { message } => return Err(ApiError::new(422, "design", message)),
        _ => return Err(unexpected()),
    };
    let (job_id, folder, name) = match (&body.save_as, &saved) {
        (Some(as_), _) => (None, as_.folder.clone(), as_.name.clone()),
        (None, Some(base)) => (Some(base.job.clone()), Vec::new(), name),
        (None, None) => {
            return Ok(refusal(
                codes::NOT_SAVED_YET,
                "This project isn't in the library yet: choose a folder and a name (Save As).",
            ));
        }
    };
    let receipt = serde_json::to_vec(&serde_json::json!({
        "schema_version": 1, "key": receipt_key, "request": fingerprint,
        "reply": CommandReply::Applied { revision: now.revision, value: serde_json::json!({"saved":true}) }
    })).map_err(storage)?;
    let service = app.service.clone();
    let key = format!("design-save:{}:{}", p.id, body.key);
    let by = p.name.clone();
    let base = saved.clone();
    let result =
        tokio::task::spawn_blocking(move || -> Result<Result<SavedRef, Response>, ApiError> {
            let expected = job_id.as_ref().and(base.as_ref()).map(|b| b.revision);
            let details = service.save_preparation(
                crate::service::Upload {
                    folder,
                    name,
                    job_id,
                    planner: cameo_pi_core::validate::PLANNER.into(),
                    project: project.into_bytes(),
                    plan: plan.into_bytes(),
                    key,
                },
                &by,
                expected,
                Some(receipt),
            );
            let details = match details {
                Ok(details) => details,
                Err(e) if e.error == "saved-elsewhere" => {
                    return Ok(Err(refusal(codes::SAVED_ELSEWHERE, e.message)));
                }
                Err(e) => return Err(e),
            };
            Ok(Ok(SavedRef {
                job: details.id,
                revision: details.current_revision,
            }))
        })
        .await
        .map_err(|e| ApiError::new(500, "internal", e.to_string()))??;
    let saved = match result {
        Ok(saved) => saved,
        Err(refused) => return Ok(refused),
    };
    let reply = app
        .design
        .ask(
            &p,
            WorkerRequest::Saved {
                request: fingerprint,
                saved,
            },
        )
        .await?;
    app.design.publish(&p).await;
    match reply {
        WorkerReply::Command(r) => Ok(Json(r).into_response()),
        _ => Err(unexpected()),
    }
}

#[derive(serde::Deserialize)]
pub struct DownloadQuery {
    kind: String,
}
pub async fn download(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<DownloadQuery>,
) -> Result<Response, ApiError> {
    let p = who(&caller, &headers, &uri)?;
    match app
        .design
        .ask(&p, WorkerRequest::Download { kind: q.kind })
        .await?
    {
        WorkerReply::Download { mime, bytes } => Ok((
            [
                (header::CONTENT_TYPE, mime),
                (header::CONTENT_DISPOSITION, "attachment".into()),
                (header::CACHE_CONTROL, "no-store".into()),
            ],
            bytes,
        )
            .into_response()),
        WorkerReply::Failed { message } => Err(ApiError::new(422, "export", message)),
        _ => Err(unexpected()),
    }
}
pub async fn update(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    headers: HeaderMap,
    uri: Uri,
    Json(body): Json<OpenBody>,
) -> Result<Response, ApiError> {
    let p = who(&caller, &headers, &uri)?;
    let session = app.design.session(&p)?;
    let _operation = session.operations.lock().await;
    let request = CommandRequest {
        id: "session.update".into(),
        params: serde_json::to_value(&body).map_err(storage)?,
        key: body.key,
        incarnation: body.incarnation,
        revision: Some(body.revision),
    };
    if let Some(reply) = replay(&app, &p, &request).await? {
        return Ok(Json(reply).into_response());
    }
    let id = body
        .design
        .ok_or_else(|| ApiError::bad_request("Choose an uploaded design."))?;
    let upload = read_index(&session.area)
        .into_iter()
        .find(|u| u.id == id)
        .ok_or_else(ApiError::not_found)?;
    if upload.ext == "cutcraft" {
        return Err(ApiError::bad_request(
            "Update needs a design file, not a saved project.",
        ));
    }
    let reply = app
        .design
        .ask(
            &p,
            WorkerRequest::Update {
                request,
                file: format!("{}.{}", upload.id, upload.ext),
                name: upload.name,
            },
        )
        .await?;
    app.design.publish(&p).await;
    match reply {
        WorkerReply::Command(reply) => Ok(Json(reply).into_response()),
        _ => Err(unexpected()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(id: &str) -> Principal {
        Principal {
            id: id.into(),
            name: id.into(),
        }
    }
    async fn snap(sessions: &Sessions, p: &Principal) -> cutcraft_web_protocol::SessionSnapshot {
        match sessions.ask(p, WorkerRequest::Snapshot).await.unwrap() {
            WorkerReply::Snapshot(s) => *s,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn resident_budget_checkpoints_lru_and_idle_unloads_without_losing_edits() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = Sessions::new(
            Mode::Thread,
            dir.path().join("people"),
            dir.path().join("lists"),
        );
        let a = principal("a");
        let b = principal("b");
        let c = principal("c");
        let initial = snap(&sessions, &a).await;
        let session = sessions.session(&a).unwrap();
        std::fs::write(session.area.join("designs/shape.svg"), r#"<svg xmlns="http://www.w3.org/2000/svg" width="30mm" height="30mm" viewBox="0 0 30 30"><rect x="5" y="5" width="20" height="20"/></svg>"#).unwrap();
        let reply = sessions
            .ask(
                &a,
                WorkerRequest::Open(OpenRequest {
                    request: None,
                    source: OpenSource::Upload {
                        file: "shape.svg".into(),
                        name: "shape.svg".into(),
                    },
                    key: "open".into(),
                    incarnation: initial.incarnation.clone(),
                    revision: initial.revision,
                    replace_unsaved: false,
                }),
            )
            .await
            .unwrap();
        assert!(matches!(
            reply,
            WorkerReply::Command(CommandReply::Applied { .. })
        ));
        let opened = snap(&sessions, &a).await;
        let edit = CommandRequest {
            id: "sheet.adjust".into(),
            params: serde_json::json!({"force":22}),
            key: "force".into(),
            incarnation: opened.incarnation,
            revision: Some(opened.revision),
        };
        assert!(matches!(
            sessions
                .ask(&a, WorkerRequest::Command(edit))
                .await
                .unwrap(),
            WorkerReply::Command(CommandReply::Applied { .. })
        ));
        snap(&sessions, &b).await;
        assert_eq!(sessions.pool.slots.available_permits(), 0);
        snap(&sessions, &c).await;
        assert!(
            session.link.lock().await.is_none(),
            "least recently used session must unload"
        );
        assert_eq!(sessions.pool.slots.available_permits(), 0);
        assert!(matches!(
            sessions
                .ask(
                    &a,
                    WorkerRequest::Lookup {
                        key: "force".into()
                    }
                )
                .await
                .unwrap(),
            WorkerReply::Lookup(cutcraft_web_protocol::KeyLookup::Applied { .. })
        ));
        let advertised = session
            .cached
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .incarnation
            .clone();
        assert_ne!(
            initial.incarnation, advertised,
            "a lookup-triggered reload must publish its new incarnation"
        );
        let recovered = snap(&sessions, &a).await;
        assert_eq!(advertised, recovered.incarnation);
        sessions
            .unload_idle(Instant::now() + IDLE + Duration::from_secs(1))
            .await;
        assert_eq!(sessions.pool.slots.available_permits(), MAX_WORKERS);
        sessions.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn request_backpressure_does_not_wait_for_a_worker() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = Sessions::new(
            Mode::Thread,
            dir.path().join("people"),
            dir.path().join("lists"),
        );
        let _held = sessions
            .pool
            .requests
            .clone()
            .acquire_many_owned(MAX_PENDING as u32)
            .await
            .unwrap();
        let error = sessions
            .ask(&principal("a"), WorkerRequest::Snapshot)
            .await
            .unwrap_err();
        assert_eq!(error.error, "preparation-busy");
        assert_eq!(sessions.pool.slots.available_permits(), MAX_WORKERS);
    }

    /// Cancelling the HTTP-facing future leaves the exchange alive until its reply is consumed.
    #[tokio::test]
    async fn a_cancelled_request_still_consumes_its_worker_reply() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = Arc::new(Sessions::new(
            Mode::Thread,
            dir.path().join("people"),
            dir.path().join("lists"),
        ));
        let principal = Principal {
            id: people::HOUSEHOLD.into(),
            name: "the household".into(),
        };
        let session = sessions.session(&principal).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        *session.link.lock().await = Some(Link::Thread(ThreadLink {
            send: tx,
            control: None,
            next: 1,
        }));
        let original = {
            let sessions = sessions.clone();
            let principal = principal.clone();
            tokio::spawn(async move { sessions.ask(&principal, WorkerRequest::Snapshot).await })
        };
        let (_, _, reply) = tokio::task::spawn_blocking(move || rx.recv())
            .await
            .unwrap()
            .unwrap();
        original.abort();
        assert!(original.await.unwrap_err().is_cancelled());
        assert!(
            reply
                .send(WorkerReply::Failed {
                    message: "first".into()
                })
                .is_ok(),
            "the detached exchange must retain its reply receiver"
        );
    }
}
