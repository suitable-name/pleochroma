//! Which rollout samples get a finite-difference label.
//!
//! The simulator can say, at a state, how each score component moves per degree of every free
//! tier (`TeacherLabel`). A label costs one or two candidate evaluations per free tier plus the
//! oracle step, so only a share of the visited states is labelled: one in every `label_every`.
//! The choice is pure arithmetic on counters, so a run labels the same samples every time.

/// Whether rollout slot `slot` labels its step of the global pass `pass`, for a period of
/// `every` steps (0 = never).
///
/// `pass` counts the passes (one step of every slot) since the start of the run, so a slot
/// labels exactly every `every`-th of its own steps and the slots are staggered by their index:
/// of any `every` consecutive passes a slot labels one, and within a pass every `every`-th slot
/// labels. Over a rollout of `q` steps a slot therefore labels ⌊q/every⌋ or ⌈q/every⌉ of them
/// whatever the slot count, and with a period of 2 or more never two steps in a row, so no slot
/// carries a run of expensive labelled steps while another carries none. The operands are
/// reduced first, so a wrapped pass counter cannot overflow.
///
/// The schedule it replaced summed the sample index and the pass count; with `n` slots that sum
/// moves by `n + 1` per pass, so whenever `n + 1` was a multiple of `every` (27 slots with
/// period 2, say) the same slots labelled every pass of the run.
pub(super) const fn labels_pass(every: usize, pass: usize, slot: usize) -> bool {
    every > 0 && (pass % every + slot % every).is_multiple_of(every)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_period_of_zero_labels_nothing() {
        for slot in 0..8 {
            assert!(!labels_pass(0, 0, slot));
            assert!(!labels_pass(0, 17, slot));
        }
    }

    #[test]
    fn a_period_of_one_labels_everything() {
        for (pass, slot) in [(0, 0), (3, 2), (128, 15)] {
            assert!(labels_pass(1, pass, slot));
        }
    }

    #[test]
    fn a_pass_schedule_labels_every_slot_once_per_period_for_any_slot_count() {
        assert!(
            labels_pass(4, usize::MAX, 1),
            "a wrapped counter stays in range"
        );
        // 27 slots, period 2: the labelled half alternates from pass to pass.
        for pass in 0..4 {
            let flagged: Vec<usize> = (0..27).filter(|&w| labels_pass(2, pass, w)).collect();
            assert_eq!(flagged.len(), 14 - pass % 2, "pass {pass}");
            assert!(flagged.iter().all(|w| (w + pass).is_multiple_of(2)));
        }
        for every in 1..=5 {
            for slot in 0..12 {
                let labelled = (0..every * 6)
                    .filter(|&pass| labels_pass(every, pass, slot))
                    .count();
                assert_eq!(labelled, 6, "every {every} slot {slot}");
            }
        }
    }

    #[test]
    fn the_share_of_labelled_samples_is_one_in_period() {
        let every = 2;
        let (workers, passes) = (6, 40);
        let mut labelled = 0;
        for pass in 0..passes {
            for slot in 0..workers {
                labelled += usize::from(labels_pass(every, pass, slot));
            }
        }
        assert_eq!(labelled, workers * passes / every);
    }
}
