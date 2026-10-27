//! Projects and sheets (spec §5.3, §5.6, §6): what is cut, on which sheet, with which material
//! and placement. Coordinates on the mat are millimetres from the start corner, x across the mat
//! and y along the feed.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::prepared::{Colour, PreparedDesign};

pub const PROJECT_SCHEMA_VERSION: u32 = 1;
/// The id of a project's one sheet.
pub const SHEET_ID: &str = "sheet/1";

/// How rows are formed (spec §5.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupBy {
    #[default]
    Colour,
    Layer,
}

/// A row of the "choose what to cut" list: pieces or cut lines of one colour (or one layer).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RowKey {
    Pieces { colour: Colour },
    CutLines { colour: Colour },
    Layer { layer: String },
}

impl RowKey {
    /// Every row of a prepared design, in first-appearance order.
    pub fn rows(design: &PreparedDesign, group_by: GroupBy) -> Vec<RowKey> {
        let mut rows: Vec<RowKey> = Vec::new();
        let mut add = |row: RowKey| {
            if !rows.contains(&row) {
                rows.push(row);
            }
        };
        for piece in &design.pieces {
            add(match group_by {
                GroupBy::Colour => RowKey::Pieces {
                    colour: piece.colour.clone(),
                },
                GroupBy::Layer => RowKey::Layer {
                    layer: piece.layer.clone(),
                },
            });
        }
        for line in &design.cut_lines {
            add(match group_by {
                GroupBy::Colour => RowKey::CutLines {
                    colour: line.colour.clone(),
                },
                GroupBy::Layer => RowKey::Layer {
                    layer: line.layer.clone(),
                },
            });
        }
        rows
    }

    pub fn has_piece(&self, colour: &Colour, layer: &str) -> bool {
        match self {
            RowKey::Pieces { colour: c } => c == colour,
            RowKey::Layer { layer: l } => l == layer,
            RowKey::CutLines { .. } => false,
        }
    }

    pub fn has_line(&self, colour: &Colour, layer: &str) -> bool {
        match self {
            RowKey::CutLines { colour: c } => c == colour,
            RowKey::Layer { layer: l } => l == layer,
            RowKey::Pieces { .. } => false,
        }
    }

    /// A short label for lists and sheet names.
    pub fn label(&self) -> String {
        match self {
            RowKey::Pieces { colour } => {
                colour.swatch.clone().unwrap_or_else(|| colour.hex.clone())
            }
            RowKey::CutLines { colour } => format!(
                "{} cut lines",
                colour.swatch.clone().unwrap_or_else(|| colour.hex.clone())
            ),
            RowKey::Layer { layer } => layer.clone(),
        }
    }
}

/// Legacy material status, retained only to read older files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// A starting point to confirm with a test cut.
    Provisional,
    /// Confirmed by a test cut on this machine.
    Tested,
}

/// How the blade's trailing tip is accounted for (spec §7.2): one method per material, never
/// both. Off until test cuts on the actual tool choose one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OffsetMethod {
    /// No compensation: the blade follows the drawn path.
    #[default]
    Off,
    /// CutCraft adjusts the path for the blade offset (corners swung round) and turns the
    /// cutter's own compensation off.
    Software,
    /// The cutter's own blade-offset setting (`FC`), with its clearance in the bounds.
    Machine,
}

/// A material profile, copied into each project (spec §5.6, §5.7).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Material {
    pub name: String,
    /// Ratchet blade setting: a manual instruction, never applied automatically.
    pub blade: u8,
    /// Device force, within the machine's range.
    pub force: u8,
    /// Device speed, within the machine's range.
    pub speed: u8,
    /// How many times each contour is cut.
    pub passes: u8,
    /// Iron-on is cut mirrored ("shiny side down").
    pub mirror: bool,
    /// Legacy field: accepted when reading old files, ignored and omitted on save.
    #[serde(
        default = "legacy_material_status",
        skip_serializing,
        deserialize_with = "ignore_material_status"
    )]
    #[schemars(skip)]
    pub status: Status,
    /// Blade-offset compensation method (spec §7.2).
    #[serde(default)]
    pub offset_method: OffsetMethod,
    /// The blade tip's distance behind the holder's axis, in mm (0.25 is a calibration
    /// candidate, not an established value).
    #[serde(default)]
    pub offset_mm: f64,
    /// How far closed contours are cut past their start, in mm (0.5 is a test candidate).
    #[serde(default)]
    pub overcut_mm: f64,
    /// Loading notes shown with the cutting steps.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Suitable for cutting from a roll.
    #[serde(default)]
    pub roll: bool,
}

fn legacy_material_status() -> Status {
    Status::Provisional
}

fn ignore_material_status<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Status, D::Error> {
    let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
    Ok(Status::Provisional)
}

/// Limits for the offset and overcut values.
pub const OFFSET_RANGE_MM: (f64, f64) = (0.0, 1.0);
pub const OVERCUT_RANGE_MM: (f64, f64) = (0.0, 2.0);

impl Material {
    /// Compare the settings that affect blade motion. Names and loading notes do not
    /// change the cut; mirror and roll suitability belong to setup rather than blade tuning.
    pub fn same_cut_settings(&self, other: &Self) -> bool {
        self.blade == other.blade
            && self.force == other.force
            && self.speed == other.speed
            && self.passes == other.passes
            && self.offset_method == other.offset_method
            && self.offset_mm == other.offset_mm
            && self.overcut_mm == other.overcut_mm
    }

    /// The v1 materials (spec §5.6). Values are published starting points, not tested settings:
    /// common charts for the two Oracal vinyls, and Siser's own Silhouette settings for its
    /// products (HTV chart 07-09-2025, Cameo 4 & 5; EasyPSV chart 5-2023, Cameo 4). The original
    /// Cameo uses the same blade and force scales, but a test cut decides. Where Siser gives a
    /// range, the low end is used so the first test errs shallow.
    pub fn defaults() -> Vec<Material> {
        let m = |name: &str, blade, force, speed, mirror, notes: &str| Material {
            name: name.into(),
            blade,
            force,
            speed,
            passes: 1,
            mirror,
            status: Status::Provisional,
            offset_method: OffsetMethod::Off,
            offset_mm: 0.0,
            overcut_mm: 0.0,
            notes: notes.into(),
            roll: true,
        };
        let iron_on = |name: &str, blade, force, speed, covers: &str| {
            m(
                name,
                blade,
                force,
                speed,
                true,
                &format!(
                    "Shiny carrier side down; it is cut mirrored. Siser's starting point (HTV chart, July 2025){covers}."
                ),
            )
        };
        let psv = |name: &str, blade, force, speed, covers: &str| {
            m(
                name,
                blade,
                force,
                speed,
                false,
                &format!(
                    "Backing paper down. Siser's starting point (EasyPSV chart, May 2023){covers}."
                ),
            )
        };
        vec![
            m("Permanent vinyl (e.g. Oracal 651)", 1, 10, 5, false, ""),
            m("Removable vinyl (e.g. Oracal 631)", 1, 9, 5, false, ""),
            iron_on(
                "Siser EasyWeed iron-on (and similar)",
                3,
                6,
                8,
                " for EasyWeed, EcoStretch, Electric, Extra, Stretch, Sub Block, EasyReflective, Easy Puff, Easy Puff Metallic, Easy Glow, Metal, Sparkle, Vernice, PureHT, EasyPatterns Plus and EasyColor DTV",
            ),
            iron_on(
                "Siser Glitter iron-on (and DiamondDust, Twinkle)",
                4,
                11,
                8,
                " for Glitter, DiamondDust and Twinkle",
            ),
            iron_on(
                "Siser Holographic / Aurora iron-on",
                3,
                14,
                8,
                " for Holographic and Aurora",
            ),
            iron_on("Siser StripFlock Pro iron-on", 3, 6, 5, ""),
            iron_on("Siser EasySubli iron-on", 3, 15, 8, ""),
            iron_on(
                "Siser Brick 600 iron-on",
                7,
                20,
                8,
                "; a thick material, so test cut first",
            ),
            iron_on(
                "Siser Easy Puff Glitter iron-on",
                4,
                25,
                8,
                "; the chart lists blade 5 and force 30 for one Cameo model and blade 4, force 25 for the other",
            ),
            psv("Siser EasyPSV Brushed", 3, 6, 8, ""),
            psv("Siser EasyPSV Etch", 2, 7, 8, ""),
            psv(
                "Siser EasyPSV Glitter, Holographic, Patterns, Starling",
                3,
                8,
                7,
                " for Glitter, Holographic, Patterns and Starling",
            ),
            psv("Siser EasyPSV Glow", 3, 12, 5, ""),
            psv("Siser EasyPSV Blackboard", 3, 10, 5, ""),
        ]
    }
}

/// What is loaded into the cutter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mat {
    #[default]
    Mat12x12,
    Mat12x24,
    /// Material straight from a roll, loaded without a mat. Planned and simulated; cutting it
    /// on the real Cameo waits for its own hardware tests (spec §7.2).
    Roll,
}

/// The units printed along one edge of a mat's grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RulerUnit {
    Inches,
    Centimetres,
    None,
}

/// A real cutting mat, for showing where their material sits on it (spec §4.1). Distances are mm
/// from the mat's top-left corner (the edge that goes in first, on the left).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MatProfile {
    pub name: String,
    /// The whole mat, `[width, length]`.
    pub outer_mm: [f64; 2],
    /// The sticky, graduated area, `[left, top, right, bottom]`.
    pub sticky_mm: [f64; 4],
    /// Where the printed rulers count from (usually the sticky area's top-left corner).
    pub grid_origin_mm: [f64; 2],
    /// Ruler units along the sticky area's top, right, bottom and left edges.
    pub rulers: [RulerUnit; 4],
    /// Where the cutter's start corner falls on this mat (a hardware measurement; presets assume
    /// the grid's corner).
    pub cut_origin_mm: [f64; 2],
    /// How the Cameo loads it: the 12 × 12 or 12 × 24 in length.
    pub class: Mat,
    /// Preset measurements are approximate: measure your mat and edit them.
    #[serde(default)]
    pub approximate: bool,
}

impl MatProfile {
    /// Starting presets. Their measurements are approximate until checked against a real mat.
    pub fn presets() -> Vec<MatProfile> {
        let inch = 25.4;
        let mat = |name: &str, length_in: f64, class, rulers| MatProfile {
            name: name.into(),
            outer_mm: [13.0 * inch, (length_in + 1.5) * inch],
            sticky_mm: [
                0.5 * inch,
                0.75 * inch,
                12.5 * inch,
                (length_in + 0.75) * inch,
            ],
            grid_origin_mm: [0.5 * inch, 0.75 * inch],
            rulers,
            cut_origin_mm: [0.5 * inch, 0.75 * inch],
            class,
            approximate: true,
        };
        use RulerUnit::{Centimetres as Cm, Inches as In};
        vec![
            mat("Silhouette 12 × 12 in mat", 12.0, Mat::Mat12x12, [In; 4]),
            mat("Silhouette 12 × 24 in mat", 24.0, Mat::Mat12x24, [In; 4]),
            mat(
                "12 × 12 in mat, inch and cm rulers",
                12.0,
                Mat::Mat12x12,
                [In, Cm, Cm, In],
            ),
        ]
    }

    /// The first preset of a load length, for sheets that name none.
    pub fn for_class(class: Mat) -> MatProfile {
        Self::presets()
            .into_iter()
            .find(|p| p.class == class)
            .unwrap_or_else(|| Self::presets().remove(0))
    }

    /// The sticky area in cutter coordinates (mm from the cutter's start corner).
    pub fn sticky_in_cut(&self) -> [f64; 4] {
        let [ox, oy] = self.cut_origin_mm;
        let [x0, y0, x1, y1] = self.sticky_mm;
        [x0 - ox, y0 - oy, x1 - ox, y1 - oy]
    }

    /// Sensible values: positive sizes, the sticky area inside the mat, a mat length class.
    pub fn check(&self) -> Result<(), String> {
        let [w, h] = self.outer_mm;
        let [x0, y0, x1, y1] = self.sticky_mm;
        let finite = [
            w,
            h,
            x0,
            y0,
            x1,
            y1,
            self.cut_origin_mm[0],
            self.cut_origin_mm[1],
        ]
        .iter()
        .all(|v| v.is_finite());
        if self.name.trim().is_empty() {
            return Err("Give the mat a name.".into());
        }
        if !finite
            || w <= 0.0
            || h <= 0.0
            || x0 < 0.0
            || y0 < 0.0
            || x1 <= x0
            || y1 <= y0
            || x1 > w + 1e-6
            || y1 > h + 1e-6
        {
            return Err("The sticky area must lie inside the mat, with positive sizes.".into());
        }
        if self.class == Mat::Roll {
            return Err("A mat loads as 12 × 12 or 12 × 24 in, not as a roll.".into());
        }
        Ok(())
    }
}

/// Planning limits of a machine as domain data (spec §6). The timing values come from the
/// timing plot on the original Cameo (measured 2026-10-20); the rest are from
/// public descriptions and the probe (spec §7.3, §7.4).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct MachineProfile {
    pub name: String,
    /// Usable cutting width across the mat.
    pub cutting_width_mm: f64,
    pub mat_12x12_mm: f64,
    pub mat_12x24_mm: f64,
    /// Drawing (tool down) speed per speed step, for estimates.
    pub speed_step_mm_s: f64,
    /// Lifting the tool and lowering it again for the next cut, for estimates.
    pub tool_change_s: f64,
    /// Tool-up travel, whatever the speed setting.
    #[serde(default = "default_travel_mm_s")]
    pub travel_mm_s: f64,
    /// Each point the tool passes through.
    #[serde(default = "default_point_s")]
    pub point_s: f64,
    /// Slowing for a change of direction while cutting, per radian turned. Measured with `FC0`;
    /// the cutter's own blade compensation adds much more (about 0.1 s per radian with a pen).
    #[serde(default = "default_turn_s_per_rad")]
    pub turn_s_per_rad: f64,
    /// The longest roll material the cutter feeds.
    #[serde(default = "default_roll_mm")]
    pub roll_max_mm: f64,
    /// The smallest step the cutter makes; contours that vanish at this resolution can't be cut.
    #[serde(default = "default_step_mm")]
    pub step_mm: f64,
    /// `hypothesis`, `observed` or `measured`.
    pub source: String,
}

impl MachineProfile {
    pub fn cameo_1() -> Self {
        Self {
            name: "Silhouette Cameo (original)".into(),
            cutting_width_mm: 304.0,
            mat_12x12_mm: 304.8,
            mat_12x24_mm: 609.6,
            speed_step_mm_s: 9.3,
            tool_change_s: 0.15,
            travel_mm_s: default_travel_mm_s(),
            point_s: default_point_s(),
            turn_s_per_rad: default_turn_s_per_rad(),
            roll_max_mm: default_roll_mm(),
            step_mm: default_step_mm(),
            source: "measured".into(),
        }
    }

    pub fn mat_length_mm(&self, mat: Mat) -> f64 {
        match mat {
            Mat::Mat12x12 => self.mat_12x12_mm,
            Mat::Mat12x24 => self.mat_12x24_mm,
            Mat::Roll => self.roll_max_mm,
        }
    }
}

/// About 3 m of roll (spec §7.3's model table, hypothesis).
fn default_roll_mm() -> f64 {
    3000.0
}

/// 20 device units per mm (spec §7.3, hypothesis).
fn default_step_mm() -> f64 {
    0.05
}

fn default_travel_mm_s() -> f64 {
    100.0
}

fn default_point_s() -> f64 {
    0.008
}

fn default_turn_s_per_rad() -> f64 {
    0.005
}

/// Where the design sits on the mat. Mirroring and rotation turn the design about its own
/// centre; the offset then places its bounding box's top-left corner.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub offset_mm: [f64; 2],
    /// Degrees, clockwise on the mat.
    pub rotation_deg: f64,
}

/// One loaded sheet: rows cut together with one material and setup.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Sheet {
    pub id: String,
    pub name: String,
    pub rows: Vec<RowKey>,
    pub material: Material,
    pub mirror: bool,
    pub mat: Mat,
    pub placement: Placement,
    /// A box around the artwork, cut after it, so the waste is easy to weed.
    pub weeding_box: bool,
    pub weeding_margin_mm: f64,
    /// Their actual piece of material, `[width, height]`, laid at the start corner of the cutting
    /// area ("How big is your piece?", spec §5.4). Every cut must then fit on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub piece_mm: Option<[f64; 2]>,
    /// Lines through the waste vinyl, clear of the artwork, so it weeds in strips (spec §5.4).
    #[serde(default)]
    pub weed_lines: bool,
    /// *Invert*: the background is kept and the shapes are the waste (a stencil or knockout),
    /// so weed lines run through the shapes instead of around them.
    #[serde(default)]
    pub weed_inverted: bool,
    /// Marks at the same place on every sheet that has them, for lining up layers (spec §5.4).
    #[serde(default)]
    pub layering_marks: bool,
    /// Where they put the test cut's top-left corner on the mat; none places it automatically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_spot_mm: Option<[f64; 2]>,
    /// Where their piece's top-left corner sits, from the cutter's start corner (default: there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub piece_at_mm: Option<[f64; 2]>,
    /// The real mat, for showing the material on it; none uses the preset for `mat`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mat_profile: Option<MatProfile>,
}

impl Sheet {
    /// The mat shown in the Cut preview.
    pub fn mat_profile(&self) -> MatProfile {
        self.mat_profile.clone().unwrap_or_else(|| {
            MatProfile::for_class(if self.mat == Mat::Roll {
                Mat::Mat12x12
            } else {
                self.mat
            })
        })
    }
}

/// Default weeding margin (¼ in, spec §5.4).
pub const WEEDING_MARGIN_MM: f64 = 6.35;
/// Clearance between the weeding box and the edge of the cutting area.
pub const PLACEMENT_CLEARANCE_MM: f64 = 3.0;

/// A sheet cut to completion, and the plan it was cut from.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompletedSheet {
    pub sheet: String,
    pub plan_id: String,
}

/// What has been cut from this project (spec §5.5): kept at sheet boundaries, so "continue
/// from Sheet N" survives closing the app or a crash.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CutRecord {
    pub completed: Vec<CompletedSheet>,
    /// The sheet whose cut was interrupted: what it cut is uncertain.
    pub interrupted: Option<String>,
}

impl CutRecord {
    pub fn is_empty(&self) -> bool {
        self.completed.is_empty() && self.interrupted.is_none()
    }

    pub fn is_completed(&self, sheet: &str) -> bool {
        self.completed.iter().any(|c| c.sheet == sheet)
    }

    pub fn complete(&mut self, sheet: &str, plan_id: &str) {
        self.completed.retain(|c| c.sheet != sheet);
        self.completed.push(CompletedSheet {
            sheet: sheet.into(),
            plan_id: plan_id.into(),
        });
        if self.interrupted.as_deref() == Some(sheet) {
            self.interrupted = None;
        }
    }

    pub fn interrupt(&mut self, sheet: &str) {
        self.completed.retain(|c| c.sheet != sheet);
        self.interrupted = Some(sheet.into());
    }
}

/// A project: a prepared design snapshot and its sheets (spec §5.7).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub schema_version: u32,
    pub prepared: PreparedDesign,
    pub group_by: GroupBy,
    /// Rows not cut (unticked).
    pub excluded_rows: Vec<RowKey>,
    pub sheets: Vec<Sheet>,
    pub machine: MachineProfile,
    /// The design's size as a factor of its imported size, the same on every sheet so the
    /// colours still fit together (spec §5.1.1).
    #[serde(default = "unit_scale")]
    pub scale: f64,
    /// Increases with every change; a plan records the revision it was made from.
    pub revision: u64,
    /// Sheets already cut. Not an edit: it changes neither the revision nor any plan.
    #[serde(default, skip_serializing_if = "CutRecord::is_empty")]
    pub cut_record: CutRecord,
}

fn unit_scale() -> f64 {
    1.0
}

/// Sizes allowed for [`Project::set_scale`].
pub const SCALE_RANGE: (f64, f64) = (0.01, 100.0);

impl Project {
    /// The initial setup (spec §5.3): one sheet holding every row, all ticked, the first
    /// default material, the design placed inside the margins at the start corner.
    pub fn new(prepared: PreparedDesign, machine: MachineProfile) -> Self {
        let group_by = GroupBy::Colour;
        let material = Material::defaults().remove(0);
        let start = WEEDING_MARGIN_MM + PLACEMENT_CLEARANCE_MM;
        let sheets = vec![Sheet {
            id: SHEET_ID.into(),
            name: "Sheet".into(),
            rows: RowKey::rows(&prepared, group_by),
            mirror: material.mirror,
            material: material.clone(),
            mat: Mat::Mat12x12,
            placement: Placement {
                offset_mm: [start, start],
                rotation_deg: 0.0,
            },
            weeding_box: true,
            weeding_margin_mm: WEEDING_MARGIN_MM,
            piece_mm: None,
            weed_lines: false,
            weed_inverted: false,
            layering_marks: false,
            test_spot_mm: None,
            piece_at_mm: None,
            mat_profile: None,
        }];
        Self {
            schema_version: PROJECT_SCHEMA_VERSION,
            prepared,
            group_by,
            excluded_rows: Vec::new(),
            sheets,
            machine,
            scale: 1.0,
            revision: 0,
            cut_record: CutRecord::default(),
        }
    }

    /// Resize the whole design, keeping its proportions (spec §5.1.1).
    pub fn set_scale(&mut self, scale: f64) -> Result<(), String> {
        let (low, high) = SCALE_RANGE;
        if !scale.is_finite() || !(low..=high).contains(&scale) {
            return Err(format!(
                "The size must be between {}% and {}% of the original.",
                low * 100.0,
                high * 100.0
            ));
        }
        self.scale = scale;
        self.revision += 1;
        Ok(())
    }

    /// Group rows by colour or by layer (spec §5.3). A new row stays unticked only when
    /// everything in it was unticked.
    pub fn set_group_by(&mut self, group_by: GroupBy) {
        if group_by == self.group_by {
            return;
        }
        let design = &self.prepared;
        // (colour, layer, is a cut line) of everything that is cut.
        let mut items: Vec<(Colour, String, bool)> = design
            .pieces
            .iter()
            .map(|p| (p.colour.clone(), p.layer.clone(), false))
            .collect();
        items.extend(
            design
                .cut_lines
                .iter()
                .map(|l| (l.colour.clone(), l.layer.clone(), true)),
        );
        let holds = |row: &RowKey, (colour, layer, line): &(Colour, String, bool)| {
            if *line {
                row.has_line(colour, layer)
            } else {
                row.has_piece(colour, layer)
            }
        };
        let rows = RowKey::rows(design, group_by);
        let excluded = rows
            .iter()
            .filter(|row| {
                let mut held = items.iter().filter(|item| holds(row, item)).peekable();
                held.peek().is_some()
                    && held.all(|item| self.excluded_rows.iter().any(|old| holds(old, item)))
            })
            .cloned()
            .collect();
        for sheet in &mut self.sheets {
            sheet.rows.clear();
        }
        if let Some(sheet) = self.sheets.first_mut() {
            sheet.rows = rows;
        }
        self.excluded_rows = excluded;
        self.group_by = group_by;
        self.revision += 1;
    }

    /// One sheet (spec §5.3): a project saved with several keeps the first sheet's settings and
    /// every row, ticked as it was. Their cut record no longer applies and is dropped.
    pub fn merge_sheets(&mut self) {
        if self.sheets.len() <= 1 {
            return;
        }
        let rows: Vec<RowKey> = self.sheets.iter().flat_map(|s| s.rows.clone()).collect();
        self.sheets.truncate(1);
        self.sheets[0].rows = rows;
        self.sheets[0].id = SHEET_ID.into();
        self.sheets[0].name = "Sheet".into();
        self.cut_record = CutRecord::default();
    }

    fn sheet_mut(&mut self, sheet: &str) -> Result<&mut Sheet, String> {
        self.sheets
            .iter_mut()
            .find(|s| s.id == sheet)
            .ok_or_else(|| format!("There is no {sheet}."))
    }

    /// Tick or untick a row (spec §5.3). Unticked rows stay listed and are not cut.
    pub fn set_row_included(&mut self, row: &RowKey, included: bool) -> Result<(), String> {
        if !self.sheets.iter().any(|s| s.rows.contains(row)) {
            return Err("That row is not on any sheet.".into());
        }
        self.excluded_rows.retain(|r| r != row);
        if !included {
            self.excluded_rows.push(row.clone());
        }
        self.revision += 1;
        Ok(())
    }

    /// Place a sheet's design on the mat, and choose the mat (spec §5.4).
    pub fn set_placement(
        &mut self,
        sheet: &str,
        placement: Placement,
        mat: Mat,
    ) -> Result<(), String> {
        if !placement
            .offset_mm
            .iter()
            .chain([&placement.rotation_deg])
            .all(|v| v.is_finite())
        {
            return Err("Placement values must be numbers.".into());
        }
        let sheet = self.sheet_mut(sheet)?;
        sheet.placement = placement;
        sheet.mat = mat;
        self.revision += 1;
        Ok(())
    }

    /// Mirror a sheet, or stop mirroring it, regardless of its material.
    pub fn set_mirror(&mut self, sheet: &str, mirror: bool) -> Result<(), String> {
        self.sheet_mut(sheet)?.mirror = mirror;
        self.revision += 1;
        Ok(())
    }

    /// Turn a sheet's weeding box on or off.
    pub fn set_weeding_box(&mut self, sheet: &str, on: bool) -> Result<(), String> {
        self.sheet_mut(sheet)?.weeding_box = on;
        self.revision += 1;
        Ok(())
    }

    /// Turn a sheet's weed lines on or off.
    pub fn set_weed_lines(&mut self, sheet: &str, on: bool) -> Result<(), String> {
        self.sheet_mut(sheet)?.weed_lines = on;
        self.revision += 1;
        Ok(())
    }

    /// Keep the background and weed the shapes (or the reverse).
    pub fn set_weed_inverted(&mut self, sheet: &str, on: bool) -> Result<(), String> {
        self.sheet_mut(sheet)?.weed_inverted = on;
        self.revision += 1;
        Ok(())
    }

    /// Turn a sheet's layering marks on or off.
    pub fn set_layering_marks(&mut self, sheet: &str, on: bool) -> Result<(), String> {
        self.sheet_mut(sheet)?.layering_marks = on;
        self.revision += 1;
        Ok(())
    }

    /// Put the test cut's top-left corner at `at` on the mat, or back to automatic (`None`).
    pub fn set_test_spot(&mut self, sheet: &str, at: Option<[f64; 2]>) -> Result<(), String> {
        if let Some([x, y]) = at {
            let mat = self.sheet_mut(sheet)?.mat;
            let length = self.machine.mat_length_mm(mat);
            if !(x.is_finite() && y.is_finite()) || x < 0.0 || y < 0.0 || x > 304.8 || y > length {
                return Err("The test cut must be on the mat.".into());
            }
        }
        self.sheet_mut(sheet)?.test_spot_mm = at;
        self.revision += 1;
        Ok(())
    }

    /// Use a real mat for a sheet: it loads at that mat's length.
    pub fn set_mat_profile(&mut self, sheet: &str, profile: MatProfile) -> Result<(), String> {
        profile.check()?;
        let sheet = self.sheet_mut(sheet)?;
        sheet.mat = profile.class;
        sheet.mat_profile = Some(profile);
        self.revision += 1;
        Ok(())
    }

    /// Move their piece of material: its top-left corner, from the cutter's start corner.
    pub fn set_piece_at(&mut self, sheet: &str, at: [f64; 2]) -> Result<(), String> {
        if !(at[0].is_finite() && at[1].is_finite()) {
            return Err("The piece's position must be numbers.".into());
        }
        self.sheet_mut(sheet)?.piece_at_mm = Some(at);
        self.revision += 1;
        Ok(())
    }

    /// Say how big their piece of material is, or forget it (`None`). It must fit the mat.
    pub fn set_piece(&mut self, sheet: &str, piece_mm: Option<[f64; 2]>) -> Result<(), String> {
        if let Some([w, h]) = piece_mm {
            if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
                return Err("The piece's width and height must be positive.".into());
            }
            let mat = self.sheet_mut(sheet)?.mat;
            let length = self.machine.mat_length_mm(mat);
            if w > 304.8 + 1e-6 || h > length + 1e-6 {
                return Err(format!(
                    "A {:.2} × {:.2} in piece is bigger than the {:.0} × {:.0} in mat.",
                    w / 25.4,
                    h / 25.4,
                    304.8 / 25.4,
                    length / 25.4
                ));
            }
        }
        self.sheet_mut(sheet)?.piece_mm = piece_mm;
        self.revision += 1;
        Ok(())
    }

    /// Give a sheet a material; iron-on materials mirror automatically (spec §5.6).
    pub fn set_material(&mut self, sheet: &str, material: Material) -> Result<(), String> {
        let sheet = self.sheet_mut(sheet)?;
        sheet.mirror = material.mirror;
        sheet.material = material;
        self.revision += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepared::{Contour, CutLine, Piece, Recipe};

    fn colour(hex: &str) -> Colour {
        Colour {
            hex: hex.into(),
            swatch: None,
        }
    }

    fn project() -> Project {
        let contour = Contour {
            start: [0.0, 0.0],
            segments: vec![[[1.0, 0.0], [1.0, 0.0], [1.0, 0.0]]],
            closed: true,
        };
        let piece = |id: &str, hex: &str| Piece {
            id: id.into(),
            layer: "source/0".into(),
            colour: colour(hex),
            source_ids: vec![],
            outer: contour.clone(),
            holes: vec![],
        };
        let prepared = PreparedDesign {
            schema_version: 1,
            stage: "preparation".into(),
            cut_ready: true,
            engine_revision: "test".into(),
            source_sha256: String::new(),
            recipe: Recipe::new(),
            fonts: vec![],
            page: None,
            pieces: vec![piece("piece/0", "#ff0000"), piece("piece/1", "#0000ff")],
            cut_lines: vec![CutLine {
                id: "line/0".into(),
                layer: "source/0".into(),
                colour: colour("#ff0000"),
                source_ids: vec![],
                contour,
            }],
            changes: vec![],
            pending: vec![],
            diagnostics: vec![],
            stroked_fills: 0,
        };
        Project::new(prepared, MachineProfile::cameo_1())
    }

    #[test]
    fn old_material_test_status_is_ignored_and_new_saves_omit_it() {
        let original = Material::defaults().remove(0);
        let mut value = serde_json::to_value(&original).unwrap();
        assert!(value.get("status").is_none());
        value["status"] = serde_json::json!("tested");
        let loaded: Material = serde_json::from_value(value).unwrap();
        assert_eq!(loaded, original);
        assert!(
            serde_json::to_value(loaded)
                .unwrap()
                .get("status")
                .is_none()
        );
    }

    #[test]
    fn one_sheet_holds_every_row_ticked_and_old_projects_merge() {
        let mut p = project();
        assert_eq!(p.sheets.len(), 1);
        assert_eq!(p.sheets[0].id, SHEET_ID);
        assert_eq!(p.sheets[0].rows.len(), 3, "red, blue and the red cut lines");
        assert!(p.excluded_rows.is_empty(), "everything is cut by default");
        // A project saved with a sheet per colour.
        let mut second = p.sheets[0].clone();
        second.id = "sheet/2".into();
        second.rows = p.sheets[0].rows.split_off(1);
        p.sheets.push(second);
        p.cut_record.complete("sheet/2", "plan");
        p.merge_sheets();
        assert_eq!(p.sheets.len(), 1);
        assert_eq!(p.sheets[0].rows.len(), 3);
        assert!(p.cut_record.is_empty());
    }

    #[test]
    fn rows_can_be_unticked_and_placement_validated() {
        let mut p = project();
        let red = RowKey::Pieces {
            colour: colour("#ff0000"),
        };
        p.set_row_included(&red, false).unwrap();
        assert_eq!(p.excluded_rows, std::slice::from_ref(&red));
        p.set_row_included(&red, true).unwrap();
        assert!(p.excluded_rows.is_empty());
        let bad = Placement {
            offset_mm: [f64::NAN, 0.0],
            rotation_deg: 0.0,
        };
        assert!(p.set_placement("sheet/1", bad, Mat::Mat12x12).is_err());
        let good = Placement {
            offset_mm: [20.0, 30.0],
            rotation_deg: 90.0,
        };
        p.set_placement("sheet/1", good.clone(), Mat::Mat12x24)
            .unwrap();
        assert_eq!(
            (p.sheets[0].placement.clone(), p.sheets[0].mat),
            (good, Mat::Mat12x24)
        );
    }

    #[test]
    fn regrouping_keeps_exclusions_only_for_wholly_unticked_rows() {
        let mut p = project();
        let red = RowKey::Pieces {
            colour: colour("#ff0000"),
        };
        p.set_row_included(&red, false).unwrap();
        p.set_group_by(GroupBy::Layer);
        assert_eq!(p.sheets.len(), 1);
        assert_eq!(p.sheets[0].rows.len(), 1, "one layer");
        assert!(
            p.excluded_rows.is_empty(),
            "the layer still has pieces to cut"
        );
        p.set_group_by(GroupBy::Colour);
        assert_eq!(p.sheets[0].rows.len(), 3);
        assert!(p.excluded_rows.is_empty());
        let blue = RowKey::Pieces {
            colour: colour("#0000ff"),
        };
        p.set_row_included(&blue, false).unwrap();
        p.set_group_by(GroupBy::Layer);
        p.set_group_by(GroupBy::Colour);
        assert!(p.excluded_rows.is_empty(), "the layer was partly cut");
        assert!(p.set_scale(0.0).is_err() && p.set_scale(f64::NAN).is_err());
        p.set_scale(2.0).unwrap();
        assert_eq!(p.scale, 2.0);
    }
}
