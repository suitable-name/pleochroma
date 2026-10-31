//! [`SceneEffects`]: the render physics a design adds to a scene, as plain wire data.
//!
//! Oriented inclusions and play-of-color are not part of [`GemMaterial`]'s serde bytes
//! (`GemMaterial::inclusions` is `#[serde(skip)]`, like the zones), and the surface finish lives
//! beside the planes, so they travel in this one record next to the material. A worker applies
//! it with [`SceneEffects::apply_to_material`] and [`SceneEffects::surface_scene`] and renders
//! exactly what the desktop renders.
//!
//! The faceting machine is the sending user's preference, never part of a design; it travels
//! here so a remote render of a design uses the machine of the person who asked for it.
//!
//! The default record is "no effects": an untouched design sends it, a worker applies nothing
//! and the render is bit-identical to one without this module.

use std::borrow::Cow;

use super::material_inclusions::MaterialInclusions;
use glam::Vec3;
use pleochroma::optics::{
    inclusions::{bragg::StructureSpec, spec::InclusionSpec},
    materials::GemMaterial,
    raytracer::surface::{EdgeAbrasion, FacetSurface, FacetingMachine, SurfaceScene, SweepRange},
};

/// The longest preset name a scene may carry.
pub const MAX_PRESET_NAME_LEN: usize = 64;

/// The most facet words a scene may carry (a stone has far fewer planes; the worker also
/// checks the count against the planes).
pub const MAX_FACET_SURFACES: usize = 4096;

/// The largest lap, mast distance or sweep radius a scene may name, in millimetres.
pub const MAX_MACHINE_MM: f32 = 10_000.0;

/// The faceting machine as it travels: [`FacetingMachine`] field for field.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
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

/// The abraded-edge setting as it travels: [`EdgeAbrasion`] field for field.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
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
/// Appended to [`super::SceneState`] (protocol v32; v34 appended [`Self::material_inclusions`]).
/// Every field is "off" at its default, which postcard still writes (it cannot omit a field).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct SceneEffects {
    /// The design's inclusion choice (`[render_effects.inclusions]`), resolved against the
    /// scene's material by the worker.
    pub inclusions: Option<InclusionSpec>,
    /// The stored Second Axis of a uniaxial stone, which the silk reads as its Silk Turn (the
    /// optics never read it); `None` for a biaxial stone, whose material carries its own.
    pub inclusion_second_axis: Option<[f32; 3]>,
    /// The design's play-of-color choice (`[render_effects.structure]`), seed included.
    pub structure: Option<StructureSpec>,
    /// One packed [`FacetSurface`] word per plane, in plane order; empty is ideal. A shorter
    /// list is padded with ideal facets.
    pub facet_surfaces: Vec<u32>,
    /// The sender's faceting machine; read only when a facet word asks for scratches.
    pub machine: MachineSpec,
    /// The abraded-edge setting.
    pub edge: EdgeSpec,
    /// The inclusions the MATERIAL itself carries (a custom material's stored families and
    /// play-of-color structure, protocol v34), which `GemMaterial` leaves out of its serde bytes
    /// and the name-keyed choices above cannot find. Applied first, so a preset the design
    /// chooses still replaces them, exactly as on the desktop. Appended last for the postcard
    /// reason of [`super::SceneState::tools`]; `#[serde(default)]` for the `scene.json` reason.
    #[serde(default)]
    pub material_inclusions: Option<MaterialInclusions>,
}

impl SceneEffects {
    /// `true` when nothing is set: the scene renders as if this record did not exist.
    #[must_use]
    pub fn is_none(&self) -> bool {
        self.inclusions.is_none()
            && self.structure.is_none()
            && self.material_inclusions.is_none()
            && self.facet_surfaces.is_empty()
            && !self.edge.to_abrasion().active()
    }

    /// `true` when the surface finish departs from a perfect polish (facet words or an abraded
    /// edge). The GPU kernels do not read the surface word yet, so a scene like this is traced
    /// on the CPU.
    #[must_use]
    pub fn has_surface(&self) -> bool {
        !self.facet_surfaces.is_empty() || self.edge.to_abrasion().active()
    }

    /// The traced material: `material` with the material's own inclusions
    /// ([`Self::material_inclusions`], unless `material` already carries a set, as a desktop
    /// material does) and then the inclusion and structure choices applied, in the desktop's
    /// order (`with_inclusion_spec`, then `with_structure_spec`). Borrowed and untouched when
    /// none is set.
    #[must_use]
    pub fn apply_to_material<'a>(&self, material: &'a GemMaterial) -> Cow<'a, GemMaterial> {
        let carried = self
            .material_inclusions
            .as_ref()
            .filter(|_| material.inclusions.is_none());
        if carried.is_none() && self.inclusions.is_none() && self.structure.is_none() {
            return Cow::Borrowed(material);
        }
        let mut traced = material.clone();
        if let Some(carried) = carried {
            traced = traced.with_inclusions(carried.to_set());
        }
        if let Some(spec) = &self.inclusions {
            let second = self.inclusion_second_axis.map(Vec3::from_array);
            traced = traced.with_inclusion_spec(spec, second);
        }
        if let Some(spec) = &self.structure {
            traced = traced.with_structure_spec(spec);
        }
        Cow::Owned(traced)
    }

    /// The facet words as [`FacetSurface`]s, parallel to the planes.
    #[must_use]
    pub fn surfaces(&self) -> Vec<FacetSurface> {
        self.facet_surfaces
            .iter()
            .copied()
            .map(FacetSurface::from_packed)
            .collect()
    }

    /// The surface input of a trace, over `surfaces` (the result of [`Self::surfaces`], which
    /// the caller keeps alive). An untouched record gives the ideal scene.
    #[must_use]
    pub fn surface_scene<'a>(&self, surfaces: &'a [FacetSurface]) -> SurfaceScene<'a> {
        if !self.has_surface() {
            return SurfaceScene::IDEAL;
        }
        SurfaceScene {
            facet_surfaces: surfaces,
            machine: self.machine.to_machine(),
            edge: self.edge.to_abrasion(),
        }
    }

    /// Refuses a record no honest sender produces. `planes` is the scene's plane count.
    ///
    /// # Errors
    ///
    /// The first problem, as a sentence.
    pub fn validate(&self, planes: usize) -> Result<(), String> {
        if let Some(spec) = &self.inclusions {
            validate_inclusions(spec)?;
        }
        if let Some(axis) = self.inclusion_second_axis
            && !axis.iter().all(|v| v.is_finite())
        {
            return Err("effects.inclusion_second_axis is non-finite".to_owned());
        }
        if let Some(spec) = &self.structure {
            validate_structure(spec)?;
        }
        if let Some(carried) = &self.material_inclusions {
            carried.validate()?;
        }
        if self.facet_surfaces.len() > planes.min(MAX_FACET_SURFACES) {
            return Err(format!(
                "effects.facet_surfaces has {} entries for {} facet(s)",
                self.facet_surfaces.len(),
                planes
            ));
        }
        validate_machine(&self.machine)?;
        validate_edge(&self.edge)
    }
}

fn validate_inclusions(spec: &InclusionSpec) -> Result<(), String> {
    if spec.preset.len() > MAX_PRESET_NAME_LEN {
        return Err(format!(
            "effects.inclusions.preset is longer than {MAX_PRESET_NAME_LEN} bytes"
        ));
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
    Ok(())
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

#[cfg(test)]
#[path = "effects_tests.rs"]
mod tests;
