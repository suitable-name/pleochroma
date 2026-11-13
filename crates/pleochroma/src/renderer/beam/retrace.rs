//! The exact-ray retracer (beam guide §7.5): the exact ray of a branch prefix through one image
//! point at one wavelength, geometry only.
//!
//! It starts from the camera ray through the image point (§5.1), meets each prefix facet's plane
//! explicitly (no intersector, Overbeck's same-plane rule) and reflects or refracts as the prefix
//! says, with the node's indices. Every "exact" sample point, snap bisection, TIR snap and leaf
//! exit direction of the beam tracer goes through [`exact_ray`] or [`exact_ray_at`].
//!
//! This file retraces isotropic media. Uniaxial and biaxial media are retraced by
//! [`birefringence::uniaxial_ray`](super::birefringence::uniaxial_ray) and
//! [`birefringence::biaxial_ray`](super::birefringence::biaxial_ray).

use glam::{DMat3, DVec2, DVec3};

use super::{
    beam::{Beam, BranchPrefix, Leaf, MAX_PREFIX, SampleStates, SpectralNode},
    birefringence::{biaxial_ray, is_biaxial_crystal, is_uniaxial, uniaxial_ray},
    interact::InteractKit,
    leaf::{exit_leaves_checked, sample_points},
    propagate::retrace_state,
    spectral::{leaf_peak, shrink_leaf, strictly_constant},
    stats::{BeamStats, DropReason},
    stone::BeamStone,
};
use crate::optics::{materials::GemMaterial, raytracer::Camera};

/// The refractive index outside the stone (air, as the tracer assumes).
pub const OUTSIDE_INDEX: f64 = 1.0;

/// Everything the retracer reads, fixed per frame.
#[derive(Clone, Copy)]
pub struct RetraceKit<'a> {
    /// The camera.
    pub camera: &'a Camera,
    /// The image size in pixels.
    pub size: (u32, u32),
    /// The stone (f64 planes, original facet ids).
    pub stone: &'a BeamStone,
    /// The material (indices per wavelength).
    pub material: &'a GemMaterial,
    /// The spectral node count `N_λ`.
    pub n_lambda: u32,
}

/// Why a prefix has no exact ray through an image point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetraceError {
    /// A prefix step names an original facet the stone does not have.
    UnknownFacet {
        /// The step.
        step: u8,
    },
    /// The ray does not meet the step's plane ahead of it.
    MissedPlane {
        /// The step.
        step: u8,
    },
    /// A transmit step is past the critical angle (total internal reflection).
    TotalReflection {
        /// The step.
        step: u8,
    },
    /// The prefix's events do not alternate inside/outside consistently (an exit while outside).
    Inconsistent {
        /// The step.
        step: u8,
    },
    /// Not produced: birefringent steps are retraced by the uniaxial and biaxial retracers.
    Anisotropic,
}

/// The exact ray after a prefix: the final ray, the hit points and the segment lengths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrefixRay {
    /// The last hit point (the camera origin for an empty prefix).
    pub origin: DVec3,
    /// The ray (Poynting) direction S after the last event, unit length.
    pub dir: DVec3,
    /// The wave normal k after the last event (equal to S in isotropic media).
    pub k: DVec3,
    /// The ray is inside the stone after the last event.
    pub inside: bool,
    /// The eigenmode label after the last event (extraordinary); `false` in isotropic media.
    pub is_ext: bool,
    /// The hit point of each step; the first `len` are used.
    pub hits: [DVec3; MAX_PREFIX],
    /// The length of the segment ending at each step's hit (the first is the camera segment).
    pub lengths: [f64; MAX_PREFIX],
    /// The number of steps.
    pub len: u8,
}

/// The unnormalised camera direction `forward + right·u + up·v` through the image point
/// `(sx, sy)` (screen coordinates, §5.1). It is affine in the image point (§6.4).
#[must_use]
pub fn camera_direction(camera: &Camera, size: (u32, u32), image_point: DVec2) -> DVec3 {
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    let aspect = w / h;
    let fov_tan = f64::from(camera.fov_tan);
    let u = (image_point.x / w - 0.5) * 2.0 * aspect * fov_tan;
    let v = (0.5 - image_point.y / h) * 2.0 * fov_tan;
    camera.forward.as_dvec3() + camera.right.as_dvec3() * u + camera.up.as_dvec3() * v
}

/// The step word's event code: 1 reflect (partial or total), 3 transmit (`branch_step`, §14.3).
#[must_use]
pub const fn step_code(word: u32) -> u32 {
    (word >> 1) & 0x7F
}

/// The step word's original facet index.
#[must_use]
pub const fn step_facet(word: u32) -> u32 {
    word >> 8
}

/// The isotropic index of `material` at `lambda_nm`, as the tracer evaluates it.
fn index_at(material: &GemMaterial, lambda_nm: f32) -> f64 {
    f64::from(material.dispersion.evaluate(lambda_nm))
}

/// The exact ray of `prefix` through `image_point` at the centre of `node` (§7.5).
///
/// # Errors
///
/// See [`RetraceError`]: the prefix has no exact ray there (a classification the caller uses to
/// detect a wrong model topology, §7.3, §7.4 step 8).
pub fn exact_ray(
    image_point: DVec2,
    node: SpectralNode,
    prefix: &BranchPrefix,
    kit: &RetraceKit<'_>,
) -> Result<PrefixRay, RetraceError> {
    exact_ray_at(image_point, node.centre_nm(kit.n_lambda), prefix, kit)
}

/// [`exact_ray`] at an arbitrary wavelength, for the bin-edge snaps of §9.2.
///
/// # Errors
///
/// See [`RetraceError`].
pub fn exact_ray_at(
    image_point: DVec2,
    lambda_nm: f32,
    prefix: &BranchPrefix,
    kit: &RetraceKit<'_>,
) -> Result<PrefixRay, RetraceError> {
    if is_uniaxial(kit.material) {
        return uniaxial_ray(image_point, lambda_nm, prefix, kit);
    }
    if is_biaxial_crystal(kit.material) {
        return biaxial_ray(image_point, lambda_nm, prefix, kit);
    }
    let n_stone = index_at(kit.material, lambda_nm);
    let mut origin = kit.camera.origin.as_dvec3();
    let mut dir = camera_direction(kit.camera, kit.size, image_point).normalize();
    let mut inside = false;
    let mut out = PrefixRay {
        origin,
        dir,
        k: dir,
        inside,
        is_ext: false,
        hits: [DVec3::ZERO; MAX_PREFIX],
        lengths: [0.0; MAX_PREFIX],
        len: prefix.len,
    };
    for (i, &word) in prefix.steps().iter().enumerate() {
        let step = u8::try_from(i).unwrap_or(u8::MAX);
        let facet = kit
            .stone
            .facet_of_original(step_facet(word))
            .ok_or(RetraceError::UnknownFacet { step })?;
        let (n_out, m) = kit.stone.plane(facet);
        let denom = n_out.dot(dir);
        // A ray parallel to the plane gives an infinite or NaN `t`, refused below.
        let t = (m - n_out.dot(origin)) / denom;
        if !(t > 0.0 && t.is_finite()) {
            return Err(RetraceError::MissedPlane { step });
        }
        let hit = origin + dir * t;
        out.hits[i] = hit;
        out.lengths[i] = t;
        // The normal facing the incoming ray.
        let unit_out = n_out.normalize();
        let normal = if denom < 0.0 { unit_out } else { -unit_out };
        let cos_i = -dir.dot(normal);
        dir = if step_code(word) == 3 {
            let (n1, n2) = if inside {
                (n_stone, OUTSIDE_INDEX)
            } else {
                (OUTSIDE_INDEX, n_stone)
            };
            let eta = n1 / n2;
            let k = (eta * eta).mul_add(-cos_i.mul_add(-cos_i, 1.0), 1.0);
            if k < 0.0 {
                return Err(RetraceError::TotalReflection { step });
            }
            inside = !inside;
            (eta * dir + eta.mul_add(cos_i, -k.sqrt()) * normal).normalize()
        } else {
            if !inside && i > 0 {
                return Err(RetraceError::Inconsistent { step });
            }
            (dir + 2.0 * cos_i * normal).normalize()
        };
        origin = hit;
    }
    out.origin = origin;
    out.dir = dir;
    out.k = dir;
    out.inside = inside;
    Ok(out)
}

/// A beam standing for the exit branch of `leaf`: its prefix, signature and interval over the
/// leaf's image polygon, with exact states retraced along the prefix at the sample points.
///
/// Only the prefix, interval, signature and sample states decide the exit leaves of
/// `leaf::exit_leaves`, so the carrier data is a stand-in: the exit facet's plane, the identity as
/// image mapping and no apex. New points are retraced with `propagate::retrace_state`. `None` when
/// the leaf has no prefix, names a facet the stone lacks or a sample point cannot be retraced.
///
/// The polygon is the leaf's pre-clip polygon when it has one (`Leaf::pre_clip`), else its image:
/// the beam stands for the piece the tree bisected before the bin-edge clipping cut the leaf.
fn leaf_beam(leaf: &Leaf, kit: &InteractKit<'_, '_>) -> Option<Beam> {
    let last = *leaf.prefix.steps().last()?;
    let facet = kit.retrace.stone.facet_of_original(step_facet(last))?;
    let image = leaf.pre_clip.unwrap_or(leaf.image);
    let mut beam = Beam {
        facet,
        polygon: image,
        apex: DVec3::ZERO,
        image_to_plane: DMat3::IDENTITY,
        inside: false,
        sig: leaf.sig,
        prefix: leaf.prefix,
        bounces: u32::from(leaf.prefix.len),
        spectrum: leaf.spectrum,
        state: SampleStates::default(),
    };
    let (points, vertices) = sample_points(&image);
    let states = points
        .iter()
        .map(|&q| retrace_state(q, &beam, kit))
        .collect::<Option<Vec<_>>>()?;
    beam.state = SampleStates {
        points: states,
        vertices,
    };
    Some(beam)
}

/// Refines an exit leaf for the exit-direction tolerance of `kit` (beam guide §11.2, §14.10): the
/// leaves a fresh render under that tolerance would hold over the same image polygon.
///
/// The sample points are re-evaluated through the stored prefix and checked against the
/// interpolation at `kit.tau_omega`; the leaf is bisected across its longest image edge while the
/// check fails (`leaf::exit_leaves_checked`, the tree's own code). A leaf that passes comes back
/// with its `tau_omega` set to the kit's. A subdivided leaf comes back as its accepted pieces in
/// bisection order, with the leaf's `tile`. Pieces still failing at the depth cap are dropped and
/// counted in `stats` as interpolation drops, so the result can be empty.
///
/// A dispersive stone's pieces go through the bin-edge clipping (`shrink_parts`) as in a fresh
/// render. A clipped leaf bisects from its pre-clip polygon (`Leaf::pre_clip`), like the tree.
/// The split depth restarts at zero because a leaf does not record its depth, so refinement may go
/// `max_split_depth` levels below the leaf. `stats` holds the per-tile allowance; a caller that
/// refines several leaves shares one `stats` across them. A piece the original render refused at
/// the split floor leaves no leaf, so nothing is refined there. A single-node piece that leaves
/// its branch is bisected, never accepted whole.
///
/// `None` when the leaf cannot be retraced (a reflection leaf, which interpolates exactly, an
/// empty prefix, or a point straddling a topological boundary); the caller keeps the leaf.
#[must_use]
pub fn refine_leaf(
    leaf: &Leaf,
    kit: &InteractKit<'_, '_>,
    stats: &mut BeamStats,
) -> Option<Vec<Leaf>> {
    if leaf.reflection {
        return None;
    }
    let beam = leaf_beam(leaf, kit)?;
    let (mut parts, whole) = exit_leaves_checked(&beam, kit, stats);
    // Only a parent that passed whole is restored; one accepted piece of a subdivided parent is
    // that piece; the dropped remainder stays dropped.
    if whole {
        let mut kept = leaf.clone();
        kept.tau_omega = kit.tau_omega;
        return Some(vec![kept]);
    }
    for part in &mut parts {
        part.tile = leaf.tile;
    }
    // A single-node leaf's pieces come back clipped from `exit_leaves_checked`; only a
    // multi-node interval is clipped here.
    if leaf.spectrum.len() > 1 && !strictly_constant(kit.retrace.material, kit.retrace.n_lambda) {
        parts = shrink_parts(parts, kit, stats);
    }
    Some(parts)
}

/// The pieces of a refined dispersive leaf after the tree's bin-edge clipping (guide §9.2,
/// `spectral::shrink_leaf`), in order, each piece keeping its `tile`.
///
/// The tree clips every multi-node exit leaf of a dispersive stone to the region true for every
/// wavelength of its bins (a single-node leaf is clipped in `leaf::exit_into`). The refinement
/// replays that rule with the same `region_mode` and chord tolerance (`kit.settings`). A piece
/// whose boundary does not move monotonically is a [`DropReason::BracketFailure`] drop: the tree
/// would split the interval there, which a refinement of a finished leaf cannot. The pieces lie
/// inside the leaf's clipped polygon, so the replay acts only where the bisection made a new
/// region edge.
fn shrink_parts(parts: Vec<Leaf>, kit: &InteractKit<'_, '_>, stats: &mut BeamStats) -> Vec<Leaf> {
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        match shrink_leaf(&part, kit, kit.settings.region_mode, stats) {
            Ok(pieces) => out.extend(pieces.into_iter().map(|mut piece| {
                piece.tile = part.tile;
                piece
            })),
            Err(_) => stats.record_drop(
                DropReason::BracketFailure,
                part.image.area() * leaf_peak(&part),
            ),
        }
    }
    out
}

/// `true` when a ray inside the stone at `lambda_nm` meets `facet` past the critical angle
/// (the exact TIR test of §8.5, isotropic).
#[must_use]
pub fn is_total_reflection(dir: DVec3, facet: u32, lambda_nm: f32, kit: &RetraceKit<'_>) -> bool {
    let (n_out, _) = kit.stone.plane(facet);
    let cos_i = dir.dot(n_out.normalize()).abs();
    let eta = index_at(kit.material, lambda_nm) / OUTSIDE_INDEX;
    (eta * eta).mul_add(-cos_i.mul_add(-cos_i, 1.0), 1.0) < 0.0
}
