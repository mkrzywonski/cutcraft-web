//! Cutting (spec §5.5): the device session, its execution states, loading prompts, the test cut
//! and the worker that owns the device while a sheet is cut. Every step is a registry command,
//! so the window, MCP and `cutcraft-cli run` go through the same states and checks.
//!
//! The only device so far is the **Simulated Cameo** (spec §7.4), compiled with the
//! `dev-automation` feature. The real USB transport arrives with the hardware milestone (M4).

use std::sync::{Arc, Mutex};

use cutcraft_device::driver::{self, Clock, Controls, Job, Outcome, Phase, Progress};
use cutcraft_device::gpgl::{self, CAMEO_1, EncodedPlan};
use cutcraft_device::transport::Transport;
use cutcraft_device::usb::UsbTransport;
use cutcraft_import::job::{Mat, Material, Project, ProjectFile, Severity};
use cutcraft_path::CutPlan;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::commands::{CommandError, Origin};
use crate::cut_log::{CutLog, Recording};
use crate::{ActionOutput, Controller};

mod remote;

/// Execution states (spec §5.5). `Stopped` is the end of a sheet stopped on request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CutState {
    #[default]
    Disconnected,
    Idle,
    AwaitingSetup,
    Ready,
    Sending,
    Draining,
    Pausing,
    Paused,
    Stopping,
    Completed,
    Stopped,
    Interrupted,
}

impl CutState {
    /// The device worker owns the device: edits wait and nothing else starts.
    pub fn busy(self) -> bool {
        matches!(
            self,
            Self::Ready
                | Self::Sending
                | Self::Draining
                | Self::Pausing
                | Self::Paused
                | Self::Stopping
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum JobKind {
    Sheet,
    TestCut,
}

/// What is being cut, with the loading prompt they follow.
#[derive(Clone, Debug, Serialize)]
pub struct CutJob {
    pub kind: JobKind,
    pub plan_id: String,
    pub material: Material,
    pub mirror: bool,
    pub mat: Mat,
    /// The loading steps, in plain words (spec §5.5 step 1).
    pub instructions: Vec<String>,
    /// From hypothesised machine speeds; not a measured time.
    pub estimate_s: f64,
    pub bounds_mm: Option<[f64; 4]>,
    /// Uncompensated test-pattern origin, used for start-time validation.
    pub test_at_mm: Option<[f64; 2]>,
    pub test_on_scrap: bool,
    /// Who started it.
    pub origin: Origin,
}

/// The device as the window and automation see it.
#[derive(Clone, Debug, Default, Serialize)]
pub struct CuttingView {
    /// `Simulated Cameo`, or none.
    pub device: Option<String>,
    /// The simulator scenario, for a simulated device.
    pub scenario: Option<String>,
    /// The firmware a real cutter reported when connected.
    pub firmware: Option<String>,
    pub state: CutState,
    pub job: Option<CutJob>,
    pub progress: Option<Progress>,
    /// The line they read now.
    pub message: Option<String>,
    /// The sheet was cut to completion.
    pub completed: bool,
    /// The sheet's last cut was interrupted: what it cut is uncertain.
    pub interrupted: bool,
    /// Every test cut made on the loaded mat (`[x0, y0, x1, y1]` mm): the main cut and later
    /// test cuts keep clear of them.
    pub test_areas_mm: Vec<[f64; 4]>,
    /// The detailed log of the last cut (every transfer with its timing), for troubleshooting.
    pub last_log: Option<String>,
    /// A cameo-pi cutter has a mat loaded that the Pi didn't see going in: the person can
    /// confirm it has fresh material (`cut.confirm_fresh`).
    #[serde(default)]
    pub mat_unconfirmed: bool,
}

/// The device side that is not shown: the connection and the plan being cut.
#[derive(Default)]
pub(crate) struct DeviceSession {
    #[cfg(feature = "dev-automation")]
    sim: Option<cutcraft_sim::Simulator>,
    #[cfg(feature = "dev-automation")]
    scenario: Option<(cutcraft_sim::Scenario, SimClock)>,
    /// The real Cameo, owned by the session; the cutting worker locks it while it cuts.
    usb: Option<Arc<Mutex<UsbTransport>>>,
    controls: Option<Arc<Controls>>,
    /// A sheet was cut on the loaded mat: the next sheet starts on fresh material.
    mat_used: bool,
    plan: Option<Arc<CutPlan>>,
    encoded: Option<Arc<EncodedPlan>>,
    /// The sheet's own plan: what a remote cut sends, a test cut too (the Pi places the test).
    sheet_plan: Option<Arc<CutPlan>>,
    /// A cutter on a cameo-pi (spec §5.11).
    remote: Option<remote::Remote>,
}

pub(crate) type Session = Mutex<DeviceSession>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SimClock {
    /// Default in the window: the blade moves at its modelled speed.
    Realtime,
    /// Default headless: time passes only as fast as the simulation runs.
    #[default]
    Virtual,
}

/// Which cutter to connect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceKind {
    /// The Simulated Cameo (development builds).
    Simulated,
    /// The original Silhouette Cameo over USB.
    Cameo,
    /// A cutter on a paired cameo-pi (spec §5.11).
    Remote,
}

/// Set while following the hardware procedure (spec §9): until the motion sequence is
/// validated on the machine, a real cutter only moves when CutCraft was started with it.
pub const HARDWARE_TESTING_ENV: &str = "CUTCRAFT_HARDWARE_TESTING";

/// Wall-clock time for the real cutter.
struct WallClock(std::time::Instant);

impl Clock for WallClock {
    fn now_s(&self) -> f64 {
        self.0.elapsed().as_secs_f64()
    }
    fn sleep(&self, duration: std::time::Duration) {
        std::thread::sleep(duration);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LoadMedia {
    /// The 12 × 12 in mat.
    Mat12,
    /// The 12 × 24 in mat.
    Mat24,
    /// Roll material, without a mat.
    Roll,
    /// Unload.
    None,
}

/// Mat cutting on the real Cameo has passed its hardware tests (spec §9): the pen sheet measured
/// within 1 mm, Stop halts it promptly and a blade cut scored a full design (2026-10-19).
const MAT_VALIDATED: bool = true;
/// Roll cutting on the real Cameo has passed its hardware tests (spec §7.2).
const ROLL_VALIDATED: bool = false;

fn fail(message: impl Into<String>) -> CommandError {
    CommandError::new("failed", message)
}

fn disabled(message: impl Into<String>) -> CommandError {
    CommandError::new("disabled", message)
}

fn inches(mm: f64) -> String {
    format!("{:.2} in", mm / 25.4)
}

/// The loading prompt (spec §5.5 step 1). Device status alone never stands in for it.
/// The loading steps. `piece` is their material on the mat when they gave its size; `loaded` means
/// test cuts are already on the loaded mat, so it stays in.
fn instructions(
    plan: &CutPlan,
    kind: JobKind,
    piece: Option<[f64; 4]>,
    loaded: bool,
) -> Vec<String> {
    let material = &plan.material;
    let name = &material.name;
    let side = if material.mirror {
        "shiny side DOWN"
    } else {
        "backing paper down"
    };
    let mat = match plan.mat {
        Mat::Mat12x12 => "12 × 12 in",
        Mat::Mat12x24 => "12 × 24 in",
        Mat::Roll => "roll",
    };
    let mut steps = Vec::new();
    if plan.mat == Mat::Roll {
        steps.push(format!(
            "Feed {name} from the roll, {side}, square under both rollers against the left guide."
        ));
        if let Some([x0, y0, x1, y1]) = plan.bounds_mm {
            steps.push(format!(
                "The cut uses {} × {}, starting {} from the left and {} from the leading edge.",
                inches(x1 - x0),
                inches(y1 - y0),
                inches(x0),
                inches(y0)
            ));
        }
        steps.push(format!("Set the blade to {}.", material.blade));
        if !material.notes.is_empty() {
            steps.push(material.notes.clone());
        }
        steps.push("Choose Load Media on the Cameo (not Load Mat).".into());
        if plan.mirror {
            steps.push("This sheet is cut mirrored, as iron-on needs.".into());
        }
        return steps;
    }
    let place = |[x0, y0, x1, y1]: [f64; 4]| {
        format!(
            "your {} × {} piece of {name}, {side}, on the {mat} mat, {} from the left and {} from the top",
            inches(x1 - x0),
            inches(y1 - y0),
            inches(x0),
            inches(y0)
        )
    };
    match (kind, plan.bounds_mm) {
        (_, Some([x, y, ..])) if loaded => steps.push(match kind {
            JobKind::TestCut => format!(
                "Leave the mat loaded, with the earlier test cut in place. The next one goes {} from the left and {} from the top, clear of it.",
                inches(x),
                inches(y)
            ),
            JobKind::Sheet => {
                "Leave the mat loaded: the design is cut on the same material, clear of the test cuts.".into()
            }
        }),
        (JobKind::TestCut, Some([x, y, ..])) => steps.push(match piece {
            Some(piece) => format!(
                "Put {}. The test cut goes on it, {} from the left and {} from the top.",
                place(piece),
                inches(x),
                inches(y)
            ),
            None => format!(
                "Put a piece of {name}, {side}, on the {mat} mat, covering the spot {} from the left and {} from the top.",
                inches(x),
                inches(y)
            ),
        }),
        (JobKind::Sheet, Some(_)) if piece.is_some() => {
            steps.push(format!("Put {}.", place(piece.unwrap_or_default())));
        }
        (_, Some([x0, y0, x1, y1])) => steps.push(format!(
            "Put {name}, {side}, on the {mat} mat. The cut uses {} × {} starting {} from the left and {} from the top.",
            inches(x1 - x0),
            inches(y1 - y0),
            inches(x0),
            inches(y0)
        )),
        _ => steps.push(format!("Put {name}, {side}, on the {mat} mat.")),
    }
    steps.push(format!("Set the blade to {}.", material.blade));
    if !material.notes.is_empty() {
        steps.push(material.notes.clone());
    }
    if !loaded {
        steps.push("Choose Load Mat on the Cameo.".into());
    }
    if plan.mirror {
        steps.push("This sheet is cut mirrored, as iron-on needs.".into());
    }
    steps
}

/// A state's line for them to read.
fn state_message(state: CutState, job: Option<&CutJob>) -> Option<String> {
    let what = job.map_or("the design".to_string(), |j| match j.kind {
        JobKind::TestCut => "the test cut".to_string(),
        JobKind::Sheet => "the design".to_string(),
    });
    Some(match state {
        CutState::AwaitingSetup => format!("Load the material for {what}, then press Start."),
        CutState::Ready => format!("Checking the cutter before {what}…"),
        CutState::Sending | CutState::Draining => format!("Cutting {what}."),
        CutState::Pausing => "Pausing at the end of this cut…".into(),
        CutState::Paused => "Paused between cuts.".into(),
        CutState::Stopping => "Stopping — the cutter may finish buffered moves.".into(),
        _ => return None,
    })
}

impl Controller {
    fn cutting(&self) -> Result<CuttingView, CommandError> {
        Ok(self.snapshot().map_err(fail)?.cutting)
    }

    fn update_cutting(&self, update: impl FnOnce(&mut CuttingView)) -> Result<(), CommandError> {
        self.change(|s| update(&mut s.cutting)).map_err(fail)
    }

    fn session(&self) -> Result<std::sync::MutexGuard<'_, DeviceSession>, CommandError> {
        self.0
            .device
            .lock()
            .map_err(|_| fail("The device session is unavailable."))
    }

    /// Edits wait while a sheet is being cut (spec §5.5, §6.2.6).
    pub(crate) fn check_not_cutting(&self) -> Result<(), CommandError> {
        if self.cutting()?.state.busy() {
            return Err(CommandError::new(
                "busy",
                "Cutting in progress: this waits until the cut is done or stopped.",
            ));
        }
        Ok(())
    }

    pub(crate) fn device_connect(
        &self,
        kind: Option<DeviceKind>,
        scenario: Option<String>,
        clock: Option<SimClock>,
        station: Option<String>,
        cutter: Option<String>,
    ) -> Result<ActionOutput, CommandError> {
        self.check_not_cutting()?;
        self.check_remote_settled()?;
        if kind == Some(DeviceKind::Remote) {
            return self.connect_remote(station.as_deref(), cutter.as_deref());
        }
        if station.is_some() || cutter.is_some() {
            return Err(CommandError::new(
                "invalid-params",
                "station and cutter are for device: remote.",
            ));
        }
        self.release_remote();
        let simulated_default = cfg!(feature = "dev-automation");
        let kind = kind.unwrap_or(if simulated_default {
            DeviceKind::Simulated
        } else {
            DeviceKind::Cameo
        });
        if kind == DeviceKind::Cameo {
            return self.connect_cameo();
        }
        #[cfg(feature = "dev-automation")]
        {
            use cutcraft_sim::{Profile, Scenario, Simulator};
            let name = scenario.unwrap_or_else(|| "normal".into());
            // Mid-cut faults happen half-way through a typical sheet's stream.
            let scenario = Scenario::named(&name, 4096).ok_or_else(|| {
                CommandError::new(
                    "invalid-params",
                    format!(
                        "There is no scenario {name:?}; choose from {}.",
                        Scenario::NAMES.join(", ")
                    ),
                )
            })?;
            let clock = clock.unwrap_or(if self.is_live() {
                SimClock::Realtime
            } else {
                SimClock::Virtual
            });
            let sim = Simulator::new(
                Profile::cameo_1(),
                match clock {
                    SimClock::Realtime => cutcraft_sim::Clock::Realtime,
                    SimClock::Virtual => cutcraft_sim::Clock::Virtual,
                },
                &scenario,
            );
            // Ask what it is, as the real Cameo is asked; this also starts its status transient.
            let firmware =
                driver::query_firmware(&mut sim.transport(), std::time::Duration::from_secs(2))
                    .map_err(|e| {
                        fail(format!("The Simulated Cameo did not say what it is: {e}."))
                    })?;
            let media = sim.snapshot().media;
            {
                let mut session = self.session()?;
                *session = DeviceSession::default();
                session.sim = Some(sim);
                session.scenario = Some((scenario, clock));
            }
            self.update_cutting(|c| {
                c.device = Some("Simulated Cameo".into());
                c.scenario = Some(name.clone());
                c.firmware = Some(firmware.clone());
                c.state = CutState::Idle;
                c.job = None;
                c.progress = None;
                c.message = Some(
                    "Connected to the Simulated Cameo. Its behaviour models our assumptions about the machine, not the machine itself."
                        .into(),
                );
                c.test_areas_mm.clear();
            })?;
            Ok(ActionOutput::json(json!({
                "device": "Simulated Cameo",
                "firmware": firmware,
                "scenario": name,
                "clock": clock,
                "media": media,
                "simulated": true,
            })))
        }
        #[cfg(not(feature = "dev-automation"))]
        {
            let _ = (scenario, clock);
            Err(CommandError::new(
                "invalid-params",
                "This build has no Simulated Cameo; connect the real Cameo.",
            ))
        }
    }

    /// Open the real Cameo, ask its status and firmware (read-only), and refuse other models.
    fn connect_cameo(&self) -> Result<ActionOutput, CommandError> {
        let mut transport = UsbTransport::open(&CAMEO_1).map_err(|e| fail(e.to_string()))?;
        let timeout = std::time::Duration::from_secs(2);
        let firmware = driver::query_firmware(&mut transport, timeout)
            .map_err(|e| fail(format!("The cutter did not say what it is: {e}.")))?;
        if !firmware.starts_with("CAMEO ") {
            self.update_cutting(|c| {
                c.state = CutState::Disconnected;
                c.message = Some(format!(
                    "The connected cutter reports {firmware:?}; CutCraft supports the original Silhouette Cameo. Sending is disabled."
                ));
            })?;
            return Err(disabled(format!(
                "Unsupported model: the cutter reports {firmware:?}; CutCraft supports the original Silhouette Cameo."
            )));
        }
        // It must answer status, but a reply this soon after FG says nothing about the mat:
        // whether one is loaded is settled when a cut starts.
        driver::query_status(&mut transport, timeout)
            .map_err(|e| fail(format!("The cutter did not report its status: {e}.")))?;
        let info = transport.info.clone();
        {
            let mut session = self.session()?;
            *session = DeviceSession::default();
            session.usb = Some(Arc::new(Mutex::new(transport)));
        }
        let testing = std::env::var_os(HARDWARE_TESTING_ENV).is_some();
        self.update_cutting(|c| {
            c.device = Some("Silhouette Cameo".into());
            c.scenario = None;
            c.firmware = Some(firmware.clone());
            c.state = CutState::Idle;
            c.job = None;
            c.progress = None;
            c.test_areas_mm.clear();
            c.message = Some(if MAT_VALIDATED || testing {
                format!("Connected to the Cameo ({firmware}).")
            } else {
                format!(
                    "Connected to the Cameo ({firmware}). Cutting on it waits for the hardware tests; until then the Simulated Cameo cuts."
                )
            });
        })?;
        Ok(ActionOutput::json(json!({
            "device": "Silhouette Cameo",
            "firmware": firmware,
            "usb": {"interface": info.interface, "bulk_out": info.bulk_out, "bulk_in": info.bulk_in, "speed": info.speed},
            "simulated": false,
            "hardware_testing": testing,
        })))
    }

    pub(crate) fn device_disconnect(&self) -> Result<ActionOutput, CommandError> {
        self.check_not_cutting()?;
        self.check_remote_settled()?;
        self.release_remote();
        *self.session()? = DeviceSession::default();
        self.update_cutting(|c| {
            *c = CuttingView {
                completed: c.completed,
                interrupted: c.interrupted,
                ..CuttingView::default()
            }
        })?;
        Ok(ActionOutput::json(json!({"disconnected": true})))
    }

    /// The simulated Cameo's Load / Unload button.
    pub(crate) fn sim_load(&self, media: LoadMedia) -> Result<ActionOutput, CommandError> {
        if self.is_remote() {
            return Err(disabled(
                "Load the mat on the Cameo itself: cameo-pi sees it go in.",
            ));
        }
        if self.session()?.usb.is_some() {
            return Err(disabled(
                "Use the Load and Unload buttons on the Cameo itself.",
            ));
        }
        #[cfg(feature = "dev-automation")]
        {
            let session = self.session()?;
            let sim = session
                .sim
                .as_ref()
                .ok_or_else(|| disabled("Connect the Simulated Cameo first."))?;
            match media {
                LoadMedia::Mat12 => sim.load(cutcraft_sim::Media::Mat12x12),
                LoadMedia::Mat24 => sim.load(cutcraft_sim::Media::Mat12x24),
                LoadMedia::Roll => sim.load(cutcraft_sim::Media::Roll),
                LoadMedia::None => sim.unload(),
            }
            let media = sim.snapshot().media;
            drop(session);
            self.wake();
            Ok(ActionOutput::json(json!({"media": media})))
        }
        #[cfg(not(feature = "dev-automation"))]
        {
            let _ = media;
            Err(disabled("This build has no Simulated Cameo."))
        }
    }

    /// Choose the sheet (or its test cut) to cut: plan, validate and encode it, then wait for them
    /// to load the material (spec §5.5 step 1).
    pub(crate) fn cut_prepare(
        &self,
        origin: Origin,
        project: &Project,
        kind: JobKind,
        on_scrap: bool,
    ) -> Result<ActionOutput, CommandError> {
        let view = self.cutting()?;
        if view.device.is_none() {
            return Err(disabled(
                "No cutter is connected. Connect the Simulated Cameo (development builds) first.",
            ));
        }
        if view.state.busy() {
            return Err(CommandError::new("busy", "A cut is already running."));
        }
        let sheet = project
            .sheets
            .first()
            .ok_or_else(|| disabled("There is nothing to cut."))?;
        let sheet_plan = cutcraft_path::plan_sheet(project, &sheet.id).map_err(fail)?;
        if on_scrap && self.is_remote() {
            return Err(disabled(
                "Test cut on scrap isn't available on cameo-pi yet: use Test cut, which goes beside the design.",
            ));
        }
        let sheet_plan_copy = Arc::new(sheet_plan.clone());
        let sheet_plan_piece = sheet_plan.piece_rect_mm().filter(|_| !on_scrap);
        let plan = match kind {
            JobKind::Sheet => {
                if view
                    .test_areas_mm
                    .iter()
                    .any(|area| sheet_plan.overlaps_art(*area))
                {
                    return Err(fail(
                        "The design overlaps the test cut on this mat. Move the design on the mat, or load fresh material.",
                    ));
                }
                sheet_plan
            }
            JobKind::TestCut => {
                let at = if on_scrap {
                    Some([5.0, 5.0])
                } else if let Some(at) = sheet_plan.test_spot_mm
                    && overlaps_art_at(&sheet_plan, at)
                {
                    // Where they put it, but the design is there now.
                    return Err(fail(
                        "The test cut would cut into the design. Drag it to a clear spot, or choose Place automatically.",
                    ));
                } else {
                    next_test_spot(&sheet_plan, &view.test_areas_mm)
                };
                let at = at.ok_or_else(|| {
                    fail("There's no room for a test cut beside the design. Load a scrap of the same material and choose Test cut on scrap.")
                })?;
                cutcraft_path::test_cut_plan(project, &sheet.id, at).map_err(fail)?
            }
        };
        if !plan.valid {
            let problems: Vec<&str> = plan
                .diagnostics
                .iter()
                .filter(|d| d.severity == Severity::Blocking)
                .map(|d| d.message.as_str())
                .collect();
            return Err(disabled(format!(
                "The sheet can't be cut yet: {}",
                problems.join(" ")
            )));
        }
        let encoded = gpgl::encode(&plan, &CAMEO_1).map_err(|problems| {
            fail(
                problems
                    .iter()
                    .map(|d| d.message.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        })?;
        let job = CutJob {
            kind,
            plan_id: plan.plan_id.clone(),
            material: plan.material.clone(),
            mirror: plan.mirror,
            mat: plan.mat,
            instructions: instructions(
                &plan,
                kind,
                sheet_plan_piece,
                !view.test_areas_mm.is_empty() && !on_scrap,
            ),
            estimate_s: plan.estimate_s,
            bounds_mm: plan.bounds_mm,
            test_on_scrap: on_scrap,
            test_at_mm: (kind == JobKind::TestCut)
                .then(|| plan.operations.last().unwrap().points[0]),
            origin,
        };
        {
            let mut session = self.session()?;
            session.plan = Some(Arc::new(plan));
            session.encoded = Some(Arc::new(encoded));
            session.sheet_plan = Some(sheet_plan_copy);
            session.controls = None;
        }
        let value = json!(job);
        self.update_cutting(|c| {
            c.state = CutState::AwaitingSetup;
            c.message = state_message(CutState::AwaitingSetup, Some(&job));
            c.progress = None;
            c.job = Some(job);
        })?;
        Ok(ActionOutput::json(value))
    }

    /// They confirmed the material is loaded: check the plan is still current and start the
    /// device worker (spec §5.5 step 2).
    pub(crate) fn cut_start(
        &self,
        design: Option<&crate::OpenDesign>,
    ) -> Result<ActionOutput, CommandError> {
        let view = self.cutting()?;
        if view.state != CutState::AwaitingSetup {
            return Err(disabled("Choose Cut or Test cut first."));
        }
        let job = view
            .job
            .clone()
            .ok_or_else(|| disabled("Nothing is ready to cut."))?;
        // Validation runs again immediately before sending (spec §5.4).
        let mut project = design.map(|d| d.project.clone());
        if job.test_on_scrap
            && let Some(sheet) = project.as_mut().and_then(|p| p.sheets.first_mut())
        {
            sheet.piece_mm = None;
            sheet.piece_at_mm = None;
        }
        let current = project
            .as_ref()
            .and_then(|p| p.sheets.first().map(|s| (p, s)))
            .and_then(|(p, s)| match job.kind {
                JobKind::Sheet => cutcraft_path::plan_sheet(p, &s.id).ok(),
                JobKind::TestCut => job
                    .test_at_mm
                    .and_then(|at| cutcraft_path::test_cut_plan(p, &s.id, at).ok()),
            });
        if current
            .as_ref()
            .is_none_or(|p| !p.valid || p.plan_id != job.plan_id)
        {
            self.update_cutting(|c| {
                c.state = CutState::Idle;
                c.job = None;
                c.message = Some("The sheet or test-cut settings changed after it was chosen. Look at the Cut Preview again, then choose the cut again.".into());
            })?;
            return Err(fail(
                "The sheet or test-cut settings changed after it was chosen; nothing was sent. Look at the Cut Preview again, then choose the cut again.",
            ));
        }
        if self.is_remote() {
            let design = design.ok_or_else(|| disabled("Open a design to cut."))?;
            return self.remote_start(job, design);
        }
        self.spawn_cut(job)
    }

    fn spawn_cut(&self, job: CutJob) -> Result<ActionOutput, CommandError> {
        let (link, encoded, plan, controls) = {
            let mut session = self.session()?;
            let encoded = session
                .encoded
                .clone()
                .ok_or_else(|| disabled("Nothing is ready to cut."))?;
            let plan = session
                .plan
                .clone()
                .ok_or_else(|| disabled("Nothing is ready to cut."))?;
            let link = match &session.usb {
                Some(usb) => {
                    let testing = std::env::var_os(HARDWARE_TESTING_ENV).is_some();
                    if !(MAT_VALIDATED || testing) {
                        return Err(disabled(format!(
                            "Cutting on the real Cameo waits until its motion is validated by the hardware tests (spec §9). For those tests, start CutCraft with {HARDWARE_TESTING_ENV}=1."
                        )));
                    }
                    // Spec §7.2: roll mode stays off until its own hardware tests pass, even
                    // once mat cutting is validated.
                    if job.mat == Mat::Roll && !(ROLL_VALIDATED || testing) {
                        return Err(disabled(
                            "Cutting from a roll on the real Cameo waits for its own hardware tests; use a mat.",
                        ));
                    }
                    Link::Usb(usb.clone())
                }
                None => self.simulator_link(&mut session, encoded.bytes.len())?,
            };
            session.mat_used = false;
            let controls = Arc::new(Controls::default());
            session.controls = Some(controls.clone());
            (link, encoded, plan, controls)
        };
        self.update_cutting(|c| {
            c.state = CutState::Ready;
            c.message = state_message(CutState::Ready, Some(&job));
            c.progress = None;
        })?;
        let controller = self.clone();
        let started = json!({"started": true, "job": job});
        let machine = cutcraft_import::job::MachineProfile::cameo_1();
        let mm_per_s = f64::from(plan.material.speed) * machine.speed_step_mm_s;
        // The log's header: what was cut, on what, with which settings (no design names).
        let view = self.snapshot().map(|s| s.cutting).unwrap_or_default();
        let header = json!({
            "app_version": crate::APP_BUILD,
            "device": view.device,
            "firmware": view.firmware,
            "scenario": view.scenario,
            "hardware_testing": std::env::var_os(HARDWARE_TESTING_ENV).is_some(),
            "kind": job.kind,
            "plan_id": job.plan_id,
            "material": {
                "name": job.material.name,
                "force": plan.material.force,
                "speed": plan.material.speed,
                "passes": plan.material.passes,
                "status": job.material.status,
            },
            "mat": job.mat,
            "mirror": job.mirror,
            "bounds_mm": job.bounds_mm,
            "estimate_s": job.estimate_s,
            "bytes": encoded.bytes.len(),
            "passes": encoded.passes.len(),
            "mm_per_s": mm_per_s,
            "lead_s": driver::LEAD_S,
            "settle_s": driver::SETTLE_S,
        });
        let name = match job.kind {
            JobKind::TestCut => "test".to_string(),
            JobKind::Sheet => "sheet".to_string(),
        };
        let log_dir = {
            let settings = self.0.store.path();
            (!settings.as_os_str().is_empty()).then(|| settings.with_file_name("cut-logs"))
        };
        std::thread::spawn(move || {
            let log = std::cell::RefCell::new(CutLog::create(log_dir.as_deref(), &name, header));
            let spec = Job {
                plan: &encoded,
                mm_per_s,
                travel_mm_s: machine.travel_mm_s,
                tool_change_s: machine.tool_change_s,
                point_s: machine.point_s,
                turn_s_per_rad: machine.turn_s_per_rad,
                units_per_mm: CAMEO_1.units_per_mm,
            };
            let outcome = match link {
                #[cfg(feature = "dev-automation")]
                Link::Sim(sim) => {
                    controller.drive(&mut sim.transport(), &sim, &spec, &controls, &log)
                }
                Link::Usb(usb) => match usb.lock() {
                    Ok(mut transport) => controller.drive(
                        &mut *transport,
                        &WallClock(std::time::Instant::now()),
                        &spec,
                        &controls,
                        &log,
                    ),
                    Err(_) => Outcome::Interrupted {
                        reason: "the cutter connection is unavailable".into(),
                        passes_sent: 0,
                    },
                },
            };
            let mut log = log.into_inner();
            log.event(json!({"ev": "outcome", "result": outcome}), true);
            let path = log.path().map(|p| p.to_string_lossy().into_owned());
            controller.finish_cut(&job, outcome, path);
        });
        Ok(ActionOutput::json(started))
    }

    #[cfg(not(feature = "dev-automation"))]
    fn simulator_link(
        &self,
        _session: &mut DeviceSession,
        _bytes: usize,
    ) -> Result<Link, CommandError> {
        Err(disabled("No cutter is connected."))
    }

    /// The simulator for the next cut. A scripted fault happens half-way through this cut, on a
    /// freshly loaded machine (a power cut or pulled cable needs one anyway).
    #[cfg(feature = "dev-automation")]
    fn simulator_link(
        &self,
        session: &mut DeviceSession,
        bytes: usize,
    ) -> Result<Link, CommandError> {
        let sim = session
            .sim
            .clone()
            .ok_or_else(|| disabled("No cutter is connected."))?;
        let sim = match &session.scenario {
            Some((scenario, clock))
                if !scenario.triggers.is_empty() || !sim.snapshot().powered || session.mat_used =>
            {
                let media = sim.snapshot().media;
                let mut fresh = cutcraft_sim::Scenario::named(&scenario.name, bytes)
                    .unwrap_or_else(|| scenario.clone());
                fresh.media = media.or(fresh.media);
                let sim = cutcraft_sim::Simulator::new(
                    cutcraft_sim::Profile::cameo_1(),
                    match clock {
                        SimClock::Realtime => cutcraft_sim::Clock::Realtime,
                        SimClock::Virtual => cutcraft_sim::Clock::Virtual,
                    },
                    &fresh,
                );
                session.sim = Some(sim.clone());
                sim
            }
            _ => sim,
        };
        Ok(Link::Sim(sim))
    }

    /// Run the driver, reporting progress into the cutting view and every transfer, phase
    /// and pace change into `log`.
    fn drive(
        &self,
        transport: &mut impl Transport,
        clock: &impl Clock,
        spec: &Job,
        controls: &Controls,
        log: &std::cell::RefCell<CutLog>,
    ) -> Outcome {
        let mut last = None;
        let mut logged: Option<Progress> = None;
        let mut recording = Recording {
            inner: transport,
            log,
        };
        driver::run(&mut recording, clock, spec, controls, |p| {
            let noteworthy = logged.as_ref().is_none_or(|l| {
                l.phase != p.phase || l.pace != p.pace || (p.fraction - l.fraction).abs() >= 0.02
            });
            if noteworthy {
                logged = Some(p.clone());
                log.borrow_mut().event(
                    json!({
                        "ev": "progress",
                        "phase": p.phase,
                        "sent_bytes": p.sent_bytes,
                        "passes_sent": p.passes_sent,
                        "fraction": (p.fraction * 1000.0).round() / 1000.0,
                        "pace": p.pace,
                        "stop": controls.stopping(),
                    }),
                    false,
                );
            }
            // Report phase changes at once, and progress (with the estimated blade) about ten
            // times a second.
            let changed = last
                .as_ref()
                .is_none_or(|(l, at): &(Progress, std::time::Instant)| {
                    l.phase != p.phase
                        || (p.fraction - l.fraction).abs() >= 0.005
                        || at.elapsed() >= std::time::Duration::from_millis(100)
                });
            if changed {
                last = Some((p.clone(), std::time::Instant::now()));
                let p = p.clone();
                let _ = self.update_cutting(|c| {
                    let state = match p.phase {
                        Phase::Sending => CutState::Sending,
                        Phase::Draining => CutState::Draining,
                        Phase::Pausing => CutState::Pausing,
                        Phase::Paused => CutState::Paused,
                        Phase::Stopping => CutState::Stopping,
                    };
                    // Stop shows at once, before the driver reaches it.
                    if c.state != CutState::Stopping || state == CutState::Stopping {
                        c.state = state;
                        c.message = state_message(state, c.job.as_ref());
                    }
                    c.progress = Some(p);
                });
            }
        })
    }

    fn finish_cut(&self, job: &CutJob, outcome: Outcome, log: Option<String>) {
        // The record is kept before the sheet is reported finished.
        let mut record_error = None;
        if job.kind == JobKind::Sheet {
            match outcome {
                Outcome::Completed { .. } => record_error = self.record_sheet(job, true),
                Outcome::Interrupted { .. } => record_error = self.record_sheet(job, false),
                _ => {}
            }
        }
        let what = match job.kind {
            JobKind::TestCut => "The test cut".to_string(),
            JobKind::Sheet => "The sheet".to_string(),
        };
        let _ = self.update_cutting(|c| {
            c.last_log = log;
            match &outcome {
                Outcome::Completed { .. } => {
                    c.state = CutState::Completed;
                    if let Some(p) = &mut c.progress {
                        p.fraction = 1.0;
                        p.remaining_s = 0.0;
                    }
                    match job.kind {
                        JobKind::Sheet => {
                            c.completed = true;
                            c.interrupted = false;
                            c.test_areas_mm.clear();
                            c.message = Some(format!("{what} done. Unload the mat."));
                        }
                        JobKind::TestCut => {
                            c.test_areas_mm.extend(job.bounds_mm);
                            c.message = Some(
                                "Test cut done. Inspect the pattern; adjust the material settings and try a fresh spot if needed."
                                    .into(),
                            );
                        }
                    }
                }
                Outcome::Refused { reason } => {
                    c.state = CutState::AwaitingSetup;
                    c.progress = None;
                    c.message = Some(format!("The job was refused before sending anything: {reason}"));
                }
                Outcome::NotLoaded => {
                    c.state = CutState::AwaitingSetup;
                    c.progress = None;
                    c.message = Some(
                        "The Cameo says nothing is loaded. Choose Load Mat on the Cameo, then press Start again."
                            .into(),
                    );
                }
                Outcome::Stopped { idle, .. } => {
                    c.state = CutState::Stopped;
                    c.message = Some(if *idle {
                        format!(
                            "Stopped. {what} was not finished; the cutter reports it is idle. Load fresh material to cut it again."
                        )
                    } else {
                        format!(
                            "Stopped, but the cutter did not confirm it is idle. Check it, switch it off if it is still moving, then load fresh material to cut {} again.",
                            what.to_lowercase()
                        )
                    });
                }
                Outcome::Interrupted { reason, .. } => {
                    c.state = CutState::Interrupted;
                    if job.kind == JobKind::Sheet {
                        c.completed = false;
                        c.interrupted = true;
                    }
                    c.message = Some(format!(
                        "The cut was interrupted. Check the cutter, then load fresh material to cut this sheet again. ({reason}){}",
                        if c.last_log.is_some() { " A detailed log was saved for troubleshooting." } else { "" }
                    ));
                }
            }
            if let Some(error) = record_error {
                c.message = Some(format!("{} {error}", c.message.take().unwrap_or_default()));
            }
        });
        if let Ok(mut session) = self.session() {
            session.controls = None;
            session.mat_used |= job.kind == JobKind::Sheet;
        }
    }

    /// Keep a finished or interrupted sheet in the project's cut record, and in its saved file
    /// when it has one (spec §5.5: persist progress at sheet boundaries).
    fn record_sheet(&self, job: &CutJob, completed: bool) -> Option<String> {
        let mut saved = None;
        let _ = self.change(|s| {
            let Some(design) = s.design.as_mut().map(Arc::make_mut) else {
                return;
            };
            let Some(sheet) = design.project.sheets.first() else {
                return;
            };
            let id = sheet.id.clone();
            let record = &mut design.project.cut_record;
            if completed {
                record.complete(&id, &job.plan_id);
            } else {
                record.interrupt(&id);
            }
            saved = design.project_path.clone().map(|p| (p, record.clone()));
        });
        persist_record(saved)
    }

    /// Start over: forget that the sheet was cut, here and in the saved project.
    pub(crate) fn cut_reset(&self) -> Result<ActionOutput, CommandError> {
        self.check_not_cutting()?;
        let mut saved = None;
        self.change(|s| {
            // Only the record: a cut being set up meanwhile (Cut again) keeps its state.
            s.cutting.completed = false;
            s.cutting.interrupted = false;
            if let Some(design) = s.design.as_mut().map(Arc::make_mut) {
                design.project.cut_record = Default::default();
                saved = design
                    .project_path
                    .clone()
                    .map(|p| (p, design.project.cut_record.clone()));
            }
        })
        .map_err(fail)?;
        if let Some(error) = persist_record(saved) {
            return Err(fail(error));
        }
        Ok(ActionOutput::json(
            json!({"completed": false, "interrupted": false}),
        ))
    }

    pub(crate) fn cut_pause(&self, pause: bool) -> Result<ActionOutput, CommandError> {
        let view = self.cutting()?;
        if self.is_remote() {
            if pause && !matches!(view.state, CutState::Sending | CutState::Ready) {
                return Err(disabled("Pause works while a sheet is being sent."));
            }
            if !pause && !matches!(view.state, CutState::Paused | CutState::Pausing) {
                return Err(disabled("Nothing is paused."));
            }
            let out = self.remote_control(if pause { "pause" } else { "resume" })?;
            if pause {
                self.update_cutting(|c| {
                    c.state = CutState::Pausing;
                    c.message = state_message(CutState::Pausing, c.job.as_ref());
                })?;
            }
            return Ok(out);
        }
        let controls = self
            .session()?
            .controls
            .clone()
            .ok_or_else(|| disabled("Nothing is being cut."))?;
        if pause {
            if !matches!(view.state, CutState::Sending | CutState::Ready) {
                return Err(disabled("Pause works while a sheet is being sent."));
            }
            controls.pause();
            self.update_cutting(|c| {
                c.state = CutState::Pausing;
                c.message = state_message(CutState::Pausing, c.job.as_ref());
            })?;
        } else {
            if !matches!(view.state, CutState::Paused | CutState::Pausing) {
                return Err(disabled("Nothing is paused."));
            }
            controls.resume();
        }
        Ok(ActionOutput::json(json!({"pause_requested": pause})))
    }

    /// Stop answers at once (spec §5.5): nothing more is sent and the sheet is abandoned.
    pub(crate) fn cut_stop(&self) -> Result<ActionOutput, CommandError> {
        let view = self.cutting()?;
        if self.is_remote() {
            return self.remote_stop(view.state);
        }
        let controls = self.session()?.controls.clone();
        match (view.state, controls) {
            (CutState::AwaitingSetup, _) => {
                self.update_cutting(|c| {
                    c.state = CutState::Idle;
                    c.job = None;
                    c.message = Some("Cancelled; nothing was sent.".into());
                })?;
                Ok(ActionOutput::json(json!({"stopped": true, "sent": false})))
            }
            (state, Some(controls)) if state.busy() => {
                controls.stop();
                self.update_cutting(|c| {
                    c.state = CutState::Stopping;
                    c.message = state_message(CutState::Stopping, c.job.as_ref());
                })?;
                Ok(ActionOutput::json(json!({"stopped": true, "sent": true})))
            }
            _ => Err(disabled("Nothing is being cut.")),
        }
    }

    /// Leave the cut workflow without starting, stopping or forgetting a physical cut.
    pub(crate) fn cut_exit(&self) -> Result<ActionOutput, CommandError> {
        if self.cutting()?.state.busy() {
            return Err(CommandError::new(
                "busy",
                "Stop the running cut before exiting.",
            ));
        }
        self.release_remote();
        self.change(|s| {
            clear_review(&mut s.cutting);
            s.view.screen = if s.design.is_some() {
                crate::Screen::Mat
            } else {
                crate::Screen::SideBySide
            };
        })
        .map_err(fail)?;
        let snapshot = self.snapshot().map_err(fail)?;
        Ok(ActionOutput::json(
            json!({"screen":snapshot.view.screen,"state":snapshot.cutting.state}),
        ))
    }

    /// Back to choosing what to cut after a sheet ends.
    pub(crate) fn cut_dismiss(&self) -> Result<ActionOutput, CommandError> {
        let view = self.cutting()?;
        if !matches!(
            view.state,
            CutState::Completed | CutState::Stopped | CutState::Interrupted
        ) {
            return Err(disabled("There is no finished cut to close."));
        }
        self.update_cutting(|c| {
            c.state = CutState::Idle;
            c.job = None;
            c.progress = None;
            c.message = None;
        })?;
        Ok(ActionOutput::json(json!({"state": CutState::Idle})))
    }

    /// The simulated mat as cut so far, for the live view: blade-down and travel segments in
    /// mm, the tool position and what is loaded. None without a simulated device.
    /// The job being cut (or last cut) as encoded: its passes, byte ranges and paths in device
    /// units, for drawing estimated progress.
    pub fn active_cut_plan(&self) -> Option<Arc<CutPlan>> {
        self.0.device.lock().ok()?.plan.clone()
    }

    pub fn cut_passes(&self) -> Option<Arc<EncodedPlan>> {
        self.0.device.lock().ok()?.encoded.clone()
    }

    pub fn virtual_mat(&self) -> Option<VirtualMat> {
        #[cfg(feature = "dev-automation")]
        {
            let session = self.0.device.lock().ok()?;
            let sim = session.sim.as_ref()?;
            Some(sim.with_machine(|m| {
                let snapshot = m.snapshot();
                VirtualMat {
                    segments: m
                        .trace()
                        .iter()
                        .map(|s| (s.from, s.to, s.tool_down))
                        .collect(),
                    position_mm: snapshot.position_mm,
                    tool_down: snapshot.tool_down,
                    loaded: snapshot.media.is_some(),
                    faults: m.faults().iter().map(|f| f.detail.clone()).collect(),
                }
            }))
        }
        #[cfg(not(feature = "dev-automation"))]
        None
    }
}

/// The connection a cut runs over.
enum Link {
    #[cfg(feature = "dev-automation")]
    Sim(cutcraft_sim::Simulator),
    Usb(Arc<Mutex<UsbTransport>>),
}

/// What the simulated blade has done.
#[derive(Clone, Debug, Default, Serialize)]
pub struct VirtualMat {
    /// `(from, to, blade down)` in mm, `[x, y]`.
    pub segments: Vec<([f64; 2], [f64; 2], bool)>,
    pub position_mm: [f64; 2],
    pub tool_down: bool,
    pub loaded: bool,
    pub faults: Vec<String>,
}

/// The test cut's square at `at`.
fn test_square(at: [f64; 2]) -> [f64; 4] {
    let s = cutcraft_path::TEST_CUT_MM;
    [at[0], at[1], at[0] + s, at[1] + s]
}

fn overlaps_art_at(plan: &CutPlan, at: [f64; 2]) -> bool {
    plan.overlaps_art(test_square(at))
}

/// Where the next test cut goes (spec §5.5): where they put it, unless an earlier test cut on
/// this mat is already there; otherwise the first spot clear of the design and every earlier
/// test cut. None when nothing fits.
pub fn next_test_spot(plan: &CutPlan, used: &[[f64; 4]]) -> Option<[f64; 2]> {
    let free = |at: [f64; 2]| {
        let [x0, y0, x1, y1] = test_square(at);
        used.iter()
            .all(|u| x1 <= u[0] || x0 >= u[2] || y1 <= u[1] || y0 >= u[3])
    };
    if let Some(at) = plan.test_spot_mm.filter(|at| free(*at)) {
        return Some(at);
    }
    let mut occupied: Vec<[f64; 4]> = plan.bounds_mm.into_iter().collect();
    occupied.extend_from_slice(used);
    cutcraft_path::test_cut_spot(plan.usable_area_mm(), &occupied)
}

/// What the Cut Preview marks for test cuts (spec §5.5): each one already cut on the loaded mat
/// (solid, `true`), then where the next one goes (dashed, `false`; draggable).
pub fn test_cut_markers(plan: &CutPlan, cutting: &CuttingView) -> Vec<([f64; 4], bool)> {
    let mut marks: Vec<([f64; 4], bool)> =
        cutting.test_areas_mm.iter().map(|a| (*a, true)).collect();
    if let Some(at) = next_test_spot(plan, &cutting.test_areas_mm) {
        marks.push((test_square(at), false));
    }
    marks
}

/// Write the cut record into the saved project, if there is one; the error, in their words.
fn persist_record(saved: Option<(String, cutcraft_import::job::CutRecord)>) -> Option<String> {
    let (path, record) = saved?;
    ProjectFile::record_cut(std::path::Path::new(&path), &record)
        .err()
        .map(|error| format!("The project file could not record that the sheet was cut: {error}"))
}

/// The cut view of a reopened project: whether its sheet was cut, or interrupted.
pub(crate) fn restore_record(c: &mut CuttingView, project: &Project) {
    let id = project.sheets.first().map(|s| s.id.as_str()).unwrap_or("");
    c.completed = project.cut_record.is_completed(id);
    c.interrupted = project.cut_record.interrupted.as_deref() == Some(id);
}

/// Clear a reviewed or finished job, retaining the loaded mat's occupied areas and cut record.
pub(crate) fn clear_review(c: &mut CuttingView) {
    if !c.state.busy() {
        c.state = if c.device.is_some() {
            CutState::Idle
        } else {
            CutState::Disconnected
        };
        c.job = None;
        c.progress = None;
        c.message = None;
    }
}

/// A new design starts uncut.
pub(crate) fn reset_record(c: &mut CuttingView) {
    c.completed = false;
    c.interrupted = false;
    c.test_areas_mm.clear();
    if matches!(
        c.state,
        CutState::AwaitingSetup | CutState::Completed | CutState::Stopped | CutState::Interrupted
    ) {
        c.state = CutState::Idle;
        c.job = None;
        c.progress = None;
        c.message = None;
    }
}
