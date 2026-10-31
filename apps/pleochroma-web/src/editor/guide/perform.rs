//! Next on an unfinished step: carries out the step's recipe the way the learner would.
//!
//! Every part of a recipe goes through the control it stands for: the Tier form's Save, the
//! tier table's Solve and Delete, the Preform tab's Apply Yield Inputs, the concave form's
//! Save, the view tabs. So the edit is one undo step like the learner's own, a refused entry is
//! reported under its field by the form's own parser, and the lesson's control locks do not
//! matter: they stop the learner's clicks, not the app. Nothing here judges success. The
//! step's goal is checked from state as ever (`guide::check_progress`), so the step completes
//! the usual way, and a recipe that did not get there leaves the step open with a button that
//! now says Skip Step.
//!
//! What a recipe turns into is `pleochroma_web_core::guide_perform::plan`, which is pure and
//! tested natively. A recipe with a [`Gap`](pleochroma_web_core::guide_perform::Gap) is refused
//! whole, with a toast that says why. A recipe that starts a solve has no result yet when it
//! returns; the runner's 300 ms tick waits up to six seconds for the goal, and a late result
//! still completes the step. A New Design may first ask about unsaved changes; the rest of the
//! recipe waits for the answer and is dropped with a Cancel.

use super::{install_lesson_design, judge, raise, register_performer};
use crate::{
    AppModel, AppWindow, DesignSettingsModel, InspectorModel, ManipulateModel, ShortcutsModel,
    SnapshotModel, TierTableModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
    },
    editor::{edit, inspector, selection, table, unsaved::confirm_discard},
};
use pleochroma_cut_core::{Design, design::TierRef};
use pleochroma_editor::guide::{
    GuideStep, TierEntry, concave_position, resolve_tier, tier_position,
};
use pleochroma_web_core::{
    guide_perform::{Atom, Command, ConcaveForm, Stop, plan},
    rows::row_id,
};
use slint::{ComponentHandle, Model as _, SharedString};
use std::collections::VecDeque;

/// What a refused recipe says before it names the reason.
const REFUSED: &str = "Next cannot do this step in the browser";

/// Registers the executor behind Next. Call once from `guide::wire`.
pub(super) fn wire(ui: &AppWindow) {
    register_performer(ui, perform_step);
}

/// Next on `step`: `true` when its recipe ran (the goal is judged by the runner), `false` when
/// it could not, which turns the button into Skip Step.
fn perform_step(ctx: &Ctx, step: &GuideStep) -> bool {
    let Some(recipe) = step.perform.as_ref() else {
        return false;
    };
    let atoms = match plan(recipe) {
        Ok(atoms) => atoms,
        Err(gap) => {
            show_message(
                ctx,
                MessageKind::Info,
                &format!("{REFUSED}: {gap}. Use Skip Step to move on."),
            );
            return false;
        }
    };
    match run(ctx, atoms.into()) {
        Ok(()) => true,
        Err(refusal) => {
            report(ctx, &refusal);
            false
        }
    }
}

/// Tells the learner why a recipe stopped, unless the control that refused it already did.
fn report(ctx: &Ctx, stop: &Stop) {
    if let Stop::Say(message) = stop {
        show_message(ctx, MessageKind::Error, message);
    }
}

/// Runs `atoms` in order. A New Design stands in front of everything after it: the unsaved
/// changes question may answer later, so the rest of the recipe continues from its answer.
fn run(ctx: &Ctx, mut atoms: VecDeque<Atom>) -> Result<(), Stop> {
    while let Some(atom) = atoms.pop_front() {
        if let Atom::NewDesign {
            template_index,
            spec,
        } = atom
        {
            let c = ctx.clone();
            confirm_discard(ctx, move || {
                install_lesson_design(&c, *spec, template_index);
                if let Err(stop) = run(&c, atoms) {
                    report(&c, &stop);
                }
            });
            return Ok(());
        }
        execute(ctx, &atom)?;
    }
    Ok(())
}

/// Carries out one atom.
fn execute(ctx: &Ctx, atom: &Atom) -> Result<(), Stop> {
    let ui = ctx
        .ui
        .upgrade()
        .ok_or_else(|| Stop::say("The window is gone."))?;
    with_design(ctx, |_| ())?;
    match atom {
        // `run` takes the New Design itself, before it gets here.
        Atom::NewDesign { .. } => Ok(()),
        Atom::Tier(entry) => save_tier(ctx, &ui, entry),
        Atom::Concave {
            edit: existing,
            form,
        } => save_concave(ctx, &ui, existing.as_deref(), form),
        Atom::Select(name) => select_row(ctx, name).map(|_| ()),
        Atom::Steps(series) => {
            table::sync(ctx);
            ui.global::<TierTableModel>().invoke_generate_steps(
                series.name.as_str().into(),
                series.start.as_str().into(),
                series.step.as_str().into(),
                series.count,
                series.indices.as_str().into(),
                series.anchor.as_str().into(),
            );
            Ok(())
        }
        Atom::Mirror { tier, suffix } => {
            let row = select_row(ctx, tier)?;
            table::sync(ctx);
            ui.global::<TierTableModel>()
                .invoke_mirror_to_other_block(row, suffix.as_str().into());
            Ok(())
        }
        Atom::Note { tier, text } => {
            let model = ui.global::<InspectorModel>();
            load_flat_form(ctx, &model, flat_index(ctx, tier)?)?;
            model.set_note_text(text.as_str().into());
            model.invoke_apply_note();
            Ok(())
        }
        Atom::Cheater { tier, text } => {
            let model = ui.global::<InspectorModel>();
            load_flat_form(ctx, &model, flat_index(ctx, tier)?)?;
            model.set_cheater_text(text.as_str().into());
            model.invoke_apply_cheater();
            Ok(())
        }
        Atom::Material(name) => apply_material(&ui, name),
        Atom::Yield { girdle_diameter_mm } => {
            let model = ui.global::<InspectorModel>();
            model.set_girdle_diameter_mm(girdle_diameter_mm.to_string().into());
            model.invoke_apply_yield();
            Ok(())
        }
        Atom::ViewMode(mode) => {
            let tab = judge::tab_for_view_mode(*mode)
                .ok_or_else(|| Stop::say(format!("The browser has no view mode {mode}.")))?;
            show_tab(&ui, tab);
            Ok(())
        }
        Atom::InspectorTab(tab) => {
            let model = ui.global::<InspectorModel>();
            model.set_tab(*tab);
            model.set_collapsed(false);
            Ok(())
        }
        Atom::Command(command) => run_command(ctx, &ui, *command),
    }
}

/// Reads the open design.
fn with_design<R>(ctx: &Ctx, read: impl FnOnce(&Design) -> R) -> Result<R, Stop> {
    let app = ctx
        .state
        .try_borrow()
        .map_err(|_| Stop::say("The editor is busy for a moment."))?;
    let design = app
        .design
        .as_ref()
        .ok_or_else(|| Stop::say("Open a design first."))?;
    Ok(read(&design.session.design))
}

/// Shows view tab `tab` the way a click on it does.
fn show_tab(ui: &AppWindow, tab: i32) {
    let app_model = ui.global::<AppModel>();
    if app_model.get_view_tab() != tab {
        app_model.set_view_tab(tab);
        app_model.invoke_view_tab_changed(tab);
    }
}

/// The table row of the tier called `name` (a flat tier, else a concave one).
fn row_of(ctx: &Ctx, name: &str) -> Result<i32, Stop> {
    with_design(ctx, |design| {
        tier_position(design, name)
            .and_then(|at| row_id(TierRef::Flat(at), design.tiers.len()))
            .or_else(|| {
                concave_position(design, name)
                    .and_then(|at| row_id(TierRef::Concave(at), design.tiers.len()))
            })
    })?
    .ok_or_else(|| Stop::say(format!("There is no tier called '{}'.", name.trim())))
}

/// Picks the row called `name` in the tier table and returns its row id.
fn select_row(ctx: &Ctx, name: &str) -> Result<i32, Stop> {
    let row = row_of(ctx, name)?;
    selection::select(ctx, row);
    Ok(row)
}

/// The position in `design.tiers` of the flat tier called `name`.
fn flat_index(ctx: &Ctx, name: &str) -> Result<usize, Stop> {
    with_design(ctx, |design| tier_position(design, name))?
        .ok_or_else(|| Stop::say(format!("There is no flat tier called '{}'.", name.trim())))
}

/// The row id of the selected tier (flat, else concave).
fn selected_row(ctx: &Ctx) -> Result<i32, Stop> {
    let app = ctx
        .state
        .try_borrow()
        .map_err(|_| Stop::say("The editor is busy for a moment."))?;
    let design = app
        .design
        .as_ref()
        .ok_or_else(|| Stop::say("Open a design first."))?;
    let flat = design.session.design.tiers.len();
    let target = app
        .selected_tier
        .map(TierRef::Flat)
        .or_else(|| design.session.selected_concave.map(TierRef::Concave));
    target
        .and_then(|tier| row_id(tier, flat))
        .ok_or_else(|| Stop::say("Pick a tier in the table first."))
}

/// A palette command, through the control that does it.
fn run_command(ctx: &Ctx, ui: &AppWindow, command: Command) -> Result<(), Stop> {
    let table_model = ui.global::<TierTableModel>();
    match command {
        Command::Solve => table_model.invoke_solve(),
        Command::Undo => ui.global::<AppModel>().invoke_undo(),
        Command::Duplicate => {
            let row = selected_row(ctx)?;
            table::sync(ctx);
            edit::duplicate_tier(ctx, row);
        }
        Command::Delete => {
            selected_row(ctx)?;
            table::sync(ctx);
            table_model.invoke_delete_selected();
        }
        Command::MoveUp | Command::MoveDown => {
            let row = selected_row(ctx)?;
            table::sync(ctx);
            edit::move_tier(ctx, row, if command == Command::MoveUp { -1 } else { 1 });
        }
        Command::AdoptAll => table_model.invoke_adopt_all(),
        Command::Snapshot => ui.global::<SnapshotModel>().invoke_take(),
        Command::Compare => ui.global::<SnapshotModel>().invoke_compare(),
        Command::LiveRender => show_tab(ui, judge::RENDER_TAB),
        Command::Snap => {
            show_tab(ui, judge::SOLID_TAB);
            let manip = ui.global::<ManipulateModel>();
            manip.set_snap_off(!manip.get_snap_off());
            manip.invoke_snap_toggled();
        }
        Command::Slice => {
            show_tab(ui, judge::SOLID_TAB);
            let manip = ui.global::<ManipulateModel>();
            if !manip.get_slice_mode() {
                manip.set_slice_mode(true);
                manip.invoke_slice_toggled();
            }
        }
        // `ShortcutsModel`'s `changed open` raises `shortcuts_opened`, as the menu and `?` do.
        Command::Shortcuts => ui.global::<ShortcutsModel>().set_open(true),
        // `plan` refuses this one before anything runs.
        Command::Manual => {
            return Err(Stop::say(
                "The browser cannot open that dialog for you yet.",
            ));
        }
    }
    // A plain view toggle has nothing to wait for: say what it did, as its control would.
    if let Some(name) = command.reported_event() {
        raise(ctx, name);
    }
    Ok(())
}

/// Makes the Tier form describe the flat tier at `index`: picks its row, lets the inspector
/// load it, and settles a draft that stood in the way.
fn load_flat_form(ctx: &Ctx, model: &InspectorModel<'_>, index: usize) -> Result<(), Stop> {
    let row = i32::try_from(index).map_err(|_| Stop::say("The design has too many tiers."))?;
    selection::select(ctx, row);
    inspector::refresh(ctx);
    if model.get_show_dirty_warning() {
        // The pending row is the one just picked: Discard Draft loads it.
        model.invoke_discard_draft();
    }
    if model.get_loaded_tier_index() != row {
        model.set_loaded_tier_index(row);
    }
    Ok(())
}

/// Opens the Tier form blank for a new tier at the end of the table ("+ Add Tier").
fn blank_flat_form(model: &InspectorModel<'_>) -> Result<(), Stop> {
    if model.get_concave_mode()
        && model.get_concave_dirty()
        && model.get_loaded_concave_index() != -1
    {
        return Err(Stop::say(
            "Save or discard the concave tier you are editing first.",
        ));
    }
    if model.get_form_dirty() && model.get_loaded_tier_index() != -1 {
        model.set_pending_load_index(-1);
        model.invoke_discard_draft();
    }
    model.invoke_new_tier();
    if model.get_concave_mode() || model.get_loaded_tier_index() != -1 {
        return Err(Stop::say("The Tier form could not be opened blank."));
    }
    Ok(())
}

/// The Tier form: "+ Add Tier" or the picked row, then Add Tier or Save Tier with the fields
/// `entry` describes.
fn save_tier(ctx: &Ctx, ui: &AppWindow, entry: &TierEntry) -> Result<(), Stop> {
    let save = with_design(ctx, |design| resolve_tier(design, entry))?.map_err(Stop::Say)?;
    let model = ui.global::<InspectorModel>();
    match usize::try_from(save.index) {
        Ok(index) => load_flat_form(ctx, &model, index)?,
        Err(_) => blank_flat_form(&model)?,
    }
    model.set_form_error(SharedString::new());
    model.set_tier_angle(save.angle.into());
    model.set_tier_constraint_kind(save.constraint_kind);
    model.set_tier_constraint_text(save.constraint_text.into());
    model.set_tier_name(save.name.into());
    model.set_tier_indices(save.indices.into());
    model.invoke_save_tier();
    if model.get_form_error().is_empty() {
        Ok(())
    } else {
        Err(Stop::Reported)
    }
}

/// The concave form: "+ Add Concave Tier" or the picked concave row, then Add or Save with the
/// whole tier typed in.
fn save_concave(
    ctx: &Ctx,
    ui: &AppWindow,
    existing: Option<&str>,
    form: &ConcaveForm,
) -> Result<(), Stop> {
    let model = ui.global::<InspectorModel>();
    match existing {
        Some(name) => open_concave_on(ctx, &model, name)?,
        None => open_concave_blank(&model)?,
    }
    let fields = &form.fields;
    model.set_concave_error(SharedString::new());
    model.set_c_name(fields.name.as_str().into());
    model.set_c_angle(fields.angle_deg.as_str().into());
    model.set_c_indices(fields.indices.as_str().into());
    model.set_c_instructions(fields.instructions.as_str().into());
    model.set_c_tool(form.tool_index);
    model.set_c_theta(fields.tool_azimuth_deg.as_str().into());
    model.set_c_x(fields.x.as_str().into());
    model.set_c_y(fields.y.as_str().into());
    model.set_c_z(fields.z.as_str().into());
    model.set_c_diameter(fields.diameter_ratio.as_str().into());
    model.set_c_tool_angle(fields.tool_angle_deg.as_str().into());
    model.set_c_reciprocating(fields.reciprocating);
    model.invoke_save_concave();
    if model.get_concave_error().is_empty() {
        Ok(())
    } else {
        Err(Stop::Reported)
    }
}

/// Opens the concave form on the concave tier called `name`.
fn open_concave_on(ctx: &Ctx, model: &InspectorModel<'_>, name: &str) -> Result<(), Stop> {
    let at = with_design(ctx, |design| concave_position(design, name))?.ok_or_else(|| {
        Stop::say(format!(
            "There is no concave tier called '{}'.",
            name.trim()
        ))
    })?;
    inspector::concave::open(ctx, Some(at));
    if model.get_concave_show_dirty_warning() {
        model.invoke_concave_discard_draft();
    }
    let wanted = i32::try_from(at).map_err(|_| Stop::say("The design has too many tiers."))?;
    if model.get_concave_mode() && model.get_loaded_concave_index() == wanted {
        Ok(())
    } else {
        Err(Stop::say(
            "The concave form could not be opened on that tier.",
        ))
    }
}

/// Opens the concave form on a blank draft ("+ Add Concave Tier").
fn open_concave_blank(model: &InspectorModel<'_>) -> Result<(), Stop> {
    model.invoke_new_concave();
    if model.get_concave_show_dirty_warning() {
        model.invoke_concave_discard_draft();
    }
    if model.get_concave_mode() && model.get_loaded_concave_index() == -1 {
        Ok(())
    } else {
        Err(Stop::say("The concave form could not be opened blank."))
    }
}

/// Design settings: pick `name` in the Material list and press Apply Material.
fn apply_material(ui: &AppWindow, name: &str) -> Result<(), Stop> {
    let model = ui.global::<DesignSettingsModel>();
    let options = model.get_material_options();
    let wanted = name.trim();
    let index = (0..options.row_count())
        .find(|&at| {
            options
                .row_data(at)
                .is_some_and(|option| option.trim().eq_ignore_ascii_case(wanted))
        })
        .and_then(|at| i32::try_from(at).ok())
        .ok_or_else(|| Stop::say(format!("'{wanted}' is not in the Material list.")))?;
    model.set_material_index(index);
    model.invoke_apply_material();
    Ok(())
}
