//! Which lessons the browser build offers, and how it words them.
//!
//! The catalogue ([`super::GuideCatalog`]) holds every built-in lesson once, worded for the
//! desktop. The browser build differs in two ways, and both are data here, so neither changes the
//! catalogue or the desktop's lessons:
//!
//! - **The offer.** [`WEB_LESSONS`] classifies every lesson of the learning path
//!   ([`super::CURRICULUM`]) once, from the lesson audit of the limitations campaign:
//!   `A` runs as it is, `D` runs once reworded, `B` needs a web feature that the campaign builds
//!   (a [`WebFeature`], passed in as [`WebFeatures`]), `C` is not offered. [`is_offered_on_web`]
//!   is the one predicate.
//! - **The wording.** `overlay` holds, per lesson and step, the web's replacement lines, omitted
//!   steps, replaced goals, highlight aliases and lock-group fixes. [`Guide::for_platform`] with
//!   [`Platform::Web`] applies it; with [`Platform::Desktop`] it returns the guide unchanged.
//!
//! The pin that keeps the overlay honest is [`web_text_problems`]: no line a web learner reads may
//! name a place only the desktop has ([`DESKTOP_ONLY_PHRASES`]).

mod data_features;
mod data_solving;
mod data_tiers;
mod data_viewing;
mod overlay;
#[cfg(test)]
mod tests;

pub use overlay::{
    GoalFix, GuideOverlay, StepOverlay, TextEdit, WEB_ONLY_TARGETS, WEB_TEXT_ONLY_TARGETS,
    web_adapt, web_target, web_view,
};

use super::{BUILD_ID_PREFIX, Guide, GuideCatalog, Platform};

/// A web feature that the limitations campaign builds, and that some lessons wait for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WebFeature {
    /// The web library: lessons about importing, searching, filtering, loading, variants and a
    /// design's own lighting, and the one lesson that needs an imported design.
    Library,
    /// The concave tier form and its table button.
    ConcaveEdit,
    /// The Deep Solve button and its report.
    DeepSolve,
    /// Cutting mode, and the Cut slider rework that goes with it.
    CuttingMode,
}

/// Which [`WebFeature`]s the running browser build has. The web passes this in; the offer follows it.
#[expect(
    clippy::struct_excessive_bools,
    reason = "four independent feature switches, which the tests build with struct-update syntax"
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WebFeatures {
    /// The web library exists.
    pub library: bool,
    /// Concave tiers can be added and edited.
    pub concave_edit: bool,
    /// Deep Solve exists.
    pub deep_solve: bool,
    /// Cutting mode exists.
    pub cutting_mode: bool,
}

impl WebFeatures {
    /// The build as it is before any of the four features lands.
    pub const NONE: Self = Self {
        library: false,
        concave_edit: false,
        deep_solve: false,
        cutting_mode: false,
    };

    /// The build once all four features have landed.
    pub const ALL: Self = Self {
        library: true,
        concave_edit: true,
        deep_solve: true,
        cutting_mode: true,
    };

    /// Whether `feature` is present.
    #[must_use]
    pub const fn has(self, feature: WebFeature) -> bool {
        match feature {
            WebFeature::Library => self.library,
            WebFeature::ConcaveEdit => self.concave_edit,
            WebFeature::DeepSolve => self.deep_solve,
            WebFeature::CuttingMode => self.cutting_mode,
        }
    }
}

/// How a lesson stands against the browser build (the classes of the lesson audit).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WebClass {
    /// Runs as written: its text is neutral and every control it names exists on the web.
    A,
    /// Needs a [`WebFeature`] that the campaign builds, and is offered once the web has it.
    B,
    /// Blocked by something the web does not have and no lane builds: never offered.
    C,
    /// Runs on the web's existing controls once the overlay rewords it.
    D,
}

/// One row of the offer table: a lesson of the learning path and where it stands on the web.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WebLesson {
    /// The lesson id, as in [`super::CURRICULUM`].
    pub id: &'static str,
    /// The audit class.
    pub class: WebClass,
    /// The feature the lesson waits for. Every class `B` lesson has one; a class `D` lesson may
    /// have one too (it is reworded, but only makes sense once the feature exists).
    pub needs: Option<WebFeature>,
}

const fn run(id: &'static str) -> WebLesson {
    WebLesson {
        id,
        class: WebClass::A,
        needs: None,
    }
}

const fn feature(id: &'static str, needs: WebFeature) -> WebLesson {
    WebLesson {
        id,
        class: WebClass::B,
        needs: Some(needs),
    }
}

const fn blocked(id: &'static str) -> WebLesson {
    WebLesson {
        id,
        class: WebClass::C,
        needs: None,
    }
}

const fn reworded(id: &'static str) -> WebLesson {
    WebLesson {
        id,
        class: WebClass::D,
        needs: None,
    }
}

const fn reworded_after(id: &'static str, needs: WebFeature) -> WebLesson {
    WebLesson {
        id,
        class: WebClass::D,
        needs: Some(needs),
    }
}

/// Every lesson of the learning path, once, in browser order, with its place on the web.
///
/// The classes come from the lesson audit (`T0_lesson_audit.md`): 11 `A`, 9 `B`, 40 `C`, 36 `D`,
/// and the two color variant lessons (2026-10-27), which are desktop-only: 42 `C` in all.
/// The variants lesson and a design's own lighting moved from `B` to `C` (2026-10-26): the browser
/// stores both but has no screen for them. One `D` lesson, adopting imported meets, needs a design
/// to adopt from; with no library to load one from it is offered only once the library exists, so
/// no features offers 46 lessons and all four offers 56.
pub const WEB_LESSONS: &[WebLesson] = &[
    // Getting Started
    reworded("welcome-tour"),
    run("new-design-walkthrough"),
    blocked("app-help-viewer"),
    blocked("app-glossary"),
    // Adding and Editing Tiers
    run("tiers-add-a-tier"),
    run("tiers-edit-a-tier"),
    reworded("tiers-quick-add"),
    run("tiers-inline-angle"),
    run("tiers-index-shorthands"),
    reworded("tiers-facet-chips"),
    reworded("tiers-tier-notes"),
    // Arranging and Repeating Tiers
    reworded("tiers-duplicate"),
    run("tiers-move"),
    reworded("tiers-delete"),
    reworded("tiers-multi-select"),
    reworded("tiers-step-series"),
    blocked("tiers-linked-series"),
    run("tiers-mirror-to-other-block"),
    // Meets, Relations and Concave Tiers
    run("tiers-meets-named-facets"),
    run("tiers-meets-mm-targets"),
    run("tiers-pin-to-mast"),
    reworded_after("tiers-adopt-imported-meets", WebFeature::Library),
    blocked("tiers-relations"),
    reworded("tiers-arithmetic"),
    reworded("tiers-cheater-offset"),
    feature("tiers-concave-cylinder-cone", WebFeature::ConcaveEdit),
    feature("tiers-concave-sphere-disc", WebFeature::ConcaveEdit),
    // Solving and Checking
    reworded("solving-solve"),
    run("solving-auto-solve"),
    reworded("solving-status"),
    reworded("solving-verdict"),
    reworded("solving-verdict-fixes"),
    reworded("solving-preform"),
    reworded("solving-yield-carat"),
    // Improving a Design
    feature("optimizing-deep-solve", WebFeature::DeepSolve),
    blocked("optimizing-optimize"),
    reworded("optimizing-retarget-shift"),
    reworded("optimizing-retarget-optimize"),
    blocked("optimizing-sweep"),
    blocked("optimizing-sweep-two-tiers"),
    blocked("solving-trials"),
    blocked("optimizing-tune-by-hand"),
    // History, Variants and Comparing
    reworded("solving-history"),
    // No Variants view in the browser (the store exists, the screen does not).
    blocked("solving-variants"),
    reworded("solving-snapshot-compare"),
    blocked("solving-raw-text"),
    // Materials and Color
    reworded("viewing-custom-material"),
    blocked("viewing-material-coefficients"),
    reworded("viewing-stone-effects"),
    // The browser has the Body Color list and the Custom editor, but no Material Editor with a
    // Color Variants section and no Color Pattern window: both color variant lessons end there.
    blocked("materials-color-variants"),
    blocked("viewing-color-pattern"),
    blocked("materials-zone-colors-from-variants"),
    reworded("viewing-ametrine"),
    blocked("materials-orient-sapphire"),
    blocked("materials-tourmaline-axis-in-table"),
    blocked("materials-best-blue-tanzanite"),
    blocked("materials-set-crystal-axis"),
    blocked("materials-polish-quality"),
    blocked("materials-scratch-direction"),
    blocked("materials-silk-sapphire"),
    blocked("materials-opal-play-of-color"),
    blocked("materials-labradorite-schiller"),
    blocked("materials-axis-curves"),
    // Working in the Viewport
    reworded("viewing-solid-picking"),
    reworded("viewing-diagram"),
    feature("viewing-cut-slider", WebFeature::CuttingMode),
    reworded("viewing-drag-handles"),
    reworded("viewing-slice"),
    // Rendering and Lighting
    reworded("viewing-live-render"),
    // No "Use This Lighting for This Design" / "Forget" buttons in the browser.
    blocked("viewing-design-lighting"),
    blocked("viewing-optical-diagnostics"),
    blocked("output-render-jobs"),
    blocked("output-render-jobs-queue"),
    // Saving, Cutting and Exporting
    reworded("output-save"),
    reworded("output-open"),
    feature("output-cutting-mode", WebFeature::CuttingMode),
    reworded("output-export-asc"),
    reworded("output-export-gcs"),
    reworded("output-cutting-sheet"),
    reworded("output-diagram-png"),
    // The Library
    feature("library-import", WebFeature::Library),
    feature("library-search", WebFeature::Library),
    feature("library-filters", WebFeature::Library),
    feature("library-load-design", WebFeature::Library),
    // Planning from the Rough
    blocked("library-rough-planner"),
    blocked("library-planner-model-rough"),
    blocked("library-planner-results"),
    blocked("library-planner-saved-plans"),
    blocked("library-planner-open-in-editor"),
    blocked("library-rough-color-wizard"),
    blocked("library-rough-color-in-planner"),
    blocked("planning-override-orientation"),
    // The App and Preferences
    blocked("prefs-interface-mode"),
    blocked("prefs-interface-scale"),
    blocked("prefs-high-contrast"),
    blocked("prefs-larger-handles"),
    blocked("app-command-palette"),
    reworded("app-keyboard-shortcuts"),
];

/// The row of lesson `id`, or `None` for an id the learning path does not list (a generated
/// lesson, for one).
#[must_use]
pub fn web_lesson(id: &str) -> Option<&'static WebLesson> {
    WEB_LESSONS.iter().find(|lesson| lesson.id == id)
}

/// Whether the browser build offers lesson `id` given the `features` it has.
///
/// Class `A` and `D` lessons are offered at once (a `D` lesson that waits for a feature, once the
/// feature exists), class `B` lessons once their feature exists, class `C` never. A generated
/// "Build this design" lesson is offered when the library exists, since it rebuilds a library
/// design. An id nothing knows is not offered.
#[must_use]
pub fn is_offered_on_web(id: &str, features: WebFeatures) -> bool {
    web_lesson(id).map_or_else(
        || id.starts_with(BUILD_ID_PREFIX) && features.library,
        |lesson| {
            lesson.class != WebClass::C && lesson.needs.is_none_or(|needed| features.has(needed))
        },
    )
}

/// The ids of the built-in lessons the browser build offers given `features`, in browser order.
#[must_use]
pub fn web_offered_ids(features: WebFeatures) -> Vec<&'static str> {
    WEB_LESSONS
        .iter()
        .filter(|lesson| is_offered_on_web(lesson.id, features))
        .map(|lesson| lesson.id)
        .collect()
}

impl GuideCatalog {
    /// The lessons the browser build offers given `features`, each with the web overlay applied,
    /// in catalogue order. Generated lessons are included when the library exists.
    #[must_use]
    pub fn web_guides(&self, features: WebFeatures) -> Vec<Guide> {
        self.all()
            .iter()
            .filter(|guide| is_offered_on_web(&guide.id, features))
            .map(|guide| guide.for_platform(Platform::Web))
            .collect()
    }

    /// The lesson `id` as the browser build shows it, or `None` when the catalogue has no such
    /// lesson or the build does not offer it given `features`.
    #[must_use]
    pub fn web_guide(&self, id: &str, features: WebFeatures) -> Option<Guide> {
        if !is_offered_on_web(id, features) {
            return None;
        }
        self.guide_for(id, Platform::Web)
    }
}

/// Phrases that name a place only the desktop has. No line of a lesson the web offers may contain
/// one; [`web_text_problems`] checks it.
pub const DESKTOP_ONLY_PHRASES: &[&str] = &[
    "Stone tab",
    "Parameters dock",
    "Ctrl+Alt",
    "Live Render",
    "Edit tab",
    "Baseline",
    "Simple interface",
    "Advanced interface",
    "(Advanced)",
    "Ctrl+K",
    "palette",
    "Preferences",
    "Tutorials...",
    "User Manual",
    "Load Selected",
    "Save dialog",
    "Save As",
    "Abandon",
    "activity chip",
    "Compare Visually",
    "compare window",
    "Chapter ",
    "Open Recent",
    ".bak",
    "Recover Unsaved",
    "Ctrl+N",
    "Ctrl+E",
    "Ctrl+F",
    "Ctrl+,",
    "File in the command bar",
    "command bar's",
    "menu of the command bar",
    "Rough Planner",
    "Material Editor",
    "New/Edit",
    "Preview As",
    "Lighting & Environment",
    "Reset This Tab",
    "Stone Properties",
    "Stone Width",
    "Edit as Text",
    "Export .asc",
    "Export Cutting Sheet",
    "Export Diagram",
    "Export as Gem",
];

/// What is wrong with the words of `guide` as the web shows them.
///
/// One sentence for every line that contains a [`DESKTOP_ONLY_PHRASES`] entry, and, unless
/// `allow_library`, for every line that mentions the library (a lesson that does not wait for
/// the library cannot send the learner there).
///
/// `guide` is the guide as the web shows it, so pass [`web_view`]'s result. Empty means clean.
#[must_use]
pub fn web_text_problems(guide: &Guide, allow_library: bool) -> Vec<String> {
    let mut lines: Vec<(String, &str)> = vec![
        (guide.title.clone(), "the title"),
        (guide.summary.clone(), "the summary"),
    ];
    for step in &guide.steps {
        lines.push((step.title.clone(), "a step title"));
        lines.push((step.intro.clone(), "an intro"));
        lines.extend(step.actions.iter().map(|a| (a.clone(), "an action")));
        lines.push((step.check.clone(), "a look-for line"));
        lines.push((step.why.clone(), "an explanation"));
        lines.push((step.waiting.clone(), "a waiting-for line"));
    }
    let mut problems = Vec::new();
    for (text, what) in &lines {
        for phrase in DESKTOP_ONLY_PHRASES {
            if text.contains(phrase) {
                problems.push(format!("{}: {what} names {phrase:?}: {text:?}", guide.id));
            }
        }
        if !allow_library && text.contains("ibrary") {
            problems.push(format!(
                "{}: {what} mentions the library: {text:?}",
                guide.id
            ));
        }
    }
    problems
}
