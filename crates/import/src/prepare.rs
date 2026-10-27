//! Make cut-ready (spec §5.2): convert an imported source and a recipe of decisions into a
//! prepared design. Each run starts from the original bytes, so the same source and recipe give
//! the same result. Only conversions verified by tests are performed; everything else remains a
//! blocking diagnostic naming the affected source objects.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use cutcraft_job::{
    Change, Colour, Contour, CutLine, Decision, Diagnostic, FontUse, ImportRoute, Page,
    PendingDecision, Piece, PreparedDesign, Recipe, Severity, StrokeRole, TraceColours,
    TraceSettings, prepared::PREPARED_SCHEMA_VERSION, prepared::RECIPE_SCHEMA_VERSION,
};
use kurbo::{BezPath, CubicBez, ParamCurve, Point, Rect, Shape};
use sha2::{Digest, Sha256};
use vectorcraft_color::{Color, Paint};
use vectorcraft_doc::{
    AppearanceItem, Node, NodeKind, StrokeLayer,
    geom::{FillRule, PathData, SubPath},
};
use vectorcraft_pathops::{BoolOp, DEFAULT_PRECISION, area, try_boolean, try_normalize, unite_all};

use crate::inspect::{
    ENGINE_REVISION, FontStatus, Imported, Inspection, MM_PER_PT, import_route, read_source,
};
use crate::inspect::{embedded_fonts, font_key};
use crate::picture;

/// Strokes this thin or thinner are cut along their centre by default (spec §5.2 step 3).
const HAIRLINE_MM: f64 = 0.1;
/// A page-matching background candidate may differ from the page by this much on each side.
const BACKGROUND_TOLERANCE_MM: f64 = 0.5;
/// Content within this distance outside the page edge still counts as on the page.
const PAGE_EDGE_TOLERANCE_PT: f64 = 0.01;
/// SVG art reaching further than this past its page uses the artwork's bounds as the page.
const PAST_PAGE_FIT_MM: f64 = 0.5;
/// A part smaller than this (pt², about 0.01 mm²) may vanish in a union; it cannot be cut.
const SPECK_PT2: f64 = 0.01 / (MM_PER_PT * MM_PER_PT);
/// Pieces smaller than this (mm²) are reported as hard to cut and weed.
const TINY_PIECE_MM2: f64 = 1.0;
/// Warnings asked as questions that they can accept and cut anyway. An accepted warning that no
/// longer occurs (after another answer) is harmless and stays in the recipe.
const ACCEPTABLE_WARNINGS: [&str; 3] = ["piece.tiny", "piece.thin", "duplicate.near"];
/// Half the width (mm) below which a piece is reported as thin: insetting each side by this
/// leaves nothing of it.
const THIN_PIECE_MM: f64 = 0.15;
/// Outlines whose bounds agree within this (mm) and lengths within 2% nearly coincide.
const NEAR_DUPLICATE_MM: f64 = 0.2;
/// Output coordinates are rounded to this many mm so reports compare across runs.
const OUTPUT_QUANTUM_MM: f64 = 1e-6;
/// Duplicate cut detection compares coordinates rounded to this many points.
const DUPLICATE_QUANTUM_PT: f64 = 1e-4;

/// Inspection codes that preparation resolves itself, rather than carrying over as blockers.
const RESOLVED_HERE: [&str; 11] = [
    "preparation.pending",
    "image.needs_conversion",
    "import.appearance_differs",
    "clip.needs_application",
    "page.choose",
    "size.confirm",
    "dxf.units_needed",
    "stroke.needs_role",
    "text.needs_outlines",
    "font.unresolved",
    // Outlined fonts are fingerprinted by the font file used; see `resolve_fonts`.
    "font.environment_unverified",
];

/// Inspection codes for appearances that v1 cuts only as a solid colour or leaves out
/// (spec §5.2): each affected object becomes a question.
const APPEARANCE_CODES: [&str; 4] = [
    "appearance.transparency",
    "appearance.effects",
    "appearance.paint_effects",
    "appearance.material_choice",
];

/// Prepare one local source with a recipe, without modifying it or writing any files.
pub fn prepare_file(path: &Path, recipe: &Recipe) -> Result<PreparedDesign, String> {
    prepare_bytes(path, &read_source(path)?, recipe)
}

/// Inspect and prepare one local source from a single read and import.
pub fn open_file(path: &Path, recipe: &Recipe) -> Result<(Inspection, PreparedDesign), String> {
    let bytes = read_source(path)?;
    let (inspection, prepared) = inspect_and_prepare(path, &bytes, recipe)?;
    Ok((inspection, prepared))
}

/// Prepare a design from bytes already read (for example a project's embedded copy).
pub fn prepare_bytes(path: &Path, bytes: &[u8], recipe: &Recipe) -> Result<PreparedDesign, String> {
    Ok(inspect_and_prepare(path, bytes, recipe)?.1)
}

/// The inspection always describes the default (editable-layer) import; preparation uses the
/// route the recipe chose when the editable layers differ from the page.
pub fn inspect_and_prepare(
    path: &Path,
    bytes: &[u8],
    recipe: &Recipe,
) -> Result<(Inspection, PreparedDesign), String> {
    let answers = Answers::new(recipe)?;
    if answers.dxf_unit.is_some() && !crate::dxf::is_dxf(path) {
        return Err("The recipe chooses DXF units, but this is not a DXF drawing.".into());
    }
    if picture::is_picture(path) {
        return prepare_picture(path, bytes, recipe, &answers);
    }
    if answers.picture_size.is_some()
        || answers.trace.is_some()
        || answers.picture_frame.is_some()
        || answers.trace_photo
    {
        return Err(
            "The recipe sets a picture's size or tracing, but this design is not a picture file."
                .into(),
        );
    }
    let imported = import_route(path, bytes, true, false, answers.dxf_unit)?;
    let differs = imported.inspection.diagnostics.iter().any(|d| {
        matches!(
            d.code.as_str(),
            "import.appearance_differs" | "import.editing_text_omitted"
        )
    });
    // Illustrator's own layers were read (not the PDF part's): the page appearance is another route.
    let editing_layers = vectorcraft_pdf::has_illustrator_data(bytes, None)
        && !imported
            .inspection
            .diagnostics
            .iter()
            .any(|d| d.code == "import.layers_from_pdf");
    if answers.route.is_some() && !editing_layers {
        return Err("The recipe chooses how to open the file's layers, but this design has no separate editable layers.".into());
    }
    let embedded = embedded_fonts(bytes);
    let from_file = !answers.fonts_from_file.is_empty();
    if from_file {
        for (family, style) in &answers.fonts_from_file {
            if !imported
                .inspection
                .fonts
                .iter()
                .any(|f| (&f.family, &f.style) == (family, style))
            {
                return Err(format!(
                    "The recipe uses the file's letter shapes for {family} {style}, but this design doesn't ask about that font. Installed fonts may have changed since it was answered; clear the answers and answer again."
                ));
            }
        }
        // Checked after stale answers, so a stale recipe is reported as stale. The file's shapes
        // come from its PDF part, where every font's text is outlined: each font must be one the
        // file carries, or some text would get a stand-in's shapes.
        for font in &imported.inspection.fonts {
            if !shapes_available(font, &embedded) {
                return Err(format!(
                    "This design also uses {} {}, which isn't saved inside the file, so CutCraft can't cut all its letters from the file's own shapes. Turn that text into shapes in your editor, or install the font and open the design again.",
                    font.family, font.style
                ));
            }
        }
        if answers.route == Some(ImportRoute::EditingData) {
            return Err(
                "The file's letter shapes come from its page appearance, not its editable layers."
                    .into(),
            );
        }
    }
    let prepared = if from_file {
        let shapes = import_route(path, bytes, false, true, answers.dxf_unit)?;
        let mut prepared = prepare_imported(
            &shapes,
            bytes,
            &answers,
            recipe,
            false,
            &BTreeSet::new(),
            false,
        )?;
        let names: Vec<String> = answers
            .fonts_from_file
            .iter()
            .map(|(family, style)| format!("{family} {style}"))
            .collect();
        prepared.changes.insert(
            0,
            Change {
                code: "font.from_file".into(),
                message: format!(
                    "Using the letter shapes saved in the file for {} (your choice)",
                    names.join(", ")
                ),
                count: names.len(),
                source_ids: vec![],
                choices: vec![],
            },
        );
        // The shapes are part of the source file, which `source_sha256` already identifies.
        for font in &imported.inspection.fonts {
            prepared.fonts.push(FontUse {
                family: font.family.clone(),
                style: font.style.clone(),
                status: "from_file".into(),
                resolved_family: font.family.clone(),
                resolved_style: font.style.clone(),
                postscript_name: String::new(),
                sha256: prepared.source_sha256.clone(),
                source_ids: font.source_ids.clone(),
            });
        }
        prepared
    } else if answers.route == Some(ImportRoute::PdfAppearance) {
        let appearance = import_route(path, bytes, false, false, answers.dxf_unit)?;
        prepare_imported(
            &appearance,
            bytes,
            &answers,
            recipe,
            differs,
            &embedded,
            false,
        )?
    } else {
        prepare_imported(
            &imported,
            bytes,
            &answers,
            recipe,
            differs,
            &embedded,
            editing_layers,
        )?
    };
    Ok((imported.inspection, prepared))
}

/// Largest cut width accepted for a picture: well beyond any roll, but bounded.
const MAX_PICTURE_WIDTH_MM: f64 = 5000.0;

/// Spec §5.1.2: establish a starting size, then trace with the recorded settings and prepare the
/// traced shapes like any vector design.
fn prepare_picture(
    path: &Path,
    bytes: &[u8],
    recipe: &Recipe,
    answers: &Answers,
) -> Result<(Inspection, PreparedDesign), String> {
    if answers.route.is_some() {
        return Err(
            "The recipe chooses how to open a file's layers, but this design is a picture.".into(),
        );
    }
    let picture = picture::decode(bytes, answers.picture_frame)?;
    if answers.picture_frame.is_some() && picture.frames == 1 {
        return Err("The recipe chooses a frame, but this picture is not animated.".into());
    }
    let inspection = picture::inspection(path, bytes, &picture);
    // Spec §5.1.2: an animated picture needs one frame chosen.
    let frame_question = (picture.frames > 1 && answers.picture_frame.is_none()).then(|| {
        PendingDecision {
            decision: "picture_frame".into(),
            message: format!(
                "This picture is animated, with {} frames. Choose the frame to cut; the first is shown until you do.",
                picture.frames
            ),
            choices: (0..picture.frames.min(24))
                .map(|frame| Decision::PictureFrame { frame })
                .collect(),
            source_ids: vec!["source/0".into()],
        }
    });
    // A photo traces into a tangle of tiny shapes: ask before tracing it.
    let photo = picture::looks_like_photo(&picture);
    if answers.trace_photo && !photo {
        return Err("The recipe traces a photo, but this picture is flat artwork.".into());
    }
    let photo_question = (photo && !answers.trace_photo).then(|| PendingDecision {
        decision: "trace_photo".into(),
        message: "This picture looks like a photo or has many colour shades. Tracing follows colour and brightness, so it may capture shadows and texture instead of the subject's outline. For a clean outline, prepare the image in VectorCraft or Illustrator, then open the exported SVG here. You can still trace it anyway. Nothing is traced until you answer.".into(),
        choices: vec![Decision::TracePhoto],
        source_ids: vec!["source/0".into()],
    });
    let settings = answers.trace.unwrap_or_default();
    let (default_width, default_reason) = picture::initial_width_mm(&picture);
    let width_mm = answers.picture_size.unwrap_or(default_width);
    let size_reason = if answers.picture_size.is_some() {
        "your choice"
    } else {
        default_reason
    };
    if !(width_mm.is_finite() && width_mm > 0.0 && width_mm <= MAX_PICTURE_WIDTH_MM) {
        return Err(format!(
            "Choose a picture width between 0 and {MAX_PICTURE_WIDTH_MM} mm."
        ));
    }
    if let Some(question) = photo_question {
        // Sized but not traced: the picture shows at its size, with nothing to cut yet.
        let height_mm =
            width_mm * f64::from(picture.raster.height) / f64::from(picture.raster.width);
        let prepared = PreparedDesign {
            schema_version: PREPARED_SCHEMA_VERSION,
            stage: "preparation".into(),
            cut_ready: false,
            engine_revision: ENGINE_REVISION.into(),
            source_sha256: inspection.source_sha256.clone(),
            recipe: recipe.clone(),
            fonts: vec![],
            page: Some(Page {
                id: "page/0".into(),
                width_mm,
                height_mm,
                origin_pt: None,
            }),
            pieces: vec![],
            cut_lines: vec![],
            changes: vec![Change {
                code: "picture.sized".into(),
                message: format!(
                    "Cut size {} ({size_reason})",
                    size_text(width_mm, height_mm)
                ),
                count: 1,
                source_ids: vec!["source/0".into()],
                choices: vec![],
            }],
            pending: frame_question.into_iter().chain([question]).collect(),
            diagnostics: picture::picture_diagnostics(&picture, Some(width_mm)),
            stroked_fills: 0,
        };
        return Ok((inspection, prepared));
    }
    let imported = picture::traced(&picture, width_mm, &settings, inspection.clone())?;
    let mut prepared = prepare_imported(
        &imported,
        bytes,
        answers,
        recipe,
        false,
        &BTreeSet::new(),
        false,
    )?;
    let height_mm = width_mm * f64::from(picture.raster.height) / f64::from(picture.raster.width);
    let (mode, other) = match settings.colours {
        TraceColours::One if picture::light_art_on_transparency(&picture, &settings) => (
            "in one colour (decal): its light art, since the background is transparent",
            TraceColours::Several,
        ),
        TraceColours::One => ("in one colour (decal)", TraceColours::Several),
        TraceColours::Several => ("in several colours", TraceColours::One),
    };
    let traced = Change {
        code: "picture.traced".into(),
        message: format!("Traced the picture {mode}"),
        count: 1,
        source_ids: vec!["source/0".into()],
        choices: vec![Decision::Trace(TraceSettings {
            colours: other,
            ..settings
        })],
    };
    let sized = Change {
        code: "picture.sized".into(),
        message: format!(
            "Cut size {} ({size_reason})",
            size_text(width_mm, height_mm)
        ),
        count: 1,
        source_ids: vec!["source/0".into()],
        choices: vec![],
    };
    prepared.changes.splice(0..0, [sized, traced]);
    if answers.trace_photo {
        prepared.changes.insert(
            0,
            Change {
                code: "picture.photo_traced".into(),
                message: "Tracing this photo anyway (your choice)".into(),
                count: 1,
                source_ids: vec!["source/0".into()],
                choices: vec![],
            },
        );
    }
    if let Some(question) = frame_question {
        prepared.pending.insert(0, question);
        prepared.cut_ready = false;
    } else if let Some(frame) = answers.picture_frame {
        prepared.changes.insert(
            0,
            Change {
                code: "picture.frame".into(),
                message: format!(
                    "Cutting frame {} of {} (your choice)",
                    frame + 1,
                    picture.frames
                ),
                count: 1,
                source_ids: vec!["source/0".into()],
                choices: vec![],
            },
        );
    }
    Ok((inspection, prepared))
}

fn prepare_imported(
    imported: &Imported,
    bytes: &[u8],
    answers: &Answers,
    recipe: &Recipe,
    differs: bool,
    embedded: &BTreeSet<String>,
    editing_layers: bool,
) -> Result<PreparedDesign, String> {
    let mut run = Run::new(imported, answers);
    if imported
        .inspection
        .diagnostics
        .iter()
        .any(|d| d.code == "dxf.units_needed")
    {
        use cutcraft_job::DxfUnit;
        run.pending.push(PendingDecision {
            decision: "dxf_units".into(),
            message: "This DXF does not declare physical units. The preview uses millimetres for now. What does one drawing unit represent?".into(),
            choices: [DxfUnit::Mm, DxfUnit::In, DxfUnit::Cm, DxfUnit::Ft, DxfUnit::M].into_iter().map(|unit| Decision::DxfUnits { unit }).collect(),
            source_ids: vec![],
        });
    }
    run.editing_layers = editing_layers;
    // The file's letter shapes are offered only when it carries every font the design uses.
    run.fonts_in_file = !imported.inspection.fonts.is_empty()
        && imported
            .inspection
            .fonts
            .iter()
            .all(|f| shapes_available(f, embedded));
    run.choose_route(differs);
    let mut page = run.choose_page()?;
    if let Some(page_rect) = page {
        run.confirm_size(page_rect)?;
        run.resolve_fonts();
        run.resolve_appearance()?;
        run.collect(page_rect);
        run.size_each_unit();
        let page_rect = run.fit_artwork(page_rect)?.unwrap_or(page_rect);
        page = Some(page_rect);
        run.convert(page_rect)?;
    }
    answers.check_all_used(&run.used)?;
    Ok(run.finish(bytes, recipe, page))
}

/// Recipe decisions indexed by what they answer. Duplicates are an error.
struct Answers {
    dxf_unit: Option<cutcraft_job::DxfUnit>,
    route: Option<ImportRoute>,
    overflow: BTreeSet<String>,
    fonts_from_file: BTreeSet<(String, String)>,
    fit_artwork: bool,
    keep_page: bool,
    solid: BTreeMap<String, Option<String>>,
    excluded_objects: BTreeSet<String>,
    picture_size: Option<f64>,
    picture_frame: Option<usize>,
    trace: Option<TraceSettings>,
    trace_photo: bool,
    accepted: BTreeSet<String>,
    pictures: BTreeSet<String>,
    page: Option<String>,
    size: Option<(f64, f64)>,
    background: BTreeMap<String, bool>,
    strokes: BTreeMap<(String, usize), StrokeRole>,
    stroke_colours: BTreeMap<String, StrokeRole>,
    outlines_on_fills: Option<StrokeRole>,
    fonts: BTreeSet<(String, String)>,
}

impl Answers {
    fn new(recipe: &Recipe) -> Result<Self, String> {
        if recipe.schema_version != RECIPE_SCHEMA_VERSION {
            return Err(format!(
                "This recipe uses schema {}; this version reads schema {RECIPE_SCHEMA_VERSION}.",
                recipe.schema_version
            ));
        }
        let mut answers = Self {
            dxf_unit: None,
            route: None,
            overflow: BTreeSet::new(),
            fonts_from_file: BTreeSet::new(),
            fit_artwork: false,
            keep_page: false,
            solid: BTreeMap::new(),
            excluded_objects: BTreeSet::new(),
            picture_size: None,
            picture_frame: None,
            trace: None,
            trace_photo: false,
            accepted: BTreeSet::new(),
            pictures: BTreeSet::new(),
            page: None,
            size: None,
            background: BTreeMap::new(),
            strokes: BTreeMap::new(),
            stroke_colours: BTreeMap::new(),
            outlines_on_fills: None,
            fonts: BTreeSet::new(),
        };
        let duplicate = |what: &str| format!("The recipe answers {what} more than once.");
        for decision in &recipe.decisions {
            match decision {
                Decision::DxfUnits { unit } => {
                    if answers.dxf_unit.replace(*unit).is_some() {
                        return Err(duplicate("DXF units"));
                    }
                }
                Decision::AcceptTextOverflow { source_id } => {
                    if !answers.overflow.insert(source_id.clone()) {
                        return Err(duplicate(&format!("the text {source_id}")));
                    }
                }
                Decision::FontFromFile { family, style } => {
                    if !answers
                        .fonts_from_file
                        .insert((family.clone(), style.clone()))
                    {
                        return Err(duplicate(&format!("the font {family} {style}")));
                    }
                }
                Decision::FitArtwork | Decision::KeepPage => {
                    if answers.fit_artwork || answers.keep_page {
                        return Err(duplicate("whether the page fits the artwork"));
                    }
                    answers.fit_artwork = *decision == Decision::FitArtwork;
                    answers.keep_page = *decision == Decision::KeepPage;
                }
                Decision::CutSolid { source_id, colour } => {
                    if answers.excluded_objects.contains(source_id)
                        || answers
                            .solid
                            .insert(source_id.clone(), colour.clone())
                            .is_some()
                    {
                        return Err(duplicate(&format!("how {source_id} is cut")));
                    }
                }
                Decision::ExcludeObject { source_id } => {
                    if answers.solid.contains_key(source_id)
                        || !answers.excluded_objects.insert(source_id.clone())
                    {
                        return Err(duplicate(&format!("how {source_id} is cut")));
                    }
                }
                Decision::PictureSize { width_mm } => {
                    if answers.picture_size.replace(*width_mm).is_some() {
                        return Err(duplicate("the picture's size"));
                    }
                }
                Decision::PictureFrame { frame } => {
                    if answers.picture_frame.replace(*frame).is_some() {
                        return Err(duplicate("the picture's frame"));
                    }
                }
                Decision::Trace(settings) => {
                    if answers.trace.replace(*settings).is_some() {
                        return Err(duplicate("how the picture is traced"));
                    }
                }
                Decision::AcceptWarning { warning } => {
                    if !ACCEPTABLE_WARNINGS.contains(&warning.as_str()) {
                        return Err(format!("{warning} is not a warning that can be accepted."));
                    }
                    if !answers.accepted.insert(warning.clone()) {
                        return Err(duplicate(&format!("the warning {warning}")));
                    }
                }
                Decision::TracePhoto => {
                    if std::mem::replace(&mut answers.trace_photo, true) {
                        return Err(duplicate("whether to trace the photo"));
                    }
                }
                Decision::ExcludePicture { source_id } => {
                    if !answers.pictures.insert(source_id.clone()) {
                        return Err(duplicate(&format!("the picture {source_id}")));
                    }
                }
                Decision::ImportRoute { route } => {
                    if answers.route.replace(*route).is_some() {
                        return Err(duplicate("how to open the file's layers"));
                    }
                }
                Decision::Page { page } => {
                    if answers.page.replace(page.clone()).is_some() {
                        return Err(duplicate("the page"));
                    }
                }
                Decision::ConfirmSize {
                    width_mm,
                    height_mm,
                } => {
                    if answers.size.replace((*width_mm, *height_mm)).is_some() {
                        return Err(duplicate("the size"));
                    }
                }
                Decision::Background { source_id, cut } => {
                    if answers.background.insert(source_id.clone(), *cut).is_some() {
                        return Err(duplicate(&format!(
                            "the background question for {source_id}"
                        )));
                    }
                }
                Decision::StrokeColourRole { colour, role } => {
                    if answers
                        .stroke_colours
                        .insert(colour.clone(), *role)
                        .is_some()
                    {
                        return Err(duplicate(&format!("lines of colour {colour}")));
                    }
                }
                Decision::OutlinesOnFills { role } => {
                    if answers.outlines_on_fills.replace(*role).is_some() {
                        return Err(duplicate("outlines on filled shapes"));
                    }
                }
                Decision::FontSubstitute { family, style } => {
                    if !answers.fonts.insert((family.clone(), style.clone())) {
                        return Err(duplicate(&format!("the font {family} {style}")));
                    }
                }
                Decision::StrokeRole {
                    source_id,
                    stroke,
                    role,
                } => {
                    if answers
                        .strokes
                        .insert((source_id.clone(), *stroke), *role)
                        .is_some()
                    {
                        return Err(duplicate(&format!("stroke {stroke} of {source_id}")));
                    }
                }
            }
        }
        Ok(answers)
    }

    /// A decision that matches nothing in this source means the recipe is stale.
    fn check_all_used(&self, used: &Used) -> Result<(), String> {
        for id in self.background.keys() {
            if !used.background.contains(id) {
                return Err(format!(
                    "The recipe answers a background question for {id}, which is not a background candidate in this design."
                ));
            }
        }
        if self.fit_artwork && !used.fit_artwork {
            return Err(
                "The recipe fits the page to the artwork, but this design has no artwork to fit."
                    .into(),
            );
        }
        if self.keep_page && !used.keep_page {
            return Err(
                "The recipe trims the art to the page, but no art in this design reaches past it."
                    .into(),
            );
        }
        for id in &self.overflow {
            if !used.overflow.contains(id) {
                return Err(format!(
                    "The recipe leaves out text of {id} that doesn't fit, but all of that text fits in this design."
                ));
            }
        }
        for id in self.solid.keys().chain(&self.excluded_objects) {
            if !used.appearance.contains(id) {
                return Err(format!(
                    "The recipe answers how {id} is cut, but that object has no appearance question in this design."
                ));
            }
        }
        for id in &self.pictures {
            if !used.pictures.contains(id) {
                return Err(format!(
                    "The recipe leaves out the picture {id}, which is not a picture in this design."
                ));
            }
        }
        for colour in self.stroke_colours.keys() {
            if !used.stroke_colours.contains(colour) {
                return Err(format!(
                    "The recipe sets how {colour} lines are cut, but this design has no {colour} lines."
                ));
            }
        }
        for (family, style) in &self.fonts {
            if !used.fonts.contains(&(family.clone(), style.clone())) {
                return Err(format!(
                    "The recipe accepts a substitute for {family} {style}, but this design doesn't need one. Installed fonts may have changed since it was answered; clear the answers and answer again."
                ));
            }
        }
        for (id, stroke) in self.strokes.keys() {
            if !used.objects.contains(id) {
                return Err(format!(
                    "The recipe sets stroke {stroke} of {id}, which is not an object in this design."
                ));
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct Used {
    fit_artwork: bool,
    keep_page: bool,
    overflow: BTreeSet<String>,
    background: BTreeSet<String>,
    appearance: BTreeSet<String>,
    pictures: BTreeSet<String>,
    fonts: BTreeSet<(String, String)>,
    stroke_colours: BTreeSet<String>,
    /// Every source id present in the imported tree.
    objects: BTreeSet<String>,
}

/// One included source object with cuttable geometry, in page points (y down).
struct Leaf {
    order: usize,
    source_id: String,
    layer: String,
    path: PathData,
    rule: FillRule,
    fill: Option<Colour>,
    strokes: Vec<(usize, StrokeLayer)>,
    /// The region of every clipping mask above this object, intersected, in page points.
    clip: Option<PathData>,
}

/// One connected filled part of a source object: an outer contour with its holes.
struct Part {
    order: usize,
    source_id: String,
    path: PathData,
}

/// What applies to everything under a node during the tree walk.
#[derive(Clone, Copy)]
struct Scope<'a> {
    /// The top-level layer, which unions never cross.
    layer: &'a str,
    page: Rect,
    excluded: &'a BTreeSet<&'a str>,
    /// Every clipping mask above, intersected, in document points.
    clip: Option<&'a PathData>,
    /// Inside an object answered *Cut as solid*: `Some`, with the colour for paints that are not
    /// one colour.
    solid: Option<Option<&'a Colour>>,
}

/// What the tree walk found, before conversion.
#[derive(Default)]
struct Collected {
    leaves: Vec<Leaf>,
    unsupported: Vec<String>,
    text_appearance: Vec<String>,
    text_inline: Vec<String>,
    text_paint: Vec<String>,
    clip_unsupported: Vec<String>,
    pictures: Vec<String>,
    outlined: Vec<String>,
    /// Text that does not fit its frame or path, and whether none of it fits.
    overflow: Vec<(String, bool)>,
    empty_text: Vec<String>,
}

impl Collected {
    fn push_leaf(
        &mut self,
        id: &str,
        scope: &Scope,
        mut path: PathData,
        rule: FillRule,
        fill: Option<Colour>,
        strokes: Vec<(usize, StrokeLayer)>,
    ) {
        let to_page = kurbo::Affine::translate(-scope.page.origin().to_vec2());
        path.transform(to_page);
        self.leaves.push(Leaf {
            order: self.leaves.len(),
            source_id: id.into(),
            layer: scope.layer.into(),
            path,
            rule,
            fill,
            strokes,
            clip: scope.clip.map(|c| c.transformed(to_page)),
        });
    }
}

/// A path or compound path's geometry and fill rule; `None` for any other kind.
fn plain_path(node: &Node) -> Option<(PathData, FillRule)> {
    match &node.kind {
        NodeKind::Path { path, rule, .. } => Some((path.clone(), *rule)),
        NodeKind::Compound { children, rule } => {
            let mut subpaths = Vec::new();
            for child in children {
                let NodeKind::Path { path, .. } = &child.kind else {
                    return None;
                };
                subpaths.extend(path.subpaths.clone());
            }
            Some((PathData::new(subpaths), *rule))
        }
        _ => None,
    }
}

/// The paints of a text object's characters stacked with the object's own fills and strokes, in
/// paint order: object items below its contents, the characters, then the rest.
fn text_appearance(
    object: &vectorcraft_doc::Appearance,
    characters: vectorcraft_doc::Appearance,
) -> vectorcraft_doc::Appearance {
    if object.items.is_empty() {
        return characters;
    }
    let below = object.contents_index.unwrap_or(0).min(object.items.len());
    let items = object.items[..below]
        .iter()
        .cloned()
        .chain(characters.items)
        .chain(object.items[below..].iter().cloned())
        .collect();
    vectorcraft_doc::Appearance {
        items,
        ..characters
    }
}

/// The topmost visible fill's colour and every visible stroke, numbered bottom first among all
/// of the object's strokes.
fn paint_of(
    appearance: &vectorcraft_doc::Appearance,
    solid: Option<Option<&Colour>>,
) -> (Option<Colour>, Vec<(usize, StrokeLayer)>) {
    // Under *Cut as solid*, a paint that is not one colour takes the chosen colour.
    let replacement = solid.flatten();
    let mut fill = None;
    let mut strokes = Vec::new();
    let mut stroke_index = 0;
    for item in &appearance.items {
        match item {
            AppearanceItem::Fill(f) => {
                if f.visible && !f.paint.is_none() {
                    // The topmost visible fill paints over any below it.
                    fill = colour_of(&f.paint).or_else(|| replacement.cloned());
                }
            }
            AppearanceItem::Stroke(s) => {
                if s.visible && !s.paint.is_none() {
                    let mut s = s.clone();
                    if let (None, Some(colour)) = (colour_of(&s.paint), replacement)
                        && let Some(color) = Color::from_hex(&colour.hex)
                    {
                        s.paint = Paint::solid(color);
                    }
                    strokes.push((stroke_index, s));
                }
                stroke_index += 1;
            }
        }
    }
    (fill, strokes)
}

/// The node at a source address (`source/<layer>/<child>/…`).
fn node_at<'d>(doc: &'d vectorcraft_doc::Document, id: &str) -> Option<&'d Node> {
    let mut steps = id.strip_prefix("source/")?.split('/');
    let mut node: &Node = doc.layers.get(steps.next()?.parse::<usize>().ok()?)?;
    for step in steps {
        node = node.children()?.get(step.parse::<usize>().ok()?)?;
    }
    Some(node)
}

/// The distinct stop colours of an object's gradient paints, as offered choices.
fn gradient_colours(node: &Node) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in &node.appearance.items {
        if let Paint::Gradient(g) = item.paint() {
            for stop in &g.gradient.stops {
                let hex = stop.color.to_hex();
                if !out.contains(&hex) && out.len() < 4 {
                    out.push(hex);
                }
            }
        }
    }
    out
}

/// Solid, opaque, normal-blend paint without effects: what can become one material colour.
fn paint_supported(appearance: &vectorcraft_doc::Appearance) -> bool {
    appearance.items.iter().all(|item| {
        let (paint, visible, opacity, blend, effects) = match item {
            AppearanceItem::Fill(f) => (&f.paint, f.visible, f.opacity, f.blend, &f.effects),
            AppearanceItem::Stroke(s) => (&s.paint, s.visible, s.opacity, s.blend, &s.effects),
        };
        !visible
            || paint.is_none()
            || (matches!(paint, Paint::Solid { .. })
                && opacity == 1.0
                && blend == vectorcraft_color::BlendMode::Normal
                && effects.is_empty())
    })
}

struct Run<'a> {
    imported: &'a Imported,
    answers: &'a Answers,
    used: Used,
    /// Objects (and their descendants) with a blocking inspection diagnostic.
    blocked: Vec<String>,
    leaves: Vec<Leaf>,
    pieces: Vec<Piece>,
    /// Filled objects with a stroke that is part of the shape by default.
    stroked_fills: usize,
    cut_lines: Vec<CutLine>,
    fonts: Vec<FontUse>,
    /// The page is the artwork's bounds rather than the file's page.
    fitted: bool,
    /// The file embeds every font its text uses, so its own letter shapes can be cut.
    fonts_in_file: bool,
    /// The layers came from Illustrator's editing data, so the page appearance is an alternative.
    editing_layers: bool,
    /// Objects with appearance questions, and the inspection codes that raised them.
    appearance: BTreeMap<String, BTreeSet<String>>,
    /// Objects answered *Cut as solid*, with the colour for paints that are not one colour.
    solid: BTreeMap<String, Option<Colour>>,
    /// Text whose fonts are pending a decision or unusable; it is not outlined.
    font_blocked: Vec<String>,
    /// Duplicate-detection keys of every planned closed cut, by colour.
    planned: BTreeSet<(Colour, Vec<[i64; 2]>)>,
    changes: Vec<Change>,
    pending: Vec<PendingDecision>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Run<'a> {
    fn new(imported: &'a Imported, answers: &'a Answers) -> Self {
        let mut diagnostics = Vec::new();
        let mut blocked = Vec::new();
        let mut appearance: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for d in &imported.inspection.diagnostics {
            if RESOLVED_HERE.contains(&d.code.as_str()) {
                continue;
            }
            if APPEARANCE_CODES.contains(&d.code.as_str()) {
                for id in &d.source_ids {
                    appearance
                        .entry(id.clone())
                        .or_default()
                        .insert(d.code.clone());
                }
                continue;
            }
            if d.severity == Severity::Blocking {
                blocked.extend(d.source_ids.iter().cloned());
            }
            diagnostics.push(d.clone());
        }
        let used = Used {
            objects: imported
                .inspection
                .objects
                .iter()
                .map(|o| o.source_id.clone())
                .collect(),
            ..Used::default()
        };
        let mut run = Self {
            imported,
            answers,
            used,
            blocked,
            leaves: Vec::new(),
            pieces: Vec::new(),
            stroked_fills: 0,
            cut_lines: Vec::new(),
            planned: BTreeSet::new(),
            fonts: Vec::new(),
            fitted: false,
            fonts_in_file: false,
            editing_layers: false,
            appearance: BTreeMap::new(),
            solid: BTreeMap::new(),
            font_blocked: Vec::new(),
            changes: Vec::new(),
            pending: Vec::new(),
            diagnostics,
        };
        run.appearance = appearance;
        run.report_ignored();
        run
    }

    fn change(&mut self, code: &str, message: String, source_ids: Vec<String>) {
        self.changes.push(Change {
            code: code.into(),
            message,
            count: source_ids.len(),
            source_ids,
            choices: Vec::new(),
        });
    }

    fn report_ignored(&mut self) {
        // Report only the outermost excluded object; its contents are excluded with it.
        let mut ignored: Vec<String> = Vec::new();
        for object in &self.imported.inspection.objects {
            if object.excluded.is_some() && !ignored.iter().any(|i| within(&object.source_id, i)) {
                ignored.push(object.source_id.clone());
            }
        }
        if !ignored.is_empty() {
            let n = ignored.len();
            self.change(
                "ignored.hidden",
                format!("Ignored {n} hidden object(s), guide(s) or non-printing layer(s)"),
                ignored,
            );
        }
    }

    /// Spec §5.1: prefer verified appearance fidelity; when the editable layers differ from the
    /// page, the person chooses the route before cutting.
    fn choose_route(&mut self, differs: bool) {
        if !differs {
            return;
        }
        match self.answers.route {
            Some(ImportRoute::EditingData) => self.change(
                "import.route",
                "Using the file's editable layers, which differ from its page (your choice)".into(),
                vec![],
            ),
            Some(ImportRoute::PdfAppearance) => self.change(
                "import.route",
                "Using the file's page appearance; its layers come from that appearance (your choice)".into(),
                vec![],
            ),
            None => {
                let detail = self
                    .imported
                    .inspection
                    .diagnostics
                    .iter()
                    .find(|d| matches!(d.code.as_str(), "import.appearance_differs" | "import.editing_text_omitted"))
                    .map(|d| d.message.clone())
                    .unwrap_or_default();
                self.pending.push(PendingDecision {
                    decision: "import_route".into(),
                    message: format!(
                        "This file's editable layers don't match its page exactly ({detail}). Open it from the page's appearance (what it looks like), or from the editable layers?"
                    ),
                    choices: [ImportRoute::PdfAppearance, ImportRoute::EditingData]
                        .map(|route| Decision::ImportRoute { route })
                        .to_vec(),
                    source_ids: vec![],
                });
            }
        }
    }

    fn choose_page(&mut self) -> Result<Option<Rect>, String> {
        let artboards = &self.imported.doc.artboards;
        let id_of = |i: usize| format!("page/{i}");
        let chosen = match &self.answers.page {
            Some(page) => Some(
                (0..artboards.len())
                    .find(|&i| id_of(i) == *page)
                    .ok_or_else(|| {
                        format!("The recipe chooses {page}, which this design does not have.")
                    })?,
            ),
            None if artboards.len() == 1 => Some(0),
            None => None,
        };
        match chosen {
            Some(i) => {
                if artboards.len() > 1 {
                    self.change(
                        "page.chosen",
                        format!("Cutting page {} of {}", i + 1, artboards.len()),
                        vec![],
                    );
                }
                Ok(Some(artboards[i].rect))
            }
            None => {
                self.pending.push(PendingDecision {
                    decision: "page".into(),
                    message: format!(
                        "This design has {} pages. Choose the page to cut.",
                        artboards.len()
                    ),
                    choices: (0..artboards.len())
                        .map(|i| Decision::Page { page: id_of(i) })
                        .collect(),
                    source_ids: vec![],
                });
                Ok(None)
            }
        }
    }

    fn confirm_size(&mut self, page: Rect) -> Result<(), String> {
        let (width, height) = (
            round(page.width() * MM_PER_PT, 1e-3),
            round(page.height() * MM_PER_PT, 1e-3),
        );
        let needs_confirmation = self
            .imported
            .inspection
            .diagnostics
            .iter()
            .any(|d| d.code == "size.confirm");
        match self.answers.size {
            Some((w, h)) => {
                if (w - width).abs() > 1e-3 || (h - height).abs() > 1e-3 {
                    return Err(format!(
                        "The recipe confirms {}, but this design is {}. Resizing is an arrangement step; confirm the reported size.",
                        size_text(w, h),
                        size_text(width, height)
                    ));
                }
                self.change(
                    "size.confirmed",
                    format!("Confirmed size {width} × {height} mm"),
                    vec![],
                );
            }
            None if needs_confirmation => self.pending.push(PendingDecision {
                decision: "confirm_size".into(),
                message: format!(
                    "This file does not state its physical size. It opened as {}; confirm that size before cutting.",
                    size_text(width, height)
                ),
                choices: vec![Decision::ConfirmSize {
                    width_mm: width,
                    height_mm: height,
                }],
                source_ids: vec![],
            }),
            None => {}
        }
        Ok(())
    }

    fn is_blocked(&self, id: &str) -> bool {
        self.blocked.iter().any(|b| within(id, b))
    }

    /// Spec §5.1: fonts are checked before substitutions become permanent geometry. Exact
    /// fonts are fingerprinted by the file the outlines come from; a stand-in needs *Use this
    /// substitute*; missing glyphs block, since a fallback font would draw them.
    fn resolve_fonts(&mut self) {
        let db = vectorcraft_text::FontDb::global();
        let mut missing_glyphs = Vec::new();
        let mut unavailable = Vec::new();
        for font in &self.imported.inspection.fonts {
            let key = (font.family.clone(), font.style.clone());
            let ids = font.source_ids.clone();
            if font.missing_glyphs > 0 {
                missing_glyphs.extend(ids.iter().cloned());
                self.font_blocked.extend(ids);
                continue;
            }
            let accepted = self.answers.fonts.contains(&key);
            if accepted && !matches!(font.status, FontStatus::Exact) {
                self.used.fonts.insert(key.clone());
            }
            let status = match font.status {
                FontStatus::Exact => "exact",
                FontStatus::Substitute | FontStatus::Missing if accepted => "substitute",
                FontStatus::Substitute | FontStatus::Missing => {
                    let stand_in = font
                        .resolved
                        .as_ref()
                        .map(|r| format!("{} {}", r.family, r.style))
                        .unwrap_or_else(|| "a fallback font".into());
                    let mut choices = Vec::new();
                    let mut message = format!(
                        "The font {} {} isn't on this computer. CutCraft can use {stand_in} instead, but its letters have different shapes and widths, so text may move or overlap. Best: save the design with its text turned into shapes (see How to save designs for cutting). Or install the font and open the design again.",
                        font.family, font.style
                    );
                    if self.fonts_in_file {
                        message = format!(
                            "The font {} {} isn't on this computer, but the file has its letter shapes saved inside. Cutting those keeps the letters as designed (recommended). Or use {stand_in} instead, or install the font and open the design again.",
                            font.family, font.style
                        );
                        choices.push(Decision::FontFromFile {
                            family: font.family.clone(),
                            style: font.style.clone(),
                        });
                    }
                    choices.push(Decision::FontSubstitute {
                        family: font.family.clone(),
                        style: font.style.clone(),
                    });
                    self.pending.push(PendingDecision {
                        decision: "font_substitute".into(),
                        message,
                        choices,
                        source_ids: ids.clone(),
                    });
                    self.font_blocked.extend(ids);
                    continue;
                }
            };
            let Some((face, _)) = db.resolve(&font.family, &font.style) else {
                unavailable.extend(ids.iter().cloned());
                self.font_blocked.extend(ids);
                continue;
            };
            if status == "substitute" {
                self.change(
                    "font.substituted",
                    format!(
                        "Using {} {} for {} {} (your choice)",
                        face.family, face.style, font.family, font.style
                    ),
                    ids.clone(),
                );
            }
            self.fonts.push(FontUse {
                family: font.family.clone(),
                style: font.style.clone(),
                status: status.into(),
                resolved_family: face.family.clone(),
                resolved_style: face.style.clone(),
                postscript_name: face.postscript_name(),
                sha256: format!("{:x}", Sha256::digest(face.file_data())),
                source_ids: ids,
            });
        }
        if !missing_glyphs.is_empty() {
            self.diagnostics.push(Diagnostic::blocking(
                "font.missing_glyphs",
                "Some characters are missing from their font and would be drawn by a fallback font. Use a font that has them, change the text, or turn the text into shapes in your editor.",
                dedup_sorted(missing_glyphs.into_iter()),
            ));
        }
        if !unavailable.is_empty() {
            self.diagnostics.push(Diagnostic::blocking(
                "font.unavailable",
                "The font for this text could not be loaded to make its outlines.",
                dedup_sorted(unavailable.into_iter()),
            ));
        }
    }

    /// Spec §5.2: partially transparent, blended, effected, gradient or pattern artwork is cut
    /// as one solid colour where its shape is well defined, or left out, by choice. Unanswered,
    /// the object waits (and is not drawn); questions inside a waiting or excluded object come
    /// once it is answered.
    fn resolve_appearance(&mut self) -> Result<(), String> {
        let mut solid_ids = Vec::new();
        let mut excluded: Vec<String> = Vec::new();
        let mut waiting: Vec<String> = Vec::new();
        for (id, codes) in std::mem::take(&mut self.appearance) {
            let inside = |list: &[String]| list.iter().any(|a| within(&id, a));
            if inside(&excluded) || inside(&waiting) {
                continue;
            }
            let node = node_at(&self.imported.doc, &id);
            let masked = node.is_some_and(|n| n.extras().mask.is_some());
            let stops = node.map(gradient_colours).unwrap_or_default();
            let needs_colour = codes.contains("appearance.material_choice");
            if self.answers.excluded_objects.contains(&id) {
                self.used.appearance.insert(id.clone());
                self.blocked.push(id.clone());
                excluded.push(id);
            } else if let Some(colour) = self.answers.solid.get(&id) {
                self.used.appearance.insert(id.clone());
                if masked {
                    return Err(format!(
                        "{id} uses an opacity mask, which changes its shape; it can only be left out."
                    ));
                }
                let colour = match colour {
                    Some(hex) => Some(Colour {
                        hex: Color::from_hex(hex)
                            .ok_or_else(|| format!("{hex} is not a colour; write it as #rrggbb."))?
                            .to_hex(),
                        swatch: None,
                    }),
                    None if needs_colour => {
                        return Err(format!(
                            "{id} has a gradient or pattern; choose the colour to cut it in."
                        ));
                    }
                    None => None,
                };
                self.solid.insert(id.clone(), colour);
                solid_ids.push(id);
            } else {
                let message = if masked {
                    "This object uses an opacity mask, which changes its visible shape. Leave it out? To cut it, release or expand the mask in an editor first."
                } else if needs_colour {
                    "This object is filled with a gradient or pattern. Cut it in one colour, or leave it out?"
                } else if codes.contains("appearance.transparency")
                    || codes.contains("appearance.paint_effects")
                {
                    "This object is partly transparent, blended or has effects. Cut it as a solid shape, or leave it out?"
                } else {
                    "This object has effects such as shadows or glows. Cut its shape without them, or leave it out?"
                };
                let mut choices = Vec::new();
                if !masked {
                    if needs_colour {
                        let colours = if stops.is_empty() {
                            vec!["#000000".to_string()]
                        } else {
                            stops
                        };
                        choices.extend(colours.into_iter().map(|hex| Decision::CutSolid {
                            source_id: id.clone(),
                            colour: Some(hex),
                        }));
                    } else {
                        choices.push(Decision::CutSolid {
                            source_id: id.clone(),
                            colour: None,
                        });
                    }
                }
                choices.push(Decision::ExcludeObject {
                    source_id: id.clone(),
                });
                self.pending.push(PendingDecision {
                    decision: "appearance".into(),
                    message: format!("{message} It is left out of the preview until you answer."),
                    choices,
                    source_ids: vec![id.clone()],
                });
                self.blocked.push(id.clone());
                waiting.push(id);
            }
        }
        if !solid_ids.is_empty() {
            let n = solid_ids.len();
            self.change(
                "appearance.solid",
                format!("Cutting {n} object(s) as solid shapes (your choice)"),
                solid_ids,
            );
        }
        if !excluded.is_empty() {
            let n = excluded.len();
            self.change(
                "object.excluded",
                format!("Left out {n} object(s) (Don't cut)"),
                excluded,
            );
        }
        Ok(())
    }

    fn collect(&mut self, page: Rect) {
        let excluded: BTreeSet<&str> = self
            .imported
            .inspection
            .objects
            .iter()
            .filter(|o| o.excluded.is_some())
            .map(|o| o.source_id.as_str())
            .collect();
        let mut found = Collected::default();
        for (i, layer) in self.imported.doc.layers.iter().enumerate() {
            let id = format!("source/{i}");
            let layer_name = if self.imported.inspection.format == "dxf" {
                layer.name.as_deref().unwrap_or(&id)
            } else {
                &id
            };
            let scope = Scope {
                layer: layer_name,
                page,
                excluded: &excluded,
                clip: None,
                solid: None,
            };
            self.walk(layer, &id, &scope, &mut found);
        }
        for (code, message, ids) in [
            (
                "geometry.unsupported",
                "This kind of object cannot be prepared yet.",
                found.unsupported,
            ),
            (
                "text.object_appearance",
                "Fills, strokes or effects added to the whole text object would be lost when it becomes shapes. Expand its appearance in an editor first.",
                found.text_appearance,
            ),
            (
                "text.inline_graphics",
                "Graphics placed inside text cannot be prepared yet.",
                found.text_inline,
            ),
            (
                "text.paint",
                "Text with a gradient, pattern or transparent colour needs an explicit colour choice.",
                found.text_paint,
            ),
            (
                "clip.unsupported",
                "This clipping mask's shape cannot be applied yet; only path and compound path masks are.",
                found.clip_unsupported,
            ),
        ] {
            if !ids.is_empty() {
                self.diagnostics.push(Diagnostic::blocking(
                    code,
                    message,
                    dedup_sorted(ids.into_iter()),
                ));
            }
        }
        // Spec §5.1.2: an embedded picture cannot be cut in v1; it is left out only by choice.
        let mut excluded_pictures = Vec::new();
        for id in found.pictures {
            if self.answers.pictures.contains(&id) {
                self.used.pictures.insert(id.clone());
                excluded_pictures.push(id);
            } else {
                self.pending.push(PendingDecision {
                    decision: "exclude_picture".into(),
                    message: "This design contains a picture, which cannot be cut. Leave it out? To cut it, trace it into shapes in an editor first.".into(),
                    choices: vec![Decision::ExcludePicture { source_id: id.clone() }],
                    source_ids: vec![id],
                });
            }
        }
        if !excluded_pictures.is_empty() {
            let n = excluded_pictures.len();
            self.change(
                "picture.excluded",
                format!("Left out {n} picture(s) (Don't cut)"),
                excluded_pictures,
            );
        }
        if !found.outlined.is_empty() {
            let n = found.outlined.len();
            self.change(
                "text.outlined",
                format!("Turned {n} text item(s) into shapes"),
                found.outlined,
            );
        }
        // Layout here may differ from the design's (another version of a font, type on a path),
        // so text that does not fit is never dropped silently: the person decides.
        let mut accepted = Vec::new();
        for (id, none_fits) in found.overflow {
            if self.answers.overflow.contains(&id) {
                self.used.overflow.insert(id.clone());
                accepted.push(id);
                continue;
            }
            let extent = if none_fits {
                "none of it fits"
            } else {
                "part of it doesn't fit"
            };
            let mut choices = Vec::new();
            let mut advice = String::new();
            if self.editing_layers {
                advice =
                    " Opening the file from its page appearance keeps the text as it was designed."
                        .into();
                choices.push(Decision::ImportRoute {
                    route: ImportRoute::PdfAppearance,
                });
            }
            choices.push(Decision::AcceptTextOverflow {
                source_id: id.clone(),
            });
            self.pending.push(PendingDecision {
                decision: "text_overflow".into(),
                message: format!(
                    "This text doesn't fit its frame or path when laid out with the installed font: {extent}. The installed font may differ from the one it was designed with.{advice} To keep it exactly as designed, turn the text into shapes in your editor."
                ),
                choices,
                source_ids: vec![id],
            });
        }
        if !accepted.is_empty() {
            let n = accepted.len();
            self.change(
                "text.overflow",
                format!("Left out the text that doesn't fit in {n} text item(s) (your choice)"),
                accepted,
            );
        }
        if !found.empty_text.is_empty() {
            let n = found.empty_text.len();
            self.change(
                "text.empty",
                format!("Left out {n} empty text item(s)"),
                found.empty_text,
            );
        }
        self.leaves = found.leaves;
    }

    fn walk(&self, node: &Node, id: &str, scope: &Scope, found: &mut Collected) {
        if scope.excluded.contains(id) || self.is_blocked(id) {
            return;
        }
        let solid_scope;
        let scope = match self.solid.get(id) {
            Some(colour) => {
                solid_scope = Scope {
                    solid: Some(colour.as_ref()),
                    ..*scope
                };
                &solid_scope
            }
            None => scope,
        };
        match &node.kind {
            // Spec §5.2 step 4: the first (bottom-most) child clips the others. Its region is
            // intersected with any clip above; the clipping path itself is not cut.
            NodeKind::Layer {
                children,
                clip: true,
                ..
            }
            | NodeKind::Group {
                children,
                clip: true,
            } => {
                let Some((mask, rule)) = children.first().and_then(|c| plain_path(c)) else {
                    found.clip_unsupported.push(id.into());
                    return;
                };
                let region = try_normalize(&mask, rule).and_then(|region| match scope.clip {
                    Some(outer) => try_boolean(
                        outer,
                        FillRule::NonZero,
                        &region,
                        FillRule::NonZero,
                        BoolOp::Intersect,
                        DEFAULT_PRECISION,
                    ),
                    None => Ok(region),
                });
                let Ok(region) = region else {
                    found.clip_unsupported.push(id.into());
                    return;
                };
                let inner = Scope {
                    clip: Some(&region),
                    ..*scope
                };
                for (i, child) in children.iter().enumerate().skip(1) {
                    self.walk(child, &format!("{id}/{i}"), &inner, found);
                }
            }
            NodeKind::Layer { children, .. } | NodeKind::Group { children, .. } => {
                for (i, child) in children.iter().enumerate() {
                    self.walk(child, &format!("{id}/{i}"), scope, found);
                }
            }
            NodeKind::Text(text) => self.outline_text(node, text, id, scope, found),
            NodeKind::Image(_) => found.pictures.push(id.into()),
            // A compound's members are painted as one with the compound's appearance.
            NodeKind::Path { .. } | NodeKind::Compound { .. } => match plain_path(node) {
                Some((path, rule)) => {
                    let (fill, strokes) = paint_of(&node.appearance, scope.solid);
                    found.push_leaf(id, scope, path, rule, fill, strokes);
                }
                None => found.unsupported.push(id.into()),
            },
            _ => found.unsupported.push(id.into()),
        }
    }

    /// Text → shapes with the engine's own layout, as `type.createOutlines` does, but without
    /// editing the tree, so other objects keep their source addresses. Every glyph keeps the
    /// text's source id.
    fn outline_text(
        &self,
        node: &Node,
        text: &vectorcraft_doc::TextObject,
        id: &str,
        scope: &Scope,
        found: &mut Collected,
    ) {
        if self.font_blocked.iter().any(|b| b == id) {
            return;
        }
        // Effects on the whole text object have no verified conversion yet. Its fills and
        // strokes are kept: Create Outlines drops them, but they are part of what is seen.
        if !node.appearance.effects.is_empty() {
            found.text_appearance.push(id.into());
            return;
        }
        let db = vectorcraft_text::FontDb::global();
        let layout = vectorcraft_text::layout(db, text);
        if !layout.inlines.is_empty() {
            found.text_inline.push(id.into());
            return;
        }
        let mut shapes = Vec::new();
        for glyph in &layout.glyphs {
            shapes.push((glyph.run, glyph.outline.clone()));
        }
        shapes.extend(vectorcraft_text::decorations(&layout, db, text));
        let mut leaves = Vec::new();
        for (run, outline) in shapes {
            let path = PathData::from_bezpath(&outline).transformed(text.xf);
            if path.is_empty() {
                continue;
            }
            let style = text
                .runs
                .get(run)
                .map(|r| r.style.clone())
                .unwrap_or_else(|| text.first_style());
            let appearance = text_appearance(&node.appearance, style.appearance());
            if scope.solid.is_none() && !paint_supported(&appearance) {
                found.text_paint.push(id.into());
                return;
            }
            let (fill, strokes) = paint_of(&appearance, scope.solid);
            leaves.push((path, fill, strokes));
        }
        if layout.overflow {
            found.overflow.push((id.into(), leaves.is_empty()));
            if leaves.is_empty() {
                return;
            }
        }
        if leaves.is_empty() {
            found.empty_text.push(id.into());
            return;
        }
        found.outlined.push(id.into());
        for (path, fill, strokes) in leaves {
            found.push_leaf(id, scope, path, FillRule::NonZero, fill, strokes);
        }
    }

    /// Spec §5.1.1: the file's page need not be the design's extent. When every included object
    /// lies off the page, ask whether to use the artwork's bounds as the page; when the recipe
    /// says so, move the collected geometry onto that page. Returns the new page in document
    /// points.
    /// The DXF units question says how big the art comes out with each answer. Until it is
    /// answered, one drawing unit is read as a millimetre.
    fn size_each_unit(&mut self) {
        let Some(question) = self.pending.iter_mut().find(|q| q.decision == "dxf_units") else {
            return;
        };
        let Some(art) = self
            .leaves
            .iter()
            .filter_map(|leaf| bez_bounds(&leaf.path))
            .reduce(|a, b| a.union(b))
        else {
            return;
        };
        let (w, h) = (art.width() * MM_PER_PT, art.height() * MM_PER_PT);
        let sizes: Vec<String> = question
            .choices
            .iter()
            .filter_map(|choice| match choice {
                Decision::DxfUnits { unit } => {
                    let mm = unit.mm();
                    Some(format!(
                        "{}: {}",
                        unit.symbol(),
                        size_text(round(w * mm, 0.1), round(h * mm, 0.1))
                    ))
                }
                _ => None,
            })
            .collect();
        question.message = format!(
            "{} The art comes out at {}.",
            question.message,
            sizes.join("; ")
        );
    }

    fn fit_artwork(&mut self, page: Rect) -> Result<Option<Rect>, String> {
        // What each object shows: its clipping mask hides the rest, so only that counts.
        let extent = |leaf: &Leaf| {
            let half = leaf
                .strokes
                .iter()
                .map(|(_, s)| s.width / 2.0)
                .fold(0.0, f64::max);
            let drawn = bez_bounds(&leaf.path)?.inflate(half, half);
            match leaf.clip.as_ref() {
                Some(clip) => {
                    let shown = drawn.intersect(bez_bounds(clip)?);
                    (shown.width() > 0.0 && shown.height() > 0.0).then_some(shown)
                }
                None => Some(drawn),
            }
        };
        let bounds: Vec<Rect> = self.leaves.iter().filter_map(extent).collect();
        let Some(artwork) = bounds.iter().copied().reduce(|a, b| a.union(b)) else {
            return Ok(None);
        };
        let page_box = Rect::new(0.0, 0.0, page.width(), page.height());
        let size = |r: Rect| {
            (
                round(r.width() * MM_PER_PT, 0.1),
                round(r.height() * MM_PER_PT, 0.1),
            )
        };
        let single = self.imported.doc.artboards.len() <= 1;
        let all_outside = bounds.iter().all(|b| !overlaps(*b, page_box));
        // An SVG's page (its viewBox) is often just where the drawing started, and the Artwork
        // view shows all of it, so art reaching past the page edge is cut whole by default.
        // Objects entirely off the page are still left out.
        let margin = PAST_PAGE_FIT_MM / MM_PER_PT;
        let on_page = bounds
            .iter()
            .copied()
            .filter(|b| overlaps(*b, page_box))
            .reduce(|a, b| a.union(b));
        let crossing = on_page.filter(|art| {
            single
                && matches!(self.imported.inspection.format.as_str(), "svg" | "svgz")
                && !page_box.inflate(margin, margin).contains_rect(*art)
        });
        if self.answers.keep_page {
            if crossing.is_some() {
                self.used.keep_page = true;
                self.changes.push(Change {
                    code: "page.kept".into(),
                    message:
                        "Keeping the file's own page and trimming the art past it (your choice)"
                            .into(),
                    count: 0,
                    source_ids: vec![],
                    choices: vec![Decision::FitArtwork],
                });
            }
            return Ok(None);
        }
        let artwork = match crossing {
            Some(art) if !self.answers.fit_artwork => art,
            _ => artwork,
        };
        if !self.answers.fit_artwork && crossing.is_none() {
            if all_outside {
                let (pw, ph) = size(page_box);
                let (aw, ah) = size(artwork);
                self.pending.push(PendingDecision {
                    decision: "fit_artwork".into(),
                    message: format!(
                        "All of this design's art lies outside its {} page. Use the artwork's own {} area as the page?",
                        size_text(pw, ph),
                        size_text(aw, ah)
                    ),
                    choices: vec![Decision::FitArtwork],
                    source_ids: vec![],
                });
            }
            return Ok(None);
        }
        if !single {
            return Err(
                "Fitting the page to the artwork works only for single-page designs.".into(),
            );
        }
        self.used.fit_artwork = true;
        self.fitted = true;
        let shift = kurbo::Affine::translate(-artwork.origin().to_vec2());
        for leaf in &mut self.leaves {
            leaf.path.transform(shift);
            if let Some(clip) = &mut leaf.clip {
                clip.transform(shift);
            }
        }
        let (w, h) = size(artwork);
        if self.answers.fit_artwork {
            self.change(
                "page.fitted",
                format!(
                    "Using the artwork's own {} area as the page (your choice)",
                    size_text(w, h)
                ),
                vec![],
            );
        } else {
            let (pw, ph) = size(page_box);
            self.changes.push(Change {
                code: "page.art_bounds".into(),
                message: format!(
                    "The art reaches past the file's {} page, so its own {} area is used as the page",
                    size_text(pw, ph),
                    size_text(w, h)
                ),
                count: 0,
                source_ids: vec![],
                choices: vec![Decision::KeepPage],
            });
        }
        Ok(Some(artwork + page.origin().to_vec2()))
    }

    fn convert(&mut self, page: Rect) -> Result<(), String> {
        let page_box = Rect::new(0.0, 0.0, page.width(), page.height());
        let leaves = std::mem::take(&mut self.leaves);
        let mut outside = Vec::new();
        let mut page_crossing = Vec::new();
        let mut mask_crossing = Vec::new();
        let mut page_trimmed = Vec::new();
        let mut mask_trimmed = Vec::new();
        let mut mask_hidden = Vec::new();
        let mut empty = Vec::new();
        let mut invalid = Vec::new();
        let mut absorbed = Vec::new();
        let mut outline_failed = Vec::new();
        let mut shape_chosen = Vec::new();
        let mut joined_chosen = Vec::new();
        let mut joined_fill: BTreeMap<String, Vec<String>> = BTreeMap::new();
        // Default-outlined strokes by colour, so each colour offers *Cut along line* at once.
        let mut outlined: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut unsupported_lines = Vec::new();
        let mut cut_along = Vec::new();
        let mut parts: BTreeMap<(String, Colour), Vec<Part>> = BTreeMap::new();
        let mut group_order: BTreeMap<(String, Colour), usize> = BTreeMap::new();
        let mut lines: Vec<(usize, String, Colour, String, SubPath)> = Vec::new();
        let mut background_seen = false;
        let page_edge =
            rect_region(page_box.inflate(PAGE_EDGE_TOLERANCE_PT, PAGE_EDGE_TOLERANCE_PT));

        for leaf in &leaves {
            let region = match &leaf.fill {
                Some(_) => match try_normalize(&leaf.path, leaf.rule) {
                    Ok(region) => Some(region),
                    Err(_) => {
                        invalid.push(leaf.source_id.clone());
                        continue;
                    }
                },
                None => None,
            };
            let region = region.filter(|r| !r.is_empty());
            let half_stroke = leaf
                .strokes
                .iter()
                .map(|(_, s)| s.width / 2.0)
                .fold(0.0, f64::max);
            let bounds = match region
                .as_ref()
                .and_then(bez_bounds)
                .or(bez_bounds(&leaf.path))
            {
                Some(b) => b.inflate(half_stroke, half_stroke),
                None => {
                    empty.push(leaf.source_id.clone());
                    continue;
                }
            };
            if region.is_none() && leaf.strokes.is_empty() {
                empty.push(leaf.source_id.clone());
                continue;
            }
            if !overlaps(bounds, page_box) {
                outside.push(leaf.source_id.clone());
                continue;
            }
            // Spec §5.1 and §5.2 step 4: cut only what is visible inside clipping masks and on
            // the chosen page.
            let mut trim = Trim::default();
            let mut produced = false;
            let region = match region {
                Some(r) => match trim_region(&r, leaf.clip.as_ref(), page_box) {
                    Ok((visible, t)) => {
                        trim.add(t);
                        (!visible.is_empty()).then_some(visible)
                    }
                    Err(_) => {
                        invalid.push(leaf.source_id.clone());
                        continue;
                    }
                },
                None => None,
            };
            // Spec §5.2 step 6: only the bottommost included object can be a background box.
            if !background_seen {
                background_seen = true;
                if let Some(r) = &region
                    && is_page_rectangle(r, page_box)
                {
                    let id = leaf.source_id.clone();
                    match self.answers.background.get(&id) {
                        Some(&cut) => {
                            self.used.background.insert(id.clone());
                            if !cut {
                                self.change(
                                    "background.excluded",
                                    "Left out the background box (Don't cut)".into(),
                                    vec![id],
                                );
                                continue;
                            }
                            self.change(
                                "background.kept",
                                "Kept the background box (Cut it)".into(),
                                vec![id],
                            );
                        }
                        None => {
                            let (w, h) = (
                                round(page_box.width() * MM_PER_PT, 0.1),
                                round(page_box.height() * MM_PER_PT, 0.1),
                            );
                            self.pending.push(PendingDecision {
                                decision: "background".into(),
                                message: format!(
                                    "Is this a background box? ({}) It is left out of the preview until you answer.",
                                    size_text(w, h)
                                ),
                                choices: [true, false]
                                    .map(|cut| Decision::Background {
                                        source_id: id.clone(),
                                        cut,
                                    })
                                    .to_vec(),
                                source_ids: vec![id],
                            });
                            continue;
                        }
                    }
                }
            }
            if let (Some(region), Some(colour)) = (&region, &leaf.fill) {
                produced = true;
                let key = (leaf.layer.clone(), colour.clone());
                group_order.entry(key.clone()).or_insert(leaf.order);
                let entry = parts.entry(key).or_default();
                for piece in split_pieces(region) {
                    let path = PathData::new(std::iter::once(piece.0).chain(piece.1).collect());
                    entry.push(Part {
                        order: leaf.order,
                        source_id: leaf.source_id.clone(),
                        path,
                    });
                }
            }
            let mut stroked_fill = false;
            for (index, stroke) in &leaf.strokes {
                let Paint::Solid { .. } = &stroke.paint else {
                    // Gradient/pattern strokes are blocked by inspection; never reached.
                    unsupported_lines.push(leaf.source_id.clone());
                    continue;
                };
                let colour = colour_of(&stroke.paint).expect("solid paint has a colour");
                let filled = region.is_some() && leaf.fill.is_some();
                let mut default = if self.imported.inspection.format == "dxf" && !filled {
                    // CAD plotting lineweights are not two physical blade outlines.
                    StrokeRole::CutLine
                } else {
                    default_role(stroke.width, &colour, filled)
                };
                if filled && default == StrokeRole::PartOfShape {
                    stroked_fill = true;
                    // *Outlines on filled shapes* replaces the default for the whole design.
                    if let Some(role) = self.answers.outlines_on_fills {
                        default = role;
                    }
                }
                let by_object = self
                    .answers
                    .strokes
                    .get(&(leaf.source_id.clone(), *index))
                    .copied();
                let by_colour = self.answers.stroke_colours.get(&colour.hex).copied();
                if by_colour.is_some() {
                    self.used.stroke_colours.insert(colour.hex.clone());
                }
                // Spec §5.2 step 3: an object's choice overrides its colour's, which overrides
                // the width-based default.
                let chosen = by_object.or(by_colour);
                let mut role = chosen.unwrap_or(default);
                // Only a filled object has a shape for its stroke to join.
                if role == StrokeRole::PartOfShape && !filled {
                    role = StrokeRole::Shape;
                }
                match role {
                    StrokeRole::Shape | StrokeRole::PartOfShape => {
                        if stroke.brush.is_some() {
                            unsupported_lines.push(leaf.source_id.clone());
                            continue;
                        }
                        // Dashes, caps, joins, width profiles, arrowheads and alignment are the
                        // engine's own Outline Stroke geometry.
                        let outline = vectorcraft_effects::stroke::outline_region(
                            &leaf.path, leaf.rule, stroke,
                        );
                        if outline.is_empty() {
                            outline_failed.push(leaf.source_id.clone());
                            continue;
                        }
                        let outline = match trim_region(&outline, leaf.clip.as_ref(), page_box) {
                            Ok((visible, t)) => {
                                trim.add(t);
                                visible
                            }
                            Err(_) => {
                                invalid.push(leaf.source_id.clone());
                                continue;
                            }
                        };
                        if outline.is_empty() {
                            continue;
                        }
                        produced = true;
                        let joins_fill = role == StrokeRole::PartOfShape;
                        let piece_colour = match (&leaf.fill, joins_fill) {
                            (Some(fill), true) => fill.clone(),
                            _ => colour.clone(),
                        };
                        let key = (leaf.layer.clone(), piece_colour);
                        group_order.entry(key.clone()).or_insert(leaf.order);
                        let entry = parts.entry(key).or_default();
                        for piece in split_pieces(&outline) {
                            let path =
                                PathData::new(std::iter::once(piece.0).chain(piece.1).collect());
                            entry.push(Part {
                                order: leaf.order,
                                source_id: leaf.source_id.clone(),
                                path,
                            });
                        }
                        if joins_fill {
                            // Same colour: nothing changes but the outline; say nothing.
                            if leaf.fill.as_ref() != Some(&colour) {
                                if chosen.is_some() {
                                    joined_chosen.push(leaf.source_id.clone());
                                } else {
                                    joined_fill
                                        .entry(colour.hex.clone())
                                        .or_default()
                                        .push(leaf.source_id.clone());
                                }
                            }
                        } else if chosen.is_some() && default != StrokeRole::Shape {
                            shape_chosen.push(leaf.source_id.clone());
                        } else if chosen.is_none() {
                            outlined
                                .entry(colour.hex.clone())
                                .or_default()
                                .push(leaf.source_id.clone());
                        }
                    }
                    StrokeRole::CutLine => {
                        if stroke.dash.is_some()
                            || stroke.start_arrow.is_some()
                            || stroke.end_arrow.is_some()
                            || stroke.profile.is_some()
                            || stroke.brush.is_some()
                        {
                            unsupported_lines.push(leaf.source_id.clone());
                            continue;
                        }
                        if chosen == Some(StrokeRole::CutLine) && default != StrokeRole::CutLine {
                            cut_along.push(leaf.source_id.clone());
                        }
                        for sp in &leaf.path.subpaths {
                            if sp.anchors.len() < 2 {
                                continue;
                            }
                            // A same-colour outline on its own fill is the fill's edge (step 5).
                            if leaf.fill.as_ref() == Some(&colour)
                                && region.is_some()
                                && vectorcraft_pathops::encloses_area(sp)
                            {
                                absorbed.push(leaf.source_id.clone());
                                continue;
                            }
                            // Clipping an open line must never add a cut along the clip edge
                            // (spec §7.1): whole lines are kept or dropped; a crossing blocks.
                            let fits = [
                                leaf.clip.as_ref().map(|c| (line_fit(sp, c), true)),
                                // A line along the page edge still counts as on the page.
                                Some((line_fit(sp, &page_edge), false)),
                            ];
                            let mut keep = true;
                            for (fit, mask) in fits.into_iter().flatten() {
                                match fit {
                                    Fit::Inside => {}
                                    Fit::Outside => {
                                        keep = false;
                                        trim.add(Trim { mask, page: !mask });
                                    }
                                    Fit::Crossing => {
                                        keep = false;
                                        if mask {
                                            mask_crossing.push(leaf.source_id.clone());
                                        } else {
                                            page_crossing.push(leaf.source_id.clone());
                                        }
                                    }
                                }
                            }
                            if !keep {
                                continue;
                            }
                            produced = true;
                            lines.push((
                                leaf.order,
                                leaf.layer.clone(),
                                colour.clone(),
                                leaf.source_id.clone(),
                                sp.clone(),
                            ));
                        }
                    }
                }
            }
            if stroked_fill {
                self.stroked_fills += 1;
            }
            if trim.mask || trim.page {
                let id = leaf.source_id.clone();
                match (produced, trim.mask) {
                    (false, true) => mask_hidden.push(id),
                    (false, false) => outside.push(id),
                    (true, _) => {
                        if trim.mask {
                            mask_trimmed.push(id.clone());
                        }
                        if trim.page {
                            page_trimmed.push(id);
                        }
                    }
                }
            }
        }
        self.check_stroke_indices(&leaves)?;

        let mut joined = Vec::new();
        let mut union_failed = Vec::new();
        let mut specks = Vec::new();
        let mut groups: Vec<_> = parts.into_iter().collect();
        groups.sort_by_key(|(key, _)| group_order[key]);
        for ((layer, colour), group) in groups {
            if let [part] = &group[..] {
                self.push_piece(&layer, &colour, vec![part.source_id.clone()], &part.path);
                continue;
            }
            // One sweep per group (the engine keeps this fast for thousands of shapes); pairwise
            // overlap tests were quadratic on traced photos.
            let inputs: Vec<_> = group.iter().map(|p| (&p.path, FillRule::NonZero)).collect();
            let united = unite_all(&inputs);
            let total: f64 = inputs.iter().map(|(p, r)| area(p, *r)).sum();
            let largest = inputs.iter().map(|(p, r)| area(p, *r)).fold(0.0, f64::max);
            let got = area(&united, FillRule::NonZero);
            let pieces = split_pieces(&united);
            let owners = (!united.is_empty()
                && got >= largest * (1.0 - 1e-6)
                && got <= total * (1.0 + 1e-6))
                .then(|| attribute_parts(&group, &pieces))
                .flatten();
            let Some(owners) = owners else {
                union_failed.extend(group.iter().map(|p| p.source_id.clone()));
                // Keep the parts unjoined; the blocking diagnostic prevents cutting.
                for part in &group {
                    self.push_piece(&layer, &colour, vec![part.source_id.clone()], &part.path);
                }
                continue;
            };
            // Specks too small to cut that the union's precision dropped are reported.
            specks.extend(
                owners
                    .iter()
                    .zip(&group)
                    .filter(|(owner, _)| owner.is_none())
                    .map(|(_, part)| part.source_id.clone()),
            );
            let owners: Vec<usize> = owners.iter().map(|o| o.unwrap_or(usize::MAX)).collect();
            // Pieces in the paint order of their first source, then as the union gave them;
            // unclaimed specks are left out, credited to the parts they touch.
            let mut order: Vec<usize> = (0..pieces.len())
                .filter(|k| {
                    if owners.contains(k) {
                        return true;
                    }
                    let (outer, _) = &pieces[*k];
                    let speck = subpath_bez(outer, true).bounding_box();
                    specks.extend(
                        group
                            .iter()
                            .filter(|p| {
                                bez_bounds(&p.path)
                                    .is_some_and(|b| overlaps(b.inflate(0.01, 0.01), speck))
                            })
                            .map(|p| p.source_id.clone()),
                    );
                    false
                })
                .collect();
            order.sort_by_key(|&k| {
                let first = owners
                    .iter()
                    .enumerate()
                    .filter(|(_, owner)| **owner == k)
                    .map(|(i, _)| group[i].order)
                    .min();
                (first, k)
            });
            for k in order {
                let sources = dedup_sorted(
                    owners
                        .iter()
                        .enumerate()
                        .filter(|(_, owner)| **owner == k)
                        .map(|(i, _)| group[i].source_id.clone()),
                );
                if sources.len() > 1 {
                    joined.extend(sources.iter().cloned());
                }
                let (outer, holes) = &pieces[k];
                let path = PathData::new(
                    std::iter::once(outer.clone())
                        .chain(holes.iter().cloned())
                        .collect(),
                );
                self.push_piece(&layer, &colour, sources, &path);
            }
        }

        let duplicates = self.push_cut_lines(lines);
        absorbed.extend(duplicates);
        self.report_features();

        let report = |run: &mut Self, ids: Vec<String>, code: &str, message: &str| {
            let ids = dedup_sorted(ids.into_iter());
            if !ids.is_empty() {
                run.change(code, message.replace("{n}", &ids.len().to_string()), ids);
            }
        };
        report(
            self,
            joined,
            "union.joined",
            "Joined {n} overlapping object(s) of the same colour",
        );
        report(
            self,
            specks,
            "speck.dropped",
            "Left out specks under 0.01 mm² in {n} object(s); they are too small to cut",
        );
        report(
            self,
            absorbed,
            "duplicate.removed",
            "Removed duplicate outlines from {n} object(s) (would have cut twice)",
        );
        report(
            self,
            cut_along,
            "stroke.cut_along",
            "Cutting {n} thick line(s) along their centre (your choice)",
        );
        report(
            self,
            shape_chosen,
            "stroke.shape_chosen",
            "Cutting around {n} line(s) as shapes of their own colour (your choice)",
        );
        report(
            self,
            joined_chosen,
            "stroke.joined_chosen",
            "Made {n} outline(s) part of their filled shapes (your choice)",
        );
        for (hex, ids) in joined_fill {
            let ids = dedup_sorted(ids.into_iter());
            let n = ids.len();
            self.changes.push(Change {
                code: "stroke.part_of_shape".into(),
                message: format!(
                    "Made {n} {hex} outline(s) part of their filled shapes: each is cut along its outer edge"
                ),
                count: n,
                source_ids: ids,
                choices: vec![
                    Decision::StrokeColourRole {
                        colour: hex.clone(),
                        role: StrokeRole::Shape,
                    },
                    Decision::StrokeColourRole {
                        colour: hex,
                        role: StrokeRole::CutLine,
                    },
                ],
            });
        }
        for (hex, ids) in outlined {
            let ids = dedup_sorted(ids.into_iter());
            let n = ids.len();
            self.changes.push(Change {
                code: "stroke.outlined".into(),
                message: format!("Turned {n} {hex} line(s) into shapes you can cut out"),
                count: n,
                source_ids: ids,
                choices: vec![Decision::StrokeColourRole {
                    colour: hex,
                    role: StrokeRole::CutLine,
                }],
            });
        }
        report(
            self,
            mask_trimmed,
            "clip.trimmed",
            "Trimmed the hidden parts of {n} object(s) inside clipping masks",
        );
        report(
            self,
            mask_hidden,
            "clip.hidden",
            "Left out {n} object(s) completely hidden by clipping masks",
        );
        report(
            self,
            page_trimmed,
            "page.trimmed",
            "Trimmed {n} object(s) at the page edge",
        );
        report(
            self,
            outside,
            "page.outside",
            "Left out {n} object(s) entirely outside the page: only what is on the page is cut. To cut them, open the design in Illustrator or VectorCraft (or the program that made it), move them onto the page, and save; CutCraft then offers to update the design",
        );
        report(
            self,
            empty,
            "empty.excluded",
            "Left out {n} empty path(s) with nothing to cut",
        );

        let mut block = |ids: Vec<String>, code: &str, message: &str| {
            let ids = dedup_sorted(ids.into_iter());
            if !ids.is_empty() {
                self.diagnostics
                    .push(Diagnostic::blocking(code, message, ids));
            }
        };
        block(
            page_crossing,
            "page.partial",
            "This cut line crosses the page edge. Trimming lines is not implemented yet; move it onto the page or remove it.",
        );
        block(
            mask_crossing,
            "clip.cut_line",
            "This cut line crosses the edge of a clipping mask. Trimming lines is not implemented yet.",
        );
        block(
            invalid,
            "geometry.invalid",
            "This shape's geometry could not be resolved into cuttable regions.",
        );
        block(
            union_failed,
            "union.failed",
            "Overlapping shapes of the same colour could not be joined reliably; they would cut through each other.",
        );
        block(
            outline_failed,
            "stroke.outline_failed",
            "This line could not be turned into a shape. Choose Cut along line, or outline it in an editor.",
        );
        block(
            unsupported_lines,
            "stroke.unsupported",
            "Brushed lines, and dashed, variable-width or arrowed lines cut along their centre, cannot be cut yet.",
        );
        Ok(())
    }

    /// Warnings about what will be hard to cut or weed, and outlines that nearly repeat each
    /// other (spec §5.2 step 5: near-duplicates are reported, not merged).
    fn report_features(&mut self) {
        let mut tiny = Vec::new();
        let mut thin = Vec::new();
        for piece in &self.pieces {
            let shape = piece_bez(piece);
            let area = shape.area().abs();
            if area < TINY_PIECE_MM2 {
                tiny.extend(piece.source_ids.iter().cloned());
                continue;
            }
            let perimeter = shape.perimeter(0.01);
            // Only pieces narrow on average can vanish when inset; skip the rest cheaply.
            if perimeter > 0.0 && 2.0 * area / perimeter < 2.0 * THIN_PIECE_MM {
                let path = PathData::from_bezpath(&shape);
                // An offset that fails also comes back empty; this is only a warning.
                let inset = vectorcraft_pathops::offset_path(
                    &path,
                    -THIN_PIECE_MM,
                    vectorcraft_pathops::Join::Round,
                    4.0,
                );
                if area_of(&inset) < 1e-6 {
                    thin.extend(piece.source_ids.iter().cloned());
                }
            }
        }
        let near = self.near_duplicates();
        // Each is a question (spec §5.2): fix it in the design, or accept it and cut anyway.
        for (code, question, message, accepted, ids) in [
            ("piece.tiny", "small_pieces", format!("Some pieces are smaller than {TINY_PIECE_MM2} mm² (about 1 × 1 mm). They may not cut cleanly and are easy to lose when weeding. Show me finds them: fix them in the design, or cut them anyway."), "Cutting the very small pieces anyway (your choice)", tiny),
            ("piece.thin", "thin_pieces", format!("Some pieces are thinner than about {} mm everywhere. They may tear or lift when weeded. Show me finds them: thicken them in the design, or cut them anyway.", THIN_PIECE_MM * 2.0), "Cutting the very thin pieces anyway (your choice)", thin),
            ("duplicate.near", "near_duplicates", "Some outlines of the same colour nearly sit on top of each other, so the blade would cut almost the same line twice and may tear the vinyl. This is usually a stray copy: Show me finds it, delete it in the design, or keep both.".to_string(), "Keeping outlines that nearly repeat (your choice)", near),
        ] {
            let ids = dedup_sorted(ids.into_iter());
            if ids.is_empty() {
                continue;
            }
            if self.answers.accepted.contains(code) {
                self.changes.push(Change {
                    code: format!("{code}.accepted"),
                    message: accepted.into(),
                    count: ids.len(),
                    source_ids: ids,
                    choices: Vec::new(),
                });
            } else {
                self.pending.push(PendingDecision {
                    decision: question.into(),
                    message,
                    choices: vec![Decision::AcceptWarning {
                        warning: code.into(),
                    }],
                    source_ids: ids,
                });
            }
        }
    }

    /// Same-colour cut lines that nearly repeat another cut line or a piece's edge: bounds
    /// within [`NEAR_DUPLICATE_MM`] and length within 2%. Exact repeats were already removed.
    fn near_duplicates(&self) -> Vec<String> {
        struct Outline<'a> {
            colour: &'a str,
            bounds: Rect,
            length: f64,
            sources: &'a [String],
            line: bool,
        }
        fn outline<'o>(
            c: &Contour,
            colour: &'o str,
            sources: &'o [String],
            line: bool,
        ) -> Outline<'o> {
            let bez = contour_bez(c);
            Outline {
                colour,
                bounds: bez.bounding_box(),
                length: bez.perimeter(0.01),
                sources,
                line,
            }
        }
        let mut all: Vec<Outline> = Vec::new();
        for p in &self.pieces {
            for c in std::iter::once(&p.outer).chain(&p.holes) {
                all.push(outline(c, &p.colour.hex, &p.source_ids, false));
            }
        }
        for l in &self.cut_lines {
            all.push(outline(&l.contour, &l.colour.hex, &l.source_ids, true));
        }
        all.sort_by(|a, b| a.bounds.x0.total_cmp(&b.bounds.x0));
        let mut out = Vec::new();
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                if b.bounds.x0 - a.bounds.x0 > NEAR_DUPLICATE_MM {
                    break;
                }
                let close = |x: f64, y: f64| (x - y).abs() <= NEAR_DUPLICATE_MM;
                let similar = (a.length - b.length).abs() <= a.length.max(b.length) * 0.02;
                if (a.line || b.line)
                    && a.colour == b.colour
                    && close(a.bounds.x1, b.bounds.x1)
                    && close(a.bounds.y0, b.bounds.y0)
                    && close(a.bounds.y1, b.bounds.y1)
                    && similar
                {
                    out.extend(a.sources.iter().chain(b.sources).cloned());
                }
            }
        }
        out
    }

    fn check_stroke_indices(&self, leaves: &[Leaf]) -> Result<(), String> {
        for (id, stroke) in self.answers.strokes.keys() {
            if let Some(leaf) = leaves.iter().find(|l| &l.source_id == id)
                && !leaf.strokes.iter().any(|(i, _)| i == stroke)
            {
                return Err(format!(
                    "The recipe sets stroke {stroke} of {id}, but that object has no visible stroke {stroke}."
                ));
            }
        }
        Ok(())
    }

    fn push_piece(&mut self, layer: &str, colour: &Colour, sources: Vec<String>, path: &PathData) {
        for (outer, holes) in split_pieces(path) {
            for c in std::iter::once(&outer).chain(&holes) {
                let mut c = c.clone();
                c.closed = true;
                self.planned.insert((colour.clone(), canonical(&c)));
            }
            self.pieces.push(Piece {
                id: format!("piece/{}", self.pieces.len()),
                layer: layer.into(),
                colour: colour.clone(),
                source_ids: sources.clone(),
                outer: contour(&outer, true),
                holes: holes.iter().map(|h| contour(h, true)).collect(),
            });
        }
    }

    /// Add cut lines, skipping exact duplicates of a cut already planned in the same colour.
    /// Returns the source ids whose duplicates were removed.
    fn push_cut_lines(
        &mut self,
        mut lines: Vec<(usize, String, Colour, String, SubPath)>,
    ) -> Vec<String> {
        lines.sort_by_key(|l| l.0);
        let mut removed = Vec::new();
        for (_, layer, colour, source_id, sp) in lines {
            let key = canonical(&sp);
            if !self.planned.insert((colour.clone(), key)) {
                removed.push(source_id);
                continue;
            }
            self.cut_lines.push(CutLine {
                id: format!("line/{}", self.cut_lines.len()),
                layer,
                colour,
                source_ids: vec![source_id],
                contour: contour(&sp, sp.closed),
            });
        }
        removed
    }

    fn finish(mut self, bytes: &[u8], recipe: &Recipe, page: Option<Rect>) -> PreparedDesign {
        let page = page.map(|rect| {
            let index = self
                .imported
                .doc
                .artboards
                .iter()
                .position(|a| a.rect == rect)
                .unwrap_or(0);
            Page {
                id: if self.fitted {
                    "artwork".into()
                } else {
                    format!("page/{index}")
                },
                width_mm: rect.width() * MM_PER_PT,
                height_mm: rect.height() * MM_PER_PT,
                origin_pt: Some([rect.x0, rect.y0]),
            }
        });
        if page.is_some()
            && self.pieces.is_empty()
            && self.cut_lines.is_empty()
            && self.pending.is_empty()
            && !self
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Blocking)
        {
            // Only when nothing else explains the empty result.
            let off_artboard = self
                .imported
                .inspection
                .diagnostics
                .iter()
                .any(|d| d.code == "import.off_artboard_skipped");
            self.diagnostics.push(Diagnostic::blocking(
                "preparation.empty",
                if off_artboard {
                    "Nothing to cut was found on the page. This file's art lies outside its artboard, where only its editor can open it: move the art onto the artboard and save the file again."
                } else {
                    "Nothing to cut was found on this page."
                },
                vec![],
            ));
        }
        let cut_ready = page.is_some()
            && self.pending.is_empty()
            && !self
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Blocking);
        PreparedDesign {
            schema_version: PREPARED_SCHEMA_VERSION,
            stage: "preparation".into(),
            cut_ready,
            engine_revision: ENGINE_REVISION.into(),
            source_sha256: format!("{:x}", Sha256::digest(bytes)),
            recipe: recipe.clone(),
            fonts: self.fonts,
            page,
            pieces: self.pieces,
            cut_lines: self.cut_lines,
            changes: self.changes,
            pending: self.pending,
            diagnostics: self.diagnostics,
            stroked_fills: self.stroked_fills,
        }
    }
}

/// Which edges trimmed an object's geometry.
#[derive(Clone, Copy, Default)]
struct Trim {
    mask: bool,
    page: bool,
}

impl Trim {
    fn add(&mut self, other: Trim) {
        self.mask |= other.mask;
        self.page |= other.page;
    }
}

fn rect_region(rect: Rect) -> PathData {
    PathData::from_bezpath(&rect.to_path(0.1))
}

/// The part of a filled region inside the clip and then the page, with the edges that removed
/// any of it. An untouched region is returned as is, so its curves are not refitted.
fn trim_region(
    region: &PathData,
    clip: Option<&PathData>,
    page: Rect,
) -> Result<(PathData, Trim), vectorcraft_pathops::PathOpsError> {
    let mut trim = Trim::default();
    let mut current = region.clone();
    let cut = |current: &mut PathData, by: &PathData| -> Result<bool, _> {
        let before = area(current, FillRule::NonZero);
        let next = try_boolean(
            current,
            FillRule::NonZero,
            by,
            FillRule::NonZero,
            BoolOp::Intersect,
            DEFAULT_PRECISION,
        )?;
        // An intersection never adds area, so equal area means nothing was removed.
        let removed = before - area(&next, FillRule::NonZero) > before * 1e-6 + 1e-9;
        if removed {
            *current = next;
        }
        Ok(removed)
    };
    if let Some(clip) = clip {
        trim.mask = cut(&mut current, clip)?;
    }
    let inside_page = bez_bounds(&current).is_none_or(|b| {
        page.inflate(PAGE_EDGE_TOLERANCE_PT, PAGE_EDGE_TOLERANCE_PT)
            .contains_rect(b)
    });
    if !inside_page {
        trim.page = cut(&mut current, &rect_region(page))?;
    }
    Ok((current, trim))
}

enum Fit {
    Inside,
    Outside,
    Crossing,
}

fn polyline(bp: &BezPath) -> Vec<Vec<Point>> {
    let mut lines: Vec<Vec<Point>> = Vec::new();
    kurbo::flatten(bp, 0.01, |el| match el {
        kurbo::PathEl::MoveTo(p) => lines.push(vec![p]),
        kurbo::PathEl::LineTo(p) => {
            if let Some(l) = lines.last_mut() {
                l.push(p);
            }
        }
        kurbo::PathEl::ClosePath => {
            if let Some(l) = lines.last_mut()
                && let Some(&first) = l.first()
            {
                l.push(first);
            }
        }
        _ => {}
    });
    lines
}

/// Where a cut line lies relative to a filled region: wholly inside, wholly outside, or
/// crossing (or touching) its edge.
fn line_fit(sp: &SubPath, region: &PathData) -> Fit {
    let line = polyline(&subpath_bez(sp, sp.closed));
    let shape = region.to_bezpath();
    let mut edges = Vec::new();
    for sp in &region.subpaths {
        edges.extend(polyline(&subpath_bez(sp, true)));
    }
    let crosses = |a: Point, b: Point, c: Point, d: Point| {
        let orient = |p: Point, q: Point, r: Point| (q - p).cross(r - p);
        let (d1, d2) = (orient(c, d, a), orient(c, d, b));
        let (d3, d4) = (orient(a, b, c), orient(a, b, d));
        let box_overlap = a.x.min(b.x) <= c.x.max(d.x)
            && c.x.min(d.x) <= a.x.max(b.x)
            && a.y.min(b.y) <= c.y.max(d.y)
            && c.y.min(d.y) <= a.y.max(b.y);
        box_overlap && d1 * d2 <= 0.0 && d3 * d4 <= 0.0
    };
    for l in &line {
        for seg in l.windows(2) {
            for e in &edges {
                for edge in e.windows(2) {
                    if crosses(seg[0], seg[1], edge[0], edge[1]) {
                        return Fit::Crossing;
                    }
                }
            }
        }
    }
    match line.first().and_then(|l| l.first()) {
        Some(&p) if shape.winding(p) != 0 => Fit::Inside,
        _ => Fit::Outside,
    }
}

/// Can this font's text be cut from the file's PDF part faithfully? Only when the file carries
/// its glyphs: checked against the archive, a font the file does not carry is drawn with a
/// stand-in there even when it is installed.
fn shapes_available(font: &crate::inspect::Font, embedded: &BTreeSet<String>) -> bool {
    embedded.contains(&font_key(&font.family))
}

/// Is `id` the object `ancestor` or inside it?
fn within(id: &str, ancestor: &str) -> bool {
    id == ancestor
        || id
            .strip_prefix(ancestor)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Do the rectangles share interior? A zero-height line inside the page still overlaps it.
fn overlaps(a: Rect, b: Rect) -> bool {
    a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
}

/// A size as they read it (spec §3): inches first, millimetres after.
fn size_text(width_mm: f64, height_mm: f64) -> String {
    format!(
        "{:.2} × {:.2} in ({} × {} mm)",
        width_mm / 25.4,
        height_mm / 25.4,
        round(width_mm, 0.1),
        round(height_mm, 0.1)
    )
}

fn round(value: f64, quantum: f64) -> f64 {
    let r = (value / quantum).round() * quantum;
    // Avoid printing artefacts such as 25.400000000000002.
    (r * 1e9).round() / 1e9
}

/// Spec §5.2 step 3: `CutContour` strokes are cut lines; a stroke on a filled object is part of
/// that shape; other hairlines are cut lines and all other strokes are shapes.
fn default_role(width_pt: f64, colour: &Colour, filled: bool) -> StrokeRole {
    // Relative tolerance: unit conversion leaves 0.1 mm a few parts in 10⁸ over.
    let hairline = width_pt * MM_PER_PT <= HAIRLINE_MM * (1.0 + 1e-6);
    let cut_contour = colour
        .swatch
        .as_deref()
        .is_some_and(|s| s.eq_ignore_ascii_case("CutContour"));
    if cut_contour {
        StrokeRole::CutLine
    } else if filled {
        StrokeRole::PartOfShape
    } else if hairline {
        StrokeRole::CutLine
    } else {
        StrokeRole::Shape
    }
}

fn colour_of(paint: &Paint) -> Option<Colour> {
    match paint {
        Paint::Solid { color, swatch, .. } => Some(Colour {
            hex: color.to_hex(),
            swatch: swatch.clone(),
        }),
        _ => None,
    }
}

fn subpath_bez(sp: &SubPath, closed: bool) -> BezPath {
    let mut bp = BezPath::new();
    let mut sp = sp.clone();
    sp.closed = closed;
    sp.to_bezpath_into(&mut bp);
    bp
}

fn bez_bounds(path: &PathData) -> Option<Rect> {
    let bp = path.to_bezpath();
    (!bp.elements().is_empty()).then(|| bp.bounding_box())
}

fn is_page_rectangle(region: &PathData, page: Rect) -> bool {
    let tolerance = BACKGROUND_TOLERANCE_MM / MM_PER_PT;
    let Some(bounds) = bez_bounds(region) else {
        return false;
    };
    let close = |a: f64, b: f64| (a - b).abs() <= tolerance;
    region.subpaths.len() == 1
        && close(bounds.x0, page.x0)
        && close(bounds.y0, page.y0)
        && close(bounds.x1, page.x1)
        && close(bounds.y1, page.y1)
        && (area(region, FillRule::NonZero) - bounds.area()).abs() <= bounds.area() * 1e-3
}

/// Group simple, non-crossing contours into pieces: an outer contour (even nesting depth) and
/// the holes directly inside it (odd depth). Shapes inside holes become their own pieces.
fn split_pieces(path: &PathData) -> Vec<(SubPath, Vec<SubPath>)> {
    let contours: Vec<&SubPath> = path
        .subpaths
        .iter()
        .filter(|s| s.anchors.len() >= 2)
        .collect();
    let shapes: Vec<BezPath> = contours.iter().map(|c| subpath_bez(c, true)).collect();
    let sample = |c: &SubPath| -> Point {
        // A point on the first segment, away from the anchor where contours may touch.
        let seg: CubicBez = c.segment(0);
        seg.eval(0.5)
    };
    let containers: Vec<Vec<usize>> = contours
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let p = sample(c);
            (0..contours.len())
                .filter(|&j| j != i && shapes[j].winding(p) != 0)
                .collect()
        })
        .collect();
    let depth = |i: usize| containers[i].len();
    let mut pieces: Vec<(usize, SubPath, Vec<SubPath>)> = Vec::new();
    for (i, c) in contours.iter().enumerate() {
        if depth(i) % 2 == 0 {
            pieces.push((i, (*c).clone(), Vec::new()));
        }
    }
    for (i, c) in contours.iter().enumerate() {
        if depth(i) % 2 == 1 {
            // The parent is the containing contour one level up.
            if let Some(parent) = containers[i].iter().find(|&&j| depth(j) + 1 == depth(i))
                && let Some(piece) = pieces.iter_mut().find(|p| p.0 == *parent)
            {
                piece.2.push((*c).clone());
            }
        }
    }
    pieces
        .into_iter()
        .map(|(_, outer, holes)| (outer, holes))
        .collect()
}

/// Which united piece each part lies in: a union never splits a part, so a point inside the
/// part lies in exactly one piece (inside its outer contour, outside its holes). A speck under
/// [`SPECK_PT2`] that the union's precision dropped has no piece (`None` in the list). `None`
/// overall when a larger part cannot be placed or a piece has no part: a failed union.
fn attribute_parts(
    parts: &[Part],
    pieces: &[(SubPath, Vec<SubPath>)],
) -> Option<Vec<Option<usize>>> {
    let shapes: Vec<(BezPath, Vec<BezPath>)> = pieces
        .iter()
        .map(|(outer, holes)| {
            (
                subpath_bez(outer, true),
                holes.iter().map(|h| subpath_bez(h, true)).collect(),
            )
        })
        .collect();
    let owners: Option<Vec<Option<usize>>> = parts
        .iter()
        .map(|part| {
            let placed = vectorcraft_pathops::interior_point(&part.path).and_then(|p| {
                shapes.iter().position(|(outer, holes)| {
                    outer.winding(p) != 0 && holes.iter().all(|h| h.winding(p) == 0)
                })
            });
            match placed {
                Some(k) => Some(Some(k)),
                None if area(&part.path, FillRule::NonZero) < SPECK_PT2 => Some(None),
                None => None,
            }
        })
        .collect();
    let owners = owners?;
    // A sliver the union made that no part claims is a speck too; a larger one is a failure.
    let unowned_ok = (0..pieces.len()).all(|k| {
        owners.contains(&Some(k)) || {
            let path = PathData::new(
                std::iter::once(pieces[k].0.clone())
                    .chain(pieces[k].1.iter().cloned())
                    .collect(),
            );
            area(&path, FillRule::NonZero) < SPECK_PT2
        }
    });
    unowned_ok.then_some(owners)
}

fn dedup_sorted(ids: impl Iterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// A prepared contour as a closed or open curve, in mm.
fn contour_bez(c: &Contour) -> BezPath {
    let mut bp = BezPath::new();
    bp.move_to((c.start[0], c.start[1]));
    for [a, b, e] in &c.segments {
        bp.curve_to((a[0], a[1]), (b[0], b[1]), (e[0], e[1]));
    }
    if c.closed {
        bp.close_path();
    }
    bp
}

/// A piece's outline and holes as one filled curve, in mm.
fn piece_bez(piece: &Piece) -> BezPath {
    let mut bp = contour_bez(&piece.outer);
    for hole in &piece.holes {
        bp.extend(contour_bez(hole));
    }
    bp
}

fn area_of(path: &PathData) -> f64 {
    area(path, FillRule::NonZero)
}

fn mm(p: Point) -> [f64; 2] {
    [
        round(p.x * MM_PER_PT, OUTPUT_QUANTUM_MM),
        round(p.y * MM_PER_PT, OUTPUT_QUANTUM_MM),
    ]
}

fn contour(sp: &SubPath, closed: bool) -> Contour {
    let mut sp = sp.clone();
    sp.closed = closed;
    Contour {
        start: mm(sp.anchors[0].p),
        segments: (0..sp.segment_count())
            .map(|i| {
                let c = sp.segment(i);
                [mm(c.p1), mm(c.p2), mm(c.p3)]
            })
            .collect(),
        closed,
    }
}

/// A direction- and start-independent key for duplicate detection: the lexicographically
/// smallest sequence of segment points over both directions and, if closed, every rotation.
fn canonical_key(segments: Vec<[[i64; 2]; 4]>, closed: bool) -> Vec<[i64; 2]> {
    let reversed: Vec<[[i64; 2]; 4]> = segments
        .iter()
        .rev()
        .map(|s| [s[3], s[2], s[1], s[0]])
        .collect();
    let flatten = |segs: &[[[i64; 2]; 4]]| -> Vec<[i64; 2]> {
        segs.iter().flat_map(|s| s.iter().copied()).collect()
    };
    let mut best: Option<Vec<[i64; 2]>> = None;
    for direction in [&segments, &reversed] {
        let rotations = if closed { direction.len().max(1) } else { 1 };
        for r in 0..rotations {
            let mut rotated = direction[r..].to_vec();
            rotated.extend_from_slice(&direction[..r]);
            let key = flatten(&rotated);
            if best.as_ref().is_none_or(|b| key < *b) {
                best = Some(key);
            }
        }
    }
    best.unwrap_or_default()
}

fn quantize(p: Point) -> [i64; 2] {
    let q = |v: f64| (v / DUPLICATE_QUANTUM_PT).round() as i64;
    [q(p.x), q(p.y)]
}

fn canonical(sp: &SubPath) -> Vec<[i64; 2]> {
    let segments = (0..sp.segment_count())
        .map(|i| {
            let c = sp.segment(i);
            [
                quantize(c.p0),
                quantize(c.p1),
                quantize(c.p2),
                quantize(c.p3),
            ]
        })
        .collect();
    canonical_key(segments, sp.closed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inspect::import_bytes;

    /// A 100 × 100 mm page where one user unit is one millimetre.
    fn svg(content: &str) -> Vec<u8> {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">{content}</svg>"#
        )
        .into_bytes()
    }

    fn prepare(bytes: &[u8], decisions: Vec<Decision>) -> Result<PreparedDesign, String> {
        let recipe = Recipe {
            decisions,
            ..Recipe::new()
        };
        prepare_bytes(Path::new("original.svg"), bytes, &recipe)
    }

    fn ok(content: &str) -> PreparedDesign {
        prepare(&svg(content), vec![]).unwrap()
    }

    fn codes(design: &PreparedDesign) -> Vec<&str> {
        design.diagnostics.iter().map(|d| d.code.as_str()).collect()
    }

    fn change<'a>(design: &'a PreparedDesign, code: &str) -> &'a Change {
        design
            .changes
            .iter()
            .find(|c| c.code == code)
            .unwrap_or_else(|| panic!("no {code} change in {:?}", design.changes))
    }

    /// Bounds of a contour's anchor and control points, in mm.
    fn bounds(c: &Contour) -> [f64; 4] {
        let mut b = [c.start[0], c.start[1], c.start[0], c.start[1]];
        for p in c.segments.iter().flatten() {
            b = [
                b[0].min(p[0]),
                b[1].min(p[1]),
                b[2].max(p[0]),
                b[3].max(p[1]),
            ];
        }
        b
    }

    fn close(a: [f64; 4], b: [f64; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn a_plain_filled_shape_is_cut_ready_at_its_physical_position() {
        let design = ok(r#"<rect x="10" y="20" width="30" height="40" fill="black"/>"#);
        assert!(design.cut_ready, "{:?}", design.diagnostics);
        assert_eq!(design.pieces.len(), 1);
        let piece = &design.pieces[0];
        assert_eq!(piece.colour.hex, "#000000");
        assert_eq!(piece.source_ids, ["source/0/0"]);
        assert!(piece.holes.is_empty() && piece.outer.closed);
        assert!(close(bounds(&piece.outer), [10.0, 20.0, 40.0, 60.0]));
        let page = design.page.unwrap();
        // The importer's mm → pt conversion is exact to a few parts in 10⁸.
        assert!((page.width_mm - 100.0).abs() < 1e-5 && page.id == "page/0");
    }

    #[test]
    fn letter_holes_stay_holes_and_islands_inside_holes_are_separate_pieces() {
        let design = ok(
            r#"<path fill-rule="evenodd" d="M10 10H90V90H10Z M30 30H70V70H30Z"/>
            <rect x="40" y="40" width="20" height="20"/>"#,
        );
        assert!(design.cut_ready, "{:?}", design.diagnostics);
        assert_eq!(design.pieces.len(), 2);
        let ring = &design.pieces[0];
        assert!(close(bounds(&ring.outer), [10.0, 10.0, 90.0, 90.0]));
        assert_eq!(ring.holes.len(), 1);
        assert!(close(bounds(&ring.holes[0]), [30.0, 30.0, 70.0, 70.0]));
        let island = &design.pieces[1];
        assert!(island.holes.is_empty());
        assert!(close(bounds(&island.outer), [40.0, 40.0, 60.0, 60.0]));
        assert!(design.changes.iter().all(|c| c.code != "union.joined"));
    }

    #[test]
    fn nonzero_self_overlap_resolves_without_a_false_hole() {
        // Two same-direction squares in one path: non-zero fills their overlap.
        let design = ok(r#"<path d="M10 10H60V60H10Z M40 40H90V90H40Z"/>"#);
        assert_eq!(design.pieces.len(), 1);
        assert!(design.pieces[0].holes.is_empty());
        assert!(close(
            bounds(&design.pieces[0].outer),
            [10.0, 10.0, 90.0, 90.0]
        ));
    }

    #[test]
    fn overlapping_and_touching_same_colour_shapes_join_per_layer_and_colour() {
        let design = ok(r#"<g>
            <rect x="10" y="10" width="30" height="30" fill="red"/>
            <rect x="30" y="30" width="30" height="30" fill="red"/>
            <rect x="60" y="30" width="20" height="20" fill="red"/>
            <rect x="10" y="70" width="20" height="20" fill="red"/>
            <rect x="20" y="20" width="10" height="10" fill="blue"/>
        </g>"#);
        assert!(design.cut_ready, "{:?}", design.diagnostics);
        let red: Vec<_> = design
            .pieces
            .iter()
            .filter(|p| p.colour.hex == "#ff0000")
            .collect();
        // The first two overlap; the third shares the second's right edge at x = 60.
        assert_eq!(red.len(), 2, "{red:?}");
        assert_eq!(
            red[0].source_ids,
            ["source/0/0/0", "source/0/0/1", "source/0/0/2"]
        );
        assert!(close(bounds(&red[0].outer), [10.0, 10.0, 80.0, 60.0]));
        let corner = ok(
            r#"<rect x="10" y="10" width="20" height="20"/><rect x="30" y="30" width="20" height="20"/>"#,
        );
        assert_eq!(
            corner.pieces.len(),
            2,
            "shapes meeting only at a corner stay separate"
        );
        assert_eq!(red[1].source_ids, ["source/0/0/3"]);
        let blue: Vec<_> = design
            .pieces
            .iter()
            .filter(|p| p.colour.hex == "#0000ff")
            .collect();
        assert_eq!(blue.len(), 1, "a different colour never joins");
        assert_eq!(change(&design, "union.joined").count, 3);
    }

    #[test]
    fn same_colour_shapes_on_different_layers_stay_separate() {
        // Root groups with ids import as layers, as in Illustrator's SVG export.
        let design = ok(
            r#"<g id="A"><rect x="10" y="10" width="30" height="30"/></g>
            <g id="B"><rect x="20" y="20" width="30" height="30"/></g>"#,
        );
        assert!(design.cut_ready, "{:?}", design.diagnostics);
        let layers: Vec<_> = design.pieces.iter().map(|p| p.layer.as_str()).collect();
        assert_eq!(layers, ["source/0", "source/1"]);
        assert!(design.changes.iter().all(|c| c.code != "union.joined"));
    }

    #[test]
    fn hairlines_are_cut_lines_and_thicker_strokes_become_shapes() {
        let design =
            ok(r#"<path d="M10 10L90 90" fill="none" stroke="black" stroke-width="0.1"/>"#);
        assert!(design.cut_ready, "{:?}", design.diagnostics);
        assert_eq!(design.cut_lines.len(), 1);
        let line = &design.cut_lines[0];
        assert!(!line.contour.closed);
        assert!(close(bounds(&line.contour), [10.0, 10.0, 90.0, 90.0]));

        let thick = r#"<path d="M10 10L90 90" fill="none" stroke="black" stroke-width="0.5"/>"#;
        let shaped = ok(thick);
        assert!(shaped.cut_ready, "{:?}", shaped.diagnostics);
        assert!(shaped.cut_lines.is_empty() && shaped.pending.is_empty());
        assert_eq!(shaped.pieces.len(), 1);
        // A butt-capped band: length × width.
        let length = 80.0 * std::f64::consts::SQRT_2;
        assert!((piece_area(&shaped.pieces[0]) - length * 0.5).abs() < 1e-3);
        let outlined = change(&shaped, "stroke.outlined");
        assert_eq!(outlined.source_ids, ["source/0/0"]);
        let along = outlined.choices[0].clone();
        assert_eq!(
            along,
            Decision::StrokeColourRole {
                colour: "#000000".into(),
                role: StrokeRole::CutLine
            }
        );

        let by_colour = prepare(&svg(thick), vec![along.clone()]).unwrap();
        assert!(by_colour.cut_ready, "{:?}", by_colour.diagnostics);
        assert_eq!((by_colour.pieces.len(), by_colour.cut_lines.len()), (0, 1));
        assert_eq!(change(&by_colour, "stroke.cut_along").count, 1);

        // An object's own choice beats its colour's.
        let object_shape = Decision::StrokeRole {
            source_id: "source/0/0".into(),
            stroke: 0,
            role: StrokeRole::Shape,
        };
        let both = prepare(&svg(thick), vec![along, object_shape.clone()]).unwrap();
        assert_eq!((both.pieces.len(), both.cut_lines.len()), (1, 0));

        let hairline_shape = prepare(
            &svg(r#"<path d="M10 10L90 90" fill="none" stroke="black" stroke-width="0.1"/>"#),
            vec![object_shape],
        )
        .unwrap();
        assert!((piece_area(&hairline_shape.pieces[0]) - length * 0.1).abs() < 1e-3);
        assert_eq!(change(&hairline_shape, "stroke.shape_chosen").count, 1);

        let stale = prepare(
            &svg(thick),
            vec![Decision::StrokeRole {
                source_id: "source/0/0".into(),
                stroke: 1,
                role: StrokeRole::CutLine,
            }],
        );
        assert!(stale.unwrap_err().contains("no visible stroke 1"));
        let stale = prepare(
            &svg(thick),
            vec![Decision::StrokeColourRole {
                colour: "#ff0000".into(),
                role: StrokeRole::CutLine,
            }],
        );
        assert!(stale.unwrap_err().contains("no #ff0000 lines"));
    }

    /// Filled area of a piece in mm²: its outer contour minus its holes.
    fn piece_area(piece: &Piece) -> f64 {
        let contour_area = |c: &Contour| {
            let mut bp = BezPath::new();
            bp.move_to((c.start[0], c.start[1]));
            for s in &c.segments {
                bp.curve_to((s[0][0], s[0][1]), (s[1][0], s[1][1]), (s[2][0], s[2][1]));
            }
            bp.close_path();
            bp.area().abs()
        };
        contour_area(&piece.outer) - piece.holes.iter().map(contour_area).sum::<f64>()
    }

    #[test]
    fn closed_strokes_become_rings_and_combine_with_a_same_colour_fill() {
        let ring = ok(
            r#"<rect x="10" y="10" width="20" height="20" fill="none" stroke="black" stroke-width="2"/>"#,
        );
        assert!(ring.cut_ready, "{:?}", ring.diagnostics);
        assert_eq!(ring.pieces.len(), 1);
        assert!(close(bounds(&ring.pieces[0].outer), [9.0, 9.0, 31.0, 31.0]));
        assert_eq!(ring.pieces[0].holes.len(), 1);
        assert!(close(
            bounds(&ring.pieces[0].holes[0]),
            [11.0, 11.0, 29.0, 29.0]
        ));

        // Spec §5.2 step 5: a same-colour fill and outlined stroke cut as one region.
        let same = ok(
            r#"<rect x="10" y="10" width="20" height="20" fill="black" stroke="black" stroke-width="2"/>"#,
        );
        assert_eq!(same.pieces.len(), 1);
        assert!(same.pieces[0].holes.is_empty());
        assert!(close(bounds(&same.pieces[0].outer), [9.0, 9.0, 31.0, 31.0]));
        assert!(
            same.changes.iter().all(|c| c.code != "union.joined"),
            "one object is not a join"
        );

        // Another colour's stroke is part of the shape by default: cut along its outer edge.
        let red_edge = r#"<rect x="10" y="10" width="20" height="20" fill="black" stroke="red" stroke-width="2"/>"#;
        let other = ok(red_edge);
        assert_eq!(other.pieces.len(), 1);
        assert_eq!(other.pieces[0].colour.hex, "#000000");
        assert!(close(
            bounds(&other.pieces[0].outer),
            [9.0, 9.0, 31.0, 31.0]
        ));
        let joined = change(&other, "stroke.part_of_shape");
        assert_eq!(joined.source_ids, ["source/0/0"]);
        // *Cut around these as shapes*: the red ring is its own piece again.
        let separate = prepare(&svg(red_edge), vec![joined.choices[0].clone()]).unwrap();
        let colours: Vec<_> = separate
            .pieces
            .iter()
            .map(|p| p.colour.hex.as_str())
            .collect();
        assert_eq!(colours, ["#000000", "#ff0000"]);
        assert_eq!(separate.pieces[1].holes.len(), 1);
    }

    #[test]
    fn dashed_thick_lines_become_their_visible_dashes() {
        let design = ok(
            r#"<path d="M10 50H90" fill="none" stroke="black" stroke-width="2" stroke-dasharray="10 10"/>"#,
        );
        assert!(design.cut_ready, "{:?}", design.diagnostics);
        let mut xs: Vec<[f64; 4]> = design.pieces.iter().map(|p| bounds(&p.outer)).collect();
        xs.sort_by(|a, b| a[0].total_cmp(&b[0]));
        assert_eq!(xs.len(), 4);
        for (dash, x) in xs.iter().zip([10.0, 30.0, 50.0, 70.0]) {
            assert!(close(*dash, [x, 49.0, x + 10.0, 51.0]), "{dash:?}");
        }
    }

    #[test]
    fn stroke_outlines_match_the_engines_outline_stroke_command() {
        let bytes = svg(
            r#"<path d="M10 80C30 10 60 10 90 80" fill="none" stroke="black" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>
            <path d="M20 90L50 60L80 90" fill="none" stroke="black" stroke-width="4" stroke-linejoin="miter"/>"#,
        );
        let design = prepare(&bytes, vec![]).unwrap();
        assert!(design.cut_ready, "{:?}", design.diagnostics);
        let ours: f64 = design.pieces.iter().map(piece_area).sum::<f64>() / (MM_PER_PT * MM_PER_PT);
        let imported = import_bytes(Path::new("original.svg"), &bytes).unwrap();
        let mut session = vectorcraft_engine::Session::new();
        session.add_document(imported.doc.clone(), None);
        let mut ids = Vec::new();
        imported.doc.walk(|n| {
            if matches!(n.kind, NodeKind::Path { .. }) {
                ids.push(n.id.0);
            }
        });
        session
            .execute("select.set", &serde_json::json!({"ids": ids}))
            .unwrap();
        session
            .execute("object.path.outlineStroke", &serde_json::json!({}))
            .unwrap();
        let mut outlines = Vec::new();
        session.doc().unwrap().doc.walk(|n| {
            if let NodeKind::Path { path, rule, .. } = &n.kind {
                outlines.push((path.clone(), *rule));
            }
        });
        // The two strokes do not overlap, so their areas add.
        let engine: f64 = outlines.iter().map(|(p, r)| area(p, *r)).sum();
        assert!((ours - engine).abs() < engine * 1e-4, "{ours} vs {engine}");
    }

    #[test]
    fn a_cut_contour_swatch_is_a_cut_line_whatever_its_width() {
        // Swatch links come from native editing data, so classify the imported paint directly.
        let paint = |swatch: Option<&str>| Paint::Solid {
            color: vectorcraft_color::Color::BLACK,
            swatch: swatch.map(Into::into),
            tint: 1.0,
        };
        let one_pt = 1.0;
        let cut_contour = colour_of(&paint(Some("cutcontour"))).unwrap();
        assert_eq!(
            default_role(one_pt, &cut_contour, false),
            StrokeRole::CutLine
        );
        let plain = colour_of(&paint(None)).unwrap();
        assert_eq!(default_role(one_pt, &plain, false), StrokeRole::Shape);
        assert_eq!(
            default_role(HAIRLINE_MM / MM_PER_PT, &plain, false),
            StrokeRole::CutLine
        );
        assert_eq!(
            default_role(0.11 / MM_PER_PT, &plain, false),
            StrokeRole::Shape
        );
        // On a filled object, any stroke but a CutContour one is part of the shape.
        assert_eq!(default_role(one_pt, &plain, true), StrokeRole::PartOfShape);
        assert_eq!(
            default_role(HAIRLINE_MM / MM_PER_PT, &plain, true),
            StrokeRole::PartOfShape
        );
        assert_eq!(
            default_role(one_pt, &cut_contour, true),
            StrokeRole::CutLine
        );
    }

    #[test]
    fn outlines_on_filled_shapes_can_be_cut_separately_for_the_whole_design() {
        let design = r##"<rect x="10" y="10" width="20" height="20" fill="black" stroke="black" stroke-width="2"/>
            <rect x="50" y="10" width="20" height="20" fill="#00aa00" stroke="red" stroke-width="2"/>
            <path d="M10 60L90 60" fill="none" stroke="blue" stroke-width="2"/>"##;
        let joined = ok(design);
        assert_eq!(joined.stroked_fills, 2, "the open blue line has no fill");
        // Black square, green square, blue band.
        assert_eq!(joined.pieces.len(), 3);
        let separate = Decision::OutlinesOnFills {
            role: StrokeRole::Shape,
        };
        let apart = prepare(&svg(design), vec![separate.clone()]).unwrap();
        assert!(apart.cut_ready, "{:?}", apart.diagnostics);
        assert_eq!(apart.stroked_fills, 2);
        let mut colours: Vec<_> = apart.pieces.iter().map(|p| p.colour.hex.as_str()).collect();
        colours.sort_unstable();
        // The black ring joins its own black square; the red ring is a piece of its own.
        assert_eq!(colours, ["#000000", "#0000ff", "#00aa00", "#ff0000"]);
        // A colour's own choice still wins over the design-wide one.
        let red_joined = Decision::StrokeColourRole {
            colour: "#ff0000".into(),
            role: StrokeRole::PartOfShape,
        };
        let mixed = prepare(&svg(design), vec![separate, red_joined]).unwrap();
        assert!(mixed.pieces.iter().all(|p| p.colour.hex != "#ff0000"));
    }

    #[test]
    fn hairline_outlines_are_part_of_their_fill_unless_cut_along() {
        let same = ok(
            r#"<rect x="10" y="10" width="30" height="30" fill="black" stroke="black" stroke-width="0.05"/>"#,
        );
        assert!(same.cut_ready, "{:?}", same.diagnostics);
        assert_eq!((same.pieces.len(), same.cut_lines.len()), (1, 0));
        assert!(same.changes.is_empty(), "{:?}", same.changes);

        let red_hairline = r#"<rect x="10" y="10" width="30" height="30" fill="black" stroke="red" stroke-width="0.05"/>"#;
        let other = ok(red_hairline);
        assert_eq!((other.pieces.len(), other.cut_lines.len()), (1, 0));
        let joined = change(&other, "stroke.part_of_shape");
        // *Cut these along their centre*: a red cut line beside the black piece, as before.
        let along = prepare(&svg(red_hairline), vec![joined.choices[1].clone()]).unwrap();
        assert_eq!((along.pieces.len(), along.cut_lines.len()), (1, 1));
        assert_eq!(along.cut_lines[0].colour.hex, "#ff0000");
        // A stroke on an unfilled path is never part of a shape.
        let open = ok(r#"<path d="M10 10L90 90" fill="none" stroke="red" stroke-width="0.05"/>"#);
        assert_eq!((open.pieces.len(), open.cut_lines.len()), (0, 1));
    }

    #[test]
    fn exact_duplicate_cuts_are_removed_regardless_of_direction_or_start() {
        let design = ok(r#"
            <rect x="10" y="10" width="30" height="30" fill="black"/>
            <path d="M40 40V10H10V40Z" fill="none" stroke="black" stroke-width="0.05"/>
            <path d="M60 60H90V90H60Z" fill="none" stroke="black" stroke-width="0.05"/>
            <path d="M90 60V90H60V60Z" fill="none" stroke="black" stroke-width="0.05"/>
            <path d="M60 60H90V90H60Z" fill="none" stroke="red" stroke-width="0.05"/>"#);
        assert!(design.cut_ready, "{:?}", design.diagnostics);
        assert_eq!(design.pieces.len(), 1);
        // The first outline repeats the black square; the reversed square repeats the third.
        // The red copy is a different colour, so it is kept.
        assert_eq!(design.cut_lines.len(), 2);
        assert_eq!(design.cut_lines[0].source_ids, ["source/0/2"]);
        assert_eq!(design.cut_lines[1].colour.hex, "#ff0000");
        assert_eq!(
            change(&design, "duplicate.removed").source_ids,
            ["source/0/1", "source/0/3"]
        );
    }

    #[test]
    fn a_page_sized_background_needs_an_answer_and_white_art_is_never_dropped() {
        let content = r#"<rect width="100" height="100" fill="white"/>
            <rect x="10" y="10" width="20" height="20" fill="white"/>
            <rect x="50" y="50" width="20" height="20" fill="black"/>"#;
        let pending = ok(content);
        assert!(!pending.cut_ready);
        assert_eq!(pending.pending.len(), 1);
        assert_eq!(pending.pending[0].decision, "background");
        assert_eq!(pending.pending[0].source_ids, ["source/0/0"]);
        // Shown without the candidate until answered; the white square is still artwork.
        assert_eq!(pending.pieces.len(), 2);

        let answer = |cut| Decision::Background {
            source_id: "source/0/0".into(),
            cut,
        };
        let dropped = prepare(&svg(content), vec![answer(false)]).unwrap();
        assert!(dropped.cut_ready, "{:?}", dropped.diagnostics);
        assert_eq!(dropped.pieces.len(), 2);
        assert_eq!(change(&dropped, "background.excluded").count, 1);

        let kept = prepare(&svg(content), vec![answer(true)]).unwrap();
        assert!(kept.cut_ready);
        // The white background and the white square overlap, so they join.
        assert_eq!(kept.pieces.len(), 2);
        assert_eq!(kept.pieces[0].source_ids, ["source/0/0", "source/0/1"]);

        let wrong = prepare(
            &svg(content),
            vec![Decision::Background {
                source_id: "source/0/1".into(),
                cut: false,
            }],
        );
        assert!(wrong.unwrap_err().contains("not a background candidate"));
    }

    #[test]
    fn off_page_art_is_left_out_and_art_crossing_the_edge_is_trimmed() {
        let design = ok(r#"<rect x="10" y="10" width="20" height="20"/>
            <rect x="150" y="10" width="20" height="20"/>"#);
        assert!(design.cut_ready, "{:?}", design.diagnostics);
        assert_eq!(change(&design, "page.outside").source_ids, ["source/0/1"]);

        // Spec §5.1: the selected page's visible area is cut when they keep the page.
        let content = r#"<rect x="90" y="10" width="20" height="20"/>"#;
        let partial = prepare(&svg(content), vec![Decision::KeepPage]).unwrap();
        assert!(partial.cut_ready, "{:?}", partial.diagnostics);
        assert!(close(
            bounds(&partial.pieces[0].outer),
            [90.0, 10.0, 100.0, 30.0]
        ));
        assert_eq!(change(&partial, "page.trimmed").source_ids, ["source/0/0"]);

        // Lines are never trimmed into new cuts; one crossing the edge blocks.
        let content = r#"<path d="M50 50H150" fill="none" stroke="black" stroke-width="0.1"/>"#;
        let line = prepare(&svg(content), vec![Decision::KeepPage]).unwrap();
        assert!(!line.cut_ready && line.cut_lines.is_empty());
        assert_eq!(codes(&line), ["page.partial"]);
        let nothing = ok("");
        assert_eq!(codes(&nothing), ["preparation.empty"]);
    }

    #[test]
    fn svg_art_crossing_the_page_edge_uses_the_artwork_as_the_page() {
        // One shape crosses the edge, one is on the page and one lies entirely off it.
        let content = r#"<rect x="90" y="10" width="20" height="20"/>
            <rect x="50" y="50" width="5" height="5"/>
            <rect x="150" y="150" width="20" height="20"/>"#;
        let design = ok(content);
        assert!(design.cut_ready, "{:?}", design.diagnostics);
        let page = design.page.as_ref().unwrap();
        assert_eq!(page.id, "artwork");
        assert!((page.width_mm - 60.0).abs() < 1e-3 && (page.height_mm - 45.0).abs() < 1e-3);
        assert_eq!(design.pieces.len(), 2);
        assert!(close(
            bounds(&design.pieces[0].outer),
            [40.0, 0.0, 60.0, 20.0]
        ));
        let fitted = change(&design, "page.art_bounds");
        assert_eq!(fitted.choices, [Decision::KeepPage]);
        let kept = prepare(&svg(content), vec![Decision::KeepPage]).unwrap();
        assert!((kept.page.as_ref().unwrap().width_mm - 100.0).abs() < 1e-3);
        assert_eq!(change(&kept, "page.kept").choices, [Decision::FitArtwork]);
        assert_eq!(change(&design, "page.outside").source_ids, ["source/0/2"]);

        // A crossing line is cut whole instead of blocking.
        let line = ok(r#"<path d="M50 50H150" fill="none" stroke="black" stroke-width="0.1"/>"#);
        assert!(line.cut_ready, "{:?}", line.diagnostics);
        assert_eq!(line.cut_lines.len(), 1);

        // Art within half a millimetre of the edge, or hidden by a mask past it, keeps the page.
        let near = ok(r#"<rect x="80" y="10" width="20.4" height="20"/>"#);
        assert!((near.page.as_ref().unwrap().width_mm - 100.0).abs() < 1e-3);
        let masked = ok(&circle_clip(
            r#"<rect x="40" y="40" width="80" height="20"/>"#,
        ));
        assert!((masked.page.as_ref().unwrap().width_mm - 100.0).abs() < 1e-3);
        // Keeping the page is refused when nothing crosses it.
        assert!(
            prepare(
                &svg(r#"<rect x="10" y="10" width="20" height="20"/>"#),
                vec![Decision::KeepPage]
            )
            .unwrap_err()
            .contains("reaches past")
        );
    }

    fn circle_clip(content: &str) -> String {
        format!(
            r#"<defs><clipPath id="c"><circle cx="50" cy="50" r="20"/></clipPath></defs><g clip-path="url(#c)">{content}</g>"#
        )
    }

    #[test]
    fn clipping_masks_trim_shapes_and_strokes_to_what_is_visible() {
        let design = ok(&circle_clip(
            r#"<rect width="60" height="100" fill="red"/>
            <rect x="65" y="45" width="10" height="10"/>
            <rect x="80" y="80" width="10" height="10"/>"#,
        ));
        assert!(design.cut_ready, "{:?}", design.diagnostics);
        // The left part of the circle (x ≤ 60) and the sliver of the square inside it; the
        // clipping circle itself is not cut and the far square is hidden.
        assert_eq!(design.pieces.len(), 2);
        let red = &design.pieces[0];
        assert!(
            close(bounds(&red.outer), [30.0, 30.0, 60.0, 70.0]),
            "{:?}",
            bounds(&red.outer)
        );
        let segment_area = {
            // Circular segment of r = 20 cut by the chord 10 units right of the centre.
            let (r, d): (f64, f64) = (20.0, 10.0);
            let half = std::f64::consts::PI * r * r / 2.0;
            half + d * (r * r - d * d).sqrt() + r * r * (d / r).asin()
        };
        // The clip circle is a four-arc Bézier approximation, a few parts in 10⁴ larger.
        let ratio = piece_area(red) / segment_area;
        assert!((ratio - 1.0).abs() < 1e-3, "{}", piece_area(red));
        assert_eq!(design.pieces[1].source_ids, ["source/0/0/2"]);
        assert_eq!(
            change(&design, "clip.trimmed").source_ids,
            ["source/0/0/1", "source/0/0/2"]
        );
        assert_eq!(change(&design, "clip.hidden").source_ids, ["source/0/0/3"]);

        let stroke = ok(&circle_clip(
            r#"<path d="M0 50H100" fill="none" stroke="blue" stroke-width="2"/>"#,
        ));
        assert!(stroke.cut_ready, "{:?}", stroke.diagnostics);
        let b = bounds(&stroke.pieces[0].outer);
        assert!(
            b[0] > 29.9 && b[2] < 70.1 && (b[1] - 49.0).abs() < 1e-3,
            "{b:?}"
        );
    }

    #[test]
    fn nested_masks_intersect_and_a_clipped_page_box_is_not_a_background() {
        let nested = ok(
            r#"<defs><clipPath id="a"><rect width="60" height="100"/></clipPath>
            <clipPath id="b"><rect x="40" width="60" height="100"/></clipPath></defs>
            <g clip-path="url(#a)"><g clip-path="url(#b)"><rect width="100" height="100"/></g></g>"#,
        );
        assert!(nested.cut_ready, "{:?}", nested.diagnostics);
        assert!(close(
            bounds(&nested.pieces[0].outer),
            [40.0, 0.0, 60.0, 100.0]
        ));

        let clipped_box = ok(&circle_clip(
            r#"<rect width="100" height="100" fill="white"/>"#,
        ));
        assert!(clipped_box.pending.is_empty(), "{:?}", clipped_box.pending);
        assert_eq!(clipped_box.pieces.len(), 1);
    }

    #[test]
    fn cut_lines_inside_a_mask_are_kept_outside_dropped_and_crossing_blocks() {
        let inside = ok(&circle_clip(
            r#"<path d="M45 50H55" fill="none" stroke="black" stroke-width="0.1"/>
            <path d="M80 80H90" fill="none" stroke="black" stroke-width="0.1"/>"#,
        ));
        assert!(inside.cut_ready, "{:?}", inside.diagnostics);
        assert_eq!(inside.cut_lines.len(), 1);
        assert_eq!(inside.cut_lines[0].source_ids, ["source/0/0/1"]);
        assert_eq!(change(&inside, "clip.hidden").source_ids, ["source/0/0/2"]);

        let crossing = ok(&circle_clip(
            r#"<path d="M0 50H100" fill="none" stroke="black" stroke-width="0.1"/>"#,
        ));
        assert!(!crossing.cut_ready && crossing.cut_lines.is_empty());
        assert_eq!(codes(&crossing), ["clip.cut_line"]);
    }

    #[test]
    fn hidden_art_is_ignored_and_listed() {
        let design = ok(r#"<rect x="10" y="10" width="20" height="20"/>
            <rect x="50" y="50" width="20" height="20" display="none"/>"#);
        assert_eq!(design.pieces.len(), 1);
        assert_eq!(change(&design, "ignored.hidden").source_ids, ["source/0/1"]);
        assert!(design.cut_ready);

        // `visibility` hiding is dropped by the importer itself, so it is reported from the SVG.
        let dropped = ok(r#"<rect x="10" y="10" width="20" height="20"/>
            <rect x="50" y="50" width="20" height="20" style="visibility: hidden"/>"#);
        assert!(dropped.cut_ready);
        assert_eq!(codes(&dropped), ["svg.hidden_dropped"]);
        let lost = ok(r#"<rect x="10" y="10" width="20" height="20"/>
            <g visibility="hidden"><rect x="50" y="50" width="20" height="20" visibility="visible"/></g>"#);
        assert!(!lost.cut_ready);
        assert_eq!(codes(&lost), ["svg.visibility_dropped"]);
    }

    #[test]
    fn an_svg_without_a_stated_size_needs_confirmation_of_that_size() {
        let bytes = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 96 96"><rect x="10" y="10" width="20" height="20"/></svg>"#;
        let pending = prepare(bytes, vec![]).unwrap();
        assert!(!pending.cut_ready);
        assert_eq!(pending.pending[0].decision, "confirm_size");
        let Decision::ConfirmSize {
            width_mm,
            height_mm,
        } = pending.pending[0].choices[0]
        else {
            panic!("unexpected choice");
        };
        let confirmed = prepare(bytes, vec![pending.pending[0].choices[0].clone()]).unwrap();
        assert!(confirmed.cut_ready, "{:?}", confirmed.diagnostics);
        assert_eq!(change(&confirmed, "size.confirmed").count, 0);
        let resized = prepare(
            bytes,
            vec![Decision::ConfirmSize {
                width_mm: width_mm * 2.0,
                height_mm,
            }],
        );
        assert!(
            resized
                .unwrap_err()
                .contains("Resizing is an arrangement step")
        );
    }

    #[test]
    fn multi_page_files_need_a_page_and_other_pages_are_left_out() {
        let bytes = crate::inspect::tests::pdf(2);
        let pending = prepare_bytes(Path::new("original.pdf"), &bytes, &Recipe::new()).unwrap();
        assert!(!pending.cut_ready && pending.page.is_none());
        assert_eq!(pending.pending[0].decision, "page");
        assert_eq!(pending.pending[0].choices.len(), 2);
        let recipe = Recipe {
            decisions: vec![Decision::Page {
                page: "page/1".into(),
            }],
            ..Recipe::new()
        };
        let chosen = prepare_bytes(Path::new("original.pdf"), &bytes, &recipe).unwrap();
        assert!(chosen.cut_ready, "{:?}", chosen.diagnostics);
        assert_eq!(chosen.page.as_ref().unwrap().id, "page/1");
        assert_eq!(chosen.pieces.len(), 1);
        // The 54 pt square sits 9 pt from the page corner.
        let b = bounds(&chosen.pieces[0].outer);
        assert!(
            close(b, [9.0, 9.0, 63.0, 63.0].map(|v| v * MM_PER_PT)),
            "{b:?}"
        );
        assert_eq!(change(&chosen, "page.outside").count, 1);
    }

    #[test]
    fn unconverted_content_blocks_with_its_source_and_is_not_drawn() {
        let design = ok(
            r#"<text x="10" y="30" font-family="Inter" font-size="20">雅</text>
            <rect x="60" y="60" width="20" height="20"/>"#,
        );
        assert!(!design.cut_ready);
        assert_eq!(codes(&design), ["font.missing_glyphs"]);
        assert_eq!(design.diagnostics[0].source_ids, ["source/0/0"]);
        assert_eq!(design.pieces.len(), 1);
        assert_eq!(design.pieces[0].source_ids, ["source/0/1"]);
    }

    fn text(content: &str) -> String {
        format!(r#"<text x="10" y="50" font-family="Inter" font-size="20">{content}</text>"#)
    }

    /// Bounds of all pieces' outer contours, in mm.
    fn overall(design: &PreparedDesign) -> [f64; 4] {
        design
            .pieces
            .iter()
            .map(|p| bounds(&p.outer))
            .reduce(|a, b| {
                [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ]
            })
            .unwrap()
    }

    #[test]
    fn text_becomes_glyph_pieces_with_holes_and_a_fingerprinted_font() {
        let design = ok(&text("Hi B"));
        assert!(design.cut_ready, "{:?}", design.diagnostics);
        // H, the i's stem and dot, and B with its two counters.
        assert_eq!(design.pieces.len(), 4);
        assert!(design.pieces.iter().all(|p| p.source_ids == ["source/0/0"]));
        let b = design.pieces.last().unwrap();
        assert_eq!(b.holes.len(), 2);
        // Glyphs sit on the baseline at y = 50 mm, starting near x = 10 mm.
        let [x0, y0, x1, y1] = overall(&design);
        assert!(
            x0 > 10.0 && x0 < 13.0 && (y1 - 50.0).abs() < 0.5 && y0 > 30.0 && x1 < 60.0,
            "{:?}",
            overall(&design)
        );
        assert_eq!(change(&design, "text.outlined").source_ids, ["source/0/0"]);
        let font = &design.fonts[0];
        assert_eq!(
            (font.family.as_str(), font.status.as_str()),
            ("Inter", "exact")
        );
        assert_eq!(font.postscript_name, "Inter-Regular");
        assert_eq!(font.sha256.len(), 64);
        assert_eq!(font.source_ids, ["source/0/0"]);
    }

    #[test]
    fn outlines_match_the_engines_create_outlines_command() {
        let bytes = svg(&format!(
            "{}{}",
            text("Ag"),
            r#"<text x="10" y="80" font-family="Inter" font-size="12" text-decoration="underline">x</text>"#
        ));
        let design = prepare(&bytes, vec![]).unwrap();
        assert!(design.cut_ready, "{:?}", design.diagnostics);
        let imported = import_bytes(Path::new("original.svg"), &bytes).unwrap();
        let mut session = vectorcraft_engine::Session::new();
        session.add_document(imported.doc.clone(), None);
        let mut ids = Vec::new();
        imported.doc.walk(|n| {
            if matches!(n.kind, NodeKind::Text(_)) {
                ids.push(n.id.0);
            }
        });
        session
            .execute("select.set", &serde_json::json!({"ids": ids}))
            .unwrap();
        session
            .execute("type.createOutlines", &serde_json::json!({}))
            .unwrap();
        // Each glyph is a compound path whose counters are member paths: fill it as one.
        let mut engine_area = 0.0;
        let mut engine_bounds: Option<Rect> = None;
        let mut members = BTreeSet::new();
        session.doc().unwrap().doc.walk(|n| {
            let filled = match &n.kind {
                NodeKind::Compound { children, rule } => {
                    let mut subpaths = Vec::new();
                    for child in children {
                        members.insert(child.id);
                        if let NodeKind::Path { path, .. } = &child.kind {
                            subpaths.extend(path.subpaths.clone());
                        }
                    }
                    Some((PathData::new(subpaths), *rule))
                }
                NodeKind::Path { path, rule, .. } if !members.contains(&n.id) => {
                    Some((path.clone(), *rule))
                }
                _ => None,
            };
            if let Some((path, rule)) = filled {
                engine_area += area(&path, rule);
                let b = bez_bounds(&path).unwrap();
                engine_bounds = Some(engine_bounds.map_or(b, |e| e.union(b)));
            }
        });
        let to_pt_area = |c: &Contour| {
            let pt = |p: [f64; 2]| (p[0] / MM_PER_PT, p[1] / MM_PER_PT);
            let mut bp = BezPath::new();
            bp.move_to(pt(c.start));
            for s in &c.segments {
                bp.curve_to(pt(s[0]), pt(s[1]), pt(s[2]));
            }
            bp.close_path();
            bp.area().abs()
        };
        let ours: f64 = design
            .pieces
            .iter()
            .map(|p| to_pt_area(&p.outer) - p.holes.iter().map(to_pt_area).sum::<f64>())
            .sum();
        assert!(
            (ours - engine_area).abs() < engine_area * 1e-4,
            "{ours} vs {engine_area}"
        );
        let e = engine_bounds.unwrap();
        let ours = overall(&design).map(|v| v / MM_PER_PT);
        assert!(close(ours, [e.x0, e.y0, e.x1, e.y1]), "{ours:?} vs {e:?}");
    }

    #[test]
    fn a_missing_font_needs_use_this_substitute_and_missing_glyphs_block() {
        let content =
            r#"<text x="10" y="50" font-family="CutCraftNoSuchFont" font-size="20">Hi</text>"#;
        let pending = ok(content);
        assert!(!pending.cut_ready && pending.pieces.is_empty());
        let question = &pending.pending[0];
        assert_eq!(question.decision, "font_substitute");
        assert_eq!(question.source_ids, ["source/0/0"]);
        let accepted = prepare(&svg(content), vec![question.choices[0].clone()]).unwrap();
        assert!(accepted.cut_ready, "{:?}", accepted.diagnostics);
        assert_eq!(accepted.pieces.len(), 3);
        assert_eq!(accepted.fonts[0].status, "substitute");
        assert_eq!(change(&accepted, "font.substituted").count, 1);

        let stale = prepare(
            &svg(&text("Hi")),
            vec![Decision::FontSubstitute {
                family: "Inter".into(),
                style: "Regular".into(),
            }],
        );
        assert!(stale.unwrap_err().contains("doesn't need one"));

        let glyphs = ok(&text("Ab 雅楽"));
        assert!(!glyphs.cut_ready && glyphs.pieces.is_empty());
        assert_eq!(codes(&glyphs), ["font.missing_glyphs"]);
    }

    #[test]
    fn overlapping_letters_weld_and_text_strokes_become_shapes() {
        let welded = ok(
            r#"<text x="10" y="50" font-family="Inter" font-size="20">O</text>
            <text x="20" y="50" font-family="Inter" font-size="20">O</text>"#,
        );
        assert!(welded.cut_ready, "{:?}", welded.diagnostics);
        // Two overlapping rings make one outline; each counter stays a hole.
        assert_eq!(welded.pieces.len(), 1);
        assert_eq!(welded.pieces[0].source_ids, ["source/0/0", "source/0/1"]);
        assert_eq!(welded.pieces[0].holes.len(), 2);
        assert_eq!(change(&welded, "union.joined").count, 2);

        let stroked = ok(
            r#"<text x="10" y="50" font-family="Inter" font-size="20" stroke="blue" stroke-width="0.5">Hi</text>"#,
        );
        assert!(stroked.cut_ready, "{:?}", stroked.diagnostics);
        assert!(stroked.pending.is_empty());
        // The blue outline is part of the black letters: H, the i's stem and its dot.
        assert_eq!(stroked.pieces.len(), 3);
        assert!(stroked.pieces.iter().all(|p| p.colour.hex == "#000000"));
        let joined = change(&stroked, "stroke.part_of_shape");
        assert_eq!(joined.source_ids, ["source/0/0"]);
        // Cut around as shapes: the blue outlines of H, the i's stem and its dot.
        let separate = prepare(
            &svg(r#"<text x="10" y="50" font-family="Inter" font-size="20" stroke="blue" stroke-width="0.5">Hi</text>"#),
            vec![joined.choices[0].clone()],
        )
        .unwrap();
        let blue = separate
            .pieces
            .iter()
            .filter(|p| p.colour.hex == "#0000ff")
            .count();
        assert_eq!(blue, 3);
    }

    #[test]
    fn text_with_unsupported_paint_is_reported() {
        let gradient = ok(
            r#"<defs><linearGradient id="g"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient></defs>
            <text x="10" y="50" font-family="Inter" font-size="20" fill="url(#g)">Hi</text>"#,
        );
        assert!(!gradient.cut_ready);
        assert!(gradient.pieces.is_empty());
        let found = codes(&gradient);
        assert!(
            found.contains(&"text.paint") || found.contains(&"appearance.material_choice"),
            "{found:?}"
        );
    }

    #[test]
    fn embedded_pictures_are_cut_only_after_dont_cut() {
        let picture = r#"<image x="10" y="10" width="20" height="20" href="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAwS2OUAAAAABJRU5ErkJggg=="/>
            <rect x="60" y="60" width="20" height="20"/>"#;
        let pending = ok(picture);
        assert!(!pending.cut_ready);
        assert_eq!(pending.pending[0].decision, "exclude_picture");
        assert_eq!(pending.pending[0].source_ids, ["source/0/0"]);
        let answered = prepare(&svg(picture), vec![pending.pending[0].choices[0].clone()]).unwrap();
        assert!(answered.cut_ready, "{:?}", answered.diagnostics);
        assert_eq!(answered.pieces.len(), 1);
        assert_eq!(
            change(&answered, "picture.excluded").source_ids,
            ["source/0/0"]
        );
        let stale = prepare(
            &svg(picture),
            vec![Decision::ExcludePicture {
                source_id: "source/0/1".into(),
            }],
        );
        assert!(stale.unwrap_err().contains("not a picture"));
    }

    #[test]
    fn transparent_art_is_cut_solid_or_left_out_by_choice() {
        let content = r#"<rect x="10" y="10" width="20" height="20" fill="red" opacity="0.5"/>
            <rect x="60" y="60" width="20" height="20"/>"#;
        let pending = ok(content);
        assert!(!pending.cut_ready);
        let question = &pending.pending[0];
        assert_eq!(
            (question.decision.as_str(), question.source_ids.as_slice()),
            ("appearance", &["source/0/0".to_string()][..])
        );
        assert_eq!(pending.pieces.len(), 1, "waiting art is not drawn");
        let solid = prepare(&svg(content), vec![question.choices[0].clone()]).unwrap();
        assert!(solid.cut_ready, "{:?}", solid.diagnostics);
        assert_eq!(solid.pieces[0].colour.hex, "#ff0000");
        assert_eq!(
            change(&solid, "appearance.solid").source_ids,
            ["source/0/0"]
        );
        let left_out = prepare(&svg(content), vec![question.choices[1].clone()]).unwrap();
        assert!(left_out.cut_ready);
        assert_eq!(left_out.pieces.len(), 1);
        assert_eq!(
            change(&left_out, "object.excluded").source_ids,
            ["source/0/0"]
        );
        let stale = prepare(
            &svg(content),
            vec![Decision::ExcludeObject {
                source_id: "source/0/1".into(),
            }],
        );
        assert!(stale.unwrap_err().contains("no appearance question"));
    }

    #[test]
    fn gradients_are_cut_in_a_chosen_colour() {
        let content = r##"<defs><linearGradient id="g"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient></defs>
            <rect x="10" y="10" width="20" height="20" fill="url(#g)" stroke="url(#g)" stroke-width="2"/>"##;
        let pending = ok(content);
        let choices = &pending.pending[0].choices;
        let colours: Vec<_> = choices
            .iter()
            .filter_map(|c| match c {
                Decision::CutSolid { colour, .. } => colour.clone(),
                _ => None,
            })
            .collect();
        assert_eq!(colours, ["#ff0000", "#0000ff"]);
        assert!(matches!(
            choices.last(),
            Some(Decision::ExcludeObject { .. })
        ));
        let blue = prepare(&svg(content), vec![choices[1].clone()]).unwrap();
        assert!(blue.cut_ready, "{:?}", blue.diagnostics);
        // Fill and outlined stroke both take the chosen colour, so they join.
        assert_eq!(blue.pieces.len(), 1);
        assert_eq!(blue.pieces[0].colour.hex, "#0000ff");
        assert!(close(bounds(&blue.pieces[0].outer), [9.0, 9.0, 31.0, 31.0]));
        let no_colour = prepare(
            &svg(content),
            vec![Decision::CutSolid {
                source_id: "source/0/0".into(),
                colour: None,
            }],
        );
        assert!(no_colour.unwrap_err().contains("choose the colour"));
    }

    #[test]
    fn masked_art_can_only_be_left_out_and_inner_questions_wait() {
        let masked = r#"<defs><mask id="m"><rect width="50" height="50" fill="white"/></mask></defs>
            <rect x="10" y="10" width="60" height="60" mask="url(#m)"/>"#;
        let pending = ok(masked);
        let question = &pending.pending[0];
        assert_eq!(question.choices.len(), 1, "{:?}", question.choices);
        assert!(matches!(
            question.choices[0],
            Decision::ExcludeObject { .. }
        ));
        let forced = prepare(
            &svg(masked),
            vec![Decision::CutSolid {
                source_id: question.source_ids[0].clone(),
                colour: None,
            }],
        );
        assert!(forced.unwrap_err().contains("opacity mask"));

        // Two children, so the importer keeps the group.
        let nested = r#"<g opacity="0.5"><rect width="10" height="10"/><rect x="20" width="10" height="10" fill="red" opacity="0.5"/></g>
            <rect x="60" y="60" width="20" height="20"/>"#;
        let outer = ok(nested);
        assert_eq!(
            outer.pending.len(),
            1,
            "the inner question waits for the outer answer"
        );
        let excluded = prepare(
            &svg(nested),
            vec![Decision::ExcludeObject {
                source_id: "source/0/0".into(),
            }],
        )
        .unwrap();
        assert!(excluded.cut_ready && excluded.pending.is_empty());
        let solid_outer = prepare(
            &svg(nested),
            vec![Decision::CutSolid {
                source_id: "source/0/0".into(),
                colour: None,
            }],
        )
        .unwrap();
        assert_eq!(solid_outer.pending[0].source_ids, ["source/0/0/1"]);
    }

    #[test]
    fn art_entirely_off_the_page_offers_the_artwork_as_the_page() {
        let content = r#"<rect x="150" y="-40" width="30" height="20"/><path d="M160 -10H175" fill="none" stroke="black" stroke-width="0.1"/>"#;
        let pending = ok(content);
        assert!(!pending.cut_ready && pending.pieces.is_empty());
        assert_eq!(pending.pending[0].decision, "fit_artwork");
        // The hairline's half width reaches 0.05 mm below the art.
        let message = &pending.pending[0].message;
        assert!(message.contains("30 × 30.1 mm"), "{message}");
        let fitted = prepare(&svg(content), vec![Decision::FitArtwork]).unwrap();
        assert!(fitted.cut_ready, "{:?}", fitted.diagnostics);
        let page = fitted.page.as_ref().unwrap();
        assert_eq!(page.id, "artwork");
        // The art keeps its physical size and lands at the new page's corner.
        assert!((page.width_mm - 30.0).abs() < 1e-3 && (page.height_mm - 30.05).abs() < 1e-3);
        assert!(close(
            bounds(&fitted.pieces[0].outer),
            [0.0, 0.0, 30.0, 20.0]
        ));
        assert!(close(
            bounds(&fitted.cut_lines[0].contour),
            [10.0, 30.0, 25.0, 30.0]
        ));
        assert_eq!(change(&fitted, "page.fitted").count, 0);
        // Allowed on any single-page design; refused for an empty one.
        let small = prepare(
            &svg(r#"<rect x="40" y="40" width="10" height="10"/>"#),
            vec![Decision::FitArtwork],
        )
        .unwrap();
        assert!((small.page.unwrap().width_mm - 10.0).abs() < 1e-3);
        assert!(
            prepare(&svg(""), vec![Decision::FitArtwork])
                .unwrap_err()
                .contains("no artwork")
        );
    }

    #[test]
    fn tiny_thin_and_nearly_duplicated_shapes_are_questions_they_can_accept() {
        let content = r#"<rect x="10" y="10" width="0.5" height="0.5"/>
            <rect x="20" y="10" width="0.2" height="20"/>
            <rect x="40" y="10" width="20" height="20"/>"#;
        let design = ok(content);
        assert!(!design.cut_ready);
        let asked = |design: &PreparedDesign, question: &str| {
            design
                .pending
                .iter()
                .find(|p| p.decision == question)
                .map(|p| (p.source_ids.clone(), p.choices.clone()))
        };
        let tiny = Decision::AcceptWarning {
            warning: "piece.tiny".into(),
        };
        let thin = Decision::AcceptWarning {
            warning: "piece.thin".into(),
        };
        assert_eq!(
            asked(&design, "small_pieces"),
            Some((vec!["source/0/0".into()], vec![tiny.clone()]))
        );
        assert_eq!(
            asked(&design, "thin_pieces"),
            Some((vec!["source/0/1".into()], vec![thin.clone()]))
        );
        let accepted = prepare(&svg(content), vec![tiny, thin]).unwrap();
        assert!(accepted.cut_ready, "{:?}", accepted.pending);
        assert_eq!(
            change(&accepted, "piece.tiny.accepted").source_ids,
            ["source/0/0"]
        );
        assert_eq!(accepted.pieces.len(), 3, "accepted pieces are still cut");

        let near_content = r#"<rect x="10" y="10" width="20" height="20"/>
            <rect x="10.1" y="10.1" width="20" height="20" fill="none" stroke="black" stroke-width="0.05"/>
            <rect x="10.1" y="10.1" width="20" height="20" fill="none" stroke="red" stroke-width="0.05"/>"#;
        let near = ok(near_content);
        let (ids, choices) = asked(&near, "near_duplicates").unwrap();
        assert_eq!(
            ids,
            ["source/0/0", "source/0/1"],
            "the red copy is another colour"
        );
        assert_eq!(
            near.cut_lines.len(),
            2,
            "near-duplicates are reported, not removed"
        );
        let kept = prepare(&svg(near_content), choices).unwrap();
        assert!(kept.cut_ready, "{:?}", kept.pending);
        let apart = ok(r#"<rect x="10" y="10" width="20" height="20"/>"#);
        assert!(apart.pending.is_empty() && apart.diagnostics.is_empty());
        let unknown = prepare(
            &svg(content),
            vec![Decision::AcceptWarning {
                warning: "page.choose".into(),
            }],
        );
        assert!(unknown.unwrap_err().contains("can be accepted"));
    }

    #[test]
    fn text_that_does_not_fit_its_path_is_a_question_never_a_silent_loss() {
        let content = r##"<defs><path id="p" d="M10 50H30"/></defs>
            <text font-family="Inter" font-size="10"><textPath href="#p">A sentence far too long for its short path</textPath></text>
            <rect x="60" y="60" width="20" height="20"/>"##;
        let pending = ok(content);
        assert!(!pending.cut_ready);
        let question = pending
            .pending
            .iter()
            .find(|p| p.decision == "text_overflow")
            .unwrap_or_else(|| panic!("{:?} {:?}", pending.pending, pending.changes));
        assert!(
            question.message.contains("part of it doesn't fit"),
            "{}",
            question.message
        );
        // An SVG has no separate editable layers, so only leaving the rest out is offered.
        assert_eq!(question.choices.len(), 1);
        let accepted = prepare(&svg(content), vec![question.choices[0].clone()]).unwrap();
        assert!(accepted.cut_ready, "{:?}", accepted.diagnostics);
        assert_eq!(
            change(&accepted, "text.overflow").source_ids,
            question.source_ids
        );
        assert!(accepted.pieces.len() > 1, "the text that fits is still cut");
    }

    #[test]
    fn specks_too_small_to_cut_do_not_fail_a_join() {
        let design = ok(r#"<rect x="10" y="10" width="20" height="20"/>
            <rect x="20" y="20" width="20" height="20"/>
            <rect x="60" y="60" width="0.02" height="0.02"/>"#);
        // A kept speck is a very small piece: a question, not a failure.
        assert!(
            design
                .diagnostics
                .iter()
                .all(|d| d.severity != Severity::Blocking),
            "{:?}",
            design.diagnostics
        );
        assert!(design.diagnostics.iter().all(|d| d.code != "union.failed"));
        let kept = design
            .pieces
            .iter()
            .filter(|p| p.source_ids.contains(&"source/0/2".to_string()))
            .count();
        let dropped = design
            .changes
            .iter()
            .any(|c| c.code == "speck.dropped" && c.source_ids == ["source/0/2"]);
        // The speck is either kept as a (tiny) piece or reported as left out, never lost silently.
        assert!(
            kept == 1 || dropped,
            "{:?} {:?}",
            design.pieces.len(),
            design.changes
        );
    }

    #[test]
    fn a_text_objects_own_paints_stack_around_its_characters() {
        use vectorcraft_doc::appearance::FillLayer;
        let fill = |hex: &str| {
            AppearanceItem::Fill(FillLayer::new(Paint::solid(Color::from_hex(hex).unwrap())))
        };
        let characters = vectorcraft_doc::Appearance {
            items: vec![fill("#000000")],
            ..Default::default()
        };
        let object = vectorcraft_doc::Appearance {
            items: vec![fill("#ff0000"), fill("#0000ff")],
            contents_index: Some(1),
            ..Default::default()
        };
        let stacked = text_appearance(&object, characters.clone());
        let hexes: Vec<String> = stacked
            .items
            .iter()
            .map(|i| colour_of(i.paint()).unwrap().hex)
            .collect();
        assert_eq!(hexes, ["#ff0000", "#000000", "#0000ff"]);
        // The topmost visible fill decides the cut colour.
        assert_eq!(paint_of(&stacked, None).0.unwrap().hex, "#0000ff");
        let plain = text_appearance(&vectorcraft_doc::Appearance::default(), characters);
        assert_eq!(plain.items.len(), 1);
    }

    #[test]
    fn omitted_editing_text_offers_pdf_without_certifying_partial_artwork() {
        let bytes = svg(r#"<rect x="10" y="10" width="20" height="20"/>"#);
        let mut imported = crate::inspect::import_bytes(Path::new("original.svg"), &bytes).unwrap();
        imported.inspection.diagnostics.push(Diagnostic::blocking(
            "import.editing_text_omitted",
            "Unsupported editable text was left out",
            vec![],
        ));
        for route in [None, Some(ImportRoute::EditingData)] {
            let mut recipe = Recipe::new();
            if let Some(route) = route {
                recipe.decisions.push(Decision::ImportRoute { route });
            }
            let answers = Answers::new(&recipe).unwrap();
            let design = prepare_imported(
                &imported,
                &bytes,
                &answers,
                &recipe,
                true,
                &BTreeSet::new(),
                true,
            )
            .unwrap();
            assert!(!design.cut_ready);
            assert!(
                design
                    .diagnostics
                    .iter()
                    .any(|d| d.code == "import.editing_text_omitted")
            );
            if route.is_none() {
                assert!(design.pending.iter().any(|p| p.decision == "import_route"
                    && p.choices.contains(&Decision::ImportRoute {
                        route: ImportRoute::PdfAppearance
                    })));
            }
        }
    }

    #[test]
    fn an_import_route_answer_needs_separate_editable_layers() {
        let stale = prepare(
            &svg(r#"<rect x="10" y="10" width="20" height="20"/>"#),
            vec![Decision::ImportRoute {
                route: ImportRoute::PdfAppearance,
            }],
        );
        assert!(stale.unwrap_err().contains("no separate editable layers"));
    }

    fn picture(bytes: &[u8], decisions: Vec<Decision>) -> Result<PreparedDesign, String> {
        let recipe = Recipe {
            decisions,
            ..Recipe::new()
        };
        prepare_bytes(Path::new("original.png"), bytes, &recipe)
    }

    /// 200 × 200 px: a black ring (radii 80 and 40) with a dot in its hole, on white.
    fn ring_png() -> Vec<u8> {
        crate::picture::tests::png(200, 200, |x, y| {
            let r = ((f64::from(x) - 99.5).powi(2) + (f64::from(y) - 99.5).powi(2)).sqrt();
            if (40.0..80.0).contains(&r) || r < 15.0 {
                [0, 0, 0, 255]
            } else {
                [255; 4]
            }
        })
    }

    #[test]
    fn a_picture_gets_a_starting_size_and_traces_to_pieces_with_holes() {
        let bytes = ring_png();
        let pending = picture(&bytes, vec![]).unwrap();
        assert!(pending.cut_ready && pending.pending.is_empty());
        assert!((pending.page.unwrap().width_mm - 200.0 / 96.0 * 25.4).abs() < 1e-6);

        let design = picture(&bytes, vec![Decision::PictureSize { width_mm: 100.0 }]).unwrap();
        assert!(
            design.cut_ready,
            "{:?} {:?}",
            design.diagnostics, design.pending
        );
        let page = design.page.as_ref().unwrap();
        assert!((page.width_mm - 100.0).abs() < 1e-6 && (page.height_mm - 100.0).abs() < 1e-6);
        assert_eq!(design.pieces.len(), 2, "the ring and the dot in its hole");
        let ring = &design.pieces[0];
        assert_eq!(ring.holes.len(), 1);
        assert_eq!(ring.colour.hex, "#000000");
        // The curve's own bounds: traced curves' control points lie outside them.
        let mut outline = BezPath::new();
        outline.move_to((ring.outer.start[0], ring.outer.start[1]));
        for [a, b, e] in &ring.outer.segments {
            outline.curve_to((a[0], a[1]), (b[0], b[1]), (e[0], e[1]));
        }
        let tight = outline.bounding_box();
        for (got, want) in [
            (tight.x0, 10.0),
            (tight.y0, 10.0),
            (tight.x1, 90.0),
            (tight.y1, 90.0),
        ] {
            assert!((got - want).abs() < 1.0, "{tight:?}");
        }
        let ring_area = std::f64::consts::PI * (40.0f64.powi(2) - 20.0f64.powi(2));
        assert!(
            (piece_area(ring) / ring_area - 1.0).abs() < 0.03,
            "{}",
            piece_area(ring)
        );
        assert_eq!(design.changes[0].code, "picture.sized");
        assert_eq!(design.changes[1].code, "picture.traced");
        assert!(
            design
                .diagnostics
                .iter()
                .all(|d| d.code != "picture.photo_like")
        );
        let again = picture(&bytes, vec![Decision::PictureSize { width_mm: 100.0 }]).unwrap();
        assert_eq!(
            serde_json::to_string(&design).unwrap(),
            serde_json::to_string(&again).unwrap()
        );
    }

    #[test]
    fn small_pictures_are_enlarged_and_low_resolution_is_reported() {
        let bytes = crate::picture::tests::with_phys(&ring_png(), 10_000); // 254 ppi.
        let pending = picture(&bytes, vec![]).unwrap();
        assert!(pending.pending.is_empty());
        assert!((pending.page.unwrap().width_mm - 25.4).abs() < 1e-6);
        let large = picture(&bytes, vec![Decision::PictureSize { width_mm: 200.0 }]).unwrap();
        assert!(
            large
                .diagnostics
                .iter()
                .any(|d| d.code == "picture.low_resolution")
        );
        assert!(large.cut_ready, "a warning does not block");
    }

    #[test]
    fn automatic_picture_sizes_use_resolution_and_fit_both_orientations() {
        let bytes = crate::picture::tests::with_phys(&ring_png(), 2_000);
        let design = picture(&bytes, vec![]).unwrap();
        assert!((design.page.unwrap().width_mm - 100.0).abs() < 1e-6);
        for (width, height) in [(200, 100), (100, 200)] {
            let bytes = crate::picture::tests::png(width, height, |x, y| {
                let dx = (f64::from(x) + 0.5) / f64::from(width) - 0.5;
                let dy = (f64::from(y) + 0.5) / f64::from(height) - 0.5;
                if dx * dx + dy * dy < 0.16 {
                    [0, 0, 0, 255]
                } else {
                    [255; 4]
                }
            });
            // Very low embedded resolution would make the image metres wide.
            let bytes = crate::picture::tests::with_phys(&bytes, 10);
            let design = picture(&bytes, vec![]).unwrap();
            assert!(design.pending.is_empty());
            let page = design.page.unwrap();
            assert!(
                (page.width_mm / page.height_mm - f64::from(width) / f64::from(height)).abs()
                    < 1e-6
            );
            assert!((page.width_mm.max(page.height_mm) - 285.3).abs() < 1e-6);
        }
        let explicit =
            picture(&ring_png(), vec![Decision::PictureSize { width_mm: 400.0 }]).unwrap();
        assert_eq!(explicit.page.unwrap().width_mm, 400.0);
    }

    #[test]
    fn transparent_pictures_trace_their_opaque_shapes() {
        let bytes = crate::picture::tests::png(100, 50, |x, y| {
            if (20..80).contains(&x) && (10..40).contains(&y) {
                [0, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let design = picture(&bytes, vec![Decision::PictureSize { width_mm: 100.0 }]).unwrap();
        assert!(design.cut_ready, "{:?}", design.diagnostics);
        assert_eq!(design.pieces.len(), 1);
        let b = bounds(&design.pieces[0].outer);
        assert!(
            close(b.map(|v| v.round()), [20.0, 10.0, 80.0, 40.0]),
            "{b:?}"
        );
    }

    #[test]
    fn several_colours_give_a_piece_per_colour_and_the_mode_can_be_switched() {
        let bytes = crate::picture::tests::png(120, 60, |x, y| match (x, y) {
            (10..50, 10..50) => [255, 0, 0, 255],
            (70..110, 10..50) => [0, 0, 255, 255],
            _ => [255; 4],
        });
        let one = picture(&bytes, vec![Decision::PictureSize { width_mm: 120.0 }]).unwrap();
        let switch = one.changes[1].choices[0].clone();
        let Decision::Trace(settings) = &switch else {
            panic!("{switch:?}")
        };
        assert_eq!(settings.colours, TraceColours::Several);
        let several = picture(
            &bytes,
            vec![Decision::PictureSize { width_mm: 120.0 }, switch],
        )
        .unwrap();
        assert!(several.cut_ready, "{:?}", several.diagnostics);
        let mut colours: Vec<_> = several
            .pieces
            .iter()
            .map(|p| p.colour.hex.as_str())
            .collect();
        colours.sort_unstable();
        assert_eq!(colours, ["#0000ff", "#ff0000"]);
    }

    #[test]
    fn gif_and_bmp_pictures_are_traced_and_animated_gifs_ask_for_a_frame() {
        let gif = crate::picture::tests::animated_gif(3);
        let gif_picture = |decisions| {
            let recipe = Recipe {
                decisions,
                ..Recipe::new()
            };
            prepare_bytes(Path::new("original.gif"), &gif, &recipe)
        };
        let size = Decision::PictureSize { width_mm: 60.0 };
        let pending = gif_picture(vec![size.clone()]).unwrap();
        assert!(!pending.cut_ready);
        assert_eq!(pending.pending[0].decision, "picture_frame");
        assert_eq!(pending.pending[0].choices.len(), 3);
        // The first frame is traced while the question waits.
        assert!(close(
            bounds(&pending.pieces[0].outer).map(f64::round),
            [0.0, 5.0, 20.0, 15.0]
        ));
        let third = gif_picture(vec![size.clone(), Decision::PictureFrame { frame: 2 }]).unwrap();
        assert!(
            third.cut_ready,
            "{:?} {:?}",
            third.diagnostics, third.pending
        );
        assert!(close(
            bounds(&third.pieces[0].outer).map(f64::round),
            [40.0, 5.0, 60.0, 15.0]
        ));
        assert_eq!(change(&third, "picture.frame").count, 1);

        let bmp = crate::picture::tests::bmp(11811);
        let recipe = Recipe::new();
        let asked = prepare_bytes(Path::new("original.bmp"), &bmp, &recipe).unwrap();
        // A tiny 40 × 20 pixel picture starts at one inch on its longest edge.
        assert!(asked.pending.is_empty());
        let page = asked.page.unwrap();
        assert!((page.width_mm - 25.4).abs() < 1e-6);
        assert!((page.height_mm - 12.7).abs() < 1e-6);
        let recipe = Recipe {
            decisions: vec![Decision::PictureSize { width_mm: 40.0 }],
            ..Recipe::new()
        };
        let traced = prepare_bytes(Path::new("original.bmp"), &bmp, &recipe).unwrap();
        assert!(traced.cut_ready, "{:?}", traced.diagnostics);
        assert!(close(
            bounds(&traced.pieces[0].outer).map(f64::round),
            [10.0, 0.0, 30.0, 20.0]
        ));
        let stale = Recipe {
            decisions: vec![Decision::PictureFrame { frame: 0 }],
            ..Recipe::new()
        };
        assert!(
            prepare_bytes(Path::new("original.bmp"), &bmp, &stale)
                .unwrap_err()
                .contains("not animated")
        );
    }

    #[test]
    fn a_photo_is_traced_only_when_asked() {
        // Smooth shading, as in a photo: few flat areas.
        let bytes = crate::picture::tests::png(64, 64, |x, y| {
            [(x * 4) as u8, (y * 4) as u8, ((x + y) * 2) as u8, 255]
        });
        let first = picture(&bytes, vec![]).unwrap();
        let asked: Vec<&str> = first.pending.iter().map(|p| p.decision.as_str()).collect();
        assert_eq!(asked, ["trace_photo"]);
        assert_eq!(first.pending[0].choices, [Decision::TracePhoto]);

        let size = Decision::PictureSize { width_mm: 50.0 };
        let sized = picture(&bytes, vec![size.clone()]).unwrap();
        assert!(
            sized.pieces.is_empty() && !sized.cut_ready,
            "nothing traced yet"
        );
        assert_eq!(sized.pending[0].decision, "trace_photo");
        assert!((sized.page.as_ref().unwrap().width_mm - 50.0).abs() < 1e-9);

        let traced = picture(&bytes, vec![size, Decision::TracePhoto]).unwrap();
        assert!(traced.pending.iter().all(|p| p.decision != "trace_photo"));
        assert_eq!(traced.changes[0].code, "picture.photo_traced");

        let flat = picture(&ring_png(), vec![Decision::TracePhoto]);
        assert!(flat.unwrap_err().contains("flat artwork"));
    }

    #[test]
    fn picture_decisions_are_validated() {
        let bytes = ring_png();
        assert!(picture(&bytes, vec![Decision::PictureSize { width_mm: -5.0 }]).is_err());
        let early = picture(
            &bytes,
            vec![Decision::Background {
                source_id: "source/0/0".into(),
                cut: false,
            }],
        );
        assert!(early.unwrap_err().contains("source/0/0"));
        let vector = prepare(&svg(""), vec![Decision::PictureSize { width_mm: 10.0 }]);
        assert!(vector.unwrap_err().contains("not a picture file"));
        let report = crate::inspect::inspect_bytes(Path::new("original.png"), &bytes).unwrap();
        assert_eq!(report.format, "png");
        assert_eq!(report.objects[0].kind, "image");
    }

    #[test]
    fn recipes_are_validated_and_replay_identically() {
        let content = r#"<rect width="100" height="100" fill="white"/><rect x="10" y="10" width="20" height="20"/>"#;
        let answer = Decision::Background {
            source_id: "source/0/0".into(),
            cut: false,
        };
        let a = prepare(&svg(content), vec![answer.clone()]).unwrap();
        let b = prepare(&svg(content), vec![answer.clone()]).unwrap();
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
        assert_eq!(a.recipe.decisions, std::slice::from_ref(&answer));
        let twice = prepare(&svg(content), vec![answer.clone(), answer]);
        assert!(twice.unwrap_err().contains("more than once"));
        let future = Recipe {
            schema_version: 99,
            decisions: vec![],
        };
        assert!(prepare_bytes(Path::new("original.svg"), &svg(content), &future).is_err());
        let unknown: Result<Recipe, _> = serde_json::from_str(
            r#"{"schema_version":1,"decisions":[{"decision":"page","page":"page/0","extra":1}]}"#,
        );
        assert!(unknown.is_err(), "unknown keys are rejected");
    }
}
