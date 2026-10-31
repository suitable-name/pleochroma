//! The guided walkthrough's tab-session state.
//!
//! The walkthrough itself (its ten steps, the control groups each step leaves usable and
//! the goals that complete a step) is `pleochroma_editor::guide`, shared with the
//! desktop. What the browser adds is that a reload of the tab must not lose the reader's
//! place: [`GuideProgress`] is the little that survives -- whether the panel is open,
//! which step it is on, whether it is collapsed and where the floating panel was dragged
//! to -- stored as JSON under `sessionStorage["pleochroma.guide.v1"]`.
//!
//! Only the position is stored, never a "done" moment: a step is judged from the design
//! (`pleochroma_editor::guide::reached_completion`), and the restored design decides
//! again whether the restored step is already complete.

use pleochroma_editor::guide::STEPS;
use serde::{Deserialize, Serialize};

/// The `sessionStorage` key the page stores [`GuideProgress`] under.
pub const GUIDE_KEY: &str = "pleochroma.guide.v1";

/// Where the reader is in the walkthrough.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuideProgress {
    /// The panel is showing (and its locks are in force).
    pub open: bool,
    /// The current step, 0-based into `pleochroma_editor::guide::STEPS`.
    pub step: usize,
    /// The panel is collapsed to its "Guide - Step N of M" pill.
    pub collapsed: bool,
    /// The floating panel was dragged; [`Self::float_x`] and [`Self::float_y`] hold the spot.
    pub float_placed: bool,
    /// The dragged panel's left edge, logical pixels.
    pub float_x: f32,
    /// The dragged panel's top edge, logical pixels.
    pub float_y: f32,
}

impl GuideProgress {
    /// Parses a stored value and repairs it: a step past the end is the last step, and a
    /// position that is not a finite, non-negative pair counts as never dragged.
    ///
    /// # Errors
    ///
    /// The JSON parser's message (the caller discards the stored entry).
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str::<Self>(text)
            .map(Self::sanitized)
            .map_err(|e| e.to_string())
    }

    /// The JSON to store.
    ///
    /// # Errors
    ///
    /// The serializer's message (in practice unreachable for these types).
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| e.to_string())
    }

    /// `self` with every field in range.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.step = self.step.min(STEPS.len().saturating_sub(1));
        let sane = |v: f32| v.is_finite() && v >= 0.0;
        if !(sane(self.float_x) && sane(self.float_y)) {
            self.float_placed = false;
            self.float_x = 0.0;
            self.float_y = 0.0;
        }
        self
    }
}

/// The walkthrough lines reworded for the browser: `(desktop line, web line)`.
///
/// They are the lines of the shared walkthrough that name a desktop-only control or place.
/// The shared `pleochroma_editor::guide::STEPS` stay the single source of the steps, their
/// locks and their goals; only these sentences differ. A test pins that every desktop
/// line still exists, so rewording one in the shared crate cannot silently drop its
/// override.
///
/// This table serves the ten-step runner of today. The shared crate now carries the same wording
/// for every lesson as a per-step overlay (`pleochroma_editor::guide::Guide::for_platform` with
/// `Platform::Web`), and a test below pins that the overlay of the walkthrough holds every pair of
/// this table, so the catalogue runner can replace this table without changing a word.
pub const WEB_WORDING: &[(&str, &str)] = &[
    (
        "Choose File > New Design... in the command bar (or in the File menu at the top of the window).",
        "Click New Design... on the start page (or File > New Design..., or Alt+N).",
    ),
    (
        "Read the status strip at the bottom of the Edit tab.",
        "Read the status strip at the bottom of the window.",
    ),
    (
        "Open the Stone tab of the Parameters dock (View > Stone Properties, or Ctrl+Alt+1).",
        "In Design Settings, set Material: Diamond.",
    ),
    (
        "Set Material: Diamond. It applies at once.",
        "Click Apply Material.",
    ),
    (
        "The carat estimate uses the specific gravity of the material set in the Stone tab unless you type an override. The Girdle Diameter is the same value as the Stone tab's Stone Width; it only changes how deep a colored stone's color looks, so a colorless Diamond looks the same. Click Skip Step if you do not need real units.",
        "The carat estimate uses the specific gravity of the material set in Design Settings unless you type an override; neither changes the render. Click Skip Step if you do not need real units.",
    ),
    (
        "Switch to Live Render to see the stone rendered in Diamond.",
        "Switch to the Render tab to see the stone rendered in Diamond. Render Settings offers Path-Traced or the faster, deterministic Ray-Traced view.",
    ),
    (
        "Save it with Save (Ctrl+S), or keep editing.",
        "Save it with File > Save design (Ctrl+S), or keep editing.",
    ),
    (
        "Continue with Chapter 8 of the manual: Deep Solve, Optimize, Adopt and Apply.",
        "Try Design > Optimize angles and Retarget for material, or press ? for the keyboard shortcuts.",
    ),
    (
        "Every control is unlocked again. Reopen this guide any time from Help > Guide: New Design Walkthrough.",
        "Every control is unlocked again. Reopen this guide any time from Help > Guided walkthrough.",
    ),
];

/// `desktop` as the browser build words it: its [`WEB_WORDING`] replacement, or `desktop`
/// itself when the line needs none.
#[must_use]
pub fn web_wording(desktop: &str) -> &str {
    WEB_WORDING
        .iter()
        .find(|(from, _)| *from == desktop)
        .map_or(desktop, |(_, to)| to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_editor::guide::{
        Guide, GuideCatalog, Platform, WEB_LESSONS, WORKED_EXAMPLE_ID, WebClass, WebFeature,
        WebFeatures, web_text_problems,
    };

    /// Every text a learner reads on `guide`'s steps.
    fn step_lines(guide: &Guide) -> Vec<String> {
        let mut lines = Vec::new();
        for step in &guide.steps {
            lines.extend(step.actions.iter().cloned());
            lines.push(step.intro.clone());
            lines.push(step.why.clone());
            lines.push(step.check.clone());
        }
        lines
    }

    #[test]
    fn the_overlay_of_the_walkthrough_carries_every_web_wording_pair() {
        let catalogue = GuideCatalog::new();
        let desktop = catalogue
            .guide_for(WORKED_EXAMPLE_ID, Platform::Desktop)
            .expect("the catalogue has the walkthrough");
        let web = catalogue
            .guide_for(WORKED_EXAMPLE_ID, Platform::Web)
            .expect("the catalogue has the walkthrough");
        assert_eq!(desktop.steps.len(), web.steps.len(), "no step is omitted");
        let (desktop_lines, web_lines) = (step_lines(&desktop), step_lines(&web));
        for (from, to) in WEB_WORDING {
            assert!(
                desktop_lines.iter().any(|line| line == from),
                "the desktop walkthrough lost {from:?}"
            );
            assert!(
                web_lines.iter().any(|line| line == to),
                "the web overlay of the walkthrough lost {to:?}"
            );
            assert!(
                !web_lines.iter().any(|line| line == from),
                "the web overlay still says {from:?}"
            );
        }
    }

    #[test]
    fn no_offered_lesson_names_a_desktop_only_place_on_the_web() {
        let catalogue = GuideCatalog::new();
        let mut checked = 0;
        for lesson in WEB_LESSONS
            .iter()
            .filter(|lesson| lesson.class != WebClass::C)
        {
            let guide = catalogue
                .web_guide(lesson.id, WebFeatures::ALL)
                .unwrap_or_else(|| panic!("{} is offered but not in the catalogue", lesson.id));
            let problems = web_text_problems(&guide, lesson.needs == Some(WebFeature::Library));
            assert!(problems.is_empty(), "{problems:#?}");
            checked += 1;
        }
        assert_eq!(checked, 56, "the offer is A + B + D lessons");
    }

    #[test]
    fn the_web_walkthrough_text_is_clean_even_before_the_old_pin_looks() {
        let catalogue = GuideCatalog::new();
        let web = catalogue
            .web_guide(WORKED_EXAMPLE_ID, WebFeatures::NONE)
            .expect("the walkthrough is offered without any feature");
        assert!(web_text_problems(&web, false).is_empty());
    }

    #[test]
    fn every_reworded_line_still_exists_in_the_shared_steps() {
        let mut lines: Vec<&str> = Vec::new();
        for step in STEPS {
            lines.extend(step.actions.iter().copied());
            lines.push(step.why);
            lines.push(step.intro);
            lines.push(step.check);
        }
        for (from, to) in WEB_WORDING {
            assert!(
                lines.contains(from),
                "the shared walkthrough no longer contains {from:?}: update WEB_WORDING"
            );
            assert_ne!(from, to);
            assert_eq!(web_wording(from), *to);
        }
        assert_eq!(web_wording("Click Create."), "Click Create.");
    }

    #[test]
    fn the_web_walkthrough_names_no_desktop_only_places() {
        for step in STEPS {
            for line in step.actions.iter().chain([&step.why, &step.intro]) {
                let text = web_wording(line);
                for desktop_only in [
                    "Live Render",
                    "Save (Ctrl+S)",
                    "Chapter 8",
                    "command bar (or File",
                    "Yield Material",
                ] {
                    assert!(
                        !text.contains(desktop_only),
                        "step {:?} still says {desktop_only:?}: {text:?}",
                        step.title
                    );
                }
            }
        }
    }

    #[test]
    fn a_fresh_guide_is_closed_at_the_first_step() {
        let fresh = GuideProgress::default();
        assert!(!fresh.open && !fresh.collapsed && !fresh.float_placed);
        assert_eq!(fresh.step, 0);
        assert_eq!(GuideProgress::from_json("{}"), Ok(fresh));
    }

    #[test]
    fn progress_round_trips_through_json() {
        let progress = GuideProgress {
            open: true,
            step: 4,
            collapsed: true,
            float_placed: true,
            float_x: 512.5,
            float_y: 96.0,
        };
        let json = progress.to_json().expect("writes");
        assert_eq!(GuideProgress::from_json(&json), Ok(progress));
    }

    #[test]
    fn a_stored_step_past_the_end_or_a_bad_position_is_repaired() {
        let far = GuideProgress::from_json(r#"{"open":true,"step":999}"#).expect("parses");
        assert_eq!(far.step, STEPS.len() - 1);
        assert!(far.open);
        let lost =
            GuideProgress::from_json(r#"{"float_placed":true,"float_x":-5.0,"float_y":10.0}"#)
                .expect("parses");
        assert!(!lost.float_placed);
        assert_eq!((lost.float_x, lost.float_y), (0.0, 0.0));
        assert!(GuideProgress::from_json("not json").is_err());
        assert!(GuideProgress::from_json(r#"{"step":-1}"#).is_err());
    }
}
