//! An original test fixture, shared with the server's tests: a 40 mm square as CutCraft would
//! save and plan it. No private designs are involved.

use std::path::Path;

use cutcraft_job::{
    Colour, Contour, MachineProfile, Piece, PreparedDesign, Project, ProjectFile, Recipe,
};
use sha2::{Digest, Sha256};

use crate::validate::PLANNER;

/// The embedded design.
pub const SVG: &[u8] = b"<svg xmlns='http://www.w3.org/2000/svg' width='40mm' height='40mm'>\
    <rect width='40' height='40'/></svg>";

/// An original fixture: a 40 mm square, as CutCraft would save and plan it.
pub fn upload() -> (Vec<u8>, Vec<u8>) {
    let square = Contour {
        start: [0.0, 0.0],
        segments: vec![
            [[40.0, 0.0], [40.0, 0.0], [40.0, 0.0]],
            [[40.0, 40.0], [40.0, 40.0], [40.0, 40.0]],
            [[0.0, 40.0], [0.0, 40.0], [0.0, 40.0]],
            [[0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
        ],
        closed: true,
    };
    let prepared = PreparedDesign {
        schema_version: 1,
        stage: "preparation".into(),
        cut_ready: true,
        engine_revision: "test".into(),
        source_sha256: format!("{:x}", Sha256::digest(SVG)),
        recipe: Recipe::new(),
        fonts: vec![],
        page: None,
        pieces: vec![Piece {
            id: "piece/0".into(),
            layer: "source/0".into(),
            colour: Colour {
                hex: "#000000".into(),
                swatch: None,
            },
            source_ids: vec![],
            outer: square,
            holes: vec![],
        }],
        cut_lines: vec![],
        changes: vec![],
        pending: vec![],
        diagnostics: vec![],
        stroked_fills: 0,
    };
    let project = Project::new(prepared, MachineProfile::cameo_1());
    let plan = cutcraft_path::plan_sheet(&project, &project.sheets[0].id).unwrap();
    let file = ProjectFile::new(project, Path::new("square.svg"), SVG, PLANNER);
    (
        serde_json::to_vec(&file).unwrap(),
        serde_json::to_vec(&plan).unwrap(),
    )
}
