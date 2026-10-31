//! Which tutorial events a change in the page raises, without the window.
//!
//! The page reads its controls, asks one of these functions what the change was, and raises the
//! answers through its lesson runner. Each compares a state before and after, as the desktop's
//! `library/tutorial_state.rs` and `raise_effect_events` do, so a step that waits for a state
//! ("Clear it", "Put everything back", "Off") is not finished by the next keystroke.
//!
//! - [`library_events`] and [`library_is_narrowed`]: the library's search box and filters.
//! - [`changed_effects`]: the render panel's stone effects (haze, frosted girdle, edge rounding).
//! - [`auto_solve_event`]: the Auto list's Off or a time.
//! - [`depth_cut_completed`]: the frame that answers a Slice depth edit.

use crate::{
    library_view::{ALL, LibraryQuery},
    settings::RenderSettings,
};
use pleochroma_editor::guide::{solving_events, viewing_events};

/// Whether a combo value restricts the list (empty and "All" do not).
fn restricts(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value != ALL
}

/// Whether the search or a filter narrows the list. The sort order is not a filter, and
/// "Show Ignored" only widens the list, so neither counts.
#[must_use]
pub fn library_is_narrowed(query: &LibraryQuery) -> bool {
    !query.text.trim().is_empty()
        || restricts(&query.shape)
        || restricts(&query.gear)
        || restricts(&query.tag)
        || query.concave_only
}

/// The tutorial events a change of the library query from `before` to `after` raises, in order.
#[must_use]
pub fn library_events(before: &LibraryQuery, after: &LibraryQuery) -> Vec<&'static str> {
    let mut raised = Vec::new();
    if before.text != after.text {
        if !after.text.trim().is_empty() {
            raised.push(viewing_events::LIBRARY_SEARCHED);
        } else if !before.text.trim().is_empty() {
            raised.push(viewing_events::LIBRARY_SEARCH_CLEARED);
        }
    }
    if before.shape != after.shape
        || before.gear != after.gear
        || before.tag != after.tag
        || before.concave_only != after.concave_only
        || before.show_ignored != after.show_ignored
        || before.sort != after.sort
    {
        raised.push(viewing_events::LIBRARY_FILTERED);
    }
    if library_is_narrowed(before) && !library_is_narrowed(after) {
        raised.push(viewing_events::LIBRARY_FILTERS_RESET);
    }
    raised
}

/// The lesson events for the stone effects that `next` changes against `before`.
///
/// The inclusion haze, the frosted girdle and the edge rounding each have their own (the
/// desktop's `raise_effect_events`). The stone width is the design's (Apply Yield Inputs), not a panel
/// edit.
#[must_use]
pub fn changed_effects(before: &RenderSettings, next: &RenderSettings) -> Vec<&'static str> {
    let mut events = Vec::new();
    if before.inclusion_sigma_s.to_bits() != next.inclusion_sigma_s.to_bits() {
        events.push(viewing_events::STONE_HAZE_CHANGED);
    }
    if before.frosted_girdle != next.frosted_girdle {
        events.push(viewing_events::STONE_GIRDLE_FROSTED_CHANGED);
    }
    if before.edge_rounding_radius.to_bits() != next.edge_rounding_radius.to_bits() {
        events.push(viewing_events::STONE_EDGE_ROUNDING_CHANGED);
    }
    events
}

/// The lesson event for choosing `budget_ms` in the Auto list: `Off` or a time.
#[must_use]
pub const fn auto_solve_event(budget_ms: u32) -> &'static str {
    if budget_ms == 0 {
        solving_events::AUTO_SOLVE_OFF
    } else {
        solving_events::AUTO_SOLVE_ON
    }
}

/// Whether the frame that landed answers the pending depth edit with a facet that reaches the
/// stone (the desktop's rule, `slice/provisional.rs`).
///
/// A frame answers the edit when the provisional tier's requested mast is the pending one and
/// the solved mast of the frame is that depth. The request stays pending until such a frame
/// shows a facet on the stone, so a late or zero-facet frame never raises the event early.
#[must_use]
pub fn depth_cut_completed(
    pending: &mut Option<f64>,
    requested: Option<f64>,
    landed: Option<f64>,
    reaches_stone: bool,
) -> bool {
    let Some(expected) = *pending else {
        return false;
    };
    let tolerance = 1.0e-9 * expected.abs().max(1.0e-12);
    let requested_matches = requested.is_some_and(|value| {
        value.is_finite() && expected.is_finite() && (value - expected).abs() <= tolerance
    });
    let landed_matches = landed.is_some_and(|value| {
        value.is_finite() && expected.is_finite() && (value - expected.abs()).abs() <= tolerance
    });
    if !requested_matches || !landed_matches || !reaches_stone {
        return false;
    }
    *pending = None;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_view::LibrarySort;

    #[test]
    fn a_frame_that_shows_the_pinned_depth_on_the_stone_answers_the_edit() {
        let mut pending = Some(0.5);
        assert!(depth_cut_completed(
            &mut pending,
            Some(0.5),
            Some(0.5),
            true
        ));
        assert_eq!(pending, None);
    }

    #[test]
    fn without_an_edit_no_frame_answers() {
        let mut pending = None;
        assert!(!depth_cut_completed(
            &mut pending,
            Some(0.5),
            Some(0.5),
            true
        ));
    }

    #[test]
    fn a_frame_of_another_depth_leaves_the_edit_pending() {
        let mut pending = Some(0.5);
        assert!(!depth_cut_completed(
            &mut pending,
            Some(0.4),
            Some(0.4),
            true
        ));
        assert!(!depth_cut_completed(
            &mut pending,
            Some(0.5),
            Some(0.4),
            true
        ));
        assert_eq!(pending, Some(0.5));
    }

    #[test]
    fn a_frame_with_no_facet_on_the_stone_leaves_the_edit_pending() {
        let mut pending = Some(0.5);
        assert!(!depth_cut_completed(
            &mut pending,
            Some(0.5),
            Some(0.5),
            false
        ));
        assert_eq!(pending, Some(0.5));
        assert!(depth_cut_completed(
            &mut pending,
            Some(0.5),
            Some(0.5),
            true
        ));
    }

    #[test]
    fn a_signed_request_lands_as_an_absolute_mast() {
        let mut pending = Some(-0.5);
        assert!(depth_cut_completed(
            &mut pending,
            Some(-0.5),
            Some(0.5),
            true
        ));
    }

    fn with_text(text: &str) -> LibraryQuery {
        LibraryQuery {
            text: text.to_owned(),
            ..LibraryQuery::default()
        }
    }

    #[test]
    fn an_untouched_library_is_not_narrowed() {
        assert!(!library_is_narrowed(&LibraryQuery::default()));
    }

    #[test]
    fn a_search_a_shape_a_gear_a_tag_or_the_concave_chip_narrows_it() {
        assert!(library_is_narrowed(&with_text("rbc")));
        for query in [
            LibraryQuery {
                shape: "Round".to_owned(),
                ..LibraryQuery::default()
            },
            LibraryQuery {
                gear: "96".to_owned(),
                ..LibraryQuery::default()
            },
            LibraryQuery {
                tag: "mine".to_owned(),
                ..LibraryQuery::default()
            },
            LibraryQuery {
                concave_only: true,
                ..LibraryQuery::default()
            },
        ] {
            assert!(library_is_narrowed(&query), "{query:?}");
        }
    }

    #[test]
    fn spaces_the_all_entry_the_sort_and_show_ignored_are_no_filter() {
        let query = LibraryQuery {
            text: "   ".to_owned(),
            shape: ALL.to_owned(),
            gear: ALL.to_owned(),
            tag: ALL.to_owned(),
            show_ignored: true,
            sort: LibrarySort::Title,
            ..LibraryQuery::default()
        };
        assert!(!library_is_narrowed(&query));
    }

    #[test]
    fn typing_is_a_search_and_emptying_the_box_is_a_cleared_search() {
        let none = LibraryQuery::default();
        assert_eq!(
            library_events(&none, &with_text("r")),
            [viewing_events::LIBRARY_SEARCHED]
        );
        assert_eq!(
            library_events(&with_text("r"), &none),
            [
                viewing_events::LIBRARY_SEARCH_CLEARED,
                viewing_events::LIBRARY_FILTERS_RESET
            ]
        );
        assert_eq!(
            library_events(&with_text("r"), &with_text("rb")),
            [viewing_events::LIBRARY_SEARCHED]
        );
    }

    #[test]
    fn spaces_typed_into_an_empty_box_clear_nothing() {
        assert!(library_events(&LibraryQuery::default(), &with_text("  ")).is_empty());
    }

    #[test]
    fn a_combo_a_chip_or_the_sort_is_a_filter_change() {
        let none = LibraryQuery::default();
        let shape = LibraryQuery {
            shape: "Round".to_owned(),
            ..LibraryQuery::default()
        };
        assert_eq!(
            library_events(&none, &shape),
            [viewing_events::LIBRARY_FILTERED]
        );
        let sorted = LibraryQuery {
            sort: LibrarySort::OldestFirst,
            ..LibraryQuery::default()
        };
        assert_eq!(
            library_events(&none, &sorted),
            [viewing_events::LIBRARY_FILTERED]
        );
        let widened = LibraryQuery {
            show_ignored: true,
            ..LibraryQuery::default()
        };
        assert_eq!(
            library_events(&none, &widened),
            [viewing_events::LIBRARY_FILTERED]
        );
    }

    #[test]
    fn only_taking_the_last_narrowing_off_is_a_reset() {
        let round = LibraryQuery {
            shape: "Round".to_owned(),
            ..LibraryQuery::default()
        };
        let round_concave = LibraryQuery {
            concave_only: true,
            ..round.clone()
        };
        // Narrowing further, and taking one of two off, are not resets.
        assert_eq!(
            library_events(&round, &round_concave),
            [viewing_events::LIBRARY_FILTERED]
        );
        assert_eq!(
            library_events(&round_concave, &round),
            [viewing_events::LIBRARY_FILTERED]
        );
        // Taking the last one off is.
        assert_eq!(
            library_events(&round, &LibraryQuery::default()),
            [
                viewing_events::LIBRARY_FILTERED,
                viewing_events::LIBRARY_FILTERS_RESET
            ]
        );
    }

    #[test]
    fn an_untouched_panel_changes_no_stone_effect() {
        let settings = RenderSettings::default();
        assert!(changed_effects(&settings, &settings.clone()).is_empty());
    }

    #[test]
    fn each_stone_effect_reports_its_own_event() {
        let before = RenderSettings::default();
        let haze = RenderSettings {
            inclusion_sigma_s: before.inclusion_sigma_s + 1.0,
            ..before.clone()
        };
        assert_eq!(
            changed_effects(&before, &haze),
            [viewing_events::STONE_HAZE_CHANGED]
        );
        let frosted = RenderSettings {
            frosted_girdle: !before.frosted_girdle,
            ..before.clone()
        };
        assert_eq!(
            changed_effects(&before, &frosted),
            [viewing_events::STONE_GIRDLE_FROSTED_CHANGED]
        );
        let rounded = RenderSettings {
            edge_rounding_radius: before.edge_rounding_radius + 0.01,
            ..before.clone()
        };
        assert_eq!(
            changed_effects(&before, &rounded),
            [viewing_events::STONE_EDGE_ROUNDING_CHANGED]
        );
    }

    #[test]
    fn a_change_that_is_not_a_stone_effect_reports_nothing() {
        let before = RenderSettings::default();
        let louder = RenderSettings {
            exposure: before.exposure + 0.5,
            ..before.clone()
        };
        assert!(changed_effects(&before, &louder).is_empty());
    }

    #[test]
    fn off_and_a_time_are_the_two_auto_solve_events() {
        assert_eq!(auto_solve_event(0), solving_events::AUTO_SOLVE_OFF);
        for budget_ms in [150, 300, 1000, 3000] {
            assert_eq!(
                auto_solve_event(budget_ms),
                solving_events::AUTO_SOLVE_ON,
                "{budget_ms} ms"
            );
        }
    }

    #[test]
    fn the_auto_list_values_map_to_the_events_the_lessons_wait_for() {
        // The combo sends 0 for Off (a negative value is read as 0 too); see `tier_table_commands.slint`.
        let off = u32::try_from(-1_i32).unwrap_or(0);
        assert_eq!(auto_solve_event(off), "auto_solve_off");
        assert_eq!(auto_solve_event(300), "auto_solve_on");
        assert_eq!(solving_events::SOLVE_REQUESTED, "solve_requested");
    }
}
