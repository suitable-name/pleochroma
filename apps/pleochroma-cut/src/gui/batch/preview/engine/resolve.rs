//! A batch item's resolved design: its geometry, material and render physics, read
//! from the catalogue row -- the shared prelude both the local and the remote lane need.

use super::{BatchContext, RecordRevision, ray, target_ri_for_design};
use crate::{
    bridge::render_effects::{design_scene_effects, words_for_planes},
    gui::{batch::catalogue_material::catalogue_material, editor::CataloguePlanes},
    settings::model::current_faceting_machine,
};
use pleochroma::{
    geometry::{plane::GpuFacetPlane, tool::ToolPrimitive},
    optics::materials::GemMaterial,
};
use pleochroma_net::scene::effects::SceneEffects;
use std::sync::PoisonError;
use tracing::warn;

/// One design's resolved geometry/material -- the shared prelude both
/// `render_item_local` and `remote_lane::render_item_remote`'s callers need before rendering
/// anything. See this group's `mod.rs` doc comment's "Why per-item resolve" section for
/// why there is deliberately no cache sharing this between a design's two items.
pub(in crate::gui::batch::preview) struct ResolvedDesign {
    pub(in crate::gui::batch::preview) title: String,
    pub(in crate::gui::batch::preview) planes: Vec<GpuFacetPlane>,
    /// The concave tools cut into `planes` (`CataloguePlanes::render_tools`); empty for a
    /// planar design, which renders exactly as it always did.
    pub(in crate::gui::batch::preview) tools: Vec<ToolPrimitive>,
    /// The planes the design frosts (ascending indices into `planes`), traced frosted by a
    /// traced preview, locally and remotely; empty for a polished design and for the solid
    /// stand-in, which is flat shaded.
    pub(in crate::gui::batch::preview) frosted: Vec<u32>,
    /// The design's render physics (`bridge::render_effects::design_scene_effects`: its
    /// inclusions, play-of-color, surface finish and abraded edge, with the user's faceting
    /// machine); the default record for a design that sets none and for the solid stand-in.
    pub(in crate::gui::batch::preview) effects: SceneEffects,
    pub(in crate::gui::batch::preview) material: GemMaterial,
    /// The record version `planes` and `material` were resolved from; hand it to
    /// `finish_item_at_revision` with the rendered bytes so a result from a superseded
    /// record is never saved.
    pub(in crate::gui::batch::preview) revision: RecordRevision,
}

/// `full`'s facet planes for a preview render, and the planes its design frosts.
///
/// The planes come with the material and stone size the design file records -- the design file
/// first, the angle table only as the fallback
/// (`gui::batch::record_planes_with_frosting_for_batch`, shared with the tilt batch and the
/// library detail view). Both the local and the remote lane get their planes here, through
/// [`resolve_design`]; a remote render ships these planes in its scene, so the worker never
/// resolves a record itself.
///
/// The second value is the design's frosted planes: the ascending indices into the returned
/// planes the design frosts, the form `PreviewJob::frosted` takes. Empty for a polished
/// design and on the angle-table path.
pub(in crate::gui::batch::preview) fn record_planes_with_frosting(
    ctx: &BatchContext<'_>,
    full: &pleochroma_vault::model::entry::FullDiagramRecord,
) -> Option<(CataloguePlanes, Vec<u32>)> {
    crate::gui::batch::record_planes_with_frosting_for_batch(full, ctx.angle_table_entries)
}

/// Resolves `entry_id`'s geometry and preview material, or `None` if either step comes
/// up empty (malformed/unreadable row, unreconstructable geometry, or no material could
/// be matched/assigned).
///
/// The material is the one `gui::batch::catalogue_material::catalogue_material` decides:
/// the design file's own (its overrides applied), else the refractive-index pick, sized by
/// the design's girdle diameter -- the stone the editor and the exports show. A remote
/// lane ships this sized material in its scene unchanged.
pub(in crate::gui::batch::preview) fn resolve_design(
    ctx: &BatchContext<'_>,
    entry_id: i64,
) -> Option<ResolvedDesign> {
    // The record and its revision stamp are read under one lock hold, so the stamp
    // describes exactly this record.
    let (full, updated_at) = {
        let guard = ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
        (
            guard.get_diagram_full(entry_id),
            guard.entry_updated_at(entry_id),
        )
    };
    let Ok(Some(full)) = full else {
        return None;
    };
    let Ok(updated_at) = updated_at else {
        return None;
    };
    let (resolved, frosted) = record_planes_with_frosting(ctx, &full)?;

    if ctx.solid {
        return Some(ResolvedDesign {
            title: full.title,
            planes: resolved.planes,
            tools: resolved.render_tools,
            // The solid picture is flat shaded: no finish shows.
            frosted: Vec::new(),
            effects: SceneEffects::default(),
            // Never shown: the solid picture is flat-shaded. The traced preview picks
            // the design's real material when it is rendered.
            material: GemMaterial::by_name("Diamond")?,
            revision: RecordRevision::Stamp(updated_at),
        });
    }
    let target_ri = target_ri_for_design(&full);
    let chosen = {
        let guard = ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
        catalogue_material(
            &guard,
            entry_id,
            &resolved,
            ctx.custom_materials,
            ctx.material_candidates,
            target_ri,
        )
    };
    let chosen = match chosen {
        Ok(Some(chosen)) => chosen,
        Ok(None) => return None,
        Err(e) => {
            warn!("Could not choose the preview material for entry {entry_id}: {e}");
            return None;
        }
    };

    if ctx.engine.is_ray() && ray::refuses_fluorescence(ctx, &resolved, &chosen.material) {
        warn!("Ray-Traced thumbnail unavailable: fluorescent material needs Path-Traced.");
        return None;
    }
    // The silk, play-of-color, surface finish and abraded edge the design file states.
    let effects = design_scene_effects(
        &resolved.render_effects,
        Some(&chosen.material),
        words_for_planes(&resolved.surface_table, &resolved.planes),
        &current_faceting_machine(),
    );
    Some(ResolvedDesign {
        title: full.title,
        planes: resolved.planes,
        tools: resolved.render_tools,
        frosted,
        effects,
        material: chosen.material,
        revision: RecordRevision::Stamp(updated_at),
    })
}
