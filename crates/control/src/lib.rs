//! Shared application actions for the native UI and MCP. No UI or transport dependencies.

mod commands;
mod cut_log;
mod cutting;
mod history;
mod settings;
mod stations;
pub use commands::{
    Authority, BatchOutcome, BatchStep, CommandError, CommandInfo, CommandOutput, Origin, Roots,
    StepOutcome, automation_allowed, rows,
};
pub use cutcraft_device::gpgl;
pub use cutcraft_import::SourceView;
pub use cutcraft_import::job;
pub use cutcraft_path as path;
pub use cutting::{
    CutJob, CutState, CuttingView, HARDWARE_TESTING_ENV, JobKind, VirtualMat, test_cut_markers,
};
pub use history::{History, HistoryStep, JournalEntry};
pub use settings::{
    AccessKey, DEFAULT_MCP_PORT, LOCAL_ADDRESS, McpSettings, NewDesign, PANEL_GROUPS, PanelLayout,
    PortState, SettingsStore, Snap, Units, port_state,
};
pub use stations::{
    FoundStation, PairingState, PairingView, RemoteCutter, StationView, StationsView,
};

use cutcraft_import::job::{
    Decision, MachineProfile, Material, PreparedDesign, Project, Recipe, Sheet,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

pub const MCP_HELP: &str = include_str!("../../../docs/help/mcp.md");
/// The plain-language guide to saving designs for cutting, with text as shapes.
pub const TROUBLESHOOTING_HELP: &str = include_str!("../../../docs/help/troubleshooting.md");

pub const DESIGN_HELP: &str = include_str!("../../../docs/help/design-guide.md");

/// Canonical public documentation and project links, shared with desktop and MCP help.
pub const PROJECT_URL: &str = "https://github.com/mkrzywonski/cutcraft";
pub const MANUAL_URL: &str =
    "https://github.com/mkrzywonski/cutcraft/blob/main/docs/manual/README.md";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// The version and the commit it was built from, e.g. "1.0.0 (abc1234)".
pub const APP_BUILD: &str = env!("CUTCRAFT_BUILD");

/// Which help window to open. The window's buttons and `cutcraft_help` share these topics.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum HelpTopic {
    /// Connecting AI tools over MCP (`docs/mcp.md`).
    #[default]
    AiSetup,
    /// Saving designs for cutting (`docs/design-guide.md`).
    Designs,
    /// Cutting problems and material adjustments.
    Troubleshooting,
    /// Application version, purpose and licence.
    About,
    /// Open the GitHub user manual in the desktop browser.
    Manual,
}

impl HelpTopic {
    /// The guide's text. The AI setup guide's examples use the configured port.
    pub fn text(self, port: u16) -> String {
        match self {
            HelpTopic::AiSetup => MCP_HELP.replace("47833", &port.to_string()),
            HelpTopic::Designs => DESIGN_HELP.to_owned(),
            HelpTopic::Troubleshooting => TROUBLESHOOTING_HELP.to_owned(),
            HelpTopic::About => format!(
                "# CutCraft\n\nVersion {APP_VERSION}\n\nPrepare designs and cut vinyl with your Silhouette Cameo.\n\nLicensed under MIT OR Apache-2.0.\n\n{PROJECT_URL}"
            ),
            HelpTopic::Manual => format!("# CutCraft user manual\n\n{MANUAL_URL}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ServiceStatus {
    pub state: String,
    pub url: Option<String>,
    pub error: Option<String>,
}

impl ServiceStatus {
    pub fn new(state: &str, url: Option<String>, error: Option<String>) -> Self {
        Self {
            state: state.into(),
            url,
            error,
        }
    }
}

#[derive(Clone, Serialize)]
pub struct OpenDesign {
    pub path: String,
    /// The `.cutcraft` file it was opened from, when it came from a project.
    pub project_path: Option<String>,
    pub inspection: Value,
    /// The decisions answered so far; preparation replays them from the original source.
    pub recipe: Recipe,
    pub prepared: Value,
    /// Sheets, materials and placement for the prepared design.
    #[serde(skip)]
    pub project: Project,
    /// The source bytes as opened: preparation replays decisions from these, and a saved project
    /// embeds them.
    #[serde(skip)]
    pub bytes: Arc<Vec<u8>>,
    /// Who gave each answer that came from automation (spec §5.10: assistant marks).
    pub answered_by: Vec<Answered>,
}

/// An answer in the recipe and the origin that gave it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Answered {
    pub decision: Decision,
    pub origin: Origin,
}

#[derive(Clone, Serialize)]
pub struct Snapshot {
    pub app_version: &'static str,
    pub revision: u64,
    pub design: Option<Arc<OpenDesign>>,
    pub loading_path: Option<String>,
    pub last_error: Option<String>,
    pub engine_check: Option<Value>,
    pub usb_discovery: Option<Value>,
    pub last_capture: Option<Value>,
    pub mcp: McpSettings,
    pub mcp_service: ServiceStatus,
    pub settings_path: String,
    pub settings_error: Option<String>,
    pub settings_revision: u64,
    /// The AI setup help window is open.
    pub show_help: bool,
    /// The Design help window is open.
    pub show_design_help: bool,
    pub show_troubleshooting: bool,
    /// The About window is open.
    pub show_about: bool,
    /// The desktop should open the manual URL once, then clear this request.
    pub open_manual: bool,
    /// Undo and redo steps for the open design.
    pub history: History,
    /// The cutter and what it is doing.
    pub cutting: CuttingView,
    /// Paired cameo-pi stations and their cutters (spec §5.11), without their keys.
    pub stations: StationsView,
    /// Their material library (spec §5.6).
    pub materials: Vec<Material>,
    /// Session settings, persisted to the library only by an explicit material save.
    pub session_material: Option<Material>,
    pub materials_error: Option<String>,
    /// Their mats, for showing material on the mat (spec §4.1).
    pub mats: Vec<cutcraft_import::job::MatProfile>,
    /// Inches or millimetres in the window.
    pub units: Units,
    /// Base interface font size in points.
    pub font_size: f32,
    /// Snap to grid on the Cut preview.
    pub snap: Snap,
    /// How they arranged the right panel.
    pub panel: PanelLayout,
    /// The material and mat a newly opened design starts with.
    pub new_design: NewDesign,
    /// Recent designs and projects, newest first.
    pub recent: Vec<String>,
    /// What the window shows.
    pub view: View,
    /// The revision when the project was last opened or saved: anything newer is unsaved.
    pub saved_revision: Option<u64>,
    /// Whether the design file changed on disk since it was opened (spec §5.7: watch/reload).
    pub source: SourceStatus,
}

/// The design file on disk compared with the design as opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceStatus {
    #[default]
    Unchanged,
    /// Saved again with different content: offer *Update from design*.
    Changed,
    /// Not where it was opened from (moved, renamed or deleted).
    Missing,
}

/// What the watch last saw of the design file, so unchanged files are not read again.
#[derive(Clone, Default)]
struct Watched {
    path: String,
    sha256: String,
    modified: Option<std::time::SystemTime>,
    len: Option<u64>,
}

/// The window's screens (spec §4), so automation can see and change what they see.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Screen {
    /// The artwork and its cut lines side by side, panned and zoomed together; the start
    /// screen without a design. (`prepare` and `choose` are accepted for it.)
    #[default]
    #[serde(alias = "prepare", alias = "choose")]
    SideBySide,
    /// The cut lines drawn over the artwork.
    Overlay,
    /// The Cut Preview: a sheet placed on its mat or roll. (`preview` is accepted for it.)
    #[serde(alias = "preview")]
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct View {
    pub screen: Screen,
}

impl Snapshot {
    /// Changes since the project was last opened or saved.
    pub fn unsaved(&self) -> bool {
        self.design.is_some() && self.saved_revision != Some(self.revision)
    }
}

pub enum Action {
    Status,
    McpSettings,
    OpenDesign(PathBuf),
    CloseDesign,
    Inspection,
    /// Read the current prepared design (Make cut-ready result).
    Prepared,
    /// Answer one question, replacing any earlier answer to it, and prepare again.
    Decide(Decision),
    /// Remove every answer and prepare again.
    ResetDecisions,
    EngineCheck,
    Probe,
    Capture {
        save_to: Option<PathBuf>,
    },
    SetMcpSettings(McpSettings),
    /// Open or close one help window and return its guide.
    Help {
        topic: HelpTopic,
        visible: bool,
    },
}

pub struct FrameImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
pub struct ActionOutput {
    pub value: Value,
    pub image: Option<FrameImage>,
}
impl ActionOutput {
    fn json(value: Value) -> Self {
        Self { value, image: None }
    }
}

pub struct CaptureRequest {
    pub reply: mpsc::SyncSender<Result<FrameImage, String>>,
}
type Wake = Arc<dyn Fn() + Send + Sync>;
struct Inner {
    /// Serialize command mutations through validation, dispatch and history recording.
    /// Reads and Stop stay independent so waiting work cannot delay emergency controls.
    commands: Mutex<()>,
    state: Mutex<Snapshot>,
    store: SettingsStore,
    /// Folders automation may read from and write to (spec §6.2.7).
    roots: Roots,
    /// A window is attached (the live app), not a headless session.
    live: bool,
    /// The session journal: next sequence number and the latest entries.
    journal: Mutex<(u64, VecDeque<JournalEntry>)>,
    captures: Mutex<VecDeque<CaptureRequest>>,
    wake: Mutex<Option<Wake>>,
    /// The connected cutter and the plan being cut.
    device: cutting::Session,
    /// Paired cameo-pi stations, with their keys.
    stations: Mutex<stations::Stations>,
    /// When automation last ran a command, for the window's activity note.
    last_automation: Mutex<Option<(Origin, std::time::Instant)>>,
    /// What the window last reported about itself (`view.inspect`).
    ui_report: Mutex<Value>,
    watched: Mutex<Watched>,
}

#[derive(Clone)]
pub struct Controller(Arc<Inner>);

/// The original design kept ready for the main window's artwork view: any region, any zoom,
/// in the same millimetres as the cut lines.
pub fn source_view(design: &OpenDesign) -> Result<cutcraft_import::SourceView, String> {
    cutcraft_import::SourceView::new(
        std::path::Path::new(&design.path),
        &design.bytes,
        &design.project.prepared,
    )
}

/// The original design as opened, before Make cut-ready (spec §5.2's before/after view), drawn
/// by the engine on white. The window and `render.preview` view `source` both use it.
pub fn source_preview(design: &OpenDesign, max_side: u32) -> Result<path::RasterPreview, String> {
    let (png, width, height) =
        cutcraft_import::source_png(std::path::Path::new(&design.path), &design.bytes, max_side)?;
    Ok(path::RasterPreview {
        png,
        width,
        height,
        px_per_mm: 0.0,
        origin_px: [0.0, 0.0],
        plan_id: "source".into(),
        mirrored: false,
        cuts_shown: 0,
        cuts_total: 0,
        highlighted: Vec::new(),
        legend: Vec::new(),
    })
}

impl Controller {
    /// Compare the design file on disk with the design as opened (spec §5.7). Cheap when nothing
    /// changed: the file is read and hashed only when its size or time changes. The window calls
    /// this every few seconds; `project.inspect` calls it before answering.
    pub fn watch_source(&self) -> SourceStatus {
        let Ok(snapshot) = self.snapshot() else {
            return SourceStatus::Unchanged;
        };
        let Some(design) = snapshot.design.as_ref() else {
            return SourceStatus::Unchanged;
        };
        let path = design.path.clone();
        let sha = design.project.prepared.source_sha256.clone();
        let metadata = std::fs::metadata(&path).ok();
        let modified = metadata.as_ref().and_then(|m| m.modified().ok());
        let len = metadata.as_ref().map(std::fs::Metadata::len);
        let status = {
            let Ok(mut watched) = self.0.watched.lock() else {
                return snapshot.source;
            };
            let same = watched.path == path
                && watched.sha256 == sha
                && watched.modified == modified
                && watched.len == len;
            if same {
                return snapshot.source;
            }
            *watched = Watched {
                path: path.clone(),
                sha256: sha.clone(),
                modified,
                len,
            };
            match metadata {
                None => SourceStatus::Missing,
                Some(_) => match cutcraft_import::read_source(std::path::Path::new(&path)) {
                    Ok(bytes) => {
                        use sha2::Digest;
                        if format!("{:x}", sha2::Sha256::digest(&bytes)) == sha {
                            SourceStatus::Unchanged
                        } else {
                            SourceStatus::Changed
                        }
                    }
                    Err(_) => SourceStatus::Missing,
                },
            }
        };
        if status != snapshot.source {
            let _ = self.change(|s| s.source = status);
        }
        status
    }

    /// The live app's controller. Automation can do anything the person can, files included.
    pub fn new(store: SettingsStore) -> Self {
        Self::build(store, Roots::default(), true)
    }

    /// A headless session (no window), optionally limited to automation folders.
    pub fn headless(roots: Roots) -> Self {
        Self::build(SettingsStore::detached(), roots, false)
    }

    fn build(store: SettingsStore, roots: Roots, live: bool) -> Self {
        let (settings, settings_error) = match store.load() {
            Ok(settings) => (settings, None),
            // A damaged setting must not accidentally re-enable a previously disabled service.
            Err(error) => (
                McpSettings {
                    enabled: false,
                    ..McpSettings::default()
                },
                Some(error),
            ),
        };
        let (materials, materials_error) = match store.load_materials() {
            Ok(materials) => (materials, None),
            Err(error) => (Material::defaults(), Some(error)),
        };
        let recent = store.load_recent();
        let units = store.load_units();
        let font_size = store.load_font_size();
        let snap = store.load_snap();
        let panel = store.load_panel();
        let new_design = store.load_new_design();
        let mats = store
            .load_mats()
            .unwrap_or_else(|_| cutcraft_import::job::MatProfile::presets());
        let paired = store.load_stations().unwrap_or_default();
        Self(Arc::new(Inner {
            commands: Mutex::new(()),
            state: Mutex::new(Snapshot {
                app_version: APP_BUILD,
                revision: 0,
                design: None,
                loading_path: None,
                last_error: None,
                engine_check: None,
                usb_discovery: None,
                last_capture: None,
                mcp: settings,
                mcp_service: ServiceStatus::new("starting", None, None),
                settings_path: store.path().to_string_lossy().into_owned(),
                settings_error,
                settings_revision: 0,
                show_help: false,
                show_design_help: false,
                show_troubleshooting: false,
                show_about: false,
                open_manual: false,
                history: History::default(),
                cutting: CuttingView::default(),
                stations: StationsView {
                    paired: paired
                        .iter()
                        .map(|c| StationView {
                            address: c.address.clone(),
                            fingerprint: c.fingerprint.to_string(),
                            cutters: Vec::new(),
                            version: None,
                            error: None,
                        })
                        .collect(),
                    pairing: None,
                    found: Vec::new(),
                },
                materials,
                session_material: None,
                materials_error,
                mats,
                units,
                font_size,
                snap,
                panel,
                new_design,
                recent,
                view: View {
                    screen: Screen::SideBySide,
                },
                saved_revision: None,
                source: SourceStatus::Unchanged,
            }),
            store,
            roots,
            live,
            journal: Mutex::new((1, VecDeque::new())),
            captures: Mutex::new(VecDeque::new()),
            wake: Mutex::new(None),
            device: Mutex::default(),
            stations: Mutex::new(stations::Stations {
                credentials: paired,
                ..Default::default()
            }),
            last_automation: Mutex::new(None),
            ui_report: Mutex::new(Value::Null),
            watched: Mutex::new(Watched::default()),
        }))
    }

    /// The window reports its screen, dialogs, windows and focus each frame.
    pub fn set_ui_report(&self, report: Value) {
        if let Ok(mut current) = self.0.ui_report.lock() {
            *current = report;
        }
    }

    pub(crate) fn ui_report(&self) -> Value {
        self.0
            .ui_report
            .lock()
            .map(|r| r.clone())
            .unwrap_or(Value::Null)
    }

    /// Who last ran a command from automation, and how long ago.
    pub fn automation_activity(&self) -> Option<(Origin, Duration)> {
        let last = *self.0.last_automation.lock().ok()?;
        last.map(|(origin, at)| (origin, at.elapsed()))
    }

    /// A window is attached (not a headless session).
    pub fn is_live(&self) -> bool {
        self.0.live
    }

    pub fn attach_renderer(&self, wake: Wake) -> Result<(), String> {
        *self
            .0
            .wake
            .lock()
            .map_err(|_| "Renderer state is unavailable.")? = Some(wake);
        Ok(())
    }
    pub fn snapshot(&self) -> Result<Snapshot, String> {
        Ok(self
            .0
            .state
            .lock()
            .map_err(|_| "Application state is unavailable.")?
            .clone())
    }
    fn change(&self, update: impl FnOnce(&mut Snapshot)) -> Result<(), String> {
        {
            let mut state = self
                .0
                .state
                .lock()
                .map_err(|_| "Application state is unavailable.")?;
            update(&mut state);
        }
        self.wake();
        Ok(())
    }
    /// Material setup has shared revision checks but must not make an unchanged design dirty.
    fn wake(&self) {
        if let Ok(wake) = self.0.wake.lock()
            && let Some(wake) = wake.as_ref()
        {
            wake();
        }
    }
    pub fn set_service_status(&self, status: ServiceStatus) -> Result<(), String> {
        if self.snapshot()?.mcp_service != status {
            self.change(|s| s.mcp_service = status)?;
        }
        Ok(())
    }
    pub fn take_capture(&self) -> Result<Option<CaptureRequest>, String> {
        Ok(self
            .0
            .captures
            .lock()
            .map_err(|_| "Capture queue is unavailable.")?
            .pop_front())
    }
    /// Cancel a running open or preparation (MCP `notifications/cancelled`, spec §5.9): its result
    /// will not be applied and the previous complete result stays, as when a newer action
    /// supersedes it.
    pub fn cancel_pending(&self) -> Result<(), String> {
        self.change(|s| {
            s.revision += 1;
            s.loading_path = None;
        })
    }

    pub fn hide_help(&self, topic: HelpTopic) -> Result<(), String> {
        self.change(|s| *help_shown(s, topic) = false)
    }

    /// UI callers use a worker; the MCP transport uses spawn_blocking. File import never runs
    /// on the drawing thread. Every caller applies the same validation and state transitions.
    pub fn run(&self, action: Action) -> Result<ActionOutput, String> {
        match action {
            Action::Status => Ok(ActionOutput::json(
                serde_json::to_value(self.snapshot()?).map_err(|e| e.to_string())?,
            )),
            Action::McpSettings => {
                let snapshot = self.snapshot()?;
                Ok(ActionOutput::json(
                    json!({"settings": snapshot.mcp, "service": snapshot.mcp_service,
                    "settings_path": snapshot.settings_path, "settings_error": snapshot.settings_error}),
                ))
            }
            Action::OpenDesign(path) => self.open_design(path, Recipe::new()),
            Action::CloseDesign => {
                self.change(|s| {
                    s.revision += 1;
                    s.design = None;
                    s.loading_path = None;
                    s.last_error = None;
                })?;
                Ok(ActionOutput::json(json!({"closed": true})))
            }
            Action::Inspection => Ok(ActionOutput::json(
                self.snapshot()?
                    .design
                    .ok_or("Open a design first.")?
                    .inspection
                    .clone(),
            )),
            Action::Prepared => Ok(ActionOutput::json(
                self.snapshot()?
                    .design
                    .ok_or("Open a design first.")?
                    .prepared
                    .clone(),
            )),
            Action::Decide(decision) => {
                let design = self.snapshot()?.design.ok_or("Open a design first.")?;
                let mut recipe = design.recipe.clone();
                recipe
                    .decisions
                    .retain(|d| !answers_same_question(d, &decision));
                recipe.decisions.push(decision);
                self.reprepare(recipe)
            }
            Action::ResetDecisions => {
                self.snapshot()?.design.ok_or("Open a design first.")?;
                self.reprepare(Recipe::new())
            }
            Action::EngineCheck => {
                let result = cutcraft_import::smoke_check();
                let value = json!({"ok": result.is_ok(), "error": result.as_ref().err()});
                self.change(|s| s.engine_check = Some(value.clone()))?;
                result?;
                Ok(ActionOutput::json(value))
            }
            Action::Probe => {
                let devices = cutcraft_device::discover().map_err(|e| e.to_string())?;
                let value =
                    json!({"devices": devices, "commands_sent": false, "firmware_queried": false});
                self.change(|s| s.usb_discovery = Some(value.clone()))?;
                Ok(ActionOutput::json(value))
            }
            Action::Capture { save_to } => self.capture(save_to),
            Action::SetMcpSettings(settings) => {
                settings.validate()?;
                // Serialize disk save and application state so concurrent clients cannot leave
                // the saved settings different from the running app's requested settings.
                {
                    let mut state = self
                        .0
                        .state
                        .lock()
                        .map_err(|_| "Settings state is unavailable.")?;
                    self.0.store.save(&settings)?;
                    state.mcp = settings.clone();
                    state.settings_revision += 1;
                    state.settings_error = None;
                }
                self.wake();
                Ok(ActionOutput::json(
                    json!({"settings": settings, "reconnect": true,
                    "message": "Settings saved. Changing the address or port, or turning MCP off, disconnects existing clients; key changes apply to the next request."}),
                ))
            }
            Action::Help { topic, visible } => {
                self.change(|s| *help_shown(s, topic) = visible)?;
                Ok(ActionOutput::json(
                    json!({"topic": topic, "help": topic.text(self.snapshot()?.mcp.port)}),
                ))
            }
        }
    }

    fn open_design(&self, path: PathBuf, recipe: Recipe) -> Result<ActionOutput, String> {
        let revision = {
            let mut state = self
                .0
                .state
                .lock()
                .map_err(|_| "Application state is unavailable.")?;
            state.revision += 1;
            state.loading_path = Some(path.to_string_lossy().into_owned());
            state.last_error = None;
            state.revision
        };
        self.wake();
        let imported = std::fs::canonicalize(path)
            .map_err(|e| format!("Cannot open the design: {e}"))
            .and_then(|path| {
                let bytes = cutcraft_import::read_source(&path)?;
                let (inspection, prepared) =
                    cutcraft_import::inspect_and_prepare(&path, &bytes, &recipe)?;
                let mut project = Project::new(prepared.clone(), MachineProfile::cameo_1());
                let snapshot = self.snapshot()?;
                let id = project.sheets[0].id.clone();
                // Their chosen starting material wins; otherwise the one they were last using.
                let chosen =
                    snapshot.new_design.material.as_ref().and_then(|name| {
                        snapshot.materials.iter().find(|m| &m.name == name).cloned()
                    });
                if let Some(material) = chosen
                    .or(snapshot.session_material)
                    .or_else(|| snapshot.materials.first().cloned())
                {
                    project.set_material(&id, material)?;
                }
                if let Some(mat) = snapshot
                    .new_design
                    .mat
                    .as_ref()
                    .and_then(|name| snapshot.mats.iter().find(|m| &m.name == name))
                {
                    project.set_mat_profile(&id, mat.clone())?;
                }
                Ok(OpenDesign {
                    path: path.to_string_lossy().into_owned(),
                    project_path: None,
                    inspection: serde_json::to_value(inspection).map_err(|e| e.to_string())?,
                    recipe,
                    prepared: serde_json::to_value(prepared).map_err(|e| e.to_string())?,
                    project,
                    bytes: Arc::new(bytes),
                    answered_by: Vec::new(),
                })
            });
        self.finish_open(revision, imported)
            .map(|design| ActionOutput::json(design.inspection.clone()))
    }

    /// Prepare the open design again with `recipe`, from its original file. The result applies
    /// only if nothing newer happened meanwhile and the file is the one that was opened.
    fn reprepare(&self, recipe: Recipe) -> Result<ActionOutput, String> {
        let (revision, design) = {
            let mut state = self
                .0
                .state
                .lock()
                .map_err(|_| "Application state is unavailable.")?;
            let design = state.design.clone().ok_or("Open a design first.")?;
            state.revision += 1;
            (state.revision, design)
        };
        let path = PathBuf::from(&design.path);
        let prepared = (|| {
            // A design opened from its file is checked against the file; a project's embedded
            // copy is its own source.
            if design.project_path.is_none() {
                let current = cutcraft_import::read_source(&path)?;
                if current != *design.bytes {
                    return Err("The design file changed on disk since it was opened. Open it again to review the update.".to_string());
                }
            }
            let prepared = cutcraft_import::prepare_bytes(&path, &design.bytes, &recipe)?;
            Ok(OpenDesign {
                path: design.path.clone(),
                project_path: design.project_path.clone(),
                inspection: design.inspection.clone(),
                recipe,
                prepared: serde_json::to_value(&prepared).map_err(|e| e.to_string())?,
                project: carry_over(&design.project, prepared),
                bytes: design.bytes.clone(),
                answered_by: design.answered_by.clone(),
            })
        })();
        // A rejected decision keeps the previous result, as a failed import does.
        let design = self.finish_open(revision, prepared)?;
        Ok(ActionOutput::json(design.prepared.clone()))
    }

    /// *Update from design* (spec §5.7): read the design file again (or the file at `path`,
    /// when it moved), prepare it with the same answers, and carry sheet settings over for rows
    /// that still match. Undo returns to the previous snapshot.
    pub(crate) fn update_from_design(&self, path: Option<PathBuf>) -> Result<ActionOutput, String> {
        let design = self.snapshot()?.design.ok_or("Open a design first.")?;
        let source = path.unwrap_or_else(|| PathBuf::from(&design.path));
        let source = std::fs::canonicalize(&source)
            .map_err(|e| format!("Cannot find the design at {}: {e}", source.display()))?;
        let bytes = cutcraft_import::read_source(&source)?;
        if bytes == *design.bytes {
            return Ok(ActionOutput::json(
                json!({"changed": false, "message": "The design file has not changed."}),
            ));
        }
        // From here a newer action supersedes this update, as with opening.
        let revision = {
            let mut state = self
                .0
                .state
                .lock()
                .map_err(|_| "Application state is unavailable.")?;
            state.revision += 1;
            state.revision
        };
        let (inspection, prepared) =
            cutcraft_import::inspect_and_prepare(&source, &bytes, &design.recipe)?;
        let project = carry_over(&design.project, prepared.clone());
        let labels = |p: &Project| -> Vec<String> {
            p.sheets
                .iter()
                .flat_map(|s| s.rows.iter().map(|r| r.label()))
                .collect()
        };
        let (before, after) = (labels(&design.project), labels(&project));
        let added: Vec<&String> = after.iter().filter(|l| !before.contains(l)).collect();
        let removed: Vec<&String> = before.iter().filter(|l| !after.contains(l)).collect();
        let value = json!({
            "changed": true,
            "rows_added": added,
            "rows_removed": removed,
            "cut_ready": prepared.cut_ready,
        });
        let updated = OpenDesign {
            path: source.to_string_lossy().into_owned(),
            project_path: design.project_path.clone(),
            inspection: serde_json::to_value(inspection).map_err(|e| e.to_string())?,
            recipe: design.recipe.clone(),
            prepared: serde_json::to_value(&prepared).map_err(|e| e.to_string())?,
            project,
            bytes: Arc::new(bytes),
            answered_by: design.answered_by.clone(),
        };
        self.finish_open(revision, Ok(updated))?;
        Ok(ActionOutput::json(value))
    }

    fn finish_open(
        &self,
        revision: u64,
        imported: Result<OpenDesign, String>,
    ) -> Result<Arc<OpenDesign>, String> {
        {
            let mut state = self
                .0
                .state
                .lock()
                .map_err(|_| "Application state is unavailable.")?;
            if state.revision != revision {
                return Err(
                    "A newer action superseded this import or preparation; its result was not applied."
                        .into(),
                );
            }
            state.loading_path = None;
            match imported {
                Ok(design) => {
                    let design = Arc::new(design);
                    state.design = Some(design.clone());
                    state.last_error = None;
                    drop(state);
                    self.wake();
                    Ok(design)
                }
                Err(error) => {
                    state.last_error = Some(error.clone());
                    drop(state);
                    self.wake();
                    Err(error)
                }
            }
        }
    }

    fn capture(&self, save_to: Option<PathBuf>) -> Result<ActionOutput, String> {
        if self
            .0
            .wake
            .lock()
            .map_err(|_| "Renderer state is unavailable.")?
            .is_none()
        {
            return Err("No desktop renderer is attached. Start the CutCraft window first.".into());
        }
        let (reply, receive) = mpsc::sync_channel(1);
        {
            let mut queue = self
                .0
                .captures
                .lock()
                .map_err(|_| "Capture queue is unavailable.")?;
            if queue.len() >= 4 {
                return Err(
                    "Too many view captures are pending. Wait for the current capture.".into(),
                );
            }
            queue.push_back(CaptureRequest { reply });
        }
        self.wake();
        let image = receive
            .recv_timeout(Duration::from_secs(20))
            .map_err(|_| "The window did not return a rendered frame in time.")??;
        if let Some(path) = &save_to {
            if let Some(design) = self.snapshot()?.design
                && (path == &PathBuf::from(&design.path)
                    || std::fs::canonicalize(path).ok().as_ref()
                        == Some(&PathBuf::from(&design.path)))
            {
                return Err(
                    "Choose a screenshot output path different from the open design.".into(),
                );
            }
            settings::write_atomic(path, &image.png)?;
        }
        let value = json!({"width": image.width, "height": image.height, "mime_type": "image/png",
            "saved_to": save_to.map(|p| p.to_string_lossy().into_owned())});
        self.change(|s| s.last_capture = Some(value.clone()))?;
        Ok(ActionOutput {
            value,
            image: Some(image),
        })
    }

    /// Run a command on a worker thread, so file import never runs on the drawing thread.
    pub fn spawn_command(
        &self,
        origin: Origin,
        id: &str,
        params: Value,
    ) -> mpsc::Receiver<Result<CommandOutput, CommandError>> {
        let (send, receive) = mpsc::sync_channel(1);
        let controller = self.clone();
        let id = id.to_owned();
        std::thread::spawn(move || {
            let _ = send.send(controller.execute(origin, &id, params, None));
        });
        receive
    }

    pub fn spawn(&self, action: Action) -> mpsc::Receiver<Result<ActionOutput, String>> {
        let (send, receive) = mpsc::sync_channel(1);
        let controller = self.clone();
        std::thread::spawn(move || {
            let _ = send.send(controller.run(action));
        });
        receive
    }
}

/// A project for a newly prepared design that keeps the old project's choices: the grouping,
/// size, the sheet's material, mirror, mat, placement and aids, and unticked rows that still
/// exist.
fn carry_over(old: &Project, prepared: PreparedDesign) -> Project {
    let mut project = Project::new(prepared, old.machine.clone());
    project.scale = old.scale;
    project.set_group_by(old.group_by);
    if let (Some(sheet), Some(previous)) = (project.sheets.first_mut(), old.sheets.first()) {
        *sheet = Sheet {
            id: sheet.id.clone(),
            rows: sheet.rows.clone(),
            ..previous.clone()
        };
    }
    let new_rows: Vec<_> = project.sheets.iter().flat_map(|s| s.rows.clone()).collect();
    project.excluded_rows = old
        .excluded_rows
        .iter()
        .filter(|r| new_rows.contains(r))
        .cloned()
        .collect();
    project.revision = old.revision + 1;
    project
}

/// The snapshot flag that keeps `topic`'s help window open.
fn help_shown(state: &mut Snapshot, topic: HelpTopic) -> &mut bool {
    match topic {
        HelpTopic::AiSetup => &mut state.show_help,
        HelpTopic::Designs => &mut state.show_design_help,
        HelpTopic::Troubleshooting => &mut state.show_troubleshooting,
        HelpTopic::About => &mut state.show_about,
        HelpTopic::Manual => &mut state.open_manual,
    }
}

/// Do two decisions answer the same question, so the later replaces the earlier?
pub(crate) fn answers_same_question(a: &Decision, b: &Decision) -> bool {
    match (a, b) {
        (Decision::DxfUnits { .. }, Decision::DxfUnits { .. })
        | (Decision::Page { .. }, Decision::Page { .. })
        | (Decision::ImportRoute { .. }, Decision::ImportRoute { .. })
        | (Decision::FitArtwork | Decision::KeepPage, Decision::FitArtwork | Decision::KeepPage)
        | (Decision::PictureSize { .. }, Decision::PictureSize { .. })
        | (Decision::PictureFrame { .. }, Decision::PictureFrame { .. })
        | (Decision::Trace(_), Decision::Trace(_))
        | (Decision::TracePhoto, Decision::TracePhoto)
        | (Decision::OutlinesOnFills { .. }, Decision::OutlinesOnFills { .. })
        | (Decision::ConfirmSize { .. }, Decision::ConfirmSize { .. }) => true,
        (
            Decision::CutSolid { source_id: a, .. } | Decision::ExcludeObject { source_id: a },
            Decision::CutSolid { source_id: b, .. } | Decision::ExcludeObject { source_id: b },
        )
        | (Decision::Background { source_id: a, .. }, Decision::Background { source_id: b, .. })
        | (Decision::ExcludePicture { source_id: a }, Decision::ExcludePicture { source_id: b })
        | (Decision::AcceptWarning { warning: a }, Decision::AcceptWarning { warning: b })
        | (
            Decision::AcceptTextOverflow { source_id: a },
            Decision::AcceptTextOverflow { source_id: b },
        ) => a == b,
        (
            Decision::StrokeColourRole { colour: a, .. },
            Decision::StrokeColourRole { colour: b, .. },
        ) => a == b,
        (
            Decision::FontSubstitute {
                family: a,
                style: s,
            }
            | Decision::FontFromFile {
                family: a,
                style: s,
            },
            Decision::FontSubstitute {
                family: b,
                style: t,
            }
            | Decision::FontFromFile {
                family: b,
                style: t,
            },
        ) => a == b && s == t,
        (
            Decision::StrokeRole {
                source_id: a,
                stroke: i,
                ..
            },
            Decision::StrokeRole {
                source_id: b,
                stroke: j,
                ..
            },
        ) => a == b && i == j,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn interface_font_size_is_validated_shared_and_saved_with_preferences() {
        let folder = tempfile::tempdir().unwrap();
        // Preferences from before the font setting retain their units and grid.
        std::fs::write(
            folder.path().join("preferences.json"),
            r#"{"schema_version":1,"units":"millimetres","snap":{"enabled":true,"step_mm":5.0}}"#,
        )
        .unwrap();
        let store = SettingsStore::new(folder.path().to_owned());
        let controller = Controller::new(store.clone());
        assert_eq!(controller.snapshot().unwrap().font_size, 14.0);
        for (origin, points) in [(Origin::User, 10.0), (Origin::Mcp, 24.0)] {
            controller
                .execute(origin, "settings.font_size", json!({"points":points}), None)
                .unwrap();
            assert_eq!(controller.snapshot().unwrap().font_size, points);
        }
        for points in [9.0, 25.0] {
            assert!(
                controller
                    .execute(
                        Origin::Mcp,
                        "settings.font_size",
                        json!({"points":points}),
                        None
                    )
                    .is_err()
            );
        }
        controller
            .execute(
                Origin::User,
                "settings.units",
                json!({"units":"inches"}),
                None,
            )
            .unwrap();
        let reopened = Controller::new(store).snapshot().unwrap();
        assert_eq!(reopened.font_size, 24.0);
        assert_eq!(reopened.units, Units::Inches);
        assert!(reopened.snap.enabled);
        assert_eq!(reopened.snap.step_mm, Some(5.0));
    }

    #[test]
    fn failed_import_preserves_previous_design_and_close_clears_it() {
        let folder = tempfile::tempdir().unwrap();
        let controller = Controller::new(SettingsStore::new(folder.path().to_owned()));
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../import/tests/fixtures/one-inch.svg");
        let report = controller.run(Action::OpenDesign(fixture)).unwrap().value;
        assert_eq!(report["cut_ready"], false);
        let broken = folder.path().join("broken.svg");
        std::fs::write(&broken, "<svg>").unwrap();
        assert!(controller.run(Action::OpenDesign(broken)).is_err());
        assert_eq!(controller.run(Action::Inspection).unwrap().value, report);
        controller.run(Action::CloseDesign).unwrap();
        assert!(controller.snapshot().unwrap().design.is_none());
    }
    #[test]
    fn a_cancelled_preparation_keeps_the_previous_result() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("original.svg");
        std::fs::write(&source, BACKGROUND_SVG).unwrap();
        let controller = Controller::new(SettingsStore::new(directory.path().join("config")));
        controller.run(Action::OpenDesign(source.clone())).unwrap();
        let before = controller.snapshot().unwrap().design.unwrap();
        // A preparation that began before the cancel finishes afterwards: it is not applied.
        let started = controller.snapshot().unwrap().revision;
        controller.cancel_pending().unwrap();
        assert!(controller.finish_open(started, Ok(stale(&source))).is_err());
        let after = controller.snapshot().unwrap();
        assert!(Arc::ptr_eq(&after.design.unwrap(), &before));
        assert!(after.loading_path.is_none());
    }

    #[test]
    fn help_topics_open_and_close_their_own_windows() {
        let folder = tempfile::tempdir().unwrap();
        let controller = Controller::new(SettingsStore::new(folder.path().to_owned()));
        let help = |topic, visible| {
            controller
                .run(Action::Help { topic, visible })
                .unwrap()
                .value
        };
        let designs = help(HelpTopic::Designs, true);
        assert_eq!(designs["topic"], "designs");
        assert_eq!(designs["help"], DESIGN_HELP);
        let snapshot = controller.snapshot().unwrap();
        assert!(snapshot.show_design_help && !snapshot.show_help);
        let setup = help(HelpTopic::AiSetup, true);
        assert_eq!(setup["topic"], "ai_setup");
        assert!(setup["help"].as_str().unwrap().contains("command_list"));
        controller.hide_help(HelpTopic::Designs).unwrap();
        let snapshot = controller.snapshot().unwrap();
        assert!(snapshot.show_help && !snapshot.show_design_help);
    }
    #[test]
    fn about_and_manual_use_shared_help_commands_without_a_design() {
        let folder = tempfile::tempdir().unwrap();
        let controller = Controller::new(SettingsStore::new(folder.path().to_owned()));
        let about = controller
            .execute(Origin::Control, "help.show", json!({"topic":"about"}), None)
            .unwrap();
        assert!(about.value["help"].as_str().unwrap().contains(APP_VERSION));
        assert!(
            about.value["help"]
                .as_str()
                .unwrap()
                .contains("MIT OR Apache-2.0")
        );
        assert!(controller.snapshot().unwrap().show_about);
        let manual = controller
            .execute(
                Origin::Control,
                "help.show",
                json!({"topic":"manual"}),
                None,
            )
            .unwrap();
        assert!(manual.value["help"].as_str().unwrap().contains(MANUAL_URL));
        let snapshot = controller.snapshot().unwrap();
        assert!(snapshot.open_manual && snapshot.show_about);
        controller.hide_help(HelpTopic::Manual).unwrap();
        assert!(!controller.snapshot().unwrap().open_manual);
        assert!(controller.snapshot().unwrap().show_about);
        controller
            .execute(
                Origin::Control,
                "help.show",
                json!({"topic":"about", "visible":false}),
                None,
            )
            .unwrap();
        assert!(!controller.snapshot().unwrap().show_about);
    }

    #[test]
    fn corrupt_settings_disable_service_without_discarding_the_error() {
        let folder = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(folder.path().to_owned());
        std::fs::write(store.path(), "invalid").unwrap();
        let controller = Controller::new(store);
        assert!(!controller.snapshot().unwrap().mcp.enabled);
        assert!(controller.snapshot().unwrap().settings_error.is_some());
    }

    #[test]
    fn a_superseded_worker_cannot_reopen_a_closed_design_or_replace_a_new_one() {
        let directory = tempfile::tempdir().unwrap();
        let controller = Controller::new(SettingsStore::new(directory.path().to_owned()));
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../import/tests/fixtures/one-inch.svg");
        controller.run(Action::OpenDesign(fixture.clone())).unwrap();
        let old_revision = controller.snapshot().unwrap().revision;
        controller.run(Action::CloseDesign).unwrap();
        assert!(
            controller
                .finish_open(old_revision, Ok(stale(&fixture)))
                .is_err()
        );
        assert!(controller.snapshot().unwrap().design.is_none());
        controller.run(Action::OpenDesign(fixture.clone())).unwrap();
        let current = controller.snapshot().unwrap().design.unwrap();
        assert!(
            controller
                .finish_open(old_revision, Ok(stale(&fixture)))
                .is_err()
        );
        assert_eq!(
            controller.snapshot().unwrap().design.unwrap().inspection,
            current.inspection
        );
    }

    fn stale(path: &std::path::Path) -> OpenDesign {
        let prepared = cutcraft_import::prepare_file(path, &Recipe::new()).unwrap();
        OpenDesign {
            path: path.to_string_lossy().into_owned(),
            project_path: None,
            inspection: json!({"stale": true}),
            recipe: Recipe::new(),
            prepared: json!({"stale": true}),
            project: Project::new(prepared, MachineProfile::cameo_1()),
            bytes: Arc::new(Vec::new()),
            answered_by: Vec::new(),
        }
    }

    /// A page-sized white background under one black square, authored for these tests.
    const BACKGROUND_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100"><rect width="100" height="100" fill="white"/><rect x="10" y="10" width="20" height="20"/></svg>"#;

    #[test]
    fn answering_a_question_prepares_again_and_replaces_the_earlier_answer() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("original.svg");
        std::fs::write(&source, BACKGROUND_SVG).unwrap();
        let controller = Controller::new(SettingsStore::new(directory.path().join("config")));
        controller.run(Action::OpenDesign(source.clone())).unwrap();
        let prepared = controller.run(Action::Prepared).unwrap().value;
        assert_eq!(prepared["cut_ready"], false);
        let choice = &prepared["pending"][0]["choices"][1];
        assert_eq!(choice["cut"], false);
        let decision: Decision = serde_json::from_value(choice.clone()).unwrap();
        let answered = controller.run(Action::Decide(decision)).unwrap().value;
        assert_eq!(answered["cut_ready"], true);
        assert_eq!(answered["pieces"].as_array().unwrap().len(), 1);
        // A second answer to the same question replaces the first.
        let keep = Decision::Background {
            source_id: "source/0/0".into(),
            cut: true,
        };
        controller.run(Action::Decide(keep.clone())).unwrap();
        let design = controller.snapshot().unwrap().design.unwrap();
        assert_eq!(design.recipe.decisions, [keep]);
        assert_eq!(design.prepared["recipe"]["decisions"][0]["cut"], true);
        controller.run(Action::ResetDecisions).unwrap();
        let reset = controller.snapshot().unwrap().design.unwrap();
        assert!(reset.recipe.decisions.is_empty());
        assert_eq!(reset.prepared["cut_ready"], false);
        assert_eq!(std::fs::read_to_string(source).unwrap(), BACKGROUND_SVG);
    }

    #[test]
    fn cut_solid_and_dont_cut_answer_the_same_question() {
        let a = Decision::CutSolid {
            source_id: "source/0/0".into(),
            colour: None,
        };
        let b = Decision::ExcludeObject {
            source_id: "source/0/0".into(),
        };
        let other = Decision::ExcludeObject {
            source_id: "source/0/1".into(),
        };
        assert!(answers_same_question(&a, &b) && answers_same_question(&b, &a));
        assert!(!answers_same_question(&a, &other));
    }

    #[test]
    fn rejected_decisions_and_changed_sources_keep_the_previous_result() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("original.svg");
        std::fs::write(&source, BACKGROUND_SVG).unwrap();
        let controller = Controller::new(SettingsStore::new(directory.path().join("config")));
        controller.run(Action::OpenDesign(source.clone())).unwrap();
        let before = controller.snapshot().unwrap().design.unwrap();
        let wrong = Decision::Background {
            source_id: "source/0/1".into(),
            cut: false,
        };
        let error = controller.run(Action::Decide(wrong)).err().unwrap();
        assert!(error.contains("not a background candidate"), "{error}");
        let after = controller.snapshot().unwrap().design.unwrap();
        assert_eq!(after.prepared, before.prepared);
        assert!(after.recipe.decisions.is_empty());

        std::fs::write(&source, BACKGROUND_SVG.replace("x=\"10\"", "x=\"20\"")).unwrap();
        let error = controller.run(Action::ResetDecisions).err().unwrap();
        assert!(error.contains("changed on disk"), "{error}");
        assert_eq!(
            controller.snapshot().unwrap().design.unwrap().prepared,
            before.prepared
        );
    }

    #[test]
    fn capture_delivers_renderer_bytes_but_never_replaces_the_open_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("original.svg");
        let original = include_bytes!("../../import/tests/fixtures/one-inch.svg");
        std::fs::write(&source, original).unwrap();
        let controller = Controller::new(SettingsStore::new(directory.path().join("config")));
        controller.run(Action::OpenDesign(source.clone())).unwrap();
        controller.attach_renderer(Arc::new(|| {})).unwrap();
        let target = directory.path().join("view.png");
        for (path, should_succeed) in [(target.clone(), true), (source.clone(), false)] {
            let result = controller.spawn(Action::Capture {
                save_to: Some(path),
            });
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(request) = controller.take_capture().unwrap() {
                    // A renderer stand-in tests byte delivery and output policy, not PNG encoding.
                    request
                        .reply
                        .send(Ok(FrameImage {
                            png: b"authored renderer fixture".to_vec(),
                            width: 1,
                            height: 1,
                        }))
                        .unwrap();
                    break;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(
                result.recv_timeout(Duration::from_secs(2)).unwrap().is_ok(),
                should_succeed
            );
        }
        assert_eq!(std::fs::read(target).unwrap(), b"authored renderer fixture");
        assert_eq!(std::fs::read(source).unwrap(), original);
    }
}
