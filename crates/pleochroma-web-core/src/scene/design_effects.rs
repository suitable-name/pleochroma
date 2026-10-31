//! [`design_effects_spec`]: a design's render physics as the [`EffectsSpec`] every consumer takes.
//!
//! The browser app and the phone app both read the design's `[render_effects]` keys (inclusions,
//! play-of-color, abraded edge) and its tier surface finish and send them with the scene, so the
//! record is built in one place: a design renders the same stone on the desktop, in the browser
//! and on the phone, and a remote worker is sent the same record the local tracer applies.

use pleochroma::{
    geometry::meet_solver::SolvedTier,
    optics::{materials::GemMaterial, raytracer::surface::FacetSurface},
};
use pleochroma_cut_core::Design;
use pleochroma_editor::stone_orientation::orientation_override;

use super::{EdgeSpec, EffectsSpec, MachineSpec};

/// The render physics of `design`, or the default record ("no effects", bit for bit) when it
/// sets none.
///
/// `traced_name` is the name of the material the render traces (a library material, before the
/// design's overrides): it only decides the stored Second Axis a uniaxial stone's silk reads as
/// its turn (a biaxial stone carries its own), looked up among `custom_materials` and then the
/// catalogue; `None`, or a name that resolves to nothing, leaves the Second Axis unset.
/// `solved` is the design's current solve, which the per-facet surface words are read from: one
/// packed word per plane of the solved stone, in plane order, and none when every plane is ideal
/// or there is no solve. The faceting machine is the default one (neither the browser nor the
/// phone has a machine preference).
#[must_use]
pub fn design_effects_spec(
    design: &Design,
    traced_name: Option<&str>,
    custom_materials: &[GemMaterial],
    solved: Option<&[SolvedTier]>,
) -> EffectsSpec {
    let effects = &design.render_effects;
    let inclusions = effects.inclusion_spec();
    // The stored Second Axis of a uniaxial stone: only the silk reads it.
    let second_axis = if inclusions.is_some() {
        traced_name
            .and_then(|name| {
                custom_materials
                    .iter()
                    .find(|material| material.name == name)
                    .cloned()
                    .or_else(|| GemMaterial::by_name(name))
            })
            .filter(|material| material.biaxial_delta_beta_alpha.is_none())
            .and_then(|material| orientation_override(effects, &material))
            .and_then(|(_, alpha)| alpha)
            .map(|axis| axis.to_array())
    } else {
        None
    };
    // One packed word per plane of the solved stone, in plane order; empty when every plane is
    // ideal (the words only mean something against the planes the render traces).
    let facet_surfaces = match solved {
        Some(tiers) if design.has_surface() => {
            let ideal = FacetSurface::IDEAL.packed();
            let words: Vec<u32> = design
                .facet_surfaces_from_solved(tiers)
                .iter()
                .map(|surface| surface.packed())
                .collect();
            if words.iter().all(|word| *word == ideal) {
                Vec::new()
            } else {
                words
            }
        }
        _ => Vec::new(),
    };
    EffectsSpec {
        inclusions,
        inclusion_second_axis: second_axis,
        structure: effects.structure_spec(),
        facet_surfaces,
        machine: MachineSpec::default(),
        edge: EdgeSpec::from_abrasion(&effects.edge_abrasion()),
    }
}

#[cfg(test)]
#[path = "design_effects_tests.rs"]
mod tests;
