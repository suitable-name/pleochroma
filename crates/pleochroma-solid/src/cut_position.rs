//! Where the Cut slider stands and what it is called, for the desktop and the web alike.
//!
//! The slider walks a design's cutting steps (`Design::preview_steps`): position `0`
//! is the rough (the preform alone, whatever its shape: block, cylinder or a planned
//! piece), position `k` the stone after the first `k` steps,
//! and the last position the finished design. Flat and concave tiers each count as one
//! step, in the order the cutting sheet lists them for every design
//! (`Design::cutting_order`: the pavilion and girdle tiers, the crown, the table last).
//!
//! The model's `tier_cutoff` is the one source of truth, in these terms:
//! [`MODEL_FINISHED`] (`-1`) for the finished design, [`MODEL_ROUGH`] (`-2`) for the
//! rough, and `n >= 0` for the stone after steps `0..=n` -- the first `n + 1` steps of the
//! cutting order, which the Slice tool reads. [`CutPosition`] decodes it;
//! everything else here is derived from the position and the design, so the label, the
//! slider handle, the planner and every full redraw cannot disagree.
//!
//! Everything here is a pure function of a position and a design: no window, no thread.

use pleochroma_cut_core::{Design, design::TierRef, is_legacy_123_abc};

/// The Cut slider's `tier_cutoff` for the finished design.
pub const MODEL_FINISHED: i32 = -1;

/// The Cut slider's `tier_cutoff` for the rough, before any tier is cut.
pub const MODEL_ROUGH: i32 = -2;

/// Where the Cut slider sits for a design with a given number of steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CutPosition {
    /// The whole design: the slider at its right end.
    #[default]
    Finished,
    /// The preform alone, before any tier is cut: the slider at its left end.
    Rough,
    /// The stone after this many steps; always at least `1` and below the step count.
    After(usize),
}

impl CutPosition {
    /// The position "after `steps` steps" of a design with `step_count` steps: the
    /// rough for none, the finished design for all of them or more.
    #[must_use]
    pub const fn from_steps(steps: usize, step_count: usize) -> Self {
        if steps >= step_count {
            Self::Finished
        } else if steps == 0 {
            Self::Rough
        } else {
            Self::After(steps)
        }
    }

    /// Decodes the model's `tier_cutoff` for a design with `step_count` steps.
    /// Anything below [`MODEL_ROUGH`] reads as finished, as does a stale cutoff past the
    /// last step (a tier was removed since it was set).
    #[must_use]
    pub fn from_model(tier_cutoff: i32, step_count: usize) -> Self {
        match tier_cutoff {
            MODEL_ROUGH => Self::from_steps(0, step_count),
            through if through >= 0 => usize::try_from(through).map_or(Self::Finished, |through| {
                Self::from_steps(through.saturating_add(1), step_count)
            }),
            _ => Self::Finished,
        }
    }

    /// The value the model's `tier_cutoff` holds for this position.
    #[must_use]
    pub fn to_model(self) -> i32 {
        match self {
            Self::Finished => MODEL_FINISHED,
            Self::Rough => MODEL_ROUGH,
            Self::After(steps) => i32::try_from(steps).map_or(MODEL_FINISHED, |steps| steps - 1),
        }
    }

    /// How many steps are cut: `None` for the finished design (nothing is truncated),
    /// `Some(0)` for the rough.
    #[must_use]
    pub const fn steps(self) -> Option<usize> {
        match self {
            Self::Finished => None,
            Self::Rough => Some(0),
            Self::After(steps) => Some(steps),
        }
    }

    /// The slider handle's value: `0` for the rough up to `step_count` for the finished
    /// design.
    #[must_use]
    pub const fn slider_value(self, step_count: usize) -> f32 {
        match self {
            Self::Finished => step_count as f32,
            Self::Rough => 0.0,
            Self::After(steps) => steps as f32,
        }
    }

    /// The position a slider handle value stands for: rounded to the nearest step.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is rounded and clamped to >= 0 first; from_steps clamps to the step count (a saturating cast)"
    )]
    pub const fn from_slider_value(value: f32, step_count: usize) -> Self {
        Self::from_steps(value.round().max(0.0) as usize, step_count)
    }
}

/// What the slider calls `step`.
///
/// The tier's code as the cutting sheet's label column shows it (`G1`, `P2`, `C3`, `T`, a
/// concave tier's `P4`), then its own name when that says more (`C2 Crown Main`). A tier with
/// no name, an old-style `123`/`ABC` one or a name equal to its code reads as the code alone. A
/// tier the design does not have reads "tier N" (flat) or "Concave tier N".
#[must_use]
pub fn step_name(design: &Design, step: TierRef) -> String {
    let codes = design.tier_codes();
    let (code, name, missing) = match step {
        TierRef::Flat(index) => (
            codes.flat.get(index).map(|label| label.code.as_str()),
            design.tiers.get(index).map(|tier| tier.name.trim()),
            format!("tier {}", index + 1),
        ),
        TierRef::Concave(index) => (
            codes.concave.get(index).map(|label| label.code.as_str()),
            design.concave_tiers.get(index).map(|tier| tier.name.trim()),
            format!("Concave tier {}", index + 1),
        ),
    };
    match (code.filter(|code| !code.is_empty()), name) {
        (Some(code), Some(name))
            if name.is_empty() || is_legacy_123_abc(name) || name.eq_ignore_ascii_case(code) =>
        {
            code.to_owned()
        }
        (Some(code), Some(name)) => format!("{code} {name}"),
        _ => missing,
    }
}

/// The slider's label for `position`: "Rough", "After <tier> (k of N)" or "Finished".
///
/// The tier is written as [`step_name`] gives it, `After C2 Crown Main (6 of 8)`. A position at
/// or past the design's last step reads "Finished".
#[must_use]
pub fn cut_label(design: &Design, position: CutPosition) -> String {
    match position {
        CutPosition::Finished => "Finished".to_string(),
        CutPosition::Rough => "Rough".to_string(),
        CutPosition::After(steps) => {
            let order = design.preview_steps();
            // `from_steps` never builds an `After` at or past the step count, but the label
            // agrees with it anyway: the last step is the finished design.
            if steps >= order.len() {
                return "Finished".to_string();
            }
            steps
                .checked_sub(1)
                .and_then(|last| order.get(last))
                .map_or_else(
                    || "Finished".to_string(),
                    |&step| {
                        format!(
                            "After {} ({steps} of {})",
                            step_name(design, step),
                            order.len()
                        )
                    },
                )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta, compute_tier_labels};

    fn round_brilliant() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    #[test]
    fn the_model_encoding_round_trips_for_every_position() {
        for step_count in [1_usize, 2, 7, 12] {
            let mut positions = vec![CutPosition::Rough, CutPosition::Finished];
            positions.extend((1..step_count).map(CutPosition::After));
            for position in positions {
                assert_eq!(
                    CutPosition::from_model(position.to_model(), step_count),
                    position,
                    "{position:?} of {step_count}"
                );
            }
        }
    }

    #[test]
    fn the_model_values_mean_what_the_slice_tool_reads() {
        // `-1` is the whole design, `n >= 0` is "through step n": the Slice tool's
        // `cut_hides_tier` compares exactly that.
        assert_eq!(CutPosition::from_model(-1, 8), CutPosition::Finished);
        assert_eq!(CutPosition::from_model(-2, 8), CutPosition::Rough);
        assert_eq!(CutPosition::from_model(0, 8), CutPosition::After(1));
        assert_eq!(CutPosition::from_model(5, 8), CutPosition::After(6));
        assert_eq!(CutPosition::After(6).to_model(), 5);
    }

    #[test]
    fn a_cutoff_past_the_last_step_or_below_rough_is_finished() {
        assert_eq!(CutPosition::from_model(7, 8), CutPosition::Finished);
        assert_eq!(CutPosition::from_model(99, 8), CutPosition::Finished);
        assert_eq!(CutPosition::from_model(-3, 8), CutPosition::Finished);
        assert_eq!(CutPosition::from_model(i32::MIN, 8), CutPosition::Finished);
    }

    #[test]
    fn a_design_without_steps_has_only_a_finished_position() {
        assert_eq!(
            CutPosition::from_model(MODEL_ROUGH, 0),
            CutPosition::Finished
        );
        assert_eq!(CutPosition::from_model(0, 0), CutPosition::Finished);
        assert_eq!(CutPosition::from_steps(0, 0), CutPosition::Finished);
    }

    #[test]
    fn steps_are_none_for_finished_and_zero_for_the_rough() {
        assert_eq!(CutPosition::Finished.steps(), None);
        assert_eq!(CutPosition::Rough.steps(), Some(0));
        assert_eq!(CutPosition::After(4).steps(), Some(4));
    }

    #[test]
    fn the_slider_spans_rough_to_finished_in_whole_steps() {
        let count = 8;
        assert_eq!(CutPosition::Rough.slider_value(count), 0.0);
        assert_eq!(CutPosition::After(3).slider_value(count), 3.0);
        assert_eq!(CutPosition::Finished.slider_value(count), 8.0);
        for step in 0..=count {
            let value = CutPosition::from_steps(step, count).slider_value(count);
            assert_eq!(
                CutPosition::from_slider_value(value, count),
                CutPosition::from_steps(step, count)
            );
        }
    }

    #[test]
    fn a_slider_value_rounds_to_the_nearest_step_and_clamps() {
        assert_eq!(
            CutPosition::from_slider_value(2.4, 8),
            CutPosition::After(2)
        );
        assert_eq!(
            CutPosition::from_slider_value(2.6, 8),
            CutPosition::After(3)
        );
        assert_eq!(CutPosition::from_slider_value(-3.0, 8), CutPosition::Rough);
        assert_eq!(CutPosition::from_slider_value(0.4, 8), CutPosition::Rough);
        assert_eq!(
            CutPosition::from_slider_value(7.6, 8),
            CutPosition::Finished
        );
        assert_eq!(
            CutPosition::from_slider_value(50.0, 8),
            CutPosition::Finished
        );
        assert_eq!(
            CutPosition::from_slider_value(f32::NAN, 8),
            CutPosition::Rough
        );
    }

    #[test]
    fn labels_name_the_last_cut_tier_and_count_the_steps() {
        let design = round_brilliant();
        let count = design.preview_step_count();
        assert_eq!(cut_label(&design, CutPosition::Rough), "Rough");
        assert_eq!(cut_label(&design, CutPosition::Finished), "Finished");
        // The fixture is stored top-down, the slider walks the cutting order: the girdle is
        // the first step, the upper girdle the last but one (the table is last).
        let order = design.preview_steps();
        let first = step_name(&design, order[0]);
        assert_eq!(first, "G1 Girdle");
        assert_eq!(
            cut_label(&design, CutPosition::After(1)),
            "After G1 Girdle (1 of 8)"
        );
        let last_but_one = step_name(&design, order[count - 2]);
        assert_eq!(last_but_one, "C3 Upper Girdle");
        assert_eq!(
            cut_label(&design, CutPosition::After(count - 1)),
            format!("After {last_but_one} ({} of {count})", count - 1)
        );
    }

    #[test]
    fn a_position_past_the_last_step_reads_finished() {
        let design = round_brilliant();
        assert_eq!(cut_label(&design, CutPosition::After(999)), "Finished");
        assert_eq!(cut_label(&design, CutPosition::After(0)), "Finished");
    }

    #[test]
    fn an_unnamed_or_legacy_named_tier_reads_as_its_canonical_code() {
        let mut design = round_brilliant();
        let named = design.tiers[2].name.clone();
        let code = compute_tier_labels(&design.tiers)[2].code.clone();
        assert_eq!(code, "C2", "the crown main is the second crown tier cut");
        assert_eq!(
            step_name(&design, TierRef::Flat(2)),
            format!("{code} {named}"),
            "a descriptive name follows the code"
        );
        for unnamed in ["", "  ", "123", "c2"] {
            design.tiers[2].name = unnamed.to_string();
            let code = compute_tier_labels(&design.tiers)[2].code.clone();
            assert!(!code.is_empty(), "a tier always has a canonical code");
            assert_eq!(
                step_name(&design, TierRef::Flat(2)),
                code,
                "name {unnamed:?}"
            );
        }
    }

    /// The table's code is `T`; the slider names it with its own name after it. The table is
    /// the last step, so no "After" label ever names it (the last position is "Finished").
    #[test]
    fn the_table_reads_as_t_and_its_name() {
        let design = round_brilliant();
        assert_eq!(step_name(&design, TierRef::Flat(0)), "T Table");
        let count = design.preview_step_count();
        assert_eq!(design.preview_steps()[count - 1], TierRef::Flat(0));
        assert_eq!(cut_label(&design, CutPosition::After(count)), "Finished");
    }

    #[test]
    fn a_missing_tier_has_a_positional_name() {
        let design = round_brilliant();
        assert_eq!(step_name(&design, TierRef::Flat(99)), "tier 100");
        assert_eq!(step_name(&design, TierRef::Concave(0)), "Concave tier 1");
    }

    #[test]
    fn concave_steps_count_and_are_labelled_in_cutting_order() {
        let design = Design::concave_fixture();
        let count = design.preview_step_count();
        assert_eq!(count, design.tiers.len() + design.concave_tiers.len());
        // Cutting order: three flat pavilion/girdle tiers, the groove, two crowns, the dimple.
        assert_eq!(
            cut_label(&design, CutPosition::After(4)),
            format!("After P3 Groove (4 of {count})"),
            "the groove continues the P count of the two flat pavilion tiers"
        );
        assert_eq!(
            cut_label(&design, CutPosition::After(count - 1)),
            format!(
                "After {} ({} of {count})",
                step_name(&design, TierRef::Flat(4)),
                count - 1
            )
        );
    }
}
