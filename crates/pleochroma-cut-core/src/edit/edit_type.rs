//! [`Edit`] itself (every reversible change a caller can describe against a
//! [`crate::design::Design`]) and [`EditError`], the one way applying one can fail.
//!
//! See the parent module's doc comment for the command/inverse model this whole crate's undo/redo
//! rests on.

use super::schedule_state::ScheduleState;
use crate::{
    crystal_axis::CrystalAxisPlan,
    design::{
        ConcaveTier, ConstraintTier, Design, StoneRenderEffects, TierFrosting, TierId,
        TierMeetPick, TierRelation, TierSurface, TierTarget, compute_tier_labels,
        is_legacy_123_abc,
    },
    material::MaterialSelection,
    native::RiRangeChoice,
    preform::PreformSpec,
};
use pleochroma::geometry::meet_solver::MeetConstraint;

mod frosting_label;
mod meet_pick_label;
mod relation_labels;
mod retarget_labels;
mod surface_label;

/// One reversible change to a [`crate::design::Design`].
///
/// Every variant names a tier by its position in `design.tiers` (`0`-based, file
/// order). A position is only meaningful against the schedule state the edit was
/// constructed against -- exactly like a text editor's line numbers -- so
/// [`crate::design::Design::apply_edit`] validates it fresh on every call rather than
/// trusting a stale index, and [`super::History::undo`]/[`super::History::redo`] only
/// ever replay an [`Edit`] this same module already validated and inverted once,
/// against the [`crate::design::Design`] that produced it.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Inserts `tier` at position `index` (`index == design.tiers.len()` appends).
    AddTier { index: usize, tier: ConstraintTier },
    /// Removes the tier currently at position `index`.
    RemoveTier { index: usize },
    /// Moves the tier at position `from` to position `to`.
    ///
    /// Tiers between the two positions shift by one slot. This is `Vec::remove(from)` then
    /// `Vec::insert(to)`, so `to` is the moved tier's position after the move.
    ///
    /// Only positions change, never names: [`MeetConstraint::MeetNamed`] refers to tiers
    /// by name, and [`ConstraintTier::detached`] travels with its tier. The position-keyed
    /// maps (cheater offsets, tier notes) are renumbered to match.
    MoveTier { from: usize, to: usize },
    /// Replaces the tier at position `index` wholesale with `tier` -- a caller
    /// wanting to change just one field reads the current tier first (e.g.
    /// `design.tiers[index].clone()`), mutates the clone, and passes that back.
    ModifyTier { index: usize, tier: ConstraintTier },
    /// Replaces the tier at position `index`'s [`MeetConstraint`] alone, leaving
    /// angle/name/indices untouched -- what "choose what this facet meets" in the
    /// editor UI applies.
    SetConstraint {
        index: usize,
        constraint: MeetConstraint,
    },
    /// Replaces the tier at `index`'s index-wheel positions and detached set
    /// together, leaving angle/name/constraint untouched -- what
    /// `crate::orbit`'s orbit-membership helpers build. Its own variant rather than
    /// reusing `ModifyTier` since membership changes only ever touch
    /// `indices`/`detached`.
    SetIndices {
        index: usize,
        indices: Vec<f64>,
        detached: Vec<f64>,
    },
    /// Replaces the design's preform wholesale.
    SetPreform { preform: PreformSpec },
    /// Replaces [`crate::design::Design::preform_y_offset`], the preform's vertical shift.
    ///
    /// Its own variant because `PreformSpec` does not carry the offset, the same reason
    /// `SetGirdleDiameterMm` has one.
    SetPreformYOffset { y_offset: f64 },
    /// Replaces the design's real-world girdle diameter (millimetres) -- see
    /// [`crate::design::Design::girdle_diameter_mm`]. `None` clears the scale
    /// anchor entirely (back to "not anchored yet").
    SetGirdleDiameterMm { girdle_diameter_mm: Option<f64> },
    /// Replaces the design's material selection wholesale -- see
    /// [`crate::material::MaterialSelection`]. Wholesale, not per-field, matching
    /// `ModifyTier`'s own reasoning. The RI override rides inside
    /// `MaterialSelection` itself -- no separate variant.
    SetMaterial { material: Box<MaterialSelection> },
    /// Replaces the design's per-design stone render effects wholesale -- see
    /// [`crate::design::StoneRenderEffects`]. Mast-inert (no tier input changes), so
    /// [`crate::resolve::resolve_after_edit`] never re-solves for it.
    SetRenderEffects { effects: StoneRenderEffects },
    /// Replaces the orientation rule the design's stone was planned under (the Rough Planner's
    /// hand-off) -- see [`crate::crystal_axis::CrystalAxisPlan`]. `None` (or a blank record)
    /// clears it. Mast-inert, like [`Edit::SetRenderEffects`].
    SetCrystalAxisPlan { plan: Option<CrystalAxisPlan> },
    /// Replaces who states the refractive-index range the cutting sheet prints -- see
    /// [`crate::design::Design::ri_range_choice`]. Mast-inert and plane-inert, like
    /// [`Edit::SetRenderEffects`], so [`crate::resolve::resolve_after_edit`] never re-solves for
    /// it. Inverse: the same variant holding the previous choice.
    SetRiRangeChoice { choice: RiRangeChoice },
    /// Replaces the design's header fields and gear reference angle.
    ///
    /// The fields are [`crate::design::ScheduleMeta::headers`] (the `.asc` `H` lines; the
    /// first is the title the app shows), [`crate::design::ScheduleMeta::footnotes`] and
    /// [`crate::design::ScheduleMeta::gear_reference_angle`] (the index wheel's zero-tooth
    /// offset). Its own variant, not part of `SetSchedule`, which always forces a full
    /// re-solve. None of these changes a solved mast, so
    /// [`crate::resolve::resolve_after_edit`] never re-solves for it.
    ///
    /// **Not geometry-inert**: `gear_reference_angle` rotates every solved plane's azimuth
    /// after the solve (`pleochroma::geometry::cuts`). For a non-symmetric preform such as a
    /// block, the mesh, yield and manufacturability warnings change even though no mast moved
    /// (see [`crate::design::Design::planes_from_solved`]).
    SetMeta {
        headers: Vec<String>,
        footnotes: Vec<String>,
        gear_reference_angle: f64,
    },
    /// Sets (or, when `offset_deg` is `None`, clears) the cheater/azimuth offset of the tier
    /// at `index`.
    ///
    /// The offset is keyed by position on `Design` (see
    /// [`crate::design::Design::cheater_offsets_deg`]), so it replaces one entry in place.
    SetCheaterOffset {
        index: usize,
        offset_deg: Option<f64>,
    },
    /// Sets (or, when `note` is `None`, clears) the free-text note of the tier at `index`.
    ///
    /// Keyed by position (see [`crate::design::Design::tier_notes`]), like `SetCheaterOffset`.
    SetTierNote { index: usize, note: Option<String> },
    /// Sets (or, when `target` is `None`, clears) the [`TierTarget`] of the tier at `index`.
    /// Keyed by [`crate::design::TierId`] (see [`crate::design::Design::tier_targets`]).
    SetTierTarget {
        index: usize,
        target: Option<TierTarget>,
    },
    /// Sets (or, when `relation` is `None`, clears) the [`TierRelation`] driving the
    /// angle of the tier currently at `index` -- see
    /// [`crate::design::Design::tier_relations`]. Keyed by [`TierId`] under the
    /// hood, like `SetTierTarget`. Inverse: the same variant holding the previous
    /// relation.
    ///
    /// This edit changes ONLY the relation. It does not move the angle: the tier
    /// keeps whatever angle it has until the relations are evaluated, which an
    /// `pleochroma_editor::session::EditorSession` does in the SAME undo step by
    /// batching this edit with an [`Edit::RetargetAngles`]. A bare `Design` that
    /// applies it alone can call [`crate::design::Design::evaluate_relations`]
    /// itself. [`crate::resolve::resolve_after_edit`] treats it like an angle edit
    /// of the driven tier.
    SetTierRelation {
        index: usize,
        relation: Option<TierRelation>,
    },
    /// Sets (or, when `frosting` is `None`, clears) how the tier currently at `index` is
    /// frosted -- see [`crate::design::Design::tier_frosting`] and [`TierFrosting`]. Keyed by
    /// [`TierId`] under the hood, like `SetTierTarget`; the frosting is normalised first
    /// (facet positions ascending, a frosting of no facet is a clear). Changes no geometry.
    /// Inverse: the same variant holding the previous frosting.
    SetTierFrosting {
        index: usize,
        frosting: Option<TierFrosting>,
    },
    /// Sets (or, when `surface` is `None`, clears) the surface finish of the tier currently at
    /// `index` -- see [`crate::design::Design::tier_surfaces`] and [`TierSurface`]. Keyed by
    /// [`TierId`] under the hood, like `SetTierFrosting`; the surface is normalised first (a
    /// surface that states nothing is a clear). Changes no geometry. Inverse: the same variant
    /// holding the previous surface.
    SetTierSurface {
        index: usize,
        surface: Option<TierSurface>,
    },
    /// Sets (or, when `pick` is `None`, clears) the corner the tier currently at `index` is cut
    /// to -- see [`crate::design::Design::tier_meet_picks`] and [`TierMeetPick`]. Keyed by
    /// [`TierId`] under the hood, like `SetTierTarget`. Changes only the pick map: the tier's
    /// constraint and target are separate edits, which
    /// [`crate::design::Design::meet_pick_edit`] batches with this one. A pick changes the
    /// solved mast, so [`crate::resolve::resolve_after_edit`] re-solves in full. Inverse: the
    /// same variant holding the previous pick.
    SetTierMeetPick {
        index: usize,
        pick: Option<TierMeetPick>,
    },
    /// Restores the removed tier's [`crate::design::TierId`] on undo.
    ///
    /// Never constructed by a caller directly. A plain [`Edit::AddTier`] allocates a fresh
    /// id, so the undo of [`Edit::RemoveTier`] needs this step. It sets [`Design::tier_ids`]
    /// at `index` to `id`; its inverse is the same variant holding the previous id.
    RestoreTierId { index: usize, id: TierId },
    /// Replaces the index-gear tooth count, symmetry order and mirror flag.
    ///
    /// A tier's own `indices`/`detached` stay numerically unchanged; [`Edit::RemapIndices`]
    /// re-derives them for a new gear.
    ///
    /// [`Design::apply_edit`] rejects `gear_teeth == 0` or `symmetry_order == 0` with an
    /// [`EditError`] (without modifying `self`), since either makes index positions
    /// meaningless and writes a `g`/`y` line that [`pleochroma_formats::asc::parse_asc`]
    /// cannot read back. It also rejects a smaller gear while a concave tier holds an index
    /// past the new wheel (the error names that tier). Run [`Edit::RemapIndices`] first, or
    /// put both in one [`Edit::Batch`] (only the batch's end state is checked), as the
    /// editor's gear change does.
    SetSchedule {
        gear_teeth: i32,
        symmetry_order: u32,
        mirror: bool,
    },
    /// Re-derives every tier's `indices`/`detached` from `from_gear` teeth to
    /// `to_gear` teeth (`new = round(old * to_gear / from_gear)`, per `rounding` --
    /// see [`RemapRounding`]) -- what the editor's gear-remap dialog applies. Lossy
    /// whenever the ratio isn't integral (e.g. 96 -> 80): rather than a lossy
    /// reverse remap on undo, applying this records every tier's REAL previous
    /// `indices`/`detached` and returns [`Edit::RestoreIndices`] as the exact
    /// inverse.
    RemapIndices {
        from_gear: i32,
        to_gear: i32,
        rounding: RemapRounding,
    },
    /// Never constructed by a caller directly -- the exact inverse
    /// [`Edit::RemapIndices`] produces. Restores `indices`/`detached` verbatim for
    /// every tier named in `tiers` (`(index, indices, detached)`), in one atomic
    /// undo step. Public like every other [`Edit`] variant, since
    /// [`super::History`]'s undo/redo stacks hold plain, inspectable [`Edit`] values.
    RestoreIndices {
        tiers: Vec<(usize, Vec<f64>, Vec<f64>)>,
    },
    /// One undoable step retargeting several tiers' angles at once -- what the
    /// "Retarget for material" proposal applies after review, so undo reverts the
    /// whole retarget in one step rather than one tier at a time. Each tuple is
    /// `(index, old_deg, new_deg)`; `old_deg` is caller-facing/diagnostic only --
    /// applying this always sets `angle_deg` to `new_deg` and computes the real
    /// inverse from the tier's own actual previous value.
    RetargetAngles { changes: Vec<(usize, f64, f64)> },
    /// Inserts a concave tier at position `index` of
    /// [`Design::concave_tiers`] (`index == len` appends). Rejected, without
    /// mutation, when the tier fails [`ConcaveTier::validate`] or reuses a flat
    /// tier's name. Inverse: [`Edit::RemoveConcaveTier`].
    AddConcaveTier { index: usize, tier: ConcaveTier },
    /// Removes the concave tier at `index`. Inverse: an [`Edit::Batch`] of
    /// [`Edit::AddConcaveTier`] and [`Edit::RestoreConcaveTierId`], so the same
    /// [`TierId`] comes back (see [`Edit::RestoreTierId`] for why).
    RemoveConcaveTier { index: usize },
    /// Replaces the concave tier at `index` wholesale, validated like
    /// [`Edit::AddConcaveTier`]. Inverse: the same variant holding the previous
    /// tier.
    ModifyConcaveTier { index: usize, tier: ConcaveTier },
    /// Moves a concave tier from `from` to `to`, with [`Edit::MoveTier`]'s
    /// remove-then-insert meaning. The order matters: it is the cutting order
    /// inside a concave group. Inverse: the positions swapped.
    MoveConcaveTier { from: usize, to: usize },
    /// Never constructed by a caller directly -- the concave twin of
    /// [`Edit::RestoreTierId`], needed because that one writes `tier_ids`.
    RestoreConcaveTierId { index: usize, id: TierId },
    /// Never constructed by a caller directly -- the concave half of the inverse
    /// [`Edit::RemapIndices`] produces. Restores each named concave tier's
    /// `indices` verbatim, as `(index, indices)`.
    RestoreConcaveIndices { tiers: Vec<(usize, Vec<f64>)> },
    /// Several sub-edits as ONE undo step.
    ///
    /// [`Design::apply_edit`] validates them all on a scratch clone before writing back, so a
    /// sub-edit that fails partway leaves the design unchanged. The undo is the sub-edits' own
    /// inverses in reverse order. Used wherever several edits are one user action, e.g.
    /// "Retarget for material" (angles plus the material change).
    Batch(Vec<Self>),
    /// Swaps the design's whole flat tier list and schedule metadata for the held
    /// [`ScheduleState`] in one step -- what applying edited `.asc` text uses, where
    /// any number of tiers may have been added, removed, renamed or changed at once.
    /// The preform, girdle size, material and concave tiers are not touched.
    /// Inverse: the same variant holding the state this edit replaced.
    ///
    /// Rejected, without changing the design, when the state is not usable (see
    /// [`Design::apply_edit`]'s `ReplaceSchedule` arm and `ScheduleState`), and when the
    /// state's gear is smaller than the design's and a concave tier holds an index past
    /// the smaller wheel (the same rule as [`Edit::SetSchedule`]; a [`Edit::Batch`] is
    /// checked once, on its end state, so a replacement that comes with a remap of the
    /// concave indices applies).
    ReplaceSchedule(Box<ScheduleState>),
}

impl Edit {
    /// A short, cutter-facing summary of what this [`Edit`] does -- e.g. "Set P1 angle
    /// to 41.0 degrees", "Remap gear 96 to 80", "Remove tier C1", "Optimize 12
    /// tiers" -- rather than a debug dump of the variant's fields. What the editor
    /// feeds [`super::History::peek_undo`]/[`super::History::peek_redo`] through to
    /// label the undo/redo hover hints and Edit menu items, so a cutter can tell what
    /// Ctrl+Z/Ctrl+Y will actually do before pressing it.
    ///
    /// `design` supplies tier names for the variants that only carry a tier's
    /// positional `index` (`RemoveTier`/`ModifyTier`/`SetConstraint`/`SetIndices`) --
    /// looked up against `design`'s CURRENT tier list, so this reads correctly when
    /// called against the same design state the `Edit` itself would apply to (exactly
    /// what `History::peek_undo`/`peek_redo` already promise -- see their own doc
    /// comments). A stale/out-of-range index (defensive only -- every real caller
    /// satisfies that promise) falls back to `"tier {index + 1}"`.
    #[must_use]
    pub fn describe(&self, design: &Design) -> String {
        match self {
            Self::AddTier { tier, .. } => format!("Add tier {}", tier_display_name(&tier.name)),
            Self::RemoveTier { index } => format!("Remove tier {}", tier_label_at(design, *index)),
            Self::MoveTier { from, to } => describe_move_tier(*from, *to, design),
            Self::ModifyTier { index, .. } => {
                format!("Modify tier {}", tier_label_at(design, *index))
            }
            Self::SetConstraint { index, .. } => {
                format!("Change meet target for {}", tier_label_at(design, *index))
            }
            Self::SetIndices { index, .. } => {
                format!(
                    "Change index positions for {}",
                    tier_label_at(design, *index)
                )
            }
            Self::SetPreform { .. } => "Change preform".to_string(),
            Self::SetPreformYOffset { y_offset } => {
                format!("Set preform offset to {y_offset:.2}")
            }
            Self::SetGirdleDiameterMm { girdle_diameter_mm } => girdle_diameter_mm.map_or_else(
                || "Clear girdle diameter".to_string(),
                |mm| format!("Set girdle diameter to {mm:.2} mm"),
            ),
            Self::SetMaterial { material } => describe_set_material(material, design),
            Self::SetRenderEffects { .. } => "Change stone render effects".to_string(),
            Self::SetCrystalAxisPlan { .. } => "Change crystal axis plan".to_string(),
            Self::SetRiRangeChoice { .. } => "Change RI range".to_string(),
            Self::SetMeta { .. } => "Change design details".to_string(),
            Self::SetCheaterOffset { index, offset_deg } => {
                let label = tier_label_at(design, *index);
                offset_deg.map_or_else(
                    || format!("Clear cheater offset for {label}"),
                    |deg| format!("Set cheater offset for {label} to {deg:.2} deg"),
                )
            }
            Self::SetTierNote { index, note } => {
                let label = tier_label_at(design, *index);
                note.as_ref().map_or_else(
                    || format!("Clear note for {label}"),
                    |_| format!("Set note for {label}"),
                )
            }
            Self::SetTierTarget { index, target } => {
                let label = tier_label_at(design, *index);
                target.map_or_else(
                    || format!("Clear target for {label}"),
                    |t| format!("Set target for {label} to {}", describe_tier_target(t)),
                )
            }
            Self::SetTierRelation { index, relation } => {
                relation_labels::describe_tier_relation(*index, relation.as_ref(), design)
            }
            Self::SetTierFrosting { index, frosting } => {
                frosting_label::describe_frosting(&tier_label_at(design, *index), frosting.as_ref())
            }
            Self::SetTierSurface { index, surface } => {
                surface_label::describe_surface(&tier_label_at(design, *index), surface.as_ref())
            }
            Self::SetTierMeetPick { index, pick } => {
                meet_pick_label::describe_meet_pick(design, *index, pick.as_ref())
            }
            // Internal bookkeeping only -- see this variant's own doc comment.
            // `describe_batch` filters it out of a `RemoveTier` undo's own
            // description before this arm is ever reached in practice; this text
            // is a defensive fallback, not something a cutter should see.
            Self::RestoreTierId { index, .. } => {
                format!(
                    "Restore tier identity for {}",
                    tier_label_at(design, *index)
                )
            }
            Self::SetSchedule { gear_teeth, .. } => format!("Set gear to {gear_teeth} teeth"),
            Self::RemapIndices {
                from_gear, to_gear, ..
            } => format!("Remap gear {from_gear} to {to_gear}"),
            Self::RestoreIndices { tiers } => {
                format!("Restore index positions for {} tier(s)", tiers.len())
            }
            Self::RetargetAngles { changes } => describe_retarget_angles(changes, design),
            Self::AddConcaveTier { tier, .. } => {
                format!("Add concave tier {}", tier_display_name(&tier.name))
            }
            Self::RemoveConcaveTier { index } => {
                format!("Remove concave tier {}", concave_label_at(design, *index))
            }
            Self::ModifyConcaveTier { index, .. } => {
                format!("Modify concave tier {}", concave_label_at(design, *index))
            }
            Self::MoveConcaveTier { from, to } => describe_move_concave_tier(*from, *to, design),
            // Internal bookkeeping only, like `RestoreTierId`.
            Self::RestoreConcaveTierId { index, .. } => format!(
                "Restore concave tier identity for {}",
                concave_label_at(design, *index)
            ),
            Self::RestoreConcaveIndices { tiers } => {
                format!(
                    "Restore index positions for {} concave tier(s)",
                    tiers.len()
                )
            }
            Self::Batch(edits) => describe_batch(edits, design),
            Self::ReplaceSchedule(_) => "Edit instructions as text".to_string(),
        }
    }
}

/// `name`, or a placeholder for an unnamed tier -- shared by every [`Edit::describe`]
/// arm that has a real [`ConstraintTier`] in hand (as opposed to only its index).
fn tier_display_name(name: &str) -> String {
    if name.is_empty() {
        "(unnamed tier)".to_string()
    } else {
        name.to_string()
    }
}

/// [`Edit::describe`]'s tier-name lookup for the variants that only carry a
/// positional `index` -- see that method's own doc comment.
///
/// What the tier table shows: the tier's own name, or its standard code (`P1`, `C2`, `T`)
/// when the name is empty or old-style (`1`, `A`). A rename is the one sentence that must
/// say the raw names ([`raw_tier_name_at`]), because the name is what changes.
fn tier_label_at(design: &Design, index: usize) -> String {
    let Some(tier) = design.tiers.get(index) else {
        return format!("tier {}", index + 1);
    };
    if (tier.name.trim().is_empty() || is_legacy_123_abc(&tier.name))
        && let Some(info) = compute_tier_labels(&design.tiers).get(index)
    {
        return info.code.clone();
    }
    tier_display_name(&tier.name)
}

/// The tier's name exactly as stored (a placeholder for an unnamed one): what a rename
/// sentence reads, since there the name itself is the thing that changes.
fn raw_tier_name_at(design: &Design, index: usize) -> String {
    design.tiers.get(index).map_or_else(
        || format!("tier {}", index + 1),
        |tier| tier_display_name(&tier.name),
    )
}

/// [`tier_label_at`] for [`Design::concave_tiers`].
fn concave_label_at(design: &Design, index: usize) -> String {
    design.concave_tiers.get(index).map_or_else(
        || format!("concave tier {}", index + 1),
        |tier| tier_display_name(&tier.name),
    )
}

/// [`Edit::describe`]'s [`Edit::MoveTier`] arm: names the moved tier (looked up at
/// `from`, its position in the design this move is about to apply to -- see
/// [`Edit::describe`]'s own doc comment for why that's the correct list to read
/// against) and says which direction it travelled. `to == from` is a no-op move
/// (never constructed by a real caller, but not a panic either).
fn describe_move_tier(from: usize, to: usize, design: &Design) -> String {
    let label = tier_label_at(design, from);
    match to.cmp(&from) {
        std::cmp::Ordering::Less => format!("Move tier {label} up"),
        std::cmp::Ordering::Greater => format!("Move tier {label} down"),
        std::cmp::Ordering::Equal => format!("Move tier {label}"),
    }
}

/// [`Edit::describe`]'s [`Edit::MoveConcaveTier`] arm, like [`describe_move_tier`].
fn describe_move_concave_tier(from: usize, to: usize, design: &Design) -> String {
    let label = concave_label_at(design, from);
    match to.cmp(&from) {
        std::cmp::Ordering::Less => format!("Move concave tier {label} up"),
        std::cmp::Ordering::Greater => format!("Move concave tier {label} down"),
        std::cmp::Ordering::Equal => format!("Move concave tier {label}"),
    }
}

/// [`Edit::describe`]'s [`Edit::SetTierTarget`] arm: a short, unit-labelled
/// description of one [`TierTarget`].
fn describe_tier_target(target: TierTarget) -> String {
    match target {
        TierTarget::DepthMm(mm) => format!("{mm:.2} mm depth"),
        TierTarget::GirdleThicknessMm(mm) => format!("{mm:.2} mm girdle thickness"),
        TierTarget::TableWidthMm(mm) => format!("{mm:.2} mm table width"),
    }
}

/// [`Edit::describe`]'s label for a [`MaterialSelection`] -- its name when set, else a
/// placeholder (an RI-override-only selection has no catalogue name to show), with a
/// color override appended in brackets (`Sapphire (Yellow)`, `Sapphire (Padparadscha)` for a
/// color variant, or `Sapphire (custom color)` for a triple that matches no preset -- see
/// [`MaterialSelection::color_label`]).
fn material_display_name(material: &MaterialSelection) -> String {
    let mut label = material
        .name
        .clone()
        .unwrap_or_else(|| "(custom material)".to_string());
    if let Some(color) = material.color_label() {
        label = format!("{label} ({color})");
    }
    label
}

/// [`Edit::describe`]'s [`Edit::SetMaterial`] arm. A change of only the stone's color pattern
/// leaves the name, the overrides and the colors as they were, so the plain label would read
/// "Set material to Ametrine" for a step that did not touch the material; it says so instead.
fn describe_set_material(material: &MaterialSelection, design: &Design) -> String {
    let label = format!("Set material to {}", material_display_name(material));
    let pattern_only = material.color_pattern_override != design.material.color_pattern_override
        && material.clone().with_color_pattern(None)
            == design.material.clone().with_color_pattern(None);
    if pattern_only {
        format!("{label} (Color Pattern)")
    } else {
        label
    }
}

/// [`Edit::describe`]'s [`Edit::RetargetAngles`] arm: a single-tier retarget names
/// that tier and its new angle (the "Set P1 angle to 41.0 degrees" example); several
/// at once is always an Optimize/batch result in practice, so it reads as "Optimize N
/// tiers" instead of an unreadable per-tier list.
///
/// The angle is a magnitude: a pavilion tier stored at -41 reads `41.0`, because the side
/// of the girdle is the tier's, never a sign a person has to read.
fn describe_retarget_angles(changes: &[(usize, f64, f64)], design: &Design) -> String {
    match changes {
        [] => "Retarget angles".to_string(),
        [(index, _, new_deg)] => {
            format!(
                "Set {} angle to {:.1} degrees",
                tier_label_at(design, *index),
                new_deg.abs()
            )
        }
        many => format!("Optimize {} tiers", many.len()),
    }
}

/// [`Edit::describe`]'s [`Edit::Batch`] arm. Special-cases the one composite this
/// crate builds today (a retarget's angle changes plus the material change it
/// implies, `callbacks::retarget_actions::setup_retarget_apply_callback`) so it reads
/// as one clear sentence rather than "2 combined edits"; anything else falls back to
/// delegating a single-edit batch, or a generic count otherwise.
fn describe_batch(edits: &[Edit], design: &Design) -> String {
    if let [Edit::RetargetAngles { .. }, Edit::SetMaterial { material }]
    | [Edit::SetMaterial { material }, Edit::RetargetAngles { .. }] = edits
    {
        return format!("Retarget for {}", material_display_name(material));
    }
    if let Some(label) = retarget_labels::describe_anchored_retarget(edits) {
        return label;
    }
    if let Some(label) = retarget_labels::describe_modified_tiers_retarget(edits, design) {
        return label;
    }
    if let Some(label) = relation_labels::describe_relation_batch(edits, design) {
        return label;
    }
    if let Some(label) = meet_pick_label::describe_meet_pick_batch(edits, design) {
        return label;
    }
    // `RestoreTierId` is `Design::apply_edit`'s own invisible bookkeeping step
    // (see that variant's doc comment) -- a `RemoveTier` undo that otherwise
    // collapses to one `AddTier` should still read as "Add tier X", not "2
    // combined edits", just because it also carries this step along.
    let visible: Vec<&Edit> = edits
        .iter()
        .filter(|edit| {
            !matches!(
                edit,
                Edit::RestoreTierId { .. } | Edit::RestoreConcaveTierId { .. }
            )
        })
        .collect();
    match visible.as_slice() {
        [] => "No-op".to_string(),
        [only] => only.describe(design),
        many => format!("{} combined edits", many.len()),
    }
}

/// How [`Edit::RemapIndices`] rounds a non-integral index-wheel position after
/// converting it from `from_gear` teeth to `to_gear` teeth.
///
/// See that variant's own doc comment. This crate never picks a rounding mode itself;
/// the caller always supplies one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemapRounding {
    /// Rounds to the nearest whole tooth (`f64::round`, ties away from zero).
    Nearest,
    /// Rounds down (`f64::floor`).
    Floor,
    /// Rounds up (`f64::ceil`).
    Ceil,
}

/// Why an [`Edit`] could not be applied.
///
/// Almost always a position out of range for the design's current tier list.
/// The one exception: [`Design::apply_edit`]'s own [`Edit::SetSchedule`] arm
/// also returns this (with `index: 0`) for a zero gear-tooth count or symmetry
/// order, a schedule-wide validation failure that names no tier at all --
/// reusing this type's only shape rather than widening this crate's one
/// edit-error type for that single caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditError {
    /// Tier position this refers to.
    pub index: usize,
    /// Number of tiers the design had when the edit was rejected.
    pub tier_count: usize,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tier index {} out of range (schedule has {} tier(s))",
            self.index, self.tier_count
        )
    }
}

impl std::error::Error for EditError {}
