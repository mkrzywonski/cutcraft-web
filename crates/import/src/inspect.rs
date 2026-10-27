//! Read-only import preflight. This is not preparation or a cut-readiness gate.

use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

pub use cutcraft_job::Diagnostic;
#[cfg(test)]
use cutcraft_job::Severity;
use vectorcraft_color::{BlendMode, Paint};
use vectorcraft_doc::{AppearanceItem, Node, NodeKind};
use vectorcraft_engine::{Session, cmd::fileio};

pub(crate) const ENGINE_REVISION: &str = "fa4e42762bf4e2b81d60c1043b2b729223d07fe7";
pub(crate) const MM_PER_PT: f64 = 25.4 / 72.0;
// An initial input guard, not the final geometry/memory budget from the corpus.
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct Inspection {
    pub schema_version: u32,
    pub engine_revision: &'static str,
    /// Hash of the original bytes only; not a preparation/cache identity.
    pub source_sha256: String,
    pub format: String,
    pub stage: &'static str,
    pub cut_ready: bool,
    pub restored_editing_data: bool,
    pub preview_only: bool,
    pub declared_svg_size_mm: Option<[f64; 2]>,
    pub pages: Vec<Page>,
    pub objects: Vec<Object>,
    pub fonts: Vec<Font>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Page {
    pub id: String,
    pub width_mm: f64,
    pub height_mm: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Object {
    /// Tree address scoped to the source hash, never a transient engine node number.
    pub source_id: String,
    pub kind: &'static str,
    pub excluded: Option<&'static str>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FontStatus {
    Exact,
    Substitute,
    Missing,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FontFace {
    pub family: String,
    pub style: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Font {
    pub family: String,
    pub style: String,
    pub status: FontStatus,
    pub resolved: Option<FontFace>,
    #[serde(alias = "missingGlyphs")]
    pub missing_glyphs: usize,
    #[serde(skip_deserializing)]
    pub source_ids: Vec<String>,
}

/// Inspect one local source without modifying it, writing output files or accessing USB.
/// Reports may contain private font names and importer messages; keep them local.
pub fn inspect_file(path: &Path) -> Result<Inspection, String> {
    inspect_bytes(path, &read_source(path)?)
}

/// The inspection report together with the imported document it describes.
pub(crate) struct Imported {
    pub inspection: Inspection,
    pub doc: vectorcraft_doc::Document,
}

pub fn read_source(path: &Path) -> Result<Vec<u8>, String> {
    if !path.is_file() {
        return Err("Choose a readable, regular design file.".into());
    }
    let mut file = File::open(path).map_err(|e| format!("Cannot open the design: {e}"))?;
    let metadata = file
        .metadata()
        .map_err(|e| format!("Cannot read the design: {e}"))?;
    if !metadata.is_file() {
        return Err("Choose a regular design file.".into());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read the design: {e}"))?;
    Ok(bytes)
}

pub fn inspect_bytes(path: &Path, bytes: &[u8]) -> Result<Inspection, String> {
    Ok(import_bytes(path, bytes)?.inspection)
}

pub(crate) fn import_bytes(path: &Path, bytes: &[u8]) -> Result<Imported, String> {
    import_route(path, bytes, true, false, None)
}

/// The fonts a PDF (or PDF-compatible AI) carries glyph programs for, as comparable keys
/// ([`font_key`] of the name without its subset tag and style). Empty for anything else.
pub(crate) fn embedded_fonts(bytes: &[u8]) -> std::collections::BTreeSet<String> {
    use hayro_syntax::object::{Dict, Name, Object, dict::keys};
    let mut out = std::collections::BTreeSet::new();
    let Ok(pdf) = hayro_syntax::Pdf::new(bytes.to_vec()) else {
        return out;
    };
    for object in pdf.objects() {
        let Object::Dict(dict) = object else { continue };
        let dict: Dict = dict;
        let carried = [keys::FONT_FILE, keys::FONT_FILE2, keys::FONT_FILE3]
            .iter()
            .any(|k| dict.contains_key(k));
        if let (true, Some(name)) = (carried, dict.get::<Name>(keys::FONT_NAME)) {
            let name = name.as_str();
            // A subset's six-letter tag, then the family before any "-Style".
            let name = match name.split_once('+') {
                Some((tag, rest)) if tag.len() == 6 => rest,
                _ => name,
            };
            out.insert(font_key(name.split('-').next().unwrap_or(name)));
        }
    }
    out
}

/// Letters and digits only, lower case: "Wood Print" and "WoodPrint" compare equal.
pub(crate) fn font_key(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Import with or without the editor's own layer data. Without it, Illustrator files open from
/// their PDF appearance (spec §5.1).
pub(crate) fn import_route(
    path: &Path,
    bytes: &[u8],
    editing_data: bool,
    text_outlines: bool,
    dxf_unit: Option<cutcraft_job::DxfUnit>,
) -> Result<Imported, String> {
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(
            "This inspection supports source files up to 64 MiB. Export a smaller design.".into(),
        );
    }
    if path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with("._"))
        || path.components().any(|c| c.as_os_str() == "__MACOSX")
    {
        return Err("This is an AppleDouble sidecar. Choose the design file beside it.".into());
    }
    if crate::picture::is_picture(path) {
        let picture = crate::picture::decode(bytes, None)?;
        return Ok(Imported {
            inspection: crate::picture::inspection(path, bytes, &picture),
            doc: vectorcraft_doc::Document::new(0.0, 0.0),
        });
    }
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(extension.as_str(), "studio" | "studio3") {
        // Spec §5.1: proprietary and undocumented; never promise an export its edition lacks.
        return Err("This is a Silhouette Studio file. Its format is private to Silhouette, so CutCraft cannot read it. Open the SVG, PDF, PNG or JPEG the design came from; some Silhouette Studio editions can also save a design as SVG or PDF.".into());
    }
    if !matches!(
        extension.as_str(),
        "svg" | "svgz" | "pdf" | "ai" | "eps" | "vectorcraft" | "drawcraft" | "dxf"
    ) {
        return Err("CutCraft opens SVG, SVGZ, PDF, PDF-compatible AI, EPS, VectorCraft, ASCII DXF, PNG, JPEG, GIF, BMP and Photoshop (PSD) files. Choose the design in one of those formats.".into());
    }
    let name = path
        .to_str()
        .ok_or("The source path is not valid Unicode; rename the file or its containing folder.")?;
    let format = fileio::detect(name, bytes).ok_or("The design format could not be identified.")?;
    // EPS is PostScript and is read by the engine's PostScript reader; any other PostScript (an
    // old Illustrator file without PDF compatibility) is refused.
    if !matches!(
        format.id,
        "svg" | "svgz" | "pdf" | "ai" | "eps" | "vectorcraft" | "dxf"
    ) || (format.id != "eps" && bytes.starts_with(b"%!"))
    {
        return Err("This content is not a supported vector source. For AI, save with PDF compatibility enabled.".into());
    }
    let mut diagnostics = Vec::new();
    let (declared_size, normalized_svg) = if matches!(format.id, "svg" | "svgz") {
        svg_preflight(bytes, &mut diagnostics)?
    } else {
        (None, None)
    };
    let dxf = if format.id == "dxf" {
        crate::dxf::options(bytes, dxf_unit, &mut diagnostics)?
    } else {
        if dxf_unit.is_some() {
            return Err("The recipe chooses DXF units, but this is not a DXF drawing.".into());
        }
        serde_json::Value::Null
    };
    // Keep text live for font inspection, preserve layers/editing data, and import all pages.
    // Page selection will be a preparation decision, not an implicit first-page import.
    let options = fileio::LoadOptions::from_params(
        "document.open",
        &json!({
            "pages": "all", "cropTo": "crop",
            "textAs": if text_outlines { "outlines" } else { "text" },
            "layers": true, "editingData": editing_data, "dxf": dxf
        }),
    )
    .map_err(|e| e.to_string())?;
    let mut loaded = fileio::load_with(name, normalized_svg.as_deref().unwrap_or(bytes), &options)
        .map_err(|e| format!("Cannot import this design: {e}"))?;
    if format.id == "dxf" {
        let expanded = crate::dxf::expand_blocks(&mut loaded.doc)?;
        if expanded > 0 {
            diagnostics.push(Diagnostic::info(
                "dxf.blocks_expanded",
                format!("Expanded {expanded} CAD block instance(s) into their cut paths."),
                vec![],
            ));
        }
    }
    for warning in &loaded.warnings {
        diagnostics.push(classify_warning(warning));
    }
    if loaded.preview_only {
        diagnostics.push(Diagnostic::blocking("import.preview_only", "Only a picture preview was recovered. Convert the original to vector shapes in an editor.", vec![]));
    }
    let pages: Vec<_> = loaded
        .doc
        .artboards
        .iter()
        .enumerate()
        .map(|(i, page)| Page {
            id: format!("page/{i}"),
            width_mm: page.rect.width() * MM_PER_PT,
            height_mm: page.rect.height() * MM_PER_PT,
        })
        .collect();
    if pages.is_empty()
        || pages.iter().any(|p| {
            !p.width_mm.is_finite()
                || !p.height_mm.is_finite()
                || p.width_mm <= 0.0
                || p.height_mm <= 0.0
        })
    {
        return Err("The importer did not produce finite, positive page dimensions.".into());
    }
    if pages.len() > 1 {
        diagnostics.push(Diagnostic::blocking(
            "page.choose",
            "Choose the page to cut before preparation.",
            vec![],
        ));
    }
    if let Some((w, h)) = declared_size
        // Relative: unit conversion leaves a few parts in 10⁸ (nanometres).
        && ((pages[0].width_mm - w).abs() > w * 1e-6 || (pages[0].height_mm - h).abs() > h * 1e-6)
    {
        diagnostics.push(Diagnostic::blocking("size.import_mismatch", format!("SVG declares {w:.6} × {h:.6} mm, but the importer produced {:.6} × {:.6} mm. Physical-size normalization is required before preparation.", pages[0].width_mm, pages[0].height_mm), vec![]));
    }
    let mut scan = Scan::default();
    for (i, layer) in loaded.doc.layers.iter().enumerate() {
        scan.node(layer, format!("source/{i}"), None);
    }
    diagnostics.extend(scan.diagnostics);
    let mut session = Session::new();
    session.add_document(loaded.doc.clone(), None);
    let fonts = inspect_fonts(&mut session, &scan.text_ids, &scan.font_sources)?;
    for font in &fonts {
        if !matches!(font.status, FontStatus::Exact) || font.missing_glyphs > 0 {
            diagnostics.push(Diagnostic::blocking("font.unresolved", format!("Review font {} {}: substitution or missing glyphs must be resolved before outlining.", font.family, font.style), font.source_ids.clone()));
        }
    }
    if !fonts.is_empty() {
        diagnostics.push(Diagnostic::blocking("font.environment_unverified", "Installed-font discovery and font fingerprints are not validated on every target platform. Inspect the reported font resolution before preparation.", vec![]));
    }
    diagnostics.push(Diagnostic::blocking("preparation.pending", "Import inspection only. Page selection, conversion decisions, geometry preparation and validation are not implemented yet.", vec![]));
    let inspection = Inspection {
        schema_version: 1,
        engine_revision: ENGINE_REVISION,
        source_sha256: format!("{:x}", Sha256::digest(bytes)),
        format: format.id.into(),
        stage: "import_inspection",
        cut_ready: false,
        restored_editing_data: loaded.restored,
        preview_only: loaded.preview_only,
        declared_svg_size_mm: declared_size.map(|(w, h)| [w, h]),
        pages,
        objects: scan.objects,
        fonts,
        diagnostics,
    };
    Ok(Imported {
        inspection,
        doc: loaded.doc,
    })
}

/// A typed boundary around the two engine commands; fixture tests cover their responses.
fn inspect_fonts(
    session: &mut Session,
    ids: &[u64],
    sources: &BTreeMap<(String, String), Vec<String>>,
) -> Result<Vec<Font>, String> {
    if ids.is_empty() {
        return Ok(vec![]);
    }
    let selected = session
        .execute("select.set", &json!({"ids": ids}))
        .map_err(|e| e.to_string())?;
    if selected.get("count").and_then(|c| c.as_u64()) != Some(ids.len() as u64) {
        return Err("The importer could not select every text object for font inspection.".into());
    }
    let value = session
        .execute("text.fonts", &json!({"selectionOnly": true}))
        .map_err(|e| e.to_string())?;
    let mut fonts: Vec<Font> = serde_json::from_value(value)
        .map_err(|e| format!("Unexpected font-query response: {e}"))?;
    for font in &mut fonts {
        font.source_ids = sources
            .get(&(font.family.clone(), font.style.clone()))
            .cloned()
            .ok_or("The font report refers to unmapped text.")?;
    }
    for key in sources.keys() {
        if !fonts
            .iter()
            .any(|f| (&f.family, &f.style) == (&key.0, &key.1))
        {
            return Err("The font query omitted a font used by included text.".into());
        }
    }
    Ok(fonts)
}

#[derive(Default)]
struct Scan {
    objects: Vec<Object>,
    diagnostics: Vec<Diagnostic>,
    text_ids: Vec<u64>,
    font_sources: BTreeMap<(String, String), Vec<String>>,
}

impl Scan {
    fn node(&mut self, node: &Node, id: String, inherited: Option<&'static str>) {
        let excluded = inherited.or({
            if !node.visible {
                Some("hidden")
            } else {
                match &node.kind {
                    NodeKind::Layer { template: true, .. } => Some("template_layer"),
                    NodeKind::Layer {
                        printable: false, ..
                    } => Some("non_printing_layer"),
                    NodeKind::Path { guide: true, .. } => Some("guide"),
                    _ => None,
                }
            }
        });
        let kind = match &node.kind {
            NodeKind::Layer { .. } => "layer",
            NodeKind::Group { .. } => "group",
            NodeKind::Path { .. } => "path",
            NodeKind::Compound { .. } => "compound",
            NodeKind::Text(_) => "text",
            NodeKind::Image(_) => "image",
            NodeKind::SymbolInstance { .. } => "symbol",
            NodeKind::PlacedDocument(_) => "placed_document",
            _ => "live_geometry",
        };
        self.objects.push(Object {
            source_id: id.clone(),
            kind,
            excluded,
        });
        if excluded.is_none() {
            let mut codes = Vec::new();
            match &node.kind {
                NodeKind::Text(text) => {
                    self.text_ids.push(node.id.0);
                    for run in &text.runs {
                        let sources = self
                            .font_sources
                            .entry((run.style.font_family.clone(), run.style.font_style.clone()))
                            .or_default();
                        if !sources.contains(&id) {
                            sources.push(id.clone());
                        }
                    }
                    codes.push((
                        "text.needs_outlines",
                        "Text must be converted to shapes after fonts are resolved.",
                    ));
                }
                NodeKind::Image(_) => codes.push((
                    "image.needs_conversion",
                    "This picture needs conversion to shapes in an editor or explicit exclusion.",
                )),
                NodeKind::SymbolInstance { .. } => codes.push((
                    "symbol.needs_expansion",
                    "Expand this instance before checking its contents and fonts.",
                )),
                NodeKind::PlacedDocument(_) => codes.push((
                    "link.unverified",
                    "Linked artwork and its dependencies need verification and fingerprints.",
                )),
                NodeKind::Layer { clip: true, .. } | NodeKind::Group { clip: true, .. } => codes
                    .push((
                        "clip.needs_application",
                        "Hidden parts must be trimmed before cutting.",
                    )),
                _ if kind == "live_geometry" => codes.push((
                    "geometry.needs_expansion",
                    "This live geometry needs a verified conversion before cutting.",
                )),
                _ => {}
            }
            if node.opacity != 1.0
                || node.blend != BlendMode::Normal
                || node.extras().mask.is_some()
            {
                codes.push(("appearance.transparency", "Transparency, blending or a mask needs an explicit conversion or exclusion decision."));
            }
            if !node.appearance.effects.is_empty() {
                codes.push((
                    "appearance.effects",
                    "Effects need a verified conversion or explicit exclusion.",
                ));
            }
            for item in &node.appearance.items {
                let (paint, visible, opacity, blend, effects) = match item {
                    AppearanceItem::Fill(f) => {
                        (&f.paint, f.visible, f.opacity, f.blend, &f.effects)
                    }
                    AppearanceItem::Stroke(s) => {
                        if s.visible && !s.paint.is_none() {
                            codes.push((
                                "stroke.needs_role",
                                "Classify each stroke as a cut line or a shape before conversion.",
                            ));
                        }
                        (&s.paint, s.visible, s.opacity, s.blend, &s.effects)
                    }
                };
                if !visible || paint.is_none() {
                    continue;
                }
                if !matches!(paint, Paint::Solid { .. }) {
                    codes.push(("appearance.material_choice", "A gradient or pattern needs an explicit material-colour or exclusion choice."));
                }
                if opacity != 1.0 || blend != BlendMode::Normal || !effects.is_empty() {
                    codes.push((
                        "appearance.paint_effects",
                        "A fill or stroke has transparency, blending or effects requiring review.",
                    ));
                }
            }
            codes.sort_unstable();
            codes.dedup();
            for (code, message) in codes {
                self.diagnostics
                    .push(Diagnostic::blocking(code, message, vec![id.clone()]));
            }
        }
        if let Some(children) = node.children() {
            for (i, child) in children.iter().enumerate() {
                self.node(child, format!("{id}/{i}"), excluded);
            }
        }
    }
}

/// Does this CSS (a style attribute or style sheet) set a property that can change an
/// element's size or geometry, or use an at-rule whose effect is not known here?
fn css_affects_geometry(css: &str) -> bool {
    const GEOMETRY: [&str; 13] = [
        "width",
        "height",
        "min-",
        "max-",
        "transform",
        "translate",
        "rotate",
        "scale",
        "zoom",
        "aspect-ratio",
        "display",
        "x",
        "y",
    ];
    let mut css = css.to_string();
    while let Some(start) = css.find("/*") {
        let end = css[start..].find("*/").map_or(css.len(), |e| start + e + 2);
        css.replace_range(start..end, "");
    }
    if css.contains('@') {
        return true;
    }
    // Declarations are the text after a rule's `{`, or the whole attribute.
    let declarations = css
        .split('}')
        .map(|rule| rule.rsplit('{').next().unwrap_or(rule));
    declarations
        .flat_map(|block| block.split(';'))
        .filter_map(|decl| decl.split_once(':'))
        .map(|(property, _)| property.trim().to_ascii_lowercase())
        .any(|property| {
            GEOMETRY
                .iter()
                .any(|g| property == *g || (g.ends_with('-') && property.starts_with(g)))
        })
}

/// Importer warnings that do not affect the cut geometry are recorded as information; a
/// difference between editable layers and the page needs a decision; anything unrecognised
/// stays blocking. Messages come from the pinned engine, using its constants where it has them.
fn classify_warning(warning: &str) -> Diagnostic {
    let info = |code| Diagnostic::info(code, warning, vec![]);
    if warning == vectorcraft_pdf::OFF_ARTBOARD_NOTE {
        // Only art on the artboards can be on the page that is cut, and only Illustrator can
        // read the rest, so they are told where to go.
        Diagnostic::info(
            "import.off_artboard_skipped",
            "Art outside this file's artboards wasn't opened: Illustrator keeps it in its own private data, which only Illustrator reads. To cut it, open the file in Illustrator, move it onto an artboard, and save; CutCraft then offers to update the design.",
            vec![],
        )
    } else if vectorcraft_pdf::is_outlines_note(warning) {
        // The file's own glyph shapes, kept faithfully (spec §5.1: embedded glyph outlines).
        info("import.text_outlines_from_file")
    } else if warning == vectorcraft_pdf::PAST_PAGE_NOTE {
        info("import.clipped_to_page")
    } else if warning.starts_with("the file's layers weren't read from its editing data") {
        info("import.layers_from_pdf")
    } else if warning.starts_with("the file's layers weren't read (") {
        // EPS: its art came in whole, as its page on one layer.
        info("import.eps_one_layer")
    } else if warning == "the file draws nothing" || warning.starts_with("only the first of the file's") {
        // An empty EPS is reported again as nothing to cut; a multi-page PostScript file is
        // read from its first page.
        info("import.eps_note")
    } else if warning.starts_with("this file's PostScript couldn't be read (") {
        // Only its preview came in: `import.preview_only` blocks, this says why.
        info("import.eps_preview")
    } else if warning.starts_with("the file was read up to an error (") {
        // Cutting part of a design would be wrong: refuse it.
        Diagnostic::blocking(
            "import.eps_partial",
            format!("Only part of this EPS could be read ({warning}). Save it as SVG or PDF in the program that made it, and open that instead."),
            vec![],
        )
    } else if warning.starts_with("the art on the layers differs from the file's own page") {
        Diagnostic::blocking("import.appearance_differs", warning, vec![])
    } else if warning.ends_with(
        "text objects of kinds VectorCraft doesn't read from the file's text document yet are left out",
    ) {
        // The editable copy lost text. Keep this blocking on that route, but let preparation
        // offer the PDF appearance instead of leaving the person with no recovery action.
        Diagnostic::blocking("import.editing_text_omitted", warning, vec![])
    } else if warning.contains("came in as their drawn look") {
        // The drawn look is what is cut.
        info("import.appearance_expanded")
    } else if warning.starts_with("fonts that aren't available show in the fallback font") {
        // Live text in a missing font gets a font question during preparation.
        info("import.fonts_unavailable")
    } else if warning.contains("on hidden layers or hidden objects couldn't be read")
        || warning.starts_with("hidden layers have art this can't read")
    {
        info("import.hidden_art_skipped")
    } else if warning.starts_with("the text is on the layers in the order the page paints it") {
        info("import.text_regrouped")
    } else {
        Diagnostic::blocking("import.warning", warning, vec![])
    }
}

fn svg_preflight(bytes: &[u8], diagnostics: &mut Vec<Diagnostic>) -> Result<SvgPreflight, String> {
    let text = vectorcraft_svg::text_of(bytes).map_err(|e| format!("Cannot decode SVG: {e}"))?;
    // Illustrator's SVG export declares a DTD whose entities name its namespaces. roxmltree
    // still limits entity expansion, and resolves no external entities.
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let xml = roxmltree::Document::parse_with_options(&text, options)
        .map_err(|e| format!("Cannot parse SVG: {e}"))?;
    let root = xml.root_element();
    if root.tag_name().name() != "svg" {
        return Err("The SVG has no svg root element.".into());
    }
    let size = root
        .attribute("width")
        .and_then(length_mm)
        .zip(root.attribute("height").and_then(length_mm));
    if size.is_none() {
        diagnostics.push(Diagnostic::blocking("size.confirm", "This SVG has no explicit, usable width and height. The reported size is inferred; set or confirm it before cutting.", vec![]));
    }
    // The importer applies style attributes and class rules for paint (Illustrator writes both,
    // plus `enable-background` on the root). Only CSS that can change size or geometry, or that
    // is not applied at all, needs review.
    let has_styles = text.contains("<?xml-stylesheet")
        || xml.descendants().any(|n| {
            n.attribute("style").is_some_and(css_affects_geometry)
                || (n.has_tag_name("style")
                    && n.descendants()
                        .filter_map(|t| t.text())
                        .any(css_affects_geometry))
        });
    if has_styles {
        diagnostics.push(Diagnostic::blocking("size.css_review", "Styles may change this SVG's size or shapes. Review the imported physical size and artwork.", vec![]));
    }
    if xml
        .descendants()
        .filter(|n| n.has_tag_name("image"))
        .any(|n| {
            n.attributes()
                .any(|a| a.name() == "href" && !a.value().trim_start().starts_with("data:"))
        })
    {
        diagnostics.push(Diagnostic::blocking("link.fingerprint_pending", "Linked assets may affect this import. Their fingerprints are not captured yet; the source hash identifies only the main file. Remote assets are not fetched.", vec![]));
    }
    // The pinned importer drops `visibility: hidden` elements with their whole subtree, so
    // they never reach the object list. Report them here; a descendant that sets itself
    // visible again may be visible artwork lost with its parent.
    fn visibility<'a>(n: &roxmltree::Node<'a, '_>) -> Option<&'a str> {
        n.attribute("visibility").map(str::trim).or_else(|| {
            n.attribute("style")?.split(';').find_map(|decl| {
                let (key, value) = decl.split_once(':')?;
                (key.trim() == "visibility").then(|| value.trim())
            })
        })
    }
    let hidden: Vec<_> = xml
        .descendants()
        .filter(|n| n.is_element() && matches!(visibility(n), Some("hidden" | "collapse")))
        .collect();
    if hidden.iter().any(|h| {
        h.descendants()
            .skip(1)
            .any(|n| visibility(&n) == Some("visible"))
    }) {
        diagnostics.push(Diagnostic::blocking("svg.visibility_dropped", "Hidden SVG content containing visible parts was not imported. Visible artwork may be missing; make it visible in an editor or move it out of the hidden group.", vec![]));
    } else if !hidden.is_empty() {
        diagnostics.push(Diagnostic::info(
            "svg.hidden_dropped",
            format!(
                "Ignored {} hidden SVG element(s); the importer does not load them.",
                hidden.len()
            ),
            vec![],
        ));
    }
    let pixel_length = |value: &str| {
        value
            .trim()
            .strip_suffix("px")
            .unwrap_or(value.trim())
            .parse::<f64>()
            .ok()
            .is_some_and(|n| n.is_finite() && n > 0.0)
    };
    let pixel_root = root.attribute("width").is_some_and(pixel_length)
        && root.attribute("height").is_some_and(pixel_length);
    let has_editing_data = xml
        .descendants()
        .any(|n| n.tag_name().namespace() == Some(vectorcraft_svg::EDITING_NS));
    let normalized = if pixel_root && !has_styles && !has_editing_data {
        let (width, height) = size.ok_or("Cannot resolve the SVG pixel dimensions.")?;
        let mut replacements = Vec::new();
        for (name, mm) in [("width", width), ("height", height)] {
            let attribute = root
                .attributes()
                .find(|a| a.namespace().is_none() && a.name() == name)
                .ok_or("Cannot locate the SVG dimension attribute.")?;
            replacements.push((attribute.range(), format!("{name}=\"{mm}mm\"")));
        }
        // text_of decodes SVGZ/UTF-16/Latin-1. The transient copy is UTF-8, so discard
        // its optional encoding declaration rather than reinterpreting its text twice.
        if text.starts_with("<?xml") && !text.starts_with("<?xml-") {
            let end = text
                .find("?>")
                .ok_or("Cannot read the SVG XML declaration.")?
                + 2;
            replacements.push((0..end, String::new()));
        }
        replacements.sort_unstable_by_key(|(range, _)| std::cmp::Reverse(range.start));
        let mut normalized = text.to_string();
        for (range, replacement) in replacements {
            normalized.replace_range(range, &replacement);
        }
        diagnostics.push(Diagnostic::info(
            "size.css_pixels_normalized",
            "Interpreted SVG pixels at 96 per inch in the import copy. The source file is unchanged.",
            vec![],
        ));
        Some(normalized.into_bytes())
    } else {
        if pixel_root && has_editing_data {
            diagnostics.push(Diagnostic::blocking("size.native_review", "This pixel-sized SVG carries native editing data. Confirm its physical size before preparation; its native geometry has not been rescaled.", vec![]));
        }
        None
    };
    Ok((size, normalized))
}

type SvgPreflight = (Option<(f64, f64)>, Option<Vec<u8>>);

fn length_mm(value: &str) -> Option<f64> {
    let value = value.trim();
    let units = [
        ("mm", 1.0),
        ("cm", 10.0),
        ("in", 25.4),
        ("pt", MM_PER_PT),
        ("pc", 25.4 / 6.0),
        ("px", 25.4 / 96.0),
    ];
    let (number, scale) = units
        .iter()
        .find_map(|(unit, scale)| value.strip_suffix(unit).map(|n| (n, *scale)))
        .unwrap_or((value, 25.4 / 96.0));
    let mm = number.trim().parse::<f64>().ok()? * scale;
    (mm.is_finite() && mm > 0.0).then_some(mm)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn svg(attributes: &str, content: &str) -> Vec<u8> {
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" {attributes}>{content}</svg>"#)
            .into_bytes()
    }

    #[test]
    fn physical_page_units_agree_with_the_engine() {
        for length in ["25.4mm", "2.54cm", "1in", "72pt", "6pc"] {
            let bytes = svg(
                &format!(r#"width="{length}" height="{length}" viewBox="0 0 200 200""#),
                r#"<path d="M0 0H200V200H0Z"/>"#,
            );
            let report = inspect_bytes(Path::new("original.SVG"), &bytes).unwrap();
            assert!((report.pages[0].width_mm - 25.4).abs() < 1e-6, "{length}");
            assert!(
                !report
                    .diagnostics
                    .iter()
                    .any(|d| d.code.starts_with("size.")),
                "{length}: {:?}",
                report.diagnostics
            );
        }
    }

    #[test]
    fn pixel_roots_are_normalized_to_css_physical_size() {
        for length in ["96px", "96"] {
            let bytes = svg(
                &format!(r#"width="{length}" height="{length}" viewBox="0 0 200 200""#),
                "",
            );
            let report = inspect_bytes(Path::new("original.svg"), &bytes).unwrap();
            assert_eq!(report.declared_svg_size_mm, Some([25.4, 25.4]));
            assert!((report.pages[0].width_mm - 25.4).abs() < 1e-6);
            let issue = report
                .diagnostics
                .iter()
                .find(|d| d.code == "size.css_pixels_normalized")
                .unwrap();
            assert_eq!(issue.severity, Severity::Info);
            assert!(
                !report
                    .diagnostics
                    .iter()
                    .any(|d| d.code == "size.import_mismatch")
            );
            assert!(!report.cut_ready);
        }
    }

    fn normalized_doc(bytes: &[u8]) -> vectorcraft_doc::Document {
        let (_, copy) = svg_preflight(bytes, &mut vec![]).unwrap();
        fileio::load("original.svg", copy.as_deref().unwrap_or(bytes))
            .unwrap()
            .doc
    }

    fn assert_path_bounds(doc: &vectorcraft_doc::Document, expected: [f64; 4]) {
        let mut found = false;
        doc.walk(|node| {
            if matches!(node.kind, NodeKind::Path { .. }) {
                let b = node.geometric_bounds().unwrap();
                for (got, want) in [b.x0, b.y0, b.x1, b.y1].into_iter().zip(expected) {
                    assert!(
                        (got - want).abs() < 1e-5,
                        "bounds {b:?}, expected {expected:?}"
                    );
                }
                found = true;
            }
        });
        assert!(found, "fixture must produce a path");
    }

    #[test]
    fn pixel_normalization_preserves_transformed_geometry_and_stroke_width() {
        let bytes = svg(
            r#"width="96px" height="96""#,
            r#"<g transform="translate(8 4) scale(2)"><rect x="4" y="8" width="12" height="16" fill="none" stroke="black" stroke-width="4"/></g>"#,
        );
        let doc = normalized_doc(&bytes);
        assert_path_bounds(&doc, [12.0, 15.0, 30.0, 39.0]);
        doc.walk(|node| {
            if matches!(node.kind, NodeKind::Path { .. }) {
                assert!((node.appearance.stroke_width() - 6.0).abs() < 1e-5);
            }
        });
    }

    #[test]
    fn viewbox_mapping_and_absolute_child_units_are_not_double_scaled() {
        let viewbox = svg(
            r#"width="96" height="96px" viewBox="0 0 192 192""#,
            r#"<rect x="32" y="16" width="64" height="32"/>"#,
        );
        assert_path_bounds(&normalized_doc(&viewbox), [12.0, 6.0, 36.0, 18.0]);
        let absolute = svg(
            r#"width="96px" height="96px""#,
            r#"<rect width="0.25in" height="6.35mm" fill="none" stroke="black" stroke-width="1pt"/>"#,
        );
        let doc = normalized_doc(&absolute);
        assert_path_bounds(&doc, [0.0, 0.0, 18.0, 18.0]);
        doc.walk(|node| {
            if matches!(node.kind, NodeKind::Path { .. }) {
                assert!((node.appearance.stroke_width() - 1.0).abs() < 1e-5);
            }
        });
    }

    #[test]
    fn rectangular_viewports_preserve_meet_and_nonuniform_viewbox_mapping() {
        for (aspect, expected) in [
            ("none", [12.0, 24.0, 36.0, 72.0]),
            ("xMidYMid meet", [12.0, 48.0, 36.0, 72.0]),
        ] {
            let bytes = svg(
                &format!(
                    r#"width="96px" height="192px" viewBox="0 0 96 96" preserveAspectRatio="{aspect}""#
                ),
                r#"<rect x="16" y="16" width="32" height="32"/>"#,
            );
            assert_path_bounds(&normalized_doc(&bytes), expected);
        }
    }

    #[test]
    fn live_text_uses_css_pixel_size_and_position_after_normalization() {
        let bytes = svg(
            r#"width="96px" height="96px""#,
            r#"<text x="16" y="32" font-family="Inter" font-size="16px">ABC</text>"#,
        );
        let doc = normalized_doc(&bytes);
        let mut found = false;
        doc.walk(|node| {
            if let NodeKind::Text(text) = &node.kind {
                assert!((text.runs[0].style.size - 12.0).abs() < 1e-5);
                let at = text.xf.translation();
                assert!(
                    (at.x - 12.0).abs() < 1e-5 && (at.y - 24.0).abs() < 1e-5,
                    "{at:?}"
                );
                found = true;
            }
        });
        assert!(found);
    }

    #[test]
    fn mixed_physical_and_pixel_root_dimensions_already_import_at_css_scale() {
        for attributes in [
            r#"width="1in" height="96px""#,
            r#"width="96" height="25.4mm""#,
        ] {
            let bytes = svg(attributes, r#"<rect width="32" height="32"/>"#);
            let (_, copy) = svg_preflight(&bytes, &mut vec![]).unwrap();
            assert!(copy.is_none());
            let doc = normalized_doc(&bytes);
            assert_path_bounds(&doc, [0.0, 0.0, 24.0, 24.0]);
            let report = inspect_bytes(Path::new("original.svg"), &bytes).unwrap();
            assert!((report.pages[0].width_mm - 25.4).abs() < 1e-6);
            assert!((report.pages[0].height_mm - 25.4).abs() < 1e-6);
        }
    }

    #[test]
    fn css_sizing_and_native_editing_data_are_not_rewritten() {
        for content in [
            r#"<style>svg { width: 2in }</style>"#,
            r#"<metadata xmlns:v="urn:vectorcraft:editing"><v:document>not-a-document</v:document></metadata>"#,
        ] {
            let bytes = svg(r#"width="96px" height="96px""#, content);
            let mut diagnostics = vec![];
            let (_, copy) = svg_preflight(&bytes, &mut diagnostics).unwrap();
            assert!(copy.is_none());
            assert!(diagnostics.iter().any(|d| d.severity == Severity::Blocking));
        }
    }

    #[test]
    fn valid_native_geometry_and_restore_status_survive_inspection() {
        // The exporter emits point-sized numeric root dimensions and matching native data.
        let doc = fileio::load(
            "original.svg",
            include_bytes!("../tests/fixtures/one-inch.svg"),
        )
        .unwrap()
        .doc;
        let bytes = fileio::encode(&doc, "svg", &json!({"preserveEditing": true})).unwrap();
        let before = fileio::load("original.svg", &bytes).unwrap();
        assert!(before.restored);
        let (_, copy) = svg_preflight(&bytes, &mut vec![]).unwrap();
        assert!(copy.is_none());
        let report = inspect_bytes(Path::new("original.svg"), &bytes).unwrap();
        assert!(report.restored_editing_data);
        assert_eq!(
            report.source_sha256,
            format!("{:x}", Sha256::digest(&bytes))
        );
        assert!(
            (report.pages[0].width_mm - before.doc.artboards[0].rect.width() * MM_PER_PT).abs()
                < 1e-9
        );
    }

    #[test]
    fn vectorcraft_documents_open_with_their_layers() {
        let doc = fileio::load(
            "original.svg",
            include_bytes!("../tests/fixtures/one-inch.svg"),
        )
        .unwrap()
        .doc;
        let bytes = fileio::encode(&doc, "vectorcraft", &json!({})).unwrap();
        for name in ["design.vectorcraft", "design.drawcraft"] {
            let report = inspect_bytes(Path::new(name), &bytes).unwrap();
            assert_eq!(report.format, "vectorcraft");
            assert!((report.pages[0].width_mm - 25.4).abs() < 1e-6, "{name}");
            assert!(
                report
                    .diagnostics
                    .iter()
                    .all(|d| d.severity != Severity::Blocking || d.code == "preparation.pending"),
                "{:?}",
                report.diagnostics
            );
        }
    }

    #[test]
    fn compressed_and_encoded_svg_keep_original_hash_and_unicode() {
        let text = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><svg xmlns=\"http://www.w3.org/2000/svg\" width='96px' height='96px'><text x='0' y='20' font-family='Inter'>é</text></svg>";
        let mut utf16 = vec![0xff, 0xfe];
        for unit in text.encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        for (name, bytes, expected_format) in [
            ("original.svg", utf16.clone(), "svg"),
            (
                "original.svgz",
                vectorcraft_svg::compress_bytes(&utf16),
                "svgz",
            ),
        ] {
            let report = inspect_bytes(Path::new(name), &bytes).unwrap();
            assert_eq!(report.format, expected_format);
            assert!((report.pages[0].width_mm - 25.4).abs() < 1e-6);
            assert_eq!(
                report.source_sha256,
                format!("{:x}", Sha256::digest(&bytes))
            );
            let doc = normalized_doc(&bytes);
            doc.walk(|node| {
                if let NodeKind::Text(text) = &node.kind {
                    assert_eq!(text.runs[0].text, "é");
                }
            });
        }
    }

    #[test]
    fn only_css_that_can_change_geometry_needs_review() {
        for harmless in [
            "enable-background:new 0 0 100 100;",
            "\n  .st0{fill:#FF0000;}\n  .st1{fill:none;stroke:#0000FF;stroke-width:2;}\n",
            ".a { font-family: 'Inter'; stroke-miterlimit: 10 } /* width: 3in */",
        ] {
            assert!(!css_affects_geometry(harmless), "{harmless}");
        }
        for sizing in [
            "width: 2in",
            "svg { height: 50% }",
            ".g{transform:rotate(45deg)}",
            "@import url(more.css);",
            "x: 10px",
        ] {
            assert!(css_affects_geometry(sizing), "{sizing}");
        }
        // Illustrator's root style no longer blocks or prevents pixel normalization.
        let bytes = svg(
            r#"width="96px" height="96px" style="enable-background:new 0 0 96 96;""#,
            r#"<style>.st0{fill:#FF0000;}</style><rect class="st0" width="10" height="10"/>"#,
        );
        let report = inspect_bytes(Path::new("original.svg"), &bytes).unwrap();
        assert!((report.pages[0].width_mm - 25.4).abs() < 1e-6);
        let codes: Vec<_> = report.diagnostics.iter().map(|d| d.code.as_str()).collect();
        assert!(!codes.contains(&"size.css_review"), "{codes:?}");
        assert!(codes.contains(&"size.css_pixels_normalized"), "{codes:?}");
    }

    #[test]
    fn silhouette_studio_files_get_their_own_guidance() {
        for name in ["design.studio3", "old.STUDIO"] {
            let error = inspect_bytes(Path::new(name), b"silhouette05;").unwrap_err();
            assert!(error.contains("Silhouette Studio file"), "{error}");
            assert!(error.contains("SVG, PDF, PNG or JPEG"), "{error}");
        }
        let other = inspect_bytes(Path::new("design.cdr"), b"").unwrap_err();
        assert!(other.contains("BMP and Photoshop (PSD)"), "{other}");
    }

    #[test]
    fn importer_warnings_are_classified_and_unknown_ones_block() {
        let cases = [
            (
                vectorcraft_pdf::OFF_ARTBOARD_NOTE,
                "import.off_artboard_skipped",
                Severity::Info,
            ),
            (
                vectorcraft_pdf::OUTLINED_TEXT_NOTE,
                "import.text_outlines_from_file",
                Severity::Info,
            ),
            (
                vectorcraft_pdf::UNMATCHED_GLYPHS_NOTE,
                "import.text_outlines_from_file",
                Severity::Info,
            ),
            (
                vectorcraft_pdf::PAST_PAGE_NOTE,
                "import.clipped_to_page",
                Severity::Info,
            ),
            (
                "the file's layers weren't read from its editing data (it has placed files): they come from its PDF part",
                "import.layers_from_pdf",
                Severity::Info,
            ),
            (
                "the art on the layers differs from the file's own page in about 2.5% of it (brush strokes)",
                "import.appearance_differs",
                Severity::Blocking,
            ),
            (
                "objects with several fills or strokes, effects or brushes came in as their drawn look (3): their appearance isn't editable yet",
                "import.appearance_expanded",
                Severity::Info,
            ),
            (
                "fonts that aren't available show in the fallback font until they are: Example",
                "import.fonts_unavailable",
                Severity::Info,
            ),
            (
                "2 text objects on hidden layers or hidden objects couldn't be read, so they are left out",
                "import.hidden_art_skipped",
                Severity::Info,
            ),
            (
                "3 text objects of kinds VectorCraft doesn't read from the file's text document yet are left out",
                "import.editing_text_omitted",
                Severity::Blocking,
            ),
            (
                "the file's layers weren't read (it has operators VectorCraft doesn't know (`--`), which VectorCraft doesn't read from it yet): it comes in as its page, in one layer",
                "import.eps_one_layer",
                Severity::Info,
            ),
            ("the file draws nothing", "import.eps_note", Severity::Info),
            (
                "the file was read up to an error (an operator it doesn't know): what follows it was left out",
                "import.eps_partial",
                Severity::Blocking,
            ),
            (
                "something new the importer says",
                "import.warning",
                Severity::Blocking,
            ),
        ];
        for (warning, code, severity) in cases {
            let d = classify_warning(warning);
            assert_eq!((d.code.as_str(), d.severity), (code, severity), "{warning}");
            // Art off the artboards gets CutCraft's own words: what to do about it.
            if code == "import.off_artboard_skipped" {
                assert!(
                    d.message.contains("open the file in Illustrator"),
                    "{}",
                    d.message
                );
            } else if code == "import.eps_partial" {
                assert!(d.message.contains("Save it as SVG or PDF"), "{}", d.message);
            } else {
                assert_eq!(d.message, warning);
            }
        }
    }

    #[test]
    fn illustrator_style_dtd_entities_parse_and_keep_physical_size() {
        // Authored in the shape of Illustrator's SVG export: namespace entities in a DTD.
        let bytes = br#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd" [
	<!ENTITY ns_svg "http://www.w3.org/2000/svg">
	<!ENTITY ns_xlink "http://www.w3.org/1999/xlink">
]>
<svg version="1.1" xmlns="&ns_svg;" xmlns:xlink="&ns_xlink;" width="72pt" height="72pt" viewBox="0 0 72 72">
<rect x="9" y="9" width="54" height="54"/>
</svg>"#;
        let report = inspect_bytes(Path::new("original.svg"), bytes).unwrap();
        assert_eq!(report.declared_svg_size_mm, Some([25.4, 25.4]));
        assert!((report.pages[0].width_mm - 25.4).abs() < 1e-6);
        assert_eq!(
            report.objects.iter().filter(|o| o.kind == "path").count(),
            1
        );
    }

    #[test]
    fn missing_glyphs_block_even_when_the_family_is_exact() {
        let bytes = svg(
            r#"width="1in" height="1in""#,
            r#"<text x="0" y="20" font-family="Inter">Ab 雅楽</text>"#,
        );
        let report = inspect_bytes(Path::new("original.svg"), &bytes).unwrap();
        assert!(matches!(report.fonts[0].status, FontStatus::Exact));
        assert_eq!(report.fonts[0].missing_glyphs, 2);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.code == "font.unresolved")
        );
    }

    #[test]
    fn ambiguous_dimensions_require_confirmation_including_percentages() {
        for attributes in [
            r#"viewBox="0 0 96 96""#,
            r#"width="100%" height="100%" viewBox="0 0 96 96""#,
        ] {
            let report = inspect_bytes(Path::new("original.svg"), &svg(attributes, "")).unwrap();
            assert!(report.diagnostics.iter().any(|d| d.code == "size.confirm"));
        }
    }

    #[test]
    fn invalid_lengths_do_not_establish_physical_size() {
        for value in [
            "NaN",
            "inf",
            "-1mm",
            "0",
            "100%",
            "auto",
            "1em",
            "calc(1in)",
        ] {
            assert_eq!(length_mm(value), None, "{value}");
        }
    }

    #[test]
    fn compressed_svg_preserves_dimensions_and_hashes_original_bytes() {
        let source = include_bytes!("../tests/fixtures/one-inch.svg");
        let compressed = vectorcraft_svg::compress_bytes(source);
        let report = inspect_bytes(Path::new("original.SVGZ"), &compressed).unwrap();
        assert_eq!(report.declared_svg_size_mm, Some([25.4, 25.4]));
        assert!((report.pages[0].width_mm - 25.4).abs() < 1e-9);
        assert_eq!(
            report.source_sha256,
            format!("{:x}", Sha256::digest(&compressed))
        );
        assert_ne!(
            report.source_sha256,
            format!("{:x}", Sha256::digest(source))
        );
    }

    #[test]
    fn unsupported_and_malformed_sources_fail_with_recovery_guidance() {
        let bytes = svg(r#"width="1in" height="1in""#, "");
        for path in [
            "original.studio3",
            "original.png",
            "._original.svg",
            "__MACOSX/original.svg",
        ] {
            assert!(inspect_bytes(Path::new(path), &bytes).is_err());
        }
        assert!(inspect_bytes(Path::new("original.svg"), b"<svg>").is_err());
        assert!(
            inspect_bytes(Path::new("original.ai"), b"%!PS-Adobe-3.0")
                .unwrap_err()
                .contains("PDF compatibility")
        );
    }

    #[test]
    fn remote_links_remain_unresolved_and_import_warnings_survive() {
        let bytes = svg(
            r#"width="1in" height="1in""#,
            r#"<image href="https://cutcraft.invalid/missing.png" width="20" height="20"/>"#,
        );
        let report = inspect_bytes(Path::new("original.svg"), &bytes).unwrap();
        for code in [
            "link.fingerprint_pending",
            "import.warning",
            "image.needs_conversion",
        ] {
            assert!(
                report.diagnostics.iter().any(|d| d.code == code),
                "{code}: {:?}",
                report.diagnostics
            );
        }
    }

    // Original tiny PDF written here, with exact offsets and no external fonts/assets.
    pub(crate) fn pdf(pages: usize) -> Vec<u8> {
        let kids = (0..pages)
            .map(|i| format!("{} 0 R", 3 + i * 2))
            .collect::<Vec<_>>()
            .join(" ");
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>"),
        ];
        for i in 0..pages {
            objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 72 72] /Resources << >> /Contents {} 0 R >>", 4 + i * 2));
            let drawing = "0 0 0 rg 9 9 54 54 re f\n";
            objects.push(format!(
                "<< /Length {} >>\nstream\n{drawing}endstream",
                drawing.len()
            ));
        }
        let mut out = "%PDF-1.4\n".to_string();
        let mut offsets = vec![];
        for (i, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{object}\nendobj\n", i + 1));
        }
        let xref = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        out.into_bytes()
    }

    #[test]
    fn pdf_and_pdf_compatible_ai_preserve_scale_and_all_pages() {
        for extension in ["pdf", "AI"] {
            let report =
                inspect_bytes(Path::new(&format!("original.{extension}")), &pdf(2)).unwrap();
            assert_eq!(report.pages.len(), 2);
            for page in report.pages {
                assert!((page.width_mm - 25.4).abs() < 1e-9);
                assert!((page.height_mm - 25.4).abs() < 1e-9);
            }
            assert!(report.diagnostics.iter().any(|d| d.code == "page.choose"));
        }
    }

    #[test]
    fn excluded_ancestors_suppress_fonts_but_locking_does_not() {
        let mut session = Session::new();
        let bytes = include_bytes!("../tests/fixtures/font-review.svg");
        let mut doc = fileio::load("original.svg", bytes).unwrap().doc;
        let first_layer = std::sync::Arc::make_mut(&mut doc.layers[0]);
        first_layer.locked = true;
        let mut scan = Scan::default();
        scan.node(first_layer, "source/0".into(), None);
        assert!(!scan.text_ids.is_empty());
        for reason in ["hidden", "template_layer", "non_printing_layer"] {
            let mut excluded = Scan::default();
            excluded.node(first_layer, "source/0".into(), Some(reason));
            assert!(excluded.text_ids.is_empty());
            assert!(excluded.objects.iter().all(|o| o.excluded == Some(reason)));
        }
        session.add_document(doc, None);
        let fonts = inspect_fonts(&mut session, &scan.text_ids, &scan.font_sources).unwrap();
        assert_eq!(fonts.len(), 1);
    }

    #[test]
    fn opacity_masks_remain_blocking_after_the_engine_node_api_change() {
        let doc = fileio::load(
            "original.svg",
            include_bytes!("../tests/fixtures/one-inch.svg"),
        )
        .unwrap()
        .doc;
        let art = (*doc.layers[0]).clone();
        let masked = art
            .clone()
            .with_mask(Some(Box::new(vectorcraft_doc::OpacityMask::new(art, true))));
        let mut scan = Scan::default();
        scan.node(&masked, "source/0".into(), None);
        assert!(
            scan.diagnostics
                .iter()
                .any(|d| d.code == "appearance.transparency"
                    && d.severity == Severity::Blocking
                    && d.source_ids == ["source/0"])
        );
    }
}
