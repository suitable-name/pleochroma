//! The ground (frosted) facet: a GGX rough dielectric interface.
//!
//! This replaces the cosine-weighted Lambertian bounce a frosted facet used to have (decision
//! D11). One event is the exact interface physics at a micro-normal drawn from the distribution
//! of visible normals of an anisotropic GGX lobe (Walter et al. 2007; Heitz 2018):
//!
//! 1. the branch (reflect or transmit) is drawn with the broadband macroscopic Fresnel
//!    probability `r_sel` of the hero, as before, and divides out of the throughput;
//! 2. a micro-normal `m` is drawn; the reflected (or refracted) direction `o` follows from the
//!    exact mirror (or Snell) law at `m`;
//! 3. the throughput is `F(m)/r_sel` (or `(1 − F(m))/(1 − r_sel)`) times the masking ratio
//!    `G2(i, o) / G1(i)`, divided by the tabulated directional albedo of the whole event
//!    ([`super::compensation`]) so a lossless rough interface keeps its energy; a direction that
//!    ends on the wrong side of the macro surface carries weight `0`.
//!
//! Next-event estimation evaluates the same lobe: the BSDF-sampled continuation and the light
//! sample are combined with the balance heuristic over the density of the direction GIVEN the
//! branch, exactly as the Lambertian version did over `cos / π`.
//!
//! [`GroundCtx`] is the pure core shared by the render tracer ([`apply_ground_bounce`]) and the
//! metrics tracer's frosted event, which therefore draw the same direction bit for bit.

use glam::Vec3;

use super::{
    compensation::ground_albedo,
    ggx::{GgxLobe, MIN_COSINE},
    micro::{fresnel_unpolarized, reflect_about, refract_about},
    unit_draw,
};
use crate::optics::{
    polarization::StokesVector,
    raytracer::{
        NUM_CHANNELS,
        camera::Ray,
        environment::sample_environment_for_nee,
        intersect_stone::intersect_stone_soa,
        refraction::{
            BounceRefractionGeometry, R_UNPOL_SELECT_MAX, R_UNPOL_SELECT_MIN, RayMaterialContext,
        },
        sampling::{
            BIREFRINGENT_SPLIT_STREAM, FRESNEL_BRANCH_STREAM, FROSTED_DIR_U_STREAM,
            FROSTED_DIR_V_STREAM, FROSTED_NEE_ENV_DIR_U_STREAM, FROSTED_NEE_ENV_DIR_V_STREAM,
        },
        scattering::{NeeContext, balance_heuristic, frosted_orthonormal_basis},
    },
};

/// An orthonormal frame at a facet: `n` the macro normal on the side the ray comes from, `t` the
/// local `x` axis (across the scratches) and `b = n × t` the `y` axis (along them).
#[derive(Clone, Copy, Debug)]
pub struct GroundFrame {
    /// The local `x` axis.
    pub t: Vec3,
    /// The local `y` axis.
    pub b: Vec3,
    /// The local `z` axis, the macro normal.
    pub n: Vec3,
}

impl GroundFrame {
    /// The frame at `normal`. `across` is the direction across the scratches (projected into the
    /// facet plane); without one the frame is arbitrary, which only a round lobe tolerates.
    #[must_use]
    pub fn new(normal: Vec3, across: Option<Vec3>) -> Self {
        let t = across
            .map(|t| (t - normal * normal.dot(t)).normalize_or_zero())
            .filter(|t| t.length_squared() > 0.0)
            .unwrap_or_else(|| frosted_orthonormal_basis(normal).0);
        Self {
            t,
            b: normal.cross(t),
            n: normal,
        }
    }

    /// `v` in the local frame.
    #[must_use]
    pub fn to_local(&self, v: Vec3) -> Vec3 {
        Vec3::new(v.dot(self.t), v.dot(self.b), v.dot(self.n))
    }

    /// The world vector of the local vector `v`.
    #[must_use]
    pub fn to_world(&self, v: Vec3) -> Vec3 {
        self.t * v.x + self.b * v.y + self.n * v.z
    }
}

/// What a facet event resolves a ground surface to: the lobe and the direction across the
/// scratches (`None` for a round lobe).
#[derive(Clone, Copy, Debug)]
pub struct GroundParams {
    /// The roughness widths.
    pub lobe: GgxLobe,
    /// The world direction across the scratches, when the lobe is anisotropic.
    pub across: Option<Vec3>,
}

impl GroundParams {
    /// What every frosted facet without a surface becomes: the round default lobe.
    pub const DEFAULT: Self = Self {
        lobe: GgxLobe {
            alpha_x: super::facet_surface::DEFAULT_GROUND_ALPHA,
            alpha_y: super::facet_surface::DEFAULT_GROUND_ALPHA,
        },
        across: None,
    };
}

/// The hero's view of the interface at one event.
#[derive(Clone, Copy, Debug)]
pub struct GroundInterface {
    /// The index on the incident side.
    pub n1: f32,
    /// The index on the far side.
    pub n2: f32,
    /// The cosine of the incidence angle against the macro normal.
    pub cos_i: f32,
    /// The squared sine of the refraction angle at the macro normal (above 1: total internal
    /// reflection).
    pub sin2_t: f32,
}

impl GroundInterface {
    /// The interface of a bounce geometry.
    #[must_use]
    pub(crate) const fn from_geometry(geo: &BounceRefractionGeometry) -> Self {
        Self {
            n1: geo.n1,
            n2: geo.n2,
            cos_i: geo.cos_i,
            sin2_t: geo.sin2_t,
        }
    }
}

/// One drawn ground event.
#[derive(Clone, Copy, Debug)]
pub struct GroundSample {
    /// The new propagation direction (a unit vector). For an invalid sample it is the mirror
    /// direction about the macro normal, which keeps the path in the medium it is in.
    pub dir: Vec3,
    /// `true` when the ray went through the facet into the other medium.
    pub crossed: bool,
    /// `true` when the branch drawn was reflection (also when the sample turned out invalid).
    pub reflect_branch: bool,
    /// The throughput factor: `0` for a direction on the wrong side of the surface.
    pub weight: f32,
    /// The probability of the branch drawn (the factor `path_pdf` takes).
    pub branch_prob: f32,
    /// The density of `dir` given the branch, per unit solid angle; `0` for an invalid sample.
    pub pdf: f32,
    /// `false` for a sample that was lost to shadowing or ended on the wrong side.
    pub valid: bool,
}

/// What [`GroundCtx::eval`] returns for a given outgoing direction: the throughput factor and
/// the density of the direction given the branch.
#[derive(Clone, Copy, Debug)]
pub struct GroundEval {
    /// The throughput factor a BSDF-sampled path would carry for this direction.
    pub weight: f32,
    /// The density of this direction given the branch, per unit solid angle.
    pub pdf: f32,
}

/// The pure state of one ground event: the frame, the incident direction in it, the lobe and the
/// interface constants.
#[derive(Clone, Copy, Debug)]
pub struct GroundCtx {
    frame: GroundFrame,
    lobe: GgxLobe,
    /// The incident direction (pointing away from the surface) in the local frame, with `z`
    /// bounded away from zero.
    i: Vec3,
    n1: f32,
    n2: f32,
    /// The reflect probability of the branch draw.
    r_sel: f32,
    /// The inverse of the directional albedo of the event.
    compensation: f32,
}

impl GroundCtx {
    /// The event of a ray travelling along `k_in` that meets the facet with macro normal
    /// `normal` (on the side the ray comes from).
    #[must_use]
    pub fn new(normal: Vec3, k_in: Vec3, params: GroundParams, iface: GroundInterface) -> Self {
        let frame = GroundFrame::new(normal, params.across);
        let mut i = frame.to_local(-k_in);
        i.z = i.z.max(MIN_COSINE);
        let i = i.normalize();
        let r_sel = if iface.sin2_t > 1.0 {
            R_UNPOL_SELECT_MAX
        } else {
            fresnel_unpolarized(iface.n1, iface.n2, iface.cos_i)
                .clamp(R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX)
        };
        let albedo = ground_albedo(params.lobe.alpha_mean(), i.z, iface.n2 / iface.n1);
        Self {
            frame,
            lobe: params.lobe,
            i,
            n1: iface.n1,
            n2: iface.n2,
            r_sel,
            compensation: 1.0 / albedo,
        }
    }

    /// The density of the outgoing direction `o` given the branch, for the half-vector `h` whose
    /// cosine against the incident direction is `c`.
    fn density(&self, o: Vec3, h: Vec3, c: f32, reflect: bool) -> f32 {
        let base = self.lobe.g1(self.i) * self.lobe.d(h) / self.i.z;
        if reflect {
            0.25 * base
        } else {
            let o_dot_h = o.dot(h);
            let denom = self.n1.mul_add(c, self.n2 * o_dot_h);
            if denom.abs() < 1e-12 {
                return 0.0;
            }
            base * c * self.n2 * self.n2 * o_dot_h.abs() / (denom * denom)
        }
    }

    /// The reflect-branch throughput energy `F / r_sel`, or the transmit-branch `(1 − F)/(1 − r_sel)`.
    fn branch_energy(&self, fresnel: f32, reflect: bool) -> f32 {
        if reflect {
            fresnel / self.r_sel
        } else {
            (1.0 - fresnel) / (1.0 - self.r_sel)
        }
    }

    /// Draws one event from the uniform numbers `(u1, u2)` of the micro-normal and `u_branch` of
    /// the branch.
    #[must_use]
    #[expect(
        clippy::option_if_let_else,
        reason = "both branches build a whole sample; map_or_else would bury them in closures"
    )]
    pub fn sample(&self, u1: f32, u2: f32, u_branch: f32) -> GroundSample {
        let i = self.i;
        let reflect = u_branch < self.r_sel;
        let branch_prob = if reflect {
            self.r_sel
        } else {
            1.0 - self.r_sel
        };
        let m = self.lobe.sample_visible(i, u1, u2);
        let c = i.dot(m);
        let outgoing = if reflect {
            Some(reflect_about(i, m))
        } else {
            refract_about(i, m, self.n1, self.n2)
        };
        let valid = outgoing.filter(|o| c > 0.0 && if reflect { o.z > 0.0 } else { o.z < 0.0 });
        if let Some(o) = valid {
            let o = o.normalize();
            let fresnel = fresnel_unpolarized(self.n1, self.n2, c);
            let masking = self.lobe.g2(i, o) / self.lobe.g1(i);
            GroundSample {
                dir: self.frame.to_world(o).normalize(),
                crossed: !reflect,
                reflect_branch: reflect,
                weight: self.branch_energy(fresnel, reflect) * masking * self.compensation,
                branch_prob,
                pdf: self.density(o, m, c, reflect),
                valid: true,
            }
        } else {
            GroundSample {
                dir: self.frame.to_world(Vec3::new(-i.x, -i.y, i.z)).normalize(),
                crossed: false,
                reflect_branch: reflect,
                weight: 0.0,
                branch_prob,
                pdf: 0.0,
                valid: false,
            }
        }
    }

    /// Evaluates the lobe for a given outgoing direction `o_world` on the branch `reflect`: the
    /// throughput factor and the density, or `None` when the direction is not reachable (wrong
    /// side, outside the visible normals, past the critical angle).
    #[must_use]
    pub fn eval(&self, o_world: Vec3, reflect: bool) -> Option<GroundEval> {
        let i = self.i;
        let o = self.frame.to_local(o_world);
        if reflect != (o.z > 0.0) || o.z == 0.0 {
            return None;
        }
        let half = if reflect {
            i + o
        } else {
            self.n1 * i + self.n2 * o
        };
        let len = half.length();
        if len < 1e-6 {
            return None;
        }
        let mut h = half / len;
        if h.z < 0.0 {
            h = -h;
        }
        let c = i.dot(h);
        if c <= 0.0 || (!reflect && o.dot(h) >= 0.0) {
            return None;
        }
        if !reflect && (self.n1 / self.n2).powi(2) * c.mul_add(-c, 1.0) >= 1.0 {
            return None;
        }
        let fresnel = fresnel_unpolarized(self.n1, self.n2, c);
        let masking = self.lobe.g2(i, o) / self.lobe.g1(i);
        let pdf = self.density(o, h, c, reflect);
        (pdf > 0.0).then(|| GroundEval {
            weight: self.branch_energy(fresnel, reflect) * masking * self.compensation,
            pdf,
        })
    }
}

/// Next-event estimation for the two ground outcomes whose direction lands on the EXTERIOR side
/// of the gem (a reflection while outside, a transmission while inside).
///
/// The light direction is drawn from the environment and the lobe is evaluated at it; the
/// deposit is `intensity × w × p / p_light × MIS`, where `w` and `p` are the throughput factor
/// and the branch-conditional density [`GroundCtx::eval`] returns and the MIS weight is the
/// balance heuristic of the light density against `p`. A concave stone casts a shadow probe, as
/// the Lambertian version did. Consumes the same two RNG streams.
#[expect(
    clippy::too_many_arguments,
    reason = "the lobe joined the NEE inputs of the Lambertian version (context, surface point, \
              light-sample RNG state, the branch's own throughput and the output radiance)"
)]
pub(crate) fn nee_contribution_ground(
    nee: NeeContext<'_>,
    ground: &GroundCtx,
    reflect: bool,
    origin: Vec3,
    lambdas: &[f32; NUM_CHANNELS],
    rng_seed: u32,
    bounce: u32,
    stokes: &[StokesVector; NUM_CHANNELS],
    radiance: &mut [f32; NUM_CHANNELS],
) {
    if !nee.enabled {
        return;
    }
    let u0 = unit_draw(rng_seed, bounce, FROSTED_NEE_ENV_DIR_U_STREAM);
    let u1 = unit_draw(rng_seed, bounce, FROSTED_NEE_ENV_DIR_V_STREAM);
    let Some(sample) = sample_environment_for_nee(nee.environment, u0, u1) else {
        return;
    };
    // A direction behind the surface (or off the lobe) has no density here; the BSDF-sampled
    // continuation still gets its own chance to reach it.
    let Some(eval) = ground.eval(sample.dir, reflect) else {
        return;
    };
    if !nee.tools.is_empty() {
        let probe = Ray {
            origin: origin + sample.dir * 1e-4,
            dir: sample.dir,
        };
        if intersect_stone_soa(probe, nee.plane_soa, nee.plane_soa.len(), nee.tools).is_some() {
            return;
        }
    }
    let mis_weight = balance_heuristic(sample.pdf, eval.pdf);
    if mis_weight <= 0.0 {
        return;
    }
    let common = eval.weight * eval.pdf * mis_weight / sample.pdf;
    for k in 0..NUM_CHANNELS {
        let env_k = sample.radiance(lambdas[k]);
        radiance[k] = (stokes[k].intensity() * common * env_k).mul_add(1.0, radiance[k]);
    }
}

/// The ground replacement for the specular TIR/partial-reflect/refract dispatch at a frosted
/// facet; the successor of the Lambertian `apply_frosted_bounce`, with the same contract.
///
/// `normal` is the facet's outward normal turned to face the medium the ray is in, `k_in` the
/// propagation direction of the ray, `params` the facet's lobe. Depolarizes every channel (the
/// micro-scattering scrambles the phase), multiplies `path_pdf` by the probability of the branch
/// drawn and the intensity by the throughput factor of the sample (the branch's energy fraction
/// cancels against its selection probability, so no further division belongs here).
///
/// # Return value
///
/// `(new direction, new inside_gem, is_extraordinary update, pending_light_mis)`. The last is
/// `Some(p)`, the density of the new direction given the branch, whenever NEE is on, the outcome
/// is exterior (see below) and the sample is valid; the caller stashes it like the HG carry.
///
/// NEE is eligible exactly for the outcomes that end outside the gem: a reflection while
/// outside, a transmission while inside. An entry through a ground facet is not NEE-sampled (the
/// light sample would have to find the exit facet), as before.
#[expect(
    clippy::too_many_arguments,
    reason = "bundles ctx/geo like the polished dispatch; nee/lambdas/radiance are the HDR-map NEE \
              inputs/output; the rest is the facet event and the per-ray state a bounce mutates"
)]
pub(crate) fn apply_ground_bounce(
    ctx: &RayMaterialContext,
    geo: &BounceRefractionGeometry,
    normal: Vec3,
    (k_in, hit_point): (Vec3, Vec3),
    params: GroundParams,
    inside_gem: bool,
    (rng_seed, bounce): (u32, u32),
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
    nee: NeeContext<'_>,
    lambdas: &[f32; NUM_CHANNELS],
    radiance: &mut [f32; NUM_CHANNELS],
) -> (Vec3, bool, Option<bool>, Option<f32>) {
    let u1 = unit_draw(rng_seed, bounce, FROSTED_DIR_U_STREAM);
    let u2 = unit_draw(rng_seed, bounce, FROSTED_DIR_V_STREAM);
    let u_branch = unit_draw(rng_seed, bounce, FRESNEL_BRANCH_STREAM);
    let ground = GroundCtx::new(normal, k_in, params, GroundInterface::from_geometry(geo));
    let sample = ground.sample(u1, u2, u_branch);

    // Exterior outcomes only: see the doc comment.
    let exterior = sample.reflect_branch != inside_gem;
    if nee.enabled && exterior {
        nee_contribution_ground(
            nee,
            &ground,
            sample.reflect_branch,
            hit_point,
            lambdas,
            rng_seed,
            bounce,
            stokes,
            radiance,
        );
    }
    for k in 0..NUM_CHANNELS {
        stokes[k] = StokesVector::unpolarized(stokes[k].intensity() * sample.weight);
        path_pdf[k] *= sample.branch_prob;
    }
    let entering_anisotropic = !inside_gem && ctx.is_anisotropic;
    let mode_update = (sample.crossed && entering_anisotropic).then(|| {
        // Mode SELECTION is a stochastic 50/50 draw with no throughput weighting, as for the
        // polished entry.
        unit_draw(rng_seed, bounce, BIREFRINGENT_SPLIT_STREAM) < 0.5
    });
    let pending = (nee.enabled && exterior && sample.valid).then_some(sample.pdf);
    (
        sample.dir,
        inside_gem != sample.crossed,
        mode_update,
        pending,
    )
}
