//! Deterministic perspective sampling using exactly the renderer camera convention.
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::{
    classify::{
        ApertureSampleContext, RayClassification, classify_ray, classify_ray_pair_detailed,
    },
    image_scint::TemporalSampler,
    image_types::{
        MAX_DIAGNOSTIC_PIXELS, OpticalImage, OpticalImageError, OpticalImageRequest, OpticalPixel,
        RayDetail,
    },
    pose_batch::resolve_threads,
    prepared::PreparedEval,
};
use crate::optics::raytracer::Camera;

/// Centre first, then a fixed low-discrepancy prefix, independent of scheduling.
///
/// The digit sum stays a plain product added to `y`, never a fused multiply-add: that would
/// re-round it and could move the jitter offsets the optical-image goldens were recorded with.
/// The product is its own binding so no clippy version suggests fusing it.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a sub-pixel offset in [0, 1) needs only f32 precision"
)]
pub(super) fn offset(sample: u32) -> (f32, f32) {
    if sample == 0 {
        return (0.5, 0.5);
    }
    let radical = f64::from(sample.reverse_bits()) / 4_294_967_296.0;
    let mut n = sample;
    let mut scale = 1.0 / 3.0;
    let mut y = 0.0;
    while n > 0 {
        let digit = f64::from(n % 3) * scale;
        y += digit;
        n /= 3;
        scale /= 3.0;
    }
    (radical as f32, y as f32)
}

/// Rejects requests the sampler cannot honour, returning the number of pixels to allocate.
fn validate_request(r: &OpticalImageRequest<'_>) -> Result<usize, OpticalImageError> {
    let count = (r.width as usize)
        .checked_mul(r.height as usize)
        .ok_or(OpticalImageError::ResourceLimit)?;
    if count == 0 || count > MAX_DIAGNOSTIC_PIXELS || r.samples == 0 || r.samples > 64 {
        return Err(OpticalImageError::ResourceLimit);
    }
    let projection = r.projection_size.unwrap_or([r.width, r.height]);
    // The offset cameras are rebuilt from the orbit pose, so scintillation cannot run
    // without one (and a degenerate pose cannot build a camera).
    if r.want_scintillation
        && !r
            .pose
            .is_some_and(|pose| pose.iter().all(|v| v.is_finite()) && pose[2] > 0.0)
    {
        return Err(OpticalImageError::InvalidInput);
    }
    if projection.contains(&0)
        || r.geometry.planes.iter().any(|p| {
            !p.d.is_finite()
                || p.normal.iter().any(|n| !n.is_finite())
                || (p.normal.iter().map(|n| n * n).sum::<f32>() - 1.0).abs() > 0.01
        })
        || r.geometry.tools.iter().any(|t| t.validate().is_err())
    {
        return Err(OpticalImageError::InvalidInput);
    }
    if let Some([left, top]) = r.tile_origin
        && (left.checked_add(r.width).is_none_or(|v| v > projection[0])
            || top.checked_add(r.height).is_none_or(|v| v > projection[1]))
    {
        return Err(OpticalImageError::InvalidInput);
    }
    let c = r.camera;
    if ![c.origin, c.forward, c.right, c.up]
        .iter()
        .all(|v| v.is_finite())
        || c.forward.dot(c.right).abs() > 0.001
        || c.forward.dot(c.up).abs() > 0.001
        || c.right.dot(c.up).abs() > 0.001
        || !c.fov_tan.is_finite()
        || c.fov_tan <= 0.0
        || [c.forward, c.right, c.up]
            .iter()
            .any(|v| (v.length_squared() - 1.0).abs() > 0.01)
        || [486.1, 589.3, 656.3].iter().any(|w| {
            let n = r.material.dispersion.evaluate(*w);
            !n.is_finite() || n < 1.0
        })
    {
        return Err(OpticalImageError::InvalidInput);
    }
    Ok(count)
}

/// Everything [`PixelSampler::sample_pixel`] reads that is the same for every pixel of one
/// evaluation.
///
/// Holds only plain data and shared references (not the whole request), so one sampler can be
/// read from every worker thread of [`evaluate_optical_image_threaded`].
struct PixelSampler<'a> {
    camera: &'a Camera,
    /// Samples per pixel.
    samples: u32,
    want_fire: bool,
    ctx: &'a ApertureSampleContext<'a>,
    /// Beauty dimensions the camera rays are generated against.
    projection: [u32; 2],
    /// Projection pixel of this image's top-left corner (the tile origin, or zero).
    origin: [u32; 2],
    /// Projection pixels per image pixel: 1 for a tile, the draft upscale otherwise.
    scale: [f32; 2],
    zero_dispersion: bool,
}

impl PixelSampler<'_> {
    /// Classifies every sample of pixel `(x, y)`; `Err` on cancellation.
    fn sample_pixel(
        &self,
        x: u32,
        y: u32,
        cancelled: &impl Fn() -> bool,
    ) -> Result<OpticalPixel, OpticalImageError> {
        let mut pixel = OpticalPixel::default();
        let mut first_facet = None;
        for sample in 0..self.samples {
            if cancelled() {
                return Err(OpticalImageError::Cancelled);
            }
            let (jx, jy) = offset(sample);
            // The renderer centres beauty pixel `x` at screen coordinate `x` (the guide pass
            // and the un-jittered sample generate their rays there), so this pixel's
            // `[x, x + 1)` extent, in edge coordinates, is shifted back by half a pixel.
            let ray = self.camera.generate_ray(
                (x as f32 + jx).mul_add(self.scale[0], self.origin[0] as f32 - 0.5),
                (y as f32 + jy).mul_add(self.scale[1], self.origin[1] as f32 - 0.5),
                self.projection[0] as f32,
                self.projection[1] as f32,
                0.0,
                0.0,
            );
            let mut detail = RayDetail::default();
            // The beauty render centres the head-shadow cone on this pixel's own eye
            // direction, not on the camera axis.
            let observer = -ray.dir;
            // An anisotropic stone traces both eigenmodes of the camera ray (ordinary first),
            // each classified like an isotropic ray and counted once; an isotropic stone's ray
            // is traced once, exactly as before the modes existed.
            let pair;
            let single;
            let traces: &[RayClassification] = if self.ctx.stone.modes.is_some() {
                let Some(modes) =
                    classify_ray_pair_detailed(self.ctx, ray, observer, Some(&mut detail))
                else {
                    continue;
                };
                pair = modes;
                pixel.modes = 2;
                &pair
            } else {
                let Some(class) = classify_ray(self.ctx, ray, observer, Some(&mut detail)) else {
                    continue;
                };
                single = [class];
                &single
            };
            pixel.hits += 1;
            if sample == 0 {
                pixel.centre = detail;
            }
            if let Some(id) = first_facet {
                pixel.mixed_facets |= detail.entry_facet != Some(id);
            } else {
                first_facet = detail.entry_facet;
            }
            for class in traces {
                let bucket = match class {
                    RayClassification::Windowed => 0,
                    RayClassification::Extinct => 1,
                    RayClassification::Returned(returned) => {
                        if self.want_fire {
                            if let Some((angle, weight)) = returned.fire {
                                if angle.is_finite() && weight.is_finite() {
                                    pixel.fire_sum_deg =
                                        (if self.zero_dispersion { 0.0 } else { angle })
                                            .mul_add(weight, pixel.fire_sum_deg);
                                    pixel.fire_qualified += 1;
                                } else {
                                    pixel.fire_rejected += 1;
                                }
                            } else {
                                pixel.fire_rejected += 1;
                            }
                        }
                        2
                    }
                    RayClassification::EntryBlocked | RayClassification::Invalid => 3,
                };
                pixel.classes[bucket] += 1;
            }
        }
        Ok(pixel)
    }
}

/// The samplers of one evaluation, able to trace any row of the image on its own.
///
/// Every pixel is a pure function of `(x, y)` and these read-only fields, so rows may be traced
/// in any order and on any thread; nothing is shared between rows.
struct RowTracer<'a> {
    sampler: PixelSampler<'a>,
    /// The scintillation temporal term, when requested.
    temporal: Option<TemporalSampler<'a>>,
    width: u32,
}

impl RowTracer<'_> {
    /// Appends the pixels of row `y`, left to right, to `out`; `Err` on cancellation.
    fn trace_row(
        &self,
        y: u32,
        cancelled: &impl Fn() -> bool,
        out: &mut Vec<OpticalPixel>,
    ) -> Result<(), OpticalImageError> {
        for x in 0..self.width {
            let mut pixel = self.sampler.sample_pixel(x, y, cancelled)?;
            if pixel.hits > 0
                && let Some(temporal) = &self.temporal
            {
                temporal.apply(x, y, &mut pixel, cancelled)?;
            }
            out.push(pixel);
        }
        Ok(())
    }
}

/// Validates `r`, prepares the stone, lets `fill` push every pixel of the image in row-major
/// order, then sums the image total over the finished pixels in that same order.
///
/// The total is folded here, serially, whoever traced the pixels: its float Fire sum is
/// therefore added pixel by pixel in the fixed row-major order whatever the thread count.
fn evaluate_with(
    r: &OpticalImageRequest<'_>,
    cancelled: &impl Fn() -> bool,
    fill: impl FnOnce(&RowTracer<'_>, &mut Vec<OpticalPixel>) -> Result<(), OpticalImageError>,
) -> Result<OpticalImage, OpticalImageError> {
    let count = validate_request(r)?;
    let projection = r.projection_size.unwrap_or([r.width, r.height]);
    let c = r.camera;
    if cancelled() {
        return Err(OpticalImageError::Cancelled);
    }
    let prepared = PreparedEval::new_finished(r.geometry, r.finishes, r.material, r.environment)
        .ok_or(OpticalImageError::EmptyGeometry)?;
    let ctx = ApertureSampleContext {
        stone: prepared.stone(),
        nd: prepared.nd,
        n_f: prepared.n_f,
        n_c: prepared.n_c,
        cam_forward: c.forward,
        cam_right: c.right,
        cam_up: c.up,
        fan: prepared.fan,
        lighting: &prepared.lighting,
        want_fire: r.want_fire,
    };
    let mut image = OpticalImage {
        width: r.width,
        height: r.height,
        samples: r.samples,
        fire_computed: r.want_fire,
        scint_computed: r.want_scintillation,
        origin: r.tile_origin.unwrap_or([0, 0]),
        pixels: Vec::with_capacity(count),
        total: OpticalPixel::default(),
    };
    let scale = if r.tile_origin.is_some() {
        [1.0, 1.0]
    } else {
        [
            projection[0] as f32 / r.width as f32,
            projection[1] as f32 / r.height as f32,
        ]
    };
    let tracer = RowTracer {
        sampler: PixelSampler {
            camera: c,
            samples: r.samples,
            want_fire: r.want_fire,
            ctx: &ctx,
            projection,
            origin: image.origin,
            scale,
            zero_dispersion: prepared.n_f.to_bits() == prepared.n_c.to_bits(),
        },
        // Validation guarantees a pose whenever scintillation is requested.
        temporal: r
            .pose
            .filter(|_| r.want_scintillation)
            .map(|pose| TemporalSampler::new(r, &prepared, pose, projection, image.origin, scale)),
        width: r.width,
    };
    fill(&tracer, &mut image.pixels)?;
    for pixel in &image.pixels {
        image.total.hits += pixel.hits;
        image.total.modes = image.total.modes.max(pixel.modes);
        for (sum, n) in image.total.classes.iter_mut().zip(pixel.classes) {
            *sum += n;
        }
        image.total.fire_sum_deg += pixel.fire_sum_deg;
        image.total.fire_qualified += pixel.fire_qualified;
        image.total.fire_rejected += pixel.fire_rejected;
    }
    Ok(image)
}

/// Computes all base classes together; optional Fire companions only for returned rays.
///
/// Cancellation is checked per pixel and per sample (each path is bounded to ten bounces).
/// The caller chooses a sampling resolution and must label it when exporting an upscale.
/// Runs on the calling thread; [`evaluate_optical_image_threaded`] spreads the same work
/// over a thread team and returns the identical image.
///
/// # Errors
///
/// Returns an [`OpticalImageError`] when the request is invalid or exceeds the resource
/// limits, when the geometry is empty, or when `cancelled` reports cancellation.
pub fn evaluate_optical_image(
    request: &OpticalImageRequest<'_>,
    cancelled: impl Fn() -> bool,
) -> Result<OpticalImage, OpticalImageError> {
    evaluate_with(request, &cancelled, |tracer, pixels| {
        for y in 0..request.height {
            tracer.trace_row(y, &cancelled, pixels)?;
        }
        Ok(())
    })
}

/// [`evaluate_optical_image`] on up to `threads` scoped worker threads, with a result that is
/// bit-identical to the serial one for every thread count.
///
/// Rows are handed out from a shared counter (a row's cost varies with how much of the stone
/// it crosses), but each finished row lands in the slot its index owns, so the pixel order, and
/// with it the image total (summed afterwards, serially, in row-major order, see
/// [`evaluate_with`]), does not depend on scheduling. `threads` is clamped to `1..=16` and to
/// the row count; `1` (and `wasm32`, which cannot spawn) runs the serial loop. The caller owns
/// the CPU budget: the live overlay passes the display share of the live partition.
///
/// Cancellation is checked per sample on every worker, exactly as in the serial pass, and the
/// first worker to see it stops the others at their next row.
///
/// # Errors
///
/// The errors of [`evaluate_optical_image`].
pub fn evaluate_optical_image_threaded(
    request: &OpticalImageRequest<'_>,
    threads: usize,
    cancelled: impl Fn() -> bool + Sync,
) -> Result<OpticalImage, OpticalImageError> {
    let threads = resolve_threads(Some(threads)).min(request.height.max(1) as usize);
    if threads == 1 {
        return evaluate_optical_image(request, cancelled);
    }
    evaluate_with(request, &cancelled, |tracer, pixels| {
        trace_rows_parallel(tracer, request.height, threads, &cancelled, pixels)
    })
}

/// Traces rows `0..height` on `threads` scoped workers and appends them to `pixels` in row
/// order. A worker claims the next unclaimed row until none is left or a worker has failed.
fn trace_rows_parallel(
    tracer: &RowTracer<'_>,
    height: u32,
    threads: usize,
    cancelled: &(impl Fn() -> bool + Sync),
    pixels: &mut Vec<OpticalPixel>,
) -> Result<(), OpticalImageError> {
    type Rows = Vec<(u32, Vec<OpticalPixel>)>;
    let next = AtomicU32::new(0);
    let failed = AtomicBool::new(false);
    let (next, failed) = (&next, &failed);
    let per_thread: Vec<Result<Rows, OpticalImageError>> = std::thread::scope(|scope| {
        #[expect(
            clippy::needless_collect,
            reason = "every thread must be spawned before the first join, or they run one by one"
        )]
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(move || {
                    let mut rows = Rows::new();
                    while !failed.load(Ordering::Relaxed) {
                        let y = next.fetch_add(1, Ordering::Relaxed);
                        if y >= height {
                            break;
                        }
                        let mut row = Vec::with_capacity(tracer.width as usize);
                        if let Err(error) = tracer.trace_row(y, cancelled, &mut row) {
                            failed.store(true, Ordering::Relaxed);
                            return Err(error);
                        }
                        rows.push((y, row));
                    }
                    Ok(rows)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    let mut rows = Rows::new();
    for result in per_thread {
        rows.extend(result?);
    }
    rows.sort_unstable_by_key(|&(y, _)| y);
    for (_, row) in rows {
        pixels.extend(row);
    }
    Ok(())
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;
