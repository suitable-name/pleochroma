//! [`EffectsSpec`]: the render physics of a design, riding beside the unchanged
//! [`SceneSpec`](super::SceneSpec).
//!
//! The browser is view-only for these effects (decision D17): the page reads the design's
//! `[render_effects]` keys and sends them with `ToWorker::SetEffects`; there are no editing
//! controls. The record has the same shape as the desktop remote protocol's `SceneEffects`
//! (`pleochroma_net::scene::effects`), which this crate cannot depend on, so a design renders
//! identically on the desktop, a remote worker and in the browser.
//!
//! Inclusions and play-of-color are applied to the scene's material
//! ([`OwnedScene::set_effects`]); the facet surface words, the sender's faceting machine and the
//! abraded edge become the trace's surface input ([`OwnedScene::surface_scene`]). An empty record
//! changes nothing, bit for bit.

use std::borrow::Cow;

use glam::Vec3;
use pleochroma::optics::{
    inclusions::{bragg::StructureSpec, spec::InclusionSpec},
    materials::GemMaterial,
    raytracer::surface::{EdgeAbrasion, FacetSurface, FacetingMachine, SurfaceScene, SweepRange},
};
use serde::{Deserialize, Serialize};

use super::OwnedScene;

/// The longest preset name a record may carry.
pub const MAX_PRESET_NAME_LEN: usize = 64;

/// The most facet words a record may carry.
pub const MAX_FACET_SURFACES: usize = 4096;

/// The largest lap, mast distance or sweep radius a record may name, in millimetres.
pub const MAX_MACHINE_MM: f32 = 10_000.0;

/// The faceting machine as it travels: `FacetingMachine` field for field.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MachineSpec {
    /// The angle in degrees between the dop arm and the radial line through the contact point.
    pub arm_offset_deg: f32,
    /// The radius of the lap in millimetres.
    pub lap_radius_mm: f32,
    /// The distance in millimetres of the line of the dop arm from the lap centre.
    pub mast_distance_mm: f32,
    /// The smaller contact radius of the last polishing pass.
    pub sweep_inner_mm: f32,
    /// The larger contact radius of the last polishing pass.
    pub sweep_outer_mm: f32,
}

impl MachineSpec {
    /// The machine of this record.
    #[must_use]
    pub const fn to_machine(self) -> FacetingMachine {
        FacetingMachine {
            arm_offset_deg: self.arm_offset_deg,
            lap_radius_mm: self.lap_radius_mm,
            mast_distance_mm: self.mast_distance_mm,
            sweep: SweepRange {
                inner_mm: self.sweep_inner_mm,
                outer_mm: self.sweep_outer_mm,
            },
        }
    }

    /// The record of `machine`.
    #[must_use]
    pub const fn from_machine(machine: &FacetingMachine) -> Self {
        Self {
            arm_offset_deg: machine.arm_offset_deg,
            lap_radius_mm: machine.lap_radius_mm,
            mast_distance_mm: machine.mast_distance_mm,
            sweep_inner_mm: machine.sweep.inner_mm,
            sweep_outer_mm: machine.sweep.outer_mm,
        }
    }
}

impl Default for MachineSpec {
    /// The default machine: the arm points at the lap centre.
    fn default() -> Self {
        Self::from_machine(&FacetingMachine::DEFAULT)
    }
}

/// The abraded-edge setting as it travels: `EdgeAbrasion` field for field.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct EdgeSpec {
    /// The width of the band in model units; `0` switches the band off.
    pub width: f32,
    /// The GGX roughness of the band.
    pub alpha: f32,
    /// The roughness in nanometres the polish next to the band is raised to; `0` is off.
    pub halo_rq_nm: f32,
}

impl EdgeSpec {
    /// The abrasion of this record.
    #[must_use]
    pub const fn to_abrasion(self) -> EdgeAbrasion {
        EdgeAbrasion {
            width: self.width,
            alpha: self.alpha,
            halo_rq_nm: self.halo_rq_nm,
        }
    }

    /// The record of `edge`.
    #[must_use]
    pub const fn from_abrasion(edge: &EdgeAbrasion) -> Self {
        Self {
            width: edge.width,
            alpha: edge.alpha,
            halo_rq_nm: edge.halo_rq_nm,
        }
    }
}

/// The render physics of one scene: inclusions, play-of-color and surface finish.
///
/// The default record is "no effects".
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EffectsSpec {
    /// The design's inclusion choice, resolved against the scene's material by the Worker.
    pub inclusions: Option<InclusionSpec>,
    /// The stored Second Axis of a uniaxial stone (the Silk Turn); `None` for a biaxial stone.
    pub inclusion_second_axis: Option<[f32; 3]>,
    /// The design's play-of-color choice, seed included.
    pub structure: Option<StructureSpec>,
    /// One packed `FacetSurface` word per plane, in plane order; empty is ideal.
    pub facet_surfaces: Vec<u32>,
    /// The sender's faceting machine; read only when a facet word asks for scratches.
    pub machine: MachineSpec,
    /// The abraded-edge setting.
    pub edge: EdgeSpec,
}

impl EffectsSpec {
    /// `true` when nothing is set.
    #[must_use]
    pub fn is_none(&self) -> bool {
        self.inclusions.is_none()
            && self.structure.is_none()
            && self.facet_surfaces.is_empty()
            && !self.edge.to_abrasion().active()
    }

    /// `true` when the surface finish departs from a perfect polish.
    #[must_use]
    pub fn has_surface(&self) -> bool {
        !self.facet_surfaces.is_empty() || self.edge.to_abrasion().active()
    }

    /// `material` with the inclusion and structure choices applied, in the desktop's order
    /// (`with_inclusion_spec`, then `with_structure_spec`); borrowed when neither is set.
    #[must_use]
    pub fn apply_to_material<'a>(&self, material: &'a GemMaterial) -> Cow<'a, GemMaterial> {
        if self.inclusions.is_none() && self.structure.is_none() {
            return Cow::Borrowed(material);
        }
        let mut traced = material.clone();
        if let Some(spec) = &self.inclusions {
            let second = self.inclusion_second_axis.map(Vec3::from_array);
            traced = traced.with_inclusion_spec(spec, second);
        }
        if let Some(spec) = &self.structure {
            traced = traced.with_structure_spec(spec);
        }
        Cow::Owned(traced)
    }

    /// The facet words as `FacetSurface`s, parallel to the planes.
    #[must_use]
    pub fn surfaces(&self) -> Vec<FacetSurface> {
        self.facet_surfaces
            .iter()
            .copied()
            .map(FacetSurface::from_packed)
            .collect()
    }

    /// Refuses a record no honest sender produces. `planes` is the scene's plane count.
    ///
    /// # Errors
    ///
    /// The first problem, as a sentence.
    pub fn validate(&self, planes: usize) -> Result<(), String> {
        if let Some(spec) = &self.inclusions {
            if spec.preset.len() > MAX_PRESET_NAME_LEN {
                return Err("effects.inclusions.preset is too long".to_owned());
            }
            if !spec.density.is_finite() || !(0.0..=1.0).contains(&spec.density) {
                return Err(format!(
                    "effects.inclusions.density must be finite in 0..=1 (got {})",
                    spec.density
                ));
            }
            if spec.turn_deg.is_some_and(|turn| !turn.is_finite()) {
                return Err("effects.inclusions.turn_deg is non-finite".to_owned());
            }
        }
        if let Some(axis) = self.inclusion_second_axis
            && !axis.iter().all(|v| v.is_finite())
        {
            return Err("effects.inclusion_second_axis is non-finite".to_owned());
        }
        if let Some(spec) = &self.structure {
            validate_structure(spec)?;
        }
        if self.facet_surfaces.len() > planes.min(MAX_FACET_SURFACES) {
            return Err(format!(
                "effects.facet_surfaces has {} entries for {planes} facet(s)",
                self.facet_surfaces.len()
            ));
        }
        validate_machine(&self.machine)?;
        validate_edge(&self.edge)
    }
}

fn validate_structure(spec: &StructureSpec) -> Result<(), String> {
    if let Some(size) = spec.size
        && (!size.is_finite() || size <= 0.0 || size > 10.0)
    {
        return Err(format!(
            "effects.structure.size must be finite in (0, 10] (got {size})"
        ));
    }
    let band_ok = spec.color_low_nm.is_finite()
        && spec.color_high_nm.is_finite()
        && spec.color_low_nm >= 200.0
        && spec.color_high_nm <= 2000.0
        && spec.color_low_nm <= spec.color_high_nm;
    if !band_ok {
        return Err(format!(
            "effects.structure color range {}..{} nm is implausible",
            spec.color_low_nm, spec.color_high_nm
        ));
    }
    for (name, value) in [("purity", spec.purity), ("coverage", spec.coverage)] {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(format!(
                "effects.structure.{name} must be finite in 0..=1 (got {value})"
            ));
        }
    }
    Ok(())
}

fn validate_machine(machine: &MachineSpec) -> Result<(), String> {
    let in_range = |v: f32, lo: f32| v.is_finite() && (lo..=MAX_MACHINE_MM).contains(&v);
    let ok = machine.arm_offset_deg.is_finite()
        && machine.arm_offset_deg.abs() <= 360.0
        && in_range(machine.lap_radius_mm, f32::MIN_POSITIVE)
        && in_range(machine.mast_distance_mm, -MAX_MACHINE_MM)
        && in_range(machine.sweep_inner_mm, 0.0)
        && in_range(machine.sweep_outer_mm, 0.0)
        && machine.sweep_inner_mm <= machine.sweep_outer_mm;
    if ok {
        Ok(())
    } else {
        Err(format!("effects.machine is implausible: {machine:?}"))
    }
}

fn validate_edge(edge: &EdgeSpec) -> Result<(), String> {
    let ok = edge.width.is_finite()
        && (0.0..=10.0).contains(&edge.width)
        && edge.alpha.is_finite()
        && (0.0..=1.0).contains(&edge.alpha)
        && edge.halo_rq_nm.is_finite()
        && (0.0..=1000.0).contains(&edge.halo_rq_nm);
    if ok {
        Ok(())
    } else {
        Err(format!("effects.edge is implausible: {edge:?}"))
    }
}

/// What an [`OwnedScene`] keeps of its effects record: the record, its decoded facet words and
/// the material it was applied to.
#[derive(Debug, Clone, Default)]
pub(super) struct AppliedEffects {
    spec: EffectsSpec,
    surfaces: Vec<FacetSurface>,
    /// The scene's material before the record was applied; `None` until one is.
    base_material: Option<GemMaterial>,
}

impl OwnedScene {
    /// Applies the design's render physics to the scene: the inclusions and play-of-color go
    /// into the traced material, the surface finish becomes [`Self::surface_scene`]. A second
    /// call replaces the first (the record is applied to the scene's original material).
    ///
    /// # Errors
    ///
    /// [`EffectsSpec::validate`]'s message; the scene is unchanged then.
    pub fn set_effects(&mut self, effects: EffectsSpec) -> Result<(), String> {
        effects.validate(self.planes.len())?;
        let base = self
            .effects
            .base_material
            .take()
            .unwrap_or_else(|| self.material.clone());
        self.material = effects.apply_to_material(&base).into_owned();
        self.effects = AppliedEffects {
            surfaces: effects.surfaces(),
            spec: effects,
            base_material: Some(base),
        };
        self.ray_prep.take();
        Ok(())
    }

    /// The effects record last applied (the default, "no effects", before any).
    #[must_use]
    pub const fn effects(&self) -> &EffectsSpec {
        &self.effects.spec
    }

    /// The surface input of a trace: the facet words, the sender's machine and the abraded
    /// edge; [`SurfaceScene::IDEAL`] without a surface finish, which traces bit-identically to a
    /// scene that never had a record.
    #[must_use]
    pub fn surface_scene(&self) -> SurfaceScene<'_> {
        if !self.effects.spec.has_surface() {
            return SurfaceScene::IDEAL;
        }
        SurfaceScene {
            facet_surfaces: &self.effects.surfaces,
            machine: self.effects.spec.machine.to_machine(),
            edge: self.effects.spec.edge.to_abrasion(),
        }
    }
}

#[cfg(test)]
#[path = "effects_tests.rs"]
mod tests;
