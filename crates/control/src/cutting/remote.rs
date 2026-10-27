//! A cutter on a cameo-pi (spec §5.11): the same cut states as a USB Cameo, driven through the
//! Pi's protocol. The Pi owns the cutter, its turns and its mat; CutCraft asks, and follows the
//! Pi's state about four times a second.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cutcraft_device::driver::{Outcome, Phase};
use cutcraft_import::job::ProjectFile;
use cutcraft_station::{Answer, CutterView, Error as StationError, Kind, Mat, Start, Station};
use serde_json::json;

use super::{CutJob, CutState, DeviceSession, JobKind, disabled, fail, state_message};
use crate::commands::CommandError;
use crate::stations::host_name;
use crate::{ActionOutput, Controller, OpenDesign};

/// How often the Pi's state is read.
const POLL: Duration = Duration::from_millis(250);
/// How often an unanswered Stop is sent again.
const STOP_AGAIN: Duration = Duration::from_secs(2);
/// The library folder CutCraft's cuts go into.
const FOLDER: &str = "CutCraft";

static CONNECTIONS: AtomicU64 = AtomicU64::new(0);

/// The connected remote cutter.
pub(crate) struct Remote {
    station: Arc<Station>,
    cutter: String,
    /// The name its user gave it on the Pi.
    name: String,
    /// A poller stops once the session has a newer connection.
    connection: u64,
    /// The last state read from the Pi.
    view: Option<CutterView>,
    boot: String,
    /// The design sent, and as which job.
    sent: Option<Sent>,
    /// The turn asked for, while it lasts.
    turn: Option<String>,
    /// The execution started from here, kept until the Pi has recorded how it ended.
    running: Option<Running>,
    /// A Start being prepared or sent, until it's definitely started or definitely not.
    pending: Option<Pending>,
    /// Changes whenever `running` or a sent Start changes, so a look at the Pi that began before
    /// is ignored rather than mistaken for the cut having ended.
    fence: u64,
    /// Waiting in line: say so when the turn comes.
    in_line: bool,
}

/// A cut started on the Pi.
struct Running {
    execution: String,
    job: CutJob,
    job_id: String,
    /// When the Pi's view first stopped showing it, while its outcome isn't recorded yet.
    gone_since: Option<Instant>,
    /// Stop was pressed and the Pi hasn't answered it yet: when it was last sent. It's sent
    /// again until the Pi answers.
    stopping: Option<Instant>,
}

/// One press of Start on a cameo-pi cutter, while its result isn't known.
struct Pending {
    op: u64,
    job: CutJob,
    job_id: Option<String>,
    /// Stop was pressed: nothing more may be sent for it, and a Start that got through is stopped.
    cancelled: bool,
    /// The Start as sent (its body and key), once it may have reached the Pi. Resent unchanged
    /// while the answer is unknown; the Pi answers a repeat with the original result.
    sent: Option<Start>,
    /// The Pi answered the Stop sent for it, so a Start arriving later is stale there.
    stop_answered: bool,
}

static OPERATIONS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
struct Sent {
    /// The design's path, or its project's.
    design: String,
    job_id: String,
    revision: u32,
    /// The sheet plan it was sent with.
    plan_id: String,
}

fn station_error(error: StationError) -> CommandError {
    fail(error.to_string())
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// The cut state a running execution's view shows.
fn running_state(view: &CutterView) -> CutState {
    match view.activity.as_str() {
        "paused" => CutState::Paused,
        "stopping" | "confirming-stop" => CutState::Stopping,
        _ => match view.progress.as_ref().map(|p| p.phase) {
            Some(Phase::Draining) => CutState::Draining,
            Some(Phase::Pausing) => CutState::Pausing,
            Some(Phase::Paused) => CutState::Paused,
            Some(Phase::Stopping) => CutState::Stopping,
            _ => CutState::Sending,
        },
    }
}

/// How an execution ended, from the job's history on the Pi, once it's recorded there. `None`
/// means not known yet (not recorded, or the Pi couldn't be asked): the cut may still be running.
fn recorded_outcome(station: &Station, job_id: &str, execution: &str) -> Option<Outcome> {
    let job = station.job(job_id).ok()?;
    let entry = job
        .history
        .iter()
        .rev()
        .find(|e| e.execution == execution)?;
    Some(match entry.ended.as_str() {
        "finished" => Outcome::Completed { duration_s: 0.0 },
        "stopped" => Outcome::Stopped {
            idle: true,
            passes_sent: 0,
        },
        "not-run" => Outcome::Refused {
            reason: "cameo-pi didn't run it. Check the mat is loaded.".into(),
        },
        _ => Outcome::Interrupted {
            reason: "cameo-pi reported it interrupted".into(),
            passes_sent: 0,
        },
    })
}

/// Whether a Stop's answer proves the Pi handled it: a stopping cut, or "nothing is cutting",
/// which also makes any Start still on its way stale there. Any other refusal (overloaded, an
/// error, an older Pi) proves nothing, and the Stop is sent again.
fn stop_reached_the_cutter(answer: &Result<Answer, StationError>) -> bool {
    match answer {
        Ok(_) => true,
        Err(StationError::Refused { code, .. }) => code == "not-cutting",
        Err(_) => false,
    }
}

/// A turn taken out of the session, to hand back to the Pi once the session is unlocked.
struct HeldTurn {
    station: Arc<Station>,
    cutter: String,
    turn: String,
    current: bool,
}

impl HeldTurn {
    fn hand_back(self) {
        let _ = if self.current {
            self.station.release(&self.cutter, &self.turn)
        } else {
            self.station.remove(&self.cutter, &self.turn)
        };
    }
}

fn take_turn(remote: &mut Remote) -> Option<HeldTurn> {
    let turn = remote.turn.take()?;
    remote.in_line = false;
    let current = remote
        .view
        .as_ref()
        .and_then(|v| v.turn.as_ref())
        .is_some_and(|t| t.id == turn);
    Some(HeldTurn {
        station: remote.station.clone(),
        cutter: remote.cutter.clone(),
        turn,
        current,
    })
}

/// An answer that may hide a Start the Pi accepted: no answer, or one CutCraft couldn't read.
fn ambiguous(error: &StationError) -> bool {
    matches!(
        error,
        StationError::Unreachable(_) | StationError::Protocol(_)
    )
}

impl Controller {
    fn remote_parts(&self) -> Result<(Arc<Station>, String, String), CommandError> {
        let session = self.session()?;
        let remote = session
            .remote
            .as_ref()
            .ok_or_else(|| disabled("No cameo-pi cutter is connected."))?;
        Ok((
            remote.station.clone(),
            remote.cutter.clone(),
            remote.name.clone(),
        ))
    }

    pub(crate) fn is_remote(&self) -> bool {
        self.session().is_ok_and(|s| s.remote.is_some())
    }

    /// Whether the connected cutter is on the Pi at `address`.
    pub(crate) fn cutting_on_station(&self, address: &str) -> bool {
        self.session().is_ok_and(|s| {
            s.remote.as_ref().is_some_and(|r| {
                r.station
                    .credentials()
                    .address
                    .eq_ignore_ascii_case(address.trim())
            })
        })
    }

    /// Connects a cutter on a paired Pi: `cutter` by its id, or the Pi's only cutter.
    pub(crate) fn connect_remote(
        &self,
        address: Option<&str>,
        cutter: Option<&str>,
    ) -> Result<ActionOutput, CommandError> {
        let station = self.station(address)?;
        let state = station.state().map_err(station_error)?;
        let chosen = match cutter {
            Some(id) => state.cutters.iter().find(|c| c.id == id).ok_or_else(|| {
                CommandError::new(
                    "invalid-params",
                    format!(
                        "That cameo-pi has no cutter {id:?}; it has {}.",
                        state
                            .cutters
                            .iter()
                            .map(|c| c.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })?,
            None => match state.cutters.as_slice() {
                [only] => only,
                _ => {
                    return Err(CommandError::new(
                        "invalid-params",
                        "That cameo-pi has several cutters: say which (cutter).",
                    ));
                }
            },
        };
        self.release_remote();
        let host = host_name(&station.credentials().address);
        let connection = CONNECTIONS.fetch_add(1, Ordering::SeqCst) + 1;
        let name = chosen.view.name.clone();
        let device = format!("{name} (on {host})");
        {
            let mut session = self.session()?;
            *session = DeviceSession::default();
            session.remote = Some(Remote {
                station: station.clone(),
                cutter: chosen.id.clone(),
                name: name.clone(),
                connection,
                view: Some(chosen.view.clone()),
                boot: state.boot.clone(),
                sent: None,
                turn: None,
                running: None,
                pending: None,
                fence: 0,
                in_line: false,
            });
        }
        let online = chosen.view.online;
        self.update_cutting(|c| {
            c.device = Some(device.clone());
            c.scenario = None;
            c.firmware = None;
            c.state = CutState::Idle;
            c.job = None;
            c.progress = None;
            c.test_areas_mm.clear();
            c.message = Some(if online {
                format!("Connected to {name} on cameo-pi.")
            } else {
                format!("Connected to cameo-pi, but {name} is offline there.")
            });
        })?;
        let controller = self.clone();
        std::thread::spawn(move || controller.poll_remote(connection));
        self.sync_catalog();
        Ok(ActionOutput::json(json!({
            "device": device,
            "station": station.credentials().address,
            "cutter": chosen.id,
            "online": online,
            "practice": chosen.simulated,
            "simulated": chosen.simulated,
            "remote": true,
        })))
    }

    /// Follows the Pi until the session has another connection.
    fn poll_remote(&self, connection: u64) {
        loop {
            std::thread::sleep(POLL);
            let (station, fence, stop_again) = match self.session() {
                Ok(session) => match &session.remote {
                    Some(r) if r.connection == connection => (
                        r.station.clone(),
                        r.fence,
                        r.running
                            .as_ref()
                            .and_then(|r| r.stopping)
                            .is_some_and(|t| t.elapsed() >= STOP_AGAIN),
                    ),
                    _ => return,
                },
                Err(_) => return,
            };
            if stop_again {
                self.send_stop();
            }
            match station.state() {
                Ok(state) => self.remote_seen(connection, fence, state),
                // The Pi carries on by itself; the next look catches up.
                Err(_) => continue,
            }
        }
    }

    /// One look at the Pi, read when the session's fence was `fence`: progress and the end of a
    /// running cut, or the turn's place in line. A look that began before the last Start (or
    /// its result) is ignored: it can't say anything about that cut.
    fn remote_seen(&self, connection: u64, fence: u64, state: cutcraft_station::State) {
        let (station, ended, waiting, name, turn_view, unseen) = {
            let Ok(mut session) = self.session() else {
                return;
            };
            let Some(remote) = session
                .remote
                .as_mut()
                .filter(|r| r.connection == connection && r.fence == fence)
            else {
                return;
            };
            let Some(view) = state.cutters.into_iter().find(|c| c.id == remote.cutter) else {
                return;
            };
            remote.boot = state.boot;
            let view = view.view;
            remote.view = Some(view.clone());
            let ended = match remote.running.as_mut() {
                Some(running) if view.execution.as_deref() == Some(running.execution.as_str()) => {
                    running.gone_since = None;
                    let progress = view.progress.clone();
                    let state = running_state(&view);
                    drop(session);
                    let _ = self.update_cutting(|c| {
                        // Stop shows at once, before the Pi reaches it.
                        if c.state != CutState::Stopping || state == CutState::Stopping {
                            c.state = state;
                            c.message = state_message(state, c.job.as_ref());
                        }
                        c.progress = progress;
                    });
                    return;
                }
                // The view no longer shows it: it ended, but only the Pi's record says how. Until
                // that's there, the cut is kept (and Stop stays available).
                Some(running) => {
                    let since = *running.gone_since.get_or_insert_with(Instant::now);
                    Some((
                        running.execution.clone(),
                        running.job_id.clone(),
                        running.job.clone(),
                        since,
                    ))
                }
                None => None,
            };
            // Where the turn is now: current, in line (its place), or gone.
            let place = remote.turn.as_ref().map(|turn| {
                if view.turn.as_ref().is_some_and(|t| &t.id == turn) {
                    Some(0)
                } else {
                    view.queue
                        .iter()
                        .position(|q| &q.turn == turn)
                        .map(|i| i + 1)
                }
            });
            if place == Some(None) {
                remote.turn = None;
            }
            let waiting = match place.flatten() {
                Some(0) if remote.in_line => {
                    remote.in_line = false;
                    Some(0)
                }
                Some(n) if n > 0 => Some(n),
                _ => None,
            };
            let turn_view = view
                .turn
                .clone()
                .filter(|t| remote.turn.as_deref() == Some(t.id.as_str()));
            let unseen = view.mat == Mat::Unseen && view.mat_loaded && view.activity == "idle";
            (
                remote.station.clone(),
                ended,
                waiting,
                remote.name.clone(),
                turn_view,
                unseen,
            )
        };
        if let Some((execution, job_id, job, since)) = ended {
            match recorded_outcome(&station, &job_id, &execution) {
                Some(outcome) => {
                    let finished = self.session().ok().is_some_and(|mut s| {
                        let Some(remote) = s.remote.as_mut().filter(|r| r.fence == fence) else {
                            return false;
                        };
                        remote.running = None;
                        remote.fence += 1;
                        true
                    });
                    if finished {
                        self.finish_cut(&job, outcome, None);
                    }
                }
                None if since.elapsed() > Duration::from_secs(30) => {
                    let _ = self.update_cutting(|c| {
                        c.message = Some(format!(
                            "cameo-pi hasn't recorded how the cut on {name} ended yet. Look at the cutter; Stop still works."
                        ));
                    });
                }
                None => {}
            }
            return;
        }
        let _ = self.update_cutting(|c| {
            c.mat_unconfirmed = unseen;
            // The Pi keeps the turn's test cuts; the next one is placed clear of them.
            if let Some(turn) = &turn_view {
                c.test_areas_mm.clone_from(&turn.taken);
            }
            if c.state != CutState::AwaitingSetup {
                return;
            }
            match waiting {
                Some(0) => {
                    c.message = Some(format!(
                        "It's your turn on {name}. Load a fresh mat, then press Start."
                    ))
                }
                Some(n) => {
                    c.message = Some(format!(
                        "In line: {} for {name}. CutCraft says when it's your turn.",
                        ordinal(n + 1)
                    ))
                }
                None => {}
            }
        });
    }

    /// Start, on a cameo-pi: send the job, ask for a turn, check for a fresh mat, then start.
    /// Each step that can't go ahead leaves the cut waiting for Start with the reason. Stop at any
    /// point cancels it: nothing more is sent, and a Start that got through is stopped.
    pub(crate) fn remote_start(
        &self,
        job: CutJob,
        design: &OpenDesign,
    ) -> Result<ActionOutput, CommandError> {
        let (station, cutter, name) = self.remote_parts()?;
        let op = self.begin_remote_start(&job)?;
        let cancelled = || -> Result<ActionOutput, CommandError> {
            self.end_remote_start(op);
            Ok(ActionOutput::json(
                json!({"started": false, "cancelled": true}),
            ))
        };
        self.sync_catalog();
        let (sent, turn, sheet_plan) = {
            let session = self.session()?;
            let remote = session.remote.as_ref();
            (
                remote.and_then(|r| r.sent.clone()),
                remote.and_then(|r| r.turn.clone()),
                session.sheet_plan.clone(),
            )
        };
        let Some(sheet_plan) = sheet_plan else {
            self.end_remote_start(op);
            return Err(disabled("Nothing is ready to cut."));
        };
        // Leaves the cut waiting for Start, unless Stop already ended it.
        let wait = |message: String| -> Result<ActionOutput, CommandError> {
            if self.end_remote_start(op) != Some(true) {
                return cancelled();
            }
            self.update_cutting(|c| {
                c.state = CutState::AwaitingSetup;
                c.progress = None;
                c.message = Some(message.clone());
            })?;
            Ok(ActionOutput::json(
                json!({"started": false, "waiting": message}),
            ))
        };
        // A definite refusal: the Pi didn't start anything.
        let refused = |error: StationError| -> CommandError {
            let message = error.to_string();
            if self.end_remote_start(op) == Some(true) {
                let _ = self.update_cutting(|c| {
                    c.state = CutState::AwaitingSetup;
                    c.progress = None;
                    c.message = Some(format!("Nothing was cut: {message}"));
                });
            } else {
                // Stop was pressed while the Start was out, and the Pi refused that Start.
                let _ = self.update_cutting(|c| {
                    if c.state == CutState::Stopping {
                        c.state = CutState::Idle;
                        c.job = None;
                        c.progress = None;
                        c.message = Some("Stopped; cameo-pi didn't start the cut.".into());
                    }
                });
            }
            fail(message)
        };
        self.update_cutting(|c| {
            c.state = CutState::Ready;
            c.progress = None;
            c.message = Some(format!("Sending to {name}…"));
        })?;

        // 1. The job, unless this plan of this design was sent already.
        let design_key = design.project_path.clone().unwrap_or(design.path.clone());
        let sent = match sent {
            Some(s) if s.design == design_key && s.plan_id == sheet_plan.plan_id => s,
            previous => {
                let earlier = previous
                    .filter(|s| s.design == design_key)
                    .map(|s| s.job_id)
                    .or_else(|| self.origin(&design_key, &station.credentials().address));
                let sent = self
                    .send_job(
                        &station,
                        design,
                        &sheet_plan,
                        &[FOLDER.to_owned()],
                        None,
                        earlier,
                    )
                    .map_err(refused)?;
                let sent = Sent {
                    design: design_key,
                    job_id: sent.id,
                    revision: sent.current_revision,
                    plan_id: sheet_plan.plan_id.clone(),
                };
                self.with_op(op, |remote| remote.sent = Some(sent.clone()));
                sent
            }
        };
        if self.remote_cancelled(op) {
            return cancelled();
        }
        self.with_op(op, |remote| {
            if let Some(pending) = remote.pending.as_mut() {
                pending.job_id = Some(sent.job_id.clone());
            }
        });

        // 2. A turn on the cutter.
        let view = station
            .state()
            .map_err(refused)?
            .cutters
            .into_iter()
            .find(|c| c.id == cutter)
            .map(|c| c.view)
            .ok_or_else(|| refused(StationError::Protocol("the cutter left cameo-pi".into())))?;
        if self.remote_cancelled(op) {
            return cancelled();
        }
        let held = turn.filter(|t| {
            view.turn.as_ref().is_some_and(|v| &v.id == t)
                || view.queue.iter().any(|q| &q.turn == t)
        });
        let (turn, place) = match held {
            Some(turn) => {
                let place = if view.turn.as_ref().is_some_and(|v| v.id == turn) {
                    0
                } else {
                    view.queue
                        .iter()
                        .position(|q| q.turn == turn)
                        .map_or(0, |i| i + 1)
                };
                (turn, place)
            }
            None => match station
                .queue(
                    &cutter,
                    &sent.job_id,
                    sent.revision,
                    &cutcraft_station::new_key(),
                )
                .map_err(refused)?
            {
                Answer::Queued { turn, position } => (turn, position),
                other => {
                    return Err(refused(StationError::Protocol(format!(
                        "queueing answered {other:?}"
                    ))));
                }
            },
        };
        self.with_op(op, |remote| {
            remote.turn = Some(turn.clone());
            remote.in_line = place > 0;
        });
        if self.remote_cancelled(op) {
            // Stop came while the turn was being asked for: ending the operation hands it back.
            return cancelled();
        }
        if place > 0 {
            return wait(format!(
                "In line: {} for {name}. CutCraft says when it's your turn.",
                ordinal(place + 1)
            ));
        }

        // 3. A fresh mat (a fresh view: the turn may have just begun).
        let state = station.state().map_err(refused)?;
        let Some(view) = state
            .cutters
            .into_iter()
            .find(|c| c.id == cutter)
            .map(|c| c.view)
        else {
            return Err(refused(StationError::Protocol(
                "the cutter left cameo-pi".into(),
            )));
        };
        if self.remote_cancelled(op) {
            return cancelled();
        }
        if !view.online {
            return wait(format!(
                "{name} is offline on cameo-pi. Check it's switched on and plugged in, then press Start."
            ));
        }
        let Mat::Fresh(generation) = view.mat else {
            return wait(if view.mat == Mat::Unseen && view.mat_loaded {
                format!(
                    "The mat loaded on {name} may contain previously cut material: unload it and load it again with fresh material, or confirm it has fresh material."
                )
            } else {
                format!("Load a fresh mat on {name}, then press Start.")
            });
        };

        // 4. Start, naming everything shown. Just after a load or a test cut the Pi may still be
        // settling its view of the mat: that's waited out, on the same mat only. Each Start is
        // sent only if Stop hasn't been pressed, and an unanswered one is never taken as "not
        // started".
        let kind = match job.kind {
            JobKind::Sheet => Kind::Main,
            JobKind::TestCut => Kind::Test,
        };
        let (mut boot, mut epoch) = (state.boot.clone(), view.action_epoch);
        for _ in 0..15 {
            let start = Start {
                boot: boot.clone(),
                kind,
                turn: Some(turn.clone()),
                job_id: sent.job_id.clone(),
                revision: sent.revision,
                generation,
                action_epoch: epoch,
                key: cutcraft_station::new_key(),
            };
            if !self.may_send_start(op, &start) {
                return cancelled();
            }
            let answer = match station.start(&cutter, &start) {
                // Asked again with the same key: the Pi answers a repeat with its first answer.
                Err(error) if ambiguous(&error) => station.start(&cutter, &start),
                other => other,
            };
            match answer {
                Ok(Answer::Started { execution, .. }) => {
                    return self.remote_started(op, execution, &job, &sent.job_id);
                }
                Ok(other) => {
                    return Err(refused(StationError::Protocol(format!(
                        "starting answered {other:?}"
                    ))));
                }
                Err(error) if ambiguous(&error) => {
                    self.update_cutting(|c| {
                        c.state = CutState::Sending;
                        c.progress = None;
                        c.message = Some(format!(
                            "cameo-pi hasn't answered the Start on {name} yet, so CutCraft is asking again. Stop still works."
                        ));
                    })?;
                    let controller = self.clone();
                    std::thread::spawn(move || controller.reconcile_remote_start(op));
                    return Ok(ActionOutput::json(
                        json!({"started": null, "unknown": true, "job": job}),
                    ));
                }
                Err(StationError::Refused { code, .. }) if code == "settling" => {
                    self.start_not_taken(op);
                    std::thread::sleep(Duration::from_secs(1));
                    let state = station.state().map_err(refused)?;
                    let Some(view) = state.cutters.into_iter().find(|c| c.id == cutter) else {
                        return Err(refused(StationError::Protocol(
                            "the cutter left cameo-pi".into(),
                        )));
                    };
                    if self.remote_cancelled(op) {
                        return cancelled();
                    }
                    if view.view.mat != Mat::Fresh(generation) {
                        return wait(format!("Load a fresh mat on {name}, then press Start."));
                    }
                    (boot, epoch) = (state.boot, view.view.action_epoch);
                }
                Err(error) => return Err(refused(error)),
            }
        }
        Err(refused(StationError::Refused {
            status: 409,
            code: "settling".into(),
            message: "The cutter is still checking the mat; press Start again in a moment.".into(),
        }))
    }

    /// A new Start operation. Only one at a time: refused while another is being prepared,
    /// cancelled, sent or run.
    fn begin_remote_start(&self, job: &CutJob) -> Result<u64, CommandError> {
        let mut session = self.session()?;
        let remote = session
            .remote
            .as_mut()
            .ok_or_else(|| disabled("No cameo-pi cutter is connected."))?;
        if remote
            .pending
            .as_ref()
            .is_some_and(|p| p.cancelled && p.sent.is_none())
        {
            return Err(disabled(
                "The last Start is still being cancelled. Press Start again in a moment.",
            ));
        }
        if remote.pending.is_some() || remote.running.is_some() {
            return Err(disabled(
                "A cut on this cutter is still starting or running. Wait, or press Stop.",
            ));
        }
        let op = OPERATIONS.fetch_add(1, Ordering::SeqCst) + 1;
        remote.pending = Some(Pending {
            op,
            job: job.clone(),
            job_id: None,
            cancelled: false,
            sent: None,
            stop_answered: false,
        });
        Ok(op)
    }

    /// Whether Stop cancelled this operation (or another one replaced it).
    fn remote_cancelled(&self, op: u64) -> bool {
        self.session().map_or(true, |s| {
            !s.remote
                .as_ref()
                .and_then(|r| r.pending.as_ref())
                .is_some_and(|p| p.op == op && !p.cancelled)
        })
    }

    /// Changes the session for `op` only while it's still the operation there, so a thread
    /// that was cancelled or outlived its connection can't touch a later one.
    fn with_op<R>(&self, op: u64, change: impl FnOnce(&mut Remote) -> R) -> Option<R> {
        let mut session = self.session().ok()?;
        let remote = session
            .remote
            .as_mut()
            .filter(|r| r.pending.as_ref().is_some_and(|p| p.op == op))?;
        Some(change(remote))
    }

    /// Ends an operation that started nothing: `Some(true)` if it was still going, `Some(false)`
    /// if Stop had cancelled it (the caller leaves the cut screen to what Stop showed, and the
    /// turn is handed back), `None` if it wasn't the session's operation any more.
    fn end_remote_start(&self, op: u64) -> Option<bool> {
        let (live, turn) = self.with_op(op, |remote| {
            let live = remote.pending.take().is_some_and(|p| !p.cancelled);
            remote.fence += 1;
            (live, if live { None } else { take_turn(remote) })
        })?;
        if let Some(turn) = turn {
            turn.hand_back();
        }
        Some(live)
    }

    /// The last check before a Start goes out, under the session lock: if Stop hasn't been
    /// pressed, the Start is recorded as possibly reaching the Pi from here on.
    fn may_send_start(&self, op: u64, start: &Start) -> bool {
        let Ok(mut session) = self.session() else {
            return false;
        };
        let Some(remote) = session.remote.as_mut() else {
            return false;
        };
        match remote.pending.as_mut() {
            Some(p) if p.op == op && !p.cancelled => {
                p.sent = Some(start.clone());
                remote.fence += 1;
                true
            }
            _ => false,
        }
    }

    /// The Pi definitely refused the Start just sent (it was settling): nothing of it is pending.
    fn start_not_taken(&self, op: u64) {
        if let Ok(mut session) = self.session()
            && let Some(p) = session
                .remote
                .as_mut()
                .and_then(|r| r.pending.as_mut())
                .filter(|p| p.op == op)
        {
            p.sent = None;
        }
    }

    /// The Pi started it: from now on the cut is followed by its execution. If Stop was pressed
    /// meanwhile, the execution is stopped, and the Stop is sent again until the Pi answers it.
    fn remote_started(
        &self,
        op: u64,
        execution: String,
        job: &CutJob,
        job_id: &str,
    ) -> Result<ActionOutput, CommandError> {
        let stop = {
            let mut session = self.session()?;
            let Some(remote) = session
                .remote
                .as_mut()
                .filter(|r| r.pending.as_ref().is_some_and(|p| p.op == op))
            else {
                // Only another connection replaces the session while a Start is out, and
                // connecting waits for it; this is never reached.
                return Err(fail(
                    "cameo-pi started the cut, but CutCraft is connected to another cutter now.",
                ));
            };
            let stop = remote.pending.take().is_some_and(|p| p.cancelled);
            remote.running = Some(Running {
                execution: execution.clone(),
                job: job.clone(),
                job_id: job_id.to_owned(),
                gone_since: None,
                stopping: stop.then(Instant::now),
            });
            remote.fence += 1;
            stop
        };
        if stop {
            self.update_cutting(|c| {
                c.state = CutState::Stopping;
                c.message = state_message(CutState::Stopping, Some(job));
            })?;
            self.send_stop();
        } else {
            self.update_cutting(|c| {
                c.state = CutState::Sending;
                c.message = state_message(CutState::Sending, Some(job));
                c.progress = None;
            })?;
        }
        Ok(ActionOutput::json(
            json!({"started": true, "job": job, "execution": execution, "stopping": stop}),
        ))
    }

    /// A Start whose answer never came: keep asking until the Pi says definitely, or Stop.
    /// Before Stop, the same Start is resent (the Pi answers a repeat with its original result).
    /// After Stop, nothing is resent: the Pi is asked whether it took it, and the Stop is repeated
    /// until the Pi answers it, after which a late Start is stale there.
    fn reconcile_remote_start(&self, op: u64) {
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let Ok(session) = self.session() else { return };
            let Some(remote) = session.remote.as_ref() else {
                return;
            };
            let Some(pending) = remote.pending.as_ref().filter(|p| p.op == op) else {
                return;
            };
            let Some(start) = pending.sent.clone() else {
                return;
            };
            let (station, cutter, name) = (
                remote.station.clone(),
                remote.cutter.clone(),
                remote.name.clone(),
            );
            let (cancelled, stop_answered, job) = (
                pending.cancelled,
                pending.stop_answered,
                pending.job.clone(),
            );
            drop(session);
            if !cancelled {
                match station.start(&cutter, &start) {
                    Ok(Answer::Started { execution, .. }) => {
                        let _ = self.remote_started(op, execution, &job, &start.job_id);
                        return;
                    }
                    Err(error) if ambiguous(&error) => continue,
                    // Refused now: it may still have been taken the first time.
                    other => match station.started(&cutter, &start.key) {
                        Ok(Some(Answer::Started { execution, .. })) => {
                            let _ = self.remote_started(op, execution, &job, &start.job_id);
                            return;
                        }
                        Ok(_) => {
                            let why = match other {
                                Err(error) => error.to_string(),
                                Ok(answer) => format!("starting answered {answer:?}"),
                            };
                            if self.end_remote_start(op) == Some(true) {
                                let _ = self.update_cutting(|c| {
                                    c.state = CutState::AwaitingSetup;
                                    c.progress = None;
                                    c.message = Some(format!("Nothing was cut: {why}"));
                                });
                            }
                            return;
                        }
                        Err(_) => continue,
                    },
                }
            }
            // Stop was pressed.
            if !stop_answered {
                if stop_reached_the_cutter(&station.stop(&cutter)) {
                    self.stop_answered(op);
                }
                continue;
            }
            match station.started(&cutter, &start.key) {
                Ok(Some(Answer::Started { execution, .. })) => {
                    let _ = self.remote_started(op, execution, &job, &start.job_id);
                    return;
                }
                Ok(_) => {
                    if self.end_remote_start(op).is_some() {
                        let _ = self.update_cutting(|c| {
                            c.state = CutState::Idle;
                            c.job = None;
                            c.progress = None;
                            c.message = Some(format!(
                                "Stopped. cameo-pi never started the cut on {name}, and after the Stop it won't."
                            ));
                        });
                    }
                    return;
                }
                Err(_) => continue,
            }
        }
    }

    fn stop_answered(&self, op: u64) {
        if let Ok(mut session) = self.session()
            && let Some(p) = session
                .remote
                .as_mut()
                .and_then(|r| r.pending.as_mut())
                .filter(|p| p.op == op)
        {
            p.stop_answered = true;
        }
    }

    /// Sends Stop for the running cut while the Pi hasn't answered one. Returns whether the Pi
    /// has answered it; until then the poller sends it again, and the cut screen says so.
    fn send_stop(&self) -> bool {
        let (station, cutter, name, execution) = {
            let Ok(mut session) = self.session() else {
                return false;
            };
            let Some(remote) = session.remote.as_mut() else {
                return false;
            };
            let Some(running) = remote.running.as_mut().filter(|r| r.stopping.is_some()) else {
                return true;
            };
            running.stopping = Some(Instant::now());
            (
                remote.station.clone(),
                remote.cutter.clone(),
                remote.name.clone(),
                running.execution.clone(),
            )
        };
        let answer = station.stop(&cutter);
        if stop_reached_the_cutter(&answer) {
            if let Ok(mut session) = self.session()
                && let Some(running) = session
                    .remote
                    .as_mut()
                    .and_then(|r| r.running.as_mut())
                    .filter(|r| r.execution == execution)
            {
                running.stopping = None;
            }
            return true;
        }
        let why = answer.err().map_or_else(String::new, |e| format!(" ({e})"));
        let _ = self.update_cutting(|c| {
            if c.state == CutState::Stopping {
                c.message = Some(format!(
                    "Stopping: cameo-pi hasn't answered the Stop on {name} yet{why}, so CutCraft keeps sending it."
                ));
            }
        });
        false
    }

    /// Connecting elsewhere or disconnecting waits while a Start is out or a cut runs here.
    pub(crate) fn check_remote_settled(&self) -> Result<(), CommandError> {
        let session = self.session()?;
        if session
            .remote
            .as_ref()
            .is_some_and(|r| r.pending.is_some() || r.running.is_some())
        {
            return Err(CommandError::new(
                "busy",
                "A cut on the cameo-pi cutter is still starting, stopping or running. Wait, or press Stop.",
            ));
        }
        Ok(())
    }

    /// Stop on a cameo-pi cutter, whatever stage the cut is at (spec §5.5: Stop answers at once).
    pub(crate) fn remote_stop(&self, state: CutState) -> Result<ActionOutput, CommandError> {
        enum Stage {
            Running,
            /// A Start may be on its way: stop on the Pi too, and find out.
            MaybeSent(u64),
            /// Still preparing: nothing reached the cutter, and now nothing will.
            Preparing,
            Nothing,
        }
        let (stage, station, cutter) = {
            let mut session = self.session()?;
            let remote = session
                .remote
                .as_mut()
                .ok_or_else(|| disabled("No cameo-pi cutter is connected."))?;
            let stage = if let Some(running) = remote.running.as_mut() {
                running.stopping = Some(Instant::now());
                Stage::Running
            } else if let Some(p) = remote.pending.as_mut() {
                p.cancelled = true;
                if p.sent.is_some() {
                    Stage::MaybeSent(p.op)
                } else {
                    Stage::Preparing
                }
            } else {
                Stage::Nothing
            };
            (stage, remote.station.clone(), remote.cutter.clone())
        };
        match stage {
            Stage::Running => {
                self.update_cutting(|c| {
                    c.state = CutState::Stopping;
                    c.message = state_message(CutState::Stopping, c.job.as_ref());
                })?;
                let answered = self.send_stop();
                Ok(ActionOutput::json(
                    json!({"stopped": true, "sent": true, "answered": answered}),
                ))
            }
            Stage::MaybeSent(op) => {
                self.update_cutting(|c| {
                    c.state = CutState::Stopping;
                    c.message =
                        Some("Stopping: making sure cameo-pi doesn't start the cut…".into());
                })?;
                if stop_reached_the_cutter(&station.stop(&cutter)) {
                    self.stop_answered(op);
                }
                // Whoever holds the Start (its own thread, or the reconciler) settles it now.
                Ok(ActionOutput::json(
                    json!({"stopped": true, "sent": "unknown"}),
                ))
            }
            Stage::Preparing => {
                self.release_remote();
                self.update_cutting(|c| {
                    c.state = CutState::Idle;
                    c.job = None;
                    c.progress = None;
                    c.message = Some("Cancelled; nothing was cut.".into());
                })?;
                Ok(ActionOutput::json(json!({"stopped": true, "sent": false})))
            }
            Stage::Nothing => match state {
                CutState::AwaitingSetup => {
                    // Out of the line, or the turn handed on.
                    self.release_remote();
                    self.update_cutting(|c| {
                        c.state = CutState::Idle;
                        c.job = None;
                        c.message = Some("Cancelled; nothing was sent.".into());
                    })?;
                    Ok(ActionOutput::json(json!({"stopped": true, "sent": false})))
                }
                state if state.busy() => {
                    self.update_cutting(|c| {
                        c.state = CutState::Stopping;
                        c.message = state_message(CutState::Stopping, c.job.as_ref());
                    })?;
                    self.remote_control("stop")?;
                    Ok(ActionOutput::json(json!({"stopped": true, "sent": true})))
                }
                _ => Err(disabled("Nothing is being cut.")),
            },
        }
    }

    /// Stores the open design's sheet in a Pi's library: a new job, or a new revision of
    /// `earlier` when it still exists.
    pub(crate) fn send_job(
        &self,
        station: &Station,
        design: &OpenDesign,
        plan: &cutcraft_path::CutPlan,
        folder: &[String],
        name: Option<&str>,
        earlier: Option<String>,
    ) -> Result<cutcraft_station::Job, StationError> {
        let file = ProjectFile::new(
            design.project.clone(),
            std::path::Path::new(&design.path),
            &design.bytes,
            crate::APP_BUILD,
        );
        let project =
            serde_json::to_vec(&file).map_err(|e| StationError::Protocol(e.to_string()))?;
        let plan = serde_json::to_vec(plan).map_err(|e| StationError::Protocol(e.to_string()))?;
        let stem = std::path::Path::new(design.project_path.as_deref().unwrap_or(&design.path))
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Design".into());
        let name = name.unwrap_or(&stem);
        // Every folder on the way, if it isn't there yet.
        for depth in 0..folder.len() {
            match station.create_folder(&folder[..depth], &folder[depth]) {
                Ok(()) | Err(StationError::Refused { status: 409, .. }) => {}
                Err(error) => return Err(error),
            }
        }
        let upload = |job_id: Option<&str>| {
            station.upload(cutcraft_station::Upload {
                folder,
                name,
                job_id,
                planner: crate::APP_BUILD,
                project: project.clone(),
                plan: plan.clone(),
                key: &cutcraft_station::new_key(),
            })
        };
        // A new job, unless this design was sent before: in this session (`earlier`), or as the
        // job of that name already in the folder (the Pi itself would add "(2)" to the name).
        let fresh = || {
            let existing = station
                .library(folder)?
                .jobs
                .into_iter()
                .find(|j| j.name == name);
            upload(existing.as_ref().map(|j| j.id.as_str()))
        };
        match earlier {
            // Deleted on the Pi meanwhile: a new job then.
            Some(id) => match upload(Some(&id)) {
                Err(StationError::Refused { status: 404, .. }) => fresh(),
                result => result,
            },
            None => fresh(),
        }
    }

    /// Send to cameo-pi: the prepared sheet into a library folder, without cutting.
    pub(crate) fn cut_send(
        &self,
        design: &OpenDesign,
        station: Option<&str>,
        folder: Option<Vec<String>>,
        name: Option<&str>,
    ) -> Result<ActionOutput, CommandError> {
        let station = match station {
            None => match self.session()?.remote.as_ref() {
                Some(remote) => remote.station.clone(),
                None => self.station(None)?,
            },
            address => self.station(address)?,
        };
        let sheet = design
            .project
            .sheets
            .first()
            .ok_or_else(|| disabled("There is nothing to send."))?;
        let plan = cutcraft_path::plan_sheet(&design.project, &sheet.id).map_err(fail)?;
        if !plan.valid {
            return Err(disabled(
                "The sheet can't be cut yet, so it can't be sent: look at Cut Preview.",
            ));
        }
        self.sync_catalog();
        let folder = folder.unwrap_or_else(|| vec![FOLDER.to_owned()]);
        // Opened from this Pi's library: a new revision of that job, wherever it is.
        let earlier = design
            .project_path
            .as_deref()
            .and_then(|path| self.origin(path, &station.credentials().address));
        let revision_of = earlier.is_some();
        let job = self
            .send_job(&station, design, &plan, &folder, name, earlier)
            .map_err(station_error)?;
        Ok(ActionOutput::json(json!({
            "station": station.credentials().address,
            // A new revision stays where its job is.
            "folder": (!revision_of).then_some(folder),
            "job_id": job.id,
            "name": job.name,
            "revision": job.current_revision,
        })))
    }

    /// Hands the turn back (released if current, taken out of the line otherwise). Best effort:
    /// the Pi also ends turns nobody uses.
    pub(crate) fn release_remote(&self) {
        let Ok(mut session) = self.session() else {
            return;
        };
        let Some(remote) = session.remote.as_mut() else {
            return;
        };
        if remote.running.is_some() || remote.pending.as_ref().is_some_and(|p| p.sent.is_some()) {
            return;
        }
        let Some(turn) = take_turn(remote) else {
            return;
        };
        drop(session);
        turn.hand_back();
    }

    /// "It has fresh material": the Pi's loaded mat, which it didn't see going in, counts as fresh.
    pub(crate) fn remote_confirm_fresh(&self) -> Result<ActionOutput, CommandError> {
        let (station, cutter, name) = self.remote_parts()?;
        let answer = station.confirm_fresh(&cutter).map_err(station_error)?;
        self.update_cutting(|c| {
            c.mat_unconfirmed = false;
            if c.state == CutState::AwaitingSetup {
                c.message = Some(format!(
                    "The mat on {name} counts as fresh now. Press Start."
                ));
            }
        })?;
        Ok(ActionOutput::json(
            json!({ "answer": format!("{answer:?}") }),
        ))
    }

    /// Pause, resume or stop on the Pi.
    pub(crate) fn remote_control(&self, what: &str) -> Result<ActionOutput, CommandError> {
        let (station, cutter, _) = self.remote_parts()?;
        let answer = match what {
            "pause" => station.pause(&cutter),
            "resume" => station.resume(&cutter),
            _ => station.stop(&cutter),
        }
        .map_err(station_error)?;
        Ok(ActionOutput::json(
            json!({ "answer": format!("{answer:?}") }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> CutJob {
        let controller = Controller::headless(crate::commands::Roots::default());
        let material = controller.snapshot().unwrap().materials[0].clone();
        CutJob {
            kind: JobKind::Sheet,
            plan_id: "plan".into(),
            material,
            mirror: false,
            mat: Default::default(),
            instructions: vec![],
            estimate_s: 60.0,
            bounds_mm: Some([5.0, 5.0, 45.0, 45.0]),
            test_at_mm: None,
            test_on_scrap: false,
            origin: crate::Origin::User,
        }
    }

    /// A controller connected to a Pi that can't be reached (nothing listens on port 1), so every
    /// request fails as "unreachable", the way a lost answer does.
    fn connected() -> Controller {
        let controller = Controller::headless(crate::commands::Roots::default());
        let station = Arc::new(
            Station::connect(cutcraft_station::Credentials {
                address: "127.0.0.1:1".into(),
                fingerprint: cutcraft_station::Fingerprint::of(b"test"),
                key: "test".into(),
            })
            .unwrap(),
        );
        controller.session().unwrap().remote = Some(Remote {
            station,
            cutter: "practice".into(),
            name: "Practice".into(),
            connection: 1,
            view: None,
            boot: "boot".into(),
            sent: None,
            turn: Some("turn".into()),
            running: None,
            pending: None,
            fence: 0,
            in_line: false,
        });
        controller
    }

    fn idle_state() -> cutcraft_station::State {
        serde_json::from_value(json!({
            "boot": "boot", "version": "test", "cutters": [{"id": "practice", "view": {
                "name": "Practice", "online": true, "mat": {"state": "fresh", "generation": 1},
                "mat_loaded": true, "activity": "idle", "execution": null, "kind": null,
                "turn": null, "queue": [], "action_epoch": 1, "progress": null }}]
        }))
        .unwrap()
    }

    fn start_body(key: &str) -> Start {
        Start {
            boot: "boot".into(),
            kind: Kind::Main,
            turn: Some("turn".into()),
            job_id: "job".into(),
            revision: 1,
            generation: 1,
            action_epoch: 1,
            key: key.into(),
        }
    }

    fn running(controller: &Controller) {
        let job = job();
        let mut session = controller.session().unwrap();
        let remote = session.remote.as_mut().unwrap();
        remote.running = Some(Running {
            execution: "execution".into(),
            job: job.clone(),
            job_id: "job".into(),
            gone_since: None,
            stopping: None,
        });
        remote.fence += 1;
        drop(session);
        controller
            .update_cutting(|c| {
                c.state = CutState::Sending;
                c.job = Some(job);
            })
            .unwrap();
    }

    /// From Codex's review: a look at the Pi that began before Start, answered after it, says
    /// nothing about the cut that just started.
    #[test]
    fn a_status_read_before_start_doesnt_forget_the_execution() {
        let controller = connected();
        let before = controller.session().unwrap().remote.as_ref().unwrap().fence;
        running(&controller);
        controller.remote_seen(1, before, idle_state());
        assert!(
            controller
                .session()
                .unwrap()
                .remote
                .as_ref()
                .unwrap()
                .running
                .is_some()
        );
        assert!(controller.cutting().unwrap().state.busy());
    }

    /// A cut the Pi's view no longer shows is kept until the Pi records how it ended: no record
    /// (here the Pi can't be asked) is "not known yet", never "ended".
    #[test]
    fn a_cut_without_a_recorded_outcome_is_kept_and_stop_still_works() {
        let controller = connected();
        running(&controller);
        let fence = controller.session().unwrap().remote.as_ref().unwrap().fence;
        controller.remote_seen(1, fence, idle_state());
        assert!(
            controller
                .session()
                .unwrap()
                .remote
                .as_ref()
                .unwrap()
                .running
                .is_some()
        );
        assert!(controller.cutting().unwrap().state.busy());
        // The Stop is sent (unanswered here: unreachable), and kept to be sent again.
        let answer = controller.remote_stop(CutState::Sending).unwrap();
        assert_eq!(answer.value["answered"], json!(false));
        assert_eq!(controller.cutting().unwrap().state, CutState::Stopping);
    }

    /// Stop while the job is still being prepared: nothing reached the cutter, and from then on
    /// the Start can't be sent.
    #[test]
    fn stop_while_preparing_means_no_start_is_ever_sent() {
        let controller = connected();
        let op = controller.begin_remote_start(&job()).unwrap();
        controller
            .update_cutting(|c| c.state = CutState::Ready)
            .unwrap();
        controller.remote_stop(CutState::Ready).unwrap();
        assert_eq!(controller.cutting().unwrap().state, CutState::Idle);
        assert!(controller.remote_cancelled(op));
        assert!(
            !controller.may_send_start(op, &start_body("k")),
            "nothing may be sent after Stop"
        );
    }

    /// Stop once a Start may be on its way: control is kept (the Start is remembered, a second
    /// Start refused) until the Pi says whether it took it.
    #[test]
    fn stop_after_a_start_went_out_keeps_control_until_the_pi_answers() {
        let controller = connected();
        let op = controller.begin_remote_start(&job()).unwrap();
        assert!(controller.may_send_start(op, &start_body("k")));
        controller
            .update_cutting(|c| c.state = CutState::Ready)
            .unwrap();
        controller.remote_stop(CutState::Ready).unwrap();
        assert_eq!(controller.cutting().unwrap().state, CutState::Stopping);
        {
            let session = controller.session().unwrap();
            let pending = session.remote.as_ref().unwrap().pending.as_ref().unwrap();
            assert!(pending.cancelled && pending.sent.is_some());
            assert!(
                !pending.stop_answered,
                "an unreachable Pi didn't answer the Stop"
            );
        }
        assert!(
            controller.begin_remote_start(&job()).is_err(),
            "no second Start meanwhile"
        );
        // The Pi refusing that Start ends it truthfully.
        assert_eq!(controller.end_remote_start(op), Some(false));
    }

    /// From Codex's review of 1.4.1: a second Start while the first is still being prepared
    /// must not replace it.
    #[test]
    fn a_second_start_cannot_replace_one_still_preparing() {
        let controller = connected();
        let first = controller.begin_remote_start(&job()).unwrap();
        assert!(controller.begin_remote_start(&job()).is_err());
        assert!(!controller.remote_cancelled(first));
    }

    /// A cancelled preparation blocks the next Start until its thread is done, and once a newer
    /// operation exists, the old thread can't change anything of it.
    #[test]
    fn a_cancelled_preparation_cant_touch_the_next_start() {
        let controller = connected();
        let old = controller.begin_remote_start(&job()).unwrap();
        controller.remote_stop(CutState::Ready).unwrap();
        assert!(controller.begin_remote_start(&job()).is_err());
        assert!(controller.device_disconnect().is_err());
        // The old thread finds it cancelled and ends.
        assert_eq!(controller.end_remote_start(old), Some(false));
        let new = controller.begin_remote_start(&job()).unwrap();
        // Whatever the old thread does afterwards is ignored.
        assert!(
            controller
                .with_op(old, |r| r.turn = Some("old".into()))
                .is_none()
        );
        assert_eq!(controller.end_remote_start(old), None);
        assert!(
            controller
                .remote_started(old, "execution".into(), &job(), "job")
                .is_err()
        );
        let session = controller.session().unwrap();
        let remote = session.remote.as_ref().unwrap();
        assert!(remote.running.is_none());
        assert_ne!(remote.turn.as_deref(), Some("old"));
        assert!(
            remote
                .pending
                .as_ref()
                .is_some_and(|p| p.op == new && !p.cancelled)
        );
    }

    /// From Codex's review of 1.4.1: Stop pressed while a Start was out, the Stop unanswered,
    /// then the Pi's late answer says it started. The Stop is kept, and sent again until the Pi
    /// answers it, and the cut screen says so.
    #[test]
    fn a_late_start_after_an_unanswered_stop_keeps_stopping() {
        let controller = connected();
        let op = controller.begin_remote_start(&job()).unwrap();
        assert!(controller.may_send_start(op, &start_body("k")));
        // Unreachable: this Stop isn't answered.
        controller.remote_stop(CutState::Ready).unwrap();
        controller
            .remote_started(op, "execution".into(), &job(), "job")
            .unwrap();
        let stopping = || {
            let session = controller.session().unwrap();
            let running = session.remote.as_ref().unwrap().running.as_ref().unwrap();
            running.stopping.is_some()
        };
        assert!(stopping(), "the unanswered Stop must be kept");
        assert!(!controller.send_stop());
        assert!(stopping(), "and kept after another unanswered try");
        let cutting = controller.cutting().unwrap();
        assert_eq!(cutting.state, CutState::Stopping);
        assert!(cutting.message.is_some_and(|m| m.contains("keeps sending")));
    }

    /// Stop on a running cut that the Pi doesn't answer is kept and retried the same way.
    #[test]
    fn an_unanswered_stop_of_a_running_cut_is_kept() {
        let controller = connected();
        running(&controller);
        let answer = controller.remote_stop(CutState::Sending).unwrap();
        assert_eq!(answer.value["answered"], json!(false));
        let session = controller.session().unwrap();
        let running = session.remote.as_ref().unwrap().running.as_ref().unwrap();
        assert!(running.stopping.is_some());
    }

    /// Only a real answer proves the Pi handled a Stop (and made a late Start stale).
    #[test]
    fn only_a_stop_the_pi_answered_counts() {
        let refused = |status: u16, code: &str| {
            Err(StationError::Refused {
                status,
                code: code.into(),
                message: String::new(),
            })
        };
        assert!(stop_reached_the_cutter(&Ok(Answer::Stopping {
            execution: "e".into()
        })));
        assert!(stop_reached_the_cutter(&refused(409, "not-cutting")));
        assert!(!stop_reached_the_cutter(&refused(429, "too-many")));
        assert!(!stop_reached_the_cutter(&refused(500, "storage")));
        assert!(!stop_reached_the_cutter(&refused(404, "not-found")));
        assert!(!stop_reached_the_cutter(&Err(StationError::Unreachable(
            "lost".into()
        ))));
    }

    #[test]
    fn places_in_line_read_naturally() {
        let words: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22].map(ordinal).to_vec();
        assert_eq!(
            words,
            [
                "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd"
            ]
        );
    }
}
