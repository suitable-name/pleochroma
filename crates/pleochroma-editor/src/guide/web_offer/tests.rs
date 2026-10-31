//! Tests for the web offer table and the web overlays, run against the real catalogue.

use super::{
    WEB_LESSONS, WEB_ONLY_TARGETS, WebClass, WebFeature, WebFeatures, is_offered_on_web,
    overlay::all_overlays, web_adapt, web_lesson, web_offered_ids, web_text_problems,
};
use crate::guide::{CURRICULUM, Goal, Group, GuideCatalog, HIGHLIGHT_TARGETS, Platform};
use std::collections::BTreeSet;

/// A build that has `feature` and none of the others.
fn only(feature: WebFeature) -> WebFeatures {
    WebFeatures {
        library: feature == WebFeature::Library,
        concave_edit: feature == WebFeature::ConcaveEdit,
        deep_solve: feature == WebFeature::DeepSolve,
        cutting_mode: feature == WebFeature::CuttingMode,
    }
}

#[test]
fn every_curriculum_lesson_is_classified_exactly_once() {
    let curriculum: Vec<&str> = CURRICULUM
        .iter()
        .flat_map(|(_, ids)| ids.iter().copied())
        .collect();
    let table: Vec<&str> = WEB_LESSONS.iter().map(|lesson| lesson.id).collect();
    let curriculum_set: BTreeSet<&str> = curriculum.iter().copied().collect();
    let table_set: BTreeSet<&str> = table.iter().copied().collect();
    assert_eq!(table.len(), table_set.len(), "a lesson is classified twice");
    assert_eq!(curriculum.len(), curriculum_set.len());
    assert_eq!(
        table_set, curriculum_set,
        "the offer table and the learning path list different lessons"
    );
}

#[test]
fn the_classes_have_the_audited_sizes() {
    let count = |class: WebClass| {
        WEB_LESSONS
            .iter()
            .filter(|lesson| lesson.class == class)
            .count()
    };
    assert_eq!(
        (
            count(WebClass::A),
            count(WebClass::B),
            count(WebClass::C),
            count(WebClass::D)
        ),
        // The audited 11, 9, 40, 36 and the two desktop-only color variant lessons in class C.
        (11, 9, 42, 36)
    );
}

#[test]
fn a_feature_is_named_by_every_b_lesson_and_by_no_a_or_c_lesson() {
    for lesson in WEB_LESSONS {
        match lesson.class {
            WebClass::B => assert!(lesson.needs.is_some(), "{} names no feature", lesson.id),
            WebClass::A | WebClass::C => {
                assert!(lesson.needs.is_none(), "{} names a feature", lesson.id);
            }
            WebClass::D => {}
        }
    }
}

#[test]
fn the_offer_counts_follow_the_features() {
    assert_eq!(web_offered_ids(WebFeatures::NONE).len(), 46);
    assert_eq!(web_offered_ids(WebFeatures::ALL).len(), 56);
    // The library: four class B lessons and the one class D lesson that needs an imported design.
    assert_eq!(web_offered_ids(only(WebFeature::Library)).len(), 46 + 5);
    assert_eq!(web_offered_ids(only(WebFeature::ConcaveEdit)).len(), 46 + 2);
    assert_eq!(web_offered_ids(only(WebFeature::DeepSolve)).len(), 46 + 1);
    assert_eq!(web_offered_ids(only(WebFeature::CuttingMode)).len(), 46 + 2);
}

#[test]
fn without_features_exactly_the_ungated_a_and_d_lessons_are_offered() {
    let offered: BTreeSet<&str> = web_offered_ids(WebFeatures::NONE).into_iter().collect();
    for lesson in WEB_LESSONS {
        let expected = matches!(lesson.class, WebClass::A | WebClass::D) && lesson.needs.is_none();
        assert_eq!(offered.contains(lesson.id), expected, "{}", lesson.id);
    }
    assert!(!offered.contains("tiers-adopt-imported-meets"));
    assert!(is_offered_on_web(
        "tiers-adopt-imported-meets",
        only(WebFeature::Library)
    ));
}

#[test]
fn a_class_c_lesson_is_never_offered() {
    for lesson in WEB_LESSONS
        .iter()
        .filter(|lesson| lesson.class == WebClass::C)
    {
        assert!(
            !is_offered_on_web(lesson.id, WebFeatures::ALL),
            "{}",
            lesson.id
        );
    }
}

#[test]
fn a_generated_lesson_follows_the_library_and_an_unknown_id_is_never_offered() {
    assert!(is_offered_on_web("build:some-design", WebFeatures::ALL));
    assert!(!is_offered_on_web("build:some-design", WebFeatures::NONE));
    assert!(!is_offered_on_web("no-such-lesson", WebFeatures::ALL));
    assert!(web_lesson("no-such-lesson").is_none());
}

#[test]
fn the_catalogue_hands_out_exactly_the_offered_lessons() {
    let catalogue = GuideCatalog::new();
    assert_eq!(catalogue.all().len(), WEB_LESSONS.len());
    assert_eq!(catalogue.web_guides(WebFeatures::NONE).len(), 46);
    assert_eq!(catalogue.web_guides(WebFeatures::ALL).len(), 56);
    assert!(
        catalogue
            .web_guide("optimizing-sweep", WebFeatures::ALL)
            .is_none()
    );
    assert!(
        catalogue
            .web_guide("optimizing-deep-solve", WebFeatures::NONE)
            .is_none()
    );
    assert!(
        catalogue
            .web_guide("optimizing-deep-solve", only(WebFeature::DeepSolve))
            .is_some()
    );
    assert!(
        catalogue
            .web_guide("no-such-lesson", WebFeatures::ALL)
            .is_none()
    );
}

#[test]
fn every_overlay_belongs_to_one_lesson_the_web_can_offer() {
    let mut seen = BTreeSet::new();
    for overlay in all_overlays() {
        assert!(
            seen.insert(overlay.guide.as_str()),
            "{} has two overlays",
            overlay.guide
        );
        let lesson = web_lesson(&overlay.guide)
            .unwrap_or_else(|| panic!("{} is not a lesson of the learning path", overlay.guide));
        assert_ne!(
            lesson.class,
            WebClass::C,
            "{} is never offered",
            overlay.guide
        );
    }
}

#[test]
fn every_reworded_and_every_feature_lesson_has_an_overlay() {
    let with_overlay: BTreeSet<&str> = all_overlays()
        .iter()
        .map(|overlay| overlay.guide.as_str())
        .collect();
    for lesson in WEB_LESSONS {
        if matches!(lesson.class, WebClass::B | WebClass::D) {
            assert!(
                with_overlay.contains(lesson.id),
                "{} has no overlay",
                lesson.id
            );
        }
    }
}

#[test]
fn every_part_of_every_overlay_still_matches_the_catalogue() {
    let catalogue = GuideCatalog::new();
    for overlay in all_overlays() {
        let guide = catalogue
            .get(&overlay.guide)
            .unwrap_or_else(|| panic!("the catalogue has no lesson {}", overlay.guide));
        let (_, unmatched) = web_adapt(guide);
        assert!(
            unmatched.is_empty(),
            "the overlay of {} is stale: {unmatched:#?}",
            overlay.guide
        );
    }
}

#[test]
fn the_desktop_view_of_every_guide_is_the_catalogue_guide() {
    let catalogue = GuideCatalog::new();
    for guide in catalogue.all() {
        assert_eq!(
            format!("{:?}", guide.for_platform(Platform::Desktop)),
            format!("{guide:?}"),
            "{} changed on the desktop",
            guide.id
        );
    }
    let desktop = catalogue.guides_for(Platform::Desktop);
    assert_eq!(
        format!("{desktop:?}"),
        format!("{:?}", catalogue.all()),
        "the desktop's guides are not the catalogue's"
    );
}

#[test]
fn building_the_web_views_leaves_the_catalogue_alone() {
    let catalogue = GuideCatalog::new();
    let before = format!("{:?}", catalogue.all());
    let web = catalogue.guides_for(Platform::Web);
    assert_eq!(web.len(), catalogue.all().len());
    assert_eq!(before, format!("{:?}", catalogue.all()));
}

#[test]
fn every_web_view_is_fit_to_run() {
    let catalogue = GuideCatalog::new();
    for guide in catalogue.guides_for(Platform::Web) {
        assert_eq!(guide.problem_on(Platform::Web), None, "{}", guide.id);
    }
}

#[test]
fn no_web_view_outlines_a_target_nobody_knows() {
    let catalogue = GuideCatalog::new();
    for guide in catalogue.guides_for(Platform::Web) {
        for step in &guide.steps {
            let target = step.highlight_target.as_str();
            assert!(
                HIGHLIGHT_TARGETS.contains(&target) || WEB_ONLY_TARGETS.contains(&target),
                "{} / {:?} outlines {target:?}",
                guide.id,
                step.title
            );
        }
    }
    for guide in catalogue.all() {
        for step in &guide.steps {
            assert!(
                !WEB_ONLY_TARGETS.contains(&step.highlight_target.as_str()),
                "the desktop lesson {} outlines a target only the web has",
                guide.id
            );
        }
    }
}

#[test]
fn no_offered_lesson_names_a_desktop_only_place_on_the_web() {
    let catalogue = GuideCatalog::new();
    for lesson in WEB_LESSONS
        .iter()
        .filter(|lesson| lesson.class != WebClass::C)
    {
        let guide = catalogue
            .guide_for(lesson.id, Platform::Web)
            .unwrap_or_else(|| panic!("the catalogue has no lesson {}", lesson.id));
        let problems = web_text_problems(&guide, lesson.needs == Some(WebFeature::Library));
        assert!(problems.is_empty(), "{problems:#?}");
    }
}

#[test]
fn the_desktop_guides_still_name_the_desktop_places() {
    // The phrases are real: the catalogue's own lessons do contain them.
    let catalogue = GuideCatalog::new();
    let tour = catalogue.get("welcome-tour").expect("the tour exists");
    assert!(!web_text_problems(tour, true).is_empty());
}

#[test]
fn a_step_can_be_omitted_and_a_lesson_keeps_the_rest() {
    let catalogue = GuideCatalog::new();
    let desktop = catalogue.get("welcome-tour").expect("the tour exists");
    let web = catalogue
        .guide_for("welcome-tour", Platform::Web)
        .expect("the tour exists");
    assert_eq!(desktop.steps.len(), 8);
    assert_eq!(web.steps.len(), 6);
    for gone in ["The library", "Simple and Advanced"] {
        assert!(
            web.steps.iter().all(|step| step.title != gone),
            "{gone} stayed"
        );
    }
    assert!(
        web.steps
            .iter()
            .any(|step| step.title == "Help and shortcuts")
    );
}

#[test]
fn an_impossible_goal_is_replaced_and_next_follows_the_new_goal() {
    let catalogue = GuideCatalog::new();
    let desktop = catalogue
        .get("viewing-solid-picking")
        .expect("the lesson exists");
    let web = catalogue
        .guide_for("viewing-solid-picking", Platform::Web)
        .expect("the lesson exists");
    let step = |guide: &crate::guide::Guide| {
        guide
            .steps
            .iter()
            .find(|step| step.title == "Pick in another view")
            .expect("the step exists")
            .clone()
    };
    assert!(matches!(step(desktop).goal, Goal::ViewMode(2)));
    let replaced = step(&web);
    assert!(matches!(replaced.goal, Goal::ViewMode(3)));
    assert!(replaced.perform.is_some(), "Next has nothing to do");
}

#[test]
fn a_highlight_target_can_be_aliased_and_overridden() {
    let catalogue = GuideCatalog::new();
    let targets = |guide: &crate::guide::Guide| -> Vec<String> {
        guide
            .steps
            .iter()
            .map(|step| step.highlight_target.clone())
            .collect()
    };
    let desktop = catalogue
        .get("viewing-custom-material")
        .expect("the lesson exists");
    let web = catalogue
        .guide_for("viewing-custom-material", Platform::Web)
        .expect("the lesson exists");
    assert!(targets(desktop).iter().any(|target| target == "stone_tab"));
    assert!(targets(&web).iter().all(|target| target != "stone_tab"));
    assert!(
        targets(&web)
            .iter()
            .any(|target| target == "design_settings")
    );

    let sheet = catalogue
        .guide_for("output-cutting-sheet", Platform::Web)
        .expect("the lesson exists");
    assert!(
        targets(&sheet)
            .iter()
            .any(|target| target == "schedule_cutting_sheet")
    );
    let png = catalogue
        .guide_for("output-diagram-png", Platform::Web)
        .expect("the lesson exists");
    assert!(
        targets(&png)
            .iter()
            .any(|target| target == "schedule_diagram")
    );
}

#[test]
fn a_step_about_the_render_tab_leaves_it_usable_on_the_web_only() {
    let catalogue = GuideCatalog::new();
    let allows_view_tabs = |guide: &crate::guide::Guide, title: &str| -> bool {
        guide
            .steps
            .iter()
            .find(|step| step.title == title)
            .expect("the step exists")
            .allow
            .contains(&Group::ViewTabs)
    };
    let desktop = catalogue
        .get("viewing-solid-picking")
        .expect("the lesson exists");
    let web = catalogue
        .guide_for("viewing-solid-picking", Platform::Web)
        .expect("the lesson exists");
    assert!(!allows_view_tabs(desktop, "A Ray-Traced picture"));
    assert!(allows_view_tabs(&web, "A Ray-Traced picture"));
    // A step that does not touch the Render tab keeps its lock.
    assert!(!allows_view_tabs(&web, "Hover over a facet"));
}

#[test]
fn the_snapshot_lesson_is_renamed_and_loses_the_compare_window() {
    let catalogue = GuideCatalog::new();
    let desktop = catalogue
        .get("solving-snapshot-compare")
        .expect("the lesson exists");
    let web = catalogue
        .guide_for("solving-snapshot-compare", Platform::Web)
        .expect("the lesson exists");
    assert_eq!(desktop.steps.len(), 5);
    assert_eq!(web.steps.len(), 3);
    assert_eq!(web.title, "Snapshot and Compare");
    assert_eq!(web.steps[0].title, "Take a Snapshot");
}

#[test]
fn the_diagram_lesson_drops_the_handle_step() {
    let catalogue = GuideCatalog::new();
    let desktop = catalogue.get("viewing-diagram").expect("the lesson exists");
    let web = catalogue
        .guide_for("viewing-diagram", Platform::Web)
        .expect("the lesson exists");
    assert_eq!(desktop.steps.len(), web.steps.len() + 1);
    assert!(
        web.steps
            .iter()
            .all(|step| step.title != "Drag a handle on the diagram")
    );
}
