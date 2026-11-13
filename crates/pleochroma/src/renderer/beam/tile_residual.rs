//! The per-batch CPU residual (beam guide §12.3, §12.4).
//!
//! It is one adaptive loop over a batch's tiles with the reference tracer, the batch's kept index
//! as the `kept` callback, `residual_min_spp` and the beam part as the noise metric's offset.
//!
//! The beam engines in `beam::engines` call [`run_batch_residual_with`].
//!
//! # Layout and determinism
//!
//! Every per-pixel slice (`offset`, [`BatchResidual`]'s vectors) is in
//! [`BATCH_PIXEL_ORDER`](super::engines::BATCH_PIXEL_ORDER): the batch's tiles in order, each tile
//! row-major. The adaptive loop is an [`AdaptiveRun`] over the whole frame's tile grid restricted
//! to the batch's tiles ([`AdaptiveRun::restricted_to`]); its noise check measures each tile on the
//! tile's own slice of the batch buffers, in the pixel order the full-frame metric uses, so the
//! decisions equal the ones the full-frame buffers would give. A pixel's samples are a pure
//! function of `(pixel, sample number)` and are summed in ascending sample order, so the image is
//! bit-identical for any thread count.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use glam::{DVec2, Vec3};

use super::{
    engines::{Batch, BatchResidual},
    primary::TileRect,
    residual::KeptIndex,
};
use crate::{
    geometry::tool::{StoneGeometry, ToolPrimitive},
    optics::{
        fluorescence::Fluorescence,
        raytracer::{
            FacetFinish, PixelRotations, add_finite_sample, pixel_rotations, sample_draws,
            surface::{SurfaceScene, merged_facet_finishes},
            transport::{
                PreparedMaterial,
                reference::{KeptFn, trace_spectral_ray_reference_prepared},
            },
        },
    },
    renderer::{
        adaptive::{
            AdaptiveRun, AdaptiveSettings, LiveTiles, Pass,
            metric::{tile_measure_with_offset, tile_noise},
        },
        frame_scene::FrameScene,
    },
    simd::PlanesSoA32,
};

/// The scene of a residual run, fixed for the frame.
#[derive(Clone, Copy)]
pub struct ResidualScene<'a> {
    /// The frame.
    pub frame: &'a FrameScene<'a>,
    /// Tools, fluorescence and the surface scene, as `cpu_frame`'s entries take them.
    pub extras: (&'a [ToolPrimitive], &'a Fluorescence, SurfaceScene<'a>),
    /// The intersection arena of `frame.planes`.
    pub plane_soa: &'a PlanesSoA32,
    /// The prepared material of `frame.material`.
    pub prepared: &'a PreparedMaterial<'a>,
    /// The spectral node count `N_λ` (the kept test's bin rule).
    pub n_lambda: u32,
    /// The absolute index of the render's first sample.
    pub sample_offset: u32,
    /// The minimum residual samples per pixel (`BeamSettings::residual_min_spp`).
    pub residual_min_spp: u32,
}

/// The display multiplier of the noise check when the caller gives none (the means as they are).
pub const DEFAULT_METRIC_SCALE: f32 = 1.0;

/// The CPU residual of `batch`: the adaptive loop restricted to the batch's tiles.
///
/// It uses `AdaptiveRun::restricted_to`, the reference tracer with the batch's kept index, and the
/// beam part `offset` (one entry per batch pixel, `engines::BATCH_PIXEL_ORDER`) inside the metric.
/// Polls `cancel` per pass; `None` when cancelled.
///
/// The batch's tiles must be tiles of the adaptive grid (`adaptive.tile` pixels, row-major over
/// the frame), as the pipeline cuts them, and `adaptive` must be valid
/// ([`AdaptiveSettings::validate`], checked by the caller); otherwise the result is `None` too. An
/// empty `offset` means no beam part. The minimum samples are
/// [`ResidualScene::residual_min_spp`] (at most `adaptive.max_spp`) and also the pass size
/// ([`residual_settings`]), the other settings are `adaptive`'s. It runs on all available cores and
/// judges the noise at the display multiplier [`DEFAULT_METRIC_SCALE`]; see
/// [`run_batch_residual_with`].
#[must_use]
pub fn run_batch_residual(
    scene: &ResidualScene<'_>,
    batch: &Batch<'_>,
    offset: &[Vec3],
    adaptive: &AdaptiveSettings,
    cancel: &AtomicBool,
) -> Option<BatchResidual> {
    let threads = super::pipeline::default_threads();
    run_batch_residual_with(
        scene,
        batch,
        offset,
        adaptive,
        (threads, DEFAULT_METRIC_SCALE),
        cancel,
        &mut |_| {},
    )
}

/// [`run_batch_residual`] on `threads` worker threads and with the noise check at the display
/// multiplier `scale` (as in [`AdaptiveRun::finish_pass`]).
///
/// The result is bit-identical for any `threads`: the tiles of a pass are traced independently and
/// merged in batch order. After every pass `progress` receives the share of the batch's sample
/// budget traced so far ([`residual_share`], 0 to 1, never decreasing).
#[must_use]
pub fn run_batch_residual_with(
    scene: &ResidualScene<'_>,
    batch: &Batch<'_>,
    offset: &[Vec3],
    adaptive: &AdaptiveSettings,
    (threads, scale): (usize, f32),
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Option<BatchResidual> {
    let frame = scene.frame;
    let size = (frame.width, frame.height);
    let settings = residual_settings(adaptive, scene.residual_min_spp);
    let slots = batch_slots(batch, size, settings.tile)?;
    let members = LiveTiles::from_indices(
        size.0,
        size.1,
        settings.tile,
        slots.iter().map(|slot| slot.frame_index).collect(),
    );
    let mut run = AdaptiveRun::new_restricted(settings, size.0, size.1, &members)
        .ok()?
        .with_sample_offset(scene.sample_offset);
    let (_, _, surface) = scene.extras;
    let finishes = merged_facet_finishes(frame.facet_finishes, surface.facet_surfaces);
    let pixels: usize = slots.iter().map(|slot| slot.rect.pixel_count()).sum();
    let mut buffers = Buffers {
        sum: vec![Vec3::ZERO; pixels],
        half_b: vec![Vec3::ZERO; pixels],
    };
    let mut order: Vec<(u32, usize)> = slots
        .iter()
        .enumerate()
        .map(|(position, slot)| (slot.frame_index, position))
        .collect();
    order.sort_unstable();
    let offset = (!offset.is_empty()).then_some(offset);
    while !cancel.load(Ordering::Relaxed) {
        let Some(pass) = run.next_pass() else {
            return Some(finish(&run, &slots, buffers));
        };
        let ctx = PassContext {
            scene,
            finishes: &finishes,
            first_sample: pass.first_sample,
            spp: pass.spp,
            cancel,
        };
        trace_pass(&ctx, &slots, &pass, threads, &mut buffers)?;
        close_pass(&mut run, (&slots, &order), &buffers, offset, scale);
        progress(residual_share(
            run.samples_traced(),
            pixels,
            settings.max_spp,
        ));
    }
    None
}

/// `adaptive` as the residual of a beam batch runs it (beam guide section 12.4).
///
/// The minimum is `residual_min_spp` clamped into `1..=max_spp`, and the pass is that minimum: the
/// first convergence check is due at the minimum anyway, and a GPU pass is one round trip (upload,
/// dispatch, readback, host check), so short passes would multiply those trips. `adaptive.pass_spp`
/// is not read.
/// The result satisfies [`AdaptiveSettings::validate`]'s sample-count rules whenever `adaptive` is
/// otherwise valid.
#[must_use]
pub fn residual_settings(adaptive: &AdaptiveSettings, residual_min_spp: u32) -> AdaptiveSettings {
    let max_spp = adaptive.max_spp.max(1);
    let min_spp = residual_min_spp.clamp(1, max_spp);
    AdaptiveSettings {
        min_spp,
        max_spp,
        pass_spp: min_spp,
        ..*adaptive
    }
}

/// The share of a batch's residual budget traced so far: `samples` over `pixels * max_spp`,
/// from 0 to 1.
///
/// A pure function of the sample count, so it is deterministic and never decreases within a batch.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "a progress share needs three digits, not exact sample counts"
)]
pub fn residual_share(samples: u64, pixels: usize, max_spp: u32) -> f32 {
    let budget = pixels as f64 * f64::from(max_spp.max(1));
    if budget <= 0.0 {
        return 0.0;
    }
    ((samples as f64 / budget) as f32).clamp(0.0, 1.0)
}

/// One tile of the batch: where it is in the frame grid and in the batch buffers.
struct Slot<'b> {
    /// The tile's pixels.
    rect: TileRect,
    /// The tile's index in the row-major grid of adaptive tiles.
    frame_index: u32,
    /// The first entry of the tile in the batch buffers.
    base: usize,
    /// The tile's kept index; `None` when it keeps nothing (the plain reference traces it).
    kept: Option<&'b KeptIndex>,
}

impl Slot<'_> {
    /// The tile's range in the batch buffers.
    const fn range(&self) -> std::ops::Range<usize> {
        self.base..self.base + self.rect.pixel_count()
    }
}

/// The batch's tiles in batch order, or `None` when one is not a tile of the adaptive grid.
fn batch_slots<'b>(batch: &Batch<'b>, size: (u32, u32), tile: u32) -> Option<Vec<Slot<'b>>> {
    let tile = tile.max(1);
    let columns = size.0.div_ceil(tile);
    let mut base = 0_usize;
    let mut slots = Vec::with_capacity(batch.tiles.len());
    for (position, &rect) in batch.tiles.iter().enumerate() {
        let frame_index = (rect.y0 / tile) * columns + rect.x0 / tile;
        if TileRect::of_index(size, tile, frame_index) != Some(rect) {
            return None;
        }
        let kept = batch
            .kept
            .get(position)
            .filter(|(kept_rect, _)| *kept_rect == rect)
            .or_else(|| batch.kept.iter().find(|(kept_rect, _)| *kept_rect == rect))
            .map(|(_, index)| index)
            .filter(|index| !index.is_empty());
        slots.push(Slot {
            rect,
            frame_index,
            base,
            kept,
        });
        base += rect.pixel_count();
    }
    Some(slots)
}

/// The running sums of a batch, in batch pixel order.
struct Buffers {
    /// Every sample.
    sum: Vec<Vec3>,
    /// The samples with an odd absolute index.
    half_b: Vec<Vec3>,
}

/// What the tracing threads read for one pass.
struct PassContext<'s, 'a> {
    /// The scene.
    scene: &'s ResidualScene<'a>,
    /// The facet finishes merged with the surface list.
    finishes: &'s [FacetFinish],
    /// The absolute index of the pass's first sample.
    first_sample: u32,
    /// The samples per pixel of the pass.
    spp: u32,
    /// The cancel token.
    cancel: &'s AtomicBool,
}

/// One tile's pass totals: per pixel the sum of the pass and of its odd-index samples.
struct TileSums {
    /// Pass sums, row-major in the tile.
    sum: Vec<Vec3>,
    /// Odd-index pass sums.
    odd: Vec<Vec3>,
}

/// One residual sample of pixel `(x, y)`: the camera ray with the pixel's jitter and the hero
/// wavelength of `sample_num` (the draws `cpu_frame` uses), traced by the reference tracer with
/// the tile's kept index bound to the image point `(x + jx, y + jy)`.
fn residual_sample(
    ctx: &PassContext<'_, '_>,
    kept: Option<&KeptIndex>,
    (x, y): (u32, u32),
    rot: &PixelRotations,
    sample_num: u32,
) -> Vec3 {
    let scene = ctx.scene;
    let frame = scene.frame;
    let draws = sample_draws(y * frame.width + x, sample_num, rot);
    let ray = frame.camera.generate_ray(
        x as f32,
        y as f32,
        frame.width as f32,
        frame.height as f32,
        draws.jitter_x,
        draws.jitter_y,
    );
    let (tools, fluorescence, surface) = scene.extras;
    let trace = |test: Option<&KeptFn<'_>>| {
        trace_spectral_ray_reference_prepared(
            ray,
            StoneGeometry {
                planes: frame.planes,
                tools,
            },
            scene.plane_soa,
            ctx.finishes,
            scene.prepared,
            fluorescence,
            frame.max_bounces,
            frame.environment,
            draws.seed,
            draws.hero_rand,
            None,
            surface,
            test,
        )
        .xyz
    };
    kept.map_or_else(
        || trace(None),
        |index| {
            let point = DVec2::new(
                f64::from(x) + f64::from(draws.jitter_x),
                f64::from(y) + f64::from(draws.jitter_y),
            );
            let n_lambda = scene.n_lambda;
            let test = move |sig: u64, lambda: f32| index.kept(point, sig, lambda, n_lambda);
            trace(Some(&test as &KeptFn<'_>))
        },
    )
}

/// The pass totals of every pixel of `slot`; `None` when the cancel token was seen set.
fn trace_tile(ctx: &PassContext<'_, '_>, slot: &Slot<'_>) -> Option<TileSums> {
    let rect = slot.rect;
    let width = ctx.scene.frame.width;
    let mut tile = TileSums {
        sum: Vec::with_capacity(rect.pixel_count()),
        odd: Vec::with_capacity(rect.pixel_count()),
    };
    for y in rect.y0..rect.y1 {
        if ctx.cancel.load(Ordering::Relaxed) {
            return None;
        }
        for x in rect.x0..rect.x1 {
            let rot = pixel_rotations(y * width + x);
            let mut pass_sum = Vec3::ZERO;
            let mut pass_odd = Vec3::ZERO;
            for local_sample in 0..ctx.spp {
                // One relaxed load per reference sample (a sample costs far more), so a pass of
                // many samples per pixel stops within one sample of the token, not one row.
                if ctx.cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let sample_num = ctx.first_sample.wrapping_add(local_sample);
                let sample = residual_sample(ctx, slot.kept, (x, y), &rot, sample_num);
                add_finite_sample(&mut pass_sum, sample);
                if !sample_num.is_multiple_of(2) {
                    add_finite_sample(&mut pass_odd, sample);
                }
            }
            tile.sum.push(pass_sum);
            tile.odd.push(pass_odd);
        }
    }
    Some(tile)
}

/// Runs `work` for the indices `0..count` on `threads` workers taking indices from one counter and
/// returns the results in index order; `None` when `cancel` was set or a job gave nothing.
fn run_jobs<T: Send>(
    count: usize,
    threads: usize,
    cancel: &AtomicBool,
    work: impl Fn(usize) -> Option<T> + Sync,
) -> Option<Vec<T>> {
    let threads = threads.clamp(1, count.max(1));
    let mut results: Vec<Option<T>> = Vec::new();
    results.resize_with(count, || None);
    if threads == 1 {
        for (index, slot) in results.iter_mut().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            *slot = work(index);
        }
    } else {
        let next = AtomicUsize::new(0);
        let done: Vec<Vec<(usize, Option<T>)>> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..threads)
                .map(|_| {
                    scope.spawn(|| {
                        let mut mine = Vec::new();
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            if index >= count || cancel.load(Ordering::Relaxed) {
                                break;
                            }
                            mine.push((index, work(index)));
                        }
                        mine
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| match worker.join() {
                    Ok(mine) => mine,
                    Err(payload) => std::panic::resume_unwind(payload),
                })
                .collect()
        });
        for (index, result) in done.into_iter().flatten() {
            if let Some(slot) = results.get_mut(index) {
                *slot = result;
            }
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    results.into_iter().collect()
}

/// Traces `pass` for the batch's live tiles and adds the totals to `buffers` in batch order;
/// `None` when cancelled.
fn trace_pass(
    ctx: &PassContext<'_, '_>,
    slots: &[Slot<'_>],
    pass: &Pass,
    threads: usize,
    buffers: &mut Buffers,
) -> Option<()> {
    let jobs: Vec<usize> = slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| pass.live.indices().binary_search(&slot.frame_index).is_ok())
        .map(|(position, _)| position)
        .collect();
    let tiles = run_jobs(jobs.len(), threads, ctx.cancel, |job| {
        jobs.get(job)
            .and_then(|&position| slots.get(position))
            .and_then(|slot| trace_tile(ctx, slot))
    })?;
    for (&position, tile) in jobs.iter().zip(&tiles) {
        let range = slots.get(position)?.range();
        let sum = buffers.sum.get_mut(range.clone())?;
        let half_b = buffers.half_b.get_mut(range)?;
        for (dst, add) in sum.iter_mut().zip(&tile.sum) {
            *dst += *add;
        }
        for (dst, add) in half_b.iter_mut().zip(&tile.odd) {
            *dst += *add;
        }
    }
    Some(())
}

/// Closes the pending pass of `run`: measures the live tiles (the beam part `offset` inside the
/// metric, laid out like the batch buffers) when a check is due and reports the noises.
fn close_pass(
    run: &mut AdaptiveRun,
    (slots, order): (&[Slot<'_>], &[(u32, usize)]),
    buffers: &Buffers,
    offset: Option<&[Vec3]>,
    scale: f32,
) {
    let Some(check) = run.pending_check() else {
        run.finish_pass_with_tile_errors(&[]);
        return;
    };
    let metric = run.settings().metric;
    let counts = (check.samples, check.odd_samples);
    let noises: Vec<f32> = check
        .live
        .indices()
        .iter()
        .map(|&frame_index| {
            let slot = order
                .binary_search_by_key(&frame_index, |&(index, _)| index)
                .ok()
                .and_then(|found| order.get(found))
                .and_then(|&(_, position)| slots.get(position));
            let Some(slot) = slot else {
                return f32::INFINITY;
            };
            let range = slot.range();
            let (width, height) = (slot.rect.width(), slot.rect.height());
            let measure = tile_measure_with_offset(
                buffers.sum.get(range.clone()).unwrap_or_default(),
                buffers.half_b.get(range.clone()).unwrap_or_default(),
                offset.map(|values| values.get(range).unwrap_or_default()),
                width,
                (0, 0, width, height),
                counts,
                scale,
            );
            tile_noise(&measure, metric)
        })
        .collect();
    run.finish_pass_with_tile_errors(&noises);
}

/// The result of a finished run: the buffers, the samples each pixel received and the total.
fn finish(run: &AdaptiveRun, slots: &[Slot<'_>], buffers: Buffers) -> BatchResidual {
    let mut counts = Vec::with_capacity(buffers.sum.len());
    for slot in slots {
        let spp = run.pixel_spp(slot.rect.x0, slot.rect.y0);
        counts.extend(std::iter::repeat_n(spp, slot.rect.pixel_count()));
    }
    BatchResidual {
        sum: buffers.sum,
        half_b: buffers.half_b,
        counts,
        samples_traced: run.samples_traced(),
    }
}
