//! Which stones may use the path cache (guide section 8.1).
//!
//! The decision is per render and per stone, never per pixel: a stone with anything the cached
//! branch tree does not reproduce is traced by the path tracer as before. Everything the records
//! reproduce (dispersion, uniaxial and biaxial modes, polarization, absorption, edge rounding)
//! stays eligible. A color-zoned stone is refused by `PathCacheFrame::new`, not here, because the
//! beam tracer shares these rules and renders zones.
//!
//! [`check_eligibility`] returns the first rule a stone breaks as a [`PathCacheOff`]; its
//! [`PathCacheOff::message`] is the text the log line and the interface show.
//!
//! The GPU frame's `classify_material` is not used: it only picks a kernel class, while the rules
//! here read the material's scattering, inclusion and look fields directly.

use std::fmt;

use crate::{
    geometry::tool::ToolPrimitive,
    optics::{
        fluorescence::Fluorescence,
        materials::{GemMaterial, MaterialLook},
        raytracer::{FacetFinish, surface::SurfaceScene},
    },
};

/// Why a stone cannot use the path cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathCacheOff {
    /// An escape must retain its position to locate the light on the paper.
    StoneShadow,
    /// The tray reflection sends an escape back into the scene along a floor bounce; the cache
    /// records end at the first escape.
    TrayReflection,
    /// The material has a Henyey-Greenstein haze (a scattering medium, opal's milky body).
    ScatteringMedium,
    /// The material has oriented inclusions or a domain structure with a phase function.
    OrientedInclusions,
    /// The material is one whose look is a design effect (opal, labradorite, moonstone).
    MaterialLook,
    /// A facet is frosted or ground.
    FrostedFacets,
    /// A facet has a polish with roughness, whose micro-normal halo the records do not carry.
    RoughPolish,
    /// The edges are abraded.
    AbradedEdges,
    /// The stone has concave tiers (tool volumes subtracted from the polyhedron).
    ConcaveTiers,
    /// The material fluoresces.
    Fluorescence,
    /// The camera has depth of field.
    DepthOfField,
    /// The material has color zones: a record holds the optical depth of one absorption tensor.
    ZonedColor,
}

impl PathCacheOff {
    /// The sentence the log line and the interface show.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::ScatteringMedium => "Path cache off: the material scatters light",
            Self::OrientedInclusions => "Path cache off: the stone has oriented inclusions",
            Self::MaterialLook => "Path cache off: the material has a play-of-color look",
            Self::FrostedFacets => "Path cache off: the stone has frosted facets",
            Self::RoughPolish => "Path cache off: the polish has roughness",
            Self::AbradedEdges => "Path cache off: the stone has abraded edges",
            Self::ConcaveTiers => "Path cache off: the stone has concave tiers",
            Self::Fluorescence => "Path cache off: the material fluoresces",
            Self::StoneShadow => {
                "Path cache off: the stone shadow needs exit positions the cache does not keep."
            }
            Self::TrayReflection => {
                "Path cache off: the tray reflection needs paths the cache does not keep."
            }
            Self::DepthOfField => "Path cache off: the camera has depth of field",
            Self::ZonedColor => "Path cache off: the material has color zones",
        }
    }
}

impl fmt::Display for PathCacheOff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for PathCacheOff {}

/// What the eligibility rules read about a render.
#[derive(Clone, Copy)]
pub struct StoneTraits<'x> {
    /// The stone's material.
    pub material: &'x GemMaterial,
    /// The per-facet finish list of the scene.
    pub facet_finishes: &'x [FacetFinish],
    /// The tool volumes subtracted from the polyhedron (empty for a convex stone).
    pub tools: &'x [ToolPrimitive],
    /// The fluorescent emitters beside the material.
    pub fluorescence: &'x Fluorescence,
    /// The surface finish of the facets and edges.
    pub surface: SurfaceScene<'x>,
    /// Whether the camera has depth of field.
    pub depth_of_field: bool,
}

/// Checks the rules of guide section 8.1; `Ok` means the stone may use the cache.
///
/// # Errors
///
/// Returns the first rule the stone breaks.
pub fn check_eligibility(traits: &StoneTraits<'_>) -> Result<(), PathCacheOff> {
    let material = traits.material;
    if material.scattering_sigma_s > 0.0 {
        return Err(PathCacheOff::ScatteringMedium);
    }
    if material.oriented_inclusions().is_some() {
        return Err(PathCacheOff::OrientedInclusions);
    }
    if !MaterialLook::of(&material.name).is_none() {
        return Err(PathCacheOff::MaterialLook);
    }
    if !traits.tools.is_empty() {
        return Err(PathCacheOff::ConcaveTiers);
    }
    let frosted_finish = traits.facet_finishes.contains(&FacetFinish::Frosted);
    if frosted_finish || traits.surface.facet_surfaces.iter().any(|s| s.is_ground()) {
        return Err(PathCacheOff::FrostedFacets);
    }
    if traits.surface.facet_surfaces.iter().any(|s| !s.is_ideal()) {
        return Err(PathCacheOff::RoughPolish);
    }
    if traits.surface.edge.active() {
        return Err(PathCacheOff::AbradedEdges);
    }
    if !traits.fluorescence.is_empty() {
        return Err(PathCacheOff::Fluorescence);
    }
    if traits.depth_of_field {
        return Err(PathCacheOff::DepthOfField);
    }
    Ok(())
}
