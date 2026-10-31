//! The self-contained design file: one text file, extension `.psdd`.
//!
//! `.psdd` stands for Pleochroma Suite Design Document. The file carries a WHOLE faceting design with no
//! paired `.asc` and no machine-local reference.
//!
//! # Format
//!
//! UTF-8 text, TOML syntax, LF line endings. The first two keys are always
//! `format = "pleochroma-design"` and `version = 1`, so a reader can judge a file
//! from its first two lines (see [`super::codec`]): a file of another kind, or one
//! written by a newer major version, is refused with its own typed error before any
//! other key is looked at. Unknown keys -- at the top level, in `[preform]`,
//! `[meta]`, `[material]`, `[schedule]`, in every `[[tiers]]` entry and in every
//! `[[attachments]]` entry -- are kept in the
//! matching `unknown` table and written back unchanged, so a build never degrades a
//! file a newer minor revision wrote. Output is deterministic: the same document
//! always serialises to the same bytes (struct fields in declaration order, maps in
//! key order, no clock or host data in the body).
//!
//! # Tables
//!
//! | Key | Content |
//! | --- | --- |
//! | `format`, `version` | Header, always first. |
//! | `girdle_diameter_mm` | Real-world girdle diameter, when anchored. |
//! | `draft` | `true` when the design did not solve at save time; informational. |
//! | `[meta]` | Descriptive facts that cannot be recomputed: id, title, designer, source, notes, tags, rights, dates ([`DesignMetadata`]). |
//! | `[preform]` | Rough shape and size ([`PreformTable`]); a plane-set rough (`kind = "planes"`, the Rough Planner's piece) makes the file `version = 4`. |
//! | `[material]` | Material name, SG/RI overrides, custom snapshot, body color ([`MaterialTable`]). |
//! | `[schedule]` | Gear, symmetry, mirror, version banner, authored RI, headers, footnotes ([`ScheduleTable`]). |
//! | `[source]` | Printed Vol/W^3, L/W, C/W, P/W, H/W of a catalogue row ([`SourceTable`]). |
//! | `[history]` | Bounded trail of edit descriptions ([`HistoryTable`]). |
//! | `[render_effects]` | Per-design stone render effects, only when one is on ([`RenderEffectsTable`]); its optional subtables `[render_effects.inclusions]` and `[render_effects.structure]` and the `edge_abrasion_*` keys are written only when set. |
//! | `[crystal_axis_plan]` | The orientation rule a Rough Planner hand-off planned the stone under, only when handed over ([`CrystalAxisPlanTable`]). |
//! | `[[tiers]]` | Every tier in full, in cutting order ([`TierTable`]). |
//! | `[[concave_tiers]]` | Concave (fantasy-cut) tiers with their tool lines, only when present; the file is then `version = 2` and carries `concave_frame = "v0"`; each record may carry a stable `concave_tier_id` ([`ConcaveTierTable`]). |
//! | `[[attachments]]` | Files that cannot be recomputed, as base64 with size and SHA-256 ([`AttachmentTable`]). |
//!
//! # Tiers
//!
//! Every `[[tiers]]` entry is self-describing: its name, meet constraint, detached
//! values, angle, index-wheel positions, imported meet, original notes, free-text
//! note, cheater offset, authoring target, optional angle relation, optional frosting
//! (`frosted = true` for the whole tier, or `frosted_facets = [..]` with the index-wheel
//! positions of single frosted facets; neither is written for a polished tier, and neither
//! changes the file version -- a build that predates them keeps them in the tier's `unknown`
//! table and writes them back), optional surface finish (`surface = "Commercial"` with
//! `surface_rq_nm`, `surface_lap_deg`, `surface_aniso`, `surface_no_sweep`, `ground_alpha` and
//! per-facet `surface_facets`; none is written for an ideal tier, and none changes the file
//! version) and its stable `tier_id`. Nothing about a
//! tier lives anywhere else in the file, so no value is ever matched to a tier by its
//! array position: reordering the entries moves each tier's note, offset and target
//! with it. `angle_deg` and `indices` are required; `tier_id` is expected and unique
//! (a missing id is assigned on load, a repeated one is refused).
//!
//! `angle_relation` is canonical relation text such as `"@3 - 2"`: the tier's angle is
//! that expression over other tiers' angle magnitudes, named by `tier_id`. The text
//! is interpreted by `pleochroma-cut-core`; this crate stores it. `angle_deg` still
//! holds the resulting angle. A file in which any tier has an `angle_relation` is
//! `version = 3` ([`DESIGN_VERSION_RELATIONS`]); a file whose `[preform]` shape is a plane
//! set is `version = 4` ([`DESIGN_VERSION_PLANES`]); every other file keeps its version.
//!
//! # Metadata and attachments: what is stored, what is recomputed
//!
//! The rule: everything an importer can RESTORE from the file's other content is NOT
//! stored; everything else about the design IS. Where restorability was doubtful the
//! field is stored. Text fields use the empty string for "not set" and are then not
//! written. Timestamps are ISO-8601 UTC text and the `id` a UUID string, both supplied
//! by the caller: the codec reads no clock and generates no randomness, so the same
//! input is always the same bytes.
//!
//! | Library field | In the file | How an importer restores it when not stored |
//! | --- | --- | --- |
//! | name / title | STORED `meta.title` | |
//! | stable design id | STORED `meta.id` | |
//! | designer | STORED `meta.designer` | |
//! | `designer_info` (display line) | STORED `meta.designer_info` | (doubt: a hand-edited line need not equal the join of designer and citation) |
//! | `source_citation` | STORED `meta.source_citation` | |
//! | `page_url` | STORED `meta.source_url` | |
//! | `design_id` (catalogue's own id text) | STORED `meta.source_design_id` | |
//! | shape label | STORED `meta.shape` | |
//! | `shape_category` | STORED `meta.shape_category` | |
//! | `competition_diagram` | STORED `meta.competition` | |
//! | `pdf_file` / `gem_file` names | STORED `meta.pdf_file`, `meta.gem_file` | |
//! | notes, tags | STORED `meta.notes`, `meta.tags` | |
//! | license, rights | STORED `meta.license`, `meta.copyright` (new) | |
//! | created / modified time | STORED `meta.created_at`, `meta.modified_at` | |
//! | ignored mark | STORED `meta.ignored` (doubt: library curation) | |
//! | Rough Planner exclusion | STORED `meta.planner_excluded` | |
//! | attached PDF, original `.gem`, original `.asc`, other files | STORED `[[attachments]]` | |
//! | diagram image (name + bytes) | STORED `[[attachments]]`, role `diagram_image` (doubt: the page may be gone; drawing it needs a render) | |
//! | printed Vol/W^3, L/W, C/W, P/W, H/W | STORED `[source]` | |
//! | lw / hw / tw / uw / pw / cw ratios, volume | DERIVED | solved geometry of the tiers (`[source]` keeps only the printed figures) |
//! | `facets_count` (`"55+6"`) | DERIVED | counted from the solved tiers |
//! | `index_gear`, `symmetry_order`, mirror | DERIVED | `[schedule]` gear, symmetry and mirror |
//! | `refractive_index` | DERIVED | `[schedule]`/`[material]` |
//! | angle-settings table | DERIVED | rebuilt from the tiers and the gear |
//! | `diagram_entries.url` (`local://...`), row ids | DERIVED | machine-local; the importer assigns its own |
//! | solid hull, solid extents | DERIVED | recomputed from the solved geometry |
//! | preview images, tilt curves, chosen preview material | DERIVED | rendered/derived again from the design |
//! | saved Rough Planner results | DERIVED | the planner is run again |
//!
//! `[[attachments]]` entries are `name` (unique, at most 255 bytes, no path
//! separators), `role` (`diagram_image`, `pdf`, `gem`, `asc` or `other`), `mime_type`,
//! optional `url`, `size`, lowercase hex `sha256` and `data` (standard base64 with
//! padding, one line). The summed size is limited to [`super::MAX_ATTACHMENT_BYTES`]; a size
//! or hash that does not match the bytes, a repeated name or an oversize payload makes
//! the whole file refused, on read and on write.
//!
//! # What is deliberately absent
//!
//! No hash of an `.asc`, no `.asc` file name and no catalogue entry id: all three
//! are machine-local and the file does not depend on any of them.
//!
//! # Example
//!
//! ```text
//! format = "pleochroma-design"
//! version = 1
//! girdle_diameter_mm = 6.5
//!
//! [meta]
//! id = "0b5f0a8e-5a43-4d52-9c1f-7c8f3a1e2d90"
//! title = "Round brilliant"
//! designer = "Capps, Jerry"
//! source_citation = "Lapidary Journal, May 1994, p95"
//! notes = "Cut in quartz first"
//! created_at = "2026-10-02T09:30:00Z"
//! tags = ["round", "classic"]
//! planner_excluded = true
//!
//! [preform]
//! shape = { kind = "block" }
//! half_width = 1.0
//! length_over_width = 1.0
//! depth = 2.0
//! y_offset = 0.0
//!
//! [material]
//! name = "Diamond"
//! refractive_index_override = 1.62
//!
//! [schedule]
//! gemcad_version = "GemCad 5.0"
//! gear_teeth = 96
//! gear_reference_angle = 0.0
//! symmetry_order = 4
//! mirror = true
//! refractive_index = 1.62
//! headers = ["Round brilliant"]
//! footnotes = []
//!
//! [source]
//! vol_w3 = 0.61
//! lw = 1.0
//!
//! [history]
//! entries = ["Set angle of Girdle"]
//!
//! [[tiers]]
//! name = "Girdle"
//! constraint = { kind = "scale_reference", mast = 1.0 }
//! angle_deg = 90.0
//! indices = [0.0, 24.0, 48.0, 72.0]
//! original_notes = "Set girdle thickness"
//! tier_id = 0
//!
//! [[tiers]]
//! name = "Table"
//! constraint = { kind = "meet_existing" }
//! angle_deg = 0.0
//! indices = []
//! note = "polish last"
//! tier_id = 1
//! target = { kind = "table_width_mm", mm = 4.1 }
//!
//! [[attachments]]
//! name = "round.pdf"
//! role = "pdf"
//! mime_type = "application/pdf"
//! size = 4
//! sha256 = "315d429b7714cedb6ad04ac31240145257692630457f3c88253c5beceac76027"
//! data = "JVBERg=="
//! ```

use super::{
    DesignMetadata,
    attachment::AttachmentTable,
    effects_tables::{InclusionEffectsTable, StructureEffectsTable},
};
use crate::native::{
    ConcaveTierTable, HistoryTable, MaterialTable, NativePreformShape, PreformTable, SourceTable,
    TierTable,
};

/// The value of the `format` header key.
pub const DESIGN_FORMAT: &str = "pleochroma-design";

/// The `format` value of design files written before the rename to Pleochroma. Read
/// exactly like [`DESIGN_FORMAT`], never written: a parsed file carries [`DESIGN_FORMAT`].
pub const LEGACY_DESIGN_FORMAT: &str = "indicatrix-design";

/// `true` iff `format` names a design file: [`DESIGN_FORMAT`] or [`LEGACY_DESIGN_FORMAT`].
#[must_use]
pub fn is_design_format(format: &str) -> bool {
    format == DESIGN_FORMAT || format == LEGACY_DESIGN_FORMAT
}

/// This module's own major schema version, the value of the `version` header key.
///
/// A file with a larger `version` is refused outright; a smaller addition (a new
/// optional key) does not bump it, because unknown keys are preserved.
pub const DESIGN_VERSION: u32 = 1;

/// The schema version of a file that carries concave tiers.
///
/// Written only when [`DesignFile::concave_tiers`] is non-empty, so every planar
/// file stays version 1 and byte-identical, and a build that predates concave
/// tiers refuses the file with `UnsupportedVersion` instead of silently showing
/// the stone without its concave cuts (plan §6.1, decision Q5).
pub const DESIGN_VERSION_CONCAVE: u32 = 2;

/// The schema version of a file in which at least one tier carries an
/// `angle_relation` (its angle is driven by an expression over other tiers' angles).
///
/// Written only when a relation exists, exactly like [`DESIGN_VERSION_CONCAVE`]: a
/// design without relations stays version 1 (or 2 with concave tiers) and
/// byte-identical, and a build that predates relations refuses a version 3 file with
/// `UnsupportedVersion` instead of opening it and losing the relations on the next
/// save. A version 3 file may also hold concave tiers (and then carries
/// `concave_frame` as usual).
pub const DESIGN_VERSION_RELATIONS: u32 = 3;

/// The schema version of a file whose `[preform]` is a plane set
/// (`shape = { kind = "planes", planes = [...], source = "..." }`), the piece of rough
/// the Rough Planner hands the editor.
///
/// Written only when the preform is a plane set, exactly like [`DESIGN_VERSION_CONCAVE`]
/// and [`DESIGN_VERSION_RELATIONS`]: every block or cylinder file keeps its version and
/// stays byte-identical, and a build that predates plane-set preforms refuses a version 4
/// file with `UnsupportedVersion` instead of failing on an unknown shape kind (or opening
/// the stone without its rough). A version 4 file may also hold concave tiers and
/// relations.
pub const DESIGN_VERSION_PLANES: u32 = 4;

/// The concave-tier frame convention this build writes and reads (plan §11a).
///
/// It fixes where X, Y, Z are measured from and what θ is relative to. Stored next to the
/// tiers because the numbers mean nothing without it; a reader that meets any
/// other string refuses the file. `pleochroma-cut-core`'s `concave_frame`
/// module owns the convention itself and asserts it equals this string.
pub const CONCAVE_FRAME_V0: &str = "v0";

/// Upper bound on the number of tiers a file may hold (flat and concave together).
pub const MAX_DESIGN_TIERS: usize = 10_000;

fn default_format() -> String {
    DESIGN_FORMAT.to_string()
}

const fn default_version() -> u32 {
    DESIGN_VERSION
}

/// The `[schedule]` table: the index-wheel and header fields a `.asc` schedule
/// carries besides its tiers.
///
/// Without these a tier's `indices` cannot be interpreted (the gear tooth count
/// decides what an index position means), which is why the file stores them itself
/// instead of relying on a paired `.asc`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScheduleTable {
    /// The `GemCad` version banner of the originating schedule (empty when none).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gemcad_version: String,
    /// Tooth count of the index gear; negative for a gear counted the other way.
    pub gear_teeth: i32,
    /// Reference angle of the index gear, in degrees.
    pub gear_reference_angle: f64,
    /// Rotational symmetry order.
    pub symmetry_order: u32,
    /// Whether the schedule carries mirror symmetry.
    pub mirror: bool,
    /// The AUTHORED (legacy schedule) refractive index, as last set or imported;
    /// distinct from the effective index the material resolves to.
    pub refractive_index: f64,
    /// Free-text header lines of the schedule.
    #[serde(default)]
    pub headers: Vec<String>,
    /// Free-text footnote lines of the schedule.
    #[serde(default)]
    pub footnotes: Vec<String>,
    /// Keys a newer build wrote that this build does not claim; written back as read.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

/// The optional `[render_effects]` table: the per-design stone effects of the render
/// (inclusion haze, crystal-axis orientation, frosted girdle, edge rounding).
///
/// Written only when at least one effect is on (`DesignFile::render_effects` is `None`
/// otherwise), so every file without effects stays byte-identical to what earlier builds
/// wrote, and a file without the table reads as "all effects off". Every key has a serde
/// default, so a table written by an older or newer revision still loads.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RenderEffectsTable {
    /// Inclusion / subsurface scattering `sigma_s` per model unit; `0.0` is off.
    #[serde(default)]
    pub inclusion_sigma_s: f64,
    /// Whether the crystal-axis orientation override is on.
    #[serde(default)]
    pub c_axis_override_enabled: bool,
    /// Tilt of the override axis from the table normal, degrees (`0..=90`).
    #[serde(default)]
    pub c_axis_tilt_deg: f64,
    /// Azimuth of the override axis around the table normal, degrees (`0..=360`).
    #[serde(default)]
    pub c_axis_azimuth_deg: f64,
    /// Turn of the Second Axis about the crystal axis, degrees (`0..=180`); written only
    /// when set, so a file without it reads as "not set".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_axis_second_azimuth_deg: Option<f64>,
    /// Bruted (frosted) girdle finish.
    #[serde(default)]
    pub girdle_frosted: bool,
    /// Facet-edge rounding radius in model units (`0.0..=0.03`); `0.0` is off.
    #[serde(default)]
    pub edge_rounding_radius: f64,
    /// Width of the abraded band along every facet edge, in model units; `0.0` is off and not
    /// written.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub edge_abrasion_width: f64,
    /// GGX roughness of the abraded band; not written while `0.0`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub edge_abrasion_alpha: f64,
    /// The polish roughness (nm Rq) the facet next to the band is dulled to at the band's rim;
    /// `0.0` is off and not written.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub edge_abrasion_halo_rq_nm: f64,
    /// The inclusion choice (`[render_effects.inclusions]`); absent (and not written) when off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inclusions: Option<InclusionEffectsTable>,
    /// The play-of-color choice (`[render_effects.structure]`); absent (and not written) when
    /// off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structure: Option<StructureEffectsTable>,
    /// Keys a newer build wrote that this build does not claim; written back as read.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

/// Whether `value` is exactly zero: the "not set" state of the optional numeric keys.
#[cfg_attr(
    target_pointer_width = "64",
    expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde's `skip_serializing_if` passes the field by reference; the lint's size \
                  limit is the pointer width, so an 8-byte `f64` only trips it on 64-bit targets"
    )
)]
const fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

impl RenderEffectsTable {
    /// Whether every effect is off (the table is then not written at all).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inclusion_sigma_s == 0.0
            && !self.c_axis_override_enabled
            && self.c_axis_tilt_deg == 0.0
            && self.c_axis_azimuth_deg == 0.0
            && self.c_axis_second_azimuth_deg.is_none()
            && !self.girdle_frosted
            && self.edge_rounding_radius == 0.0
            && self.edge_abrasion_width == 0.0
            && self.edge_abrasion_alpha == 0.0
            && self.edge_abrasion_halo_rq_nm == 0.0
            && self.inclusions.is_none()
            && self.structure.is_none()
            && self.unknown.is_empty()
    }
}

/// The optional `[crystal_axis_plan]` table: the orientation rule a stone was planned under
/// (the Rough Planner's hand-off writes it) and the Reason of an override, for the cutting sheet.
///
/// Written only when the hand-off had something to record, so every other file stays
/// byte-identical to what earlier builds wrote. The words (`rule`, `strength`, `source`,
/// `preset`) are stable lower-case keys that `pleochroma-cut-core` reads and writes; this crate
/// stores them. Every key has a serde default, so a table from an older or newer revision still
/// loads.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CrystalAxisPlanTable {
    /// `none`, `table_across`, `in_table_plane`, `custom_angle` or `face_up`.
    #[serde(default)]
    pub rule: String,
    /// The tilt of a `custom_angle` rule, degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tilt_deg: Option<f64>,
    /// The azimuth of a `custom_angle` rule, degrees, when it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azimuth_deg: Option<f64>,
    /// The color-named preset of a `face_up` rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// The angle within which the stone counts as following the rule, degrees.
    #[serde(default)]
    pub tolerance_deg: f64,
    /// `preferred` or `required`.
    #[serde(default)]
    pub strength: String,
    /// Percent of the score per degree beyond the tolerance of a `preferred` rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight_per_deg: Option<f64>,
    /// Where the rule came from: `plan`, `design` or `material_default`.
    #[serde(default)]
    pub source: String,
    /// Why the cutter overrode the rule; empty (and not written) when no reason was given.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// Keys a newer build wrote that this build does not claim; written back as read.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

/// The whole self-contained design document -- see the module documentation.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DesignFile {
    /// The format name; always [`DESIGN_FORMAT`] for a file this build writes.
    #[serde(default = "default_format")]
    pub format: String,
    /// The major schema version: [`DESIGN_VERSION`], [`DESIGN_VERSION_CONCAVE`] for a
    /// file with concave tiers (see [`DesignFile::with_concave_tiers`]),
    /// [`DESIGN_VERSION_RELATIONS`] for a file in which a tier has an `angle_relation`,
    /// or [`DESIGN_VERSION_PLANES`] for a file whose preform is a plane set
    /// (see [`DesignFile::needed_version`]).
    #[serde(default = "default_version")]
    pub version: u32,
    /// Real-world girdle diameter in millimetres, when the design is anchored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub girdle_diameter_mm: Option<f64>,
    /// `true` when the design did not solve when the file was saved. Informational:
    /// every tier carries its full geometry either way.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub draft: bool,
    /// The frame convention the concave tiers are expressed in: [`CONCAVE_FRAME_V0`]
    /// when [`Self::concave_tiers`] is non-empty, empty otherwise. Declared
    /// ahead of every table: a TOML scalar written after a table header would
    /// belong to that table.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub concave_frame: String,
    /// Descriptive metadata; nothing is written when it is empty.
    #[serde(default, skip_serializing_if = "DesignMetadata::is_empty")]
    pub meta: DesignMetadata,
    /// Rough shape and size.
    pub preform: PreformTable,
    /// Material selection.
    pub material: MaterialTable,
    /// Gear, symmetry, mirror, banner, authored RI, headers and footnotes.
    pub schedule: ScheduleTable,
    /// Printed proportions of the catalogue row the design came from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceTable>,
    /// Bounded trail of edit descriptions, oldest first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistoryTable>,
    /// Per-design stone render effects; absent (and not written) when all are off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_effects: Option<RenderEffectsTable>,
    /// The orientation rule the stone was planned under; absent (and not written) for a design
    /// that was not handed over by the Rough Planner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crystal_axis_plan: Option<CrystalAxisPlanTable>,
    /// Every tier in full, in cutting order.
    #[serde(default)]
    pub tiers: Vec<TierTable>,
    /// Concave (fantasy-cut) tiers in cutting order within their groups; empty (and
    /// not written) for a planar design. Non-empty makes the file version 2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub concave_tiers: Vec<ConcaveTierTable>,
    /// Files kept byte for byte (see the module documentation), in the order given.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentTable>,
    /// Top-level keys a newer build wrote that this build does not claim.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

impl DesignFile {
    /// Builds a fresh document: header at the current version, not a draft, no source,
    /// no history, no unknown keys.
    #[must_use]
    pub fn new(
        preform: PreformTable,
        material: MaterialTable,
        schedule: ScheduleTable,
        girdle_diameter_mm: Option<f64>,
        tiers: Vec<TierTable>,
    ) -> Self {
        let mut file = Self {
            format: DESIGN_FORMAT.to_string(),
            version: DESIGN_VERSION,
            girdle_diameter_mm,
            draft: false,
            meta: DesignMetadata::default(),
            preform,
            material,
            schedule,
            source: None,
            history: None,
            render_effects: None,
            crystal_axis_plan: None,
            tiers,
            concave_tiers: Vec::new(),
            concave_frame: String::new(),
            attachments: Vec::new(),
            unknown: toml::Table::new(),
        };
        file.version = file.needed_version();
        file
    }

    /// The lowest version that can hold this document: [`DESIGN_VERSION_PLANES`] when
    /// the preform is a plane set, else [`DESIGN_VERSION_RELATIONS`] when any tier has
    /// an `angle_relation`, else [`DESIGN_VERSION_CONCAVE`] when there are concave
    /// tiers, else [`DESIGN_VERSION`]. A planar document without relations and with a
    /// block or cylinder preform is therefore always version 1.
    #[must_use]
    pub fn needed_version(&self) -> u32 {
        if matches!(self.preform.shape, NativePreformShape::Planes { .. }) {
            DESIGN_VERSION_PLANES
        } else if self.tiers.iter().any(|tier| tier.angle_relation.is_some()) {
            DESIGN_VERSION_RELATIONS
        } else if self.concave_tiers.is_empty() {
            DESIGN_VERSION
        } else {
            DESIGN_VERSION_CONCAVE
        }
    }

    /// Sets [`Self::meta`].
    #[must_use]
    pub fn with_meta(mut self, meta: DesignMetadata) -> Self {
        self.meta = meta;
        self
    }

    /// Sets [`Self::attachments`] from `blobs`, encoded in order (limits are checked
    /// when the file is written, see [`super::to_string`]).
    #[must_use]
    pub fn with_attachments(mut self, blobs: &[super::AttachmentBlob]) -> Self {
        self.attachments = super::attachment_tables(blobs);
        self
    }

    /// Attaches [`Self::source`]; `None` when the table has nothing to write.
    #[must_use]
    pub fn with_source(mut self, source: SourceTable) -> Self {
        self.source = (!source.is_empty()).then_some(source);
        self
    }

    /// Attaches [`Self::history`]; `None` when the table has nothing to write.
    #[must_use]
    pub fn with_history(mut self, history: HistoryTable) -> Self {
        self.history = (!history.is_empty()).then_some(history);
        self
    }

    /// Attaches [`Self::render_effects`]; `None` when every effect is off.
    #[must_use]
    pub fn with_render_effects(mut self, effects: RenderEffectsTable) -> Self {
        self.render_effects = (!effects.is_empty()).then_some(effects);
        self
    }

    /// Sets [`Self::crystal_axis_plan`] (`None` writes no table).
    #[must_use]
    pub fn with_crystal_axis_plan(mut self, plan: Option<CrystalAxisPlanTable>) -> Self {
        self.crystal_axis_plan = plan;
        self
    }

    /// Sets [`Self::concave_tiers`] and keeps the two fields that depend on them
    /// consistent: a non-empty list makes the file [`DESIGN_VERSION_CONCAVE`] with
    /// [`CONCAVE_FRAME_V0`]; an empty one restores the planar header (version
    /// [`DESIGN_VERSION`], no frame), so a planar design can never be written as
    /// version 2. A document whose tiers carry relations stays
    /// [`DESIGN_VERSION_RELATIONS`] either way (see [`Self::needed_version`]).
    #[must_use]
    pub fn with_concave_tiers(mut self, concave_tiers: Vec<ConcaveTierTable>) -> Self {
        self.concave_frame = if concave_tiers.is_empty() {
            String::new()
        } else {
            CONCAVE_FRAME_V0.to_string()
        };
        self.concave_tiers = concave_tiers;
        self.version = self.needed_version();
        self
    }

    /// Sets [`Self::draft`].
    #[must_use]
    pub const fn with_draft(mut self, draft: bool) -> Self {
        self.draft = draft;
        self
    }
}
