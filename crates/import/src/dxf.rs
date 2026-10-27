//! DXF policy on top of the pinned VectorCraft CAD importer: explicit units, model space,
//! and no silent projection of non-planar geometry. Geometry decoding stays in VectorCraft.
use std::path::Path;

use cutcraft_job::{Diagnostic, DxfUnit};
use serde_json::{Value, json};

pub(crate) fn is_dxf(path: &Path) -> bool {
    path.extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("dxf"))
}

pub(crate) fn options(
    bytes: &[u8],
    unit: Option<DxfUnit>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Value, String> {
    let info = vectorcraft_cad::info(bytes).map_err(|e| format!("Cannot open this DXF: {e}"))?;
    let unitless = info.units == "Unitless";
    if !unitless && unit.is_some() {
        return Err("This DXF already declares its units; the recipe's unitless-DXF answer no longer applies.".into());
    }
    if unitless && unit.is_none() {
        diagnostics.push(Diagnostic::blocking(
            "dxf.units_needed",
            "Choose the DXF drawing's units before cutting.",
            vec![],
        ));
    }
    if non_planar(bytes) {
        diagnostics.push(Diagnostic::blocking("dxf.non_planar", "This DXF contains geometry outside the XY plane or with 3D thickness. Export a flat 2D model-space DXF from the CAD application; CutCraft will not silently project it for cutting.", vec![]));
    }
    let chosen = unitless.then(|| unit.unwrap_or(DxfUnit::Mm).symbol());
    diagnostics.push(Diagnostic::info("dxf.imported", format!(
        "Opened DXF model space at 1:1 in {}. Unfilled CAD paths are single centreline cuts; plotting lineweights do not create extra outlines.{}",
        chosen.unwrap_or(&info.units),
        if info.layouts.len() > 1 { " Paper layouts are not imported; export their cut paths into model space." } else { "" },
    ), vec![]));
    Ok(
        json!({"layout":"Model", "fit":false, "center":true, "mergeLayers":false,
        "scaleLineweights":false, "unit":chosen, "scale":chosen.map(|_| 1.0)}),
    )
}

/// Break CAD block links through the pinned engine before inspection, so their contents,
/// fonts and transforms go through the same preparation as ordinary paths. Bound the fully
/// expanded graph before cloning any instances (a small DXF can reference huge repeated art).
pub(crate) fn expand_blocks(doc: &mut vectorcraft_doc::Document) -> Result<usize, String> {
    use std::collections::BTreeMap;
    use vectorcraft_doc::{Document, Node, NodeKind};
    const LIMIT: usize = 100_000;
    fn cost(
        node: &Node,
        doc: &Document,
        cache: &mut BTreeMap<String, usize>,
        depth: usize,
    ) -> Result<usize, String> {
        if depth > 64 {
            return Err("DXF block nesting is too deep; expand it in the CAD application.".into());
        }
        let mut total = 1usize;
        if let NodeKind::SymbolInstance { symbol, .. } = &node.kind {
            total += if let Some(count) = cache.get(symbol) {
                *count
            } else {
                let definition = doc.symbols.iter().find(|s| &s.name == symbol).ok_or(
                    "A DXF block definition is missing; expand blocks in the CAD application.",
                )?;
                let count = cost(&definition.art, doc, cache, depth + 1)?;
                cache.insert(symbol.clone(), count);
                count
            };
        } else if let Some(children) = node.children() {
            for child in children {
                total = total.saturating_add(cost(child, doc, cache, depth + 1)?);
                if total > LIMIT {
                    break;
                }
            }
        }
        if total > LIMIT {
            return Err("Expanded DXF blocks exceed 100,000 objects. Simplify or split the drawing in the CAD application.".into());
        }
        Ok(total)
    }
    let mut ids = Vec::new();
    doc.walk(|n| {
        if matches!(n.kind, NodeKind::SymbolInstance { .. }) {
            ids.push(n.id.0);
        }
    });
    if ids.is_empty() {
        return Ok(0);
    }
    let mut cache = BTreeMap::new();
    let mut count = 0usize;
    for layer in &doc.layers {
        count = count.saturating_add(cost(layer, doc, &mut cache, 0)?);
        if count > LIMIT {
            return Err("Expanded DXF blocks exceed 100,000 objects. Simplify or split the drawing in the CAD application.".into());
        }
    }
    let mut session = vectorcraft_engine::Session::new();
    session.add_document(doc.clone(), None);
    let mut expanded = 0;
    for _ in 0..=8 {
        let mut ids = Vec::new();
        session
            .active()
            .ok_or("DXF block expansion lost its document.")?
            .doc
            .walk(|n| {
                if matches!(n.kind, NodeKind::SymbolInstance { .. }) {
                    ids.push(n.id.0);
                }
            });
        if ids.is_empty() {
            *doc = session
                .active()
                .ok_or("DXF block expansion lost its document.")?
                .doc
                .as_ref()
                .clone();
            return Ok(expanded);
        }
        expanded += ids.len();
        session
            .execute("symbol.breakLink", &json!({"ids":ids}))
            .map_err(|e| format!("Cannot expand DXF blocks: {e}"))?;
    }
    Err(
        "DXF blocks remain nested after expansion; flatten the drawing in the CAD application."
            .into(),
    )
}

/// Read only numeric planarity tags, not entity geometry. Autodesk's DXF reference:
/// https://help.autodesk.com/cloudhelp/2024/ENU/AutoCAD-DXF/files/GUID-3F0380A5-1C15-464D-BC66-2C5F094BCFB9.htm
/// Codes 30–38 carry Z/elevation, 39 thickness, and 210/220 the extrusion normal's X/Y.
/// Paper-space entities are not part of the selected model-space drawing.
fn non_planar(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.trim_start_matches('\u{feff}').lines();
    let mut section = "";
    let mut awaiting_section = false;
    let mut bad = false;
    let mut paper = false;
    while let (Some(code), Some(value)) = (lines.next(), lines.next()) {
        let (Ok(code), value) = (code.trim().parse::<u16>(), value.trim()) else {
            continue;
        };
        if code == 0 {
            if bad && !paper {
                return true;
            }
            bad = false;
            paper = false;
            if value == "SECTION" {
                awaiting_section = true;
            }
            if value == "ENDSEC" {
                section = "";
            }
        }
        if awaiting_section && code == 2 {
            section = value;
            awaiting_section = false;
        }
        if !matches!(section, "ENTITIES" | "BLOCKS") {
            continue;
        }
        if (code == 67 && value == "1") || (code == 410 && !value.eq_ignore_ascii_case("Model")) {
            paper = true;
        }
        if matches!(code, 30..=39 | 210 | 220)
            && let Ok(n) = value.parse::<f64>()
        {
            bad |= !n.is_finite() || n.abs() > 1e-9;
        }
    }
    bad && !paper
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SourceView, inspect_and_prepare, prepare_bytes};
    use cutcraft_job::{Decision, Recipe};

    // Original fixtures authored as DXF group-code/value records; no private design assets.
    fn drawing(units: Option<u8>, entities: &str) -> Vec<u8> {
        let units = units.map_or(String::new(), |u| format!("9\n$INSUNITS\n70\n{u}\n"));
        format!("0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1027\n{units}0\nENDSEC\n0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n").into_bytes()
    }
    fn square(size: f64) -> String {
        // A heavy CAD plotting lineweight must still produce one blade centreline.
        format!(
            "0\nLWPOLYLINE\n8\nCut paths\n62\n1\n370\n100\n90\n4\n70\n1\n10\n0\n20\n0\n10\n{size}\n20\n0\n10\n{size}\n20\n{size}\n10\n0\n20\n{size}\n"
        )
    }
    fn recipe(unit: DxfUnit) -> Recipe {
        Recipe {
            decisions: vec![Decision::DxfUnits { unit }],
            ..Recipe::new()
        }
    }
    fn extent(design: &cutcraft_job::PreparedDesign) -> [f64; 2] {
        let points: Vec<_> = design
            .cut_lines
            .iter()
            .flat_map(|l| {
                std::iter::once(l.contour.start).chain(l.contour.segments.iter().map(|s| s[2]))
            })
            .collect();
        let mut b = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        for [x, y] in points {
            b[0] = b[0].min(x);
            b[1] = b[1].min(y);
            b[2] = b[2].max(x);
            b[3] = b[3].max(y);
        }
        [b[2] - b[0], b[3] - b[1]]
    }

    #[test]
    fn millimetres_and_inches_preserve_size_and_cad_lineweights_are_single_cuts() {
        for (unit, size) in [(4, 25.4), (1, 1.0), (5, 2.54)] {
            let bytes = drawing(Some(unit), &square(size));
            let (inspection, design) =
                inspect_and_prepare(Path::new("authored.DXF"), &bytes, &Recipe::new()).unwrap();
            assert_eq!(inspection.format, "dxf");
            assert!(
                design.cut_ready,
                "{:?} {:?}",
                design.pending, design.diagnostics
            );
            assert_eq!(design.cut_lines.len(), 1);
            assert!(design.pieces.is_empty());
            assert!(design.cut_lines[0].contour.closed);
            for d in extent(&design) {
                assert!((d - 25.4).abs() < 1e-5, "{d}");
            }
        }
    }

    #[test]
    fn unitless_drawings_ask_once_and_the_source_renderer_uses_the_chosen_units() {
        let bytes = drawing(None, &square(1.0));
        let pending = prepare_bytes(Path::new("authored.dxf"), &bytes, &Recipe::new()).unwrap();
        assert!(!pending.cut_ready);
        assert_eq!(
            pending
                .pending
                .iter()
                .map(|p| p.decision.as_str())
                .collect::<Vec<_>>(),
            ["dxf_units"]
        );
        // The question says how big the one-unit square comes out with each answer.
        let message = &pending.pending[0].message;
        for size in [
            "mm: 0.04 × 0.04 in (1 × 1 mm)",
            "in: 1.00 × 1.00 in (25.4 × 25.4 mm)",
        ] {
            assert!(message.contains(size), "{message}");
        }
        let chosen =
            prepare_bytes(Path::new("authored.dxf"), &bytes, &recipe(DxfUnit::In)).unwrap();
        assert!(
            chosen.cut_ready,
            "{:?} {:?}",
            chosen.pending, chosen.diagnostics
        );
        for d in extent(&chosen) {
            assert!((d - 25.4).abs() < 1e-5);
        }
        let stated = drawing(Some(1), &square(1.0));
        let stated_design =
            prepare_bytes(Path::new("stated.dxf"), &stated, &Recipe::new()).unwrap();
        let a = SourceView::new(Path::new("authored.dxf"), &bytes, &chosen).unwrap();
        let b = SourceView::new(Path::new("stated.dxf"), &stated, &stated_design).unwrap();
        assert_eq!(
            a.render([0., 0., 25.4, 25.4], 128, 128).unwrap(),
            b.render([0., 0., 25.4, 25.4], 128, 128).unwrap()
        );
        let mut duplicate = recipe(DxfUnit::Mm);
        duplicate
            .decisions
            .push(Decision::DxfUnits { unit: DxfUnit::In });
        assert!(prepare_bytes(Path::new("authored.dxf"), &bytes, &duplicate).is_err());
        assert!(prepare_bytes(Path::new("stated.dxf"), &stated, &recipe(DxfUnit::Mm)).is_err());
        assert!(prepare_bytes(Path::new("not-dxf.svg"), b"<svg/>", &recipe(DxfUnit::Mm)).is_err());
    }

    #[test]
    fn circles_arcs_and_bulges_remain_curved_cut_lines() {
        let entities = "0\nCIRCLE\n8\nCurves\n62\n1\n10\n0\n20\n0\n40\n12.7\n0\nARC\n8\nCurves\n62\n2\n10\n50\n20\n0\n40\n10\n50\n0\n51\n180\n0\nLWPOLYLINE\n8\nCurves\n62\n3\n90\n2\n70\n0\n10\n70\n20\n0\n42\n1\n10\n90\n20\n0\n";
        let design = prepare_bytes(
            Path::new("curves.dxf"),
            &drawing(Some(4), entities),
            &Recipe::new(),
        )
        .unwrap();
        assert!(
            design.cut_ready,
            "{:?} {:?}",
            design.pending, design.diagnostics
        );
        assert_eq!(design.cut_lines.len(), 3);
        assert_eq!(
            design.cut_lines.iter().filter(|l| l.contour.closed).count(),
            1
        );
        for line in design.cut_lines {
            assert!(line.contour.segments.len() >= 2);
        }
    }

    #[test]
    fn hidden_layers_are_left_out_and_visible_layers_keep_their_names() {
        let tables = "0\nSECTION\n2\nTABLES\n0\nTABLE\n2\nLAYER\n70\n2\n0\nLAYER\n2\nVisible\n70\n0\n62\n1\n0\nLAYER\n2\nHidden\n70\n0\n62\n-1\n0\nENDTAB\n0\nENDSEC\n";
        let entities = "0\nLINE\n8\nVisible\n10\n0\n20\n0\n11\n25.4\n21\n25.4\n0\nLINE\n8\nHidden\n10\n0\n20\n25.4\n11\n25.4\n21\n0\n";
        let bytes = String::from_utf8(drawing(Some(4), entities))
            .unwrap()
            .replace(
                "0\nSECTION\n2\nENTITIES\n",
                &format!("{tables}0\nSECTION\n2\nENTITIES\n"),
            );
        let design =
            prepare_bytes(Path::new("layers.dxf"), bytes.as_bytes(), &Recipe::new()).unwrap();
        assert!(
            design.cut_ready,
            "{:?} {:?}",
            design.pending, design.diagnostics
        );
        assert_eq!(design.cut_lines.len(), 1);
        assert_eq!(design.cut_lines[0].layer, "Visible");
    }

    #[test]
    fn inserted_blocks_preserve_rotation_and_nonuniform_scale() {
        let block = format!(
            "0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n2\nMotif\n10\n0\n20\n0\n{}0\nENDBLK\n0\nENDSEC\n",
            square(10.0)
        );
        let entities = "0\nINSERT\n8\nPlaced\n2\nMotif\n10\n50\n20\n60\n41\n2\n42\n3\n50\n90\n";
        let bytes = String::from_utf8(drawing(Some(4), entities))
            .unwrap()
            .replace(
                "0\nSECTION\n2\nENTITIES\n",
                &format!("{block}0\nSECTION\n2\nENTITIES\n"),
            );
        let design =
            prepare_bytes(Path::new("blocks.dxf"), bytes.as_bytes(), &Recipe::new()).unwrap();
        assert!(
            design.cut_ready,
            "{:?} {:?}",
            design.pending, design.diagnostics
        );
        assert_eq!(design.cut_lines.len(), 1);
        let [w, h] = extent(&design);
        assert!(
            (w - 30.0).abs() < 1e-5 && (h - 20.0).abs() < 1e-5,
            "{w} {h}"
        );
    }

    #[test]
    fn cubic_splines_keep_their_curve_shape() {
        use kurbo::{BezPath, Shape};
        let spline = "0\nSPLINE\n8\nCurves\n62\n1\n70\n8\n71\n3\n72\n8\n73\n4\n40\n0\n40\n0\n40\n0\n40\n0\n40\n1\n40\n1\n40\n1\n40\n1\n10\n0\n20\n0\n10\n0\n20\n25.4\n10\n25.4\n20\n25.4\n10\n25.4\n20\n0\n";
        let design = prepare_bytes(
            Path::new("spline.dxf"),
            &drawing(Some(4), spline),
            &Recipe::new(),
        )
        .unwrap();
        assert!(
            design.cut_ready,
            "{:?} {:?}",
            design.pending, design.diagnostics
        );
        assert_eq!(design.cut_lines.len(), 1);
        let c = &design.cut_lines[0].contour;
        assert!(!c.closed);
        let mut path = BezPath::new();
        path.move_to((c.start[0], c.start[1]));
        for [a, b, e] in &c.segments {
            path.curve_to((a[0], a[1]), (b[0], b[1]), (e[0], e[1]));
        }
        let b = path.bounding_box();
        assert!(
            (b.width() - 25.4).abs() < 0.01 && (b.height() - 19.05).abs() < 0.01,
            "{b:?}"
        );
    }

    #[test]
    fn repeated_blocks_are_bounded_before_expansion() {
        let line = "0\nLINE\n8\nCut\n10\n0\n20\n0\n11\n25.4\n21\n25.4\n";
        let block = format!(
            "0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n2\nLarge\n10\n0\n20\n0\n{}0\nENDBLK\n0\nENDSEC\n",
            line.repeat(4_000)
        );
        let insert = "0\nINSERT\n8\nCut\n2\nLarge\n10\n0\n20\n0\n";
        let bytes = String::from_utf8(drawing(Some(4), &insert.repeat(30)))
            .unwrap()
            .replace(
                "0\nSECTION\n2\nENTITIES\n",
                &format!("{block}0\nSECTION\n2\nENTITIES\n"),
            );
        let error = prepare_bytes(
            Path::new("large-blocks.dxf"),
            bytes.as_bytes(),
            &Recipe::new(),
        )
        .unwrap_err();
        assert!(error.contains("100,000"), "{error}");
    }

    #[test]
    fn incomplete_and_non_planar_imports_cannot_be_cut_and_binary_is_rejected() {
        let unsupported = format!("{}0\n3DSOLID\n8\nUnsupported\n", square(25.4));
        let incomplete = prepare_bytes(
            Path::new("partial.dxf"),
            &drawing(Some(4), &unsupported),
            &Recipe::new(),
        )
        .unwrap();
        assert!(!incomplete.cut_ready);
        assert!(
            incomplete
                .diagnostics
                .iter()
                .any(|d| d.code == "import.warning")
        );
        let zline = "0\nLINE\n10\n0\n20\n0\n30\n1\n11\n25.4\n21\n25.4\n31\n1\n";
        let z = prepare_bytes(
            Path::new("nonplanar.dxf"),
            &drawing(Some(4), zline),
            &Recipe::new(),
        )
        .unwrap();
        assert!(!z.cut_ready);
        assert!(z.diagnostics.iter().any(|d| d.code == "dxf.non_planar"));
        let error = prepare_bytes(
            Path::new("binary.dxf"),
            b"AutoCAD Binary DXF\r\n\x1a\0",
            &Recipe::new(),
        )
        .unwrap_err();
        assert!(error.contains("ASCII"), "{error}");
        assert!(prepare_bytes(Path::new("damaged.dxf"), b"not a drawing", &Recipe::new()).is_err());
    }
}
