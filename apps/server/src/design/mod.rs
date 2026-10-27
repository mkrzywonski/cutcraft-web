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
use cutcraft_web_protocol::{
    CommandReply, CommandRequest, PROTOCOL_VERSION, PictureRequest, WorkerReply, WorkerRequest,
    codes,
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
/// Designs CutCraft can open.
const DESIGNS: &[&str] = &[
    "svg", "svgz", "pdf", "ai", "eps", "dxf", "png", "jpg", "jpeg",
];

/// How workers run.
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
    fn start(mode: &Mode, area: &Path) -> Result<Self, String> {
        match mode {
            Mode::Process(program) => {
                let mut child = tokio::process::Command::new(program)
                    .args(["worker", "--area"])
                    .arg(area)
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
                let area = area.to_path_buf();
                std::thread::spawn(move || {
                    let mut worker = worker::Worker::new(&area);
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
    link: tokio::sync::Mutex<Option<Link>>,
    /// Snapshots as they change, for this principal's pages.
    live: broadcast::Sender<String>,
}

/// Every principal's design session.
pub struct Sessions {
    mode: Mode,
    people: PathBuf,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

fn storage(e: impl std::fmt::Display) -> ApiError {
    ApiError::new(500, "storage", e.to_string())
}

impl Sessions {
    pub fn new(mode: Mode, people: PathBuf) -> Self {
        Self {
            mode,
            people,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn session(&self, principal: &Principal) -> Result<Arc<Session>, ApiError> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(s) = sessions.get(&principal.id) {
            return Ok(s.clone());
        }
        let area = people::area(&self.people, principal).map_err(storage)?;
        let session = Arc::new(Session {
            area,
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
        let mut link = session.link.lock().await;
        if link.is_none() {
            *link = Some(Link::start(&self.mode, &session.area).map_err(storage)?);
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
    let reply = app.design.ask(&p, WorkerRequest::Command(body)).await?;
    if matches!(reply, WorkerReply::Command(CommandReply::Applied { .. })) {
        app.design.publish(&p).await;
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
    match app.design.ask(&p, WorkerRequest::Lookup { key }).await? {
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
/// server, then opens in their session.
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
    let file = format!("{}.{ext}", crate::keys::random_hex(12));
    let path = session.area.join("designs").join(&file);
    tokio::fs::write(&path, &body).await.map_err(storage)?;
    let name: String = q
        .name
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect();
    let reply = app.design.ask(&p, WorkerRequest::Open { file, name }).await;
    app.design.publish(&p).await;
    match reply? {
        WorkerReply::Snapshot(s) => Ok(Json(s).into_response()),
        WorkerReply::Failed { message } => Err(ApiError::new(422, "design", message)),
        _ => Err(unexpected()),
    }
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
    Ok(Sse::new(initial.chain(rest))
        .keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(15))))
}
