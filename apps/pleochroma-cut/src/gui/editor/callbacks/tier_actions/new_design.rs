//! "Create" on the New Design dialog, and the actual replacement work it (and the
//! unsaved-changes guard's resume) shares.
//!
//! Two ways in, one way through:
//!
//! - The Empty form (and the worked-example guide, which fills it in) fires
//!   `EditorModel.new_design_create` with ten fields; [`do_new_design_create`] builds
//!   the design from the parsed [`FreshDesignSpec`] and the template index.
//! - A template card fires `TemplateGalleryModel.create` with four fields. The choice
//!   (template, gear, material, width) is stashed in [`TEMPLATE_CHOICE`] and the same
//!   dirty-design guard runs; [`do_new_design_create`] then builds the design with
//!   `pleochroma_editor::templates::create_from_template` -- remapped to the gear, at the
//!   chosen width, with the facet angles adapted to the material when its refractive
//!   index differs from the template's -- and tells the cutter what happened to the
//!   angles. The stash also survives the unsaved-changes guard, which resumes with
//!   `PendingUnsavedAction::New { spec, template_index }`.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use pleochroma_cut_core::FreshDesignSpec;
use pleochroma_editor::{
    EditorSession,
    templates::{NewDesignChoice, create_from_template, template_spec},
};
use slint::{ComponentHandle, SharedString};

use super::{FIXED_CYLINDER_PREFORM_SIDES, misc::bump_form_reset_pulse};
use crate::{
    EditorModel, MainWindow, TemplateGalleryModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            callbacks::solve_actions::clear_analysis_results,
            guide, landing, loading,
            stall_guard::stall_guard,
            state::{EditorState, PendingUnsavedAction, gear_choice_to_teeth},
            templates::GalleryDialog,
            view::{SolidLastSolved, push_has_design, refresh_all_now},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

thread_local! {
    /// The template choice the New Design dialog's template path is creating, between the
    /// click on Create and [`do_new_design_create`] -- which may run straight away or, for
    /// a design with unsaved changes, only after the Save/Discard/Cancel guard resumes it.
    /// The UI thread runs both ends, so a thread-local is enough; it holds `None` whenever
    /// nothing is pending.
    static TEMPLATE_CHOICE: RefCell<Option<NewDesignChoice>> = const { RefCell::new(None) };
}

/// Replaces (or clears) the pending template choice.
fn stash_template_choice(choice: Option<NewDesignChoice>) {
    TEMPLATE_CHOICE.with(|slot| *slot.borrow_mut() = choice);
}

/// Takes the pending template choice, leaving nothing behind. A choice for another
/// template than `template_index` is dropped, not used: it belongs to a create that
/// never reached [`do_new_design_create`] (a cancelled guard).
fn take_template_choice(template_index: i32) -> Option<NewDesignChoice> {
    TEMPLATE_CHOICE
        .with(|slot| slot.borrow_mut().take())
        .filter(|choice| choice.template_index == template_index)
}

/// The dirty check both Create paths share: a design with unsaved changes stashes
/// [`PendingUnsavedAction::New`] and opens the Save/Discard/Cancel guard; a clean one
/// goes straight to [`do_new_design_create`].
fn create_or_ask(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    spec: FreshDesignSpec,
    template_index: i32,
) {
    if state.borrow().is_dirty() {
        state.borrow_mut().pending_unsaved_action = Some(PendingUnsavedAction::New {
            spec: Box::new(spec),
            template_index,
        });
        ui.global::<EditorModel>().set_unsaved_dialog_message(
            "Starting a new design will discard the current one's unsaved changes.".into(),
        );
        ui.global::<EditorModel>().set_unsaved_dialog_open(true);
    } else {
        do_new_design_create(
            ui,
            state,
            render_ctx,
            preview_state,
            solid_last_solved,
            spec,
            template_index,
        );
    }
}

/// The template half of the New Design dialog: the options it shows (`dialog_shown`,
/// `template_chosen`, `material_chosen`) and "Create" for a template card. See this
/// module's doc comment for how Create reaches [`do_new_design_create`].
fn setup_template_dialog_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let dialog = Rc::new(GalleryDialog::new(render_ctx));
    let gallery = ui.global::<TemplateGalleryModel>();

    let ui_weak = ui.as_weak();
    let shown = Rc::clone(&dialog);
    gallery.on_dialog_shown(move || {
        stall_guard("on_template_dialog_shown", || {
            if let Some(ui) = ui_weak.upgrade() {
                shown.shown(&ui);
            }
        });
    });

    let ui_weak = ui.as_weak();
    let chosen = Rc::clone(&dialog);
    gallery.on_template_chosen(move |template_index: i32| {
        stall_guard("on_template_chosen", || {
            if let Some(ui) = ui_weak.upgrade() {
                chosen.template_chosen(&ui, template_index);
            }
        });
    });

    let ui_weak = ui.as_weak();
    let material = Rc::clone(&dialog);
    gallery.on_material_chosen(move |material_index: i32| {
        stall_guard("on_template_material_chosen", || {
            if let Some(ui) = ui_weak.upgrade() {
                material.material_chosen(&ui, material_index);
            }
        });
    });

    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    gallery.on_create(
        move |template_index: i32,
              gear_index: i32,
              material_index: i32,
              width_text: SharedString| {
            stall_guard("on_template_create", || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                // Validation first, before any unsaved-changes question -- the same order
                // as the Empty form's.
                let choice =
                    match dialog.choice(template_index, gear_index, material_index, &width_text) {
                        Ok(choice) => choice,
                        Err(message) => {
                            show_toast(&ui, &message, "error");
                            return;
                        }
                    };
                let Some(template) = template_spec(choice.template_index) else {
                    return;
                };
                let spec = template.fresh_spec(choice.material.clone());
                let template_index = choice.template_index;
                stash_template_choice(Some(choice));
                create_or_ask(
                    &ui,
                    &state,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                    spec,
                    template_index,
                );
            });
        },
    );
}

/// "Create" on the New Design dialog's Empty form. Replaces the editor state with a new design
/// built from the form's preform, gear, symmetry, mirror and material fields, via
/// [`install_new_design`]. The previous design and its history are discarded.
/// Validation runs first, so a form error shows at once and not behind the unsaved-changes guard.
/// A design with unsaved changes stashes [`PendingUnsavedAction::New`] and opens that guard; see
/// `setup_unsaved_guard_dispatch` for how "Save" and "Discard" resume it. Opening and closing the
/// dialog is Slint-only; this fires on "Create" alone.
///
/// The guides also use this route. A template card's Create goes through
/// [`setup_template_dialog_callbacks`], registered here too.
pub(in crate::gui::editor) fn setup_new_design_create_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    setup_template_dialog_callbacks(ui, state, render_ctx, preview_state, solid_last_solved);
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_new_design_create(
        move |shape_index: i32,
              half_width: SharedString,
              length_over_width: SharedString,
              depth: SharedString,
              gear_preset_index: i32,
              gear_custom_text: SharedString,
              symmetry_order_text: SharedString,
              mirror: bool,
              material_index: i32,
              template_index: i32| {
            stall_guard("on_new_design_create", || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                // A template choice left behind by a cancelled guard does not describe
                // this create.
                stash_template_choice(None);
                let gear_teeth = match gear_choice_to_teeth(gear_preset_index, &gear_custom_text) {
                    Ok(t) => t,
                    Err(e) => {
                        show_toast(&ui, &e, "error");
                        return;
                    }
                };
                let preform = match loading::parse_preform_form(
                    shape_index,
                    &half_width,
                    &length_over_width,
                    &depth,
                    FIXED_CYLINDER_PREFORM_SIDES,
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        show_toast(&ui, &e, "error");
                        return;
                    }
                };
                match loading::parse_new_design_form(
                    gear_teeth,
                    preform,
                    &symmetry_order_text,
                    mirror,
                    material_index,
                ) {
                    Ok(spec) => create_or_ask(
                        &ui,
                        &state,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        spec,
                        template_index,
                    ),
                    Err(e) => show_toast(&ui, &e, "error"),
                }
            });
        },
    );
}

/// The actual "New" work, run directly for a clean design or as the resume of
/// [`PendingUnsavedAction::New`] once the guard clears. The dirty check runs first, in
/// `create_or_ask`.
///
/// `pub(super)` since [`super::lifecycle`]'s unsaved-changes guard resumes it too.
pub(super) fn do_new_design_create(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    spec: FreshDesignSpec,
    // The gallery's template index (0 is "Empty"); see [`install_new_design`].
    template_index: i32,
) {
    // A template choice in `TEMPLATE_CHOICE` builds that design, remapped and sized. A refused
    // build keeps the dialog open and says why. Otherwise the design is built from `spec`.
    let prepared = match take_template_choice(template_index) {
        Some(choice) => match create_from_template(&choice) {
            Ok(created) => Some(created),
            Err(message) => {
                show_toast(ui, &message, "error");
                return;
            }
        },
        None => None,
    };
    let adaptation = if let Some(created) = prepared {
        install_prepared_design(&mut state.borrow_mut(), created.session);
        Some(created.adaptation)
    } else {
        install_new_design(&mut state.borrow_mut(), spec, template_index);
        None
    };
    // Analysis results of the replaced design no longer describe the screen; see
    // `clear_analysis_results`.
    clear_analysis_results(ui);
    // The material suggestion banner describes the replaced design, so it is cleared. Unlike Load
    // Selected, "New" has no schedule RI to suggest a material from.
    ui.global::<EditorModel>()
        .set_material_suggestion_name("".into());
    ui.global::<EditorModel>()
        .set_material_suggestion_text("".into());
    refresh_all_now(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        state,
        true,
    );
    // The same selection reset as `apply_loaded_design`.
    ui.global::<EditorModel>().set_selected_tier_index(-1);
    bump_form_reset_pulse(ui);
    ui.global::<EditorModel>().set_new_dialog_open(false);
    // A freshly created design is unsaved by decision (`install_new_design`), so the
    // model mirrors the state instead of forcing `false`.
    ui.global::<EditorModel>()
        .set_is_dirty(state.borrow().is_dirty());
    // A brand-new design has no file behind it yet, so the window title drops the
    // name entirely rather than keeping whatever was open before.
    ui.set_loaded_design_name(SharedString::new());
    // Right away, not only via `refresh_all`, which `refresh_all_now` may defer by
    // one event-loop tick: the empty-state card grid must give way to the new
    // design, even an Empty one with zero tiers.
    push_has_design(ui, &state.borrow());
    // The new design is worked on in the Edit view, so the cutter is taken there from whichever
    // page Create was pressed on (the dialog floats over all of them). Asked for before the
    // guide hears of the design: a design a guide launched must leave the page to the guide,
    // and that launch ends inside `guide::notify`. The view itself changes after the refresh
    // above, which may wait one turn of the event loop (see `landing`).
    let generation = state.borrow().current_generation();
    landing::design_landed(
        ui,
        generation,
        landing::Landing::ShowEditView,
        landing::Refresh::MayBeDeferred,
    );
    // The worked-example guide's first step completes on exactly this: a design
    // was created. Only this success path reaches here, never a rejected form.
    guide::notify(ui, guide::NEW_DESIGN_CREATED);
    // Last, so no other toast of the create overwrites it: what the material did to the
    // facet angles (adapted, or kept because the adapted stone would not be valid).
    if let Some(adaptation) = adaptation
        && let Some(message) = adaptation.message()
    {
        let kind = if adaptation.adapted() {
            "info"
        } else {
            "warning"
        };
        show_toast(ui, &message, kind);
    }
}

/// [`install_new_design`]'s twin for a design built before it reaches the state: the
/// New Design dialog's template path builds the whole session (gear remapped, width set,
/// angles adapted) with `pleochroma_editor::templates::create_from_template`, so this
/// only swaps it in -- through [`EditorState::replace_wholesale`], like every other
/// replacement, and marked a real design.
pub(super) fn install_prepared_design(st: &mut EditorState, session: EditorSession) {
    st.replace_wholesale(EditorState::fresh_from_session(session));
    st.has_design = true;
    // A design the user just created is unsaved until the first Save.
    st.session.mark_unsaved();
}

/// The state half of [`do_new_design_create`], with no window involved: replaces
/// `st` wholesale with a fresh design built from `spec`, marks it a real design
/// ([`EditorState::has_design`]) whatever the template -- template `0`, "Empty", is
/// a real design with zero tiers, not the startup placeholder -- and seeds
/// template `template_index`'s tiers when it names one.
///
/// `template_index`: 0 is "Empty"; 1..=N indexes
/// `pleochroma_cut_core::templates::TEMPLATES` at `template_index - 1` (index 1 is
/// "Standard Round Brilliant", `TEMPLATES[0]`, matching the gallery's own display
/// order -- see `gui/editor/templates.rs::setup_template_gallery`). Any index the
/// table has no entry for (0, a negative value, or one past the end) is treated as
/// empty rather than panicking -- the combo/gallery is the only producer, but a
/// stale index must not lose a design.
pub(super) fn install_new_design(st: &mut EditorState, spec: FreshDesignSpec, template_index: i32) {
    // `replace_wholesale` keeps this state's `generation` counter (and bumps it), so a background
    // solve against the replaced design still sees the change.
    // Seeded on the fresh design, not through `History`: it is the starting state, not an edit,
    // so it must not undo back to an empty schedule. `EditorSession::from_template` also refuses
    // a negative or stale index. `replace_wholesale` marks the replacement saved, so the design is
    // marked unsaved again: a new design has no file until its first Save.
    st.replace_wholesale(EditorState::fresh_from_template(spec, template_index));
    st.session.mark_unsaved();
    // The empty-state overlay depends on this.
    st.has_design = true;
}
