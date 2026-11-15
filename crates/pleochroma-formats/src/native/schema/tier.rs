//! The per-tier tables of the native document: [`TierTable`] (one `[[tiers]]` entry) and
//! [`ConcaveTierTable`] (one concave tier of a self-contained design file).

use super::{NativeMeetConstraint, NativeTierTarget};

#[cfg(doc)]
use super::PreformTable;

/// One `[[tiers]]` entry of a design file: a tier in full, in cutting order.
///
/// `angle_deg` and `indices` are optional in the type so a record missing them still
/// parses; the design file's validation refuses such a tier. The other fields carry what
/// a catalogue `.asc` cannot express: the authored [`NativeMeetConstraint`], the detached
/// parameter values, and the meet and notes text an imported `.asc` `G` field stated.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TierTable {
    /// Display name.
    pub name: String,
    /// Meet-point constraint.
    pub constraint: NativeMeetConstraint,
    /// Detached parameter values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detached: Vec<f64>,
    /// Mirrors `pleochroma_cut_core::design::ConstraintTier::angle_deg`. Validation refuses
    /// a tier that lacks it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle_deg: Option<f64>,
    /// Mirrors `pleochroma_cut_core::design::ConstraintTier::indices`. Same rule as
    /// [`Self::angle_deg`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indices: Option<Vec<f64>>,
    /// Mirrors `pleochroma_cut_core::design::ConstraintTier::imported_meet`: the meet
    /// instruction the `.asc` `G` field stated for this tier at import. Kept so a reload
    /// can offer the editor's "Adopt" action without re-deriving it from `.asc` text, which
    /// a draft save may not hold reliably. Absent loads as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_meet: Option<NativeMeetConstraint>,
    /// Mirrors `pleochroma_cut_core::design::ConstraintTier::original_notes`: the raw `.asc`
    /// `G`-field text of this tier, verbatim, as imported. Absent loads as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_notes: Option<String>,
    /// A cutter-authored free-text note for this tier. Unlike [`Self::original_notes`], it
    /// has no `.asc` counterpart, so it lives only here. Absent loads as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A per-tier "cheater" azimuth offset in degrees, keyed by array position. Mirrors
    /// `pleochroma_cut_core::design::Design::cheater_offsets_deg`. Like [`Self::note`], it has
    /// no `.asc` counterpart. Absent loads as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cheater_offset_deg: Option<f64>,
    /// A stable per-tier identity, mirroring `pleochroma_cut_core::design::TierId::value`.
    /// `cheater_offset_deg` and `note` are keyed by array position, which renumbers on add,
    /// remove and move; a `TierId` does not. Absent loads as `None`, and the loader then
    /// assigns a fresh id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_id: Option<u64>,
    /// The tier's authoring-level target, if any (see [`NativeTierTarget`]). Absent loads
    /// as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<NativeTierTarget>,
    /// The relation driving this tier's angle, in canonical text (`@3 - 2`, naming tiers by
    /// stable id so a rename does not break it). Mirrors
    /// `pleochroma_cut_core::design::Design::tier_relations` one-to-one; this crate does not
    /// interpret the text. `angle_deg` still holds the angle the relation gives. A tier
    /// without a relation writes no key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle_relation: Option<String>,
    /// Whether every facet of this tier is frosted, a diffuse finish instead of a polished
    /// one. Mirrors `pleochroma_cut_core::design::TierFrosting::Tier`. A polished tier
    /// writes no key. No version bump: an older build keeps the key in [`Self::unknown`]
    /// and writes it back unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub frosted: bool,
    /// Index-wheel positions of the frosted facets when only some are frosted. Mirrors
    /// `pleochroma_cut_core::design::TierFrosting::Facets`. Empty when the tier is polished
    /// or frosted as a whole, and then not written. Same rules as [`Self::frosted`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frosted_facets: Vec<f64>,
    /// The surface finish of the tier: `"Mirror"`, `"Fine"`, `"Commercial"`, `"Prepolish"`,
    /// `"Ground (Fine)"`, `"Ground (Coarse)"` or `"Custom"`. Absent for an ideal polish.
    /// Mirrors `pleochroma_cut_core::design::TierSurface`. The companion keys
    /// ([`Self::surface_rq_nm`], [`Self::surface_lap_deg`], [`Self::surface_aniso`],
    /// [`Self::surface_no_sweep`], [`Self::ground_alpha`], [`Self::surface_facets`]) are
    /// skipped at their defaults and need no version bump, as for [`Self::frosted`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    /// The RMS roughness in nanometres of a `"Custom"` polish.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_rq_nm: Option<f64>,
    /// The fixed scratch direction in degrees from the girdle-parallel direction (the Custom
    /// Angle); absent follows the faceting machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_lap_deg: Option<f64>,
    /// The anisotropy ratio (at least 1) of a `"Custom"` finish.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_aniso: Option<f64>,
    /// Whether the fan of scratch directions is collapsed to the machine's nominal angle.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub surface_no_sweep: bool,
    /// The GGX roughness of a ground finish, and of a frosted tier; absent is the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_alpha: Option<f64>,
    /// Facets whose surface differs from the tier's, by index-wheel position.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surface_facets: Vec<SurfaceFacetTable>,
    /// The corner this tier is cut to: three facets, each named by the `tier_id` of its
    /// tier and its index-wheel position. Mirrors `Design::tier_meet_picks`. Empty when
    /// the tier has no pick, and then not written. A file with any pick is version 5
    /// (see `DESIGN_VERSION_MEET_PICKS`), so an older build refuses it instead of solving
    /// the tier by the shallowest rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub meet_pick: Vec<MeetPickFacetTable>,
    /// Carries unknown keys through a save, as [`PreformTable::unknown`] does.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

/// One facet of a picked meet corner: the tier's `tier_id` and the facet's index-wheel
/// position.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MeetPickFacetTable {
    /// The `tier_id` of the tier the facet belongs to.
    pub tier: u64,
    /// The index-wheel position of the facet within that tier.
    pub at: f64,
}

/// One entry of [`TierTable::surface_facets`]: one facet's own surface.
///
/// The facet is the one at the index-wheel position `at`; its surface overrides the tier's.
/// Same keys as the tier's own, with `surface` defaulting to ideal (so an entry can restore a
/// perfect polish on one facet).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SurfaceFacetTable {
    /// The index-wheel position of the facet.
    pub at: f64,
    /// The grade name; absent is ideal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    /// See [`TierTable::surface_rq_nm`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rq_nm: Option<f64>,
    /// See [`TierTable::surface_lap_deg`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lap_deg: Option<f64>,
    /// See [`TierTable::surface_aniso`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aniso: Option<f64>,
    /// See [`TierTable::surface_no_sweep`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_sweep: bool,
    /// See [`TierTable::ground_alpha`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_alpha: Option<f64>,
}

/// The surface columns of a [`TierTable`] as one value, for [`TierTable::with_surface`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SurfaceColumns {
    /// [`TierTable::surface`].
    pub surface: Option<String>,
    /// [`TierTable::surface_rq_nm`].
    pub rq_nm: Option<f64>,
    /// [`TierTable::surface_lap_deg`].
    pub lap_deg: Option<f64>,
    /// [`TierTable::surface_aniso`].
    pub aniso: Option<f64>,
    /// [`TierTable::surface_no_sweep`].
    pub no_sweep: bool,
    /// [`TierTable::ground_alpha`].
    pub ground_alpha: Option<f64>,
    /// [`TierTable::surface_facets`].
    pub facets: Vec<SurfaceFacetTable>,
}

/// One concave (fantasy-cut) tier of a self-contained design file: the facet line and tool
/// line of the two-line faceting-diagram standard, kept in authored form so nothing is lost.
///
/// A separate table from [`TierTable`]: a concave tier has no meet constraint, mast or
/// target, and an older build refuses the file by version rather than mistaking a concave
/// tier for a flat one. Tool and motion keep the standard's strings: `tool` is `"CYL"`,
/// `"CON"`, `"CIR"`, `"DSC"` or `"SPH"`, `motion` is `"reciprocating"` or `"plunge"`.
/// This crate does not interpret them; the loader in `pleochroma-cut-core` refuses a string
/// it does not know. `tool`, `diameter_ratio` and `motion` have no serde default, so a
/// record missing one is an error rather than a guessed tool.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConcaveTierTable {
    /// Facet name, free text.
    pub name: String,
    /// φ in degrees, signed like a flat tier's `angle_deg`.
    pub angle_deg: f64,
    /// Index-wheel positions of the placements.
    pub indices: Vec<f64>,
    /// Free-text cutting instructions, kept verbatim.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    /// The tool code.
    pub tool: String,
    /// θ: direction of the tool axis in the facet plane, degrees.
    pub tool_azimuth_deg: f64,
    /// X, Y, Z displacement as ratios of the stone width.
    pub displacement: [f64; 3],
    /// Tool diameter as a ratio of the stone width.
    pub diameter_ratio: f64,
    /// Included angle of a cone or disc; absent for the other tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_angle_deg: Option<f64>,
    /// How the tool moves while it cuts.
    pub motion: String,
    /// A stable per-tier identity, the concave counterpart of [`TierTable::tier_id`]. The
    /// cutting-mode marks of a concave step hang on it, so it survives a save and a reopen,
    /// unlike the list position. Drawn from the same counter as the flat ids, so a value
    /// never repeats within a file. Absent loads as `None` (the loader then assigns a fresh
    /// id). No version bump is needed: an older build keeps the key in [`Self::unknown`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concave_tier_id: Option<u64>,
    /// Carries unknown keys through a save, as [`PreformTable::unknown`] does.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

impl TierTable {
    /// Builds an entry with no unknown keys and every optional field unset, including
    /// `angle_deg`, `indices`, `imported_meet`, `original_notes` and `cheater_offset_deg`.
    /// A caller sets those with the matching `with_*` method. Construction lives here so
    /// callers need not name `toml::Table` to fill `unknown` (see [`PreformTable::new`]).
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        constraint: NativeMeetConstraint,
        detached: Vec<f64>,
    ) -> Self {
        Self {
            name: name.into(),
            constraint,
            detached,
            angle_deg: None,
            indices: None,
            imported_meet: None,
            original_notes: None,
            note: None,
            cheater_offset_deg: None,
            tier_id: None,
            target: None,
            angle_relation: None,
            frosted: false,
            frosted_facets: Vec::new(),
            surface: None,
            surface_rq_nm: None,
            surface_lap_deg: None,
            surface_aniso: None,
            surface_no_sweep: false,
            ground_alpha: None,
            surface_facets: Vec::new(),
            meet_pick: Vec::new(),
            unknown: toml::Table::new(),
        }
    }

    /// Attaches the surface columns ([`Self::surface`] and the keys that follow it) -- see
    /// [`Self::surface`]'s doc comment. The default [`SurfaceColumns`] writes nothing.
    #[must_use]
    pub fn with_surface(mut self, columns: SurfaceColumns) -> Self {
        self.surface = columns.surface;
        self.surface_rq_nm = columns.rq_nm;
        self.surface_lap_deg = columns.lap_deg;
        self.surface_aniso = columns.aniso;
        self.surface_no_sweep = columns.no_sweep;
        self.ground_alpha = columns.ground_alpha;
        self.surface_facets = columns.facets;
        self
    }

    /// The surface columns of this entry, cloned (the reading half of [`Self::with_surface`]).
    #[must_use]
    pub fn surface_columns(&self) -> SurfaceColumns {
        SurfaceColumns {
            surface: self.surface.clone(),
            rq_nm: self.surface_rq_nm,
            lap_deg: self.surface_lap_deg,
            aniso: self.surface_aniso,
            no_sweep: self.surface_no_sweep,
            ground_alpha: self.ground_alpha,
            facets: self.surface_facets.clone(),
        }
    }

    /// Attaches [`Self::frosted`] and [`Self::frosted_facets`] -- see those fields' own doc
    /// comments.
    #[must_use]
    pub fn with_frosting(mut self, frosted: bool, frosted_facets: Vec<f64>) -> Self {
        self.frosted = frosted;
        self.frosted_facets = frosted_facets;
        self
    }

    /// Attaches [`Self::meet_pick`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_meet_pick(mut self, meet_pick: Vec<MeetPickFacetTable>) -> Self {
        self.meet_pick = meet_pick;
        self
    }

    /// Attaches [`Self::angle_relation`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_angle_relation(mut self, angle_relation: Option<String>) -> Self {
        self.angle_relation = angle_relation;
        self
    }

    /// Attaches [`Self::tier_id`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_tier_id(mut self, tier_id: Option<u64>) -> Self {
        self.tier_id = tier_id;
        self
    }

    /// Attaches [`Self::target`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_target(mut self, target: Option<NativeTierTarget>) -> Self {
        self.target = target;
        self
    }

    /// Attaches [`Self::angle_deg`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_angle_deg(mut self, angle_deg: Option<f64>) -> Self {
        self.angle_deg = angle_deg;
        self
    }

    /// Attaches [`Self::indices`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_indices(mut self, indices: Option<Vec<f64>>) -> Self {
        self.indices = indices;
        self
    }

    /// Attaches [`Self::imported_meet`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_imported_meet(mut self, imported_meet: Option<NativeMeetConstraint>) -> Self {
        self.imported_meet = imported_meet;
        self
    }

    /// Attaches [`Self::original_notes`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_original_notes(mut self, original_notes: Option<String>) -> Self {
        self.original_notes = original_notes;
        self
    }

    /// Attaches [`Self::note`] -- see that field's own doc comment.
    #[must_use]
    pub fn with_note(mut self, note: Option<String>) -> Self {
        self.note = note;
        self
    }

    /// Attaches [`Self::cheater_offset_deg`] -- see that field's own doc comment.
    #[must_use]
    pub const fn with_cheater_offset_deg(mut self, cheater_offset_deg: Option<f64>) -> Self {
        self.cheater_offset_deg = cheater_offset_deg;
        self
    }
}
