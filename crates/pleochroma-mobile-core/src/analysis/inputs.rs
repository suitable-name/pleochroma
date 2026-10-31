//! What a metrics or tilt job measures: the scene built from the app state, and the cheap
//! key that says whether a stored result still describes it.
//!
//! The key is compared on every refresh of the Checks tab, so it is built from plain
//! fields only (no plane copies, no material cloning).

use std::sync::Arc;

use pleochroma::renderer::env_map::EnvironmentMap;
use pleochroma_web_core::{
    scene::SceneSpec,
    settings::{SceneInputs, render_material, scene_spec_with_frosting},
};

use super::Kind;
use crate::{app_state::AppState, design_props::render_settings};

/// Every custom material of the session: those opened files brought, then those made in
/// the Stone tab (a name in both lists counts once).
fn custom_materials(state: &AppState) -> Vec<pleochroma::optics::materials::GemMaterial> {
    crate::document::custom_catalogue(state)
}

/// The id the held HDR map is given in a metrics request (there is only ever one).
pub(super) const HDR_ID: u64 = 1;

/// The scene to measure and the map it is lit by.
pub(super) struct Inputs {
    pub(super) tools: Vec<pleochroma::geometry::tool::ToolPrimitive>,
    /// The stone, material and light (frame size and exposure are irrelevant here).
    pub(super) spec: SceneSpec,
    /// The HDR map when it is the chosen lighting.
    pub(super) hdr: Option<Arc<EnvironmentMap>>,
}

/// The scene the Checks tab measures, from the current solve, material and lighting.
///
/// The pose is the renderer's default camera (about 66 degrees, near face-up, as on the
/// desktop): the numbers describe the stone, not the angle the viewport happens to show.
///
/// # Errors
///
/// A sentence when there is nothing to measure: no current solve, no facets, or a linked
/// material that resolves to nothing.
pub(super) fn scene_inputs(state: &AppState) -> Result<Inputs, String> {
    let generation = state.session.current_generation();
    let planes = state
        .solve
        .current_planes(generation)
        .ok_or_else(|| "Solve the design to measure it.".to_owned())?;
    if planes.is_empty() {
        return Err("The design has no facets to measure yet.".to_owned());
    }
    let tiers = state
        .solve
        .current_tiers(generation)
        .ok_or_else(|| "Solve the design to measure it.".to_owned())?;
    let settings = render_settings(state, true);
    let customs = custom_materials(state);
    let material = render_material(&settings, Some(&state.session.design), &customs)?;
    let hdr = state.hdr.active();
    // The finishes travel with the scene: the metrics pass and the tilt sweep both score the
    // frosted facets.
    let spec = scene_spec_with_frosting(
        &settings,
        &SceneInputs {
            planes,
            material: &material,
            custom_materials: &customs,
            width: 64,
            height: 64,
            hdr_id: hdr.as_ref().map(|_| HDR_ID),
        },
        &state.session.design.frosted_planes_from_solved(tiers),
    );
    let (tools, _) = state
        .session
        .design
        .concave_tools_from_solved(tiers)
        .map_err(|error| error.to_string())?;
    Ok(Inputs { tools, spec, hdr })
}

/// Everything a stored result depends on. Two equal keys mean the same numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    document: u64,
    generation: u64,
    material: String,
    linked: bool,
    stone_width: u32,
    /// Inclusions and edge rounding (metrics only).
    effects: [u32; 2],
    /// The Crystal Axis: tilt, azimuth and the Second Axis turn (`u32::MAX` when not set);
    /// zero while the override is off. Both measurements use it: the tilt sweep scores the
    /// stone as oriented.
    axis: [u32; 3],
    c_axis_on: bool,
    preset: String,
    light: [u32; 2],
    /// [`crate::hdr::HdrState::identity`] (metrics only).
    hdr: usize,
    /// The frosted-girdle toggle and the design's frosted planes (both measurements score the
    /// frosted stone, so both keys carry them).
    frosting: (bool, Vec<u32>),
    customs: crate::operations::InputFingerprint,
}

impl Key {
    /// The key of `kind` for the app as it stands, `None` while no solve is current.
    ///
    /// The tilt sweep ignores inclusions, edge rounding and the HDR map, so those parts of its
    /// key stay zero and changing them does not stale its curves. It does honour the crystal
    /// orientation (the axis and the Second Axis), so those are part of the tilt key too. The
    /// frosting is part of both keys: the metrics pass scores the frosted stone as the tilt
    /// sweep does.
    pub(super) fn new(state: &AppState, kind: Kind) -> Option<Self> {
        let generation = state.session.current_generation();
        state.solve.current_planes(generation)?;
        let tiers = state.solve.current_tiers(generation)?;
        let settings = render_settings(state, true);
        let metrics = kind == Kind::Metrics;
        let effect = |on: bool, value: f32| if on { value.to_bits() } else { 0 };
        Some(Self {
            document: state.operations.document_id(),
            generation,
            material: settings.material,
            linked: settings.link_material,
            stone_width: settings.stone_width_mm.to_bits(),
            effects: [
                effect(metrics, settings.inclusion_sigma_s),
                effect(metrics, settings.edge_rounding_radius),
            ],
            axis: [
                effect(settings.c_axis_override, settings.c_axis_tilt_deg),
                effect(settings.c_axis_override, settings.c_axis_azimuth_deg),
                if settings.c_axis_override && settings.c_axis_second_on {
                    settings.c_axis_second_deg.to_bits()
                } else {
                    u32::MAX
                },
            ],
            c_axis_on: settings.c_axis_override,
            preset: settings.lighting,
            light: [
                settings.light_yaw_deg.to_bits(),
                settings.light_pitch_deg.to_bits(),
            ],
            hdr: if metrics { state.hdr.identity() } else { 0 },
            frosting: (
                settings.frosted_girdle,
                state.session.design.frosted_planes_from_solved(tiers),
            ),
            customs: crate::operations::InputFingerprint::from_parts([serde_json::to_vec(
                &crate::document::custom_catalogue(state),
            )
            .ok()?
            .as_slice()]),
        })
    }
}
