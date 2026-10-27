//! The cameo-pi client: CutCraft's side of the network cut server's protocol (cameo-pi spec §9,
//! §10, §11 and §14).
//!
//! - **Pairing** ([`Pairing`]): CutCraft asks the Pi for access under a name, shows the code the
//!   Pi shows, and collects its key once someone approves it on the Pi's web interface.
//! - **Identity:** the Pi's certificate is self-signed. It's pinned at pairing, and afterwards
//!   only that certificate is accepted ([`Error::IdentityChanged`] otherwise, never a silent
//!   re-trust).
//! - **A paired Pi** ([`Station`]): its cutters' live state, the library, uploads, and the turn
//!   requests (queue, start, stop, pause, resume, release). Every changing request carries an
//!   idempotency key the caller chooses ([`new_key`]), so a retried request is answered, not done
//!   twice.
//!
//! - **Discovery** ([`discover`]): Pis advertise themselves over Bonjour.
//!
//! Requests are blocking, for CutCraft's worker threads; never call them from async code.

mod discover;
mod http;
mod tls;

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use cutcraft_device::driver::Progress;
pub use discover::{Found, discover};
pub use tls::Fingerprint;

/// How long an ordinary request may take.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(15);
/// An upload validates on the Pi (and may re-plan the sheet there) before it answers.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(180);

/// What went wrong, in words CutCraft can show.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The address isn't one CutCraft can use.
    Address(String),
    /// The Pi couldn't be reached.
    Unreachable(String),
    /// The Pi presented a different certificate from the one pinned at pairing.
    IdentityChanged {
        pinned: Fingerprint,
        seen: Fingerprint,
    },
    /// The key no longer works: it was removed, or its user's password changed.
    NotPaired(String),
    /// The Pi refused the request; nothing changed. `code` is the Pi's stable error code.
    Refused {
        status: u16,
        code: String,
        message: String,
    },
    /// A reply this version doesn't understand.
    Protocol(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Address(why) => write!(f, "{why}"),
            Self::Unreachable(why) => write!(f, "cameo-pi can't be reached ({why})."),
            Self::IdentityChanged { .. } => write!(
                f,
                "cameo-pi's identity changed: it isn't the Pi CutCraft was paired with, or it was rebuilt. Pair again."
            ),
            Self::NotPaired(why) => write!(
                f,
                "CutCraft isn't paired with this cameo-pi any more ({why}). Pair again."
            ),
            Self::Refused { message, .. } => write!(f, "{message}"),
            Self::Protocol(why) => write!(
                f,
                "cameo-pi answered in a way this version of CutCraft doesn't understand ({why}). Update CutCraft or cameo-pi."
            ),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// A fresh idempotency key (cameo-pi spec §9.1): one per request the user made, reused for its
/// retries.
pub fn new_key() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system's random source");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `https://HOST[:PORT]` from what the user typed: `cameo-pi.local`, `10.0.0.5`, `pi.lan:8443`, or
/// any of those with `https://` in front.
pub fn base_url(address: &str) -> Result<String> {
    let trimmed = address.trim().trim_end_matches('/');
    let host = trimmed.strip_prefix("https://").unwrap_or(trimmed);
    let bad = |why: &str| {
        Err(Error::Address(format!(
            "{address:?} isn't a cameo-pi address: {why}"
        )))
    };
    if host.is_empty() {
        return bad("it's empty");
    }
    if host.contains("://") {
        return bad("cameo-pi only speaks https");
    }
    if host
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '/' | '?' | '#' | '@'))
    {
        return bad("give just the Pi's name or address");
    }
    Ok(format!("https://{host}"))
}

/// The Pi's error body.
#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: String,
    #[serde(default)]
    message: String,
}

/// Sends a request and reads its JSON reply, turning failures into [`Error`]s.
fn send<T: DeserializeOwned>(http: &http::Http, request: http::Request<'_>) -> Result<T> {
    let (status, bytes) = http.send(request)?;
    if !(200..300).contains(&status) {
        let body: ErrorBody = serde_json::from_slice(&bytes).unwrap_or(ErrorBody {
            error: String::new(),
            message: String::new(),
        });
        let message = if body.message.is_empty() {
            format!("cameo-pi answered {status}.")
        } else {
            body.message
        };
        return Err(if status == 401 {
            Error::NotPaired(message)
        } else {
            Error::Refused {
                status,
                code: body.error,
                message,
            }
        });
    }
    serde_json::from_slice(&bytes).map_err(|e| Error::Protocol(e.to_string()))
}

// --- Pairing ---------------------------------------------------------------------------------

/// What CutCraft keeps for a paired Pi. The key is a secret: store it with the user's settings,
/// never in a project or a log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Credentials {
    /// As the user gave it (`cameo-pi.local`, an address).
    pub address: String,
    /// The certificate pinned at pairing.
    #[serde(with = "fingerprint_text")]
    pub fingerprint: Fingerprint,
    pub key: String,
}

mod fingerprint_text {
    use super::Fingerprint;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(f: &Fingerprint, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(f)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Fingerprint, D::Error> {
        let text = String::deserialize(d)?;
        Fingerprint::parse(&text).ok_or_else(|| serde::de::Error::custom("not a fingerprint"))
    }
}

/// A request for access, waiting for someone to approve it on the Pi (cameo-pi spec §14.2).
pub struct Pairing {
    address: String,
    /// The certificate this Pi presented; it's pinned from here on.
    pub fingerprint: Fingerprint,
    secret: String,
    /// What the Pi's web interface shows beside the request, so the user knows which to allow.
    pub code: String,
    http: http::Http,
}

/// Where a pairing request stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Collected {
    /// Nobody has answered yet.
    Waiting,
    /// Someone chose Deny.
    Denied,
    /// The request ran out (10 minutes) or was already collected.
    Expired,
    Paired(Credentials),
}

impl Pairing {
    /// Asks the Pi at `address` for access as `program` ("CutCraft on the kitchen MacBook").
    pub fn begin(address: &str, program: &str) -> Result<Self> {
        let base = base_url(address)?;
        let first = http::Http::new(&base, None)?;
        #[derive(Deserialize)]
        struct Reply {
            request_secret: String,
            code: String,
        }
        let reply: Reply = send(
            &first,
            http::Request::json("/api/v1/pair", &json!({ "name": program })),
        )?;
        let fingerprint = first
            .seen()
            .ok_or_else(|| Error::Protocol("no certificate was presented".into()))?;
        // The key is collected from the same Pi only.
        Ok(Self {
            address: address.trim().to_owned(),
            fingerprint,
            secret: reply.request_secret,
            code: reply.code,
            http: http::Http::new(&base, Some(fingerprint))?,
        })
    }

    /// Asks whether the request was answered. Call it every second or two.
    pub fn collect(&self) -> Result<Collected> {
        #[derive(Deserialize)]
        #[serde(rename_all = "kebab-case", tag = "state")]
        enum Reply {
            Waiting,
            Denied,
            Key { key: String },
        }
        let reply = send::<Reply>(
            &self.http,
            http::Request::json(
                "/api/v1/pair/collect",
                &json!({ "request_secret": self.secret }),
            ),
        );
        match reply {
            Ok(Reply::Waiting) => Ok(Collected::Waiting),
            Ok(Reply::Denied) => Ok(Collected::Denied),
            Ok(Reply::Key { key }) => Ok(Collected::Paired(Credentials {
                address: self.address.clone(),
                fingerprint: self.fingerprint,
                key,
            })),
            Err(Error::Refused { status: 404, .. }) => Ok(Collected::Expired),
            Err(e) => Err(e),
        }
    }
}

// --- A paired Pi -----------------------------------------------------------------------------

/// The Pi as a whole: its boot (restarts change it), version and cutters.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct State {
    pub boot: String,
    pub version: String,
    pub cutters: Vec<Cutter>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Cutter {
    /// Stable on this Pi; requests name the cutter by it.
    pub id: String,
    /// The practice cutter (a Simulated Cameo on the Pi).
    #[serde(default)]
    pub simulated: bool,
    pub view: CutterView,
}

/// One cutter as the Pi sees it (cameo-pi spec §8).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct CutterView {
    /// The name the user gave it.
    pub name: String,
    pub online: bool,
    pub mat: Mat,
    pub mat_loaded: bool,
    /// `idle`, `cutting`, `testing`, `paused`, `stopping`, … in the Pi's words.
    pub activity: String,
    pub execution: Option<String>,
    pub kind: Option<Kind>,
    /// The revision the running execution cuts.
    #[serde(default)]
    pub cutting: Option<JobRef>,
    pub turn: Option<Turn>,
    #[serde(default)]
    pub queue: Vec<Queued>,
    pub action_epoch: u64,
    pub progress: Option<Progress>,
}

/// What the Pi knows about the mat (cameo-pi spec §8.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "state", content = "generation")]
pub enum Mat {
    None,
    /// Loaded, and seen being loaded; no main cut on it yet. A cut needs this.
    Fresh(u64),
    Used(u64),
    /// Loaded, but its load wasn't seen (after a restart or reconnect).
    Unseen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Test,
    Main,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct JobRef {
    pub job_id: String,
    pub revision: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Actor {
    pub user: Option<String>,
    pub client: String,
}

/// The cutter's current turn: whose mat it is.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Turn {
    pub id: String,
    pub job: JobRef,
    pub actor: Actor,
    /// The mat generation the turn took with its first cut.
    pub mat: Option<u64>,
    /// Test cuts already on the turn's mat, `[x0, y0, x1, y1]` mm.
    #[serde(default)]
    pub taken: Vec<[f64; 4]>,
    #[serde(default)]
    pub tests: u32,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Queued {
    pub turn: String,
    pub job: JobRef,
    pub actor: Actor,
}

/// What an accepted turn request did.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "answer")]
pub enum Answer {
    Started {
        execution: String,
        turn: String,
    },
    /// `position` 0 is the cutter's current turn.
    Queued {
        turn: String,
        position: usize,
    },
    Stopping {
        execution: String,
    },
    Paused,
    Resumed,
    Released {
        turn: String,
    },
    Removed {
        turn: String,
    },
    MatConfirmed {
        generation: u64,
    },
    /// An answer newer than this client.
    #[serde(other)]
    Other,
}

/// A folder of the library.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Listing {
    pub folders: Vec<String>,
    pub jobs: Vec<JobSummary>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct JobSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub notes: String,
    pub current_revision: u32,
    #[serde(default)]
    pub times_cut: u32,
}

/// A job as the Pi stored it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub current_revision: u32,
    /// Every execution that ended, oldest first.
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
}

/// One ended execution in a job's history.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct HistoryEntry {
    pub execution: String,
    pub cutter: String,
    pub revision: u32,
    /// `test` or `main`.
    pub kind: String,
    /// `finished`, `stopped`, `interrupted` or `not-run`.
    pub ended: String,
}

/// A prepared sheet to store in the library (cameo-pi spec §10).
pub struct Upload<'a> {
    /// The folder, from the library's top (empty: the top).
    pub folder: &'a [String],
    pub name: &'a str,
    /// A new revision of this job instead of a new job.
    pub job_id: Option<&'a str>,
    /// The CutCraft build that planned it, `VERSION (COMMIT)`.
    pub planner: &'a str,
    /// The `.cutcraft` project file.
    pub project: Vec<u8>,
    /// Its `CutPlan`, as JSON.
    pub plan: Vec<u8>,
    pub key: &'a str,
}

/// A start request (cameo-pi spec §9.2): everything the user saw, so the Pi refuses it if any of
/// it changed since.
#[derive(Clone, Debug, Serialize)]
pub struct Start {
    pub boot: String,
    pub kind: Kind,
    pub turn: Option<String>,
    pub job_id: String,
    pub revision: u32,
    /// The fresh mat's generation.
    pub generation: u64,
    pub action_epoch: u64,
    pub key: String,
}

/// A paired Pi.
pub struct Station {
    credentials: Credentials,
    http: http::Http,
}

fn folder_path(folder: &[String]) -> String {
    folder.join("/")
}

impl Station {
    /// Ready to talk to the Pi. Nothing is sent yet.
    pub fn connect(credentials: Credentials) -> Result<Self> {
        let http = http::Http::new(
            &base_url(&credentials.address)?,
            Some(credentials.fingerprint),
        )?;
        Ok(Self { credentials, http })
    }

    pub fn credentials(&self) -> &Credentials {
        &self.credentials
    }

    fn call<'a, T: DeserializeOwned>(&'a self, mut request: http::Request<'a>) -> Result<T> {
        request.bearer = Some(&self.credentials.key);
        send(&self.http, request)
    }

    fn post<T: DeserializeOwned>(&self, path: &str, body: Value) -> Result<T> {
        self.call(http::Request::json(path, &body))
    }

    /// Every cutter's state, and the Pi's boot and version.
    pub fn state(&self) -> Result<State> {
        self.call(http::Request::get("/api/v1/state"))
    }

    /// One folder of the library.
    pub fn library(&self, folder: &[String]) -> Result<Listing> {
        self.call(http::Request::get(format!(
            "/api/v1/library?path={}",
            http::encode(&folder_path(folder))
        )))
    }

    pub fn create_folder(&self, parent: &[String], name: &str) -> Result<()> {
        let _: Value = self.post(
            "/api/v1/library/folders",
            json!({ "parent": folder_path(parent), "name": name }),
        )?;
        Ok(())
    }

    /// Stores a prepared sheet in the library. The Pi validates it fully first; a refusal
    /// stores nothing.
    pub fn upload(&self, upload: Upload<'_>) -> Result<Job> {
        let folder = folder_path(upload.folder);
        let mut parts: Vec<(&str, Option<&str>, &[u8])> = vec![
            ("folder", None, folder.as_bytes()),
            ("name", None, upload.name.as_bytes()),
            ("planner", None, upload.planner.as_bytes()),
            ("key", None, upload.key.as_bytes()),
            ("project", Some("project.cutcraft"), &upload.project),
            ("plan", Some("plan.json"), &upload.plan),
        ];
        if let Some(id) = upload.job_id {
            parts.push(("job_id", None, id.as_bytes()));
        }
        let (content_type, body) = http::multipart(&parts);
        self.call(http::Request {
            method: "POST",
            path: "/api/v1/jobs".into(),
            content_type: Some(content_type),
            body,
            bearer: None,
            timeout: UPLOAD_TIMEOUT,
        })
    }

    /// Asks for a turn on `cutter` for the job's `revision` (refused if the job has a newer one).
    pub fn queue(&self, cutter: &str, job_id: &str, revision: u32, key: &str) -> Result<Answer> {
        self.post(
            &format!("/api/v1/cutters/{cutter}/queue"),
            json!({ "job_id": job_id, "revision": revision, "key": key }),
        )
    }

    /// Starts the turn's test cut or main cut on its loaded mat.
    pub fn start(&self, cutter: &str, start: &Start) -> Result<Answer> {
        self.post(
            &format!("/api/v1/cutters/{cutter}/start"),
            serde_json::to_value(start).map_err(|e| Error::Protocol(e.to_string()))?,
        )
    }

    /// The Start accepted with `key` on `cutter`, if the Pi has one. `None` (the Pi's
    /// `start-not-found`) means not accepted *so far*: a Start still on its way can arrive later,
    /// unless a Stop since made it stale. It's never proof that nothing will cut. A Pi without the
    /// lookup answers some other refusal, an error here: it doesn't make a late Start stale after
    /// Stop either, so nothing about the Start can be concluded.
    pub fn started(&self, cutter: &str, key: &str) -> Result<Option<Answer>> {
        let request = http::Request::get(format!(
            "/api/v1/cutters/{}/starts/{}",
            http::encode(cutter),
            http::encode(key)
        ));
        match self.call(request) {
            Ok(answer) => Ok(Some(answer)),
            Err(Error::Refused {
                status: 404, code, ..
            }) if code == "start-not-found" => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Says the loaded mat has fresh material, though the Pi didn't see it loading (after a
    /// restart or reconnect, cameo-pi spec §8.2). Refused for a mat that was cut on.
    pub fn confirm_fresh(&self, cutter: &str) -> Result<Answer> {
        self.post(&format!("/api/v1/cutters/{cutter}/mat-fresh"), json!({}))
    }

    pub fn stop(&self, cutter: &str) -> Result<Answer> {
        self.post(&format!("/api/v1/cutters/{cutter}/stop"), json!({}))
    }

    pub fn pause(&self, cutter: &str) -> Result<Answer> {
        self.post(&format!("/api/v1/cutters/{cutter}/pause"), json!({}))
    }

    pub fn resume(&self, cutter: &str) -> Result<Answer> {
        self.post(&format!("/api/v1/cutters/{cutter}/resume"), json!({}))
    }

    /// Sends CutCraft's material and mat lists (cameo-pi spec §11): they replace the ones the Pi
    /// had, for Adjust on its web page. `from` says which CutCraft sent them.
    pub fn put_catalog(&self, materials: &Value, mats: &Value, from: &str) -> Result<()> {
        let mut request = http::Request::json(
            "/api/v1/catalog",
            &json!({ "materials": materials, "mats": mats, "from": from }),
        );
        request.method = "PUT";
        let _: Value = self.call(request)?;
        Ok(())
    }

    /// A job's details and history.
    pub fn job(&self, id: &str) -> Result<Job> {
        #[derive(Deserialize)]
        struct Reply {
            details: Job,
        }
        let reply: Reply = self.call(http::Request::get(format!(
            "/api/v1/jobs/{}",
            http::encode(id)
        )))?;
        Ok(reply.details)
    }

    /// One of a revision's files: `project` (the `.cutcraft` file), `design` or `cut-lines`.
    pub fn file(&self, job_id: &str, revision: u32, what: &str) -> Result<Vec<u8>> {
        let mut request = http::Request::get(format!(
            "/api/v1/jobs/{}/revisions/{revision}/{}",
            http::encode(job_id),
            http::encode(what)
        ));
        request.bearer = Some(&self.credentials.key);
        request.timeout = UPLOAD_TIMEOUT;
        let (status, bytes) = self.http.send(request)?;
        if (200..300).contains(&status) {
            return Ok(bytes);
        }
        // An error comes as JSON, like every other answer.
        let body: Option<ErrorBody> = serde_json::from_slice(&bytes).ok();
        let message = body
            .as_ref()
            .map(|b| b.message.clone())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| format!("cameo-pi answered {status}."));
        Err(if status == 401 {
            Error::NotPaired(message)
        } else {
            Error::Refused {
                status,
                code: body.map(|b| b.error).unwrap_or_default(),
                message,
            }
        })
    }

    /// Takes a turn that hasn't come yet out of the line.
    pub fn remove(&self, cutter: &str, turn: &str) -> Result<Answer> {
        self.post(
            &format!("/api/v1/cutters/{cutter}/remove"),
            json!({ "turn": turn }),
        )
    }

    /// Ends the turn, for the next in line.
    pub fn release(&self, cutter: &str, turn: &str) -> Result<Answer> {
        self.post(
            &format!("/api/v1/cutters/{cutter}/release"),
            json!({ "turn": turn }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_become_https_urls() {
        assert_eq!(
            base_url("cameo-pi.local").unwrap(),
            "https://cameo-pi.local"
        );
        assert_eq!(
            base_url(" 10.0.0.5:8443/ ").unwrap(),
            "https://10.0.0.5:8443"
        );
        assert_eq!(base_url("https://pi.lan").unwrap(), "https://pi.lan");
        assert_eq!(base_url("[fe80::1]:443").unwrap(), "https://[fe80::1]:443");
        for bad in ["", "http://pi.lan", "pi.lan/api", "user@pi.lan", "pi lan"] {
            assert!(matches!(base_url(bad), Err(Error::Address(_))), "{bad:?}");
        }
    }

    #[test]
    fn credentials_keep_the_fingerprint_as_the_pi_prints_it() {
        let credentials = Credentials {
            address: "cameo-pi.local".into(),
            fingerprint: Fingerprint::of(b"certificate"),
            key: "secret".into(),
        };
        let text = serde_json::to_string(&credentials).unwrap();
        assert!(text.contains(&credentials.fingerprint.to_string()));
        assert_eq!(
            serde_json::from_str::<Credentials>(&text).unwrap(),
            credentials
        );
    }

    #[test]
    fn the_pis_cutter_view_reads() {
        let state: State = serde_json::from_value(json!({
            "boot": "38310347", "seq": 5, "version": "0.10.0",
            "cutters": [{"id": "cameo", "simulated": false, "view": {
                "action_epoch": 3, "activity": "idle", "cutting": null, "execution": null,
                "kind": null, "mat": {"state": "fresh", "generation": 2}, "mat_loaded": true,
                "name": "Cameo", "online": true, "progress": null,
                "queue": [{"turn": "t2", "job": {"job_id": "j", "revision": 1},
                           "actor": {"user": "Sam", "client": "phone"}}],
                "turn": {"id": "t1", "job": {"job_id": "j", "revision": 1},
                         "actor": {"user": null, "client": "CutCraft"}, "mat": null,
                         "taken": [], "tests": 0},
                "something_new": true
            }}]
        }))
        .unwrap();
        let view = &state.cutters[0].view;
        assert_eq!(view.mat, Mat::Fresh(2));
        assert_eq!(view.queue[0].actor.user.as_deref(), Some("Sam"));
        assert_eq!(view.turn.as_ref().unwrap().id, "t1");
        let answer: Answer =
            serde_json::from_value(json!({"answer": "queued", "turn": "t1", "position": 0}))
                .unwrap();
        assert_eq!(
            answer,
            Answer::Queued {
                turn: "t1".into(),
                position: 0
            }
        );
        let newer: Answer = serde_json::from_value(json!({"answer": "teleported"})).unwrap();
        assert_eq!(newer, Answer::Other);
    }

    #[test]
    fn keys_are_fresh() {
        let (a, b) = (new_key(), new_key());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }

    #[test]
    fn an_unreachable_pi_says_so() {
        // Nothing listens on port 9 of localhost.
        let error = Pairing::begin("127.0.0.1:9", "CutCraft test")
            .err()
            .unwrap();
        assert!(matches!(error, Error::Unreachable(_)), "{error:?}");
    }
}
