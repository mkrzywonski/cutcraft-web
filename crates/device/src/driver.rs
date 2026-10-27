//! The cutting driver (spec §5.5, §7.3): sends an encoded plan through a [`Transport`] and owns
//! the execution states between Ready and Completed. It paces what it queues by estimated
//! motion time, not bytes, so Stop is not stuck behind a full buffer; Pause waits for the next
//! contour boundary and lifts the blade there; Stop sends nothing more, asks for the blade to
//! lift and waits for the cutter to go idle. Completion needs an idle status after the last
//! byte, never write completion alone, and an uncertain stream position ends the sheet as
//! Interrupted, never as a replay.
//!
//! Clean room: written from the public descriptions in spec §7.3. The status bytes and the
//! tool-up `M` used to lift the blade are hypotheses until the hardware tests confirm them.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::gpgl::{self, EncodedPlan};
use crate::transport::{Transport, TransportError};

const ETX: u8 = 0x03;
const ESC: u8 = 0x1b;
/// Estimated motion the driver lets the cutter hold before it waits (spec §7.3: limit queued
/// motion by duration as well as bytes).
pub const LEAD_S: f64 = 1.5;
/// One write may wait at least this long for buffer space before delivery counts as uncertain
/// (longer when one command is estimated to take longer: [`write_timeout`]). A real cut on
/// 2026-10-18 timed out halfway with 10 s: the machine ran behind the estimates until its buffer
/// was full, and a write then waits for the command in progress to finish.
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);
/// A write that waits longer than this found the cutter's buffer full: the machine is behind
/// the estimates, so pacing slows down.
const BLOCKED_S: f64 = 0.25;
/// Pacing never assumes the machine is more than this many times slower than estimated.
const MAX_PACE: f64 = 8.0;
/// A status reply may take this long.
const STATUS_TIMEOUT: Duration = Duration::from_secs(2);
/// Before anything is sent, status must say Ready without a break for this long. After FG an
/// unloaded Cameo answered Ready for about 1 s, then NoMedia from 1.5 s (2026-10-18), so one
/// Ready reply is not evidence that a mat is loaded; this is twice that window.
pub const SETTLE_S: f64 = 3.0;
/// How often status is read while it settles.
const SETTLE_POLL: Duration = Duration::from_millis(500);
/// The longest status may take to settle before the start gives up.
const SETTLE_DEADLINE_S: f64 = 15.0;
/// How often status is read while sending.
const STATUS_EVERY_S: f64 = 1.0;
const POLL: Duration = Duration::from_millis(50);
/// After the last byte, status must say Ready this long before the cut counts as finished. The
/// Cameo answers Ready whenever it is not moving, including the second or so before it starts
/// commands it has just received (timing plot, 2026-10-20), so one Ready reply is not evidence
/// that it is done.
pub const READY_HOLD_S: f64 = 1.5;
/// How often status is read while waiting for that.
const READY_POLL: Duration = Duration::from_millis(250);
/// After idling at least this long, the Cameo starts the next command about this much later
/// (timing plot, 2026-10-20); estimates and pacing allow for it.
pub const START_DELAY_S: f64 = 1.0;

/// What the cutter reports to ESC ENQ (hypothesis, spec §7.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceStatus {
    Ready,
    Moving,
    NoMedia,
}

/// How time passes for the driver: the wall clock, or a simulator's virtual clock.
pub trait Clock {
    fn now_s(&self) -> f64;
    fn sleep(&self, duration: Duration);
}

/// Requests from the UI and automation, read by the driver between commands. Stop wins over
/// Pause and cannot be taken back.
#[derive(Debug, Default)]
pub struct Controls {
    pause: AtomicBool,
    stop: AtomicBool,
}

impl Controls {
    pub fn pause(&self) {
        self.pause.store(true, Ordering::SeqCst);
    }
    pub fn resume(&self) {
        self.pause.store(false, Ordering::SeqCst);
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
    fn pausing(&self) -> bool {
        self.pause.load(Ordering::SeqCst)
    }
}

/// The execution states the driver passes through (spec §5.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Sending,
    Pausing,
    Paused,
    Draining,
    Stopping,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    pub phase: Phase,
    pub sent_bytes: usize,
    pub total_bytes: usize,
    /// Cuts (plan operation passes) fully queued.
    pub passes_sent: usize,
    pub passes_total: usize,
    /// Estimated share done, 0 to 1. Never 1 before the cutter reports idle.
    pub fraction: f64,
    pub remaining_s: f64,
    /// How many times slower than estimated the machine is taken to be when pacing (1 until
    /// a write finds its buffer full).
    pub pace: f64,
    /// Where the blade is estimated to be. The cutter never reports its position: this follows
    /// what was sent, when, and the paced speed.
    pub estimate: Option<Estimate>,
}

/// An estimate of the blade's progress, never a measurement.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    /// Bytes of the stream whose commands are estimated finished.
    pub done_bytes: usize,
    /// The estimated blade position, device units `[x, y]`.
    pub at: [i32; 2],
    /// Whether the blade is estimated down (cutting) there.
    pub down: bool,
    /// What is still queued in the cutter, from `at` to the last point sent (device units): the
    /// real blade is somewhere along it.
    pub ahead: Vec<[i32; 2]>,
}

/// One command as sent, on the estimated timeline.
struct Sent {
    start: f64,
    end: f64,
    byte_end: usize,
    from: [i32; 2],
    points: Vec<[i32; 2]>,
    down: bool,
}

/// The point a fraction `f` of the way along `from`, `points` by length, and how many of
/// `points` it has passed.
fn along(from: [i32; 2], points: &[[i32; 2]], f: f64) -> ([i32; 2], usize) {
    let length = |a: [i32; 2], b: [i32; 2]| f64::from(b[0] - a[0]).hypot(f64::from(b[1] - a[1]));
    let mut total = 0.0;
    let mut previous = from;
    for &p in points {
        total += length(previous, p);
        previous = p;
    }
    let mut left = total * f.clamp(0.0, 1.0);
    let mut previous = from;
    for (i, &p) in points.iter().enumerate() {
        let d = length(previous, p);
        if d > 0.0 && left <= d {
            let t = left / d;
            let x = f64::from(previous[0]) + t * f64::from(p[0] - previous[0]);
            let y = f64::from(previous[1]) + t * f64::from(p[1] - previous[1]);
            return ([x.round() as i32, y.round() as i32], i);
        }
        left -= d;
        previous = p;
    }
    (previous, points.len())
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum Outcome {
    /// Whole-stream validation failed before any transport access.
    Refused { reason: String },
    /// Every byte was accepted and the cutter then reported idle.
    Completed { duration_s: f64 },
    /// Stopped on request. `idle` is true only when the cutter reported idle afterwards; the
    /// blade-up request is a hypothesis, so nothing claims the blade is up.
    Stopped { idle: bool, passes_sent: usize },
    /// The cutter reported nothing loaded before anything was sent.
    NotLoaded,
    /// Communication failed or the material went away: what was cut is uncertain.
    Interrupted { reason: String, passes_sent: usize },
}

/// One plan to cut, with the speeds the estimates use.
pub struct Job<'a> {
    pub plan: &'a EncodedPlan,
    /// Cutting (tool down) speed for the plan's speed setting, in mm/s.
    pub mm_per_s: f64,
    /// Tool-up travel speed, in mm/s; the Cameo's does not follow the speed setting.
    pub travel_mm_s: f64,
    /// Lifting the tool and lowering it again.
    pub tool_change_s: f64,
    /// Fixed time per point moved to, beside the travel itself.
    pub point_s: f64,
    /// Slowing for a change of direction while cutting, per radian.
    pub turn_s_per_rad: f64,
    pub units_per_mm: f64,
}

/// One ETX-framed command of the stream.
struct Command {
    range: [usize; 2],
    /// Estimated execution time.
    duration_s: f64,
    setup_s: f64,
    /// Where the tool is after it, in device units `[x, y]`.
    end: Option<[i32; 2]>,
    /// A safe place to pause before it: between cuts.
    checkpoint: bool,
    /// Passes complete once it is sent.
    passes_after: usize,
    /// The points it moves through, and whether the blade is down (`D`) or up (`M`).
    points: Vec<[i32; 2]>,
    down: bool,
}

/// Experimental setup allowance, in seconds before the first tool-up move. This is a
/// declared upper bound for a probe, not a measurement of the real machine.
#[derive(Clone, Copy, Debug, Default)]
pub struct SetupTiming {
    pub before_motion_s: f64,
}

fn commands(job: &Job) -> Vec<Command> {
    commands_with_setup(job, SetupTiming::default())
}

fn commands_with_setup(job: &Job, setup: SetupTiming) -> Vec<Command> {
    let bytes = &job.plan.bytes;
    let mut out = Vec::new();
    let mut start = 0;
    // The tool starts at the start corner.
    let mut at: Option<[i32; 2]> = Some([0, 0]);
    let per_unit = 1.0 / (job.units_per_mm * job.mm_per_s.max(0.1));
    let per_unit_travel = 1.0 / (job.units_per_mm * job.travel_mm_s.max(0.1));
    // Whether the tool is down, and the direction of the last cut segment (for turns).
    let mut tool_down = false;
    let mut heading: Option<f64> = None;
    let mut first_motion = true;
    while start < bytes.len() {
        let end = if bytes[start] == ESC {
            (start + 2).min(bytes.len())
        } else {
            bytes[start..]
                .iter()
                .position(|b| *b == ETX)
                .map_or(bytes.len(), |n| start + n + 1)
        };
        let i = end - 1;
        let text = String::from_utf8_lossy(&bytes[start..i]);
        let mut duration_s = 0.0;
        let mut end = at;
        let mut points = Vec::new();
        let mut down = false;
        if let Some(code @ ('M' | 'D')) = text.chars().next() {
            down = code == 'D';
            let numbers: Vec<i32> = text[1..]
                .split(',')
                .filter_map(|n| n.trim().parse().ok())
                .collect();
            if code == 'M' {
                heading = None;
                if tool_down {
                    duration_s += job.tool_change_s;
                }
            }
            tool_down = down;
            for pair in numbers.chunks_exact(2) {
                let point = [pair[1], pair[0]];
                if let Some(from) = end {
                    let (dx, dy) = (f64::from(point[0] - from[0]), f64::from(point[1] - from[1]));
                    let d = dx.hypot(dy);
                    duration_s += d * if down { per_unit } else { per_unit_travel };
                    if down && d > 0.0 {
                        let a = dy.atan2(dx);
                        if let Some(h) = heading {
                            let turn = (a - h).rem_euclid(std::f64::consts::TAU);
                            duration_s +=
                                turn.min(std::f64::consts::TAU - turn) * job.turn_s_per_rad;
                        }
                        heading = Some(a);
                    }
                }
                duration_s += job.point_s;
                end = Some(point);
                points.push(point);
            }
        }
        let setup_s = if !points.is_empty() && first_motion {
            first_motion = false;
            setup.before_motion_s
        } else {
            0.0
        };
        duration_s += setup_s;
        at = end;
        let checkpoint = job.plan.passes.iter().any(|p| p.bytes[0] == start)
            || job.plan.passes.last().is_none_or(|p| start >= p.bytes[1]);
        let passes_after = job
            .plan
            .passes
            .iter()
            .filter(|p| p.bytes[1] <= i + 1)
            .count();
        out.push(Command {
            range: [start, i + 1],
            duration_s,
            setup_s,
            end,
            checkpoint,
            passes_after,
            points,
            down,
        });
        start = i + 1;
    }
    out
}

/// The estimated motion time of the whole job, as the driver paces it.
pub fn estimate_s(job: &Job) -> f64 {
    commands(job).iter().map(|c| c.duration_s).sum()
}

pub fn estimate_with_setup_s(job: &Job, setup: SetupTiming) -> f64 {
    commands_with_setup(job, setup)
        .iter()
        .map(|c| c.duration_s)
        .sum()
}

/// Asks for status: ESC ENQ, one reply byte, ETX.
pub fn query_status(
    transport: &mut impl Transport,
    timeout: Duration,
) -> Result<DeviceStatus, TransportError> {
    status_after(transport, timeout, timeout)
}

/// Status while commands are queued: ESC ENQ shares the OUT endpoint with them, so with the
/// cutter's buffer full it waits for space like any write (up to `write_timeout`); the reply
/// then has `read_timeout`.
fn status_after(
    transport: &mut impl Transport,
    write_timeout: Duration,
    read_timeout: Duration,
) -> Result<DeviceStatus, TransportError> {
    write_all(transport, &[ESC, 0x05], write_timeout)?;
    let timeout = read_timeout;
    let mut reply = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        transport.read(&mut byte, timeout)?;
        if byte[0] == ETX {
            break;
        }
        if reply.len() >= 16 {
            return Err(TransportError::Other("status reply without ETX".into()));
        }
        reply.push(byte[0]);
    }
    match reply.as_slice() {
        b"0" => Ok(DeviceStatus::Ready),
        b"1" => Ok(DeviceStatus::Moving),
        b"2" => Ok(DeviceStatus::NoMedia),
        other => Err(TransportError::Other(format!(
            "unexpected status reply {other:?}"
        ))),
    }
}

/// Asks for the firmware version (`FG`): the reply up to ETX, "CAMEO V1.20" on the tested machine.
pub fn query_firmware(
    transport: &mut impl Transport,
    timeout: Duration,
) -> Result<String, TransportError> {
    write_all(transport, &[b'F', b'G', ETX], timeout)?;
    let mut reply = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        transport.read(&mut byte, timeout)?;
        if byte[0] == ETX {
            break;
        }
        if reply.len() >= 64 {
            return Err(TransportError::Other("firmware reply without ETX".into()));
        }
        reply.push(byte[0]);
    }
    Ok(String::from_utf8_lossy(&reply).trim().to_owned())
}

/// One reply recorded by the probe.
#[derive(Clone, Debug, Serialize)]
pub struct ProbeStep {
    pub query: &'static str,
    pub reply: Option<String>,
    pub error: Option<String>,
    pub millis: u128,
}

/// The read-only probe (spec §7.3): status, firmware, status again. Nothing that moves, resets
/// or unloads the machine is sent; every step has a timeout.
pub fn probe(transport: &mut impl Transport, timeout: Duration) -> Vec<ProbeStep> {
    let mut steps = Vec::new();
    for query in ["status", "firmware", "status"] {
        let start = std::time::Instant::now();
        let result = match query {
            "firmware" => query_firmware(transport, timeout),
            _ => query_status(transport, timeout).map(|s| format!("{s:?}")),
        };
        steps.push(ProbeStep {
            query,
            millis: start.elapsed().as_millis(),
            reply: result.as_ref().ok().cloned(),
            error: result.err().map(|e| e.to_string()),
        });
    }
    steps
}

fn write_all(
    transport: &mut impl Transport,
    data: &[u8],
    timeout: Duration,
) -> Result<(), TransportError> {
    let mut sent = 0;
    while sent < data.len() {
        match transport.write(&data[sent..], timeout)? {
            0 => return Err(TransportError::Other("the device accepted nothing".into())),
            n => sent += n,
        }
    }
    Ok(())
}

enum Settled {
    Ready,
    NotLoaded,
    Stopped,
    Unsettled,
    NoAnswer(TransportError),
}

/// Waits for status to say Ready for [`SETTLE_S`] without a break. NoMedia twice in a row means
/// nothing is loaded (a single reply can be stale or transient); Moving or NoMedia restarts the
/// wait. Nothing but status queries is sent.
fn settle(transport: &mut impl Transport, clock: &impl Clock, controls: &Controls) -> Settled {
    let start = clock.now_s();
    let mut ready_since: Option<f64> = None;
    let mut no_media = false;
    loop {
        if controls.stopping() {
            return Settled::Stopped;
        }
        let now = clock.now_s();
        match query_status(transport, STATUS_TIMEOUT) {
            Ok(DeviceStatus::Ready) => {
                no_media = false;
                if now - *ready_since.get_or_insert(now) >= SETTLE_S {
                    return Settled::Ready;
                }
            }
            Ok(DeviceStatus::NoMedia) if no_media => return Settled::NotLoaded,
            Ok(status) => {
                no_media = status == DeviceStatus::NoMedia;
                ready_since = None;
            }
            Err(error) => return Settled::NoAnswer(error),
        }
        if now - start >= SETTLE_DEADLINE_S {
            return Settled::Unsettled;
        }
        clock.sleep(SETTLE_POLL);
    }
}

/// How long a write may wait for space: the longest command, at the slowest pace, could be
/// what the buffer is waiting on.
fn write_timeout(commands: &[Command]) -> Duration {
    let longest = commands.iter().map(|c| c.duration_s).fold(0.0, f64::max);
    WRITE_TIMEOUT.max(Duration::from_secs_f64(longest * MAX_PACE * 2.0))
}

/// A tool-up move to where the tool is: the blade-up request (hypothesis).
fn lift(at: [i32; 2]) -> Vec<u8> {
    let mut bytes = format!("M{},{}", at[1], at[0]).into_bytes();
    bytes.push(ETX);
    bytes
}

struct Run<'a, T, C, R> {
    transport: &'a mut T,
    clock: &'a C,
    controls: &'a Controls,
    report: R,
    total_s: f64,
    total_bytes: usize,
    passes_total: usize,
    sent_s: f64,
    sent_bytes: usize,
    passes_sent: usize,
    /// When the queued motion is estimated to finish.
    busy_until: f64,
    /// How long a write may wait for buffer space.
    write_timeout: Duration,
    /// How many times slower than estimated the machine is taken to be.
    pace: f64,
    /// Commands sent and not yet estimated finished, oldest first.
    timeline: std::collections::VecDeque<Sent>,
    /// The stream estimated finished, and where the blade then was.
    done_bytes: usize,
    done_at: [i32; 2],
    /// Where the last command sent ends.
    sent_at: [i32; 2],
    /// `wait_idle` saw the cutter moving since this was last cleared.
    saw_moving: bool,
    /// Until then the cutter may still be starting up and answer Ready.
    waking_until: f64,
    experimental_setup: bool,
}

impl<T: Transport, C: Clock, R: FnMut(&Progress)> Run<'_, T, C, R> {
    fn progress(&mut self, phase: Phase) {
        let now = self.clock.now_s();
        let queued = (self.busy_until - now).max(0.0) / self.pace;
        let done = (self.sent_s - queued).max(0.0);
        let progress = Progress {
            phase,
            sent_bytes: self.sent_bytes,
            total_bytes: self.total_bytes,
            passes_sent: self.passes_sent,
            passes_total: self.passes_total,
            fraction: if self.total_s > 0.0 {
                (done / self.total_s).clamp(0.0, 0.99)
            } else {
                0.0
            },
            remaining_s: (self.total_s - done).max(0.0) * self.pace,
            pace: self.pace,
            estimate: Some(self.estimate(now)),
        };
        (self.report)(&progress);
    }

    /// Where the blade is estimated to be at `now`, retiring commands estimated finished.
    fn estimate(&mut self, now: f64) -> Estimate {
        while self.timeline.front().is_some_and(|c| c.end <= now) {
            if let Some(done) = self.timeline.pop_front() {
                self.done_bytes = done.byte_end;
                self.done_at = done.points.last().copied().unwrap_or(done.from);
            }
        }
        let Some(current) = self.timeline.front() else {
            return Estimate {
                done_bytes: self.done_bytes,
                at: self.done_at,
                down: false,
                ahead: Vec::new(),
            };
        };
        let span = (current.end - current.start).max(1e-9);
        let f = ((now - current.start) / span).clamp(0.0, 1.0);
        let (at, passed) = along(current.from, &current.points, f);
        let mut ahead = vec![at];
        ahead.extend_from_slice(&current.points[passed.min(current.points.len())..]);
        for later in self.timeline.iter().skip(1) {
            ahead.extend_from_slice(&later.points);
        }
        Estimate {
            done_bytes: self.done_bytes,
            at,
            down: current.down && f > 0.0,
            ahead,
        }
    }

    /// The machine is further behind than estimated (`by` seconds): queued commands finish
    /// later.
    fn delay(&mut self, by: f64) {
        let now = self.clock.now_s();
        for sent in self.timeline.iter_mut().filter(|c| c.end > now) {
            sent.start = sent.start.max(now) + by;
            sent.end += by;
        }
    }

    /// The cutter is idle: everything sent is finished.
    fn caught_up(&mut self) {
        let now = self.clock.now_s();
        for sent in &mut self.timeline {
            sent.start = sent.start.min(now);
            sent.end = sent.end.min(now);
        }
        let _ = self.estimate(now);
    }

    fn interrupted(&self, reason: impl Into<String>) -> Outcome {
        Outcome::Interrupted {
            reason: reason.into(),
            passes_sent: self.passes_sent,
        }
    }

    /// Polls until the cutter is no longer moving, for at most the queued time (at the slowest
    /// pace) plus a margin, as long as it keeps answering. `None` hands a Stop received during
    /// draining back to the caller immediately.
    fn wait_idle(&mut self, phase: Phase) -> Result<Option<DeviceStatus>, TransportError> {
        let queued = (self.busy_until - self.clock.now_s()).max(0.0) / self.pace;
        let limit = self.clock.now_s() + queued * MAX_PACE * 2.0 + 120.0;
        let mut ready_since = None;
        loop {
            if phase == Phase::Draining && self.controls.stopping() {
                return Ok(None);
            }
            let status = status_after(self.transport, self.write_timeout, STATUS_TIMEOUT)?;
            if phase == Phase::Draining && self.controls.stopping() {
                return Ok(None);
            }
            if status == DeviceStatus::NoMedia {
                return Ok(Some(status));
            }
            if status == DeviceStatus::Ready && self.clock.now_s() >= self.waking_until {
                // A Stop during experimental setup must not call a deferred tool-up move
                // idle in the tiny gap between the allowance and its actual start.
                let since = *ready_since.get_or_insert(self.clock.now_s());
                if !self.experimental_setup
                    || phase != Phase::Stopping
                    || self.clock.now_s() - since >= READY_HOLD_S
                {
                    return Ok(Some(status));
                }
            } else {
                ready_since = None;
            }
            self.saw_moving |= status == DeviceStatus::Moving;
            if self.clock.now_s() > limit {
                return Err(TransportError::Timeout);
            }
            self.progress(phase);
            self.clock.sleep(Duration::from_millis(250));
        }
    }

    /// After Stop: send nothing more, ask for the blade to lift, and wait for idle.
    fn stop(&mut self, at: Option<[i32; 2]>) -> Outcome {
        self.progress(Phase::Stopping);
        if let Some(at) = at
            && write_all(self.transport, &lift(at), self.write_timeout).is_err()
        {
            return Outcome::Stopped {
                idle: false,
                passes_sent: self.passes_sent,
            };
        }
        let idle =
            matches!(self.wait_idle(Phase::Stopping), Ok(Some(s)) if s != DeviceStatus::Moving);
        Outcome::Stopped {
            idle,
            passes_sent: self.passes_sent,
        }
    }
}

/// Cut one encoded plan. `report` hears about progress and every phase change. The material
/// must already be loaded: a cutter reporting nothing loaded is [`Outcome::NotLoaded`].
pub fn run(
    transport: &mut impl Transport,
    clock: &impl Clock,
    job: &Job,
    controls: &Controls,
    report: impl FnMut(&Progress),
) -> Outcome {
    run_checked(
        transport,
        clock,
        job,
        controls,
        Permission::Production,
        SetupTiming::default(),
        report,
    )
}

/// Hardware-test entry point. Only the selected candidate setting is added to the production
/// allowlist. The app uses `run`, which cannot enable these permissions through job data.
pub fn run_experiment(
    transport: &mut impl Transport,
    clock: &impl Clock,
    job: &Job,
    controls: &Controls,
    experiment: Option<gpgl::Experiment>,
    setup: SetupTiming,
    report: impl FnMut(&Progress),
) -> Outcome {
    let permission = experiment.map_or(Permission::Production, Permission::Experiment);
    run_checked(transport, clock, job, controls, permission, setup, report)
}

/// Only the staged hardware probe uses this capability. It accepts one exact frame-setting
/// command, with no motion, prefix or suffix. It cannot enable a full orientation drawing job.
#[derive(Clone, Copy, Debug)]
pub enum FrameCommand {
    Fn0,
    Tb50,
}
impl FrameCommand {
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::Fn0 => b"FN0\x03",
            Self::Tb50 => b"TB50,0\x03",
        }
    }
}

enum Permission {
    Production,
    Experiment(gpgl::Experiment),
    FrameOnly(FrameCommand),
}

/// Requires a separate physical-origin review by the caller. Ordinary jobs and the old
/// experiment permissions continue to refuse FN/TB. All subsequent motion is a separate job.
pub fn run_frame_step(
    transport: &mut impl Transport,
    clock: &impl Clock,
    job: &Job,
    controls: &Controls,
    command: FrameCommand,
    report: impl FnMut(&Progress),
) -> Outcome {
    run_checked(
        transport,
        clock,
        job,
        controls,
        Permission::FrameOnly(command),
        SetupTiming::default(),
        report,
    )
}

fn run_checked(
    transport: &mut impl Transport,
    clock: &impl Clock,
    job: &Job,
    controls: &Controls,
    permission: Permission,
    setup: SetupTiming,
    report: impl FnMut(&Progress),
) -> Outcome {
    if job.plan.model != gpgl::CAMEO_1.name
        || !setup.before_motion_s.is_finite()
        || !(0.0..=120.0).contains(&setup.before_motion_s)
    {
        return Outcome::Refused {
            reason: "Unsupported model or invalid setup allowance. Nothing sent.".into(),
        };
    }
    let checked = match permission {
        Permission::Experiment(e) => gpgl::validate_experiment(&job.plan.bytes, &gpgl::CAMEO_1, job.plan.cutting_area_mm, e),
        Permission::Production => gpgl::validate(&job.plan.bytes, &gpgl::CAMEO_1, job.plan.cutting_area_mm),
        Permission::FrameOnly(command) if job.plan.bytes == command.bytes() => Ok(()),
        Permission::FrameOnly(_) => Err(gpgl::StreamError { byte: 0, reason: "The staged frame step accepts only its one exact command, with no motion or other bytes".into() }),
    };
    if let Err(error) = checked {
        return Outcome::Refused {
            reason: error.to_string(),
        };
    }
    let commands = commands_with_setup(job, setup);
    if setup.before_motion_s > 0.0
        && commands
            .iter()
            .find(|c| !c.points.is_empty())
            .is_some_and(|c| c.down)
    {
        return Outcome::Refused {
            reason: "A setup probe must begin with a tool-up move. Nothing sent.".into(),
        };
    }
    let mut run = Run {
        transport,
        clock,
        controls,
        report,
        total_s: commands.iter().map(|c| c.duration_s).sum(),
        total_bytes: job.plan.bytes.len(),
        passes_total: job.plan.passes.len(),
        sent_s: 0.0,
        sent_bytes: 0,
        passes_sent: 0,
        busy_until: clock.now_s(),
        write_timeout: write_timeout(&commands),
        pace: 1.0,
        timeline: std::collections::VecDeque::new(),
        done_bytes: 0,
        done_at: [0, 0],
        sent_at: [0, 0],
        saw_moving: false,
        waking_until: f64::NEG_INFINITY,
        experimental_setup: setup.before_motion_s > 0.0,
    };
    let start = clock.now_s();
    match settle(run.transport, clock, controls) {
        Settled::Ready => {}
        Settled::NotLoaded => return Outcome::NotLoaded,
        Settled::Stopped => return run.stop(None),
        Settled::Unsettled => {
            return run.interrupted(
                "The cutter's status did not settle on ready before the cut started.",
            );
        }
        Settled::NoAnswer(error) => {
            return run.interrupted(format!("The cutter did not answer: {error}."));
        }
    }
    let mut at: Option<[i32; 2]> = None;
    let mut last_status = clock.now_s();
    for command in &commands {
        if run.controls.stopping() {
            return run.stop(at);
        }
        if command.checkpoint && run.controls.pausing() {
            // Pause at the boundary between cuts, with the blade lifted.
            run.progress(Phase::Pausing);
            if let Some(at) = at
                && let Err(error) = write_all(run.transport, &lift(at), run.write_timeout)
            {
                return run.interrupted(format!("Pausing failed: {error}."));
            }
            match run.wait_idle(Phase::Pausing) {
                Ok(Some(DeviceStatus::NoMedia)) => {
                    return run.interrupted("The material was unloaded while pausing.");
                }
                Ok(None) => return run.stop(at),
                Ok(Some(_)) => {}
                Err(error) => return run.interrupted(format!("Pausing failed: {error}.")),
            }
            let idle_since = clock.now_s();
            while run.controls.pausing() && !run.controls.stopping() {
                run.progress(Phase::Paused);
                clock.sleep(Duration::from_millis(100));
            }
            if run.controls.stopping() {
                return run.stop(None);
            }
            // Idle since the pause began: a long pause brings the start-up delay.
            run.busy_until = idle_since;
        }
        // Pace by estimated motion time, keeping the estimate moving meanwhile.
        while run.busy_until - clock.now_s() > LEAD_S
            || clock.now_s() < run.waking_until && setup.before_motion_s > 0.0
        {
            if run.controls.stopping() {
                return run.stop(at);
            }
            clock.sleep(POLL);
            // Experimental setup may move rollers while delaying the first draw. Keep
            // recording status during that allowance, without treating Ready as catch-up.
            if setup.before_motion_s > 0.0 && clock.now_s() - last_status >= STATUS_EVERY_S {
                last_status = clock.now_s();
                match status_after(run.transport, run.write_timeout, STATUS_TIMEOUT) {
                    Ok(DeviceStatus::NoMedia) => {
                        return run.interrupted("The material was unloaded during setup.");
                    }
                    Ok(_) => {}
                    Err(error) => {
                        return run.interrupted(format!("Lost contact during setup: {error}."));
                    }
                }
            }
            run.progress(Phase::Sending);
        }
        if clock.now_s() - last_status >= STATUS_EVERY_S {
            last_status = clock.now_s();
            match status_after(run.transport, run.write_timeout, STATUS_TIMEOUT) {
                Ok(DeviceStatus::NoMedia) => {
                    return run.interrupted("The material was unloaded during the cut.");
                }
                // Idle with motion still estimated, past its start-up delay: the machine is
                // faster than the pace.
                Ok(DeviceStatus::Ready)
                    if run.busy_until - clock.now_s() > 0.5
                        && clock.now_s() >= run.waking_until + 0.5 =>
                {
                    run.pace = (run.pace * 0.9).max(1.0);
                    run.busy_until = clock.now_s();
                    run.caught_up();
                }
                Ok(_) => {}
                Err(error) => {
                    return run.interrupted(format!(
                        "Lost contact with the cutter while asking its status, {:.0}% sent: {error}.",
                        100.0 * run.sent_bytes as f64 / run.total_bytes.max(1) as f64
                    ));
                }
            }
        }
        let bytes = &job.plan.bytes[command.range[0]..command.range[1]];
        let before = clock.now_s();
        if let Err(error) = write_all(run.transport, bytes, run.write_timeout) {
            return run.interrupted(format!(
                "Lost contact with the cutter while sending, {:.0}% sent: {error}.",
                100.0 * run.sent_bytes as f64 / run.total_bytes.max(1) as f64
            ));
        }
        let now = clock.now_s();
        if now - before > BLOCKED_S {
            // The buffer was full: the machine is behind; slow the pace so it stays short.
            run.pace = (run.pace * 1.25).min(MAX_PACE);
            let behind = (now + LEAD_S - run.busy_until).max(0.0);
            run.delay(behind);
            run.busy_until = run.busy_until.max(now + LEAD_S);
        }
        run.sent_bytes = command.range[1];
        run.sent_s += command.duration_s;
        let starts = if now - run.busy_until >= START_DELAY_S {
            run.waking_until = now + START_DELAY_S;
            now + START_DELAY_S
        } else {
            run.busy_until.max(now)
        };
        if command.setup_s > 0.0 {
            run.waking_until = run.waking_until.max(starts + command.setup_s * run.pace);
        }
        run.busy_until = starts + command.duration_s * run.pace;
        let from = run.sent_at;
        if let Some(end) = command.end {
            run.sent_at = end;
        }
        run.timeline.push_back(Sent {
            start: starts + command.setup_s * run.pace,
            end: run.busy_until,
            byte_end: command.range[1],
            from,
            points: command.points.clone(),
            down: command.down,
        });
        run.passes_sent = command.passes_after;
        at = command.end.or(at);
        run.progress(Phase::Sending);
    }
    // Everything is queued; done only when the cutter has said it is idle for READY_HOLD_S.
    let mut ready_since: Option<f64> = None;
    loop {
        run.saw_moving = false;
        match run.wait_idle(Phase::Draining) {
            Ok(None) => return run.stop(at),
            Ok(Some(DeviceStatus::Ready)) => {
                let now = clock.now_s();
                // It moved meanwhile: the idle spell starts now.
                if run.saw_moving {
                    ready_since = None;
                }
                let since = *ready_since.get_or_insert(now);
                if now - since >= READY_HOLD_S {
                    return Outcome::Completed {
                        duration_s: since - start,
                    };
                }
                run.progress(Phase::Draining);
                clock.sleep(READY_POLL);
            }
            Ok(Some(DeviceStatus::NoMedia)) => {
                return run.interrupted("The material was unloaded before the cut finished.");
            }
            Ok(Some(DeviceStatus::Moving)) => continue,
            Err(error) => {
                return run.interrupted(format!("The cutter did not report finishing: {error}."));
            }
        }
    }
}
