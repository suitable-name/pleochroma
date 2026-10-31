//! The Shift stage, retained independently of the selected Optimize check.
//!
//! This is view-only geometry. Apply still reads the selected proposal and its guarded
//! check; keeping a Shift reference must never make an unselected Optimize result apply.

use super::check_run::PrevalidatedEdit;
use pleochroma_cut_core::Design;
use pleochroma_editor::retarget::check::RetargetCheck;
use std::cell::RefCell;

#[derive(Clone)]
struct ShiftSeed {
    source: Design,
    preview: ShiftPreview,
    generation: u64,
}

thread_local! {
    static SEED: RefCell<Option<ShiftSeed>> = const { RefCell::new(None) };
}

pub(super) fn clear() {
    SEED.with(|slot| *slot.borrow_mut() = None);
}

/// A certificate names valid geometry. An uncertified preview keeps its failing verdict
/// and can never authorize Apply; a check without geometry remains explicitly unavailable.
pub(super) fn install(
    generation: u64,
    source: &Design,
    check: &RetargetCheck,
    edit: Option<&PrevalidatedEdit>,
) {
    let seed = ShiftSeed {
        source: source.clone(),
        preview: ShiftPreview {
            candidate: check.candidate.clone(),
            validated: edit.is_some() && check.allows_apply() && check.candidate.is_some(),
            headline: check.validity.headline(),
        },
        generation,
    };
    SEED.with(|slot| *slot.borrow_mut() = Some(seed));
}

/// An older generation, or a different source design, never supplies a fallback baseline.
pub(super) fn preview(source: &Design, generation: u64) -> Option<ShiftPreview> {
    SEED.with(|slot| {
        slot.borrow()
            .as_ref()
            .filter(|seed| seed.generation == generation && &seed.source == source)
            .map(|seed| seed.preview.clone())
    })
}

#[derive(Clone)]
pub(super) struct ShiftPreview {
    pub(super) candidate: Option<Design>,
    pub(super) validated: bool,
    pub(super) headline: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::{ConstraintTier, Edit, PreformSpec, ScheduleMeta};
    use pleochroma_editor::retarget::{
        metrics::RetargetMetrics,
        validity::{InvalidReason, RetargetValidity, ValidityStatus},
    };

    fn design() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    fn check(candidate: Design) -> RetargetCheck {
        let mut validity = RetargetValidity::unchecked(&InvalidReason::NotClosed);
        validity.status = ValidityStatus::Valid;
        validity.reasons.clear();
        RetargetCheck {
            candidate: Some(candidate),
            baseline: None,
            validity,
            anchors: Vec::new(),
            metrics: RetargetMetrics::default(),
        }
    }

    #[test]
    fn picking_an_optimized_check_does_not_replace_the_shift_reference() {
        let source = design();
        let mut shifted = source.clone();
        shifted.tiers[0].angle_deg += 1.0;
        let certificate = (source.clone(), Edit::Batch(Vec::new()));
        install(7, &source, &check(shifted.clone()), Some(&certificate));

        let mut optimized = source.clone();
        optimized.tiers[0].angle_deg += 2.0;
        super::super::check_run::set_check_for_test(7, Some(check(optimized)));
        let shown = preview(&source, 7).expect("Shift survives candidate selection");
        assert_eq!(shown.candidate, Some(shifted));
        assert!(shown.validated);
        clear();
    }

    #[test]
    fn a_changed_source_or_generation_never_reuses_the_shift_side() {
        let source = design();
        let certificate = (source.clone(), Edit::Batch(Vec::new()));
        install(7, &source, &check(source.clone()), Some(&certificate));
        assert!(preview(&source, 8).is_none());
        let mut changed = source.clone();
        changed.tiers[0].angle_deg += 1.0;
        assert!(preview(&changed, 7).is_none());
        clear();
        assert!(preview(&source, 7).is_none());
    }

    #[test]
    fn an_uncertified_shift_is_never_labeled_validated() {
        let source = design();
        let certificate = (source.clone(), Edit::Batch(Vec::new()));
        let mut failed = check(source.clone());
        failed.candidate = None;
        // Establish source identity as the worker does through its baseline, then
        // exercise a delivered check with no usable Shift geometry.
        install(7, &source, &failed, Some(&certificate));
        let shown = preview(&source, 7).expect("source identified");
        assert!(shown.candidate.is_none());
        assert!(!shown.validated);
        // Buildable geometry without a certificate stays an unvalidated preview.
        install(7, &source, &check(source.clone()), None);
        let shown = preview(&source, 7).expect("uncertified preview");
        assert!(!shown.validated);
        clear();
    }
}
