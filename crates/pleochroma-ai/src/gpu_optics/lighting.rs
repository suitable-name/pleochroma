//! MIT source: pleochroma/color/metrics/lighting.rs. Production prepares the canonical
//! radiance threshold/rig once per preset; the complete CPU classifier is a parity oracle.
//! The illumination the metrics are scored under: whether a ray leaving the crown sees a
//! light source or only the dim surround.
//!
//! The test reads the same radiance the tracer lights the image with
//! (`sample_studio_environment_with_rig` for the analytic rigs, the map's own luminance for an
//! HDR panorama), so the lobes that make a facet flash in the render are the lobes that
//! count as "illuminated" here, and a preset change moves the numbers the way it moves
//! the picture.
//!
//! "Illuminated" is relative to the environment's ambient level, the median radiance over
//! a fixed set of directions spread evenly over the upper hemisphere. Two rules apply.
//!
//! - The `Studio` rigs (daylight, incandescent, ring lights, dark spotlight) are pinpoint
//!   sources on a dim charcoal surround: a direction is lit when its radiance exceeds
//!   [`SOURCE_OVER_AMBIENT`] times the ambient level, which separates the softboxes and
//!   ring pinpoints from the backdrop. An HDR map follows the same rule: with no region
//!   brighter than [`SOURCE_OVER_AMBIENT`] times its median luminance nothing in it
//!   counts as lit.
//! - Every other model (the light tent, the daylight dome, the ISO hemisphere) is a lit
//!   environment: its walls, sky or dome are the light. A direction is lit when its
//!   radiance exceeds [`UNIFORM_FRACTION_OF_AMBIENT`] times the ambient level, so the
//!   brilliance metric means "light return" under all of them, as it does for the ISO
//!   figure cutters know from `GemRay`. Under the tent the walls count as lit while the
//!   black cards (x0.1), the ground and the head shadow do not, so a facet reflecting a
//!   card still costs extinction. The old 4x rule counted only the tent's key cone and
//!   the dome's sun disc, which made brilliance a measure of aiming exits at one patch.
//!
//! The observer's head shadow is not part of this radiance (the observer is passed as the
//! zero vector): the host reduction applies the observer cone separately.

use glam::Vec3;

use super::head_shadow_cone_cos;
use pleochroma::{
    optics::{
        raytracer::{
            EnvironmentSource, LightingModel, LightingPreset,
            sample_studio_environment_with_rig_spec_power, studio_spec_power,
        },
        studio_rig::StudioRig,
    },
    renderer::env_map::EnvironmentMap,
};

/// Wavelength the analytic rigs are probed at, the photopic luminosity peak.
const PROBE_WAVELENGTH_NM: f32 = 550.0;

/// Exposure the analytic rigs are probed at. The threshold scales with the radiance it
/// is compared against, so the classification does not depend on exposure.
const PROBE_EXPOSURE: f32 = 1.0;

/// Directions [`hemisphere_direction`] spreads over the upper hemisphere, sampled to find
/// the ambient level.
pub(super) const HEMISPHERE_DIRECTIONS: usize = 96;

/// Golden angle in radians, the azimuth step of the equal-area spiral over the hemisphere.
const GOLDEN_ANGLE_RAD: f32 = 2.399_963_2;

/// How many times brighter than the ambient level a direction must be to count as lit
/// under a `Studio` rig or an HDR map (pinpoint sources on a dim surround).
const SOURCE_OVER_AMBIENT: f32 = 4.0;

/// The fraction of the ambient level above which a direction of a lit environment (every
/// non-`Studio` model: tent, dome, ISO hemisphere) counts as lit; below it only the dark
/// cards, the ground and the soft falloff at the horizon remain.
const UNIFORM_FRACTION_OF_AMBIENT: f32 = 0.5;

/// Rec. 709 luminance weights for an HDR texel's linear RGB.
const LUMINANCE_WEIGHTS: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// Where the radiance of a direction comes from.
#[derive(Clone)]
enum RadianceSource<'a> {
    /// The analytic studio rig of a preset at a light pose, its key, fill and ring
    /// directions built once for the pose (boxed: the rig is far larger than the other
    /// variant).
    Studio {
        preset: LightingPreset,
        rig: Box<StudioRig>,
        /// [`studio_spec_power`] of `preset` at [`PROBE_WAVELENGTH_NM`], evaluated once: it
        /// is a pure function of the preset, and every exited ray used to recompute it.
        spec_power: f32,
    },
    /// A loaded HDR panorama.
    Hdr(&'a EnvironmentMap),
}

impl RadianceSource<'_> {
    /// The radiance seen along unit direction `dir`.
    fn radiance(&self, dir: Vec3) -> f32 {
        match self {
            Self::Studio {
                preset,
                rig,
                spec_power,
            } => sample_studio_environment_with_rig_spec_power(
                dir,
                *preset,
                PROBE_EXPOSURE,
                rig,
                Vec3::ZERO,
                *spec_power,
            ),
            Self::Hdr(map) => {
                let [r, g, b] = map.radiance_rgb(dir);
                LUMINANCE_WEIGHTS[0]
                    .mul_add(r, LUMINANCE_WEIGHTS[1].mul_add(g, LUMINANCE_WEIGHTS[2] * b))
            }
        }
    }
}

/// The `index`-th of [`HEMISPHERE_DIRECTIONS`] directions spread evenly by solid angle
/// over the upper hemisphere (a Fibonacci spiral in height and azimuth).
pub(super) fn hemisphere_direction(index: usize) -> Vec3 {
    let y = (index as f32 + 0.5) / HEMISPHERE_DIRECTIONS as f32;
    let ring_radius = y.mul_add(-y, 1.0).max(0.0).sqrt();
    let (sin_azimuth, cos_azimuth) = (index as f32 * GOLDEN_ANGLE_RAD).sin_cos();
    Vec3::new(ring_radius * cos_azimuth, y, ring_radius * sin_azimuth)
}

/// The preset whose radiance and rule the metrics score under. The ASET contrast view is a
/// diagnostic false-colour map, not an illumination: it is scored as the grading standard
/// (`IsoHemisphere`), so switching to it never moves the numbers.
const fn scoring_preset(preset: LightingPreset) -> LightingPreset {
    match preset {
        LightingPreset::Aset => LightingPreset::IsoHemisphere,
        other => other,
    }
}

/// The median radiance over the reference directions.
fn ambient_level(source: &RadianceSource<'_>) -> f32 {
    let mut samples: [f32; HEMISPHERE_DIRECTIONS] =
        std::array::from_fn(|index| source.radiance(hemisphere_direction(index)));
    samples.sort_by(f32::total_cmp);
    samples[HEMISPHERE_DIRECTIONS / 2]
}

/// Whether an exit direction sees a light source, for one environment and light pose.
///
/// Built once per evaluation: finding the ambient level costs
/// [`HEMISPHERE_DIRECTIONS`] radiance lookups.
#[derive(Clone)]
pub(super) struct ExitLighting<'a> {
    source: RadianceSource<'a>,
    /// Radiance a direction must exceed to count as lit.
    threshold: f32,
    /// Cosine of the observer head-shadow cone, see [`head_shadow_cone_cos`].
    head_shadow_cos: f32,
}

impl<'a> ExitLighting<'a> {
    /// Canonical preset data for the native mirror of environment/rig.rs. The ambient
    /// median is prepared once, preserving the exact CPU comparison threshold.
    pub(super) fn cuda_parameters(&self) -> Option<Vec<f32>> {
        let RadianceSource::Studio {
            preset,
            rig,
            spec_power,
        } = &self.source
        else {
            return None;
        };
        let params = preset.params();
        let tent = params.tent;
        let step = |e0: f32, e1: f32, x: f32| {
            let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
            t * t * (-2.0f32).mul_add(t, 3.0)
        };
        let mut data = vec![
            preset.model().gpu_id() as f32,
            params.spot_mult,
            *spec_power,
            if preset.has_ambient_fill() { 1.0 } else { 0.0 },
            tent.walls,
            tent.cards,
            tent.spark,
            tent.ground,
            tent.flat,
            self.head_shadow_cos,
            self.threshold,
            40_000.0 * step(-0.05, 0.05, rig.key_dir.y),
        ];
        data.extend(rig.key_dir.to_array());
        data.extend(rig.fill_dir.to_array());
        for direction in rig.ring_dirs {
            data.extend(direction.to_array());
        }
        Some(data)
    }
    /// The lighting of `environment`. A studio environment's exposure and backdrop are
    /// ignored: neither changes which directions stand out from the ambient level, and the
    /// backdrop (the lit tray under the stone) is not part of the scoring radiance -- the
    /// metrics score the cut under the lighting model, not under the user's tray. Its
    /// `head_shadow_deg` sets the metrics' head-shadow cone for the lit models.
    pub(super) fn new(environment: EnvironmentSource<'a>) -> Self {
        let head_shadow_cos = head_shadow_cone_cos(
            match environment {
                EnvironmentSource::Studio { preset, .. } => preset.model() == LightingModel::Studio,
                EnvironmentSource::HdrMap(_) => true,
            },
            environment.head_shadow_deg(),
        );
        let (source, uniform) = match environment {
            EnvironmentSource::Studio {
                preset,
                light_yaw,
                light_pitch,
                ..
            } => {
                let preset = scoring_preset(preset);
                (
                    RadianceSource::Studio {
                        preset,
                        rig: Box::new(StudioRig::new(light_yaw, light_pitch)),
                        spec_power: studio_spec_power(preset, PROBE_WAVELENGTH_NM),
                    },
                    // Only the Studio rigs have pinpoint sources; every other model (tent,
                    // dome, ISO hemisphere, and any later one) scores "light return".
                    preset.model() != LightingModel::Studio,
                )
            }
            EnvironmentSource::HdrMap(map) => (RadianceSource::Hdr(map), false),
        };
        let factor = if uniform {
            UNIFORM_FRACTION_OF_AMBIENT
        } else {
            SOURCE_OVER_AMBIENT
        };
        let threshold = (ambient_level(&source) * factor).max(f32::MIN_POSITIVE);
        Self {
            source,
            threshold,
            head_shadow_cos,
        }
    }

    /// Cosine of the observer's head-shadow cone: exits closer than this to the eye
    /// direction are lost to the observer's own shadow.
    #[cfg(test)]
    pub(super) const fn head_shadow_cos(&self) -> f32 {
        self.head_shadow_cos
    }

    /// Whether a ray leaving the stone along unit direction `exit_dir` sees a light source.
    #[cfg(test)]
    pub(super) fn is_illuminated(&self, exit_dir: Vec3) -> bool {
        self.source.radiance(exit_dir) > self.threshold
    }
}
