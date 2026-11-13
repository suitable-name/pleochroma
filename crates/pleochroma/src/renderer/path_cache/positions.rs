//! Adaptive sub-pixel positions (guide section 8.3).
//!
//! Every stone pixel starts with [`K0`] stratified positions, the first sample positions of the
//! path tracer's own jitter sequence. A pixel whose positions disagree on the facet sequence of
//! their dominant branch is an **edge pixel**: it doubles its positions, up to [`K_MAX`], until two
//! consecutive levels agree on the pixel's mean to within half the adaptive-sampling target noise.
//!
//! The cache is lighting independent, so "the pixel's mean" is measured under a fixed probe
//! environment (a smooth sky plus twelve soft lobes) tone-mapped and compared with the adaptive
//! sampler's own metric. The probe only decides how many positions an edge pixel gets; it never
//! enters the cache.

use std::sync::OnceLock;

use glam::Vec3;

use super::{
    build::{DISPERSION_GATE_BG, TraceKit},
    record::ExitRecord,
};
use crate::{
    optics::raytracer::{Camera, pixel_rotations, sample_draws},
    renderer::{
        adaptive::{
            AdaptiveSettings, NoiseMetric,
            metric::{L_FLOOR, pixel_error},
        },
        frame_scene::FrameScene,
        whitted::CUTOFF,
    },
};

/// Sub-pixel positions every stone pixel starts with.
pub const K0: u32 = 4;

/// Most sub-pixel positions an edge pixel can reach.
pub const K_MAX: u32 = 64;

/// Edge of a cache tile in pixels, the adaptive-sampling tile.
pub const DEFAULT_TILE: u32 = 16;

/// Number of soft lobes in the probe environment.
const PROBE_LOBES: u32 = 12;

/// Display multiplier of the probe tone-map.
const PROBE_SCALE: f32 = 1.0;

/// Everything phase A depends on besides the scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuildSettings {
    /// Tile edge in pixels.
    pub tile: u32,
    /// Positions every stone pixel starts with.
    pub k0: u32,
    /// Position cap of an edge pixel.
    pub k_max: u32,
    /// Adaptive-sampling target noise in the unit of [`Self::metric`].
    pub target_noise: f32,
    /// How [`Self::target_noise`] is measured.
    pub metric: NoiseMetric,
    /// Branches lighter than this are dropped into the residual.
    pub cutoff: f32,
    /// Gemological dispersion above which every branch gets per-bin records.
    pub dispersion_gate: f32,
}

impl Default for BuildSettings {
    fn default() -> Self {
        Self {
            tile: DEFAULT_TILE,
            k0: K0,
            k_max: K_MAX,
            target_noise: NoiseMetric::RelativeLstar.default_target(),
            metric: NoiseMetric::RelativeLstar,
            cutoff: CUTOFF,
            dispersion_gate: DISPERSION_GATE_BG,
        }
    }
}

impl BuildSettings {
    /// The defaults with the tile, metric and target of adaptive sampling's settings.
    #[must_use]
    pub fn from_adaptive(adaptive: &AdaptiveSettings) -> Self {
        Self {
            tile: adaptive.tile,
            target_noise: adaptive.target_noise,
            metric: adaptive.metric,
            ..Self::default()
        }
    }

    /// The settings with `k0 >= 1`, `k_max >= k0` and `tile >= 1`.
    #[must_use]
    pub fn sanitized(self) -> Self {
        let k0 = self.k0.max(1);
        Self {
            tile: self.tile.max(1),
            k0,
            k_max: self.k_max.max(k0),
            ..self
        }
    }
}

/// One cached sub-pixel position.
#[derive(Clone, Debug)]
pub struct PositionData {
    /// The position's exit records.
    pub records: Vec<ExitRecord>,
    /// Summed spectral-mean weight of the branches dropped at this position.
    pub residual: f32,
    /// Signature of the position's dominant branch.
    pub signature: u64,
}

impl PositionData {
    /// `true` when the camera ray missed the stone (a single direct-view record).
    #[must_use]
    pub fn is_direct(&self) -> bool {
        self.records.len() == 1 && self.records.iter().all(ExitRecord::is_direct)
    }
}

/// The twelve probe lobe axes on a golden spiral.
fn probe_lobes() -> &'static [Vec3; PROBE_LOBES as usize] {
    static LOBES: OnceLock<[Vec3; PROBE_LOBES as usize]> = OnceLock::new();
    LOBES.get_or_init(|| {
        std::array::from_fn(|i| {
            let t = (i as f32 + 0.5) / PROBE_LOBES as f32;
            let z = t.mul_add(-2.0, 1.0);
            let r = z.mul_add(-z, 1.0).max(0.0).sqrt();
            let phi = i as f32 * 2.399_963_2;
            Vec3::new(r * phi.cos(), z, r * phi.sin())
        })
    })
}

/// The probe environment's XYZ radiance in a direction.
fn probe_radiance(dir: Vec3) -> Vec3 {
    let sky = dir.y.mul_add(0.5, 0.5);
    let mut luminance = sky.mul_add(0.5, 0.1);
    for lobe in probe_lobes() {
        luminance = 0.8f32.mul_add(dir.dot(*lobe).max(0.0).powi(16), luminance);
    }
    Vec3::new(0.95 * luminance, luminance, 1.09 * luminance)
}

/// A position's mean radiance under the probe environment, absorption applied at the comb.
fn probe_of(kit: &TraceKit<'_, '_>, records: &[ExitRecord]) -> Vec3 {
    records.iter().fold(Vec3::ZERO, |acc, record| {
        let mean = kit.comb_weights(record).iter().sum::<f32>() / 8.0;
        acc + probe_radiance(record.direction()) * mean
    })
}

/// `true` when two consecutive levels' means agree to within half the target noise.
fn levels_agree(previous: Vec3, current: Vec3, settings: &BuildSettings) -> bool {
    let Some((half_delta_e, lightness)) =
        pixel_error(previous + current, current, 2, 1, PROBE_SCALE)
    else {
        return false;
    };
    let delta_e = 2.0 * half_delta_e;
    let noise = match settings.metric {
        NoiseMetric::RelativeLstar => delta_e / lightness.max(L_FLOOR),
        NoiseMetric::AbsoluteDeltaE => delta_e,
    };
    noise < 0.5 * settings.target_noise
}

/// The positions of one pixel as they are built, with their probe values.
struct PixelBuild<'x, 's, 'a> {
    kit: &'x TraceKit<'s, 'a>,
    camera: &'x Camera,
    /// Frame width and height in pixels.
    frame: (u32, u32),
    pixel: u32,
    xy: (u32, u32),
    positions: Vec<PositionData>,
    probes: Vec<Vec3>,
}

impl PixelBuild<'_, '_, '_> {
    /// Adds positions until there are `count`, continuing the pixel's sample sequence.
    fn extend_to(&mut self, count: u32) {
        let rotations = pixel_rotations(self.pixel);
        let first = u32::try_from(self.positions.len()).unwrap_or(count);
        for index in first..count {
            let draws = sample_draws(self.pixel, index, &rotations);
            let ray = self.camera.generate_ray(
                self.xy.0 as f32,
                self.xy.1 as f32,
                self.frame.0 as f32,
                self.frame.1 as f32,
                draws.jitter_x,
                draws.jitter_y,
            );
            let output = self.kit.position(ray);
            self.probes.push(probe_of(self.kit, &output.records));
            self.positions.push(PositionData {
                records: output.records,
                residual: output.residual,
                signature: output.signature,
            });
        }
    }

    fn mean_probe(&self) -> Vec3 {
        let sum = self.probes.iter().fold(Vec3::ZERO, |acc, &p| acc + p);
        sum / self.probes.len().max(1) as f32
    }

    fn is_edge(&self) -> bool {
        let first = self.positions.first().map(|p| p.signature);
        self.positions.iter().any(|p| Some(p.signature) != first)
    }
}

/// Builds the cached positions of the pixel `(x, y)`; empty when no camera ray of the first
/// level hits the stone.
#[must_use]
pub fn build_pixel(
    kit: &TraceKit<'_, '_>,
    scene: &FrameScene<'_>,
    (x, y): (u32, u32),
    settings: &BuildSettings,
) -> Vec<PositionData> {
    let mut build = PixelBuild {
        kit,
        camera: scene.camera,
        frame: (scene.width, scene.height),
        pixel: y * scene.width + x,
        xy: (x, y),
        positions: Vec::new(),
        probes: Vec::new(),
    };
    build.extend_to(settings.k0);
    if build.positions.iter().all(PositionData::is_direct) {
        return Vec::new();
    }
    let mut count = settings.k0;
    while count < settings.k_max && build.is_edge() {
        let previous = build.mean_probe();
        count = count.saturating_mul(2).min(settings.k_max);
        build.extend_to(count);
        if levels_agree(previous, build.mean_probe(), settings) {
            break;
        }
    }
    build.positions
}
