//! Girdle finish (bruted/frosted facets): `FacetFinish` is looked up per-hit via
//! `HitRecord::facet_idx` into a `&[FacetFinish]` slice PARALLEL to `&[GpuFacetPlane]` on
//! the CPU -- extending `GpuFacetPlane` itself would disturb that struct's layout, echo
//! test, and the `intersect_polyhedron` kernel, none of which need to know about finish.
//! The GPU encoding mirrors that with a SEPARATE storage buffer (one u32 per facet).
//! Still gets its own struct-echo test (`layout_check::run_facet_finish`).

use crate::optics::raytracer::{FacetFinish, surface::FacetSurface};

/// The `facet_finish` submodule.
///
/// The per-facet word is `surface::FacetSurface`'s packed `u32`: bits 0-1 are the kind, the
/// rest carry the roughness and the scratch parameters. Words `0` and `1` are the polished
/// and frosted facets of old. A frosted test must therefore compare the masked kind
/// (`word & KIND_MASK == FROSTED`), never the whole word; `shaders/transport_bounce`'s
/// `FACET_FINISH_KIND_MASK` is the WGSL twin of [`KIND_MASK`].
pub mod facet_finish {
    /// Identifier for polished.
    pub const POLISHED: u32 = 0;
    /// Identifier for frosted.
    pub const FROSTED: u32 = 1;
    /// A polish with a roughness (a halo of scattered light).
    pub const POLISHED_ROUGH: u32 = 2;
    /// A GGX ground surface.
    pub const GROUND: u32 = 3;
    /// The bits of the word that hold the kind.
    pub const KIND_MASK: u32 = 0b11;
}

/// Encodes a `&[FacetFinish]` slice into a `facet_finish::{POLISHED,FROSTED}`-valued
/// `Vec<u32>` of exactly `num_planes` entries.
///
/// Mirrors `trace_spectral_ray_with_finish`'s per-facet lookup semantics
/// (`facet_finishes.get(i).copied().unwrap_or_default()`): an index past the end of a
/// shorter slice, or an empty slice, defaults to `FacetFinish::Polished`, matching the
/// CPU's "no explicit finish means polished" default.
#[must_use]
pub fn encode_facet_finishes(finishes: &[FacetFinish], num_planes: usize) -> Vec<u32> {
    (0..num_planes)
        .map(|i| match finishes.get(i).copied().unwrap_or_default() {
            FacetFinish::Polished => facet_finish::POLISHED,
            FacetFinish::Frosted => facet_finish::FROSTED,
        })
        .collect()
}

/// Encodes the per-facet surface finishes into the packed words of binding 8, exactly
/// `num_planes` of them.
///
/// A facet with a surface (`surfaces[i]` not Ideal) takes that surface's packed word. A facet
/// whose surface is Ideal (or missing) falls back to `finishes[i]`, so a frosted girdle without
/// a surface is still word `1`; with no surfaces at all the result equals
/// [`encode_facet_finishes`] word for word.
///
/// A frosted finish beats a polish with roughness, as it does in the CPU's `begin_facet_surface`
/// (the facet is ground with the default lobe): such a facet is word `1`, not the polish's word.
/// A Ground surface (kind 3) is never overridden: its own lobe is the one the CPU uses.
#[must_use]
pub fn encode_facet_surfaces(
    surfaces: &[FacetSurface],
    finishes: &[FacetFinish],
    num_planes: usize,
) -> Vec<u32> {
    (0..num_planes)
        .map(|i| {
            let surface = surfaces.get(i).copied().unwrap_or(FacetSurface::IDEAL);
            let finish = finishes.get(i).copied().unwrap_or_default();
            if surface.is_ground() {
                surface.packed()
            } else if finish == FacetFinish::Frosted {
                facet_finish::FROSTED
            } else if surface.is_ideal() {
                facet_finish::POLISHED
            } else {
                surface.packed()
            }
        })
        .collect()
}
