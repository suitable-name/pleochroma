//! The tutorial browser's list without the window (Help > Tutorials...): the rows for a search
//! query over the lessons the browser offers, and the progress line under them.
//!
//! Which lessons exist, how they are grouped and searched, and the "Done" bookkeeping are pure
//! functions in `pleochroma_editor::guide` (`browser_rows`, `progress_text`, ...). This module
//! turns them into plain-string rows ([`TutorialRow`]); the page copies each one into the Slint
//! `TutorialRowData` its `TutorialsModel` draws (`editor::tutorials`).

use pleochroma_editor::guide::{
    BrowserRow, Guide, browser_rows, completed_count, progress_text, start_label, steps_text,
};
use std::collections::BTreeSet;

/// One row of the tutorial browser, as the texts and flags it draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TutorialRow {
    /// The lesson's id.
    pub id: String,
    /// The lesson's title.
    pub title: String,
    /// The one-line summary.
    pub summary: String,
    /// The section's name.
    pub category: String,
    /// The row is the first of its section, which then shows the heading.
    pub first_in_category: bool,
    /// The lesson has been finished before.
    pub done: bool,
    /// "Start", or "Restart" once finished.
    pub start_label: String,
    /// Why the lesson cannot start right now; empty when it can.
    pub blocked_reason: String,
    /// "N steps".
    pub step_text: String,
}

/// One [`BrowserRow`] as the row the browser draws.
fn row_data(row: &BrowserRow) -> TutorialRow {
    TutorialRow {
        id: row.id.clone(),
        title: row.title.clone(),
        summary: row.summary.clone(),
        category: row.category.label().to_owned(),
        first_in_category: row.first_in_category,
        done: row.done,
        start_label: start_label(row.done).to_owned(),
        blocked_reason: row.blocked.unwrap_or_default().to_owned(),
        step_text: steps_text(row.step_count),
    }
}

/// The rows the browser shows for `query` over `guides`, and the line under them. The count in
/// that line is over every lesson offered, not only the ones the search leaves.
#[must_use]
pub fn listing(
    guides: &[Guide],
    finished: &BTreeSet<String>,
    has_design: bool,
    query: &str,
) -> (Vec<TutorialRow>, String) {
    let rows = browser_rows(guides, finished, has_design, query)
        .iter()
        .map(row_data)
        .collect();
    let line = progress_text(completed_count(guides, finished), guides.len());
    (rows, line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide_runner::Offers;
    use pleochroma_editor::guide::{
        Goal, GuideCategory, GuideStep, NEEDS_A_DESIGN, StartingState, WORKED_EXAMPLE_ID,
        WebFeatures, is_offered_on_web,
    };

    fn needs_a_design() -> Guide {
        Guide::new(
            "slice-a-facet",
            "Slicing a facet",
            "Cut a facet with the Slice tool.",
            GuideCategory::Tiers,
        )
        .starting(StartingState::RequiresOpenDesign)
        .step(
            GuideStep::new("Add the girdle", "Build the first tier.")
                .actions(["Click + Add Tier."])
                .check("a G1 row in the tier table.")
                .goal(Goal::tier("G1", 90.0), "tier G1 at 90.0")
                .highlight("tier_table"),
        )
    }

    fn offered() -> Vec<Guide> {
        Offers::new().offered()
    }

    #[test]
    fn the_list_marks_finished_lessons_and_counts_them() {
        let guides = offered();
        let total = guides.len();
        assert!(
            total > 40,
            "the browser offers dozens of lessons, not {total}"
        );
        let (_, line) = listing(&guides, &BTreeSet::new(), true, "");
        assert_eq!(
            line,
            format!("No tutorials finished yet ({total} available).")
        );

        let finished = BTreeSet::from([WORKED_EXAMPLE_ID.to_owned()]);
        let (rows, line) = listing(&guides, &finished, true, "");
        assert_eq!(line, format!("1 of {total} tutorials finished."));
        let done: Vec<&str> = rows
            .iter()
            .filter(|row| row.done)
            .map(|row| row.id.as_str())
            .collect();
        assert_eq!(done, [WORKED_EXAMPLE_ID]);
        assert!(
            rows.iter()
                .any(|row| row.done && row.start_label == "Restart"),
            "a finished lesson offers Restart"
        );
    }

    #[test]
    fn a_search_narrows_the_list_but_not_the_count_under_it() {
        let guides = offered();
        let total = guides.len();
        let (rows, line) = listing(&guides, &BTreeSet::new(), true, "walkthrough");
        assert!(rows.len() < total);
        assert!(rows.iter().any(|row| row.id.as_str() == WORKED_EXAMPLE_ID));
        assert!(line.contains(&total.to_string()), "{line}");
        let (rows, _) = listing(&guides, &BTreeSet::new(), true, "no such tutorial anywhere");
        assert_eq!(rows.len(), 0);
    }

    #[test]
    fn the_list_offers_no_lesson_the_web_cannot_complete() {
        let guides = offered();
        let (rows, _) = listing(&guides, &BTreeSet::new(), true, "");
        let ids: BTreeSet<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        // A class C lesson: blocked by something no lane builds.
        assert!(!ids.contains("tiers-relations"));
        assert!(!ids.contains("app-command-palette"));
        // The offer table decides, not the browser: every row is an offered lesson.
        let features = Offers::new().features();
        assert_eq!(
            features,
            WebFeatures {
                library: false,
                ..WebFeatures::ALL
            }
        );
        for row in &rows {
            assert!(
                is_offered_on_web(row.id.as_str(), features),
                "{} is listed but not offered",
                row.id
            );
        }
    }

    #[test]
    fn a_lesson_that_needs_a_design_says_why_it_cannot_start() {
        let guides = vec![needs_a_design()];
        assert_eq!(needs_a_design().problem(), None, "a fit lesson");
        let (rows, _) = listing(&guides, &BTreeSet::new(), false, "");
        assert_eq!(rows[0].blocked_reason.as_str(), NEEDS_A_DESIGN);
        let (rows, _) = listing(&guides, &BTreeSet::new(), true, "");
        assert!(rows[0].blocked_reason.is_empty());
    }

    #[test]
    fn every_section_heading_sits_on_the_first_row_of_its_section_only() {
        let guides = offered();
        let (rows, _) = listing(&guides, &BTreeSet::new(), true, "");
        let mut last_category = String::new();
        for row in &rows {
            let new_section = row.category != last_category;
            assert_eq!(row.first_in_category, new_section, "{}", row.id);
            last_category.clone_from(&row.category);
        }
    }

    #[test]
    fn a_row_carries_the_texts_the_browser_draws() {
        let guides = offered();
        let (rows, _) = listing(&guides, &BTreeSet::new(), true, "walkthrough");
        let row = rows
            .iter()
            .find(|row| row.id.as_str() == WORKED_EXAMPLE_ID)
            .expect("listed");
        assert_eq!(row.title.as_str(), "New Design Walkthrough");
        assert_eq!(row.category.as_str(), GuideCategory::GettingStarted.label());
        assert!(row.step_text.ends_with("steps"), "{}", row.step_text);
        assert!(!row.summary.is_empty());
        assert_eq!(row.start_label.as_str(), "Start");
    }
}
