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
///
/// Version 6 carries what the desktop layout draws (spec §12): the issue and info cards, the
/// rows, the sheet's settings, per-project units and snap, the person's panel arrangement and
/// history, and the material and mat lists with their settings.
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
    /// The person's material list, for choosing one.
    #[serde(default)]
    pub materials: Vec<MaterialView>,
    /// Their mat list (spec §8: both are the household's).
    #[serde(default)]
    pub mats: Vec<MatView>,
    /// What a newly opened design starts with.
    #[serde(default)]
    pub new_design: NewDesignView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materials_error: Option<String>,
    /// Inches or millimetres, kept with the project (spec §8).
    #[serde(default)]
    pub units: Units,
    /// The interface's base font size in points, per person.
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// The interface's colour scheme, per person.
    #[serde(default)]
    pub theme: Theme,
    /// Snap to grid on the Cut preview, kept with the project (spec §8).
    #[serde(default)]
    pub snap: SnapView,
    /// How they arranged the right panel, per person.
    #[serde(default)]
    pub panel: PanelView,
    /// What the desktop window shows, shared by the person's desktop layouts (spec §8).
    #[serde(default)]
    pub screen: Screen,
    /// Undo and redo for the open design, labelled with where each change came from.
    #[serde(default)]
    pub history: HistoryView,
}

fn default_font_size() -> f32 {
    14.0
}

/// How lengths are shown and typed; stored values are always mm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Units {
    #[default]
    Inches,
    Millimetres,
}

/// The interface's colour scheme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Whatever the device prefers.
    #[default]
    System,
    Light,
    Dark,
}

/// The window's screens (spec §4): what `view.show` changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Screen {
    /// The artwork and its cut lines side by side; the start screen without a design.
    #[default]
    SideBySide,
    /// The cut lines drawn over the artwork.
    Overlay,
    /// The Cut Preview: a sheet placed on its mat or roll.
    Mat,
    /// The sheet cutting screen.
    Cut,
    /// The optional square-and-triangle pattern screen.
    TestCut,
}

impl Screen {
    pub fn is_cut_screen(self) -> bool {
        matches!(self, Self::Cut | Self::TestCut)
    }
}

/// Snap to grid on the Cut preview.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SnapView {
    pub enabled: bool,
    /// The grid step in mm; none until they set one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_mm: Option<f64>,
}

/// How they arranged the right panel (spec §8: the order and locks are the person's; whether a
/// group is folded is the browser's).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PanelView {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locked: Vec<String>,
    /// Whether they left a group open (true) or closed (false).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub open: std::collections::BTreeMap<String, bool>,
}

/// One Undo or Redo step, as the buttons show it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryStepView {
    pub label: String,
    /// The registry command that made it.
    pub command: String,
    /// True when an AI tool made the change (spec §5.10).
    #[serde(default)]
    pub ai: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HistoryView {
    #[serde(default)]
    pub undo: Vec<HistoryStepView>,
    #[serde(default)]
    pub redo: Vec<HistoryStepView>,
}

/// A material, as the library and the sheet's settings carry it. The field names are the
/// registry's own (spec §13): the value round-trips into `sheet.adjust`, `materials.save` and
/// `materials.save_current` unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MaterialView {
    pub name: String,
    pub blade: u8,
    pub force: u8,
    pub speed: u8,
    pub passes: u8,
    #[serde(default)]
    pub mirror: bool,
    #[serde(default)]
    pub offset_method: String,
    #[serde(default)]
    pub offset_mm: f64,
    #[serde(default)]
    pub overcut_mm: f64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default)]
    pub roll: bool,
}

/// A cutting mat. The field names are the registry's own, so the value round-trips into
/// `mats.save` unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MatView {
    pub name: String,
    /// The whole mat, `[width, length]` in mm.
    pub outer_mm: [f64; 2],
    /// The sticky, graduated area, `[left, top, right, bottom]` in mm.
    pub sticky_mm: [f64; 4],
    /// Where the printed rulers count from, in mm.
    #[serde(default)]
    pub grid_origin_mm: [f64; 2],
    /// Ruler units along the sticky area's top, right, bottom and left edges.
    #[serde(default)]
    pub rulers: [String; 4],
    /// Where the cutter's start corner falls on this mat, in mm.
    pub cut_origin_mm: [f64; 2],
    /// How the Cameo loads it: `mat12x12`, `mat12x24` or `roll`.
    #[serde(default)]
    pub class: String,
    /// Preset measurements are approximate.
    #[serde(default)]
    pub approximate: bool,
}

/// What a newly opened design starts with.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NewDesignView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mat: Option<String>,
}

/// The open project, as the screens need it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DesignView {
    /// The design's file name, as uploaded.
    pub name: String,
    /// The project's revision: it changes whenever the project changes.
    pub project_revision: u64,
    pub cut_ready: bool,
    /// Questions and blocking problems still to resolve.
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
    /// What Make cut-ready asked, found and changed (spec §12: the issue and info cards).
    #[serde(default)]
    pub prepared: PreparedView,
}

/// The Make cut-ready report as the screens show it: the cards, the rows and the size. The
/// report's own parts ([`PreparedView::changes`], [`PreparedView::diagnostics`]) are carried as
/// they come; splitting them into resolved answers and notes is the screens' job, exactly as
/// CutCraft's window does it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PreparedView {
    /// Questions still to answer, in the report's order.
    #[serde(default)]
    pub questions: Vec<Card>,
    /// Problems that stop the design being cut: the report's blocking diagnostics.
    #[serde(default)]
    pub problems: Vec<Card>,
    /// What Make cut-ready did and the answers they gave, in the report's order.
    #[serde(default)]
    pub changes: Vec<Card>,
    /// The report's non-blocking diagnostics, and its blocking ones again in
    /// [`PreparedView::problems`].
    #[serde(default)]
    pub diagnostics: Vec<Card>,
    /// Answers that came from an AI tool (spec §5.10).
    #[serde(default)]
    pub answered_by_ai: u32,
    /// True when any answer is recorded, so *Clear answers* is available.
    #[serde(default)]
    pub answered_any: bool,
    /// The colours or layers to cut, in the panel's order.
    #[serde(default)]
    pub rows: Vec<RowView>,
    /// How the rows are grouped: `colour` or `layer`.
    #[serde(default)]
    pub group_by: String,
    /// The *Outlines on filled shapes* choice, when the design has such outlines:
    /// `part_of_shape` or `shape`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outlines_on_fills: Option<String>,
    /// The drawn design's size in mm, as resized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_mm: Option<[f64; 2]>,
    /// The resize: mm as resized per mm of the design.
    #[serde(default)]
    pub scale: f64,
    /// Rows they chose not to cut.
    #[serde(default)]
    pub excluded_rows: u32,
}

/// One issue, resolved answer or note, as a card shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Card {
    /// The diagnostic or change code, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The pending question's decision, when this is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    pub message: String,
    /// The allowed answers, as the registry's decision values.
    #[serde(default)]
    pub choices: Vec<Value>,
    /// The source objects the card is about, for *Show me*.
    #[serde(default)]
    pub source_ids: Vec<String>,
}

/// One colour or layer to cut, with the prepared items it covers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RowView {
    /// The row key, as `cut.include` names it.
    pub key: Value,
    pub label: String,
    /// The row's colour as `#rrggbb`, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
    #[serde(default)]
    pub included: bool,
    #[serde(default)]
    pub pieces: u64,
    #[serde(default)]
    pub cut_lines: u64,
    /// The prepared pieces and cut lines it covers, for *Show me* and pointing.
    #[serde(default)]
    pub ids: Vec<String>,
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SheetView {
    pub material: String,
    /// The mat's name, or "Roll".
    pub mat: String,
    /// The current plan is valid to cut.
    pub plan_valid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_s: Option<f64>,
    /// The sheet's cut settings, as the panel edits them. The field names are the registry's
    /// own, so parts round-trip into `sheet.adjust` unchanged.
    #[serde(default)]
    pub settings: MaterialView,
    /// Iron-on is cut mirrored: a sheet setting beside the material's own default.
    #[serde(default)]
    pub mirror: bool,
    /// How the mat loads: `mat12x12`, `mat12x24` or `roll`.
    #[serde(default)]
    pub mat_class: String,
    /// The cut lines' placement: `[x, y, rotation_deg]`, from the cutter's start corner.
    #[serde(default)]
    pub placement: [f64; 3],
    /// Their piece of material, when they cut one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub piece: Option<PieceView>,
    #[serde(default)]
    pub weeding_box: bool,
    #[serde(default)]
    pub weed_lines: bool,
    #[serde(default)]
    pub weed_inverted: bool,
    #[serde(default)]
    pub layering_marks: bool,
    /// Where they placed the test cut themselves, from the start corner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_spot_mm: Option<[f64; 2]>,
    /// How many rows the sheet has.
    #[serde(default)]
    pub rows: u32,
}

/// Their piece of material: its size and where it sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PieceView {
    /// `[width, height]` in mm.
    pub size_mm: [f64; 2],
    /// The top-left corner from the cutter's start corner, in mm.
    #[serde(default)]
    pub at_mm: [f64; 2],
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
    /// The plan's operations in cut order, on the mat, with what each is made from: what the
    /// Cut preview and the cut screen draw (spec §12).
    #[serde(default)]
    pub ops: Vec<PlanOp>,
    /// Their piece of material on the mat, when they cut one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub piece_mm: Option<[f64; 4]>,
    /// The usable cutting area `[width, height]` in mm.
    #[serde(default)]
    pub cutting_area_mm: [f64; 2],
    /// What the plan says about this sheet: warnings and blockers, with their fixes' commands.
    #[serde(default)]
    pub diagnostics: Vec<PlanDiag>,
    /// Iron-on is cut mirrored.
    #[serde(default)]
    pub mirror: bool,
    /// The material the plan cuts.
    #[serde(default)]
    pub material: String,
    /// The design's size after mirror and rotation, in mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub design_size_mm: Option<[f64; 2]>,
    /// The bounds of all blade-down paths, on the mat, in mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds_mm: Option<[f64; 4]>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MatGeometry {
    /// The mat's outer edge: [width, height] in mm.
    pub outer_mm: [f64; 2],
    /// The sticky grid: [x0, y0, x1, y1] in mm.
    pub sticky_mm: [f64; 4],
    /// The cutter's start corner on the mat, in mm. Placement (`sheet.place`) counts from here.
    pub origin_mm: [f64; 2],
    /// Where the printed rulers count from, in mm.
    #[serde(default)]
    pub grid_origin_mm: [f64; 2],
    /// Ruler units along the sticky area's top, right, bottom and left edges.
    #[serde(default)]
    pub rulers: [String; 4],
    /// How the mat loads: `mat12x12`, `mat12x24` or `roll`.
    #[serde(default)]
    pub class: String,
    /// Preset measurements are approximate.
    #[serde(default)]
    pub approximate: bool,
}

/// One polyline, drawn in its colour.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    /// `#rrggbb`.
    pub colour: String,
    /// An aid (weeding box, weed line, layering mark), drawn lighter.
    #[serde(default)]
    pub aid: bool,
    /// Single precision is plenty on screen and halves the transfer.
    pub points: Vec<[f32; 2]>,
    /// The prepared piece or cut line it comes from, for *Show me*.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// The source objects it was made from, for *Show me*.
    #[serde(default)]
    pub sources: Vec<String>,
    /// False when its row is not cut: drawn only while its row is pointed at.
    #[serde(default = "default_true")]
    pub included: bool,
    /// A closed path ends where it starts.
    #[serde(default)]
    pub closed: bool,
}

fn default_true() -> bool {
    true
}

/// One operation of the plan, in cut order, in mat mm from the mat's top-left corner.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanOp {
    /// `hole`, `outline`, `line`, `weeding_box`, `weed_line` or `mark`.
    pub kind: String,
    /// `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
    /// The prepared piece or cut line it comes from, for *Show me*.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// The source objects it was made from, for *Show me*.
    #[serde(default)]
    pub sources: Vec<String>,
    pub points: Vec<[f32; 2]>,
    /// The path the tool follows when it differs: overcut and blade compensation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_path: Option<Vec<[f32; 2]>>,
}

/// What the plan says about one thing on the sheet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanDiag {
    pub code: String,
    /// `blocking` or `info`.
    pub severity: String,
    pub message: String,
    #[serde(default)]
    pub source_ids: Vec<String>,
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
    pub key: String,
    pub incarnation: String,
    pub revision: u64,
}

/// The immutable copy named by queue, loading preview and Start. Its stored revision is 1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CutCopy {
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
        Geometry,
        Picture(PictureRequest),
        /// Open something in place of the session's project: a change, keyed and checked like
        /// any other (spec §7).
        Open(OpenRequest),
        /// The open project as a `.cutcraft` file and its first sheet's plan, for saving.
        /// The plan is best-effort: an unfinished project saves with its invalid plan or none
        /// (spec §11), and cutting still validates at the handoff.
        Export,
        /// The validated execution export (`cut.copy`): refused unless the project is cut-ready.
        ExportValidated,
        /// Replace the stored bytes of the design the session opened with a newer upload and
        /// replay the project's decisions on it (`project.update`, spec §10). The file is
        /// already in the person's area under a name the server chose.
        Update {
            file: String,
        },
        /// The person's fonts changed in their area: read them again (spec §10).
        ReloadFonts,
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
        Command(CommandReply),
        Cancel {
            requested: bool,
        },
        Replay {
            result: Option<CommandReply>,
        },
        Lookup(KeyLookup),
        Snapshot(Box<SessionSnapshot>),
        Geometry(Box<Geometry>),
        /// A PNG, base64-encoded (a JSON line can't carry bytes).
        Picture {
            png_base64: String,
        },
        Export {
            name: String,
            /// The `.cutcraft` file's JSON.
            project: String,
            /// The first sheet's plan, as JSON: none when the project isn't cut-ready.
            plan: Option<String>,
            saved: Option<SavedRef>,
        },
        /// The validated execution export (`cut.copy`), for the cut-copy handoff: its plan is
        /// always checked, so it is always there.
        ExportValidated {
            name: String,
            project: String,
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
        assert_eq!(snapshot.units, Units::Inches);
        assert_eq!(snapshot.screen, Screen::SideBySide);
        assert_eq!(snapshot.font_size, 14.0);
        assert_eq!(snapshot.theme, Theme::System);
    }

    #[test]
    fn the_desktop_view_round_trips() {
        let snapshot = SessionSnapshot {
            protocol: PROTOCOL_VERSION,
            incarnation: "i".into(),
            revision: 7,
            materials: vec![MaterialView {
                name: "Siser EasyWeed".into(),
                blade: 2,
                force: 8,
                speed: 5,
                passes: 2,
                mirror: true,
                offset_method: "off".into(),
                ..Default::default()
            }],
            mats: vec![MatView {
                name: "My mat".into(),
                outer_mm: [330.2, 355.6],
                sticky_mm: [12.0, 12.0, 318.2, 330.2],
                grid_origin_mm: [12.0, 12.0],
                rulers: [
                    "inches".into(),
                    "centimetres".into(),
                    "none".into(),
                    "inches".into(),
                ],
                cut_origin_mm: [12.0, 12.0],
                class: "mat12x12".into(),
                approximate: true,
            }],
            units: Units::Millimetres,
            snap: SnapView {
                enabled: true,
                step_mm: Some(5.0),
            },
            screen: Screen::Mat,
            history: HistoryView {
                undo: vec![HistoryStepView {
                    label: "Moved the design".into(),
                    command: "sheet.place".into(),
                    ai: false,
                }],
                ..Default::default()
            },
            design: Some(DesignView {
                name: "star.svg".into(),
                prepared: PreparedView {
                    questions: vec![Card {
                        decision: Some("background".into()),
                        message: "This design has a background box.".into(),
                        choices: vec![json!({"decision": "background", "cut": true})],
                        ..Default::default()
                    }],
                    changes: vec![Card {
                        code: Some("background.excluded".into()),
                        message: "Background box: not cut.".into(),
                        ..Default::default()
                    }],
                    rows: vec![RowView {
                        key: json!({"colour": {"hex": "#dd2222"}}),
                        label: "#dd2222".into(),
                        colour: Some("#dd2222".into()),
                        pieces: 3,
                        ids: vec!["piece-1".into()],
                        ..Default::default()
                    }],
                    size_mm: Some([80.0, 60.0]),
                    ..Default::default()
                },
                sheet: Some(SheetView {
                    material: "Siser EasyWeed".into(),
                    mat: "My mat".into(),
                    settings: MaterialView {
                        blade: 2,
                        force: 8,
                        ..Default::default()
                    },
                    placement: [10.0, 12.0, 90.0],
                    piece: Some(PieceView {
                        size_mm: [152.4, 152.4],
                        at_mm: [5.0, 5.0],
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let text = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(
            serde_json::from_str::<SessionSnapshot>(&text).unwrap(),
            snapshot
        );
        // The parts commands take keep the registry's own names.
        let material = serde_json::to_value(&snapshot.materials[0]).unwrap();
        assert_eq!(material["force"], json!(8));
        let sheet = serde_json::to_value(snapshot.design.unwrap().sheet.unwrap()).unwrap();
        assert_eq!(sheet["placement"], json!([10.0, 12.0, 90.0]));
    }

    #[test]
    fn geometry_carries_the_plan_and_line_identity() {
        let geometry = Geometry {
            id: "7-plan".into(),
            mat: Some(MatGeometry {
                outer_mm: [330.2, 355.6],
                sticky_mm: [12.0, 12.0, 318.2, 330.2],
                origin_mm: [12.0, 12.0],
                rulers: ["inches".into(), "none".into(), "none".into(), "none".into()],
                class: "mat12x12".into(),
                ..Default::default()
            }),
            ops: vec![PlanOp {
                kind: "outline".into(),
                colour: Some("#dd2222".into()),
                item: Some("piece-1".into()),
                sources: vec!["svg-path-3".into()],
                points: vec![[1.0, 2.0], [3.0, 4.0]],
                tool_path: Some(vec![[1.0, 2.0], [3.0, 4.0]]),
            }],
            diagnostics: vec![PlanDiag {
                code: "plan.off_mat_grid".into(),
                severity: "info".into(),
                message: "Part of the cut is off the sticky grid.".into(),
                ..Default::default()
            }],
            lines: vec![Stroke {
                colour: "#dd2222".into(),
                points: vec![[0.0, 0.0], [1.0, 1.0]],
                item: Some("piece-1".into()),
                closed: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let text = serde_json::to_string(&geometry).unwrap();
        assert_eq!(serde_json::from_str::<Geometry>(&text).unwrap(), geometry);
        // An old client's stroke, without the identity fields, still reads.
        let old = serde_json::from_value::<Stroke>(json!({
            "colour": "#000000", "points": [[0.0, 0.0]]
        }))
        .unwrap();
        assert!(old.included && old.sources.is_empty());
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
