//! The Inspector's Optimize tab: the objective preset, what may change, the angle ranges,
//! the ranked candidate list and its selection. The pure rules (defaults, clamping, row
//! text, applying a candidate) live in `pleochroma_editor::optimize_view`; this module is
//! the glue between them and `ui/models/optimize.slint`.
//!
//! - [`setup_optimize_panel`] registers the tab's callbacks (preset, "Vary anchored tiers",
//!   the ranges table, picking a candidate).
//! - [`refresh`] runs with every panel refresh (a hook in
//!   `view::panel_stale::refresh_optimize_availability`): it applies the "Vary anchored
//!   tiers" default, the availability and hint, the ranges table and the time estimate.
//! - [`plan_from_ui`] turns the tab into a run for `callbacks::solve_actions::optimize_run`.
//! - [`show_run`] shows a finished run: the candidate rows, the first candidate picked.
//! - [`apply_pending`] applies the picked candidate (angles and masts, relations folded in)
//!   for `callbacks::solve_actions::optimize_outcome`.
//!
//! Picking a candidate makes it the pending Optimize outcome (`EditorState::pending_optimize`),
//! which is what the Preview toggle, the compare window and Apply already read -- so none of
//! them needed to learn about candidates. They find the candidate's masts through
//! [`candidate_for_outcome`].

mod rows;
mod simple_mode;
mod state;

pub(in crate::gui::editor) use state::{StoredRun, candidate_for_outcome, with_panel};

use super::{
    material_lookup::EditorMaterialLookup,
    state::{EditorState, TOOLTIP_LINE_CHARS, apply_proposed_angles, wrap_tooltip_text},
    view::{optimize_change_rows, optimize_result_rows, result_row},
};
use crate::{
    CompareModel, EditorModel, EditorTierItem, MainWindow, OptimizeCandidateRow, OptimizeChangeRow,
    OptimizeModel, OptimizeResultRow, OptimizeToneSwatches,
    gui::tutorial_events::{raise, raise_weak},
};
use pleochroma_cut_core::{
    CANONICAL_LIGHTING_PRESET, Design, OptimizeCandidate, OptimizeConfig, OptimizeOutcome,
    OptimizeResult,
    material::{BuiltinMaterials, MaterialLookup},
    optimize::{effective_starts, inclusive_max_evaluations_for},
};
use pleochroma_editor::{
    EditorSession,
    guide::solving_events::{
        OPTIMIZE_CANDIDATE_PICKED, OPTIMIZE_PRESET_CHOSEN, OPTIMIZE_RANGES_OPENED,
    },
    optimize_view::{
        DEFAULT_BUDGET, DEFAULT_CANDIDATES, DEFAULT_STARTS, RangeInput, RunForm, RunPlan,
        apply_candidate, baseline_line, build_run_plan, candidate_lines, default_vary_anchored,
        estimate_run_seconds, estimate_text, measured_ms_per_evaluation, optimize_availability,
        parse_budget, parse_starts, preset_description, preset_labels, range_rows, range_summary,
        signed_tone, tone_lighting_label, tone_result_rows, tone_scale_note, weights_for_preset,
    },
    retarget::plan::tier_display_names,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};
use state::{MeasuredRate, RangeTable, range_signature};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError, atomic::Ordering},
};

/// The weight boxes and yield slider as the Optimize tab's button hands them over.
#[derive(Clone, Copy)]
pub(in crate::gui::editor) struct WeightBoxes<'a> {
    pub windowing: &'a str,
    pub extinction: &'a str,
    pub tilt_brilliance: &'a str,
    pub yield_weight: f32,
    /// The signed tone slider: negative lighter, positive deeper, `0` off.
    pub tone: f32,
}

/// A finished run, as [`show_run`] needs it.
pub(in crate::gui::editor) struct FinishedRun {
    /// What the search found.
    pub result: OptimizeResult,
    /// The design the run started from.
    pub design: Design,
    /// The design generation the run started at.
    pub generation: u64,
    /// The design was edited while the run was going: its candidates can no longer apply.
    pub stale: bool,
    /// Wall time of the run, for the next time estimate.
    pub elapsed_secs: f32,
}

/// Registers the Optimize tab's callbacks and fills its static texts. Called once from
/// `setup_editor_callbacks`.
pub(in crate::gui::editor) fn setup_optimize_panel(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let model = ui.global::<OptimizeModel>();
    setup_simple_mode(&model);
    model.set_preset_description(
        preset_description(usize::try_from(model.get_preset_index()).unwrap_or(0)).into(),
    );

    model.on_preset_changed({
        let ui_weak = ui.as_weak();
        let state = Rc::clone(state);
        move |index| {
            if let Some(ui) = ui_weak.upgrade() {
                on_preset_changed(&ui, index);
                // The tone note and its visibility follow the preset.
                refresh_from_callback(&ui_weak, &state, false);
            }
        }
    });
    model.on_vary_toggled({
        let ui_weak = ui.as_weak();
        let state = Rc::clone(state);
        move |_ticked| {
            // From here on the tick is the cutter's own, not the default rule's.
            with_panel(|panel| panel.vary_touched = true);
            refresh_from_callback(&ui_weak, &state, true);
        }
    });
    model.on_ranges_toggled({
        let ui_weak = ui.as_weak();
        let state = Rc::clone(state);
        move |open| {
            if open {
                raise_weak(&ui_weak, OPTIMIZE_RANGES_OPENED);
            }
            refresh_from_callback(&ui_weak, &state, true);
        }
    });
    model.on_settings_edited({
        let ui_weak = ui.as_weak();
        let state = Rc::clone(state);
        move || refresh_from_callback(&ui_weak, &state, false)
    });
    model.on_range_edited({
        let ui_weak = ui.as_weak();
        move |tier, min_text, max_text| {
            let Ok(tier) = usize::try_from(tier) else {
                return;
            };
            let customised = with_panel(|panel| {
                let table = panel.ranges.as_mut()?;
                table.set(tier, min_text.to_string(), max_text.to_string());
                Some(simple_mode::ranges_customised(&table.inputs))
            });
            // The Simple interface hides the table; this is what lets it say a range is set.
            if let (Some(customised), Some(ui)) = (customised, ui_weak.upgrade()) {
                ui.global::<OptimizeModel>()
                    .set_ranges_customised(customised);
            }
        }
    });
    model.on_reset_ranges({
        let ui_weak = ui.as_weak();
        let state = Rc::clone(state);
        move || {
            with_panel(|panel| {
                if let Some(table) = panel.ranges.as_mut() {
                    table.inputs.clear();
                }
            });
            refresh_from_callback(&ui_weak, &state, true);
        }
    });
    model.on_select_candidate({
        let ui_weak = ui.as_weak();
        let state = Rc::clone(state);
        move |index| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // A pick during a call that holds the editor state (a refresh in progress) is
            // dropped; the cutter clicks again.
            let updated = state.try_borrow().is_ok_and(|st| {
                select_candidate(&ui, &st, index);
                true
            });
            // Open only after releasing the editor borrow: comparison reads the source
            // and the exact newly selected candidate itself.
            if updated && ui.global::<CompareModel>().get_optimizer_inline_open() {
                if ui.global::<EditorModel>().get_optimize_can_apply() {
                    ui.global::<CompareModel>().invoke_open_optimize();
                } else {
                    ui.global::<CompareModel>().invoke_close();
                }
            }
        }
    });
}

/// The preset lists and the Simple interface's one question. The tab lists the presets in
/// full in the Advanced interface and without "Custom" in the Simple one; and
/// `OptimizeModel.settings_in_use` says whether a setting the Simple interface hides is away
/// from its default (see [`simple_mode`]).
fn setup_simple_mode(model: &OptimizeModel<'_>) {
    let labels = preset_labels();
    model.set_simple_preset_labels(ModelRc::new(VecModel::from(
        simple_mode::simple_labels(&labels)
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    model.set_preset_labels(ModelRc::new(VecModel::from(
        labels
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    model.on_settings_in_use(
        |vary,
         vary_default,
         keep_girdle,
         ranges_customised,
         budget,
         seed,
         candidates,
         polish,
         only_selected,
         starts| {
            simple_mode::settings_in_use(&simple_mode::HiddenSettings {
                vary_anchored: simple_mode::Tick {
                    on: vary,
                    default_on: vary_default,
                },
                keep_girdle: simple_mode::Tick {
                    on: keep_girdle,
                    default_on: true,
                },
                polish: simple_mode::Tick {
                    on: polish,
                    default_on: true,
                },
                only_selected: simple_mode::Tick {
                    on: only_selected,
                    default_on: false,
                },
                ranges_customised,
                budget_text: &budget,
                seed_text: &seed,
                candidates,
                // A starts box that cannot be read counts as away from the default.
                starts: parse_starts(&starts)
                    .ok()
                    .and_then(|n| i32::try_from(n).ok())
                    .unwrap_or(-1),
            })
        },
    );
}

/// The preset combo changed: show what the preset favours, and copy its weights into the
/// weight boxes so "Custom" starts from them.
fn on_preset_changed(ui: &MainWindow, index: i32) {
    let index = usize::try_from(index).unwrap_or(0);
    ui.global::<OptimizeModel>()
        .set_preset_description(preset_description(index).into());
    if let Some(weights) = weights_for_preset(index) {
        let editor = ui.global::<EditorModel>();
        editor.set_optimize_weight_windowing(weights.windowing.to_string().into());
        editor.set_optimize_weight_extinction(weights.extinction.to_string().into());
        editor.set_optimize_weight_tilt_brilliance(weights.tilt_brilliance.to_string().into());
        editor.set_optimize_weight_yield(weights.yield_weight);
        editor.set_optimize_weight_tone(signed_tone(&weights));
    }
    raise(ui, OPTIMIZE_PRESET_CHOSEN);
}

/// [`refresh_with`] for a Slint callback, which has no `EditorState` in hand. Skipped when
/// the state is borrowed (a refresh is already running and will do the same work).
fn refresh_from_callback(
    ui_weak: &Weak<MainWindow>,
    state: &Rc<RefCell<EditorState>>,
    rebuild_ranges: bool,
) {
    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    if let Ok(st) = state.try_borrow() {
        refresh_with(&ui, &st, rebuild_ranges);
    }
}

/// Brings the tab up to date with `state`: the panel's own memory follows the design, the
/// "Vary anchored tiers" default applies, and availability, the ranges table, the time
/// estimate and the stale flag are redone. Called with every panel refresh.
pub(in crate::gui::editor) fn refresh(ui: &MainWindow, state: &EditorState) {
    refresh_with(ui, state, false);
}

/// [`refresh`], optionally rebuilding the ranges table's rows from what the cutter typed
/// (the table was just opened, or reset).
fn refresh_with(ui: &MainWindow, state: &EditorState, rebuild_ranges: bool) {
    let model = ui.global::<OptimizeModel>();
    let epoch = state.design_epoch.load(Ordering::Relaxed);
    let new_design = with_panel(|panel| {
        let changed = panel.epoch != Some(epoch);
        if changed {
            panel.reset_for_new_design(epoch);
        }
        changed
    });
    if new_design {
        clear_results_ui(ui);
    }
    let wanted = default_vary_anchored(&state.design);
    model.set_vary_default(wanted);
    if !with_panel(|panel| panel.vary_touched) && model.get_vary_anchored() != wanted {
        model.set_vary_anchored(wanted);
    }
    model.set_ranges_customised(with_panel(|panel| {
        panel
            .ranges
            .as_ref()
            .is_some_and(|table| simple_mode::ranges_customised(&table.inputs))
    }));
    let vary = model.get_vary_anchored();
    let editor = ui.global::<EditorModel>();
    let budget = parse_budget(&editor.get_optimize_budget_text()).unwrap_or(DEFAULT_BUDGET);
    let (available, hint) = optimize_availability(&state.design, budget, vary);
    editor.set_optimize_available(available);
    // The hint only feeds tooltips (the "Optimize" heading and the Run button), and the
    // built-in Slint tooltip cannot wrap, so the line breaks go in here.
    editor.set_optimize_hint(wrap_tooltip_text(&hint, TOOLTIP_LINE_CHARS).into());
    sync_ranges(ui, &state.design, vary, rebuild_ranges);
    set_estimate(ui, &state.design, vary);
    let results_outdated = with_panel(|panel| {
        panel
            .run
            .as_ref()
            .is_some_and(|run| run.generation != state.generation.load(Ordering::Relaxed))
    });
    model.set_results_stale(results_outdated);
    if results_outdated {
        invalidate_inline_comparison(ui);
    }
    refresh_tone(ui, state);
}

/// The face-up colour objective's note and whether it applies: a tone preset is chosen or the
/// Custom Tone slider is away from zero.
///
/// The note is worked out under the grading tray (`CANONICAL_LIGHTING_PRESET`), exactly what
/// [`plan_from_ui`] hands the run. The custom materials are not at hand here (they live on
/// the render context), so a design naming a custom catalogue material gets no note rather
/// than a wrong "no body colour" one.
///
/// The material is resolved with the design's color pattern applied, so a stone with color zones
/// is told the objective rates its base color only (see [`tone_note_for`]).
fn refresh_tone(ui: &MainWindow, state: &EditorState) {
    let model = ui.global::<OptimizeModel>();
    let editor = ui.global::<EditorModel>();
    let preset = usize::try_from(model.get_preset_index()).unwrap_or(0);
    let active = weights_for_preset(preset).is_some_and(|weights| signed_tone(&weights) != 0.0)
        || editor.get_optimize_weight_tone().abs() > 0.05;
    // `tone_active` is what shows the note, so it stays on for a zoned stone: the note is the
    // place that says why the objective is unreliable there.
    model.set_tone_active(active);
    model.set_tone_note(tone_note_for(&state.design, active).into());
}

/// The note under the Tone slider for `design`: empty while the objective is off or the design
/// names a custom catalogue material this panel cannot resolve.
///
/// The material is the design's own, color pattern included (at the girdle diameter, 7 mm when
/// none is set), so [`tone_scale_note`] can tell a zoned stone from a plain one.
fn tone_note_for(design: &Design, active: bool) -> String {
    let selection = &design.material;
    let unknown_custom = selection.name.as_deref().is_some_and(|name| {
        BuiltinMaterials.lookup(name).is_none()
            && selection.body_color_override.is_none()
            && selection.body_color_bands_override.is_none()
            && selection.color_variant_override.is_none()
    });
    if !active || unknown_custom {
        return String::new();
    }
    let width_mm = design
        .girdle_diameter_mm
        .filter(|mm| *mm > 0.0 && mm.is_finite())
        .unwrap_or(7.0);
    let resolved = selection.resolve(&EditorMaterialLookup::new(&[])).gem;
    let material = selection.apply_overrides_at_width(resolved, width_mm);
    // The same rating conditions as the run (see `plan_from_ui`).
    tone_scale_note(design, &material, CANONICAL_LIGHTING_PRESET)
}

/// Pushes the one-line summary above the ranges table, and rebuilds the table's rows when
/// the table is open and either `force` is set or the tiers it lists changed.
///
/// The rows are a fresh model each time (never edited in place), so the fields rebuild with
/// the typed texts instead of keeping a stale binding. A change in the listed tiers drops
/// what was typed: the ranges described other tiers.
fn sync_ranges(ui: &MainWindow, design: &Design, vary: bool, force: bool) {
    let model = ui.global::<OptimizeModel>();
    let tiers = range_rows(design, vary);
    model.set_range_summary(range_summary(&tiers).into());
    if !model.get_ranges_open() {
        return;
    }
    let signature = range_signature(design, vary);
    let typed = with_panel(|panel| {
        let same = panel
            .ranges
            .as_ref()
            .is_some_and(|table| table.signature == signature);
        if !same {
            panel.ranges = Some(RangeTable::new(signature));
        }
        (force || !same).then(|| {
            panel
                .ranges
                .as_ref()
                .map(|table| table.inputs.clone())
                .unwrap_or_default()
        })
    });
    if let Some(typed) = typed {
        let names = tier_display_names(design);
        let shown: Vec<_> = tiers
            .iter()
            .map(|tier| {
                rows::range_row(
                    tier,
                    typed.iter().find(|input| input.tier == tier.tier),
                    names
                        .get(tier.tier)
                        .map_or(tier.name.as_str(), String::as_str),
                )
            })
            .collect();
        model.set_range_rows(ModelRc::new(VecModel::from(shown)));
    }
}

/// The line under the budget box.
fn set_estimate(ui: &MainWindow, design: &Design, vary: bool) {
    let model = ui.global::<OptimizeModel>();
    let editor = ui.global::<EditorModel>();
    let movable = range_rows(design, vary)
        .iter()
        .filter(|row| !row.driven)
        .count();
    if movable == 0 {
        model.set_estimate_text(SharedString::new());
        return;
    }
    let defaults = OptimizeConfig::default();
    let config = OptimizeConfig {
        max_evaluations: parse_budget(&editor.get_optimize_budget_text()).unwrap_or(DEFAULT_BUDGET),
        polish_start_step_deg: if editor.get_optimize_polish_enabled() {
            defaults.polish_start_step_deg
        } else {
            None
        },
        ..defaults
    };
    let candidates = usize::try_from(model.get_candidates()).unwrap_or(DEFAULT_CANDIDATES);
    let starts = parse_starts(&model.get_starts_text()).unwrap_or(DEFAULT_STARTS);
    let config = OptimizeConfig { starts, ..config };
    // With several starts the estimate adds screening and per-start polish itself, so it
    // takes the plain budget; a single start takes the inclusive total as it always did.
    let evaluations = if effective_starts(&config, movable) > 1 {
        config.max_evaluations
    } else {
        inclusive_max_evaluations_for(&config, candidates, movable)
    };
    let measured = with_panel(|panel| {
        panel
            .measured
            .filter(|rate| rate.tier_count == design.tiers.len())
    });
    let seconds = estimate_run_seconds(
        design.tiers.len(),
        movable,
        evaluations,
        candidates,
        starts,
        measured.map(|rate| rate.ms_per_evaluation),
    );
    model.set_estimate_text(estimate_text(seconds, measured.is_some()).into());
}

/// The run the tab asks for, for `design`.
///
/// # Errors
///
/// A message naming the field that cannot be read (a weight, the budget, the seed or an
/// angle range), for a toast.
pub(in crate::gui::editor) fn plan_from_ui(
    ui: &MainWindow,
    design: &Design,
    boxes: WeightBoxes<'_>,
) -> Result<RunPlan, String> {
    let model = ui.global::<OptimizeModel>();
    let editor = ui.global::<EditorModel>();
    let vary = model.get_vary_anchored();
    let signature = range_signature(design, vary);
    // The typed ranges count only while they still describe this design's tiers.
    let typed: Vec<RangeInput> = with_panel(|panel| {
        panel
            .ranges
            .as_ref()
            .filter(|table| table.signature == signature)
            .map(|table| table.inputs.clone())
            .unwrap_or_default()
    });
    let budget_text = editor.get_optimize_budget_text();
    let seed_text = editor.get_optimize_seed_text();
    let starts = parse_starts(&model.get_starts_text())?;
    let form = RunForm {
        preset_index: usize::try_from(model.get_preset_index()).unwrap_or(0),
        weight_windowing: boxes.windowing,
        weight_extinction: boxes.extinction,
        weight_tilt_brilliance: boxes.tilt_brilliance,
        yield_weight: boxes.yield_weight,
        tone: boxes.tone,
        vary_anchored: vary,
        keep_girdle: model.get_keep_girdle(),
        budget_text: &budget_text,
        starts,
        seed_text: &seed_text,
        polish: editor.get_optimize_polish_enabled(),
        candidates: usize::try_from(model.get_candidates()).unwrap_or(DEFAULT_CANDIDATES),
        ranges: &typed,
    };
    // Every optimization rates under the same conditions: the grading tray, as Retarget does,
    // whatever lighting the viewport shows.
    let lighting = CANONICAL_LIGHTING_PRESET;
    build_run_plan(design, &form, lighting)
}

/// Shows a finished run: the starting stone's row, the candidate rows, the best candidate
/// picked (its figures and changed tiers in the shared result tables, Apply and Compare
/// ready), and the next time estimate learned from it. Returns the picked outcome, if any
/// candidate was found.
///
/// `pending` receives the picked outcome only while the run is current (the design was not
/// edited meanwhile), which is what lets Apply and Compare work.
pub(in crate::gui::editor) fn show_run(
    ui: &MainWindow,
    pending: &Arc<Mutex<Option<(OptimizeOutcome, u64)>>>,
    run: FinishedRun,
) -> Option<OptimizeOutcome> {
    let FinishedRun {
        result,
        design,
        generation,
        stale,
        elapsed_secs,
    } = run;
    let model = ui.global::<OptimizeModel>();
    let lines = candidate_lines(&result);
    model.set_baseline_row(rows::candidate_row(&baseline_line(&result)));
    model.set_candidate_rows(ModelRc::new(VecModel::from(
        lines.iter().map(rows::candidate_row).collect::<Vec<_>>(),
    )));
    model.set_has_results(true);
    model.set_results_stale(stale);
    let selected = (!result.candidates.is_empty()).then_some(0);
    model.set_selected_candidate(selected.map_or(-1, |_| 0));
    if let Some(ms_per_evaluation) = measured_ms_per_evaluation(
        f64::from(elapsed_secs),
        result.outcome.evaluations,
        result.candidates.len(),
    ) {
        with_panel(|panel| {
            panel.measured = Some(MeasuredRate {
                tier_count: design.tiers.len(),
                ms_per_evaluation,
            });
        });
    }
    let stored = StoredRun {
        result,
        design,
        generation,
        selected,
    };
    let outcome = stored.selected_outcome();
    let (result_rows, change_rows) = outcome.as_ref().map_or_else(
        || (Vec::new(), Vec::new()),
        |picked| {
            (
                picked_result_rows(picked, &stored.result, stored.selected),
                optimize_change_rows(picked, &stored.design),
            )
        },
    );
    push_result_tables(ui, result_rows, change_rows);
    model.set_tone_swatches(tone_swatches(&stored.result, stored.selected));
    let can_apply = outcome.is_some() && !stale;
    ui.global::<EditorModel>().set_optimize_can_apply(can_apply);
    *pending.lock().unwrap_or_else(PoisonError::into_inner) = outcome
        .clone()
        .filter(|_| can_apply)
        .map(|picked| (picked, generation));
    with_panel(|panel| panel.run = Some(stored));
    repaint_ghost_angles(ui, outcome.as_ref().filter(|_| can_apply));
    outcome
}

/// The candidate at position `selected` of `result`, if one is picked.
fn picked_candidate(
    result: &OptimizeResult,
    selected: Option<usize>,
) -> Option<&OptimizeCandidate> {
    selected.and_then(|position| result.candidates.get(position))
}

/// The result table of the picked candidate: the five objective rows, then the two face-up
/// tone rows when the run measured the colour.
fn picked_result_rows(
    picked: &OptimizeOutcome,
    result: &OptimizeResult,
    selected: Option<usize>,
) -> Vec<OptimizeResultRow> {
    let mut rows = optimize_result_rows(picked);
    if let Some(candidate) = picked_candidate(result, selected) {
        rows.extend(
            tone_result_rows(result, candidate)
                .into_iter()
                .map(result_row),
        );
    }
    rows
}

/// The before and after face-up colour swatches of the picked candidate, as the screen shows
/// the light. `has` is false without a measured colour on either side or without a pick.
fn tone_swatches(result: &OptimizeResult, selected: Option<usize>) -> OptimizeToneSwatches {
    let (Some(before), Some(after)) = (
        result.tone_before,
        picked_candidate(result, selected).and_then(|candidate| candidate.tone),
    ) else {
        return OptimizeToneSwatches::default();
    };
    OptimizeToneSwatches {
        has: true,
        before: rows::srgb_color(before.srgb),
        after: rows::srgb_color(after.srgb),
        before_text: format!("L* {:.1}", before.l_star).into(),
        after_text: format!("L* {:.1}", after.l_star).into(),
        lighting: tone_lighting_label(result.lighting).into(),
    }
}

/// Writes the picked candidate's figures and changed tiers into the shared result tables.
fn push_result_tables(
    ui: &MainWindow,
    result_rows: Vec<OptimizeResultRow>,
    change_rows: Vec<OptimizeChangeRow>,
) {
    let editor = ui.global::<EditorModel>();
    editor.set_optimize_result_rows(ModelRc::new(VecModel::from(result_rows)));
    editor.set_optimize_change_rows(ModelRc::new(VecModel::from(change_rows)));
}

/// A click on the candidate list: `index` is a candidate, `-1` the starting stone (nothing
/// picked). Makes the pick the pending Optimize outcome, so Preview, Compare and Apply act
/// on it, and refreshes the tables beside the list.
fn select_candidate(ui: &MainWindow, state: &EditorState, index: i32) {
    let wanted = usize::try_from(index).ok();
    let picked = with_panel(|panel| {
        let run = panel.run.as_mut()?;
        run.selected = wanted.filter(|&candidate| candidate < run.result.candidates.len());
        let outcome = run.selected_outcome();
        let tables = outcome.as_ref().map(|picked| {
            (
                picked_result_rows(picked, &run.result, run.selected),
                optimize_change_rows(picked, &run.design),
            )
        });
        let swatches = tone_swatches(&run.result, run.selected);
        Some((run.selected, run.generation, outcome, tables, swatches))
    });
    let Some((selected, generation, outcome, tables, swatches)) = picked else {
        return;
    };
    ui.global::<OptimizeModel>().set_tone_swatches(swatches);
    let current = generation == state.generation.load(Ordering::Relaxed);
    let outcome = outcome.filter(|_| current);
    let (result_rows, change_rows) = tables.filter(|_| current).unwrap_or_default();
    ui.global::<OptimizeModel>().set_selected_candidate(
        selected.map_or(-1, |candidate| i32::try_from(candidate).unwrap_or(-1)),
    );
    push_result_tables(ui, result_rows, change_rows);
    ui.global::<EditorModel>()
        .set_optimize_can_apply(outcome.is_some());
    *state
        .pending_optimize
        .lock()
        .unwrap_or_else(PoisonError::into_inner) =
        outcome.clone().map(|picked| (picked, generation));
    repaint_ghost_angles(ui, outcome.as_ref());
    if selected.is_some() {
        raise(ui, OPTIMIZE_CANDIDATE_PICKED);
    }
}

/// Moves the tier table's "proposed angle" ghost column to `outcome`'s changes (none when
/// `None`), patching the rows in place. The table is rebuilt with the ghost on every
/// panel refresh; this keeps it right between refreshes when another candidate is picked.
fn repaint_ghost_angles(ui: &MainWindow, outcome: Option<&OptimizeOutcome>) {
    let model = ui.global::<EditorModel>().get_tiers();
    let mut tiers: Vec<EditorTierItem> = model.iter().collect();
    let before: Vec<SharedString> = tiers
        .iter()
        .map(|tier| tier.proposed_angle.clone())
        .collect();
    for tier in &mut tiers {
        tier.proposed_angle = SharedString::new();
    }
    if let Some(outcome) = outcome {
        apply_proposed_angles(&mut tiers, &outcome.changes);
    }
    for (position, tier) in tiers.into_iter().enumerate() {
        if before.get(position) != Some(&tier.proposed_angle) {
            model.set_row_data(position, tier);
        }
    }
}

/// Forgets the last run and empties the list: a new run is starting, the design was
/// replaced, or the candidate was applied.
pub(in crate::gui::editor) fn clear_results(ui: &MainWindow) {
    with_panel(|panel| panel.run = None);
    clear_results_ui(ui);
}

/// The list's properties back to "no run yet".
fn clear_results_ui(ui: &MainWindow) {
    invalidate_inline_comparison(ui);
    let model = ui.global::<OptimizeModel>();
    model.set_has_results(false);
    model.set_results_stale(false);
    model.set_selected_candidate(-1);
    model.set_baseline_row(OptimizeCandidateRow::default());
    model.set_candidate_rows(ModelRc::new(VecModel::from(
        Vec::<OptimizeCandidateRow>::new(),
    )));
    model.set_progress(0.0);
    model.set_tone_swatches(OptimizeToneSwatches::default());
}

/// Clears the view-only session when its source/result goes away. The Preview handler
/// retries a held editor borrow, so Apply and replacement can call this during refresh.
fn invalidate_inline_comparison(ui: &MainWindow) {
    let compare = ui.global::<CompareModel>();
    if compare.get_optimizer_inline_open() {
        compare.invoke_close();
        compare.set_optimize_preview_reset_serial(
            compare.get_optimize_preview_reset_serial().wrapping_add(1),
        );
        ui.global::<EditorModel>()
            .invoke_optimize_preview_toggled(false);
    }
}

/// Applies the picked candidate to `session` as one undo step.
///
/// A candidate of the last run is applied with its masts and with the tiers that follow a
/// relation moved along ([`apply_candidate`]); a portable outcome carries the same
/// certified depths and source identity. Uncertified outcomes are refused.
///
/// # Errors
///
/// A plain-English message; nothing changes on `Err`.
pub(in crate::gui::editor) fn apply_pending(
    session: &mut EditorSession,
    outcome: &OptimizeOutcome,
) -> Result<usize, String> {
    match candidate_for_outcome(outcome) {
        Some(candidate) => apply_candidate(session, &candidate),
        None => session
            .apply_optimize_outcome(outcome)
            .map_err(|error| error.to_string()),
    }
}

#[cfg(test)]
mod tests;
