//! Restores per-tier state from a design file's tier tables onto a fresh [`Design`].
//!
//! The helpers cover notes, cheater offsets, ids, targets, relations and concave tiers. The
//! design file loader ([`crate::native::design_from_file`]) uses them.

use super::MaterialResolution;
use crate::{
    design::{ConstraintTier, Design, RelationError, TierFrosting, TierId, TierRelation},
    native::convert::{
        concave_tier_from_table, meet_constraint_from_native, tier_target_from_native,
    },
};
use pleochroma_formats::native::{ConcaveTierTable, NativeTierTarget, TierTable};
use std::collections::{BTreeMap, BTreeSet};

/// The [`MaterialResolution`] of `name`, by exact built-in name (see that type for the
/// heuristic).
///
/// This does not use [`pleochroma::optics::materials::GemMaterial::by_name`], whose substring
/// fallback would report "My Blue Sapphire" as `Known`. The `.asc`'s `I` line would then take
/// Sapphire's refractive index.
pub(in crate::native) fn material_resolution_of(name: Option<&str>) -> MaterialResolution {
    match name {
        None => MaterialResolution::NoneSelected,
        Some(name) if crate::material::built_in_material_by_exact_name(name).is_some() => {
            MaterialResolution::Known
        }
        Some(_) => MaterialResolution::Unresolved,
    }
}

/// Collects [`Design::tier_notes`] from a file's `tiers` array, keyed by array
/// position. Takes a reference (not ownership) since the caller still needs to move
/// the tier tables themselves into the tier list afterward.
pub(in crate::native) fn tier_notes_from_native(
    native_tiers: &[TierTable],
) -> BTreeMap<usize, String> {
    native_tiers
        .iter()
        .enumerate()
        .filter_map(|(index, saved)| saved.note.clone().map(|note| (index, note)))
        .collect()
}

/// Collects [`Design::cheater_offsets_deg`] from a file's `tiers` array, keyed by
/// array position. Exactly [`tier_notes_from_native`]'s own shape, one field over.
pub(in crate::native) fn cheater_offsets_from_native(
    native_tiers: &[TierTable],
) -> BTreeMap<usize, f64> {
    native_tiers
        .iter()
        .enumerate()
        .filter_map(|(index, saved)| saved.cheater_offset_deg.map(|offset| (index, offset)))
        .collect()
}

/// Every tier's saved [`TierTable::tier_id`], in order. `None` for a tier with no id, such as
/// one from a file that predates the field. Non-consuming, so a caller extracts this before
/// `native_tiers` is consumed to build the tier list. [`apply_tier_ids_and_targets`] uses it.
pub(in crate::native) fn raw_tier_ids_from_native(native_tiers: &[TierTable]) -> Vec<Option<u64>> {
    native_tiers.iter().map(|t| t.tier_id).collect()
}

/// [`raw_tier_ids_from_native`]'s counterpart for [`TierTable::target`].
pub(in crate::native) fn raw_tier_targets_from_native(
    native_tiers: &[TierTable],
) -> Vec<Option<NativeTierTarget>> {
    native_tiers.iter().map(|t| t.target).collect()
}

/// Restores `design.tier_ids` and `design.tier_targets` from `raw_ids` and `raw_targets`,
/// captured before the file's `tiers` array was consumed. Constructors such as
/// `Design::new` assign fresh ids, so without this the ids and targets would be lost.
///
/// Each `Some(id)` is restored verbatim, except a duplicate. Only the first holder of an id
/// keeps it. Later holders get fresh ids, since two tiers sharing a [`TierId`] would make
/// the id-keyed maps pick the wrong tier. [`Design::next_tier_id`] is first bumped past the
/// highest restored id, so [`Design::allocate_tier_id`] cannot hand out a claimed id.
///
/// `design.tier_ids` is replaced wholesale, because the tiers were rebuilt from this same
/// list. `design.tier_targets` is keyed by the id each tier ended up with.
///
/// `raw_ids`, `raw_targets` and `design.tiers` must have the same length and order.
pub(in crate::native) fn apply_tier_ids_and_targets(
    design: &mut Design,
    raw_ids: Vec<Option<u64>>,
    raw_targets: Vec<Option<NativeTierTarget>>,
) {
    debug_assert_eq!(design.tiers.len(), raw_ids.len());
    debug_assert_eq!(raw_ids.len(), raw_targets.len());

    if let Some(max_restored) = raw_ids.iter().filter_map(|id| *id).max() {
        design.next_tier_id = design.next_tier_id.max(max_restored + 1);
    }

    let mut seen = BTreeSet::new();
    let ids: Vec<TierId> = raw_ids
        .into_iter()
        .map(|maybe_id| {
            maybe_id
                .filter(|id| seen.insert(*id))
                .map_or_else(|| design.allocate_tier_id(), TierId)
        })
        .collect();
    design.tier_targets = raw_targets
        .into_iter()
        .zip(&ids)
        .filter_map(|(maybe_target, &id)| maybe_target.map(|t| (id, tier_target_from_native(t))))
        .collect();
    design.tier_ids = ids;
}

/// [`raw_tier_ids_from_native`]'s counterpart for [`TierTable::frosted`] and
/// [`TierTable::frosted_facets`]: per tier, whether it is frosted as a whole and which facets
/// are frosted one by one.
pub(in crate::native) fn raw_tier_frosting_from_native(
    native_tiers: &[TierTable],
) -> Vec<(bool, Vec<f64>)> {
    native_tiers
        .iter()
        .map(|t| (t.frosted, t.frosted_facets.clone()))
        .collect()
}

/// Restores `design.tier_frosting` from `raw` (captured via [`raw_tier_frosting_from_native`]
/// before the `tiers` array was consumed), keyed by each tier's id. A tier the file does not
/// mark is polished, and a non-finite facet position is dropped. Must run after
/// [`apply_tier_ids_and_targets`].
pub(in crate::native) fn apply_tier_frosting(design: &mut Design, raw: &[(bool, Vec<f64>)]) {
    design.tier_frosting.clear();
    for ((frosted, facets), &id) in raw.iter().zip(&design.tier_ids) {
        let frosting = if *frosted {
            Some(TierFrosting::Tier)
        } else {
            TierFrosting::of_facets(facets.iter().copied())
        };
        if let Some(frosting) = frosting {
            design.tier_frosting.insert(id, frosting);
        }
    }
}

/// [`raw_tier_ids_from_native`]'s counterpart for [`TierTable::angle_relation`].
pub(in crate::native) fn raw_tier_relations_from_native(
    native_tiers: &[TierTable],
) -> Vec<Option<String>> {
    native_tiers
        .iter()
        .map(|t| t.angle_relation.clone())
        .collect()
}

/// Restores `design.tier_relations` from `raw` (captured via [`raw_tier_relations_from_native`]
/// before the `tiers` array was consumed). Then every driven tier's angle follows its relation,
/// so a stored angle that drifted from its relation, through a hand edit, opens consistent.
///
/// Must run after [`apply_tier_ids_and_targets`]: a relation names tiers by id.
///
/// # Errors
///
/// The position of the tier whose relation is unusable and why: text that is not
/// canonical, a tier that is gone, a loop, or a result that is not an angle. The position
/// is that of the tier the error is about, not of the first tier that has a relation. The
/// design is left without relations in that case.
pub(in crate::native) fn apply_tier_relations(
    design: &mut Design,
    raw: &[Option<String>],
) -> Result<(), (usize, String)> {
    design.tier_relations.clear();
    let mut first_driven = None;
    for (index, text) in raw.iter().enumerate() {
        let (Some(text), Some(&id)) = (text, design.tier_ids.get(index)) else {
            continue;
        };
        match TierRelation::parse_canonical(text) {
            Ok(relation) => {
                design.tier_relations.insert(id, relation);
                first_driven.get_or_insert(index);
            }
            Err(error) => {
                design.tier_relations.clear();
                return Err((index, error.to_string()));
            }
        }
    }
    let Some(first) = first_driven else {
        return Ok(());
    };
    match design.evaluate_relations() {
        Ok(updates) => {
            for (position, angle) in updates {
                design.tiers[position].angle_deg = angle;
            }
            Ok(())
        }
        Err(error) => {
            // Find the tier before the relations are cleared: it is found by its label
            // among the driven tiers.
            let position = failing_tier_position(design, &error, first);
            design.tier_relations.clear();
            Err((position, error.to_string()))
        }
    }
}

/// The position of the driven tier an evaluation `error` is about, or `fallback` when the
/// error names no tier that can be found.
///
/// The error names tiers by label (it is written for the cutter), so the first driven tier
/// with that label is taken; for a loop, the first tier named in it.
fn failing_tier_position(design: &Design, error: &RelationError, fallback: usize) -> usize {
    let label = match error {
        RelationError::MissingTier { tier }
        | RelationError::OutOfRange { tier, .. }
        | RelationError::HorizontalTier { tier }
        | RelationError::GirdleTier { tier }
        | RelationError::DivisionByZero { tier }
        | RelationError::NotFinite { tier } => tier.as_str(),
        RelationError::Cycle(names) => match names.first() {
            Some(name) => name.as_str(),
            None => return fallback,
        },
        RelationError::NoSuchTier { index } => return *index,
        RelationError::Parse(_) => return fallback,
    };
    (0..design.tiers.len())
        .find(|&position| {
            design
                .tier_ids
                .get(position)
                .is_some_and(|id| design.tier_relations.contains_key(id))
                && design.relation_label(position) == label
        })
        .unwrap_or(fallback)
}

/// Installs concave tiers read from a file onto `design`. Each record is converted and the
/// set is validated against the design's gear and flat tier names. Each tier keeps its
/// stable id, since the cutting-mode marks of a concave step hang on it.
///
/// Ids follow the rule of [`apply_tier_ids_and_targets`]: a recorded id is kept unless an
/// earlier holder has it, and a missing id gets a fresh one. Both lists draw from one
/// counter, so [`Design::next_tier_id`] is bumped past the highest restored id first.
///
/// The tiers are installed only when all of them pass, so a design is never half-loaded.
/// An empty `tables` is a no-op.
///
/// # Errors
///
/// The position of the first unusable tier and why.
pub(in crate::native) fn restore_concave_tiers(
    design: &mut Design,
    tables: Vec<ConcaveTierTable>,
) -> Result<(), (usize, String)> {
    if tables.is_empty() {
        return Ok(());
    }
    let raw_ids: Vec<Option<u64>> = tables.iter().map(|table| table.concave_tier_id).collect();
    design.concave_tiers = tables
        .into_iter()
        .enumerate()
        .map(|(index, table)| concave_tier_from_table(table).map_err(|reason| (index, reason)))
        .collect::<Result<_, _>>()?;
    if let Err((index, error)) = design.validate_concave_tiers() {
        design.concave_tiers.clear();
        return Err((index, error.to_string()));
    }
    if let Some(max_restored) = raw_ids.iter().filter_map(|id| *id).max() {
        design.next_tier_id = design.next_tier_id.max(max_restored.saturating_add(1));
    }
    let mut taken: BTreeSet<u64> = design.tier_ids.iter().map(|id| id.value()).collect();
    let ids: Vec<TierId> = raw_ids
        .into_iter()
        .map(|maybe_id| {
            maybe_id
                .filter(|id| taken.insert(*id))
                .map_or_else(|| design.allocate_tier_id(), TierId)
        })
        .collect();
    design.concave_tier_ids = ids;
    Ok(())
}

/// The one-tier body of [`crate::native::design_from_file`]: `None` iff `saved` is
/// missing `angle_deg`/`indices`, the two fields a design file's tier list is the SOLE
/// source for.
pub(in crate::native) fn tier_from_table_with_full_geometry(
    saved: TierTable,
) -> Option<ConstraintTier> {
    Some(ConstraintTier {
        angle_deg: saved.angle_deg?,
        name: saved.name,
        indices: saved.indices?,
        constraint: meet_constraint_from_native(saved.constraint),
        imported_meet: saved.imported_meet.map(meet_constraint_from_native),
        original_notes: saved.original_notes,
        detached: saved.detached,
    })
}
