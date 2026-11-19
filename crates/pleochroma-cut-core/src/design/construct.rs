//! [`Design`]'s constructors.
//!
//! See the parent module's doc comment for the constraints-not-masts model they build against,
//! and [`Design::from_asc_schedule`]'s doc comment for the "pin every tier" import policy.

use super::{ConstraintTier, Design, ScheduleMeta};
use crate::{material::MaterialSelection, native::RiRangeChoice, preform::PreformSpec};
use pleochroma::geometry::meet_solver::{MeetConstraint, meet_tier_inputs_from_asc};
use pleochroma_formats::asc::AscSchedule;
use std::collections::BTreeMap;

/// Everything [`Design::fresh_from_spec`] needs to build a brand-new design.
///
/// That is the index-gear, symmetry and mirror settings a real `.asc` file would open with, the
/// starting material selection, and the preform to cut it from. The editor's "New" dialog fills one
/// of these directly. [`Design::fresh`] takes positional parameters and cannot set a material.
#[derive(Debug, Clone, PartialEq)]
pub struct FreshDesignSpec {
    /// Number of teeth on the index gear.
    pub gear_teeth: i32,
    /// Rotational symmetry order of the schedule.
    pub symmetry_order: u32,
    /// Whether the schedule carries mirror symmetry.
    pub mirror: bool,
    /// Gem material the design is cut from.
    pub material: MaterialSelection,
    /// Rough (preform) the design is cut from.
    pub preform: PreformSpec,
}

impl Design {
    /// The placeholder [`ScheduleMeta::refractive_index`] of a fresh design: `1.54`. Read by
    /// [`Design::effective_refractive_index`] only once a design has neither an RI override nor
    /// a material name that resolves.
    ///
    /// Public so a material pick can tell this placeholder (no recorded figure) from an `I`
    /// line a design really carries.
    pub const LEGACY_DEFAULT_REFRACTIVE_INDEX: f64 = 1.54;

    /// Pairs an existing preform, schedule metadata and constraint-tier list, with no
    /// scale anchor or material selection yet. A caller that wants those set from the
    /// start uses a struct-update literal or applies
    /// `Edit::SetGirdleDiameterMm`/`Edit::SetMaterial` afterward through `History`.
    #[must_use]
    pub fn new(preform: PreformSpec, meta: ScheduleMeta, tiers: Vec<ConstraintTier>) -> Self {
        let tier_ids: Vec<super::TierId> = (0..tiers.len() as u64).map(super::TierId).collect();
        let next_tier_id = tier_ids.len() as u64;
        Self {
            preform,
            meta,
            tiers,
            girdle_diameter_mm: None,
            preform_y_offset: 0.0,
            cheater_offsets_deg: BTreeMap::new(),
            tier_notes: BTreeMap::new(),
            material: MaterialSelection::none(),
            tier_ids,
            next_tier_id,
            tier_targets: BTreeMap::new(),
            concave_tiers: Vec::new(),
            concave_tier_ids: Vec::new(),
            tier_relations: BTreeMap::new(),
            tier_frosting: BTreeMap::new(),
            tier_surfaces: BTreeMap::new(),
            tier_meet_picks: BTreeMap::new(),
            render_effects: super::render_effects::StoneRenderEffects::default(),
            crystal_axis_plan: None,
            ri_range_choice: RiRangeChoice::Off,
            file_extras: None,
        }
    }

    /// A brand-new design from a full [`FreshDesignSpec`]: gear, symmetry, mirror and starting
    /// material are all set up front. There are no tiers yet. The schedule's
    /// [`ScheduleMeta::refractive_index`] starts at [`Self::LEGACY_DEFAULT_REFRACTIVE_INDEX`],
    /// which [`Design::effective_refractive_index`] reads only as a fallback, since a resolved
    /// material's `n_D` takes precedence.
    #[must_use]
    pub fn fresh_from_spec(spec: FreshDesignSpec) -> Self {
        Self {
            preform: spec.preform,
            meta: ScheduleMeta {
                gemcad_version: "5.0".to_string(),
                gear_teeth: spec.gear_teeth,
                symmetry_order: spec.symmetry_order,
                mirror: spec.mirror,
                refractive_index: Self::LEGACY_DEFAULT_REFRACTIVE_INDEX,
                ..ScheduleMeta::default()
            },
            tiers: Vec::new(),
            girdle_diameter_mm: None,
            preform_y_offset: 0.0,
            cheater_offsets_deg: BTreeMap::new(),
            tier_notes: BTreeMap::new(),
            material: spec.material,
            tier_ids: Vec::new(),
            next_tier_id: 0,
            tier_targets: BTreeMap::new(),
            concave_tiers: Vec::new(),
            concave_tier_ids: Vec::new(),
            tier_relations: BTreeMap::new(),
            tier_frosting: BTreeMap::new(),
            tier_surfaces: BTreeMap::new(),
            tier_meet_picks: BTreeMap::new(),
            render_effects: super::render_effects::StoneRenderEffects::default(),
            crystal_axis_plan: None,
            ri_range_choice: RiRangeChoice::Off,
            file_extras: None,
        }
    }

    /// A brand-new design: `preform`, and an empty schedule carrying only the
    /// index-gear/symmetry/refractive-index settings a real `.asc` file would open
    /// with. No tiers yet -- see [`Design::planes`] and `fresh_design_alone_is_closed`
    /// for why that's still a fully renderable solid, not an empty viewport.
    ///
    /// A thin wrapper over [`Design::fresh_from_spec`] (`mirror` always `true`, no
    /// material selection) kept for source compatibility; call
    /// [`Design::fresh_from_spec`] directly to set either.
    #[must_use]
    pub fn fresh(
        preform: PreformSpec,
        gear_teeth: i32,
        symmetry_order: u32,
        refractive_index: f64,
    ) -> Self {
        let mut design = Self::fresh_from_spec(FreshDesignSpec {
            gear_teeth,
            symmetry_order,
            mirror: true,
            material: MaterialSelection::none(),
            preform,
        });
        design.meta.refractive_index = refractive_index;
        design
    }

    /// Builds a [`Design`] from a real, parsed `.asc` schedule by **pinning every
    /// tier's mast exactly**: each tier's [`MeetConstraint`] becomes
    /// [`MeetConstraint::ScaleReference`] at that tier's own real recorded
    /// `mast`, full stop -- regardless of what the file's `G` field said there.
    /// Re-solving a design built this way (`design.solve()`) therefore
    /// reproduces every original mast **exactly** (to float rounding, not a
    /// tolerance): opening a catalogue design in the editor must show the same
    /// stone the file records, not a solver's reconstruction of it.
    ///
    /// # Why every tier is pinned
    ///
    /// Re-deriving every mast from its meet constraints does not reliably reproduce the recorded
    /// geometry: about 11 % of the catalogue lands more than 10 % off its recorded masts (see
    /// `discard_and_resolve_gate_on_real_fixtures`). So adopting a meet is a deliberate,
    /// per-tier, undoable act. [`ConstraintTier::imported_meet`] keeps the file's own meet
    /// instruction beside the pinned constraint, and the editor switches a tier over to it with
    /// one edit (`Edit::SetConstraint`). A tier with an explicit scale-reference instruction has
    /// nothing to adopt, so `imported_meet` is `None` there.
    ///
    /// [`ConstraintTier::original_notes`] separately preserves the file's raw `G`
    /// text verbatim (whatever it said, not just the three instructions this crate
    /// classifies), so exporting an untouched design writes back the same notes a
    /// human reads while cutting rather than a synthesized stand-in.
    #[must_use]
    pub fn from_asc_schedule(preform: PreformSpec, schedule: &AscSchedule) -> Self {
        let inputs = meet_tier_inputs_from_asc(schedule);

        let tiers = inputs
            .into_iter()
            .zip(&schedule.tiers)
            .map(|(input, original)| {
                // Anything that isn't already a stated scale reference is worth
                // keeping around for one-click adoption; a stated `ScaleReference`
                // has nothing to adopt.
                let imported_meet = match input.constraint {
                    MeetConstraint::ScaleReference(_) => None,
                    other => Some(other),
                };
                ConstraintTier {
                    angle_deg: input.angle_deg,
                    name: original.name.clone(),
                    indices: input.indices,
                    constraint: MeetConstraint::ScaleReference(original.mast),
                    imported_meet,
                    // Kept verbatim, even when empty: export needs it to undo the flattening
                    // that pinning every tier to `ScaleReference` applies to `G` text (see
                    // `ConstraintTier::original_notes`).
                    original_notes: Some(original.notes.clone()),
                    detached: Vec::new(),
                }
            })
            .collect::<Vec<ConstraintTier>>();
        let tier_ids: Vec<super::TierId> = (0..tiers.len() as u64).map(super::TierId).collect();
        let next_tier_id = tier_ids.len() as u64;

        Self {
            preform,
            meta: ScheduleMeta {
                gemcad_version: schedule.gemcad_version.clone(),
                gear_teeth: schedule.gear_teeth,
                gear_reference_angle: schedule.gear_reference_angle,
                symmetry_order: schedule.symmetry_order,
                mirror: schedule.mirror,
                refractive_index: schedule.refractive_index,
                headers: schedule.headers.clone(),
                footnotes: schedule.footnotes.clone(),
            },
            tiers,
            // `.asc` has no field for either; see `Self::girdle_diameter_mm`.
            girdle_diameter_mm: None,
            preform_y_offset: 0.0,
            cheater_offsets_deg: BTreeMap::new(),
            tier_notes: BTreeMap::new(),
            material: MaterialSelection::none(),
            // A fresh id per imported tier: the `.asc` format has no stable tier identity
            // (see `TierId`).
            tier_ids,
            next_tier_id,
            tier_targets: BTreeMap::new(),
            concave_tiers: Vec::new(),
            concave_tier_ids: Vec::new(),
            tier_relations: BTreeMap::new(),
            // `.asc` has no notation for frosting: an imported schedule is all polished.
            // (A `.gcs` import applies its frosting afterwards, see
            // `Design::apply_imported_frosting`.)
            tier_frosting: BTreeMap::new(),
            tier_surfaces: BTreeMap::new(),
            tier_meet_picks: BTreeMap::new(),
            render_effects: super::render_effects::StoneRenderEffects::default(),
            crystal_axis_plan: None,
            ri_range_choice: RiRangeChoice::Off,
            file_extras: None,
        }
    }
}
