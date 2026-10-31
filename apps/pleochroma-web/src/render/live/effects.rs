//! The design's render physics for the live view and the export: inclusions, play-of-color,
//! surface finish and abraded edge, read from the design file (view-only in the browser, so the
//! page has no controls for them). A design that sets none gives `EffectsSpec::default()`, which
//! changes nothing, bit for bit. The faceting machine is the default one: the browser has no
//! machine preference. The record itself is built by
//! `pleochroma_web_core::scene::design_effects_spec`, which the phone app calls too.

use crate::app::Ctx;
use pleochroma_web_core::{
    scene::{EffectsSpec, design_effects_spec},
    settings::render_material,
};

/// The render physics of the open design, or the default record when there is no design, no
/// matching solve or nothing is set.
pub(in crate::render) fn live_effects(ctx: &Ctx) -> EffectsSpec {
    let app = ctx.state.borrow();
    let Some(state) = &app.design else {
        return EffectsSpec::default();
    };
    let design = &state.session.design;
    // The traced material only decides the silk's Second Axis, so it is resolved only for a
    // design that chooses inclusions.
    let traced = design
        .render_effects
        .inclusion_spec()
        .and_then(|_| render_material(&app.settings, Some(design), &app.custom_materials).ok());
    design_effects_spec(
        design,
        traced.as_ref().map(|material| material.name.as_str()),
        &app.custom_materials,
        app.current_solved(),
    )
}
