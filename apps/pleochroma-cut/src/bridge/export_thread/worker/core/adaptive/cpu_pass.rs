//! One adaptive pass on the CPU: the samples of a [`Pass`] for the pixels of its live tiles.
//!
//! # Threads
//!
//! The live tiles are claimed one at a time from an atomic counter, so a tile through the stone
//! (slow) and a tile of background (fast) balance out. A thread traces a whole tile into its own
//! small buffer, one `(pass sum, odd-sample sum)` pair per pixel; nothing is shared while tracing.
//! Once every thread has finished, the tiles' buffers are added into the frame's `sum` and
//! `half_b` in live-tile order, on the calling thread. A pixel's pair depends only on
//! `(pixel, sample index)`, and each pixel belongs to exactly one tile, so the result does not
//! depend on the thread count or on which thread got which tile.
//!
//! The per-pixel arithmetic is `renderer::cpu_frame::trace_live_tiles_surface`'s, step for step
//! (the pass total is formed first, from zero and in ascending sample order, then added to the
//! frame), so the two agree bit for bit; the tests pin that.

use crate::bridge::export_thread::{batch::ExportCtx, cache_run::CacheRun};
use glam::Vec3;
use pleochroma::{
    geometry::tool::StoneGeometry,
    optics::{
        fluorescence::Fluorescence,
        raytracer::{
            Camera, EnvironmentSource, FacetFinish, add_finite_sample, build_plane_soa,
            pixel_rotations, sample_draws,
            surface::{SurfaceScene, merged_facet_finishes},
            transport::{PreparedMaterial, trace_spectral_ray_prepared},
        },
    },
    renderer::{adaptive::Pass, path_cache::PathCacheFrame},
    simd::PlanesSoA32,
};
use std::{
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

/// A traced tile: its position among the live tiles, and one `(pass sum, odd sum)` per pixel.
type TracedTile = (usize, Vec<(Vec3, Vec3)>);

/// Everything the tracer of one pass reads, built once per pass and shared by every thread.
struct PassTracer<'a, 'm> {
    camera: &'a Camera,
    width: u32,
    height: u32,
    stone: StoneGeometry<'a>,
    plane_soa: &'a PlanesSoA32,
    finishes: &'a [FacetFinish],
    prepared: &'a PreparedMaterial<'m>,
    fluorescence: &'a Fluorescence,
    max_bounces: u32,
    environment: EnvironmentSource<'a>,
    surface: SurfaceScene<'a>,
    /// The render's path cache: a pixel it has positions for is shaded from its records.
    cache: Option<&'a PathCacheFrame<'a, 'a>>,
}

impl PassTracer<'_, '_> {
    /// The `(pass sum, odd-sample sum)` of every pixel of the rectangle `(x0, y0, x1, y1)`,
    /// row-major, for the samples `first_sample..first_sample + spp`.
    fn trace_rect(
        &self,
        (x0, y0, x1, y1): (u32, u32, u32, u32),
        first_sample: u32,
        spp: u32,
    ) -> Vec<(Vec3, Vec3)> {
        let mut out = Vec::with_capacity((x1 - x0) as usize * (y1 - y0) as usize);
        for y in y0..y1 {
            for x in x0..x1 {
                let pixel = y * self.width + x;
                let rot = pixel_rotations(pixel);
                let mut pass_sum = Vec3::ZERO;
                let mut pass_odd = Vec3::ZERO;
                for local_sample in 0..spp {
                    let sample_num = first_sample.saturating_add(local_sample);
                    let cached = self
                        .cache
                        .and_then(|cache| cache.shade_sample((x, y), sample_num, &rot));
                    let sample = cached.map_or_else(
                        || {
                            let draws = sample_draws(pixel, sample_num, &rot);
                            let ray = self.camera.generate_ray(
                                x as f32,
                                y as f32,
                                self.width as f32,
                                self.height as f32,
                                draws.jitter_x,
                                draws.jitter_y,
                            );
                            trace_spectral_ray_prepared(
                                ray,
                                self.stone,
                                self.plane_soa,
                                self.finishes,
                                self.prepared,
                                self.fluorescence,
                                self.max_bounces,
                                self.environment,
                                draws.seed,
                                draws.hero_rand,
                                None,
                                self.surface,
                            )
                        },
                        |parts| parts.total(),
                    );
                    // A non-finite sample is dropped but still counted, the rule of every
                    // other backend's sums.
                    add_finite_sample(&mut pass_sum, sample);
                    if !sample_num.is_multiple_of(2) {
                        add_finite_sample(&mut pass_odd, sample);
                    }
                }
                out.push((pass_sum, pass_odd));
            }
        }
        out
    }
}

/// Adds the samples of `pass` for every pixel of its live tiles to `sum`, and the odd-index ones
/// to `half_b`, on all CPU cores; pixels outside the live tiles are untouched. Returns `true`
/// when `cancel` was raised before the pass finished, in which case neither buffer was touched.
///
/// Does nothing (and returns `false`) when the buffers are not `ctx.width * ctx.height` long or
/// the pass describes another frame size.
pub(super) fn trace_live_pass(
    ctx: &ExportCtx<'_>,
    pass: &Pass,
    sum: &mut [Vec3],
    half_b: &mut [Vec3],
    cancel: &AtomicBool,
) -> bool {
    let (width, height) = (ctx.width, ctx.height);
    let pixels = width as usize * height as usize;
    let tiles = pass.live.indices();
    if sum.len() != pixels
        || half_b.len() != pixels
        || pass.live.width() != width
        || pass.live.height() != height
        || tiles.is_empty()
        || pass.spp == 0
    {
        return false;
    }

    let scene = ctx.scene;
    // The same inputs `batch::render_batch` builds, from the context the GPU is offered: the
    // traced material and the environment are the ones the GPU request carries.
    let plane_soa = build_plane_soa(&scene.active_planes);
    let prepared = PreparedMaterial::new(ctx.gpu_scene.material);
    let finishes = merged_facet_finishes(&scene.facet_finishes, ctx.surface.facet_surfaces);
    let tracer = PassTracer {
        camera: ctx.camera,
        width,
        height,
        stone: StoneGeometry {
            planes: &scene.active_planes,
            tools: &scene.tools,
        },
        plane_soa: &plane_soa,
        finishes: &finishes,
        prepared: &prepared,
        fluorescence: &scene.fluorescence,
        max_bounces: scene.max_bounces,
        environment: ctx.gpu_scene.environment,
        surface: ctx.surface,
        cache: ctx
            .path_cache
            .as_ref()
            .map(CacheRun::frame)
            .filter(|cache| cache.matches(width, height)),
    };

    let threads = thread::available_parallelism()
        .map_or(8, std::num::NonZero::get)
        .min(tiles.len())
        .max(1);
    let next_tile = AtomicUsize::new(0);
    let traced_tiles: Mutex<Vec<TracedTile>> = Mutex::new(Vec::new());
    thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let mut mine = Vec::new();
                while !cancel.load(Ordering::Relaxed) {
                    let position = next_tile.fetch_add(1, Ordering::Relaxed);
                    let Some(&tile) = tiles.get(position) else {
                        break;
                    };
                    let rect = pass.live.tile_rect(tile);
                    mine.push((
                        position,
                        tracer.trace_rect(rect, pass.first_sample, pass.spp),
                    ));
                }
                traced_tiles
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend(mine);
            });
        }
    });
    if cancel.load(Ordering::Relaxed) {
        return true;
    }

    // Live-tile order, whatever order the threads finished in.
    let mut traced_tiles = traced_tiles
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    traced_tiles.sort_unstable_by_key(|(position, _)| *position);
    for (position, results) in traced_tiles {
        let Some(&tile) = tiles.get(position) else {
            continue;
        };
        let (x0, y0, x1, y1) = pass.live.tile_rect(tile);
        let mut results = results.into_iter();
        for y in y0..y1 {
            for x in x0..x1 {
                let Some((total, odd)) = results.next() else {
                    break;
                };
                let pixel = (y * width + x) as usize;
                sum[pixel] += total;
                half_b[pixel] += odd;
            }
        }
    }
    false
}
