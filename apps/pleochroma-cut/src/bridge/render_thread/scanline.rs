//! The CPU scanline tracer: renders one full frame in parallel across
//! `thread::available_parallelism` CPU threads.

use super::gpu_backend::{BackendFrame, FrameOutputs};
use glam::Vec3;
use pleochroma::{
    geometry::tool::StoneGeometry,
    optics::raytracer::{
        FacetFinish, HitRecord, build_plane_soa, pixel_rotations, sample_draws,
        surface::merged_facet_finishes,
        transport::{PreparedMaterial, trace_spectral_ray_prepared},
    },
};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

/// One row's worth of the four output buffers, handed out to whichever thread's atomic
/// counter claims that row -- see [`render_frame_scanlines_cancellable`].
struct RowSlices<'a> {
    acc: &'a mut [Vec3],
    depth: &'a mut [f32],
    normal: &'a mut [Vec3],
    facet: &'a mut [i32],
}

/// Renders one full frame's scanlines in parallel across `thread::available_parallelism`
/// CPU threads, accumulating each pixel's radiance into `accum_buffer` and each pixel's
/// PRIMARY-ray first-hit depth/normal/facet-index into the three `first_hit_*` guide
/// buffers (these feed the À-Trous denoiser). Per-pixel tone-mapping is not done here --
/// that's `denoise_and_tonemap_frame`'s job, run once over the whole frame afterward
/// since the denoiser needs every pixel's guide data at once.
///
/// # Work distribution: a shared atomic row counter, not contiguous row bands
///
/// Rows through the stone cost far more than background rows, so splitting the image
/// into `num_threads` contiguous row bands starves threads that land entirely on cheap
/// background rows while stone-covering threads are still working -- measured on a
/// 480x360/2spp/16-thread render: 256ms wall time at 49% thread utilization for
/// contiguous bands, 145ms at 99% for the dynamic counter below. Each thread instead
/// repeatedly claims "the next unclaimed row" via `next_row.fetch_add(1, ..)`, so a
/// thread finishing a cheap row picks up the next one immediately rather than idling.
///
/// Each row's four output slices are handed out exactly once: `rows` pre-splits every
/// buffer into per-row slices via `chunks_mut(width)`, wrapped in a [`Mutex`]. A thread
/// claiming row `y` locks `rows` just long enough to `Option::take` that row's slices
/// (held per ROW, not per pixel), then processes the whole row without the lock held.
/// `next_row.fetch_add` hands out each index exactly once, so results stay
/// bit-identical to a contiguous-band split -- only which thread renders which
/// row changes run to run.
#[cfg(test)]
pub(super) fn render_frame_scanlines(
    frame: &BackendFrame<'_>,
    spp: u32,
    current_sample_count: u32,
    outputs: &mut FrameOutputs<'_>,
) {
    render_frame_scanlines_cancellable(
        frame,
        spp,
        current_sample_count,
        outputs,
        &AtomicBool::new(false),
    );
}

/// Stops between samples when the camera changes. A partial frame must be discarded.
pub(super) fn render_frame_scanlines_cancellable(
    frame: &BackendFrame<'_>,
    spp: u32,
    current_sample_count: u32,
    outputs: &mut FrameOutputs<'_>,
    cancel: &AtomicBool,
) -> bool {
    let num_threads = pleochroma::renderer::tonemap::live_cpu_budget::partition().0;
    render_frame_scanlines_with_threads_cancellable(
        frame,
        spp,
        current_sample_count,
        outputs,
        num_threads,
        cancel,
    )
}

/// [`render_frame_scanlines_cancellable`] on exactly `num_threads` scoped threads (at least one).
///
/// The hybrid CPU+GPU frame passes `cores - 2`: the GPU driver thread and the display
/// thread are both runnable at the same time, and a tracer thread on every core made the
/// worker's identical split lose to GPU-only (0.58x, `render_core/hybrid.rs`). Rows are
/// claimed from one atomic counter and each pixel depends only on its index, so the
/// output is bit-identical for any thread count.
pub(super) fn render_frame_scanlines_with_threads_cancellable(
    frame: &BackendFrame<'_>,
    spp: u32,
    current_sample_count: u32,
    outputs: &mut FrameOutputs<'_>,
    num_threads: usize,
    cancel: &AtomicBool,
) -> bool {
    if cancel.load(Ordering::Relaxed) {
        return false;
    }
    let width = frame.width;
    let height = frame.height;
    if width == 0 || height == 0 {
        return true;
    }
    let _lease = pleochroma::renderer::tonemap::live_cpu_budget::single_core_lease();
    let _trace_lease = pleochroma::renderer::tonemap::live_cpu_budget::trace_lease();
    let num_threads = num_threads
        .min(pleochroma::renderer::tonemap::live_cpu_budget::partition().0)
        .max(1);
    let width_usize = width as usize;

    // Reborrowed as four disjoint fields so the simultaneous `zip` below borrows each
    // independently.
    let FrameOutputs {
        accum,
        depth,
        normal,
        facet_id,
    } = outputs;
    let rows: Vec<Option<RowSlices<'_>>> = accum
        .chunks_mut(width_usize)
        .zip(depth.chunks_mut(width_usize))
        .zip(normal.chunks_mut(width_usize))
        .zip(facet_id.chunks_mut(width_usize))
        .map(|(((acc, depth), normal), facet)| {
            Some(RowSlices {
                acc,
                depth,
                normal,
                facet,
            })
        })
        .collect();
    let rows = Mutex::new(rows);
    let next_row = AtomicUsize::new(0);

    // Built ONCE per call rather than once per traced sample: the non-SoA
    // `trace_spectral_ray_with_finish` rebuilds this SIMD arena from `frame.planes` on
    // every invocation -- a heap allocation per (pixel, sample) tuple. `build_plane_soa`/
    // `trace_spectral_ray_with_finish_soa` are the caller-builds-the-arena-once pair
    // documented on `trace_spectral_ray_with_finish_soa` for exactly this case; results
    // stay bit-identical since the arena's contents depend only on `frame.planes`, which
    // is fixed for the whole frame.
    let plane_soa = build_plane_soa(frame.planes);
    // A ground facet surface is also a frosted facet to the readers of the finish list; merged
    // once per frame (borrowed untouched for every stone without one).
    let merged_finishes = merged_facet_finishes(frame.facet_finishes, frame.surface.facet_surfaces);
    // The zoned material's kernels, built once per frame and shared by every traced sample.
    let prepared = PreparedMaterial::new(frame.material);

    let job = ScanlineJob {
        frame,
        spp,
        current_sample_count,
        planes: &plane_soa,
        finishes: &merged_finishes,
        prepared: &prepared,
        cancel,
    };
    thread::scope(|scope| {
        for _ in 0..num_threads {
            let (rows, next_row, job) = (&rows, &next_row, &job);
            scope.spawn(move || {
                loop {
                    let y = next_row.fetch_add(1, Ordering::Relaxed);
                    if y >= height as usize || cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let row = rows
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)[y]
                        .take()
                        .expect("each row is claimed once by the atomic counter");
                    if !trace_row(job, y, row) {
                        break;
                    }
                }
            });
        }
    });
    !cancel.load(Ordering::Relaxed)
}

struct ScanlineJob<'a, 'scene> {
    frame: &'a BackendFrame<'scene>,
    spp: u32,
    current_sample_count: u32,
    planes: &'a pleochroma::simd::PlanesSoA32,
    finishes: &'a [FacetFinish],
    prepared: &'a PreparedMaterial<'a>,
    cancel: &'a AtomicBool,
}

fn trace_row(job: &ScanlineJob<'_, '_>, y: usize, row: RowSlices<'_>) -> bool {
    let ScanlineJob {
        frame,
        spp,
        current_sample_count,
        planes: plane_soa,
        finishes,
        prepared,
        cancel,
    } = *job;
    let width = frame.width;
    let height = frame.height;
    let width_usize = width as usize;
    let RowSlices {
        acc: acc_row,
        depth: depth_row,
        normal: normal_row,
        facet: facet_row,
    } = row;
    for x in 0..width_usize {
        let global_pixel_idx =
            u32::try_from(y * width_usize + x).expect("pixel index of a frame fits u32");
        let mut sample_sum = Vec3::ZERO;
        let mut primary_hit: Option<HitRecord> = None;

        // Per-pixel Cranley-Patterson rotations for the stratified
        // pixel-jitter/hero-wavelength draws below -- pure functions of
        // `global_pixel_idx` alone. Must stay in sync with
        // `pleochroma-worker/src/render_core.rs::trace_into`, which
        // computes through the same shared functions.
        let rot = pixel_rotations(global_pixel_idx);

        for s_idx in 0..spp {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            let sample_number = current_sample_count - spp + s_idx;
            let draws = sample_draws(global_pixel_idx, sample_number, &rot);

            let ray = frame.camera.generate_ray(
                x as f32,
                y as f32,
                width as f32,
                height as f32,
                draws.jitter_x,
                draws.jitter_y,
            );

            // Frosted girdle: `facet_finishes` is `&[]` when
            // `RenderContext::girdle_frosted` is off, equivalent to
            // `trace_spectral_ray` (every facet looks up
            // `FacetFinish::default() == Polished`).
            let sample_xyz = trace_spectral_ray_prepared(
                ray,
                StoneGeometry {
                    planes: frame.planes,
                    tools: frame.tools,
                },
                plane_soa,
                finishes,
                prepared,
                frame.fluorescence,
                frame.max_bounces,
                frame.environment,
                draws.seed,
                draws.hero_rand,
                Some(&mut primary_hit),
                frame.surface,
            );

            // Defensive: no reachable NaN/Inf producer is
            // known in the CPU path today, but this accumulator adds
            // unconditionally into a buffer that never resets except on
            // a full scene change -- one non-finite sample would poison
            // every pixel's running average for the rest of the session
            // rather than just corrupting the one frame it came from.
            if sample_xyz.is_finite() {
                sample_sum += sample_xyz;
            }
        }

        acc_row[x] += sample_sum;
        // `primary_hit` is captured from the LAST sample traced this
        // call (the hero channel's own first hit). AA jitter means
        // consecutive samples can land on different facets at a
        // silhouette edge; the denoiser's facet-identity guide term is
        // a hard Kronecker delta, so a stale-by-one-sample id there
        // costs at most a conservative edge-stop, never a wrong blend.
        depth_row[x] = primary_hit.map_or(1.0e6, |h| h.t);
        normal_row[x] = primary_hit.map_or(Vec3::ZERO, |h| h.normal);
        facet_row[x] = primary_hit.map_or(-1, |h| {
            i32::try_from(h.facet_idx).expect("facet index fits i32")
        });
    }
    true
}

/// Estimates CPU sample throughput from at most 64 spatially distributed rays.
///
/// The probe has no image contribution: its deterministic samples are measured, then
/// discarded. Unlike the former full-image calibration, it does not hold up a camera
/// update for a complete CPU pass. Two milliseconds is a soft limit checked after each
/// ray; a ray already inside the optical kernel must finish before cancellation.
pub(super) fn measure_cpu_rate(frame: &BackendFrame<'_>, cancel: &AtomicBool) -> Option<f64> {
    if cancel.load(Ordering::Relaxed) || frame.width == 0 || frame.height == 0 {
        return None;
    }
    let planes = build_plane_soa(frame.planes);
    let finishes = merged_facet_finishes(frame.facet_finishes, frame.surface.facet_surfaces);
    // Prepared before the clock starts: the kernels are a once-per-frame cost, not a sample cost.
    let prepared = PreparedMaterial::new(frame.material);
    let started = std::time::Instant::now();
    let mut completed = 0u32;
    for sample in 0..64u32 {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        // A permutation of an 8x8 grid, beginning near the gem's centre.
        let cell = (sample * 37 + 27) % 64;
        let x = ((cell % 8 * 2 + 1) * frame.width / 16).min(frame.width.saturating_sub(1));
        let y = ((cell / 8 * 2 + 1) * frame.height / 16).min(frame.height.saturating_sub(1));
        let pixel = y * frame.width + x;
        let draws = sample_draws(pixel, frame.sample_offset, &pixel_rotations(pixel));
        let ray = frame.camera.generate_ray(
            x as f32,
            y as f32,
            frame.width as f32,
            frame.height as f32,
            draws.jitter_x,
            draws.jitter_y,
        );
        let value = trace_spectral_ray_prepared(
            ray,
            frame.stone(),
            &planes,
            &finishes,
            &prepared,
            frame.fluorescence,
            frame.max_bounces,
            frame.environment,
            draws.seed,
            draws.hero_rand,
            None,
            frame.surface,
        );
        let _ = std::hint::black_box(value);
        completed += 1;
        if started.elapsed() >= std::time::Duration::from_millis(2) {
            break;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let workers = pleochroma::renderer::tonemap::live_cpu_budget::partition().0;
    let workers = f64::from(u32::try_from(workers).unwrap_or(u32::MAX));
    // Conservative scaling: memory traffic and thread coordination make a pool less
    // than perfectly linear. Actual hybrid frames subsequently replace this estimate.
    let pixels = f64::from(frame.width) * f64::from(frame.height);
    Some(
        f64::from(completed) * workers * 0.5 / (started.elapsed().as_secs_f64().max(1e-9) * pixels),
    )
}
