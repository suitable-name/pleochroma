//! The surface-finish hooks on the bounce loop's [`PathState`].
//!
//! A child module of `bounce_body` so the hooks read the loop's private state. With the ideal
//! scene both are cheap pass-throughs; the logic lives in `optics::raytracer::surface::dispatch`.

use super::{HERO_IDX, PathState};
use crate::optics::raytracer::{
    camera::{FacetFinish, HitRecord},
    surface::dispatch::{
        FacetSurfaceEvent, SurfaceOutcome, SurfaceQuery, begin_facet_surface, finish_facet_surface,
    },
};
use glam::Vec3;

impl PathState<'_> {
    /// Before a facet's interface event: resolves the facet's surface finish (ground lobe, polish
    /// halo draw, abraded edge) from the macroscopic shading normal `normal_out`.
    pub(super) fn begin_surface(
        &self,
        hit_rec: HitRecord,
        hit_point: Vec3,
        normal_out: Vec3,
        bounce: u32,
        finish: FacetFinish,
    ) -> FacetSurfaceEvent {
        begin_facet_surface(&SurfaceQuery {
            scene: &self.scene.surface,
            planes: self.scene.planes,
            tools: self.scene.tools,
            material: self.scene.material,
            finish,
            facet_idx: hit_rec.facet_idx,
            hit_point,
            normal_out,
            k: self.current_k,
            inside_gem: self.inside_gem,
            lambdas: &self.lambdas,
            hero_idx: HERO_IDX,
            split_radiance: &*self.exit_split_ctx.split_radiance,
            rng_seed: self.rng_seed,
            bounce,
        })
    }

    /// After the interface event: the throughput correction of a polish with roughness.
    pub(super) fn finish_surface(&mut self, event: &FacetSurfaceEvent, inside_before: bool) {
        let outcome = SurfaceOutcome {
            inside_before,
            inside_after: self.inside_gem,
            new_k: self.current_k,
        };
        finish_facet_surface(
            event,
            outcome,
            &mut self.stokes,
            &mut self.path_pdf,
            self.exit_split_ctx.split_radiance,
        );
    }
}
