use super::*;
use crate::live_deltas::DeltaTone;
use pleochroma_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};

const PAVILION_MAIN: usize = 5;

fn rbc() -> Design {
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design.ensure_tier_ids();
    design
}

fn figures(brilliance: f32, windowing: f32) -> RowMetrics {
    RowMetrics {
        brilliance_pct: brilliance,
        windowing_pct: windowing,
        extinction_pct: 10.0,
        fire_index: 20.0,
        scintillation_pct: 30.0,
        yield_pct: Some(40.0),
        warning_count: 0,
        has_girdle: true,
        tilt: None,
    }
}

fn draft(design: &Design, metrics: Option<RowMetrics>, epoch: u64) -> TrialDraft {
    TrialDraft {
        design: design.clone(),
        solved: None,
        metrics,
        material_key: "diamond".to_owned(),
        design_epoch: epoch,
        label: "label".to_owned(),
        origin: TrialOrigin::Manual,
    }
}

fn nudged(design: &Design, tiers: &[usize], by: f64) -> Design {
    let mut moved = design.clone();
    for &tier in tiers {
        let angle = moved.tiers[tier].angle_deg;
        moved.tiers[tier].angle_deg = angle + by.copysign(angle);
    }
    moved
}

#[test]
fn numbers_increase_and_are_never_reused() {
    let design = rbc();
    let mut list = TrialList::new();
    assert!(list.is_empty());
    assert_eq!(list.push(draft(&design, None, 1)), 1);
    assert_eq!(list.push(draft(&design, None, 1)), 2);
    assert!(list.remove(2));
    assert!(!list.remove(2));
    assert_eq!(list.push(draft(&design, None, 1)), 3);
    assert_eq!(list.len(), 2);
    assert!(list.get(2).is_none());
    assert_eq!(list.get(3).map(|trial| trial.number), Some(3));
    list.clear();
    assert!(list.is_empty());
}

#[test]
fn the_cap_drops_the_oldest_unstarred_and_keeps_starred() {
    let design = rbc();
    let mut list = TrialList::new();
    for _ in 0..MAX_TRIALS {
        list.push(draft(&design, None, 1));
    }
    assert_eq!(list.toggle_star(1), Some(true));
    let newest = list.push(draft(&design, None, 1));
    assert_eq!(list.len(), MAX_TRIALS);
    assert!(list.get(1).is_some(), "the starred trial 1 stays");
    assert!(list.get(2).is_none(), "the oldest unstarred trial drops");
    assert!(list.get(newest).is_some());
    assert_eq!(list.toggle_star(1), Some(false));
    assert_eq!(list.toggle_star(999), None);
}

#[test]
fn notes_are_kept() {
    let design = rbc();
    let mut list = TrialList::new();
    let number = list.push(draft(&design, None, 1));
    assert!(list.set_note(number, "wider table"));
    assert!(!list.set_note(99, "nothing"));
    assert_eq!(
        list.get(number).map(|t| t.note.as_str()),
        Some("wider table")
    );
}

#[test]
fn labels_for_no_one_and_four_changes() {
    let design = rbc();
    assert_eq!(trial_label(&design, &design), "No angle change");

    let one = nudged(&design, &[PAVILION_MAIN], 0.25);
    let label = trial_label(&design, &one);
    assert!(label.contains("41.00 \u{2192} 41.25"), "{label}");
    assert!(!label.contains(','), "{label}");

    let four = nudged(&design, &[2, 3, PAVILION_MAIN, 6], 0.25);
    let label = trial_label(&design, &four);
    assert!(label.ends_with(" and 1 more"), "{label}");
    assert_eq!(label.matches('\u{2192}').count(), 3, "{label}");
}

#[test]
fn restore_refuses_a_trial_of_another_epoch() {
    let design = rbc();
    let mut list = TrialList::new();
    let number = list.push(draft(&design, None, 1));
    let trial = list.get(number).expect("kept");
    let refusal = restore_edit(&design, trial, 2).err();
    assert_eq!(
        refusal.as_deref(),
        Some("This trial belongs to a design that is no longer open.")
    );
}

#[test]
fn restore_of_an_equal_design_is_nothing_to_apply() {
    let design = rbc();
    let mut list = TrialList::new();
    let number = list.push(draft(&design, None, 1));
    let trial = list.get(number).expect("kept");
    assert!(matches!(restore_edit(&design, trial, 1), Ok(None)));
}

#[test]
fn restore_of_a_different_design_gives_an_edit() {
    let design = rbc();
    let moved = nudged(&design, &[PAVILION_MAIN], 0.25);
    let mut list = TrialList::new();
    let number = list.push(draft(&moved, None, 1));
    let trial = list.get(number).expect("kept");
    assert!(matches!(restore_edit(&design, trial, 1), Ok(Some(_))));
}

#[test]
fn rows_carry_tones_against_the_baseline_and_best_marks() {
    let design = rbc();
    let baseline = figures(20.0, 60.0);
    let mut list = TrialList::new();
    // 1: brilliance within the noise, windowing clearly worse.
    list.push(draft(&design, Some(figures(21.0, 65.0)), 1));
    // 2: brilliance clearly better, windowing clearly better.
    list.push(draft(&design, Some(figures(25.0, 55.0)), 1));
    // 3: not scored.
    list.push(draft(&design, None, 1));

    let rows = trial_rows(&list, Some(&baseline));
    assert_eq!(rows.len(), 3);
    let cell = |row: &TrialRowView, figure: DeltaFigure| {
        row.cells
            .iter()
            .find(|cell| cell.figure == figure)
            .cloned()
            .expect("the cell exists")
    };
    assert_eq!(
        cell(&rows[0], DeltaFigure::Brilliance).tone,
        DeltaTone::Same
    );
    assert_eq!(
        cell(&rows[0], DeltaFigure::Windowing).tone,
        DeltaTone::Worse
    );
    assert_eq!(
        cell(&rows[1], DeltaFigure::Brilliance).tone,
        DeltaTone::Better
    );
    assert_eq!(
        cell(&rows[1], DeltaFigure::Windowing).tone,
        DeltaTone::Better
    );
    assert_eq!(cell(&rows[1], DeltaFigure::Brilliance).text, "25.0");
    assert_eq!(cell(&rows[1], DeltaFigure::Brilliance).change_text, "+5.0");
    assert!(cell(&rows[1], DeltaFigure::Brilliance).best);
    assert!(!cell(&rows[0], DeltaFigure::Brilliance).best);
    assert!(cell(&rows[1], DeltaFigure::Windowing).best);
    // Extinction is the same in both scored trials: nothing stands out.
    assert!(!cell(&rows[0], DeltaFigure::Extinction).best);
    assert!(!rows[2].has_metrics);
    assert_eq!(rows[2].cells, [] as [crate::trials::rows::TrialCell; 0]);
    assert_eq!(rows[0].origin, "Manual");

    // Without a Baseline there is nothing to compare.
    let rows = trial_rows(&list, None);
    let plain = cell(&rows[1], DeltaFigure::Brilliance);
    assert_eq!(plain.tone, DeltaTone::Same);
    assert_eq!(plain.change_text, "");
}

#[test]
fn sorting_by_a_figure_puts_the_best_first_and_unscored_last() {
    let design = rbc();
    let mut list = TrialList::new();
    list.push(draft(&design, Some(figures(21.0, 65.0)), 1));
    list.push(draft(&design, None, 1));
    list.push(draft(&design, Some(figures(25.0, 55.0)), 1));
    let order = |sort| {
        list.sort_by(sort)
            .iter()
            .map(|trial| trial.number)
            .collect::<Vec<_>>()
    };
    assert_eq!(order(TrialSort::Number), vec![1, 2, 3]);
    assert_eq!(
        order(TrialSort::Figure(DeltaFigure::Brilliance)),
        vec![3, 1, 2]
    );
    // Windowing: lower is better.
    assert_eq!(
        order(TrialSort::Figure(DeltaFigure::Windowing)),
        vec![3, 1, 2]
    );
}
