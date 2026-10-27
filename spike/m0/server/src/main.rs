//! Milestone 0 (spec §19): one working slice from the browser to CutCraft's controller.
//!
//!     cutcraft-web-m0 serve --data DIR --web DIR [--listen 127.0.0.1:8443]
//!     cutcraft-web-m0 worker --area DIR        (started by serve, one per session)
//!
//! The password comes from CUTCRAFT_WEB_PASSWORD. One household user, one session, one worker:
//! enough to prove the boundary, not the product.

mod worker;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use cutcraft_web_protocol::{
    CommandReply, CommandRequest, PROTOCOL_VERSION, PictureRequest, WorkerReply, WorkerRequest,
    codes,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::broadcast;

/// Spec §18, provisional.
const UPLOAD_LIMIT: usize = 64 * 1024 * 1024;
const COOKIE: &str = "m0session";
const DESIGNS: &[&str] = &[
    "svg", "svgz", "pdf", "ai", "eps", "dxf", "png", "jpg", "jpeg",
];

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("the system's random source works");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
    };
    let result = match args.first().map(String::as_str) {
        Some("worker") => match value("--area") {
            Some(area) => worker::run(Path::new(&area)),
            None => Err("worker needs --area DIR".into()),
        },
        Some("serve") => match (value("--data"), value("--web")) {
            (Some(data), Some(web)) => serve(
                PathBuf::from(data),
                PathBuf::from(web),
                value("--listen").unwrap_or_else(|| "127.0.0.1:8443".into()),
            ),
            _ => Err("serve needs --data DIR and --web DIR".into()),
        },
        _ => Err("usage: cutcraft-web-m0 serve --data DIR --web DIR [--listen ADDR]".into()),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cutcraft-web-m0: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

struct App {
    password_hash: String,
    sessions: Mutex<HashSet<String>>,
    worker: tokio::sync::Mutex<Option<WorkerProcess>>,
    area: PathBuf,
    web: PathBuf,
    live: broadcast::Sender<String>,
}

/// The session's preparation worker. Requests are one at a time.
struct WorkerProcess {
    _child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
}

impl WorkerProcess {
    fn start(area: &Path) -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut child = tokio::process::Command::new(exe)
            .args(["worker", "--area"])
            .arg(area)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("starting a worker: {e}"))?;
        let stdin = child.stdin.take().ok_or("no worker stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("no worker stdout")?).lines();
        Ok(Self {
            _child: child,
            stdin,
            stdout,
        })
    }

    async fn ask(&mut self, request: &WorkerRequest) -> Result<WorkerReply, String> {
        let mut line = serde_json::to_string(request).map_err(|e| e.to_string())?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        match self.stdout.next_line().await.map_err(|e| e.to_string())? {
            Some(reply) => serde_json::from_str(&reply).map_err(|e| e.to_string()),
            None => Err("the worker stopped".into()),
        }
    }
}

impl App {
    /// Asks the worker, starting it if needed. A worker that stopped (a crash, its memory limit)
    /// is replaced, and the request fails: the session comes back empty in the slice; from its
    /// draft in milestone 2.
    async fn ask(&self, request: WorkerRequest) -> Result<WorkerReply, String> {
        let mut slot = self.worker.lock().await;
        if slot.is_none() {
            *slot = Some(WorkerProcess::start(&self.area)?);
        }
        let worker = slot.as_mut().expect("just started");
        match worker.ask(&request).await {
            Ok(reply) => Ok(reply),
            Err(e) => {
                *slot = None;
                Err(format!("The design stopped working ({e}). Open it again."))
            }
        }
    }

    /// Pushes the current snapshot to every open page.
    async fn publish(&self) {
        if let Ok(WorkerReply::Snapshot(s)) = self.ask(WorkerRequest::Snapshot).await
            && let Ok(text) = serde_json::to_string(&s)
        {
            let _ = self.live.send(text);
        }
    }

    fn signed_in(&self, headers: &HeaderMap) -> bool {
        let Some(cookies) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) else {
            return false;
        };
        let sessions = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        cookies.split(';').any(|c| {
            c.trim()
                .strip_prefix(COOKIE)
                .and_then(|r| r.strip_prefix('='))
                .is_some_and(|token| sessions.contains(token))
        })
    }
}

fn serve(data: PathBuf, web: PathBuf, listen: String) -> Result<(), String> {
    use argon2::password_hash::{PasswordHasher, SaltString};
    let password = std::env::var("CUTCRAFT_WEB_PASSWORD")
        .map_err(|_| "set CUTCRAFT_WEB_PASSWORD to the sign-in password")?;
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).map_err(|e| e.to_string())?;
    let salt = SaltString::encode_b64(&salt).map_err(|e| e.to_string())?;
    let password_hash = argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| e.to_string())?
        .to_string();
    let area = data.join("people").join("household");
    std::fs::create_dir_all(area.join("designs")).map_err(|e| e.to_string())?;
    let identity = certificate(&data.join("private"))?;
    let app = Arc::new(App {
        password_hash,
        sessions: Mutex::new(HashSet::new()),
        worker: tokio::sync::Mutex::new(None),
        area,
        web,
        live: broadcast::channel(16).0,
    });
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(&listen)
            .await
            .map_err(|e| format!("{listen}: {e}"))?;
        eprintln!("cutcraft-web-m0: https://{listen}/");
        serve_tls(listener, router(app), identity).await;
        Ok(())
    })
}

fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/assets/{file}", get(asset))
        .route("/api/login", post(login))
        .route("/api/upload", post(upload))
        .route("/api/command", post(command))
        .route("/api/lookup/{key}", get(lookup))
        .route("/api/snapshot", get(snapshot))
        .route("/api/geometry", get(geometry))
        .route("/api/picture", get(picture))
        .route("/api/live", get(live))
        .layer(axum::middleware::from_fn(same_protocol))
        .layer(DefaultBodyLimit::max(UPLOAD_LIMIT))
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(axum::middleware::map_response(security_headers))
        .with_state(app)
}

/// Spec §13: a client speaking another protocol version is told to reload, before anything
/// reaches a worker. Pages and sign-in carry no protocol data and pass.
async fn same_protocol(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let path = request.uri().path();
    if !path.starts_with("/api/") || path == "/api/login" {
        return next.run(request).await;
    }
    let wanted = PROTOCOL_VERSION.to_string();
    let from_header = request
        .headers()
        .get("x-cutcraft-protocol")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let from_query = request.uri().query().and_then(|q| {
        q.split('&')
            .find_map(|kv| kv.strip_prefix("protocol=").map(str::to_owned))
    });
    if from_header.or(from_query).as_deref() == Some(wanted.as_str()) {
        return next.run(request).await;
    }
    (
        StatusCode::CONFLICT,
        axum::Json(CommandReply::Refused {
            code: codes::PROTOCOL.into(),
            message: "CutCraft was updated. Reload this page.".into(),
        }),
    )
        .into_response()
}

/// Spec §13: nothing from elsewhere, nothing inline, WebAssembly allowed.
async fn security_headers(mut response: Response) -> Response {
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; \
             img-src 'self' blob: data:; connect-src 'self'; frame-ancestors 'none'; \
             base-uri 'none'; form-action 'self'",
        ),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn status(code: StatusCode, message: &str) -> Response {
    (code, message.to_owned()).into_response()
}

/// Changes must come from this site's own pages: browsers can't send this header cross-site
/// without asking first.
fn refusal(app: &App, headers: &HeaderMap) -> Option<Response> {
    if !app.signed_in(headers) {
        return Some(status(StatusCode::UNAUTHORIZED, "Sign in first."));
    }
    if headers.get("x-cutcraft").is_none() {
        return Some(status(StatusCode::FORBIDDEN, "Not from this site."));
    }
    None
}

async fn index(State(app): State<Arc<App>>) -> Response {
    match tokio::fs::read(app.web.join("index.html")).await {
        Ok(html) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            html,
        )
            .into_response(),
        Err(_) => status(StatusCode::NOT_FOUND, "Build the web shell first."),
    }
}

/// The web shell's files. Their names carry a content hash (scripts/m0-web.sh), so they're
/// cached for good.
async fn asset(State(app): State<Arc<App>>, UrlPath(file): UrlPath<String>) -> Response {
    if !file
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        || file.starts_with('.')
    {
        return status(StatusCode::NOT_FOUND, "No such file.");
    }
    let kind = match file.rsplit('.').next() {
        Some("wasm") => "application/wasm",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        _ => return status(StatusCode::NOT_FOUND, "No such file."),
    };
    match tokio::fs::read(app.web.join("assets").join(&file)).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, kind),
                (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => status(StatusCode::NOT_FOUND, "No such file."),
    }
}

#[derive(serde::Deserialize)]
struct Login {
    password: String,
}

async fn login(State(app): State<Arc<App>>, body: axum::Json<Login>) -> Response {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    let Ok(hash) = PasswordHash::new(&app.password_hash) else {
        return status(StatusCode::INTERNAL_SERVER_ERROR, "No password.");
    };
    if argon2::Argon2::default()
        .verify_password(body.password.as_bytes(), &hash)
        .is_err()
    {
        return status(StatusCode::UNAUTHORIZED, "That password isn't right.");
    }
    let token = random_hex(32);
    app.sessions
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(token.clone());
    (
        [(
            header::SET_COOKIE,
            format!("{COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Strict"),
        )],
        "Signed in.",
    )
        .into_response()
}

#[derive(serde::Deserialize)]
struct UploadQuery {
    name: String,
}

/// Spec §10: the upload becomes a server-owned copy, named by the server, then opens.
async fn upload(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> Response {
    if let Some(r) = refusal(&app, &headers) {
        return r;
    }
    let ext = q.name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if !DESIGNS.contains(&ext.as_str()) || body.is_empty() {
        return status(
            StatusCode::BAD_REQUEST,
            "That isn't a design CutCraft can open.",
        );
    }
    let file = format!("{}.{ext}", random_hex(12));
    if tokio::fs::write(app.area.join("designs").join(&file), &body)
        .await
        .is_err()
    {
        return status(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The design couldn't be stored.",
        );
    }
    let name: String = q
        .name
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect();
    let reply = app.ask(WorkerRequest::Open { file, name }).await;
    app.publish().await;
    match reply {
        Ok(WorkerReply::Snapshot(s)) => axum::Json(s).into_response(),
        Ok(WorkerReply::Failed { message }) | Err(message) => {
            status(StatusCode::UNPROCESSABLE_ENTITY, &message)
        }
        Ok(_) => status(StatusCode::INTERNAL_SERVER_ERROR, "Unexpected answer."),
    }
}

async fn command(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    body: axum::Json<CommandRequest>,
) -> Response {
    if let Some(r) = refusal(&app, &headers) {
        return r;
    }
    let reply = app.ask(WorkerRequest::Command(body.0)).await;
    // Milestone 0's lost-reply test: the change is made, but neither the answer nor the live
    // snapshot is sent, so the page must recover by looking up its key.
    if headers.get("x-m0-drop-reply").is_some() {
        return status(StatusCode::SERVICE_UNAVAILABLE, "");
    }
    if matches!(
        reply,
        Ok(WorkerReply::Command(CommandReply::Applied { .. }))
    ) {
        app.publish().await;
    }
    match reply {
        Ok(WorkerReply::Command(r)) => axum::Json(r).into_response(),
        Ok(WorkerReply::Failed { message }) | Err(message) => {
            status(StatusCode::INTERNAL_SERVER_ERROR, &message)
        }
        Ok(_) => status(StatusCode::INTERNAL_SERVER_ERROR, "Unexpected answer."),
    }
}

async fn lookup(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    UrlPath(key): UrlPath<String>,
) -> Response {
    if !app.signed_in(&headers) {
        return status(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    match app.ask(WorkerRequest::Lookup { key }).await {
        Ok(WorkerReply::Lookup(l)) => axum::Json(l).into_response(),
        Ok(WorkerReply::Failed { message }) | Err(message) => {
            status(StatusCode::INTERNAL_SERVER_ERROR, &message)
        }
        Ok(_) => status(StatusCode::INTERNAL_SERVER_ERROR, "Unexpected answer."),
    }
}

async fn snapshot(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if !app.signed_in(&headers) {
        return status(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    match app.ask(WorkerRequest::Snapshot).await {
        Ok(WorkerReply::Snapshot(s)) => axum::Json(s).into_response(),
        Ok(WorkerReply::Failed { message }) | Err(message) => {
            status(StatusCode::INTERNAL_SERVER_ERROR, &message)
        }
        Ok(_) => status(StatusCode::INTERNAL_SERVER_ERROR, "Unexpected answer."),
    }
}

async fn geometry(State(app): State<Arc<App>>, headers: HeaderMap) -> Response {
    if !app.signed_in(&headers) {
        return status(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    match app.ask(WorkerRequest::Geometry).await {
        Ok(WorkerReply::Geometry(g)) => axum::Json(g).into_response(),
        Ok(WorkerReply::Failed { message }) | Err(message) => {
            status(StatusCode::NOT_FOUND, &message)
        }
        Ok(_) => status(StatusCode::INTERNAL_SERVER_ERROR, "Unexpected answer."),
    }
}

#[derive(serde::Deserialize)]
struct PictureQuery {
    art: String,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    zoom_step: i32,
    width: u32,
    height: u32,
}

async fn picture(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(q): Query<PictureQuery>,
) -> Response {
    use base64::Engine;
    if !app.signed_in(&headers) {
        return status(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    let request = PictureRequest {
        art: q.art,
        region: [q.x0, q.y0, q.x1, q.y1],
        zoom_step: q.zoom_step,
        width: q.width,
        height: q.height,
    };
    match app.ask(WorkerRequest::Picture(request)).await {
        Ok(WorkerReply::Picture { png_base64 }) => {
            match base64::engine::general_purpose::STANDARD.decode(png_base64) {
                // Named by everything that affects it: the browser may keep it.
                Ok(png) => (
                    [
                        (header::CONTENT_TYPE, "image/png"),
                        (header::CACHE_CONTROL, "private, max-age=86400"),
                    ],
                    png,
                )
                    .into_response(),
                Err(_) => status(StatusCode::INTERNAL_SERVER_ERROR, "Bad picture."),
            }
        }
        Ok(WorkerReply::Failed { message }) | Err(message) => {
            status(StatusCode::NOT_FOUND, &message)
        }
        Ok(_) => status(StatusCode::INTERNAL_SERVER_ERROR, "Unexpected answer."),
    }
}

/// Snapshots as they change; the latest one first.
async fn live(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !app.signed_in(&headers) {
        return status(StatusCode::UNAUTHORIZED, "Sign in first.");
    }
    upgrade.on_upgrade(move |socket| follow(app, socket))
}

async fn follow(app: Arc<App>, mut socket: WebSocket) {
    let mut changes = app.live.subscribe();
    if let Ok(WorkerReply::Snapshot(s)) = app.ask(WorkerRequest::Snapshot).await
        && let Ok(text) = serde_json::to_string(&s)
        && socket.send(Message::Text(text.into())).await.is_err()
    {
        return;
    }
    loop {
        match changes.recv().await {
            Ok(text) => {
                if socket.send(Message::Text(text.into())).await.is_err() {
                    return;
                }
            }
            // A slow page skips to the latest (spec §18: coalesced).
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// The server's own certificate, made on first start (as cameo-pi's).
fn certificate(private: &Path) -> Result<Arc<rustls::ServerConfig>, String> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    let dir = private.join("tls");
    let (cert_path, key_path) = (dir.join("cert.der"), dir.join("key.der"));
    let (cert, key) = match (std::fs::read(&cert_path), std::fs::read(&key_path)) {
        (Ok(c), Ok(k)) => (c, k),
        _ => {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let host = std::fs::read_to_string("/etc/hostname")
                .map(|h| h.trim().to_owned())
                .unwrap_or_default();
            let mut names = vec!["localhost".to_owned(), "127.0.0.1".to_owned()];
            if !host.is_empty() {
                names.push(format!("{host}.local"));
            }
            let params = rcgen::CertificateParams::new(names).map_err(|e| e.to_string())?;
            let pair = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
            let cert = params.self_signed(&pair).map_err(|e| e.to_string())?;
            let (c, k) = (cert.der().to_vec(), pair.serialize_der());
            std::fs::write(&key_path, &k).map_err(|e| e.to_string())?;
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| e.to_string())?;
            }
            std::fs::write(&cert_path, &c).map_err(|e| e.to_string())?;
            (c, k)
        }
    };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(cert)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)),
        )
        .map_err(|e| e.to_string())?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

async fn serve_tls(
    listener: tokio::net::TcpListener,
    router: Router,
    config: Arc<rustls::ServerConfig>,
) {
    let acceptor = tokio_rustls::TlsAcceptor::from(config);
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let (acceptor, service) = (acceptor.clone(), router.clone());
        tokio::spawn(async move {
            let handshake =
                tokio::time::timeout(std::time::Duration::from_secs(10), acceptor.accept(stream));
            let Ok(Ok(tls)) = handshake.await else {
                return;
            };
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(
                    hyper_util::rt::TokioIo::new(tls),
                    hyper_util::service::TowerToHyperService::new(service),
                )
                .with_upgrades()
                .await;
        });
    }
}
