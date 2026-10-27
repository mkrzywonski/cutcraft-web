//! The simulated machine: loaded media, tool position, settings, a bounded command buffer and
//! timed execution. Time is seconds since power-on; the caller decides how it advances.

use std::collections::VecDeque;

use serde::Serialize;

use crate::profile::{Media, Profile};
use crate::protocol::{Command, ESC, ETX, Parsed, Parser, Point};

/// What ESC ENQ reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Ready,
    Moving,
    NoMedia,
}

impl Status {
    pub fn reply_byte(self) -> u8 {
        match self {
            Self::Ready => b'0',
            Self::Moving => b'1',
            Self::NoMedia => b'2',
        }
    }

    pub fn from_reply_byte(byte: u8) -> Option<Self> {
        match byte {
            b'0' => Some(Self::Ready),
            b'1' => Some(Self::Moving),
            b'2' => Some(Self::NoMedia),
            _ => None,
        }
    }
}

/// One straight move the tool made, in mm (`x` across, `y` along the feed).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Segment {
    pub from: [f64; 2],
    pub to: [f64; 2],
    pub tool_down: bool,
    pub force: i32,
    pub speed: i32,
    pub start_s: f64,
    pub end_s: f64,
    /// False when power was lost part-way along it.
    pub complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FaultKind {
    BadCommand,
    OutOfRange,
    OutOfArea,
    NoMedia,
    InitializeWhileMoving,
    Disconnected,
    PowerLost,
}

/// Something the real machine would probably not do as the driver intended.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Fault {
    pub at_s: f64,
    pub kind: FaultKind,
    pub detail: String,
}

#[derive(Debug)]
enum Step {
    Line { to: Point, down: bool },
    Setting(Command),
    Firmware,
}

#[derive(Debug)]
struct Queued {
    step: Step,
    /// Buffer bytes released when this step finishes.
    bytes: usize,
}

#[derive(Debug)]
struct Active {
    from: Point,
    to: Point,
    down: bool,
    moves: bool,
    start: f64,
    end: f64,
    bytes: usize,
    /// The tool was raised or lowered at the start of this step.
    tool_change: bool,
}

/// A point-in-time view for displays.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub now_s: f64,
    pub powered: bool,
    pub media: Option<Media>,
    pub status: Status,
    /// Tool position in mm (`[x, y]`), interpolated along the current move.
    pub position_mm: [f64; 2],
    pub tool_down: bool,
    pub force: i32,
    pub speed: i32,
    pub buffered_bytes: usize,
}

#[derive(Debug)]
pub struct Machine {
    profile: Profile,
    now: f64,
    powered: bool,
    media: Option<Media>,
    position: Point,
    origin: Point,
    tool_down: bool,
    force: i32,
    speed: i32,
    blade_offset: Vec<i32>,
    setup: std::collections::BTreeMap<String, Vec<i32>>,
    setup_applied: bool,
    media_setting: Vec<i32>,
    parser: Parser,
    queue: VecDeque<Queued>,
    queued_bytes: usize,
    max_buffered: usize,
    active: Option<Active>,
    replies: Vec<u8>,
    stale_status_replies: u32,
    last_status: Status,
    /// Status says Ready until then, whatever is loaded (the observed FG transient).
    transient_ready_until: f64,
    trace: Vec<Segment>,
    faults: Vec<Fault>,
    /// When the last command finished (for the start-up delay after idling).
    last_end: f64,
    /// Direction of the last tool-down move, for the slowdown at turns.
    heading: Option<f64>,
}

impl Machine {
    pub fn new(profile: Profile) -> Self {
        Self {
            force: profile.default_force,
            speed: profile.default_speed,
            profile,
            now: 0.0,
            powered: true,
            media: None,
            position: Point { y: 0, x: 0 },
            origin: Point { y: 0, x: 0 },
            tool_down: false,
            blade_offset: Vec::new(),
            setup: Default::default(),
            setup_applied: false,
            media_setting: Vec::new(),
            parser: Parser::new(),
            queue: VecDeque::new(),
            queued_bytes: 0,
            max_buffered: 0,
            active: None,
            replies: Vec::new(),
            stale_status_replies: 0,
            last_status: Status::NoMedia,
            transient_ready_until: f64::NEG_INFINITY,
            trace: Vec::new(),
            faults: Vec::new(),
            last_end: f64::NEG_INFINITY,
            heading: None,
        }
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    pub fn now(&self) -> f64 {
        self.now
    }

    pub fn media(&self) -> Option<Media> {
        self.media
    }

    pub fn powered(&self) -> bool {
        self.powered
    }

    pub fn trace(&self) -> &[Segment] {
        &self.trace
    }

    pub fn faults(&self) -> &[Fault] {
        &self.faults
    }

    pub fn max_buffered(&self) -> usize {
        self.max_buffered
    }

    /// The last blade-offset (`FC`) and media (`FW`) arguments received.
    pub fn settings(&self) -> (&[i32], &[i32]) {
        (&self.blade_offset, &self.media_setting)
    }

    pub fn setup_settings(&self) -> &std::collections::BTreeMap<String, Vec<i32>> {
        &self.setup
    }

    /// Unexecuted bytes held by the device.
    pub fn buffered(&self) -> usize {
        self.queued_bytes + self.parser.pending_len()
    }

    /// Pressing the load button with media in place.
    pub fn load(&mut self, media: Media) {
        self.media = Some(media);
    }

    /// Pressing the unload button.
    pub fn unload(&mut self) {
        self.media = None;
    }

    /// The next `count` status replies repeat the previous one.
    pub fn set_stale_status_replies(&mut self, count: u32) {
        self.stale_status_replies = count;
    }

    pub fn fault(&mut self, kind: FaultKind, detail: impl Into<String>) {
        self.faults.push(Fault {
            at_s: self.now,
            kind,
            detail: detail.into(),
        });
    }

    pub fn status(&self) -> Status {
        let waking = self.active.as_ref().is_some_and(|a| self.now < a.start);
        if (self.now < self.transient_ready_until && !self.busy()) || waking {
            Status::Ready
        } else if self.media.is_none() {
            Status::NoMedia
        } else if self.busy() {
            Status::Moving
        } else {
            Status::Ready
        }
    }

    /// Whether commands are executing or queued, whatever the status reply says.
    pub fn busy(&self) -> bool {
        self.active.is_some() || !self.queue.is_empty()
    }

    /// Takes reply bytes produced since the last call.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    /// Accepts bytes from the start of `data` while buffer space remains, returning how many.
    /// Escape sequences are always accepted.
    pub fn accept(&mut self, data: &[u8]) -> usize {
        if !self.powered {
            return 0;
        }
        let mut accepted = 0;
        for &byte in data {
            let escape = self.parser.in_escape() || byte == ESC;
            let full = self.buffered() >= self.profile.buffer_bytes;
            if full && (!escape || self.profile.escapes_wait_when_full) {
                break;
            }
            accepted += 1;
            if let Some(parsed) = self.parser.push(byte) {
                self.handle(parsed);
            }
            self.max_buffered = self.max_buffered.max(self.buffered());
        }
        accepted
    }

    fn handle(&mut self, parsed: Parsed) {
        let (command, bytes) = match parsed {
            Parsed::Command(command, bytes) => (command, bytes),
            Parsed::Error(error, _) => {
                self.fault(FaultKind::BadCommand, error.to_string());
                return;
            }
        };
        match command {
            Command::Status => {
                let status = if self.stale_status_replies > 0 {
                    self.stale_status_replies -= 1;
                    self.last_status
                } else {
                    self.status()
                };
                self.last_status = status;
                self.replies.extend([status.reply_byte(), ETX]);
            }
            Command::Initialize => {
                if self.status() == Status::Moving {
                    self.fault(
                        FaultKind::InitializeWhileMoving,
                        "ESC EOT arrived with motion queued",
                    );
                }
                self.force = self.profile.default_force;
                self.speed = self.profile.default_speed;
                self.blade_offset.clear();
                self.media_setting.clear();
                self.setup.clear();
                self.setup_applied = false;
            }
            Command::Move(points) | Command::Draw(points) if points.is_empty() => {}
            Command::Move(points) => self.enqueue_lines(points, false, bytes),
            Command::Draw(points) => self.enqueue_lines(points, true, bytes),
            Command::Firmware => self.enqueue(Step::Firmware, bytes),
            setting => self.enqueue(Step::Setting(setting), bytes),
        }
    }

    fn enqueue(&mut self, step: Step, bytes: usize) {
        self.queued_bytes += bytes;
        self.queue.push_back(Queued { step, bytes });
    }

    fn enqueue_lines(&mut self, points: Vec<Point>, down: bool, bytes: usize) {
        let last = points.len() - 1;
        for (index, to) in points.into_iter().enumerate() {
            let share = if index == last { bytes } else { 0 };
            self.enqueue(Step::Line { to, down }, share);
        }
    }

    /// Runs the machine for `dt` seconds.
    pub fn advance(&mut self, dt: f64) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        let target = self.now + dt;
        if !self.powered {
            self.now = target;
            return;
        }
        loop {
            if let Some(active) = &self.active {
                if active.end > target {
                    self.now = target;
                    return;
                }
                self.now = active.end;
                self.finish_active();
            } else if let Some(queued) = self.queue.pop_front() {
                self.start(queued);
            } else {
                self.now = target;
                return;
            }
        }
    }

    fn start(&mut self, queued: Queued) {
        let overhead = self.profile.command_overhead_s;
        // After idling, the Cameo takes a moment to start, answering Ready meanwhile.
        let begin = if self.now - self.last_end >= self.profile.start_delay_s {
            self.now + self.profile.start_delay_s
        } else {
            self.now
        };
        let setup_delay = if matches!(&queued.step, Step::Line { .. }) && !self.setup_applied {
            self.setup_applied = true;
            self.profile.alignment_tick_s
                + if self.force >= 19 && self.setup.get("FY").is_some_and(|v| v == &[0]) {
                    self.profile.track_enhancing_s
                } else {
                    0.0
                }
        } else {
            0.0
        };
        let begin = begin + setup_delay;
        let mut active = Active {
            from: self.position,
            to: self.position,
            down: self.tool_down,
            moves: false,
            start: begin,
            end: begin + overhead,
            bytes: queued.bytes,
            tool_change: false,
        };
        match queued.step {
            Step::Setting(command) => self.apply_setting(command),
            Step::Firmware => {
                self.replies.extend(self.profile.firmware.bytes());
                self.replies.push(ETX);
                self.transient_ready_until =
                    self.now + overhead + self.profile.firmware_status_transient_s;
            }
            Step::Line { to, down } => {
                // Apply the frame when the queued move executes, after earlier settings.
                let to = Point {
                    y: to.y.saturating_add(self.origin.y),
                    x: to.x.saturating_add(self.origin.x),
                };
                if let Some(problem) = self.line_problem(to) {
                    self.fault(problem.0, problem.1);
                } else {
                    let length = self.mm_between(self.position, to);
                    // Cuts follow the speed setting; tool-up travel does not.
                    let rate = if down {
                        f64::from(self.speed) * self.profile.speed_step_mm_s
                    } else {
                        self.profile.travel_mm_s
                    };
                    let change = if down == self.tool_down {
                        0.0
                    } else {
                        self.profile.tool_change_s
                    };
                    let mut turn = 0.0;
                    if !down {
                        self.heading = None;
                    } else if length > 0.0 {
                        let ([ax, ay], [bx, by]) = (self.mm(self.position), self.mm(to));
                        let a = (by - ay).atan2(bx - ax);
                        if let Some(h) = self.heading {
                            let d = (a - h).rem_euclid(std::f64::consts::TAU);
                            turn = d.min(std::f64::consts::TAU - d) * self.profile.turn_s_per_rad;
                        }
                        self.heading = Some(a);
                    }
                    active.to = to;
                    active.down = down;
                    active.moves = true;
                    active.end = begin + overhead + change + turn + length / rate;
                    active.tool_change = down != self.tool_down;
                    self.tool_down = down;
                }
            }
        }
        self.active = Some(active);
    }

    fn line_problem(&self, to: Point) -> Option<(FaultKind, String)> {
        let Some(media) = self.media else {
            return Some((FaultKind::NoMedia, "motion with no media loaded".to_owned()));
        };
        let [x, y] = self.mm(to);
        let length = self.profile.media_length_mm(media);
        if !(0.0..=self.profile.width_mm).contains(&x) || !(0.0..=length).contains(&y) {
            return Some((
                FaultKind::OutOfArea,
                format!(
                    "move to x {x:.2} mm, y {y:.2} mm is outside 0-{:.1} x 0-{length:.1} mm",
                    self.profile.width_mm
                ),
            ));
        }
        None
    }

    fn apply_setting(&mut self, command: Command) {
        match command {
            Command::Force(value) => {
                let (low, high) = self.profile.force_range;
                if (low..=high).contains(&value) {
                    self.force = value;
                    self.setup_applied = false;
                } else {
                    self.fault(
                        FaultKind::OutOfRange,
                        format!("force {value} outside {low}-{high}"),
                    );
                }
            }
            Command::Speed(value) => {
                let (low, high) = self.profile.speed_range;
                if (low..=high).contains(&value) {
                    self.speed = value;
                } else {
                    self.fault(
                        FaultKind::OutOfRange,
                        format!("speed {value} outside {low}-{high}"),
                    );
                }
            }
            Command::Setup { code, args } => {
                if code == "FN" && args == [0] && self.profile.fn0_rebases_origin {
                    self.origin = self.position;
                    self.heading = None;
                }
                self.setup.insert(code, args);
            }
            Command::BladeOffset(args) => self.blade_offset = args,
            Command::Media(args) => self.media_setting = args,
            Command::Corner { code, args } => {
                self.setup.insert(code, args);
            }
            // Candidate settings have no modelled geometric effect until hardware probes.
            _ => {}
        }
    }

    fn finish_active(&mut self) {
        let Some(active) = self.active.take() else {
            return;
        };
        self.queued_bytes = self.queued_bytes.saturating_sub(active.bytes);
        self.last_end = active.end;
        if active.moves {
            self.record(&active, active.to, true);
            self.position = active.to;
        }
    }

    fn record(&mut self, active: &Active, to: Point, complete: bool) {
        // A tool-up step that goes nowhere is not recorded, unless it lifted the blade: the
        // lift separates two cuts.
        if active.from == to && !active.down && !active.tool_change {
            return;
        }
        self.trace.push(Segment {
            from: self.mm(active.from),
            to: self.mm(to),
            tool_down: active.down,
            force: self.force,
            speed: self.speed,
            start_s: active.start,
            end_s: self.now,
            complete,
        });
    }

    /// Cuts power: queued commands are lost and the tool stops where it is.
    pub fn power_off(&mut self) {
        if !self.powered {
            return;
        }
        if let Some(active) = self.active.take()
            && active.moves
        {
            let stop = self.interpolate(&active);
            self.record(&active, stop, false);
            self.position = stop;
        }
        let lost = self.queue.len();
        self.queue.clear();
        self.queued_bytes = 0;
        self.parser = Parser::new();
        self.replies.clear();
        self.tool_down = false;
        self.powered = false;
        self.fault(
            FaultKind::PowerLost,
            format!("power lost with {lost} queued steps"),
        );
    }

    pub fn power_on(&mut self) {
        if self.powered {
            return;
        }
        self.powered = true;
        self.force = self.profile.default_force;
        self.speed = self.profile.default_speed;
        self.blade_offset.clear();
        self.media_setting.clear();
        self.setup.clear();
        self.setup_applied = false;
    }

    fn interpolate(&self, active: &Active) -> Point {
        let span = active.end - active.start;
        let t = if span > 0.0 {
            ((self.now - active.start) / span).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let lerp = |a: i32, b: i32| a + ((f64::from(b - a)) * t).round() as i32;
        Point {
            y: lerp(active.from.y, active.to.y),
            x: lerp(active.from.x, active.to.x),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let position = match &self.active {
            Some(active) if active.moves => self.interpolate(active),
            _ => self.position,
        };
        Snapshot {
            now_s: self.now,
            powered: self.powered,
            media: self.media,
            status: self.status(),
            position_mm: self.mm(position),
            tool_down: self.tool_down,
            force: self.force,
            speed: self.speed,
            buffered_bytes: self.buffered(),
        }
    }

    fn mm(&self, point: Point) -> [f64; 2] {
        let scale = self.profile.units_per_mm;
        [f64::from(point.x) / scale, f64::from(point.y) / scale]
    }

    fn mm_between(&self, a: Point, b: Point) -> f64 {
        let [ax, ay] = self.mm(a);
        let [bx, by] = self.mm(b);
        (bx - ax).hypot(by - ay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded() -> Machine {
        let mut machine = Machine::new(Profile::cameo_1());
        machine.load(Media::Mat12x12);
        machine
    }

    #[test]
    fn draws_a_square_and_reports_moving_then_ready() {
        let mut machine = loaded();
        let data = b"!2\x03M200,200\x03D200,400,400,400,400,200,200,200\x03";
        assert_eq!(machine.accept(data), data.len());
        assert_eq!(machine.status(), Status::Moving);
        machine.advance(60.0);
        assert_eq!(machine.status(), Status::Ready);
        assert!(machine.faults().is_empty(), "{:?}", machine.faults());
        let cuts: Vec<_> = machine.trace().iter().filter(|s| s.tool_down).collect();
        assert_eq!(cuts.len(), 4);
        assert_eq!(cuts[0].from, [10.0, 10.0]);
        assert_eq!(cuts[0].to, [20.0, 10.0]);
        // 10 mm at speed 2 (18.6 mm/s) plus overhead and the tool change.
        let first = cuts[0].end_s - cuts[0].start_s;
        assert!(
            (first - (10.0 / 18.6 + 0.008 + 0.075)).abs() < 1e-9,
            "{first}"
        );
        assert_eq!(machine.buffered(), 0);
    }

    #[test]
    fn buffer_limits_what_is_accepted() {
        let mut machine = loaded();
        let mut data = Vec::new();
        for i in 0..2000 {
            data.extend(format!("D{},{}\x03", 200 + i % 100, 200).bytes());
        }
        let accepted = machine.accept(&data);
        assert!(accepted <= machine.profile().buffer_bytes);
        assert!(accepted >= machine.profile().buffer_bytes - 16);
        // Status is still answered with the buffer full.
        assert_eq!(machine.accept(b"\x1b\x05"), 2);
        assert_eq!(machine.take_replies(), vec![b'1', ETX]);
        // Past the start-up delay, executed commands free space.
        machine.advance(2.0);
        assert!(machine.accept(&data[accepted..]) > 0);
    }

    #[test]
    fn starts_a_moment_after_idling_and_says_ready_meanwhile() {
        let mut machine = loaded();
        machine.accept(b"D0,2000\x03");
        machine.advance(0.5);
        machine.accept(b"\x1b\x05");
        assert_eq!(machine.take_replies(), vec![b'0', ETX]);
        machine.advance(1.0);
        machine.accept(b"\x1b\x05");
        assert_eq!(machine.take_replies(), vec![b'1', ETX]);
        // A command queued behind another starts as soon as it finishes.
        machine.accept(b"D0,0\x03");
        machine.advance(60.0);
        let cuts: Vec<_> = machine.trace().iter().filter(|s| s.tool_down).collect();
        assert_eq!(cuts[1].start_s, cuts[0].end_s);
    }

    #[test]
    fn faults_are_recorded_not_ignored() {
        let mut machine = Machine::new(Profile::cameo_1());
        machine.accept(b"D100,100\x03");
        machine.advance(1.0);
        machine.load(Media::Mat12x12);
        machine.accept(b"FX40\x03!0\x03D100,7000\x03TB50\x03");
        machine.advance(5.0);
        let kinds: Vec<_> = machine.faults().iter().map(|f| f.kind).collect();
        assert_eq!(
            kinds,
            vec![
                FaultKind::NoMedia,
                FaultKind::BadCommand,
                FaultKind::OutOfRange,
                FaultKind::OutOfRange,
                FaultKind::OutOfArea,
            ]
        );
        assert!(machine.trace().is_empty());
    }

    #[test]
    fn firmware_reply_follows_queued_commands() {
        let mut machine = loaded();
        machine.accept(b"D2000,0\x03FG\x03");
        assert!(machine.take_replies().is_empty());
        machine.advance(30.0);
        assert_eq!(machine.take_replies(), b"CAMEO V1.20\x03");
    }

    #[test]
    fn power_off_stops_mid_move_and_drops_the_queue() {
        let mut machine = loaded();
        machine.accept(b"!1\x03D0,2000\x03D2000,2000\x03");
        // 1 s start-up delay, then 9.3 mm/s.
        machine.advance(6.5);
        machine.power_off();
        assert_eq!(machine.buffered(), 0);
        let last = machine.trace().last().cloned();
        let last = last.expect("a partial segment");
        assert!(!last.complete);
        assert!(last.to[0] > 40.0 && last.to[0] < 60.0, "{:?}", last.to);
        assert_eq!(machine.accept(b"\x1b\x05"), 0);
    }

    #[test]
    fn stale_status_repeats_the_previous_reply() {
        let mut machine = loaded();
        machine.accept(b"\x1b\x05");
        machine.set_stale_status_replies(1);
        machine.accept(b"D0,2000\x03\x1b\x05\x1b\x05");
        assert_eq!(
            machine.take_replies(),
            vec![b'0', ETX, b'0', ETX, b'1', ETX]
        );
    }

    #[test]
    fn status_says_ready_for_a_moment_after_firmware_even_with_nothing_loaded() {
        let mut machine = Machine::new(Profile::cameo_1());
        machine.accept(b"FG\x03");
        machine.advance(0.01);
        let mut replies = Vec::new();
        for _ in 0..4 {
            machine.accept(b"\x1b\x05");
            replies.extend(machine.take_replies().into_iter().filter(|&b| b != ETX));
            machine.advance(0.5);
        }
        assert_eq!(replies, b"CAMEO V1.20\x30\x30\x30\x32");
    }
}
