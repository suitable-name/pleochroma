//! Fragment lighting (beam guide §11.3–11.4).
//!
//! The tracer's own environment function is integrated over an image-space fragment.
//!
//! The integration uses a smoothness test, quadrisection, the 7-point rule, HDR subdivision to ½
//! texel and the exact sun disc.
//!
//! The integrand at an image point `x` and node `k` is `I(x, λ_k) · L(ω(x, λ_k), λ_k)`: the
//! Wachspress-interpolated weight and exit direction of the leaf, and the radiance of
//! [`sample_environment_channels`] with the per-point observer `−camera_dir(x)`. No lighting
//! model lives here; head shadow, tray and horizon blend are part of the tracer's function, so
//! the smoothness test sees them.

use std::sync::atomic::{AtomicBool, Ordering};

use glam::{DMat3, DVec2, DVec3, Vec3};

use super::{
    beam::{GROUP, Leaf},
    geom::{
        circle::{sun_disc_preimage, sun_disc_preimage_jacobian},
        polygon::{ConvexPolygon, MAX_VERTICES},
    },
};
use crate::optics::{
    raytracer::{
        Camera, EnvironmentSource, LightingModel,
        environment::{SUN_DISC_COS, SUN_RADIANCE, horizon_blend, sample_environment_channels},
        studio_spec_power,
    },
    studio_rig::StudioRig,
};

/// How far outside the disc the sky part is evaluated for a direction inside it (a factor on the
/// disc radius). The sky is smooth across the disc, so the shift is far below any tolerance.
const SKY_NUDGE: f64 = 1.05;
/// Slack on the "inside the disc" test for the sky shift, so every direction the tracer counts as
/// sun is shifted despite `f32` rounding of the dot product.
const SKY_NUDGE_BAND: f32 = 1e-6;

/// `σ_L` of the studio model, in degrees (§11.3 table).
const SIGMA_STUDIO_DEG: f32 = 1.0;
/// `σ_L` of the light-tent model, degrees.
const SIGMA_TENT_DEG: f32 = 0.5;
/// `σ_L` of the daylight-sun model away from the disc, degrees.
const SIGMA_SUN_DEG: f32 = 0.1;
/// `σ_L` of the smooth models (iso hemisphere, daylight dome, ASET), degrees.
const SIGMA_SMOOTH_DEG: f32 = 2.0;
/// The largest `τ_ω` (§11.2).
const TAU_OMEGA_MAX: f32 = 1e-4;

/// `√15`, for the 7-point rule.
const SQRT_15: f64 = 3.872_983_346_207_417;
/// The first ring abscissa of the 7-point rule, `(6 − √15) / 21`.
const RADON_A1: f64 = (6.0 - SQRT_15) / 21.0;
/// The second ring abscissa of the 7-point rule, `(6 + √15) / 21`.
const RADON_A2: f64 = (6.0 + SQRT_15) / 21.0;
/// The third barycentric coordinate of the first ring, `1 − 2 a1`.
const RADON_B1: f64 = 1.0 - 2.0 * RADON_A1;
/// The third barycentric coordinate of the second ring, `1 − 2 a2`.
const RADON_B2: f64 = 1.0 - 2.0 * RADON_A2;
/// The weight (fraction of the triangle area) of the centroid point.
const RADON_W0: f64 = 9.0 / 40.0;
/// The weight of each point of the first ring, `(155 − √15) / 1200`.
const RADON_W1: f64 = (155.0 - SQRT_15) / 1200.0;
/// The weight of each point of the second ring, `(155 + √15) / 1200`.
const RADON_W2: f64 = (155.0 + SQRT_15) / 1200.0;

/// Radon's 7-point rule for a triangle, exact for polynomials up to degree 5: the barycentric
/// coordinates of each point and its weight (a fraction of the triangle area; they sum to one).
pub const RADON_7: [([f64; 3], f64); 7] = [
    ([1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0], RADON_W0),
    ([RADON_A1, RADON_A1, RADON_B1], RADON_W1),
    ([RADON_A1, RADON_B1, RADON_A1], RADON_W1),
    ([RADON_B1, RADON_A1, RADON_A1], RADON_W1),
    ([RADON_A2, RADON_A2, RADON_B2], RADON_W2),
    ([RADON_A2, RADON_B2, RADON_A2], RADON_W2),
    ([RADON_B2, RADON_A2, RADON_A2], RADON_W2),
];

/// The deepest fragment subdivision the integrator ever uses.
///
/// It mirrors `gpu::beam::fragments::MAX_REFINE_LEVELS` (that module is behind the `gpu` feature,
/// so the value is repeated here), so a large supplied depth cannot make the CPU subdivide deeper
/// than the GPU.
pub const MAX_FRAGMENT_LEVELS: u32 = 6;

/// The lighting of a frame, fixed for all its fragments.
#[derive(Clone, Copy)]
pub struct LightingScene<'a> {
    /// The environment the path tracer shades escapes with.
    pub environment: EnvironmentSource<'a>,
    /// The studio rig of the light pose (`None` for an HDR map), built once per frame.
    pub rig: Option<StudioRig>,
    /// The camera, for the per-point observer direction `−camera_dir(x)` (§11.1).
    pub camera: &'a Camera,
    /// The image size in pixels (the camera's screen mapping).
    pub size: (u32, u32),
    /// `τ_L`, the smoothness tolerance (§11.4).
    pub tolerance: f32,
    /// The deepest fragment subdivision (§11.4), capped at `MAX_FRAGMENT_LEVELS` when used.
    pub max_levels: u32,
    /// The spectral node count `N_λ`.
    pub n_lambda: u32,
}

/// One fragment to light: a convex image-space piece of one leaf, and the 8-node group of the
/// leaf's interval to evaluate.
#[derive(Clone, Copy)]
pub struct FragmentInput<'a> {
    /// The leaf the fragment belongs to (its vertex directions and weights).
    pub leaf: &'a Leaf,
    /// The piece, in image space, inside the leaf's image polygon and one pixel cell.
    pub polygon: &'a ConvexPolygon,
    /// The 8-node group of the leaf's interval (`SpectralInterval::group_lambdas`).
    pub group: usize,
}

/// What one fragment integration did, for statistics and tests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FragmentWork {
    /// Evaluation points of the integrand (each is one Wachspress interpolation and one call of
    /// the environment function, or one per node for a dispersed exit).
    pub evaluations: u32,
    /// The deepest subdivision level reached (0 = the fragment itself).
    pub deepest: u32,
    /// Pieces integrated by the centroid rule.
    pub centroid_pieces: u32,
    /// Pieces integrated by the 7-point rule.
    pub rule_pieces: u32,
}

/// `σ_L`, the angular scale of the sharpest feature of the lighting, in radians (§11.3 table).
///
/// Studio rigs: 1°. Light tent: 0.5°. Daylight sun: 0.1° away from the disc (the disc itself is
/// integrated exactly). Iso hemisphere, daylight dome and ASET: 2°. An HDR map: half a texel,
/// `180° / width`.
#[must_use]
pub fn sigma_l(environment: &EnvironmentSource<'_>) -> f32 {
    match environment {
        EnvironmentSource::HdrMap { map, .. } => {
            let width = px_f32(u32::try_from(map.width()).unwrap_or(u32::MAX)).max(1.0);
            std::f32::consts::PI / width
        }
        EnvironmentSource::Studio { preset, .. } => match preset.model() {
            LightingModel::Studio => SIGMA_STUDIO_DEG,
            LightingModel::LightTent => SIGMA_TENT_DEG,
            LightingModel::DaylightSun => SIGMA_SUN_DEG,
            LightingModel::IsoHemisphere | LightingModel::DaylightDome | LightingModel::Aset => {
                SIGMA_SMOOTH_DEG
            }
        }
        .to_radians(),
    }
}

/// `τ_ω = min(1e-4, σ_L / 20)`, the exit-direction tolerance leaves are built with (§11.2).
#[must_use]
pub fn tau_omega(environment: &EnvironmentSource<'_>) -> f32 {
    (sigma_l(environment) / 20.0).min(TAU_OMEGA_MAX)
}

/// A pixel count or image size as `f32`; sizes are far below 2^24, so the cast is exact.
#[expect(
    clippy::cast_precision_loss,
    reason = "image sizes and texel counts are far below 2^24, exactly representable"
)]
const fn px_f32(v: u32) -> f32 {
    v as f32
}

/// An image-space `f64` as `f32` for the tracer's `f32` camera and environment code.
#[expect(
    clippy::cast_possible_truncation,
    reason = "image coordinates and radii are far inside the f32 range; the tracer works in f32"
)]
const fn to_f32(v: f64) -> f32 {
    v as f32
}

/// The Wachspress coordinates of a piece's corners and, last, of its centre.
type PieceCoordinates = [[f64; MAX_VERTICES]; MAX_VERTICES + 1];

/// The integrand at one evaluation point: its per-slot values and the direction of slot 0.
#[derive(Clone, Copy)]
struct Sample {
    /// `I · glare · L` per slot (zero past the interval's last node).
    value: [f32; GROUP],
    /// The unit exit direction of slot 0, for the HDR extent test.
    dir: Vec3,
}

impl Sample {
    /// The all-zero sample.
    const ZERO: Self = Self {
        value: [0.0; GROUP],
        dir: Vec3::ZERO,
    };
}

/// The sun of a `DaylightSun` scene: its direction, disc size and per-slot radiance.
struct Sun {
    /// The rig's key direction (the disc centre), as the tracer's `f32` unit vector.
    key: Vec3,
    /// The same direction in `f64`, normalised.
    dir: DVec3,
    /// `tan` of the disc's angular radius (the gnomonic radius).
    tan_radius: f64,
    /// The disc's angular radius, radians.
    radius: f64,
    /// The chord length of the disc radius on the unit sphere.
    chord: f64,
    /// `sin` and `cos` of the sky-shift angle (`SKY_NUDGE × radius`).
    nudge: (f32, f32),
    /// The sun's radiance per slot: `SUN_RADIANCE · horizon_blend · spec_power · exposure`.
    radiance: [f32; GROUP],
}

impl Sun {
    /// The sun of `scene`, when its environment has one that is lit.
    fn new(scene: &LightingScene<'_>, lambdas: &[f32; GROUP], valid: usize) -> Option<Self> {
        let EnvironmentSource::Studio {
            preset, exposure, ..
        } = scene.environment
        else {
            return None;
        };
        if preset.model() != LightingModel::DaylightSun {
            return None;
        }
        let key = scene.rig.as_ref()?.key_dir;
        let factor = SUN_RADIANCE * horizon_blend(key);
        if factor.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return None;
        }
        let cos = f64::from(SUN_DISC_COS);
        let sin = cos.mul_add(-cos, 1.0).sqrt();
        let radius = sin.atan2(cos);
        let shifted = radius * SKY_NUDGE;
        let radiance = std::array::from_fn(|k| {
            if k < valid {
                factor * (studio_spec_power(preset, lambdas[k]) * exposure)
            } else {
                0.0
            }
        });
        Some(Self {
            key,
            dir: key.as_dvec3().try_normalize()?,
            tan_radius: sin / cos,
            radius,
            chord: 2.0 * (0.5 * radius).sin(),
            nudge: (to_f32(shifted.sin()), to_f32(shifted.cos())),
            radiance,
        })
    }
}

/// The state of one fragment integration.
struct Integrator<'s, 'a> {
    /// The frame's lighting.
    scene: &'s LightingScene<'a>,
    /// The leaf being lit.
    leaf: &'s Leaf,
    /// The leaf's nodes per vertex.
    nodes: usize,
    /// The first node (relative to the interval) of the group.
    base: usize,
    /// The number of live slots of the group (`1..=8`).
    valid: usize,
    /// The group's node centres.
    lambdas: [f32; GROUP],
    /// The glare factor, `1` unless the leaf is a glare leaf.
    glare: f32,
    /// `σ_L`, radians.
    sigma: f32,
    /// Whether the environment is an HDR map.
    hdr: bool,
    /// The sun, for `DaylightSun`.
    sun: Option<Sun>,
    /// For a reflection leaf, the exact map from `(sx, sy, 1)` to the unnormalised direction.
    reflect_map: Option<DMat3>,
    /// The work counters.
    work: FragmentWork,
    /// The cancel token, polled once per piece.
    cancel: &'s AtomicBool,
}

/// A token that is never set, for the callers that cannot be cancelled.
static NEVER_CANCELLED: AtomicBool = AtomicBool::new(false);

/// Adds `b` to `a` slot by slot.
fn add_into(a: &mut [f64; GROUP], b: &[f64; GROUP]) {
    for (x, y) in a.iter_mut().zip(b) {
        *x += *y;
    }
}

/// `v · s` in `f64`.
fn scaled(v: &[f32; GROUP], s: f64) -> [f64; GROUP] {
    v.map(|x| f64::from(x) * s)
}

/// The point of the triangle `(a, b, c)` with barycentric coordinates `w`.
fn bary(a: DVec2, b: DVec2, c: DVec2, w: [f64; 3]) -> DVec2 {
    a * w[0] + b * w[1] + c * w[2]
}

/// Whether the environment can be evaluated: a studio environment needs its rig.
const fn environment_ready(scene: &LightingScene<'_>) -> bool {
    !matches!(scene.environment, EnvironmentSource::Studio { .. }) || scene.rig.is_some()
}

/// The exact map `(sx, sy, 1) → direction` of a reflection leaf, fitted through the three
/// vertices of the largest triangle (the directions are affine in the image, §11.2).
fn fit_direction_map(leaf: &Leaf, node: usize) -> Option<DMat3> {
    let (pts, count) = leaf.image.points_2d();
    let nodes = leaf.nodes();
    let dir_at = |v: usize| -> Option<DVec3> {
        leaf.dirs
            .get(v * nodes + node)
            .copied()
            .map(Vec3::as_dvec3)
            .filter(|dir| dir.is_finite())
    };
    let mut best: Option<(f64, [usize; 3])> = None;
    for i in 0..count {
        for j in i + 1..count {
            for k in j + 1..count {
                let twice = (pts[j] - pts[i]).perp_dot(pts[k] - pts[i]).abs();
                if best.is_none_or(|(best_twice, _)| twice > best_twice) {
                    best = Some((twice, [i, j, k]));
                }
            }
        }
    }
    let (twice, [i, j, k]) = best?;
    if twice <= f64::MIN_POSITIVE {
        return None;
    }
    let corners = DMat3::from_cols(pts[i].extend(1.0), pts[j].extend(1.0), pts[k].extend(1.0));
    let dirs = DMat3::from_cols(dir_at(i)?, dir_at(j)?, dir_at(k)?);
    let map = dirs * corners.inverse();
    map.is_finite().then_some(map)
}

/// The least-squares affine fit of directions over sample points, as
/// `(value at the centroid, d/dsx, d/dsy)`; `None` when the points do not span the plane.
///
/// The offsets from `centre` are divided by the polygon's extent before the normal equations are
/// formed, so the degeneracy test is scale invariant: a tiny, well-shaped
/// polygon fits as well as a large one, and only a polygon that is truly flat (collinear points)
/// fails. The returned slopes are per screen unit.
fn fit_affine(centre: DVec2, samples: &[(DVec2, DVec3)]) -> Option<(DVec3, DVec3, DVec3)> {
    let scale = samples
        .iter()
        .map(|&(x, _)| (x - centre).abs().max_element())
        .fold(0.0, f64::max);
    if !(scale.is_finite() && scale > f64::MIN_POSITIVE) {
        return None;
    }
    let mut cols = [DVec3::ZERO; 3];
    let mut rhs = [DVec3::ZERO; 3];
    for &(x, w) in samples {
        let d = (x - centre) / scale;
        let row = DVec3::new(d.x, d.y, 1.0);
        cols[0] += row * row.x;
        cols[1] += row * row.y;
        cols[2] += row * row.z;
        rhs[0] += row * w.x;
        rhs[1] += row * w.y;
        rhs[2] += row * w.z;
    }
    let normal = DMat3::from_cols(cols[0], cols[1], cols[2]);
    let trace = normal.x_axis.x + normal.y_axis.y + normal.z_axis.z;
    let det = normal.determinant();
    if !(det.is_finite() && trace > 0.0 && det.abs() > 1e-14 * trace.powi(3)) {
        return None;
    }
    let inv = normal.inverse();
    let (cx, cy, cz) = (inv * rhs[0], inv * rhs[1], inv * rhs[2]);
    Some((
        DVec3::new(cx.z, cy.z, cz.z),
        DVec3::new(cx.x, cy.x, cz.x) / scale,
        DVec3::new(cx.y, cy.y, cz.y) / scale,
    ))
}

impl<'s, 'a> Integrator<'s, 'a> {
    /// Prepares the integration of `group` of `leaf`, or `None` when nothing is to be lit.
    fn new(
        scene: &'s LightingScene<'a>,
        fragment: &FragmentInput<'s>,
        cancel: &'s AtomicBool,
    ) -> Option<Self> {
        let leaf = fragment.leaf;
        let nodes = leaf.nodes();
        let base = fragment.group.checked_mul(GROUP)?;
        let valid = nodes.saturating_sub(base).min(GROUP);
        let vertices = leaf.image.len();
        let needed = vertices * nodes;
        if valid == 0
            || leaf.dirs.len() < needed
            || leaf.weights.len() < needed
            || !environment_ready(scene)
        {
            return None;
        }
        let lambdas = leaf.spectrum.group_lambdas(fragment.group, scene.n_lambda);
        let sun = Sun::new(scene, &lambdas, valid);
        let reflect_map = if sun.is_some() && leaf.reflection {
            fit_direction_map(leaf, base)
        } else {
            None
        };
        Some(Self {
            scene,
            leaf,
            nodes,
            base,
            valid,
            lambdas,
            glare: if leaf.glare {
                scene.environment.surface_glare()
            } else {
                1.0
            },
            sigma: sigma_l(&scene.environment),
            hdr: matches!(scene.environment, EnvironmentSource::HdrMap { .. }),
            sun,
            reflect_map,
            work: FragmentWork::default(),
            cancel,
        })
    }

    /// The interpolated direction (unnormalised, `f64`) and weight of `slot` at the Wachspress
    /// coordinates `w`.
    fn field_with(&self, w: &[f64; MAX_VERTICES], slot: usize) -> (DVec3, f64) {
        let node = self.base + slot;
        let mut dir = DVec3::ZERO;
        let mut weight = 0.0_f64;
        for (v, &wv) in w.iter().enumerate().take(self.leaf.image.len()) {
            let idx = v * self.nodes + node;
            dir += self
                .leaf
                .dirs
                .get(idx)
                .copied()
                .map_or(DVec3::ZERO, Vec3::as_dvec3)
                * wv;
            let stokes = f64::from(self.leaf.weights.get(idx).copied().unwrap_or(0.0));
            weight = stokes.mul_add(wv, weight);
        }
        (dir, weight.max(0.0))
    }

    /// The unit direction (`f64`) and weight of `slot` at the Wachspress coordinates `w` of an
    /// image point.
    fn field_at(&self, slot: usize, w: &[f64; MAX_VERTICES]) -> (DVec3, f64) {
        let (dir, weight) = self.field_with(w, slot);
        (dir.try_normalize().unwrap_or(DVec3::ZERO), weight)
    }

    /// The Wachspress coordinates of the corners `pts` and of the `centre` of a piece, the centre
    /// last. They do not depend on the slot, so the slots of a refracted fragment share them.
    fn piece_coordinates(&self, pts: &[DVec2], centre: DVec2) -> PieceCoordinates {
        let mut out = [[0.0_f64; MAX_VERTICES]; MAX_VERTICES + 1];
        for (w, &p) in out.iter_mut().zip(pts) {
            *w = self.leaf.image.wachspress(p);
        }
        out[pts.len().min(MAX_VERTICES)] = self.leaf.image.wachspress(centre);
        out
    }

    /// [`Self::piece_coordinates`] of the corners and the centroid of `poly`.
    fn coordinates_of(&self, poly: &ConvexPolygon) -> PieceCoordinates {
        let (pts, n) = poly.points_2d();
        let c = poly.centroid();
        self.piece_coordinates(&pts[..n], DVec2::new(c.x, c.y))
    }

    /// The unit exit directions (as the tracer's `f32`) and weights of every slot at `x`.
    fn interpolate(&self, x: DVec2) -> ([Vec3; GROUP], [f32; GROUP]) {
        let w = self.leaf.image.wachspress(x);
        let mut dirs = [Vec3::ZERO; GROUP];
        let mut weights = [0.0_f32; GROUP];
        for (slot, (d, i)) in dirs
            .iter_mut()
            .zip(weights.iter_mut())
            .enumerate()
            .take(self.valid)
        {
            let (dir, weight) = self.field_with(&w, slot);
            *d = dir.as_vec3().normalize_or_zero();
            *i = to_f32(weight);
        }
        (dirs, weights)
    }

    /// The direction to look the sky part up at: inside the sun disc it is moved radially to just
    /// outside it, so the tracer's function returns the sky without the disc.
    fn sky_dir(&self, d: Vec3) -> Vec3 {
        let Some(sun) = &self.sun else {
            return d;
        };
        if d.dot(sun.key) < SUN_DISC_COS - SKY_NUDGE_BAND {
            return d;
        }
        let tangent = (d - sun.key * d.dot(sun.key))
            .try_normalize()
            .unwrap_or_else(|| sun.key.any_orthonormal_vector());
        (sun.key * sun.nudge.1 + tangent * sun.nudge.0).normalize()
    }

    /// The integrand at the image point `x`.
    fn sample(&mut self, x: DVec2) -> Sample {
        self.work.evaluations = self.work.evaluations.saturating_add(1);
        let (dirs, weights) = self.interpolate(x);
        let (w, h) = (px_f32(self.scene.size.0), px_f32(self.scene.size.1));
        let ray = self
            .scene
            .camera
            .generate_ray(to_f32(x.x), to_f32(x.y), w, h, 0.0, 0.0);
        let observer = -ray.dir;
        let mut value = [0.0_f32; GROUP];
        let live = &dirs[..self.valid];
        if live.iter().all(|d| *d == live[0]) {
            let radiance = self.radiance_row(live[0], observer);
            for ((v, &i), &r) in value
                .iter_mut()
                .zip(&weights)
                .zip(&radiance)
                .take(self.valid)
            {
                *v = i * self.glare * r;
            }
        } else {
            for (slot, v) in value.iter_mut().enumerate().take(self.valid) {
                let r = self.radiance_single(dirs[slot], self.lambdas[slot], observer);
                *v = weights[slot] * self.glare * r;
            }
        }
        Sample {
            value,
            dir: dirs[0],
        }
    }

    /// The radiance of all eight node wavelengths along the shared direction `dir`.
    fn radiance_row(&self, dir: Vec3, observer: Vec3) -> [f32; GROUP] {
        if dir == Vec3::ZERO {
            return [0.0; GROUP];
        }
        sample_environment_channels(
            self.scene.environment,
            self.sky_dir(dir),
            &self.lambdas,
            self.scene.rig.as_ref(),
            observer,
        )
    }

    /// The radiance of one wavelength along its own direction (a dispersed exit).
    fn radiance_single(&self, dir: Vec3, lambda_nm: f32, observer: Vec3) -> f32 {
        if dir == Vec3::ZERO {
            return 0.0;
        }
        sample_environment_channels(
            self.scene.environment,
            self.sky_dir(dir),
            &[lambda_nm],
            self.scene.rig.as_ref(),
            observer,
        )[0]
    }

    /// Whether the spread of every live slot over the corners and the centroid is within
    /// `τ_L · max` (§11.4 step 1).
    fn is_smooth(&self, corners: &[Sample], centre: &Sample) -> bool {
        (0..self.valid).all(|k| {
            let (mut lo, mut hi) = (centre.value[k], centre.value[k]);
            for s in corners {
                lo = lo.min(s.value[k]);
                hi = hi.max(s.value[k]);
            }
            hi - lo <= self.scene.tolerance * hi
        })
    }

    /// The angular extent of a piece (twice the largest corner-to-centroid chord), radians.
    fn extent(corners: &[Sample], centre: &Sample) -> f32 {
        2.0 * corners
            .iter()
            .map(|s| (s.dir - centre.dir).length())
            .fold(0.0, f32::max)
    }

    /// Integrates the sky part of the piece `poly` (§11.4 steps 1, 2 and 4).
    ///
    /// It returns `None` once the cancel token is set.
    fn piece(&mut self, poly: &ConvexPolygon, level: u32) -> Option<[f64; GROUP]> {
        if self.cancel.load(Ordering::Relaxed) {
            return None;
        }
        self.work.deepest = self.work.deepest.max(level);
        let (pts, n) = poly.points_2d();
        let area = poly.area();
        let c = poly.centroid();
        let centre_pt = DVec2::new(c.x, c.y);
        let centre = self.sample(centre_pt);
        let mut corners = [Sample::ZERO; MAX_VERTICES];
        for (s, &p) in corners.iter_mut().zip(&pts[..n]) {
            *s = self.sample(p);
        }
        let corners = &corners[..n];
        let smooth = self.is_smooth(corners, &centre);
        let half_texel = self.hdr && Self::extent(corners, &centre) <= self.sigma;
        if smooth && (!self.hdr || half_texel) {
            self.work.centroid_pieces = self.work.centroid_pieces.saturating_add(1);
            return Some(scaled(&centre.value, area));
        }
        if !half_texel && level < self.scene.max_levels.min(MAX_FRAGMENT_LEVELS) {
            let pieces = poly.quadrisect();
            if pieces.len() > 1 {
                let mut sum = [0.0_f64; GROUP];
                for part in &pieces {
                    add_into(&mut sum, &self.piece(part, level + 1)?);
                }
                return Some(sum);
            }
        }
        Some(self.rule7(centre_pt, &pts[..n]))
    }

    /// The 7-point rule over the fan of triangles from `centre` to the polygon's edges.
    fn rule7(&mut self, centre: DVec2, pts: &[DVec2]) -> [f64; GROUP] {
        self.work.rule_pieces = self.work.rule_pieces.saturating_add(1);
        let mut sum = [0.0_f64; GROUP];
        for (i, &a) in pts.iter().enumerate() {
            let b = pts[(i + 1) % pts.len()];
            let area = 0.5 * (a - centre).perp_dot(b - centre).abs();
            for (w, weight) in RADON_7 {
                let s = self.sample(bary(centre, a, b, w));
                add_into(&mut sum, &scaled(&s.value, weight * area));
            }
        }
        sum
    }

    /// The sun disc's contribution over `poly`, per slot (§11.4 step 3): `sun radiance × covered
    /// area`, weighted by the leaf's `I`.
    ///
    /// It returns `None` once the cancel token is set.
    fn sun_disc(&self, poly: &ConvexPolygon) -> Option<[f64; GROUP]> {
        let mut out = [0.0_f64; GROUP];
        let Some(sun) = &self.sun else {
            return Some(out);
        };
        let (pts, n) = poly.points_2d();
        let c = poly.centroid();
        let centre = DVec2::new(c.x, c.y);
        if let Some(map) = self.reflect_map {
            let Some(ellipse) = sun_disc_preimage(map, sun.dir, sun.tan_radius) else {
                return Some(out);
            };
            let area = ellipse.polygon_area(&pts[..n]).min(poly.area());
            if area <= 0.0 {
                return Some(out);
            }
            let (_, weights) = self.interpolate(centre);
            for ((o, &i), &s) in out.iter_mut().zip(&weights).zip(&sun.radiance) {
                *o = f64::from(i) * f64::from(self.glare) * f64::from(s) * area;
            }
        } else {
            let coordinates = self.piece_coordinates(&pts[..n], centre);
            for (slot, o) in out.iter_mut().enumerate().take(self.valid) {
                let covered = self.sun_refracted(sun, slot, poly, (0, &coordinates))?;
                *o = f64::from(self.glare) * f64::from(sun.radiance[slot]) * covered;
            }
        }
        Some(out)
    }

    /// `∫ I` over the part of `poly` the sun disc covers for a refracted leaf's `slot`, from the
    /// local Jacobian of the exit direction; the piece is quadrisected until the affine fit of
    /// the direction is within `τ_L` of the disc radius (or `max_levels`).
    ///
    /// `coordinates` are the piece's Wachspress coordinates, computed once by the caller for all
    /// slots.
    ///
    /// It returns `None` once the cancel token is set.
    fn sun_refracted(
        &self,
        sun: &Sun,
        slot: usize,
        poly: &ConvexPolygon,
        (level, coordinates): (u32, &PieceCoordinates),
    ) -> Option<f64> {
        if self.cancel.load(Ordering::Relaxed) {
            return None;
        }
        let (pts, n) = poly.points_2d();
        let c = poly.centroid();
        let centre = DVec2::new(c.x, c.y);
        let mut samples = [(DVec2::ZERO, DVec3::ZERO); MAX_VERTICES + 1];
        for ((s, &p), w) in samples.iter_mut().zip(&pts[..n]).zip(coordinates) {
            *s = (p, self.field_at(slot, w).0);
        }
        let (omega_c, weight_c) = self.field_at(slot, &coordinates[n.min(MAX_VERTICES)]);
        samples[n] = (centre, omega_c);
        let samples = &samples[..=n];
        let spread = samples[..n]
            .iter()
            .map(|(_, w)| (*w - omega_c).length())
            .fold(0.0, f64::max);
        if (omega_c - sun.dir).length() > 2.0f64.mul_add(spread, sun.chord) + 1e-9 {
            return Some(0.0);
        }
        let Some((omega0, dw_d_screen_x, dw_d_screen_y)) = fit_affine(centre, samples) else {
            return Some(sample_share_area(sun, samples, poly.area() * weight_c));
        };
        let residual = samples
            .iter()
            .map(|&(x, w)| {
                let d = x - centre;
                let linear = dw_d_screen_y.mul_add(
                    DVec3::splat(d.y),
                    dw_d_screen_x.mul_add(DVec3::splat(d.x), omega0),
                );
                (linear - w).length()
            })
            .fold(0.0, f64::max);
        if residual > f64::from(self.scene.tolerance) * sun.radius
            && level < self.scene.max_levels.min(MAX_FRAGMENT_LEVELS)
        {
            let pieces = poly.quadrisect();
            if pieces.len() > 1 {
                return pieces
                    .iter()
                    .map(|part| {
                        let coordinates = self.coordinates_of(part);
                        self.sun_refracted(sun, slot, part, (level + 1, &coordinates))
                    })
                    .sum::<Option<f64>>();
            }
        }
        let Some(ellipse) = sun_disc_preimage_jacobian(
            centre,
            omega0,
            dw_d_screen_x,
            dw_d_screen_y,
            sun.dir,
            sun.tan_radius,
        ) else {
            return Some(sample_share_area(sun, samples, poly.area() * weight_c));
        };
        Some(ellipse.polygon_area(&pts[..n]).min(poly.area()) * weight_c)
    }
}

/// `whole × (share of the sample directions inside the sun disc)`.
///
/// It is the fallback when the affine fit of a refracted fragment or its preimage ellipse
/// degenerates: a fragment whose every sample points into the disc is
/// wholly covered, a mixed one gets the share of samples inside, and only a fragment with no
/// sample inside gets zero. The GPU twin (`bm_sun_cover`) applies the same rule.
fn sample_share_area(sun: &Sun, samples: &[(DVec2, DVec3)], whole: f64) -> f64 {
    let inside = samples
        .iter()
        .filter(|(_, w)| (*w - sun.dir).length() <= sun.chord)
        .count();
    #[expect(
        clippy::cast_precision_loss,
        reason = "the sample count is at most MAX_VERTICES + 1"
    )]
    let share = inside as f64 / samples.len().max(1) as f64;
    whole * share
}

/// `v` as `f32` per slot, with the slots past `valid` zero.
fn to_f32_row(v: &[f64; GROUP], valid: usize) -> [f32; GROUP] {
    std::array::from_fn(|k| if k < valid { to_f32(v[k]) } else { 0.0 })
}

/// The sun disc's term alone, per slot.
///
/// It is `Σ over the covered part of the fragment of I · sun radiance`, with `surface_glare`
/// applied for a glare leaf. [`integrate_fragment`] adds it to the sky.
#[must_use]
pub fn sun_disc_term(scene: &LightingScene<'_>, fragment: &FragmentInput<'_>) -> [f32; 8] {
    Integrator::new(scene, fragment, &NEVER_CANCELLED).map_or([0.0; GROUP], |it| {
        it.sun_disc(fragment.polygon)
            .map_or([0.0; GROUP], |sun| to_f32_row(&sun, it.valid))
    })
}

/// [`integrate_fragment`] plus what the integration did (evaluations, deepest level, rule counts).
#[must_use]
pub fn integrate_fragment_counted(
    scene: &LightingScene<'_>,
    fragment: &FragmentInput<'_>,
) -> ([f32; 8], FragmentWork) {
    integrate_fragment_work(scene, fragment, &NEVER_CANCELLED)
        .unwrap_or_else(|| ([0.0; GROUP], FragmentWork::default()))
}

/// [`integrate_fragment`] that stops when `cancel` is set, or `None` if it did.
///
/// With the token unset the result equals [`integrate_fragment`] bit for bit.
#[must_use]
pub fn integrate_fragment_cancellable(
    scene: &LightingScene<'_>,
    fragment: &FragmentInput<'_>,
    cancel: &AtomicBool,
) -> Option<[f32; 8]> {
    integrate_fragment_work(scene, fragment, cancel).map(|(row, _)| row)
}

/// The integration behind the public entry points; `None` when `cancel` is set.
fn integrate_fragment_work(
    scene: &LightingScene<'_>,
    fragment: &FragmentInput<'_>,
    cancel: &AtomicBool,
) -> Option<([f32; 8], FragmentWork)> {
    let Some(mut it) = Integrator::new(scene, fragment, cancel) else {
        return Some(([0.0; GROUP], FragmentWork::default()));
    };
    let mut total = it.piece(fragment.polygon, 0)?;
    add_into(&mut total, &it.sun_disc(fragment.polygon)?);
    Some((to_f32_row(&total, it.valid), it.work))
}

/// The fragment's integral `∫_frag I(x, λ_k) · L(ω(x, λ_k), λ_k) dx` per node of the group, in
/// image-space area units (pixel area 1).
///
/// `surface_glare` is applied when the leaf's glare flag is set. Slots past the interval's last
/// node are zero.
///
/// The CIE weights are applied by the caller through `color::integrate_nodes_to_xyz`.
#[must_use]
pub fn integrate_fragment(scene: &LightingScene<'_>, fragment: &FragmentInput<'_>) -> [f32; 8] {
    integrate_fragment_counted(scene, fragment).0
}
