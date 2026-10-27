use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn original_white_art_has_correct_size_and_is_not_automatically_excluded() {
    let report = cutcraft_import::inspect_file(&fixture("one-inch.svg")).unwrap();
    assert!((report.pages[0].width_mm - 25.4).abs() < 1e-9);
    assert!((report.pages[0].height_mm - 25.4).abs() < 1e-9);
    assert!(!report.cut_ready);
    assert!(report.fonts.is_empty());
    let paths: Vec<_> = report.objects.iter().filter(|o| o.kind == "path").collect();
    assert_eq!(paths.len(), 1);
    assert!(paths[0].excluded.is_none());
    assert_eq!(
        report
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect::<Vec<_>>(),
        ["preparation.pending"]
    );
    let value = serde_json::to_value(&report).unwrap();
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/one-inch.json")).unwrap();
    assert_eq!(value, golden);
}

#[test]
fn missing_fonts_block_and_hidden_text_does_not_enter_font_selection() {
    let report = cutcraft_import::inspect_file(&fixture("font-review.svg")).unwrap();
    assert_eq!(report.fonts.len(), 1);
    let font = &report.fonts[0];
    assert_eq!(font.family, "CutCraft Deliberately Missing Fixture Font");
    let json = serde_json::to_value(font).unwrap();
    assert_eq!(json["status"], "missing");
    // The fallback has ABC, but that does not resolve the requested missing family.
    assert_eq!(font.missing_glyphs, 0);
    assert!(!font.source_ids.is_empty());
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "font.unresolved" && d.source_ids == font.source_ids)
    );
    assert!(report.objects.iter().any(|o| o.excluded == Some("hidden")));
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/font-review.json")).unwrap();
    assert_eq!(serde_json::to_value(&report).unwrap(), golden);
}

#[test]
fn appearances_are_flagged_with_affected_source_ids() {
    let report = cutcraft_import::inspect_file(&fixture("appearance-review.svg")).unwrap();
    for code in [
        "appearance.material_choice",
        "appearance.transparency",
        "stroke.needs_role",
        "clip.needs_application",
    ] {
        let diagnostic = report
            .diagnostics
            .iter()
            .find(|d| d.code == code)
            .unwrap_or_else(|| panic!("missing {code}: {:?}", report.diagnostics));
        assert!(!diagnostic.source_ids.is_empty());
        assert!(
            diagnostic
                .source_ids
                .iter()
                .all(|id| report.objects.iter().any(|o| &o.source_id == id))
        );
    }
}

#[test]
fn repeated_inspection_is_identical_and_source_is_unchanged() {
    let path = fixture("font-review.svg");
    let before = std::fs::read(&path).unwrap();
    let first = serde_json::to_vec(&cutcraft_import::inspect_file(&path).unwrap()).unwrap();
    let second = serde_json::to_vec(&cutcraft_import::inspect_file(&path).unwrap()).unwrap();
    assert_eq!(first, second);
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
fn linked_svg_is_resolved_from_the_source_folder_and_still_needs_fingerprints() {
    let path = fixture("linked.svg");
    let asset = fixture("one-inch.svg");
    let before = std::fs::read(&asset).unwrap();
    let report = cutcraft_import::inspect_file(&path).unwrap();
    assert!(report.objects.iter().any(|o| o.kind == "path"));
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code == "import.warning")
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code == "link.fingerprint_pending")
    );
    assert_eq!(std::fs::read(asset).unwrap(), before);
}

#[test]
fn prepared_one_inch_fixture_matches_its_golden_report() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/one-inch.svg");
    let design = cutcraft_import::prepare_file(&path, &cutcraft_job::Recipe::new()).unwrap();
    assert!(design.cut_ready, "{:?}", design.diagnostics);
    let value = serde_json::to_value(&design).unwrap();
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/one-inch.prepared.json")).unwrap();
    assert_eq!(value, golden);
}

#[test]
fn an_eps_opens_at_its_physical_size_and_prepares_its_shapes() {
    let path = fixture("one-inch.eps");
    let report = cutcraft_import::inspect_file(&path).unwrap();
    assert_eq!(report.format, "eps");
    assert!((report.pages[0].width_mm - 25.4).abs() < 1e-6);
    assert!((report.pages[0].height_mm - 25.4).abs() < 1e-6);
    let prepared =
        cutcraft_import::prepare_file(&path, &cutcraft_import::job::Recipe::new()).unwrap();
    assert!(prepared.cut_ready, "{:?}", prepared.diagnostics);
    assert_eq!(prepared.pieces.len(), 1);
    // The square is inset 7.2 pt (2.54 mm) on every side of the one-inch page.
    let outer = &prepared.pieces[0].outer;
    let xs = std::iter::once(outer.start[0])
        .chain(outer.segments.iter().flatten().map(|p| p[0]))
        .fold((f64::MAX, f64::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
    assert!(
        (xs.0 - 2.54).abs() < 1e-3 && (xs.1 - 22.86).abs() < 1e-3,
        "{xs:?}"
    );
}

#[test]
fn an_eps_that_draws_nothing_has_nothing_to_cut() {
    let bytes =
        b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 72 72\n%%EndComments\nshowpage\n%%EOF\n";
    let prepared = cutcraft_import::prepare_bytes(
        Path::new("empty.eps"),
        bytes,
        &cutcraft_import::job::Recipe::new(),
    )
    .unwrap();
    assert!(!prepared.cut_ready);
    assert!(
        prepared
            .diagnostics
            .iter()
            .any(|d| d.code == "preparation.empty"),
        "{:?}",
        prepared.diagnostics
    );
}
