//! Rows of the rough-colour side tables.
//!
//! The vault never parses these payloads: `zoned_json` / `fit_json` are opaque JSON text and
//! the photo `data` is an opaque blob whose layout `encoding` names. The application
//! (`pleochroma-cut`) owns every format, so a change to one needs no migration here.

/// The fitted rough colour of one saved rough plan (`rough_colour`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoughColourRow {
    /// The saved rough plan this colour belongs to (`saved_rough_plans.plan_id`).
    pub plan_id: i64,
    /// The zoned absorption in the rough frame, as JSON text.
    pub zoned_json: String,
    /// The fit report (model comparison, uncertainty, leave-one-view-out), as JSON text.
    pub fit_json: String,
    /// The application's format version of both texts.
    pub version: u32,
    /// Unix seconds when the colour was stored.
    pub created: i64,
}

/// One cached working-resolution photo image of a plan (`rough_colour_photo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoughColourPhotoRow {
    /// The saved rough plan.
    pub plan_id: i64,
    /// The rig view the image belongs to.
    pub view: u32,
    /// What the image is (the application's name for it, for example `transmittance`).
    pub kind: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The layout of `data` (for example `f16le`, `f32le`, `u8`).
    pub encoding: String,
    /// The pixel bytes.
    pub data: Vec<u8>,
}

/// A photo row without its bytes, for listings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoughColourPhotoMeta {
    /// The saved rough plan.
    pub plan_id: i64,
    /// The rig view.
    pub view: u32,
    /// The application's name for the image.
    pub kind: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The layout of the bytes.
    pub encoding: String,
    /// Size of the stored bytes.
    pub byte_len: u64,
}

/// The pose a planned stone was assigned (`stone_pose_choice`): which of the box-symmetric
/// poses of the stone to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct StonePoseChoiceRow {
    /// The result (layout) of the plan, in the plan's own order.
    pub layout_index: u32,
    /// The stone within that layout.
    pub stone_index: u32,
    /// The pose index (`0` is the canonical pose the planner placed).
    pub pose: u8,
}

/// Everything the cutter chose for one stone of a plan's layout.
///
/// It is the `stone_pose_choice` row, with its nullable `free_pose_json`, `locked` and `reason` columns): a box-symmetric pose or a free
/// pose, whether the stone is locked, and the Reason for an override.
///
/// The vault never parses `free_pose_json`: the application owns its format.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StoneChoiceRow {
    /// The result (layout) of the plan, in the plan's own order.
    pub layout_index: u32,
    /// The stone within that layout.
    pub stone_index: u32,
    /// The pose index (`0` is the canonical pose the planner placed).
    pub pose: u8,
    /// The free pose (a re-scaled pose that is not one of the four box poses) as JSON text, if
    /// the cutter set one.
    pub free_pose_json: Option<String>,
    /// Whether the stone is locked (kept, and cut first).
    pub locked: bool,
    /// The cutter's Reason for the choice; empty when none was given.
    pub reason: String,
}

impl StoneChoiceRow {
    /// Whether the row says nothing (the canonical pose, no free pose, not locked, no Reason): a
    /// row like that is not stored.
    #[must_use]
    pub const fn is_default(&self) -> bool {
        self.pose == 0 && self.free_pose_json.is_none() && !self.locked && self.reason.is_empty()
    }
}

/// The zones of one custom material (`material_zoning`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialZoningRow {
    /// The custom material's name (`custom_gem_materials.name`).
    pub material_name: String,
    /// The zoned absorption as JSON text.
    pub zoned_json: String,
    /// Whether the zone geometry scales with the stone's width (a library zoned material)
    /// rather than being fixed in millimetres (a material adopted from a rough).
    pub relative_to_stone: bool,
    /// The application's format version of `zoned_json`.
    pub version: u32,
}

/// The Color Pattern of one custom material (`material_pattern`).
///
/// The vault never parses `pattern_json`: the application owns the format (see
/// `pleochroma_cut_core::material::pattern`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialPatternRow {
    /// The custom material's name (`custom_gem_materials.name`).
    pub material_name: String,
    /// The pattern as JSON text.
    pub pattern_json: String,
    /// The application's format version of `pattern_json`.
    pub version: u32,
}

/// One color variant a cutter saved for a material (`material_color_variants`).
///
/// The vault never parses `variant_json`: the application owns the format (see
/// `pleochroma_cut::gui::optics::color_variant_store`). The two timestamps are written by the
/// vault itself; a row handed to a writer may carry any text there, it is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialColorVariantRow {
    /// The variant's stable key (`user.<uuid>`), unique across all materials.
    pub key: String,
    /// The material the variant belongs to (a built-in or a custom material's name; compared
    /// ignoring ASCII case).
    pub material_name: String,
    /// The display label.
    pub label: String,
    /// The application's format version of `variant_json`.
    pub version: u32,
    /// The variant's tensor and unit as JSON text.
    pub variant_json: String,
    /// When the row was first stored: UTC, `YYYY-MM-DDTHH:MM:SSZ`.
    pub created_utc: String,
    /// When the row was last written: UTC, `YYYY-MM-DDTHH:MM:SSZ`.
    pub updated_utc: String,
}

/// One saved Color Pattern preset (`color_pattern_presets`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorPatternPresetRow {
    /// The preset's name, unique ignoring ASCII case.
    pub name: String,
    /// The pattern as JSON text.
    pub pattern_json: String,
    /// The application's format version of `pattern_json`.
    pub version: u32,
}
