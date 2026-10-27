//! One cutter's state (spec §8.3–§8.5, §9): its connection, mat, activity, turn, queue and
//! action epoch, and every request that changes them.
//!
//! [`Cutter`] is plain data behind the worker's mutex. Each method is one row of the state
//! table, applied while the lock is held, so two requests can never both win. The device side
//! ([`crate::worker`]) takes accepted executions with [`Cutter::take_pending`] and reports back
//! with [`Cutter::progress`] and [`Cutter::finished`].

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use cutcraft_device::driver::{Controls, Outcome, Progress};
use serde::{Deserialize, Serialize};

use crate::mat::{Belief, Mat, Observer, Reading};
use crate::runnable::Runnable;

/// Who asked: the user (when users exist) and the device, browser or program (spec §14.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Actor {
    pub user: Option<String>,
    pub client: String,
}

/// A job revision: what a turn is for. Revisions never change once published (spec §7.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRef {
    pub job_id: String,
    pub revision: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Test,
    Main,
}

/// A request to run a test cut or the main cut (spec §9.2). It names everything the person saw,
/// and is refused if any of it has changed.
#[derive(Clone, Debug)]
pub struct StartRequest {
    /// The server boot the page was made under: a request from before a restart never starts
    /// anything after it, whatever its counters say (spec §9.2).
    pub boot: String,
    pub kind: Kind,
    /// The current turn, or `None` to start a turn on a free cutter.
    pub turn: Option<String>,
    pub job: JobRef,
    /// The mat generation the page showed as fresh.
    pub generation: u64,
    /// The action epoch the page showed.
    pub action_epoch: u64,
    /// What to send: the main cut's plan, or the test cut's.
    pub run: Arc<Runnable>,
    /// Chosen by the client for this one action (spec §9.1).
    pub key: String,
    pub actor: Actor,
}

/// The client-supplied identity of a Start, independent of the pattern prepared for it.
/// The same test request must replay even after its original position is occupied.
#[derive(Clone, Debug)]
pub struct StartAction {
    pub boot: String,
    pub kind: Kind,
    pub turn: Option<String>,
    pub job: JobRef,
    pub generation: u64,
    pub action_epoch: u64,
}

impl StartAction {
    fn fingerprint(&self) -> String {
        format!(
            "start-v2|{}",
            serde_json::to_string(&(
                &self.boot,
                self.kind,
                &self.turn,
                &self.job,
                self.generation,
                self.action_epoch,
            ))
            .expect("Start fields serialize")
        )
    }

    /// Older execution records included a server-derived digest. Match their original
    /// client fields, while leaving the digest itself in the durable execution record.
    fn matches(&self, stored: &str) -> bool {
        if stored == self.fingerprint() {
            return true;
        }
        let Some((action, digest)) = stored.rsplit_once('|') else {
            return false;
        };
        digest.len() == 64
            && digest.bytes().all(|b| b.is_ascii_hexdigit())
            && action == self.legacy_fingerprint()
    }

    fn legacy_fingerprint(&self) -> String {
        format!(
            "start|{}|{:?}|{:?}|{}|{}|{}|{}",
            self.boot,
            self.kind,
            self.turn,
            self.job.job_id,
            self.job.revision,
            self.generation,
            self.action_epoch,
        )
    }
}

impl StartRequest {
    pub fn action(&self) -> StartAction {
        StartAction {
            boot: self.boot.clone(),
            kind: self.kind,
            turn: self.turn.clone(),
            job: self.job.clone(),
            generation: self.generation,
            action_epoch: self.action_epoch,
        }
    }
}

/// A Stop that must still be confirmed, independent of the source project or live session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopIntent {
    #[serde(default)]
    pub turn: Option<String>,
    pub cutter: String,
    pub execution: String,
    pub job: JobRef,
    pub kind: Kind,
}

/// What an accepted request did.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "answer")]
pub enum Answer {
    Started {
        execution: String,
        turn: String,
    },
    Queued {
        turn: String,
        /// 0 when it is the cutter's current turn.
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
    Skipped {
        turn: String,
        requeued_as: String,
    },
    Removed {
        turn: String,
    },
    /// A person confirmed the loaded mat has fresh material.
    MatConfirmed {
        generation: u64,
    },
}

/// Why a request was refused. Nothing changed (spec §8.5, last row).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "refusal")]
pub enum Refusal {
    Offline,
    /// The cutter is running (or confirming) an execution.
    Busy {
        execution: String,
    },
    /// The cutter needs checking (an uncertain stop or an interruption): unload the mat.
    CheckCutter,
    /// Another job's turn is in progress: queue instead.
    TurnInProgress {
        turn: String,
    },
    /// The turn named was skipped, removed or finished.
    StaleTurn,
    /// The job has a different revision now.
    StaleRevision,
    /// The mat was unloaded or replaced since the page was shown.
    StaleMat,
    /// cameo-pi restarted since the page was shown.
    StaleBoot,
    /// The mat is fresh, but the cutter's status hasn't settled since the last cut or load.
    Settling,
    /// Things changed since the page was shown (spec §9.2).
    StaleEpoch {
        current: u64,
    },
    /// The mat wasn't seen loading, or has been used.
    MatNotFresh {
        mat: Mat,
    },
    /// The cut would overlap a test cut already made on this mat.
    AreaTaken,
    /// The same idempotency key was used for a different request.
    KeyReused,
    NotCutting,
    StopFirst,
    RecordingFailed,
    NotFound,
    /// Only a mat that is loaded but wasn't seen loading can be confirmed fresh.
    NothingToConfirm {
        mat: Mat,
    },
}

impl Refusal {
    /// The plain message a person sees.
    pub fn message(&self) -> String {
        match self {
            Self::Offline => {
                "The cutter isn't connected. Check it's switched on and plugged in.".into()
            }
            Self::Busy { .. } => "The cutter is busy with a cut.".into(),
            Self::CheckCutter => {
                "Check the cutter: unload the mat, then load fresh material.".into()
            }
            Self::TurnInProgress { .. } => {
                "Someone else's job is on this cutter. Add yours to the queue.".into()
            }
            Self::StaleTurn => "That job is no longer up on this cutter.".into(),
            Self::StaleRevision => "This job was updated; check it again.".into(),
            Self::StaleMat | Self::MatNotFresh { mat: Mat::Used(_) } => {
                "Unload the mat, put on fresh material, load it, then press Start again.".into()
            }
            Self::MatNotFresh { mat: Mat::None } => "Load a fresh mat, then press Start.".into(),
            Self::MatNotFresh { .. } => {
                "The loaded mat may have cut material on it: unload it and load it again with fresh material, or confirm it's fresh.".into()
            }
            Self::StaleEpoch { .. } => "Things changed; check the cutter again.".into(),
            Self::StaleBoot => {
                "cameo-pi restarted since this page was shown; check the cutter again.".into()
            }
            Self::Settling => {
                "The cutter is still checking the mat; try again in a few seconds.".into()
            }
            Self::AreaTaken => "That would cut over a test cut already on this mat.".into(),
            Self::KeyReused => {
                "This request doesn't match the earlier one with the same key.".into()
            }
            Self::NotCutting => "Nothing is cutting.".into(),
            Self::StopFirst => "Stop the cut first.".into(),
            Self::RecordingFailed => "The cut could not be recorded safely; nothing was sent. Check the server storage.".into(),
            Self::NotFound => "Not found.".into(),
            Self::NothingToConfirm { mat: Mat::None } => "No mat is loaded.".into(),
            Self::NothingToConfirm { mat: Mat::Used(_) } => {
                "This mat was cut on: unload it and load fresh material.".into()
            }
            Self::NothingToConfirm { .. } => "The loaded mat is already fresh.".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Turn {
    pub id: String,
    pub job: JobRef,
    pub actor: Actor,
    /// The mat generation this turn took with its first execution.
    pub mat: Option<u64>,
    /// Areas of test cuts made on its mat, `[x0, y0, x1, y1]` mm.
    pub taken: Vec<[f64; 4]>,
    pub tests: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Queued {
    pub turn: String,
    pub job: JobRef,
    pub actor: Actor,
}

#[derive(Clone, Debug)]
enum Activity {
    Idle,
    /// The driver owns the cutter.
    Running {
        execution: String,
        kind: Kind,
        job: JobRef,
        area: Option<[f64; 4]>,
        controls: Arc<Controls>,
        stopping: bool,
    },
    /// Stopped with `idle: true`: waiting for the mat to settle as loaded before believing it
    /// was an orderly stop (spec §8.2).
    Confirming {
        execution: String,
        kind: Kind,
        job: JobRef,
        area: Option<[f64; 4]>,
    },
    /// "Check the cutter": nothing runs until an unload is seen.
    Fault,
}

/// An accepted execution, for the device side to run.
#[derive(Clone)]
pub struct Execution {
    pub id: String,
    pub kind: Kind,
    pub turn: String,
    pub job: JobRef,
    /// The request's idempotency key and fingerprint, kept with the execution record so a retry
    /// after a restart gets the same answer (spec §9.1).
    pub key: String,
    pub fingerprint: String,
    pub actor: Actor,
    pub run: Arc<Runnable>,
    pub controls: Arc<Controls>,
}

/// How an execution ended, as recorded in the job's history.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "ended")]
pub enum Ended {
    Finished,
    Stopped,
    /// What was cut is uncertain.
    Interrupted {
        reason: String,
    },
    /// Nothing was cut: refused before sending, or no mat.
    NotRun {
        reason: String,
    },
}

/// One finished execution, for history and the execution record.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Finished {
    pub execution: String,
    pub turn: String,
    pub job: JobRef,
    pub kind: Kind,
    pub ended: Ended,
}

/// A serialisable view of the cutter, for the web interface, the protocol and MCP.
#[derive(Clone, Debug, Serialize)]
pub struct CutterView {
    pub name: String,
    pub online: bool,
    pub mat: Mat,
    pub mat_loaded: bool,
    pub activity: &'static str,
    pub execution: Option<String>,
    pub kind: Option<Kind>,
    /// The revision the running execution is cutting. The turn may have moved on to a newer
    /// revision meanwhile ([`Cutter::job_updated`]); the viewer draws this one.
    pub cutting: Option<JobRef>,
    pub turn: Option<Turn>,
    pub queue: Vec<Queued>,
    pub action_epoch: u64,
    pub progress: Option<Progress>,
    /// Who confirmed the current fresh mat by hand (spec §8.2), when the Pi didn't see it load.
    pub mat_confirmed_by: Option<Actor>,
}

/// One cutter. All fields change only through the methods below.
pub struct Cutter {
    /// Stable and unique on this Pi; part of every execution and turn id, so two cutters' records
    /// never collide.
    id: String,
    name: String,
    /// Prefix that makes ids unique across restarts (spec §9.2).
    boot: String,
    next_id: u64,
    online: bool,
    observer: Observer,
    activity: Activity,
    turn: Option<Turn>,
    queue: VecDeque<Queued>,
    action_epoch: u64,
    progress: Option<Progress>,
    pending: Option<Execution>,
    /// The service must sync a staged Start before the device worker may take it.
    pending_ready: bool,
    pending_generation: Option<u64>,
    stop_intent: Option<StopIntent>,
    confirmed_stops: Vec<StopIntent>,
    recovered_stops: Vec<StopIntent>,
    /// Idempotency keys: the request's fingerprint and what it got.
    keys: HashMap<String, (String, Result<Answer, Refusal>)>,
    /// Executions finished since last collected, for history and records.
    finished: Vec<Finished>,
    /// Who confirmed which mat generation fresh by hand, rather than the Pi seeing it load.
    confirmed: Option<(u64, Actor)>,
}

impl Cutter {
    pub fn new(id: impl Into<String>, name: impl Into<String>, boot: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            boot: boot.into(),
            next_id: 0,
            online: false,
            observer: Observer::new(),
            activity: Activity::Idle,
            turn: None,
            queue: VecDeque::new(),
            action_epoch: 1,
            progress: None,
            pending: None,
            pending_ready: false,
            pending_generation: None,
            stop_intent: None,
            confirmed_stops: Vec::new(),
            recovered_stops: Vec::new(),
            keys: HashMap::new(),
            finished: Vec::new(),
            confirmed: None,
        }
    }

    fn new_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}-{}-{}-{}", self.id, self.boot, self.next_id)
    }

    fn advance(&mut self) {
        self.action_epoch += 1;
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn boot(&self) -> &str {
        &self.boot
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn mat(&self) -> Mat {
        self.observer.mat()
    }

    pub fn action_epoch(&self) -> u64 {
        self.action_epoch
    }

    pub fn view(&self) -> CutterView {
        let cutting = match &self.activity {
            Activity::Running { job, .. } | Activity::Confirming { job, .. } => Some(job.clone()),
            Activity::Idle | Activity::Fault => None,
        };
        let (activity, execution, kind) = match &self.activity {
            Activity::Idle => ("idle", None, None),
            Activity::Running {
                execution,
                kind,
                stopping,
                ..
            } => {
                let paused = self
                    .progress
                    .as_ref()
                    .is_some_and(|p| p.phase == cutcraft_device::driver::Phase::Paused);
                let name = match (stopping, paused, kind) {
                    (true, _, _) => "stopping",
                    (false, true, _) => "paused",
                    (false, false, Kind::Test) => "testing",
                    (false, false, Kind::Main) => "cutting",
                };
                (name, Some(execution.clone()), Some(*kind))
            }
            Activity::Confirming {
                execution, kind, ..
            } => ("confirming-stop", Some(execution.clone()), Some(*kind)),
            Activity::Fault => ("check-cutter", None, None),
        };
        CutterView {
            name: self.name.clone(),
            online: self.online,
            mat: self.observer.mat(),
            mat_loaded: self.observer.loaded(),
            activity,
            execution,
            kind,
            cutting,
            turn: self.turn.clone(),
            queue: self.queue.iter().cloned().collect(),
            action_epoch: self.action_epoch,
            progress: self.progress.clone(),
            mat_confirmed_by: match (&self.confirmed, self.observer.mat()) {
                (Some((generation, actor)), Mat::Fresh(current)) if *generation == current => {
                    Some(actor.clone())
                }
                _ => None,
            },
        }
    }

    // --- The device side -------------------------------------------------------------------

    /// The cutter appeared or reappeared: a new connection, nothing seen yet.
    pub fn connected(&mut self) {
        self.online = true;
        self.observer.reconnected();
        if let Some(stop) = &self.stop_intent {
            self.activity = Activity::Confirming {
                execution: stop.execution.clone(),
                kind: stop.kind,
                job: stop.job.clone(),
                area: None,
            };
        }
        self.advance();
    }

    /// The cutter vanished. A running execution ends through [`Self::finished`] (the driver
    /// reports it interrupted); a stop being confirmed can no longer be confirmed.
    pub fn disconnected(&mut self) {
        self.online = false;
        self.observer.reconnected();
        // Accepted but not yet taken by the device side: it never runs.
        if let Some(pending) = self.pending.take() {
            let reason = "the cutter disconnected before the cut started".to_owned();
            self.finished(&pending.id, Outcome::Refused { reason });
        }
        if let Activity::Confirming { .. } = self.activity {
            self.uncertain("the cutter disconnected before the stop was confirmed");
        }
        self.advance();
    }

    /// One status reply while idle.
    pub fn observe(&mut self, at_s: f64, reading: Reading) {
        let before = self.observer.mat();
        let belief = self.observer.observe(at_s, reading);
        let changed = before != self.observer.mat() || belief.is_some();
        // Fresh material seen loading during a turn: the turn continues on it, and the old mat's
        // test squares no longer apply (spec §8.2). Earlier Start authorisations are void, as
        // the epoch advances below.
        if let (Mat::Fresh(generation), Some(turn)) = (self.observer.mat(), self.turn.as_mut())
            && turn.mat.is_some_and(|taken| taken != generation)
        {
            turn.mat = None;
            turn.taken.clear();
            turn.tests = 0;
        }
        match (belief, &self.activity) {
            (Some(Belief::NoMat), Activity::Fault) => {
                if let Some(stop) = self.stop_intent.clone() {
                    self.complete_stop(&stop.execution);
                }
                self.activity = Activity::Idle;
            }
            (Some(Belief::Loaded), Activity::Confirming { .. }) => self.confirm(),
            (Some(Belief::NoMat), Activity::Confirming { .. }) => {
                // Unloaded before the stop was confirmed: uncertain, but nothing is loaded now.
                self.uncertain("the mat was unloaded before the stop was confirmed");
                if let Some(stop) = self.stop_intent.clone() {
                    self.complete_stop(&stop.execution);
                }
                self.activity = Activity::Idle;
            }
            _ => {}
        }
        if changed {
            self.advance();
        }
    }

    /// Polling restarts after the driver had the cutter.
    pub fn resume_polling(&mut self, at_s: f64) {
        self.observer.resume_after(at_s);
    }

    pub fn take_pending(&mut self) -> Option<Execution> {
        if !self.pending_ready {
            return None;
        }
        self.pending.take()
    }

    /// Stage under the cutter lock, then sync the returned record without holding that lock.
    /// The service serializes Start/replay/lookup through its acceptance transaction.
    pub fn stage_start(
        &mut self,
        request: StartRequest,
    ) -> Result<(Answer, Option<Execution>), Refusal> {
        if let Some(answer) = self.replay_start(&request.key, &request.action()) {
            return answer.map(|answer| (answer, None));
        }
        let answer = self.start(request)?;
        let execution = self
            .pending
            .as_ref()
            .expect("a new Start has a pending execution")
            .clone();
        self.pending_ready = false;
        Ok((answer, Some(execution)))
    }

    /// Release only the same staged execution. A disconnect can have cancelled it meanwhile;
    /// Stop's control flag is retained, so committing never revokes a Stop received during sync.
    pub fn commit_start(&mut self, id: &str) -> bool {
        if self.pending.as_ref().is_none_or(|p| p.id != id)
            || !matches!(&self.activity, Activity::Running { execution, .. } if execution == id)
        {
            return false;
        }
        if !self
            .pending_generation
            .is_some_and(|generation| self.observer.mat() == Mat::Fresh(generation))
            || !self.observer.loaded()
        {
            let execution = self.pending.take().expect("checked above");
            execution.controls.stop();
            self.pending_ready = false;
            self.finished(
                id,
                Outcome::Refused {
                    reason: "The mat changed while Start was being recorded.".into(),
                },
            );
            return false;
        }
        self.pending_ready = true;
        true
    }

    /// Failed persistence cannot leave a job available for the worker to send.
    pub fn abort_start(&mut self, id: &str) {
        if self.pending.as_ref().is_some_and(|p| p.id == id) {
            let execution = self.pending.take().expect("checked above");
            execution.controls.stop();
            self.pending_ready = false;
            self.finished(
                id,
                Outcome::Refused {
                    reason: Refusal::RecordingFailed.message(),
                },
            );
            if let Some((_, answer)) = self.keys.get_mut(&execution.key) {
                *answer = Err(Refusal::RecordingFailed);
            }
        }
    }

    /// Progress from the driver; ignored unless it belongs to the current execution.
    pub fn progress(&mut self, execution: &str, progress: &Progress) {
        if let Activity::Running { execution: e, .. } = &self.activity
            && e == execution
        {
            self.progress = Some(progress.clone());
        }
    }

    /// The driver's outcome. A late outcome for an execution that already ended is ignored, so
    /// it can never change the next turn (spec §8.5).
    pub fn finished(&mut self, execution: &str, outcome: Outcome) {
        let Activity::Running {
            execution: current,
            kind,
            job,
            area,
            ..
        } = &self.activity
        else {
            return;
        };
        if current != execution {
            return;
        }
        let (kind, job, area) = (*kind, job.clone(), *area);
        self.progress = None;
        if matches!(
            &outcome,
            Outcome::Completed { .. } | Outcome::Refused { .. } | Outcome::NotLoaded
        ) {
            self.complete_stop(execution);
        }
        match outcome {
            Outcome::Completed { .. } => {
                self.record(execution, kind, &job, Ended::Finished);
                self.activity = Activity::Idle;
                match kind {
                    Kind::Test => self.take_area(area),
                    Kind::Main => self.end_turn(),
                }
            }
            Outcome::Stopped { idle: true, .. } => {
                self.observer.restart_settling();
                self.activity = Activity::Confirming {
                    execution: execution.to_owned(),
                    kind,
                    job,
                    area,
                };
            }
            Outcome::Stopped { idle: false, .. } => {
                self.uncertain("the stop wasn't confirmed by the cutter");
            }
            Outcome::Interrupted { reason, .. } => self.uncertain(&reason),
            Outcome::Refused { reason } => {
                self.record(execution, kind, &job, Ended::NotRun { reason });
                self.activity = Activity::Idle;
            }
            Outcome::NotLoaded => {
                let reason = "the cutter reported no mat".to_owned();
                self.record(execution, kind, &job, Ended::NotRun { reason });
                self.activity = Activity::Idle;
                self.observer.forget();
            }
        }
        self.advance();
    }

    /// An orderly stop is confirmed: the mat settled as loaded again (spec §8.2).
    fn confirm(&mut self) {
        let Activity::Confirming {
            execution,
            kind,
            job,
            area,
        } = self.activity.clone()
        else {
            return;
        };
        let ends_current_turn = self.stop_owns_current_turn();
        self.record(&execution, kind, &job, Ended::Stopped);
        self.complete_stop(&execution);
        self.activity = Activity::Idle;
        match kind {
            Kind::Test => self.take_area(area),
            Kind::Main if ends_current_turn => self.end_turn(),
            Kind::Main => {}
        }
    }

    /// An execution ended without proof of where things are: a fault (spec §8.2).
    fn uncertain(&mut self, reason: &str) {
        let (execution, kind, job) = match &self.activity {
            Activity::Running {
                execution,
                kind,
                job,
                ..
            }
            | Activity::Confirming {
                execution,
                kind,
                job,
                ..
            } => (execution.clone(), *kind, job.clone()),
            _ => return,
        };
        let ended = Ended::Interrupted {
            reason: reason.to_owned(),
        };
        self.record(&execution, kind, &job, ended);
        self.observer.distrust();
        self.activity = Activity::Fault;
        if kind == Kind::Main && self.stop_owns_current_turn() {
            self.end_turn();
        }
    }

    fn take_area(&mut self, area: Option<[f64; 4]>) {
        if let (Some(turn), Some(area)) = (&mut self.turn, area) {
            turn.taken.push(area);
            turn.tests += 1;
        }
    }

    fn record(&mut self, execution: &str, kind: Kind, job: &JobRef, ended: Ended) {
        let turn = match self
            .stop_intent
            .as_ref()
            .filter(|stop| stop.execution == execution)
        {
            Some(stop) => stop.turn.clone().unwrap_or_default(),
            None => self
                .turn
                .as_ref()
                .map(|turn| turn.id.clone())
                .unwrap_or_default(),
        };
        self.finished.push(Finished {
            execution: execution.to_owned(),
            turn,
            job: job.clone(),
            kind,
            ended,
        });
    }

    /// Executions finished since the last call, oldest first.
    pub fn take_finished(&mut self) -> Vec<Finished> {
        std::mem::take(&mut self.finished)
    }

    /// The turn ends: its mat generation is retired, and the next queued job's turn starts.
    fn end_turn(&mut self) {
        if let Some(turn) = self.turn.take()
            && let (Some(taken), Mat::Fresh(current)) = (turn.mat, self.observer.mat())
            && taken == current
        {
            self.observer.retire();
        }
        if let Some(next) = self.queue.pop_front() {
            self.turn = Some(Turn {
                id: next.turn,
                job: next.job,
                actor: next.actor,
                mat: None,
                taken: Vec::new(),
                tests: 0,
            });
        }
        self.advance();
    }

    // --- Requests ----------------------------------------------------------------------------

    /// Applies `f` once per idempotency key: a repeat with the same request gets the same
    /// answer; a different request with the same key is refused (spec §9.1).
    fn once(
        &mut self,
        key: &str,
        fingerprint: String,
        f: impl FnOnce(&mut Self) -> Result<Answer, Refusal>,
    ) -> Result<Answer, Refusal> {
        if let Some((seen, answer)) = self.keys.get(key) {
            return if *seen == fingerprint {
                answer.clone()
            } else {
                Err(Refusal::KeyReused)
            };
        }
        let answer = f(self);
        // Refusals aren't remembered: the same action may be asked for again once things change.
        if answer.is_ok() {
            self.keys
                .insert(key.to_owned(), (fingerprint, answer.clone()));
        }
        answer
    }

    /// Start a test cut or the main cut (spec §8.5 "ready to run", §9.2).
    pub fn start(&mut self, request: StartRequest) -> Result<Answer, Refusal> {
        let key = request.key.clone();
        let action = request.action();
        if let Some(answer) = self.replay_start(&key, &action) {
            return answer;
        }
        let fingerprint = action.fingerprint();
        let stored = fingerprint.clone();
        self.once(&key, stored, |cutter| {
            cutter.start_checked(request, fingerprint)
        })
    }

    /// Checks an accepted request before any mutable preparation or revision checks.
    pub fn replay_start(&self, key: &str, action: &StartAction) -> Option<Result<Answer, Refusal>> {
        self.keys.get(key).map(|(seen, answer)| {
            if action.matches(seen) {
                answer.clone()
            } else {
                Err(Refusal::KeyReused)
            }
        })
    }

    /// Read-only reconciliation: absence is unknown, never proof no request is in flight.
    pub fn start_result(&self, key: &str) -> Option<Answer> {
        self.keys.get(key).and_then(|(_, result)| match result {
            Ok(answer @ Answer::Started { .. }) => Some(answer.clone()),
            _ => None,
        })
    }

    /// Seeds the answer to an idempotency key from a durable record (after a restart).
    pub fn remember(&mut self, key: String, fingerprint: String, answer: Answer) {
        self.keys.insert(key, (fingerprint, Ok(answer)));
    }

    fn start_checked(
        &mut self,
        request: StartRequest,
        fingerprint: String,
    ) -> Result<Answer, Refusal> {
        if request.boot != self.boot {
            return Err(Refusal::StaleBoot);
        }
        if !self.online {
            return Err(Refusal::Offline);
        }
        match &self.activity {
            Activity::Idle => {}
            Activity::Running { execution, .. } | Activity::Confirming { execution, .. } => {
                return Err(Refusal::Busy {
                    execution: execution.clone(),
                });
            }
            Activity::Fault => return Err(Refusal::CheckCutter),
        }
        if request.action_epoch != self.action_epoch {
            return Err(Refusal::StaleEpoch {
                current: self.action_epoch,
            });
        }
        match (&self.turn, &request.turn) {
            (None, None) => {}
            (Some(turn), Some(id)) if turn.id == *id => {
                if turn.job != request.job {
                    return Err(Refusal::StaleRevision);
                }
            }
            (Some(turn), None) => {
                return Err(Refusal::TurnInProgress {
                    turn: turn.id.clone(),
                });
            }
            _ => return Err(Refusal::StaleTurn),
        }
        let mat = self.observer.mat();
        let Mat::Fresh(generation) = mat else {
            return Err(Refusal::MatNotFresh { mat });
        };
        if generation != request.generation {
            return Err(Refusal::StaleMat);
        }
        if !self.observer.loaded() {
            return Err(Refusal::Settling);
        }
        if let Some(turn) = &self.turn
            && turn.mat.is_some_and(|taken| taken != generation)
        {
            return Err(Refusal::StaleMat);
        }
        let area = request.run.bounds_mm;
        if let (Some(turn), Some(area)) = (&self.turn, area)
            && turn.taken.iter().any(|t| overlaps(*t, area))
        {
            return Err(Refusal::AreaTaken);
        }

        if self.turn.is_none() {
            let id = self.new_id("turn");
            self.turn = Some(Turn {
                id,
                job: request.job.clone(),
                actor: request.actor.clone(),
                mat: None,
                taken: Vec::new(),
                tests: 0,
            });
        }
        let execution = self.new_id("exec");
        let controls = Arc::new(Controls::default());
        let turn = self.turn.as_mut().expect("a turn was just ensured");
        turn.mat = Some(generation);
        let turn_id = turn.id.clone();
        self.activity = Activity::Running {
            execution: execution.clone(),
            kind: request.kind,
            job: request.job.clone(),
            area,
            controls: controls.clone(),
            stopping: false,
        };
        self.pending_ready = true;
        self.pending_generation = Some(request.generation);
        self.pending = Some(Execution {
            id: execution.clone(),
            kind: request.kind,
            turn: turn_id.clone(),
            job: request.job,
            key: request.key,
            fingerprint,
            actor: request.actor,
            run: request.run,
            controls,
        });
        self.advance();
        Ok(Answer::Started {
            execution,
            turn: turn_id,
        })
    }

    /// Queue a job (spec §8.4). On a free cutter it becomes the current turn at once.
    pub fn queue(&mut self, job: JobRef, actor: Actor, key: &str) -> Result<Answer, Refusal> {
        let fingerprint = format!("queue|{}|{}", job.job_id, job.revision);
        self.once(key, fingerprint, |cutter| {
            let turn = cutter.new_id("turn");
            if cutter.turn.is_none() {
                cutter.turn = Some(Turn {
                    id: turn.clone(),
                    job,
                    actor,
                    mat: None,
                    taken: Vec::new(),
                    tests: 0,
                });
                cutter.advance();
                return Ok(Answer::Queued { turn, position: 0 });
            }
            cutter.queue.push_back(Queued {
                turn: turn.clone(),
                job,
                actor,
            });
            Ok(Answer::Queued {
                turn,
                position: cutter.queue.len(),
            })
        })
    }

    /// A job got a new revision: its turn and queued entries now cut that one (spec §9.2). Pages
    /// showing the old revision can no longer start anything: their Start names the old revision,
    /// and the epoch advances. A running execution keeps cutting what it started with.
    pub fn job_updated(&mut self, job_id: &str, revision: u32) {
        let mut changed = false;
        let turns = self.turn.iter_mut().map(|t| &mut t.job);
        for job in turns.chain(self.queue.iter_mut().map(|q| &mut q.job)) {
            if job.job_id == job_id && job.revision < revision {
                job.revision = revision;
                changed = true;
            }
        }
        if changed {
            self.advance();
        }
    }

    /// Restore only a Stop's confirmation, never its old execution or runnable.
    pub fn recover_stop(&mut self, stop: StopIntent) {
        if stop.cutter != self.id {
            return;
        }
        self.activity = Activity::Confirming {
            execution: stop.execution.clone(),
            kind: stop.kind,
            job: stop.job.clone(),
            area: None,
        };
        self.recovered_stops.push(stop.clone());
        self.stop_intent = Some(stop);
        self.advance();
    }

    pub fn stop_intent(&self) -> Option<StopIntent> {
        self.stop_intent.clone()
    }

    fn stop_owns_current_turn(&self) -> bool {
        self.stop_intent.as_ref().is_none_or(|stop| {
            stop.turn
                .as_ref()
                .is_some_and(|id| self.turn.as_ref().is_some_and(|turn| &turn.id == id))
        })
    }

    fn complete_stop(&mut self, id: &str) {
        if self
            .stop_intent
            .as_ref()
            .is_some_and(|stop| stop.execution == id)
        {
            let active = self.stop_intent.take().expect("checked above");
            if !self.recovered_stops.contains(&active) {
                self.confirmed_stops.push(active);
            }
            self.confirmed_stops.append(&mut self.recovered_stops);
        }
    }

    pub fn take_confirmed_stops(&mut self) -> Vec<StopIntent> {
        std::mem::take(&mut self.confirmed_stops)
    }

    pub fn retry_stop_confirmation(&mut self, intent: StopIntent) {
        self.confirmed_stops.push(intent);
    }

    /// Stop always wins and needs no turn (spec §8.6). Stop while stopping returns the same stop.
    pub fn stop(&mut self) -> Result<Answer, Refusal> {
        if self.stop_intent.is_none() {
            let current = match &self.activity {
                Activity::Running {
                    execution,
                    kind,
                    job,
                    ..
                }
                | Activity::Confirming {
                    execution,
                    kind,
                    job,
                    ..
                } => Some(StopIntent {
                    turn: self.turn.as_ref().map(|turn| turn.id.clone()),
                    cutter: self.id.clone(),
                    execution: execution.clone(),
                    kind: *kind,
                    job: job.clone(),
                }),
                _ => None,
            };
            self.stop_intent = current;
        }
        match &mut self.activity {
            Activity::Running {
                execution,
                controls,
                stopping,
                ..
            } => {
                let execution = execution.clone();
                if !*stopping {
                    *stopping = true;
                    controls.stop();
                    self.advance();
                }
                Ok(Answer::Stopping { execution })
            }
            Activity::Confirming { execution, .. } => Ok(Answer::Stopping {
                execution: execution.clone(),
            }),
            _ if self.stop_intent.is_some() => Ok(Answer::Stopping {
                execution: self
                    .stop_intent
                    .as_ref()
                    .expect("checked above")
                    .execution
                    .clone(),
            }),
            _ => {
                // A Start already on the wire must not win after an idle Stop.
                self.advance();
                Err(Refusal::NotCutting)
            }
        }
    }

    /// Pause between cuts, as the driver does.
    pub fn pause(&mut self) -> Result<Answer, Refusal> {
        match &self.activity {
            Activity::Running {
                controls,
                stopping: false,
                ..
            } => {
                controls.pause();
                Ok(Answer::Paused)
            }
            _ => Err(Refusal::NotCutting),
        }
    }

    pub fn resume(&mut self) -> Result<Answer, Refusal> {
        match &self.activity {
            Activity::Running {
                controls,
                stopping: false,
                ..
            } => {
                controls.resume();
                Ok(Answer::Resumed)
            }
            _ => Err(Refusal::NotCutting),
        }
    }

    fn turn_idle(&self, turn: &str) -> Result<(), Refusal> {
        match &self.turn {
            Some(t) if t.id == turn => {}
            _ => return Err(Refusal::StaleTurn),
        }
        match self.activity {
            Activity::Idle | Activity::Fault => Ok(()),
            _ => Err(Refusal::StopFirst),
        }
    }

    /// "Done with this cutter" (spec §8.3): the turn ends and its mat is retired.
    /// A person confirms the loaded mat has fresh material, though the Pi didn't see it loading
    /// (after a restart or reconnect, spec §8.2). It then counts as a fresh mat. Never for a mat
    /// the Pi knows was cut on, and not while the cutter is busy.
    pub fn confirm_fresh(&mut self, actor: Actor) -> Result<Answer, Refusal> {
        if !self.online {
            return Err(Refusal::Offline);
        }
        match &self.activity {
            Activity::Running { execution, .. } | Activity::Confirming { execution, .. } => {
                return Err(Refusal::Busy {
                    execution: execution.clone(),
                });
            }
            Activity::Fault => return Err(Refusal::CheckCutter),
            Activity::Idle => {}
        }
        let mat = self.observer.mat();
        if mat == Mat::Unseen && !self.observer.loaded() {
            return Err(Refusal::Settling);
        }
        let generation = self
            .observer
            .confirm_fresh()
            .ok_or(Refusal::NothingToConfirm { mat })?;
        // As for a mat seen loading: the turn's test cuts were on the mat before.
        if let Some(turn) = self.turn.as_mut()
            && turn.mat.is_some_and(|taken| taken != generation)
        {
            turn.mat = None;
            turn.taken.clear();
            turn.tests = 0;
        }
        self.confirmed = Some((generation, actor));
        self.advance();
        Ok(Answer::MatConfirmed { generation })
    }

    pub fn release(&mut self, turn: &str) -> Result<Answer, Refusal> {
        self.turn_idle(turn)?;
        self.end_turn();
        Ok(Answer::Released {
            turn: turn.to_owned(),
        })
    }

    /// Skip the current turn: it ends, and its job goes to the end of the queue.
    pub fn skip(&mut self, turn: &str) -> Result<Answer, Refusal> {
        self.turn_idle(turn)?;
        let current = self.turn.clone().expect("checked above");
        self.end_turn();
        let requeued_as = self.new_id("turn");
        let entry = Queued {
            turn: requeued_as.clone(),
            job: current.job,
            actor: current.actor,
        };
        if self.turn.is_none() {
            self.turn = Some(Turn {
                id: entry.turn,
                job: entry.job,
                actor: entry.actor,
                mat: None,
                taken: Vec::new(),
                tests: 0,
            });
        } else {
            self.queue.push_back(entry);
        }
        self.advance();
        Ok(Answer::Skipped {
            turn: turn.to_owned(),
            requeued_as,
        })
    }

    /// Remove a job from the queue, or end the current turn without requeuing it.
    pub fn remove(&mut self, turn: &str) -> Result<Answer, Refusal> {
        if let Some(index) = self.queue.iter().position(|q| q.turn == turn) {
            self.queue.remove(index);
            return Ok(Answer::Removed {
                turn: turn.to_owned(),
            });
        }
        self.turn_idle(turn)?;
        self.end_turn();
        Ok(Answer::Removed {
            turn: turn.to_owned(),
        })
    }
}

fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runnable::tests::runnable_at;

    fn actor() -> Actor {
        Actor {
            user: None,
            client: "test".into(),
        }
    }

    fn job(revision: u32) -> JobRef {
        JobRef {
            job_id: "job-1".into(),
            revision,
        }
    }

    /// An online cutter with a mat seen loading (generation 1).
    fn ready_cutter() -> Cutter {
        let mut c = Cutter::new("workshop", "Workshop Cameo", "b1");
        c.connected();
        let mut at = 0.0;
        for reading in [Reading::NoMedia, Reading::NoMedia]
            .into_iter()
            .chain(std::iter::repeat_n(Reading::Ready, 8))
        {
            c.observe(at, reading);
            at += 0.5;
        }
        assert_eq!(c.mat(), Mat::Fresh(1));
        c
    }

    fn request(c: &Cutter, kind: Kind, turn: Option<&str>, key: &str) -> StartRequest {
        let at = if kind == Kind::Test { 10.0 } else { 50.0 };
        StartRequest {
            boot: c.boot.clone(),
            kind,
            turn: turn.map(str::to_owned),
            job: job(1),
            generation: 1,
            action_epoch: c.action_epoch(),
            run: runnable_at(at),
            key: key.into(),
            actor: actor(),
        }
    }

    fn started(answer: Result<Answer, Refusal>) -> (String, String) {
        match answer {
            Ok(Answer::Started { execution, turn }) => (execution, turn),
            other => panic!("expected Started, got {other:?}"),
        }
    }

    #[test]
    fn a_test_then_the_main_cut_on_one_mat() {
        let mut c = ready_cutter();
        let (exec, turn) = started(c.start(request(&c, Kind::Test, None, "k1")));
        assert!(c.take_pending().is_some());
        c.finished(&exec, Outcome::Completed { duration_s: 1.0 });
        assert_eq!(c.mat(), Mat::Fresh(1), "a test cut doesn't use up the mat");
        assert_eq!(c.view().turn.unwrap().taken.len(), 1);

        let (exec, _) = started(c.start(request(&c, Kind::Main, Some(&turn), "k2")));
        c.finished(&exec, Outcome::Completed { duration_s: 30.0 });
        assert_eq!(c.mat(), Mat::Used(1));
        assert!(c.view().turn.is_none());
        let history = c.take_finished();
        assert_eq!(history.len(), 2);
        assert!(history.iter().all(|h| h.ended == Ended::Finished));
    }

    #[test]
    fn a_used_or_unseen_mat_refuses_a_start() {
        let mut c = ready_cutter();
        let (exec, _) = started(c.start(request(&c, Kind::Main, None, "k1")));
        c.finished(&exec, Outcome::Completed { duration_s: 1.0 });
        let refusal = c.start(request(&c, Kind::Main, None, "k2")).unwrap_err();
        assert_eq!(refusal, Refusal::MatNotFresh { mat: Mat::Used(1) });

        let mut c = Cutter::new("c", "c", "b");
        c.connected();
        for i in 0..10 {
            c.observe(f64::from(i) * 0.5, Reading::Ready);
        }
        let refusal = c.start(request(&c, Kind::Main, None, "k")).unwrap_err();
        assert_eq!(refusal, Refusal::MatNotFresh { mat: Mat::Unseen });
        assert!(refusal.message().contains("may have cut material on it"));
    }

    #[test]
    fn a_retried_start_returns_the_same_execution_and_never_cuts_twice() {
        let mut c = ready_cutter();
        let first = c.start(request(&c, Kind::Test, None, "k1"));
        let mut again = request(&c, Kind::Test, None, "k1");
        again.action_epoch -= 1; // the retry carries what the page showed before
        assert_eq!(c.start(again), first);
        assert!(c.take_pending().is_some());
        assert!(c.take_pending().is_none(), "one execution only");
    }

    #[test]
    fn a_reused_key_with_a_different_request_is_refused() {
        let mut c = ready_cutter();
        started(c.start(request(&c, Kind::Test, None, "k1")));
        let other = request(&c, Kind::Main, None, "k1");
        assert_eq!(c.start(other), Err(Refusal::KeyReused));
    }

    #[test]
    fn an_idle_stop_revokes_a_delayed_start() {
        let mut c = ready_cutter();
        let delayed = request(&c, Kind::Main, None, "late");
        assert_eq!(c.stop(), Err(Refusal::NotCutting));
        assert!(matches!(c.start(delayed), Err(Refusal::StaleEpoch { .. })));
        assert!(c.take_pending().is_none());
    }

    #[test]
    fn accepted_start_lookup_survives_seeded_records_and_legacy_fingerprints() {
        let mut original = ready_cutter();
        let sent = request(&original, Kind::Test, None, "retry");
        let action = sent.action();
        let legacy = format!("{}|{}", action.legacy_fingerprint(), sent.run.digest);
        assert!(original.start_result("retry").is_none());
        let answer = original.start(sent.clone()).unwrap();
        let pending = original.take_pending().unwrap();
        assert_eq!(original.start_result("retry"), Some(answer.clone()));
        for fingerprint in [pending.fingerprint, legacy] {
            let mut restarted = Cutter::new("practice", "Practice", "different-boot");
            restarted.remember("retry".into(), fingerprint, answer.clone());
            assert_eq!(restarted.start_result("retry"), Some(answer.clone()));
            assert_eq!(restarted.start(sent.clone()), Ok(answer.clone()));
            assert!(
                restarted.take_pending().is_none(),
                "replay must never submit motion"
            );
            let mut different = sent.clone();
            different.generation += 1;
            assert_eq!(restarted.start(different), Err(Refusal::KeyReused));
        }
    }

    #[test]
    fn every_client_start_field_is_part_of_its_replay_identity() {
        let mut c = ready_cutter();
        let sent = request(&c, Kind::Test, None, "retry");
        c.start(sent.clone()).unwrap();
        for field in 0..7 {
            let mut different = sent.clone();
            match field {
                0 => different.boot.push('x'),
                1 => different.kind = Kind::Main,
                2 => different.turn = Some("another-turn".into()),
                3 => different.job.job_id.push('x'),
                4 => different.job.revision += 1,
                5 => different.generation += 1,
                _ => different.action_epoch += 1,
            }
            assert_eq!(c.start(different), Err(Refusal::KeyReused), "field {field}");
        }
    }

    #[test]
    fn two_starts_at_once_only_one_wins() {
        let mut c = ready_cutter();
        let a = request(&c, Kind::Main, None, "a");
        let b = request(&c, Kind::Main, None, "b");
        let (exec, _) = started(c.start(a));
        assert_eq!(c.start(b), Err(Refusal::Busy { execution: exec }));
    }

    #[test]
    fn a_delayed_start_from_before_a_stopped_test_fails() {
        let mut c = ready_cutter();
        let (exec, turn) = started(c.start(request(&c, Kind::Test, None, "k1")));
        // A main Start prepared now, delivered after the test is stopped and confirmed.
        let delayed = request(&c, Kind::Main, Some(&turn), "k2");
        c.stop().unwrap();
        c.finished(
            &exec,
            Outcome::Stopped {
                idle: true,
                passes_sent: 1,
            },
        );
        c.resume_polling(100.0); // as the worker does after every execution
        for i in 1..9 {
            c.observe(100.0 + f64::from(i) * 0.5, Reading::Ready);
        }
        assert_eq!(c.view().activity, "idle", "the orderly stop was confirmed");
        assert_eq!(c.mat(), Mat::Fresh(1));
        assert!(matches!(c.start(delayed), Err(Refusal::StaleEpoch { .. })));
    }

    #[test]
    fn stop_while_stopping_returns_the_same_stop() {
        let mut c = ready_cutter();
        let (exec, _) = started(c.start(request(&c, Kind::Main, None, "k1")));
        let first = c.stop().unwrap();
        let epoch = c.action_epoch();
        assert_eq!(c.stop().unwrap(), first);
        assert_eq!(c.action_epoch(), epoch, "nothing changes the second time");
        assert_eq!(first, Answer::Stopping { execution: exec });
    }

    #[test]
    fn an_unconfirmed_stop_is_a_fault_until_an_unload_is_seen() {
        let mut c = ready_cutter();
        let (exec, turn) = started(c.start(request(&c, Kind::Test, None, "k1")));
        c.stop().unwrap();
        c.finished(
            &exec,
            Outcome::Stopped {
                idle: false,
                passes_sent: 0,
            },
        );
        assert_eq!(c.view().activity, "check-cutter");
        assert_eq!(c.mat(), Mat::Used(1));
        assert_eq!(
            c.start(request(&c, Kind::Test, Some(&turn), "k2")),
            Err(Refusal::CheckCutter)
        );
        c.observe(200.0, Reading::NoMedia);
        c.observe(200.5, Reading::NoMedia);
        assert_eq!(c.view().activity, "idle");
        assert_eq!(
            c.view().turn.unwrap().id,
            turn,
            "a test fault keeps the turn"
        );
    }

    #[test]
    fn a_stop_with_the_mat_gone_is_not_confirmed() {
        let mut c = ready_cutter();
        let (exec, _) = started(c.start(request(&c, Kind::Main, None, "k1")));
        c.stop().unwrap();
        // The driver's idle also covers No media.
        c.finished(
            &exec,
            Outcome::Stopped {
                idle: true,
                passes_sent: 2,
            },
        );
        c.observe(300.0, Reading::NoMedia);
        c.observe(300.5, Reading::NoMedia);
        let history = c.take_finished();
        assert!(matches!(history[0].ended, Ended::Interrupted { .. }));
        assert!(c.view().turn.is_none(), "a main cut's turn ends");
    }

    #[test]
    fn interruption_of_a_main_cut_ends_the_turn_with_a_fault() {
        let mut c = ready_cutter();
        c.queue(job(1), actor(), "q1").unwrap();
        let turn = c.view().turn.unwrap().id;
        let queued = c.queue(job(2), actor(), "q2").unwrap();
        let (exec, _) = started(c.start(request(&c, Kind::Main, Some(&turn), "k1")));
        c.finished(
            &exec,
            Outcome::Interrupted {
                reason: "transfer timed out".into(),
                passes_sent: 3,
            },
        );
        assert_eq!(c.view().activity, "check-cutter");
        let Answer::Queued { turn: next, .. } = queued else {
            panic!()
        };
        assert_eq!(c.view().turn.unwrap().id, next, "the next queued job is up");
    }

    #[test]
    fn a_late_outcome_never_changes_the_next_turn() {
        let mut c = ready_cutter();
        let (exec, _) = started(c.start(request(&c, Kind::Main, None, "k1")));
        c.finished(&exec, Outcome::Completed { duration_s: 1.0 });
        let view = c.view();
        c.finished(&exec, Outcome::Completed { duration_s: 1.0 });
        c.finished("exec-other", Outcome::NotLoaded);
        assert_eq!(c.view().action_epoch, view.action_epoch);
        assert_eq!(c.mat(), Mat::Used(1));
    }

    #[test]
    fn a_free_cutter_queues_as_the_current_turn_and_a_busy_one_queues_behind() {
        let mut c = ready_cutter();
        assert_eq!(
            c.queue(job(1), actor(), "q1").unwrap(),
            Answer::Queued {
                turn: c.view().turn.unwrap().id,
                position: 0
            }
        );
        let Answer::Queued { position, .. } = c.queue(job(2), actor(), "q2").unwrap() else {
            panic!()
        };
        assert_eq!(position, 1);
        assert!(matches!(
            c.start(request(&c, Kind::Main, None, "k")),
            Err(Refusal::TurnInProgress { .. })
        ));
    }

    #[test]
    fn release_retires_the_turns_mat_and_skip_requeues_it() {
        let mut c = ready_cutter();
        let (exec, turn) = started(c.start(request(&c, Kind::Test, None, "k1")));
        c.finished(&exec, Outcome::Completed { duration_s: 1.0 });
        assert_eq!(
            c.release(&turn).unwrap(),
            Answer::Released { turn: turn.clone() }
        );
        assert_eq!(c.mat(), Mat::Used(1), "a released turn's mat is retired");

        // A turn that never took a mat has nothing to retire.
        let mut c = ready_cutter();
        c.queue(job(1), actor(), "q").unwrap();
        let turn = c.view().turn.unwrap().id;
        let Answer::Skipped { requeued_as, .. } = c.skip(&turn).unwrap() else {
            panic!()
        };
        assert_eq!(c.mat(), Mat::Fresh(1));
        assert_eq!(c.view().turn.unwrap().id, requeued_as);
    }

    #[test]
    fn release_while_cutting_is_refused() {
        let mut c = ready_cutter();
        let (_, turn) = started(c.start(request(&c, Kind::Main, None, "k1")));
        assert_eq!(c.release(&turn), Err(Refusal::StopFirst));
        assert_eq!(c.skip(&turn), Err(Refusal::StopFirst));
    }

    #[test]
    fn a_main_cut_over_a_test_square_is_refused() {
        let mut c = ready_cutter();
        let (exec, turn) = started(c.start(request(&c, Kind::Test, None, "k1")));
        c.finished(&exec, Outcome::Completed { duration_s: 1.0 });
        let mut over = request(&c, Kind::Main, Some(&turn), "k2");
        over.run = runnable_at(10.0);
        assert_eq!(c.start(over), Err(Refusal::AreaTaken));
    }

    #[test]
    fn a_stale_revision_or_turn_is_refused() {
        let mut c = ready_cutter();
        c.queue(job(1), actor(), "q").unwrap();
        let turn = c.view().turn.unwrap().id;
        let mut newer = request(&c, Kind::Main, Some(&turn), "k1");
        newer.job = job(2);
        assert_eq!(c.start(newer), Err(Refusal::StaleRevision));
        let other = request(&c, Kind::Main, Some("turn-gone"), "k2");
        assert_eq!(c.start(other), Err(Refusal::StaleTurn));
    }

    #[test]
    fn a_disconnect_makes_the_mat_unseen() {
        let mut c = ready_cutter();
        c.disconnected();
        c.connected();
        assert_eq!(c.mat(), Mat::Unseen);
        assert!(matches!(
            c.start(request(&c, Kind::Main, None, "k")),
            Err(Refusal::MatNotFresh { .. })
        ));
    }

    #[test]
    fn a_person_can_confirm_an_unseen_mat_fresh_but_never_a_used_one() {
        // A mat loaded while cameo-pi restarted: Ready all along, never seen going in.
        let mut c = Cutter::new("workshop", "Workshop Cameo", "b1");
        c.connected();
        assert!(matches!(
            c.confirm_fresh(actor()),
            Err(Refusal::Settling) | Err(Refusal::NothingToConfirm { .. })
        ));
        for i in 0..8 {
            c.observe(f64::from(i) * 0.5, Reading::Ready);
        }
        assert_eq!(c.mat(), Mat::Unseen);
        assert!(matches!(
            c.start(request(&c, Kind::Main, None, "k0")),
            Err(Refusal::MatNotFresh { .. })
        ));
        let epoch = c.action_epoch();
        let Ok(Answer::MatConfirmed { generation }) = c.confirm_fresh(actor()) else {
            panic!("not confirmed");
        };
        assert_eq!(c.mat(), Mat::Fresh(generation));
        assert!(
            c.action_epoch() > epoch,
            "pages showing the old mat can't start"
        );
        assert_eq!(c.view().mat_confirmed_by, Some(actor()));
        let mut start = request(&c, Kind::Main, None, "k1");
        start.generation = generation;
        let (exec, _) = started(c.start(start));
        c.finished(&exec, Outcome::Completed { duration_s: 1.0 });
        assert_eq!(c.mat(), Mat::Used(generation));
        assert_eq!(c.view().mat_confirmed_by, None);
        assert_eq!(
            c.confirm_fresh(actor()),
            Err(Refusal::NothingToConfirm {
                mat: Mat::Used(generation)
            })
        );
    }

    #[test]
    fn not_loaded_runs_nothing_and_distrusts_the_mat() {
        let mut c = ready_cutter();
        let (exec, _) = started(c.start(request(&c, Kind::Test, None, "k1")));
        c.finished(&exec, Outcome::NotLoaded);
        assert_eq!(c.view().activity, "idle");
        assert_eq!(c.mat(), Mat::Unseen);
        assert!(matches!(c.take_finished()[0].ended, Ended::NotRun { .. }));
    }

    #[test]
    fn review_unseen_start_from_previous_boot_must_be_refused() {
        let before = ready_cutter();
        let delayed = request(&before, Kind::Main, None, "never-seen-before-restart");
        let mut after = ready_cutter();
        after.boot = "different-boot".into();
        assert!(
            after.start(delayed).is_err(),
            "old boot request started on a new boot"
        );
    }
    #[test]
    fn review_reload_after_test_must_allow_continuing_the_turn() {
        let mut c = ready_cutter();
        let (exec, turn) = started(c.start(request(&c, Kind::Test, None, "test-first")));
        c.take_pending();
        c.finished(&exec, Outcome::Completed { duration_s: 1.0 });
        c.observe(10.0, Reading::NoMedia);
        c.observe(10.5, Reading::NoMedia);
        for i in 0..8 {
            c.observe(11.0 + f64::from(i) * 0.5, Reading::Ready);
        }
        assert_eq!(c.mat(), Mat::Fresh(2));
        assert_eq!(
            c.view().turn.unwrap().tests,
            0,
            "no test cuts on this new mat"
        );
        let mut next = request(&c, Kind::Main, Some(&turn), "main-after-reload");
        next.generation = 2;
        assert!(
            c.start(next).is_ok(),
            "same turn cannot continue after reloading fresh material"
        );
    }

    #[test]
    fn a_new_revision_moves_turns_and_queue_and_refuses_the_old_one() {
        let mut c = ready_cutter();
        c.queue(job(1), actor(), "q1").unwrap();
        c.queue(job(1), actor(), "q2").unwrap();
        let turn = c.view().turn.unwrap().id;
        let shown = request(&c, Kind::Main, Some(&turn), "old-page");
        let epoch = c.action_epoch();
        c.job_updated("job-1", 2);
        assert!(c.action_epoch() > epoch);
        let view = c.view();
        assert_eq!(view.turn.unwrap().job.revision, 2);
        assert!(view.queue.iter().all(|q| q.job.revision == 2));
        assert!(c.start(shown).is_err(), "the page showed revision 1");
        let mut old = request(&c, Kind::Main, Some(&turn), "old-revision");
        old.job = job(1);
        assert_eq!(c.start(old), Err(Refusal::StaleRevision));
        let mut new = request(&c, Kind::Main, Some(&turn), "new-revision");
        new.job = job(2);
        started(c.start(new));
        // Older revisions and other jobs change nothing.
        let epoch = c.action_epoch();
        c.job_updated("job-1", 1);
        c.job_updated("job-2", 9);
        assert_eq!(c.action_epoch(), epoch);
    }

    #[test]
    fn a_running_cut_keeps_cutting_its_revision_when_the_job_changes() {
        let mut c = ready_cutter();
        let (exec, turn) = started(c.start(request(&c, Kind::Test, None, "t1")));
        c.take_pending();
        c.job_updated("job-1", 2);
        let view = c.view();
        assert_eq!(
            view.cutting,
            Some(job(1)),
            "the viewer draws what is being cut"
        );
        assert_eq!(
            view.turn.unwrap().job.revision,
            2,
            "what comes next is the new one"
        );
        c.finished(&exec, Outcome::Completed { duration_s: 1.0 });
        let history = c.take_finished();
        assert_eq!(
            history[0].job,
            job(1),
            "history records the revision that was cut"
        );
        assert!(c.view().cutting.is_none());
        let mut main = request(&c, Kind::Main, Some(&turn), "main");
        main.job = job(2);
        started(c.start(main));
    }

    #[test]
    fn review_execution_ids_must_differ_between_cutters() {
        let mut a = ready_cutter();
        let mut b = ready_cutter();
        b.id = "other".into();
        let (ea, _) = started(a.start(request(&a, Kind::Main, None, "cutter-a")));
        let (eb, _) = started(b.start(request(&b, Kind::Main, None, "cutter-b")));
        assert_ne!(
            ea, eb,
            "different cutters share a durable execution-record filename"
        );
    }

    #[test]
    fn after_an_interrupted_test_a_reload_lets_the_turn_continue() {
        let mut c = ready_cutter();
        let (exec, turn) = started(c.start(request(&c, Kind::Test, None, "t1")));
        c.take_pending();
        c.finished(
            &exec,
            Outcome::Interrupted {
                reason: "cable".into(),
                passes_sent: 1,
            },
        );
        assert_eq!(c.view().activity, "check-cutter");
        c.observe(20.0, Reading::NoMedia);
        c.observe(20.5, Reading::NoMedia);
        for i in 0..8 {
            c.observe(21.0 + f64::from(i) * 0.5, Reading::Ready);
        }
        assert_eq!(c.mat(), Mat::Fresh(2));
        let view = c.view();
        assert_eq!(view.turn.as_ref().unwrap().id, turn, "the same turn");
        assert!(
            view.turn.unwrap().taken.is_empty(),
            "the old mat's squares are gone"
        );
        let mut next = request(&c, Kind::Test, Some(&turn), "t2");
        next.generation = 2;
        assert!(c.start(next).is_ok());
    }

    #[test]
    fn a_start_prepared_before_a_reload_is_refused_after_it() {
        let mut c = ready_cutter();
        let (exec, turn) = started(c.start(request(&c, Kind::Test, None, "t1")));
        c.take_pending();
        c.finished(&exec, Outcome::Completed { duration_s: 1.0 });
        let before = request(&c, Kind::Main, Some(&turn), "m1");
        c.observe(30.0, Reading::NoMedia);
        c.observe(30.5, Reading::NoMedia);
        for i in 0..8 {
            c.observe(31.0 + f64::from(i) * 0.5, Reading::Ready);
        }
        assert!(c.start(before).is_err(), "made for the old mat");
    }
    #[test]
    fn a_staged_start_cannot_be_taken_before_recording_and_retains_stop() {
        let mut c = ready_cutter();
        let request = request(&c, Kind::Main, None, "staged");
        let (_, execution) = c.stage_start(request).unwrap();
        let execution = execution.unwrap();
        assert!(c.take_pending().is_none());
        c.stop().unwrap();
        assert!(c.commit_start(&execution.id));
        let pending = c.take_pending().unwrap();
        assert!(
            pending.controls.stopping(),
            "persistence completion never revokes Stop"
        );
    }

    #[test]
    fn abort_or_disconnect_cannot_release_a_staged_start() {
        for disconnect in [false, true] {
            let mut c = ready_cutter();
            let request = request(&c, Kind::Main, None, "staged");
            let (_, execution) = c.stage_start(request).unwrap();
            let execution = execution.unwrap();
            if disconnect {
                c.disconnected();
            } else {
                c.abort_start(&execution.id);
            }
            assert!(!c.commit_start(&execution.id));
            assert!(c.take_pending().is_none());
        }
    }
    #[test]
    fn a_mat_change_during_recording_cancels_the_staged_execution() {
        let mut c = ready_cutter();
        let request = request(&c, Kind::Main, None, "staged");
        let (_, execution) = c.stage_start(request).unwrap();
        let execution = execution.unwrap();
        c.observe(20.0, Reading::NoMedia);
        c.observe(20.5, Reading::NoMedia);
        assert!(!c.commit_start(&execution.id));
        assert!(c.take_pending().is_none());
        assert!(c.view().execution.is_none());
    }
    #[test]
    fn recovered_stops_wait_for_quiet_and_confirm_the_whole_startup_batch() {
        let mut c = Cutter::new("workshop", "Workshop Cameo", "after");
        for id in ["first", "second"] {
            c.recover_stop(StopIntent {
                turn: None,
                cutter: "workshop".into(),
                execution: id.into(),
                job: job(1),
                kind: Kind::Main,
            });
        }
        c.connected();
        c.observe(0.0, Reading::Moving);
        c.observe(0.5, Reading::Moving);
        assert_eq!(c.view().activity, "confirming-stop");
        assert!(c.take_pending().is_none());
        assert!(c.take_confirmed_stops().is_empty());
        for i in 0..10 {
            c.observe(1.0 + f64::from(i) * 0.5, Reading::Ready);
        }
        assert_eq!(c.view().activity, "idle");
        assert_eq!(c.take_confirmed_stops().len(), 2);
    }
    #[test]
    fn confirming_a_recovered_stop_never_consumes_the_next_peoples_turn() {
        for unload in [false, true] {
            let mut c = ready_cutter();
            let (_, execution) = c
                .stage_start(request(&c, Kind::Main, None, "first"))
                .unwrap();
            let execution = execution.unwrap();
            assert!(c.commit_start(&execution.id));
            c.take_pending().unwrap();
            let queued = c.queue(job(2), actor(), "next").unwrap();
            let Answer::Queued { turn, .. } = queued else {
                panic!("not queued");
            };
            c.stop().unwrap();
            c.finished(
                &execution.id,
                Outcome::Interrupted {
                    reason: "connection lost".into(),
                    passes_sent: 0,
                },
            );
            assert_eq!(c.view().turn.as_ref().unwrap().id, turn);
            c.disconnected();
            c.connected();
            for i in 0..10 {
                c.observe(
                    20.0 + f64::from(i) * 0.5,
                    if unload {
                        Reading::NoMedia
                    } else {
                        Reading::Ready
                    },
                );
            }
            assert_eq!(
                c.view().turn.as_ref().unwrap().id,
                turn,
                "old Stop cannot release the next turn"
            );
            for finished in c.take_finished() {
                assert_eq!(
                    finished.turn, execution.turn,
                    "old Stop retains the original turn attribution"
                );
            }
        }
    }
}
