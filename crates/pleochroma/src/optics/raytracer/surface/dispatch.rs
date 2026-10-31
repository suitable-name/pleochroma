//! The facet-event hooks of the surface finish: what a facet is, and the polished halo.
//!
//! [`begin_facet_surface`] runs before the facet's interface dispatch. It resolves the facet to
//! one of three things:
//!
//! - a ground facet (a frosted girdle, a Ground surface, or a point inside the abraded-edge
//!   band): the finish becomes `Frosted` and the event carries the GGX lobe for
//!   [`apply_ground_bounce`](super::apply_ground_bounce);
//! - a polish with roughness: with the probability of the total integrated scatter the
//!   macroscopic normal is replaced by a micro-normal drawn from the anisotropic GGX
//!   visible-normal distribution, and the unchanged polished dispatch then runs at that normal, so
//!   the Lekner uniaxial entry, the biaxial modes, total internal reflection, exit splitting and
//!   the Mueller frame rotation all stay exact;
//! - anything else: untouched.
//!
//! [`finish_facet_surface`] runs after the dispatch. For a polish with roughness it corrects the
//! throughput for the branch that happened (halo or specular, reflected or transmitted), channel by
//! channel, and kills a halo direction that ends on the wrong side of the macroscopic surface.

use glam::Vec3;

use super::{
    SURFACE_HALO_STREAM, SURFACE_SWEEP_STREAM, SURFACE_VNDF_U_STREAM, SURFACE_VNDF_V_STREAM,
    SurfaceScene,
    edge::{edge_distance, in_edge_band},
    facet_surface::FacetSurface,
    ggx::{GgxLobe, MIN_COSINE},
    ground::{GroundFrame, GroundParams},
    sweep::{scratch_angle_deg, scratch_direction},
    tis::{tis_reflection, tis_transmission},
    unit_draw,
};
use crate::{
    geometry::{plane::GpuFacetPlane, tool::ToolPrimitive},
    optics::{
        materials::GemMaterial,
        polarization::StokesVector,
        raytracer::{
            NUM_CHANNELS,
            camera::FacetFinish,
            refraction::{R_UNPOL_PDF_MAX, R_UNPOL_PDF_MIN},
        },
    },
};

/// Everything [`begin_facet_surface`] reads about one facet event.
#[derive(Clone, Copy)]
pub(crate) struct SurfaceQuery<'a> {
    pub(crate) scene: &'a SurfaceScene<'a>,
    pub(crate) planes: &'a [GpuFacetPlane],
    pub(crate) tools: &'a [ToolPrimitive],
    pub(crate) material: &'a GemMaterial,
    /// The finish `facet_finishes` names for the facet.
    pub(crate) finish: FacetFinish,
    pub(crate) facet_idx: usize,
    pub(crate) hit_point: Vec3,
    /// The facet's outward shading normal.
    pub(crate) normal_out: Vec3,
    /// The wave normal `k` of the incident ray (its propagation direction).
    pub(crate) k: Vec3,
    pub(crate) inside_gem: bool,
    pub(crate) lambdas: &'a [f32; NUM_CHANNELS],
    pub(crate) hero_idx: usize,
    /// The exit-split radiance staged so far, to scale the event's own share afterwards.
    pub(crate) split_radiance: &'a [f32; NUM_CHANNELS],
    pub(crate) rng_seed: u32,
    pub(crate) bounce: u32,
}

/// The state of a polish with roughness between [`begin_facet_surface`] and
/// [`finish_facet_surface`].
#[derive(Clone, Copy)]
struct RoughnessEvent {
    /// Whether this event took the halo branch (a micro-normal was drawn).
    halo: bool,
    /// The frame at the macro normal on the incident side (`x` across the scratches); only
    /// meaningful when `halo`.
    frame: GroundFrame,
    lobe: GgxLobe,
    /// The reflection and transmission scatter fraction of every channel at the macro angle.
    tis_r: [f32; NUM_CHANNELS],
    tis_t: [f32; NUM_CHANNELS],
    /// The hero's halo probability, the one the draw used.
    p_hero: f32,
    /// The exit-split radiance before the dispatch.
    split_before: [f32; NUM_CHANNELS],
}

/// What [`begin_facet_surface`] decided for a facet event.
#[derive(Clone, Copy)]
pub(crate) struct FacetSurfaceEvent {
    /// The finish the dispatch must use (`Frosted` for every ground outcome).
    pub(crate) finish: FacetFinish,
    /// The lobe of the ground arm; unused unless `finish` is `Frosted`.
    pub(crate) ground: GroundParams,
    /// The normal the event runs at: the macro normal, or a micro-normal of the halo branch.
    pub(crate) normal_out: Vec3,
    roughness: Option<RoughnessEvent>,
}

/// The outcome of the dispatch, as [`finish_facet_surface`] needs it.
#[derive(Clone, Copy)]
pub(crate) struct SurfaceOutcome {
    pub(crate) inside_before: bool,
    pub(crate) inside_after: bool,
    /// The wave normal after the event.
    pub(crate) new_k: Vec3,
}

/// The direction across the scratches (in the facet plane, perpendicular to them) of a sample
/// on a facet with outward normal `normal_out`.
fn across_scratches(
    surface: FacetSurface,
    scene: &SurfaceScene<'_>,
    normal_out: Vec3,
    rng_seed: u32,
    bounce: u32,
) -> Vec3 {
    let angle = scratch_angle_deg(
        &surface,
        &scene.machine,
        unit_draw(rng_seed, bounce, SURFACE_SWEEP_STREAM),
    );
    normal_out.cross(scratch_direction(normal_out, angle))
}

/// The ground outcome for a facet whose surface is `surface`.
fn ground_event(
    q: &SurfaceQuery<'_>,
    surface: Option<&FacetSurface>,
    edge_alpha: Option<f32>,
) -> FacetSurfaceEvent {
    let ground = match (surface, edge_alpha) {
        (Some(surface), _) => GroundParams {
            lobe: surface.ground_lobe(),
            across: (surface.anisotropy() > 1.0)
                .then(|| across_scratches(*surface, q.scene, q.normal_out, q.rng_seed, q.bounce)),
        },
        (None, Some(alpha)) => GroundParams {
            lobe: GgxLobe::isotropic(alpha),
            across: None,
        },
        (None, None) => GroundParams::DEFAULT,
    };
    FacetSurfaceEvent {
        finish: FacetFinish::Frosted,
        ground,
        normal_out: q.normal_out,
        roughness: None,
    }
}

/// The halo draw of a polish with roughness: the per-channel scatter fractions, the draw, and
/// (on the halo branch) the micro-normal.
fn roughness_event(q: &SurfaceQuery<'_>, surface: FacetSurface) -> FacetSurfaceEvent {
    let (k, n_out) = (q.k, q.normal_out);
    let to_eye = -k;
    let n_side = if to_eye.dot(n_out) > 0.0 {
        n_out
    } else {
        -n_out
    };
    let cos_i = to_eye.dot(n_side).clamp(0.0, 1.0);
    let rq = surface.rq_nm();
    let mut tis_r = [0.0f32; NUM_CHANNELS];
    let mut tis_t = [0.0f32; NUM_CHANNELS];
    for c in 0..NUM_CHANNELS {
        let lambda = q.lambdas[c];
        let n_c = q.material.dispersion.evaluate(lambda);
        let (n1, n2) = if q.inside_gem { (n_c, 1.0) } else { (1.0, n_c) };
        tis_r[c] = tis_reflection(rq, n1, cos_i, lambda);
        tis_t[c] = tis_transmission(rq, n1, n2, cos_i, lambda);
    }
    let p_hero = tis_r[q.hero_idx].clamp(R_UNPOL_PDF_MIN, R_UNPOL_PDF_MAX);
    let halo = unit_draw(q.rng_seed, q.bounce, SURFACE_HALO_STREAM) < p_hero;
    let lobe = surface.halo_lobe();
    let (frame, normal_out) = if halo {
        let across = across_scratches(surface, q.scene, n_out, q.rng_seed, q.bounce);
        let frame = GroundFrame::new(n_side, Some(across));
        let mut wi = frame.to_local(to_eye);
        wi.z = wi.z.max(MIN_COSINE);
        let m = lobe.sample_visible(
            wi.normalize(),
            unit_draw(q.rng_seed, q.bounce, SURFACE_VNDF_U_STREAM),
            unit_draw(q.rng_seed, q.bounce, SURFACE_VNDF_V_STREAM),
        );
        // The micro-normal faces the incident side; the dispatch wants it outward like `n_out`.
        let m_world = frame.to_world(m);
        let normal_out = if n_side.dot(n_out) > 0.0 {
            m_world
        } else {
            -m_world
        };
        (frame, normal_out)
    } else {
        (GroundFrame::new(n_side, None), n_out)
    };
    FacetSurfaceEvent {
        finish: q.finish,
        ground: GroundParams::DEFAULT,
        normal_out,
        roughness: Some(RoughnessEvent {
            halo,
            frame,
            lobe,
            tis_r,
            tis_t,
            p_hero,
            split_before: *q.split_radiance,
        }),
    }
}

/// Resolves the surface of the facet a ray just hit; see the module documentation. With an ideal
/// scene it returns the inputs unchanged and touches nothing.
#[must_use]
pub(crate) fn begin_facet_surface(q: &SurfaceQuery<'_>) -> FacetSurfaceEvent {
    let scene = q.scene;
    let passthrough = FacetSurfaceEvent {
        finish: q.finish,
        ground: GroundParams::DEFAULT,
        normal_out: q.normal_out,
        roughness: None,
    };
    if scene.is_ideal() {
        return passthrough;
    }
    let surface = scene
        .facet_surfaces
        .get(q.facet_idx)
        .copied()
        .unwrap_or(FacetSurface::IDEAL);
    // The frosted flag of the finish list and the ground kinds of the word both name the ground
    // arm; the word supplies the lobe when it has one.
    if q.finish == FacetFinish::Frosted || surface.is_ground() {
        return if surface.is_ground() {
            ground_event(q, Some(&surface), None)
        } else {
            ground_event(q, None, None)
        };
    }
    if scene.edge.ground_active()
        && q.tools.is_empty()
        && in_edge_band(q.planes, q.hit_point, q.facet_idx, scene.edge.width)
    {
        return ground_event(q, None, Some(scene.edge.alpha));
    }
    // The TIS boost: the polish next to the band is duller than the facet's own.
    let surface = if scene.edge.boost_active() && q.tools.is_empty() {
        let distance = edge_distance(q.planes, q.hit_point, q.facet_idx);
        surface.with_rq_at_least(scene.edge.wear_rq_nm(distance))
    } else {
        surface
    };
    if surface.has_roughness() {
        return roughness_event(q, surface);
    }
    passthrough
}

/// Corrects the throughput of a polish with roughness after its dispatch; a no-op for every
/// other event.
///
/// The branch (halo or specular) was drawn with the hero's scatter fraction `P`, whatever the
/// interface then did, so each channel `c` is weighted by the true fraction for what happened
/// (`TIS_r` for a reflection, `TIS_t` for a transmission) over the probability used:
/// `TIS_c / P` for a halo, `(1 − TIS_c) / (1 − P)` for the specular branch. `path_pdf` takes the
/// probability that channel's own technique would have drawn the branch with. A halo direction
/// that ends on the wrong side of the macroscopic surface is killed; the rest carry the Smith
/// masking `G1` of the outgoing direction. A transmission's share of the exit-split radiance
/// (staged during the dispatch) is scaled the same way.
pub(crate) fn finish_facet_surface(
    event: &FacetSurfaceEvent,
    outcome: SurfaceOutcome,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
    split_radiance: &mut [f32; NUM_CHANNELS],
) {
    let Some(rough) = &event.roughness else {
        return;
    };
    let reflected = outcome.inside_before == outcome.inside_after;
    // The outgoing masking of a halo direction; 1 for the specular branch.
    let masking = if rough.halo {
        let along = rough.frame.n.dot(outcome.new_k);
        let right_side = if reflected { along > 0.0 } else { along < 0.0 };
        if right_side {
            rough.lobe.g1(rough.frame.to_local(outcome.new_k))
        } else {
            0.0
        }
    } else {
        1.0
    };
    let tis = if reflected {
        &rough.tis_r
    } else {
        &rough.tis_t
    };
    for c in 0..NUM_CHANNELS {
        let (scale, technique) = if rough.halo {
            (
                tis[c] / rough.p_hero * masking,
                rough.tis_r[c].clamp(R_UNPOL_PDF_MIN, R_UNPOL_PDF_MAX),
            )
        } else {
            (
                (1.0 - tis[c]) / (1.0 - rough.p_hero),
                (1.0 - rough.tis_r[c]).clamp(R_UNPOL_PDF_MIN, R_UNPOL_PDF_MAX),
            )
        };
        stokes[c] = stokes[c].scale(scale);
        path_pdf[c] *= technique;
        if !reflected {
            split_radiance[c] =
                (split_radiance[c] - rough.split_before[c]).mul_add(scale, rough.split_before[c]);
        }
    }
}
