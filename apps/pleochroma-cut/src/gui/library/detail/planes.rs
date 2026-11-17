//! Rebuilds a design's 3D facet planes and resolves the viewport's material, shared
//! by the local and remote detail-load paths so both can never drift apart.

use crate::{
    AngleItem, MainWindow, SettingsModel, TiltModel, ViewportModel,
    bridge::render_thread::{PlanesOwner, RenderContext},
    gui::{
        batch::catalogue_material::design_file_material,
        editor::{
            CataloguePlanes,
            material_lookup::{MATERIAL_MATCH_TOLERANCE, material_for_refractive_index},
        },
        is_c_axis_override_available,
        render::{fit_view, stone_effects},
    },
};
use pleochroma::{
    geometry::{
        cuts::{FacetSpec, StandardGemCuts},
        plane::GpuFacetPlane,
        tool::ToolPrimitive,
    },
    optics::materials::GemMaterial,
};
use pleochroma_cut_core::{
    StoneRenderEffects,
    design::ConcaveTableRow,
    material::{BuiltinMaterials, MaterialLookup, MaterialSelection},
    native::CustomMaterialSnapshot,
};
use pleochroma_solid::preview::StoneGeometryBuf;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};
use tracing::info;

/// The material and size a catalogue design's own file records.
///
/// The detail view draws the stone with them, as the editor and the exports do. The preview
/// and tilt batches apply the same rule (`gui::batch::catalogue_material`).
///
/// The remote route has no design file and passes the [`Default`]: no selection, snapshot or
/// size. The persisted-material and refractive-index rules then apply.
#[derive(Debug, Clone, Default)]
pub(super) struct DesignFileMaterial {
    /// The design file's material selection: the species name, and the refractive-index and
    /// body-color overrides applied on top of it. `None` when there is no design file.
    pub(super) selection: Option<MaterialSelection>,
    /// The design file's own snapshot of a custom material the vault may lack.
    pub(super) custom_snapshot: Option<CustomMaterialSnapshot>,
    /// The design file's girdle diameter in millimetres; `None` is "not set".
    pub(super) girdle_mm: Option<f64>,
}

impl DesignFileMaterial {
    /// The stone width the viewport draws the design at, in millimetres: the file's girdle
    /// diameter, or `0.0` -- "not set", the 7 mm the built-in colors are tuned for.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a girdle diameter in millimetres is a small positive value; f32 is the viewport precision"
    )]
    pub(super) fn stone_width_mm(&self) -> f32 {
        self.girdle_mm
            .filter(|mm| mm.is_finite() && *mm > 0.0)
            .map_or(0.0, |mm| mm as f32)
    }
}

impl From<&CataloguePlanes> for DesignFileMaterial {
    /// The material facts of a resolved catalogue design; the angle-table path has none, so
    /// it converts to the default.
    fn from(resolved: &CataloguePlanes) -> Self {
        Self {
            selection: resolved.material.clone(),
            custom_snapshot: resolved.custom_material.clone(),
            girdle_mm: resolved.girdle_diameter_mm,
        }
    }
}

/// The inputs to [`apply_reconstructed_planes`] for one design.
///
/// `real_design` holds the planes, gear teeth and reference angle from
/// [`crate::gui::editor::resolve_catalogue_planes`], or `None` on the remote route. See
/// [`planes_gear_and_reference_angle`] for the fallback to the angle table. `design_material`
/// is what the same resolution found about the material and size.
///
/// Grouped into one struct to keep the function under clippy's `too_many_arguments` limit.
///
/// Applying claims the shared plane slot, as `gui::editor::view::refresh_viewport` does. An
/// open Tilt Performance dialog then does not keep showing the previous design's curves.
/// `ui` is only needed for the resulting re-sweep request.
pub(super) struct ReconstructedPlanesInput<'a> {
    pub(super) shape: Option<&'a str>,
    pub(super) index_gear: Option<&'a str>,
    pub(super) angle_items: &'a [AngleItem],
    pub(super) refractive_index: Option<&'a str>,
    /// The design's persisted preview material, which both callers also set as
    /// `TiltModel.cached_curve_material`. [`apply_catalogue_material`] tries it before the
    /// refractive-index guess.
    pub(super) preview_material: Option<&'a str>,
    pub(super) real_design: Option<(Vec<GpuFacetPlane>, u32, f32)>,
    /// Ascending indices of the planes in `real_design` that are frosted. Empty for a
    /// polished design and on the remote route.
    pub(super) frosted: Vec<u32>,
    pub(super) design_material: DesignFileMaterial,
    /// The concave tools and their `(tier, placement)` list. Local route: from
    /// `CataloguePlanes::render_tools`. Remote route: empty, and [`Self::concave_rows`] is used.
    pub(super) tools: Vec<ToolPrimitive>,
    pub(super) tool_placements: Vec<(usize, usize)>,
    /// The concave angle-table rows, for the remote route, which has no design file. The
    /// tools are placed on the rebuilt planes ([`remote_tools`]). Ignored when `tools` is set.
    pub(super) concave_rows: Vec<ConcaveRow>,
}

/// The cells of one concave angle-table row, owned (the wire record's own strings).
#[derive(Debug, Clone)]
pub(super) struct ConcaveRow {
    pub(super) facet: String,
    /// The signed angle cell (`-42°`): the angle items of the table drop the sign.
    pub(super) angle: String,
    pub(super) index: String,
    pub(super) notes: String,
    /// The formatted tool line (the code is its first column).
    pub(super) tool_line: String,
}

impl ConcaveRow {
    /// The concave rows among `rows` (`(facet, angle, index, notes, tool_line)`): those with
    /// a tool line. A flat row has none and is skipped.
    pub(super) fn from_wire<'a>(
        rows: impl IntoIterator<Item = (&'a str, &'a str, &'a str, &'a str, Option<&'a str>)>,
    ) -> Vec<Self> {
        rows.into_iter()
            .filter_map(|(facet, angle, index, notes, tool_line)| {
                Some(Self {
                    facet: facet.to_owned(),
                    angle: angle.to_owned(),
                    index: index.to_owned(),
                    notes: notes.to_owned(),
                    tool_line: tool_line.filter(|line| !line.trim().is_empty())?.to_owned(),
                })
            })
            .collect()
    }
}

/// The tools the concave `rows` describe, placed on the flat stone `planes` of an index gear
/// with `gear_teeth` teeth.
///
/// The remote record carries only the rows, so this is all that route knows about a concave
/// tier. Empty without rows.
pub(super) fn remote_tools(
    rows: &[ConcaveRow],
    planes: &[GpuFacetPlane],
    gear_teeth: u32,
) -> (Vec<ToolPrimitive>, Vec<(usize, usize)>) {
    if rows.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let facets: Vec<(glam::DVec3, f64)> = planes
        .iter()
        .copied()
        .map(GpuFacetPlane::to_halfspace_f64)
        .collect();
    crate::gui::editor::tools_from_table_rows(
        rows.iter().map(|row| ConcaveTableRow {
            facet: &row.facet,
            angle: &row.angle,
            index: &row.index,
            notes: &row.notes,
            tool: "",
            tool_line: &row.tool_line,
        }),
        &facets,
        i32::try_from(gear_teeth).unwrap_or(96),
        0.0,
    )
}

/// Builds the planes from the angle table when `real_design` is `None`.
///
/// That is the remote route only, which has no design-file bytes to parse. The gear reference
/// angle is `0.0`, since library records carry none. The local route passes the result of
/// [`crate::gui::editor::resolve_catalogue_planes`], which applies the same fallback itself.
pub(super) fn planes_gear_and_reference_angle(
    real_design: Option<(Vec<GpuFacetPlane>, u32, f32)>,
    shape: Option<&str>,
    index_gear: Option<&str>,
    angle_items: &[AngleItem],
) -> (Vec<GpuFacetPlane>, u32, f32) {
    if let Some(resolved) = real_design {
        return resolved;
    }

    // `pleochroma` must not depend on Slint, so convert the Slint-generated `AngleItem`
    // rows into plain `FacetSpec`s at this boundary.
    let facet_specs = facet_specs_from_rows(angle_items.iter().map(|a| {
        (
            a.facet.as_str(),
            a.angle.as_str(),
            a.index_val.as_str(),
            a.notes.as_str(),
            !a.second_line.is_empty(),
        )
    }));
    placeholder_planes_and_gear(shape, index_gear, &facet_specs)
}

/// The plain `FacetSpec`s of angle-table rows `(facet, angle, index, notes, has_tool_line)`.
/// Both the Slint-row path and the wire path use this, so they cannot disagree.
///
/// A concave row (one with a tool line) is not a flat facet, so it is left out of the
/// placeholder. `angle` is used as given: the callers have already dropped the sign.
fn facet_specs_from_rows<'a>(
    rows: impl IntoIterator<Item = (&'a str, &'a str, &'a str, &'a str, bool)>,
) -> Vec<FacetSpec> {
    rows.into_iter()
        .filter(|&(_, _, _, _, has_tool_line)| !has_tool_line)
        .map(|(facet, angle, index, notes, _)| FacetSpec {
            facet: facet.to_string(),
            angle: angle.to_string(),
            index: index.to_string(),
            notes: notes.to_string(),
        })
        .collect()
}

/// The angle-table placeholder stone: its planes, the gear tooth count and the reference
/// angle (`0.0`, see [`planes_gear_and_reference_angle`]).
fn placeholder_planes_and_gear(
    shape: Option<&str>,
    index_gear: Option<&str>,
    facet_specs: &[FacetSpec],
) -> (Vec<GpuFacetPlane>, u32, f32) {
    let planes = reconstruct_planes(shape, index_gear, facet_specs);
    // Library records carry no reference angle, so the diagram view's index wheel
    // uses the gear tooth count alone (96 when the record has none).
    let gear_teeth: u32 = index_gear
        .and_then(|g| g.trim().parse().ok())
        .filter(|&g| g > 0)
        .unwrap_or(96);
    (planes, gear_teeth, 0.0)
}

/// One angle-table row of a remote record as borrowed wire strings:
/// `(facet, signed angle, index, notes, tool line)`.
pub(super) type WireRow<'a> = (&'a str, &'a str, &'a str, &'a str, Option<&'a str>);

/// A remote design's geometry, built from owned wire data on the request thread so the UI
/// callback only applies it (see `bridge::library::source::spawn_library_request_prepared`).
#[derive(Debug)]
pub(super) struct PreparedRemoteGeometry {
    pub(super) planes: Vec<GpuFacetPlane>,
    pub(super) gear_teeth: u32,
    pub(super) gear_reference_angle: f32,
    pub(super) tools: Vec<ToolPrimitive>,
    pub(super) tool_placements: Vec<(usize, usize)>,
}

/// Builds a remote record's stone: the angle-table placeholder planes from the wire rows and
/// the concave tools placed on them ([`remote_tools`]). Pure over its arguments, so it runs
/// on any thread.
pub(super) fn prepare_remote_geometry(
    shape: Option<&str>,
    index_gear: Option<&str>,
    rows: &[WireRow<'_>],
) -> PreparedRemoteGeometry {
    let facet_specs =
        facet_specs_from_rows(rows.iter().map(|&(facet, angle, index, notes, tool_line)| {
            (
                facet,
                // The angle-table cells drop the sign, and the placeholder is built from
                // the cells.
                angle.trim_start_matches('-'),
                index,
                notes,
                tool_line.is_some_and(|line| !line.is_empty()),
            )
        }));
    let (planes, gear_teeth, gear_reference_angle) =
        placeholder_planes_and_gear(shape, index_gear, &facet_specs);
    let concave_rows = ConcaveRow::from_wire(rows.iter().copied());
    let (tools, tool_placements) = remote_tools(&concave_rows, &planes, gear_teeth);
    PreparedRemoteGeometry {
        planes,
        gear_teeth,
        gear_reference_angle,
        tools,
        tool_placements,
    }
}

/// Claims the viewport's stone for catalogue row `entry_id`: planes, tools and frosted planes.
///
/// The editor's viewport claims its stone the same way. `claim_active_geometry` resets the
/// frosting, so the frosted planes are set only after an accepted claim. Returns whether the
/// claim was accepted.
pub(super) fn claim_catalogue_stone(
    ctx: &mut RenderContext,
    entry_id: i64,
    stone: StoneGeometryBuf,
    design_gear: Option<(u32, f32)>,
) -> bool {
    let StoneGeometryBuf {
        planes,
        tools,
        placements,
        frosted,
    } = stone;
    let claimed = ctx.claim_active_geometry(
        Arc::new(planes),
        Arc::new(tools),
        placements,
        design_gear,
        PlanesOwner::Catalogue { entry_id },
    );
    if claimed {
        ctx.claim_active_frosted(frosted);
    }
    claimed
}

/// Rebuilds the viewport's facet planes for a design and claims the viewport for it. Shared
/// by the local and remote detail loads, so both sources build the stone the same way.
pub(super) fn apply_reconstructed_planes(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
    input: ReconstructedPlanesInput<'_>,
) {
    let ReconstructedPlanesInput {
        shape,
        index_gear,
        angle_items,
        refractive_index,
        preview_material,
        real_design,
        frosted,
        design_material,
        tools,
        tool_placements,
        concave_rows,
    } = input;
    let (planes, gear_teeth, gear_reference_angle) =
        planes_gear_and_reference_angle(real_design, shape, index_gear, angle_items);
    let (tools, tool_placements) = if tools.is_empty() {
        remote_tools(&concave_rows, &planes, gear_teeth)
    } else {
        (tools, tool_placements)
    };

    info!(
        "Reconstructed {} 3D facet planes for diagram #{}",
        planes.len(),
        entry_id
    );

    // The stone is framed unless the cutter has zoomed. Measured before the render context
    // is locked, since it builds the stone's solid.
    let framing = fit_view::auto_frame_distance(ui, render_ctx, &planes);
    let mut ctx = RenderContext::lock(render_ctx);
    // An explicit Library selection takes the viewport, also from a design loaded in the
    // editor, which reclaims it on its next load or edit. The planes and tools go in
    // together, so they never mix two designs. A planar design claims no tools.
    let claimed = claim_catalogue_stone(
        &mut ctx,
        entry_id,
        StoneGeometryBuf {
            planes,
            tools,
            placements: tool_placements,
            frosted,
        },
        Some((gear_teeth, gear_reference_angle)),
    );
    let (resolved_material, c_axis_available, stone_width_mm) = if claimed {
        let resolved = apply_catalogue_material(
            &mut ctx,
            refractive_index,
            preview_material,
            &design_material,
        );
        // A library design is not editable and has no stone effects of its own. The Stone
        // tab's effects reset to the defaults, so the editor design's values do not leak
        // onto it. The width is the design file's girdle diameter when it has one (set by
        // `apply_catalogue_material`), otherwise 7 mm.
        stone_effects::clear_live_edit_pending();
        stone_effects::apply_effects_to_context(&mut ctx, &StoneRenderEffects::default());
        // Re-checked under the lock: the cutter may have zoomed while the stone was measured.
        if let Some(distance) = framing
            && fit_view::is_unzoomed(ctx.distance)
        {
            ctx.distance = distance;
            fit_view::note_auto_framed(distance);
        }
        ctx.dirty = true;
        // The crystal-axis control follows the material this row is traced as.
        let c_axis_available = ctx
            .material_override
            .as_ref()
            .is_some_and(is_c_axis_override_available);
        // The viewport now shows a Library row, drawn whole: Live Render drops its
        // "Cut: ..." pill (the Cut slider belongs to the editor's design).
        crate::gui::editor::push_viewport_owner(ui, ctx.planes_owner);
        (resolved, c_axis_available, ctx.stone_width_mm)
    } else {
        (None, false, 0.0)
    };
    drop(ctx);
    if claimed {
        // Relative-to-stone zones follow the width this row shows, not the
        // previous design's.
        crate::gui::rough_colour::material_zones::stone_width_applied(render_ctx, stone_width_mm);
        stone_effects::push_effects_to_controls(ui, &StoneRenderEffects::default());
        ui.global::<SettingsModel>()
            .set_stone_width_mm(stone_width_mm);
        ui.global::<SettingsModel>()
            .set_c_axis_override_available(c_axis_available);
        // The colour readouts show the colour at this size, and the colour control now
        // targets the view (a library design is never edited).
        ui.global::<ViewportModel>().invoke_render_color_refresh();
    }
    if let Some(name) = resolved_material {
        show_resolved_material(ui, render_ctx, &name);
    }
    if !claimed {
        return;
    }

    // A no-op re-sweep when the planes did not change: `AxesCacheKey` hashes them.
    if ui.global::<TiltModel>().get_dialog_open() {
        ui.global::<TiltModel>().invoke_request_tilt_profile_axes();
    }
}

/// Shows `name` as the Material dropdown's selection and the Stone tab's material facts.
fn show_resolved_material(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>, name: &str) {
    // The dropdown's own selection is set as well as the traced material, since they are
    // separate state. A missing option leaves the selection unchanged: built-in materials
    // are always listed, so this only happens before the options are pushed.
    let options = ui.global::<ViewportModel>().get_material_options();
    if let Some(index) = crate::gui::startup_settings::find_option_index(&options, name) {
        ui.global::<ViewportModel>()
            .set_selected_material_index(index);
    }
    // The Stone tab's material facts follow the material the library row is shown in.
    let (custom, custom_gem, custom_sg) = {
        let ctx = RenderContext::lock(render_ctx);
        (
            ctx.custom_materials.clone(),
            ctx.custom_material_gem_data.clone(),
            ctx.custom_material_specific_gravity.clone(),
        )
    };
    crate::gui::editor::push_gem_card(ui, Some(name), &custom, &custom_gem, &custom_sg);
}

/// Resolves `name` to a custom material first, then to a built-in.
///
/// This matches `gui::editor::material_lookup`, so a saved name resolves the same way in both
/// places. Used only by the preview-material path of [`apply_catalogue_material`].
fn lookup_named_material(ctx: &RenderContext, name: &str) -> Option<GemMaterial> {
    ctx.custom_materials
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(name))
        .cloned()
        .or_else(|| BuiltinMaterials.lookup(name))
}

/// The material that `design`'s file names, resolved as the editor's viewport resolves it.
///
/// This is `gui::batch::catalogue_material::design_file_material`: custom materials first,
/// then the built-ins, with the file's overrides on top and the file's own snapshot as a last
/// resort. It is written to both halves of the material, since `material_override` beats
/// `material_name`.
///
/// `None`, with the context untouched, when the design has no file, the file names no
/// material, or the name resolves nowhere (the viewport would refuse such a design; this view
/// goes on to the persisted material and the refractive-index guess).
fn apply_design_file_material(
    ctx: &mut RenderContext,
    design: &DesignFileMaterial,
) -> Option<String> {
    let selection = design.selection.as_ref()?;
    let (name, material) = design_file_material(
        selection,
        design.custom_snapshot.as_ref(),
        &ctx.custom_materials,
    )?;
    ctx.material_unresolved = None;
    ctx.material_name.clone_from(&name);
    ctx.material_override = Some(material);
    Some(name)
}

/// Points the viewport's material and stone width at this catalogue row.
///
/// Without this, the preview would keep the material of whatever was shown before, so
/// clicking through the library would change the shape on screen but never the stone.
///
/// The material is the first of these that answers, the order the library batches use
/// for the thumbnails and tilt curves of the same design
/// (`gui::batch::catalogue_material`):
///
/// 1. the design file's own material ([`apply_design_file_material`]) -- what the editor and
///    the exports show;
/// 2. the persisted `preview_material`;
/// 3. the built-in the row's refractive index names.
///
/// The stone width follows the design file as well: `ctx.stone_width_mm` becomes the file's
/// girdle diameter ([`DesignFileMaterial::stone_width_mm`]; `0.0`, the 7 mm the built-in
/// colors are tuned for, when the file has none or there is no file), set whichever of the
/// three answers, so a thumbnail, a tilt curve and this view agree on the stone.
///
/// Both halves of the material are written, not just the name: `material_override` BEATS
/// `material_name` in `render_thread::context::resolve_material_with_override`, so leaving a
/// stale override in place would make the name below purely decorative.
///
/// A row whose refractive index is absent, unparsable, or within
/// [`MATERIAL_MATCH_TOLERANCE`] of no built-in at all sets `material_unresolved` instead:
/// the viewport then says why it will not trace rather than borrowing some other
/// design's optics.
///
/// `preview_material`, when on file, is tried before the refractive-index guess, via
/// [`lookup_named_material`]. It is the material the cached thumbnail and tilt curves
/// were rendered under, so it is preferred over a nearest-preset guess. That keeps the
/// viewport in step with the thumbnail.
///
/// Returns the material's name on success, so the caller can point the viewport's material
/// list at it once the `RenderContext` lock is released; `None` on either refusal,
/// where there is no name to show.
pub(super) fn apply_catalogue_material(
    ctx: &mut RenderContext,
    refractive_index: Option<&str>,
    preview_material: Option<&str>,
    design: &DesignFileMaterial,
) -> Option<String> {
    ctx.stone_width_mm = design.stone_width_mm();
    if let Some(name) = apply_design_file_material(ctx, design) {
        return Some(name);
    }
    if let Some(name) = preview_material.map(str::trim).filter(|s| !s.is_empty()) {
        return if let Some(gem) = lookup_named_material(ctx, name) {
            ctx.material_unresolved = None;
            ctx.material_name = name.to_string();
            ctx.material_override = Some(gem);
            Some(name.to_string())
        } else {
            // The row names a SPECIFIC material that resolves to neither a
            // built-in preset nor a saved custom material -- refuse outright
            // (matching `gui::editor::view::inspector::traced_material_for`'s own
            // rule for the same situation) rather than falling through to the
            // RI-based guess below, which would silently trace/tilt-sweep/HUD-score
            // a DIFFERENT stone than the one this row actually names, with no hint
            // why. Clears `material_name`/`material_override` rather than leaving
            // them at whatever the PREVIOUSLY loaded catalogue row set them to --
            // without this, switching from a resolving row to this one kept
            // showing the previous row's material as if it were this row's own
            // honest resolution.
            ctx.material_name.clear();
            ctx.material_override = None;
            ctx.material_unresolved = Some(format!(
                "This design's material '{name}' is not a built-in preset or a saved \
                 custom material, so there is nothing to trace -- pick a material in \
                 the Material list in the Stone tab, or re-save the missing custom \
                 material."
            ));
            None
        };
    }

    let n_d = refractive_index.and_then(|text| text.trim().parse::<f64>().ok());
    let Some(n_d) = n_d.filter(|v| v.is_finite() && *v > 1.0) else {
        // See the comment above for why every refusal in this function clears
        // `material_name`/`material_override` rather than leaving the previous
        // row's.
        ctx.material_name.clear();
        ctx.material_override = None;
        ctx.material_unresolved = Some(
            "This design records no usable refractive index, so there is no honest \
             material to render it in. Pick one in the Material list of the Stone tab."
                .to_string(),
        );
        return None;
    };
    let Some((name, gem)) = material_for_refractive_index(n_d) else {
        ctx.material_name.clear();
        ctx.material_override = None;
        ctx.material_unresolved = Some(format!(
            "This design's refractive index ({n_d:.4}) matches no built-in preset within \
             {MATERIAL_MATCH_TOLERANCE:.2}. Pick a material in the Material list of the \
             Stone tab -- rendering it as something else would give you the optics of a \
             different stone."
        ));
        return None;
    };
    ctx.material_unresolved = None;
    ctx.material_name.clone_from(&name);
    ctx.material_override = Some(gem);
    Some(name)
}

/// Builds a design's 3D facet planes from its shape, gear and facet specs.
///
/// Used by [`apply_reconstructed_planes`] and by the import measurement, so both see the same
/// planes the viewport shows. `shape` may be `None` at import, which skips the emerald case.
pub fn reconstruct_planes(
    shape: Option<&str>,
    index_gear: Option<&str>,
    facet_specs: &[FacetSpec],
) -> Vec<GpuFacetPlane> {
    let shape_str = shape.unwrap_or_default().to_lowercase();
    let gear_num: u32 = index_gear.unwrap_or_default().parse().unwrap_or(96);

    if shape_str.contains("emerald") || shape_str.contains("baguette") || shape_str.contains("rect")
    {
        StandardGemCuts::emerald_cut()
    } else if !facet_specs.is_empty() {
        StandardGemCuts::from_database_angles(facet_specs, gear_num)
    } else {
        StandardGemCuts::standard_round_brilliant()
    }
}

#[cfg(test)]
mod prepared_geometry_tests {
    use super::{
        ConcaveRow, WireRow, planes_gear_and_reference_angle, prepare_remote_geometry, remote_tools,
    };
    use crate::AngleItem;

    /// The Slint rows of `wire`, built the way `apply_design_record_to_ui` builds its
    /// table (the angle drops its sign, the tool line is the second line).
    fn angle_items(wire: &[WireRow<'_>]) -> Vec<AngleItem> {
        wire.iter()
            .enumerate()
            .map(
                |(order, &(facet, angle, index, notes, tool_line))| AngleItem {
                    order_idx: i32::try_from(order).expect("a handful of rows"),
                    side: 0,
                    facet: facet.into(),
                    code: facet.into(),
                    angle: angle.trim_start_matches('-').into(),
                    index_val: index.into(),
                    notes: notes.into(),
                    second_line: tool_line.unwrap_or_default().into(),
                },
            )
            .collect()
    }

    /// A flat remote table prepares the same placeholder stone as the Slint row path: equal
    /// planes, gear 96, reference angle 0.0, and no tools.
    #[test]
    fn a_flat_table_prepares_the_same_stone_as_the_slint_row_path() {
        let wire: Vec<WireRow<'_>> = vec![
            ("P1", "-41.0\u{b0}", "0, 24, 48, 72", "", None),
            ("P2", "-43.5\u{b0}", "12, 36, 60, 84", "", None),
        ];
        let prepared = prepare_remote_geometry(Some("Round"), Some("96"), &wire);
        let (planes, gear_teeth, reference_angle) =
            planes_gear_and_reference_angle(None, Some("Round"), Some("96"), &angle_items(&wire));
        assert_eq!(prepared.planes, planes);
        assert_eq!(prepared.gear_teeth, 96);
        assert_eq!(gear_teeth, 96);
        assert_eq!(prepared.gear_reference_angle.to_bits(), 0.0_f32.to_bits());
        assert_eq!(reference_angle.to_bits(), 0.0_f32.to_bits());
        assert!(prepared.tools.is_empty() && prepared.tool_placements.is_empty());
    }

    /// With concave rows, the prepared tools are the ones `remote_tools` places on the
    /// placeholder stone, and a concave row never becomes a flat facet.
    #[test]
    fn concave_rows_prepare_the_tools_remote_tools_places() {
        let design = pleochroma_cut_core::Design::concave_fixture();
        let owned: Vec<(String, String, String, String, String)> = design
            .concave_tiers
            .iter()
            .map(|tier| {
                (
                    tier.name.clone(),
                    format!("{}\u{b0}", tier.angle_deg),
                    tier.indices
                        .iter()
                        .map(f64::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                    tier.instructions.clone(),
                    tier.second_line_fields().join("  "),
                )
            })
            .collect();
        let mut wire: Vec<WireRow<'_>> = vec![("P1", "-41.0\u{b0}", "0, 24, 48, 72", "", None)];
        wire.extend(owned.iter().map(|(facet, angle, index, notes, tool_line)| {
            (
                facet.as_str(),
                angle.as_str(),
                index.as_str(),
                notes.as_str(),
                Some(tool_line.as_str()),
            )
        }));

        let prepared = prepare_remote_geometry(Some("Round"), Some("96"), &wire);
        let (planes, gear_teeth, _) =
            planes_gear_and_reference_angle(None, Some("Round"), Some("96"), &angle_items(&wire));
        assert_eq!(
            prepared.planes, planes,
            "a concave row is no flat facet, in either path"
        );
        let concave = ConcaveRow::from_wire(wire.iter().copied());
        let (tools, placements) = remote_tools(&concave, &planes, gear_teeth);
        assert_eq!(prepared.tools.len(), tools.len());
        assert_eq!(prepared.tool_placements, placements);
    }
}
