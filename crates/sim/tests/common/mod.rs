//! Shared fixtures: an authored design for the simulator tests.

use cutcraft_job::{
    Colour, Contour, CutLine, MachineProfile, Piece, PreparedDesign, Project, Recipe,
};

pub fn colour(hex: &str) -> Colour {
    Colour {
        hex: hex.into(),
        swatch: None,
    }
}

pub fn square(x: f64, y: f64, size: f64) -> Contour {
    let line = |a: [f64; 2], b: [f64; 2]| [a, b, b];
    Contour {
        start: [x, y],
        segments: vec![
            line([x, y], [x + size, y]),
            line([x + size, y], [x + size, y + size]),
            line([x + size, y + size], [x, y + size]),
            line([x, y + size], [x, y]),
        ],
        closed: true,
    }
}

/// A circle from four cubics, so flattening is exercised.
pub fn circle(cx: f64, cy: f64, r: f64) -> Contour {
    let k = 0.552_284_75 * r;
    Contour {
        start: [cx + r, cy],
        segments: vec![
            [[cx + r, cy + k], [cx + k, cy + r], [cx, cy + r]],
            [[cx - k, cy + r], [cx - r, cy + k], [cx - r, cy]],
            [[cx - r, cy - k], [cx - k, cy - r], [cx, cy - r]],
            [[cx + k, cy - r], [cx + r, cy - k], [cx + r, cy]],
        ],
        closed: true,
    }
}

/// An asymmetric design, so a missed mirror or rotation shows: a ring with a round hole and an
/// island, a separate small square, and an open cut line.
pub fn project() -> Project {
    let piece = |id: &str, outer, holes| Piece {
        id: id.into(),
        layer: "source/0".into(),
        colour: colour("#202020"),
        source_ids: vec![],
        outer,
        holes,
    };
    let design = PreparedDesign {
        schema_version: 1,
        stage: "preparation".into(),
        cut_ready: true,
        engine_revision: "test".into(),
        source_sha256: "0".repeat(64),
        recipe: Recipe::new(),
        fonts: vec![],
        page: None,
        pieces: vec![
            piece(
                "piece/0",
                square(0.0, 0.0, 40.0),
                vec![circle(20.0, 20.0, 12.0)],
            ),
            piece("piece/1", circle(20.0, 20.0, 5.0), vec![]),
            piece("piece/2", square(50.0, 30.0, 8.0), vec![]),
        ],
        cut_lines: vec![CutLine {
            id: "line/0".into(),
            layer: "source/0".into(),
            colour: colour("#202020"),
            source_ids: vec![],
            contour: Contour {
                start: [2.0, 45.0],
                segments: vec![[[10.0, 50.0], [30.0, 40.0], [38.0, 45.0]]],
                closed: false,
            },
        }],
        changes: vec![],
        pending: vec![],
        diagnostics: vec![],
        stroked_fills: 0,
    };
    Project::new(design, MachineProfile::cameo_1())
}
