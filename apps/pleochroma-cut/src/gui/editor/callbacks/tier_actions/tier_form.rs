//! The tier inspector's Save Tier form: parsing, target/name bookkeeping, and the
//! form's own error reporting.

use std::{
    cell::{RefCell, RefMut},
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use pleochroma::geometry::meet_solver::MeetConstraint;
use pleochroma_cut_core::TierTarget;
// The error-field classification and the non-integral-index warning live in
// `pleochroma_editor::loading`, shared with the web inspector. `report_tier_form_error`
// documents the field strings. Re-exported here for `super::tests`.
pub(super) use pleochroma_editor::loading::tier_form_error_field;
use pleochroma_editor::{
    loading::{non_integral_index_warning, parse_tier_form_with_relation},
    slider_ranges::{
        AnglePreset, AngleSide, SliderSpecData, angle_presets_for, angle_spec_data,
        format_angle_text, margin_preview_text, stepped_angle_text,
    },
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{nudge::tier_nudge_label, tier_generation::next_free_block_name};
use crate::{
    EditorModel, MainWindow, SliderModel, SliderPreset, SliderSpec,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            guide, loading,
            relation_ui::{
                is_relation_text, plan_tier_save, relation_placeholder_angle_deg,
                save_error_text_and_field, with_followers,
            },
            state::{EditorState, first_unresolved_meet_name},
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// The name tokens (`ConstraintTier::names()`, split on `/`) of every tier except the one
/// at `excluded_index`, so `parse_tier_form` can reject name collisions. A negative
/// `excluded_index` (a new tier) excludes nothing.
fn other_tier_names_excluding(st: &EditorState, excluded_index: i32) -> Vec<String> {
    pleochroma_editor::tier_save::other_tier_names_excluding(&st.design, excluded_index)
}

/// The label of the tier at `dirty_index`, read while `st` is still borrowed, before the
/// caller drops it.
fn added_tier_label(st: &EditorState, dirty_index: usize) -> Option<String> {
    st.design
        .tiers
        .get(dirty_index)
        .map(|tier| tier_nudge_label(tier, dirty_index))
}

/// Selects the tier at `dirty_index` and names it in a toast. Setting
/// `EditorModel.selected_tier_index` is enough to reveal it: `editor_view.slint`'s
/// `changed tracked_selected_tier_index` re-seeds the inspector and calls
/// `tier_table.focus_row`, which scrolls the row into view.
fn select_and_announce_added_tier(ui: &MainWindow, dirty_index: usize, label: Option<String>) {
    ui.global::<EditorModel>()
        .set_selected_tier_index(i32::try_from(dirty_index).expect("tier index fits in i32"));
    if let Some(label) = label {
        show_toast(ui, &format!("Added {label}"), "info");
    }
}

/// Parses the Meets combo's target from `constraint_kind`/`constraint_text` via
/// [`loading::parse_tier_target`].
///
/// Kinds `3`/`4`/`5` (cut to depth, girdle thickness, table width) carry a target; every
/// other kind gives `Ok(None)`.
///
/// A parse error is reported like a `parse_tier_form` failure and returns `Err(())`,
/// meaning "already reported, apply nothing". The unit error avoids clippy's
/// `option_option` lint for `Option<Option<TierTarget>>`.
fn parse_tier_target_reporting(
    ui: &MainWindow,
    constraint_kind: i32,
    constraint_text: &str,
) -> Result<Option<TierTarget>, ()> {
    loading::parse_tier_target(constraint_kind, constraint_text).map_err(|e| {
        report_tier_form_error(ui, &e, tier_form_error_field(&e));
    })
}

/// The table row selected in the inspector; a new tier is inserted right after it.
fn selected_row(ui: &MainWindow) -> Option<usize> {
    usize::try_from(ui.global::<EditorModel>().get_selected_tier_index()).ok()
}

/// What [`save_tier_now`] reads from the design besides the form's own five fields: the
/// existing row's `imported_meet` and `original_notes`, the gear tooth count, and the
/// other tiers' name tokens.
struct SaveTierFormContext {
    imported_meet: Option<MeetConstraint>,
    original_notes: Option<String>,
    gear_teeth_abs: u32,
    other_tier_names: Vec<String>,
}

/// Builds [`SaveTierFormContext`] from a short-lived immutable borrow of
/// `state` -- dropped before this returns, so the caller's later
/// `state.borrow_mut()` never races it.
fn save_tier_form_context(state: &Rc<RefCell<EditorState>>, index: i32) -> SaveTierFormContext {
    let st = state.borrow();
    // An existing row keeps its own `imported_meet` and the `.asc` file's
    // original `G` note across this save -- looked up here, before the
    // caller's mutable borrow, so editing an imported tier's name/angle/indices
    // never silently drops what the file claimed it meets, nor the note a
    // cutter reads while cutting it.
    let (imported_meet, original_notes) = (index >= 0)
        .then(|| {
            let tier = st.design.tiers.get(usize::try_from(index).ok()?)?;
            Some((tier.imported_meet.clone(), tier.original_notes.clone()))
        })
        .flatten()
        .unwrap_or_default();
    SaveTierFormContext {
        imported_meet,
        original_notes,
        gear_teeth_abs: st.design.meta.gear_teeth_abs(),
        other_tier_names: other_tier_names_excluding(&st, index),
    }
}

/// The outcome of a successful save, bundled to keep `apply_tier_save_success`'s argument
/// count within clippy's limit.
struct TierSaveOutcome {
    /// The form's own `index` argument: negative for a fresh `AddTier`, the
    /// tier's own index for a `ModifyTier`.
    index: i32,
    /// The saved tier's actual index in `st.design.tiers` after the edit applied.
    dirty_index: usize,
    /// Every row the save changed: the saved tier and the tiers that follow it through
    /// a relation (`relation_ui::with_followers`).
    dirty: BTreeSet<usize>,
    /// [`non_integral_index_warning`]'s verdict for the saved indices, if any.
    non_integral_warning: Option<String>,
}

/// The success arm of [`save_tier_now`]: refreshes the panel and preview, and reveals a new row.
///
/// Clears the form's error and shows the non-integral-index warning (if any). Takes `st` by
/// value so the `AddTier` branch can drop it before [`select_and_announce_added_tier`] calls
/// back into Slint.
fn apply_tier_save_success(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    st: RefMut<'_, EditorState>,
    outcome: TierSaveOutcome,
) {
    let TierSaveOutcome {
        index,
        dirty_index,
        dirty,
        non_integral_warning,
    } = outcome;
    // A successful save means the form is valid again, so this clears whatever the
    // last save's parse/validation error left behind.
    clear_tier_form_error(ui);
    // `AddTier` changes the tier count, so the alignment check falls back to a full solve
    // whatever `dirty` holds; for `ModifyTier` the saved tier and the tiers following it
    // through a relation are exactly what changed.
    refresh_editor_panel_stale(ui, render_ctx, &st, &dirty);
    // The guide's tier steps complete on the saved tier itself (name, angle,
    // indices) -- checked here explicitly as well as inside the stale refresh.
    guide::check_progress(ui, &st);
    submit_preview_replan(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        &st,
        dirty,
        false,
    );
    // Selects and reveals the row just added -- an `AddTier`-only branch, since a
    // `ModifyTier` save is already on the row it edited. See
    // [`select_and_announce_added_tier`]'s own doc comment for why setting
    // `selected_tier_index` alone is enough to reveal it too.
    if index < 0 {
        let label = added_tier_label(&st, dirty_index);
        drop(st);
        select_and_announce_added_tier(ui, dirty_index, label);
    }
    // Shown last so it stays on screen: the toast slot keeps only the latest call, and a
    // possibly unintended fractional index matters more to the cutter than the "Added" toast.
    if let Some(warning) = non_integral_warning {
        show_toast(ui, &warning, "info");
    }
}

/// Preserves an existing tier's detached indices and block side (pavilion vs crown)
/// across a tier form save, or infers pavilion side from name for a new tier.
fn preserve_saved_tier_side_and_detached(
    tier: &mut pleochroma_cut_core::ConstraintTier,
    design: &pleochroma_cut_core::Design,
    index: i32,
) {
    if let Some(current) = usize::try_from(index)
        .ok()
        .and_then(|i| design.tiers.get(i))
    {
        tier.detached.clone_from(&current.detached);
        if current.angle_deg.is_sign_negative() {
            tier.angle_deg = -tier.angle_deg.abs();
        }
    } else if index < 0
        && pleochroma_cut_core::design::labelling::name_indicates_pavilion(&tier.name)
    {
        tier.angle_deg = -tier.angle_deg.abs();
    }
}

/// Settles the side of a saved tier ([`preserve_saved_tier_side_and_detached`]), then
/// names a new tier saved with a blank Name field. An unnamed tier cannot be met by name,
/// so it gets `G<n>` at 90 degrees, else `P<n>` or `C<n>` by the side it ends up on, as
/// Duplicate names its copies. The side comes from the final angle, not the typed text,
/// so a relation to a pavilion tier (`=P1-2`) is named `P<n>`. Only for `index < 0`: an
/// existing tier's blank name was cleared on purpose and stays blank.
fn settle_saved_tier(
    tier: &mut pleochroma_cut_core::ConstraintTier,
    design: &pleochroma_cut_core::Design,
    index: i32,
    other_tier_names: &[String],
) {
    preserve_saved_tier_side_and_detached(tier, design, index);
    if index < 0 && tier.name.is_empty() {
        tier.name = next_free_block_name(tier.angle_deg, other_tier_names);
    }
}

/// The handles [`save_tier_now`] needs besides the form itself.
struct SaveServices<'a> {
    state: &'a Rc<RefCell<EditorState>>,
    render_ctx: &'a Arc<Mutex<RenderContext>>,
    preview_state: &'a Arc<SolidPreviewState>,
    solid_last_solved: &'a SolidLastSolved,
}

/// The six fields `EditorModel.save_tier` hands over, as the Save action received them.
struct TierFormInput<'a> {
    /// The tier's table row, or negative for a new tier.
    index: i32,
    /// The Angle field: a number, arithmetic (`41.5+0.3`) or a relation (`=C1-4`).
    angle: &'a str,
    constraint_kind: i32,
    constraint_text: &'a str,
    name: &'a str,
    indices: &'a str,
}

/// Parses the form into its tier and, when the Angle field holds a relation (`=...`), the
/// relation text after the `=`. Any other Angle text goes through [`loading::parse_tier_form`].
/// `Err` is the message to show.
fn parse_save_form(
    st: &Rc<RefCell<EditorState>>,
    form: &TierFormInput<'_>,
    context: SaveTierFormContext,
) -> Result<(pleochroma_cut_core::ConstraintTier, Option<String>), String> {
    let SaveTierFormContext {
        imported_meet,
        original_notes,
        gear_teeth_abs,
        other_tier_names,
    } = context;
    let fields = loading::TierFormFields {
        angle: form.angle,
        constraint_kind: form.constraint_kind,
        constraint_text: form.constraint_text,
        name: form.name,
        indices: form.indices,
        gear_teeth_abs,
        imported_meet,
        original_notes,
        other_tier_names,
    };
    if is_relation_text(form.angle) {
        // The relation, not the form, decides the angle: the tier carries its current
        // angle (a new tier: the one its relation would give) until the session applies it.
        let placeholder =
            relation_placeholder_angle_deg(&st.borrow().design, form.index, form.angle, form.name);
        parse_tier_form_with_relation(fields, placeholder)
    } else {
        loading::parse_tier_form(fields).map(|tier| (tier, None))
    }
}

/// The Tier form's Save action once its six fields are in: parses them
/// ([`parse_save_form`]), applies the resulting edit through the session and refreshes the
/// preview and panel on success. A refusal -- a bad field, a relation that cannot hold, a
/// direct angle edit of a tier that follows a relation -- is shown under the field it
/// belongs to ([`report_tier_form_error`]) and changes nothing.
fn save_tier_now(ui: &MainWindow, services: &SaveServices<'_>, form: &TierFormInput<'_>) {
    let SaveServices {
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    } = services;
    let index = form.index;
    let context = save_tier_form_context(state, index);
    let other_tier_names = context.other_tier_names.clone();
    let (mut tier, relation_text) = match parse_save_form(state, form, context) {
        Ok(parsed) => parsed,
        Err(e) => {
            report_tier_form_error(ui, &e, tier_form_error_field(&e));
            return;
        }
    };
    // The form's Meets combo also carries three target kinds that
    // `loading::parse_tier_form` above only turns into a `ScaleReference(0.0)`
    // placeholder -- see `parse_tier_target_reporting`'s own doc comment.
    let Ok(target) = parse_tier_target_reporting(ui, form.constraint_kind, form.constraint_text)
    else {
        return;
    };
    // Captured before `tier` is moved into the edit below -- see
    // `non_integral_index_warning`'s own doc comment for why this warns rather than
    // rejects.
    let non_integral_warning = non_integral_index_warning(&tier.indices);
    let mut st = state.borrow_mut();
    // The tier's side first, then (for a new tier left unnamed) its name -- see
    // `settle_saved_tier`.
    settle_saved_tier(&mut tier, &st.design, index, &other_tier_names);
    // The solver silently drops an unresolved `MeetNamed` token (see `MeetNameResolver` in
    // the meet solver), so it is refused here, naming the facet.
    if let MeetConstraint::MeetNamed(names) = &tier.constraint
        && let Some(bad_name) = first_unresolved_meet_name(&st.design, names)
    {
        report_tier_form_error(
            ui,
            &format!("No facet named '{bad_name}' -- check the Meets field."),
            "constraint",
        );
        return;
    }
    // Blanking the name of a tier other tiers meet by name would leave
    // their references dangling: refuse, at the Name field.
    if let Some(message) =
        pleochroma_editor::tier_save::unnaming_blocked_message(&st.design, index, form.name)
    {
        report_tier_form_error(ui, &message, "name");
        return;
    }
    // Fields apply when they are left, so a field left untouched hands over the tier as it
    // is: nothing changed, and no undo step is spent on it.
    if index >= 0
        && relation_text.is_none()
        && target.is_none()
        && usize::try_from(index)
            .ok()
            .and_then(|row| st.design.tiers.get(row))
            == Some(&tier)
    {
        drop(st);
        clear_tier_form_error(ui);
        return;
    }
    // The edit: the tier as the form describes it and, for `=...`, its relation in the same
    // undo step. The session then keeps every relation true and refuses what cannot be.
    let applied = plan_tier_save(
        &st.design,
        index,
        tier,
        relation_text.as_deref(),
        target,
        selected_row(ui),
    )
    .and_then(|(dirty_index, edit)| st.try_apply(edit).map(|_| dirty_index));
    match applied {
        Ok(dirty_index) => {
            // Followers (tiers whose angle follows the saved one) moved with it.
            let dirty = with_followers(&st.design, [dirty_index]);
            apply_tier_save_success(
                ui,
                render_ctx,
                preview_state,
                solid_last_solved,
                st,
                TierSaveOutcome {
                    index,
                    dirty_index,
                    dirty,
                    non_integral_warning,
                },
            );
        }
        Err(error) => {
            let (message, field) = save_error_text_and_field(&error);
            report_tier_form_error(ui, &message, field);
        }
    }
}

/// The UI's [`SliderSpec`] for the plain data `pleochroma_editor::slider_ranges` worked out:
/// numbers become `f32`, a missing band becomes an empty one (`band_to <= band_from`).
/// `pub(super)` since the Preform tab's sliders map their data the same way.
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the Slint property precision; slider values are small angles and steps"
)]
pub(super) fn slider_spec_from(data: SliderSpecData) -> SliderSpec {
    let (band_from, band_to) = data.band.unwrap_or((0.0, 0.0));
    SliderSpec {
        visible: data.visible,
        usable: data.usable,
        side: data.side.into(),
        value: data.value as f32,
        minimum: data.range.min as f32,
        maximum: data.range.max as f32,
        step: data.range.step as f32,
        fine_step: data.range.fine_step as f32,
        mark: data.range.mark as f32,
        band_from: band_from as f32,
        band_to: band_to as f32,
        note: data.note.into(),
        value_text: data.value_text.into(),
    }
}

/// The "Typical" menu's rows for the UI.
fn preset_rows(presets: Vec<AnglePreset>) -> ModelRc<SliderPreset> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 is the Slint property precision; preset values are small angles in degrees"
    )]
    let rows: Vec<SliderPreset> = presets
        .into_iter()
        .map(|preset| SliderPreset {
            label: preset.label.into(),
            value: preset.value_deg as f32,
            value_text: preset.value_text.into(),
            reason: preset.reason.into(),
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

/// Answers the Tier form's slider (`SliderModel`'s angle callbacks). All five are pure
/// functions of their arguments; the design's refractive index arrives as text, so the
/// answers follow a material change without touching the editor state. The work is done
/// in `pleochroma_editor::slider_ranges`:
///
/// - `angle_spec`: whether the form has a slider, its range, steps, marked band and value;
/// - `angle_presets`: the "Typical" menu;
/// - `angle_text`: the number the slider writes into the Angle field;
/// - `stepped_angle_text`: the field's text after an Up or Down key press (empty when the
///   field holds a calculation or a relation, which the key leaves alone);
/// - `margin_preview_text`: the Angle text as the live margin bar should read it.
///
/// The slider only fills the Angle field in; the tier is saved by "Save Tier" as ever.
fn setup_angle_slider_callbacks(ui: &MainWindow) {
    let model = ui.global::<SliderModel>();
    model.on_angle_spec(|kind, text, name, ri_text| {
        slider_spec_from(angle_spec_data(&kind, &text, &name, &ri_text))
    });
    model.on_angle_presets(|side, ri_text, material| {
        preset_rows(angle_presets_for(&side, &ri_text, &material))
    });
    model.on_angle_text(|value, current| format_angle_text(f64::from(value), &current).into());
    model.on_stepped_angle_text(|current, delta| {
        stepped_angle_text(&current, f64::from(delta))
            .unwrap_or_default()
            .into()
    });
    model.on_margin_preview_text(|side, text| {
        margin_preview_text(AngleSide::from_name(&side), &text).into()
    });
}

/// The Tier form's Save action: hands the form to [`save_tier_now`], which parses it,
/// applies the resulting edit through the session and refreshes the preview and panel on
/// success.
pub(in crate::gui::editor) fn setup_save_tier_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    // The concave form's callbacks are wired here, where every argument they need is available.
    super::concave_tier::setup_concave_tier_callbacks(
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    );
    // So are the Angle field's slider callbacks (they need no state at all).
    setup_angle_slider_callbacks(ui);
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_save_tier(
        move |index: i32,
              angle: SharedString,
              constraint_kind: i32,
              constraint_text: SharedString,
              name: SharedString,
              indices: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            save_tier_now(
                &ui,
                &SaveServices {
                    state: &state,
                    render_ctx: &render_ctx,
                    preview_state: &preview_state,
                    solid_last_solved: &solid_last_solved,
                },
                &TierFormInput {
                    index,
                    angle: &angle,
                    constraint_kind,
                    constraint_text: &constraint_text,
                    name: &name,
                    indices: &indices,
                },
            );
        },
    );
}

/// Clears the inline error state a failed save left behind -- what a successful save of
/// either form (flat or concave) does, so a marker never outlives the failure it reports.
pub(super) fn clear_tier_form_error(ui: &MainWindow) {
    let model = ui.global::<EditorModel>();
    model.set_tier_form_error("".into());
    model.set_tier_form_error_field("".into());
}

/// Shows one tier-form failure inline under its field and as a toast.
///
/// Every failure branch of [`save_tier_now`] goes through here, so the inline message and
/// the toast always agree. `field` is `"angle"`, `"name"`, `"indices"`, `"constraint"`, or
/// `""` for an unclassified error. `editor_inspector.slint` reads it to mark the offending
/// control;
/// see `EditorModel.tier_form_error_field` in `ui/models/editor.slint` for the contract.
pub(super) fn report_tier_form_error(ui: &MainWindow, message: &str, field: &str) {
    let model = ui.global::<EditorModel>();
    model.set_tier_form_error(message.into());
    model.set_tier_form_error_field(field.into());
    show_toast(ui, message, "error");
}

#[cfg(test)]
mod side_tests;
#[cfg(test)]
mod slider_tests;
