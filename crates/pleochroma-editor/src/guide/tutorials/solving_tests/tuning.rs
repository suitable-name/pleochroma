//! The walks of the Two Tiers, Keep Trials and Tune by Hand lessons, and the premises they state
//! about the design.

use super::{RICH, Sim, act, events, read, walk};
use crate::{
    guide::StartingState,
    sweep::{MAX_SWEEP_CELLS, apply_sweep_angles},
    trials::{MAX_TRIALS, TrialDraft, TrialList, TrialOrigin, restore_edit},
};

/// The design epoch the walks use for the open design.
const EPOCH: u64 = 1;

/// A trial draft of the design as it is now.
fn draft_of(sim: &Sim, origin: TrialOrigin) -> TrialDraft {
    TrialDraft {
        design: sim.session.design.clone(),
        solved: None,
        metrics: None,
        material_key: String::new(),
        design_epoch: EPOCH,
        label: String::new(),
        origin,
    }
}

/// Keep as Trial: the design is kept and the desktop reports it.
fn keep_trial(sim: &mut Sim) {
    sim.variants.push(sim.session.design.clone());
    sim.raise(events::TRIAL_KEPT);
}

/// Restore on the first kept design, planned and applied the way the Trials view does it.
fn restore_first_trial(sim: &mut Sim) {
    let mut list = TrialList::new();
    let mut draft = draft_of(sim, TrialOrigin::Manual);
    draft.design = sim.variants[0].clone();
    let number = list.push(draft);
    let trial = list.get(number).expect("the trial is in the list");
    let edit = restore_edit(&sim.session.design, trial, EPOCH)
        .expect("the trial can be restored")
        .expect("the trial differs from the design");
    sim.apply(edit);
    sim.raise(events::TRIAL_RESTORED);
}

#[test]
fn the_two_tier_sweep_lesson_can_be_played() {
    walk(
        "optimizing-sweep-two-tiers",
        vec![
            read(),
            act(|sim| {
                // The sweep runs on private copies, then the chosen pair is used.
                sim.raise(events::TWO_TIER_SWEEP_RUN);
                let crown = sim.row("Crown Main");
                let pavilion = sim.row("Pavilion Main");
                apply_sweep_angles(&mut sim.session, (crown, 35.5), (pavilion, -41.0))
                    .expect("the pair is used");
                sim.solved = false;
                sim.raise(events::TWO_TIER_APPLIED);
            }),
            act(Sim::undo),
            read(),
        ],
    );
}

#[test]
fn using_a_pair_is_one_undo_step_and_one_undo_puts_both_tiers_back() {
    let mut sim = Sim::start(&StartingState::Template(RICH));
    let steps_before = sim.session.history_entries().len();
    let crown = sim.row("Crown Main");
    let pavilion = sim.row("Pavilion Main");
    apply_sweep_angles(&mut sim.session, (crown, 35.5), (pavilion, -41.0))
        .expect("the pair is used");
    assert_eq!(
        sim.session.history_entries().len(),
        steps_before + 1,
        "a pair is one step"
    );
    sim.undo();
    assert!((sim.session.design.tiers[crown].angle_deg - 34.5).abs() < 1e-9);
    assert!((sim.session.design.tiers[pavilion].angle_deg + 40.0).abs() < 1e-9);
}

#[test]
fn the_same_tier_twice_cannot_be_swept_and_the_grid_has_a_cap() {
    let mut sim = Sim::start(&StartingState::Template(RICH));
    let crown = sim.row("Crown Main");
    assert!(
        apply_sweep_angles(&mut sim.session, (crown, 35.0), (crown, 36.0)).is_err(),
        "one tier cannot be both axes"
    );
    assert_eq!(
        MAX_SWEEP_CELLS, 900,
        "the lesson says a grid is capped at 900"
    );
}

#[test]
fn the_two_tier_lesson_names_the_pair_the_form_starts_with() {
    let sim = Sim::start(&StartingState::Template(RICH));
    // The form starts on the crown main across and the pavilion main down.
    assert!(sim.session.design.tiers.len() > sim.row("Crown Main"));
    assert!(sim.session.design.tiers.len() > sim.row("Pavilion Main"));
}

#[test]
fn the_keep_trials_lesson_can_be_played() {
    walk(
        "solving-trials",
        vec![
            act(keep_trial),
            act(|sim| sim.set_angle("Crown Main", "36")),
            act(keep_trial),
            act(|sim| {
                sim.open_tab(4);
                sim.raise(events::TRIALS_VIEW_OPENED);
            }),
            act(restore_first_trial),
            act(|sim| sim.raise(events::SNAPSHOT_TAKEN)),
            read(),
        ],
    );
}

#[test]
fn a_restore_is_one_step_and_a_trial_of_another_design_is_refused() {
    let mut sim = Sim::start(&StartingState::Template(RICH));
    let mut list = TrialList::new();
    let first = list.push(draft_of(&sim, TrialOrigin::Manual));
    sim.set_angle("Crown Main", "36");
    let steps_before = sim.session.history_entries().len();
    let trial = list.get(first).expect("kept");
    let edit = restore_edit(&sim.session.design, trial, EPOCH)
        .expect("the trial can be restored")
        .expect("the trial differs from the design");
    sim.apply(edit);
    assert_eq!(sim.session.history_entries().len(), steps_before + 1);
    assert!((sim.session.design.tiers[sim.row("Crown Main")].angle_deg - 34.5).abs() < 1e-9);
    let trial = list.get(first).expect("kept");
    assert!(
        restore_edit(&sim.session.design, trial, EPOCH + 1).is_err(),
        "a trial of a design that is no longer open"
    );
}

#[test]
fn the_list_holds_fifty_trials_and_the_oldest_unstarred_one_drops() {
    let sim = Sim::start(&StartingState::Template(RICH));
    assert_eq!(MAX_TRIALS, 50, "the lesson says the list holds 50");
    let mut list = TrialList::new();
    let first = list.push(draft_of(&sim, TrialOrigin::Manual));
    assert!(list.toggle_star(first).is_some());
    for _ in 0..MAX_TRIALS {
        list.push(draft_of(&sim, TrialOrigin::Manual));
    }
    assert_eq!(list.len(), MAX_TRIALS);
    assert!(list.get(first).is_some(), "the starred trial stays");
    assert!(
        list.get(first + 1).is_none(),
        "the oldest unstarred one dropped"
    );
}

#[test]
fn the_tune_lesson_can_be_played() {
    walk(
        "optimizing-tune-by-hand",
        vec![
            act(|sim| {
                sim.open_tab(2);
                sim.raise(events::TUNE_OPENED);
            }),
            act(|sim| {
                sim.set_angle("Pavilion Main", "-40.1");
                sim.raise(events::TUNE_NUDGED);
            }),
            act(|sim| sim.raise(events::TUNE_ACCEPTED)),
            act(|sim| {
                sim.set_angle("Crown Main", "34.6");
                sim.raise(events::TUNE_NUDGED);
            }),
            act(|sim| sim.raise(events::TUNE_ACCEPTED)),
            read(),
        ],
    );
}

#[test]
fn a_nudge_alone_is_not_an_accept() {
    let guide = super::guide_named("optimizing-tune-by-hand");
    let mut sim = Sim::start(&guide.starting_state);
    sim.set_angle("Pavilion Main", "-40.1");
    assert!(
        !sim.met(&guide.steps[1].goal),
        "the nudge step waits for the nudge event as well as the angle"
    );
    sim.raise(events::TUNE_NUDGED);
    assert!(sim.met(&guide.steps[1].goal));
    assert!(
        !sim.met(&guide.steps[2].goal),
        "the accept step waits for Accept"
    );
}
