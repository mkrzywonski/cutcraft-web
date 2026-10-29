//! The contract between the browser, the server and its preparation workers (spec §13).
//!
//! Everything that crosses a process or network boundary is defined here and nowhere else. It
//! builds for `wasm32-unknown-unknown`, so it holds no controller types, threads, clocks, paths or
//! settings files: the browser sees command ids and their params, a small session snapshot, and
//! references to geometry and pictures it fetches separately.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Raised whenever a change is not additive (docs/development.md §5). A client with another
/// version is told to reload.
pub const PROTOCOL_VERSION: u32 = 6;

/// One change or question, as a registry command (spec §5, §7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandRequest {
    /// The registry id, e.g. `sheet.place`.
    pub id: String,
    /// The command's own params, as the registry's schema describes them.
    #[serde(default)]
    pub params: Value,
    /// Chosen by the client, unique per change. Asking again with the same key returns the first
    /// answer; the same key with a different body is refused.
    pub key: String,
    /// The session load the client saw. A request for an earlier load is refused as stale, even
    /// if the revision number happens to match.
    pub incarnation: String,
    /// The session revision the change was made against, or none for a read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
}

/// The answer to a [`CommandRequest`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum CommandReply {
    /// Done: the session is now at `revision`.
    Applied { revision: u64, value: Value },
    /// Not done, and nothing changed: a stable `code` and a message in the person's words.
    Refused { code: String, message: String },
}

/// Looking up a key after a lost reply or a reconnect (spec §7): the three answers a device can
/// act on. `NotApplied` means it may send the change again with a new key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case")]
pub enum KeyLookup {
    Applied { revision: u64, value: Value },
    Refused { code: String, message: String },
    NotApplied,
}

/// Refusal codes the client acts on. Others are shown as they come.
pub mod codes {
    /// The request's revision is older than the session's: refresh and decide again.
    pub const STALE_REVISION: &str = "stale-revision";
    /// The request was made for an earlier load of the session.
    pub const STALE_INCARNATION: &str = "stale-incarnation";
    /// The key was used before for a different change.
    pub const KEY_REUSED: &str = "key-reused";
    /// The client speaks another protocol version: reload the page.
    pub const PROTOCOL: &str = "protocol-version";
    /// A change must name the revision it was made against.
    pub const REVISION_REQUIRED: &str = "revision-required";
    /// Opening something else would replace changes that aren't saved: ask the person first.
    pub const UNSAVED: &str = "unsaved-changes";
    /// Someone else saved this project since it was opened: offer Reload or Save As (spec §11).
    pub const SAVED_ELSEWHERE: &str = "saved-elsewhere";
    /// The project isn't in the library yet: Save As.
    pub const NOT_SAVED_YET: &str = "not-saved-yet";
    /// Not allowed for this person.
    pub const FORBIDDEN: &str = "forbidden";
}

/// What every screen of the session draws from (spec §12). Small: no geometry, no pixels and no
/// server paths. Pushed at each new revision.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Browser-safe presentation state. No source paths, source bytes or device authority.
    #[serde(default)]
    pub ui: Presentation,

    pub protocol: u32,
    pub incarnation: String,
    pub revision: u64,
    /// Opening or preparing, with what to show meanwhile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design: Option<DesignView>,
    /// The person's material list, by name, for choosing one.
    #[serde(default)]
    pub materials: Vec<String>,
}

/// Full desktop presentation, projected explicitly from the controller. JSON values here use
/// the shared registry's existing material, mat, report and row schemas; they are not commands.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Presentation {
    pub screen: String,
    pub units: String,
    pub font_size: f32,
    pub snap: Value,
    pub panel: Value,
    pub defaults: Value,
    pub history: Value,
    pub materials: Vec<Value>,
    pub mats: Vec<Value>,
    pub sheet: Value,
    pub rows: Vec<Value>,
    pub group_by: String,
    pub size_mm: Option<[f64; 2]>,
    pub report: Value,
    pub plan: Value,
    pub commands: Vec<Value>,
}

/// The open project, as the screens need it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DesignView {
    /// The design's file name, as uploaded.
    pub name: String,
    /// The project's revision: it changes whenever the project changes.
    pub project_revision: u64,
    pub cut_ready: bool,
    /// Questions and problems still to resolve.
    pub issues: u32,
    /// The prepared vectors and the mat, fetched by id when it changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<GeometryRef>,
    /// The artwork, rendered by region on request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<ArtView>,
    /// The first sheet's settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<SheetView>,
    /// Changes since it was opened or last saved (spec §7, §11).
    #[serde(default)]
    pub unsaved: bool,
    /// The library revision it was opened from or last saved as: Save names it as its base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved: Option<SavedRef>,
}

/// A library project's revision (spec §11, §16).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedRef {
    pub job: String,
    pub revision: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeometryRef {
    /// Changes whenever the prepared vectors or the sheet's plan change.
    pub id: String,
}

/// Where the artwork is. The overlay view works in millimetres as resized; pictures are asked
/// for in the design's own millimetres (before resizing), which is that divided by `scale`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArtView {
    /// Names everything a picture depends on: the session load, the opened source file, the
    /// project revision and the resize. Two designs never share one, so a picture is never
    /// shown for the wrong design (Codex's review of milestone 0).
    pub id: String,
    /// The design revision pictures are rendered from.
    pub design_revision: u64,
    /// What to fit: the page, grown to any art outside it, in mm as resized: [x0, y0, x1, y1].
    pub bounds: [f64; 4],
    /// The resize: mm as resized per mm of the design.
    pub scale: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SheetView {
    pub material: String,
    /// The mat's name, or "Roll".
    pub mat: String,
    /// The current plan is valid to cut.
    pub plan_valid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_s: Option<f64>,
}

/// The prepared vectors for drawing locally every frame (spec §12): the sheet on its mat, in
/// millimetres from the mat's top-left corner, and the cut lines over the artwork, in mm as
/// resized.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Geometry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mat: Option<MatGeometry>,
    /// The planned cut, on the mat.
    #[serde(default)]
    pub sheet: Vec<Stroke>,
    /// The bounding box of what's cut (not aids), on the mat: what a drag moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art_box_mm: Option<[f64; 4]>,
    /// Where the next test cut goes, on the mat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_spot_mm: Option<[f64; 4]>,
    /// The cut lines over the artwork, in mm as resized.
    #[serde(default)]
    pub lines: Vec<Stroke>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MatGeometry {
    /// The mat's outer edge: [width, height] in mm.
    pub outer_mm: [f64; 2],
    /// The sticky grid: [x0, y0, x1, y1] in mm.
    pub sticky_mm: [f64; 4],
    /// The cutter's start corner on the mat, in mm. Placement (`sheet.place`) counts from here.
    pub origin_mm: [f64; 2],
}

/// One polyline, drawn in its colour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    #[serde(default)]
    pub item: Option<String>,
    #[serde(default)]
    pub source_ids: Vec<String>,
    #[serde(default = "included_by_default")]
    pub included: bool,
    /// `#rrggbb`.
    pub colour: String,
    /// An aid (weeding box, weed line, layering mark), drawn lighter.
    #[serde(default)]
    pub aid: bool,
    /// Single precision is plenty on screen and halves the transfer.
    pub points: Vec<[f32; 2]>,
}

fn included_by_default() -> bool {
    true
}

/// One picture of the artwork (spec §12), named by everything that affects it, so the browser
/// can cache it and a stale one is never mistaken for the current one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PictureRequest {
    /// The artwork's identity ([`ArtView::id`]).
    pub art: String,
    /// The region, in the design's own mm (before resizing): [x0, y0, x1, y1].
    pub region: [f64; 4],
    /// The zoom, rounded to a step, as the desktop app rounds it.
    pub zoom_step: i32,
    pub width: u32,
    pub height: u32,
}

impl PictureRequest {
    /// The largest picture a client may ask for, per side.
    pub const MAX_SIDE: u32 = 4096;

    /// The query string for `GET /api/picture`.
    pub fn query(&self) -> String {
        let [x0, y0, x1, y1] = self.region;
        format!(
            "art={}&x0={x0}&y0={y0}&x1={x1}&y1={y1}&zoom_step={}&width={}&height={}",
            self.art, self.zoom_step, self.width, self.height
        )
    }
}

/// Prepare the current sheet for cutting, without queuing or starting motion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyRequest {
    #[serde(default)]
    pub test: bool,
    pub key: String,
    pub incarnation: String,
    pub revision: u64,
}

/// The immutable copy named by queue, loading preview and Start. Its stored revision is 1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CutCopy {
    #[serde(default)]
    pub test_only: bool,
    pub id: String,
    pub name: String,
    pub revision: u32,
    pub project_revision: u64,
    pub sheet: String,
    pub material: String,
    pub mirrored: bool,
    pub digest: String,
}

/// The privileged contract between the server and its preparation workers (spec §5), one JSON
/// line each way. Kept apart from the browser's: nothing here is reachable from a page, which
/// only ever reaches the server's own routes (Codex's review of the development standard).
pub mod worker {
    use serde::{Deserialize, Serialize};

    use super::{
        CommandReply, CommandRequest, Geometry, KeyLookup, PictureRequest, SavedRef,
        SessionSnapshot,
    };

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "ask", rename_all = "snake_case")]
    pub enum WorkerRequest {
        Command(CommandRequest),
        /// Control-plane cancellation for one in-flight call, never a later one.
        Cancel {
            target: u64,
        },
        Replay(CommandRequest),
        /// Record an already durable server-side action, without marking the project edited.
        Remember {
            request: CommandRequest,
            answer: CommandReply,
        },
        Lookup {
            key: String,
        },
        Snapshot,
        Update {
            request: CommandRequest,
            file: String,
            name: String,
        },
        Download {
            kind: String,
        },
        Geometry,
        Picture(PictureRequest),
        /// Open something in place of the session's project: a change, keyed and checked like
        /// any other (spec §7).
        Open(OpenRequest),
        /// The open project as a `.cutcraft` file and its first sheet's plan, for saving.
        Export,
        /// The server saved the project to the library: from now on it's that revision.
        Saved {
            request: CommandRequest,
            saved: SavedRef,
        },
        /// Another session changed the household's material or mat list: read them again.
        ReloadLists,
        Checkpoint,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct Call {
        pub id: u64,
        pub request: WorkerRequest,
    }
    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct Response {
        pub id: u64,
        pub reply: WorkerReply,
    }

    /// Opening a design or a library project in the session (spec §7).
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    pub struct OpenRequest {
        pub source: OpenSource,
        #[serde(default)]
        pub request: Option<CommandRequest>,
        pub key: String,
        pub incarnation: String,
        pub revision: u64,
        /// The person was told the project open now has unsaved changes, and chose to open
        /// anyway. Its last state is kept as a draft.
        #[serde(default)]
        pub replace_unsaved: bool,
    }

    /// What to open, already in the person's area under a name the server chose.
    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "from", rename_all = "snake_case")]
    pub enum OpenSource {
        Upload {
            file: String,
            name: String,
        },
        Project {
            file: String,
            name: String,
            saved: SavedRef,
        },
    }

    #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "reply", rename_all = "snake_case")]
    pub enum WorkerReply {
        Download {
            mime: String,
            bytes: Vec<u8>,
        },
        Command(CommandReply),
        Cancel {
            requested: bool,
        },
        Replay {
            result: Option<CommandReply>,
        },
        Lookup(KeyLookup),
        Snapshot(Box<SessionSnapshot>),
        Geometry(Geometry),
        /// A PNG, base64-encoded (a JSON line can't carry bytes).
        Picture {
            png_base64: String,
        },
        Export {
            name: String,
            /// The `.cutcraft` file's JSON.
            project: String,
            /// The first sheet's plan, as JSON.
            plan: String,
            saved: Option<SavedRef>,
        },
        Failed {
            message: String,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_command_round_trips_and_omits_an_absent_revision() {
        let request = CommandRequest {
            id: "sheet.place".into(),
            params: json!({"x": 12.5, "y": 30.0}),
            key: "k1".into(),
            incarnation: "i1".into(),
            revision: None,
        };
        let text = serde_json::to_string(&request).unwrap();
        assert!(!text.contains("revision"));
        assert_eq!(
            serde_json::from_str::<CommandRequest>(&text).unwrap(),
            request
        );
    }

    #[test]
    fn replies_are_tagged_for_the_client() {
        let refused = CommandReply::Refused {
            code: codes::STALE_REVISION.into(),
            message: "Someone changed this project. Here it is now.".into(),
        };
        assert_eq!(
            serde_json::to_value(&refused).unwrap()["answer"],
            json!("refused")
        );
        assert_eq!(
            serde_json::to_value(KeyLookup::NotApplied).unwrap(),
            json!({"answer": "not_applied"})
        );
    }

    #[test]
    fn an_older_snapshot_without_new_fields_still_reads() {
        let snapshot: SessionSnapshot =
            serde_json::from_value(json!({"protocol": 1, "incarnation": "i", "revision": 3}))
                .unwrap();
        assert_eq!(snapshot.revision, 3);
        assert!(snapshot.design.is_none() && snapshot.materials.is_empty());
    }

    #[test]
    fn worker_lines_round_trip() {
        use crate::worker::WorkerRequest;
        for request in [
            WorkerRequest::Snapshot,
            WorkerRequest::Remember {
                request: CommandRequest {
                    id: "cut.copy".into(),
                    params: json!({}),
                    key: "copy".into(),
                    incarnation: "i".into(),
                    revision: Some(1),
                },
                answer: CommandReply::Applied {
                    revision: 1,
                    value: json!({"copy":{"id":"copy-test"}}),
                },
            },
            WorkerRequest::Lookup { key: "k".into() },
            WorkerRequest::Picture(PictureRequest {
                art: "i-abc-2-1".into(),
                region: [0.0, 0.0, 100.0, 50.0],
                zoom_step: -3,
                width: 800,
                height: 400,
            }),
        ] {
            let line = serde_json::to_string(&request).unwrap();
            assert!(!line.contains('\n'));
            assert_eq!(
                serde_json::from_str::<WorkerRequest>(&line).unwrap(),
                request
            );
        }
    }
    #[test]
    fn worker_replay_replies_round_trip_with_and_without_a_record() {
        use worker::WorkerReply;
        for reply in [
            WorkerReply::Replay { result: None },
            WorkerReply::Replay {
                result: Some(CommandReply::Applied {
                    revision: 7,
                    value: json!({"saved":true}),
                }),
            },
        ] {
            let line = serde_json::to_string(&reply).unwrap();
            assert_eq!(serde_json::from_str::<WorkerReply>(&line).unwrap(), reply);
        }
    }
}
