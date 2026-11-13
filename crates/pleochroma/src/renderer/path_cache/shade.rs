//! Phase B: shading a sample of a cached stone pixel from its exit records (guide section 8.4).
//!
//! A sample picks one of the pixel's cached sub-pixel positions, draws the hero wavelengths
//! exactly as the path tracer does, evaluates every exit record of the position against the
//! environment, and completes the energy the cache dropped. The result is the per-sample XYZ the
//! path tracer would have added, so accumulation, the half buffer and the counts stay as they are.
//!
//! # What matches the path tracer
//!
//! - The wavelengths are [`wrapped_hero_wavelengths`] of the sample's `hero_rand`, from the same
//!   [`sample_draws`].
//! - The lighting is [`sample_environment_channels`] / [`sample_environment_channel`], the
//!   functions the tracer's exit (`accumulate_miss_radiance`) calls: the studio rig is the one the
//!   tracer builds, the observer is `-ray.dir` of the sample's camera ray, and the glare scale
//!   multiplies the first-surface reflection. A specular exit has no next-event estimation (the
//!   tracer only connects scattering and frosted vertices to lights), so its miss weight is `1`.
//! - The absorption is the tracer's: `exp(-tau)` at each of the sample's wavelengths from the
//!   record's optical-depth coefficients and the material's band sums (`build::principal_alphas`),
//!   never interpolated over the comb.
//! - The spectral integration is [`integrate_channels_to_xyz`] with equal path densities (every
//!   channel's own direction and weight are used, so no hero weighting is left), and the white
//!   balance of a studio environment is applied to the sum like the tracer's `finish`.
//!
//! # The residual
//!
//! Branches the tree dropped under the cutoff carry the position's `residual`. Phase B adds them
//! back without bias: with probability `p = min(1, RESIDUAL_FIRE_GAIN * residual)` it traces one
//! ordinary path-tracer sample through the position's camera ray and keeps `sample / p` only if
//! that path is **not covered** by a record, that is, if no record of the position has the path's
//! branch signature (its facets, reflect or transmit at each, and the eigenmode after each) and
//! energy at the hero wavelength. A path a
//! record covers is already in the kept part; every other path belongs to the dropped energy.
//! Positions with a residual under [`RESIDUAL_FLOOR`] skip this. The GPU twin of the completion and
//! of [`is_covered`] is `shaders/path_cache/complete.wgsl`.

use glam::Vec3;

use super::{
    build::principal_alphas,
    layout::TileCache,
    record::{COMB_LEN, DEPTH_LEN, ExitRecord, FLAG_GLARE},
    signature::fold_sig,
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{
        fluorescence::Fluorescence,
        raytracer::{
            PathEvents, PixelRotations, Ray, apply_von_kries_white_balance,
            cranley_patterson_rotate,
            environment::{
                EnvironmentSource, environment_white_balance, sample_environment_channel,
                sample_environment_channels,
            },
            hash_u32, integrate_channels_to_xyz, low_discrepancy_base2, radical_inverse_base,
            sample_draws,
            surface::{SurfaceScene, unit_draw},
            transport::{
                PreparedMaterial, trace_spectral_ray_attributed_prepared, wrapped_hero_wavelengths,
            },
        },
        studio_rig::StudioRig,
        tables::beer_lambert_factors,
    },
    renderer::frame_scene::FrameScene,
    simd::PlanesSoA32,
};

/// Positions with a residual below this skip the residual completion.
pub const RESIDUAL_FLOOR: f32 = 1.0e-4;

/// How much more often than the residual itself the completion traces.
///
/// The probability of tracing is `min(1, RESIDUAL_FIRE_GAIN * residual)`. A larger gain costs more
/// traces and lowers the noise of the completion (its variance is about `residual / probability`).
pub const RESIDUAL_FIRE_GAIN: f32 = 8.0;

/// Radical-inverse base of the position choice (the jitter uses 2 and 3, the hero 5).
const POSITION_BASE: u32 = 7;

/// Salt of the per-pixel rotation of the position choice.
const POSITION_ROTATION_STREAM: u32 = 0x2C1B_3C6D;

/// Stream of the draw that decides whether a sample traces its residual.
const RESIDUAL_FIRE_STREAM: u32 = 0x297A_2D39;

/// The records and residual of one cached position.
#[derive(Clone, Copy, Debug)]
pub struct PositionRef<'r> {
    /// The position's exit records.
    pub records: &'r [ExitRecord],
    /// The summed spectral-mean weight of the branches dropped at the position.
    pub residual: f32,
}

/// What one shaded sample adds to its pixel, in two parts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleParts {
    /// The records' contribution: XYZ, white balanced and clamped like a traced sample.
    pub kept: Vec3,
    /// The completion of the dropped energy: zero unless the sample traced and the path was not
    /// covered, then already divided by the probability of tracing.
    pub residual: Vec3,
    /// Whether the sample traced the residual.
    pub traced: bool,
}

impl SampleParts {
    /// The per-sample XYZ the path tracer would add.
    #[must_use]
    pub fn total(&self) -> Vec3 {
        self.kept + self.residual
    }
}

/// Everything phase B reads besides the cache of the pixel.
pub struct Shader<'s, 'a> {
    scene: &'s FrameScene<'a>,
    plane_soa: &'s PlanesSoA32,
    prepared: PreparedMaterial<'a>,
    rig: Option<StudioRig>,
    white_balance: Option<Vec3>,
    glare_scale: f32,
}

/// The index a unit value picks among `count` positions.
fn index_of(unit: f32, count: usize) -> usize {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is in [0, 1) and scaled by a small count; negative or NaN gives 0"
    )]
    let index = (unit * count as f32) as usize;
    index.min(count.saturating_sub(1))
}

/// The position (0-based, below `count`) sample `sample_num` of `pixel` shades.
///
/// A rotated radical-inverse sequence: uniform over the positions, stratified over the samples,
/// and independent of the sample's parity so the two halves of the convergence check see the same
/// mix of positions.
#[must_use]
pub fn pick_position(pixel: u32, sample_num: u32, count: usize) -> usize {
    let rotation = low_discrepancy_base2(hash_u32(pixel ^ POSITION_ROTATION_STREAM));
    let unit = cranley_patterson_rotate(radical_inverse_base(sample_num, POSITION_BASE), rotation);
    index_of(unit, count)
}

impl<'s, 'a> Shader<'s, 'a> {
    /// Builds the shader of a frame: the studio rig, the white balance and the glare scale of its
    /// environment, and the prepared material the residual trace uses.
    #[must_use]
    pub fn new(
        scene: &'s FrameScene<'a>,
        plane_soa: &'s PlanesSoA32,
        prepared: PreparedMaterial<'a>,
    ) -> Self {
        let environment = scene.environment;
        let (rig, white_balance) = match environment {
            EnvironmentSource::Studio {
                light_yaw,
                light_pitch,
                ..
            } => (
                Some(StudioRig::new(light_yaw, light_pitch)),
                Some(environment_white_balance(environment)),
            ),
            EnvironmentSource::HdrMap { .. } => (None, None),
        };
        let glare = environment.surface_glare().clamp(0.0, 1.0);
        Self {
            scene,
            plane_soa,
            prepared,
            rig,
            white_balance,
            glare_scale: if glare < 1.0 { glare } else { 1.0 },
        }
    }

    /// Shades sample `sample_num` of the pixel `(x, y)` from its cache.
    ///
    /// `tile` is the cache of the pixel's tile and `local_pixel` the pixel's row-major index in
    /// it. Returns `None` for a pixel without positions: the caller traces it.
    #[must_use]
    pub fn shade_sample(
        &self,
        tile: &TileCache,
        local_pixel: usize,
        (x, y): (u32, u32),
        sample_num: u32,
        rot: &PixelRotations,
    ) -> Option<SampleParts> {
        let positions = tile.pixel_positions(local_pixel);
        if positions.is_empty() {
            return None;
        }
        let (width, height) = (self.scene.width, self.scene.height);
        let pixel = y * width + x;
        let local = pick_position(pixel, sample_num, positions.len());
        let position = positions.start + local;
        // Position `j` is the sample-sequence jitter of sample index `j`, as phase A built it.
        let jitter = sample_draws(pixel, u32::try_from(local).ok()?, rot);
        let ray = self.scene.camera.generate_ray(
            x as f32,
            y as f32,
            width as f32,
            height as f32,
            jitter.jitter_x,
            jitter.jitter_y,
        );
        let draws = sample_draws(pixel, sample_num, rot);
        let reference = PositionRef {
            records: tile.position_records(position),
            residual: tile.position_residual(position),
        };
        Some(self.shade_position(reference, ray, (draws.seed, draws.hero_rand)))
    }

    /// Shades one sample of one position: `ray` is the position's camera ray and `(seed,
    /// hero_rand)` the sample's per-bounce seed and hero-wavelength draw.
    #[must_use]
    pub fn shade_position(
        &self,
        position: PositionRef<'_>,
        ray: Ray,
        (seed, hero_rand): (u32, f32),
    ) -> SampleParts {
        let lambdas = wrapped_hero_wavelengths::<COMB_LEN>(hero_rand);
        let kept = self.kept(position.records, ray, &lambdas);
        let (residual, traced) = self.residual(position, ray, (seed, hero_rand));
        SampleParts {
            kept,
            residual,
            traced,
        }
    }

    /// The records' XYZ for the comb `lambdas`, the tracer's `finish` applied.
    fn kept(&self, records: &[ExitRecord], ray: Ray, lambdas: &[f32; COMB_LEN]) -> Vec3 {
        let observer = -ray.dir;
        // The band sums are needed only where a record absorbs.
        let alphas = if records.iter().any(ExitRecord::has_depth) {
            principal_alphas(self.prepared.material, lambdas)
        } else {
            [[0.0; DEPTH_LEN]; COMB_LEN]
        };
        let mut radiance = [0.0_f32; COMB_LEN];
        for record in records {
            self.add_record(record, (ray, observer), (lambdas, &alphas), &mut radiance);
        }
        let xyz = integrate_channels_to_xyz(&radiance, lambdas, &[1.0; COMB_LEN], 0);
        let xyz = self
            .white_balance
            .map_or(xyz, |scale| apply_von_kries_white_balance(xyz, scale));
        xyz.max(Vec3::ZERO)
    }

    /// Adds one record's per-channel radiance to `radiance`.
    ///
    /// A channel's radiance is the record's absorption-free weight times the Beer-Lambert
    /// transmittance at the channel's wavelength (`alphas` are the material's principal absorption
    /// coefficients there) times the environment along its exit direction at that wavelength.
    fn add_record(
        &self,
        record: &ExitRecord,
        (ray, observer): (Ray, Vec3),
        (lambdas, alphas): (&[f32; COMB_LEN], &[[f32; DEPTH_LEN]; COMB_LEN]),
        radiance: &mut [f32; COMB_LEN],
    ) {
        let scale = if record.flags & FLAG_GLARE == 0 {
            1.0
        } else {
            self.glare_scale
        };
        let mut weights = [0.0_f32; COMB_LEN];
        for (weight, &lambda) in weights.iter_mut().zip(lambdas) {
            *weight = record.weight_at(lambda) * scale;
        }
        if record.has_depth() {
            let mut args = [0.0_f32; COMB_LEN];
            for (arg, channel) in args.iter_mut().zip(alphas) {
                *arg = -record.optical_depth(*channel);
            }
            // The tracer's vectorized exponential, without the optics tables.
            let transmittance = beer_lambert_factors(None, args);
            for (weight, factor) in weights.iter_mut().zip(transmittance) {
                *weight *= factor;
            }
        }
        let active = weights.iter().filter(|&&weight| weight > 0.0).count();
        if active == 0 {
            return;
        }
        let environment = self.scene.environment;
        let direct = record.is_direct();
        if (direct || record.ddir_dlambda == [0, 0]) && active > 2 {
            // One direction for every channel: the wavelength-independent lighting once.
            let dir = if direct { ray.dir } else { record.direction() };
            let env =
                sample_environment_channels(environment, dir, lambdas, self.rig.as_ref(), observer);
            for ((out, &weight), &light) in radiance.iter_mut().zip(&weights).zip(&env) {
                if weight > 0.0 {
                    *out = weight.mul_add(light, *out);
                }
            }
            return;
        }
        for ((out, &weight), &lambda) in radiance.iter_mut().zip(&weights).zip(lambdas) {
            if weight > 0.0 {
                let dir = if direct {
                    ray.dir
                } else {
                    record.direction_at(lambda)
                };
                let light = sample_environment_channel(
                    environment,
                    dir,
                    lambda,
                    self.rig.as_ref(),
                    observer,
                );
                *out = weight.mul_add(light, *out);
            }
        }
    }

    /// The completion of the dropped energy of one sample, and whether it traced.
    fn residual(
        &self,
        position: PositionRef<'_>,
        ray: Ray,
        (seed, hero_rand): (u32, f32),
    ) -> (Vec3, bool) {
        if position.residual < RESIDUAL_FLOOR {
            return (Vec3::ZERO, false);
        }
        let probability = (position.residual * RESIDUAL_FIRE_GAIN).min(1.0);
        if unit_draw(seed, 0, RESIDUAL_FIRE_STREAM) > probability {
            return (Vec3::ZERO, false);
        }
        (
            self.complete_residual(position, ray, (seed, hero_rand)),
            true,
        )
    }

    /// The completion of one sample that is known to trace its residual.
    ///
    /// The GPU's phase B makes the decision to trace (the same draw as [`Self::shade_position`]),
    /// and the GPU completes the fired samples itself with this function's twin
    /// (`shaders/path_cache/complete.wgsl`). The host runs this function for the CPU cached passes
    /// and where the GPU cannot complete them (`GpuPathCache::complete_residuals`): one ordinary
    /// path-tracer sample through `ray`, kept divided by the probability of tracing only if no
    /// record covers the path. Zero when the position has no residual.
    #[must_use]
    pub fn complete_residual(
        &self,
        position: PositionRef<'_>,
        ray: Ray,
        (seed, hero_rand): (u32, f32),
    ) -> Vec3 {
        let probability = (position.residual * RESIDUAL_FIRE_GAIN).min(1.0);
        if probability <= 0.0 {
            return Vec3::ZERO;
        }
        let (traced, events) = trace_spectral_ray_attributed_prepared(
            ray,
            StoneGeometry::planes_only(self.scene.planes),
            self.plane_soa,
            self.scene.facet_finishes,
            &self.prepared,
            Fluorescence::none(),
            self.scene.max_bounces,
            self.scene.environment,
            seed,
            hero_rand,
            SurfaceScene::IDEAL,
        );
        let hero = wrapped_hero_wavelengths::<COMB_LEN>(hero_rand)[0];
        if is_covered(position.records, hero, &events) {
            Vec3::ZERO
        } else {
            traced / probability
        }
    }

    /// The completion of sample `sample_num` of pixel `(x, y)` at its cached position `local`.
    ///
    /// `local` is the position's index within the pixel, as phase B picked it (on the GPU, the
    /// position the sample was shaded with, so the cover test sees the records that were summed).
    /// Zero for a position the pixel does not have.
    #[must_use]
    pub fn complete_residual_of_sample(
        &self,
        tile: &TileCache,
        local_pixel: usize,
        (x, y): (u32, u32),
        (sample_num, local): (u32, usize),
        rot: &PixelRotations,
    ) -> Vec3 {
        let positions = tile.pixel_positions(local_pixel);
        let Ok(position_sample) = u32::try_from(local) else {
            return Vec3::ZERO;
        };
        if local >= positions.len() {
            return Vec3::ZERO;
        }
        let (width, height) = (self.scene.width, self.scene.height);
        let pixel = y * width + x;
        let jitter = sample_draws(pixel, position_sample, rot);
        let ray = self.scene.camera.generate_ray(
            x as f32,
            y as f32,
            width as f32,
            height as f32,
            jitter.jitter_x,
            jitter.jitter_y,
        );
        let draws = sample_draws(pixel, sample_num, rot);
        let position = positions.start + local;
        let reference = PositionRef {
            records: tile.position_records(position),
            residual: tile.position_residual(position),
        };
        self.complete_residual(reference, ray, (draws.seed, draws.hero_rand))
    }
}

/// `true` when a record of the position stands for the path `events` describe.
///
/// Such a record has the path's branch signature and an absorption-free weight above zero at the
/// hero wavelength (the transmittance never decides the cover; a branch absorbed to nearly nothing
/// is still covered). A path with no escape direction returned no light, so it is reported
/// covered.
#[must_use]
pub fn is_covered(records: &[ExitRecord], lambda_hero: f32, events: &PathEvents) -> bool {
    if events.escaped_dir.is_none() {
        return true;
    }
    let signature = fold_sig(events.signature);
    records
        .iter()
        .any(|record| record.signature == signature && record.weight_at(lambda_hero) > 0.0)
}
