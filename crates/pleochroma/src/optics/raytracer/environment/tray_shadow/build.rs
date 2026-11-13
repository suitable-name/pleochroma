//! Deterministic native orchestration of blocked light and ordered photon deposits.
use super::{
    super::LightingPreset,
    TRAY_SHADOW_BAND_WIDTH_NM, TRAY_SHADOW_BANDS, TRAY_SHADOW_FIRST_CENTER_NM,
    TRAY_SHADOW_MODEL_VERSION, TrayShadowKey, TrayShadowMap, TrayShadowOff, TrayShadowQuality,
    gather::blocked_fraction,
    geometry::{StoneFrame, StoneHit},
    lights::LightTable,
    photons::{Deposit, PhotonCtx, trace_chunk},
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::materials::{GemMaterial, material_fingerprint},
    render_setup::hash_geometry,
    renderer::whitted::tint::{bin_weights, binned_absorption_for},
};
use glam::Vec3;
use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    sync::{
        Condvar, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

/// Inputs affecting the paper's spectral irradiance ratio; camera and exposure cancel.
pub struct TrayShadowInput<'a> {
    /// The facet planes and subtractive tools.
    pub geom: StoneGeometry<'a>,
    /// The stone's dispersion and absorption.
    pub material: &'a GemMaterial,
    /// Analytic incident lighting.
    pub preset: LightingPreset,
    /// Studio yaw in radians.
    pub yaw: f32,
    /// Studio pitch in radians.
    pub pitch: f32,
    /// Maximum optical interface depth.
    pub max_bounces: u32,
}

/// Computes the identity of precisely the physical precompute inputs.
#[must_use]
pub fn tray_shadow_key(input: &TrayShadowInput<'_>, quality: TrayShadowQuality) -> TrayShadowKey {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for word in [
        hash_geometry(input.geom),
        material_fingerprint(input.material),
        u64::from(input.preset as u8),
        u64::from(input.yaw.to_bits()),
        u64::from(input.pitch.to_bits()),
        u64::from(quality.resolution()),
        u64::from(input.max_bounces),
        u64::from(TRAY_SHADOW_MODEL_VERSION),
    ] {
        for byte in word.to_le_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
        }
    }
    TrayShadowKey(hash)
}

/// Builds the spectral irradiance ratio with a fixed reduction order, on every available core.
///
/// # Errors
/// Returns a decline for unsupported lighting, invalid geometry, or cancellation.
pub fn build_tray_shadow(
    input: &TrayShadowInput<'_>,
    quality: TrayShadowQuality,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<TrayShadowMap, TrayShadowOff> {
    let threads = thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
    build_tray_shadow_with_threads(input, quality, cancel, progress, threads)
}

/// [`build_tray_shadow`] with a cap on the worker threads. The cap never changes the map's
/// bits: seeds and the order of every floating-point reduction are independent of it.
///
/// # Errors
/// Returns a decline for unsupported lighting, invalid geometry, or cancellation.
pub fn build_tray_shadow_with_threads(
    input: &TrayShadowInput<'_>,
    quality: TrayShadowQuality,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
    threads: NonZeroUsize,
) -> Result<TrayShadowMap, TrayShadowOff> {
    build_with_settings(
        input,
        quality,
        cancel,
        progress,
        BuildSettings {
            resolution: quality.resolution(),
            directions: quality.gather_directions(),
            photons: u64::from(quality.photons()),
            #[cfg(test)]
            photon_chunk_size: 65_536,
            threads: threads.get(),
        },
    )
}

/// Sampling configuration, overridable for deterministic small-map verification.
#[derive(Clone, Copy)]
pub(super) struct BuildSettings {
    /// Width and height in texels.
    pub(super) resolution: u32,
    /// Gather directions per texel.
    pub(super) directions: u32,
    /// Total incident photons.
    pub(super) photons: u64,
    /// Test-only chunk budget; production always traces 65,536 photons per chunk.
    #[cfg(test)]
    pub(super) photon_chunk_size: u64,
    /// Concurrent workers; seeds and reduction order are independent of this value.
    pub(super) threads: usize,
}

/// Executes the same ordered build with small sizes and forced workers for verification.
pub(super) fn build_with_settings(
    input: &TrayShadowInput<'_>,
    quality: TrayShadowQuality,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
    settings: BuildSettings,
) -> Result<TrayShadowMap, TrayShadowOff> {
    build_inner(
        input,
        quality,
        cancel,
        progress,
        settings,
        #[cfg(test)]
        None,
    )
}

/// Test-only access to the pre-filter, pre-fade energy balance of the production stages.
#[cfg(test)]
pub(super) fn build_with_energy(
    input: &TrayShadowInput<'_>,
    quality: TrayShadowQuality,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
    settings: BuildSettings,
) -> Result<(TrayShadowMap, [f64; TRAY_SHADOW_BANDS]), TrayShadowOff> {
    let mut energy = [0.0; TRAY_SHADOW_BANDS];
    let map = build_inner(
        input,
        quality,
        cancel,
        progress,
        settings,
        Some(&mut energy),
    )?;
    Ok((map, energy))
}

fn build_inner(
    input: &TrayShadowInput<'_>,
    quality: TrayShadowQuality,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
    settings: BuildSettings,
    #[cfg(test)] energy: Option<&mut [f64; TRAY_SHADOW_BANDS]>,
) -> Result<TrayShadowMap, TrayShadowOff> {
    let BuildSettings {
        resolution,
        photons,
        ..
    } = settings;
    if input.preset.is_uv_lamp() {
        return Err(TrayShadowOff::UvLamp);
    }
    if input.preset == LightingPreset::Aset {
        return Err(TrayShadowOff::Aset);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(TrayShadowOff::Cancelled);
    }
    let frame = StoneFrame::new(input.geom)?;
    let stone = StoneHit::new(input.geom);
    let gather = LightTable::gather(input.preset, input.yaw, input.pitch);
    let emission = LightTable::emission(input.preset, input.yaw, input.pitch);
    let environment = input.preset.studio(1.0, input.yaw, input.pitch);
    let absorption = binned_absorption_for(input.material, environment);
    let n = std::array::from_fn(|band| {
        input
            .material
            .dispersion
            .evaluate(TRAY_SHADOW_BAND_WIDTH_NM.mul_add(band as f32, TRAY_SHADOW_FIRST_CENTER_NM))
    });
    let side = usize::try_from(resolution).expect("map resolution fits usize");
    let blocked = gather_blocked(&frame, &stone, &gather, &settings, cancel, progress);
    if cancel.load(Ordering::Relaxed) {
        return Err(TrayShadowOff::Cancelled);
    }
    let radius = f64::from(frame.sphere_radius);
    let weight = emission.total * std::f64::consts::PI * radius * radius / photons as f64;
    let ctx = PhotonCtx {
        frame: &frame,
        stone: &stone,
        table: &emission,
        n,
        absorption: &absorption,
        opaque_body: input.material.scattering_sigma_s > 0.0,
        convex: input.geom.tools.is_empty(),
        max_bounces: input.max_bounces,
        weight: round_to_f32(weight),
    };
    let acc = ordered_photons(&ctx, &settings, cancel, progress);
    if cancel.load(Ordering::Relaxed) {
        return Err(TrayShadowOff::Cancelled);
    }
    let texel_side = 2.0 * f64::from(frame.half_extent) / f64::from(resolution);
    let area = texel_side * texel_side;
    let caustic: Vec<f32> = acc
        .into_iter()
        .map(|value| round_to_f32(value / (area * gather.total)))
        .collect();
    #[cfg(test)]
    if let Some(energy) = energy {
        for (texel, &blocked_fraction) in blocked.iter().enumerate() {
            for (band, balance) in energy.iter_mut().enumerate() {
                let net = f64::from(blocked_fraction)
                    - f64::from(caustic[texel * TRAY_SHADOW_BANDS + band]);
                *balance = (net * gather.total).mul_add(area, *balance);
            }
        }
    }
    let caustic = blur(&caustic, side);
    let weights = bin_weights(environment);
    let (factors, rgb) = assemble_factors(&caustic, blocked, side, resolution, &weights);
    if cancel.load(Ordering::Relaxed) {
        return Err(TrayShadowOff::Cancelled);
    }
    // `from_parts` only debug-asserts its data, so a nonfinite frame or ratio is declined here.
    let finite_frame = frame.plane_y.is_finite()
        && frame.center.iter().all(|x| x.is_finite())
        && frame.half_extent.is_finite()
        && frame.half_extent > 0.0;
    if !finite_frame
        || !factors.iter().all(|x| x.is_finite() && *x >= 0.0)
        || !rgb.iter().flatten().all(|x| x.is_finite())
    {
        return Err(TrayShadowOff::DegenerateStone);
    }
    Ok(TrayShadowMap::from_parts(
        frame.plane_y,
        frame.center,
        frame.half_extent,
        resolution,
        factors,
        rgb,
        tray_shadow_key(input, quality),
    ))
}

/// Combines the blocked fractions and filtered caustic into faded per-band ratios and their
/// Ray-Traced RGB projections, one entry per texel.
fn assemble_factors(
    caustic: &[f32],
    blocked: Vec<f32>,
    side: usize,
    resolution: u32,
    weights: &[Vec3; 40],
) -> (Vec<f32>, Vec<[f32; 3]>) {
    let mut factors = Vec::with_capacity(caustic.len());
    let mut rgb = Vec::with_capacity(blocked.len());
    for (texel, blocked) in blocked.into_iter().enumerate() {
        let u = -1.0 + (2 * (texel % side) + 1) as f32 / resolution as f32;
        let v = -1.0 + (2 * (texel / side) + 1) as f32 / resolution as f32;
        let edge = ((u.abs().max(v.abs()) - 0.85) / 0.15).clamp(0.0, 1.0);
        let smooth = edge * edge * (-2.0_f32).mul_add(edge, 3.0);
        let fade = 1.0 - smooth;
        let mut texel_factors = [0.0; TRAY_SHADOW_BANDS];
        for (band, value) in texel_factors.iter_mut().enumerate() {
            let redirected = caustic[texel * TRAY_SHADOW_BANDS + band];
            let factor = (redirected - blocked).mul_add(fade, 1.0);
            factors.push(factor);
            *value = factor;
        }
        rgb.push(spectral_to_rgb(&texel_factors, weights));
    }
    (factors, rgb)
}

/// Adds photons in chunk order, preserving every floating-point reduction across workers.
fn splat(acc: &mut [f64], frame: &StoneFrame, resolution: u32, deposits: &[Deposit]) {
    let side = usize::try_from(resolution).expect("map resolution fits usize");
    for deposit in deposits {
        let half = 0.5 * resolution as f32;
        let fx = ((deposit.x - frame.center[0]) / frame.half_extent + 1.0).mul_add(half, -0.5);
        let fy = ((deposit.z - frame.center[1]) / frame.half_extent + 1.0).mul_add(half, -0.5);
        let x0 = fx.floor();
        let y0 = fy.floor();
        let ax = f64::from(fx - x0);
        let ay = f64::from(fy - y0);
        for (x, wx) in [(x0, 1.0 - ax), (x0 + 1.0, ax)] {
            for (y, wy) in [(y0, 1.0 - ay), (y0 + 1.0, ay)] {
                if x >= 0.0 && y >= 0.0 && x < resolution as f32 && y < resolution as f32 {
                    let ix = texel_index(x);
                    let iy = texel_index(y);
                    let slot = (iy * side + ix) * TRAY_SHADOW_BANDS + usize::from(deposit.band);
                    acc[slot] = (f64::from(deposit.weight) * wx).mul_add(wy, acc[slot]);
                }
            }
        }
    }
}

/// Normalized radius-two Gaussian with sigma 0.75 texel.
pub(super) fn gaussian_kernel() -> [f32; 5] {
    let raw = [-2.0_f32, -1.0, 0.0, 1.0, 2.0].map(|x| (-x * x / (2.0 * 0.75 * 0.75)).exp());
    let sum: f32 = raw.iter().sum();
    raw.map(|x| x / sum)
}

/// Filters only redirected irradiance, leaving geometric shadow edges untouched.
fn blur(input: &[f32], side: usize) -> Vec<f32> {
    let kernel = gaussian_kernel();
    let mut source = input.to_vec();
    for horizontal in [true, false] {
        let mut target = vec![0.0; source.len()];
        for y in 0..side {
            for x in 0..side {
                for band in 0..TRAY_SHADOW_BANDS {
                    let mut value = 0.0;
                    for (tap, weight) in kernel.iter().enumerate() {
                        let shifted = |coordinate: usize| {
                            coordinate
                                .saturating_add(tap)
                                .saturating_sub(2)
                                .min(side - 1)
                        };
                        let (sx, sy) = if horizontal {
                            (shifted(x), y)
                        } else {
                            (x, shifted(y))
                        };
                        value += source[(sy * side + sx) * TRAY_SHADOW_BANDS + band] * weight;
                    }
                    target[(y * side + x) * TRAY_SHADOW_BANDS + band] = value;
                }
            }
        }
        source = target;
    }
    source
}

/// Gathers fixed row chunks into their texel indices, irrespective of arrival order.
fn gather_blocked(
    frame: &StoneFrame,
    stone: &StoneHit<'_>,
    gather: &LightTable,
    settings: &BuildSettings,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Vec<f32> {
    let BuildSettings {
        resolution,
        directions,
        threads,
        ..
    } = *settings;
    let side = usize::try_from(resolution).expect("map resolution fits usize");
    let mut blocked = vec![0.0; side * side];
    progress(0.0);
    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        let (tx, rx) = mpsc::sync_channel(2 * threads);
        for _ in 0..threads {
            let (next, tx, frame, stone, gather) = (&next, tx.clone(), frame, stone, gather);
            scope.spawn(move || {
                loop {
                    let chunk = next.fetch_add(1, Ordering::Relaxed);
                    let row = chunk * 8;
                    if row >= side || cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let mut values = Vec::new();
                    for y in row..(row + 8).min(side) {
                        for x in 0..side {
                            values.push(blocked_fraction(
                                frame,
                                stone,
                                gather,
                                (
                                    u32::try_from(x).expect("texel x fits u32"),
                                    u32::try_from(y).expect("texel y fits u32"),
                                ),
                                resolution,
                                directions,
                            ));
                        }
                    }
                    if tx.send((row, values)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        let mut done = 0;
        for (row, values) in rx {
            blocked[row * side..row * side + values.len()].copy_from_slice(&values);
            done += values.len();
            progress(0.3 * done as f32 / blocked.len() as f32);
        }
    });
    blocked
}

/// Raises its flag when dropped while a panic unwinds, so threads that wait on the panicking
/// thread's output stop waiting instead of hanging the scope.
struct PanicFlag<'a>(&'a AtomicBool);

impl Drop for PanicFlag<'_> {
    fn drop(&mut self) {
        if thread::panicking() {
            self.0.store(true, Ordering::Release);
        }
    }
}

/// Reduces the bounded producer stream in ascending chunk order using double precision.
fn ordered_photons(
    ctx: &PhotonCtx<'_>,
    settings: &BuildSettings,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Vec<f64> {
    let BuildSettings {
        resolution,
        photons,
        threads,
        ..
    } = *settings;
    let side = usize::try_from(resolution).expect("map resolution fits usize");
    #[cfg(test)]
    let chunk_size = settings.photon_chunk_size;
    #[cfg(not(test))]
    let chunk_size = 65_536;
    let chunks = usize::try_from(photons.div_ceil(chunk_size)).unwrap_or(usize::MAX);
    let mut acc = vec![0.0_f64; side * side * TRAY_SHADOW_BANDS];
    let next = AtomicUsize::new(0);
    let committed = AtomicUsize::new(0);
    // Raised when any thread of this scope panics: a panicked worker's chunk never commits,
    // so the others must stop waiting for it and let the scope propagate the panic.
    let failed = AtomicBool::new(false);
    // Workers ahead of the window park here; the reducer notifies after every commit.
    let gate = (Mutex::new(()), Condvar::new());
    // A sliding window bounds completed, queued and in-flight deposit vectors. A slow
    // early chunk cannot let all later chunks accumulate in the pending tree.
    let window = 2 * threads;
    thread::scope(|scope| {
        let (tx, rx) = mpsc::sync_channel(window);
        for _ in 0..threads {
            let (next, committed, failed, gate, tx) =
                (&next, &committed, &failed, &gate, tx.clone());
            scope.spawn(move || {
                let _guard = PanicFlag(failed);
                loop {
                    let chunk = next.fetch_add(1, Ordering::Relaxed);
                    if chunk >= chunks
                        || cancel.load(Ordering::Relaxed)
                        || failed.load(Ordering::Acquire)
                    {
                        break;
                    }
                    // Park until the reducer commits; the short timeout also observes `cancel`
                    // and `failed`, which nobody notifies.
                    let mut held = gate.0.lock().unwrap_or_else(PoisonError::into_inner);
                    while chunk >= committed.load(Ordering::Acquire).saturating_add(window) {
                        if cancel.load(Ordering::Relaxed) || failed.load(Ordering::Acquire) {
                            break;
                        }
                        held = gate
                            .1
                            .wait_timeout(held, Duration::from_millis(5))
                            .unwrap_or_else(PoisonError::into_inner)
                            .0;
                    }
                    drop(held);
                    if cancel.load(Ordering::Relaxed) || failed.load(Ordering::Acquire) {
                        break;
                    }
                    let first = chunk as u64 * chunk_size;
                    let mut deposits = Vec::new();
                    trace_chunk(
                        ctx,
                        chunk as u64,
                        first,
                        (photons - first).min(chunk_size),
                        &mut deposits,
                    );
                    if tx.send((chunk, deposits)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        // A panic in the reducer (e.g. in the progress callback) must also release the workers.
        let _guard = PanicFlag(&failed);
        let mut pending = BTreeMap::new();
        let mut want = 0;
        for (chunk, deposits) in rx {
            if cancel.load(Ordering::Relaxed) {
                continue;
            }
            pending.insert(chunk, deposits);
            while let Some(deposits) = pending.remove(&want) {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                splat(&mut acc, ctx.frame, resolution, &deposits);
                want += 1;
                committed.store(want, Ordering::Release);
                gate.1.notify_all();
                progress(0.3 + 0.7 * want as f32 / chunks as f32);
            }
        }
    });
    acc
}

/// Rounds double-precision accumulated energy once to the map's scalar format.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the map intentionally stores f32 after ordered f64 accumulation"
)]
const fn round_to_f32(value: f64) -> f32 {
    value as f32
}

/// Converts an already bounds-checked nonnegative integral texel coordinate.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "splat checks the integral coordinate against the map bounds before indexing"
)]
const fn texel_index(value: f32) -> usize {
    value as usize
}

/// Projects twenty tray bands with the actual Ray-Traced illuminant-bin weights.
pub(super) fn spectral_to_rgb(
    factors: &[f32; TRAY_SHADOW_BANDS],
    weights: &[Vec3; 40],
) -> [f32; 3] {
    let mut color = Vec3::ZERO;
    for (band, factor) in factors.iter().enumerate() {
        color += *factor * (weights[2 * band] + weights[2 * band + 1]);
    }
    color.to_array()
}
