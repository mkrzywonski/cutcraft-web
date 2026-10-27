//! The simulated connection: a shared [`Simulator`] and the [`SimTransport`] handles a driver
//! uses, with a virtual or real-time clock and scripted faults.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cutcraft_device::transport::{Transport, TransportError};
use serde::Serialize;

use crate::machine::{FaultKind, Machine, Snapshot};
use crate::profile::{Media, Profile};

/// How simulated time passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clock {
    /// Time moves only when a caller waits or calls [`Simulator::advance`]: fast and
    /// deterministic, for tests and the CLI.
    Virtual,
    /// Time follows the wall clock, for interactive use.
    Realtime,
}

/// Granularity of waits inside transport calls.
const WAIT_STEP: Duration = Duration::from_millis(5);

/// A fault scripted at a byte offset of the stream the device has received.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Trigger {
    /// The cable is pulled after `at` bytes; the machine keeps running what it buffered.
    Disconnect { at: usize },
    /// Power is lost after `at` bytes; buffered motion is lost.
    PowerOff { at: usize },
    /// The write that delivers byte `at` reports a timeout although its bytes arrived.
    TimeoutAfterDelivery { at: usize },
    /// The OUT endpoint halts before byte `at` until the driver clears it.
    Stall { at: usize },
    /// Someone presses the unload button before byte `at`.
    Unload { at: usize },
}

impl Trigger {
    fn at(self) -> usize {
        match self {
            Self::Disconnect { at }
            | Self::PowerOff { at }
            | Self::TimeoutAfterDelivery { at }
            | Self::Stall { at }
            | Self::Unload { at } => at,
        }
    }

    /// Fires before the byte at its offset is accepted, rather than after it.
    fn before(self) -> bool {
        matches!(self, Self::Stall { .. } | Self::Unload { .. })
    }
}

/// Starting conditions and scripted faults.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Scenario {
    pub name: String,
    pub media: Option<Media>,
    pub triggers: Vec<Trigger>,
    /// Delay before reply bytes can be read.
    pub reply_delay_s: f64,
    /// How many of the first status replies repeat the previous answer (initially no media).
    pub stale_status_replies: u32,
    /// The machine moves this many times slower than the profile's speeds (the driver's
    /// estimates stay the same, so its queue runs ahead of the machine).
    pub slowdown: f64,
    /// With the buffer full, escape sequences wait for space like any byte (the USB endpoint
    /// refuses everything until the firmware reads more).
    pub escapes_wait_when_full: bool,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            name: "normal".to_owned(),
            media: Some(Media::Mat12x12),
            triggers: Vec::new(),
            reply_delay_s: 0.0,
            stale_status_replies: 0,
            slowdown: 1.0,
            escapes_wait_when_full: false,
        }
    }
}

impl Scenario {
    pub const NAMES: &[&str] = &[
        "normal",
        "no-media",
        "unload-mid-cut",
        "disconnect-mid-cut",
        "power-off-mid-cut",
        "timeout-after-delivery",
        "stall",
        "slow-status",
        "stale-status",
        "slow-full-buffer",
    ];

    /// A named scenario; mid-cut faults happen halfway through a stream of `total_bytes`.
    pub fn named(name: &str, total_bytes: usize) -> Option<Self> {
        let half = (total_bytes / 2).max(1);
        let mut scenario = Self {
            name: name.to_owned(),
            ..Self::default()
        };
        match name {
            "normal" => {}
            "no-media" => scenario.media = None,
            "unload-mid-cut" => scenario.triggers.push(Trigger::Unload { at: half }),
            "disconnect-mid-cut" => scenario.triggers.push(Trigger::Disconnect { at: half }),
            "power-off-mid-cut" => scenario.triggers.push(Trigger::PowerOff { at: half }),
            "timeout-after-delivery" => scenario
                .triggers
                .push(Trigger::TimeoutAfterDelivery { at: half }),
            "stall" => scenario.triggers.push(Trigger::Stall { at: half }),
            "slow-status" => scenario.reply_delay_s = 0.5,
            "stale-status" => scenario.stale_status_replies = 1,
            // The 2026-10-18 real cut: interrupted halfway with a transfer timeout.
            "slow-full-buffer" => {
                scenario.slowdown = 2.5;
                scenario.escapes_wait_when_full = true;
            }
            _ => return None,
        }
        Some(scenario)
    }
}

#[derive(Debug)]
struct State {
    machine: Machine,
    clock: Clock,
    epoch: Instant,
    generation: u64,
    connected: bool,
    halted: bool,
    received: usize,
    triggers: Vec<(Trigger, bool)>,
    reply_delay_s: f64,
    replies: VecDeque<(f64, u8)>,
}

impl State {
    fn sync(&mut self) {
        if self.clock == Clock::Realtime {
            let elapsed = self.epoch.elapsed().as_secs_f64();
            let dt = elapsed - self.machine.now();
            if dt > 0.0 {
                self.machine.advance(dt);
            }
        }
        self.collect_replies();
    }

    fn advance(&mut self, dt: f64) {
        self.machine.advance(dt);
        self.collect_replies();
    }

    fn collect_replies(&mut self) {
        let ready_at = self.machine.now() + self.reply_delay_s;
        for byte in self.machine.take_replies() {
            self.replies.push_back((ready_at, byte));
        }
    }

    fn check(&self, generation: u64) -> Result<(), TransportError> {
        if generation != self.generation || !self.connected {
            Err(TransportError::Disconnected)
        } else {
            Ok(())
        }
    }

    /// Fires due triggers of one phase; the first one that fails the transfer is returned.
    fn fire(&mut self, before: bool) -> Result<(), TransportError> {
        let mut result = Ok(());
        for index in 0..self.triggers.len() {
            let (trigger, fired) = self.triggers[index];
            if fired || trigger.before() != before {
                continue;
            }
            let due = if before {
                self.received >= trigger.at()
            } else {
                self.received >= trigger.at() && self.received > 0
            };
            if !due {
                continue;
            }
            self.triggers[index].1 = true;
            let outcome = match trigger {
                Trigger::Disconnect { .. } => {
                    self.connected = false;
                    self.machine.fault(
                        FaultKind::Disconnected,
                        format!("disconnected after {} bytes", self.received),
                    );
                    Err(TransportError::Disconnected)
                }
                Trigger::PowerOff { .. } => {
                    self.connected = false;
                    self.machine.power_off();
                    Err(TransportError::Disconnected)
                }
                Trigger::TimeoutAfterDelivery { .. } => Err(TransportError::Timeout),
                Trigger::Stall { .. } => {
                    self.halted = true;
                    Err(TransportError::Stalled)
                }
                Trigger::Unload { .. } => {
                    self.machine.unload();
                    Ok(())
                }
            };
            if result.is_ok() {
                result = outcome;
            }
        }
        result
    }

    /// How many bytes may be accepted before the next unfired trigger.
    fn limit(&self, len: usize) -> usize {
        self.triggers
            .iter()
            .filter(|(trigger, fired)| !fired && trigger.at() > self.received)
            .map(|(trigger, _)| trigger.at() - self.received)
            .fold(len, usize::min)
    }
}

/// A simulated Cameo shared between transports, displays and test code.
#[derive(Debug, Clone)]
pub struct Simulator {
    state: Arc<Mutex<State>>,
}

impl Simulator {
    pub fn new(mut profile: Profile, clock: Clock, scenario: &Scenario) -> Self {
        let slowdown = scenario.slowdown.max(0.01);
        profile.speed_step_mm_s /= slowdown;
        profile.travel_mm_s /= slowdown;
        profile.tool_change_s *= slowdown;
        profile.command_overhead_s *= slowdown;
        profile.turn_s_per_rad *= slowdown;
        profile.escapes_wait_when_full = scenario.escapes_wait_when_full;
        let mut machine = Machine::new(profile);
        if let Some(media) = scenario.media {
            machine.load(media);
        }
        machine.set_stale_status_replies(scenario.stale_status_replies);
        Self {
            state: Arc::new(Mutex::new(State {
                machine,
                clock,
                epoch: Instant::now(),
                generation: 0,
                connected: true,
                halted: false,
                received: 0,
                triggers: scenario.triggers.iter().map(|&t| (t, false)).collect(),
                reply_delay_s: scenario.reply_delay_s.max(0.0),
                replies: VecDeque::new(),
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock leaves the simulation usable for inspection.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A handle for the current connection.
    pub fn transport(&self) -> SimTransport {
        let generation = self.lock().generation;
        SimTransport {
            simulator: self.clone(),
            generation,
        }
    }

    pub fn clock(&self) -> Clock {
        self.lock().clock
    }

    /// Advances a virtual clock; with a real-time clock this only catches up.
    pub fn advance(&self, dt: Duration) {
        let mut state = self.lock();
        if state.clock == Clock::Virtual {
            state.advance(dt.as_secs_f64());
        } else {
            state.sync();
        }
    }

    /// Waits as the clock requires: advances a virtual clock or sleeps for real.
    pub fn wait(&self, dt: Duration) {
        if self.clock() == Clock::Virtual {
            self.advance(dt);
        } else {
            std::thread::sleep(dt);
            self.lock().sync();
        }
    }

    /// Runs until nothing is executing or queued, or `limit` passes; returns whether it stopped.
    pub fn run_until_idle(&self, limit: Duration) -> bool {
        let step = Duration::from_millis(100);
        let mut waited = Duration::ZERO;
        loop {
            {
                let mut state = self.lock();
                state.sync();
                if !state.machine.busy() || !state.machine.powered() {
                    return true;
                }
            }
            if waited >= limit {
                return false;
            }
            self.wait(step);
            waited += step;
        }
    }

    pub fn load(&self, media: Media) {
        let mut state = self.lock();
        state.sync();
        state.machine.load(media);
    }

    pub fn unload(&self) {
        let mut state = self.lock();
        state.sync();
        state.machine.unload();
    }

    /// Power loss also drops the connection.
    pub fn power_off(&self) {
        let mut state = self.lock();
        state.sync();
        state.machine.power_off();
        state.connected = false;
    }

    pub fn power_on(&self) {
        let mut state = self.lock();
        state.sync();
        state.machine.power_on();
    }

    /// Plugs the cable back in. Handles from before keep failing as `Disconnected`.
    pub fn reconnect(&self) {
        let mut state = self.lock();
        state.sync();
        if state.machine.powered() {
            state.generation += 1;
            state.connected = true;
            state.halted = false;
            state.replies.clear();
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut state = self.lock();
        state.sync();
        state.machine.snapshot()
    }

    /// Bytes the device has received so far.
    pub fn received(&self) -> usize {
        self.lock().received
    }

    /// Runs `f` with the machine, for reports and inspection.
    pub fn with_machine<T>(&self, f: impl FnOnce(&Machine) -> T) -> T {
        let mut state = self.lock();
        state.sync();
        f(&state.machine)
    }
}

/// The driver's clock is the simulator's: virtual time advances when the driver waits.
impl cutcraft_device::driver::Clock for Simulator {
    fn now_s(&self) -> f64 {
        self.with_machine(Machine::now)
    }
    fn sleep(&self, duration: Duration) {
        self.wait(duration);
    }
}

/// One connection's handle to a [`Simulator`].
#[derive(Debug, Clone)]
pub struct SimTransport {
    simulator: Simulator,
    generation: u64,
}

impl SimTransport {
    fn pause(&self, step: Duration) {
        self.simulator.wait(step);
    }
}

impl Transport for SimTransport {
    fn write(&mut self, data: &[u8], timeout: Duration) -> Result<usize, TransportError> {
        if data.is_empty() {
            return Ok(0);
        }
        let mut waited = Duration::ZERO;
        loop {
            {
                let mut state = self.simulator.lock();
                state.sync();
                state.check(self.generation)?;
                state.fire(true)?;
                if state.halted {
                    return Err(TransportError::Stalled);
                }
                let limit = state.limit(data.len());
                let accepted = state.machine.accept(&data[..limit]);
                if accepted > 0 {
                    state.received += accepted;
                    state.collect_replies();
                    state.fire(false)?;
                    return Ok(accepted);
                }
                if !state.machine.powered() {
                    return Err(TransportError::Disconnected);
                }
            }
            if waited >= timeout {
                return Err(TransportError::Timeout);
            }
            let step = WAIT_STEP.min(timeout - waited);
            self.pause(step);
            waited += step;
        }
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, TransportError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut waited = Duration::ZERO;
        loop {
            {
                let mut state = self.simulator.lock();
                state.sync();
                state.check(self.generation)?;
                let now = state.machine.now();
                let mut count = 0;
                while count < buf.len() {
                    match state.replies.front() {
                        Some(&(ready_at, byte)) if ready_at <= now + 1e-12 => {
                            buf[count] = byte;
                            count += 1;
                            state.replies.pop_front();
                        }
                        _ => break,
                    }
                }
                if count > 0 {
                    return Ok(count);
                }
            }
            if waited >= timeout {
                return Err(TransportError::Timeout);
            }
            let step = WAIT_STEP.min(timeout - waited);
            self.pause(step);
            waited += step;
        }
    }

    fn clear_halt(&mut self) -> Result<(), TransportError> {
        let mut state = self.simulator.lock();
        state.check(self.generation)?;
        state.halted = false;
        Ok(())
    }
}
