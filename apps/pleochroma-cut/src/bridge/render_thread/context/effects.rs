//! The render physics the context carries: oriented inclusions, play-of-color, the per-facet
//! surface finish and the abraded edge, all in the one wire record the remote render uses
//! (`SceneEffects`) so a local frame, an export and a remote worker trace exactly the same
//! stone.
//!
//! The design supplies the parts (`gui::render::stone_effects` for the effects,
//! `bridge::render_effects` for the facet words); the faceting machine is the user's own
//! preference and is held beside them. With nothing set the record is
//! `SceneEffects::default()` and every consumer behaves as it did before the feature existed.

use super::RenderContext;
use crate::bridge::render_effects::words_for_planes;
use pleochroma::{
    geometry::GpuFacetPlane,
    optics::{
        inclusions::{bragg::StructureSpec, spec::InclusionSpec},
        materials::GemMaterial,
        raytracer::surface::FacetingMachine,
    },
};
use pleochroma_net::scene::effects::{EdgeSpec, MachineSpec};
use std::borrow::Cow;

impl RenderContext {
    /// Sets the facet surface finish of the stone [`Self::claim_active_geometry`] just claimed,
    /// from the whole stone's table (`bridge::render_effects::surface_table`; empty for an ideal
    /// stone). The words of the traced planes follow from it. Call it right after an accepted
    /// claim, under the same lock. Does NOT set `dirty`, like the claim.
    pub fn claim_active_surfaces(&mut self, table: Vec<(GpuFacetPlane, u32)>) {
        let words = words_for_planes(&table, &self.active_planes);
        self.surface_table = table;
        if self.scene_effects.facet_surfaces != words {
            self.scene_effects.facet_surfaces = words;
            self.sync_machine_into_effects();
        }
    }

    /// Sets the design's inclusion, play-of-color and abraded-edge choices (the facet words
    /// are [`Self::claim_active_surfaces`]'s). `second_axis` is the stored Second Axis of a
    /// uniaxial stone, which the silk reads as its turn. Does NOT set `dirty`.
    pub fn set_design_physics(
        &mut self,
        inclusions: Option<InclusionSpec>,
        second_axis: Option<[f32; 3]>,
        structure: Option<StructureSpec>,
        edge: EdgeSpec,
    ) {
        let effects = &mut self.scene_effects;
        effects.inclusions = inclusions;
        effects.inclusion_second_axis = second_axis;
        effects.structure = structure;
        effects.edge = edge;
        self.sync_machine_into_effects();
    }

    /// Sets the user's faceting machine (the scratch directions). A change only matters, and
    /// only restarts the render, while the stone has a surface finish.
    pub fn set_faceting_machine(&mut self, machine: FacetingMachine) {
        if self.faceting_machine != machine {
            self.faceting_machine = machine;
            self.sync_machine_into_effects();
        }
    }

    /// Writes the user's machine into the effects record while the stone has a surface finish,
    /// and the default machine otherwise, so a stone without one keeps the default record.
    pub(super) fn sync_machine_into_effects(&mut self) {
        let machine = if self.scene_effects.has_surface() {
            MachineSpec::from_machine(&self.faceting_machine)
        } else {
            MachineSpec::default()
        };
        if self.scene_effects.machine != machine {
            self.scene_effects.machine = machine;
        }
    }

    /// `material` as the tracer sees it: with the design's inclusions and play-of-color applied
    /// (borrowed and untouched without them).
    #[must_use]
    pub fn traced_material<'a>(&self, material: &'a GemMaterial) -> Cow<'a, GemMaterial> {
        self.scene_effects.apply_to_material(material)
    }
}
