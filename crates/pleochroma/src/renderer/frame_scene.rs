//! [`FrameScene`]: the scene a frame tracer renders, independent of which backend traces it.
//!
//! Compiled without the `gpu` feature because the wasm-safe CPU tracer in [`super::cpu_frame`]
//! uses this struct in that build. The GPU path reaches it as the alias `GpuFrameScene`
//! (`renderer::gpu::frame`), so existing construction and import sites keep working.

use crate::{
    geometry::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, FacetFinish},
    },
};

/// Everything the frame tracer needs to render a frame, GPU or CPU.
///
/// Bundled so a caller assembles the scene once rather than per chunk (the GPU side)
/// or per worker (the CPU side).
pub struct FrameScene<'a> {
    /// Camera the frame is rendered from.
    pub camera: &'a Camera,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Facet planes of the stone.
    pub planes: &'a [GpuFacetPlane],
    /// Per-plane finish, indexed in step with `planes`. A shorter slice is padded with
    /// [`FacetFinish::default`], matching [`crate::renderer::buffers::encode_facet_finishes`].
    pub facet_finishes: &'a [FacetFinish],
    /// Gem material.
    pub material: &'a GemMaterial,
    /// Maximum number of internal bounces per path.
    pub max_bounces: u32,
    /// Lighting environment.
    pub environment: EnvironmentSource<'a>,
}
