//! What the simulator believes about the machine, and where each belief comes from.
//!
//! Spec §7.4: every behaviour records its source. Until the read-only probe and the hardware
//! tests replace them, almost all of these are hypotheses from public descriptions of GP-GL and
//! the Cameo (spec §7.3). A passing simulation of a hypothesis is not validated behaviour.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// From public descriptions; not yet checked on the machine.
    Hypothesis,
    /// Seen in a recorded capture from the real Cameo.
    Observed,
    /// Timed or measured in a hardware test.
    Measured,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Assumption {
    pub id: &'static str,
    pub statement: &'static str,
    pub source: Source,
}

const fn hypothesis(id: &'static str, statement: &'static str) -> Assumption {
    Assumption {
        id,
        statement,
        source: Source::Hypothesis,
    }
}

/// The original Cameo's behaviour as currently modelled.
pub const CAMEO_1_ASSUMPTIONS: &[Assumption] = &[
    hypothesis(
        "fn0-origin",
        "off-paper scan 2026-10-23 matches FN0 rebasing to the current pen position; conservatively model that translation, recovery semantics unverified",
    ),
    hypothesis(
        "setup-sequence",
        "candidate setup commands are parsed and retained; production header awaits hardware probes",
    ),
    hypothesis(
        "blade-offset-units",
        "FC is retained; command scaling and physical blade offset remain unmeasured",
    ),
    hypothesis(
        "track-enhancing",
        "FY0 with force >=19 may add configured pre-motion delay; default zero, awaiting hardware probes",
    ),
    hypothesis(
        "alignment-tick",
        "an optional pre-motion delay represents alignment; default zero, no tick geometry is invented",
    ),
    Assumption {
        id: "usb-identity",
        statement: "USB 0x0B4D:0x1121, interface 0 Printer class, bulk OUT 0x01, bulk IN 0x82, \
                    interrupt IN 0x83",
        source: Source::Observed,
    },
    hypothesis(
        "framing",
        "text commands end with ETX; ESC EOT and ESC ENQ are unframed",
    ),
    hypothesis(
        "escape-immediate",
        "escape sequences are handled on arrival, even with the buffer full or mid-command",
    ),
    Assumption {
        id: "status-ready",
        statement: "an idle Cameo answers ESC ENQ with '0' then ETX (probe, 2026-10-17)",
        source: Source::Observed,
    },
    Assumption {
        id: "status-no-media",
        statement: "ESC ENQ answers '2' then ETX after operator-confirmed unload; loaded and stationary answers '0' (2026-10-18)",
        source: Source::Observed,
    },
    Assumption {
        id: "status-replies",
        statement: "ESC ENQ answers '1' while moving and '0' whenever it is not, including mid-job \
                    while waiting for commands and for about 1 s before starting commands received \
                    after idling (timing plot, 2026-10-20)",
        source: Source::Observed,
    },
    Assumption {
        id: "firmware-status-transient",
        statement: "after FG an unloaded Cameo reported '0' for about 1 s, then '2' from 1.5 s (2026-10-18); modelled as '0' for 1.25 s after FG, whatever is loaded",
        source: Source::Observed,
    },
    Assumption {
        id: "firmware-reply",
        statement: "FG answers \"CAMEO V1.20\" then ETX, after about 0.7 s (probe, 2026-10-17)",
        source: Source::Observed,
    },
    hypothesis(
        "firmware-order",
        "the FG reply comes in order after queued commands",
    ),
    hypothesis(
        "initialize",
        "ESC EOT resets force, speed and offset settings and does not clear queued motion",
    ),
    hypothesis(
        "units",
        "20 device units per mm; coordinates are y (feed) first",
    ),
    hypothesis("force-range", "FX accepts 1-33"),
    hypothesis("speed-range", "! accepts 1-10"),
    Assumption {
        id: "speed-rate",
        statement: "speed n cuts at n x 9.3 mm/s and slows 0.005 s per radian of turn; tool-up \
                    travel runs at about 100 mm/s at any speed; no separate acceleration (raw \
                    timing blocks with FC0, 2026-10-23: lettering-like block exact, others up to \
                    15% slower than real, chosen so pacing never runs ahead)",
        source: Source::Observed,
    },
    Assumption {
        id: "tool-change",
        statement: "a lift and the next lowering take 0.15 s; each point adds 8 ms (timing plot, \
                    2026-10-20; refit with FC0 2026-10-23)",
        source: Source::Observed,
    },
    Assumption {
        id: "stop-drains",
        statement: "after the driver stops sending, the Cameo finishes the commands it holds: a \
                    Stop at speed 1 drew about 3 s more (35.6 mm line, 2026-10-20)",
        source: Source::Observed,
    },
    hypothesis(
        "buffer",
        "the device buffers 4096 bytes of unexecuted commands",
    ),
    hypothesis(
        "cutting-area",
        "x from 0 to 304 mm across; y from 0 to the loaded length",
    ),
    hypothesis(
        "media-lengths",
        "12x12 mat 304.8 mm, 12x24 mat 609.6 mm, roll 3000 mm",
    ),
    hypothesis(
        "no-media-motion",
        "motion commands with no media loaded are not executed",
    ),
    hypothesis(
        "out-of-area",
        "a move outside the cutting area is not executed",
    ),
];

/// Loaded media, chosen with the button on the Cameo 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Media {
    Mat12x12,
    Mat12x24,
    Roll,
}

impl Media {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "mat12" | "mat-12x12" => Some(Self::Mat12x12),
            "mat24" | "mat-12x24" => Some(Self::Mat12x24),
            "roll" => Some(Self::Roll),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Profile {
    pub name: &'static str,
    pub vendor_id: u16,
    pub product_id: u16,
    pub firmware: String,
    pub units_per_mm: f64,
    pub width_mm: f64,
    pub mat_12x12_mm: f64,
    pub mat_12x24_mm: f64,
    pub roll_mm: f64,
    pub force_range: (i32, i32),
    pub speed_range: (i32, i32),
    pub default_force: i32,
    pub default_speed: i32,
    /// Cutting (tool down) mm/s per speed step.
    pub speed_step_mm_s: f64,
    /// Tool-up travel, whatever the speed setting.
    pub travel_mm_s: f64,
    /// Raising or lowering the tool (a lift and the next lowering is twice this).
    pub tool_change_s: f64,
    /// Each command (each point), beside its motion.
    pub command_overhead_s: f64,
    /// Slowing for a change of direction while cutting, per radian.
    pub turn_s_per_rad: f64,
    /// After idling this long, a command starts this much later (status Ready meanwhile).
    pub start_delay_s: f64,
    /// After FG, status says Ready this long whatever is loaded.
    pub firmware_status_transient_s: f64,
    /// Unverified setup timing experiments. Zero by default; these are not hardware evidence.
    pub track_enhancing_s: f64,
    /// Conservative interpretation of the observed off-paper FN0 probe, not a full protocol
    /// specification. No assumptions about power-cycle/load recovery follow from it.
    pub fn0_rebases_origin: bool,
    pub alignment_tick_s: f64,
    pub buffer_bytes: usize,
    /// With the buffer full, escape sequences (status) wait for space too.
    pub escapes_wait_when_full: bool,
    pub assumptions: &'static [Assumption],
}

impl Profile {
    /// The original Cameo as currently modelled.
    pub fn cameo_1() -> Self {
        Self {
            name: "Silhouette Cameo (original)",
            vendor_id: 0x0b4d,
            product_id: 0x1121,
            firmware: "CAMEO V1.20".to_owned(),
            units_per_mm: 20.0,
            width_mm: 304.0,
            mat_12x12_mm: 304.8,
            mat_12x24_mm: 609.6,
            roll_mm: 3000.0,
            force_range: (1, 33),
            speed_range: (1, 10),
            default_force: 10,
            default_speed: 5,
            speed_step_mm_s: 9.3,
            travel_mm_s: 100.0,
            tool_change_s: 0.075,
            command_overhead_s: 0.008,
            turn_s_per_rad: 0.005,
            start_delay_s: 1.0,
            firmware_status_transient_s: 1.25,
            track_enhancing_s: 0.0,
            fn0_rebases_origin: true,
            alignment_tick_s: 0.0,
            buffer_bytes: 4096,
            escapes_wait_when_full: false,
            assumptions: CAMEO_1_ASSUMPTIONS,
        }
    }

    pub fn media_length_mm(&self, media: Media) -> f64 {
        match media {
            Media::Mat12x12 => self.mat_12x12_mm,
            Media::Mat12x24 => self.mat_12x24_mm,
            Media::Roll => self.roll_mm,
        }
    }
}
