//! Seeing the mat (spec §8.1): what the cutter's status replies show about the mat, and which
//! loads the Pi actually saw happen.
//!
//! The Cameo only answers ready / moving / no media. An unloaded Cameo answers Ready for about a
//! second after a firmware query (measured on the desktop 2026-10-18 and on the Pi 2026-10-24),
//! so one reply proves nothing. The observer believes "no mat" after two No-media replies in a
//! row, "loaded" after Ready without a break for [`SETTLE_S`], and counts a load as *seen* only
//! when "no mat" was believed first with no gap in between. Anything else is Unseen, never fresh.

use serde::Serialize;

/// Ready without a break for this long means a mat is loaded: CutCraft's driver uses the same
/// window before it sends anything.
pub const SETTLE_S: f64 = cutcraft_device::driver::SETTLE_S;
/// No successful reply for longer than this breaks the evidence (spec §8.1).
pub const GAP_S: f64 = 2.0;
/// No-media replies in a row before "no mat" is believed.
const NO_MEDIA_REPLIES: u32 = 2;

/// One status query's result, as the observer sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reading {
    Ready,
    Moving,
    NoMedia,
    /// No usable answer (a timeout or a garbled reply). The connection is still there.
    Failed,
}

/// What the Pi knows about the mat on one cutter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "state", content = "generation")]
pub enum Mat {
    /// No mat loaded.
    None,
    /// Its load was seen; this generation hasn't had a main cut.
    Fresh(u64),
    /// Its load was seen, and it has been used (a main cut, an ended turn, a fault).
    Used(u64),
    /// Loaded, but its load wasn't seen (loaded at start-up, after a reconnect or a gap).
    Unseen,
}

/// A change the observer reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Belief {
    /// "No mat" became believed.
    NoMat,
    /// "Loaded" became believed (fresh or not: see [`Observer::mat`]).
    Loaded,
}

#[derive(Debug, Clone)]
pub struct Observer {
    mat: Mat,
    /// "No mat" is believed and nothing has broken the chain since, so the next believed load is
    /// a seen one.
    chain: bool,
    no_media: u32,
    ready_since: Option<f64>,
    /// "Loaded" is currently believed.
    loaded: bool,
    last_reply_s: Option<f64>,
    generations: u64,
}

impl Default for Observer {
    fn default() -> Self {
        Self::new()
    }
}

impl Observer {
    /// After start-up or a (re)connection: nothing seen yet.
    pub fn new() -> Self {
        Self {
            mat: Mat::Unseen,
            chain: false,
            no_media: 0,
            ready_since: None,
            loaded: false,
            last_reply_s: None,
            generations: 0,
        }
    }

    pub fn mat(&self) -> Mat {
        self.mat
    }

    /// Whether "loaded" is believed now (Ready has held for [`SETTLE_S`]).
    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// A new connection: all evidence is gone, and the generation counter continues so a
    /// generation is never reused.
    pub fn reconnected(&mut self) {
        let generations = self.generations;
        *self = Self::new();
        self.generations = generations;
    }

    /// The mat's generation is used up (a main cut, an ended turn, a fault). A mat that wasn't
    /// seen loading has nothing to retire and stays Unseen.
    pub fn retire(&mut self) {
        if let Mat::Fresh(generation) = self.mat {
            self.mat = Mat::Used(generation);
        }
    }

    /// Something happened that may have moved the material (the driver reported it gone, or a
    /// cut ended uncertainly): the mat can't be trusted until it is seen loading again.
    pub fn distrust(&mut self) {
        self.retire();
        self.loaded = false;
        self.ready_since = None;
    }

    /// The driver found no mat although one was believed: whatever is there now wasn't seen
    /// loading.
    pub fn forget(&mut self) {
        if self.mat != Mat::None {
            self.mat = Mat::Unseen;
        }
        self.chain = false;
        self.loaded = false;
        self.ready_since = None;
    }

    /// A person says the loaded mat, whose loading wasn't seen, has fresh material (spec §8.2):
    /// it then counts as freshly loaded, with a generation of its own. Only for a settled mat that
    /// wasn't seen loading; returns its generation.
    pub fn confirm_fresh(&mut self) -> Option<u64> {
        if self.mat != Mat::Unseen || !self.loaded {
            return None;
        }
        self.generations += 1;
        self.mat = Mat::Fresh(self.generations);
        Some(self.generations)
    }

    /// The driver sent commands, so the earlier replies say nothing about now: "loaded" must be
    /// believed afresh (used to confirm an orderly stop, spec §8.2).
    pub fn restart_settling(&mut self) {
        self.loaded = false;
        self.ready_since = None;
        self.no_media = 0;
    }

    /// Time passed without polling (a cut ran) or a query failed: breaks the chain when it was
    /// longer than [`GAP_S`].
    fn check_gap(&mut self, at_s: f64) {
        if let Some(last) = self.last_reply_s
            && at_s - last > GAP_S
        {
            self.chain = false;
            self.no_media = 0;
            self.ready_since = None;
            if let Mat::Fresh(_) = self.mat {
                self.mat = Mat::Unseen;
            }
        }
    }

    /// The polling stopped on purpose (the driver owns the cutter while it cuts). The time it
    /// takes is not a gap: the worker records what the cut did instead.
    pub fn resume_after(&mut self, at_s: f64) {
        self.last_reply_s = Some(at_s);
        self.restart_settling();
    }

    /// One status reply at `at_s` (monotonic seconds). Returns a newly formed belief, if any.
    pub fn observe(&mut self, at_s: f64, reading: Reading) -> Option<Belief> {
        self.check_gap(at_s);
        if reading == Reading::Failed {
            return None;
        }
        self.last_reply_s = Some(at_s);
        match reading {
            Reading::NoMedia => {
                self.ready_since = None;
                self.loaded = false;
                self.no_media += 1;
                if self.no_media >= NO_MEDIA_REPLIES && (!self.chain || self.mat != Mat::None) {
                    self.mat = Mat::None;
                    self.chain = true;
                    return Some(Belief::NoMat);
                }
                None
            }
            Reading::Moving => {
                self.no_media = 0;
                self.ready_since = None;
                None
            }
            Reading::Ready => {
                self.no_media = 0;
                let since = *self.ready_since.get_or_insert(at_s);
                if self.loaded || at_s - since < SETTLE_S {
                    return None;
                }
                self.loaded = true;
                match self.mat {
                    Mat::None if self.chain => {
                        self.generations += 1;
                        self.mat = Mat::Fresh(self.generations);
                    }
                    Mat::None => self.mat = Mat::Unseen,
                    Mat::Fresh(_) | Mat::Used(_) | Mat::Unseen => {}
                }
                self.chain = false;
                Some(Belief::Loaded)
            }
            Reading::Failed => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds replies every 0.5 s from `start`, returning the time after the last.
    fn feed(observer: &mut Observer, start: f64, readings: &[Reading]) -> f64 {
        let mut at = start;
        for &reading in readings {
            observer.observe(at, reading);
            at += 0.5;
        }
        at
    }

    fn repeat(reading: Reading, n: usize) -> Vec<Reading> {
        vec![reading; n]
    }

    #[test]
    fn only_a_settled_unseen_mat_can_be_confirmed_fresh() {
        let mut o = Observer::new();
        assert_eq!(o.confirm_fresh(), None, "nothing loaded yet");
        feed(&mut o, 0.0, &repeat(Reading::Ready, 10));
        assert_eq!(o.mat(), Mat::Unseen);
        let generation = o.confirm_fresh().unwrap();
        assert_eq!(o.mat(), Mat::Fresh(generation));
        assert_eq!(o.confirm_fresh(), None, "already fresh");
        o.retire();
        assert_eq!(o.confirm_fresh(), None, "a used mat can't be called fresh");
    }

    #[test]
    fn a_mat_loaded_at_start_up_is_unseen() {
        let mut o = Observer::new();
        feed(&mut o, 0.0, &repeat(Reading::Ready, 10));
        assert_eq!(o.mat(), Mat::Unseen);
        assert!(o.loaded());
    }

    #[test]
    fn an_observed_unload_then_load_is_fresh() {
        let mut o = Observer::new();
        let at = feed(&mut o, 0.0, &repeat(Reading::Ready, 10));
        let at = feed(&mut o, at, &repeat(Reading::NoMedia, 4));
        assert_eq!(o.mat(), Mat::None);
        let at = feed(&mut o, at, &repeat(Reading::Ready, 6));
        assert_eq!(o.mat(), Mat::None, "not before SETTLE_S of Ready");
        feed(&mut o, at, &repeat(Reading::Ready, 2));
        assert_eq!(o.mat(), Mat::Fresh(1));
    }

    #[test]
    fn one_no_media_reply_is_not_believed() {
        let mut o = Observer::new();
        let at = feed(&mut o, 0.0, &[Reading::NoMedia]);
        feed(&mut o, at, &repeat(Reading::Ready, 10));
        assert_eq!(o.mat(), Mat::Unseen);
    }

    /// The measured case: after a firmware query an unloaded Cameo says Ready for about a second,
    /// then No media.
    #[test]
    fn the_false_ready_after_a_firmware_query_mints_nothing() {
        let mut o = Observer::new();
        let mut readings = repeat(Reading::Ready, 3);
        readings.extend(repeat(Reading::NoMedia, 10));
        feed(&mut o, 0.0, &readings);
        assert_eq!(o.mat(), Mat::None);
        assert!(!o.loaded());
    }

    #[test]
    fn a_gap_turns_fresh_into_unseen_and_breaks_a_half_chain() {
        let mut o = Observer::new();
        let at = feed(&mut o, 0.0, &repeat(Reading::NoMedia, 2));
        let at = feed(&mut o, at, &repeat(Reading::Ready, 8));
        assert_eq!(o.mat(), Mat::Fresh(1));
        // 3 s of silence.
        o.observe(at + 3.0, Reading::Ready);
        assert_eq!(o.mat(), Mat::Unseen);

        // None believed, then a gap, then a load: not seen.
        let mut o = Observer::new();
        let at = feed(&mut o, 0.0, &repeat(Reading::NoMedia, 2));
        feed(&mut o, at + 5.0, &repeat(Reading::Ready, 8));
        assert_eq!(o.mat(), Mat::Unseen);
    }

    #[test]
    fn failed_queries_count_towards_a_gap() {
        let mut o = Observer::new();
        let mut at = feed(&mut o, 0.0, &repeat(Reading::NoMedia, 2));
        for _ in 0..6 {
            o.observe(at, Reading::Failed);
            at += 0.5;
        }
        feed(&mut o, at, &repeat(Reading::Ready, 8));
        assert_eq!(o.mat(), Mat::Unseen);
    }

    #[test]
    fn used_stays_used_until_a_new_seen_load() {
        let mut o = Observer::new();
        let at = feed(&mut o, 0.0, &repeat(Reading::NoMedia, 2));
        let at = feed(&mut o, at, &repeat(Reading::Ready, 8));
        o.retire();
        assert_eq!(o.mat(), Mat::Used(1));
        let at = feed(&mut o, at + 3.0, &repeat(Reading::Ready, 8));
        assert_eq!(o.mat(), Mat::Used(1), "a gap doesn't refresh a used mat");
        let at = feed(&mut o, at, &repeat(Reading::NoMedia, 2));
        feed(&mut o, at, &repeat(Reading::Ready, 8));
        assert_eq!(o.mat(), Mat::Fresh(2));
    }

    #[test]
    fn generations_are_never_reused_across_reconnects() {
        let mut o = Observer::new();
        let at = feed(&mut o, 0.0, &repeat(Reading::NoMedia, 2));
        feed(&mut o, at, &repeat(Reading::Ready, 8));
        assert_eq!(o.mat(), Mat::Fresh(1));
        o.reconnected();
        assert_eq!(o.mat(), Mat::Unseen);
        let at = feed(&mut o, 100.0, &repeat(Reading::NoMedia, 2));
        feed(&mut o, at, &repeat(Reading::Ready, 8));
        assert_eq!(o.mat(), Mat::Fresh(2));
    }

    #[test]
    fn a_reload_faster_than_polling_is_invisible() {
        // The documented limitation (spec §8.1): no No-media reply, so the same mat.
        let mut o = Observer::new();
        let at = feed(&mut o, 0.0, &repeat(Reading::NoMedia, 2));
        let at = feed(&mut o, at, &repeat(Reading::Ready, 8));
        o.retire();
        feed(&mut o, at, &repeat(Reading::Ready, 8));
        assert_eq!(o.mat(), Mat::Used(1));
    }

    #[test]
    fn settling_restarts_after_a_cut() {
        let mut o = Observer::new();
        let at = feed(&mut o, 0.0, &repeat(Reading::NoMedia, 2));
        let at = feed(&mut o, at, &repeat(Reading::Ready, 8));
        assert!(o.loaded());
        o.resume_after(at + 60.0);
        assert!(!o.loaded());
        assert_eq!(o.mat(), Mat::Fresh(1), "a cut is not a gap");
        let belief = (0..8)
            .filter_map(|i| o.observe(at + 60.5 + 0.5 * f64::from(i), Reading::Ready))
            .last();
        assert_eq!(belief, Some(Belief::Loaded));
    }
}
