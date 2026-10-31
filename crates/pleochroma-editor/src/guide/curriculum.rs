//! The learning path: which built-in lesson sits in which browser section, and in what order.
//!
//! The tutorial browser lists the sections of [`GuideCategory::ALL`] and, inside a section, the
//! lessons in the order of the catalogue (`static_guides`). That order is also the upload order
//! of the tutorial videos. [`CURRICULUM`] is the one place that fixes it: a section's lessons
//! run from basic to advanced, and [`arrange`] sorts the built-in guides by their place in the
//! table, whatever the order their modules register them in.
//!
//! Lesson ids never change (completion marks and video names hang on them), so moving a lesson
//! means moving its id in this table and giving its `Guide::new` the same section.

use super::model::{Guide, GuideCategory};

/// Every built-in lesson in browser order: each section with its lessons' ids, basic first.
///
/// The generated lessons ([`GuideCategory::BuildThisDesign`]) are not listed: they are added at
/// run time, after the built-in ones.
pub const CURRICULUM: &[(GuideCategory, &[&str])] = &[
    (
        GuideCategory::GettingStarted,
        &[
            "welcome-tour",
            "new-design-walkthrough",
            "app-help-viewer",
            "app-glossary",
        ],
    ),
    (
        GuideCategory::Tiers,
        &[
            "tiers-add-a-tier",
            "tiers-edit-a-tier",
            "tiers-quick-add",
            "tiers-inline-angle",
            "tiers-index-shorthands",
            "tiers-facet-chips",
            "tiers-tier-notes",
        ],
    ),
    (
        GuideCategory::ArrangingTiers,
        &[
            "tiers-duplicate",
            "tiers-move",
            "tiers-delete",
            "tiers-multi-select",
            "tiers-step-series",
            "tiers-linked-series",
            "tiers-mirror-to-other-block",
        ],
    ),
    (
        GuideCategory::AdvancedTiers,
        &[
            "tiers-meets-named-facets",
            "tiers-meets-mm-targets",
            "tiers-pin-to-mast",
            "tiers-adopt-imported-meets",
            "tiers-relations",
            "tiers-arithmetic",
            "tiers-cheater-offset",
            "tiers-concave-cylinder-cone",
            "tiers-concave-sphere-disc",
        ],
    ),
    (
        GuideCategory::Solving,
        &[
            "solving-solve",
            "solving-auto-solve",
            "solving-status",
            "solving-verdict",
            "solving-verdict-fixes",
            "solving-preform",
            "solving-yield-carat",
        ],
    ),
    (
        GuideCategory::Optimizing,
        &[
            "optimizing-deep-solve",
            "optimizing-optimize",
            "optimizing-retarget-shift",
            "optimizing-retarget-optimize",
            "optimizing-sweep",
            "optimizing-sweep-two-tiers",
            "solving-trials",
            "optimizing-tune-by-hand",
        ],
    ),
    (
        GuideCategory::History,
        &[
            "solving-history",
            "solving-variants",
            "solving-snapshot-compare",
            "solving-raw-text",
        ],
    ),
    (
        GuideCategory::Materials,
        &[
            "viewing-custom-material",
            "viewing-material-coefficients",
            "viewing-stone-effects",
            "materials-color-variants",
            "viewing-color-pattern",
            "materials-zone-colors-from-variants",
            "viewing-ametrine",
            "materials-orient-sapphire",
            "materials-tourmaline-axis-in-table",
            "materials-best-blue-tanzanite",
            "materials-set-crystal-axis",
            "materials-polish-quality",
            "materials-scratch-direction",
            "materials-silk-sapphire",
            "materials-opal-play-of-color",
            "materials-labradorite-schiller",
            "materials-axis-curves",
        ],
    ),
    (
        GuideCategory::Viewport,
        &[
            "viewing-solid-picking",
            "viewing-diagram",
            "viewing-cut-slider",
            "viewing-drag-handles",
            "viewing-slice",
        ],
    ),
    (
        GuideCategory::Rendering,
        &[
            "viewing-live-render",
            "viewing-design-lighting",
            "viewing-optical-diagnostics",
            "output-render-jobs",
            "output-render-jobs-queue",
        ],
    ),
    (
        GuideCategory::Output,
        &[
            "output-save",
            "output-open",
            "output-cutting-mode",
            "output-export-asc",
            "output-export-gcs",
            "output-cutting-sheet",
            "output-diagram-png",
        ],
    ),
    (
        GuideCategory::Library,
        &[
            "library-import",
            "library-search",
            "library-filters",
            "library-load-design",
        ],
    ),
    (
        GuideCategory::RoughPlanning,
        &[
            "library-rough-planner",
            "library-planner-model-rough",
            "library-planner-results",
            "library-planner-saved-plans",
            "library-planner-open-in-editor",
            "library-rough-color-wizard",
            "library-rough-color-in-planner",
            "planning-override-orientation",
        ],
    ),
    (
        GuideCategory::App,
        &[
            "prefs-interface-mode",
            "prefs-interface-scale",
            "prefs-high-contrast",
            "prefs-larger-handles",
            "app-command-palette",
            "app-keyboard-shortcuts",
        ],
    ),
];

/// The place of lesson `id` in the learning path (section, then lesson), or `None` for a guide
/// the table does not list.
#[must_use]
pub fn position(id: &str) -> Option<usize> {
    CURRICULUM
        .iter()
        .flat_map(|(_, ids)| ids.iter())
        .position(|listed| *listed == id)
}

/// Sorts the built-in `guides` into the order of [`CURRICULUM`]. A guide the table does not list
/// (a test catches that) keeps its relative place after every listed one.
pub fn arrange(guides: &mut [Guide]) {
    guides.sort_by_key(|guide| position(&guide.id).unwrap_or(usize::MAX));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guide::static_guides;
    use std::collections::BTreeSet;

    #[test]
    fn every_built_in_lesson_is_listed_exactly_once() {
        let listed: Vec<&str> = CURRICULUM
            .iter()
            .flat_map(|(_, ids)| ids.iter().copied())
            .collect();
        let unique: BTreeSet<&str> = listed.iter().copied().collect();
        assert_eq!(unique.len(), listed.len(), "a lesson is listed twice");
        let guides = static_guides();
        assert_eq!(
            guides.len(),
            listed.len(),
            "the table and the catalogue differ"
        );
        for guide in &guides {
            assert!(
                unique.contains(guide.id.as_str()),
                "{} is not in the curriculum",
                guide.id
            );
        }
    }

    #[test]
    fn the_catalogue_follows_the_table_and_each_lesson_sits_in_its_section() {
        let guides = static_guides();
        let table: Vec<(&str, GuideCategory)> = CURRICULUM
            .iter()
            .flat_map(|(category, ids)| ids.iter().map(move |id| (*id, *category)))
            .collect();
        let found: Vec<(&str, GuideCategory)> = guides
            .iter()
            .map(|guide| (guide.id.as_str(), guide.category))
            .collect();
        assert_eq!(found, table);
    }

    #[test]
    fn the_sections_come_in_the_browser_order() {
        let sections: Vec<GuideCategory> =
            CURRICULUM.iter().map(|(category, _)| *category).collect();
        let browser: Vec<GuideCategory> = GuideCategory::ALL
            .into_iter()
            .filter(|category| *category != GuideCategory::BuildThisDesign)
            .collect();
        assert_eq!(sections, browser);
        assert_eq!(
            GuideCategory::ALL.last(),
            Some(&GuideCategory::BuildThisDesign),
            "the generated lessons come last"
        );
    }

    #[test]
    fn every_section_has_at_least_two_lessons_and_none_is_generated() {
        for (category, ids) in CURRICULUM {
            assert!(
                ids.len() >= 2,
                "{} has only {} lesson(s)",
                category.label(),
                ids.len()
            );
        }
        for guide in static_guides() {
            assert_ne!(
                guide.category,
                GuideCategory::BuildThisDesign,
                "{} is a built-in lesson in the generated section",
                guide.id
            );
        }
    }

    #[test]
    fn arranging_puts_listed_lessons_in_table_order_and_unlisted_last() {
        let make = |id: &str| Guide::new(id, "Title", "Summary.", GuideCategory::Tiers);
        let mut guides = vec![
            make("app-glossary"),
            make("not-in-the-table"),
            make("welcome-tour"),
        ];
        arrange(&mut guides);
        let ids: Vec<&str> = guides.iter().map(|guide| guide.id.as_str()).collect();
        assert_eq!(ids, ["welcome-tour", "app-glossary", "not-in-the-table"]);
    }
}
