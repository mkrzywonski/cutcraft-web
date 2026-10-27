//! The command registry (spec §6.2.1): every user-visible action as a command with an id, label,
//! parameter schema generated from its Rust type, and an authority class. The window, MCP and
//! `cutcraft-cli run` all go through [`Controller::execute`]. Automation can do anything the
//! person can (spec §6.2.2): the origin is recorded, not used to refuse. It checks optional file
//! roots (spec §6.2.7) and `expect_revision`, and reports the revision after the command.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use cutcraft_import::job::{
    Decision, GroupBy, Mat, Material, Placement, ProjectFile, RowKey, Severity, Sheet,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::cutting::{JobKind, LoadMedia, SimClock};
use crate::history::{HistoryStep, JournalEntry, MAX_JOURNAL, redact};
use crate::{
    Action, ActionOutput, Answered, Controller, FrameImage, HelpTopic, McpSettings, OpenDesign,
    Screen, answers_same_question, settings,
};

/// What a command may do, which decides who may run it (spec §6.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Authority {
    Read,
    Edit,
    FileRead,
    FileWrite,
    DeviceQuery,
    DeviceStop,
    /// Moves the cutter: cut, test cut, pause/resume, unload.
    Motion,
}

/// Who asked. Everything except the person's own input is automation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    User,
    Mcp,
    CliRun,
    /// The control protocol (spec §6.2.6), including input it injects into the window.
    Control,
}

/// Whether automation may run a command of this class: `allowed`, or `roots` when the session
/// was started with an optional automation folder for it (spec §6.2.2, §6.2.7).
pub fn automation_allowed(authority: Authority) -> &'static str {
    match authority {
        Authority::FileRead | Authority::FileWrite => "roots",
        _ => "allowed",
    }
}

/// Optional folders that limit automation's file reads and writes (spec §6.2.7). A root left
/// out leaves that access as wide as the person's own.
#[derive(Clone, Debug, Default)]
pub struct Roots {
    read: Option<PathBuf>,
    write: Option<PathBuf>,
}

impl Roots {
    /// Each root must be an existing folder; links are followed.
    pub fn new(read: Option<&Path>, write: Option<&Path>) -> Result<Self, String> {
        let folder = |path: Option<&Path>, what: &str| -> Result<Option<PathBuf>, String> {
            path.map(|p| {
                let canonical = std::fs::canonicalize(p)
                    .map_err(|e| format!("The automation {what} root {}: {e}", p.display()))?;
                if !canonical.is_dir() {
                    return Err(format!(
                        "The automation {what} root {} is not a folder.",
                        p.display()
                    ));
                }
                Ok(canonical)
            })
            .transpose()
        };
        Ok(Self {
            read: folder(read, "read")?,
            write: folder(write, "write")?,
        })
    }
}

/// A command's entry in `command_list` (spec §6.2.4).
#[derive(Clone, Debug, Serialize)]
pub struct CommandInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub menu: Option<&'static str>,
    pub shortcut: Option<&'static str>,
    pub authority: Authority,
    pub automation: &'static str,
    pub params: Value,
    pub result: Value,
    pub enabled: bool,
    #[serde(rename = "disabledReason")]
    pub disabled_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CommandError {
    /// Stable: `unknown-command`, `invalid-params`, `refused`, `outside-roots`,
    /// `revision-mismatch`, `disabled`, `busy` (a sheet is being cut), `not-implemented`,
    /// `failed`.
    pub code: &'static str,
    pub message: String,
}

impl CommandError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    fn failed(message: impl Into<String>) -> Self {
        Self::new("failed", message)
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl std::error::Error for CommandError {}

pub struct CommandOutput {
    pub value: Value,
    pub image: Option<FrameImage>,
    /// The application revision after the command.
    pub revision: u64,
}

/// What a command needs before it can run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Needs {
    Nothing,
    Design,
    Window,
}

struct Spec {
    id: &'static str,
    label: &'static str,
    description: &'static str,
    menu: Option<&'static str>,
    shortcut: Option<&'static str>,
    authority: Authority,
    needs: Needs,
    params: fn() -> Value,
}

fn schema<T: JsonSchema>() -> Value {
    let mut value = serde_json::to_value(schemars::schema_for!(T)).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
    value
}

// Parameters. Unknown keys are rejected at every level (spec §6.2.1).

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoParams {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OpenParams {
    /// Absolute path of a local design (SVG, SVGZ, PDF, AI, EPS, ASCII DXF, PNG, JPEG, GIF, BMP or PSD).
    path: String,
    /// Answers to apply while preparing it, as a recipe saved earlier.
    recipe: Option<cutcraft_import::job::Recipe>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProjectOpenParams {
    /// Absolute path of a `.cutcraft` project.
    path: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SaveParams {
    /// Absolute path for the `.cutcraft` project.
    path: String,
    /// Replace an existing file at `path`.
    #[serde(default)]
    overwrite: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DecideParams {
    /// One answer, normally copied from a choice listed under a pending question.
    decision: Decision,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IncludeParams {
    /// The row, as `rows.list` reports it.
    row: RowKey,
    /// False is *Don't cut*.
    include: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SheetSetParams {
    /// A material name from `materials.list` (a unique start of one is enough).
    material: Option<String>,
    /// Mirror the cut. Choosing an iron-on material turns it on.
    mirror: Option<bool>,
    mat: Option<Mat>,
    weeding_box: Option<bool>,
    /// A mat from `mats.list` by name (a unique start is enough): shown in the Cut preview, and
    /// the sheet loads at its length.
    mat_profile: Option<String>,
    /// Lines through the waste vinyl, clear of the artwork.
    weed_lines: Option<bool>,
    /// Invert: keep the background and weed the shapes (a stencil), so weed lines run through
    /// the shapes instead.
    weed_inverted: Option<bool>,
    /// Marks at the same place on every sheet that has them, for lining up layers.
    layering_marks: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlaceParams {
    /// Distance of the design's left edge from the start corner, in mm.
    x: Option<f64>,
    /// Distance of the design's top edge from the start corner, in mm.
    y: Option<f64>,
    /// Clockwise rotation in degrees.
    rotate: Option<f64>,
    /// *Move to corner*: place the design at the start corner, inside the margins.
    #[serde(default)]
    corner: bool,
    /// *Move onto the grid* / *Move it inside*: the smallest move that brings every cut inside
    /// the cutting area and, on a mat, its sticky grid.
    #[serde(default)]
    onto_grid: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PieceParams {
    /// Width of their piece of material, in mm. Give both sizes, or neither to forget the piece
    /// (unless only moving it).
    width_mm: Option<f64>,
    /// Height (along the feed) of their piece, in mm.
    height_mm: Option<f64>,
    /// Where the piece's left edge is, mm from the cutter's start corner (keeps the size).
    x_mm: Option<f64>,
    /// Where the piece's top edge is, mm from the cutter's start corner.
    y_mm: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TestSpotParams {
    /// The test cut's left edge on the mat, in mm. Give x and y, or neither to place it
    /// automatically again.
    x_mm: Option<f64>,
    /// The test cut's top edge on the mat, in mm.
    y_mm: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateParams {
    /// Where the design is now, when it moved. Defaults to where it was opened from.
    path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ViewParams {
    screen: Screen,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
enum ExportKind {
    /// The Cut Preview as SVG.
    PreviewSvg,
    /// The Cut Preview as a 1024-pixel PNG.
    PreviewPng,
    /// The encoded GP-GL bytes, for inspection only; CutCraft never sends a file.
    CutterFile,
    /// The cut plan as JSON.
    Plan,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExportParams {
    kind: ExportKind,
    /// Absolute output path. Without it the content is returned.
    path: Option<String>,
    #[serde(default)]
    overwrite: bool,
}

#[derive(Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
enum PreviewView {
    /// The sheet as it will be cut.
    #[default]
    Sheet,
    /// The sheet with the blade's tool-up travel between cuts.
    ToolMotion,
    /// Every colour of the design together, as they fit (sheet is ignored).
    Assembly,
    /// The original design as opened, before Make cut-ready (sheet is ignored).
    Source,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenderParams {
    #[serde(default)]
    view: PreviewView,
    /// Longest side in pixels, 64 to 2048 (default 1024).
    #[schemars(range(min = 64, max = 2048))]
    max_side: Option<u32>,
    /// Scrub the cut order, 0 to 1, as the play slider does: cuts so far solid, the rest faint.
    #[schemars(range(min = 0.0, max = 1.0))]
    order: Option<f64>,
    /// Piece or cut-line ids to highlight, as *Show me* does.
    #[serde(default)]
    highlight: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct HelpParams {
    #[serde(default)]
    topic: HelpTopic,
    /// Open (default) or close the window.
    visible: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CaptureParams {
    /// Absolute path to also save the PNG to.
    save_to: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct McpSettingsParams {
    enabled: Option<bool>,
    #[schemars(range(min = 1))]
    port: Option<u16>,
    /// 127.0.0.1 (this computer only) or one of this computer's network addresses (which always
    /// needs an access key).
    address: Option<String>,
    /// AI tools must send an access key (Authorization: Bearer …).
    require_key: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct McpKeyParams {
    /// The key's name, e.g. claude, codex or grok.
    name: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct McpPortParams {
    /// The address to check (default: the current one).
    address: Option<String>,
    /// The port to check (default: the current one).
    #[schemars(range(min = 1))]
    port: Option<u16>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StepsParams {
    /// How many steps (default 1).
    #[schemars(range(min = 1))]
    steps: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct JournalParams {
    /// The latest this many entries (default 100).
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ConnectParams {
    /// simulated (the default in development builds), cameo (the real cutter over USB) or
    /// remote (a cutter on a paired cameo-pi).
    device: Option<crate::cutting::DeviceKind>,
    /// remote: the paired cameo-pi's address (default: the only one paired).
    station: Option<String>,
    /// remote: the cutter's id on that Pi, from station.list (default: its only cutter).
    cutter: Option<String>,
    /// Simulator scenario: normal (default), no-media, unload-mid-cut, disconnect-mid-cut,
    /// power-off-mid-cut, timeout-after-delivery, stall, slow-status, stale-status or slow-full-buffer.
    scenario: Option<String>,
    /// realtime (the window's default) or virtual (headless default: as fast as it runs).
    clock: Option<SimClock>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WaitParams {
    /// Longest wait in seconds, 1 to 600 (default 60).
    #[schemars(range(min = 1, max = 600))]
    timeout_s: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LoadParams {
    /// What the simulated Cameo's Load button loads; none unloads.
    media: LoadMedia,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StationAddressParams {
    /// The Pi's name or address, as typed: cameo-pi.local, 10.0.0.5, pi.lan:8443.
    address: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StationLibraryParams {
    /// The paired cameo-pi's address (default: the only one paired).
    station: Option<String>,
    /// The folder, from the library's top (default: the top).
    #[serde(default)]
    folder: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StationOpenParams {
    /// The paired cameo-pi's address (default: the only one paired).
    station: Option<String>,
    /// The job's id, from station.library.
    job_id: String,
    /// A revision (default: the job's current one).
    revision: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SendParams {
    /// The paired cameo-pi's address (default: the connected one, else the only one paired).
    station: Option<String>,
    /// The library folder, from the top (default: CutCraft). Missing folders are made.
    folder: Option<Vec<String>>,
    /// The job's name (default: the design's file name).
    name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TestCutParams {
    /// Cut at the start corner of a separate scrap instead of beside the design.
    #[serde(default)]
    on_scrap: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SizeParams {
    /// New design width in mm; the height follows.
    width_mm: Option<f64>,
    /// New design height in mm; the width follows.
    height_mm: Option<f64>,
    /// Or a factor of the imported size (1 is the original size).
    scale: Option<f64>,
    /// Or *Fit*: the largest size whose cuts fit where they can go (their piece when they gave its
    /// size, otherwise the mat's sticky grid or the roll's width) inside the margins, moved to
    /// that area's start corner.
    #[serde(default)]
    fit: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GroupParams {
    /// Form rows by colour or by layer.
    by: GroupBy,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AdjustParams {
    /// Blade force, 1 to 33.
    #[schemars(range(min = 1, max = 33))]
    force: Option<u8>,
    /// Speed, 1 to 10.
    #[schemars(range(min = 1, max = 10))]
    speed: Option<u8>,
    /// Cut once, twice or 3 times.
    #[schemars(range(min = 1, max = 3))]
    passes: Option<u8>,
    /// The ratchet blade number to set by hand, 1 to 10.
    #[schemars(range(min = 1, max = 10))]
    blade: Option<u8>,
    /// Mirror the cut (saved as the material default).
    mirror: Option<bool>,
    /// Blade-offset compensation: off, software or machine.
    offset_method: Option<cutcraft_import::job::OffsetMethod>,
    /// The blade offset in mm, 0 to 1.
    offset_mm: Option<f64>,
    /// Overcut of closed contours in mm, 0 to 2.
    overcut_mm: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MaterialSaveCurrentParams {
    /// Omit to save over the current material. A new name saves a copy and selects it.
    name: Option<String>,
    /// Optional current editor values to apply and save in the same command.
    material: Option<Material>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MaterialSaveParams {
    /// The material to save.
    material: Material,
    /// Replace the material of the same name.
    #[serde(default)]
    replace: bool,
    /// Edit this library material, renaming it to `material.name` (the new name must be free).
    rename_from: Option<String>,
    /// Make it the material new designs start with (true), or stop (false).
    #[serde(default)]
    default: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UnitsParams {
    /// inches or millimetres.
    units: crate::Units,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FontSizeParams {
    /// Base interface font size in points, from 10 to 24; default 14.
    points: f32,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NewDesignParams {
    /// A material name from `materials.list` (a unique start is enough); "" goes back to the
    /// material last used. Left out, it is unchanged.
    #[serde(default)]
    material: Option<String>,
    /// A mat name from `mats.list` (a unique start is enough); "" goes back to the 12 × 12 in
    /// mat. Left out, it is unchanged.
    #[serde(default)]
    mat: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PanelParams {
    /// Group titles in the order to show them: Issues, Info, Artwork size, What to cut,
    /// Material, Mat, Media, Cut lines, Cutting aids, Estimate.
    #[serde(default)]
    order: Option<Vec<String>>,
    /// The groups locked above the scrolling part.
    #[serde(default)]
    locked: Option<Vec<String>>,
    /// Groups to show open (true) or closed (false); groups left out keep their usual state.
    /// Replaces the remembered set; `{}` forgets it.
    #[serde(default)]
    open: Option<std::collections::BTreeMap<String, bool>>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SnapParams {
    /// Snap drags on the Cut preview to the grid.
    #[serde(default)]
    enabled: Option<bool>,
    /// The grid step in mm, 0.1 to 100 (for example 6.35 for ¼ in, or 5).
    #[serde(default)]
    step_mm: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MatSaveParams {
    /// The mat: its outer size, sticky graduated area, ruler units, where the cutter's start
    /// corner falls on it, and the length it loads at.
    mat: cutcraft_import::job::MatProfile,
    /// Edit this mat (renaming it to `mat.name`); without it a new mat is added.
    rename_from: Option<String>,
    /// Make it the mat new designs start on (true), or stop (false).
    #[serde(default)]
    default: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MaterialDeleteParams {
    name: String,
}

macro_rules! spec {
    ($id:literal, $label:literal, $menu:expr, $shortcut:expr, $authority:ident, $needs:ident, $params:ty, $description:literal) => {
        Spec {
            id: $id,
            label: $label,
            description: $description,
            menu: $menu,
            shortcut: $shortcut,
            authority: Authority::$authority,
            needs: Needs::$needs,
            params: schema::<$params>,
        }
    };
}

fn registry() -> Vec<Spec> {
    vec![
        spec!(
            "app.status",
            "Status",
            None,
            None,
            Read,
            Nothing,
            NoParams,
            "The app's state: open design, loading, last error, MCP service."
        ),
        spec!(
            "design.open",
            "Open a design…",
            Some("File"),
            Some("Cmd+O"),
            FileRead,
            Nothing,
            OpenParams,
            "Import a design and run Make cut-ready. Returns the import report."
        ),
        spec!(
            "design.close",
            "Close design",
            Some("File"),
            Some("Cmd+W"),
            Edit,
            Nothing,
            NoParams,
            "Close the design and supersede any pending import."
        ),
        spec!(
            "design.inspection",
            "Import report",
            None,
            None,
            Read,
            Design,
            NoParams,
            "The full import report: pages, objects, fonts, scale and appearance diagnostics."
        ),
        spec!(
            "project.open",
            "Open a project…",
            Some("File"),
            Some("Cmd+Shift+O"),
            FileRead,
            Nothing,
            ProjectOpenParams,
            "Open a .cutcraft project from its embedded design and saved sheet settings; needs neither the original file nor its fonts. A project saved with several sheets opens as one sheet with every row."
        ),
        spec!(
            "project.save",
            "Save project…",
            Some("File"),
            Some("Cmd+S"),
            FileWrite,
            Design,
            SaveParams,
            "Save the design, its answers, what to cut, the material and placement as a .cutcraft project. Never writes over the design file."
        ),
        spec!(
            "project.update",
            "Update from design",
            Some("File"),
            None,
            FileRead,
            Design,
            UpdateParams,
            "Read the design file again (or where it moved), prepare it with the same answers, and keep the sheet settings and unticked rows that still exist. Lists rows added and removed; Undo goes back."
        ),
        spec!(
            "project.inspect",
            "Project summary",
            None,
            None,
            Read,
            Design,
            NoParams,
            "Source, size, readiness, rows, the sheet with its plan validity, and the revision."
        ),
        spec!(
            "prepare.report",
            "Make cut-ready report",
            None,
            None,
            Read,
            Design,
            NoParams,
            "Pieces, cut lines, the change list, pending questions with their allowed answers, and blockers."
        ),
        spec!(
            "prepare.decide",
            "Answer a question",
            None,
            None,
            Edit,
            Design,
            DecideParams,
            "Answer one pending question and prepare again from the original. Replaces an earlier answer to the same question; a decision that does not apply is rejected and the previous result kept."
        ),
        spec!(
            "prepare.run",
            "Make cut-ready",
            Some("Design"),
            None,
            Edit,
            Design,
            NoParams,
            "Run Make cut-ready again from the imported original with the current answers."
        ),
        spec!(
            "view.show",
            "Show screen",
            Some("View"),
            None,
            Edit,
            Window,
            ViewParams,
            "Show a view of the window: side-by-side (artwork and cut lines), overlay, mat (the Cut Preview) or cut (the cutting steps)."
        ),
        spec!(
            "prepare.reset",
            "Clear answers",
            None,
            None,
            Edit,
            Design,
            NoParams,
            "Remove every answer and prepare again."
        ),
        spec!(
            "rows.list",
            "Rows",
            None,
            None,
            Read,
            Design,
            NoParams,
            "The choose-what-to-cut rows: colour or layer, piece and line counts, and whether each is cut. Every row is cut unless unticked."
        ),
        spec!(
            "cut.include",
            "Cut / Don't cut",
            Some("Sheet"),
            None,
            Edit,
            Design,
            IncludeParams,
            "Tick or untick a row."
        ),
        spec!(
            "sheet.set",
            "Sheet settings",
            Some("Sheet"),
            None,
            Edit,
            Design,
            SheetSetParams,
            "Set the sheet's material, mirror, mat, weeding box, weed lines or layering marks."
        ),
        spec!(
            "sheet.place",
            "Place on mat",
            Some("Sheet"),
            None,
            Edit,
            Design,
            PlaceParams,
            "Move or rotate the design on the mat."
        ),
        spec!(
            "sheet.test_spot",
            "Move test cut",
            Some("Sheet"),
            None,
            Edit,
            Design,
            TestSpotParams,
            "Put the test cut's 1 in square anywhere on the mat (its top-left corner in mm), even in a gap inside the design's box, as long as it misses the artwork itself; give neither x nor y to place it automatically again."
        ),
        spec!(
            "sheet.piece",
            "How big is your piece?",
            Some("Sheet"),
            None,
            Edit,
            Design,
            PieceParams,
            "Say how big their piece of material is (laid at the mat's start corner) so the preview outlines it and any cut off the piece blocks the cut; give neither size to forget it."
        ),
        spec!(
            "materials.list",
            "Materials",
            None,
            None,
            Read,
            Nothing,
            NoParams,
            "Saved material presets."
        ),
        spec!(
            "plan.sheet",
            "Plan the sheet",
            None,
            None,
            Read,
            Design,
            NoParams,
            "Plan the sheet: plan id, validity, bounds, size, cut and travel length, estimate, cut counts and diagnostics. Nothing is sent to a cutter."
        ),
        spec!(
            "export",
            "Export…",
            Some("File"),
            Some("Cmd+E"),
            FileWrite,
            Design,
            ExportParams,
            "Export the Cut Preview SVG, cutter file or plan, to a path or returned in the result."
        ),
        spec!(
            "render.preview",
            "Cut Preview picture",
            None,
            None,
            Read,
            Design,
            RenderParams,
            "Render the Cut Preview as a PNG from the same plan as the window, with a legend: scale in px/mm, origin and colours. order scrubs the cut order; highlight marks pieces or cut lines; view tool-motion adds blade-up travel."
        ),
        spec!(
            "history.undo",
            "Undo",
            Some("Edit"),
            Some("Cmd+Z"),
            Edit,
            Design,
            StepsParams,
            "Undo the last change to the design (an answer, a sheet setting); each step names the command and who made it."
        ),
        spec!(
            "history.redo",
            "Redo",
            Some("Edit"),
            Some("Cmd+Shift+Z"),
            Edit,
            Design,
            StepsParams,
            "Redo the last undone change."
        ),
        spec!(
            "journal.list",
            "Session journal",
            None,
            None,
            Read,
            Nothing,
            JournalParams,
            "What was run this session: command, parameters (file paths as names only), origin, revisions and outcome. Answers what an assistant changed."
        ),
        spec!(
            "help.show",
            "Help",
            Some("Help"),
            None,
            Edit,
            Window,
            HelpParams,
            "Open or close a help window and return its guide. Topics: AI setup, saving designs, troubleshooting, About, or the GitHub user manual (opens in the desktop browser)."
        ),
        spec!(
            "view.inspect",
            "Inspect window",
            None,
            None,
            Read,
            Window,
            NoParams,
            "What the window shows: the screen, an open dialog with its choices, open windows, focus, size, running actions and the last message."
        ),
        spec!(
            "view.capture",
            "Capture window",
            None,
            None,
            Read,
            Window,
            CaptureParams,
            "The rendered CutCraft window as a PNG, optionally also saved to a path."
        ),
        spec!(
            "settings.mcp",
            "AI connection settings",
            Some("Settings"),
            None,
            Edit,
            Window,
            McpSettingsParams,
            "Read the AI connection (MCP) settings; in the window, change enabled, address, port or whether an access key is required. AI tools can read but not change them."
        ),
        spec!(
            "settings.mcp_key_create",
            "New access key",
            Some("Settings"),
            None,
            Edit,
            Nothing,
            McpKeyParams,
            "Make a named access key for an AI tool and return it once (only its fingerprint is kept). Window only: AI tools cannot make keys."
        ),
        spec!(
            "settings.mcp_key_delete",
            "Delete access key",
            Some("Settings"),
            None,
            Edit,
            Nothing,
            McpKeyParams,
            "Delete a named access key; the tool using it is refused from its next request. Window only."
        ),
        spec!(
            "settings.mcp_port",
            "Check AI connection port",
            Some("Settings"),
            None,
            Read,
            Nothing,
            McpPortParams,
            "Who uses an address and port: free, this window, another CutCraft window, or another program."
        ),
        spec!(
            "app.engine_check",
            "Check engine",
            Some("Help"),
            None,
            Read,
            Nothing,
            NoParams,
            "Run the pinned engine's authored one-inch SVG smoke check."
        ),
        spec!(
            "device.discover",
            "Find cutters",
            Some("Device"),
            None,
            Read,
            Nothing,
            NoParams,
            "List candidate cutter USB descriptors. Opens nothing and sends no commands."
        ),
        spec!(
            "design.size",
            "Design size",
            Some("Arrange"),
            None,
            Edit,
            Design,
            SizeParams,
            "Resize the whole design with linked proportions, by width, height or factor. Every colour changes together, so they still fit."
        ),
        spec!(
            "rows.group",
            "Group by",
            Some("Arrange"),
            None,
            Edit,
            Design,
            GroupParams,
            "Group the rows by colour or by layer. A new row is unticked only when everything in it was."
        ),
        spec!(
            "sheet.adjust",
            "Adjust material settings",
            Some("Sheet"),
            None,
            Edit,
            Design,
            AdjustParams,
            "Change the sheet's force, speed, repeats or blade number. A changed force or speed is provisional until a test cut."
        ),
        spec!(
            "materials.save",
            "Save material",
            Some("Materials"),
            None,
            Edit,
            Nothing,
            MaterialSaveParams,
            "Add or edit a library material. Only this explicit save changes the material library."
        ),
        spec!(
            "settings.units",
            "Units",
            Some("View"),
            None,
            Edit,
            Nothing,
            UnitsParams,
            "Show and type lengths in inches or millimetres in the window. Commands always take mm."
        ),
        spec!(
            "settings.font_size",
            "Interface font size",
            Some("View"),
            None,
            Edit,
            Nothing,
            FontSizeParams,
            "Set the saved font size for menus, panels and dialogs, from 10 to 24 points (default 14)."
        ),
        spec!(
            "settings.snap",
            "Snap to grid",
            Some("View"),
            None,
            Edit,
            Nothing,
            SnapParams,
            "Snap the design, material and test-cut spot to a grid on the mat's rulers when they are dragged on the Cut preview, and set the grid step."
        ),
        spec!(
            "settings.panel",
            "Arrange the panel",
            Some("View"),
            None,
            Edit,
            Nothing,
            PanelParams,
            "Order the groups of the window's right panel, lock some above the scrolling part so they stay in view, and choose which are open. Groups not listed follow in their usual order and open state."
        ),
        spec!(
            "settings.new_design",
            "Starting material and mat",
            None,
            None,
            Edit,
            Nothing,
            NewDesignParams,
            "Choose the material and mat every newly opened design starts with (CutCraft projects keep their own). Without one, a design starts with the material last used and the 12 × 12 in mat."
        ),
        spec!(
            "mats.list",
            "Mats",
            None,
            None,
            Read,
            Nothing,
            NoParams,
            "Their mats: outer size, sticky graduated area, ruler units per edge, where the cutter's start corner falls, and load length. Presets are approximate until measured."
        ),
        spec!(
            "mats.save",
            "Save mat",
            Some("Materials"),
            None,
            Edit,
            Nothing,
            MatSaveParams,
            "Add a mat, or edit one (rename_from) — e.g. a third-party mat with inch rulers on two edges and cm on the others and its own margins."
        ),
        spec!(
            "mats.delete",
            "Delete mat",
            Some("Materials"),
            None,
            Edit,
            Nothing,
            MaterialDeleteParams,
            "Remove a mat from the list. A project keeps its own copy."
        ),
        spec!(
            "materials.save_current",
            "Save material settings",
            None,
            None,
            Edit,
            Design,
            MaterialSaveCurrentParams,
            "Save the current sheet's material settings to the library. Omit name for Save; supply a new unused name for Save As. Unsaved settings otherwise remain in this session."
        ),
        spec!(
            "materials.delete",
            "Delete material",
            Some("Materials"),
            None,
            Edit,
            Nothing,
            MaterialDeleteParams,
            "Remove a material from the library. Projects keep their own copies."
        ),
        spec!(
            "device.connect",
            "Connect cutter",
            Some("Device"),
            None,
            DeviceQuery,
            Nothing,
            ConnectParams,
            "Connect a cutter: the real Cameo over USB (read-only status and firmware check on connecting; it cuts from a mat, and from a roll only with CUTCRAFT_HARDWARE_TESTING=1), or the Simulated Cameo (development builds) with an optional fault scenario."
        ),
        spec!(
            "device.disconnect",
            "Disconnect cutter",
            Some("Device"),
            None,
            DeviceQuery,
            Nothing,
            NoParams,
            "Disconnect the cutter. Not while cutting."
        ),
        spec!(
            "device.status",
            "Cutter status",
            None,
            None,
            Read,
            Nothing,
            NoParams,
            "The cutting state, the job and its loading steps, estimated progress, and whether the sheet was cut or interrupted. Sends nothing to the cutter."
        ),
        spec!(
            "device.wait",
            "Wait for the cutter",
            None,
            None,
            Read,
            Nothing,
            WaitParams,
            "Wait until the running cut ends (completed, stopped or interrupted) or is paused, or the timeout passes; returns device.status."
        ),
        spec!(
            "device.load",
            "Load / unload (simulated)",
            Some("Device"),
            None,
            Motion,
            Nothing,
            LoadParams,
            "Press the Simulated Cameo's Load Mat or Unload button."
        ),
        spec!(
            "cut.copy",
            "Prepare cut copy",
            None,
            None,
            Read,
            Design,
            NoParams,
            "An immutable current-sheet project and plan, without queuing or moving a cutter."
        ),
        spec!(
            "cut.sheet",
            "Cut",
            Some("Cut"),
            None,
            Motion,
            Design,
            NoParams,
            "Cut the sheet: it is planned, checked and encoded, and the loading steps are returned. Nothing moves until cut.start. Also Cut again."
        ),
        spec!(
            "cut.test",
            "Test cut",
            Some("Cut"),
            None,
            Motion,
            Nothing,
            TestCutParams,
            "Choose a test cut (a triangle inside a 1 in square) with the sheet's material, placed clear of the design, or at the corner of a scrap. Nothing moves until cut.start."
        ),
        spec!(
            "cut.start",
            "Start cutting",
            Some("Cut"),
            None,
            Motion,
            Nothing,
            NoParams,
            "The material is loaded: check the plan is still current, then cut. Progress is estimated; follow it with device.status."
        ),
        spec!(
            "cut.pause",
            "Pause",
            Some("Cut"),
            None,
            Motion,
            Nothing,
            NoParams,
            "Pause at the end of the current cut, with a blade-up request."
        ),
        spec!(
            "cut.resume",
            "Resume",
            Some("Cut"),
            None,
            Motion,
            Nothing,
            NoParams,
            "Resume a paused cut."
        ),
        spec!(
            "cut.stop",
            "Stop",
            Some("Cut"),
            Some("Escape"),
            DeviceStop,
            Nothing,
            NoParams,
            "Stop now: nothing more is sent, the cutter may finish buffered moves, and the sheet is abandoned. Also cancels a cut waiting for loading."
        ),
        spec!(
            "cut.exit",
            "Exit cut screen",
            None,
            None,
            Edit,
            Nothing,
            NoParams,
            "Return to Cut Preview, cancelling any reviewed job without sending commands. Disabled while a cut is running; retains completed cuts and occupied test areas."
        ),
        spec!(
            "cut.dismiss",
            "Done",
            Some("Cut"),
            None,
            Edit,
            Nothing,
            NoParams,
            "Close a finished, stopped or interrupted cut and go back to choosing what to cut."
        ),
        spec!(
            "cut.reset",
            "Start over",
            Some("Cut"),
            None,
            Edit,
            Nothing,
            NoParams,
            "Forget that the sheet was cut (or interrupted), here and in the saved project."
        ),
        spec!(
            "station.pair",
            "Add cameo-pi…",
            None,
            None,
            DeviceQuery,
            Nothing,
            StationAddressParams,
            "Ask a cameo-pi (network cut server) for access. Returns the code its web page shows; someone allows it there, and stations.pairing in app.status follows the answer. The Pi's certificate is pinned from here on."
        ),
        spec!(
            "station.discover",
            "Find cameo-pis",
            None,
            None,
            DeviceQuery,
            Nothing,
            NoParams,
            "Look for cameo-pis on the network (Bonjour, about 3 s): each one's name, the address to pair with, and whether it's paired already."
        ),
        spec!(
            "station.list",
            "cameo-pi cutters",
            None,
            None,
            DeviceQuery,
            Nothing,
            NoParams,
            "Look at every paired cameo-pi: its version and its cutters (ids for device.connect), or why it can't be reached."
        ),
        spec!(
            "station.forget",
            "Forget cameo-pi",
            None,
            None,
            Edit,
            Nothing,
            StationAddressParams,
            "Forget a paired cameo-pi and its key, or cancel a pairing request to it."
        ),
        spec!(
            "station.library",
            "cameo-pi library",
            None,
            None,
            DeviceQuery,
            Nothing,
            StationLibraryParams,
            "One folder of a paired cameo-pi's job library: its folders and jobs."
        ),
        spec!(
            "station.open",
            "Open From cameo-pi…",
            None,
            None,
            DeviceQuery,
            Nothing,
            StationOpenParams,
            "Open a job's CutCraft project from a cameo-pi's library, to prepare it again or change it. It's saved in the settings folder's cameo-pi-projects; cutting or sending it adds a new revision of that job."
        ),
        spec!(
            "cut.confirm_fresh",
            "It has fresh material",
            None,
            None,
            DeviceQuery,
            Nothing,
            NoParams,
            "On a cameo-pi cutter whose loaded mat the Pi didn't see going in (device.status mat_unconfirmed): confirm it has fresh material, so it can be cut on. Only when the person says so; a mat that was cut on is refused."
        ),
        spec!(
            "cut.send",
            "Send to cameo-pi…",
            None,
            None,
            DeviceQuery,
            Design,
            SendParams,
            "Store the prepared sheet (the project and its cut plan) in a cameo-pi's library without cutting. Sending a design that's already there by that name adds a new revision of it."
        ),
    ]
}

/// Offset and overcut within their ranges; machine or software compensation needs an offset.
fn check_blade_values(material: &Material) -> Result<(), CommandError> {
    use cutcraft_import::job::{OffsetMethod, project::OFFSET_RANGE_MM, project::OVERCUT_RANGE_MM};
    let within = |v: f64, (low, high): (f64, f64)| v.is_finite() && (low..=high).contains(&v);
    if !within(material.offset_mm, OFFSET_RANGE_MM)
        || !within(material.overcut_mm, OVERCUT_RANGE_MM)
    {
        return Err(CommandError::new(
            "invalid-params",
            "The blade offset is 0 to 1 mm and the overcut 0 to 2 mm.",
        ));
    }
    if material.offset_method != OffsetMethod::Off && material.offset_mm <= 0.0 {
        return Err(CommandError::new(
            "invalid-params",
            "Blade offset compensation needs an offset above 0 mm.",
        ));
    }
    Ok(())
}

fn decode<T: DeserializeOwned>(params: Value) -> Result<T, CommandError> {
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params).map_err(|e| CommandError::new("invalid-params", e.to_string()))
}

impl Controller {
    /// The command catalogue with live enablement.
    pub fn commands(&self) -> Vec<CommandInfo> {
        let snapshot = self.snapshot().ok();
        let has_design = snapshot.as_ref().is_some_and(|s| s.design.is_some());
        registry()
            .into_iter()
            .map(|spec| {
                let reason = match spec.needs {
                    Needs::Design if !has_design => Some("Open a design first.".to_string()),
                    Needs::Window if !self.0.live => {
                        Some("Only the CutCraft window has this; this session is headless.".into())
                    }
                    _ => None,
                };
                CommandInfo {
                    id: spec.id,
                    label: spec.label,
                    description: spec.description,
                    menu: spec.menu,
                    shortcut: spec.shortcut,
                    authority: spec.authority,
                    automation: automation_allowed(spec.authority),
                    params: (spec.params)(),
                    result: result_schema(spec.id),
                    enabled: reason.is_none(),
                    disabled_reason: reason,
                }
            })
            .collect()
    }

    /// Run one command for `origin` (spec §6.2.1, §6.2.2).
    pub fn execute(
        &self,
        origin: Origin,
        id: &str,
        params: Value,
        expect_revision: Option<u64>,
    ) -> Result<CommandOutput, CommandError> {
        let spec = registry().into_iter().find(|s| s.id == id).ok_or_else(|| {
            CommandError::new("unknown-command", format!("There is no command {id}."))
        })?;
        let _guard = if matches!(spec.authority, Authority::Read | Authority::DeviceStop) {
            None
        } else {
            Some(
                self.0
                    .commands
                    .lock()
                    .map_err(|_| CommandError::failed("The command dispatcher is unavailable."))?,
            )
        };
        self.execute_inner(origin, id, params, expect_revision)
    }

    /// The caller holds the mutation gate, or is running a read/Stop command.
    fn execute_inner(
        &self,
        origin: Origin,
        id: &str,
        params: Value,
        expect_revision: Option<u64>,
    ) -> Result<CommandOutput, CommandError> {
        let spec = registry().into_iter().find(|s| s.id == id).ok_or_else(|| {
            CommandError::new("unknown-command", format!("There is no command {id}."))
        })?;
        match spec.needs {
            Needs::Design
                if self
                    .snapshot()
                    .map_err(CommandError::failed)?
                    .design
                    .is_none() =>
            {
                return Err(CommandError::new("disabled", "Open a design first."));
            }
            Needs::Window if !self.0.live => {
                return Err(CommandError::new(
                    "disabled",
                    "Only the CutCraft window has this; this session is headless.",
                ));
            }
            _ => {}
        }
        // Spec §6.2.6: from Sending until the sheet ends, edits and opening files wait.
        if matches!(spec.authority, Authority::Edit | Authority::FileRead)
            && !matches!(id, "help.show" | "settings.mcp" | "cut.dismiss")
        {
            self.check_not_cutting()?;
        }
        if let Some(expected) = expect_revision {
            let current = self.snapshot().map_err(CommandError::failed)?.revision;
            if current != expected {
                return Err(CommandError::new(
                    "revision-mismatch",
                    format!(
                        "The project is at revision {current}, not {expected}; nothing was changed. Read it again and retry."
                    ),
                ));
            }
        }
        if origin != Origin::User
            && let Ok(mut last) = self.0.last_automation.lock()
        {
            *last = Some((origin, std::time::Instant::now()));
        }
        let before = self.snapshot().map_err(CommandError::failed)?;
        let journalled = redact(&params);
        let result = self.dispatch(origin, id, params);
        let after = self.snapshot().map_err(CommandError::failed)?;
        self.journal(JournalEntry {
            seq: 0,
            id: id.to_owned(),
            origin,
            params: journalled,
            revision_before: before.revision,
            revision_after: after.revision,
            ok: result.is_ok(),
            code: result.as_ref().err().map(|e| e.code),
        });
        let output = result?;
        self.record(origin, &spec, before.design)?;
        Ok(CommandOutput {
            value: output.value,
            image: output.image,
            revision: self.snapshot().map_err(CommandError::failed)?.revision,
        })
    }

    fn journal(&self, mut entry: JournalEntry) {
        if let Ok(mut journal) = self.0.journal.lock() {
            entry.seq = journal.0;
            journal.0 += 1;
            journal.1.push_back(entry);
            if journal.1.len() > MAX_JOURNAL {
                journal.1.pop_front();
            }
        }
    }

    /// After a successful command: a new design clears the history; an edit that changed the
    /// design becomes an undo step, and an answer from automation is marked with its origin.
    fn record(
        &self,
        origin: Origin,
        spec: &Spec,
        before: Option<Arc<OpenDesign>>,
    ) -> Result<(), CommandError> {
        // A changed material or mat list goes to the paired cameo-pis (their Adjust uses it).
        if spec.id.starts_with("materials.") || spec.id.starts_with("mats.") {
            self.sync_catalog();
        }
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| CommandError::failed("Application state is unavailable."))?;
        match spec.id {
            "design.open" | "design.close" | "project.open" | "station.open" => {
                let project = matches!(spec.id, "project.open" | "station.open");
                state.history.clear();
                state.view.screen = Screen::SideBySide;
                // A project opens saved; a design opens as a new, unsaved project.
                state.saved_revision = project.then_some(state.revision);
                crate::cutting::reset_record(&mut state.cutting);
                if project && let Some(design) = state.design.as_ref() {
                    let project = design.project.clone();
                    crate::cutting::restore_record(&mut state.cutting, &project);
                }
                let opened = state
                    .design
                    .as_ref()
                    .map(|d| d.project_path.clone().unwrap_or_else(|| d.path.clone()));
                if spec.id != "design.close"
                    && let Some(path) = opened
                {
                    state.recent.retain(|p| p != &path);
                    state.recent.insert(0, path);
                    state.recent.truncate(settings::MAX_RECENT);
                    let _ = self.0.store.save_recent(&state.recent);
                }
            }
            "project.save" => state.saved_revision = Some(state.revision),
            "history.undo" | "history.redo" => {}
            _ if matches!(spec.authority, Authority::Edit | Authority::Motion)
                || spec.id == "project.update" =>
            {
                let (Some(before), Some(after)) = (before, state.design.clone()) else {
                    return Ok(());
                };
                if Arc::ptr_eq(&before, &after) {
                    return Ok(());
                }
                if spec.id == "prepare.decide" || spec.id == "prepare.reset" {
                    let mut marked = (*after).clone();
                    if spec.id == "prepare.reset" {
                        marked.answered_by.clear();
                    } else if let Some(last) = marked.recipe.decisions.last().cloned() {
                        marked
                            .answered_by
                            .retain(|a| !answers_same_question(&a.decision, &last));
                        if origin != Origin::User {
                            marked.answered_by.push(Answered {
                                decision: last,
                                origin,
                            });
                        }
                    }
                    state.design = Some(Arc::new(marked));
                }
                state.history.push(HistoryStep {
                    // Opening the Cut preview changes the design only to fit it on the mat.
                    label: if spec.id == "view.show" {
                        "Fit to the mat".to_owned()
                    } else {
                        spec.label.to_owned()
                    },
                    command: spec.id.to_owned(),
                    origin,
                    design: before,
                });
            }
            _ => {}
        }
        if let Some(material) = state
            .design
            .as_ref()
            .and_then(|d| d.project.sheets.first())
            .map(|s| {
                let mut material = s.material.clone();
                material.mirror = s.mirror;
                material
            })
        {
            state.session_material = Some(material);
        }
        Ok(())
    }

    /// Undo (or redo) up to `steps` steps, returning their labels.
    fn step_history(&self, steps: usize, undo: bool) -> Result<ActionOutput, CommandError> {
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| CommandError::failed("Application state is unavailable."))?;
        let mut done = Vec::new();
        for _ in 0..steps.max(1) {
            let (from, to) = if undo {
                (&mut state.history.undo, false)
            } else {
                (&mut state.history.redo, true)
            };
            let Some(step) = from.pop() else { break };
            let current = state.design.clone();
            let Some(current) = current else { break };
            let back = HistoryStep {
                design: current,
                ..step.clone()
            };
            if to {
                state.history.undo.push(back);
            } else {
                state.history.redo.push(back);
            }
            state.design = Some(step.design.clone());
            state.revision += 1;
            done.push(json!({"label": step.label, "command": step.command, "origin": step.origin}));
        }
        let (can_undo, can_redo) = (state.history.undo.len(), state.history.redo.len());
        drop(state);
        if done.is_empty() {
            return Err(CommandError::new(
                "disabled",
                if undo {
                    "There is nothing to undo."
                } else {
                    "There is nothing to redo."
                },
            ));
        }
        self.wake();
        Ok(ActionOutput::json(json!({
            if undo { "undone" } else { "redone" }: done,
            "undo_steps": can_undo,
            "redo_steps": can_redo,
        })))
    }

    fn dispatch(
        &self,
        origin: Origin,
        id: &str,
        params: Value,
    ) -> Result<ActionOutput, CommandError> {
        let action = |action| self.run(action).map_err(CommandError::failed);
        let value = |value: Value| Ok(ActionOutput::json(value));
        match id {
            "app.status" => {
                decode::<NoParams>(params)?;
                action(Action::Status)
            }
            "design.open" => {
                let p: OpenParams = decode(params)?;
                let path = self.check_read(origin, &p.path)?;
                self.open_design(path, p.recipe.unwrap_or_default())
                    .map_err(CommandError::failed)
            }
            "design.close" => {
                decode::<NoParams>(params)?;
                action(Action::CloseDesign)
            }
            "design.inspection" => {
                decode::<NoParams>(params)?;
                action(Action::Inspection)
            }
            "project.open" => {
                let p: ProjectOpenParams = decode(params)?;
                let path = self.check_read(origin, &p.path)?;
                self.open_project(&path).map_err(CommandError::failed)
            }
            "project.save" => {
                let p: SaveParams = decode(params)?;
                let design = self.design()?;
                let path = self.check_write(origin, &p.path, p.overwrite, &design)?;
                let file = ProjectFile::new(
                    design.project.clone(),
                    Path::new(&design.path),
                    &design.bytes,
                    crate::APP_BUILD,
                );
                file.save(&path).map_err(CommandError::failed)?;
                // Later saves and the cut record go to the project now saved.
                let saved = path.to_string_lossy().into_owned();
                self.change(|s| {
                    if let Some(open) = s.design.as_mut().map(Arc::make_mut) {
                        open.project_path = Some(saved);
                    }
                })
                .map_err(CommandError::failed)?;
                value(json!({"saved_to": path}))
            }
            "project.update" => {
                let p: UpdateParams = decode(params)?;
                let path = match p.path {
                    Some(path) => Some(self.check_read(origin, &path)?),
                    None => {
                        let current = self.design()?.path.clone();
                        Some(self.check_read(origin, &current)?)
                    }
                };
                self.update_from_design(path).map_err(CommandError::failed)
            }
            "project.inspect" => {
                decode::<NoParams>(params)?;
                value(self.project_summary()?)
            }
            "prepare.report" => {
                decode::<NoParams>(params)?;
                let design = self.design()?;
                let mut report = design.prepared.clone();
                report["answered_by"] = json!(design.answered_by);
                value(report)
            }
            "history.undo" | "history.redo" => {
                let p: StepsParams = decode(params)?;
                self.step_history(p.steps.unwrap_or(1), id == "history.undo")
            }
            "journal.list" => {
                let p: JournalParams = decode(params)?;
                let journal = self
                    .0
                    .journal
                    .lock()
                    .map_err(|_| CommandError::failed("The journal is unavailable."))?;
                let limit = p.limit.unwrap_or(100);
                let entries: Vec<&JournalEntry> = journal
                    .1
                    .iter()
                    .rev()
                    .take(limit)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                value(json!(entries))
            }
            "prepare.decide" => {
                let p: DecideParams = decode(params)?;
                action(Action::Decide(p.decision))
            }
            "prepare.reset" => {
                decode::<NoParams>(params)?;
                action(Action::ResetDecisions)
            }
            "prepare.run" => {
                decode::<NoParams>(params)?;
                let recipe = self.design()?.recipe.clone();
                self.reprepare(recipe).map_err(CommandError::failed)
            }
            "view.inspect" => {
                decode::<NoParams>(params)?;
                let mut report = self.ui_report();
                let snapshot = self.snapshot().map_err(CommandError::failed)?;
                if report.is_object() {
                    report["view"] = json!(snapshot.view);
                    report["unsaved"] = json!(snapshot.unsaved());
                    report["cutting"] = json!({"state": snapshot.cutting.state, "message": snapshot.cutting.message});
                }
                value(report)
            }
            "view.show" => {
                let p: ViewParams = decode(params)?;
                let mut state = self
                    .0
                    .state
                    .lock()
                    .map_err(|_| CommandError::failed("Application state is unavailable."))?;
                let sheets = state.design.as_ref().map_or(0, |d| d.project.sheets.len());
                if p.screen != Screen::SideBySide && !p.screen.is_cut_screen() && sheets == 0 {
                    return Err(CommandError::new("disabled", "Open a design first."));
                }
                if state.view.screen.is_cut_screen()
                    && state.view.screen != p.screen
                    && !state.cutting.state.busy()
                {
                    crate::cutting::clear_review(&mut state.cutting);
                }
                let entering_preview = p.screen == Screen::Mat && state.view.screen != Screen::Mat;
                state.view.screen = p.screen;
                let view = state.view;
                drop(state);
                self.wake();
                let mut result = json!(view);
                // Opening the Cut preview brings cut lines that don't fit back onto the mat.
                if entering_preview && let Some(adjusted) = self.bring_onto_mat()? {
                    result["adjusted"] = json!(adjusted);
                }
                value(result)
            }
            "rows.list" => {
                decode::<NoParams>(params)?;
                value(json!(rows(&*self.design()?)))
            }
            "cut.include" => {
                let p: IncludeParams = decode(params)?;
                self.edit_project(|project| {
                    project.set_row_included(&p.row, p.include)?;
                    Ok(json!({"row": p.row, "included": p.include}))
                })
            }
            "sheet.set" => {
                let p: SheetSetParams = decode(params)?;
                let snapshot = self.snapshot().map_err(CommandError::failed)?;
                let library = snapshot.materials;
                let mat_profile = match &p.mat_profile {
                    Some(name) => {
                        let wanted = name.to_lowercase();
                        let found: Vec<_> = snapshot
                            .mats
                            .iter()
                            .filter(|m| m.name.to_lowercase().starts_with(&wanted))
                            .collect();
                        match found.as_slice() {
                            [one] => Some((*one).clone()),
                            _ => match snapshot
                                .mats
                                .iter()
                                .find(|m| m.name.eq_ignore_ascii_case(name))
                            {
                                Some(m) => Some(m.clone()),
                                None => {
                                    return Err(CommandError::new(
                                        "invalid-params",
                                        format!(
                                            "Name one mat from mats.list; {name:?} matches {}.",
                                            found.len()
                                        ),
                                    ));
                                }
                            },
                        }
                    }
                    None => None,
                };
                self.edit_project(|project| {
                    let id = sheet_id(project, 1)?;
                    if let Some(name) = &p.material {
                        project.set_material(&id, material_named(name, &library)?)?;
                    }
                    if let Some(mirror) = p.mirror {
                        project.set_mirror(&id, mirror)?;
                        project.sheets[0].material.mirror = mirror;
                    }
                    if let Some(mat) = p.mat {
                        let placement = sheet(project, 1)?.placement.clone();
                        project.set_placement(&id, placement, mat)?;
                    }
                    if let Some(on) = p.weeding_box {
                        project.set_weeding_box(&id, on)?;
                    }
                    if let Some(profile) = mat_profile {
                        project.set_mat_profile(&id, profile)?;
                    }
                    if let Some(on) = p.weed_lines {
                        project.set_weed_lines(&id, on)?;
                    }
                    if let Some(on) = p.weed_inverted {
                        project.set_weed_inverted(&id, on)?;
                    }
                    if let Some(on) = p.layering_marks {
                        project.set_layering_marks(&id, on)?;
                    }
                    Ok(sheet_json(project, 1))
                })
            }
            "sheet.place" => {
                let p: PlaceParams = decode(params)?;
                self.edit_project(|project| {
                    let id = sheet_id(project, 1)?;
                    let current = sheet(project, 1)?;
                    let start = cutcraft_import::job::project::PLACEMENT_CLEARANCE_MM
                        + if current.weeding_box {
                            current.weeding_margin_mm
                        } else {
                            0.0
                        };
                    let (x, y) = if p.corner {
                        (Some(start), Some(start))
                    } else if p.onto_grid {
                        let plan = cutcraft_path::plan_sheet(project, &id)?;
                        let [bx0, by0, bx1, by1] = plan
                            .bounds_mm
                            .ok_or("There is nothing to cut on this sheet.")?;
                        let [aw, ah] = plan.cutting_area_mm;
                        let [sx0, sy0, sx1, sy1] = if current.mat == cutcraft_import::job::Mat::Roll
                        {
                            [0.0, 0.0, aw, ah]
                        } else {
                            let [x0, y0, x1, y1] = current.mat_profile().sticky_in_cut();
                            [x0.max(0.0), y0.max(0.0), x1.min(aw), y1.min(ah)]
                        };
                        if bx1 - bx0 > sx1 - sx0 || by1 - by0 > sy1 - sy0 {
                            return Err("The design is bigger than the area it can be cut in; make it smaller or use a longer mat.".into());
                        }
                        let shift = |lo: f64, hi: f64, min: f64, max: f64| {
                            if lo < min {
                                min - lo
                            } else if hi > max {
                                max - hi
                            } else {
                                0.0
                            }
                        };
                        let [ox, oy] = current.placement.offset_mm;
                        (
                            Some(ox + shift(bx0, bx1, sx0, sx1)),
                            Some(oy + shift(by0, by1, sy0, sy1)),
                        )
                    } else {
                        (p.x, p.y)
                    };
                    let placement = Placement {
                        offset_mm: [
                            x.unwrap_or(current.placement.offset_mm[0]),
                            y.unwrap_or(current.placement.offset_mm[1]),
                        ],
                        rotation_deg: p.rotate.unwrap_or(current.placement.rotation_deg),
                    };
                    let mat = current.mat;
                    project.set_placement(&id, placement, mat)?;
                    Ok(sheet_json(project, 1))
                })
            }
            "sheet.test_spot" => {
                let p: TestSpotParams = decode(params)?;
                let at = match (p.x_mm, p.y_mm) {
                    (Some(x), Some(y)) => Some([x, y]),
                    (None, None) => None,
                    _ => {
                        return Err(CommandError::new(
                            "invalid-params",
                            "Give the test cut's x and y, or neither to place it automatically.",
                        ));
                    }
                };
                self.edit_project(|project| {
                    let id = sheet_id(project, 1)?;
                    project.set_test_spot(&id, at)?;
                    Ok(sheet_json(project, 1))
                })
            }
            "sheet.piece" => {
                let p: PieceParams = decode(params)?;
                let moving = match (p.x_mm, p.y_mm) {
                    (Some(x), Some(y)) => Some([x, y]),
                    (None, None) => None,
                    _ => {
                        return Err(CommandError::new(
                            "invalid-params",
                            "Give the piece's x and y together.",
                        ));
                    }
                };
                let piece = match (p.width_mm, p.height_mm) {
                    (Some(w), Some(h)) => Some(Some([w, h])),
                    (None, None) if moving.is_some() => None,
                    (None, None) => Some(None),
                    _ => {
                        return Err(CommandError::new(
                            "invalid-params",
                            "Give the piece's width and height, or neither to forget it.",
                        ));
                    }
                };
                self.edit_project(|project| {
                    let id = sheet_id(project, 1)?;
                    if let Some(piece) = piece {
                        project.set_piece(&id, piece)?;
                    }
                    if let Some(at) = moving {
                        project.set_piece_at(&id, at)?;
                    }
                    Ok(sheet_json(project, 1))
                })
            }
            "plan.sheet" => {
                decode::<NoParams>(params)?;
                let design = self.design()?;
                let plan = plan(&design, 1)?;
                value(plan_summary(&plan, 1))
            }
            "export" => {
                let p: ExportParams = decode(params)?;
                let design = self.design()?;
                let plan = plan(&design, 1)?;
                let (bytes, mime) = match p.kind {
                    ExportKind::PreviewSvg => (
                        cutcraft_path::preview_svg(&plan).into_bytes(),
                        "image/svg+xml",
                    ),
                    ExportKind::PreviewPng => {
                        let options = cutcraft_path::RasterOptions {
                            max_side: 1024,
                            ..Default::default()
                        };
                        let preview = cutcraft_path::preview_png(&plan, &options)
                            .map_err(CommandError::failed)?;
                        (preview.png, "image/png")
                    }
                    ExportKind::Plan => (
                        serde_json::to_vec_pretty(&plan)
                            .map_err(|e| CommandError::failed(e.to_string()))?,
                        "application/json",
                    ),
                    ExportKind::CutterFile => {
                        let encoded =
                            cutcraft_device::gpgl::encode(&plan, &cutcraft_device::gpgl::CAMEO_1)
                                .map_err(|problems| {
                                CommandError::failed(
                                    problems
                                        .iter()
                                        .map(|d| d.message.as_str())
                                        .collect::<Vec<_>>()
                                        .join(" "),
                                )
                            })?;
                        (encoded.bytes, "application/octet-stream")
                    }
                };
                match p.path {
                    Some(path) => {
                        let path = self.check_write(origin, &path, p.overwrite, &design)?;
                        settings::write_atomic(&path, &bytes).map_err(CommandError::failed)?;
                        value(
                            json!({"saved_to": path, "bytes": bytes.len(), "mime_type": mime, "plan_id": plan.plan_id}),
                        )
                    }
                    None => value(json!({
                        "mime_type": mime,
                        "plan_id": plan.plan_id,
                        "dataBase64": base64::engine::general_purpose::STANDARD.encode(&bytes),
                    })),
                }
            }
            "render.preview" => {
                let p: RenderParams = decode(params)?;
                let design = self.design()?;
                if matches!(p.view, PreviewView::Source) {
                    let preview = crate::source_preview(&design, p.max_side.unwrap_or(1024))
                        .map_err(CommandError::failed)?;
                    return Ok(ActionOutput {
                        value: json!({"width": preview.width, "height": preview.height, "plan_id": "source"}),
                        image: Some(FrameImage {
                            png: preview.png,
                            width: preview.width,
                            height: preview.height,
                        }),
                    });
                }
                if matches!(p.view, PreviewView::Assembly) {
                    let preview =
                        cutcraft_path::assembly_png(&design.project, p.max_side.unwrap_or(1024))
                            .map_err(CommandError::failed)?;
                    let value = serde_json::to_value(&preview).unwrap_or_default();
                    return Ok(ActionOutput {
                        value,
                        image: Some(FrameImage {
                            png: preview.png,
                            width: preview.width,
                            height: preview.height,
                        }),
                    });
                }
                let plan = plan(&design, 1)?;
                let snapshot = self.snapshot().map_err(CommandError::failed)?;
                let cutting = snapshot.cutting;
                let options = cutcraft_path::RasterOptions {
                    view: match p.view {
                        PreviewView::Sheet => cutcraft_path::RasterView::Sheet,
                        PreviewView::ToolMotion | PreviewView::Assembly | PreviewView::Source => {
                            cutcraft_path::RasterView::ToolMotion
                        }
                    },
                    max_side: p.max_side.unwrap_or(1024),
                    order: p.order,
                    highlight: p.highlight,
                    test_cut: crate::cutting::test_cut_markers(&plan, &cutting),
                };
                let preview =
                    cutcraft_path::preview_png(&plan, &options).map_err(CommandError::failed)?;
                let mut value = serde_json::to_value(&preview).unwrap_or_default();
                value["sheet"] = json!(1);
                value["valid"] = json!(plan.valid);
                Ok(ActionOutput {
                    value,
                    image: Some(FrameImage {
                        png: preview.png,
                        width: preview.width,
                        height: preview.height,
                    }),
                })
            }
            "help.show" => {
                let p: HelpParams = decode(params)?;
                action(Action::Help {
                    topic: p.topic,
                    visible: p.visible.unwrap_or(true),
                })
            }
            "view.capture" => {
                let p: CaptureParams = decode(params)?;
                let save_to = match p.save_to {
                    Some(path) => {
                        let design = self.snapshot().map_err(CommandError::failed)?.design;
                        Some(match &design {
                            Some(design) => self.check_write(origin, &path, true, design)?,
                            None => self.check_write_path(origin, &path)?,
                        })
                    }
                    None => None,
                };
                action(Action::Capture { save_to })
            }
            "settings.mcp" => {
                let p: McpSettingsParams = decode(params)?;
                if p.enabled.is_none()
                    && p.port.is_none()
                    && p.address.is_none()
                    && p.require_key.is_none()
                {
                    return action(Action::McpSettings);
                }
                window_only(origin, "change the AI connection")?;
                let current = self.snapshot().map_err(CommandError::failed)?.mcp;
                action(Action::SetMcpSettings(McpSettings {
                    enabled: p.enabled.unwrap_or(current.enabled),
                    port: p.port.unwrap_or(current.port),
                    address: p
                        .address
                        .map(|a| a.trim().to_owned())
                        .unwrap_or(current.address),
                    require_key: p.require_key.unwrap_or(current.require_key),
                    keys: current.keys,
                }))
            }
            "settings.mcp_key_create" => {
                let p: McpKeyParams = decode(params)?;
                window_only(origin, "make access keys")?;
                let mut settings = self.snapshot().map_err(CommandError::failed)?.mcp;
                let (made, key) = crate::AccessKey::create(&p.name)
                    .map_err(|e| CommandError::new("invalid-params", e))?;
                if settings
                    .keys
                    .iter()
                    .any(|k| k.name.eq_ignore_ascii_case(&made.name))
                {
                    return Err(CommandError::new(
                        "invalid-params",
                        format!(
                            "There is already a key named {}; delete it first or choose another name.",
                            made.name
                        ),
                    ));
                }
                settings.keys.push(made.clone());
                let url = settings.url();
                action(Action::SetMcpSettings(settings))?;
                value(json!({
                    "name": made.name,
                    "key": key,
                    "url": url,
                    "message": "Copy the key now: CutCraft keeps only its fingerprint and cannot show it again.",
                }))
            }
            "settings.mcp_key_delete" => {
                let p: McpKeyParams = decode(params)?;
                window_only(origin, "delete access keys")?;
                let mut settings = self.snapshot().map_err(CommandError::failed)?.mcp;
                let before = settings.keys.len();
                settings.keys.retain(|k| k.name != p.name);
                if settings.keys.len() == before {
                    return Err(CommandError::new(
                        "invalid-params",
                        format!("There is no key named {}.", p.name),
                    ));
                }
                // The last key gone on this computer: stop requiring one rather than lock out.
                if settings.keys.is_empty() && settings.local_only() {
                    settings.require_key = false;
                }
                if settings.validate().is_err() {
                    return Err(CommandError::new(
                        "disabled",
                        "This is the last key while listening beyond this computer; switch to This computer only first.",
                    ));
                }
                action(Action::SetMcpSettings(settings))?;
                value(json!({"deleted": p.name}))
            }
            "settings.mcp_port" => {
                let p: McpPortParams = decode(params)?;
                let snapshot = self.snapshot().map_err(CommandError::failed)?;
                let mut probe = snapshot.mcp.clone();
                if let Some(address) = p.address {
                    probe.address = address.trim().to_owned();
                }
                let address: std::net::IpAddr = probe.address.parse().map_err(|_| {
                    CommandError::new(
                        "invalid-params",
                        format!("{} is not an IP address.", probe.address),
                    )
                })?;
                let port = p.port.unwrap_or(probe.port);
                let own = (snapshot.mcp_service.state == "running")
                    .then(|| (snapshot.mcp.ip(), snapshot.mcp.port));
                let state = crate::port_state(address, port, own);
                value(json!({"address": address, "port": port, "use": state}))
            }
            "app.engine_check" => {
                decode::<NoParams>(params)?;
                action(Action::EngineCheck)
            }
            "device.discover" => {
                decode::<NoParams>(params)?;
                action(Action::Probe)
            }
            "design.size" => {
                let p: SizeParams = decode(params)?;
                self.edit_project(|project| {
                    let size = cutcraft_path::design_size_mm(project)
                        .ok_or("The design has nothing to size.")?;
                    let scale = match (p.width_mm, p.height_mm, p.scale, p.fit) {
                        (Some(w), None, None, false) if w > 0.0 => project.scale * w / size[0],
                        (None, Some(h), None, false) if h > 0.0 => project.scale * h / size[1],
                        (None, None, Some(f), false) => f,
                        (None, None, None, true) => project.scale * fit_factor(project)?,
                        _ => {
                            return Err(
                                "Give one of width_mm, height_mm or scale as a positive number, or fit."
                                    .into(),
                            );
                        }
                    };
                    project.set_scale(scale)?;
                    if p.fit {
                        move_to_fit_corner(project)?;
                    }
                    Ok(json!({
                        "scale": project.scale,
                        "size_mm": cutcraft_path::design_size_mm(project),
                    }))
                })
            }
            "rows.group" => {
                let p: GroupParams = decode(params)?;
                self.edit_project(|project| {
                    project.set_group_by(p.by);
                    Ok(json!({"group_by": project.group_by}))
                })
            }
            "sheet.adjust" => {
                let p: AdjustParams = decode(params)?;
                let model = &cutcraft_device::gpgl::CAMEO_1;
                for (value, (low, high), what) in [
                    (p.force, model.force_range, "Force"),
                    (p.speed, model.speed_range, "Speed"),
                    (p.passes, (1, 3), "Repeats"),
                    (p.blade, (1, 10), "The blade number"),
                ] {
                    if value.is_some_and(|v| !(low..=high).contains(&v)) {
                        return Err(CommandError::new(
                            "invalid-params",
                            format!("{what} must be from {low} to {high}."),
                        ));
                    }
                }
                self.edit_project(|project| {
                    let id = sheet_id(project, 1)?;
                    let mut material = sheet(project, 1)?.material.clone();
                    material.force = p.force.unwrap_or(material.force);
                    material.speed = p.speed.unwrap_or(material.speed);
                    material.passes = p.passes.unwrap_or(material.passes);
                    material.blade = p.blade.unwrap_or(material.blade);
                    material.offset_method = p.offset_method.unwrap_or(material.offset_method);
                    material.offset_mm = p.offset_mm.unwrap_or(material.offset_mm);
                    material.overcut_mm = p.overcut_mm.unwrap_or(material.overcut_mm);
                    check_blade_values(&material).map_err(|e| e.message)?;
                    material.mirror = p.mirror.unwrap_or(sheet(project, 1)?.mirror);
                    project.set_material(&id, material)?;
                    let mut out = sheet_json(project, 1);
                    out["adjusted"] = json!(sheet(project, 1)?.material);
                    Ok(out)
                })
            }
            "materials.list" => {
                decode::<NoParams>(params)?;
                value(json!(
                    self.snapshot().map_err(CommandError::failed)?.materials
                ))
            }
            "materials.save" => {
                let p: MaterialSaveParams = decode(params)?;
                let saved =
                    self.save_material(p.material, p.replace, p.rename_from.as_deref(), false)?;
                if let (Some(on), Some(name)) = (p.default, saved.value["saved"]["name"].as_str()) {
                    self.mark_default(true, name, on)?;
                }
                Ok(saved)
            }
            "materials.save_current" => {
                let p: MaterialSaveCurrentParams = decode(params)?;
                let design = self.design()?;
                let current = sheet(&design.project, 1).map_err(CommandError::failed)?;
                let mut material = p.material.unwrap_or_else(|| {
                    let mut material = current.material.clone();
                    material.mirror = current.mirror;
                    material
                });
                if material.name != current.material.name {
                    return Err(CommandError::new(
                        "invalid-params",
                        "The material selection changed. Review its current settings before saving.",
                    ));
                }
                let replace = p.name.is_none();
                if let Some(name) = p.name {
                    material.name = name;
                }
                self.save_material(material, replace, None, true)
            }
            "materials.delete" => {
                let p: MaterialDeleteParams = decode(params)?;
                self.delete_material(&p.name)
            }
            "settings.units" => {
                let p: UnitsParams = decode(params)?;
                self.0
                    .store
                    .save_units(p.units)
                    .map_err(CommandError::failed)?;
                self.change(|s| s.units = p.units)
                    .map_err(CommandError::failed)?;
                value(json!({"units": p.units}))
            }
            "settings.font_size" => {
                let p: FontSizeParams = decode(params)?;
                if !(10.0..=24.0).contains(&p.points) {
                    return Err(CommandError::new(
                        "invalid-params",
                        "Interface font size must be between 10 and 24 points.",
                    ));
                }
                self.0
                    .store
                    .save_font_size(p.points)
                    .map_err(CommandError::failed)?;
                self.change(|s| s.font_size = p.points)
                    .map_err(CommandError::failed)?;
                value(json!({"points": p.points}))
            }
            "settings.snap" => {
                let p: SnapParams = decode(params)?;
                let mut snap = self.snapshot().map_err(CommandError::failed)?.snap;
                if let Some(step) = p.step_mm {
                    if !(0.1..=100.0).contains(&step) {
                        return Err(CommandError::new(
                            "invalid-params",
                            "The grid step must be between 0.1 and 100 mm.",
                        ));
                    }
                    snap.step_mm = Some(step);
                }
                if let Some(enabled) = p.enabled {
                    snap.enabled = enabled;
                }
                self.0.store.save_snap(snap).map_err(CommandError::failed)?;
                self.change(|s| s.snap = snap)
                    .map_err(CommandError::failed)?;
                value(json!(snap))
            }
            "settings.panel" => {
                let p: PanelParams = decode(params)?;
                let mut panel = self.snapshot().map_err(CommandError::failed)?.panel;
                let opened = p.open.iter().flat_map(|o| o.keys());
                for name in p
                    .order
                    .iter()
                    .chain(p.locked.iter())
                    .flatten()
                    .chain(opened)
                {
                    if !crate::settings::PANEL_GROUPS.contains(&name.as_str()) {
                        return Err(CommandError::new(
                            "invalid-params",
                            format!(
                                "There is no panel group {name:?}; the groups are {}.",
                                crate::settings::PANEL_GROUPS.join(", ")
                            ),
                        ));
                    }
                }
                if let Some(order) = p.order {
                    panel.order = order;
                    let arranged = panel.arranged();
                    // The usual order is stored as no order at all.
                    panel.order = if arranged == crate::settings::PANEL_GROUPS {
                        Vec::new()
                    } else {
                        arranged.into_iter().map(str::to_owned).collect()
                    };
                }
                if let Some(mut locked) = p.locked {
                    locked.sort_by_key(|g| panel.arranged().iter().position(|a| a == g));
                    locked.dedup();
                    panel.locked = locked;
                }
                if let Some(open) = p.open {
                    panel.open = open;
                }
                self.0
                    .store
                    .save_panel(panel.clone())
                    .map_err(CommandError::failed)?;
                self.change(|s| s.panel = panel.clone())
                    .map_err(CommandError::failed)?;
                value(json!(panel))
            }
            "settings.new_design" => {
                let p: NewDesignParams = decode(params)?;
                let snapshot = self.snapshot().map_err(CommandError::failed)?;
                let mut new_design = snapshot.new_design;
                if let Some(name) = p.material {
                    new_design.material = pick_name(
                        &name,
                        snapshot.materials.iter().map(|m| m.name.as_str()),
                        "material",
                    )?;
                }
                if let Some(name) = p.mat {
                    new_design.mat =
                        pick_name(&name, snapshot.mats.iter().map(|m| m.name.as_str()), "mat")?;
                }
                self.0
                    .store
                    .save_new_design(new_design.clone())
                    .map_err(CommandError::failed)?;
                self.change(|s| s.new_design = new_design.clone())
                    .map_err(CommandError::failed)?;
                value(json!(new_design))
            }
            "mats.list" => {
                decode::<NoParams>(params)?;
                value(json!(self.snapshot().map_err(CommandError::failed)?.mats))
            }
            "mats.save" => {
                let p: MatSaveParams = decode(params)?;
                let saved = self.save_mat(p.mat, p.rename_from.as_deref())?;
                if let (Some(on), Some(name)) = (p.default, saved.value["saved"]["name"].as_str()) {
                    self.mark_default(false, name, on)?;
                }
                Ok(saved)
            }
            "mats.delete" => {
                let p: MaterialDeleteParams = decode(params)?;
                let mut state = self
                    .0
                    .state
                    .lock()
                    .map_err(|_| CommandError::failed("Application state is unavailable."))?;
                // Shared lists (CutCraft-Web): held and read again, so no one else's edit is undone.
                let _lists = self.0.store.lock_lists().map_err(CommandError::failed)?;
                let mut mats = if self.0.store.shares_lists() {
                    self.0.store.load_mats().map_err(CommandError::failed)?
                } else {
                    state.mats.clone()
                };
                let before = mats.len();
                mats.retain(|m| !m.name.eq_ignore_ascii_case(&p.name));
                if mats.len() == before {
                    return Err(CommandError::failed(format!(
                        "There is no mat {:?}.",
                        p.name
                    )));
                }
                if mats.is_empty() {
                    return Err(CommandError::new("refused", "Keep at least one mat."));
                }
                self.0
                    .store
                    .save_mats(&mats)
                    .map_err(CommandError::failed)?;
                state.mats = mats;
                state.revision += 1;
                drop(state);
                self.wake();
                value(json!({"deleted": p.name}))
            }
            "device.connect" => {
                let p: ConnectParams = decode(params)?;
                self.device_connect(p.device, p.scenario, p.clock, p.station, p.cutter)
            }
            "station.pair" => {
                let p: StationAddressParams = decode(params)?;
                self.station_pair(&p.address)
            }
            "station.discover" => {
                decode::<NoParams>(params)?;
                self.station_discover()
            }
            "station.list" => {
                decode::<NoParams>(params)?;
                self.station_list()
            }
            "station.forget" => {
                let p: StationAddressParams = decode(params)?;
                self.station_forget(&p.address)
            }
            "station.library" => {
                let p: StationLibraryParams = decode(params)?;
                self.station_library(p.station.as_deref(), &p.folder)
            }
            "station.open" => {
                let p: StationOpenParams = decode(params)?;
                self.station_open(p.station.as_deref(), &p.job_id, p.revision)
            }
            "cut.confirm_fresh" => {
                decode::<NoParams>(params)?;
                if !self.is_remote() {
                    return Err(CommandError::new(
                        "disabled",
                        "Only a cutter on cameo-pi needs this: a USB Cameo's mat is loaded at the cutter.",
                    ));
                }
                self.remote_confirm_fresh()
            }
            "cut.send" => {
                let p: SendParams = decode(params)?;
                let design = self.design()?;
                self.cut_send(&design, p.station.as_deref(), p.folder, p.name.as_deref())
            }
            "device.disconnect" => {
                decode::<NoParams>(params)?;
                self.device_disconnect()
            }
            "device.status" => {
                decode::<NoParams>(params)?;
                value(json!(
                    self.snapshot().map_err(CommandError::failed)?.cutting
                ))
            }
            "device.wait" => {
                let p: WaitParams = decode(params)?;
                let limit = std::time::Duration::from_secs(p.timeout_s.unwrap_or(60).clamp(1, 600));
                let start = std::time::Instant::now();
                loop {
                    let view = self.snapshot().map_err(CommandError::failed)?.cutting;
                    if !view.state.busy()
                        || view.state == crate::CutState::Paused
                        || start.elapsed() >= limit
                    {
                        break value(json!(view));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
            "device.load" => {
                let p: LoadParams = decode(params)?;
                self.sim_load(p.media)
            }
            "cut.copy" => {
                decode::<NoParams>(params)?;
                let design = self.design()?;
                if !design.project.prepared.cut_ready {
                    return Err(CommandError::new(
                        "not-cut-ready",
                        "Review the design's issues before cutting.",
                    ));
                }
                let mut project = design.project.clone();
                project.sheets.truncate(1);
                let sheet = project
                    .sheets
                    .first()
                    .ok_or_else(|| CommandError::failed("There is nothing to cut."))?;
                let plan =
                    cutcraft_path::plan_sheet(&project, &sheet.id).map_err(CommandError::failed)?;
                if !plan.valid {
                    return Err(CommandError::new(
                        "not-cut-ready",
                        "Review the sheet's cutting problems first.",
                    ));
                }
                let file = ProjectFile::new(
                    project,
                    Path::new(&design.path),
                    &design.bytes,
                    crate::APP_BUILD,
                );
                value(json!({"project": file, "plan": plan}))
            }
            "cut.sheet" => {
                decode::<NoParams>(params)?;
                let out =
                    self.cut_prepare(origin, &self.design()?.project, JobKind::Sheet, false)?;
                self.change(|s| s.view.screen = Screen::Cut)
                    .map_err(CommandError::failed)?;
                Ok(out)
            }
            "cut.test" => {
                let p: TestCutParams = decode(params)?;
                let mut project = self.design()?.project.clone();
                if p.on_scrap {
                    project.sheets[0].piece_mm = None;
                    project.sheets[0].piece_at_mm = None;
                }
                let out = self.cut_prepare(origin, &project, JobKind::TestCut, p.on_scrap)?;
                self.change(|s| s.view.screen = Screen::TestCut)
                    .map_err(CommandError::failed)?;
                Ok(out)
            }
            "cut.start" => {
                decode::<NoParams>(params)?;
                let design = self.snapshot().map_err(CommandError::failed)?.design;
                self.cut_start(design.as_deref())
            }
            "cut.pause" | "cut.resume" => {
                decode::<NoParams>(params)?;
                self.cut_pause(id == "cut.pause")
            }
            "cut.stop" => {
                decode::<NoParams>(params)?;
                self.cut_stop()
            }
            "cut.exit" => {
                decode::<NoParams>(params)?;
                self.cut_exit()
            }
            "cut.dismiss" => {
                decode::<NoParams>(params)?;
                self.cut_dismiss()
            }
            "cut.reset" => {
                decode::<NoParams>(params)?;
                self.cut_reset()
            }
            _ => Err(CommandError::new(
                "unknown-command",
                format!("There is no command {id}."),
            )),
        }
    }

    fn design(&self) -> Result<Arc<OpenDesign>, CommandError> {
        self.snapshot()
            .map_err(CommandError::failed)?
            .design
            .ok_or_else(|| CommandError::new("disabled", "Open a design first."))
    }

    /// When the first sheet's cuts reach outside their piece or the mat's sticky grid, make them
    /// fit: shrink them to the largest size that fits if they are too big, otherwise move them
    /// inside. Says what it did, or nothing when they already fit or can't be planned.
    fn bring_onto_mat(&self) -> Result<Option<String>, CommandError> {
        let Ok(design) = self.design() else {
            return Ok(None);
        };
        let project = &design.project;
        let Some(first) = project.sheets.first() else {
            return Ok(None);
        };
        let Ok(plan) = cutcraft_path::plan_sheet(project, &first.id) else {
            return Ok(None);
        };
        let (Some([bx0, by0, bx1, by1]), Ok([x0, y0, x1, y1])) =
            (plan.bounds_mm, fit_area(project, &plan))
        else {
            return Ok(None);
        };
        // Inside means inside the area itself; the clearance only applies when moving.
        let c = cutcraft_import::job::project::PLACEMENT_CLEARANCE_MM;
        let tolerance = 1e-6;
        if bx0 >= x0 - c - tolerance
            && by0 >= y0 - c - tolerance
            && bx1 <= x1 + c + tolerance
            && by1 <= y1 + c + tolerance
        {
            return Ok(None);
        }
        let where_ = if plan.piece_mm.is_some() {
            "your material"
        } else {
            "the mat"
        };
        if bx1 - bx0 > x1 - x0 || by1 - by0 > y1 - y0 {
            self.edit_project(|project| {
                project.set_scale(project.scale * fit_factor(project)?)?;
                move_to_fit_corner(project)?;
                Ok(json!({}))
            })?;
            Ok(Some(format!(
                "The cut lines were bigger than {where_}, so they were shrunk to fit. Undo restores their size."
            )))
        } else {
            self.edit_project(|project| {
                let id = sheet_id(project, 1)?;
                let current = sheet(project, 1)?;
                let shift = |lo: f64, hi: f64, min: f64, max: f64| {
                    if lo < min {
                        min - lo
                    } else if hi > max {
                        max - hi
                    } else {
                        0.0
                    }
                };
                let [ox, oy] = current.placement.offset_mm;
                let placement = Placement {
                    offset_mm: [ox + shift(bx0, bx1, x0, x1), oy + shift(by0, by1, y0, y1)],
                    rotation_deg: current.placement.rotation_deg,
                };
                let mat = current.mat;
                project.set_placement(&id, placement, mat)?;
                Ok(json!({}))
            })?;
            Ok(Some(format!(
                "The cut lines reached off {where_}, so they were moved onto it. Undo puts them back."
            )))
        }
    }

    /// Change the open design's project as one step: the revision moves on and the window
    /// shows it.
    fn edit_project(
        &self,
        edit: impl FnOnce(&mut cutcraft_import::job::Project) -> Result<Value, String>,
    ) -> Result<ActionOutput, CommandError> {
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| CommandError::failed("Application state is unavailable."))?;
        let design = state
            .design
            .clone()
            .ok_or_else(|| CommandError::new("disabled", "Open a design first."))?;
        let mut changed = (*design).clone();
        let value = edit(&mut changed.project).map_err(CommandError::failed)?;
        state.session_material = changed.project.sheets.first().map(|s| {
            let mut material = s.material.clone();
            material.mirror = s.mirror;
            material
        });
        state.design = Some(Arc::new(changed));
        state.revision += 1;
        drop(state);
        self.wake();
        Ok(ActionOutput::json(value))
    }

    pub(crate) fn open_project(&self, path: &Path) -> Result<ActionOutput, String> {
        let file = ProjectFile::open(path)?;
        let bytes = file.source.bytes()?;
        let source = PathBuf::from(&file.source.original_path);
        // The import report comes from the embedded copy; the prepared geometry is the
        // project's own, so no fonts are needed to cut it again.
        let inspection = cutcraft_import::inspect_bytes(&source, &bytes)?;
        let prepared = &file.project.prepared;
        let design = OpenDesign {
            path: file.source.original_path.clone(),
            project_path: Some(path.to_string_lossy().into_owned()),
            inspection: serde_json::to_value(inspection).map_err(|e| e.to_string())?,
            recipe: prepared.recipe.clone(),
            prepared: serde_json::to_value(prepared).map_err(|e| e.to_string())?,
            project: file.project.clone(),
            bytes: Arc::new(bytes),
            answered_by: Vec::new(),
        };
        let revision = {
            let mut state = self
                .0
                .state
                .lock()
                .map_err(|_| "Application state is unavailable.")?;
            state.revision += 1;
            state.revision
        };
        let design = self.finish_open(revision, Ok(design))?;
        Ok(ActionOutput::json(json!({
            "project": path,
            "design": design.path,
            "cut_ready": design.project.prepared.cut_ready,
        })))
    }

    fn project_summary(&self) -> Result<Value, CommandError> {
        let status = self.watch_source();
        let snapshot = self.snapshot().map_err(CommandError::failed)?;
        let design = snapshot
            .design
            .ok_or_else(|| CommandError::new("disabled", "Open a design first."))?;
        let project = &design.project;
        let prepared = &project.prepared;
        let sheet = project.sheets.first().map(|s| {
            let mut summary = sheet_json(project, 1);
            let validity = match cutcraft_path::plan_sheet(project, &s.id) {
                Ok(plan) => json!({
                    "plan_id": plan.plan_id,
                    "valid": plan.valid,
                    "blocking": plan.diagnostics.iter()
                        .filter(|d| d.severity == Severity::Blocking)
                        .map(|d| &d.code).collect::<Vec<_>>(),
                }),
                Err(error) => json!({"valid": false, "error": error}),
            };
            summary["plan"] = validity;
            summary
        });
        Ok(json!({
            "source": {"path": design.path, "project": design.project_path, "sha256": prepared.source_sha256, "status": status},
            "page": prepared.page,
            "cut_ready": prepared.cut_ready,
            "pending": prepared.pending.len(),
            "blocking": prepared.diagnostics.iter()
                .filter(|d| d.severity == Severity::Blocking)
                .map(|d| &d.code).collect::<Vec<_>>(),
            "pieces": prepared.pieces.len(),
            "cut_lines": prepared.cut_lines.len(),
            "rows": rows(&design),
            "sheet": sheet,
            "revision": snapshot.revision,
        }))
    }

    /// Spec §6.2.7: automation reads anywhere the person can, unless a read root limits it.
    fn check_read(&self, origin: Origin, path: &str) -> Result<PathBuf, CommandError> {
        let path = PathBuf::from(path);
        if origin == Origin::User {
            return Ok(path);
        }
        if !path.is_absolute() {
            return Err(CommandError::new(
                "invalid-params",
                "Supply an absolute path on the CutCraft machine.",
            ));
        }
        let Some(root) = self.0.roots.read.as_ref() else {
            return Ok(path);
        };
        let canonical = std::fs::canonicalize(&path)
            .map_err(|e| CommandError::failed(format!("Cannot open {}: {e}", path.display())))?;
        if !canonical.starts_with(root) {
            return Err(CommandError::new(
                "outside-roots",
                format!(
                    "{} is outside the automation read root {}.",
                    path.display(),
                    root.display()
                ),
            ));
        }
        Ok(canonical)
    }

    /// Where a write may go: inside the write root for automation, never over the open design,
    /// and over an existing file only with `overwrite`.
    fn check_write(
        &self,
        origin: Origin,
        path: &str,
        overwrite: bool,
        design: &OpenDesign,
    ) -> Result<PathBuf, CommandError> {
        let target = self.check_write_path(origin, path)?;
        let canonical = std::fs::canonicalize(&target).ok();
        let source = std::fs::canonicalize(&design.path).ok();
        if target == Path::new(&design.path) || (canonical.is_some() && canonical == source) {
            return Err(CommandError::new(
                "refused",
                "CutCraft never writes over the design file. Choose another path.",
            ));
        }
        if target.exists() && !overwrite {
            return Err(CommandError::new(
                "refused",
                format!(
                    "{} already exists; pass overwrite: true to replace it.",
                    target.display()
                ),
            ));
        }
        Ok(target)
    }

    fn check_write_path(&self, origin: Origin, path: &str) -> Result<PathBuf, CommandError> {
        let path = PathBuf::from(path);
        if origin == Origin::User {
            return Ok(path);
        }
        if !path.is_absolute() {
            return Err(CommandError::new(
                "invalid-params",
                "Supply an absolute output path.",
            ));
        }
        let Some(root) = self.0.roots.write.as_ref() else {
            return Ok(path);
        };
        let parent = path
            .parent()
            .and_then(|p| std::fs::canonicalize(p).ok())
            .ok_or_else(|| {
                CommandError::failed(format!("The folder for {} does not exist.", path.display()))
            })?;
        if !parent.starts_with(root) {
            return Err(CommandError::new(
                "outside-roots",
                format!(
                    "{} is outside the automation write root {}.",
                    path.display(),
                    root.display()
                ),
            ));
        }
        Ok(parent.join(path.file_name().unwrap_or_default()))
    }
}

/// One step of a batch (spec §6.2.4). String values `$N.path` or `$last.path` in `params` take
/// a value from an earlier step's result.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchStep {
    pub id: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct StepOutcome {
    pub step: usize,
    pub id: String,
    pub ok: bool,
    pub result: Option<Value>,
    pub error: Option<CommandError>,
    pub revision: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct BatchOutcome {
    pub steps: Vec<StepOutcome>,
    pub ok: bool,
    /// An atomic batch that failed was undone as a whole.
    pub rolled_back: bool,
    pub revision: u64,
}

impl Controller {
    /// Run steps in order. With `stop_on_error` the first failure ends the batch; with `atomic`
    /// a failure also restores the design and project as they were before the batch.
    pub fn execute_batch(
        &self,
        origin: Origin,
        steps: &[BatchStep],
        stop_on_error: bool,
        atomic: bool,
    ) -> Result<BatchOutcome, CommandError> {
        // Keep another edit from interleaving with an atomic batch or its rollback.
        let stop_only = !steps.is_empty() && steps.iter().all(|step| step.id == "cut.stop");
        // Stop has no reversible project edits. Never restore a project snapshot for a
        // Stop-only batch: ordinary edits are allowed to run concurrently with it.
        let atomic = atomic && !stop_only;
        let _guard = if atomic {
            Some(
                self.0
                    .commands
                    .lock()
                    .map_err(|_| CommandError::failed("The command dispatcher is unavailable."))?,
            )
        } else {
            None
        };
        let snapshot = self.snapshot().map_err(CommandError::failed)?;
        let session_material = snapshot.session_material;
        let (before, history) = (snapshot.design, snapshot.history);
        let mut results: Vec<Value> = Vec::new();
        let mut outcomes = Vec::new();
        let mut failed = false;
        for (index, step) in steps.iter().enumerate() {
            let outcome = resolve(&step.params, &results).and_then(|params| {
                if atomic {
                    self.execute_inner(origin, &step.id, params, None)
                } else {
                    self.execute(origin, &step.id, params, None)
                }
            });
            let revision = self.snapshot().map_err(CommandError::failed)?.revision;
            match outcome {
                Ok(output) => {
                    results.push(output.value.clone());
                    outcomes.push(StepOutcome {
                        step: index,
                        id: step.id.clone(),
                        ok: true,
                        result: Some(output.value),
                        error: None,
                        revision,
                    });
                }
                Err(error) => {
                    failed = true;
                    results.push(Value::Null);
                    outcomes.push(StepOutcome {
                        step: index,
                        id: step.id.clone(),
                        ok: false,
                        result: None,
                        error: Some(error),
                        revision,
                    });
                    if stop_on_error || atomic {
                        break;
                    }
                }
            }
        }
        let rolled_back = failed && atomic;
        if rolled_back {
            {
                let mut state = self
                    .0
                    .state
                    .lock()
                    .map_err(|_| CommandError::failed("Application state is unavailable."))?;
                state.design = before;
                state.session_material = session_material;
                state.history = history;
                state.revision += 1;
            }
            self.wake();
        } else if atomic {
            // An atomic batch is one undo step, back to the design before it.
            let mut state = self
                .0
                .state
                .lock()
                .map_err(|_| CommandError::failed("Application state is unavailable."))?;
            let start = history.undo.len();
            if state.history.undo.len() > start + 1
                && state.history.undo.len() <= crate::history::MAX_STEPS
            {
                let first = state.history.undo[start].clone();
                let count = state.history.undo.len() - start;
                state.history.undo.truncate(start);
                state.history.undo.push(HistoryStep {
                    label: format!("{count} changes"),
                    command: "batch".into(),
                    ..first
                });
            }
        }
        Ok(BatchOutcome {
            ok: !failed,
            rolled_back,
            steps: outcomes,
            revision: self.snapshot().map_err(CommandError::failed)?.revision,
        })
    }
}

/// Replace `$N.path` / `$last.path` strings with values from earlier results.
fn resolve(params: &Value, results: &[Value]) -> Result<Value, CommandError> {
    Ok(match params {
        Value::String(text) if text.starts_with('$') => {
            let mut parts = text[1..].split('.');
            let head = parts.next().unwrap_or_default();
            let index = match head {
                "last" => results.len().checked_sub(1),
                n => n.parse::<usize>().ok().filter(|n| *n < results.len()),
            }
            .ok_or_else(|| {
                CommandError::new("invalid-params", format!("{text} names no earlier step."))
            })?;
            let mut value = &results[index];
            for part in parts {
                value = match value {
                    Value::Array(items) => part.parse::<usize>().ok().and_then(|i| items.get(i)),
                    Value::Object(map) => map.get(part),
                    _ => None,
                }
                .ok_or_else(|| {
                    CommandError::new(
                        "invalid-params",
                        format!("{text}: step {index}'s result has no {part}."),
                    )
                })?;
            }
            value.clone()
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| resolve(v, results))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| Ok((k.clone(), resolve(v, results)?)))
                .collect::<Result<_, CommandError>>()?,
        ),
        other => other.clone(),
    })
}

fn sheet(project: &cutcraft_import::job::Project, number: usize) -> Result<&Sheet, String> {
    number
        .checked_sub(1)
        .and_then(|i| project.sheets.get(i))
        .ok_or_else(|| {
            format!(
                "There is no sheet {number}; the project has {}.",
                project.sheets.len()
            )
        })
}

/// Where the first sheet's cuts may go, `[x0, y0, x1, y1]` in cutter mm, inside the placement
/// clearance: their piece, the mat's sticky grid or the roll's width.
fn fit_area(
    project: &cutcraft_import::job::Project,
    plan: &cutcraft_path::CutPlan,
) -> Result<[f64; 4], String> {
    let current = sheet(project, 1)?;
    let [aw, ah] = plan.cutting_area_mm;
    let [x0, y0, x1, y1] = if plan.piece_mm.is_some() {
        plan.usable_area_mm()
    } else if current.mat == cutcraft_import::job::Mat::Roll {
        [0.0, 0.0, aw, f64::INFINITY]
    } else {
        let [x0, y0, x1, y1] = current.mat_profile().sticky_in_cut();
        [x0.max(0.0), y0.max(0.0), x1.min(aw), y1.min(ah)]
    };
    let c = cutcraft_import::job::project::PLACEMENT_CLEARANCE_MM;
    Ok([x0 + c, y0 + c, x1 - c, y1 - c])
}

/// How much the design must grow (or shrink) for every sheet's cuts to fit [`fit_area`]. The
/// weeding box keeps its margin whatever the design's size.
fn fit_factor(project: &cutcraft_import::job::Project) -> Result<f64, String> {
    let mut factor = f64::INFINITY;
    for s in &project.sheets {
        let plan = cutcraft_path::plan_sheet(project, &s.id)?;
        let Some([bx0, by0, bx1, by1]) = plan.bounds_mm else {
            continue;
        };
        let margin = if s.weeding_box {
            2.0 * s.weeding_margin_mm
        } else {
            0.0
        };
        let [x0, y0, x1, y1] = fit_area(project, &plan)?;
        let (w, h) = (bx1 - bx0 - margin, by1 - by0 - margin);
        let (room_w, room_h) = (x1 - x0 - margin, y1 - y0 - margin);
        if w <= 0.0 || h <= 0.0 || room_w <= 0.0 || room_h <= 0.0 {
            return Err("There is no room to fit the design.".into());
        }
        factor = factor.min(room_w / w).min(room_h / h);
    }
    if factor.is_finite() {
        Ok(factor)
    } else {
        Err("There is nothing to cut to fit.".into())
    }
}

/// After fitting, the first sheet's cuts start at [`fit_area`]'s corner.
fn move_to_fit_corner(project: &mut cutcraft_import::job::Project) -> Result<(), String> {
    let id = sheet_id(project, 1)?;
    let plan = cutcraft_path::plan_sheet(project, &id)?;
    let Some([bx0, by0, ..]) = plan.bounds_mm else {
        return Ok(());
    };
    let [x0, y0, ..] = fit_area(project, &plan)?;
    let current = sheet(project, 1)?;
    let [ox, oy] = current.placement.offset_mm;
    let placement = Placement {
        offset_mm: [ox + x0 - bx0, oy + y0 - by0],
        rotation_deg: current.placement.rotation_deg,
    };
    let mat = current.mat;
    project.set_placement(&id, placement, mat)
}

fn sheet_id(project: &cutcraft_import::job::Project, number: usize) -> Result<String, String> {
    Ok(sheet(project, number)?.id.clone())
}

fn sheet_json(project: &cutcraft_import::job::Project, number: usize) -> Value {
    let Ok(s) = sheet(project, number) else {
        return json!({});
    };
    json!({
        "material": s.material.name,
        "mirror": s.mirror,
        "mat": s.mat,
        "placement": {"x": s.placement.offset_mm[0], "y": s.placement.offset_mm[1], "rotate": s.placement.rotation_deg},
        "weeding_box": s.weeding_box,
        "weed_lines": s.weed_lines,
        "weed_inverted": s.weed_inverted,
        "layering_marks": s.layering_marks,
        "piece": s.piece_mm.map(|[w, h]| {
            let [x, y] = s.piece_at_mm.unwrap_or([0.0, 0.0]);
            json!({"width_mm": w, "height_mm": h, "x_mm": x, "y_mm": y})
        }),
        "mat_profile": s.mat_profile().name,
        "test_spot": s.test_spot_mm.map(|[x, y]| json!({"x_mm": x, "y_mm": y})),
    })
}

/// The choose-what-to-cut rows as `rows.list` reports them; the window shows the same list.
pub fn rows(design: &OpenDesign) -> Vec<Value> {
    let project = &design.project;
    let prepared = &project.prepared;
    project
        .sheets
        .iter()
        .flat_map(|s| s.rows.iter())
        .map(|row| {
            let pieces = prepared
                .pieces
                .iter()
                .filter(|p| row.has_piece(&p.colour, &p.layer))
                .count();
            let lines = prepared
                .cut_lines
                .iter()
                .filter(|l| row.has_line(&l.colour, &l.layer))
                .count();
            json!({
                "row": row,
                "label": row.label(),
                "included": !project.excluded_rows.contains(row),
                "pieces": pieces,
                "cut_lines": lines,
            })
        })
        .collect()
}

/// A library name for a starting choice: "" is none, otherwise an exact name or a unique start.
fn pick_name<'a>(
    name: &str,
    names: impl Iterator<Item = &'a str> + Clone,
    kind: &str,
) -> Result<Option<String>, CommandError> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    if let Some(exact) = names.clone().find(|n| n.eq_ignore_ascii_case(name)) {
        return Ok(Some(exact.to_owned()));
    }
    let lower = name.to_lowercase();
    let found: Vec<&str> = names
        .clone()
        .filter(|n| n.to_lowercase().starts_with(&lower))
        .collect();
    match found.as_slice() {
        [one] => Ok(Some((*one).to_owned())),
        _ => Err(CommandError::new(
            "invalid-params",
            format!(
                "No single {kind} starts with {name:?}. Choose from: {}.",
                names.collect::<Vec<_>>().join("; ")
            ),
        )),
    }
}

fn material_named(name: &str, all: &[Material]) -> Result<Material, String> {
    if let Some(exact) = all.iter().find(|m| m.name.eq_ignore_ascii_case(name)) {
        return Ok(exact.clone());
    }
    let lower = name.to_lowercase();
    let matches: Vec<&Material> = all
        .iter()
        .filter(|m| m.name.to_lowercase().starts_with(&lower))
        .collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        _ => Err(format!(
            "No single material starts with {name:?}. Choose from: {}.",
            all.iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )),
    }
}

impl Controller {
    fn save_material(
        &self,
        mut material: Material,
        replace: bool,
        rename_from: Option<&str>,
        apply_current: bool,
    ) -> Result<ActionOutput, CommandError> {
        let model = &cutcraft_device::gpgl::CAMEO_1;
        let name = material.name.trim().to_owned();
        if name.is_empty() {
            return Err(CommandError::new(
                "invalid-params",
                "Give the material a name.",
            ));
        }
        let in_range = |v: u8, (low, high): (u8, u8)| (low..=high).contains(&v);
        if !in_range(material.force, model.force_range)
            || !in_range(material.speed, model.speed_range)
            || !in_range(material.passes, (1, 3))
            || !in_range(material.blade, (1, 10))
        {
            return Err(CommandError::new(
                "invalid-params",
                "Force is 1 to 33, speed 1 to 10, repeats 1 to 3 and the blade 1 to 10.",
            ));
        }
        check_blade_values(&material)?;
        material.name = name;
        material.notes = material.notes.trim().to_owned();
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| CommandError::failed("Application state is unavailable."))?;
        // Shared lists (CutCraft-Web): held and read again, so no one else's edit is undone.
        let _lists = self.0.store.lock_lists().map_err(CommandError::failed)?;
        let mut library = if self.0.store.shares_lists() {
            self.0
                .store
                .load_materials()
                .map_err(CommandError::failed)?
        } else {
            state.materials.clone()
        };
        if let Some(old) = rename_from {
            let Some(i) = library
                .iter()
                .position(|m| m.name.eq_ignore_ascii_case(old))
            else {
                return Err(CommandError::failed(format!(
                    "There is no material {old:?}."
                )));
            };
            let taken = library
                .iter()
                .enumerate()
                .any(|(j, m)| j != i && m.name.eq_ignore_ascii_case(&material.name));
            if taken {
                return Err(CommandError::new(
                    "refused",
                    format!("{} already exists; choose another name.", material.name),
                ));
            }
            if state
                .new_design
                .material
                .as_ref()
                .is_some_and(|d| d.eq_ignore_ascii_case(old))
            {
                state.new_design.material = Some(material.name.clone());
                self.0
                    .store
                    .save_new_design(state.new_design.clone())
                    .map_err(CommandError::failed)?;
            }
            library[i] = material.clone();
        } else {
            match library
                .iter()
                .position(|m| m.name.eq_ignore_ascii_case(&material.name))
            {
                Some(i) if replace => library[i] = material.clone(),
                Some(_) => {
                    return Err(CommandError::new(
                        "refused",
                        format!(
                            "{} already exists; choose a different name or use Save to update the current material.",
                            material.name
                        ),
                    ));
                }
                None => library.push(material.clone()),
            }
        }
        let changed = if apply_current {
            let design = state
                .design
                .as_ref()
                .ok_or_else(|| CommandError::failed("Open a design first."))?;
            let mut changed = (**design).clone();
            let id = sheet_id(&changed.project, 1).map_err(CommandError::failed)?;
            changed
                .project
                .set_material(&id, material.clone())
                .map_err(CommandError::failed)?;
            Some(Arc::new(changed))
        } else {
            None
        };
        self.0
            .store
            .save_materials(&library)
            .map_err(CommandError::failed)?;
        state.materials = library;
        if let Some(changed) = changed {
            state.session_material = Some(material.clone());
            state.design = Some(changed);
        }
        state.materials_error = None;
        state.revision += 1;
        drop(state);
        self.wake();
        Ok(ActionOutput::json(json!({"saved": material})))
    }

    /// Make a saved material (or mat) the one new designs start with, or stop if it was.
    fn mark_default(&self, material: bool, name: &str, on: bool) -> Result<(), CommandError> {
        let mut new_design = self.snapshot().map_err(CommandError::failed)?.new_design;
        let slot = if material {
            &mut new_design.material
        } else {
            &mut new_design.mat
        };
        if on {
            *slot = Some(name.to_owned());
        } else if slot.as_deref() == Some(name) {
            *slot = None;
        } else {
            return Ok(());
        }
        self.0
            .store
            .save_new_design(new_design.clone())
            .map_err(CommandError::failed)?;
        self.change(|s| s.new_design = new_design)
            .map_err(CommandError::failed)
    }

    fn save_mat(
        &self,
        mut mat: cutcraft_import::job::MatProfile,
        rename_from: Option<&str>,
    ) -> Result<ActionOutput, CommandError> {
        mat.name = mat.name.trim().to_owned();
        mat.check()
            .map_err(|e| CommandError::new("invalid-params", e))?;
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| CommandError::failed("Application state is unavailable."))?;
        // Shared lists (CutCraft-Web): held and read again, so no one else's edit is undone.
        let _lists = self.0.store.lock_lists().map_err(CommandError::failed)?;
        let mut mats = if self.0.store.shares_lists() {
            self.0.store.load_mats().map_err(CommandError::failed)?
        } else {
            state.mats.clone()
        };
        let existing = mats
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(&mat.name));
        match rename_from {
            Some(old) => {
                let Some(i) = mats.iter().position(|m| m.name.eq_ignore_ascii_case(old)) else {
                    return Err(CommandError::failed(format!("There is no mat {old:?}.")));
                };
                if existing.is_some_and(|j| j != i) {
                    return Err(CommandError::new(
                        "refused",
                        format!("{} already exists; choose another name.", mat.name),
                    ));
                }
                if state
                    .new_design
                    .mat
                    .as_ref()
                    .is_some_and(|d| d.eq_ignore_ascii_case(old))
                {
                    state.new_design.mat = Some(mat.name.clone());
                    self.0
                        .store
                        .save_new_design(state.new_design.clone())
                        .map_err(CommandError::failed)?;
                }
                mats[i] = mat.clone();
            }
            None if existing.is_some() => {
                return Err(CommandError::new(
                    "refused",
                    format!("{} already exists; edit it with rename_from.", mat.name),
                ));
            }
            None => mats.push(mat.clone()),
        }
        self.0
            .store
            .save_mats(&mats)
            .map_err(CommandError::failed)?;
        state.mats = mats;
        state.revision += 1;
        drop(state);
        self.wake();
        Ok(ActionOutput::json(json!({"saved": mat})))
    }

    fn delete_material(&self, name: &str) -> Result<ActionOutput, CommandError> {
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| CommandError::failed("Application state is unavailable."))?;
        // Shared lists (CutCraft-Web): held and read again, so no one else's edit is undone.
        let _lists = self.0.store.lock_lists().map_err(CommandError::failed)?;
        let mut library = if self.0.store.shares_lists() {
            self.0
                .store
                .load_materials()
                .map_err(CommandError::failed)?
        } else {
            state.materials.clone()
        };
        let before = library.len();
        library.retain(|m| !m.name.eq_ignore_ascii_case(name));
        if library.len() == before {
            return Err(CommandError::failed(format!(
                "There is no material {name:?}."
            )));
        }
        if library.is_empty() {
            return Err(CommandError::new("refused", "Keep at least one material."));
        }
        self.0
            .store
            .save_materials(&library)
            .map_err(CommandError::failed)?;
        state.materials = library;
        state.revision += 1;
        drop(state);
        self.wake();
        Ok(ActionOutput::json(json!({"deleted": name})))
    }
}

fn plan(design: &OpenDesign, number: usize) -> Result<cutcraft_path::CutPlan, CommandError> {
    let id = sheet_id(&design.project, number).map_err(CommandError::failed)?;
    cutcraft_path::plan_sheet(&design.project, &id).map_err(CommandError::failed)
}

fn plan_summary(plan: &cutcraft_path::CutPlan, number: usize) -> Value {
    let count = |kind: cutcraft_path::OperationKind| {
        plan.operations.iter().filter(|o| o.kind == kind).count()
    };
    use cutcraft_path::OperationKind as K;
    json!({
        "sheet": number,
        "name": plan.sheet_name,
        "plan_id": plan.plan_id,
        "valid": plan.valid,
        "material": plan.material.name,
        "mirror": plan.mirror,
        "mat": plan.mat,
        "bounds_mm": plan.bounds_mm,
        "design_size_mm": plan.design_size_mm,
        "cut_length_mm": plan.cut_length_mm,
        "travel_length_mm": plan.travel_length_mm,
        "estimate_s": plan.estimate_s,
        "cuts": {"holes": count(K::Hole), "outlines": count(K::Outline), "lines": count(K::Line), "weeding_box": count(K::WeedingBox), "weed_lines": count(K::WeedLine), "marks": count(K::Mark)},
        "compensation": plan.compensation,
        "diagnostics": plan.diagnostics,
    })
}

/// A result object: each field's JSON type(s) and meaning. Extra fields may follow in later
/// versions, so objects stay open.
fn fields(list: &[(&str, &[&str], &str)]) -> Value {
    let properties: serde_json::Map<String, Value> = list
        .iter()
        .map(|(name, types, description)| {
            let kind = if types.len() == 1 {
                json!(types[0])
            } else {
                json!(types)
            };
            (
                (*name).to_owned(),
                json!({"type": kind, "description": description}),
            )
        })
        .collect();
    json!({"type": "object", "properties": properties})
}

fn array_of(item: Value) -> Value {
    json!({"type": "array", "items": item})
}

/// The AI connection and its access keys change only from the window (or its test control
/// port): an AI tool must not grant itself access (spec §6: the one exception to AI parity).
fn window_only(origin: Origin, what: &str) -> Result<(), CommandError> {
    if origin == Origin::Mcp {
        return Err(CommandError::new(
            "refused",
            format!("AI tools cannot {what}; the person does that in Edit ▸ AI Connection (MCP)…."),
        ));
    }
    Ok(())
}

/// What each command returns (spec §6.2.1: result schemas for `outputSchema`).
pub(crate) fn result_schema(id: &str) -> Value {
    const S: &[&str] = &["string"];
    const SN: &[&str] = &["string", "null"];
    const I: &[&str] = &["integer"];
    const N: &[&str] = &["number"];
    const B: &[&str] = &["boolean"];
    const A: &[&str] = &["array"];
    const O: &[&str] = &["object"];
    const ON: &[&str] = &["object", "null"];
    const AN: &[&str] = &["array", "null"];
    let inspection = || {
        fields(&[
            ("schema_version", I, "Report format version."),
            ("engine_revision", S, "The pinned VectorCraft revision."),
            ("source_sha256", S, "Fingerprint of the file as read."),
            ("format", S, "Detected file format."),
            ("cut_ready", B, "Always false here; see prepare.report."),
            (
                "preview_only",
                B,
                "The file can be viewed but not cut as is.",
            ),
            ("pages", A, "Pages with their size in mm."),
            (
                "objects",
                A,
                "Imported objects with source ids, kinds and paint.",
            ),
            ("fonts", A, "Fonts used and whether each is available."),
            ("diagnostics", A, "Import findings with stable codes."),
        ])
    };
    let prepared = || {
        fields(&[
            ("schema_version", I, "Result format version."),
            ("stage", S, "\"preparation\"."),
            ("cut_ready", B, "Nothing blocking or unanswered."),
            ("source_sha256", S, "Fingerprint of the source."),
            (
                "recipe",
                O,
                "The answers applied, replayed from the original.",
            ),
            ("fonts", A, "Fonts and how each was resolved."),
            ("page", ON, "The page cut, with its size in mm."),
            (
                "pieces",
                A,
                "Filled pieces to cut out, with colour, layer, outline and holes.",
            ),
            ("cut_lines", A, "Lines to cut along."),
            (
                "changes",
                A,
                "The plain-language change list, with alternative choices.",
            ),
            (
                "pending",
                A,
                "Questions to answer, each with its allowed choices.",
            ),
            (
                "diagnostics",
                A,
                "Findings with stable codes; blocking ones stop cutting.",
            ),
            (
                "answered_by",
                A,
                "Answers that came from automation, with their origin.",
            ),
        ])
    };
    let sheet = || {
        fields(&[
            ("material", S, "Material name."),
            ("mirror", B, "The cut is mirrored."),
            ("mat", S, "mat12x12 or mat12x24."),
            (
                "placement",
                O,
                "x and y in mm from the start corner, rotate in degrees.",
            ),
            ("weeding_box", B, "A weeding box is cut around the artwork."),
            ("weed_lines", B, "Weed lines are cut through the waste."),
            (
                "weed_inverted",
                B,
                "Inverted: the background is kept and the shapes are the waste.",
            ),
            (
                "layering_marks",
                B,
                "Layering marks are cut, at the same place on every sheet with them.",
            ),
            ("mat_profile", S, "The mat shown in the Cut preview."),
            (
                "test_spot",
                ON,
                "Where they put the test cut (x_mm, y_mm), or null when it is placed automatically.",
            ),
            (
                "piece",
                ON,
                "Their piece of material (width_mm, height_mm, and x_mm, y_mm from the start corner), if given.",
            ),
        ])
    };
    match id {
        "app.status" => fields(&[
            ("app_version", S, "CutCraft version."),
            ("revision", I, "Application revision."),
            (
                "design",
                ON,
                "The open design: path, inspection, recipe, prepared result.",
            ),
            ("loading_path", SN, "A design being opened."),
            ("last_error", SN, "The last failure shown."),
            ("mcp", O, "MCP settings."),
            ("mcp_service", O, "MCP service state."),
            (
                "history",
                O,
                "Undo and redo steps with their labels and origins.",
            ),
        ]),
        "design.open" | "design.inspection" => inspection(),
        "design.close" => fields(&[("closed", B, "The design was closed.")]),
        "project.open" => fields(&[
            ("project", S, "The project file."),
            ("design", S, "Where its design came from."),
            ("cut_ready", B, "The prepared design is cut-ready."),
        ]),
        "project.save" => fields(&[("saved_to", S, "The project file written.")]),
        "project.inspect" => fields(&[
            (
                "source",
                O,
                "Design path, project path, fingerprint, and status: unchanged, changed (offer project.update) or missing.",
            ),
            ("page", ON, "The page cut."),
            ("cut_ready", B, "Nothing blocking or unanswered."),
            ("pending", I, "Questions still to answer."),
            ("blocking", A, "Blocking diagnostic codes."),
            ("pieces", I, "Pieces to cut out."),
            ("cut_lines", I, "Lines to cut along."),
            ("rows", A, "Rows as rows.list reports them."),
            ("sheet", ON, "The sheet's settings and plan validity."),
            ("revision", I, "Application revision."),
        ]),
        "prepare.report" | "prepare.decide" | "prepare.reset" => prepared(),
        "rows.list" => array_of(fields(&[
            ("row", O, "The row key, to pass back to cut.include."),
            ("label", S, "Colour hex, swatch name or layer."),
            ("included", B, "It is cut."),
            ("pieces", I, "Pieces in the row."),
            ("cut_lines", I, "Cut lines in the row."),
        ])),
        "cut.include" => fields(&[("row", O, "The row."), ("included", B, "It is cut.")]),
        "sheet.set" | "sheet.place" | "sheet.piece" | "sheet.test_spot" => sheet(),
        "materials.list" => schema::<Vec<cutcraft_import::job::Material>>(),
        "mats.list" => schema::<Vec<cutcraft_import::job::MatProfile>>(),
        "settings.units" => fields(&[("units", S, "inches or millimetres.")]),
        "settings.font_size" => fields(&[("points", N, "Base interface font size in points.")]),
        "settings.snap" => schema::<crate::Snap>(),
        "settings.panel" => schema::<crate::PanelLayout>(),
        "settings.new_design" => schema::<crate::NewDesign>(),
        "mats.save" => fields(&[("saved", O, "The mat as saved.")]),
        "mats.delete" => fields(&[("deleted", S, "The mat removed.")]),
        "plan.sheet" => fields(&[
            ("sheet", I, "Sheet number."),
            ("name", S, "Sheet name."),
            (
                "plan_id",
                S,
                "Fingerprint of everything that determines the cut.",
            ),
            ("valid", B, "Nothing blocks cutting this plan."),
            ("material", S, "Material name."),
            ("mirror", B, "Mirrored."),
            ("mat", S, "Mat."),
            ("bounds_mm", AN, "Blade bounds [x0, y0, x1, y1] on the mat."),
            ("design_size_mm", AN, "Design width and height."),
            ("cut_length_mm", N, "Blade-down length."),
            ("travel_length_mm", N, "Blade-up travel."),
            ("estimate_s", N, "Estimated time from hypothesised speeds."),
            (
                "cuts",
                O,
                "Counts of holes, outlines, lines, weeding box, weed lines and layering-mark strokes.",
            ),
            ("compensation", S, "Blade compensation in use."),
            ("diagnostics", A, "Plan findings with stable codes."),
        ]),
        "export" => fields(&[
            ("mime_type", S, "Content type."),
            ("plan_id", S, "The plan exported."),
            ("saved_to", S, "The file written, when a path was given."),
            ("bytes", I, "Bytes written."),
            ("dataBase64", S, "The content, when no path was given."),
        ]),
        "render.preview" => fields(&[
            ("width", I, "Pixels."),
            ("height", I, "Pixels."),
            ("px_per_mm", N, "Scale."),
            ("origin_px", A, "Where mat (0, 0) is, in pixels."),
            ("plan_id", S, "The plan drawn."),
            ("mirrored", B, "The sheet is mirrored."),
            ("cuts_shown", I, "Cuts drawn as already cut."),
            ("cuts_total", I, "Cuts in the plan."),
            ("highlighted", A, "Ids highlighted."),
            ("legend", A, "What each colour means."),
            ("sheet", I, "Sheet number."),
            ("valid", B, "The plan can be cut."),
        ]),
        "help.show" => fields(&[
            (
                "topic",
                S,
                "ai_setup, designs, troubleshooting, about or manual.",
            ),
            ("help", S, "The guide's Markdown."),
        ]),
        "view.capture" => fields(&[
            ("width", I, "Pixels."),
            ("height", I, "Pixels."),
            ("mime_type", S, "image/png."),
            ("saved_to", SN, "The file written, if any."),
        ]),
        "settings.mcp_key_create" => fields(&[
            ("name", S, "The key's name."),
            ("key", S, "The key, shown this once."),
            ("url", S, "The address AI tools connect to."),
            ("message", S, "What to do with it."),
        ]),
        "settings.mcp_key_delete" => fields(&[("deleted", S, "The key's name.")]),
        "settings.mcp_port" => fields(&[
            ("address", S, "The address checked."),
            ("port", I, "The port checked."),
            (
                "use",
                O,
                "state: free, this-window, another-cutcraft (with pid), other-program or unavailable (with reason).",
            ),
        ]),
        "settings.mcp" => fields(&[
            (
                "settings",
                O,
                "enabled, port, address and (beyond this computer) the connection key.",
            ),
            ("service", O, "Service state, address and error."),
            ("reconnect", B, "Clients must reconnect."),
            (
                "remembered",
                B,
                "Saved for next time (false: this window only).",
            ),
            ("message", S, "What happens next."),
        ]),
        "app.engine_check" => fields(&[
            ("ok", B, "The engine check passed."),
            ("error", SN, "Why it failed."),
        ]),
        "device.discover" => fields(&[
            ("devices", A, "Candidate cutter USB descriptors."),
            ("commands_sent", B, "Always false: discovery sends nothing."),
            ("firmware_queried", B, "Always false."),
        ]),
        "history.undo" | "history.redo" => fields(&[
            ("undone", A, "Steps undone, with label, command and origin."),
            ("redone", A, "Steps redone."),
            ("undo_steps", I, "Steps left to undo."),
            ("redo_steps", I, "Steps left to redo."),
        ]),
        "journal.list" => array_of(fields(&[
            ("seq", I, "Sequence number."),
            ("id", S, "Command id."),
            ("origin", S, "user, mcp or cli-run."),
            ("params", O, "Parameters, with file paths as names only."),
            ("revision_before", I, "Revision before."),
            ("revision_after", I, "Revision after."),
            ("ok", B, "It succeeded."),
            ("code", SN, "Failure code."),
        ])),
        "design.size" => fields(&[
            (
                "scale",
                N,
                "The design's size as a factor of its imported size.",
            ),
            ("size_mm", AN, "The whole design's width and height."),
        ]),
        "rows.group" => fields(&[("group_by", S, "colour or layer.")]),
        "sheet.adjust" => fields(&[
            ("sheet", I, "Sheet number, from 1."),
            ("material", S, "Material name."),
            ("adjusted", O, "The sheet's material with its new values."),
        ]),
        "materials.save" | "materials.save_current" => {
            fields(&[("saved", O, "The saved material settings.")])
        }
        "materials.delete" => fields(&[("deleted", S, "The material removed.")]),
        "device.connect" => fields(&[
            (
                "device",
                S,
                "Simulated Cameo, Silhouette Cameo, or a cameo-pi cutter's name (on its Pi).",
            ),
            ("station", S, "remote: the cameo-pi's address."),
            ("cutter", S, "remote: the cutter's id on the Pi."),
            (
                "online",
                B,
                "remote: the cutter is plugged in and answering.",
            ),
            ("practice", B, "remote: the Pi's practice cutter."),
            ("remote", B, "A cutter on a cameo-pi."),
            (
                "firmware",
                S,
                "What the cutter reported to FG; the Simulated Cameo answers as the tested machine.",
            ),
            (
                "hardware_testing",
                B,
                "Hardware testing is on: a real cutter may also cut from a roll.",
            ),
            ("scenario", S, "The simulator scenario."),
            ("clock", S, "realtime or virtual."),
            ("media", SN, "What the simulated cutter has loaded."),
            (
                "simulated",
                B,
                "True for the Simulated Cameo: a model of the machine, not the machine.",
            ),
        ]),
        "device.disconnect" => fields(&[("disconnected", B, "The cutter was disconnected.")]),
        "station.pair" => fields(&[
            ("address", S, "The Pi's address, as given."),
            (
                "code",
                S,
                "The code the Pi's web page shows beside the request.",
            ),
            (
                "fingerprint",
                S,
                "The Pi's certificate, pinned from here on.",
            ),
            ("state", S, "waiting, paired, denied, expired or failed."),
            ("message", SN, "What to do, or why it failed."),
        ]),
        "station.discover" => fields(&[(
            "found",
            A,
            "Each Pi found: name, address (to pass to station.pair), ips, paired.",
        )]),
        "station.list" => fields(&[(
            "stations",
            A,
            "Each paired Pi: address, fingerprint, version, cutters (id, name, online, practice) or the error reaching it.",
        )]),
        "station.forget" => fields(&[
            ("forgotten", S, "The address forgotten."),
            (
                "pairing_cancelled",
                B,
                "A pairing request to it was cancelled.",
            ),
        ]),
        "station.library" => fields(&[
            ("folder", A, "The folder listed."),
            ("folders", A, "Its folders."),
            ("jobs", A, "Its jobs: id, name, notes, revision, times_cut."),
        ]),
        "station.open" => fields(&[
            ("path", S, "Where the project was saved and opened from."),
            ("job_id", S, "The job it came from."),
            ("revision", I, "Its revision."),
        ]),
        "cut.confirm_fresh" => fields(&[("answer", S, "What cameo-pi answered.")]),
        "cut.send" => fields(&[
            ("station", S, "The Pi it went to."),
            (
                "folder",
                AN,
                "The library folder; null for a new revision, which stays where its job is.",
            ),
            ("job_id", S, "The job on the Pi."),
            ("name", S, "The job's name."),
            ("revision", I, "Its revision: 1 for a new job."),
        ]),
        "device.status" | "device.wait" => fields(&[
            ("device", SN, "The connected cutter, if any."),
            ("scenario", SN, "The simulator scenario."),
            (
                "state",
                S,
                "disconnected, idle, awaiting-setup, ready, sending, draining, pausing, paused, stopping, completed, stopped or interrupted.",
            ),
            ("job", ON, "What is being cut, with its loading steps."),
            (
                "progress",
                ON,
                "Estimated progress; 1 only once the cutter reports idle.",
            ),
            ("message", SN, "The line shown to them."),
            ("completed", B, "The sheet was cut to completion."),
            (
                "interrupted",
                B,
                "The sheet's last cut was interrupted: what it cut is uncertain.",
            ),
            (
                "test_areas_mm",
                A,
                "Where each test cut on the loaded mat is; the main cut and later tests keep clear.",
            ),
            (
                "mat_unconfirmed",
                B,
                "A cameo-pi cutter's loaded mat wasn't seen going in: cut.confirm_fresh if it has fresh material.",
            ),
        ]),
        "device.load" => fields(&[("media", SN, "What is loaded now.")]),
        "cut.sheet" | "cut.test" => fields(&[
            ("kind", S, "sheet or test-cut."),
            ("plan_id", S, "The plan that will be sent."),
            ("material", O, "Material and its settings."),
            ("mirror", B, "Cut mirrored."),
            ("mat", S, "Mat."),
            ("instructions", A, "The loading steps to follow."),
            ("estimate_s", N, "Estimated time from hypothesised speeds."),
            ("bounds_mm", AN, "Where it cuts on the mat."),
            ("origin", S, "Who chose it."),
        ]),
        "cut.copy" => fields(&[
            ("project", O, "Current-sheet project."),
            ("plan", O, "Its cut plan."),
        ]),
        "cut.start" => fields(&[
            ("started", B, "The device worker started."),
            ("job", O, "What is being cut."),
        ]),
        "cut.pause" | "cut.resume" => {
            fields(&[("pause_requested", B, "Pause was asked for (false: resume).")])
        }
        "cut.stop" => fields(&[
            ("stopped", B, "Stop was requested."),
            ("sent", B, "Anything had been sent."),
        ]),
        "cut.exit" => fields(&[
            ("screen", S, "mat or side-by-side."),
            ("state", S, "idle or disconnected."),
        ]),
        "cut.dismiss" => fields(&[("state", S, "idle.")]),
        "cut.reset" => fields(&[
            ("completed", B, "Always false."),
            ("interrupted", B, "Always false."),
        ]),
        "prepare.run" => prepared(),
        "project.update" => fields(&[
            ("changed", B, "The design file had changed."),
            ("message", S, "Why nothing changed."),
            ("rows_added", A, "Rows that are new."),
            ("rows_removed", A, "Rows that are gone."),
            ("cut_ready", B, "The updated design is cut-ready."),
        ]),
        "view.inspect" => fields(&[
            ("screen", S, "side-by-side, overlay, mat, cut or test-cut."),
            ("view", O, "The screen shown."),
            ("dialog", ON, "An open dialog: name and choices."),
            ("windows", O, "Which helper windows are open."),
            ("focused", B, "The window has focus."),
            ("size", A, "Window size in points."),
            ("pixels_per_point", N, "Screenshot pixels per window point."),
            ("running_actions", I, "Actions still running."),
            ("message", SN, "The last error shown."),
            ("unsaved", B, "The project has unsaved changes."),
            ("cutting", O, "Cutting state and message."),
        ]),
        "view.show" => fields(&[
            ("screen", S, "side-by-side, overlay, mat, cut or test-cut."),
            (
                "adjusted",
                S,
                "Opening the Cut preview resized or moved cut lines that didn't fit on the mat or their material: what was done.",
            ),
        ]),
        _ => json!({"type": "object"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two colours and a cut line, authored for these tests.
    const DESIGN: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100"><rect x="10" y="10" width="30" height="30" fill="#ff0000"/><rect x="50" y="50" width="20" height="20" fill="#0000ff"/></svg>"##;

    fn headless(dir: &Path) -> (Controller, PathBuf, PathBuf) {
        let read = dir.join("in");
        let write = dir.join("out");
        std::fs::create_dir_all(&read).unwrap();
        std::fs::create_dir_all(&write).unwrap();
        std::fs::write(read.join("design.svg"), DESIGN).unwrap();
        let roots = Roots::new(Some(&read), Some(&write)).unwrap();
        (Controller::headless(roots), read, write)
    }

    fn run(c: &Controller, id: &str, params: Value) -> Result<Value, CommandError> {
        c.execute(Origin::Mcp, id, params, None).map(|o| o.value)
    }

    #[test]
    fn opening_the_cut_preview_fits_cut_lines_onto_the_mat() {
        let dir = tempfile::tempdir().unwrap();
        let (_, read, _) = headless(dir.path());
        let controller = Controller::new(crate::SettingsStore::new(dir.path().join("settings")));
        let check = |id: &str, params: Value| {
            let value = run(&controller, id, params).unwrap_or_else(|e| panic!("{id}: {e}"));
            conforms(&value, &result_schema(id), id).unwrap();
            value
        };
        check("design.open", json!({"path": read.join("design.svg")}));
        // Opening the Cut preview shrinks cut lines that are too big for the mat, and moves
        // ones that only reach off it; ones that fit stay as they are.
        check("design.size", json!({"width_mm": 3000.0}));
        let shown = check("view.show", json!({"screen": "mat"}));
        assert!(
            shown["adjusted"].as_str().unwrap().contains("shrunk"),
            "{shown}"
        );
        let design = controller.snapshot().unwrap().design.unwrap();
        let plan =
            cutcraft_path::plan_sheet(&design.project, &design.project.sheets[0].id).unwrap();
        assert!(plan.valid, "{:?}", plan.diagnostics);
        assert_eq!(
            controller
                .snapshot()
                .unwrap()
                .history
                .undo
                .last()
                .unwrap()
                .label,
            "Fit to the mat"
        );
        check("view.show", json!({"screen": "side-by-side"}));
        check("design.size", json!({"width_mm": 120.0}));
        check("sheet.place", json!({"x": 250.0}));
        let shown = check("view.show", json!({"screen": "mat"}));
        assert!(
            shown["adjusted"].as_str().unwrap().contains("moved"),
            "{shown}"
        );
        let size =
            cutcraft_path::design_size_mm(&controller.snapshot().unwrap().design.unwrap().project)
                .unwrap();
        assert!((size[0] - 120.0).abs() < 0.01);
        check("view.show", json!({"screen": "side-by-side"}));
        assert!(
            check("view.show", json!({"screen": "mat"}))
                .get("adjusted")
                .is_none()
        );
        check("view.show", json!({"screen": "side-by-side"}));
    }

    #[test]
    fn the_panel_arrangement_is_saved_and_unknown_groups_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::SettingsStore::new(dir.path().to_owned());
        let controller = Controller::build(store, Roots::default(), false);
        let panel = run(
            &controller,
            "settings.panel",
            json!({"order": ["Estimate", "Issues", "Estimate"], "locked": ["Issues", "Estimate"]}),
        )
        .unwrap();
        // The rest follow in their usual order; locked groups are kept in panel order.
        assert_eq!(panel["order"][0], "Estimate");
        assert_eq!(panel["order"][1], "Issues");
        assert_eq!(panel["order"][2], "Info");
        assert_eq!(
            panel["order"].as_array().unwrap().len(),
            crate::PANEL_GROUPS.len()
        );
        assert_eq!(panel["locked"], json!(["Estimate", "Issues"]));
        let reloaded = Controller::build(
            crate::SettingsStore::new(dir.path().to_owned()),
            Roots::default(),
            false,
        );
        assert_eq!(
            reloaded.snapshot().unwrap().panel.locked,
            ["Estimate", "Issues"]
        );
        // Open and closed groups are remembered too; leaving `open` out keeps them.
        run(
            &controller,
            "settings.panel",
            json!({"open": {"Material": false, "Cutting aids": true}}),
        )
        .unwrap();
        run(&controller, "settings.panel", json!({"locked": []})).unwrap();
        let reloaded = Controller::build(
            crate::SettingsStore::new(dir.path().to_owned()),
            Roots::default(),
            false,
        );
        let open = reloaded.snapshot().unwrap().panel.open;
        assert_eq!(open.get("Material"), Some(&false));
        assert_eq!(open.get("Cutting aids"), Some(&true));
        assert_eq!(
            run(
                &controller,
                "settings.panel",
                json!({"open": {"Sidebar": true}})
            )
            .unwrap_err()
            .code,
            "invalid-params"
        );
        assert_eq!(
            run(
                &controller,
                "settings.panel",
                json!({"locked": ["Sidebar"]})
            )
            .unwrap_err()
            .code,
            "invalid-params"
        );
    }

    #[test]
    fn every_command_has_schemas_and_an_authority_and_automation_reaches_cutting() {
        let controller = Controller::headless(Roots::default());
        let commands = controller.commands();
        assert!(commands.len() > 20);
        for c in &commands {
            assert_eq!(c.params["type"], "object", "{}", c.id);
            assert!(
                ["object", "array"].contains(&c.result["type"].as_str().unwrap()),
                "{}",
                c.id
            );
        }
        let decide = commands.iter().find(|c| c.id == "prepare.decide").unwrap();
        assert!(decide.params.to_string().contains("font_from_file"));
        for id in [
            "cut.sheet",
            "cut.test",
            "cut.start",
            "cut.pause",
            "cut.resume",
            "device.load",
        ] {
            let info = commands.iter().find(|c| c.id == id).unwrap();
            assert_eq!(
                (info.authority, info.automation),
                (Authority::Motion, "allowed")
            );
            // Automation reaches cutting exactly as the person does: the same reasons to fail.
            for origin in [Origin::User, Origin::Mcp, Origin::CliRun] {
                let error = controller
                    .execute(origin, id, json!({}), None)
                    .err()
                    .unwrap();
                assert_ne!(error.code, "refused", "{id}");
            }
        }
    }

    #[test]
    fn unknown_keys_bad_types_and_unknown_commands_are_rejected_before_running() {
        let controller = Controller::headless(Roots::default());
        let code = |id, params| {
            controller
                .execute(Origin::Mcp, id, params, None)
                .err()
                .unwrap()
                .code
        };
        assert_eq!(
            code("materials.list", json!({"extra": 1})),
            "invalid-params"
        );
        assert_eq!(code("design.open", json!({"path": 3})), "invalid-params");
        assert_eq!(code("no.such", json!({})), "unknown-command");
        assert_eq!(code("rows.list", json!({})), "disabled");
        assert_eq!(code("help.show", json!({})), "disabled");
    }

    #[test]
    fn optional_roots_limit_automation_and_nothing_writes_over_the_design() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, write) = headless(dir.path());
        let outside = dir.path().join("elsewhere.svg");
        std::fs::write(&outside, DESIGN).unwrap();
        let error = run(&controller, "design.open", json!({"path": outside}))
            .err()
            .unwrap();
        assert_eq!(error.code, "outside-roots");
        let error = run(&controller, "design.open", json!({"path": "relative.svg"}))
            .err()
            .unwrap();
        assert_eq!(error.code, "invalid-params");
        run(
            &controller,
            "design.open",
            json!({"path": read.join("design.svg")}),
        )
        .unwrap();

        let error = run(
            &controller,
            "project.save",
            json!({"path": read.join("p.cutcraft")}),
        )
        .err()
        .unwrap();
        assert_eq!(error.code, "outside-roots");
        let error = run(
            &controller,
            "export",
            json!({"kind": "preview-svg", "path": read.join("design.svg")}),
        )
        .err()
        .unwrap();
        assert_eq!(error.code, "outside-roots");
        let saved = write.join("p.cutcraft");
        run(&controller, "project.save", json!({"path": saved})).unwrap();
        let error = run(&controller, "project.save", json!({"path": saved}))
            .err()
            .unwrap();
        assert!(error.message.contains("overwrite"), "{error}");
        run(
            &controller,
            "project.save",
            json!({"path": saved, "overwrite": true}),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(read.join("design.svg")).unwrap(),
            DESIGN
        );

        // A session without roots can do neither.
        // Without roots, automation reads and writes anywhere the person can.
        let none = Controller::headless(Roots::default());
        run(&none, "design.open", json!({"path": outside})).unwrap();
        run(
            &none,
            "project.save",
            json!({"path": dir.path().join("free.cutcraft")}),
        )
        .unwrap();
    }

    #[test]
    fn the_sheet_is_chosen_planned_saved_and_reopened_through_commands() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, write) = headless(dir.path());
        run(
            &controller,
            "design.open",
            json!({"path": read.join("design.svg")}),
        )
        .unwrap();
        let rows = run(&controller, "rows.list", json!({})).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 2);
        let blue = rows[1]["row"].clone();

        assert!(
            rows.as_array()
                .unwrap()
                .iter()
                .all(|r| r["included"] == true),
            "every colour is cut by default"
        );

        let revision = controller.snapshot().unwrap().revision;
        let error = controller
            .execute(
                Origin::Mcp,
                "cut.include",
                json!({"row": blue, "include": false}),
                Some(revision + 5),
            )
            .err()
            .unwrap();
        assert_eq!(error.code, "revision-mismatch");
        let unticked = controller
            .execute(
                Origin::Mcp,
                "cut.include",
                json!({"row": blue, "include": false}),
                Some(revision),
            )
            .unwrap();
        assert_eq!(unticked.revision, revision + 1);
        let only_red = run(&controller, "plan.sheet", json!({})).unwrap();
        assert_eq!(only_red["cuts"]["outlines"], 1);
        run(
            &controller,
            "cut.include",
            json!({"row": blue, "include": true}),
        )
        .unwrap();

        let set = run(
            &controller,
            "sheet.set",
            json!({"material": "Siser EasyWeed"}),
        )
        .unwrap();
        assert_eq!(set["mirror"], true);
        run(&controller, "sheet.place", json!({"x": 20, "rotate": 90})).unwrap();
        let plan = run(&controller, "plan.sheet", json!({})).unwrap();
        assert_eq!(plan["valid"], true, "{plan}");
        assert_eq!(plan["mirror"], true);
        assert_eq!(plan["cuts"]["outlines"], 2);
        let svg = run(&controller, "export", json!({"kind": "preview-svg"})).unwrap();
        assert_eq!(svg["plan_id"], plan["plan_id"]);
        run(
            &controller,
            "export",
            json!({"kind": "cutter-file", "path": write.join("sheet.gpgl")}),
        )
        .unwrap();
        assert!(
            std::fs::read(write.join("sheet.gpgl"))
                .unwrap()
                .starts_with(b"FX")
        );

        let saved = write.join("p.cutcraft");
        run(&controller, "project.save", json!({"path": saved})).unwrap();
        std::fs::remove_file(read.join("design.svg")).unwrap();
        let other = Controller::headless(Roots::new(Some(&write), Some(&write)).unwrap());
        let opened = run(&other, "project.open", json!({"path": saved})).unwrap();
        assert_eq!(opened["cut_ready"], true);
        let replanned = run(&other, "plan.sheet", json!({})).unwrap();
        assert_eq!(replanned["plan_id"], plan["plan_id"]);
        let summary = run(&other, "project.inspect", json!({})).unwrap();
        assert_eq!(summary["sheet"]["plan"]["valid"], true);
        assert_eq!(summary["sheet"]["material"], set["material"]);
    }

    #[test]
    fn dxf_opens_plans_and_round_trips_with_unit_answers_through_shared_commands() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("authored.dxf");
        let authored = include_str!("../../import/tests/fixtures/one-inch.dxf");
        std::fs::write(&source, authored.replace("9\n$INSUNITS\n70\n4\n", "")).unwrap();
        let controller = Controller::headless(Roots::default());
        run(&controller, "design.open", json!({"path":source})).unwrap();
        let pending = run(&controller, "prepare.report", json!({})).unwrap();
        assert_eq!(pending["pending"][0]["decision"], "dxf_units");
        run(
            &controller,
            "prepare.decide",
            json!({"decision":{"decision":"dxf_units","unit":"mm"}}),
        )
        .unwrap();
        assert!(
            controller
                .snapshot()
                .unwrap()
                .design
                .unwrap()
                .project
                .prepared
                .cut_ready
        );
        let planned = run(&controller, "plan.sheet", json!({})).unwrap();
        assert_eq!(planned["valid"], true);
        for i in 0..2 {
            assert!((planned["design_size_mm"][i].as_f64().unwrap() - 25.4).abs() < 1e-4);
        }
        let saved = dir.path().join("authored.cutcraft");
        run(&controller, "project.save", json!({"path":saved})).unwrap();
        run(&controller, "design.close", json!({})).unwrap();
        run(&controller, "project.open", json!({"path":saved})).unwrap();
        let snapshot = controller.snapshot().unwrap();
        let design = snapshot.design.unwrap();
        assert_eq!(design.inspection["format"], "dxf");
        assert!(design.project.prepared.cut_ready);
        assert_eq!(design.project.prepared.cut_lines.len(), 2);
        assert!(
            design
                .recipe
                .decisions
                .iter()
                .any(|d| matches!(d, Decision::DxfUnits { .. }))
        );
        assert_eq!(
            run(&controller, "plan.sheet", json!({})).unwrap()["valid"],
            true
        );
        let preview = run(
            &controller,
            "render.preview",
            json!({"view":"source","max_side":128}),
        )
        .unwrap();
        assert!(preview["width"].as_u64().unwrap() > 0);
    }

    #[test]
    fn outlines_on_filled_shapes_are_a_design_wide_answer_with_undo() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        std::fs::write(
            read.join("outlined.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100"><rect x="10" y="10" width="20" height="20" fill="black" stroke="red" stroke-width="2"/></svg>"#,
        )
        .unwrap();
        run(
            &controller,
            "design.open",
            json!({"path": read.join("outlined.svg")}),
        )
        .unwrap();
        let pieces = |c: &Controller| {
            run(c, "prepare.report", json!({})).unwrap()["pieces"]
                .as_array()
                .unwrap()
                .len()
        };
        let report = run(&controller, "prepare.report", json!({})).unwrap();
        assert_eq!(report["stroked_fills"], 1);
        assert_eq!(pieces(&controller), 1, "part of the shape by default");
        let separately = json!({"decision": {"decision": "outlines_on_fills", "role": "shape"}});
        run(&controller, "prepare.decide", separately.clone()).unwrap();
        assert_eq!(pieces(&controller), 2, "the red outline is cut too");
        // Choosing again replaces the answer rather than repeating it.
        run(&controller, "prepare.decide", separately).unwrap();
        let recipe = &run(&controller, "prepare.report", json!({})).unwrap()["recipe"];
        assert_eq!(recipe["decisions"].as_array().unwrap().len(), 1);
        run(&controller, "history.undo", json!({"steps": 2})).unwrap();
        assert_eq!(pieces(&controller), 1);
    }

    #[test]
    fn answering_a_question_keeps_the_sheet_choices() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        std::fs::write(
            read.join("bg.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100"><rect width="100" height="100" fill="white"/><rect x="10" y="10" width="20" height="20"/></svg>"#,
        )
        .unwrap();
        run(
            &controller,
            "design.open",
            json!({"path": read.join("bg.svg")}),
        )
        .unwrap();
        let report = run(&controller, "prepare.report", json!({})).unwrap();
        let keep = report["pending"][0]["choices"][1].clone();
        run(&controller, "prepare.decide", json!({"decision": keep})).unwrap();
        run(&controller, "sheet.set", json!({"material": "Removable"})).unwrap();
        // Answering again (the same answer) prepares again; the sheet keeps its material.
        run(&controller, "prepare.decide", json!({"decision": keep})).unwrap();
        let summary = run(&controller, "project.inspect", json!({})).unwrap();
        assert!(
            summary["sheet"]["material"]
                .as_str()
                .unwrap()
                .starts_with("Removable")
        );
    }

    #[test]
    fn batches_pass_results_forward_and_atomic_failures_roll_back() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        let step = |id: &str, params: Value| BatchStep {
            id: id.into(),
            params,
        };
        let outcome = controller
            .execute_batch(
                Origin::CliRun,
                &[
                    step("design.open", json!({"path": read.join("design.svg")})),
                    step("rows.list", json!({})),
                    step("cut.include", json!({"row": "$1.1.row", "include": false})),
                ],
                true,
                false,
            )
            .unwrap();
        assert!(outcome.ok, "{outcome:?}");
        assert_eq!(outcome.steps[2].result.as_ref().unwrap()["included"], false);

        let before = run(&controller, "project.inspect", json!({})).unwrap();
        let outcome = controller
            .execute_batch(
                Origin::CliRun,
                &[
                    step("sheet.set", json!({"material": "Siser Glitter"})),
                    step("sheet.set", json!({"material": "No such material"})),
                    step("rows.list", json!({})),
                ],
                true,
                true,
            )
            .unwrap();
        assert!(!outcome.ok && outcome.rolled_back);
        assert_eq!(outcome.steps.len(), 2);
        let after = run(&controller, "project.inspect", json!({})).unwrap();
        assert_eq!(after["sheet"], before["sheet"]);

        let bad = controller
            .execute_batch(
                Origin::Mcp,
                &[step("rows.list", json!({"x": "$4.a"}))],
                true,
                false,
            )
            .unwrap();
        assert_eq!(bad.steps[0].error.as_ref().unwrap().code, "invalid-params");
        let cut = controller
            .execute_batch(Origin::Mcp, &[step("cut.sheet", json!({}))], true, false)
            .unwrap();
        // Cutting from a batch fails for the person's reason: no cutter is connected.
        assert_eq!(cut.steps[0].error.as_ref().unwrap().code, "disabled");
    }

    #[test]
    fn render_preview_returns_a_png_and_its_legend() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        run(
            &controller,
            "design.open",
            json!({"path": read.join("design.svg")}),
        )
        .unwrap();
        let output = controller
            .execute(
                Origin::Mcp,
                "render.preview",
                json!({"max_side": 256, "view": "tool-motion", "order": 0.5}),
                None,
            )
            .unwrap();
        let image = output.image.unwrap();
        assert!(image.png.starts_with(b"\x89PNG"));
        assert_eq!(image.width.max(image.height), 256);
        assert_eq!(
            output.value["plan_id"],
            run(&controller, "plan.sheet", json!({})).unwrap()["plan_id"]
        );
        assert!(output.value["px_per_mm"].as_f64().unwrap() > 0.0);
        let assembly = controller
            .execute(
                Origin::Mcp,
                "render.preview",
                json!({"view": "assembly", "max_side": 128}),
                None,
            )
            .unwrap();
        assert_eq!(assembly.value["plan_id"], "assembly");
        assert!(assembly.image.unwrap().png.starts_with(b"\x89PNG"));
    }

    #[test]
    fn edits_undo_and_redo_with_their_origin_and_the_journal_records_them() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        std::fs::write(
            read.join("bg.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100"><rect width="100" height="100" fill="white"/><rect x="10" y="10" width="20" height="20"/></svg>"#,
        )
        .unwrap();
        let user = |id: &str, params: Value| {
            controller
                .execute(Origin::User, id, params, None)
                .map(|o| o.value)
        };
        user("design.open", json!({"path": read.join("bg.svg")})).unwrap();
        let report = run(&controller, "prepare.report", json!({})).unwrap();
        let dont_cut = report["pending"][0]["choices"][1].clone();
        // An assistant answers; the report marks it.
        run(&controller, "prepare.decide", json!({"decision": dont_cut})).unwrap();
        let report = run(&controller, "prepare.report", json!({})).unwrap();
        assert_eq!(report["cut_ready"], true);
        assert_eq!(report["answered_by"][0]["origin"], "mcp");
        // The person changes the material; then undoes both.
        user("sheet.set", json!({"material": "Removable"})).unwrap();
        let history = controller.snapshot().unwrap().history;
        let labels: Vec<_> = history
            .undo
            .iter()
            .map(|s| (s.command.as_str(), s.origin))
            .collect();
        assert_eq!(
            labels,
            [("prepare.decide", Origin::Mcp), ("sheet.set", Origin::User)]
        );
        let undone = user("history.undo", json!({"steps": 2})).unwrap();
        assert_eq!(undone["undone"][1]["origin"], "mcp");
        let report = run(&controller, "prepare.report", json!({})).unwrap();
        assert_eq!(report["cut_ready"], false);
        assert!(report["answered_by"].as_array().unwrap().is_empty());
        user("history.redo", json!({})).unwrap();
        assert_eq!(
            run(&controller, "prepare.report", json!({})).unwrap()["cut_ready"],
            true
        );
        assert_eq!(user("history.redo", json!({})).unwrap()["redo_steps"], 0);
        assert_eq!(
            user("history.redo", json!({})).err().unwrap().code,
            "disabled"
        );

        // An atomic batch is one step.
        let step = |id: &str, params: Value| BatchStep {
            id: id.into(),
            params,
        };
        let outcome = controller
            .execute_batch(
                Origin::Mcp,
                &[
                    step("sheet.set", json!({"material": "Siser Glitter"})),
                    step("sheet.place", json!({"x": 30})),
                ],
                true,
                true,
            )
            .unwrap();
        assert!(outcome.ok);
        let history = controller.snapshot().unwrap().history;
        assert_eq!(history.undo.last().unwrap().command, "batch");
        user("history.undo", json!({})).unwrap();
        let summary = run(&controller, "project.inspect", json!({})).unwrap();
        assert!(
            !summary["sheet"]["material"]
                .as_str()
                .unwrap()
                .starts_with("Siser Glitter")
        );

        // The journal names commands and origins, with file names instead of paths.
        let journal = user("journal.list", json!({"limit": 500})).unwrap();
        let open = journal
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == "design.open")
            .unwrap();
        assert_eq!(open["params"]["path"], "bg.svg");
        assert_eq!(open["origin"], "user");
        assert!(
            journal
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["id"] == "prepare.decide" && e["origin"] == "mcp")
        );
        // Opening another design starts a fresh history.
        user("design.open", json!({"path": read.join("design.svg")})).unwrap();
        assert!(controller.snapshot().unwrap().history.undo.is_empty());
    }

    /// Enough of JSON Schema to check our result schemas: types, properties and items.
    fn conforms(value: &Value, schema: &Value, path: &str) -> Result<(), String> {
        if let Some(kind) = schema.get("type") {
            let kinds: Vec<&str> = match kind {
                Value::String(k) => vec![k.as_str()],
                Value::Array(ks) => ks.iter().filter_map(Value::as_str).collect(),
                _ => vec![],
            };
            let actual = match value {
                Value::Null => "null",
                Value::Bool(_) => "boolean",
                Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Array(_) => "array",
                Value::Object(_) => "object",
            };
            let fits =
                kinds.contains(&actual) || (actual == "integer" && kinds.contains(&"number"));
            if !fits {
                return Err(format!("{path}: {actual}, expected {kinds:?}"));
            }
        }
        if let (Some(Value::Object(properties)), Value::Object(map)) =
            (schema.get("properties"), value)
        {
            for (key, sub) in properties {
                if let Some(v) = map.get(key) {
                    conforms(v, sub, &format!("{path}.{key}"))?;
                }
            }
        }
        if let (Some(items), Value::Array(values)) = (schema.get("items"), value) {
            for (i, v) in values.iter().enumerate() {
                conforms(v, items, &format!("{path}[{i}]"))?;
            }
        }
        Ok(())
    }

    #[test]
    fn results_match_their_declared_schemas() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, write) = headless(dir.path());
        let check = |id: &str, params: Value| {
            let value = controller
                .execute(Origin::Mcp, id, params, None)
                .unwrap()
                .value;
            conforms(&value, &result_schema(id), id).unwrap();
            value
        };
        check("design.open", json!({"path": read.join("design.svg")}));
        let rows = check("rows.list", json!({}));
        for (id, params) in [
            ("app.status", json!({})),
            ("design.inspection", json!({})),
            ("project.inspect", json!({})),
            ("prepare.report", json!({})),
            (
                "cut.include",
                json!({"row": rows[1]["row"], "include": false}),
            ),
            (
                "cut.include",
                json!({"row": rows[0]["row"], "include": true}),
            ),
            ("sheet.set", json!({"material": "Siser EasyWeed"})),
            ("sheet.place", json!({"x": 30})),
            ("materials.list", json!({})),
            ("plan.sheet", json!({})),
            ("export", json!({"kind": "preview-svg"})),
            (
                "export",
                json!({"kind": "plan", "path": write.join("plan.json")}),
            ),
            ("render.preview", json!({"max_side": 64})),
            ("history.undo", json!({})),
            ("history.redo", json!({})),
            ("project.save", json!({"path": write.join("p.cutcraft")})),
            ("journal.list", json!({})),
            ("app.engine_check", json!({})),
            ("design.close", json!({})),
        ] {
            check(id, params);
        }
        let free = Controller::headless(Roots::default());
        let opened = free
            .execute(
                Origin::Mcp,
                "project.open",
                json!({"path": write.join("p.cutcraft")}),
                None,
            )
            .unwrap()
            .value;
        conforms(&opened, &result_schema("project.open"), "project.open").unwrap();
    }

    #[test]
    fn concurrent_edits_at_one_revision_accept_exactly_one_and_keep_history() {
        use std::sync::{Arc, Barrier};

        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        run(
            &controller,
            "design.open",
            json!({"path": read.join("design.svg")}),
        )
        .unwrap();
        let revision = controller.snapshot().unwrap().revision;
        let barrier = Arc::new(Barrier::new(16));
        let threads: Vec<_> = (1..=16)
            .map(|force| {
                let controller = controller.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    controller.execute(
                        Origin::Mcp,
                        "sheet.adjust",
                        json!({"force": force}),
                        Some(revision),
                    )
                })
            })
            .collect();
        let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert!(
            results
                .iter()
                .filter_map(|r| r.as_ref().err())
                .all(|e| e.code == "revision-mismatch")
        );
        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.revision, revision + 1);
        assert_eq!(snapshot.history.undo.len(), 1);
        run(&controller, "history.undo", json!({})).unwrap();
        assert_eq!(
            controller
                .snapshot()
                .unwrap()
                .design
                .unwrap()
                .project
                .sheets[0]
                .material
                .force,
            10
        );
    }

    #[test]
    fn stop_does_not_wait_for_the_mutation_gate_even_in_an_atomic_batch() {
        let controller = Controller::headless(Roots::default());
        let _guard = controller.0.commands.lock().unwrap();
        // No device: disabled proves we reached Stop, rather than waiting on the held gate.
        assert_eq!(
            run(&controller, "cut.stop", json!({})).unwrap_err().code,
            "disabled"
        );
        let result = controller
            .execute_batch(
                Origin::Mcp,
                &[BatchStep {
                    id: "cut.stop".into(),
                    params: json!({}),
                }],
                true,
                true,
            )
            .unwrap();
        assert_eq!(result.steps[0].error.as_ref().unwrap().code, "disabled");
        assert!(
            !result.rolled_back,
            "Stop cannot roll back a concurrent project edit"
        );
        assert_eq!(controller.snapshot().unwrap().revision, 0);
    }

    #[cfg(feature = "dev-automation")]
    #[test]
    fn cut_and_test_screens_exit_without_sending_or_losing_the_loaded_mat_record() {
        let dir = tempfile::tempdir().unwrap();
        let (_, read, _) = headless(dir.path());
        let controller = Controller::new(crate::SettingsStore::new(dir.path().join("settings")));
        run(
            &controller,
            "design.open",
            json!({"path":read.join("design.svg")}),
        )
        .unwrap();
        for screen in [Screen::Cut, Screen::TestCut] {
            run(&controller, "view.show", json!({"screen":screen})).unwrap();
            let exit = run(&controller, "cut.exit", json!({})).unwrap();
            assert_eq!(exit["screen"], "mat");
            assert_eq!(exit["state"], "disconnected");
        }
        run(
            &controller,
            "device.connect",
            json!({"device":"simulated","clock":"virtual"}),
        )
        .unwrap();
        run(&controller, "cut.test", json!({})).unwrap();
        assert_eq!(controller.snapshot().unwrap().view.screen, Screen::TestCut);
        run(&controller, "cut.exit", json!({})).unwrap();
        assert!(controller.snapshot().unwrap().cutting.job.is_none());
        assert!(controller.virtual_mat().unwrap().segments.is_empty());
        assert!(run(&controller, "cut.start", json!({})).is_err());
        run(&controller, "cut.test", json!({})).unwrap();
        run(&controller, "view.show", json!({"screen":"cut"})).unwrap();
        assert!(
            controller.snapshot().unwrap().cutting.job.is_none(),
            "Switching workflows cancels the old reviewed pattern"
        );
        run(&controller, "cut.test", json!({})).unwrap();
        run(&controller, "cut.start", json!({})).unwrap();
        let done = wait_for(&controller, crate::CutState::Completed);
        assert!(!done.test_areas_mm.is_empty());
        run(&controller, "cut.exit", json!({})).unwrap();
        assert_eq!(
            controller.snapshot().unwrap().cutting.test_areas_mm,
            done.test_areas_mm
        );
        assert!(controller.snapshot().unwrap().cutting.job.is_none());
        controller
            .change(|s| {
                s.view.screen = Screen::TestCut;
                s.cutting.state = crate::CutState::Sending;
            })
            .unwrap();
        assert_eq!(
            run(&controller, "cut.exit", json!({})).unwrap_err().code,
            "busy"
        );
        assert_eq!(controller.snapshot().unwrap().view.screen, Screen::TestCut);
        assert_eq!(
            controller.snapshot().unwrap().cutting.state,
            crate::CutState::Sending
        );
    }

    // Needs the Simulated Cameo; never falls back to a real cutter.
    #[cfg(feature = "dev-automation")]
    #[test]
    fn test_cuts_reject_changed_settings_before_sending() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        run(
            &controller,
            "design.open",
            json!({"path": read.join("design.svg")}),
        )
        .unwrap();
        run(
            &controller,
            "device.connect",
            json!({"device": "simulated", "clock": "virtual"}),
        )
        .unwrap();
        for params in [
            json!({"force": 33}),
            json!({"speed": 1}),
            json!({"passes": 2}),
            json!({"blade": 2}),
        ] {
            run(&controller, "cut.test", json!({})).unwrap();
            run(&controller, "sheet.adjust", params).unwrap();
            assert_eq!(
                run(&controller, "cut.start", json!({})).unwrap_err().code,
                "failed"
            );
            let view = controller.snapshot().unwrap().cutting;
            assert_eq!(view.state, crate::CutState::Idle);
            assert!(view.job.is_none());
            assert!(controller.virtual_mat().unwrap().segments.is_empty());
        }
        let test = run(&controller, "cut.test", json!({})).unwrap();
        assert_eq!(test["material"]["force"], 33);
        run(&controller, "cut.start", json!({})).unwrap();
        assert_eq!(
            wait_for(&controller, crate::CutState::Completed).state,
            crate::CutState::Completed
        );
    }

    // Needs the Simulated Cameo; never falls back to a real cutter.
    #[cfg(feature = "dev-automation")]
    #[test]
    fn completed_sheets_are_kept_in_the_saved_project_until_started_over() {
        use crate::CutState;
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, write) = headless(dir.path());
        run(
            &controller,
            "design.open",
            json!({"path": read.join("design.svg")}),
        )
        .unwrap();
        let saved = write.join("design.cutcraft");
        run(&controller, "project.save", json!({"path": saved})).unwrap();
        run(
            &controller,
            "device.connect",
            json!({"device": "simulated", "clock": "virtual"}),
        )
        .unwrap();
        run(&controller, "cut.sheet", json!({})).unwrap();
        run(&controller, "cut.start", json!({})).unwrap();
        assert!(wait_for(&controller, CutState::Completed).completed);
        // An unsaved edit is not written by the record.
        run(&controller, "cut.dismiss", json!({})).unwrap();
        run(&controller, "sheet.set", json!({"mirror": true})).unwrap();
        let file = ProjectFile::open(&saved).unwrap();
        assert!(file.project.cut_record.is_completed("sheet/1"));
        assert!(!file.project.sheets[0].mirror);

        let roots = Roots::new(Some(&write), Some(&write)).unwrap();
        let reopened = Controller::headless(roots);
        run(&reopened, "project.open", json!({"path": saved})).unwrap();
        assert!(reopened.snapshot().unwrap().cutting.completed);
        let reset = run(&reopened, "cut.reset", json!({})).unwrap();
        conforms(&reset, &result_schema("cut.reset"), "cut.reset").unwrap();
        assert!(!reopened.snapshot().unwrap().cutting.completed);
        assert!(
            ProjectFile::open(&saved)
                .unwrap()
                .project
                .cut_record
                .is_empty()
        );
    }

    #[test]
    fn the_piece_size_is_set_both_ways_or_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        run(
            &controller,
            "design.open",
            json!({"path": read.join("design.svg")}),
        )
        .unwrap();
        let half = run(&controller, "sheet.piece", json!({"width_mm": 100.0}));
        assert_eq!(half.unwrap_err().code, "invalid-params");
        let set = run(
            &controller,
            "sheet.piece",
            json!({"width_mm": 152.4, "height_mm": 203.2}),
        )
        .unwrap();
        conforms(&set, &result_schema("sheet.piece"), "sheet.piece").unwrap();
        assert_eq!(set["piece"]["width_mm"], 152.4);
        let plan = run(&controller, "plan.sheet", json!({})).unwrap();
        assert!(plan["valid"].as_bool().unwrap(), "{plan}");
        let tiny = run(
            &controller,
            "sheet.piece",
            json!({"width_mm": 10.0, "height_mm": 10.0}),
        );
        assert!(tiny.is_ok());
        let plan = run(&controller, "plan.sheet", json!({})).unwrap();
        assert!(!plan["valid"].as_bool().unwrap());
        let cleared = run(&controller, "sheet.piece", json!({})).unwrap();
        assert!(cleared["piece"].is_null());
    }

    #[test]
    fn the_original_renders_and_the_preview_marks_the_test_cut_spot() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        run(
            &controller,
            "design.open",
            json!({"path": read.join("design.svg")}),
        )
        .unwrap();
        let source = controller
            .execute(
                Origin::Mcp,
                "render.preview",
                json!({"view": "source", "max_side": 256}),
                None,
            )
            .unwrap();
        assert_eq!(source.value["plan_id"], "source");
        let image = source.image.expect("a picture");
        assert_eq!(image.width.max(image.height), 256);
        let design = controller.snapshot().unwrap().design.unwrap();
        let plan =
            cutcraft_path::plan_sheet(&design.project, &design.project.sheets[0].id).unwrap();
        let cutting = controller.snapshot().unwrap().cutting;
        let marks = crate::test_cut_markers(&plan, &cutting);
        let [(area, done)] = marks[..] else {
            panic!("one spot for the provisional material: {marks:?}")
        };
        assert!(!done);
        let b = plan.bounds_mm.unwrap();
        assert!(
            area[0] >= b[2] || area[1] >= b[3],
            "beside the design: {area:?} vs {b:?}"
        );
        let mut tested = plan.clone();
        tested.material.status = cutcraft_import::job::Status::Tested;
        assert!(
            !crate::test_cut_markers(&tested, &cutting).is_empty(),
            "legacy status has no effect"
        );
    }

    #[test]
    fn a_design_saved_again_is_noticed_until_updated_and_a_missing_one_is_reported() {
        use crate::SourceStatus;
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        let design = read.join("design.svg");
        run(&controller, "design.open", json!({"path": design})).unwrap();
        assert_eq!(controller.watch_source(), SourceStatus::Unchanged);
        let edited = DESIGN.replace(
            "</svg>",
            "<rect x=\"1\" y=\"1\" width=\"2\" height=\"2\"/></svg>",
        );
        std::fs::write(&design, &edited).unwrap();
        assert_eq!(controller.watch_source(), SourceStatus::Changed);
        assert_eq!(controller.snapshot().unwrap().source, SourceStatus::Changed);
        let summary = run(&controller, "project.inspect", json!({})).unwrap();
        assert_eq!(summary["source"]["status"], "changed");
        run(&controller, "project.update", json!({})).unwrap();
        assert_eq!(controller.watch_source(), SourceStatus::Unchanged);
        std::fs::remove_file(&design).unwrap();
        assert_eq!(controller.watch_source(), SourceStatus::Missing);
    }

    #[cfg(feature = "dev-automation")]
    #[test]
    fn every_cut_leaves_a_detailed_log_beside_the_settings() {
        let dir = tempfile::tempdir().unwrap();
        let (_, read, write) = headless(dir.path());
        let settings = dir.path().join("settings");
        let roots = Roots::new(Some(&read), Some(&write)).unwrap();
        let controller =
            Controller::build(crate::SettingsStore::new(settings.clone()), roots, false);
        run(
            &controller,
            "design.open",
            json!({"path": read.join("design.svg")}),
        )
        .unwrap();
        run(
            &controller,
            "device.connect",
            json!({"device": "simulated", "clock": "virtual", "scenario": "slow-full-buffer"}),
        )
        .unwrap();
        run(&controller, "cut.sheet", json!({})).unwrap();
        run(&controller, "cut.start", json!({})).unwrap();
        let done = wait_for(&controller, crate::CutState::Completed);
        let log = done.last_log.expect("a log");
        assert!(log.starts_with(&*settings.join("cut-logs").to_string_lossy()));
        let lines: Vec<Value> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines[0]["ev"], "start");
        assert_eq!(lines[0]["scenario"], "slow-full-buffer");
        assert!(lines[0]["bytes"].as_u64().unwrap() > 0);
        assert!(
            lines
                .iter()
                .any(|l| l["ev"] == "write" && l["escape"] == "[1b, 05]")
        );
        assert!(lines.iter().any(|l| l["ev"] == "progress"));
        let last = lines.last().unwrap();
        assert_eq!(last["ev"], "outcome");
        assert_eq!(last["result"]["outcome"], "completed");
        assert!(
            !std::fs::read_to_string(&log)
                .unwrap()
                .contains("design.svg")
        );
    }

    #[test]
    fn move_onto_the_grid_brings_every_cut_inside_the_sticky_area() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        run(
            &controller,
            "design.open",
            json!({"path": read.join("design.svg")}),
        )
        .unwrap();
        let off_grid = |controller: &Controller| {
            run(controller, "plan.sheet", json!({})).unwrap()["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["code"] == "plan.off_mat_grid")
        };
        // Past the right-hand end of a 12 in grid.
        run(&controller, "sheet.place", json!({"x": 290})).unwrap();
        assert!(off_grid(&controller));
        run(&controller, "sheet.place", json!({"onto_grid": true})).unwrap();
        assert!(!off_grid(&controller));
        let plan = run(&controller, "plan.sheet", json!({})).unwrap();
        assert!(
            plan["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .all(|d| d["severity"] != "blocking"),
            "inside the cutting area too: {plan}"
        );
        let x = controller
            .snapshot()
            .unwrap()
            .design
            .unwrap()
            .project
            .sheets[0]
            .placement
            .offset_mm[0];
        assert!(x < 290.0, "moved only as far as needed: {x}");
    }

    #[cfg(feature = "dev-automation")]
    #[test]
    fn the_test_cut_can_be_placed_but_not_on_the_design() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        run(
            &controller,
            "design.open",
            json!({"path": read.join("design.svg")}),
        )
        .unwrap();
        run(
            &controller,
            "device.connect",
            json!({"device": "simulated", "clock": "virtual"}),
        )
        .unwrap();
        assert_eq!(
            run(&controller, "sheet.test_spot", json!({"x_mm": 10.0}))
                .unwrap_err()
                .code,
            "invalid-params"
        );
        let placed = run(
            &controller,
            "sheet.test_spot",
            json!({"x_mm": 200.0, "y_mm": 200.0}),
        )
        .unwrap();
        conforms(
            &placed,
            &result_schema("sheet.test_spot"),
            "sheet.test_spot",
        )
        .unwrap();
        assert_eq!(placed["test_spot"]["x_mm"], 200.0);
        let test = run(&controller, "cut.test", json!({})).unwrap();
        assert_eq!(test["bounds_mm"][0], 200.0);
        run(&controller, "cut.stop", json!({})).unwrap();
        // On the red square (placed at the start corner, 30 mm wide).
        let design = controller.snapshot().unwrap().design.unwrap();
        let [ox, oy] = design.project.sheets[0].placement.offset_mm;
        run(
            &controller,
            "sheet.test_spot",
            json!({"x_mm": ox + 2.0, "y_mm": oy + 2.0}),
        )
        .unwrap();
        let refused = run(&controller, "cut.test", json!({})).unwrap_err();
        assert!(
            refused.message.contains("cut into the design"),
            "{}",
            refused.message
        );
        let auto = run(&controller, "sheet.test_spot", json!({})).unwrap();
        assert!(auto["test_spot"].is_null());
    }

    #[test]
    fn mats_are_kept_chosen_per_sheet_and_the_material_sits_anywhere_on_them() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        run(
            &controller,
            "design.open",
            json!({"path": read.join("design.svg")}),
        )
        .unwrap();
        let mats = run(&controller, "mats.list", json!({})).unwrap();
        assert_eq!(mats.as_array().unwrap().len(), 3);
        let mut mine =
            cutcraft_import::job::MatProfile::for_class(cutcraft_import::job::Mat::Mat12x12);
        mine.name = "Teal mat".into();
        mine.rulers[1] = cutcraft_import::job::RulerUnit::Centimetres;
        let saved = run(&controller, "mats.save", json!({"mat": mine})).unwrap();
        conforms(&saved, &result_schema("mats.save"), "mats.save").unwrap();
        let mut broken = mine.clone();
        broken.sticky_mm[2] = 999.0;
        assert_eq!(
            run(
                &controller,
                "mats.save",
                json!({"mat": broken, "rename_from": "Teal mat"})
            )
            .unwrap_err()
            .code,
            "invalid-params"
        );
        let set = run(&controller, "sheet.set", json!({"mat_profile": "Teal"})).unwrap();
        assert_eq!(set["mat_profile"], "Teal mat");
        // Their 6 × 4 in piece, 1 in from the start corner; cut lines must sit on it.
        run(
            &controller,
            "sheet.piece",
            json!({"width_mm": 152.4, "height_mm": 101.6, "x_mm": 25.4, "y_mm": 25.4}),
        )
        .unwrap();
        let moved = run(
            &controller,
            "sheet.piece",
            json!({"x_mm": 0.0, "y_mm": 0.0}),
        )
        .unwrap();
        assert_eq!(moved["piece"]["width_mm"], 152.4, "moving keeps the size");
        assert_eq!(moved["piece"]["x_mm"], 0.0);
        assert_eq!(
            run(&controller, "sheet.piece", json!({"x_mm": 1.0}))
                .unwrap_err()
                .code,
            "invalid-params"
        );
        run(
            &controller,
            "sheet.piece",
            json!({"x_mm": 150.0, "y_mm": 150.0}),
        )
        .unwrap();
        let plan = run(&controller, "plan.sheet", json!({})).unwrap();
        assert!(
            !plan["valid"].as_bool().unwrap(),
            "the design is off the moved piece"
        );
        run(&controller, "mats.delete", json!({"name": "Teal mat"})).unwrap();
        let design = controller.snapshot().unwrap().design.unwrap();
        assert_eq!(
            design.project.sheets[0].mat_profile().name,
            "Teal mat",
            "the sheet keeps its copy"
        );
    }

    // Needs the Simulated Cameo; never falls back to a real cutter.
    #[cfg(feature = "dev-automation")]
    #[test]
    fn repeated_test_cuts_keep_clear_of_each_other_and_follow_the_moved_spot() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        run(
            &controller,
            "design.open",
            json!({"path": read.join("design.svg")}),
        )
        .unwrap();
        // Their piece has room for the design and several test cuts.
        run(
            &controller,
            "sheet.piece",
            json!({"width_mm": 250, "height_mm": 150}),
        )
        .unwrap();
        run(
            &controller,
            "device.connect",
            json!({"device": "simulated", "clock": "virtual"}),
        )
        .unwrap();
        let test = |_answer: &str| -> [f64; 4] {
            let job = run(&controller, "cut.test", json!({})).unwrap();
            run(&controller, "cut.start", json!({})).unwrap();
            wait_for(&controller, crate::CutState::Completed);
            run(&controller, "cut.dismiss", json!({})).unwrap();
            serde_json::from_value(job["bounds_mm"].clone()).unwrap()
        };
        let overlap =
            |a: [f64; 4], b: [f64; 4]| a[2] > b[0] && a[0] < b[2] && a[3] > b[1] && a[1] < b[3];
        let first = test("too-shallow");
        let job = run(&controller, "cut.test", json!({})).unwrap();
        assert!(
            job["instructions"][0]
                .as_str()
                .unwrap()
                .starts_with("Leave the mat loaded"),
            "{job}"
        );
        run(&controller, "cut.stop", json!({})).unwrap();
        let second = test("too-deep");
        assert!(!overlap(first, second), "{first:?} and {second:?}");
        // The preview marks both, and where the next one goes, clear of them.
        let design = controller.snapshot().unwrap().design.unwrap();
        let plan = cutcraft_path::plan_sheet(&design.project, "sheet/1").unwrap();
        let cutting = controller.snapshot().unwrap().cutting;
        let marks = crate::test_cut_markers(&plan, &cutting);
        assert_eq!(marks.iter().filter(|(_, done)| *done).count(), 2);
        let (next, _) = *marks.iter().find(|(_, done)| !done).expect("a next spot");
        assert!(!overlap(next, first) && !overlap(next, second));
        // They move the spot: the next test cut goes there.
        run(
            &controller,
            "sheet.test_spot",
            json!({"x_mm": 200, "y_mm": 110}),
        )
        .unwrap();
        let third = test("looks-good");
        assert_eq!([third[0], third[1]], [200.0, 110.0]);
        // A spot on an earlier test cut is never cut again: the next one goes clear of it.
        run(
            &controller,
            "sheet.test_spot",
            json!({"x_mm": first[0], "y_mm": first[1]}),
        )
        .unwrap();
        let fourth = test("looks-good");
        for earlier in [first, second, third] {
            assert!(!overlap(fourth, earlier), "{fourth:?} on {earlier:?}");
        }
    }

    /// Wait for the device worker to reach `state`.
    #[cfg(feature = "dev-automation")]
    fn wait_for(c: &Controller, state: crate::CutState) -> crate::CuttingView {
        for _ in 0..2000 {
            let view = c.snapshot().unwrap().cutting;
            if view.state == state {
                return view;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!(
            "never reached {state:?}: {:?}",
            c.snapshot().unwrap().cutting
        );
    }

    // Needs the Simulated Cameo; never falls back to a real cutter.
    #[cfg(feature = "dev-automation")]
    #[test]
    fn the_cutting_workflow_runs_against_the_simulated_cameo() {
        use crate::CutState;
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        let check = |id: &str, params: Value| {
            let value = run(&controller, id, params).unwrap_or_else(|e| panic!("{id}: {e}"));
            conforms(&value, &result_schema(id), id).unwrap();
            value
        };
        check("design.open", json!({"path": read.join("design.svg")}));
        assert_eq!(
            run(&controller, "cut.sheet", json!({})).unwrap_err().code,
            "disabled",
            "no cutter yet"
        );
        check(
            "device.connect",
            json!({"device": "simulated", "clock": "virtual"}),
        );
        // A test cut first: placed clear of the design, with loading steps.
        let test = check("cut.test", json!({}));
        assert_eq!(test["kind"], "test-cut");
        assert!(
            test["instructions"][0]
                .as_str()
                .unwrap()
                .contains("from the left")
        );
        check("cut.start", json!({}));
        let done = wait_for(&controller, CutState::Completed);
        assert_eq!(done.state, CutState::Completed);
        check("cut.dismiss", json!({}));

        // Sheet 1, then status reports it done.
        let job = check("cut.sheet", json!({}));
        assert_eq!(
            controller.snapshot().unwrap().cutting.state,
            CutState::AwaitingSetup
        );
        assert!(job["instructions"][1].as_str().unwrap().contains("blade"));
        check("cut.start", json!({}));
        let done = wait_for(&controller, CutState::Completed);
        assert!(done.completed);
        assert_eq!(done.progress.unwrap().fraction, 1.0);
        let status = check("device.status", json!({}));
        assert_eq!(status["state"], "completed");
        check("cut.dismiss", json!({}));

        // Again in real time: pause, edits wait, stop.
        check(
            "device.connect",
            json!({"device": "simulated", "clock": "realtime"}),
        );
        check("cut.sheet", json!({}));
        check("cut.start", json!({}));
        check("cut.pause", json!({}));
        wait_for(&controller, CutState::Paused);
        let busy = run(&controller, "sheet.set", json!({"mirror": true}));
        assert_eq!(busy.unwrap_err().code, "busy");
        assert_eq!(
            run(&controller, "design.close", json!({}))
                .unwrap_err()
                .code,
            "busy"
        );
        check("device.status", json!({}));
        let stop = check("cut.stop", json!({}));
        assert_eq!(stop["sent"], true);
        let stopped = wait_for(&controller, CutState::Stopped);
        assert!(stopped.message.unwrap().starts_with("Stopped."));
        assert!(stopped.completed, "the earlier complete cut still counts");
        let mat = controller.virtual_mat().unwrap();
        assert!(!mat.tool_down);
        check("cut.dismiss", json!({}));
        check("sheet.set", json!({"mirror": true}));
    }

    // Needs the Simulated Cameo; never falls back to a real cutter.
    #[cfg(feature = "dev-automation")]
    #[test]
    fn a_changed_sheet_is_not_sent_and_faults_interrupt() {
        use crate::CutState;
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        run(
            &controller,
            "design.open",
            json!({"path": read.join("design.svg")}),
        )
        .unwrap();
        run(
            &controller,
            "device.connect",
            json!({"device": "simulated", "scenario": "no-media"}),
        )
        .unwrap();
        run(&controller, "cut.sheet", json!({})).unwrap();
        // Edits are allowed while they load, but the changed plan is never sent.
        run(&controller, "sheet.place", json!({"x": 40})).unwrap();
        assert_eq!(
            run(&controller, "cut.start", json!({})).unwrap_err().code,
            "failed"
        );
        assert_eq!(controller.snapshot().unwrap().cutting.state, CutState::Idle);
        // Nothing loaded: back to the loading step, until the Load button is pressed.
        run(&controller, "cut.sheet", json!({})).unwrap();
        run(&controller, "cut.start", json!({})).unwrap();
        let view = wait_for(&controller, CutState::AwaitingSetup);
        assert!(view.message.unwrap().contains("nothing is loaded"));
        run(&controller, "device.load", json!({"media": "mat12"})).unwrap();
        run(&controller, "cut.start", json!({})).unwrap();
        wait_for(&controller, CutState::Completed);
        run(&controller, "cut.dismiss", json!({})).unwrap();

        run(
            &controller,
            "device.connect",
            json!({"device": "simulated", "scenario": "power-off-mid-cut"}),
        )
        .unwrap();
        run(&controller, "cut.sheet", json!({})).unwrap();
        run(&controller, "cut.start", json!({})).unwrap();
        let view = wait_for(&controller, CutState::Interrupted);
        assert!(view.interrupted && !view.completed);
        assert!(view.message.unwrap().contains("load fresh material"));
    }

    #[test]
    fn new_designs_start_with_the_chosen_material_and_mat() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("design.svg");
        std::fs::write(&source, DESIGN).unwrap();
        let store = crate::SettingsStore::new(dir.path().join("settings"));
        let controller = Controller::build(store.clone(), Roots::default(), false);
        let materials = controller.snapshot().unwrap().materials;
        let wanted = materials.last().unwrap().name.clone();
        let chosen = run(
            &controller,
            "settings.new_design",
            json!({"material": wanted.to_lowercase(), "mat": "Silhouette 12 × 24"}),
        )
        .unwrap();
        conforms(
            &chosen,
            &result_schema("settings.new_design"),
            "settings.new_design",
        )
        .unwrap();
        assert_eq!(chosen["material"], wanted.as_str());
        assert_eq!(chosen["mat"], "Silhouette 12 × 24 in mat");
        // Another material used in this session does not displace their choice.
        run(&controller, "design.open", json!({"path": source})).unwrap();
        run(
            &controller,
            "sheet.set",
            json!({"material": materials[0].name}),
        )
        .unwrap();
        let reloaded = Controller::build(store, Roots::default(), false);
        for c in [&controller, &reloaded] {
            run(c, "design.open", json!({"path": source})).unwrap();
            let sheet = c.snapshot().unwrap().design.unwrap().project.sheets[0].clone();
            assert_eq!(sheet.material.name, wanted);
            assert_eq!(sheet.mat, cutcraft_import::job::Mat::Mat12x24);
            assert_eq!(sheet.mat_profile().name, "Silhouette 12 × 24 in mat");
        }
        // Renaming the mat keeps it chosen; "" goes back to the usual start.
        let mut mat = reloaded.snapshot().unwrap().mats[1].clone();
        mat.name = "Long mat".into();
        run(
            &reloaded,
            "mats.save",
            json!({"mat": mat, "rename_from": "Silhouette 12 × 24 in mat"}),
        )
        .unwrap();
        assert_eq!(
            reloaded.snapshot().unwrap().new_design.mat.as_deref(),
            Some("Long mat")
        );
        // Saving from the editor can make it the default, or stop.
        let mut copy = reloaded.snapshot().unwrap().materials[0].clone();
        copy.name = "Shop vinyl".into();
        run(
            &reloaded,
            "materials.save",
            json!({"material": copy, "default": true}),
        )
        .unwrap();
        assert_eq!(
            reloaded.snapshot().unwrap().new_design.material.as_deref(),
            Some("Shop vinyl")
        );
        run(
            &reloaded,
            "materials.save",
            json!({"material": copy, "replace": true, "default": false}),
        )
        .unwrap();
        assert_eq!(reloaded.snapshot().unwrap().new_design.material, None);
        run(
            &reloaded,
            "settings.new_design",
            json!({"material": wanted}),
        )
        .unwrap();
        run(
            &reloaded,
            "mats.save",
            json!({"mat": mat, "rename_from": "Long mat", "default": false}),
        )
        .unwrap();
        run(&reloaded, "design.open", json!({"path": source})).unwrap();
        let sheet = reloaded.snapshot().unwrap().design.unwrap().project.sheets[0].clone();
        assert_eq!(sheet.mat, cutcraft_import::job::Mat::Mat12x12);
        assert_eq!(sheet.material.name, wanted);
        assert_eq!(
            run(
                &reloaded,
                "settings.new_design",
                json!({"material": "No such"})
            )
            .unwrap_err()
            .code,
            "invalid-params"
        );
    }

    #[test]
    fn material_adjustments_are_session_only_until_explicitly_saved() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("design.svg");
        std::fs::write(&source, DESIGN).unwrap();
        let store = crate::SettingsStore::new(dir.path().join("settings"));
        let controller = Controller::build(store.clone(), Roots::default(), false);
        run(&controller, "design.open", json!({"path":source})).unwrap();
        let presets = controller.snapshot().unwrap().materials;
        let original = presets[0].clone();
        run(
            &controller,
            "sheet.adjust",
            json!({"blade":3,"force":18,"speed":4,"passes":2,"mirror":true,"overcut_mm":0.4}),
        )
        .unwrap();
        let adjusted = controller
            .snapshot()
            .unwrap()
            .design
            .unwrap()
            .project
            .sheets[0]
            .material
            .clone();
        assert_eq!(
            (
                adjusted.blade,
                adjusted.force,
                adjusted.speed,
                adjusted.passes,
                adjusted.mirror,
                adjusted.overcut_mm
            ),
            (3, 18, 4, 2, true, 0.4)
        );
        assert_eq!(controller.snapshot().unwrap().materials, presets);
        run(&controller, "design.close", json!({})).unwrap();
        run(&controller, "design.open", json!({"path":source})).unwrap();
        assert_eq!(
            controller
                .snapshot()
                .unwrap()
                .design
                .unwrap()
                .project
                .sheets[0]
                .material,
            adjusted
        );
        let restarted = Controller::build(store.clone(), Roots::default(), false);
        assert_eq!(restarted.snapshot().unwrap().materials[0], original);
        run(
            &controller,
            "sheet.set",
            json!({"material":presets[1].name}),
        )
        .unwrap();
        run(&controller, "sheet.set", json!({"material":original.name})).unwrap();
        assert_eq!(
            controller
                .snapshot()
                .unwrap()
                .design
                .unwrap()
                .project
                .sheets[0]
                .material,
            original
        );
        run(
            &controller,
            "sheet.adjust",
            json!({"force":18,"mirror":true}),
        )
        .unwrap();
        run(&controller, "materials.save_current", json!({})).unwrap();
        let restarted = Controller::build(store, Roots::default(), false);
        assert_eq!(restarted.snapshot().unwrap().materials[0].force, 18);
        assert!(restarted.snapshot().unwrap().materials[0].mirror);
        run(&restarted, "design.open", json!({"path":source})).unwrap();
        let loaded = restarted.snapshot().unwrap().design.unwrap().project.sheets[0]
            .material
            .clone();
        assert_eq!(loaded.force, 18);
        assert!(loaded.mirror);
    }

    #[test]
    fn material_save_as_selects_the_copy_and_rejected_saves_keep_the_draft() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        run(
            &controller,
            "design.open",
            json!({"path":read.join("design.svg")}),
        )
        .unwrap();
        let original = controller.snapshot().unwrap().materials[0].clone();
        run(
            &controller,
            "sheet.adjust",
            json!({"force":21,"mirror":true}),
        )
        .unwrap();
        let before = controller.snapshot().unwrap();
        for name in [original.name.as_str(), " "] {
            assert!(run(&controller, "materials.save_current", json!({"name":name})).is_err());
            let after = controller.snapshot().unwrap();
            assert_eq!(after.materials, before.materials);
            assert_eq!(after.session_material, before.session_material);
            assert_eq!(after.revision, before.revision);
        }
        let saved = run(
            &controller,
            "materials.save_current",
            json!({"name":"My custom vinyl"}),
        )
        .unwrap();
        assert_eq!(saved["saved"]["force"], 21);
        assert!(saved["saved"].get("status").is_none());
        let snapshot = controller.snapshot().unwrap();
        assert_eq!(snapshot.materials[0], original);
        assert_eq!(
            snapshot.design.unwrap().project.sheets[0].material.name,
            "My custom vinyl"
        );
        assert_eq!(snapshot.session_material.unwrap().name, "My custom vinyl");
        let mut stale = original.clone();
        stale.force = 30;
        assert!(
            run(
                &controller,
                "materials.save_current",
                json!({"material":stale})
            )
            .is_err()
        );
        let library = controller.snapshot().unwrap().materials;
        run(&controller, "sheet.adjust", json!({"force":22})).unwrap();
        // A failed disk write must not replace the library or the current settings.
        let bad_store = crate::SettingsStore::new(dir.path().join("not-a-directory"));
        std::fs::write(dir.path().join("not-a-directory"), "occupied").unwrap();
        let failing = Controller::build(bad_store, Roots::default(), false);
        run(
            &failing,
            "design.open",
            json!({"path":read.join("design.svg")}),
        )
        .unwrap();
        run(&failing, "sheet.adjust", json!({"force":22})).unwrap();
        let before = failing.snapshot().unwrap();
        assert!(run(&failing, "materials.save_current", json!({})).is_err());
        let after = failing.snapshot().unwrap();
        assert_eq!(after.materials, before.materials);
        assert_eq!(after.session_material, before.session_material);
        assert_eq!(after.revision, before.revision);
        assert_eq!(controller.snapshot().unwrap().materials, library);
    }

    #[test]
    fn sizing_grouping_adjusting_and_the_material_library() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        let check = |id: &str, params: Value| {
            let value = run(&controller, id, params).unwrap_or_else(|e| panic!("{id}: {e}"));
            conforms(&value, &result_schema(id), id).unwrap();
            value
        };
        check("design.open", json!({"path": read.join("design.svg")}));
        let size = check("design.size", json!({"width_mm": 120.0}));
        let [w, h] = [
            size["size_mm"][0].as_f64().unwrap(),
            size["size_mm"][1].as_f64().unwrap(),
        ];
        assert!(
            (w - 120.0).abs() < 0.01 && (h - 120.0).abs() < 0.01,
            "{size}"
        );
        assert_eq!(
            run(
                &controller,
                "design.size",
                json!({"width_mm": 1, "height_mm": 2})
            )
            .unwrap_err()
            .code,
            "failed"
        );
        // Fit: from far too big to the largest size whose cuts sit on the mat's sticky grid
        // inside the margins, moved to the grid's corner.
        check("design.size", json!({"width_mm": 3000.0}));
        let fitted = check("design.size", json!({"fit": true}));
        let project = controller
            .snapshot()
            .unwrap()
            .design
            .unwrap()
            .project
            .clone();
        let current = &project.sheets[0];
        let plan = cutcraft_path::plan_sheet(&project, &current.id).unwrap();
        assert!(plan.valid, "{:?}", plan.diagnostics);
        let [bx0, by0, bx1, by1] = plan.bounds_mm.unwrap();
        let [sx0, sy0, sx1, sy1] = current.mat_profile().sticky_in_cut();
        let c = cutcraft_import::job::project::PLACEMENT_CLEARANCE_MM;
        let (x0, y0) = (sx0.max(0.0) + c, sy0.max(0.0) + c);
        let (x1, y1) = (
            sx1.min(plan.cutting_area_mm[0]) - c,
            sy1.min(plan.cutting_area_mm[1]) - c,
        );
        assert!(
            (bx0 - x0).abs() < 0.05 && (by0 - y0).abs() < 0.05,
            "{:?}",
            plan.bounds_mm
        );
        assert!(bx1 <= x1 + 0.05 && by1 <= y1 + 0.05, "{:?}", plan.bounds_mm);
        // One side fills the room.
        assert!((bx1 - x1).abs() < 0.5 || (by1 - y1).abs() < 0.5, "{fitted}");
        assert!(run(&controller, "design.size", json!({"fit": true, "scale": 2})).is_err());
        check("design.size", json!({"width_mm": 120.0}));
        let grouped = check("rows.group", json!({"by": "layer"}));
        assert_eq!(grouped["group_by"], "layer");
        check("rows.group", json!({"by": "colour"}));
        let adjusted = check("sheet.adjust", json!({"force": 14, "passes": 2}));
        assert_eq!(adjusted["adjusted"]["force"], 14);
        assert!(adjusted.get("material_status").is_none());
        assert_eq!(
            run(&controller, "sheet.adjust", json!({"force": 40}))
                .unwrap_err()
                .code,
            "invalid-params"
        );
        let mut material = Material::defaults().remove(0);
        material.name = "Kraft board".into();
        material.status = cutcraft_import::job::Status::Tested;
        let saved = check("materials.save", json!({"material": material}));
        assert!(saved["saved"].get("status").is_none());
        assert_eq!(
            run(&controller, "materials.save", json!({"material": material}))
                .unwrap_err()
                .code,
            "refused"
        );
        check("sheet.set", json!({"material": "Kraft"}));
        // Edit and rename in place; offsets need a value and stay in range.
        let mut edited = material.clone();
        edited.name = "Kraft card".into();
        edited.notes = "  Smooth side up.  ".into();
        edited.offset_method = cutcraft_import::job::OffsetMethod::Software;
        edited.offset_mm = 0.0;
        assert_eq!(
            run(
                &controller,
                "materials.save",
                json!({"material": edited, "rename_from": "Kraft board"})
            )
            .unwrap_err()
            .code,
            "invalid-params",
            "compensation needs an offset"
        );
        edited.offset_mm = 0.25;
        let renamed = check(
            "materials.save",
            json!({"material": edited, "rename_from": "Kraft board"}),
        );
        assert_eq!(renamed["saved"]["notes"], "Smooth side up.");
        let names: Vec<String> = check("materials.list", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap().to_owned())
            .collect();
        assert!(
            names.contains(&"Kraft card".to_owned()) && !names.contains(&"Kraft board".to_owned())
        );
        // The sheet keeps its own copy until it is chosen again.
        let design = controller.snapshot().unwrap().design.unwrap();
        assert_eq!(design.project.sheets[0].material.name, "Kraft board");
        let adjusted = check(
            "sheet.adjust",
            json!({"offset_method": "machine", "offset_mm": 0.3, "overcut_mm": 0.5}),
        );
        assert_eq!(adjusted["adjusted"]["offset_method"], "machine");
        assert!(adjusted["adjusted"].get("status").is_none());
        assert_eq!(
            run(&controller, "sheet.adjust", json!({"overcut_mm": 5.0}))
                .unwrap_err()
                .code,
            "failed"
        );
        let edited = edited.name.clone();
        check("materials.delete", json!({"name": edited}));
        assert_eq!(
            check("materials.list", json!({})).as_array().unwrap().len(),
            Material::defaults().len()
        );
    }

    #[test]
    fn update_from_design_keeps_the_sheet_settings_and_undo_goes_back() {
        let dir = tempfile::tempdir().unwrap();
        let (controller, read, _) = headless(dir.path());
        let path = read.join("design.svg");
        run(&controller, "design.open", json!({"path": path})).unwrap();
        run(
            &controller,
            "sheet.set",
            json!({"material": "Siser EasyWeed"}),
        )
        .unwrap();
        let same = run(&controller, "project.update", json!({})).unwrap();
        assert_eq!(same["changed"], false);
        std::fs::write(
            &path,
            DESIGN.replace(
                "</svg>",
                r##"<rect x="5" y="80" width="10" height="10" fill="#00aa00"/></svg>"##,
            ),
        )
        .unwrap();
        let updated = run(&controller, "project.update", json!({})).unwrap();
        assert_eq!(updated["changed"], true);
        assert_eq!(updated["rows_added"], json!(["#00aa00"]));
        let design = controller.snapshot().unwrap().design.unwrap();
        assert_eq!(
            design.project.sheets[0].material.name, "Siser EasyWeed iron-on (and similar)",
            "kept"
        );
        assert_eq!(design.project.sheets.len(), 1);
        assert_eq!(
            design.project.sheets[0].rows.len(),
            3,
            "the new colour is cut too"
        );
        run(&controller, "history.undo", json!({})).unwrap();
        let design = controller.snapshot().unwrap().design.unwrap();
        assert_eq!(
            design.project.sheets[0].rows.len(),
            2,
            "undo returns to the previous snapshot"
        );
    }
}
