//! The HTTP side: the protocol (`/api/v1`), the live event stream, and the guards every request
//! passes (spec §14.3): the Host check, and for changes either a program's access key or a
//! browser's same-origin request with its anti-CSRF token.

use std::convert::Infallible;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Multipart, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use cameo_pi_core::cutter::Actor;
use futures_util::stream::Stream;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::keys::{Collect, random_hex};
use crate::service::{ApiError, Service, StartBody, Upload};
use crate::users::{AuthError, Identity, Role};

/// Names the Pi answers to besides IP addresses (spec §14.3, DNS-rebinding protection).
#[derive(Clone)]
pub struct Names(pub Vec<String>);

#[derive(Clone)]
pub struct App {
    pub service: Arc<Service>,
    pub names: Names,
    pub events: broadcast::Sender<Value>,
    pub seq: Arc<AtomicU64>,
    /// Served over HTTPS: cookies get `Secure`.
    pub secure: bool,
    /// Each person's design session (spec §7).
    pub design: Arc<crate::design::Sessions>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self)).into_response()
    }
}

type Result<T> = std::result::Result<T, ApiError>;

/// Runs a service call off the async threads: it may touch the disk or wait on a cutter's lock.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::new(500, "internal", e.to_string()))?
}

fn host_allowed(names: &Names, headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
        return false;
    };
    // Strip a port; keep an IPv6 literal's brackets for parsing.
    let bare = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        host.rsplit_once(':').map_or(host, |(h, _)| h)
    };
    // IP literals can't be rebound; names must be ours.
    bare.parse::<IpAddr>().is_ok() || names.0.iter().any(|n| n.eq_ignore_ascii_case(bare))
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

const CSRF_COOKIE: &str = "cpi_csrf";
const CSRF_HEADER: &str = "x-csrf-token";
const DEVICE_COOKIE: &str = "cpi_device";
const SESSION_COOKIE: &str = "cpi_session";

/// Who is asking, worked out by the guard and passed to handlers.
#[derive(Clone)]
pub struct Caller {
    pub actor: Actor,
    pub identity: Identity,
    /// The browser's session cookie, if it used one (for logout and password changes).
    pub session: Option<String>,
    /// The program's access key, if it used one (event streams re-check it).
    pub key: Option<String>,
}

impl Caller {
    fn admin(&self) -> bool {
        matches!(
            self.identity,
            Identity::Open
                | Identity::User {
                    role: Role::Admin,
                    ..
                }
        )
    }
    /// Who the design work is for (spec §6): the person, or the household in open mode.
    pub fn principal(&self) -> Option<crate::people::Principal> {
        crate::people::Principal::of(&self.identity)
    }
    fn user(&self) -> Option<&str> {
        match &self.identity {
            Identity::User { name, .. } => Some(name),
            _ => None,
        }
    }
}

/// Routes anyone may use, logged in or not: the login page and its files, logging in, and pairing
/// bootstrap (which grants nothing by itself, spec §14.1).
fn public(method: &Method, path: &str) -> bool {
    matches!(
        (method.as_str(), path),
        (
            "GET",
            "/" | "/app.js" | "/app.css" | "/icon.svg" | "/api/v1/me"
        ) | ("POST", "/api/v1/login")
    ) || matches!(path, "/api/v1/pair" | "/api/v1/pair/collect")
}

/// Routes a session that must change its password may still use.
fn while_must_change(path: &str) -> bool {
    matches!(
        path,
        "/" | "/app.js"
            | "/app.css"
            | "/icon.svg"
            | "/api/v1/me"
            | "/api/v1/me/password"
            | "/api/v1/logout"
    )
}

/// Who presents this key or session, against the current users and keys.
pub(crate) fn identify(app: &App, key: Option<&str>, session: Option<&str>) -> Result<Identity> {
    let users = &app.service.users;
    if let Some(presented) = key {
        let Some(found) = app.service.keys.check(presented) else {
            return Err(ApiError::new(
                401,
                "bad-key",
                "This program's access key isn't valid; pair it again.",
            ));
        };
        return Ok(match (users.identify(None), found.user) {
            (Identity::Locked, _) => Identity::Locked,
            (Identity::Open, _) => Identity::Open,
            // A key acts as its user, with that user's current role; it dies with them.
            // A key whose user's password has changed since, here or by the command-line
            // reset, no longer works.
            (_, Some(user)) => match users.for_key(&user, &found.credentials) {
                Some(u) => Identity::User {
                    name: u.name,
                    id: u.id,
                    role: u.role,
                    must_change: u.must_change,
                },
                None => Identity::Nobody,
            },
            (_, None) => Identity::Nobody,
        });
    }
    Ok(users.identify(session))
}

/// Every request: the Host check; then who is asking, against the current state (spec §14.1);
/// then for changes from a browser, the same-origin and anti-CSRF checks (spec §14.3).
async fn guard(State(app): State<App>, mut request: Request, next: Next) -> Response {
    let headers = request.headers().clone();
    if !host_allowed(&app.names, &headers) {
        return ApiError::new(421, "wrong-host", "This isn't one of cameo-pi's addresses.")
            .into_response();
    }
    if app.design.is_stopping()
        && !matches!(
            *request.method(),
            Method::GET | Method::HEAD | Method::OPTIONS
        )
    {
        return ApiError::new(
            503,
            "server-stopping",
            "The server is restarting. Reconnect shortly.",
        )
        .into_response();
    }
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    let changes = !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS);

    let key = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    let session = cookie(&headers, SESSION_COOKIE).map(str::to_owned);
    let app2 = app.clone();
    let (k, s) = (key.clone(), session.clone());
    let identity = match tokio::task::spawn_blocking(move || {
        identify(&app2, k.as_deref(), s.as_deref())
    })
    .await
    {
        Ok(Ok(identity)) => identity,
        Ok(Err(error)) => return error.into_response(),
        Err(e) => return ApiError::new(500, "internal", e.to_string()).into_response(),
    };
    match &identity {
        Identity::Locked if path != "/" => {
            return ApiError::new(503, "locked", AuthError::Locked.message()).into_response();
        }
        Identity::Nobody if !public(&method, &path) => {
            return ApiError::new(401, "login", "Log in to cameo-pi first.").into_response();
        }
        Identity::User {
            must_change: true, ..
        } if !while_must_change(&path) => {
            return ApiError::new(403, "change-password", "Choose a new password first.")
                .into_response();
        }
        _ => {}
    }

    if key.is_none() && changes && !matches!(path.as_str(), "/api/v1/pair" | "/api/v1/pair/collect")
    {
        let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
        let origin = headers
            .get(header::ORIGIN)
            .and_then(|o| o.to_str().ok())
            .and_then(|o| o.split_once("://").map(|(_, rest)| rest));
        if origin.is_none() || origin != host {
            return ApiError::new(
                403,
                "cross-origin",
                "Requests must come from cameo-pi's own pages.",
            )
            .into_response();
        }
        let token = headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok());
        if token.is_none() || token != cookie(&headers, CSRF_COOKIE) {
            return ApiError::new(403, "csrf", "Reload the page and try again.").into_response();
        }
    }

    let client = match &key {
        Some(k) => app
            .service
            .keys
            .check(k)
            .map(|found| found.name)
            .unwrap_or_else(|| "a program".into()),
        None => cookie(&headers, DEVICE_COOKIE)
            .map(|d| {
                d.chars()
                    .filter(|c| c.is_alphanumeric() || " -_'".contains(*c))
                    .take(40)
                    .collect::<String>()
            })
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "a browser".into()),
    };
    let user = match &identity {
        Identity::User { name, .. } => Some(name.clone()),
        _ => None,
    };
    request.extensions_mut().insert(Caller {
        actor: Actor { user, client },
        identity,
        session,
        key,
    });
    let mut response = next.run(request).await;
    if cookie(&headers, CSRF_COOKIE).is_none()
        && let Ok(value) = HeaderValue::from_str(&format!(
            "{CSRF_COOKIE}={}; Path=/; SameSite=Strict{}",
            random_hex(16),
            if app.secure { "; Secure" } else { "" }
        ))
    {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

pub fn router(app: App) -> Router {
    Router::new()
        .route("/api/v1/state", get(state))
        .route("/api/v1/events", get(events))
        .route("/api/v1/cutters/{id}/queue", post(queue))
        .route("/api/v1/cutters/{id}/start", post(start))
        .route("/api/v1/cutters/{id}/starts/{key}", get(start_result))
        .route("/api/v1/cutters/{id}/stop", post(stop))
        .route("/api/v1/cutters/{id}/pause", post(pause))
        .route("/api/v1/cutters/{id}/resume", post(resume))
        .route("/api/v1/cutters/{id}/mat-fresh", post(mat_fresh))
        .route("/api/v1/cutters/{id}/release", post(release))
        .route("/api/v1/cutters/{id}/skip", post(skip))
        .route("/api/v1/cutters/{id}/remove", post(remove))
        .route("/api/v1/cutters/{id}/practice", post(practice))
        .route("/api/v1/library", get(list))
        .route("/api/v1/library/search", get(search))
        .route(
            "/api/v1/library/folders",
            post(create_folder).delete(delete_folder),
        )
        .route("/api/v1/library/folders/move", post(move_folder))
        .route("/api/v1/library/folders/rename", post(rename_folder))
        .route("/api/v1/deleted", get(deleted))
        .route(
            "/api/v1/deleted/{entry}",
            axum::routing::delete(purge_deleted),
        )
        .route("/api/v1/deleted/{entry}/restore", post(restore_deleted))
        .route("/api/v1/deleted/{entry}/thumb", get(deleted_thumb))
        .route("/api/v1/backup", get(backup_view).patch(backup_configure))
        .route("/api/v1/backup/key", post(backup_key))
        .route(
            "/api/v1/backup/identity",
            post(backup_accept).delete(backup_forget),
        )
        .route("/api/v1/backup/encryption", post(backup_encryption))
        .route("/api/v1/backup/test", post(backup_test))
        .route("/api/v1/backup/run", post(backup_run))
        .route(
            "/api/v1/backup/restore",
            post(restore_fetch).delete(restore_cancel),
        )
        .route("/api/v1/backup/restore/apply", post(restore_apply))
        .route(
            "/api/v1/jobs",
            post(upload).layer(middleware::from_fn_with_state(
                app.clone(),
                crate::design::upload_capacity,
            )),
        )
        .route("/api/v1/jobs/{id}", get(job).patch(edit).delete(delete_job))
        .route("/api/v1/jobs/{id}/move", post(move_job))
        .route("/api/v1/jobs/{id}/adjust", post(adjust_save))
        .route("/api/v1/jobs/{id}/adjust/preview", post(adjust_preview))
        .route("/api/v1/catalog", get(catalog_get).put(catalog_put))
        .route(
            "/api/v1/jobs/{id}/revisions/{n}/{what}",
            get(download).layer(middleware::from_fn_with_state(
                app.clone(),
                crate::design::preview_capacity,
            )),
        )
        .route("/api/v1/pair", post(pair))
        .route("/api/v1/pair/collect", post(collect))
        .route("/api/v1/pair/waiting", get(waiting))
        .route("/api/v1/pair/decide", post(decide))
        .route("/api/v1/keys", get(keys).post(create_key))
        .route("/api/v1/me", get(me))
        .route("/api/v1/me/password", post(change_password))
        .route("/api/v1/login", post(login))
        .route("/api/v1/logout", post(logout))
        .route("/api/v1/users", get(list_users).post(create_user))
        .route("/api/v1/users/first", post(create_first_user))
        .route(
            "/api/v1/users/{name}",
            axum::routing::patch(edit_user).delete(remove_user),
        )
        .route("/api/v1/keys/{id}", delete(remove_key))
        .route("/", get(index))
        .route("/app.js", get(app_js))
        .route("/app.css", get(app_css))
        .route("/icon.svg", get(icon))
        .route(
            "/api/v1/jobs/{id}/revisions/{n}/geometry",
            get(geometry).layer(middleware::from_fn_with_state(
                app.clone(),
                crate::design::preview_capacity,
            )),
        )
        .route("/api/v1/design/snapshot", get(crate::design::snapshot))
        .route("/api/v1/design/command", post(crate::design::command))
        .route("/api/v1/design/lookup/{key}", get(crate::design::lookup))
        .route(
            "/api/v1/design/geometry",
            get(crate::design::geometry).layer(middleware::from_fn_with_state(
                app.clone(),
                crate::design::preview_capacity,
            )),
        )
        .route(
            "/api/v1/design/picture",
            get(crate::design::picture).layer(middleware::from_fn_with_state(
                app.clone(),
                crate::design::preview_capacity,
            )),
        )
        .route("/api/v1/design/live", get(crate::design::live))
        .route("/api/v1/design/designs", get(crate::design::designs))
        .route("/api/v1/design/open", post(crate::design::open))
        .route("/api/v1/design/save", post(crate::design::save))
        .route(
            "/api/v1/design/upload",
            post(crate::design::upload)
                .layer(axum::extract::DefaultBodyLimit::max(
                    crate::design::UPLOAD_LIMIT,
                ))
                .layer(middleware::from_fn_with_state(
                    app.clone(),
                    crate::design::upload_capacity,
                )),
        )
        .nest_service("/mcp", crate::mcp::service(app.clone()))
        .layer(axum::extract::DefaultBodyLimit::max(
            cameo_pi_core::validate::MAX_PROJECT_BYTES
                + cameo_pi_core::validate::MAX_PLAN_BYTES
                + 64 * 1024,
        ))
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app)
}

pub(crate) fn snapshot(app: &App) -> Value {
    json!({
        "boot": app.service.boot,
        "version": env!("CARGO_PKG_VERSION"),
        "seq": app.seq.load(Ordering::SeqCst),
        "cutters": app.service.cutters_view(),
        "update": { "waiting": app.service.updates.waiting(), "ready": app.service.update_ready() },
    })
}

async fn state(State(app): State<App>) -> Result<Json<Value>> {
    blocking(move || Ok(Json(snapshot(&app)))).await
}

/// The live stream (spec §9.3): a full snapshot first, then each change with the boot id and a
/// sequence number. A client that falls behind gets a fresh snapshot.
async fn events(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
) -> Sse<impl Stream<Item = std::result::Result<Event, Infallible>>> {
    let receiver = app.events.subscribe();
    let first = snapshot(&app);
    let initial = tokio_stream::once(Some(
        Event::default().event("snapshot").data(first.to_string()),
    ));
    let for_changes = app.clone();
    let changes = BroadcastStream::new(receiver).map(move |item| {
        Some(match item {
            Ok(value) => Event::default()
                .event(if value.get("cutters").is_some() {
                    "snapshot"
                } else {
                    "change"
                })
                .data(value.to_string()),
            Err(_) => Event::default()
                .event("snapshot")
                .data(snapshot(&for_changes).to_string()),
        })
    });
    // Every few seconds the caller is checked again: a removed user, a changed password or a
    // removed key ends the stream (spec §14.1).
    let checks =
        tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(Duration::from_secs(1)))
            .map(|_| None);
    let still_allowed = move || {
        !app.design.is_stopping()
            && matches!(
                identify(&app, caller.key.as_deref(), caller.session.as_deref()),
                Ok(Identity::Open
                    | Identity::User {
                        must_change: false,
                        ..
                    })
            )
    };
    let stream = initial
        .chain(changes.merge(checks))
        .take_while(move |_| still_allowed())
        .filter_map(|event| event.map(Ok));
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

/// Watches every cutter and publishes a change event when its view changes (every 250 ms).
pub async fn watch(app: App) {
    let mut last: Vec<String> = Vec::new();
    let mut last_update = String::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    loop {
        tick.tick().await;
        let service = app.service.clone();
        let Ok(views) = tokio::task::spawn_blocking(move || service.cutters_view()).await else {
            continue;
        };
        for (i, view) in views.iter().enumerate() {
            let text = view.to_string();
            if last.get(i) != Some(&text) {
                let seq = app.seq.fetch_add(1, Ordering::SeqCst) + 1;
                let _ = app.events.send(json!({
                    "boot": app.service.boot,
                    "seq": seq,
                    "cutter": view,
                }));
            }
        }
        last = views.iter().map(Value::to_string).collect();
        let update = format!(
            "{}:{}",
            app.service.updates.waiting(),
            app.service.update_ready()
        );
        if update != last_update {
            app.seq.fetch_add(1, Ordering::SeqCst);
            let _ = app.events.send(snapshot(&app));
            last_update = update;
        }
    }
}

#[derive(Deserialize)]
struct QueueBody {
    job_id: String,
    /// The revision the person was looking at; refused if the job has changed since.
    #[serde(default)]
    revision: Option<u32>,
    key: String,
}

#[derive(Deserialize)]
struct TurnBody {
    turn: String,
}

async fn queue(
    State(app): State<App>,
    Path(id): Path<String>,
    axum::Extension(caller): axum::Extension<Caller>,
    Json(body): Json<QueueBody>,
) -> Result<Json<Value>> {
    blocking(move || {
        let answer =
            app.service
                .queue(&id, &body.job_id, body.revision, caller.actor, &body.key)?;
        Ok(Json(json!(answer)))
    })
    .await
}

async fn start(
    State(app): State<App>,
    Path(id): Path<String>,
    axum::Extension(caller): axum::Extension<Caller>,
    Json(body): Json<StartBody>,
) -> Result<Json<Value>> {
    blocking(move || Ok(Json(json!(app.service.start(&id, body, caller.actor)?)))).await
}

async fn stop(State(app): State<App>, Path(id): Path<String>) -> Result<Json<Value>> {
    // Stop never waits for anything but the cutter's lock.
    blocking(move || Ok(Json(json!(app.service.stop(&id)?)))).await
}

async fn start_result(
    State(app): State<App>,
    Path((id, key)): Path<(String, String)>,
) -> Result<Json<Value>> {
    blocking(move || Ok(Json(json!(app.service.start_result(&id, &key)?)))).await
}
async fn pause(State(app): State<App>, Path(id): Path<String>) -> Result<Json<Value>> {
    blocking(move || Ok(Json(json!(app.service.pause(&id)?)))).await
}
/// "It's fresh material": a mat loaded while cameo-pi wasn't watching counts as fresh (spec §8.2).
async fn mat_fresh(
    State(app): State<App>,
    Path(id): Path<String>,
    axum::Extension(caller): axum::Extension<Caller>,
) -> Result<Json<Value>> {
    blocking(move || Ok(Json(json!(app.service.confirm_fresh(&id, caller.actor)?)))).await
}

async fn resume(State(app): State<App>, Path(id): Path<String>) -> Result<Json<Value>> {
    blocking(move || Ok(Json(json!(app.service.resume(&id)?)))).await
}
async fn release(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<TurnBody>,
) -> Result<Json<Value>> {
    blocking(move || Ok(Json(json!(app.service.release(&id, &body.turn)?)))).await
}
async fn skip(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<TurnBody>,
) -> Result<Json<Value>> {
    blocking(move || Ok(Json(json!(app.service.skip(&id, &body.turn)?)))).await
}
async fn remove(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<TurnBody>,
) -> Result<Json<Value>> {
    blocking(move || Ok(Json(json!(app.service.remove(&id, &body.turn)?)))).await
}

#[derive(Deserialize)]
struct PracticeBody {
    load: bool,
}

async fn practice(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<PracticeBody>,
) -> Result<Json<Value>> {
    blocking(move || {
        app.service.practice(&id, body.load)?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

/// A library path as `a/b/c`, split into checked names by the library.
fn split(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

#[derive(Deserialize)]
struct PathQuery {
    #[serde(default)]
    path: String,
}

async fn list(State(app): State<App>, Query(q): Query<PathQuery>) -> Result<Json<Value>> {
    blocking(move || Ok(Json(json!(app.service.list(&split(&q.path))?)))).await
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    text: String,
}

/// Jobs whose name or notes contain the text, anywhere in the library, with their folders.
async fn search(State(app): State<App>, Query(q): Query<SearchQuery>) -> Result<Json<Value>> {
    blocking(move || {
        let found = app.service.library.search(&q.text);
        Ok(Json(json!(
            found
                .into_iter()
                .map(|(folder, job)| json!({"folder": folder, "job": job}))
                .collect::<Vec<_>>()
        )))
    })
    .await
}

#[derive(Deserialize)]
struct FolderBody {
    #[serde(default)]
    parent: String,
    name: Option<String>,
    path: Option<String>,
}

async fn create_folder(
    State(app): State<App>,
    Json(body): Json<FolderBody>,
) -> Result<Json<Value>> {
    blocking(move || {
        let name = body
            .name
            .ok_or_else(|| ApiError::bad_request("A folder needs a name."))?;
        app.service.create_folder(&split(&body.parent), &name)?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

async fn delete_folder(
    State(app): State<App>,
    Json(body): Json<FolderBody>,
) -> Result<Json<Value>> {
    blocking(move || {
        let path = body
            .path
            .ok_or_else(|| ApiError::bad_request("Which folder?"))?;
        app.service.delete_folder(&split(&path))?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

#[derive(Deserialize)]
struct MoveFolderBody {
    path: String,
    #[serde(default)]
    to: String,
}

async fn move_folder(
    State(app): State<App>,
    Json(body): Json<MoveFolderBody>,
) -> Result<Json<Value>> {
    blocking(move || {
        let path = app
            .service
            .move_folder(&split(&body.path), &split(&body.to))?;
        Ok(Json(json!({ "path": path })))
    })
    .await
}

#[derive(Deserialize)]
struct RenameFolderBody {
    path: String,
    name: String,
}

async fn rename_folder(
    State(app): State<App>,
    Json(body): Json<RenameFolderBody>,
) -> Result<Json<Value>> {
    blocking(move || {
        let path = app.service.rename_folder(&split(&body.path), &body.name)?;
        Ok(Json(json!({ "path": path })))
    })
    .await
}

// --- Recently deleted (spec §7.4) ----------------------------------------------------------------

async fn deleted(State(app): State<App>) -> Result<Json<Value>> {
    blocking(move || Ok(Json(json!(app.service.deleted()?)))).await
}

async fn restore_deleted(State(app): State<App>, Path(entry): Path<String>) -> Result<Json<Value>> {
    blocking(move || Ok(Json(app.service.restore_deleted(&entry)?))).await
}

async fn purge_deleted(State(app): State<App>, Path(entry): Path<String>) -> Result<Json<Value>> {
    blocking(move || {
        app.service.purge_deleted(&entry)?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

/// Drawn by the Pi, so shown only as an image, like a job's thumbnail.
async fn deleted_thumb(State(app): State<App>, Path(entry): Path<String>) -> Result<Response> {
    let bytes = blocking(move || app.service.deleted_thumb(&entry)).await?;
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("image/svg+xml"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; sandbox"),
    );
    Ok(response)
}

/// An upload: multipart fields `folder`, `name`, `job_id` (for a new revision), `planner`,
/// `key`, and files `project` (.cutcraft) and `plan` (JSON).
async fn upload(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    mut form: Multipart,
) -> Result<Json<Value>> {
    let mut fields: std::collections::HashMap<String, Vec<u8>> = Default::default();
    while let Some(field) = form
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?
    {
        let name = field.name().unwrap_or("").to_owned();
        if !matches!(
            name.as_str(),
            "folder" | "name" | "job_id" | "planner" | "key" | "project" | "plan"
        ) {
            return Err(ApiError::bad_request(format!("Unexpected field {name:?}.")));
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        fields.insert(name, bytes.to_vec());
    }
    let text = |name: &str| -> Result<String> {
        let bytes = fields
            .get(name)
            .ok_or_else(|| ApiError::bad_request(format!("Missing {name}.")))?;
        String::from_utf8(bytes.clone())
            .map_err(|_| ApiError::bad_request(format!("{name} isn't text.")))
    };
    let upload = Upload {
        folder: split(&text("folder").unwrap_or_default()),
        name: text("name")?,
        job_id: text("job_id").ok().filter(|s| !s.is_empty()),
        planner: text("planner")?,
        key: text("key")?,
        project: fields
            .remove("project")
            .ok_or_else(|| ApiError::bad_request("Missing project."))?,
        plan: fields
            .remove("plan")
            .ok_or_else(|| ApiError::bad_request("Missing plan."))?,
    };
    blocking(move || Ok(Json(json!(app.service.upload(upload, &by(&caller))?)))).await
}

#[derive(Deserialize)]
struct JobQuery {
    revision: Option<u32>,
}

async fn job(
    State(app): State<App>,
    Path(id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<JobQuery>,
) -> Result<Json<Value>> {
    blocking(move || Ok(Json(app.service.job(&id, query.revision)?))).await
}

#[derive(Deserialize)]
struct EditBody {
    based_on: u64,
    name: Option<String>,
    notes: Option<String>,
}

async fn edit(
    State(app): State<App>,
    Path(id): Path<String>,
    axum::Extension(caller): axum::Extension<Caller>,
    Json(body): Json<EditBody>,
) -> Result<Json<Value>> {
    blocking(move || {
        let details = app.service.edit(
            &id,
            body.based_on,
            body.name.as_deref(),
            body.notes.as_deref(),
            &by(&caller),
        )?;
        Ok(Json(json!(details)))
    })
    .await
}

#[derive(Deserialize)]
struct MoveBody {
    to: String,
}

async fn move_job(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<MoveBody>,
) -> Result<Json<Value>> {
    blocking(move || {
        app.service.move_job(&id, &split(&body.to))?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

async fn delete_job(State(app): State<App>, Path(id): Path<String>) -> Result<Json<Value>> {
    blocking(move || {
        app.service.delete_job(&id)?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

/// Downloads are attachments, never shown inside cameo-pi's pages (spec §14.3). The preview is
/// drawn by the Pi, so it may be shown, but only as an image.
async fn download(
    State(app): State<App>,
    Path((id, n, what)): Path<(String, u32, String)>,
) -> Result<Response> {
    let inline = matches!(what.as_str(), "preview" | "thumb");
    let (name, kind, bytes) = blocking(move || app.service.download(&id, n, &what)).await?;
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " .-_()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&kind)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    if !inline && let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{safe}\""))
    {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; sandbox"),
    );
    Ok(response)
}

// --- Backup to another computer (spec §15) --------------------------------------------------------

async fn backup_view(State(app): State<App>) -> Json<Value> {
    Json(app.service.backup_view())
}

/// Where and how to back up; fields left out stay.
async fn backup_configure(
    State(app): State<App>,
    Json(change): Json<Value>,
) -> Result<Json<Value>> {
    blocking(move || Ok(Json(app.service.backup_configure(change)?))).await
}

async fn backup_key(State(app): State<App>) -> Result<Json<Value>> {
    blocking(move || Ok(Json(app.service.backup_make_key()?))).await
}

#[derive(Deserialize)]
struct IdentityBody {
    #[serde(default)]
    fingerprint: String,
    #[serde(default)]
    algorithm: String,
    #[serde(default)]
    host: String,
    #[serde(default)]
    port: u16,
}

async fn backup_accept(
    State(app): State<App>,
    Json(body): Json<IdentityBody>,
) -> Result<Json<Value>> {
    blocking(move || {
        Ok(Json(
            app.service
                .backup_accept(&body.fingerprint, &body.algorithm)?,
        ))
    })
    .await
}

async fn backup_forget(
    State(app): State<App>,
    Json(body): Json<IdentityBody>,
) -> Result<Json<Value>> {
    blocking(move || Ok(Json(app.service.backup_forget(&body.host, body.port)?))).await
}

#[derive(Deserialize)]
struct EncryptionBody {
    /// The new password, or none to turn encryption off.
    password: Option<String>,
}

async fn backup_encryption(
    State(app): State<App>,
    Json(body): Json<EncryptionBody>,
) -> Result<Json<Value>> {
    blocking(move || {
        Ok(Json(
            app.service.backup_encryption(body.password.as_deref())?,
        ))
    })
    .await
}

async fn backup_test(State(app): State<App>) -> Result<Json<Value>> {
    blocking(move || Ok(Json(app.service.backup_test()?))).await
}

/// Back up now: starts the run and answers at once; the backup view follows its progress.
async fn backup_run(State(app): State<App>) -> Result<Json<Value>> {
    let service = app.service.clone();
    std::thread::spawn(move || {
        let _ = service.backup_now();
    });
    Ok(Json(json!({ "started": true })))
}

#[derive(Deserialize)]
struct RestoreBody {
    #[serde(default)]
    password: Option<String>,
}

/// Restore, step one: fetch and check the whole backup (it can take a while).
async fn restore_fetch(
    State(app): State<App>,
    Json(body): Json<RestoreBody>,
) -> Result<Json<Value>> {
    blocking(move || {
        Ok(Json(
            app.service.backup_restore_fetch(body.password.as_deref())?,
        ))
    })
    .await
}

async fn restore_apply(State(app): State<App>) -> Result<Json<Value>> {
    blocking(move || Ok(Json(app.service.backup_restore_apply()?))).await
}

async fn restore_cancel(State(app): State<App>) -> Json<Value> {
    app.service.backup_restore_cancel();
    Json(json!({ "ok": true }))
}

#[derive(Deserialize)]
struct NewKeyBody {
    name: String,
}

/// A key shown once, for a program that can't ask for access itself (an AI tool). It belongs to
/// the logged-in user, as an approved one would.
async fn create_key(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    Json(body): Json<NewKeyBody>,
) -> Result<Json<Value>> {
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request("Give the program a short name."));
    }
    if caller.key.is_some() {
        return Err(ApiError::new(
            403,
            "browser-only",
            "Keys are made on cameo-pi's web page.",
        ));
    }
    let user = caller.user().map(|name| {
        let credentials = app.service.users.credentials(name).unwrap_or_default();
        (name.to_owned(), credentials)
    });
    let by = user
        .as_ref()
        .map_or("open mode".to_owned(), |(n, _)| n.clone());
    let key = app
        .service
        .keys
        .create(
            name,
            &by,
            user.as_ref().map(|(n, c)| (n.as_str(), c.as_str())),
        )
        .map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
    Ok(Json(json!({ "key": key, "name": name })))
}

#[derive(Deserialize)]
struct AdjustBody {
    /// The revision being adjusted.
    revision: u32,
    #[serde(default)]
    adjustments: cameo_pi_core::adjust::Adjustments,
    #[serde(default)]
    key: Option<String>,
}

/// Adjust's preview: the cut the changes would make, stored nowhere.
async fn adjust_preview(
    State(app): State<App>,
    Path(id): Path<String>,
    Json(body): Json<AdjustBody>,
) -> Result<Json<Value>> {
    blocking(move || {
        Ok(Json(app.service.adjust_preview(
            &id,
            body.revision,
            &body.adjustments,
        )?))
    })
    .await
}

/// Adjust's Save: the job's new revision.
async fn adjust_save(
    State(app): State<App>,
    Path(id): Path<String>,
    axum::Extension(caller): axum::Extension<Caller>,
    Json(body): Json<AdjustBody>,
) -> Result<Json<Value>> {
    blocking(move || {
        let key = body.key.unwrap_or_else(|| random_hex(16));
        let details =
            app.service
                .adjust_save(&id, body.revision, &body.adjustments, key, &by(&caller))?;
        Ok(Json(json!(details)))
    })
    .await
}

/// CutCraft's material and mat lists, as last sent.
async fn catalog_get(State(app): State<App>) -> Json<Value> {
    let mut value = json!(app.service.catalog());
    // The standard mats work without CutCraft's list.
    value["standard_mats"] = json!(
        cutcraft_job::MatProfile::presets()
            .into_iter()
            .filter(|m| m.class != cutcraft_job::Mat::Roll)
            .map(|m| m.name)
            .collect::<Vec<_>>()
    );
    Json(value)
}

/// A CutCraft sends its lists (spec §11): they replace the last ones.
async fn catalog_put(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    Json(catalog): Json<cameo_pi_core::adjust::Catalog>,
) -> Result<Json<Value>> {
    // Each CutCraft's lists are kept apart, by its key; a page's are "page".
    let key = caller
        .key
        .as_deref()
        .and_then(|k| app.service.keys.check(k))
        .map_or_else(|| "page".to_owned(), |k| k.id);
    blocking(move || Ok(Json(app.service.set_catalog(catalog, &key)?))).await
}

#[derive(Deserialize)]
struct PairBody {
    name: String,
}

async fn pair(State(app): State<App>, Json(body): Json<PairBody>) -> Result<Json<Value>> {
    let (secret, code) = app
        .service
        .keys
        .request(&body.name)
        .map_err(|e| ApiError::new(429, "too-many", e))?;
    Ok(Json(json!({ "request_secret": secret, "code": code })))
}

#[derive(Deserialize)]
struct CollectBody {
    request_secret: String,
}

async fn collect(State(app): State<App>, Json(body): Json<CollectBody>) -> Result<Json<Value>> {
    match app.service.keys.collect(&body.request_secret) {
        Some(Collect::Key { key }) => Ok(Json(json!({ "state": "key", "key": key }))),
        Some(other) => Ok(Json(json!(other))),
        None => Err(ApiError::not_found()),
    }
}

async fn waiting(State(app): State<App>) -> Json<Value> {
    Json(json!(app.service.keys.waiting()))
}

#[derive(Deserialize)]
struct DecideBody {
    code: String,
    allow: bool,
}

async fn decide(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    Json(body): Json<DecideBody>,
) -> Result<Json<Value>> {
    let user = caller.user().map(|name| {
        let credentials = app.service.users.credentials(name).unwrap_or_default();
        (name.to_owned(), credentials)
    });
    let found = app
        .service
        .keys
        .decide(
            &body.code,
            body.allow,
            &by(&caller),
            user.as_ref().map(|(n, c)| (n.as_str(), c.as_str())),
        )
        .map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
    if found {
        Ok(Json(json!({ "ok": true })))
    } else {
        Err(ApiError::not_found())
    }
}

/// Admins see every key; a user sees their own.
async fn keys(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
) -> Json<Value> {
    let all = app.service.keys.list();
    let visible: Vec<_> = if caller.admin() {
        all
    } else {
        all.into_iter()
            .filter(|k| k.user.as_deref() == caller.user())
            .collect()
    };
    Json(json!(visible))
}

async fn remove_key(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let owner = app
        .service
        .keys
        .list()
        .into_iter()
        .find(|k| k.id == id)
        .map(|k| k.user);
    match owner {
        None => return Err(ApiError::not_found()),
        Some(owner) if !caller.admin() && owner.as_deref() != caller.user() => {
            return Err(ApiError::new(
                403,
                "forbidden",
                AuthError::Forbidden.message(),
            ));
        }
        _ => {}
    }
    match app.service.keys.remove(&id) {
        Ok(true) => Ok(Json(json!({ "ok": true }))),
        Ok(false) => Err(ApiError::not_found()),
        Err(e) => Err(ApiError::new(500, "storage", e.to_string())),
    }
}

/// The web interface's own pages and files, built into the binary: nothing is fetched from
/// anywhere else, and the policy allows only cameo-pi's own scripts and styles (spec §14.3).
const PAGE_POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; \
    img-src 'self'; connect-src 'self'; manifest-src 'self'; base-uri 'none'; \
    form-action 'self'; frame-ancestors 'none'";

fn asset(kind: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, kind),
            (header::CONTENT_SECURITY_POLICY, PAGE_POLICY),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

async fn index() -> Response {
    let mut response = asset(
        "text/html; charset=utf-8",
        include_str!("../web/index.html"),
    );
    response
        .headers_mut()
        .insert("x-frame-options", HeaderValue::from_static("DENY"));
    response
}

async fn app_js() -> Response {
    asset(
        "text/javascript; charset=utf-8",
        include_str!("../web/app.js"),
    )
}

async fn app_css() -> Response {
    asset("text/css; charset=utf-8", include_str!("../web/app.css"))
}

async fn icon() -> Response {
    asset("image/svg+xml", include_str!("../web/icon.svg"))
}

async fn geometry(
    State(app): State<App>,
    Path((id, n)): Path<(String, u32)>,
) -> Result<Json<Value>> {
    blocking(move || Ok(Json(app.service.geometry(&id, n)?))).await
}

/// How an action is attributed: the user if there is one, and the device or program.
pub(crate) fn by(caller: &Caller) -> String {
    match &caller.actor.user {
        Some(user) => format!("{user} ({})", caller.actor.client),
        None => caller.actor.client.clone(),
    }
}

fn auth_error(error: AuthError) -> ApiError {
    let (status, code) = match error {
        AuthError::BadLogin => (401, "bad-login"),
        AuthError::Wait(_) => (429, "wait"),
        AuthError::Busy => (503, "busy"),
        AuthError::Weak => (400, "weak-password"),
        AuthError::BadName => (400, "bad-name"),
        AuthError::Exists => (409, "exists"),
        AuthError::NotFound => (404, "not-found"),
        AuthError::LastAdmin => (409, "last-admin"),
        AuthError::Locked => (503, "locked"),
        AuthError::Forbidden => (403, "forbidden"),
        AuthError::Storage(_) => (500, "storage"),
    };
    ApiError::new(status, code, error.message())
}

fn require_admin(caller: &Caller) -> Result<()> {
    if caller.admin() {
        Ok(())
    } else {
        Err(auth_error(AuthError::Forbidden))
    }
}

fn session_cookie(app: &App, value: &str, max_age: u64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{}",
        if app.secure { "; Secure" } else { "" }
    ))
    .expect("a valid cookie")
}

async fn me(axum::Extension(caller): axum::Extension<Caller>) -> Json<Value> {
    Json(match &caller.identity {
        Identity::Open => json!({ "mode": "open", "role": "admin" }),
        Identity::User {
            name,
            role,
            must_change,
            ..
        } => json!({ "mode": "users", "user": name, "role": role, "must_change": must_change }),
        Identity::Nobody => json!({ "mode": "users", "user": null }),
        Identity::Locked => json!({ "mode": "locked" }),
    })
}

#[derive(Deserialize)]
struct LoginBody {
    name: String,
    password: String,
}

async fn login(State(app): State<App>, request: Request) -> Result<Response> {
    let address = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_else(|| "unknown".into());
    let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let body: LoginBody =
        serde_json::from_slice(&bytes).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let service = app.service.clone();
    let cookie = blocking(move || {
        service
            .users
            .login(&body.name, &body.password, &address)
            .map_err(auth_error)
    })
    .await?;
    let mut response = Json(json!({ "ok": true })).into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        session_cookie(&app, &cookie, crate::users::SESSION_DAYS * 86_400),
    );
    Ok(response)
}

async fn logout(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
) -> Result<Response> {
    if let Some(session) = caller.session.clone() {
        let service = app.service.clone();
        blocking(move || service.users.logout(&session).map_err(auth_error)).await?;
    }
    let mut response = Json(json!({ "ok": true })).into_response();
    response
        .headers_mut()
        .append(header::SET_COOKIE, session_cookie(&app, "", 0));
    Ok(response)
}

#[derive(Deserialize)]
struct PasswordBody {
    old: String,
    new: String,
}

/// A user changes their own password: every other session and their program keys end.
async fn change_password(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    Json(body): Json<PasswordBody>,
) -> Result<Json<Value>> {
    let Some(name) = caller.user().map(str::to_owned) else {
        return Err(ApiError::bad_request("There are no users yet."));
    };
    blocking(move || {
        let users = &app.service.users;
        if !users.check_password(&name, &body.old).map_err(auth_error)? {
            return Err(auth_error(AuthError::BadLogin));
        }
        users
            .set_password(&name, &body.new, false, caller.session.as_deref())
            .map_err(auth_error)?;
        app.service
            .keys
            .remove_user(&name)
            .map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

async fn list_users(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
) -> Result<Json<Value>> {
    require_admin(&caller)?;
    blocking(move || Ok(Json(json!(app.service.users.list().map_err(auth_error)?)))).await
}

#[derive(Deserialize)]
struct NewUser {
    name: String,
    password: String,
    #[serde(default = "user_role")]
    role: Role,
}

fn user_role() -> Role {
    Role::User
}

async fn create_user(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    Json(body): Json<NewUser>,
) -> Result<Json<Value>> {
    require_admin(&caller)?;
    if caller.identity == Identity::Open {
        return Err(ApiError::bad_request(
            "Create the first user with /api/v1/users/first.",
        ));
    }
    blocking(move || {
        app.service
            .users
            .create(&body.name, &body.password, body.role)
            .map_err(auth_error)?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

#[derive(Deserialize)]
struct FirstUser {
    name: String,
    password: String,
    /// Program keys approved in open mode to keep; the rest are removed.
    #[serde(default)]
    keep_keys: Vec<String>,
}

/// Ends open mode (spec §14.1): the first user, always an admin, in one step.
async fn create_first_user(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    Json(body): Json<FirstUser>,
) -> Result<Response> {
    if caller.identity != Identity::Open {
        return Err(auth_error(AuthError::Forbidden));
    }
    tokio::spawn(async move { create_first_user_inner(app, body).await })
        .await
        .map_err(|e| ApiError::new(500, "internal", e.to_string()))?
}

async fn create_first_user_inner(app: App, body: FirstUser) -> Result<Response> {
    // Detach the household worker before moving its files; it may still have a dirty draft.
    app.design.retire_household().await?;
    let service = app.service.clone();
    let name = body.name.clone();
    let cookie = blocking(move || {
        let cookie = service
            .users
            .create_first(&body.name, &body.password)
            .map_err(auth_error)?;
        let credentials = service.users.credentials(&name).unwrap_or_default();
        service
            .keys
            .adopt(&body.keep_keys, &name, &credentials)
            .map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
        // What the household made in open mode becomes the first admin's (spec §6).
        if let Some(user) = service
            .users
            .list()
            .map_err(auth_error)?
            .into_iter()
            .find(|u| u.name == name)
        {
            crate::people::adopt_household(&service.people, &user.id)
                .map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
        }
        Ok(cookie)
    })
    .await;
    let cookie = match cookie {
        Ok(cookie) => cookie,
        Err(e) => {
            if app
                .service
                .users
                .list()
                .map(|u| u.is_empty())
                .unwrap_or(false)
            {
                app.design.resume_household();
            }
            return Err(e);
        }
    };
    let mut response = Json(json!({ "ok": true })).into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        session_cookie(&app, &cookie, crate::users::SESSION_DAYS * 86_400),
    );
    Ok(response)
}

#[derive(Deserialize)]
struct EditUser {
    role: Option<Role>,
    /// A new first password: the user must change it at their next login.
    password: Option<String>,
}

async fn edit_user(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    Path(name): Path<String>,
    Json(body): Json<EditUser>,
) -> Result<Json<Value>> {
    require_admin(&caller)?;
    blocking(move || {
        let users = &app.service.users;
        if let Some(role) = body.role {
            users.set_role(&name, role).map_err(auth_error)?;
        }
        if let Some(password) = body.password {
            users
                .set_password(&name, &password, true, None)
                .map_err(auth_error)?;
            app.service
                .keys
                .remove_user(&name)
                .map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
        }
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

async fn remove_user(
    State(app): State<App>,
    axum::Extension(caller): axum::Extension<Caller>,
    Path(name): Path<String>,
) -> Result<Json<Value>> {
    require_admin(&caller)?;
    blocking(move || {
        app.service.users.remove(&name).map_err(auth_error)?;
        app.service
            .keys
            .remove_user(&name)
            .map_err(|e| ApiError::new(500, "storage", e.to_string()))?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

/// Plain HTTP that only redirects to HTTPS (spec §14.4). Nothing else answers there.
pub fn redirect_router(https_port: String) -> Router {
    Router::new().fallback(move |headers: HeaderMap, uri: axum::http::Uri| {
        let port = https_port.clone();
        async move {
            let host = headers
                .get(header::HOST)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("cameo-pi.local");
            let bare = if host.starts_with('[') {
                host.split(']')
                    .next()
                    .map(|h| format!("{h}]"))
                    .unwrap_or_default()
            } else {
                host.rsplit_once(':').map_or(host, |(h, _)| h).to_owned()
            };
            let port = if port == "443" {
                String::new()
            } else {
                format!(":{port}")
            };
            let path = uri.path_and_query().map_or("/", |p| p.as_str());
            let location = format!("https://{bare}{port}{path}");
            (
                StatusCode::PERMANENT_REDIRECT,
                [(header::LOCATION, location)],
                "",
            )
        }
    })
}
