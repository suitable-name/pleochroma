//! The Rust half of the Stone tab's Inclusions, Play-of-Color and Abraded Edges groups
//! (`RenderEffectsModel`, `ui/models/render_effects.slint`).
//!
//! The values are the design's render effects and ride the same live path as the other Stone-tab
//! controls: a change shows at once in the render context ([`apply_live`]) and a debounced commit
//! writes ONE undoable `Edit::SetRenderEffects`
//! (`editor::callbacks::tier_actions::viewport_material`), which reads the controls through
//! [`physics_from_controls`] (called by `stone_effects::effects_from_controls`). This module adds
//!
//! - the conversion of the three groups' controls to and from the design ([`physics_from_controls`],
//!   [`push_physics`]);
//! - the groups' derived state: the material's presets, the Cat's-Eye wait for a Second Axis,
//!   whether the material has a Play-of-Color ([`refresh`]);
//! - the "Figures ignore ..." note the Checks strip, the Verdict and the Compare header share
//!   ([`push_figures_note`]);
//! - the silk directions the "Show Silk Directions" overlay draws ([`silk_directions`]).

use super::{crystal_axis_ui, edge_effects, inclusion_effects, stone_effects, structure_effects};
use crate::{
    CrystalAxisModel, MainWindow, RenderEffectsModel, SettingsModel, ViewportModel,
    bridge::render_thread::RenderContext,
};
use glam::Vec3;
use pleochroma::optics::inclusions::bragg::structure_kind;
use pleochroma_cut_core::{Design, StoneRenderEffects};
use pleochroma_editor::stone_orientation::oriented_for_effects;
use slint::{ComponentHandle, Model, SharedString};
use std::sync::{Arc, Mutex};

/// The render physics of `base` with the three groups' controls applied: what the controls say
/// for the inclusions, the play-of-color and the abraded edges, keeping `base`'s own values where
/// a control still holds them. The other fields are the defaults (`with_physics_of` takes only
/// these three).
#[must_use]
pub(in crate::gui) fn physics_from_controls(
    ui: &MainWindow,
    base: &StoneRenderEffects,
) -> StoneRenderEffects {
    StoneRenderEffects {
        inclusions: inclusion_effects::effects_from_controls(
            &inclusion_effects::read_controls(ui),
            base.inclusions.as_ref(),
        ),
        structure: structure_effects::effects_from_controls(
            &structure_effects::read_controls(ui),
            base.structure.as_ref(),
        ),
        edge_abrasion: edge_effects::effects_from_controls(
            edge_effects::read_controls(ui),
            &base.edge_abrasion,
        ),
        ..StoneRenderEffects::default()
    }
}

/// Shows the render physics of `effects` in the three groups' controls.
pub(in crate::gui) fn push_physics(ui: &MainWindow, effects: &StoneRenderEffects) {
    inclusion_effects::write_controls(
        ui,
        &inclusion_effects::controls_for(effects.inclusions.as_ref()),
    );
    structure_effects::write_controls(
        ui,
        &structure_effects::controls_for(effects.structure.as_ref()),
    );
    edge_effects::write_controls(ui, edge_effects::controls_for(&effects.edge_abrasion));
}

/// Recomputes the groups' derived state: the material's preset rows (and which wait for a Second
/// Axis), the note under the Inclusions, whether the Play-of-Color group is offered and its title.
/// Called when the groups are shown and whenever an input changed (the Crystal Axis watcher).
pub(in crate::gui) fn refresh(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    let material = crystal_axis_ui::resolved_material(&RenderContext::lock(render_ctx));
    let name = material.as_ref().map_or("", |m| m.name.as_str());
    let model = ui.global::<RenderEffectsModel>();
    // A Second Axis counts only while the Crystal Axis is set for this stone: the render reads it
    // from there.
    let second_set = ui.global::<SettingsModel>().get_c_axis_override_enabled()
        && ui.global::<CrystalAxisModel>().get_second_set();
    let rows = inclusion_effects::preset_rows(name, second_set);
    inclusion_effects::show_preset_rows(ui, &rows);
    let chosen = model.get_inclusion_preset();
    model.set_inclusion_note(SharedString::from(inclusion_effects::note_for(
        &rows, &chosen,
    )));
    model.set_structure_available(structure_kind(name).is_some() || model.get_structure_on());
    model.set_structure_title(SharedString::from(structure_effects::title_for(name)));
    // A custom material's own families and structure (authored in the Material Editor): shown as
    // read-only lines, and counted by the "Figures ignore ..." note.
    let own_families = material
        .as_ref()
        .map_or(0, inclusion_effects::own_family_count);
    model.set_inclusion_own_count(i32::try_from(own_families).unwrap_or(i32::MAX));
    model.set_inclusion_own_text(SharedString::from(inclusion_effects::own_families_text(
        own_families,
    )));
    model.set_structure_own(
        material
            .as_ref()
            .is_some_and(structure_effects::has_own_structure),
    );
    model.set_structure_own_text(SharedString::from(structure_effects::OWN_STRUCTURE_TEXT));
}

/// A control changed the picture: hold the editor refresh off the controls until the design has
/// the new value, then ask for the debounced commit. Nothing to write for a library design,
/// whose effects are view-only.
fn raise_edited(ui: &MainWindow) {
    if !ui.global::<ViewportModel>().get_shows_editor_design() {
        return;
    }
    stone_effects::mark_live_edit_pending();
    ui.global::<SettingsModel>().invoke_stone_effects_edited();
}

/// Makes the render context trace the controls' current effects and queues their commit: the one
/// path every control of the three groups takes, so each is one set of the controls and one undo
/// step. Recomputes the derived state and the silk lines afterwards.
fn apply_live(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    let effects = stone_effects::effects_from_controls(ui, &StoneRenderEffects::default());
    {
        let mut ctx = RenderContext::lock(render_ctx);
        stone_effects::apply_effects_to_context(&mut ctx, &effects);
    }
    raise_edited(ui);
    // The overlay's refresh recomputes this module's derived state and the silk lines too.
    ui.global::<CrystalAxisModel>().invoke_overlay_changed();
}

/// An inclusion preset button (or "None", the empty name) was chosen.
fn choose_preset(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>, name: &str) {
    let mut controls = inclusion_effects::read_controls(ui);
    if name.is_empty() {
        controls.preset.clear();
    } else {
        let blocked = ui
            .global::<RenderEffectsModel>()
            .get_inclusion_preset_names()
            .iter()
            .zip(
                ui.global::<RenderEffectsModel>()
                    .get_inclusion_preset_blocked()
                    .iter(),
            )
            .any(|(row, blocked)| blocked && row.as_str() == name);
        if blocked {
            return;
        }
        // The first preset starts from the catalogue's own look; a switch between presets keeps
        // the density, turn, sheen and color the person already chose.
        if controls.preset.is_empty() {
            controls = inclusion_effects::InclusionControls::default();
        }
        controls.preset = name.to_string();
    }
    inclusion_effects::write_controls(ui, &controls);
    apply_live(ui, render_ctx);
}

/// Any Play-of-Color control changed. Switching it on starts from the material's own default.
fn structure_edited(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    let controls = structure_effects::read_controls(ui);
    let was_off = RenderContext::lock(render_ctx)
        .scene_effects
        .structure
        .is_none();
    if controls.on && was_off {
        let material = crystal_axis_ui::resolved_material(&RenderContext::lock(render_ctx));
        let defaults = material
            .as_ref()
            .and_then(|m| structure_effects::default_controls_for(&m.name));
        if let Some(defaults) = defaults {
            structure_effects::write_controls(ui, &defaults);
        }
    }
    apply_live(ui, render_ctx);
}

/// The Reseed button: a new seed with every other value kept.
fn reseed(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    let mut controls = structure_effects::read_controls(ui);
    if !controls.on {
        return;
    }
    structure_effects::reseed(&mut controls);
    structure_effects::write_controls(ui, &controls);
    apply_live(ui, render_ctx);
}

/// Wires the callbacks of the three groups. Called once from
/// `render::material::setup_material_effect_override_callbacks`.
pub(in crate::gui) fn setup(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    let model = ui.global::<RenderEffectsModel>();

    let (ctx, weak) = (Arc::clone(render_ctx), ui.as_weak());
    model.on_refresh(move || {
        if let Some(ui) = weak.upgrade() {
            refresh(&ui, &ctx);
        }
    });

    let (ctx, weak) = (Arc::clone(render_ctx), ui.as_weak());
    model.on_inclusion_preset_chosen(move |name: SharedString| {
        if let Some(ui) = weak.upgrade() {
            choose_preset(&ui, &ctx, &name);
        }
    });

    let (ctx, weak) = (Arc::clone(render_ctx), ui.as_weak());
    model.on_inclusions_edited(move || {
        if let Some(ui) = weak.upgrade() {
            apply_live(&ui, &ctx);
        }
    });

    let (ctx, weak) = (Arc::clone(render_ctx), ui.as_weak());
    model.on_structure_edited(move || {
        if let Some(ui) = weak.upgrade() {
            structure_edited(&ui, &ctx);
        }
    });

    let (ctx, weak) = (Arc::clone(render_ctx), ui.as_weak());
    model.on_structure_reseed(move || {
        if let Some(ui) = weak.upgrade() {
            reseed(&ui, &ctx);
        }
    });

    let (ctx, weak) = (Arc::clone(render_ctx), ui.as_weak());
    model.on_edges_edited(move || {
        if let Some(ui) = weak.upgrade() {
            let controls = edge_effects::with_starting_wear(edge_effects::read_controls(&ui));
            edge_effects::write_controls(&ui, controls);
            apply_live(&ui, &ctx);
        }
    });

    // The silk directions are a view aid: nothing is saved. They ride the Crystal Axis overlay,
    // so switching them on switches that overlay on for the session.
    let weak = ui.as_weak();
    model.on_silk_toggled(move |show: bool| {
        if let Some(ui) = weak.upgrade() {
            if show {
                crystal_axis_ui::show_axis_for_session(&ui);
            }
            ui.global::<CrystalAxisModel>().invoke_overlay_changed();
        }
    });
}

// --- The "Figures ignore ..." note --------------------------------------------------------

/// "Figures ignore Inclusions, Surface Finish and Play-of-Color." naming only the ones that are
/// on, or `None` while none is. The optical figures (Brilliance, Fire, the verdict and the
/// compare numbers) come from a separate analytic tracer that draws none of them.
#[must_use]
pub(in crate::gui) fn figures_note_text(
    inclusions: bool,
    surface: bool,
    structure: bool,
) -> Option<String> {
    let names: Vec<&str> = [
        (inclusions, "Inclusions"),
        (surface, "Surface Finish"),
        (structure, "Play-of-Color"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect();
    let list = match names.as_slice() {
        [] => return None,
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    Some(format!("Figures ignore {list}."))
}

/// Which of Inclusions, Surface Finish and Play-of-Color `design` has on (the order of the
/// note).
#[must_use]
pub(in crate::gui) fn figures_flags(design: &Design) -> [bool; 3] {
    [
        design.render_effects.inclusion_spec().is_some(),
        design.has_surface(),
        design.render_effects.structure_spec().is_some(),
    ]
}

/// `flags` (Inclusions, Surface Finish, Play-of-Color) with a custom material's own inclusion
/// families and own structure counted as those two being on.
#[must_use]
pub(in crate::gui) const fn with_own_physics(
    flags: [bool; 3],
    own_inclusions: bool,
    own_structure: bool,
) -> [bool; 3] {
    [
        flags[0] || own_inclusions,
        flags[1],
        flags[2] || own_structure,
    ]
}

/// Shows the note for `design` in `RenderEffectsModel` (empty while none of the three is on).
/// Called on every editor refresh, so a surface finish edit updates it too.
pub(in crate::gui) fn push_figures_note(ui: &MainWindow, design: &Design) {
    let model = ui.global::<RenderEffectsModel>();
    // The material's own families and structure draw too; `refresh` keeps these two current.
    let [inclusions, surface, structure] = with_own_physics(
        figures_flags(design),
        model.get_inclusion_own_count() > 0,
        model.get_structure_own(),
    );
    let note = figures_note_text(inclusions, surface, structure);
    model.set_figures_note(SharedString::from(note.as_deref().unwrap_or("")));
    model.set_figures_note_suffix(SharedString::from(
        note.map(|text| format!(" {text}")).unwrap_or_default(),
    ));
}

// --- The silk directions ----------------------------------------------------------------

/// The directions of the stone's inclusion families in the stone frame (unit vectors, either
/// sign), for the "Show Silk Directions" overlay. Empty while the toggle is off, no preset is
/// on, or the preset resolves to nothing for the traced material. A random-needle family has no
/// direction and gives none.
#[must_use]
pub(in crate::gui) fn silk_directions(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> Vec<Vec3> {
    if !ui.global::<RenderEffectsModel>().get_show_silk_directions() {
        return Vec::new();
    }
    let effects = stone_effects::effects_from_controls(ui, &StoneRenderEffects::default());
    let (material, spec, second) = {
        let ctx = RenderContext::lock(render_ctx);
        let Some(material) = crystal_axis_ui::resolved_material(&ctx) else {
            return Vec::new();
        };
        (
            material,
            ctx.scene_effects.inclusions.clone(),
            ctx.scene_effects
                .inclusion_second_axis
                .map(Vec3::from_array),
        )
    };
    let Some(spec) = spec else {
        return Vec::new();
    };
    let oriented = oriented_for_effects(material, &effects);
    spec.resolve(&oriented, second)
        .map(|resolved| {
            resolved
                .set
                .families()
                .iter()
                .map(|family| family.dir)
                .filter(|dir| dir.length_squared() > 0.0)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::{Edit, InclusionEffects, StructureEffects};
    use pleochroma_editor::EditorSession;

    #[test]
    fn the_note_names_only_the_active_features() {
        assert_eq!(figures_note_text(false, false, false), None);
        assert_eq!(
            figures_note_text(true, false, false).as_deref(),
            Some("Figures ignore Inclusions.")
        );
        assert_eq!(
            figures_note_text(false, true, true).as_deref(),
            Some("Figures ignore Surface Finish and Play-of-Color.")
        );
        assert_eq!(
            figures_note_text(true, true, true).as_deref(),
            Some("Figures ignore Inclusions, Surface Finish and Play-of-Color.")
        );
    }

    #[test]
    fn a_materials_own_physics_counts_in_the_note() {
        let own = with_own_physics([false, true, false], true, true);
        assert_eq!(own, [true, true, true]);
        assert_eq!(
            figures_note_text(own[0], own[1], own[2]).as_deref(),
            Some("Figures ignore Inclusions, Surface Finish and Play-of-Color.")
        );
        assert_eq!(
            with_own_physics([false; 3], false, false),
            [false, false, false]
        );
        let only_families = with_own_physics([false; 3], true, false);
        assert_eq!(
            figures_note_text(only_families[0], only_families[1], only_families[2]).as_deref(),
            Some("Figures ignore Inclusions.")
        );
    }

    /// The groups' choices commit as ONE undoable edit, and an unchanged write-back is no edit.
    #[test]
    fn the_three_groups_commit_as_one_undo_step() {
        let mut session = EditorSession::fresh();
        let effects = StoneRenderEffects {
            inclusions: InclusionEffects::of_preset("Fine Silk"),
            structure: Some(StructureEffects::from_spec(
                &pleochroma::optics::inclusions::bragg::StructureSpec::default_for("Labradorite")
                    .expect("spec"),
            )),
            edge_abrasion: pleochroma_cut_core::EdgeAbrasionEffects {
                width: 0.02,
                alpha: 0.3,
                halo_rq_nm: 0.0,
            },
            ..StoneRenderEffects::default()
        };
        let edit = stone_effects::effects_edit(&session.design, effects).expect("a change");
        assert!(matches!(edit, Edit::SetRenderEffects { .. }));
        session.apply(edit).expect("the effects apply");
        assert!(stone_effects::effects_edit(&session.design, effects).is_none());
        session.undo().expect("undoes").expect("something to undo");
        assert!(session.design.render_effects.is_default());
    }
}
