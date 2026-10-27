//! The device side of one cutter: the single owner of its USB handle (or simulator), which polls
//! status while idle and runs accepted executions through CutCraft's driver (spec §5, §8.1).
//!
//! Polling stops while the driver runs, so replies can't go to the wrong reader. Each query has
//! a bounded timeout, and a Stop never waits for a poll: it goes straight to the execution's
//! [`Controls`](cutcraft_device::driver::Controls) through the cutter's lock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cutcraft_device::driver::{self, Clock, DeviceStatus};
use cutcraft_device::gpgl::Model;
use cutcraft_device::transport::{Transport, TransportError};
use cutcraft_device::usb::UsbTransport;
use cutcraft_sim::{SimTransport, Simulator};

use crate::cutter::{Cutter, Finished};
use crate::journal::Journal;
use crate::mat::Reading;

/// The candidate polling rate (spec §8.1), measured harmless for 120 s on the Pi; milestone 1's
/// long run measures more.
pub const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// The longest a status query may take.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(2);

/// One cutter's state, shared between its worker and the requests.
pub type Shared = Arc<Mutex<Cutter>>;

/// Locks a cutter, recovering it after a panic elsewhere so Stop keeps working.
pub fn lock(cutter: &Shared) -> MutexGuard<'_, Cutter> {
    cutter
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How to reach one cutter.
pub trait Link {
    type Transport: Transport;
    type Clock: Clock;
    /// Opens the cutter. It isn't believed connected until a status query is answered.
    fn open(&mut self) -> Result<Self::Transport, String>;
    fn clock(&self) -> &Self::Clock;
}

/// The Simulated Cameo (spec §6): the practice cutter, and every test.
pub struct SimLink {
    pub sim: Simulator,
}

impl Link for SimLink {
    type Transport = SimTransport;
    type Clock = Simulator;
    fn open(&mut self) -> Result<SimTransport, String> {
        Ok(self.sim.transport())
    }
    fn clock(&self) -> &Simulator {
        &self.sim
    }
}

/// Wall-clock time for a real cutter.
pub struct WallClock(Instant);

impl Default for WallClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl Clock for WallClock {
    fn now_s(&self) -> f64 {
        self.0.elapsed().as_secs_f64()
    }
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// A Cameo on USB. Opening takes the interface from `usblp` for as long as the handle lives.
pub struct UsbLink {
    pub model: &'static Model,
    pub clock: WallClock,
}

impl Link for UsbLink {
    type Transport = UsbTransport;
    type Clock = WallClock;
    fn open(&mut self) -> Result<UsbTransport, String> {
        UsbTransport::open(self.model).map_err(|e| e.to_string())
    }
    fn clock(&self) -> &WallClock {
        &self.clock
    }
}

/// What one [`Worker::step`] did, for tests and logs.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// The cutter couldn't be opened.
    Unavailable(String),
    Polled(Reading),
    Disconnected,
    /// An execution ran to its outcome.
    Ran(String),
}

pub struct Worker<L: Link> {
    cutter: Shared,
    link: L,
    journal: Arc<Journal>,
    /// Where ended executions go for the jobs' history.
    history: Option<std::sync::mpsc::Sender<Finished>>,
    transport: Option<L::Transport>,
    /// The cutter has answered on this connection and been reported connected.
    announced: bool,
    next_poll_s: f64,
}

impl<L: Link> Worker<L> {
    pub fn new(
        cutter: Shared,
        link: L,
        journal: Arc<Journal>,
        history: Option<std::sync::mpsc::Sender<Finished>>,
    ) -> Self {
        Self {
            cutter,
            link,
            journal,
            history,
            transport: None,
            announced: false,
            next_poll_s: 0.0,
        }
    }

    pub fn link(&self) -> &L {
        &self.link
    }

    fn connection_lost(&mut self) {
        self.transport = None;
        if self.announced {
            self.announced = false;
            lock(&self.cutter).disconnected();
        }
    }

    /// Completes the records of executions that ended, and passes them on for history. A record
    /// that can't be completed stays open and is marked interrupted at the next start-up.
    fn settle_records(&mut self) {
        let ended = lock(&self.cutter).take_finished();
        for finished in ended {
            let _ = self.journal.ended(&finished.execution, &finished.ended);
            if let Some(history) = &self.history {
                let _ = history.send(finished);
            }
        }
    }

    /// Runs a pending execution if there is one; otherwise waits for the next polling slot and
    /// asks for status once.
    pub fn step(&mut self) -> Step {
        let step = self.step_inner();
        self.settle_records();
        step
    }

    fn step_inner(&mut self) -> Step {
        // Taken in its own statement, so the lock is released before the cut runs.
        let pending = if self.transport.is_some() {
            lock(&self.cutter).take_pending()
        } else {
            None
        };
        if let (Some(transport), Some(execution)) = (self.transport.as_mut(), pending) {
            // The record comes first: without it, nothing is sent (spec §8.7).
            let cutter = lock(&self.cutter).id().to_owned();
            if let Err(error) = self.journal.started(&cutter, &execution) {
                let reason = format!("couldn't record the cut before starting it: {error}");
                lock(&self.cutter).finished(
                    &execution.id,
                    cutcraft_device::driver::Outcome::Refused { reason },
                );
                return Step::Ran(execution.id);
            }
            let clock = self.link.clock();
            let job = execution.run.job();
            let cutter = self.cutter.clone();
            let id = execution.id.clone();
            let outcome = driver::run(transport, clock, &job, &execution.controls, |p| {
                lock(&cutter).progress(&id, p);
            });
            let at = clock.now_s();
            {
                let mut c = lock(&self.cutter);
                c.finished(&execution.id, outcome);
                c.resume_polling(at);
            }
            self.next_poll_s = at;
            return Step::Ran(execution.id);
        }

        let clock = self.link.clock();
        let now = clock.now_s();
        if now < self.next_poll_s {
            clock.sleep(Duration::from_secs_f64(self.next_poll_s - now));
        }
        let interval = POLL_INTERVAL.as_secs_f64();
        self.next_poll_s = (self.next_poll_s + interval).max(clock.now_s());

        if self.transport.is_none() {
            match self.link.open() {
                Ok(transport) => self.transport = Some(transport),
                Err(error) => return Step::Unavailable(error),
            }
        }
        let clock = self.link.clock();
        let transport = self.transport.as_mut().expect("opened above");
        let at = clock.now_s();
        let reading = match driver::query_status(transport, STATUS_TIMEOUT) {
            Ok(DeviceStatus::Ready) => Reading::Ready,
            Ok(DeviceStatus::Moving) => Reading::Moving,
            Ok(DeviceStatus::NoMedia) => Reading::NoMedia,
            Err(TransportError::Disconnected) => {
                self.connection_lost();
                return Step::Disconnected;
            }
            Err(_) if !self.announced => {
                // Never answered on this connection: try opening again next time.
                self.transport = None;
                return Step::Unavailable("no answer".into());
            }
            Err(_) => Reading::Failed,
        };
        let mut cutter = lock(&self.cutter);
        if !self.announced {
            self.announced = true;
            cutter.connected();
        }
        cutter.observe(at, reading);
        Step::Polled(reading)
    }
}

impl<L: Link + Send + 'static> Worker<L>
where
    L::Transport: Send,
{
    /// Runs the worker on its own thread until `shutdown` is set.
    pub fn spawn(mut self, shutdown: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            while !shutdown.load(Ordering::SeqCst) {
                if let Step::Unavailable(_) = self.step() {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use cutcraft_sim::{Clock as SimClock, Media, Profile, Scenario, Trigger};

    use super::*;
    use crate::cutter::{Actor, Answer, Ended, JobRef, Kind, StartRequest};
    use crate::mat::Mat;
    use crate::runnable::tests::calibration;

    fn shared() -> Shared {
        Arc::new(Mutex::new(Cutter::new("practice", "Practice Cameo", "t")))
    }

    /// A worker with a journal in a temporary folder, and the history it reports.
    fn worker<L: Link>(
        cutter: &Shared,
        link: L,
    ) -> (Worker<L>, mpsc::Receiver<Finished>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let (journal, _) = Journal::open(dir.path()).unwrap();
        let (tx, rx) = mpsc::channel();
        (
            Worker::new(cutter.clone(), link, Arc::new(journal), Some(tx)),
            rx,
            dir,
        )
    }

    fn sim(scenario: &Scenario) -> Simulator {
        Simulator::new(Profile::cameo_1(), SimClock::Virtual, scenario)
    }

    fn steps<L: Link>(worker: &mut Worker<L>, n: usize) {
        for _ in 0..n {
            worker.step();
        }
    }

    /// Unloads and reloads the simulator's mat while the worker watches: a seen load.
    fn load_fresh<L: Link>(worker: &mut Worker<L>, sim: &Simulator) {
        sim.unload();
        steps(worker, 3);
        sim.load(Media::Mat12x12);
        steps(worker, 9);
    }

    fn start(cutter: &Shared, kind: Kind, key: &str) -> Result<Answer, crate::cutter::Refusal> {
        let mut c = lock(cutter);
        let Mat::Fresh(generation) = c.mat() else {
            panic!("mat not fresh: {:?}", c.mat())
        };
        let turn = c.view().turn.map(|t| t.id);
        let request = StartRequest {
            boot: c.boot().to_owned(),
            kind,
            turn,
            job: JobRef {
                job_id: "calibration".into(),
                revision: 1,
            },
            generation,
            action_epoch: c.action_epoch(),
            run: Arc::new(calibration()),
            key: key.into(),
            actor: Actor {
                user: None,
                client: "test".into(),
            },
        };
        c.start(request)
    }

    #[test]
    fn a_mat_loaded_at_start_is_unseen_until_reloaded_then_a_cut_completes() {
        let cutter = shared();
        let sim = sim(&Scenario::default());
        let (mut worker, history, _dir) = worker(&cutter, SimLink { sim: sim.clone() });
        steps(&mut worker, 9);
        assert_eq!(lock(&cutter).mat(), Mat::Unseen);
        load_fresh(&mut worker, &sim);
        assert_eq!(lock(&cutter).mat(), Mat::Fresh(1));

        start(&cutter, Kind::Main, "k1").unwrap();
        let Step::Ran(execution) = worker.step() else {
            panic!("expected the cut to run")
        };
        assert_eq!(lock(&cutter).mat(), Mat::Used(1));
        assert_eq!(history.try_recv().unwrap().ended, Ended::Finished);
        assert!(sim.received() > 1000, "the stream reached the cutter");

        // The record was written before the cut and completed after it.
        let (_, records) = Journal::open(_dir.path()).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].execution, execution);
        assert_eq!(records[0].outcome.as_ref().unwrap()["ended"], "finished");
    }

    #[test]
    fn unplugging_an_idle_cutter_and_plugging_it_back() {
        let cutter = shared();
        let sim = sim(&Scenario::default());
        let (mut worker, _history, _dir) = worker(&cutter, SimLink { sim: sim.clone() });
        load_fresh(&mut worker, &sim);
        sim.power_off();
        assert_eq!(worker.step(), Step::Disconnected);
        assert!(!lock(&cutter).view().online);
        sim.power_on();
        sim.reconnect();
        steps(&mut worker, 2);
        let view = lock(&cutter).view();
        assert!(view.online);
        assert_eq!(view.mat, Mat::Unseen, "a reconnect discards the evidence");
    }

    #[test]
    fn power_lost_mid_cut_is_a_fault_until_the_mat_is_unloaded() {
        let cutter = shared();
        let total = calibration().encoded.bytes.len();
        let scenario = Scenario {
            triggers: vec![Trigger::PowerOff { at: total / 2 }],
            ..Scenario::default()
        };
        let sim = sim(&scenario);
        let (mut worker, history, _dir) = worker(&cutter, SimLink { sim: sim.clone() });
        load_fresh(&mut worker, &sim);
        start(&cutter, Kind::Test, "k1").unwrap();
        worker.step();
        assert_eq!(lock(&cutter).view().activity, "check-cutter");
        assert!(matches!(
            history.try_recv().unwrap().ended,
            Ended::Interrupted { .. }
        ));
        sim.power_on();
        sim.reconnect();
        steps(&mut worker, 4);
        assert_eq!(lock(&cutter).view().activity, "check-cutter");
        sim.unload();
        steps(&mut worker, 3);
        assert_eq!(lock(&cutter).view().activity, "idle");
    }

    /// A transport that blocks once `at` bytes have been written, until the test lets it go:
    /// makes "Stop in the middle of a cut" deterministic on a virtual clock.
    struct Gate {
        inner: SimTransport,
        at: usize,
        written: usize,
        reached: Option<mpsc::Sender<()>>,
        release: Option<mpsc::Receiver<()>>,
    }

    impl Transport for Gate {
        fn write(&mut self, data: &[u8], timeout: Duration) -> Result<usize, TransportError> {
            if self.written >= self.at
                && let (Some(reached), Some(release)) = (self.reached.take(), self.release.take())
            {
                reached.send(()).unwrap();
                release.recv().unwrap();
            }
            let n = self.inner.write(data, timeout)?;
            self.written += n;
            Ok(n)
        }
        fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, TransportError> {
            self.inner.read(buf, timeout)
        }
        fn clear_halt(&mut self) -> Result<(), TransportError> {
            self.inner.clear_halt()
        }
    }

    struct GateLink {
        sim: Simulator,
        gate: Option<Gate>,
    }

    impl Link for GateLink {
        type Transport = Gate;
        type Clock = Simulator;
        fn open(&mut self) -> Result<Gate, String> {
            self.gate.take().ok_or_else(|| "opened once".into())
        }
        fn clock(&self) -> &Simulator {
            &self.sim
        }
    }

    #[test]
    fn stopping_mid_cut_is_confirmed_by_the_mat_settling_and_ends_the_turn() {
        let cutter = shared();
        let sim = sim(&Scenario::default());
        let (reached_tx, reached_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let gate = Gate {
            inner: sim.transport(),
            at: usize::MAX,
            written: 0,
            reached: Some(reached_tx),
            release: Some(release_rx),
        };
        let (mut worker, history, _dir) = worker(
            &cutter,
            GateLink {
                sim: sim.clone(),
                gate: Some(gate),
            },
        );
        load_fresh(&mut worker, &sim);
        // Arm the gate halfway through the stream, now that polling is done.
        let half = calibration().encoded.bytes.len() / 2;
        worker.transport.as_mut().unwrap().at = worker.transport.as_ref().unwrap().written + half;
        let Answer::Started { execution, .. } = start(&cutter, Kind::Main, "k1").unwrap() else {
            panic!()
        };

        let runner = std::thread::spawn(move || {
            let step = worker.step();
            (worker, step)
        });
        reached_rx.recv().unwrap();
        let epoch = lock(&cutter).action_epoch();
        assert_eq!(
            lock(&cutter).stop().unwrap(),
            Answer::Stopping {
                execution: execution.clone()
            }
        );
        assert!(
            lock(&cutter).action_epoch() > epoch,
            "Stop advances the epoch"
        );
        release_tx.send(()).unwrap();
        let (mut worker, step) = runner.join().unwrap();
        assert_eq!(step, Step::Ran(execution));
        assert_eq!(lock(&cutter).view().activity, "confirming-stop");

        steps(&mut worker, 9);
        let c = lock(&cutter);
        assert_eq!(c.view().activity, "idle");
        assert!(c.view().turn.is_none(), "a stopped main cut ends its turn");
        assert_eq!(c.mat(), Mat::Used(1));
        assert_eq!(history.try_recv().unwrap().ended, Ended::Stopped);
    }

    #[test]
    fn a_start_accepted_just_before_a_disconnect_never_runs() {
        let cutter = shared();
        let sim = sim(&Scenario::default());
        let (mut worker, history, _dir) = worker(&cutter, SimLink { sim: sim.clone() });
        load_fresh(&mut worker, &sim);
        start(&cutter, Kind::Test, "k1").unwrap();
        lock(&cutter).disconnected();
        assert!(lock(&cutter).take_pending().is_none());
        worker.step();
        assert!(matches!(
            history.try_recv().unwrap().ended,
            Ended::NotRun { .. }
        ));
    }

    #[test]
    fn two_cutters_sharing_a_journal_keep_separate_records() {
        let dir = tempfile::tempdir().unwrap();
        let (journal, _) = Journal::open(dir.path()).unwrap();
        let journal = Arc::new(journal);
        let mut executions = Vec::new();
        for id in ["practice", "cameo"] {
            let cutter: Shared = Arc::new(Mutex::new(Cutter::new(id, id, "same-boot")));
            let sim = sim(&Scenario::default());
            let mut worker = Worker::new(
                cutter.clone(),
                SimLink { sim: sim.clone() },
                journal.clone(),
                None,
            );
            load_fresh(&mut worker, &sim);
            start(&cutter, Kind::Main, "k1").unwrap();
            let Step::Ran(execution) = worker.step() else {
                panic!("the cut ran")
            };
            executions.push(execution);
        }
        assert_ne!(executions[0], executions[1]);
        let (_, records) = Journal::open(dir.path()).unwrap();
        assert_eq!(records.len(), 2);
        for record in &records {
            assert_eq!(record.outcome.as_ref().unwrap()["ended"], "finished");
        }
        let cutters: Vec<_> = records.iter().map(|r| r.cutter.as_str()).collect();
        assert!(cutters.contains(&"practice") && cutters.contains(&"cameo"));
    }
}
