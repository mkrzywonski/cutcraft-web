//! Update drain admission (spec §14). The updater closes this gate before waiting for idle
//! cutters. Preparation and disk work happen before admission; Stop never takes this lock.

use std::sync::Mutex;

#[derive(Default)]
pub struct UpdateGate {
    waiting: Mutex<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpdateWaiting;

impl UpdateGate {
    /// Begins draining. Already accepted executions finish; queued turns remain queued.
    /// The running program retains its own assets until the updater actually restarts it.
    pub fn begin(&self) {
        *self.waiting.lock().unwrap_or_else(|p| p.into_inner()) = true;
    }

    pub fn waiting(&self) -> bool {
        *self.waiting.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Linearizes admission with beginning an update. The closure only changes cutter state;
    /// it must not prepare plans, touch disk, await, or run the driver. Replays precede this gate.
    pub fn admit_start<T>(&self, accept: impl FnOnce() -> T) -> Result<T, UpdateWaiting> {
        let waiting = self.waiting.lock().unwrap_or_else(|p| p.into_inner());
        if *waiting {
            return Err(UpdateWaiting);
        }
        Ok(accept())
    }

    /// Idle is read while admission is closed, so no new execution can invalidate readiness.
    /// Call this again immediately before a graceful restart. Paused/stopping cuts are not idle.
    pub fn ready(&self, all_idle: impl FnOnce() -> bool) -> bool {
        let waiting = self.waiting.lock().unwrap_or_else(|p| p.into_inner());
        *waiting && all_idle()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier, mpsc};

    #[test]
    fn draining_blocks_new_starts_without_claiming_an_active_cut_is_idle() {
        let gate = UpdateGate::default();
        assert!(!gate.ready(|| true));
        assert_eq!(gate.admit_start(|| "accepted"), Ok("accepted"));
        gate.begin();
        assert!(gate.waiting());
        assert_eq!(
            gate.admit_start(|| panic!("must not accept")),
            Err(UpdateWaiting)
        );
        assert!(!gate.ready(|| false));
        assert!(gate.ready(|| true));
        gate.begin();
        assert_eq!(gate.admit_start(|| ()), Err(UpdateWaiting));
    }

    #[test]
    fn beginning_an_update_serializes_with_an_in_flight_admission() {
        let gate = Arc::new(UpdateGate::default());
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let starter = {
            let (gate, entered, release) = (gate.clone(), entered.clone(), release.clone());
            std::thread::spawn(move || {
                gate.admit_start(|| {
                    entered.wait();
                    release.wait();
                })
            })
        };
        entered.wait();
        assert!(
            gate.waiting.try_lock().is_err(),
            "admission stays locked through acceptance"
        );
        let (done, completed) = mpsc::channel();
        let updater = {
            let gate = gate.clone();
            std::thread::spawn(move || {
                gate.begin();
                done.send(()).unwrap();
            })
        };
        assert!(completed.try_recv().is_err());
        release.wait();
        starter.join().unwrap().unwrap();
        updater.join().unwrap();
        completed.recv().unwrap();
        assert!(gate.waiting());
        assert_eq!(gate.admit_start(|| ()), Err(UpdateWaiting));
    }
}
