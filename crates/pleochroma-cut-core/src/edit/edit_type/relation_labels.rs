//! The cutter-facing sentences of the edits that involve tier relations: a single
//! [`super::Edit::SetTierRelation`], and the batches an editor session builds around one (a tier
//! form save that also sets the relation, an edit plus the angle updates that follow it). Split
//! from `edit_type` to keep that file readable; the wording is unchanged.

use super::{Design, Edit, TierRelation, raw_tier_name_at, tier_display_name, tier_label_at};

/// [`Edit::describe`]'s [`Edit::SetTierRelation`] arm: "Set P2 = P1 - 2" (tiers by
/// name) or "Clear relation for P2".
pub(super) fn describe_tier_relation(
    index: usize,
    relation: Option<&TierRelation>,
    design: &Design,
) -> String {
    let label = tier_label_at(design, index);
    relation.map_or_else(
        || format!("Clear relation for {label}"),
        |relation| format!("Set {label} = {}", relation.to_display(design)),
    )
}

/// [`describe_tier_relation`]'s wording as the second half of a longer label ("Rename P2 to
/// P3 and set P3 = P1 - 2"): the same sentence, for a tier already called `label`, with its
/// first letter in lower case.
fn describe_relation_clause(
    label: &str,
    relation: Option<&TierRelation>,
    design: &Design,
) -> String {
    relation.map_or_else(
        || format!("clear relation for {label}"),
        |relation| format!("set {label} = {}", relation.to_display(design)),
    )
}

/// The label `Edit::describe` gives the batches an editor session builds when tier
/// relations are in play: an edit plus the [`Edit::RetargetAngles`] that moves the
/// tiers following it, in either order (an undo entry holds the pair reversed).
///
/// - a [`Edit::SetTierRelation`] with its angle update reads as that relation edit;
/// - a [`Edit::ModifyTier`] with its angle update reads as that modification;
/// - angle changes only (a nudge or drag plus the tiers that follow) read as "Set X
///   angle to ..." for one tier and "Change angles of N tiers" for several;
/// - any other first edit followed only by angle updates and relation clears (a tier
///   removal that also frees the relations reading it) reads as that first edit;
/// - a tier form save that also set the tier's relation reads as both ("Rename P2 to P3 and
///   set P3 = P1 - 2"), see [`describe_tier_save_with_relation`].
///
/// `None` for any other batch, which keeps the generic label.
pub(super) fn describe_relation_batch(edits: &[Edit], design: &Design) -> Option<String> {
    if let Some(label) = describe_tier_save_with_relation(edits, design) {
        return Some(label);
    }
    let follows = |edit: &Edit| {
        matches!(
            edit,
            Edit::RetargetAngles { .. } | Edit::SetTierRelation { relation: None, .. }
        )
    };
    if let [first, rest @ ..] = edits
        && !rest.is_empty()
        && !matches!(first, Edit::RetargetAngles { .. })
        && rest.iter().all(follows)
    {
        return Some(first.describe(design));
    }
    let [first, second] = edits else {
        return all_angle_changes_label(edits, design);
    };
    match (first, second) {
        (Edit::SetTierRelation { .. }, Edit::RetargetAngles { .. })
        | (Edit::RetargetAngles { .. }, Edit::SetTierRelation { .. })
        | (Edit::ModifyTier { .. }, Edit::RetargetAngles { .. })
        | (Edit::RetargetAngles { .. }, Edit::ModifyTier { .. }) => {
            let edit = if matches!(first, Edit::RetargetAngles { .. }) {
                second
            } else {
                first
            };
            Some(edit.describe(design))
        }
        _ => all_angle_changes_label(edits, design),
    }
}

/// Every edit of `edits` with the nested batches opened up, in order. A tier form save is a
/// batch inside the batch an editor session folds the following angles into.
fn flattened<'a>(edits: &'a [Edit], into: &mut Vec<&'a Edit>) {
    for edit in edits {
        match edit {
            Edit::Batch(inner) => flattened(inner, into),
            other => into.push(other),
        }
    }
}

/// [`describe_relation_batch`]'s label for a tier form save that also sets the tier's relation,
/// or for the undo of one: the tier part followed by the relation part, "Rename P2 to P3 and
/// set P3 = P1 - 2", "Modify tier P2 and set P2 = P1 - 2", "Add tier P5 and set P5 = P1 - 2".
///
/// The batch (nested batches opened up) holds exactly one [`Edit::AddTier`] or
/// [`Edit::ModifyTier`] and a [`Edit::SetTierRelation`] for that same tier, and nothing but the
/// edits that ride along with a tier save: the meet-name rewrites a rename brings
/// ([`Edit::SetConstraint`]), a depth target ([`Edit::SetTierTarget`]) and the angles that
/// follow ([`Edit::RetargetAngles`]). A change of name reads as a rename, and the relation
/// part names the tier by its name after the edit. The undo of such a save holds the same
/// edits the other way round and reads the same way ("Rename P3 to P2 and clear relation for
/// P2").
///
/// A [`Edit::RemoveTier`] with nothing but relation clears (the undo of adding a tier with a
/// relation, or a removal that frees the relations reading it) reads as the removal.
///
/// `None` for any other batch.
fn describe_tier_save_with_relation(edits: &[Edit], design: &Design) -> Option<String> {
    let mut flat = Vec::new();
    flattened(edits, &mut flat);
    let mut tier_part: Option<&Edit> = None;
    let mut relations: Vec<(usize, Option<&TierRelation>)> = Vec::new();
    for edit in flat {
        match edit {
            Edit::AddTier { .. } | Edit::ModifyTier { .. } | Edit::RemoveTier { .. } => {
                if tier_part.replace(edit).is_some() {
                    return None;
                }
            }
            Edit::SetTierRelation { index, relation } => {
                relations.push((*index, relation.as_ref()));
            }
            Edit::SetConstraint { .. }
            | Edit::SetTierTarget { .. }
            | Edit::RetargetAngles { .. }
            | Edit::RestoreTierId { .. } => {}
            _ => return None,
        }
    }
    match tier_part? {
        remove @ Edit::RemoveTier { .. } => (!relations.is_empty()
            && relations.iter().all(|(_, relation)| relation.is_none()))
        .then(|| remove.describe(design)),
        Edit::AddTier { index, tier } => {
            let (_, relation) = relations.iter().find(|(at, _)| at == index)?;
            let name = tier_display_name(&tier.name);
            Some(format!(
                "Add tier {name} and {}",
                describe_relation_clause(&name, *relation, design)
            ))
        }
        Edit::ModifyTier { index, tier } => {
            let (_, relation) = relations.iter().find(|(at, _)| at == index)?;
            let new = tier_display_name(&tier.name);
            let renamed = design
                .tiers
                .get(*index)
                .is_some_and(|current| current.name != tier.name);
            // A rename says the names as they are; any other change says what the table shows.
            let old = if renamed {
                raw_tier_name_at(design, *index)
            } else {
                tier_label_at(design, *index)
            };
            let first = if renamed {
                format!("Rename {old} to {new}")
            } else {
                format!("Modify tier {old}")
            };
            Some(format!(
                "{first} and {}",
                describe_relation_clause(&new, *relation, design)
            ))
        }
        _ => None,
    }
}

/// The label of a batch made only of [`Edit::RetargetAngles`] (at least two of them),
/// or `None` for anything else.
fn all_angle_changes_label(edits: &[Edit], design: &Design) -> Option<String> {
    if edits.len() < 2 {
        return None;
    }
    let mut tiers = Vec::new();
    for edit in edits {
        let Edit::RetargetAngles { changes } = edit else {
            return None;
        };
        tiers.extend(changes.iter().map(|&(index, _, new_deg)| (index, new_deg)));
    }
    let mut distinct: Vec<usize> = tiers.iter().map(|&(index, _)| index).collect();
    distinct.sort_unstable();
    distinct.dedup();
    match (distinct.as_slice(), tiers.last()) {
        ([index], Some(&(_, new_deg))) => Some(format!(
            "Set {} angle to {:.1} degrees",
            tier_label_at(design, *index),
            new_deg.abs()
        )),
        (many, _) => Some(format!("Change angles of {} tiers", many.len())),
    }
}
