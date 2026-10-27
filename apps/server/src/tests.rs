//! The protocol end to end, through the real router: guards, pairing, the library, and a test
//! cut then the main cut on a Simulated Cameo stepped by the test.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use cameo_pi_core::cutter::Cutter;
use cameo_pi_core::worker::{SimLink, Worker, lock};
use cutcraft_sim::{Clock as SimClock, Media, Profile, Scenario, Simulator};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::http::{App, Names, router};
use crate::service::{CutterEntry, Service};

struct Harness {
    app: App,
    worker: Worker<SimLink>,
    sim: Simulator,
    _dir: tempfile::TempDir,
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let sim = Simulator::new(Profile::cameo_1(), SimClock::Virtual, &Scenario::default());
    let shared = Arc::new(Mutex::new(Cutter::new("practice", "Practice Cameo", "t")));
    let service = Service::new(
        "t".into(),
        dir.path(),
        vec![CutterEntry {
            id: "practice".into(),
            shared: shared.clone(),
            simulator: Some(sim.clone()),
        }],
    )
    .unwrap();
    let journal = service.journal.clone();
    let worker = Worker::new(shared, SimLink { sim: sim.clone() }, journal, None);
    let (events, _) = tokio::sync::broadcast::channel(16);
    let design = Arc::new(crate::design::Sessions::new(
        crate::design::Mode::Thread,
        service.people.clone(),
        service.lists.clone(),
    ));
    Harness {
        app: App {
            design,
            service: Arc::new(service),
            names: Names(vec!["cameo-pi.local".into()]),
            events,
            seq: Arc::new(AtomicU64::new(0)),
            secure: false,
        },
        worker,
        sim,
        _dir: dir,
    }
}

const TOKEN: &str = "test-token";

/// A request from cameo-pi's own page: right Host and Origin, and the CSRF token.
fn page(method: &str, uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "cameo-pi.local")
        .header(header::ORIGIN, "http://cameo-pi.local")
        .header(header::COOKIE, format!("cpi_csrf={TOKEN}"))
        .header("x-csrf-token", TOKEN)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn send(app: &App, request: Request<Body>) -> (StatusCode, Value) {
    let response = router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

fn get(uri: &str, host: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(header::HOST, host)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn only_cameo_pis_own_names_and_ip_addresses_are_answered() {
    let h = harness();
    let (status, _) = send(&h.app, get("/api/v1/state", "evil.example")).await;
    assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
    for host in ["cameo-pi.local", "192.0.2.20:8080", "[::1]:8080"] {
        let (status, body) = send(&h.app, get("/api/v1/state", host)).await;
        assert_eq!(status, StatusCode::OK, "{host}");
        assert_eq!(body["cutters"][0]["id"], "practice");
    }
}

#[tokio::test]
async fn a_change_from_another_site_or_without_the_token_is_refused() {
    let h = harness();
    let mut cross = page("POST", "/api/v1/cutters/practice/stop", json!({}));
    cross
        .headers_mut()
        .insert(header::ORIGIN, "http://evil.example".parse().unwrap());
    assert_eq!(send(&h.app, cross).await.0, StatusCode::FORBIDDEN);

    let mut no_token = page("POST", "/api/v1/cutters/practice/stop", json!({}));
    no_token.headers_mut().remove("x-csrf-token");
    let (status, body) = send(&h.app, no_token).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("csrf"))
    );

    // With both, the request reaches the cutter: nothing is cutting.
    let (status, body) = send(
        &h.app,
        page("POST", "/api/v1/cutters/practice/stop", json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "not-cutting");
}

#[tokio::test]
async fn a_program_pairs_and_then_uses_its_key() {
    let h = harness();
    let request = |uri: &str, body: Value| {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::HOST, "cameo-pi.local")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    // Bootstrap needs no key or token.
    let (status, asked) = send(&h.app, request("/api/v1/pair", json!({"name": "CutCraft"}))).await;
    assert_eq!(status, StatusCode::OK);
    let secret = asked["request_secret"].as_str().unwrap().to_owned();
    let collect = json!({ "request_secret": secret });
    let (_, waiting) = send(&h.app, request("/api/v1/pair/collect", collect.clone())).await;
    assert_eq!(waiting["state"], "waiting");

    // Approving is a change from the page, with its token.
    let decide = json!({"code": asked["code"], "allow": true});
    let (status, _) = send(&h.app, page("POST", "/api/v1/pair/decide", decide)).await;
    assert_eq!(status, StatusCode::OK);
    let (_, got) = send(&h.app, request("/api/v1/pair/collect", collect.clone())).await;
    let key = got["key"].as_str().unwrap().to_owned();
    let (status, _) = send(&h.app, request("/api/v1/pair/collect", collect)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the key is handed out once");

    let with_key = |key: &str| {
        let mut r = request("/api/v1/cutters/practice/stop", json!({}));
        r.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {key}").parse().unwrap(),
        );
        r
    };
    assert_eq!(send(&h.app, with_key(&key)).await.0, StatusCode::CONFLICT);
    assert_eq!(
        send(&h.app, with_key("wrong")).await.0,
        StatusCode::UNAUTHORIZED
    );
}

fn multipart(fields: &[(&str, &[u8])]) -> (String, Vec<u8>) {
    let boundary = "cameo-pi-test-boundary";
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{name}\"\r\n\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(value);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn upload(app: &App, key: &str) -> (StatusCode, Value) {
    let (project, plan) = cameo_pi_core::fixtures::upload();
    let (kind, body) = multipart(&[
        ("folder", b""),
        ("name", b"Square"),
        ("planner", cameo_pi_core::validate::PLANNER.as_bytes()),
        ("key", key.as_bytes()),
        ("project", &project),
        ("plan", &plan),
    ]);
    let mut request = page("POST", "/api/v1/jobs", json!({}));
    *request.body_mut() = Body::from(body);
    request
        .headers_mut()
        .insert(header::CONTENT_TYPE, kind.parse().unwrap());
    send(app, request).await
}

#[tokio::test]
async fn upload_queue_test_cut_then_cut_on_the_practice_cutter() {
    let mut h = harness();
    let (status, job) = upload(&h.app, "u1").await;
    assert_eq!(status, StatusCode::OK, "{job}");
    let job_id = job["id"].as_str().unwrap().to_owned();
    let (_, again) = upload(&h.app, "u1").await;
    assert_eq!(again["id"], job_id, "a retried upload makes no second job");

    let (_, listing) = send(&h.app, get("/api/v1/library", "cameo-pi.local")).await;
    assert_eq!(listing["jobs"][0]["name"], "Square");

    // A mat seen loading on the practice cutter.
    for _ in 0..3 {
        h.worker.step();
    }
    h.sim.unload();
    for _ in 0..3 {
        h.worker.step();
    }
    h.sim.load(Media::Mat12x12);
    for _ in 0..9 {
        h.worker.step();
    }

    let (status, queued) = send(
        &h.app,
        page(
            "POST",
            "/api/v1/cutters/practice/queue",
            json!({"job_id": job_id, "key": "q1"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{queued}");
    let turn = queued["turn"].as_str().unwrap().to_owned();

    let view = || lock(&h.app.service.cutters[0].shared).view();
    let start = |kind: &str, key: &str| {
        let v = view();
        let generation = match v.mat {
            cameo_pi_core::mat::Mat::Fresh(g) => g,
            other => panic!("mat {other:?}"),
        };
        page(
            "POST",
            "/api/v1/cutters/practice/start",
            json!({
                "boot": "t", "kind": kind, "turn": turn, "job_id": job_id, "revision": 1,
                "generation": generation, "action_epoch": v.action_epoch, "key": key,
            }),
        )
    };
    let original = view();
    let test_body = json!({"boot": "t", "kind": "test", "turn": turn, "job_id": job_id,
        "revision": 1, "generation": 1, "action_epoch": original.action_epoch, "key": "s1"});
    let (missing, missing_body) = send(
        &h.app,
        get("/api/v1/cutters/practice/starts/s1", "cameo-pi.local"),
    )
    .await;
    assert_eq!(missing, StatusCode::NOT_FOUND);
    assert_eq!(missing_body["error"], "start-not-found");
    assert!(view().execution.is_none(), "lookup never starts a cut");
    let (status, started) = send(&h.app, start("test", "s1")).await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let original_test_answer = started.clone();
    let replay = || page("POST", "/api/v1/cutters/practice/start", test_body.clone());
    let (status, while_running) = send(&h.app, replay()).await;
    assert_eq!(status, StatusCode::OK, "{while_running}");
    assert_eq!(while_running, started);
    let (status, lookup) = send(
        &h.app,
        get("/api/v1/cutters/practice/starts/s1", "cameo-pi.local"),
    )
    .await;
    assert_eq!((status, lookup), (StatusCode::OK, started.clone()));
    h.worker.step();
    let (status, after_finish) = send(&h.app, replay()).await;
    assert_eq!((status, after_finish), (StatusCode::OK, started.clone()));
    assert!(view().execution.is_none());
    assert_eq!(
        view().turn.unwrap().taken.len(),
        1,
        "the test square is taken"
    );

    // Straight after a cut the mat is still settling: refused with a plain reason, not run.
    let (status, body) = send(&h.app, start("main", "early")).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::CONFLICT, Some("settling"))
    );
    for _ in 0..8 {
        h.worker.step();
    }

    let (status, started) = send(&h.app, start("main", "s2")).await;
    assert_eq!(status, StatusCode::OK, "{started}");
    h.worker.step();
    let v = view();
    assert!(v.turn.is_none(), "the main cut ended the turn");
    assert!(matches!(v.mat, cameo_pi_core::mat::Mat::Used(_)));

    // A stale start (old epoch) is refused, not run.
    let stale = page(
        "POST",
        "/api/v1/cutters/practice/start",
        json!({"boot": "t", "kind": "main", "turn": null, "job_id": job_id, "revision": 1,
               "generation": 1, "action_epoch": 1, "key": "s3"}),
    );
    let (status, body) = send(&h.app, stale).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "stale-epoch");
    // Updating (and then deleting) the job cannot change an already accepted request's answer.
    publish_revision_2(&h.app, &job_id);
    let (status, after_revision) = send(
        &h.app,
        page("POST", "/api/v1/cutters/practice/start", test_body.clone()),
    )
    .await;
    assert_eq!(
        (status, after_revision),
        (StatusCode::OK, original_test_answer.clone())
    );
    let mut changed = test_body.clone();
    changed["revision"] = json!(2);
    let (status, error) = send(
        &h.app,
        page("POST", "/api/v1/cutters/practice/start", changed),
    )
    .await;
    assert_eq!(
        (status, error["error"].as_str()),
        (StatusCode::CONFLICT, Some("key-reused"))
    );
    h.app.service.delete_job(&job_id).unwrap();
    let (status, after_delete) = send(
        &h.app,
        page("POST", "/api/v1/cutters/practice/start", test_body),
    )
    .await;
    assert_eq!(
        (status, after_delete),
        (StatusCode::OK, original_test_answer)
    );
    assert!(view().execution.is_none());
}

#[tokio::test]
async fn downloads_are_attachments_and_the_design_is_the_original() {
    let h = harness();
    let (_, job) = upload(&h.app, "u1").await;
    let id = job["id"].as_str().unwrap();
    let fetch = |what: &str| {
        get(
            &format!("/api/v1/jobs/{id}/revisions/1/{what}"),
            "cameo-pi.local",
        )
    };

    let response = router(h.app.clone())
        .oneshot(fetch("design"))
        .await
        .unwrap();
    let disposition = response.headers()[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(disposition.starts_with("attachment;"), "{disposition}");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], cameo_pi_core::fixtures::SVG);

    let response = router(h.app.clone())
        .oneshot(fetch("preview"))
        .await
        .unwrap();
    assert!(
        response
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .is_none()
    );
    assert!(
        response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    let response = router(h.app.clone())
        .oneshot(fetch("../job.json"))
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_invalid_upload_is_refused_and_stores_nothing() {
    let h = harness();
    let (project, _) = cameo_pi_core::fixtures::upload();
    let (kind, body) = multipart(&[
        ("name", b"Bad"),
        ("planner", b"1.2.1 (b6aad51)"),
        ("key", b"u2"),
        ("project", &project),
        ("plan", b"\x1b\x05TT\x03"),
    ]);
    let mut request = page("POST", "/api/v1/jobs", json!({}));
    *request.body_mut() = Body::from(body);
    request
        .headers_mut()
        .insert(header::CONTENT_TYPE, kind.parse().unwrap());
    let (status, body) = send(&h.app, request).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (_, listing) = send(&h.app, get("/api/v1/library", "cameo-pi.local")).await;
    assert_eq!(listing["jobs"].as_array().unwrap().len(), 0);
}

fn with_session(mut request: Request<Body>, session: &str) -> Request<Body> {
    request.headers_mut().insert(
        header::COOKIE,
        format!("cpi_csrf={TOKEN}; cpi_session={session}")
            .parse()
            .unwrap(),
    );
    request
}

fn set_session(response_cookie: &str) -> String {
    response_cookie
        .split(';')
        .next()
        .and_then(|kv| kv.strip_prefix("cpi_session="))
        .unwrap()
        .to_owned()
}

async fn send_raw(app: &App, request: Request<Body>) -> axum::response::Response {
    router(app.clone()).oneshot(request).await.unwrap()
}

fn session_from(response: &axum::response::Response) -> String {
    let cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("cpi_session="))
        .unwrap()
        .to_owned();
    set_session(&cookie)
}

#[tokio::test]
async fn creating_the_first_user_ends_open_mode() {
    let h = harness();
    // Open mode: anyone may look and act.
    assert_eq!(
        send(&h.app, get("/api/v1/state", "cameo-pi.local")).await.0,
        StatusCode::OK
    );
    let response = send_raw(
        &h.app,
        page(
            "POST",
            "/api/v1/users/first",
            json!({"name": "Sam", "password": "correct horse"}),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let sam = session_from(&response);

    // Now nobody without a session gets in, except to log in.
    let (status, body) = send(&h.app, get("/api/v1/state", "cameo-pi.local")).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("login"))
    );
    let mut me = get("/api/v1/me", "cameo-pi.local");
    me = with_session(me, &sam);
    let (_, body) = send(&h.app, me).await;
    assert_eq!(body["user"], "Sam");
    let state = with_session(get("/api/v1/state", "cameo-pi.local"), &sam);
    assert_eq!(send(&h.app, state).await.0, StatusCode::OK);

    // Open mode is over for good: a second "first user" is refused.
    let again = with_session(
        page(
            "POST",
            "/api/v1/users/first",
            json!({"name": "Eve", "password": "12345678"}),
        ),
        &sam,
    );
    assert_eq!(send(&h.app, again).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_new_user_must_change_their_password_and_only_admins_manage_users() {
    let h = harness();
    let response = send_raw(
        &h.app,
        page(
            "POST",
            "/api/v1/users/first",
            json!({"name": "Sam", "password": "correct horse"}),
        ),
    )
    .await;
    let sam = session_from(&response);
    let add = with_session(
        page(
            "POST",
            "/api/v1/users",
            json!({"name": "Alex", "password": "first password"}),
        ),
        &sam,
    );
    assert_eq!(send(&h.app, add).await.0, StatusCode::OK);

    let login = send_raw(
        &h.app,
        page(
            "POST",
            "/api/v1/login",
            json!({"name": "Alex", "password": "first password"}),
        ),
    )
    .await;
    assert_eq!(login.status(), StatusCode::OK);
    let alex = session_from(&login);
    let state = with_session(get("/api/v1/state", "cameo-pi.local"), &alex);
    let (status, body) = send(&h.app, state).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("change-password"))
    );

    let change = with_session(
        page(
            "POST",
            "/api/v1/me/password",
            json!({"old": "first password", "new": "alex's own"}),
        ),
        &alex,
    );
    assert_eq!(send(&h.app, change).await.0, StatusCode::OK);
    let state = with_session(get("/api/v1/state", "cameo-pi.local"), &alex);
    assert_eq!(send(&h.app, state).await.0, StatusCode::OK);

    // Alex is a user, not an admin.
    let users = with_session(get("/api/v1/users", "cameo-pi.local"), &alex);
    assert_eq!(send(&h.app, users).await.0, StatusCode::FORBIDDEN);

    // Removing Alex ends their session at once.
    let remove = with_session(page("DELETE", "/api/v1/users/Alex", json!({})), &sam);
    assert_eq!(send(&h.app, remove).await.0, StatusCode::OK);
    let state = with_session(get("/api/v1/state", "cameo-pi.local"), &alex);
    assert_eq!(send(&h.app, state).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_programs_key_belongs_to_its_user_and_ends_with_them() {
    let h = harness();
    let response = send_raw(
        &h.app,
        page(
            "POST",
            "/api/v1/users/first",
            json!({"name": "Sam", "password": "correct horse"}),
        ),
    )
    .await;
    let sam = session_from(&response);
    let add = with_session(
        page(
            "POST",
            "/api/v1/users",
            json!({"name": "Alex", "password": "first password", "role": "admin"}),
        ),
        &sam,
    );
    send(&h.app, add).await;

    let bootstrap = |uri: &str, body: Value| {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::HOST, "cameo-pi.local")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let (_, asked) = send(
        &h.app,
        bootstrap("/api/v1/pair", json!({"name": "CutCraft"})),
    )
    .await;
    // Without a login, nobody can approve.
    let anonymous = page(
        "POST",
        "/api/v1/pair/decide",
        json!({"code": asked["code"], "allow": true}),
    );
    assert_eq!(send(&h.app, anonymous).await.0, StatusCode::UNAUTHORIZED);
    let approve = with_session(
        page(
            "POST",
            "/api/v1/pair/decide",
            json!({"code": asked["code"], "allow": true}),
        ),
        &sam,
    );
    assert_eq!(send(&h.app, approve).await.0, StatusCode::OK);
    let (_, got) = send(
        &h.app,
        bootstrap(
            "/api/v1/pair/collect",
            json!({"request_secret": asked["request_secret"]}),
        ),
    )
    .await;
    let key = got["key"].as_str().unwrap().to_owned();
    let with_key = || {
        let mut r = get("/api/v1/state", "cameo-pi.local");
        r.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {key}").parse().unwrap(),
        );
        r
    };
    assert_eq!(send(&h.app, with_key()).await.0, StatusCode::OK);

    // Sam is removed by the other admin: Sam's program key stops working.
    let login = send_raw(
        &h.app,
        page(
            "POST",
            "/api/v1/login",
            json!({"name": "Alex", "password": "first password"}),
        ),
    )
    .await;
    let alex = session_from(&login);
    let change = with_session(
        page(
            "POST",
            "/api/v1/me/password",
            json!({"old": "first password", "new": "alex's own"}),
        ),
        &alex,
    );
    send(&h.app, change).await;
    let remove = with_session(page("DELETE", "/api/v1/users/Sam", json!({})), &alex);
    assert_eq!(send(&h.app, remove).await.0, StatusCode::OK);
    assert_eq!(send(&h.app, with_key()).await.0, StatusCode::UNAUTHORIZED);
}

#[test]
fn review_parallel_uploads_must_not_share_staging_files() {
    use crate::service::Upload;
    let h = harness();
    let service = h.app.service.clone();
    let (project, plan) = cameo_pi_core::fixtures::upload();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let s = service.clone();
            let b = barrier.clone();
            let project = project.clone();
            let plan = plan.clone();
            std::thread::spawn(move || {
                b.wait();
                s.upload(
                    Upload {
                        folder: vec![],
                        name: format!("job-{i}"),
                        job_id: None,
                        planner: cameo_pi_core::validate::PLANNER.into(),
                        project,
                        plan,
                        key: format!("upload-{i:03}"),
                    },
                    "review",
                )
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(
        results.iter().all(|r| r.is_ok()),
        "valid simultaneous uploads interfered: {results:?}"
    );
}

#[test]
fn simultaneous_uploads_with_the_same_key_make_one_job() {
    use crate::service::Upload;
    let h = harness();
    let service = h.app.service.clone();
    let (project, plan) = cameo_pi_core::fixtures::upload();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let (s, b) = (service.clone(), barrier.clone());
            let (project, plan) = (project.clone(), plan.clone());
            std::thread::spawn(move || {
                b.wait();
                s.upload(
                    Upload {
                        folder: vec![],
                        name: "Same".into(),
                        job_id: None,
                        planner: cameo_pi_core::validate::PLANNER.into(),
                        project,
                        plan,
                        key: "one-key".into(),
                    },
                    "review",
                )
            })
        })
        .collect();
    let ids: Vec<String> = handles
        .into_iter()
        .map(|h| h.join().unwrap().unwrap().id)
        .collect();
    assert!(ids.iter().all(|id| *id == ids[0]), "{ids:?}");
    assert_eq!(service.list(&[]).unwrap().jobs.len(), 1);
}

#[tokio::test]
async fn the_web_interface_loads_before_login_with_a_strict_policy() {
    let h = harness();
    send_raw(
        &h.app,
        page(
            "POST",
            "/api/v1/users/first",
            json!({"name": "Sam", "password": "correct horse"}),
        ),
    )
    .await;
    // Logged out: the page and its files still load (they show the login form)…
    for path in ["/", "/app.js", "/app.css", "/icon.svg"] {
        let response = send_raw(&h.app, get(path, "cameo-pi.local")).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let policy = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(
            policy.contains("script-src 'self'") && !policy.contains("unsafe"),
            "{policy}"
        );
    }
    // …but nothing else does.
    let (status, _) = send(&h.app, get("/api/v1/library", "cameo-pi.local")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // The page script never builds HTML from data.
    let script = include_str!("../web/app.js");
    assert!(!script.contains("innerHTML") && !script.contains("insertAdjacentHTML"));
    assert!(
        !script.contains("style:"),
        "inline styles are blocked by the policy"
    );
}

#[tokio::test]
async fn thumbnails_are_cropped_to_the_design_and_shown_inline() {
    let h = harness();
    let (_, job) = upload(&h.app, "u1").await;
    let id = job["id"].as_str().unwrap();
    let response = send_raw(
        &h.app,
        get(
            &format!("/api/v1/jobs/{id}/revisions/1/thumb"),
            "cameo-pi.local",
        ),
    )
    .await;
    assert!(
        response
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .is_none()
    );
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let svg = String::from_utf8_lossy(&body);
    assert!(
        svg.contains("viewBox=\"-1 -1 60.7"),
        "{}",
        &svg[..120.min(svg.len())]
    );
    let (status, geometry) = send(
        &h.app,
        get(
            &format!("/api/v1/jobs/{id}/revisions/1/geometry"),
            "cameo-pi.local",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(geometry["units_per_mm"], 20.0);
    assert!(!geometry["operations"].as_array().unwrap().is_empty());
}

/// A second revision of the fixture job, with a different material and blade.
fn publish_revision_2(app: &App, id: &str) {
    let (project, _) = cameo_pi_core::fixtures::upload();
    let mut project: cutcraft_job::ProjectFile = serde_json::from_slice(&project).unwrap();
    project.project.revision += 1;
    project.project.sheets[0].material.blade = 8;
    project.project.sheets[0].material.name = "Different material".into();
    let plan = cutcraft_path::plan_sheet(&project.project, &project.project.sheets[0].id).unwrap();
    let details = app
        .service
        .upload(
            crate::service::Upload {
                folder: vec![],
                name: "Square".into(),
                job_id: Some(id.to_owned()),
                planner: cameo_pi_core::validate::PLANNER.into(),
                project: serde_json::to_vec(&project).unwrap(),
                plan: serde_json::to_vec(&plan).unwrap(),
                key: format!("revision-2-{id}"),
            },
            "another browser",
        )
        .unwrap();
    assert_eq!(details.current_revision, 2);
}

#[tokio::test]
async fn a_new_revision_moves_the_turn_and_a_page_showing_the_old_one_starts_nothing() {
    use crate::service::StartBody;
    use cameo_pi_core::cutter::{Actor, Kind};
    let mut h = harness();
    let (_, j) = upload(&h.app, "revision-1").await;
    let id = j["id"].as_str().unwrap().to_owned();
    let actor = || Actor {
        user: None,
        client: "a browser".into(),
    };
    h.app
        .service
        .queue("practice", &id, Some(1), actor(), "q1")
        .unwrap();
    h.sim.unload();
    for _ in 0..3 {
        h.worker.step();
    }
    h.sim.load(Media::Mat12x12);
    for _ in 0..9 {
        h.worker.step();
    }
    // The page as it was shown: revision 1, this mat, this epoch.
    let shown = lock(&h.app.service.cutters[0].shared).view();
    let generation = match shown.mat {
        cameo_pi_core::mat::Mat::Fresh(g) => g,
        other => panic!("mat not fresh: {other:?}"),
    };
    let turn = shown.turn.clone().unwrap();
    assert_eq!(turn.job.revision, 1);

    // Another client publishes revision 2 (blade 8) while the page is open.
    publish_revision_2(&h.app, &id);
    let now = lock(&h.app.service.cutters[0].shared).view();
    assert_eq!(
        now.turn.as_ref().unwrap().job.revision,
        2,
        "the turn follows the job"
    );
    assert!(now.action_epoch > shown.action_epoch);

    // The old page's Start is refused, whichever revision it names.
    let start = |revision: u32, epoch: u64, key: &str| StartBody {
        boot: h.app.service.boot.clone(),
        kind: Kind::Main,
        turn: Some(turn.id.clone()),
        job_id: id.clone(),
        revision,
        generation,
        action_epoch: epoch,
        key: key.into(),
    };
    let stale = h
        .app
        .service
        .start("practice", start(1, shown.action_epoch, "s1"), actor());
    assert!(
        stale.is_err(),
        "a page showing revision 1 must not start a cut"
    );
    let stale = h
        .app
        .service
        .start("practice", start(1, now.action_epoch, "s2"), actor());
    assert!(
        stale.is_err(),
        "revision 1 is refused even with the new epoch"
    );

    // The page asks for the turn's own revision: blade 8, and Start for it runs.
    let (status, page) = send(
        &h.app,
        get(&format!("/api/v1/jobs/{id}?revision=2"), "cameo-pi.local"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["blade"], 8);
    assert!(page["steps"].to_string().contains("Set the blade to 8."));
    let (_, old) = send(
        &h.app,
        get(&format!("/api/v1/jobs/{id}?revision=1"), "cameo-pi.local"),
    )
    .await;
    assert_eq!(
        old["blade"], 1,
        "a revision is described as it was published"
    );
    let fresh = h
        .app
        .service
        .start("practice", start(2, now.action_epoch, "s3"), actor());
    assert!(fresh.is_ok(), "{fresh:?}");
}

#[tokio::test]
async fn queueing_from_a_page_showing_an_old_revision_is_refused() {
    let h = harness();
    let (_, j) = upload(&h.app, "queue-old").await;
    let id = j["id"].as_str().unwrap().to_owned();
    publish_revision_2(&h.app, &id);
    let (status, body) = send(
        &h.app,
        page(
            "POST",
            "/api/v1/cutters/practice/queue",
            json!({ "job_id": id, "revision": 1, "key": "k" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(lock(&h.app.service.cutters[0].shared).view().turn.is_none());
    let (status, _) = send(
        &h.app,
        page(
            "POST",
            "/api/v1/cutters/practice/queue",
            json!({ "job_id": id, "revision": 2, "key": "k2" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn the_command_line_reset_ends_that_admins_program_keys_in_a_running_server() {
    let h = harness();
    let users = &h.app.service.users;
    users.create_first("Sam", "correct horse").unwrap();
    users
        .create("Ann", "battery staple", crate::users::Role::User)
        .unwrap();
    // Ann chooses their own password at their first login (until then their keys can't act).
    users
        .set_password("Ann", "ann's own password", false, None)
        .unwrap();
    let pair = |user: &str| {
        let keys = &h.app.service.keys;
        let (secret, code) = keys.request("a program").unwrap();
        let credentials = users.credentials(user).unwrap();
        keys.decide(&code, true, user, Some((user, &credentials)))
            .unwrap();
        match keys.collect(&secret).unwrap() {
            crate::keys::Collect::Key { key } => key,
            other => panic!("{other:?}"),
        }
    };
    let (sam, ann) = (pair("Sam"), pair("Ann"));
    let state = |key: &str| {
        let mut r = get("/api/v1/state", "cameo-pi.local");
        r.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {key}").parse().unwrap(),
        );
        r
    };
    assert_eq!(send(&h.app, state(&sam)).await.0, StatusCode::OK);

    // What `cameo-pi reset-admin` does, from another process: the running server's caches
    // don't see it happen.
    let private = h._dir.path().join("private");
    let (_, temporary) = crate::users::reset_admin(&private, Some("Sam")).unwrap();
    crate::keys::Keys::open(&private)
        .unwrap()
        .remove_user("Sam")
        .unwrap();
    assert_eq!(send(&h.app, state(&sam)).await.0, StatusCode::UNAUTHORIZED);
    assert!(
        h.app
            .service
            .keys
            .list()
            .iter()
            .all(|k| k.user.as_deref() != Some("Sam")),
        "the running server's key list shows the removal"
    );

    // The key stays dead after Sam chooses a new password, and Ann's still works.
    users
        .set_password("Sam", "a new password", false, None)
        .unwrap();
    assert!(users.check_password("Sam", "a new password").unwrap());
    assert!(!users.check_password("Sam", &temporary).unwrap());
    assert_eq!(send(&h.app, state(&sam)).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(send(&h.app, state(&ann)).await.0, StatusCode::OK);
}

#[tokio::test]
async fn a_key_survives_the_key_file_being_kept_but_not_its_users_password_change() {
    let h = harness();
    let users = &h.app.service.users;
    users.create_first("Sam", "correct horse").unwrap();
    let keys = &h.app.service.keys;
    let (secret, code) = keys.request("a program").unwrap();
    let credentials = users.credentials("Sam").unwrap();
    keys.decide(&code, true, "Sam", Some(("Sam", &credentials)))
        .unwrap();
    let crate::keys::Collect::Key { key } = keys.collect(&secret).unwrap() else {
        panic!()
    };
    let state = || {
        let mut r = get("/api/v1/state", "cameo-pi.local");
        r.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {key}").parse().unwrap(),
        );
        r
    };
    assert_eq!(send(&h.app, state()).await.0, StatusCode::OK);
    // Even if nothing removes the key itself, a password change ends it.
    users
        .set_password("Sam", "another password", false, None)
        .unwrap();
    assert_eq!(send(&h.app, state()).await.0, StatusCode::UNAUTHORIZED);
}

/// CutCraft's own client (`cutcraft-station`, at the pinned CutCraft commit) against this server
/// over real HTTPS: pairing with the certificate pinned, the state, an upload, the library, a
/// turn, the job's history, and the refusals for a changed certificate and a removed key.
#[tokio::test(flavor = "multi_thread")]
async fn cutcraft_pairs_and_works_through_the_real_protocol() {
    use cutcraft_station::{Answer, Collected, Error, Fingerprint, Pairing, Station, Upload};
    let h = harness();
    let identity =
        crate::tls::load_or_create(&h._dir.path().join("private"), &["localhost".into()]).unwrap();
    let config = crate::tls::server_config(&identity).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(crate::serve_tls(listener, router(h.app.clone()), config));
    let service = h.app.service.clone();
    let fingerprint = identity.fingerprint();
    tokio::task::spawn_blocking(move || {
        let pairing = Pairing::begin(&address, "CutCraft on the test machine").unwrap();
        assert_eq!(
            pairing.fingerprint.to_string(),
            fingerprint,
            "the pinned certificate is the Pi's"
        );
        assert_eq!(pairing.collect().unwrap(), Collected::Waiting);
        assert!(
            service
                .keys
                .decide(&pairing.code, true, "admin", None)
                .unwrap()
        );
        let Collected::Paired(credentials) = pairing.collect().unwrap() else {
            panic!("not paired");
        };
        let station = Station::connect(credentials.clone()).unwrap();

        let state = station.state().unwrap();
        assert_eq!(state.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(state.cutters[0].id, "practice");

        let (project, plan) = cameo_pi_core::fixtures::upload();
        let upload = |key: &str| {
            station.upload(Upload {
                folder: &[],
                name: "Square",
                job_id: None,
                planner: cameo_pi_core::validate::PLANNER,
                project: project.clone(),
                plan: plan.clone(),
                key,
            })
        };
        let job = upload("cutcraft-u1").unwrap();
        assert_eq!(
            upload("cutcraft-u1").unwrap().id,
            job.id,
            "a retried upload is answered"
        );
        assert_eq!(station.library(&[]).unwrap().jobs[0].id, job.id);
        assert!(station.job(&job.id).unwrap().history.is_empty());

        let Answer::Queued { turn, position: 0 } = station
            .queue("practice", &job.id, 1, "cutcraft-q1")
            .unwrap()
        else {
            panic!("not the current turn");
        };
        let view = station.state().unwrap().cutters.remove(0).view;
        assert_eq!(view.turn.unwrap().id, turn);
        assert_eq!(
            station.release("practice", &turn).unwrap(),
            Answer::Released { turn }
        );

        // Another certificate where the Pi was: refused, never re-trusted.
        let other = Station::connect(cutcraft_station::Credentials {
            fingerprint: Fingerprint::of(b"another certificate"),
            ..credentials.clone()
        })
        .unwrap();
        assert!(matches!(other.state(), Err(Error::IdentityChanged { .. })));

        // A removed key no longer works.
        for key in service.keys.list() {
            service.keys.remove(&key.id).unwrap();
        }
        assert!(matches!(station.state(), Err(Error::NotPaired(_))));
    })
    .await
    .unwrap();
}

/// MCP on the Pi (spec §13) through the real router: a key made on the web page, then initialize,
/// the tool list, the cutters, a library search and a queued turn marked with the program; no key
/// or a browser Origin is refused.
#[tokio::test]
async fn an_ai_tool_works_through_mcp_with_a_key() {
    let h = harness();
    let (_, job) = upload(&h.app, "mcp-u1").await;
    let (status, made) = send(
        &h.app,
        page("POST", "/api/v1/keys", json!({"name": "Claude"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let key = made["key"].as_str().unwrap().to_owned();

    let rpc = |session: Option<String>, body: Value, key: Option<&str>, origin: Option<&str>| {
        let mut request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::HOST, "cameo-pi.local")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream");
        if let Some(key) = key {
            request = request.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        if let Some(session) = session {
            request = request.header("mcp-session-id", session);
        }
        request.body(Body::from(body.to_string())).unwrap()
    };
    // One router, as the server has: MCP sessions live in it.
    let mcp_router = router(h.app.clone());
    async fn call(
        app: &axum::Router,
        request: Request<Body>,
    ) -> (StatusCode, Option<String>, Value) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let session = response
            .headers()
            .get("mcp-session-id")
            .map(|v| v.to_str().unwrap().to_owned());
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        // An event stream: the reply is the last data line.
        let value = text
            .lines()
            .rev()
            .filter_map(|l| l.strip_prefix("data:"))
            .find_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
            .or_else(|| serde_json::from_str(&text).ok())
            .unwrap_or_else(|| json!({"status": status.as_u16(), "raw": text.to_string()}));
        (status, session, value)
    }
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}});

    let (status, _, _) = call(&mcp_router, rpc(None, init.clone(), None, None)).await;
    assert_ne!(status, StatusCode::OK, "no key, no MCP");
    let (status, _, _) = call(
        &mcp_router,
        rpc(None, init.clone(), Some(&key), Some("https://evil.example")),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "a browser Origin is refused");

    let (status, session, reply) = call(&mcp_router, rpc(None, init, Some(&key), None)).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["result"]["serverInfo"]["name"], "cameo-pi");
    let session = session.expect("a session");
    let initialized = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    call(
        &mcp_router,
        rpc(Some(session.clone()), initialized, Some(&key), None),
    )
    .await;

    let tool = |id: u64, name: &str, arguments: Value| {
        rpc(
            Some(session.clone()),
            json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": arguments}}),
            Some(&key),
            None,
        )
    };
    let (_, _, tools) = call(
        &mcp_router,
        rpc(
            Some(session.clone()),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            Some(&key),
            None,
        ),
    )
    .await;
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list: {tools}"))
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"cut_start") && names.contains(&"library_search"),
        "{names:?}"
    );

    assert!(names.contains(&"design_status") && names.contains(&"design_cut_copy"));
    open_star(&h.app, None).await;
    let (_, _, status) = call(&mcp_router, tool(90, "design_status", json!({}))).await;
    let snapshot = &status["result"]["structuredContent"]["result"];
    let args = json!({"key":"mcp-copy", "incarnation":snapshot["incarnation"], "revision":snapshot["revision"]});
    let (_, _, prepared) = call(&mcp_router, tool(91, "design_cut_copy", args.clone())).await;
    assert_ne!(prepared["result"]["isError"], true, "{prepared}");
    assert!(
        prepared["result"]["structuredContent"]["result"]["value"]["copy"]["id"]
            .as_str()
            .unwrap()
            .starts_with("copy-")
    );
    let replay = post_design(
        &h.app,
        "/api/v1/design/command",
        json!({"id":"cut.copy", "params":{},
        "key":"mcp-copy", "incarnation":args["incarnation"], "revision":args["revision"]}),
        None,
    )
    .await;
    assert_eq!(replay, prepared["result"]["structuredContent"]["result"]);
    let (_, _, cutters) = call(&mcp_router, tool(3, "cutters", json!({}))).await;
    assert_eq!(
        cutters["result"]["structuredContent"]["result"]["cutters"][0]["id"],
        "practice"
    );
    let (_, _, found) = call(
        &mcp_router,
        tool(4, "library_search", json!({"text": "squ"})),
    )
    .await;
    assert_eq!(
        found["result"]["structuredContent"]["result"][0]["job"]["id"],
        job["id"]
    );
    let (_, _, queued) = call(
        &mcp_router,
        tool(
            5,
            "cut_queue",
            json!({"cutter": "practice", "job_id": job["id"], "revision": 1}),
        ),
    )
    .await;
    assert_eq!(
        queued["result"]["structuredContent"]["result"]["answer"], "queued",
        "{queued}"
    );
    let turn = lock(&h.app.service.cutters[0].shared).view().turn.unwrap();
    assert_eq!(turn.actor.client, "Claude", "marked with the program");
    let (_, _, refused) = call(&mcp_router, tool(6, "job_get", json!({"job_id": "nope"}))).await;
    assert_eq!(refused["result"]["isError"], true);
    assert_eq!(refused["result"]["structuredContent"]["code"], "not-found");
    let (_, _, adjusted) = call(
        &mcp_router,
        tool(7, "job_adjust_preview", json!({"job_id": job["id"], "revision": 1, "adjustments": {"width_mm": 30.0, "anchor": "centre"}})),
    )
    .await;
    assert_eq!(
        adjusted["result"]["structuredContent"]["result"]["valid"], true,
        "{adjusted}"
    );
    // Folders: make, move (never into itself), delete.
    for (n, name) in [(8, "A"), (9, "B")] {
        let (_, _, made) = call(&mcp_router, tool(n, "folder_create", json!({"name": name}))).await;
        assert_eq!(made["result"]["isError"], false, "{made}");
    }
    let (_, _, moved) = call(
        &mcp_router,
        tool(10, "folder_move", json!({"path": ["A"], "to": ["B"]})),
    )
    .await;
    assert_eq!(
        moved["result"]["structuredContent"]["result"]["path"],
        json!(["B", "A"]),
        "{moved}"
    );
    let (_, _, inside) = call(
        &mcp_router,
        tool(11, "folder_move", json!({"path": ["B"], "to": ["B", "A"]})),
    )
    .await;
    assert_eq!(
        inside["result"]["structuredContent"]["code"], "into-itself",
        "{inside}"
    );
    let (_, _, renamed) = call(
        &mcp_router,
        tool(
            13,
            "folder_rename",
            json!({"path": ["B", "A"], "name": "Logos"}),
        ),
    )
    .await;
    assert_eq!(
        renamed["result"]["structuredContent"]["result"]["path"],
        json!(["B", "Logos"]),
        "{renamed}"
    );
    let (_, _, kept) = call(
        &mcp_router,
        tool(
            14,
            "folder_rename",
            json!({"path": ["B"], "name": "Recently deleted"}),
        ),
    )
    .await;
    assert_eq!(
        kept["result"]["structuredContent"]["code"], "exists",
        "{kept}"
    );
    let (_, _, bin) = call(&mcp_router, tool(15, "deleted_list", json!({}))).await;
    assert_eq!(
        bin["result"]["structuredContent"]["result"],
        json!([]),
        "{bin}"
    );
    let (_, _, none) = call(
        &mcp_router,
        tool(16, "deleted_restore", json!({"entry": "../library"})),
    )
    .await;
    assert_eq!(
        none["result"]["structuredContent"]["code"], "not-found",
        "{none}"
    );
    let (_, _, deleted) = call(
        &mcp_router,
        tool(12, "folder_delete", json!({"path": ["B", "Logos"]})),
    )
    .await;
    assert_eq!(
        deleted["result"]["structuredContent"]["result"]["deleted"], true,
        "{deleted}"
    );
}

/// The practice cutter takes one mat at a time, as the real Cameo does: Load needs it empty
/// (a used mat comes out first), Unload needs a mat.
#[tokio::test]
async fn the_practice_mat_loads_only_when_empty() {
    let h = harness();
    let practice = |load: bool| {
        page(
            "POST",
            "/api/v1/cutters/practice/practice",
            json!({"load": load}),
        )
    };
    // The test harness's simulator starts with a mat in.
    assert_eq!(send(&h.app, practice(false)).await.0, StatusCode::OK);
    let (status, body) = send(&h.app, practice(false)).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::CONFLICT, Some("no-mat"))
    );
    assert_eq!(send(&h.app, practice(true)).await.0, StatusCode::OK);
    let (status, body) = send(&h.app, practice(true)).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::CONFLICT, Some("mat-loaded"))
    );
}

/// Adjust (spec §12.1) through the router: CutCraft's lists arrive, a preview changes nothing,
/// Save makes a verified revision that remembers what it was adjusted from, and a stale or
/// uncuttable adjustment is refused. A material changed in CutCraft since shows as drift.
#[tokio::test]
async fn a_job_is_adjusted_into_a_new_verified_revision() {
    let h = harness();
    let (_, job) = upload(&h.app, "adjust-u1").await;
    let id = job["id"].as_str().unwrap().to_owned();
    let (_, details) = send(&h.app, get(&format!("/api/v1/jobs/{id}"), "cameo-pi.local")).await;
    let material = details["settings"]["material"].clone();
    assert!(material["force"].is_u64(), "{details}");

    // CutCraft's list has the same material with a higher force: drift.
    let mut changed = material.clone();
    changed["force"] = json!(material["force"].as_u64().unwrap() + 3);
    let catalog =
        json!({"materials": [changed], "mats": [], "from": "CutCraft on the test machine"});
    let (status, sent) = send(&h.app, page("PUT", "/api/v1/catalog", catalog)).await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    let (_, details) = send(&h.app, get(&format!("/api/v1/jobs/{id}"), "cameo-pi.local")).await;
    assert_eq!(details["material_drift"]["force"], changed["force"]);

    let preview = json!({"revision": 1, "adjustments": {"width_mm": 30.0, "anchor": "centre", "mirror": true}});
    let (status, seen) = send(
        &h.app,
        page(
            "POST",
            &format!("/api/v1/jobs/{id}/adjust/preview"),
            preview,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert_eq!(seen["valid"], true);
    assert!((seen["settings"]["design_size_mm"][0].as_f64().unwrap() - 30.0).abs() < 0.01);
    assert!(
        !seen["geometry"]["operations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        h.app.service.library.details(&id).unwrap().current_revision,
        1,
        "a preview stores nothing"
    );

    let save = json!({"revision": 1, "adjustments": {"width_mm": 30.0, "material": material["name"], "passes": 2}});
    let (status, saved) = send(
        &h.app,
        page("POST", &format!("/api/v1/jobs/{id}/adjust"), save.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["current_revision"], 2);
    let meta = h.app.service.library.revision(&id, 2).unwrap();
    assert_eq!(meta.adjusted_from, Some(1));
    assert_eq!(
        meta.planner,
        cameo_pi_core::validate::PLANNER,
        "planned here, so verified"
    );
    let (_, details) = send(&h.app, get(&format!("/api/v1/jobs/{id}"), "cameo-pi.local")).await;
    assert_eq!(
        details["settings"]["material"]["force"], changed["force"],
        "the list's material came with it"
    );
    assert_eq!(details["settings"]["material"]["passes"], 2);

    // Adjusting revision 1 again is stale now; a design too big for its piece can't be saved.
    let (status, _) = send(
        &h.app,
        page("POST", &format!("/api/v1/jobs/{id}/adjust"), save),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let tiny = json!({"revision": 2, "adjustments": {"piece_mm": [10.0, 10.0]}});
    let (status, refused) = send(
        &h.app,
        page("POST", &format!("/api/v1/jobs/{id}/adjust"), tiny),
    )
    .await;
    assert_eq!(
        (status, refused["error"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("cannot-cut"))
    );
}

/// Several CutCraft computers' lists (spec §12.1): each key's lists are kept apart and shown
/// together, the newest winning a shared name; a re-send replaces only that computer's lists, and
/// a removed key's lists go. An 0.14.0 one-list file still reads.
#[tokio::test]
async fn each_cutcraft_has_its_own_lists_and_the_pi_shows_them_together() {
    let h = harness();
    let key_a = h
        .app
        .service
        .keys
        .create("CutCraft on A", "test", None)
        .unwrap();
    let key_b = h
        .app
        .service
        .keys
        .create("CutCraft on B", "test", None)
        .unwrap();
    let defaults = cutcraft_job::Material::defaults();
    let named = |i: usize, force: u8| {
        let mut m = serde_json::to_value(&defaults[i]).unwrap();
        m["force"] = json!(force);
        m
    };
    let put = |key: &str, materials: Value, from: &str| {
        Request::builder()
            .method("PUT")
            .uri("/api/v1/catalog")
            .header(header::HOST, "cameo-pi.local")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {key}"))
            .body(Body::from(
                json!({"materials": materials, "mats": [], "from": from}).to_string(),
            ))
            .unwrap()
    };
    // The old one-list file, before any key sends.
    let old = json!({"materials": [named(2, 7)], "mats": [], "from": "CutCraft (older)", "at": 1});
    std::fs::write(h._dir.path().join("private/catalog.json"), old.to_string()).unwrap();
    assert_eq!(
        h.app.service.catalog().materials.len(),
        1,
        "the 0.14.0 file reads"
    );

    assert_eq!(
        send(&h.app, put(&key_a, json!([named(0, 10)]), "CutCraft on A"))
            .await
            .0,
        StatusCode::OK
    );
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert_eq!(
        send(
            &h.app,
            put(&key_b, json!([named(0, 14), named(1, 9)]), "CutCraft on B")
        )
        .await
        .0,
        StatusCode::OK
    );
    let catalog = h.app.service.catalog();
    assert_eq!(
        catalog.materials.len(),
        2,
        "A's and B's, and the old list is gone now that keys sent"
    );
    let shared = catalog
        .materials
        .iter()
        .find(|m| m.name == defaults[0].name)
        .unwrap();
    assert_eq!(shared.force, 14, "B sent it later");
    assert_eq!(catalog.senders.len(), 2);

    // A sends again with nothing: only A's list changes.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    send(&h.app, put(&key_a, json!([]), "CutCraft on A")).await;
    assert_eq!(
        h.app.service.catalog().materials.len(),
        2,
        "B's lists are untouched"
    );

    // B's key goes: so do its lists.
    let b = h
        .app
        .service
        .keys
        .list()
        .into_iter()
        .find(|k| k.name == "CutCraft on B")
        .unwrap();
    h.app.service.keys.remove(&b.id).unwrap();
    assert!(h.app.service.catalog().materials.is_empty());
}

/// "It has fresh material" through the router: refused while nothing unseen is loaded, accepted
/// for a settled unseen mat, then the mat is fresh with who confirmed it.
#[tokio::test]
async fn an_unseen_mat_can_be_confirmed_fresh_through_the_api() {
    let mut h = harness();
    // The harness's simulated cutter starts loaded, and its load isn't seen.
    for _ in 0..10 {
        h.worker.step();
    }
    let view = lock(&h.app.service.cutters[0].shared).view();
    assert_eq!(view.mat, cameo_pi_core::mat::Mat::Unseen, "{view:?}");
    let (status, answer) = send(
        &h.app,
        page("POST", "/api/v1/cutters/practice/mat-fresh", json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["answer"], "mat-confirmed");
    let view = lock(&h.app.service.cutters[0].shared).view();
    assert!(matches!(view.mat, cameo_pi_core::mat::Mat::Fresh(_)));
    assert!(view.mat_confirmed_by.is_some());
    let (status, refused) = send(
        &h.app,
        page("POST", "/api/v1/cutters/practice/mat-fresh", json!({})),
    )
    .await;
    assert_eq!(
        (status, refused["error"].as_str()),
        (StatusCode::CONFLICT, Some("nothing-to-confirm"))
    );
}

// --- Design sessions (spec §6, §7) ----------------------------------------------------------

const STAR: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="60mm" viewBox="0 0 80 60"><path fill="#d22" d="M20 5 L24.7 17.6 L38 17.6 L27.2 25.4 L31.4 38 L20 30.2 L8.6 38 L12.8 25.4 L2 17.6 L15.3 17.6 Z"/></svg>"##;

/// The design files stored in an area's `designs` folder (not its index).
fn stored_designs(folder: &std::path::Path) -> usize {
    std::fs::read_dir(folder).map_or(0, |entries| {
        entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name() != "index.json")
            .count()
    })
}

/// A design-session request from cameo-pi's own page, speaking this protocol version.
fn design(method: &str, uri: &str, body: Body, session: Option<&str>) -> Request<Body> {
    let mut request = page(method, uri, Value::Null);
    *request.body_mut() = body;
    request.headers_mut().insert(
        "x-cutcraft-protocol",
        cutcraft_web_protocol::PROTOCOL_VERSION
            .to_string()
            .parse()
            .unwrap(),
    );
    match session {
        Some(s) => with_session(request, s),
        None => request,
    }
}

fn upload_star(session: Option<&str>) -> Request<Body> {
    let mut r = design(
        "POST",
        "/api/v1/design/upload?name=star.svg",
        Body::from(STAR),
        session,
    );
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    r
}

/// Uploads the star and opens it in the session: the snapshot after.
async fn open_star(app: &App, session: Option<&str>) -> Value {
    let (status, uploaded) = send(app, upload_star(session)).await;
    assert_eq!(status, StatusCode::OK, "{uploaded}");
    let s = now(app, session).await;
    let body = json!({"design": uploaded["design"]["id"], "key": format!("open-{}", crate::keys::random_hex(4)),
        "incarnation": s["incarnation"], "revision": s["revision"]});
    let reply = post_design(app, "/api/v1/design/open", body, session).await;
    assert_eq!(reply["answer"], "applied", "{reply}");
    now(app, session).await
}

async fn post_design(app: &App, uri: &str, body: Value, session: Option<&str>) -> Value {
    let (status, reply) = send(
        app,
        design("POST", uri, Body::from(body.to_string()), session),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    reply
}

async fn now(app: &App, session: Option<&str>) -> Value {
    send(
        app,
        design("GET", "/api/v1/design/snapshot", Body::empty(), session),
    )
    .await
    .1
}

async fn set_material(app: &App, key: &str, material: &str, session: Option<&str>) -> Value {
    let s = now(app, session).await;
    post_design(
        app,
        "/api/v1/design/command",
        json!({"id": "sheet.set", "params": {"material": material},
        "key": key, "incarnation": s["incarnation"], "revision": s["revision"]}),
        session,
    )
    .await
}

#[tokio::test]
async fn in_open_mode_the_household_opens_and_edits_a_design() {
    let h = harness();
    let snap = open_star(&h.app, None).await;
    assert_eq!(snap["design"]["cut_ready"], true);
    assert_eq!(snap["design"]["name"], "star.svg");
    // The upload is the server's own copy, in the household's private area.
    let designs = h.app.service.people.join("household/designs");
    assert_eq!(stored_designs(&designs), 1);

    let edit = json!({"id": "sheet.set", "params": {"material": "Removable"}, "key": "k1",
        "incarnation": snap["incarnation"], "revision": snap["revision"]});
    let (status, reply) = send(
        &h.app,
        design(
            "POST",
            "/api/v1/design/command",
            Body::from(edit.to_string()),
            None,
        ),
    )
    .await;
    assert_eq!(
        (status, reply["answer"].as_str()),
        (StatusCode::OK, Some("applied")),
        "{reply}"
    );
    let (_, found) = send(
        &h.app,
        design("GET", "/api/v1/design/lookup/k1", Body::empty(), None),
    )
    .await;
    assert_eq!(found["answer"], "applied");
}

#[tokio::test]
async fn a_design_request_without_this_protocol_version_is_refused_first() {
    let h = harness();
    let mut request = upload_star(None);
    request.headers_mut().remove("x-cutcraft-protocol");
    let (status, body) = send(&h.app, request).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (StatusCode::CONFLICT, Some("protocol-version"))
    );
    // Nothing was stored.
    assert!(!h.app.service.people.join("household").exists());
}

#[tokio::test]
async fn two_people_have_separate_sessions_and_private_areas() {
    let h = harness();
    let users = &h.app.service.users;
    let sam = users.create_first("Sam", "correct horse").unwrap();
    users
        .create("Alex", "first password", crate::users::Role::User)
        .unwrap();
    users
        .set_password("Alex", "alex's own pass", false, None)
        .unwrap();
    let alex = users.login("Alex", "alex's own pass", "127.0.0.1").unwrap();
    let ids: std::collections::HashMap<String, String> = users
        .list()
        .unwrap()
        .into_iter()
        .map(|u| (u.name, u.id))
        .collect();

    let sams = open_star(&h.app, Some(&sam)).await;
    let (_, alexs) = send(
        &h.app,
        design("GET", "/api/v1/design/snapshot", Body::empty(), Some(&alex)),
    )
    .await;
    assert!(
        alexs["design"].is_null(),
        "Alex's session is Alex's own: {alexs}"
    );

    // Each person's upload is in their own area, by their account's id.
    let people = &h.app.service.people;
    assert_eq!(stored_designs(&people.join(&ids["Sam"]).join("designs")), 1);
    assert_eq!(
        stored_designs(&people.join(&ids["Alex"]).join("designs")),
        0
    );

    // Alex can't fetch Sam's artwork: it isn't the design open in Alex's session.
    let art = sams["design"]["art"]["id"].as_str().unwrap();
    let uri = format!(
        "/api/v1/design/picture?art={art}&x0=0&y0=0&x1=80&y1=60&zoom_step=0&width=100&height=75"
    );
    let (status, _) = send(&h.app, design("GET", &uri, Body::empty(), Some(&alex))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(&h.app, design("GET", &uri, Body::empty(), Some(&sam))).await;
    assert_eq!(status, StatusCode::OK);

    // Signed out, nothing.
    let (status, _) = send(
        &h.app,
        design("GET", "/api/v1/design/snapshot", Body::empty(), None),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn uploads_are_listed_and_reopened_by_owned_id_only_by_their_owner() {
    let h = harness();
    let users = &h.app.service.users;
    let sam = users.create_first("Sam", "correct horse").unwrap();
    users
        .create("Alex", "first password", crate::users::Role::User)
        .unwrap();
    users
        .set_password("Alex", "alex's own pass", false, None)
        .unwrap();
    let alex = users.login("Alex", "alex's own pass", "127.0.0.1").unwrap();

    send(&h.app, upload_star(Some(&sam))).await;
    let (_, listing) = send(
        &h.app,
        design("GET", "/api/v1/design/designs", Body::empty(), Some(&sam)),
    )
    .await;
    let ids: Vec<&str> = listing["designs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 1);
    assert!(
        !listing
            .to_string()
            .contains(h.app.service.people.to_string_lossy().as_ref()),
        "no paths: {listing}"
    );
    assert!(
        listing["designs"][0].get("ext").is_none(),
        "nothing about storage: {listing}"
    );
    let s = now(&h.app, Some(&sam)).await;
    let open = json!({"design": ids[0], "key": "o1", "incarnation": s["incarnation"], "revision": s["revision"]});
    let reply = post_design(&h.app, "/api/v1/design/open", open, Some(&sam)).await;
    assert_eq!(reply["answer"], "applied", "{reply}");
    assert_eq!(now(&h.app, Some(&sam)).await["design"]["name"], "star.svg");
    // Alex can't open Sam's upload, even knowing its id: it isn't in Alex's area.
    let a = now(&h.app, Some(&alex)).await;
    let open = json!({"design": ids[0], "key": "o2", "incarnation": a["incarnation"], "revision": a["revision"]});
    let (status, _) = send(
        &h.app,
        design(
            "POST",
            "/api/v1/design/open",
            Body::from(open.to_string()),
            Some(&alex),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, alexs) = send(
        &h.app,
        design("GET", "/api/v1/design/designs", Body::empty(), Some(&alex)),
    )
    .await;
    assert_eq!(alexs["designs"].as_array().unwrap().len(), 0);
}

/// Spec §11: Save As puts the project in the library, Save adds revisions, and a project
/// someone else saved since is refused so the person can Reload or Save As.
#[tokio::test]
async fn saving_publishes_revisions_and_refuses_a_save_over_someone_elses() {
    let h = harness();
    let users = &h.app.service.users;
    let sam = users.create_first("Sam", "correct horse").unwrap();
    users
        .create("Alex", "first password", crate::users::Role::User)
        .unwrap();
    users
        .set_password("Alex", "alex's own pass", false, None)
        .unwrap();
    let alex = users.login("Alex", "alex's own pass", "127.0.0.1").unwrap();
    let (sam, alex) = (Some(sam.as_str()), Some(alex.as_str()));
    let save = |key: &str, s: &Value, save_as: Option<Value>| {
        let mut body =
            json!({"key": key, "incarnation": s["incarnation"], "revision": s["revision"]});
        if let Some(as_) = save_as {
            body["as"] = as_;
        }
        body
    };

    open_star(&h.app, sam).await;
    let s = now(&h.app, sam).await;
    let reply = post_design(&h.app, "/api/v1/design/save", save("s0", &s, None), sam).await;
    assert_eq!(reply["code"], "not-saved-yet", "{reply}");
    let as_star = Some(json!({"folder": [], "name": "Star"}));
    let reply = post_design(&h.app, "/api/v1/design/save", save("s1", &s, as_star), sam).await;
    assert_eq!(reply["answer"], "applied", "{reply}");
    let saved = now(&h.app, sam).await["design"]["saved"].clone();
    assert_eq!(saved["revision"], 1);
    let job = saved["job"].as_str().unwrap().to_owned();

    // Sam edits and saves: revision 2, nothing unsaved.
    set_material(&h.app, "e1", "Removable", sam).await;
    assert_eq!(now(&h.app, sam).await["design"]["unsaved"], true);
    let s = now(&h.app, sam).await;
    assert_eq!(
        post_design(&h.app, "/api/v1/design/save", save("s2", &s, None), sam).await["answer"],
        "applied"
    );
    let d = now(&h.app, sam).await["design"].clone();
    assert_eq!(
        (d["saved"]["revision"].as_u64(), d["unsaved"].as_bool()),
        (Some(2), Some(false))
    );

    // Alex opens the library project, edits and saves: revision 3.
    let a = now(&h.app, alex).await;
    let open = json!({"project": {"job": job}, "key": "ao", "incarnation": a["incarnation"], "revision": a["revision"]});
    assert_eq!(
        post_design(&h.app, "/api/v1/design/open", open, alex).await["answer"],
        "applied"
    );
    assert_eq!(now(&h.app, alex).await["design"]["saved"]["revision"], 2);
    set_material(&h.app, "ae", "Permanent", alex).await;
    let a = now(&h.app, alex).await;
    assert_eq!(
        post_design(&h.app, "/api/v1/design/save", save("as1", &a, None), alex).await["answer"],
        "applied"
    );

    // Sam, still on revision 2, edits and saves: refused, nothing overwritten.
    set_material(&h.app, "e2", "Siser EasyWeed", sam).await;
    let s = now(&h.app, sam).await;
    let reply = post_design(&h.app, "/api/v1/design/save", save("s3", &s, None), sam).await;
    assert_eq!(reply["code"], "saved-elsewhere", "{reply}");
    assert_eq!(
        h.app
            .service
            .library
            .details(&job)
            .unwrap()
            .current_revision,
        3
    );
    // Save As keeps Sam's work as a new project.
    let as_sams = Some(json!({"folder": [], "name": "Star, Sam's"}));
    let reply = post_design(&h.app, "/api/v1/design/save", save("s4", &s, as_sams), sam).await;
    assert_eq!(reply["answer"], "applied", "{reply}");
    assert_ne!(now(&h.app, sam).await["design"]["saved"]["job"], json!(job));
}

/// Spec §7, over HTTP: unsaved changes are refused, then kept when the person opens anyway.
#[tokio::test]
async fn opening_over_unsaved_work_is_refused_until_the_person_chooses() {
    let h = harness();
    open_star(&h.app, None).await;
    set_material(&h.app, "e1", "Removable", None).await;
    let (_, uploaded) = send(&h.app, upload_star(None)).await;
    let s = now(&h.app, None).await;
    let body = json!({"design": uploaded["design"]["id"], "key": "o2", "incarnation": s["incarnation"], "revision": s["revision"]});
    assert_eq!(
        post_design(&h.app, "/api/v1/design/open", body, None).await["code"],
        "unsaved-changes"
    );
    let body = json!({"design": uploaded["design"]["id"], "key": "o3", "incarnation": s["incarnation"],
        "revision": s["revision"], "replace_unsaved": true});
    assert_eq!(
        post_design(&h.app, "/api/v1/design/open", body, None).await["answer"],
        "applied"
    );
    assert_eq!(now(&h.app, None).await["design"]["unsaved"], false);
}

/// Spec §8: the material list is the household's. A material Sam saves shows up in Alex's open
/// session, pushed to Alex's pages.
#[tokio::test]
async fn a_material_saved_by_one_person_reaches_everyone_elses_session() {
    let h = harness();
    let users = &h.app.service.users;
    let sam = users.create_first("Sam", "correct horse").unwrap();
    users
        .create("Alex", "first password", crate::users::Role::User)
        .unwrap();
    users
        .set_password("Alex", "alex's own pass", false, None)
        .unwrap();
    let alex = users.login("Alex", "alex's own pass", "127.0.0.1").unwrap();
    let (sam, alex) = (Some(sam.as_str()), Some(alex.as_str()));
    open_star(&h.app, sam).await;
    open_star(&h.app, alex).await;
    let s = now(&h.app, sam).await;
    let reply = post_design(
        &h.app,
        "/api/v1/design/command",
        json!({"id": "materials.save_current",
        "params": {"name": "Household glitter"}, "key": "m1", "incarnation": s["incarnation"],
        "revision": s["revision"]}),
        sam,
    )
    .await;
    assert_eq!(reply["answer"], "applied", "{reply}");
    let alexs = now(&h.app, alex).await;
    assert!(
        alexs["materials"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m == "Household glitter"),
        "{alexs}"
    );
    assert!(h.app.service.lists.join("materials.json").is_file());
    let s = now(&h.app, sam).await;
    let a = now(&h.app, alex).await;
    let sam_edit = json!({"id":"materials.save_current", "params":{"name":"Concurrent A"}, "key":"m2", "incarnation":s["incarnation"], "revision":s["revision"]});
    let alex_edit = json!({"id":"materials.save_current", "params":{"name":"Concurrent B"}, "key":"m3", "incarnation":a["incarnation"], "revision":a["revision"]});
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(
            post_design(&h.app, "/api/v1/design/command", sam_edit, sam),
            post_design(&h.app, "/api/v1/design/command", alex_edit, alex)
        )
    })
    .await
    .expect("cross-session reloads must not deadlock");
    assert!(result.0["answer"] == "applied" || result.1["answer"] == "applied");
}

/// A lost library Open reply can be retried with the identical browser request.
#[tokio::test]
async fn library_open_and_save_replay_the_original_request_after_later_edits() {
    let h = harness();
    open_star(&h.app, None).await;
    let s = now(&h.app, None).await;
    let save = json!({"key":"save-retry", "incarnation":s["incarnation"], "revision":s["revision"], "as":{"folder":[], "name":"Replay star"}});
    let first = post_design(&h.app, "/api/v1/design/save", save.clone(), None).await;
    assert_eq!(first["answer"], "applied", "{first}");
    let saved = now(&h.app, None).await["design"]["saved"].clone();
    set_material(&h.app, "later-edit", "Removable", None).await;
    assert_eq!(
        post_design(&h.app, "/api/v1/design/save", save.clone(), None).await,
        first
    );
    assert_eq!(now(&h.app, None).await["design"]["unsaved"], true);
    let mut changed = save;
    changed["as"]["name"] = json!("Different");
    assert_eq!(
        post_design(&h.app, "/api/v1/design/save", changed, None).await["code"],
        "key-reused"
    );
    let s = now(&h.app, None).await;
    let open = json!({"project":{"job":saved["job"]}, "key":"library-retry", "incarnation":s["incarnation"], "revision":s["revision"], "replace_unsaved":true});
    let first = post_design(&h.app, "/api/v1/design/open", open.clone(), None).await;
    assert_eq!(first["answer"], "applied", "{first}");
    set_material(&h.app, "another-edit", "Permanent", None).await;
    assert_eq!(
        post_design(&h.app, "/api/v1/design/open", open.clone(), None).await,
        first
    );
    assert_eq!(now(&h.app, None).await["design"]["unsaved"], true);
    let mut changed = open;
    changed["replace_unsaved"] = json!(false);
    assert_eq!(
        post_design(&h.app, "/api/v1/design/open", changed, None).await["code"],
        "key-reused"
    );
}

#[tokio::test]
async fn a_published_save_survives_losing_the_session_acknowledgement() {
    let mut h = harness();
    open_star(&h.app, None).await;
    let draft = h.app.service.people.join("household/drafts/session.json");
    let before_save = std::fs::read(&draft).unwrap();
    let s = now(&h.app, None).await;
    let body = json!({"key":"durable-save", "incarnation":s["incarnation"], "revision":s["revision"], "as":{"name":"Durable star"}});
    let first = post_design(&h.app, "/api/v1/design/save", body.clone(), None).await;
    assert_eq!(first["answer"], "applied", "{first}");
    let job = now(&h.app, None).await["design"]["saved"]["job"]
        .as_str()
        .unwrap()
        .to_owned();
    // Reproduce a crash after publishing but before checkpointing the session's Save result.
    std::fs::write(&draft, before_save).unwrap();
    h.app.design = std::sync::Arc::new(crate::design::Sessions::new(
        crate::design::Mode::Thread,
        h.app.service.people.clone(),
        h.app.service.lists.clone(),
    ));
    assert_ne!(now(&h.app, None).await["incarnation"], s["incarnation"]);
    let (_, lookup) = send(
        &h.app,
        design(
            "GET",
            "/api/v1/design/lookup/durable-save",
            Body::empty(),
            None,
        ),
    )
    .await;
    assert_eq!(lookup, first);

    assert_eq!(
        post_design(&h.app, "/api/v1/design/save", body.clone(), None).await,
        first
    );
    assert_eq!(
        h.app
            .service
            .library
            .details(&job)
            .unwrap()
            .current_revision,
        1
    );
    let receipt_key = "design-save:household:durable-save";
    assert!(
        h.app
            .service
            .library
            .receipt(receipt_key)
            .unwrap()
            .is_some()
    );
    let mut changed = body;
    changed["as"]["name"] = json!("Another star");
    assert_eq!(
        post_design(&h.app, "/api/v1/design/save", changed, None).await["code"],
        "key-reused"
    );
}

#[tokio::test]
async fn simultaneous_saves_of_one_library_base_publish_only_one_revision() {
    let h = harness();
    let users = &h.app.service.users;
    let sam = users.create_first("Sam", "correct horse").unwrap();
    users
        .create("Alex", "first password", crate::users::Role::User)
        .unwrap();
    users
        .set_password("Alex", "alex's own pass", false, None)
        .unwrap();
    let alex = users.login("Alex", "alex's own pass", "127.0.0.1").unwrap();
    open_star(&h.app, Some(&sam)).await;
    let s = now(&h.app, Some(&sam)).await;
    let save = json!({"key":"seed", "incarnation":s["incarnation"], "revision":s["revision"], "as":{"name":"Shared star"}});
    assert_eq!(
        post_design(&h.app, "/api/v1/design/save", save, Some(&sam)).await["answer"],
        "applied"
    );
    let job = now(&h.app, Some(&sam)).await["design"]["saved"]["job"]
        .as_str()
        .unwrap()
        .to_owned();
    let a = now(&h.app, Some(&alex)).await;
    let open = json!({"project":{"job":job}, "key":"open", "incarnation":a["incarnation"], "revision":a["revision"]});
    assert_eq!(
        post_design(&h.app, "/api/v1/design/open", open, Some(&alex)).await["answer"],
        "applied"
    );
    set_material(&h.app, "sam-edit", "Removable", Some(&sam)).await;
    set_material(&h.app, "alex-edit", "Permanent", Some(&alex)).await;
    let s = now(&h.app, Some(&sam)).await;
    let a = now(&h.app, Some(&alex)).await;
    // The same client key is valid for different people.
    let sb = json!({"key":"save", "incarnation":s["incarnation"], "revision":s["revision"]});
    let ab = json!({"key":"save", "incarnation":a["incarnation"], "revision":a["revision"]});
    let (s, a) = tokio::join!(
        post_design(&h.app, "/api/v1/design/save", sb, Some(&sam)),
        post_design(&h.app, "/api/v1/design/save", ab, Some(&alex))
    );
    assert!(
        s["answer"] == "applied" && a["code"] == "saved-elsewhere"
            || a["answer"] == "applied" && s["code"] == "saved-elsewhere",
        "{s} / {a}"
    );
    assert_eq!(
        h.app
            .service
            .library
            .details(&job)
            .unwrap()
            .current_revision,
        2
    );
}

#[tokio::test]
async fn first_admin_inherits_the_latest_household_draft() {
    let h = harness();
    open_star(&h.app, None).await;
    set_material(&h.app, "before-adoption", "Removable", None).await;
    let response = send_raw(
        &h.app,
        page(
            "POST",
            "/api/v1/users/first",
            json!({"name":"Sam", "password":"correct horse"}),
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let session = session_from(&response);
    let inherited = now(&h.app, Some(&session)).await;
    assert!(
        inherited["design"]["sheet"]["material"]
            .as_str()
            .unwrap()
            .starts_with("Removable")
    );
    assert_eq!(inherited["design"]["unsaved"], true);
    assert!(!h.app.service.people.join("household").exists());
}

#[tokio::test]
async fn a_revoked_persons_design_stream_ends_before_the_next_event() {
    let h = harness();
    h.app
        .service
        .users
        .create_first("Sam", "correct horse")
        .unwrap();
    let users = &h.app.service.users;
    users
        .create("Alex", "first password", crate::users::Role::User)
        .unwrap();
    users
        .set_password("Alex", "alex's own pass", false, None)
        .unwrap();
    let alex = users.login("Alex", "alex's own pass", "127.0.0.1").unwrap();
    let mut response = send_raw(
        &h.app,
        design("GET", "/api/v1/design/live", Body::empty(), Some(&alex)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.body_mut().frame().await.unwrap().is_ok());
    users.remove("Alex").unwrap();
    let next = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        response.body_mut().frame(),
    )
    .await;
    assert!(
        next.unwrap().is_none(),
        "revoked streams end rather than waiting for another private snapshot"
    );
}

#[tokio::test]
async fn a_slow_design_stream_coalesces_to_the_latest_snapshot() {
    let h = harness();
    open_star(&h.app, None).await;
    set_material(&h.app, "before-subscribe", "Permanent", None).await;
    let mut response = send_raw(
        &h.app,
        design("GET", "/api/v1/design/live", Body::empty(), None),
    )
    .await;
    let initial = response
        .body_mut()
        .frame()
        .await
        .unwrap()
        .unwrap()
        .into_data()
        .unwrap();
    let initial = String::from_utf8(initial.to_vec()).unwrap();
    let initial: Value = serde_json::from_str(
        initial
            .trim()
            .strip_prefix("event: snapshot\ndata: ")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(initial["revision"], now(&h.app, None).await["revision"]);
    set_material(&h.app, "stream-edit-1", "Removable", None).await;
    set_material(&h.app, "stream-edit-2", "Permanent", None).await;
    set_material(&h.app, "stream-edit-3", "Siser EasyWeed", None).await;
    let latest = now(&h.app, None).await;
    let next = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        response.body_mut().frame(),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap()
    .into_data()
    .unwrap();
    let next = String::from_utf8(next.to_vec()).unwrap();
    let next: Value =
        serde_json::from_str(next.trim().strip_prefix("event: snapshot\ndata: ").unwrap()).unwrap();
    assert_eq!(
        next["revision"], latest["revision"],
        "skip older buffered snapshots"
    );
    assert_eq!(
        next["design"]["sheet"]["material"],
        latest["design"]["sheet"]["material"]
    );
}

/// Drain closes admission, not observation or Stop. The same service backs HTTP and MCP.
#[tokio::test]
async fn update_drain_keeps_start_replay_and_stop_available() {
    let mut h = harness();
    let (_, job) = upload(&h.app, "drain-upload").await;
    let id = job["id"].as_str().unwrap();
    for _ in 0..3 {
        h.worker.step();
    }
    h.sim.unload();
    for _ in 0..3 {
        h.worker.step();
    }
    h.sim.load(Media::Mat12x12);
    for _ in 0..9 {
        h.worker.step();
    }
    let (_, queued) = send(
        &h.app,
        page(
            "POST",
            "/api/v1/cutters/practice/queue",
            json!({"job_id":id, "key":"drain-queue"}),
        ),
    )
    .await;
    let view = lock(&h.app.service.cutters[0].shared).view();
    let cameo_pi_core::mat::Mat::Fresh(generation) = view.mat else {
        panic!("not fresh");
    };
    let start = json!({"boot":"t", "kind":"main", "turn":queued["turn"], "job_id":id, "revision":1,
        "generation":generation, "action_epoch":view.action_epoch, "key":"drain-start"});
    let (status, accepted) = send(
        &h.app,
        page("POST", "/api/v1/cutters/practice/start", start.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    h.app.service.updates.begin();
    let (status, waiting_turn) = send(
        &h.app,
        page(
            "POST",
            "/api/v1/cutters/practice/queue",
            json!({"job_id":id, "key":"waiting-turn"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{waiting_turn}");

    assert!(
        !h.app.service.update_ready(),
        "accepted execution is already active before the driver steps"
    );
    let (status, replay) = send(
        &h.app,
        page("POST", "/api/v1/cutters/practice/start", start.clone()),
    )
    .await;
    assert_eq!((status, replay), (StatusCode::OK, accepted));
    let mut new_start = start;
    new_start["key"] = json!("another-start");
    let (status, refused) = send(
        &h.app,
        page("POST", "/api/v1/cutters/practice/start", new_start.clone()),
    )
    .await;
    assert_eq!(
        (status, refused["error"].as_str()),
        (StatusCode::CONFLICT, Some("update-waiting")),
        "{refused}"
    );
    let (status, stopped) = send(
        &h.app,
        page("POST", "/api/v1/cutters/practice/stop", json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    for _ in 0..20 {
        h.worker.step();
    }
    assert!(
        h.app.service.update_ready(),
        "the cancelled execution must settle before restart"
    );

    let waiting = lock(&h.app.service.cutters[0].shared).view();
    assert!(waiting.execution.is_none());
    assert_eq!(
        waiting.turn.unwrap().id,
        waiting_turn["turn"].as_str().unwrap()
    );
    let (_, state) = send(&h.app, get("/api/v1/state", "cameo-pi.local")).await;
    assert_eq!(state["update"], json!({"waiting":true, "ready":true}));
    let (status, refused) = send(
        &h.app,
        page("POST", "/api/v1/cutters/practice/start", new_start),
    )
    .await;
    assert_eq!(
        (status, refused["error"].as_str()),
        (StatusCode::CONFLICT, Some("update-waiting")),
        "drain stays closed: {refused}"
    );
}

async fn cut_copy(app: &App, key: &str) -> Value {
    let s = now(app, None).await;
    post_design(
        app,
        "/api/v1/design/command",
        json!({"id":"cut.copy", "params":{},
        "key":key, "incarnation":s["incarnation"], "revision":s["revision"]}),
        None,
    )
    .await
}

#[tokio::test]
async fn a_cut_copy_survives_edits_library_revisions_and_closing_the_design() {
    let mut h = harness();
    open_star(&h.app, None).await;
    let s = now(&h.app, None).await;
    let saved = post_design(
        &h.app,
        "/api/v1/design/save",
        json!({"key":"copy-source-save", "incarnation":s["incarnation"],
        "revision":s["revision"], "as":{"name":"Copy source"}}),
        None,
    )
    .await;
    assert_eq!(saved["answer"], "applied", "{saved}");
    let first = cut_copy(&h.app, "first-copy").await;
    assert_eq!(first["answer"], "applied", "{first}");
    let copy = &first["value"]["copy"];
    let id = copy["id"].as_str().unwrap();
    let original = h.app.service.stored_plan(id, 1).unwrap();
    assert!(
        h.app.service.cutters_view()[0]["view"]["turn"].is_null(),
        "preparation does not queue or move"
    );
    let (_, queued) = send(
        &h.app,
        page(
            "POST",
            "/api/v1/cutters/practice/queue",
            json!({"job_id":id, "revision":1, "key":"copy-queue"}),
        ),
    )
    .await;
    let s = now(&h.app, None).await;
    assert_eq!(
        post_design(
            &h.app,
            "/api/v1/design/command",
            json!({"id":"sheet.adjust", "params":{"force":22},
        "key":"copy-edit", "incarnation":s["incarnation"], "revision":s["revision"]}),
            None
        )
        .await["answer"],
        "applied"
    );
    let s = now(&h.app, None).await;
    assert_eq!(
        post_design(
            &h.app,
            "/api/v1/design/save",
            json!({"key":"copy-save-again", "incarnation":s["incarnation"],
        "revision":s["revision"]}),
            None
        )
        .await["answer"],
        "applied"
    );
    let second = cut_copy(&h.app, "second-copy").await;
    assert_ne!(second["value"]["copy"]["id"], copy["id"]);
    assert_ne!(second["value"]["copy"]["digest"], copy["digest"]);
    assert_eq!(
        h.app.service.stored_plan(id, 1).unwrap().material.force,
        original.material.force
    );
    let turn = lock(&h.app.service.cutters[0].shared).view().turn.unwrap();
    assert_eq!(turn.job.job_id, id);
    assert_eq!(turn.job.revision, 1);
    let loading = h.app.service.job(id, Some(1)).unwrap();
    assert_eq!(loading["immutable"], true);
    assert_eq!(loading["revision"]["digest"], copy["digest"]);
    assert!(h.app.service.download(id, 1, "preview").is_ok());
    assert!(h.app.service.download(id, 1, "project").is_err());
    assert!(h.app.service.download(id, 1, "design").is_err());
    assert!(h.app.service.copies.get(id, 2).is_err());
    let s = now(&h.app, None).await;
    assert_eq!(
        post_design(
            &h.app,
            "/api/v1/design/command",
            json!({"id":"design.close", "params":{},
        "key":"close-copy-source", "incarnation":s["incarnation"], "revision":s["revision"]}),
            None
        )
        .await["answer"],
        "applied"
    );
    for _ in 0..3 {
        h.worker.step();
    }
    h.sim.unload();
    for _ in 0..3 {
        h.worker.step();
    }
    h.sim.load(Media::Mat12x12);
    for _ in 0..9 {
        h.worker.step();
    }
    let view = lock(&h.app.service.cutters[0].shared).view();
    let cameo_pi_core::mat::Mat::Fresh(generation) = view.mat else {
        panic!("not fresh");
    };
    let (status, started) = send(&h.app, page("POST", "/api/v1/cutters/practice/start", json!({"boot":"t", "kind":"main", "turn":queued["turn"],
        "job_id":id, "revision":1, "generation":generation, "action_epoch":view.action_epoch, "key":"copy-start"}))).await;
    assert_eq!(status, StatusCode::OK, "{started}");
    h.worker.step();
    let records: Vec<cameo_pi_core::journal::Record> =
        std::fs::read_dir(h._dir.path().join("private/executions"))
            .unwrap()
            .map(|entry| {
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
            })
            .collect();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].job_id, id);
    assert_eq!(
        records[0].digest,
        copy["digest"].as_str().unwrap(),
        "the actual driver runs the original copy, not the edited project"
    );
}

#[tokio::test]
async fn http_and_mcp_replay_the_same_copy_after_losing_the_worker_checkpoint() {
    let mut h = harness();
    open_star(&h.app, None).await;
    let s = now(&h.app, None).await;
    let draft = h.app.service.people.join("household/drafts/session.json");
    let before_copy = std::fs::read(&draft).unwrap();
    let args =
        json!({"key":"shared-copy", "incarnation":s["incarnation"], "revision":s["revision"]});
    let request = json!({"id":"cut.copy", "params":{}, "key":"shared-copy", "incarnation":s["incarnation"], "revision":s["revision"]});
    let first = post_design(&h.app, "/api/v1/design/command", request.clone(), None).await;
    let caller = crate::http::Caller {
        actor: cameo_pi_core::cutter::Actor {
            user: None,
            client: "copy-test".into(),
        },
        identity: crate::users::Identity::Open,
        session: None,
        key: None,
    };
    let tools = crate::mcp::Tools::new(h.app.clone());
    assert_eq!(
        tools
            .invoke("design_cut_copy", args.clone(), caller.clone())
            .await
            .unwrap(),
        first
    );
    std::fs::write(&draft, before_copy).unwrap();
    h.app.design = Arc::new(crate::design::Sessions::new(
        crate::design::Mode::Thread,
        h.app.service.people.clone(),
        h.app.service.lists.clone(),
    ));
    assert_ne!(now(&h.app, None).await["incarnation"], s["incarnation"]);
    assert_eq!(
        post_design(&h.app, "/api/v1/design/command", request.clone(), None).await,
        first
    );
    let (_, found) = send(
        &h.app,
        design(
            "GET",
            "/api/v1/design/lookup/shared-copy",
            Body::empty(),
            None,
        ),
    )
    .await;
    assert_eq!(found, first);
    let tools = crate::mcp::Tools::new(h.app.clone());
    assert_eq!(
        tools
            .invoke("design_cut_copy", args.clone(), caller.clone())
            .await
            .unwrap(),
        first
    );
    let mut changed = request;
    changed["revision"] = json!(999);
    assert_eq!(
        post_design(&h.app, "/api/v1/design/command", changed, None).await["code"],
        "key-reused"
    );
    let current = now(&h.app, None).await;
    let edit = json!({"id":"sheet.adjust", "params":{"force":22}, "key":"shared-copy", "incarnation":current["incarnation"], "revision":current["revision"]});
    assert_eq!(
        post_design(&h.app, "/api/v1/design/command", edit, None).await["code"],
        "key-reused"
    );
    let status = tools
        .invoke("design_status", json!({}), caller)
        .await
        .unwrap();
    assert_eq!(status["revision"], current["revision"]);
}

#[tokio::test]
async fn an_absent_or_stale_design_never_publishes_a_copy() {
    let h = harness();
    let empty = cut_copy(&h.app, "empty-copy").await;
    assert_eq!(empty["code"], "not-cut-ready", "{empty}");
    let s = open_star(&h.app, None).await;
    set_material(&h.app, "copy-stale-edit", "Removable", None).await;
    let stale = post_design(
        &h.app,
        "/api/v1/design/command",
        json!({"id":"cut.copy", "params":{},
        "key":"stale-copy", "incarnation":s["incarnation"], "revision":s["revision"]}),
        None,
    )
    .await;
    assert_eq!(stale["code"], "stale-revision", "{stale}");
    assert_eq!(
        std::fs::read_dir(h._dir.path().join("private/cut-copies"))
            .unwrap()
            .count(),
        0
    );
}

async fn ready_start(h: &mut Harness, key: &str) -> (String, Value) {
    let (_, job) = upload(&h.app, &format!("upload-{key}")).await;
    let id = job["id"].as_str().unwrap().to_owned();
    for _ in 0..3 {
        h.worker.step();
    }
    h.sim.unload();
    for _ in 0..3 {
        h.worker.step();
    }
    h.sim.load(Media::Mat12x12);
    for _ in 0..9 {
        h.worker.step();
    }
    let view = lock(&h.app.service.cutters[0].shared).view();
    let cameo_pi_core::mat::Mat::Fresh(generation) = view.mat else {
        panic!("not fresh");
    };
    (
        id.clone(),
        json!({"boot":"t", "kind":"main", "turn":null, "job_id":id, "revision":1,
        "generation":generation, "action_epoch":view.action_epoch, "key":key}),
    )
}

#[tokio::test]
async fn an_acknowledged_start_survives_restart_before_the_driver_takes_it() {
    let mut h = harness();
    let (id, body) = ready_start(&mut h, "durable-start").await;
    let (status, first) = send(
        &h.app,
        page("POST", "/api/v1/cutters/practice/start", body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let dir = h._dir;
    drop(h.worker);
    drop(h.app);
    let shared = Arc::new(Mutex::new(Cutter::new(
        "practice",
        "Practice Cameo",
        "after",
    )));
    let service = Service::new(
        "after".into(),
        dir.path(),
        vec![CutterEntry {
            id: "practice".into(),
            shared: shared.clone(),
            simulator: None,
        }],
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(service.start_result("practice", "durable-start").unwrap()).unwrap(),
        first
    );
    assert!(
        lock(&shared).take_pending().is_none(),
        "restart restores the answer, never the motion"
    );
    let records: Vec<cameo_pi_core::journal::Record> =
        std::fs::read_dir(dir.path().join("private/executions"))
            .unwrap()
            .map(|entry| {
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
            })
            .collect();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].outcome.as_ref().unwrap()["ended"], "interrupted");
    // Ordinary library/history removal cannot delete the cutter authority's accepted result.
    service.delete_job(&id).unwrap();
    let actor = cameo_pi_core::cutter::Actor {
        user: None,
        client: "reconnected".into(),
    };
    let replay = service
        .start(
            "practice",
            serde_json::from_value(body.clone()).unwrap(),
            actor.clone(),
        )
        .unwrap();
    assert_eq!(serde_json::to_value(replay).unwrap(), first);
    let mut changed = body;
    changed["job_id"] = json!("different");
    assert_eq!(
        service
            .start("practice", serde_json::from_value(changed).unwrap(), actor)
            .unwrap_err()
            .error,
        "key-reused"
    );
}

#[tokio::test]
async fn failed_start_recording_never_releases_a_job_to_the_driver() {
    let mut h = harness();
    let (_, body) = ready_start(&mut h, "failed-recording").await;
    let records = h._dir.path().join("private/executions");
    std::fs::remove_dir(&records).unwrap();
    std::fs::write(&records, b"not a directory").unwrap();
    let (status, reply) = send(&h.app, page("POST", "/api/v1/cutters/practice/start", body)).await;
    assert_eq!(
        (status, reply["error"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("recording-failed")),
        "{reply}"
    );
    assert!(
        lock(&h.app.service.cutters[0].shared)
            .take_pending()
            .is_none()
    );
    for _ in 0..3 {
        h.worker.step();
    }
    assert!(
        h.sim.with_machine(|m| m.trace().is_empty()),
        "no drawing after persistence failure"
    );
}

#[test]
fn stop_preempts_a_start_blocked_on_record_persistence() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut h = harness();
    let (_, body) = runtime.block_on(ready_start(&mut h, "blocked-recording"));
    let service = h.app.service.clone();
    let journal = service.journal.clone();
    let storage = journal.hold_writes_for_fixture();
    let (sent, received) = std::sync::mpsc::channel();
    let starter = {
        let service = service.clone();
        std::thread::spawn(move || {
            let result = service.start(
                "practice",
                serde_json::from_value(body).unwrap(),
                cameo_pi_core::cutter::Actor {
                    user: None,
                    client: "test".into(),
                },
            );
            sent.send(result).unwrap();
        })
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while lock(&service.cutters[0].shared).view().execution.is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "Start never reached staging"
        );
        std::thread::yield_now();
    }
    assert!(
        received.try_recv().is_err(),
        "no successful acknowledgement before sync"
    );
    h.worker.step();
    assert!(h.sim.with_machine(|m| m.trace().is_empty()));
    assert!(matches!(
        service.stop("practice").unwrap(),
        cameo_pi_core::cutter::Answer::Stopping { .. }
    ));
    drop(storage);
    assert!(
        received
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .is_ok()
    );
    starter.join().unwrap();
    h.worker.step();
    assert!(
        h.sim.with_machine(|m| m.trace().is_empty()),
        "Stop during persistence wins before any drawing"
    );
}
