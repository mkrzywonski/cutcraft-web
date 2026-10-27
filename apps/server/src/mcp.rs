//! MCP on the Pi (spec §13): Streamable HTTP at `/mcp`, offering what the web interface offers.
//! Every tool calls the same service methods as the web interface and the protocol, so the same
//! rules apply: one validation path, turns, the queue, the fresh mat, idempotency keys and bound
//! starts.
//!
//! The router's guard runs first: it checks the Host, and a change needs an access key (spec §14).
//! Once users exist, the key belongs to a user and the tools act as that user; every action is
//! marked with the user and the program. Requests carrying a browser `Origin` are refused.

use std::sync::Arc;

use base64::Engine;
use cameo_pi_core::cutter::Kind;
use cameo_pi_core::worker::lock;
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Map, Value, json};

use crate::http::{App, Caller, by};
use crate::service::{ApiError, StartBody, Upload};

/// The `/mcp` service for the router.
pub fn service(app: App) -> StreamableHttpService<Tools, LocalSessionManager> {
    let config = StreamableHttpServerConfig::default()
        // The router's guard checks the Host already, against the Pi's own names.
        .disable_allowed_hosts()
        // Programs only: a request with any browser Origin is refused.
        .enforce_origin_validation();
    StreamableHttpService::new(
        move || Ok(Tools::new(app.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    )
}

#[derive(Clone)]
pub struct Tools {
    app: App,
}

type Out = Result<Value, ApiError>;

struct ToolSpec {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    read_only: bool,
    destructive: bool,
    /// The input schema's `properties`, and the required ones.
    properties: fn() -> Value,
    required: &'static [&'static str],
}

fn s(description: &str) -> Value {
    json!({"type": "string", "description": description})
}
fn i(description: &str) -> Value {
    json!({"type": "integer", "minimum": 0, "description": description})
}
fn folder(description: &str) -> Value {
    json!({"type": "array", "items": {"type": "string"}, "description": description})
}

const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "preparation_cancel",
        title: "Cancel preparation",
        description: "Cancel a specific in-flight open or Make cut-ready request in your session. Give its key from design_status.busy_key. A completed operation cannot be undone by cancellation.",
        read_only: false,
        destructive: false,
        properties: || json!({"key":s("The preparation request key to cancel.")}),
        required: &["key"],
    },
    ToolSpec {
        name: "design_status",
        title: "Current design",
        description: "Your current design session, incarnation and revision. Use before design_cut_copy.",
        read_only: true,
        destructive: false,
        properties: || json!({}),
        required: &[],
    },
    ToolSpec {
        name: "design_cut_copy",
        title: "Prepare cut copy",
        description: "Prepare a validated immutable copy of your current sheet (the shared cut.copy command). Returns copy.id for cut_queue and cut_start, revision 1. Does not queue or move a cutter. Later design edits cannot change it.",
        read_only: false,
        destructive: false,
        properties: || json!({"key": s("Unique key for this cut copy; retain it for retries."), "incarnation": s("From design_status."), "revision": i("The design_status revision reviewed.")}),
        required: &["key", "incarnation", "revision"],
    },
    ToolSpec {
        name: "cutters",
        title: "Cutters and their state",
        description: "Every cutter: online, the mat (none, fresh with its generation, used, unseen), activity, the current turn, the queue, progress, and the action epoch; plus the Pi's boot. A start names these values as seen.",
        read_only: true,
        destructive: false,
        properties: || json!({}),
        required: &[],
    },
    ToolSpec {
        name: "cutter_wait",
        title: "Wait for a cutter",
        description: "Wait until the cutter's running execution ends or pauses, or the timeout passes (default 60 s, at most 600); returns its state.",
        read_only: true,
        destructive: false,
        properties: || json!({"cutter": s("The cutter's id."), "timeout_s": i("Longest wait in seconds.")}),
        required: &["cutter"],
    },
    ToolSpec {
        name: "library_list",
        title: "List a library folder",
        description: "One folder of the job library: its folders and jobs (id, name, notes, times cut, current revision).",
        read_only: true,
        destructive: false,
        properties: || json!({"folder": folder("The folder, from the top (default: the top).")}),
        required: &[],
    },
    ToolSpec {
        name: "library_search",
        title: "Search the library",
        description: "Jobs whose name or notes contain the text (any case), with their folders.",
        read_only: true,
        destructive: false,
        properties: || json!({"text": s("What to look for.")}),
        required: &["text"],
    },
    ToolSpec {
        name: "job_get",
        title: "A job's details",
        description: "A job's details and history, and one revision (default the current): material, mat, the loading steps and the steps once the mat is loaded.",
        read_only: true,
        destructive: false,
        properties: || json!({"job_id": s("The job's id."), "revision": i("A revision number (default: current).")}),
        required: &["job_id"],
    },
    ToolSpec {
        name: "job_file",
        title: "Download a job's file",
        description: "One of a revision's files, base64: the CutCraft project (project), the original design (design) or the cut lines as SVG (cut-lines).",
        read_only: true,
        destructive: false,
        properties: || {
            json!({
                "job_id": s("The job's id."),
                "revision": i("A revision number (default: current)."),
                "what": {"type": "string", "enum": ["project", "design", "cut-lines"]},
            })
        },
        required: &["job_id", "what"],
    },
    ToolSpec {
        name: "catalog",
        title: "Materials and mats",
        description: "The material and mat lists CutCraft last sent, with who sent them and when; materials and mats for job_adjust are named from these (mats also by the standard names).",
        read_only: true,
        destructive: false,
        properties: || json!({}),
        required: &[],
    },
    ToolSpec {
        name: "job_adjust_preview",
        title: "Preview an adjustment",
        description: "What changing a job's cut settings would make, stored nowhere: valid or why not, the estimate, the new settings and the cut lines. adjustments may hold scale or width_mm, offset_mm or anchor (top-left, top-right, bottom-left, bottom-right, centre), rotation_deg, mirror, material (a name from catalog), blade, force, speed, passes, overcut_mm, mat (a name), piece_mm ([w, h] or null), piece_at_mm, weeding_box, weeding_margin_mm and layering_marks. The design itself can't change here.",
        read_only: true,
        destructive: false,
        properties: || json!({"job_id": s("The job's id."), "revision": i("The revision to adjust (its current one)."), "adjustments": {"type": "object"}}),
        required: &["job_id", "revision", "adjustments"],
    },
    ToolSpec {
        name: "job_adjust",
        title: "Adjust a job",
        description: "Save changed cut settings (as for job_adjust_preview) as the job's next revision, planned here and checked like any upload. Refused if the job has a newer revision, or if the result can't be cut.",
        read_only: false,
        destructive: false,
        properties: || json!({"job_id": s("The job's id."), "revision": i("The revision adjusted (its current one)."), "adjustments": {"type": "object"}, "key": s("An idempotency key.")}),
        required: &["job_id", "revision", "adjustments"],
    },
    ToolSpec {
        name: "job_send",
        title: "Send a job",
        description: "Store a prepared sheet: a .cutcraft project (base64) and its CutPlan (JSON), as CutCraft makes them. Validated fully before anything is stored. With job_id it becomes a new revision of that job.",
        read_only: false,
        destructive: false,
        properties: || {
            json!({
                "name": s("The job's name."),
                "folder": folder("The folder, from the top (default: the top)."),
                "project_base64": s("The .cutcraft project file, base64."),
                "plan": {"type": "object", "description": "The CutPlan, as JSON."},
                "planner": s("The CutCraft build that planned it, VERSION (COMMIT)."),
                "job_id": s("Make it a new revision of this job."),
                "key": s("An idempotency key; a retry with the same key is answered, not repeated."),
            })
        },
        required: &["name", "project_base64", "plan", "planner"],
    },
    ToolSpec {
        name: "job_edit",
        title: "Rename a job or change its notes",
        description: "Change a job's name and/or notes. details_revision is the job's as you read it; a newer one is refused with the current details.",
        read_only: false,
        destructive: false,
        properties: || json!({"job_id": s("The job's id."), "details_revision": i("From job_get."), "name": s("New name."), "notes": s("New notes.")}),
        required: &["job_id", "details_revision"],
    },
    ToolSpec {
        name: "job_move",
        title: "Move a job",
        description: "Move a job to another folder. Not while it's queued or on a cutter.",
        read_only: false,
        destructive: false,
        properties: || json!({"job_id": s("The job's id."), "folder": folder("The folder, from the top.")}),
        required: &["job_id", "folder"],
    },
    ToolSpec {
        name: "job_delete",
        title: "Delete a job",
        description: "Move a job to Recently deleted (outside the library and its backups). Not while it's queued or on a cutter.",
        read_only: false,
        destructive: true,
        properties: || json!({"job_id": s("The job's id.")}),
        required: &["job_id"],
    },
    ToolSpec {
        name: "folder_create",
        title: "Make a folder",
        description: "Make a library folder.",
        read_only: false,
        destructive: false,
        properties: || json!({"parent": folder("Where (default: the top)."), "name": s("Its name.")}),
        required: &["name"],
    },
    ToolSpec {
        name: "folder_move",
        title: "Move a folder",
        description: "Move a library folder, with everything in it, into another folder. Not into itself, and not while one of its jobs is queued or on a cutter. A folder of the same name there makes it \"name (2)\". Returns its new path.",
        read_only: false,
        destructive: false,
        properties: || json!({"path": folder("The folder to move."), "to": folder("Where to (default: the top).")}),
        required: &["path"],
    },
    ToolSpec {
        name: "folder_rename",
        title: "Rename a folder",
        description: "Rename a library folder. Not while one of its jobs is queued or on a cutter. \"Recently deleted\" is kept at the top. Returns its new path.",
        read_only: false,
        destructive: false,
        properties: || json!({"path": folder("The folder."), "name": s("Its new name.")}),
        required: &["path", "name"],
    },
    ToolSpec {
        name: "deleted_list",
        title: "Recently deleted",
        description: "What's in Recently deleted: deleted jobs (with the folder they were in) and any library a restore replaced. Each has `entry` for deleted_restore and deleted_purge, and `expires_at`, when it's deleted for good (30 days after deletion).",
        read_only: true,
        destructive: false,
        properties: || json!({}),
        required: &[],
    },
    ToolSpec {
        name: "deleted_restore",
        title: "Restore a deleted job",
        description: "Put a deleted job back in the folder it was in (the top if that folder is gone). A job of the same name there makes it \"name (2)\". Returns its id and folder.",
        read_only: false,
        destructive: false,
        properties: || json!({"entry": s("From deleted_list.")}),
        required: &["entry"],
    },
    ToolSpec {
        name: "deleted_purge",
        title: "Delete for good",
        description: "Delete something in Recently deleted now, for good. It can't be undone.",
        read_only: false,
        destructive: true,
        properties: || json!({"entry": s("From deleted_list.")}),
        required: &["entry"],
    },
    ToolSpec {
        name: "folder_delete",
        title: "Delete a folder",
        description: "Delete an empty library folder.",
        read_only: false,
        destructive: true,
        properties: || json!({"path": folder("The empty folder.")}),
        required: &["path"],
    },
    ToolSpec {
        name: "cut_queue",
        title: "Ask for a turn",
        description: "Queue a job's revision on a cutter. Position 0 is the cutter's current turn. Refused if the job has a newer revision.",
        read_only: false,
        destructive: false,
        properties: || json!({"cutter": s("The cutter's id."), "job_id": s("The job's id."), "revision": i("The revision you looked at."), "key": s("An idempotency key.")}),
        required: &["cutter", "job_id", "revision"],
    },
    ToolSpec {
        name: "cut_start",
        title: "Start a test cut or the cut",
        description: "Start the turn's test cut or main cut. It moves the cutter: someone must have loaded the material, and the mat must be fresh. Name what you saw in cutters: boot, the turn, the revision, the mat's generation and the action epoch; anything changed since is refused.",
        read_only: false,
        destructive: true,
        properties: || {
            json!({
                "cutter": s("The cutter's id."),
                "kind": {"type": "string", "enum": ["test", "main"], "description": "A test cut (beside the design) or the cut."},
                "boot": s("The Pi's boot, from cutters."),
                "turn": s("The turn's id."),
                "job_id": s("The job's id."),
                "revision": i("The turn's revision."),
                "generation": i("The fresh mat's generation."),
                "action_epoch": i("The cutter's action epoch."),
                "key": s("An idempotency key."),
            })
        },
        required: &[
            "cutter",
            "kind",
            "boot",
            "job_id",
            "revision",
            "generation",
            "action_epoch",
        ],
    },
    ToolSpec {
        name: "cut_pause",
        title: "Pause",
        description: "Pause the running cut at the end of the current contour.",
        read_only: false,
        destructive: false,
        properties: || json!({"cutter": s("The cutter's id.")}),
        required: &["cutter"],
    },
    ToolSpec {
        name: "cut_resume",
        title: "Resume",
        description: "Resume a paused cut.",
        read_only: false,
        destructive: false,
        properties: || json!({"cutter": s("The cutter's id.")}),
        required: &["cutter"],
    },
    ToolSpec {
        name: "cut_stop",
        title: "Stop",
        description: "Stop now: nothing more is sent and the cut is abandoned.",
        read_only: false,
        destructive: true,
        properties: || json!({"cutter": s("The cutter's id.")}),
        required: &["cutter"],
    },
    ToolSpec {
        name: "turn_release",
        title: "End a turn",
        description: "Hand the cutter to the next in line.",
        read_only: false,
        destructive: false,
        properties: || json!({"cutter": s("The cutter's id."), "turn": s("The turn's id.")}),
        required: &["cutter", "turn"],
    },
    ToolSpec {
        name: "mat_confirm_fresh",
        title: "Confirm the mat is fresh",
        description: "The cutter has a mat loaded that cameo-pi didn't see going in (after a restart or reconnect): confirm it has fresh material, so it can be cut on. Only when the person says so; never for a mat that was cut on.",
        read_only: false,
        destructive: false,
        properties: || json!({"cutter": s("The cutter's id.")}),
        required: &["cutter"],
    },
    ToolSpec {
        name: "practice_mat",
        title: "Practice cutter: load or unload",
        description: "Press the practice cutter's Load Mat (load true) or Unload (load false). Only the practice cutter.",
        read_only: false,
        destructive: false,
        properties: || json!({"cutter": s("The practice cutter's id."), "load": {"type": "boolean"}}),
        required: &["cutter", "load"],
    },
    ToolSpec {
        name: "backup_status",
        title: "Backup status",
        description: "The backup to another computer (SFTP): where, how often, the last success and attempt, whether it's failing, and any run in progress.",
        read_only: true,
        destructive: false,
        properties: || json!({}),
        required: &[],
    },
    ToolSpec {
        name: "backup_now",
        title: "Back up now",
        description: "Start a copy of the library to the scheduled backup's computer; backup_status shows how it went.",
        read_only: false,
        destructive: false,
        properties: || json!({}),
        required: &[],
    },
];

fn text<'a>(args: &'a Value, name: &str) -> Result<&'a str, ApiError> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request(format!("Give {name}.")))
}

fn number(args: &Value, name: &str) -> Result<u64, ApiError> {
    args.get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| ApiError::bad_request(format!("Give {name} as a whole number.")))
}

fn names(args: &Value, name: &str) -> Vec<String> {
    args.get(name)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn key(args: &Value) -> String {
    args.get("key")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| crate::keys::random_hex(16))
}

impl Tools {
    pub(crate) fn new(app: App) -> Self {
        Self { app }
    }

    pub(crate) async fn invoke(&self, name: &str, args: Value, caller: Caller) -> Out {
        if matches!(
            name,
            "design_status" | "design_cut_copy" | "preparation_cancel"
        ) {
            let principal = caller
                .principal()
                .ok_or_else(|| ApiError::new(401, "login", "Log in first."))?;
            if name == "preparation_cancel" {
                return crate::design::cancel_preparation(
                    &self.app,
                    &principal,
                    text(&args, "key")?,
                )
                .await;
            }
            if name == "design_status" {
                return serde_json::to_value(
                    crate::design::session_status(&self.app, &principal).await?,
                )
                .map_err(|e| ApiError::bad_request(e.to_string()));
            }
            let request =
                serde_json::from_value(args).map_err(|e| ApiError::bad_request(e.to_string()))?;
            return match crate::design::prepare_copy(self.app.clone(), principal, request).await? {
                cutcraft_web_protocol::CommandReply::Applied { revision, value } => {
                    Ok(json!({"answer":"applied", "revision":revision, "value":value}))
                }
                cutcraft_web_protocol::CommandReply::Refused { code, message } => {
                    Err(ApiError::new(409, &code, message))
                }
            };
        }
        let tools = self.clone();
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || tools.run(&name, &args, &caller))
            .await
            .map_err(|e| ApiError::new(500, "internal", e.to_string()))?
    }

    fn run(&self, name: &str, args: &Value, caller: &Caller) -> Out {
        let service = &self.app.service;
        Ok(match name {
            "cutters" => crate::http::snapshot(&self.app),
            "cutter_wait" => {
                let cutter = service.cutter(text(args, "cutter")?)?;
                let limit = args
                    .get("timeout_s")
                    .and_then(Value::as_u64)
                    .unwrap_or(60)
                    .min(600);
                let until = std::time::Instant::now() + std::time::Duration::from_secs(limit);
                loop {
                    let view = lock(&cutter.shared).view();
                    let paused = view.activity == "paused";
                    if view.execution.is_none() || paused || std::time::Instant::now() >= until {
                        break json!(view);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            }
            "library_list" => json!(service.list(&names(args, "folder"))?),
            "library_search" => {
                let found = service.library.search(text(args, "text")?);
                json!(
                    found
                        .into_iter()
                        .map(|(folder, job)| json!({"folder": folder, "job": job}))
                        .collect::<Vec<_>>()
                )
            }
            "job_get" => {
                let revision = args
                    .get("revision")
                    .and_then(Value::as_u64)
                    .map(|r| r as u32);
                service.job(text(args, "job_id")?, revision)?
            }
            "job_file" => {
                let id = text(args, "job_id")?;
                let what = text(args, "what")?;
                if !matches!(what, "project" | "design" | "cut-lines") {
                    return Err(ApiError::bad_request(
                        "what is project, design or cut-lines.",
                    ));
                }
                let revision = match args.get("revision").and_then(Value::as_u64) {
                    Some(r) => r as u32,
                    None => service.library.details(id)?.current_revision,
                };
                let (name, kind, bytes) = service.download(id, revision, what)?;
                json!({
                    "file_name": name,
                    "content_type": kind,
                    "revision": revision,
                    "base64": base64::engine::general_purpose::STANDARD.encode(bytes),
                })
            }
            "catalog" => json!(service.catalog()),
            "job_adjust_preview" | "job_adjust" => {
                let adjustments: cameo_pi_core::adjust::Adjustments =
                    serde_json::from_value(args.get("adjustments").cloned().unwrap_or(json!({})))
                        .map_err(|e| ApiError::bad_request(format!("adjustments: {e}")))?;
                let id = text(args, "job_id")?;
                let revision = number(args, "revision")? as u32;
                if name == "job_adjust_preview" {
                    service.adjust_preview(id, revision, &adjustments)?
                } else {
                    json!(service.adjust_save(
                        id,
                        revision,
                        &adjustments,
                        key(args),
                        &by(caller)
                    )?)
                }
            }
            "job_send" => {
                let project = base64::engine::general_purpose::STANDARD
                    .decode(text(args, "project_base64")?)
                    .map_err(|_| ApiError::bad_request("project_base64 isn't base64."))?;
                let plan = serde_json::to_vec(
                    args.get("plan")
                        .ok_or_else(|| ApiError::bad_request("Give plan."))?,
                )
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
                let upload = Upload {
                    folder: names(args, "folder"),
                    name: text(args, "name")?.to_owned(),
                    job_id: args
                        .get("job_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    planner: text(args, "planner")?.to_owned(),
                    project,
                    plan,
                    key: key(args),
                };
                json!(service.upload(upload, &by(caller))?)
            }
            "job_edit" => json!(service.edit(
                text(args, "job_id")?,
                number(args, "details_revision")?,
                args.get("name").and_then(Value::as_str),
                args.get("notes").and_then(Value::as_str),
                &by(caller),
            )?),
            "job_move" => {
                service.move_job(text(args, "job_id")?, &names(args, "folder"))?;
                json!({"moved": true})
            }
            "job_delete" => {
                service.delete_job(text(args, "job_id")?)?;
                json!({"deleted": true})
            }
            "folder_create" => {
                service.create_folder(&names(args, "parent"), text(args, "name")?)?;
                json!({"created": true})
            }
            "folder_move" => {
                json!({"path": service.move_folder(&names(args, "path"), &names(args, "to"))?})
            }
            "folder_rename" => {
                json!({"path": service.rename_folder(&names(args, "path"), text(args, "name")?)?})
            }
            "deleted_list" => json!(service.deleted()?),
            "deleted_restore" => service.restore_deleted(text(args, "entry")?)?,
            "deleted_purge" => {
                service.purge_deleted(text(args, "entry")?)?;
                json!({"deleted": true})
            }
            "folder_delete" => {
                service.delete_folder(&names(args, "path"))?;
                json!({"deleted": true})
            }
            "cut_queue" => json!(service.queue(
                text(args, "cutter")?,
                text(args, "job_id")?,
                Some(number(args, "revision")? as u32),
                caller.actor.clone(),
                &key(args),
            )?),
            "cut_start" => {
                let kind = match text(args, "kind")? {
                    "test" => Kind::Test,
                    "main" => Kind::Main,
                    _ => return Err(ApiError::bad_request("kind is test or main.")),
                };
                let body = StartBody {
                    boot: text(args, "boot")?.to_owned(),
                    kind,
                    turn: args.get("turn").and_then(Value::as_str).map(str::to_owned),
                    job_id: text(args, "job_id")?.to_owned(),
                    revision: number(args, "revision")? as u32,
                    generation: number(args, "generation")?,
                    action_epoch: number(args, "action_epoch")?,
                    key: key(args),
                };
                json!(service.start(text(args, "cutter")?, body, caller.actor.clone())?)
            }
            "cut_pause" => json!(service.pause(text(args, "cutter")?)?),
            "cut_resume" => json!(service.resume(text(args, "cutter")?)?),
            "cut_stop" => json!(service.stop(text(args, "cutter")?)?),
            "turn_release" => json!(service.release(text(args, "cutter")?, text(args, "turn")?)?),
            "mat_confirm_fresh" => {
                json!(service.confirm_fresh(text(args, "cutter")?, caller.actor.clone())?)
            }
            "practice_mat" => {
                let load = args
                    .get("load")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| ApiError::bad_request("Give load as true or false."))?;
                service.practice(text(args, "cutter")?, load)?;
                json!({"loaded": load})
            }
            "backup_status" => service.backup_view(),
            "backup_now" => {
                let service = service.clone();
                std::thread::spawn(move || {
                    let _ = service.backup_now();
                });
                json!({"started": true})
            }
            _ => {
                return Err(ApiError::new(
                    404,
                    "unknown-tool",
                    format!("There is no tool {name:?}."),
                ));
            }
        })
    }
}

impl ServerHandler for Tools {
    fn get_info(&self) -> ServerConfig {
        let capabilities = ServerCapabilities::builder().enable_tools().build();
        ServerConfig::new(capabilities)
            .with_server_info(Implementation::new("cameo-pi", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "cameo-pi, a network cut server for Silhouette Cameo cutters. Use design_status and design_cut_copy to prepare an immutable copy of the current sheet without motion, then cut_queue its copy.id at revision 1. Or start with cutters and library_list. To cut: job_send a prepared sheet (or pick a library job), cut_queue it, and when it's the cutter's current turn and its mat is fresh, cut_start with the values cutters shows. Cutting moves the machine: tell the person what you're about to cut, and let them load the material. cut_stop stops at once.",
            )
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = TOOLS
            .iter()
            .map(|t| {
                let schema = Map::from_iter([
                    ("type".into(), json!("object")),
                    ("properties".into(), (t.properties)()),
                    ("required".into(), json!(t.required)),
                    ("additionalProperties".into(), json!(false)),
                ]);
                Tool::new(t.name, t.description, schema).with_annotations(
                    ToolAnnotations::with_title(t.title)
                        .read_only(t.read_only)
                        .destructive(t.destructive)
                        .idempotent(t.read_only || t.name == "design_cut_copy")
                        .open_world(false),
                )
            })
            .collect();
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let caller = context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<Caller>().cloned())
            .ok_or_else(|| ErrorData::internal_error("no caller", None))?;
        let args = Value::Object(request.arguments.unwrap_or_default());
        let name = request.name.to_string();
        let result = self.invoke(&name, args, caller).await;
        Ok(match result {
            Ok(value) => CallToolResult::structured(json!({ "result": value })),
            Err(error) => CallToolResult::structured_error(json!({
                "error": error.message,
                "code": error.error,
            })),
        }
        .into())
    }
}
