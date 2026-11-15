//! The cutter-facing sentence of an [`super::Edit::SetTierMeetPick`], for the undo and redo
//! hints and the history list.

use super::{Design, Edit, tier_label_at};
use crate::design::TierMeetPick;

/// "Pick the meet point of P3 (P1, P2, G1)" for a pick and "Clear the picked meet point of P3"
/// for a clear. The list names the tiers the three picked facets belong to, once each, in
/// facet order; a tier that is no longer in `design` reads as "a removed tier".
pub(super) fn describe_meet_pick(
    design: &Design,
    index: usize,
    pick: Option<&TierMeetPick>,
) -> String {
    let label = tier_label_at(design, index);
    let Some(pick) = pick else {
        return format!("Clear the picked meet point of {label}");
    };
    let mut names: Vec<String> = Vec::with_capacity(3);
    for facet in &pick.facets {
        let name = design.index_of_tier_id(facet.tier).map_or_else(
            || "a removed tier".to_owned(),
            |at| tier_label_at(design, at),
        );
        if !names.contains(&name) {
            names.push(name);
        }
    }
    format!("Pick the meet point of {label} ({})", names.join(", "))
}

/// The sentence of a batch that is one pick or clear and nothing else: a
/// [`Edit::SetTierMeetPick`] together with the [`Edit::SetConstraint`] and [`Edit::SetTierTarget`]
/// that [`Design::meet_pick_edit`] puts beside it. `None` for any other batch, so a tier removal
/// that restores its pick still reads as "Add tier ...".
pub(super) fn describe_meet_pick_batch(edits: &[Edit], design: &Design) -> Option<String> {
    let mut pick_step = None;
    for edit in edits {
        match edit {
            Edit::SetTierMeetPick { index, pick } if pick_step.is_none() => {
                pick_step = Some((*index, pick.as_ref()));
            }
            Edit::SetConstraint { .. } | Edit::SetTierTarget { .. } => {}
            _ => return None,
        }
    }
    pick_step.map(|(index, pick)| describe_meet_pick(design, index, pick))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        design::{ConstraintTier, PickedFacet},
        preform::PreformSpec,
    };
    use pleochroma::geometry::meet_solver::MeetConstraint;

    fn design_with_tiers(names: &[&str]) -> Design {
        let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
        for name in names {
            design.tiers.push(ConstraintTier {
                angle_deg: -41.0,
                name: (*name).to_owned(),
                indices: vec![0.0],
                constraint: MeetConstraint::ScaleReference(0.5),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            });
        }
        design.ensure_tier_ids();
        design
    }

    fn pick_of(design: &Design, at: [(usize, f64); 3]) -> TierMeetPick {
        TierMeetPick {
            facets: at.map(|(position, index)| PickedFacet {
                tier: design.tier_id_at(position).expect("tier"),
                index,
            }),
        }
    }

    #[test]
    fn the_sentence_names_the_tier_and_the_corner() {
        let design = design_with_tiers(&["P1", "P2", "G1", "P3"]);
        let pick = pick_of(&design, [(0, 12.0), (1, 18.0), (2, 12.0)]);
        assert_eq!(
            describe_meet_pick(&design, 3, Some(&pick)),
            "Pick the meet point of P3 (P1, P2, G1)"
        );
        assert_eq!(
            describe_meet_pick(&design, 3, None),
            "Clear the picked meet point of P3"
        );
    }

    #[test]
    fn a_tier_that_appears_twice_is_named_once() {
        let design = design_with_tiers(&["P1", "P2", "P3"]);
        let pick = pick_of(&design, [(0, 12.0), (1, 18.0), (0, 36.0)]);
        assert_eq!(
            describe_meet_pick(&design, 2, Some(&pick)),
            "Pick the meet point of P3 (P1, P2)"
        );
    }

    #[test]
    fn a_pick_batch_reads_as_the_pick() {
        let design = design_with_tiers(&["P1", "P2", "G1", "P3"]);
        let pick = pick_of(&design, [(0, 12.0), (1, 18.0), (2, 12.0)]);
        let batch = Edit::Batch(vec![
            Edit::SetConstraint {
                index: 3,
                constraint: MeetConstraint::MeetNamed(vec!["P1".into(), "P2".into()]),
            },
            Edit::SetTierMeetPick {
                index: 3,
                pick: Some(pick),
            },
        ]);
        assert_eq!(
            batch.describe(&design),
            "Pick the meet point of P3 (P1, P2, G1)"
        );
    }
}
