//! `cameo-pi`: the network cut server (spec §5).
//!
//! It owns the cutters, the job library, the protocol and (later) the web interface and MCP.
//! Until HTTPS and users arrive (spec §14), run it only on a trusted network.

mod adjusting;
mod backups;
mod copies;
mod design;
mod http;
mod keys;
mod mcp;
mod people;
mod queues;
mod service;
#[cfg(test)]
mod tests;
mod tls;
mod users;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex, mpsc};

use cameo_pi_core::cutter::{Cutter, Finished};
use cameo_pi_core::journal::Journal;
use cameo_pi_core::worker::{Link, Shared, SimLink, UsbLink, WallClock, Worker, lock};
use cutcraft_device::gpgl::CAMEO_1;
use cutcraft_sim::{Clock as SimClock, Profile, Scenario, Simulator};

use crate::http::{App, Names};
use crate::service::{CutterEntry, Service};

const HELP: &str = "\
cutcraft-web: the network cut server for Silhouette Cameo cutters.

Usage: cutcraft-web [--data DIR] [--listen ADDR] [--redirect ADDR] [--practice] [--usb] [--name HOST]...
       sudo cutcraft-web reset-admin [--data DIR] [--user NAME]
       cutcraft-web fingerprint [--data DIR]
       cutcraft-web certificate [--data DIR]

  --data DIR      Where the library and cameo-pi's own state live (default /var/lib/cameo-pi
                  where the service is installed, otherwise ./cameo-pi-data).
  --listen ADDR   Address and port to serve HTTPS on (default 127.0.0.1:8443).
  --redirect ADDR Also listen on ADDR (usually port 80) with plain HTTP that only redirects to
                  HTTPS.
  --insecure-http Serve plain HTTP instead of HTTPS (development only).
  --practice      Add the practice cutter (a Simulated Cameo with a virtual mat).
  --usb           Add the Cameo plugged in over USB.
  --name HOST     Another name the server answers to (cameo-pi.local, this machine's
                  <hostname>.local and localhost always do).
  --version

fingerprint prints the HTTPS certificate's SHA-256 fingerprint, to compare with what a browser or
CutCraft shows (spec §14.4).

certificate prints the HTTPS certificate as PEM, for a program that must be told to trust it (an
AI tool connecting to /mcp, for example with NODE_EXTRA_CA_CERTS).

reset-admin gives an admin (the first one, or NAME, created if missing) a temporary password,
printed here, to change at the next login. It ends that admin's sessions. It also repairs damaged
account files. It's the only way to recover a forgotten admin password (spec §14.1). Files it
writes keep the data folder's owner, so the service can still read them.";

/// The service's data folder (ops/cutcraft-web.service).
const SERVICE_DATA: &str = "/var/lib/cameo-pi";

/// `--data`'s default: the service's folder where it's installed, else one in the current folder.
fn default_data() -> PathBuf {
    if std::path::Path::new(SERVICE_DATA).is_dir() {
        PathBuf::from(SERVICE_DATA)
    } else {
        PathBuf::from("cameo-pi-data")
    }
}

/// Gives everything under `private` the data folder's owner. `sudo cutcraft-web reset-admin` writes
/// as root; without this, the service's own user could no longer read its accounts.
fn keep_owner(data: &std::path::Path, private: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let owner = std::fs::metadata(data)?;
    let mut folders = vec![private.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let meta = std::fs::symlink_metadata(&folder)?;
        if meta.uid() != owner.uid() || meta.gid() != owner.gid() {
            std::os::unix::fs::lchown(&folder, Some(owner.uid()), Some(owner.gid()))?;
        }
        if meta.is_dir() {
            for entry in std::fs::read_dir(&folder)? {
                folders.push(entry?.path());
            }
        }
    }
    Ok(())
}

/// `cameo-pi reset-admin`.
fn reset_admin(args: &[String]) -> Result<(), String> {
    let mut data = default_data();
    let mut user = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--data" => data = PathBuf::from(it.next().ok_or("--data needs a value")?),
            "--user" => user = Some(it.next().ok_or("--user needs a value")?.clone()),
            other => return Err(format!("unknown option {other:?}")),
        }
    }
    let private = data.join("private");
    let (name, password) = users::reset_admin(&private, user.as_deref())?;
    // Program keys of this admin end with the reset, like any password change.
    let keys = keys::Keys::open(&private).map_err(|e| e.to_string())?;
    keys.remove_user(&name).map_err(|e| e.to_string())?;
    keep_owner(&data, &private)
        .map_err(|e| format!("can't give the files back to their owner: {e}"))?;
    println!("Admin {name:?} now has the temporary password: {password}");
    println!("Log in with it on the web interface and choose a new one.");
    Ok(())
}

struct Options {
    data: PathBuf,
    listen: String,
    redirect: Option<String>,
    insecure_http: bool,
    practice: bool,
    usb: bool,
    names: Vec<String>,
}

fn options(args: &[String]) -> Result<Option<Options>, String> {
    let mut o = Options {
        data: default_data(),
        listen: "127.0.0.1:8443".into(),
        redirect: None,
        insecure_http: false,
        practice: false,
        usb: false,
        names: vec!["cameo-pi.local".into(), "localhost".into()],
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{HELP}");
                return Ok(None);
            }
            "--version" => {
                println!("cutcraft-web {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--data" => o.data = PathBuf::from(value("--data")?),
            "--listen" => o.listen = value("--listen")?,
            "--redirect" => o.redirect = Some(value("--redirect")?),
            "--insecure-http" => o.insecure_http = true,
            "--name" => o.names.push(value("--name")?),
            "--practice" => o.practice = true,
            "--usb" => o.usb = true,
            other => return Err(format!("unknown option {other:?}; see --help")),
        }
    }
    if let Some(host) = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
    {
        o.names.push(format!("{host}.local"));
    }
    if !o.practice && !o.usb {
        return Err("add at least one cutter: --practice or --usb".into());
    }
    Ok(Some(o))
}

/// Starts a cutter's worker and the thread that records its finished executions in the jobs'
/// history.
fn launch<L: Link + Send + 'static>(
    service: &Arc<Service>,
    shared: Shared,
    link: L,
    journal: Arc<Journal>,
    shutdown: Arc<AtomicBool>,
) -> (std::thread::JoinHandle<()>, std::thread::JoinHandle<()>)
where
    L::Transport: Send,
{
    let (tx, rx) = mpsc::channel::<Finished>();
    let device = Worker::new(shared.clone(), link, journal, Some(tx)).spawn(shutdown);
    let service = service.clone();
    let history = std::thread::spawn(move || {
        for finished in rx {
            let (id, name) = {
                let state = lock(&shared);
                (state.id().to_owned(), state.name().to_owned())
            };
            service.record_finished(&id, &name, &finished, "cutcraft-web");
        }
    });
    (device, history)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // A preparation worker for one person's design session, started by the server (spec §5).
    if args.first().map(String::as_str) == Some("worker") {
        let area = match args
            .iter()
            .position(|a| a == "--area")
            .and_then(|i| args.get(i + 1))
        {
            Some(area) => std::path::PathBuf::from(area),
            None => {
                eprintln!("cutcraft-web worker: --area DIR is needed");
                return ExitCode::FAILURE;
            }
        };
        let Some(lists) = args
            .iter()
            .position(|a| a == "--lists")
            .and_then(|i| args.get(i + 1))
            .map(std::path::PathBuf::from)
        else {
            eprintln!("cutcraft-web worker: --lists DIR is needed");
            return ExitCode::FAILURE;
        };
        return match design::worker::run_process(&area, &lists) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("cutcraft-web worker: {e}");
                ExitCode::FAILURE
            }
        };
    }
    // Preparation workers are synchronous. Do not reserve an entire server runtime's
    // thread stacks inside their address-space budget (especially on many-core hosts).
    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(server_main(args)),
        Err(error) => {
            eprintln!("cutcraft-web: starting runtime: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn server_main(args: Vec<String>) -> ExitCode {
    if args.first().map(String::as_str) == Some("certificate") {
        let data = match args.get(1).map(String::as_str) {
            Some("--data") => args.get(2).map(PathBuf::from),
            _ => None,
        }
        .unwrap_or_else(default_data);
        return match std::fs::read(data.join("private/tls/cert.der")) {
            Ok(cert) => {
                use base64::Engine;
                let text = base64::engine::general_purpose::STANDARD.encode(cert);
                println!("-----BEGIN CERTIFICATE-----");
                for line in text.as_bytes().chunks(64) {
                    println!("{}", String::from_utf8_lossy(line));
                }
                println!("-----END CERTIFICATE-----");
                ExitCode::SUCCESS
            }
            Err(_) => {
                eprintln!(
                    "cutcraft-web certificate: no certificate yet; start cutcraft-web once first"
                );
                ExitCode::FAILURE
            }
        };
    }
    if args.first().map(String::as_str) == Some("fingerprint") {
        let data = match args.get(1).map(String::as_str) {
            Some("--data") => args.get(2).map(PathBuf::from),
            _ => None,
        }
        .unwrap_or_else(default_data);
        return match std::fs::read(data.join("private/tls/cert.der")) {
            Ok(cert) => {
                let identity = tls::Identity {
                    cert,
                    key: Vec::new(),
                };
                println!("{}", identity.fingerprint());
                ExitCode::SUCCESS
            }
            Err(_) => {
                eprintln!(
                    "cameo-pi fingerprint: no certificate yet; start cutcraft-web once first"
                );
                ExitCode::FAILURE
            }
        };
    }
    if args.first().map(String::as_str) == Some("reset-admin") {
        return match reset_admin(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("cameo-pi reset-admin: {message}");
                ExitCode::FAILURE
            }
        };
    }
    let options = match options(&args) {
        Ok(Some(o)) => o,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cutcraft-web: {message}");
            return ExitCode::FAILURE;
        }
    };
    match run(options).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("cutcraft-web: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn run(options: Options) -> Result<(), String> {
    let boot = keys::random_hex(4);
    let private = options.data.join("private");
    let mut cutters = Vec::new();
    let mut links: Vec<(Shared, Option<Simulator>)> = Vec::new();
    if options.practice {
        let scenario = Scenario {
            name: "practice".into(),
            media: None,
            ..Scenario::default()
        };
        let sim = Simulator::new(Profile::cameo_1(), SimClock::Realtime, &scenario);
        let shared: Shared = Arc::new(Mutex::new(Cutter::new("practice", "Practice Cameo", &boot)));
        cutters.push(CutterEntry {
            id: "practice".into(),
            shared: shared.clone(),
            simulator: Some(sim.clone()),
        });
        links.push((shared, Some(sim)));
    }
    if options.usb {
        let shared: Shared = Arc::new(Mutex::new(Cutter::new("cameo", "Cameo", &boot)));
        cutters.push(CutterEntry {
            id: "cameo".into(),
            shared: shared.clone(),
            simulator: None,
        });
        links.push((shared, None));
    }
    let service =
        Arc::new(Service::new(boot.clone(), &options.data, cutters).map_err(|e| e.message)?);
    let journal = service.journal.clone();
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut device_threads = Vec::new();
    for (shared, sim) in links {
        let threads = match sim {
            Some(sim) => launch(
                &service,
                shared,
                SimLink { sim },
                journal.clone(),
                shutdown.clone(),
            ),
            None => launch(
                &service,
                shared,
                UsbLink {
                    model: &CAMEO_1,
                    clock: WallClock::default(),
                },
                journal.clone(),
                shutdown.clone(),
            ),
        };
        device_threads.push(threads);
    }

    let (events, _) = tokio::sync::broadcast::channel(256);
    // The installed path may be atomically replaced during an update. Workers must keep
    // speaking the running server's protocol until that server has drained and exited.
    #[cfg(target_os = "linux")]
    let program = PathBuf::from("/proc/self/exe");
    #[cfg(not(target_os = "linux"))]
    let program = std::env::current_exe()
        .map_err(|e| format!("can't find this program to start design workers: {e}"))?;
    let design = Arc::new(design::Sessions::new(
        design::Mode::Process(program),
        service.people.clone(),
        service.lists.clone(),
    ));
    let app = App {
        design: design.clone(),
        service,
        names: Names(options.names),
        events,
        seq: Arc::new(AtomicU64::new(0)),
        secure: !options.insecure_http,
    };
    let housekeeping = design.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            housekeeping.unload_idle(std::time::Instant::now()).await;
        }
    });
    tokio::spawn(http::watch(app.clone()));
    // The backup (spec §15): on every change, daily or weekly, with retries; looked at often.
    let scheduled = app.service.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            if scheduled.backup_due() {
                let service = scheduled.clone();
                let _ = tokio::task::spawn_blocking(move || service.backup_scheduled()).await;
            }
        }
    });
    app.service.backup_tighten();
    // Recently deleted keeps jobs 30 days (spec §7.4): looked at hourly, and once at start.
    let expiring = app.service.clone();
    tokio::spawn(async move {
        loop {
            let service = expiring.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                service.library.expire_deleted(now)
            })
            .await;
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });
    let queue_checkpoint = app.service.clone();
    let names = app.names.0.clone();
    let listener = tokio::net::TcpListener::bind(&options.listen)
        .await
        .map_err(|e| format!("can't listen on {}: {e}", options.listen))?;
    let router = http::router(app.clone());
    let (closing, closed) = tokio::sync::watch::channel(false);
    let serving = if options.insecure_http {
        eprintln!(
            "cutcraft-web {} listening on http://{} (boot {boot}; plain HTTP: development only)",
            env!("CARGO_PKG_VERSION"),
            options.listen
        );
        tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(wait_closed(closed))
            .await
            .map_err(|e| e.to_string())
        })
    } else {
        let mut cert_names = names;
        if let Ok(address) = options.listen.parse::<std::net::SocketAddr>()
            && !address.ip().is_unspecified()
        {
            cert_names.push(address.ip().to_string());
        }
        let identity = tls::load_or_create(&private, &cert_names)?;
        let config = tls::server_config(&identity)?;
        if let Some(redirect) = &options.redirect {
            let https_port = options
                .listen
                .rsplit_once(':')
                .map(|(_, p)| p.to_owned())
                .unwrap_or_else(|| "443".into());
            let plain = tokio::net::TcpListener::bind(redirect)
                .await
                .map_err(|e| format!("can't listen on {redirect}: {e}"))?;
            tokio::spawn(async move {
                let _ = axum::serve(plain, http::redirect_router(https_port)).await;
            });
        }
        eprintln!(
            "cutcraft-web {} listening on https://{} (boot {boot}; certificate {})",
            env!("CARGO_PKG_VERSION"),
            options.listen,
            identity.fingerprint()
        );
        tokio::spawn(async move {
            serve_tls(listener, router, config, closed).await;
            Ok(())
        })
    };
    // Keep listening, serving this version's assets and accepting Stop until every cutter
    // has settled. SIGTERM is a drain request, never an abort or a resumed execution.
    tokio::select! {
        _ = stopped() => {},
        _ = async { while !serving.is_finished() { tokio::time::sleep(std::time::Duration::from_millis(50)).await; } } => {
            return serving.await.map_err(|e| e.to_string())?;
        }
    }
    app.service.updates.begin();
    eprintln!("cutcraft-web: update waiting; waiting for every cutter to become idle");
    wait_idle(&app.service).await;
    design.shutdown().await.map_err(|e| e.message)?;
    // Recheck immediately before closing listeners, even if draft checkpointing took time.
    wait_idle(&app.service).await;
    closing.send_replace(true);
    // SSE ends on the stopping flag. An abandoned client cannot hold a drained process alive.
    if tokio::time::timeout(std::time::Duration::from_secs(6), async {
        while !serving.is_finished() {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .is_err()
    {
        serving.abort();
    }
    if !serving.is_finished() {
        serving.abort();
    }
    shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
    tokio::task::spawn_blocking(move || {
        for (device, history) in device_threads {
            device.join().map_err(|_| "device thread panicked")?;
            history.join().map_err(|_| "history thread panicked")?;
        }
        queue_checkpoint.persist_queues().map_err(|e| e.message)
    })
    .await
    .map_err(|e| e.to_string())??;
    eprintln!("cutcraft-web: idle, checkpointed; ready for restart");
    Ok(())
}

async fn wait_idle(service: &Service) {
    while !service.update_ready() {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
async fn wait_closed(mut closed: tokio::sync::watch::Receiver<bool>) {
    while !*closed.borrow_and_update() {
        if closed.changed().await.is_err() {
            return;
        }
    }
}

/// Resolves on Ctrl-C or SIGTERM (what `systemctl stop` sends).
async fn stopped() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = async {
            match term.as_mut() {
                Some(term) => { term.recv().await; }
                None => std::future::pending::<()>().await,
            }
        } => {}
    }
}

/// Serves the router over TLS until Ctrl-C or SIGTERM. Each connection gets its client address, for login
/// backoff; a handshake that doesn't finish in 10 s is dropped.
async fn serve_tls(
    listener: tokio::net::TcpListener,
    router: axum::Router,
    config: Arc<rustls::ServerConfig>,
    closed: tokio::sync::watch::Receiver<bool>,
) {
    let acceptor = tokio_rustls::TlsAcceptor::from(config);
    let stop = wait_closed(closed);
    tokio::pin!(stop);
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let (stream, address) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(_) => continue,
            },
            _ = &mut stop => break,
            Some(_) = connections.join_next(), if !connections.is_empty() => continue,
        };
        let acceptor = acceptor.clone();
        let service = router
            .clone()
            .layer(axum::Extension(axum::extract::ConnectInfo(address)));
        connections.spawn(async move {
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
                .await;
        });
    }
    // Existing TLS responses may still be sending. Bound abandoned keep-alive/SSE
    // connections only after all cutters are idle and preparation is checkpointed.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while connections.join_next().await.is_some() {}
    })
    .await;
    connections.abort_all();
}
