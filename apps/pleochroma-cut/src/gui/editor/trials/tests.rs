//! Tests of the Trials view's pure parts: the edit watch that keeps Undo and Redo out of "Record
//! Every Change", its one-per-second gate, the sort entries and the table rows' greying. The
//! list itself is tested in `pleochroma_editor::trials`.

use super::{
    present::{self, NOT_SCORED, OTHER_MATERIAL},
    record::{EditWatch, MIN_GAP_MS, RecordGate, material_key, material_part, trial_key},
};
use pleochroma_cut_core::{CrystalOrientation, MaterialSelection, StoneRenderEffects};
use pleochroma_editor::{
    live_deltas::{DeltaFigure, DeltaTone},
    trials::{TrialCell, TrialRowView, TrialSort},
};

// --- the one-per-second gate ----------------------------------------------------------------

#[test]
fn the_gate_lets_one_trial_through_per_second() {
    let mut gate = RecordGate::default();
    assert!(gate.allow(5_000), "the first one always passes");
    assert!(!gate.allow(5_000), "the same instant is too soon");
    assert!(
        !gate.allow(5_000 + MIN_GAP_MS - 1),
        "just under a second is too soon"
    );
    assert!(gate.allow(5_000 + MIN_GAP_MS), "a second later passes");
    assert!(
        !gate.allow(5_000 + MIN_GAP_MS + 1),
        "and the clock starts again from that one"
    );
}

#[test]
fn a_clock_that_goes_backwards_never_opens_the_gate() {
    let mut gate = RecordGate::default();
    assert!(gate.allow(10_000));
    assert!(!gate.allow(9_000));
}

// --- the edit watch -------------------------------------------------------------------------

/// A history of `done` steps with revisions 1.. and `undone` further steps on the redo side.
fn history(done: u64, undone: u64) -> Vec<(u64, bool)> {
    (1..=done)
        .map(|revision| (revision, false))
        .chain((done + 1..=done + undone).map(|revision| (revision, true)))
        .collect()
}

#[test]
fn the_first_look_at_a_design_only_learns_it() {
    let mut watch = EditWatch::default();
    assert!(!watch.observe(1, &history(3, 0)));
    assert!(!watch.observe(1, &history(3, 0)), "nothing changed");
}

#[test]
fn a_new_step_is_an_edit() {
    let mut watch = EditWatch::default();
    watch.observe(1, &history(2, 0));
    assert!(watch.observe(1, &history(3, 0)));
    assert!(!watch.observe(1, &history(3, 0)), "seen once, not again");
}

#[test]
fn more_nudges_merged_into_the_newest_step_are_an_edit() {
    let mut watch = EditWatch::default();
    watch.observe(1, &history(2, 0));
    // The newest step keeps its position and gets a new revision.
    assert!(watch.observe(1, &[(1, false), (7, false)]));
}

#[test]
fn undo_and_redo_are_never_an_edit() {
    let mut watch = EditWatch::default();
    watch.observe(1, &history(3, 0));
    assert!(!watch.observe(1, &history(2, 1)), "undo");
    assert!(!watch.observe(1, &history(1, 2)), "a second undo");
    assert!(
        !watch.observe(1, &history(3, 0)),
        "a jump forward, which is a redo"
    );
    assert!(!watch.observe(1, &history(2, 1)), "undo again");
    assert!(!watch.observe(1, &history(3, 0)), "redo");
}

#[test]
fn an_edit_after_an_undo_is_an_edit() {
    let mut watch = EditWatch::default();
    watch.observe(1, &history(3, 0));
    assert!(!watch.observe(1, &history(2, 1)));
    // A new step replaces the undone one: a revision never seen, on the done side.
    assert!(watch.observe(1, &[(1, false), (2, false), (9, false)]));
}

#[test]
fn a_restore_the_cutter_made_is_absorbed() {
    let mut watch = EditWatch::default();
    watch.observe(1, &history(2, 0));
    // The restore adds a step; absorbing it makes it a change that was already seen.
    watch.absorb(1, &history(3, 0));
    assert!(!watch.observe(1, &history(3, 0)));
    assert!(
        watch.observe(1, &history(4, 0)),
        "the next real edit still counts"
    );
}

#[test]
fn another_design_starts_the_watch_again() {
    let mut watch = EditWatch::default();
    watch.observe(1, &history(5, 0));
    // Another design: its revisions count from the start, and none of them is an edit.
    assert!(!watch.observe(2, &history(1, 0)));
    assert!(watch.observe(2, &history(2, 0)));
    watch.reset();
    assert!(!watch.observe(2, &history(2, 0)), "a reset learns again");
}

// --- keys, sorts and rows -------------------------------------------------------------------

#[test]
fn a_material_key_tells_materials_apart() {
    let quartz = MaterialSelection {
        name: Some("Quartz".to_owned()),
        ..MaterialSelection::none()
    };
    let beryl = MaterialSelection {
        name: Some("Beryl".to_owned()),
        ..MaterialSelection::none()
    };
    assert_eq!(material_key(&quartz), material_key(&quartz.clone()));
    assert_ne!(material_key(&quartz), material_key(&beryl));
    assert_ne!(
        material_key(&quartz),
        material_key(&MaterialSelection::none())
    );
}

/// The trial key names the design's own Crystal Axis when it is on, and is exactly the material
/// key otherwise (a stone with no axis of its own, or the override left off).
#[test]
fn a_trial_key_names_the_crystal_axis_only_when_the_override_is_on() {
    let quartz = MaterialSelection {
        name: Some("Quartz".to_owned()),
        ..MaterialSelection::none()
    };
    let off = StoneRenderEffects::default();
    assert_eq!(trial_key(&quartz, &off, None), material_key(&quartz));
    // An axis that is stored but not switched on changes nothing.
    let stored_only = StoneRenderEffects {
        c_axis_tilt_deg: 30.0,
        c_axis_azimuth_deg: 120.0,
        ..off
    };
    assert_eq!(
        trial_key(&quartz, &stored_only, None),
        material_key(&quartz)
    );

    let on = StoneRenderEffects {
        c_axis_override_enabled: true,
        ..stored_only
    };
    let key = trial_key(&quartz, &on, None);
    assert_ne!(key, material_key(&quartz));
    assert!(key.ends_with(";caxis=30.00/120.00"), "{key}");
    assert_eq!(material_part(&key), material_key(&quartz));
    assert_eq!(material_part(&material_key(&quartz)), material_key(&quartz));

    let other_tilt = StoneRenderEffects {
        c_axis_tilt_deg: 31.0,
        ..on
    };
    assert_ne!(trial_key(&quartz, &other_tilt, None), key, "another tilt");
    let turned = StoneRenderEffects {
        c_axis_second_azimuth_deg: Some(20.0),
        ..on
    };
    let turned_key = trial_key(&quartz, &turned, None);
    assert!(
        turned_key.ends_with(";caxis=30.00/120.00/20.00"),
        "{turned_key}"
    );
    assert_ne!(turned_key, key, "a Second Axis turn");
}

/// A custom material's stored default axis is part of the key while the design's own axis is
/// off; the design's own axis, when on, wins over it; no default keeps the plain key.
#[test]
fn a_trial_key_follows_the_materials_stored_default_axis() {
    let custom = MaterialSelection {
        name: Some("My Rutile".to_owned()),
        ..MaterialSelection::none()
    };
    let off = StoneRenderEffects::default();
    let flat = CrystalOrientation::from_angles(0.0, 0.0, None);
    let tilted = CrystalOrientation::from_angles(30.0, 120.0, Some(20.0));

    assert_eq!(trial_key(&custom, &off, None), material_key(&custom));
    let flat_key = trial_key(&custom, &off, Some(flat));
    assert!(flat_key.ends_with(";caxis=0.00/0.00"), "{flat_key}");
    let tilted_key = trial_key(&custom, &off, Some(tilted));
    assert!(
        tilted_key.ends_with(";caxis=30.00/120.00/20.00"),
        "{tilted_key}"
    );
    assert_ne!(flat_key, tilted_key, "a changed default is another key");
    assert_eq!(material_part(&tilted_key), material_key(&custom));

    // The design's own axis is what renders, so the default does not matter then.
    let own = StoneRenderEffects {
        c_axis_override_enabled: true,
        c_axis_tilt_deg: 10.0,
        c_axis_azimuth_deg: 5.0,
        ..off
    };
    assert_eq!(
        trial_key(&custom, &own, Some(tilted)),
        trial_key(&custom, &own, None)
    );
}

#[test]
fn the_sort_entries_map_to_their_orders() {
    assert_eq!(present::sort_for(0), TrialSort::Number);
    assert_eq!(
        present::sort_for(1),
        TrialSort::Figure(DeltaFigure::Brilliance)
    );
    assert_eq!(present::sort_for(5), TrialSort::Figure(DeltaFigure::Yield));
    assert_eq!(present::sort_for(-1), TrialSort::Number);
    assert_eq!(present::sort_for(99), TrialSort::Number);
}

fn brilliance_cell(best: bool) -> TrialCell {
    TrialCell {
        figure: DeltaFigure::Brilliance,
        text: "20.6".to_owned(),
        change_text: "+1.8".to_owned(),
        tone: DeltaTone::Better,
        best,
        tooltip: String::new(),
    }
}

fn row(has_metrics: bool, material: &str) -> TrialRowView {
    TrialRowView {
        number: 3,
        label: "P1 40.75 \u{2192} 41.25".to_owned(),
        note: String::new(),
        origin: "Sweep",
        starred: false,
        material_key: material.to_owned(),
        has_metrics,
        cells: if has_metrics {
            vec![brilliance_cell(true)]
        } else {
            Vec::new()
        },
    }
}

#[test]
fn a_row_always_has_one_cell_per_column() {
    let scored = present::row_of(&row(true, "a"), "a");
    assert_eq!(scored.cells.len(), present::COLUMNS.len());
    let unscored = present::row_of(&row(false, "a"), "a");
    assert_eq!(unscored.cells.len(), present::COLUMNS.len());
    assert!(unscored.cells.iter().all(|cell| cell.tooltip == NOT_SCORED));
}

#[test]
fn a_figure_in_the_design_s_material_keeps_its_tone_and_best_mark() {
    let view = present::row_of(&row(true, "a"), "a");
    let cell = &view.cells[0];
    assert_eq!((cell.tone, cell.best), (1, true));
    assert_eq!(cell.change_text, "+1.8");
    assert_eq!(cell.tooltip, "Best of the list.");
}

#[test]
fn a_figure_from_another_material_shows_grey_with_its_reason() {
    let view = present::row_of(&row(true, "a"), "b");
    let cell = &view.cells[0];
    assert_eq!(cell.tone, 0);
    assert!(
        !cell.best,
        "a figure of another material is not the best of this one"
    );
    assert_eq!(cell.change_text.len(), 0);
    assert_eq!(cell.tooltip, OTHER_MATERIAL);
    // The value itself is still shown.
    assert_eq!(cell.text, "20.6");
}

/// A trial of the same material that differs only in its Crystal Axis is not greyed as another
/// material: it keeps its tone and says "Other Crystal Axis". Another material still greys.
#[test]
fn a_figure_that_differs_only_in_its_crystal_axis_says_so_and_is_not_greyed() {
    let material = material_key(&MaterialSelection {
        name: Some("Rutile".to_owned()),
        ..MaterialSelection::none()
    });
    let own = format!("{material};caxis=30.00/120.00");
    let now = format!("{material};caxis=45.00/0.00");

    let view = present::row_of(&row(true, &own), &now);
    let cell = &view.cells[0];
    assert_eq!((cell.tone, cell.best), (1, true));
    assert_eq!(cell.change_text, "+1.8");
    assert!(
        cell.tooltip.starts_with(present::OTHER_AXIS),
        "{}",
        cell.tooltip
    );
    assert_ne!(cell.tooltip, OTHER_MATERIAL);

    // No axis now against an axis then is a difference of the axis too.
    let plain = present::row_of(&row(true, &own), &material);
    assert!(plain.cells[0].tooltip.starts_with(present::OTHER_AXIS));

    // The same key says nothing; another material greys whatever the axis.
    let same = present::row_of(&row(true, &own), &own);
    assert_eq!(same.cells[0].tooltip, "Best of the list.");
    let elsewhere = present::row_of(&row(true, &own), "beryl;caxis=30.00/120.00");
    assert_eq!(elsewhere.cells[0].tooltip, OTHER_MATERIAL);
    assert_eq!(elsewhere.cells[0].tone, 0);
}

#[test]
fn an_unknown_current_material_greys_nothing() {
    let view = present::row_of(&row(true, "a"), "");
    assert_eq!(view.cells[0].tone, 1);
}

// --- re-scoring in each trial's own orientation ----------------------------------------------

/// Two trials of one design that differ only in their Crystal Axis, re-scored while the design
/// itself sits at a third orientation: each gets the figures of a direct evaluation under ITS OWN
/// effects, the two differ (a strongly birefringent stone), and each is filed under the key that
/// names its own axis, never the design's now.
#[test]
fn rescoring_scores_each_trial_in_its_own_orientation_and_files_it_under_that_key() {
    use super::rescore::{self, Item, Job};
    use crate::gui::editor::material_lookup::{
        EditorMaterialLookup, resolved_gem_material_for_design,
    };
    use pleochroma_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};
    use pleochroma_editor::sweep::{SweepOptions, SweepScene, score_design};
    use std::sync::{Mutex, atomic::AtomicBool};

    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design.ensure_tier_ids();
    let rutile = MaterialSelection {
        name: Some("Rutile".to_owned()),
        ..MaterialSelection::none()
    };
    let at = |tilt: f64| StoneRenderEffects {
        c_axis_override_enabled: true,
        c_axis_tilt_deg: tilt,
        c_axis_azimuth_deg: 0.0,
        ..StoneRenderEffects::default()
    };
    let (flat, upright, now) = (at(0.0), at(90.0), at(45.0));
    let item = |number: u32, effects: StoneRenderEffects| Item {
        number,
        design: {
            let mut trial = design.clone();
            trial.render_effects = effects;
            trial
        },
        solved: None,
    };
    let job = Job {
        items: vec![item(1, flat), item(2, upright)],
        material: rutile.clone(),
        custom_materials: Vec::new(),
        material_default: None,
    };
    let current_key = trial_key(&rutile, &now, None);
    let results = Mutex::new(Vec::new());
    rescore::run(&job, &current_key, &AtomicBool::new(false), &results);
    let done = results.into_inner().expect("the lock is not poisoned");
    assert_eq!(done.len(), 2);

    let lookup = EditorMaterialLookup::new(&[]);
    let direct = |effects: &StoneRenderEffects| {
        let material = resolved_gem_material_for_design(&rutile, &lookup, effects);
        let scene = SweepScene {
            material: &material,
            environment: pleochroma_cut_core::CANONICAL_LIGHTING_PRESET.studio(
                1.0,
                pleochroma_cut_core::optimize::CANONICAL_LIGHT_YAW,
                pleochroma_cut_core::optimize::CANONICAL_LIGHT_PITCH,
            ),
        };
        let options = SweepOptions {
            tilt_average: false,
            workers: 1,
        };
        score_design(
            &design_with(&design, effects),
            None,
            &scene,
            &options,
            &AtomicBool::new(false),
        )
        .expect("not cancelled")
    };
    assert_eq!(done[0].1, direct(&flat));
    assert_eq!(done[1].1, direct(&upright));
    assert_ne!(done[0].1, done[1].1, "another axis, other figures");
    assert_eq!(done[0].2, trial_key(&rutile, &flat, None));
    assert_eq!(done[1].2, trial_key(&rutile, &upright, None));
    assert_ne!(done[0].2, current_key);
}

/// `design` with `effects` as its stone effects.
fn design_with(
    design: &pleochroma_cut_core::Design,
    effects: &StoneRenderEffects,
) -> pleochroma_cut_core::Design {
    let mut copy = design.clone();
    copy.render_effects = *effects;
    copy
}

/// Inclusions, play-of-color and an abraded edge tell trial keys apart; a design that sets none of
/// them keeps exactly the key it always had.
#[test]
fn a_trial_key_names_the_render_physics_only_when_one_is_set() {
    use pleochroma_cut_core::EdgeAbrasionEffects;
    let quartz = MaterialSelection {
        name: Some("Quartz".to_owned()),
        ..MaterialSelection::none()
    };
    let off = StoneRenderEffects::default();
    assert_eq!(trial_key(&quartz, &off, None), material_key(&quartz));

    let edge = StoneRenderEffects {
        edge_abrasion: EdgeAbrasionEffects {
            width: 0.02,
            alpha: 0.4,
            halo_rq_nm: 0.0,
        },
        ..off
    };
    let edge_key = trial_key(&quartz, &edge, None);
    assert_ne!(edge_key, material_key(&quartz));
    assert!(edge_key.starts_with(&material_key(&quartz)));

    let wider = StoneRenderEffects {
        edge_abrasion: EdgeAbrasionEffects {
            width: 0.03,
            ..edge.edge_abrasion
        },
        ..off
    };
    assert_ne!(trial_key(&quartz, &wider, None), edge_key);
}
