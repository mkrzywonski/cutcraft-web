//! The virtual mat: what the tool did, as a report and an SVG drawing.

use std::collections::HashSet;
use std::fmt::Write as _;

use serde::Serialize;

use crate::machine::{Fault, Machine, Segment, Status};
use crate::profile::{Assumption, Media};

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub model: &'static str,
    pub firmware: String,
    pub media: Option<Media>,
    pub final_status: Status,
    pub duration_s: f64,
    pub cut_mm: f64,
    pub travel_mm: f64,
    pub cut_segments: usize,
    /// Cut segments retracing one already cut (either direction), at device resolution.
    pub repeated_cut_segments: usize,
    pub incomplete_segments: usize,
    pub max_buffered_bytes: usize,
    pub faults: Vec<Fault>,
    /// What the simulation rests on; hypotheses are not validated behaviour.
    pub assumptions: Vec<Assumption>,
}

impl Report {
    pub fn of(machine: &Machine) -> Self {
        let trace = machine.trace();
        let length = |s: &Segment| (s.to[0] - s.from[0]).hypot(s.to[1] - s.from[1]);
        let resolution = machine.profile().units_per_mm;
        let mut seen = HashSet::new();
        let mut repeated = 0;
        for segment in trace.iter().filter(|s| s.tool_down) {
            let quantize = |p: [f64; 2]| {
                (
                    (p[0] * resolution).round() as i64,
                    (p[1] * resolution).round() as i64,
                )
            };
            let (a, b) = (quantize(segment.from), quantize(segment.to));
            if !seen.insert((a.min(b), a.max(b))) {
                repeated += 1;
            }
        }
        Self {
            model: machine.profile().name,
            firmware: machine.profile().firmware.clone(),
            media: machine.media(),
            final_status: machine.status(),
            duration_s: trace.last().map_or(0.0, |s| s.end_s),
            cut_mm: trace
                .iter()
                .filter(|s| s.tool_down)
                .map(length)
                .sum::<f64>()
                + 0.0,
            travel_mm: trace
                .iter()
                .filter(|s| !s.tool_down)
                .map(length)
                .sum::<f64>()
                + 0.0,
            cut_segments: trace.iter().filter(|s| s.tool_down).count(),
            repeated_cut_segments: repeated,
            incomplete_segments: trace.iter().filter(|s| !s.complete).count(),
            max_buffered_bytes: machine.max_buffered(),
            faults: machine.faults().to_vec(),
            assumptions: machine.profile().assumptions.to_vec(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SvgOptions {
    /// Draw tool-up travel as dashed lines.
    pub travel: bool,
}

/// Draws the cutting area at 1 SVG unit per mm: x across, y along the feed (top goes in first).
pub fn svg(machine: &Machine, options: SvgOptions) -> String {
    let profile = machine.profile();
    let trace = machine.trace();
    let extent = trace
        .iter()
        .flat_map(|s| [s.from[1], s.to[1]])
        .fold(0.0_f64, f64::max);
    let height = machine
        .media()
        .map_or(extent.max(profile.mat_12x12_mm), |media| {
            profile.media_length_mm(media)
        });
    let width = profile.width_mm;
    let mut out = String::new();
    let _ = writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}mm" height="{height}mm" viewBox="0 0 {width} {height}">"#
    );
    let _ = writeln!(
        out,
        "<title>CutCraft simulated {} — virtual mat (hypothesised machine behaviour)</title>",
        profile.name
    );
    let _ = writeln!(
        out,
        r##"<rect width="{width}" height="{height}" fill="#f4f4f0" stroke="#999" stroke-width="0.3"/>"##
    );
    let inch = 25.4;
    let mut grid = String::new();
    let mut x = inch;
    while x < width {
        let _ = write!(grid, "M{x:.3} 0V{height:.3}");
        x += inch;
    }
    let mut y = inch;
    while y < height {
        let _ = write!(grid, "M0 {y:.3}H{width:.3}");
        y += inch;
    }
    let _ = writeln!(
        out,
        r##"<path d="{grid}" fill="none" stroke="#d8d8d0" stroke-width="0.2"/>"##
    );
    if options.travel {
        let d = path_data(trace.iter().filter(|s| !s.tool_down));
        let _ = writeln!(
            out,
            r##"<path d="{d}" fill="none" stroke="#7a8aa0" stroke-width="0.2" stroke-dasharray="1 1"/>"##
        );
    }
    let d = path_data(trace.iter().filter(|s| s.tool_down));
    let _ = writeln!(
        out,
        r##"<path d="{d}" fill="none" stroke="#c62828" stroke-width="0.3" stroke-linejoin="round"/>"##
    );
    out.push_str("</svg>\n");
    out
}

fn path_data<'a>(segments: impl Iterator<Item = &'a Segment>) -> String {
    let mut d = String::new();
    let mut last: Option<[f64; 2]> = None;
    for segment in segments {
        if last != Some(segment.from) {
            let _ = write!(d, "M{:.3} {:.3}", segment.from[0], segment.from[1]);
        }
        let _ = write!(d, "L{:.3} {:.3}", segment.to[0], segment.to[1]);
        last = Some(segment.to);
    }
    d
}
