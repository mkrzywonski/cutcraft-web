//! Design sessions (spec §7): one per principal, each with its own preparation worker running
//! CutCraft's controller, behind the server's guard (login, CSRF, Host). The browser speaks
//! `cutcraft-web-protocol` here; every request is checked for the protocol version first.

pub mod worker;

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::BroadcastStream;

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
    "svg", "svgz", "pdf", "ai", "eps", "dxf", "png", "jpg", "jpeg",
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

type Ask = (WorkerRequest, tokio::sync::oneshot::Sender<WorkerReply>);

/// The connection to one session's worker. Requests go one at a time.
enum Link {
    Process(Box<ProcessLink>),
    Thread(std::sync::mpsc::Sender<Ask>),
}

struct ProcessLink {
    _child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
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
                let stdout = BufReader::new(child.stdout.take().ok_or("no worker stdout")?).lines();
                Ok(Self::Process(Box::new(ProcessLink {
                    _child: child,
                    stdin,
                    stdout,
                })))
            }
            Mode::Thread => {
                let (send, receive) = std::sync::mpsc::channel::<Ask>();
                let (area, lists) = (area.to_path_buf(), lists.to_path_buf());
                std::thread::spawn(move || {
                    let mut worker = worker::Worker::new(&area, &lists);
                    loop {
                        match receive.recv_timeout(std::time::Duration::from_millis(500)) {
                            Ok((request, reply)) => {
                                let answer = match worker.as_mut() {
                                    Ok(w) => w.answer(request),
                                    Err(e) => WorkerReply::Failed { message: e.clone() },
                                };
                                let _ = reply.send(answer);
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                                if let Ok(w) = worker.as_mut() {
                                    let _ = w.checkpoint();
                                }
                                return;
                            }
                        }
                        if let Ok(w) = worker.as_mut() {
                            w.tick(std::time::Instant::now());
                        }
                    }
                });
                Ok(Self::Thread(send))
            }
        }
    }

    async fn ask(&mut self, request: &WorkerRequest) -> Result<WorkerReply, String> {
        match self {
            Self::Process(p) => {
                let mut line = serde_json::to_string(request).map_err(|e| e.to_string())?;
                line.push('\n');
                p.stdin
                    .write_all(line.as_bytes())
                    .await
                    .map_err(|e| e.to_string())?;
                match p.stdout.next_line().await.map_err(|e| e.to_string())? {
                    Some(reply) => serde_json::from_str(&reply).map_err(|e| e.to_string()),
                    None => Err("the worker stopped".into()),
                }
            }
            Self::Thread(send) => {
                let (reply, answer) = tokio::sync::oneshot::channel();
                send.send((request.clone(), reply))
                    .map_err(|_| "the worker stopped".to_string())?;
                answer.await.map_err(|_| "the worker stopped".to_string())
            }
        }
    }
}

struct Session {
    area: PathBuf,
    /// Held while the upload index is read and rewritten.
    index: Mutex<()>,
    operations: tokio::sync::Mutex<()>,
    retired: std::sync::atomic::AtomicBool,
    link: tokio::sync::Mutex<Option<Link>>,
    /// Snapshots as they change, for this principal's pages.
    live: broadcast::Sender<String>,
}

/// Every principal's design session.
pub struct Sessions {
    mode: Mode,
    people: PathBuf,
    /// The household's material and mat lists (spec §8).
    lists: PathBuf,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
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
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn session(&self, principal: &Principal) -> Result<Arc<Session>, ApiError> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(s) = sessions.get(&principal.id) {
            if s.retired.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(ApiError::new(401, "login", "Log in first."));
            }
            return Ok(s.clone());
        }
        let area = people::area(&self.people, principal).map_err(storage)?;
        let session = Arc::new(Session {
            area,
            index: Mutex::new(()),
            operations: tokio::sync::Mutex::new(()),
            retired: std::sync::atomic::AtomicBool::new(false),
            link: tokio::sync::Mutex::new(None),
            live: broadcast::channel(16).0,
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
        // A disconnected HTTP client must not leave an unread worker reply in the pipe.
        tokio::spawn(async move {
            let mut link = session.link.lock().await;
            if session.retired.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(ApiError::new(401, "login", "Log in first."));
            }
            if link.is_none() {
                *link = Some(Link::start(&mode, &session.area, &lists).map_err(storage)?);
            }
            let worker = link.as_mut().expect("just started");
            match worker.ask(&request).await {
                Ok(reply) => Ok(reply),
                Err(e) => {
                    *link = None;
                    Err(ApiError::new(
                        503,
                        "worker-stopped",
                        format!("The design stopped working ({e}). Open it again."),
                    ))
                }
            }
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
        link.take();
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
        for (_, session) in others {
            let mut link = session.link.lock().await;
            let Some(worker) = link.as_mut() else {
                continue;
            };
            match worker.ask(&WorkerRequest::ReloadLists).await {
                Ok(WorkerReply::Snapshot(s)) => {
                    if let Ok(text) = serde_json::to_string(&s) {
                        let _ = session.live.send(text);
                    }
                }
                Ok(_) => {}
                // It stopped: the next request starts a fresh one, which reads the lists anyway.
                Err(_) => *link = None,
            }
        }
    }

    /// Sends this principal's pages the current snapshot.
    async fn publish(&self, principal: &Principal) {
        if let Ok(WorkerReply::Snapshot(s)) = self.ask(principal, WorkerRequest::Snapshot).await
            && let (Ok(text), Ok(session)) = (serde_json::to_string(&s), self.session(principal))
        {
            let _ = session.live.send(text);
        }
    }
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
    match app.design.ask(&p, WorkerRequest::Snapshot).await? {
        WorkerReply::Snapshot(s) => Ok(Json(s).into_response()),
        WorkerReply::Failed { message } => Err(ApiError::new(500, "worker", message)),
        _ => Err(unexpected()),
    }
}

pub async fn command(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    headers: HeaderMap,
    uri: Uri,
    Json(body): Json<CommandRequest>,
) -> Result<Response, ApiError> {
    let p = who(&caller, &headers, &uri)?;
    let session = app.design.session(&p)?;
    let _operation = session.operations.lock().await;
    let lists = LIST_EDITS.contains(&body.id.as_str());
    let reply = app.design.ask(&p, WorkerRequest::Command(body)).await?;
    if matches!(reply, WorkerReply::Command(CommandReply::Applied { .. })) {
        app.design.publish(&p).await;
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
            let key = format!("design-save:{}:{}", p.id, key);
            let receipt = tokio::task::spawn_blocking(move || service.library.receipt(&key))
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
                None => cutcraft_web_protocol::KeyLookup::NotApplied,
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
        WorkerReply::Failed { message } => Err(ApiError::new(404, "not-found", message)),
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
    let changes = BroadcastStream::new(session.live.subscribe());
    let first = match app.design.ask(&p, WorkerRequest::Snapshot).await? {
        WorkerReply::Snapshot(s) => serde_json::to_string(&s).unwrap_or_default(),
        _ => String::new(),
    };
    let initial = tokio_stream::once(Ok(Event::default().event("snapshot").data(first)));
    // A slow page skips to the latest (spec §18: coalesced).
    let rest = changes.filter_map(|item| {
        item.ok()
            .map(|t| Ok(Event::default().event("snapshot").data(t)))
    });
    let checks =
        tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(Duration::from_secs(5)))
            .map(|_| None);
    let stream = initial
        .map(Some)
        .chain(rest.map(Some).merge(checks))
        .take_while(move |_| {
            crate::http::identify(&app, caller.key.as_deref(), caller.session.as_deref())
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
    if let WorkerReply::Replay {
        result: Some(reply),
    } = app
        .design
        .ask(&p, WorkerRequest::Replay(fingerprint.clone()))
        .await?
    {
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
    if matches!(reply, WorkerReply::Command(CommandReply::Applied { .. })) {
        app.design.publish(&p).await;
    }
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
    if let WorkerReply::Replay {
        result: Some(reply),
    } = app
        .design
        .ask(&p, WorkerRequest::Replay(fingerprint.clone()))
        .await?
    {
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
            let details = service.upload_with_base(
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
                None,
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

#[cfg(test)]
mod tests {
    use super::*;

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
        *session.link.lock().await = Some(Link::Thread(tx));
        let original = {
            let sessions = sessions.clone();
            let principal = principal.clone();
            tokio::spawn(async move { sessions.ask(&principal, WorkerRequest::Snapshot).await })
        };
        let (_, reply) = tokio::task::spawn_blocking(move || rx.recv())
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
