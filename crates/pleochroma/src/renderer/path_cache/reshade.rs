//! Re-shading a cached pose under another lighting or exposure, without tracing (guide 8.11).
//!
//! The path cache holds, per pose, every way light leaves the stone, and its key leaves out the
//! lighting and the exposure. [`reshade`] therefore turns a cache plus a new environment into an
//! image directly: every pixel's every cached position, every exit record, evaluated against the
//! new environment. There is no noise, no random number and no ray tracing.
//!
//! # What it evaluates
//!
//! The evaluation of one record is [`Shader::shade_position`] of phase B itself, so the
//! lighting model cannot diverge from the path tracer's: the very functions the tracer's exit
//! calls (`sample_environment_channels` and `sample_environment_channel`), the studio rig and the
//! small lights, the glare scale of the first surface reflection, the spectral integration
//! (`integrate_channels_to_xyz`) and the white balance of a studio environment. `reshade` only
//! replaces the random choices by a fixed grid:
//!
//! - **Positions.** A pixel's cached positions are the sub-pixel positions phase A built; each
//!   counts equally, which is what the path tracer's uniform position choice converges to.
//! - **Wavelengths.** [`COMBS`] (6) hero combs of 8 channels each: the hero draws
//!   `(i + 0.5) / 48` for `i` in `0..6`, through the tracer's own
//!   `wrapped_hero_wavelengths`. That is 48 wavelengths, 400/48 = 8.33 nm apart, covering
//!   `[380, 780)` evenly (a midpoint rule of the very estimator the path tracer draws from at
//!   random). A record's absorption is evaluated at each of these wavelengths (it is not part of
//!   the comb weights), and the environment varies faster than the 50 nm comb, so the denser set
//!   costs 6 times the lighting work and reduces the quadrature error.
//!
//! The result is the per-pixel mean XYZ, white balanced like a traced sample (the sum over taps
//! the Ray-Traced engine hands to its tone map is this mean times the tap count).
//!
//! # What it leaves out
//!
//! The residual (the energy of branches the build dropped below its cutoff) is **not** completed:
//! there is no tracing here. Each dropped branch is below `CUTOFF` (1e-3) of the light, and the
//! mean of what was dropped per position is returned in [`Reshaded::mean_residual`] so the
//! caller can see the size; for an ordinary stone it is a fraction of a percent, so the
//! re-shaded stone is that much dimmer than the path tracer's mean. Pixels without cached
//! positions (the background, an ineligible stone) are not covered: [`Reshaded::covered`] says
//! which, and the caller renders those as it did before.

use std::{
    fmt,
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

use glam::Vec3;

use super::{
    frame::PathCacheFrame,
    layout::{TileCache, tile_rect},
    record::COMB_LEN,
    shade::{PositionRef, Shader},
};
use crate::{
    optics::raytracer::{
        build_plane_soa, pixel_rotations, sample_draws, transport::PreparedMaterial,
    },
    renderer::frame_scene::FrameScene,
};

/// Hero combs of the fixed wavelength set: 6 combs of [`COMB_LEN`] channels, 48 wavelengths.
pub const COMBS: usize = 6;

/// The hero draw of comb `index`: the middle of the `index`-th of 48 equal slices of the spectrum,
/// so the 48 wavelengths of all combs are 400/48 nm apart.
#[must_use]
pub fn comb_hero_rand(index: usize) -> f32 {
    (index as f32 + 0.5) / (COMBS * COMB_LEN) as f32
}

/// Why a re-shade produced no image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReshadeError {
    /// The cancel flag was raised during the pass.
    Cancelled,
    /// The cache was built for another frame size than the scene's.
    SizeMismatch,
}

impl fmt::Display for ReshadeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cancelled => "the re-shade was cancelled",
            Self::SizeMismatch => "the path cache was built for another frame size",
        })
    }
}

impl std::error::Error for ReshadeError {}

/// The re-shaded frame: the mean XYZ of every pixel and which pixels the cache covered.
#[derive(Clone, Debug, PartialEq)]
pub struct Reshaded {
    /// Mean XYZ per pixel, row-major, white balanced; zero where [`Self::covered`] is `false`.
    pub xyz: Vec<Vec3>,
    /// Whether the pixel has cached positions (and so was shaded here).
    pub covered: Vec<bool>,
    /// The mean residual (dropped energy, as a fraction of the light) per cached position.
    pub mean_residual: f64,
}

/// One shaded row, kept with its index so the rows can be put back in order.
struct RowOut {
    y: usize,
    xyz: Vec<Vec3>,
    covered: Vec<bool>,
    residual_sum: f64,
    positions: u64,
}

/// One shaded pixel.
struct PixelOut {
    xyz: Vec3,
    residual_sum: f64,
    positions: u64,
}

/// Shades pixel `(x, y)` from its cached positions, or `None` when it has none.
fn shade_pixel(
    shader: &Shader<'_, '_>,
    scene: &FrameScene<'_>,
    tile: &TileCache,
    local_pixel: usize,
    (x, y): (u32, u32),
) -> Option<PixelOut> {
    let positions = tile.pixel_positions(local_pixel);
    let count = positions.len();
    if count == 0 {
        return None;
    }
    let pixel = y * scene.width + x;
    let rot = pixel_rotations(pixel);
    let mut sum = Vec3::ZERO;
    let mut residual_sum = 0.0_f64;
    for (local, position) in positions.enumerate() {
        // Position `j` is the sample-sequence jitter of sample index `j`, as phase A built it.
        let jitter = sample_draws(pixel, u32::try_from(local).ok()?, &rot);
        let ray = scene.camera.generate_ray(
            x as f32,
            y as f32,
            scene.width as f32,
            scene.height as f32,
            jitter.jitter_x,
            jitter.jitter_y,
        );
        // No residual: nothing is traced here.
        let reference = PositionRef {
            records: tile.position_records(position),
            residual: 0.0,
        };
        for comb in 0..COMBS {
            sum += shader
                .shade_position(reference, ray, (0, comb_hero_rand(comb)))
                .kept;
        }
        residual_sum += f64::from(tile.position_residual(position));
    }
    Some(PixelOut {
        xyz: sum / (count * COMBS) as f32,
        residual_sum,
        positions: count as u64,
    })
}

/// Shades row `y` of the frame.
fn shade_row(
    frame: &PathCacheFrame<'_, '_>,
    shader: &Shader<'_, '_>,
    scene: &FrameScene<'_>,
    y: u32,
) -> RowOut {
    let width = scene.width as usize;
    let tile_size = frame.settings().tile;
    let mut row = RowOut {
        y: y as usize,
        xyz: Vec::with_capacity(width),
        covered: Vec::with_capacity(width),
        residual_sum: 0.0,
        positions: 0,
    };
    for x in 0..scene.width {
        let index = frame.tile_of((x, y));
        let result = frame.tile(index).and_then(|tile| {
            let (x0, y0, x1, _) = tile_rect(scene.width, scene.height, tile_size, index);
            let local = (y - y0) as usize * (x1 - x0) as usize + (x - x0) as usize;
            shade_pixel(shader, scene, tile, local, (x, y))
        });
        if let Some(pixel) = result {
            row.xyz.push(pixel.xyz);
            row.covered.push(true);
            row.residual_sum += pixel.residual_sum;
            row.positions += pixel.positions;
        } else {
            row.xyz.push(Vec3::ZERO);
            row.covered.push(false);
        }
    }
    row
}

/// Re-shades the cached pose of `frame` under `scene`'s environment.
///
/// `scene` is the frame's scene with the NEW lighting (its `environment`, which carries the
/// exposure); its geometry, material, camera and size must be those the cache was built for (the
/// cache key says so). `threads` workers each take every `threads`-th row; the result does not
/// depend on the count: every pixel is computed alone and the rows are put back in order. A tile
/// the cache does not hold yet is built on first use, deterministically, as in phase B.
///
/// # Errors
///
/// [`ReshadeError::SizeMismatch`] when `frame` was built for another size than `scene`'s, and
/// [`ReshadeError::Cancelled`] when `cancel` was raised (checked once per row).
pub fn reshade(
    frame: &PathCacheFrame<'_, '_>,
    scene: &FrameScene<'_>,
    threads: usize,
    cancel: &AtomicBool,
) -> Result<Reshaded, ReshadeError> {
    if !frame.matches(scene.width, scene.height) {
        return Err(ReshadeError::SizeMismatch);
    }
    let (width, height) = (scene.width as usize, scene.height as usize);
    if width == 0 || height == 0 {
        return Ok(Reshaded {
            xyz: Vec::new(),
            covered: Vec::new(),
            mean_residual: 0.0,
        });
    }
    let soa = build_plane_soa(scene.planes);
    let shader = Shader::new(scene, &soa, PreparedMaterial::new(scene.material));
    let threads = threads.clamp(1, height);
    let mut rows: Vec<RowOut> = thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|first| {
                let shader = &shader;
                scope.spawn(move || {
                    let mut done = Vec::new();
                    for y in (first..height).step_by(threads) {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        // `y` is below the frame height, which is a `u32`.
                        let row = u32::try_from(y).unwrap_or(u32::MAX);
                        done.push(shade_row(frame, shader, scene, row));
                    }
                    done
                })
            })
            .collect();
        // Every worker is spawned above before the first join, so the rows run in parallel.
        let mut rows = Vec::with_capacity(height);
        for handle in handles {
            rows.extend(
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            );
        }
        rows
    });
    if cancel.load(Ordering::Relaxed) || rows.len() != height {
        return Err(ReshadeError::Cancelled);
    }
    rows.sort_by_key(|row| row.y);
    let mut xyz = Vec::with_capacity(width * height);
    let mut covered = Vec::with_capacity(width * height);
    let (mut residual_sum, mut positions) = (0.0_f64, 0_u64);
    for row in rows {
        xyz.extend(row.xyz);
        covered.extend(row.covered);
        residual_sum += row.residual_sum;
        positions += row.positions;
    }
    let mean_residual = if positions == 0 {
        0.0
    } else {
        residual_sum / positions as f64
    };
    Ok(Reshaded {
        xyz,
        covered,
        mean_residual,
    })
}
