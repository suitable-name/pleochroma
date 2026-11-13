//! The design settings panel and the viewport material link.
//!
//! Covers [`refresh_design_settings`], the "linked to design" material sync
//! ([`sync_viewport_material_link`], which traces with
//! `pleochroma_editor::material_lookup::traced_material_for`), the material-guess badge, and
//! the Tier tab's per-facet chip row ([`push_selected_tier_chips`]).

use super::state::{
    EditorState, ScratchDelta, builtin_preset_names, design_material_index_from_name,
    gear_index_from_teeth, index_chip_items, push_rows, ri_source_text,
};
use crate::{
    EditorModel, IndexChipItem, MainWindow, SettingsModel, ViewportModel,
    bridge::render_thread::{PlanesOwner, RenderContext},
    gui::{
        editor::{
            body_color_combo,
            material_lookup::{
                EditorMaterialLookup, applied_material_guess, material_guess,
                traced_gem_material_at_width, traced_material_for,
            },
        },
        optics::color_variant_store::saved_variants,
        render::{
            physics_effects_ui::push_figures_note, render_body_color::display_rgb_for_stone,
            stone_effects,
        },
    },
};
use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::{Design, TierFrosting, critical_angle_deg};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::sync::{Arc, Mutex};

/// The material-guess badge of [`refresh_design_settings`]. Split out to stay under the
/// function-length lint.
///
/// An inferred material is shown as a guess, never as a fact. When `design.material.name` is
/// `None` (an untouched `.asc` import carries only an `I`-line refractive index), the nearest
/// built-in preset within tolerance is offered as a guess, with the other close candidates in
/// the tooltip. Once a name is set, the three guess properties are cleared.
///
/// A material applied from a library entry (`EditorState::applied_library_material`) keeps the
/// badge while it is still the design's material. There is no "Set Material" button for it
/// (`material_guess_applied`), and it stays labelled as inferred.
fn push_material_guess(ui: &MainWindow, design: &Design, n_d: f64, applied: Option<&str>) {
    let guess = material_guess(design, n_d);
    let applied_guess = if guess.is_none() {
        applied_material_guess(design, n_d, applied)
    } else {
        None
    };
    ui.global::<EditorModel>()
        .set_material_guess_applied(applied_guess.is_some());
    let (text, name, others) = guess.or(applied_guess).unwrap_or_default();
    ui.global::<EditorModel>()
        .set_material_guess_text(text.into());
    ui.global::<EditorModel>()
        .set_material_guess_name(name.into());
    ui.global::<EditorModel>()
        .set_material_guess_other_candidates_text(others.into());
}

/// The Live Render readout for a design's body color: the traced material's name with the
/// color's label (`Sapphire (Yellow)`), or `""` when the design sets no color or nothing traces
/// (`traced_material_for`'s refusal). It names the same stone as
/// `sync_viewport_material_link`, computed whether or not the viewport is linked. A custom
/// color's hex is taken at the current stone width (`stone_width_mm`, `0.0` = not set), like
/// the toolbar swatch.
fn body_color_readout(design: &Design, custom: &[GemMaterial], stone_width_mm: f32) -> String {
    let rgb = design.material.body_color_override;
    // A color variant is named by its own label (`Sapphire (Padparadscha)`); the legacy colors
    // keep their preset label or on-screen hex.
    let is_variant = design.material.color_variant_override.is_some();
    design
        .material
        .color_label()
        .map_or_else(String::new, |color| {
            let (name, unresolved) = traced_material_for(design, custom);
            if unresolved.is_some() {
                String::new()
            } else if let Some(rgb) =
                rgb.filter(|rgb| !is_variant && body_color_combo::is_custom(Some(*rgb)))
            {
                // A custom colour is named by its on-screen hex, not just "custom color".
                let [r, g, b] = display_rgb_for_stone(rgb, stone_width_mm);
                format!("{name} (Custom #{r:02x}{g:02x}{b:02x})")
            } else {
                format!("{name} ({color})")
            }
        })
}

/// Whether the design's material is a custom material whose color is a physics recipe -- the
/// explicit `RenderContext::custom_material_physics` list, never inferred from the bands.
fn is_physics_material(design: &Design, ctx: &RenderContext) -> bool {
    design.material.name.as_deref().is_some_and(|name| {
        ctx.custom_material_physics
            .iter()
            .any(|n| n.eq_ignore_ascii_case(name))
    })
}

/// Whether the design's material is a custom material with colour zones. Its colour is its
/// zones, so the design's body-colour override does not apply to it (the cut-core
/// `MaterialSelection::apply_overrides` ignores it) and the Stone tab greys the control out.
fn is_zoned_material(design: &Design, ctx: &RenderContext) -> bool {
    design.material.color_pattern_override.is_some()
        || design.material.name.as_deref().is_some_and(|name| {
            ctx.custom_materials
                .iter()
                .chain(GemMaterial::built_in())
                .any(|m| m.zoning.is_some() && m.name.eq_ignore_ascii_case(name))
        })
}

/// The color combo half of [`refresh_design_settings`]. The option list is the keyed list of
/// [`body_color_combo::shown_colors`]: the material's own color, its color variants, the
/// presets and the trailing "Custom..." entry. The labels are pushed only when the list
/// changed; swatches and tooltips are pushed every time. The selected index is re-seeded when
/// `material_changed` (the same
/// [`ScratchDelta::material`] gate as the material combo, so an in-progress pick survives an
/// unrelated refresh) or when the list changed. `readout` (see [`body_color_readout`]) is
/// always pushed.
fn push_body_color_fields(
    ui: &MainWindow,
    design: &Design,
    readout: String,
    (physics_material, zoned_material): (bool, bool),
    shown: &body_color_combo::ShownColors,
    material_changed: bool,
) {
    let model = ui.global::<EditorModel>();
    let labels: Vec<SharedString> = shown
        .items
        .iter()
        .map(|item| SharedString::from(item.label.as_str()))
        .collect();
    let current = model.get_body_color_options();
    let list_unchanged = current.row_count() == labels.len()
        && current
            .iter()
            .zip(labels.iter())
            .all(|(shown_now, new)| shown_now == *new);
    if !list_unchanged {
        model.set_body_color_options(ModelRc::new(VecModel::from(labels)));
    }
    // The swatch chip and the tooltip of each row (a combo row has neither of its own).
    model.set_body_color_swatches(ModelRc::new(VecModel::from(
        shown
            .items
            .iter()
            .map(|item| {
                item.swatch_rgb().map_or_else(
                    || slint::Color::from_argb_u8(0, 0, 0, 0),
                    |[r, g, b]| slint::Color::from_rgb_u8(r, g, b),
                )
            })
            .collect::<Vec<_>>(),
    )));
    model.set_body_color_tips(ModelRc::new(VecModel::from(
        shown
            .items
            .iter()
            .map(|item| SharedString::from(item.tooltip.as_str()))
            .collect::<Vec<_>>(),
    )));
    if material_changed || !list_unchanged {
        model.set_body_color_index(i32::try_from(shown.selected).unwrap_or(0));
    }
    // A legacy single color on a pleochroic material keeps rendering as saved. The Stone tab
    // says so and offers "Convert...".
    model.set_body_color_legacy_note(shown.legacy_note);
    // A snapshot of a saved variant that was edited since: "Variant Changed: Update".
    model.set_body_color_variant_stale(shown.stale_variant);
    model.set_body_color_readout(readout.into());
    // The override replaces the whole absorption tensor, so a physics material greys it out.
    // One already set (a color variant included) gets a warning instead.
    model.set_design_material_is_physics(physics_material);
    // A zoned material's colour is its zones: the override is ignored for it, so the control is
    // greyed out with a note (the Stone tab) instead of silently doing nothing.
    model.set_design_material_is_zoned(zoned_material);
    model.set_physics_override_conflict(
        physics_material && design.material.has_body_color_override(),
    );
}

/// Pushes the design settings panel: the material combo, the refractive index and critical
/// angle readouts, and the gear, symmetry and mirror fields. While "linked to design" is on,
/// it also syncs the viewport's render material and its displayed selection. Used by
/// [`super::panel::refresh_editor_panel_from_solve`] and
/// [`super::panel_stale::push_stale_content`].
///
/// The sync is gated on `viewport_material_linked` alone, so a display override made while
/// unlinked is never overwritten.
///
/// Returns the design's effective refractive index, which `tier_items` and
/// `tier_items_stale_with_last_solved` reuse for the per-tier margin column.
///
/// `delta` (from [`EditorState::record_scratch_push`], computed once by the caller) gates the
/// material, gear and symmetry field pushes separately, so an edit that changed only one of
/// them does not reseed the others and discard typing in progress.
pub(super) fn refresh_design_settings(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    delta: &ScratchDelta,
) -> f64 {
    let design = &state.design;

    // The `RenderContext` lock (the render thread takes it every frame) is held only in this
    // block, not across the `delta`-gated pushes below, which need only `design` and `ui`.
    let (n_d, options, color_readout, physics_material, zoned_material, shown_colors) = {
        let ctx = RenderContext::lock(render_ctx);
        // The color combo's keyed list for the design's material (its variants and presets). The
        // cutter's saved variants of the host come from the main window's vault without waiting
        // for it (`saved_variants`: a busy vault keeps the list last read).
        let saved = body_color_combo::host_material(design, &ctx.custom_materials)
            .map_or_else(Vec::new, |host| saved_variants(&host.name));
        let shown_colors = body_color_combo::shown_colors(design, &ctx.custom_materials, &saved);
        let zoned_material = is_zoned_material(design, &ctx);
        // A zoned material ignores the override, so the readout never names a colour it lacks.
        let color_readout = if zoned_material {
            String::new()
        } else {
            body_color_readout(design, &ctx.custom_materials, ctx.stone_width_mm)
        };
        // Unlike `effective_refractive_index`, this also resolves a custom material by name.
        let n_d = design.effective_refractive_index_with(&ctx.custom_materials);
        ui.global::<EditorModel>()
            .set_ri_source_text(ri_source_text(&design.material, &ctx.custom_materials).into());
        // Custom materials can change any time, independently of `design` -- this
        // option LIST is always refreshed; only the in-out `*_index`/`*_text`
        // selections below are gated on `delta`.
        let options = state.material_combo_options(&ctx.custom_materials);
        let physics_material = is_physics_material(design, &ctx);
        // The design's own traced material, published ahead of the link gate so "Preview as..."
        // can tell the design's material from another one even while a preview is running
        // (`ctx.material_name` is then the preview's).
        ui.global::<ViewportModel>()
            .set_design_material_name(traced_material_for(design, &ctx.custom_materials).0.into());
        drop(ctx);
        (
            n_d,
            options,
            color_readout,
            physics_material,
            zoned_material,
            shown_colors,
        )
    };
    push_body_color_fields(
        ui,
        design,
        color_readout,
        (physics_material, zoned_material),
        &shown_colors,
        delta.material,
    );
    // The stone width (and the zone scale that follows it) is synced before the material link,
    // which derives `material_override` from the re-scaled list.
    push_stone_fields(ui, render_ctx, design);
    // A second lock, so the first is not held across the pushes below.
    let (selected_material_index, c_axis_available) = {
        let mut ctx = RenderContext::lock(render_ctx);
        let index = sync_viewport_material_link(ui, &mut ctx, design);
        let c_axis_available = traced_c_axis_available(&ctx);
        drop(ctx);
        (index, c_axis_available)
    };
    // Set only after the `render_ctx` guard above is dropped -- see
    // `sync_viewport_material_link`'s own doc comment.
    if let Some(idx) = selected_material_index {
        ui.global::<ViewportModel>()
            .set_selected_material_index(idx);
    }
    // The Crystal Axis controls follow the material the view traces: a design switched from
    // an isotropic material to an anisotropic one must offer them (and the reverse).
    ui.global::<SettingsModel>()
        .set_c_axis_override_available(c_axis_available);
    // The Stone tab's Inclusions and Play-of-Color groups follow the traced material too.
    crate::gui::render::physics_effects_ui::refresh(ui, render_ctx);
    push_figures_note(ui, design);
    push_material_options(ui, &options);
    if delta.material {
        ui.global::<EditorModel>()
            .set_material_combo_index(design_material_index_from_name(
                design.material.name.as_deref(),
                &options,
            ));
        ui.global::<EditorModel>().set_ri_override_text(
            design
                .material
                .refractive_index_override
                .map_or_else(String::new, |v| format!("{v:.4}"))
                .into(),
        );
    }
    ui.global::<EditorModel>()
        .set_effective_ri_text(format!("{n_d:.4}").into());
    ui.global::<EditorModel>()
        .set_critical_angle_text(format!("{:.2}\u{b0}", critical_angle_deg(n_d)).into());
    push_material_guess(ui, design, n_d, state.applied_library_material.as_deref());
    push_gear_symmetry_meta(ui, design, delta);

    n_d
}

/// The stone width and render effects are the design's own, preview or not: syncs the render
/// context from the design, pushes the Stone tab's controls when they differ and no live edit
/// waits to commit, and follows a width change (mirror property, swatches, zoning hook).
fn push_stone_fields(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>, design: &Design) {
    let stone = {
        let mut ctx = RenderContext::lock(render_ctx);
        sync_stone_from_design(&mut ctx, design)
    };
    if !stone_effects::live_edit_pending()
        && (stone.effects_applied
            || stone_effects::effects_from_controls(ui, &design.render_effects)
                != design.render_effects)
    {
        stone_effects::push_effects_to_controls(ui, &design.render_effects);
    }
    // The "Figures ignore ..." note of the Checks strip and the Verdict.
    push_figures_note(ui, design);
    if stone.width_changed {
        // A read-only mirror of the design's width for the code that sizes library zones
        // (`SettingsModel.stone_width_mm`; the design is the only place the width is edited).
        #[expect(
            clippy::cast_possible_truncation,
            reason = "f32 is the storage precision of the stone width in mm"
        )]
        let girdle_mm = design.girdle_diameter_mm.unwrap_or(0.0) as f32;
        ui.global::<SettingsModel>().set_stone_width_mm(girdle_mm);
        // The toolbar swatches show the colour at this size.
        ui.global::<ViewportModel>().invoke_render_color_refresh();
        // Library zones stored "relative to stone" follow the new width.
        crate::gui::rough_colour::material_zones::stone_width_applied(render_ctx, girdle_mm);
    }
    if stone.effects_applied {
        // Color patterns that follow the Crystal Axis turn with the design's axis.
        crate::gui::color_pattern::follow::axis_applied(render_ctx);
    }
}

/// Pushes the material combo's option list and the New Design dialog's material list.
fn push_material_options(ui: &MainWindow, options: &[String]) {
    // `options` is cached by `EditorState::material_combo_options`. Push a new model only
    // when the list differs, since a new model resets the combo box rows.
    let current_options = ui.global::<EditorModel>().get_material_combo_options();
    let unchanged = current_options.row_count() == options.len()
        && current_options
            .iter()
            .zip(options.iter())
            .all(|(current, new)| current == new.as_str());
    if !unchanged {
        ui.global::<EditorModel>()
            .set_material_combo_options(ModelRc::new(VecModel::from(
                options
                    .iter()
                    .cloned()
                    .map(SharedString::from)
                    .collect::<Vec<_>>(),
            )));
    }
    // The New Design dialog's material list, so it is not a literal kept in step with
    // `builtin_preset_names` in Slint. The list is static; it is pushed here with the others.
    ui.global::<EditorModel>()
        .set_new_material_options(ModelRc::new(VecModel::from(
            builtin_preset_names()
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
}

/// The gear, symmetry and header/footnote scratch pushes, each gated on its own `delta` flag so
/// an edit that changed one never re-seeds (and discards typing in) the others.
fn push_gear_symmetry_meta(ui: &MainWindow, design: &Design, delta: &ScratchDelta) {
    if delta.gear {
        ui.global::<EditorModel>()
            .set_gear_index(gear_index_from_teeth(design.meta.gear_teeth));
        ui.global::<EditorModel>()
            .set_gear_custom_text(design.meta.gear_teeth.to_string().into());
    }
    if delta.symmetry {
        ui.global::<EditorModel>()
            .set_symmetry_order_text(design.meta.symmetry_order.to_string().into());
        ui.global::<EditorModel>().set_mirror(design.meta.mirror);
        // A typed-but-unapplied preview no longer matches the reseeded design, so it is cleared.
        ui.global::<EditorModel>()
            .set_symmetry_preview_text("".into());
    }
    if delta.meta {
        // The first header is the design's title; the rest are extra header lines, joined
        // with `;` for the form's single-line field.
        let (title, extra_headers) = design
            .meta
            .headers
            .split_first()
            .map_or((String::new(), String::new()), |(title, rest)| {
                (title.clone(), rest.join(";"))
            });
        ui.global::<EditorModel>().set_design_title(title.into());
        ui.global::<EditorModel>()
            .set_design_extra_headers(extra_headers.into());
        ui.global::<EditorModel>()
            .set_design_footnotes(design.meta.footnotes.join(";").into());
        ui.global::<EditorModel>().set_design_gear_reference_angle(
            format!("{}", design.meta.gear_reference_angle).into(),
        );
    }
}

/// The viewport's material sync, which runs while the view follows the design (not while a
/// material preview is on). The stone width and effects are synced by
/// [`sync_stone_from_design`]. Split out to stay under the function-length lint.
///
/// Returns the index of the traced material in the viewport's material options, rather than
/// setting `ViewportModel.selected_material_index` here. That setter's `changed` handler runs a
/// DB query, and the caller holds the `render_ctx` lock across this call, which would stall the
/// render thread. The caller sets the index after dropping the lock.
#[must_use]
pub(in crate::gui::editor) fn sync_viewport_material_link(
    ui: &MainWindow,
    ctx: &mut RenderContext,
    design: &Design,
) -> Option<i32> {
    // A library design on the viewport is not this design's picture: never sync it.
    if matches!(ctx.planes_owner, PlanesOwner::Catalogue { .. })
        || !ui.global::<ViewportModel>().get_viewport_material_linked()
    {
        return None;
    }
    let (name, unresolved) = traced_material_for(design, &ctx.custom_materials);
    if ctx.material_unresolved != unresolved {
        ctx.material_unresolved.clone_from(&unresolved);
        ctx.dirty = true;
    }
    // The resolved material itself, not only its name: `material_override` is what the
    // tracer, the HUD metrics, the tilt sweep and the hover preview use before a name lookup.
    //
    // It is keyed on `name`, the material the design is traced as, and not on the raw
    // `design.material`. A raw selection with no name falls back to diamond in
    // `MaterialSelection::resolve`, and that override would then beat the name. Here
    // `traced_gem_material_at_width` returns `None` instead, so the name and the override
    // always describe the same stone. The body-color variant is carried in the override,
    // while `material_name` stays the species.
    let resolved = unresolved
        .is_none()
        .then(|| {
            traced_gem_material_at_width(
                &name,
                &design.material,
                &EditorMaterialLookup::new(&ctx.custom_materials),
                design.girdle_diameter_mm.unwrap_or(7.0),
            )
        })
        .flatten();
    if ctx.material_override != resolved {
        ctx.material_override = resolved;
        ctx.dirty = true;
    }
    if ctx.material_name != name {
        // Cloned, not moved -- `name` is looked up again just below to sync
        // the dropdown's own selected index. `clone_from` reuses the existing
        // allocation instead of dropping it for a fresh one.
        ctx.material_name.clone_from(&name);
        ctx.dirty = true;
    }
    // The displayed selection follows the traced material on every refresh, so it never
    // shows a material the trace no longer uses.
    let options = ui.global::<ViewportModel>().get_material_options();
    crate::gui::startup_settings::find_option_index(&options, &name)
}

/// Whether the material the view traces now has a crystal axis to orient: the design sync's
/// `material_override` when it set one, else the traced name looked up like the render does.
pub(in crate::gui::editor) fn traced_c_axis_available(ctx: &RenderContext) -> bool {
    ctx.material_override.as_ref().map_or_else(
        || {
            crate::bridge::render_thread::resolve_material(
                &GemMaterial::all_materials(),
                &ctx.custom_materials,
                &ctx.material_name,
            )
            .as_ref()
            .is_some_and(crate::gui::is_c_axis_override_available)
        },
        crate::gui::is_c_axis_override_available,
    )
}

/// What [`sync_stone_from_design`] changed.
pub(in crate::gui::editor) struct StoneSync {
    /// The stone width moved (the zoning hook rescales library zones, and the swatches follow).
    pub width_changed: bool,
    /// The design's effects were pushed into the render context.
    pub effects_applied: bool,
}

/// Makes the render context's stone width and render effects the design's own. Unlike the
/// material, they follow the design whether or not a material preview is on, so there is one
/// stone width, `design.girdle_diameter_mm`.
///
/// The effects are left alone while a Stone-tab control waits to commit its change to the design
/// ([`stone_effects::live_edit_pending`]), so a refresh during a slider drag does not take the
/// slider's value back. The caller pushes the controls after dropping the context lock.
#[must_use]
pub(in crate::gui::editor) fn sync_stone_from_design(
    ctx: &mut RenderContext,
    design: &Design,
) -> StoneSync {
    // A library design on the viewport keeps its own view-only width and effects.
    if matches!(ctx.planes_owner, PlanesOwner::Catalogue { .. }) {
        return StoneSync {
            width_changed: false,
            effects_applied: false,
        };
    }
    // 0.0 = not set in the design: the render assumes the 7 mm the built-in colours are tuned for.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 is the render-context precision of the stone width in mm"
    )]
    let stone_width_mm = design.girdle_diameter_mm.unwrap_or(0.0) as f32;
    let width_changed = (ctx.stone_width_mm - stone_width_mm).abs() > f32::EPSILON;
    if width_changed {
        ctx.stone_width_mm = stone_width_mm;
        ctx.dirty = true;
    }
    let effects_applied = !stone_effects::live_edit_pending()
        && !stone_effects::context_matches(ctx, &design.render_effects);
    if effects_applied {
        stone_effects::apply_effects_to_context(ctx, &design.render_effects);
    }
    StoneSync {
        width_changed,
        effects_applied,
    }
}

/// Builds the per-facet chip row for the tier loaded in the Tier tab
/// (`EditorModel.selected_tier_index`; empty for "Add Tier" or an out-of-range value).
/// It is built on the UI thread for that one tier only, because [`crate::EditorTierItem`]
/// is built on a background thread and must be `Send`.
#[must_use]
fn selected_tier_chips(design: &Design, selected_tier_index: i32) -> Vec<IndexChipItem> {
    let Some(index) = usize::try_from(selected_tier_index).ok() else {
        return Vec::new();
    };
    let Some(tier) = design.tiers.get(index) else {
        return Vec::new();
    };
    let frosting = design.tier_frosting(index);
    let mut chips = index_chip_items(&tier.indices, &tier.detached);
    if let Some(frosting) = frosting {
        // A chip's position is the tier's own `f64` rounded to `f32`, so match by position
        // in the tier's index list rather than by value.
        for (chip, &stored) in chips.iter_mut().zip(&tier.indices) {
            chip.frosted = frosting.covers(stored);
        }
    }
    chips
}

/// Pushes the selected tier's chips, cheater offset, note and frosting into `EditorModel`.
/// Called after every change of the selected tier: a refresh, a stale refresh and a plain
/// row or facet click.
pub(in crate::gui::editor) fn push_selected_tier_chips(ui: &MainWindow, state: &EditorState) {
    let selected = ui.global::<EditorModel>().get_selected_tier_index();
    let cheater_offset_text = usize::try_from(selected)
        .ok()
        .and_then(|index| state.design.cheater_offset_deg(index))
        .map_or_else(String::new, |deg| format!("{deg:.2}"));
    ui.global::<EditorModel>()
        .set_selected_tier_cheater_offset_text(cheater_offset_text.into());
    let note_text = usize::try_from(selected)
        .ok()
        .and_then(|index| state.design.tier_note(index))
        .map_or_else(String::new, str::to_string);
    ui.global::<EditorModel>()
        .set_selected_tier_note_text(note_text.into());
    // How the loaded tier is frosted: the whole tier (the Frosted tick box) or only some facets
    // (a note under it; the chips mark which).
    let loaded = usize::try_from(selected).ok();
    let frosting = loaded.and_then(|index| state.design.tier_frosting(index));
    ui.global::<EditorModel>()
        .set_selected_tier_frosted(matches!(frosting, Some(TierFrosting::Tier)));
    ui.global::<EditorModel>()
        .set_selected_tier_frosted_partly(matches!(frosting, Some(TierFrosting::Facets(_))));
    let chips = selected_tier_chips(&state.design, selected);
    push_rows(
        &ui.global::<EditorModel>().get_selected_tier_chips(),
        chips,
        |model| ui.global::<EditorModel>().set_selected_tier_chips(model),
    );
    // The Surface Finish section of the same tier (`SurfaceModel`).
    crate::gui::editor::callbacks::publish_selected_tier(ui, &state.design);
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::geometry::meet_solver::MeetConstraint;
    use pleochroma_cut_core::ConstraintTier;

    fn design_with_named_tiers(names: &[&str]) -> Design {
        let mut state = EditorState::fresh();
        for &name in names {
            state.design.tiers.push(ConstraintTier {
                angle_deg: -40.0,
                name: name.to_string(),
                indices: vec![0.0],
                constraint: MeetConstraint::MeetExisting,
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            });
        }
        state.session.design
    }

    // --- selected_tier_chips ---

    #[test]
    fn selected_tier_chips_is_empty_when_nothing_is_selected() {
        let design = design_with_named_tiers(&["G1"]);
        assert_eq!(selected_tier_chips(&design, -1).len(), 0);
    }

    #[test]
    fn selected_tier_chips_is_empty_for_an_out_of_range_index() {
        let design = design_with_named_tiers(&["G1"]);
        assert_eq!(selected_tier_chips(&design, 5).len(), 0);
    }

    #[test]
    fn selected_tier_chips_reads_the_selected_tiers_own_indices_and_detached_set() {
        let mut design = design_with_named_tiers(&["G1", "P1"]);
        design.tiers[1].indices = vec![0.0, 24.0];
        design.tiers[1].detached = vec![24.0];
        let chips = selected_tier_chips(&design, 1);
        assert_eq!(chips.len(), 2);
        assert!(!chips[0].detached);
        assert!(chips[1].detached);
    }

    /// A refresh must not take a slider's value back while its commit is pending, and must
    /// apply the design's own effects once nothing is pending; a library design on the viewport
    /// is never synced.
    #[test]
    fn the_design_effects_wait_for_a_pending_edit_and_skip_library_views() {
        let _guard = stone_effects::PENDING_FLAG_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut design = EditorState::fresh().design.clone();
        design.render_effects = pleochroma_cut_core::StoneRenderEffects::from_render_values(
            0.5, true, 30.0, 120.0, None, true, 0.01,
        );

        stone_effects::mark_live_edit_pending();
        let mut ctx = RenderContext::default();
        assert!(!sync_stone_from_design(&mut ctx, &design).effects_applied);
        assert!(!stone_effects::context_matches(
            &ctx,
            &design.render_effects
        ));

        stone_effects::clear_live_edit_pending();
        ctx.planes_owner = PlanesOwner::Catalogue { entry_id: 1 };
        assert!(!sync_stone_from_design(&mut ctx, &design).effects_applied);

        ctx.planes_owner = PlanesOwner::Editor { generation: 1 };
        assert!(sync_stone_from_design(&mut ctx, &design).effects_applied);
        assert!(stone_effects::context_matches(&ctx, &design.render_effects));
    }
}
