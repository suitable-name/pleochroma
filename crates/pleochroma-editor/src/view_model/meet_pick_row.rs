//! The tier form's Meet Point row: what the row says about the corner a tier is cut to.
//!
//! [`meet_pick_row`] reads the design's stored pick of one tier and how it stands after the solve
//! ([`pleochroma_cut_core::Design::meet_pick_states`]) and returns the state, the chip text and
//! the explanation under the row, ready for any front end to show. The row is a function of its
//! own, so the shared tier-row struct and the web and mobile row code stay as they are.
//! [`meet_pick_blocked_text`] words why a tier cannot take a pick at all.

use super::row_format::format_index_value;
use crate::retarget::plan::tier_display_names;
use pleochroma::geometry::meet_solver::{Block, MeetConstraint, SolvedTier, classify_blocks};
use pleochroma_cut_core::{
    ConstraintTier, Design, MeetPickBlock, MeetPickLoss, MeetPickState, TierMeetPick,
};

/// Two index-wheel positions this close (modulo the gear) are the same facet; the tolerance
/// `pleochroma_cut_core` uses to find a picked facet.
const INDEX_TOLERANCE: f64 = 1e-6;

/// How the tier form shows a tier's meet pick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MeetPickRowState {
    /// The tier has no pick.
    #[default]
    None,
    /// The tier's facets pass through the picked corner, and the corner is on the stone.
    Met,
    /// The pick is stored but not in use: the tier's Meets setting or names do not cover it.
    Dormant,
    /// The tier still passes through the picked point, but an earlier tier cut the corner away.
    OffStone,
    /// The corner cannot be formed; the tier is solved by its names.
    Lost,
}

impl MeetPickRowState {
    /// The number the Slint `EditorModel.meet_pick_state` property carries: `0` none, `1` met,
    /// `2` dormant, `3` off the stone, `4` lost.
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::None => 0,
            Self::Met => 1,
            Self::Dormant => 2,
            Self::OffStone => 3,
            Self::Lost => 4,
        }
    }
}

/// What the Meet Point row shows for one tier.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeetPickRow {
    /// How the pick stands.
    pub state: MeetPickRowState,
    /// The row's chip: what the tier meets now (the corner's tier names, or who chooses the
    /// corner), or why the pick does not hold. Empty for a tier whose Meets setting has no
    /// corner to pick (an explicit scale).
    pub chip: String,
    /// The explanation under the row. Empty for a tier whose Meets setting has no corner to
    /// pick.
    pub note: String,
}

/// `items` as a list in a sentence: `A`, `A and B`, `A, B and C`.
fn list_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The tier's display name at `index`, `""` for a position the list lacks.
fn label_at(display: &[String], index: usize) -> String {
    display.get(index).cloned().unwrap_or_default()
}

/// The names `tier` meets as the constraint writes them; empty unless it is a `MeetNamed` tier.
fn constraint_names(tier: &ConstraintTier) -> Vec<String> {
    match &tier.constraint {
        MeetConstraint::MeetNamed(names) => names.clone(),
        _ => Vec::new(),
    }
}

/// The display names of the tiers a pick's three facets belong to, in facet order, each once.
fn pick_tier_names(design: &Design, pick: &TierMeetPick, display: &[String]) -> Vec<String> {
    let mut names: Vec<String> = Vec::with_capacity(3);
    for facet in &pick.facets {
        let Some(tier) = design.index_of_tier_id(facet.tier) else {
            continue;
        };
        let name = label_at(display, tier);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// How the stored pick of tier `index` stands. A pick whose state cannot be read (the masts do
/// not belong to the design, or the stone does not close) reads as met: nothing says otherwise.
fn state_of(design: &Design, solved: &[SolvedTier], index: usize) -> MeetPickState {
    design
        .meet_pick_states(solved)
        .into_iter()
        .find(|&(tier, _)| tier == index)
        .map_or(MeetPickState::Met, |(_, state)| state)
}

/// Whether `tier` has a facet at index-wheel position `position` (modulo `gear`). A tier without
/// indices has the one facet.
fn has_facet_at(tier: &ConstraintTier, gear: f64, position: f64) -> bool {
    tier.indices.is_empty()
        || (position.is_finite()
            && tier.indices.iter().any(|&candidate| {
                let diff = (candidate - position).rem_euclid(gear);
                diff.min(gear - diff) <= INDEX_TOLERANCE
            }))
}

/// The first picked facet that does not exist: its tier's display name and index text. A pick
/// facet on the picking tier itself counts as missing, as the corner cannot be formed from it.
fn missing_facet(
    design: &Design,
    index: usize,
    pick: &TierMeetPick,
    display: &[String],
) -> Option<(String, String)> {
    let gear = f64::from(design.meta.gear_teeth_abs().max(1));
    pick.facets.iter().find_map(|facet| {
        let tier = design.index_of_tier_id(facet.tier)?;
        let present = tier != index
            && design
                .tiers
                .get(tier)
                .is_some_and(|flat| has_facet_at(flat, gear, facet.index));
        (!present).then(|| (label_at(display, tier), format_index_value(facet.index)))
    })
}

/// The row of a tier without a pick.
fn unpicked_row(tier: &ConstraintTier, label: &str) -> MeetPickRow {
    let chip = match tier.constraint {
        MeetConstraint::MeetNamed(_) => "Shallowest of the names",
        MeetConstraint::MeetExisting => "Chosen by the solver",
        MeetConstraint::ScaleReference(_) => {
            return MeetPickRow::default();
        }
    };
    MeetPickRow {
        state: MeetPickRowState::None,
        chip: chip.to_string(),
        note: format!(
            "No corner picked: {label} meets the shallowest point where its named facets meet."
        ),
    }
}

/// The Meet Point row of tier `index` of `design`, with `solved` the masts of the design's solve.
///
/// A tier without a pick says who chooses its corner. A picked tier says which corner it is cut
/// to, or why the pick does not hold (cut away by an earlier tier, lost with a removed tier or
/// facet or parallel planes, or not in use). An `index` the design lacks gives the empty row.
#[must_use]
pub fn meet_pick_row(design: &Design, solved: &[SolvedTier], index: usize) -> MeetPickRow {
    let Some(tier) = design.tiers.get(index) else {
        return MeetPickRow::default();
    };
    let display = tier_display_names(design);
    let label = label_at(&display, index);
    let Some(pick) = design.tier_meet_pick(index) else {
        return unpicked_row(tier, &label);
    };
    let names = constraint_names(tier);
    let corner_names = if names.is_empty() {
        pick_tier_names(design, pick, &display)
    } else {
        names.clone()
    };
    let shallowest = if names.is_empty() {
        "its named facets".to_string()
    } else {
        list_and(&names)
    };
    match state_of(design, solved, index) {
        MeetPickState::Met => MeetPickRow {
            state: MeetPickRowState::Met,
            chip: corner_names.join(" \u{b7} "),
            note: format!(
                "{label} is cut until it reaches the corner where {} meet. The pick follows \
                 that corner when angles change.",
                list_and(&corner_names)
            ),
        },
        MeetPickState::OffStone => MeetPickRow {
            state: MeetPickRowState::OffStone,
            chip: "corner cut away".to_string(),
            note: format!(
                "The picked corner is cut away before {label} is cut. {label} still passes \
                 through that point, but there is no corner to stop at."
            ),
        },
        MeetPickState::Dormant => MeetPickRow {
            state: MeetPickRowState::Dormant,
            chip: "not in use".to_string(),
            note: format!(
                "The picked corner is not in use: the Meets setting or the names of {label} no \
                 longer include {}. Clear the pick, or restore the names to use it.",
                list_and(&pick_tier_names(design, pick, &display))
            ),
        },
        MeetPickState::Lost(loss) => {
            let cause = match loss {
                MeetPickLoss::TierRemoved => "one of its tiers was removed".to_string(),
                MeetPickLoss::FacetRemoved => missing_facet(design, index, pick, &display)
                    .map_or_else(
                        || "one of its facets does not exist any more".to_string(),
                        |(name, at)| format!("{name} has no facet at index {at} any more"),
                    ),
                MeetPickLoss::PlanesParallel => {
                    "its facets no longer meet in one point".to_string()
                }
            };
            MeetPickRow {
                state: MeetPickRowState::Lost,
                chip: "corner lost".to_string(),
                note: format!(
                    "The picked corner no longer exists: {cause}. {label} meets the shallowest \
                     point of {shallowest} until you pick again."
                ),
            }
        }
    }
}

/// The sentence that says why tier `index` cannot take a pick ([`MeetPickBlock`]).
#[must_use]
pub fn meet_pick_blocked_text(design: &Design, index: usize, block: MeetPickBlock) -> String {
    let label = label_at(&tier_display_names(design), index);
    match block {
        MeetPickBlock::NothingCutBefore => {
            format!("Nothing is cut before {label}, so there is no corner to pick.")
        }
        MeetPickBlock::SoleBlockAnchor => {
            let block = match classify_blocks(&design.meet_tier_inputs()).get(index) {
                Some(Block::Crown) => "crown",
                Some(Block::Pavilion) => "pavilion",
                Some(Block::Girdle) => "girdle",
                None => "block",
            };
            format!(
                "{label} is the only tier that sets the size of the {block}. Give another \
                 {block} tier an exact scale value first."
            )
        }
        MeetPickBlock::SizingTier => {
            format!("{label} sets the stone's size and cannot meet a corner.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{ANGLE_NUDGE_COALESCE_WINDOW, EditorSession};
    use pleochroma_cut_core::{History, PickedFacet, PreformSpec, ScheduleMeta, TierId};

    const GIRDLE: usize = 4;
    const PAVILION_MAIN: usize = 5;
    const LOWER_GIRDLE: usize = 6;

    /// The standard round brilliant, every tier pinned, in a session.
    fn brilliant_session() -> EditorSession {
        EditorSession::with_history(
            Design::new(
                PreformSpec::block(2.0, 1.0, 4.0),
                ScheduleMeta::standard_round_brilliant(),
                ConstraintTier::standard_round_brilliant(),
            ),
            History::with_coalesce_window(ANGLE_NUDGE_COALESCE_WINDOW),
        )
    }

    /// The brilliant with its lower girdle cut to the corner of the girdle and the pavilion
    /// mains, and its solved masts.
    fn picked() -> (Design, Vec<SolvedTier>) {
        let mut session = brilliant_session();
        let solved = session.design.solve().expect("every tier is pinned");
        let corner = session
            .design
            .pick_corners(&solved, LOWER_GIRDLE)
            .expect("the stone closes")
            .into_iter()
            .find(|corner| corner.tiers == [GIRDLE, PAVILION_MAIN])
            .expect("the girdle and the mains form a corner");
        session
            .pick_meet_corner(LOWER_GIRDLE, &corner)
            .expect("the corner can be picked");
        let solved = session.design.solve().expect("the picked design solves");
        (session.design, solved)
    }

    /// The display name of tier `index`.
    fn label(design: &Design, index: usize) -> String {
        tier_display_names(design)[index].clone()
    }

    /// Replaces the lower girdle's stored pick.
    fn set_pick(design: &mut Design, pick: TierMeetPick) {
        let id = design.tier_id_at(LOWER_GIRDLE).expect("the tier has an id");
        design.tier_meet_picks.insert(id, pick);
    }

    #[test]
    fn a_tier_without_a_pick_says_who_chooses_its_corner() {
        let mut design = brilliant_session().design;
        let label = label(&design, LOWER_GIRDLE);

        // An explicit scale has no corner to pick: the row is empty.
        assert_eq!(
            meet_pick_row(&design, &[], LOWER_GIRDLE),
            MeetPickRow::default()
        );

        design.tiers[LOWER_GIRDLE].constraint = MeetConstraint::MeetNamed(vec!["Girdle".into()]);
        let row = meet_pick_row(&design, &[], LOWER_GIRDLE);
        assert_eq!(row.state, MeetPickRowState::None);
        assert_eq!(row.chip, "Shallowest of the names");
        assert_eq!(
            row.note,
            format!(
                "No corner picked: {label} meets the shallowest point where its named facets \
                 meet."
            )
        );

        design.tiers[LOWER_GIRDLE].constraint = MeetConstraint::MeetExisting;
        let row = meet_pick_row(&design, &[], LOWER_GIRDLE);
        assert_eq!(row.chip, "Chosen by the solver");
        assert_eq!(row.state.code(), 0);

        assert_eq!(meet_pick_row(&design, &[], 99), MeetPickRow::default());
    }

    #[test]
    fn a_met_pick_names_the_corner_and_says_it_follows_it() {
        let (design, solved) = picked();
        let label = label(&design, LOWER_GIRDLE);
        let row = meet_pick_row(&design, &solved, LOWER_GIRDLE);
        assert_eq!(row.state, MeetPickRowState::Met);
        assert_eq!(row.state.code(), 1);
        assert_eq!(row.chip, "Girdle \u{b7} Pavilion Main");
        assert_eq!(
            row.note,
            format!(
                "{label} is cut until it reaches the corner where Girdle and Pavilion Main \
                 meet. The pick follows that corner when angles change."
            )
        );
        // The tiers without a pick keep the plain row.
        assert_eq!(
            meet_pick_row(&design, &solved, PAVILION_MAIN).state,
            MeetPickRowState::None
        );
    }

    #[test]
    fn a_corner_cut_away_before_the_tier_says_the_tier_still_passes_through_the_point() {
        let (design, solved) = picked();
        let label = label(&design, LOWER_GIRDLE);
        let mut deeper = solved;
        deeper[LOWER_GIRDLE].mast += 0.02;
        let row = meet_pick_row(&design, &deeper, LOWER_GIRDLE);
        assert_eq!(row.state, MeetPickRowState::OffStone);
        assert_eq!(row.state.code(), 3);
        assert_eq!(row.chip, "corner cut away");
        assert_eq!(
            row.note,
            format!(
                "The picked corner is cut away before {label} is cut. {label} still passes \
                 through that point, but there is no corner to stop at."
            )
        );
    }

    #[test]
    fn a_lost_pick_says_why_and_what_the_tier_meets_instead() {
        let (mut design, solved) = picked();
        let label = label(&design, LOWER_GIRDLE);
        let stored = *design
            .tier_meet_pick(LOWER_GIRDLE)
            .expect("the tier is picked");
        let shallowest = format!(
            "{label} meets the shallowest point of Girdle and Pavilion Main until you pick again."
        );

        let mut removed = stored;
        removed.facets[0].tier = TierId(9_999);
        set_pick(&mut design, removed);
        let row = meet_pick_row(&design, &solved, LOWER_GIRDLE);
        assert_eq!(row.state, MeetPickRowState::Lost);
        assert_eq!(row.state.code(), 4);
        assert_eq!(row.chip, "corner lost");
        assert_eq!(
            row.note,
            format!(
                "The picked corner no longer exists: one of its tiers was removed. {shallowest}"
            )
        );

        let mut facet_removed = stored;
        facet_removed.facets[0].index = 1.5;
        set_pick(&mut design, facet_removed);
        let row = meet_pick_row(&design, &solved, LOWER_GIRDLE);
        assert_eq!(row.state, MeetPickRowState::Lost);
        let first_tier = design
            .index_of_tier_id(stored.facets[0].tier)
            .expect("the tier exists");
        assert_eq!(
            row.note,
            format!(
                "The picked corner no longer exists: {} has no facet at index 1.50 any more. \
                 {shallowest}",
                label_at(&tier_display_names(&design), first_tier)
            )
        );

        let girdle = design.tier_id_at(GIRDLE).expect("the tier has an id");
        let on_girdle = |index: f64| PickedFacet {
            tier: girdle,
            index,
        };
        set_pick(
            &mut design,
            TierMeetPick {
                facets: [on_girdle(0.0), on_girdle(6.0), on_girdle(12.0)],
            },
        );
        let row = meet_pick_row(&design, &solved, LOWER_GIRDLE);
        assert_eq!(row.state, MeetPickRowState::Lost);
        assert_eq!(
            row.note,
            format!(
                "The picked corner no longer exists: its facets no longer meet in one point. \
                 {shallowest}"
            )
        );
    }

    #[test]
    fn a_pick_the_meets_setting_no_longer_covers_is_not_in_use() {
        let (mut design, solved) = picked();
        design.tiers[LOWER_GIRDLE].constraint = MeetConstraint::MeetNamed(vec!["Girdle".into()]);
        let row = meet_pick_row(&design, &solved, LOWER_GIRDLE);
        assert_eq!(row.state, MeetPickRowState::Dormant);
        assert_eq!(row.state.code(), 2);
        assert_eq!(row.chip, "not in use");
        assert!(
            row.note
                .contains("no longer include Girdle and Pavilion Main. Clear the pick"),
            "{}",
            row.note
        );
    }

    #[test]
    fn the_blocked_reasons_name_the_tier() {
        let design = brilliant_session().design;
        let girdle = label(&design, GIRDLE);
        assert_eq!(
            meet_pick_blocked_text(&design, GIRDLE, MeetPickBlock::NothingCutBefore),
            format!("Nothing is cut before {girdle}, so there is no corner to pick.")
        );
        assert_eq!(
            meet_pick_blocked_text(&design, GIRDLE, MeetPickBlock::SizingTier),
            format!("{girdle} sets the stone's size and cannot meet a corner.")
        );
        let main = label(&design, PAVILION_MAIN);
        let text = meet_pick_blocked_text(&design, PAVILION_MAIN, MeetPickBlock::SoleBlockAnchor);
        assert_eq!(
            text,
            format!(
                "{main} is the only tier that sets the size of the pavilion. Give another \
                 pavilion tier an exact scale value first."
            )
        );
    }
}
