//! The design settings panel's refresh ([`refresh_design_settings`]), the
//! "linked to design" viewport material sync ([`sync_viewport_material_link`], over
//! `pleochroma_editor::material_lookup::traced_material_for`), the material-guess
//! badge, and the Tier tab's
//! per-facet chip row ([`selected_tier_chips`]/[`push_selected_tier_chips`]).

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

/// The material-guess badge's push half of [`refresh_design_settings`] --
/// split out purely to keep that function under clippy's function-length
/// lint. Inferred material is shown as a guess, never as a fact: when
/// `design.material.name` is `None`
/// (every untouched `.asc` import carries only an `I`-line refractive index,
/// no species), this looks up the nearest built-in preset within tolerance
/// (`pleochroma_editor::material_lookup::material_guess`) and pushes a guess label
/// plus the OTHER close candidates for the tooltip.
/// Once a name IS set, every one of these three properties goes back to
/// `""` -- there is nothing left to guess, and [`super::state::
/// proportion_verdicts`]-style "guess vs fact" confusion is exactly what this
/// exists to prevent.
///
/// A material the load applied from the library entry
/// (`EditorState::applied_library_material`) keeps a badge too, while the design's material
/// still is that one: it is the design's material (no "Set Material" button, see
/// `material_guess_applied`), but still labelled as inferred so it is never mistaken for a
/// cutter's choice.
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

/// The Live Render readout for a design's body-color variant: the traced material's
/// name with the color's label (`Sapphire (Yellow)`), or `""` when the design sets
/// no color or nothing traces at all (`traced_material_for`'s refusal) -- the same
/// name `sync_viewport_material_link` traces, so the readout can never name a
/// different stone than the one rendered. Computed whether or not the viewport is
/// linked; the toolbar only shows it while linked. A custom colour's hex is the colour at the
/// current Stone Size (`stone_width_mm`, `0.0` = not set), like the toolbar swatch; it is
/// recomputed with the next editor refresh after the size changes.
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

/// The design settings panel's color combo half of [`refresh_design_settings`] --
/// split out purely to keep that function under clippy's function-length lint. The
/// option list is the keyed list of [`body_color_combo::shown_colors`]: the material's own
/// color, its color variants, the nine presets (a material that looks the same in every
/// direction only) and the trailing "Custom..." entry. Its labels are pushed only while the combo
/// does not hold them yet; the swatch of each row and its tooltip with them. The selected
/// index is re-seeded from the design when `material_changed` (the same
/// [`ScratchDelta::material`] gate as the material combo, so an in-progress pick survives an
/// unrelated refresh) or when the list itself changed (another material, another set of
/// variants); `readout` (see [`body_color_readout`]) always.
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
    // The swatch chip and the tooltip text of each row (a combo row has neither of its own):
    // the Stone tab shows the selected row's.
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
    // A legacy single color on a pleochroic material keeps rendering as saved; the Stone tab
    // says so and offers "Convert".
    model.set_body_color_legacy_note(shown.legacy_note);
    // A snapshot of a saved variant that was edited since: "Variant Changed: Update".
    model.set_body_color_variant_stale(shown.stale_variant);
    model.set_body_color_readout(readout.into());
    // The override replaces the whole absorption tensor, so it is unavailable (greyed out) for a
    // physics material; one that is already set (a color variant included) gets a warning rather
    // than a silent override.
    model.set_design_material_is_physics(physics_material);
    // A zoned material's colour is its zones: the override is ignored for it, so the control is
    // greyed out with a note (the Stone tab) instead of silently doing nothing.
    model.set_design_material_is_zoned(zoned_material);
    model.set_physics_override_conflict(
        physics_material && design.material.has_body_color_override(),
    );
}

/// Pushes the design settings panel's state (material combo options and index,
/// RI-override/effective-RI/critical-angle readouts, gear/symmetry/mirror) and,
/// while "linked to design" is on, syncs the shared viewport's render material
/// (and its displayed selection/index) to match -- a display override made while
/// unlinked must never be silently overridden. Shared by
/// [`super::panel::refresh_editor_panel_from_solve`]/
/// [`super::panel_stale::push_stale_content`].
///
/// `viewport_material_linked` alone is the real gate a display override needs.
/// Also requiring `render_view_tab == 1` (the Edit tab itself being shown)
/// would tie the render's material to tab history rather than to the design:
/// switching to Live Render and back would never resync anything until the
/// next edit happened to run this function again.
///
/// Returns this design's effective refractive index so
/// [`super::state::tier_items`]/[`super::state::tier_items_stale`] can reuse the
/// identical value for their per-tier margin/risk column rather than
/// re-deriving it.
///
/// `delta` (from [`EditorState::record_scratch_push`], computed once by the
/// caller) gates the material/gear/symmetry scratch pushes independently, so
/// an edit that only changed one of the three never re-seeds -- and so
/// silently discards any in-progress typing/selection in -- the other two's
/// fields.
///
/// `pub(super)`, not private: shared by `panel.rs`/`panel_stale.rs` (both
/// siblings within `view`), neither of which touches the design settings panel
/// directly otherwise.
pub(super) fn refresh_design_settings(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    delta: &ScratchDelta,
) -> f64 {
    let design = &state.design;

    // Scoped so the `RenderContext` lock (a shared resource the render thread
    // also wants every frame) is held only for the work that actually needs
    // it, and is a real `Drop` guard released at the end of this block rather
    // than sitting locked across the `delta`-gated pushes below, which touch
    // only `design`/`ui` -- clippy::nursery's `significant_drop_tightening`.
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
        // Custom-catalogue-aware: unlike `effective_refractive_index`,
        // this also resolves a custom material by name before falling back to a built-in
        // or the design's `I` line -- see `ri_source_text` below for the matching
        // "where did this number come from" explanation shown in the inspector.
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
    // The stone width (and the zone scale that follows it) is synced BEFORE the
    // viewport material link: the link derives `material_override` from the re-scaled list, and
    // `stone_width_applied` replaces a same-named override with the bare list entry.
    push_stone_fields(ui, render_ctx, design);
    // A second, separate lock acquisition (rather than reusing the guard
    // above): its own last use sits right at this block's own end, so the
    // lock is never held across work -- the `delta`-gated pushes below -- that
    // does not need it either.
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
    // The Stone tab's Inclusions and Play-of-Color groups follow the traced material too (its
    // presets, whether it has a play-of-color, its own families); the note counts the latter.
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
    // `options` is already cached at the `Vec<String>` level
    // (`EditorState::material_combo_options`'s own `MaterialComboCache`, rebuilt
    // only when the catalogue's custom-material names change). Comparing against
    // what is already pushed skips rebuilding a brand-new `ModelRc<VecModel<_>>`
    // from that cached `Vec` (and the Slint-side model reset it triggers) unless
    // the catalogue itself actually changed -- rebuilding unconditionally on every
    // refresh would force the combo box to fully reset its rows on every angle
    // nudge just as much as on an actual material save.
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
    // Pushed the same way, so `new_design_dialog.slint`'s New
    // Design material `ComboBox` binds its `model` to `EditorModel.new_material_options`
    // instead of hand-maintaining a literal list in the SAME order as
    // `builtin_preset_names` -- see that function's own doc comment.
    // `builtin_preset_names` never changes at runtime, so this is redundant work on
    // every refresh, not a correctness issue -- matching `material_combo_options`
    // just above rather than adding a one-shot-at-startup special case for one more
    // static list.
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
        // Whatever the last typed-but-not-yet-applied preview
        // said is no longer meaningful once the fields are freshly reseeded from
        // the real (just-applied, or freshly loaded) design -- a value that
        // matches the live design has nothing left to preview.
        ui.global::<EditorModel>()
            .set_symmetry_preview_text("".into());
    }
    if delta.meta {
        // `design.meta.headers`' first entry is the design's
        // title (the catalogue round-trip convention -- see `EditorModel.
        // design_title`'s own doc comment, `ui/models/editor.slint`), every
        // further entry an "extra" header line, `';'`-joined back for the
        // form's single-line field -- `apply_design_meta`'s own inverse split.
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

/// The viewport's material sync while the view follows the design (no sync while the person is
/// previewing another material) -- the stone width and effects are [`sync_stone_from_design`]'s.
///
/// See
/// [`refresh_design_settings`]'s doc comment for why there is no separate
/// `render_view_tab` gate. Split out purely to keep that function under clippy's
/// function-length lint.
///
/// Returns the viewport material selection's (the "Preview as…" and library-row
/// picker) resolved index instead of setting `ViewportModel.selected_material_index`
/// itself. That setter's `changed` handler runs a DB query
/// (`custom_materials.rs`), and every caller holds `ctx` -- the `render_ctx`
/// mutex the render thread also locks every frame -- across this function's
/// call; setting the property from inside that guard stalls the render
/// thread on every refresh. Callers must set the returned index themselves,
/// after the guard on `ctx` has been dropped.
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
    // The resolved material itself, not merely its name:
    // `context::resolve_material` can only look a name up in the built-in table,
    // so a catalogue custom material or a typed RI override never reached the
    // trace at all. `material_override` is what the tracer, the HUD metrics, the
    // tilt sweep and the hover preview all prefer over that name lookup.
    //
    // Keyed on `name` -- the material this design is traced AS, decided by
    // `traced_material_for` just above -- and NOT on `design.material`. Passing the
    // raw selection here sent it through `MaterialSelection::resolve`, whose
    // no-name fallback is `GemMaterial::diamond()`: every `.asc` import and every
    // new design names no material, so the override was diamond, the override beats
    // the name, and the stone rendered with diamond's (empty) absorption bands --
    // colorless whatever the schedule said. `traced_gem_material` returns `None`
    // instead of substituting, and an absent override falls through to
    // `context::resolve_material`'s own by-name lookup, so the name and the override
    // can no longer describe two different stones. The design's body-color variant
    // rides along here too (`MaterialSelection::apply_overrides`, inside
    // `traced_gem_material`): `material_name` stays the species ("Sapphire"), the
    // override carries its recolored absorption.
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
    // Keeps the viewport material selection's displayed index in sync with
    // the material actually being traced on every refresh. Writing this only
    // once, at startup (`startup_settings::apply_saved_settings`), would let the
    // selection go on showing e.g. "Diamond" long after `ctx.material_name` (and
    // so the trace, and the tilt dialog's staleness check, both of which read
    // `ViewportModel.selected_material_index`/`material_options`) had moved to
    // something else entirely.
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
/// material, these are NOT view choices: they follow the design whether or not a material
/// preview is on (the preview only swaps the material), so the old overwrite of a manual size
/// by a linked refresh is gone -- there is one stone width, `design.girdle_diameter_mm`.
///
/// The effects are left alone while a Stone-tab control waits to commit its change to the design
/// ([`stone_effects::live_edit_pending`]), so a refresh that lands mid-drag never takes the
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

/// Builds the chip row for whichever tier the inspector's Tier tab currently has
/// loaded (`EditorModel.selected_tier_index`; `None` for "Add Tier" or an
/// out-of-range value) -- the per-facet editing surface. Computed
/// fresh from `design` on the UI thread every refresh, for only the one tier
/// being edited, rather than for every row: see [`IndexChipItem`]'s own doc
/// comment for why this can never be a field on the (background-thread-built,
/// `Send`) [`crate::EditorTierItem`] instead.
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

/// Pushes [`selected_tier_chips`]'s result into `EditorModel.selected_tier_chips`,
/// plus the currently selected tier's own cheater-offset text
/// into `EditorModel.selected_tier_cheater_offset_text` -- shared by
/// [`super::panel::refresh_editor_panel`]/[`super::panel_stale::push_stale_content`]/
/// `callbacks::tier_actions::apply_selected_tier_change` (a plain row-click/
/// facet-click selection change, with no edit of its own) so both stay current
/// after every one of the three ways the selected tier can change, exactly like
/// `tiers` itself.
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
