//! One adaptive viewport pass on the CPU: the samples of a [`Pass`] for the pixels of its live
//! tiles.
//!
//! The live tiles are claimed one at a time from an atomic counter by scoped threads, so a tile
//! through the stone (slow) and a tile of background (fast) balance out. A thread traces a whole
//! tile into its own small buffer, one `(pass sum, odd-sample sum)` pair per pixel; nothing is
//! shared while tracing. Once every thread has finished, the tiles' buffers are added into the
//! frame's `sum` and `half_b` in live-tile order, on the calling thread. A pixel's pair depends
//! only on `(pixel, sample index)` and each pixel belongs to exactly one tile, so the result does
//! not depend on the thread count or on which thread got which tile.
//!
//! The per-pixel arithmetic is `renderer::cpu_frame::trace_live_tiles_surface`'s, step for step
//! (the pass total is formed first, from zero and in ascending sample order, then added to the
//! frame), as the export thread's pass does. The viewport's own tracing partition is respected
//! like the scanline tracer does: one core stays free for the display thread and the tracing
//! lease is held for the pass.

use crate::bridge::render_thread::gpu_backend::BackendFrame;
use glam::Vec3;
use pleochroma::{
    optics::raytracer::{
        FacetFinish, add_finite_sample, build_plane_soa, pixel_rotations, sample_draws,
        surface::merged_facet_finishes,
        transport::{PreparedMaterial, trace_spectral_ray_prepared},
    },
    renderer::{adaptive::Pass, path_cache::PathCacheFrame, tonemap::live_cpu_budget},
    simd::PlanesSoA32,
};
use std::{
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

/// The `(pass sum, odd-sample sum)` of every pixel of one tile rectangle, row-major.
type TileResult = Vec<(Vec3, Vec3)>;

/// Everything the tracer of one pass reads, built once per pass and shared by every thread.
struct PassTracer<'a, 'scene> {
    frame: &'a BackendFrame<'scene>,
    planes: &'a PlanesSoA32,
    finishes: &'a [FacetFinish],
    prepared: &'a PreparedMaterial<'a>,
    /// The viewport's path cache: a pixel it has positions for is shaded from its records.
    cache: Option<&'a PathCacheFrame<'a, 'a>>,
}

impl PassTracer<'_, '_> {
    /// The pairs of every pixel of the rectangle `(x0, y0, x1, y1)` for the samples
    /// `first_sample..first_sample + spp`, or `None` when `cancel` was raised (checked per row).
    fn trace_rect(
        &self,
        (x0, y0, x1, y1): (u32, u32, u32, u32),
        pass: &Pass,
        cancel: &AtomicBool,
    ) -> Option<TileResult> {
        let frame = self.frame;
        let mut out = Vec::with_capacity((x1 - x0) as usize * (y1 - y0) as usize);
        for y in y0..y1 {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            for x in x0..x1 {
                let pixel = y * frame.width + x;
                let rot = pixel_rotations(pixel);
                let mut pass_sum = Vec3::ZERO;
                let mut pass_odd = Vec3::ZERO;
                for local_sample in 0..pass.spp {
                    let sample_num = pass.first_sample.saturating_add(local_sample);
                    let cached = self
                        .cache
                        .and_then(|cache| cache.shade_sample((x, y), sample_num, &rot));
                    let sample = cached.map_or_else(
                        || {
                            let draws = sample_draws(pixel, sample_num, &rot);
                            let ray = frame.camera.generate_ray(
                                x as f32,
                                y as f32,
                                frame.width as f32,
                                frame.height as f32,
                                draws.jitter_x,
                                draws.jitter_y,
                            );
                            trace_spectral_ray_prepared(
                                ray,
                                frame.stone(),
                                self.planes,
                                self.finishes,
                                self.prepared,
                                frame.fluorescence,
                                frame.max_bounces,
                                frame.environment,
                                draws.seed,
                                draws.hero_rand,
                                None,
                                frame.surface,
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
        Some(out)
    }
}

/// Adds the samples of `pass` for every pixel of its live tiles to `sum`, and the odd-index ones
/// to `half_b`; pixels outside the live tiles are untouched. Returns `true` when `cancel` was
/// raised before the pass finished, in which case neither buffer was touched.
///
/// Does nothing (and returns `false`) when the buffers are not `frame.width * frame.height` long,
/// the pass describes another frame size, or there is nothing to trace.
///
/// With the viewport's path cache (`cache` set, built for this frame size) every pixel the cache
/// has positions for is shaded from its records and the others are traced; `buffers` is
/// `(sum, half_b)`. With `None` every pixel is traced.
pub(super) fn trace_live_pass_cached(
    frame: &BackendFrame<'_>,
    pass: &Pass,
    buffers: (&mut [Vec3], &mut [Vec3]),
    cache: Option<&PathCacheFrame<'_, '_>>,
    cancel: &AtomicBool,
) -> bool {
    let (sum, half_b) = buffers;
    let cache = cache.filter(|cache| cache.matches(frame.width, frame.height));
    let (width, height) = (frame.width, frame.height);
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
    if cancel.load(Ordering::Relaxed) {
        return true;
    }

    let _lease = live_cpu_budget::single_core_lease();
    let _trace_lease = live_cpu_budget::trace_lease();
    let plane_soa = build_plane_soa(frame.planes);
    let finishes = merged_facet_finishes(frame.facet_finishes, frame.surface.facet_surfaces);
    let prepared = PreparedMaterial::new(frame.material);
    let tracer = PassTracer {
        frame,
        planes: &plane_soa,
        finishes: &finishes,
        prepared: &prepared,
        cache,
    };

    let threads = live_cpu_budget::partition().0.min(tiles.len()).max(1);
    let next_tile = AtomicUsize::new(0);
    let traced_tiles: Mutex<Vec<(usize, TileResult)>> = Mutex::new(Vec::new());
    let cancelled = AtomicBool::new(false);
    thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let mut mine = Vec::new();
                loop {
                    let position = next_tile.fetch_add(1, Ordering::Relaxed);
                    let Some(&tile) = tiles.get(position) else {
                        break;
                    };
                    let rect = pass.live.tile_rect(tile);
                    let Some(result) = tracer.trace_rect(rect, pass, cancel) else {
                        cancelled.store(true, Ordering::Relaxed);
                        break;
                    };
                    mine.push((position, result));
                }
                traced_tiles
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend(mine);
            });
        }
    });
    if cancelled.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
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
