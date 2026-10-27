//! The prepared design (spec §6): cut geometry with provenance, produced from an immutable
//! imported source and a recipe of decisions. Geometry is in page millimetres, y down, with the
//! origin at the selected page's top-left corner.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const PREPARED_SCHEMA_VERSION: u32 = 1;
pub const RECIPE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Must be resolved before the affected artwork can be cut.
    Blocking,
    /// Recorded for review; does not block.
    Info,
}

/// A stable diagnostic code with affected source objects.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub source_ids: Vec<String>,
}

impl Diagnostic {
    pub fn blocking(code: &str, message: impl Into<String>, source_ids: Vec<String>) -> Self {
        Self {
            code: code.into(),
            severity: Severity::Blocking,
            message: message.into(),
            source_ids,
        }
    }

    pub fn info(code: &str, message: impl Into<String>, source_ids: Vec<String>) -> Self {
        Self {
            severity: Severity::Info,
            ..Self::blocking(code, message, source_ids)
        }
    }
}

/// A closed or open run of cubic segments. `segments` hold `[control 1, control 2, end]`; a
/// closed contour's last segment ends at `start`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Contour {
    pub start: [f64; 2],
    pub segments: Vec<[[f64; 2]; 3]>,
    pub closed: bool,
}

/// The semantic colour that groups artwork: sRGB hex, plus the swatch name when linked.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Colour {
    pub hex: String,
    pub swatch: Option<String>,
}

/// A filled region to cut out: one outer contour and its holes. Shapes inside a hole are
/// separate pieces.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Piece {
    pub id: String,
    pub layer: String,
    pub colour: Colour,
    /// Every source object that contributed. A union may have several sources.
    pub source_ids: Vec<String>,
    pub outer: Contour,
    pub holes: Vec<Contour>,
}

/// A line cut once along its centre.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct CutLine {
    pub id: String,
    pub layer: String,
    pub colour: Colour,
    pub source_ids: Vec<String>,
    pub contour: Contour,
}

/// Physical units offered for DXF drawings without declared units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DxfUnit {
    Mm,
    Cm,
    In,
    Ft,
    M,
}

impl DxfUnit {
    /// Millimetres in one unit.
    pub fn mm(self) -> f64 {
        match self {
            Self::Mm => 1.0,
            Self::Cm => 10.0,
            Self::In => 25.4,
            Self::Ft => 304.8,
            Self::M => 1000.0,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Mm => "mm",
            Self::Cm => "cm",
            Self::In => "in",
            Self::Ft => "ft",
            Self::M => "m",
        }
    }
}

/// One user decision, replayed against the same source.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum Decision {
    /// Interpret a unitless DXF drawing at one drawing unit per selected physical unit.
    DxfUnits { unit: DxfUnit },
    /// Cut this page (`page/N`).
    Page { page: String },
    /// Use the artwork's own bounds as the page (spec §5.1.1), keeping its physical scale.
    FitArtwork,
    /// Keep the file's own page and trim the art reaching past it, instead of the default of
    /// using the artwork's own bounds when an SVG's art crosses its page edge.
    KeepPage,
    /// Accept the reported physical size. Resizing is an arrangement step, not a decision here.
    ConfirmSize { width_mm: f64, height_mm: f64 },
    /// Answer the background question for this source object.
    Background { source_id: String, cut: bool },
    /// For an Illustrator file whose editable layers differ from its page: open it from the
    /// editable layers, or from the PDF appearance (the look, with its layer limitations).
    ImportRoute { route: ImportRoute },
    /// The cut width of a picture file; its height follows its proportions (spec §5.1.2).
    PictureSize { width_mm: f64 },
    /// Which frame of an animated picture is cut, counting from 0 (spec §5.1.2).
    PictureFrame { frame: usize },
    /// How a picture file is traced into shapes (spec §5.1.2).
    Trace(TraceSettings),
    /// *Trace it anyway*: trace a picture that looks like a photo rather than flat artwork.
    TracePhoto,
    /// *Cut them anyway* / *Keep both*: cut despite a warning about what is hard to cut or weed
    /// (`piece.tiny`, `piece.thin`) or outlines that nearly repeat (`duplicate.near`).
    AcceptWarning { warning: String },
    /// *Cut as solid*: ignore this object's transparency, blending and effects and cut its
    /// shapes opaque; paints that are not one colour (gradients, patterns) use `colour`.
    CutSolid {
        source_id: String,
        colour: Option<String>,
    },
    /// *Don't cut* this object and its contents.
    ExcludeObject { source_id: String },
    /// *Don't cut* a picture embedded in a vector design (spec §5.1.2).
    ExcludePicture { source_id: String },
    /// *Use the letter shapes saved in the file*: cut this font's text from the glyph outlines the
    /// file embeds, without the font installed (spec §5.1: embedded glyph outlines).
    FontFromFile { family: String, style: String },
    /// *Leave out the text that doesn't fit*: cut only the part of this text that fits its frame
    /// or path when laid out with the installed font.
    AcceptTextOverflow { source_id: String },
    /// *Use this substitute*: accept the stand-in the report names for a font that is not
    /// installed (or lacks this style).
    FontSubstitute { family: String, style: String },
    /// Override how every stroke of this colour (`#rrggbb`) is cut, unless an object's own
    /// `stroke_role` says otherwise.
    StrokeColourRole { colour: String, role: StrokeRole },
    /// *Outlines on filled shapes*: how every stroke on a filled object is cut, unless its
    /// colour's or object's own choice says otherwise. `part_of_shape` (the default) or `shape`
    /// (cut separately, in the stroke's own colour).
    OutlinesOnFills { role: StrokeRole },
    /// Override how one stroke (the Nth stroke of the object, bottom first) is cut.
    StrokeRole {
        source_id: String,
        stroke: usize,
        role: StrokeRole,
    },
}

/// Image Trace settings as the person sees them.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceSettings {
    pub colours: TraceColours,
    /// One colour: pixels darker than this (0–255) count as black.
    pub threshold: u8,
    /// 0–100: higher follows the pixels more closely.
    pub detail: u8,
    /// 0–100: higher rounds off more corners.
    pub smoothness: u8,
    /// Leave white areas out instead of cutting them.
    pub ignore_white: bool,
}

impl Default for TraceSettings {
    /// One colour (decal), as Image Trace's Black and White Logo preset.
    fn default() -> Self {
        Self {
            colours: TraceColours::One,
            threshold: 128,
            detail: 95,
            smoothness: 20,
            ignore_white: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TraceColours {
    /// One colour (decal).
    One,
    /// Several colours: one group per flat colour.
    Several,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportRoute {
    EditingData,
    PdfAppearance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StrokeRole {
    /// *Cut along line*: one cut along the centre, whatever its width.
    CutLine,
    /// *Cut around shape*: outline the stroke and cut its edges, in its own colour.
    Shape,
    /// *Part of the shape*: the outlined stroke joins its object's fill, so the piece is cut
    /// along the stroke's outer edge in the fill's colour. The default on filled objects.
    PartOfShape,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema_version: u32,
    pub decisions: Vec<Decision>,
}

impl Recipe {
    pub fn new() -> Self {
        Self {
            schema_version: RECIPE_SCHEMA_VERSION,
            decisions: Vec::new(),
        }
    }
}

/// An empty recipe at the current schema version.
impl Default for Recipe {
    fn default() -> Self {
        Self::new()
    }
}

/// One line of the plain-language change list.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Change {
    pub code: String,
    pub message: String,
    pub count: usize,
    pub source_ids: Vec<String>,
    /// Decisions that would reverse or change this default, offered beside it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<Decision>,
}

/// A question that must be answered before the design can be cut.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct PendingDecision {
    /// The decision kind a recipe answers with.
    pub decision: String,
    pub message: String,
    /// Allowed answers, as recipe decisions.
    pub choices: Vec<Decision>,
    pub source_ids: Vec<String>,
}

/// A font whose outlines became cut geometry, identified by the font file actually used.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct FontUse {
    /// The family and style the text asks for.
    pub family: String,
    pub style: String,
    /// `exact`, or `substitute` when the person accepted a stand-in.
    pub status: String,
    pub resolved_family: String,
    pub resolved_style: String,
    pub postscript_name: String,
    /// SHA-256 of the font file (or embedded font data) the outlines came from.
    pub sha256: String,
    pub source_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Page {
    pub id: String,
    pub width_mm: f64,
    pub height_mm: f64,
    /// The page's top-left corner in the source document, in points: prepared geometry is
    /// measured from here, so views of the original can line up with it exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_pt: Option<[f64; 2]>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct PreparedDesign {
    pub schema_version: u32,
    pub stage: String,
    /// True only when nothing blocks and nothing is pending. Planning and cutting are later stages.
    pub cut_ready: bool,
    pub engine_revision: String,
    /// Hash of the original main-file bytes only; linked assets and fonts are not yet included.
    pub source_sha256: String,
    pub recipe: Recipe,
    /// Fonts outlined into the geometry, so a change of installed font is detectable.
    pub fonts: Vec<FontUse>,
    pub page: Option<Page>,
    pub pieces: Vec<Piece>,
    pub cut_lines: Vec<CutLine>,
    pub changes: Vec<Change>,
    pub pending: Vec<PendingDecision>,
    pub diagnostics: Vec<Diagnostic>,
    /// Filled objects with an outline (stroke), whichever way it is cut: the *Outlines on
    /// filled shapes* setting matters only when there are some.
    #[serde(default)]
    pub stroked_fills: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_decision_round_trips_through_its_json_form() {
        let decisions = vec![
            Decision::Page {
                page: "page/0".into(),
            },
            Decision::PictureSize { width_mm: 250.0 },
            Decision::Trace(TraceSettings::default()),
            Decision::ImportRoute {
                route: ImportRoute::PdfAppearance,
            },
            Decision::StrokeColourRole {
                colour: "#000000".into(),
                role: StrokeRole::CutLine,
            },
            Decision::OutlinesOnFills {
                role: StrokeRole::Shape,
            },
        ];
        for decision in decisions {
            let json = serde_json::to_value(&decision).unwrap();
            let back: Decision = serde_json::from_value(json.clone()).unwrap();
            assert_eq!(back, decision, "{json}");
        }
        let trace = serde_json::json!({"decision": "trace", "colours": "several", "threshold": 100,
            "detail": 80, "smoothness": 10, "ignore_white": false});
        assert!(serde_json::from_value::<Decision>(trace).is_ok());
        let extra = serde_json::json!({"decision": "trace", "colours": "one", "threshold": 1,
            "detail": 1, "smoothness": 1, "ignore_white": true, "bogus": 1});
        assert!(serde_json::from_value::<Decision>(extra).is_err());
    }
}
