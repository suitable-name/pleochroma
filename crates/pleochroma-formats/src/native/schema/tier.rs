//! The per-tier tables of the native document: [`TierTable`] (one `[[tiers]]` entry) and
//! [`ConcaveTierTable`] (one concave tier of a self-contained design file). Split from
//! the parent module so the file stays readable; the types are unchanged.

use super::{NativeMeetConstraint, NativeTierTarget};

#[cfg(doc)]
use super::PreformTable;

/// One `[[tiers]]` entry of a design file: a tier in full, in cutting order.
///
/// `angle_deg`/`indices` are optional in the type only so a record written without
/// them parses; the design file's validation refuses a tier that lacks them. `name` is
/// a human-readable label for raw-TOML readers (e.g. `git diff`); the other fields
/// carry what a catalogue `.asc` cannot express (the authored [`NativeMeetConstraint`],
/// which orbit members are detached, and the meet instruction and raw notes text a real
/// `.asc` file's `G` field stated at import time).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TierTable {
    /// Display name.
    pub name: String,
    /// Meet-point constraint.
    pub constraint: NativeMeetConstraint,
    /// Detached parameter values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detached: Vec<f64>,
    /// Mirrors `pleochroma_cut_core::design::ConstraintTier::angle_deg`. `#[serde(default)]`
    /// so a record without it still parses; the design file's validation then refuses
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle_deg: Option<f64>,
    /// Mirrors `pleochroma_cut_core::design::ConstraintTier::indices`. See
    /// [`Self::angle_deg`]'s own doc comment -- same rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indices: Option<Vec<f64>>,
    /// Mirrors `pleochroma_cut_core::design::ConstraintTier::imported_meet`: the meet
    /// instruction a real `.asc` file's `G` field actually stated for this tier at
    /// import time, preserved so a native reload can restore the editor's one-click
    /// "Adopt" action without needing to re-derive it from `.asc` text that (for a
    /// draft save) may not even be trustworthy. `#[serde(default)]` so a file saved
    /// before this field existed still loads, as `None` (nothing to adopt on
    /// reload from such a file, same as a tier `pleochroma_cut_core` never imported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_meet: Option<NativeMeetConstraint>,
    /// Mirrors `pleochroma_cut_core::design::ConstraintTier::original_notes`: the raw
    /// `.asc` `G`-field text this tier's file actually carried at import time,
    /// verbatim. `#[serde(default)]` for the same before-this-field-existed reason as
    /// [`Self::imported_meet`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_notes: Option<String>,
    /// A cutter-authored free-text note for this tier --
    /// unlike [`Self::original_notes`] (read-only imported `.asc` `G`-field text),
    /// this is a new authoring surface with no `.asc` counterpart to round-trip
    /// through, so it lives only here. `#[serde(default)]` so a file saved before
    /// this field existed still loads, as `None` (no note yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A per-tier "cheater"/azimuth-offset annotation, in degrees -- mirrors
    /// `pleochroma_cut_core::design::Design::cheater_offsets_deg`'s own map, keyed
    /// by this tier's array position. Like [`Self::note`],
    /// this is authored, undoable data with no `.asc` counterpart to round-trip
    /// through, so it lives only here. `#[serde(default)]` so a file saved before
    /// this field existed still loads, as `None` (no cheater offset recorded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cheater_offset_deg: Option<f64>,
    /// A stable per-tier identity, mirroring
    /// `pleochroma_cut_core::design::TierId::value` --
    /// `cheater_offset_deg`/`note` above are keyed by ARRAY POSITION
    /// (this whole table already documents that), which renumbers on add/remove/
    /// move; a `TierId` does not. `#[serde(default)]` so a file saved before this
    /// field existed still loads, as `None` -- the loader is expected to assign a
    /// fresh id to every such tier on load (old files have no stable identity to
    /// recover, only a fresh one to start from).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_id: Option<u64>,
    /// This tier's authoring-level target, if any -- see [`NativeTierTarget`].
    /// `#[serde(default)]` so a file saved before this field existed still loads,
    /// as `None` (no target authored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<NativeTierTarget>,
    /// The relation driving this tier's angle, in canonical text (`@3 - 2`: tiers by
    /// stable id, so a rename never breaks it) -- mirrors
    /// `pleochroma_cut_core::design::Design::tier_relations` one-to-one, and this
    /// crate does not interpret the text. `angle_deg` still holds the angle the
    /// relation gives. `#[serde(default, skip_serializing_if = ...)]` so a file with
    /// no relation never grows the key (its bytes do not change) and a file saved
    /// before relations existed loads as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle_relation: Option<String>,
    /// Whether every facet of this tier is frosted (a diffusely scattering finish instead of
    /// a polished one) -- mirrors `pleochroma_cut_core::design::TierFrosting::Tier`.
    /// `#[serde(default, skip_serializing_if = ...)]` so a polished tier never grows the
    /// key (a design without frosting keeps its bytes) and a file saved before frosting
    /// could be authored loads as polished. No version bump: a build that predates the key
    /// parks it in [`Self::unknown`] and writes it back unchanged, exactly as it does for
    /// [`ConcaveTierTable::concave_tier_id`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub frosted: bool,
    /// The index-wheel positions of the frosted facets of a tier that is frosted only in
    /// part -- mirrors `pleochroma_cut_core::design::TierFrosting::Facets`. Empty when the
    /// tier is polished or [`Self::frosted`] as a whole (then not written). Same serde and
    /// version rules as [`Self::frosted`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frosted_facets: Vec<f64>,
    /// The surface finish of the tier: `"Mirror"`, `"Fine"`, `"Commercial"`, `"Prepolish"`,
    /// `"Ground (Fine)"`, `"Ground (Coarse)"` or `"Custom"`. Absent for an ideal (perfectly
    /// polished) tier. Mirrors `pleochroma_cut_core::design::TierSurface`; the key set
    /// ([`Self::surface_rq_nm`], [`Self::surface_lap_deg`], [`Self::surface_aniso`],
    /// [`Self::surface_no_sweep`], [`Self::ground_alpha`], [`Self::surface_facets`]) follows the
    /// frosting precedent: each key is skipped at its default, so a design without a surface
    /// finish keeps its bytes, and there is no version bump (a build that predates the keys parks
    /// them in [`Self::unknown`] and writes them back unchanged).
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
    /// See [`PreformTable::unknown`]'s doc comment -- the same rule, per tier.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
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

/// One concave (fantasy-cut) tier of a self-contained design file: the two-line
/// faceting-diagram standard's facet line plus its tool line, stored in the
/// authored form so nothing is lost (plan §6.1).
///
/// A separate table from [`TierTable`] on purpose: a concave tier has no meet
/// constraint, mast or target, and keeping it apart means a build that predates
/// concave tiers cannot mistake one for a flat tier (it refuses the file by
/// version instead). Tool and motion are the standard's own strings so the file
/// reads like the diagram: `tool` is `"CYL"`, `"CON"`, `"CIR"`, `"DSC"` or
/// `"SPH"`, `motion` is `"reciprocating"` or `"plunge"`. This crate does not
/// interpret them; the loader in `pleochroma-cut-core` does and refuses a
/// string it does not know. `tool`, `diameter_ratio` and `motion` have no serde
/// default, so a record missing one is an error rather than a guessed tool.
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
    /// A stable per-tier identity, the concave counterpart of [`TierTable::tier_id`]: the
    /// cutting-mode marks of a concave step hang on it, so it must survive a save and a
    /// reopen (the position in this list does not: tiers get reordered). Drawn from the
    /// same counter as the flat ids, so a value never repeats within a file.
    /// `#[serde(default, skip_serializing_if = ...)]` so a file saved before this key
    /// existed still loads, as `None` (the loader hands such a tier a fresh id), and a
    /// record without an id never grows the key. No version bump is needed: a build that
    /// predates the key parks it in [`Self::unknown`] and writes it back unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concave_tier_id: Option<u64>,
    /// See [`PreformTable::unknown`]'s doc comment -- the same rule, per concave tier.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

impl TierTable {
    /// Builds a fresh entry with no unknown/future fields, and no `angle_deg`/
    /// `indices`/`imported_meet`/`original_notes`/`cheater_offset_deg` (all `None`)
    /// carried over -- see [`PreformTable::new`]'s own doc comment for why this
    /// lives here rather than in `pleochroma-cut-core`. A caller with one of those
    /// five to attach uses the matching `with_*` method afterward.
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
