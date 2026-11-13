//! Fixed-order internal reflection chains, including bounded cavity re-entry.

use glam::Vec3;

use crate::{
    geometry::tool::ToolPrimitive,
    optics::raytracer::{FacetFinish, HitRecord, Ray, intersect_stone::intersect_stone_soa_side},
    renderer::frame_scene::FrameScene,
    simd::PlanesSoA32,
};

use super::{
    CUTOFF, WORKLIST_CAPACITY, WhittedPrep,
    fresnel::{dot, fresnel_unpolarised_f32, reflect, refract, scale_add},
};

const RESTART: f32 = 1.0e-4;

#[derive(Clone, Copy)]
struct Branch {
    ray: Ray,
    /// The product of every Fresnel factor on the way here.
    fresnel: f32,
    /// Model units travelled inside the stone so far, and the axis-weighted share of them
    /// (see [`super::tint::BinnedAbsorption`]): absorption is a function of the whole path,
    /// not a product of per-segment RGB factors.
    length: f32,
    axial: f32,
    /// What the branch contributes per unit of radiance: `fresnel` times the path's
    /// transmission. The cutoff prunes on it.
    weight: Vec3,
    inside: bool,
    depth: u32,
    primary: bool,
}

impl Branch {
    /// The camera ray: nothing travelled, nothing reflected.
    const fn camera(ray: Ray) -> Self {
        Self {
            ray,
            fresnel: 1.0,
            length: 0.0,
            axial: 0.0,
            weight: Vec3::ONE,
            inside: false,
            depth: 0,
            primary: true,
        }
    }

    /// A branch continuing this one's path along `ray`, scaled by the Fresnel `factor`.
    fn child(&self, ray: Ray, factor: f32, inside: bool, depth: u32) -> Self {
        Self {
            ray,
            fresnel: self.fresnel * factor,
            length: self.length,
            axial: self.axial,
            weight: self.weight * factor,
            inside,
            depth,
            primary: false,
        }
    }
}

struct Worklist {
    slots: [Option<Branch>; WORKLIST_CAPACITY],
    len: usize,
    cutoff: f32,
}

impl Worklist {
    const fn new(cutoff: f32) -> Self {
        Self {
            slots: [None; WORKLIST_CAPACITY],
            len: 0,
            cutoff,
        }
    }

    fn push(&mut self, branch: Branch, max_depth: u32) {
        if branch.depth <= max_depth && branch.weight.max_element() >= self.cutoff {
            debug_assert!(self.len < WORKLIST_CAPACITY);
            self.slots[self.len] = Some(branch);
            self.len += 1;
        }
    }

    const fn pop(&mut self) -> Option<Branch> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        self.slots[self.len].take()
    }
}

fn restarted(point: Vec3, dir: Vec3) -> Ray {
    Ray {
        origin: scale_add(dir, RESTART, point),
        dir,
    }
}

/// Resolve a ray that left the stone for good: the lit environment, or, over a glossy tray,
/// the floor split.
///
/// With no mirror, or a ray that does not meet the tray (upward, or already below it), this
/// is `weight * leaf_at(origin, dir)`. When the ray meets the tray plane at
/// `P` with Schlick reflectance `F`, the matte paper keeps `1 - F` and the mirror takes `F`:
///
/// `sum += weight * (1 - F) * leaf_at(origin, dir)`, then the reflected ray, restarted
/// 1e-4 off the plane, continues as a child branch with factor `F`. The child is pushed
/// only while the depth cap allows it and the last worklist slot stays free for the internal
/// continuation; otherwise it resolves at once as the environment along the reflected
/// direction (the stone is not searched for that one leaf, as on cavity overflow).
///
/// The tray reflects unpolarized light whatever the leaf's polarization (the Ray-Traced
/// engine carries none), identically for every channel, so the RGB chains need no extra care.
/// Both escape sites, the branch miss in [`trace_chain_with_cutoff`] and [`emit_outside`],
/// call this one function. The WGSL twin is `wt_floor_escape` in the Ray-Traced shader (it calls
/// `wt_floor`, the twin of `TrayMirror::hit`), applied at the twin of each of these two sites in
/// the same order.
fn resolve_escape(
    work: &mut Worklist,
    sum: &mut Vec3,
    prep: &WhittedPrep<'_>,
    leaf: Branch,
    observer: Vec3,
) {
    let ray = leaf.ray;
    if let Some(floor) = prep
        .env
        .mirror()
        .and_then(|mirror| mirror.hit(ray.origin, ray.dir))
    {
        *sum +=
            leaf.weight * (1.0 - floor.fresnel) * prep.env.leaf_at(ray.origin, ray.dir, observer);
        let child = leaf.child(
            restarted(floor.point, floor.reflected),
            floor.fresnel,
            false,
            leaf.depth.saturating_add(1),
        );
        if child.depth <= prep.max_depth && work.len < WORKLIST_CAPACITY - 1 {
            work.push(child, prep.max_depth);
        } else {
            *sum += child.weight * prep.env.leaf_at(child.ray.origin, child.ray.dir, observer);
        }
    } else {
        *sum += leaf.weight * prep.env.leaf_at(ray.origin, ray.dir, observer);
    }
}

/// Reserve the last stack slot for the internal continuation. Outside leaves
/// resolve directly for convex stones; on cavity overflow only that leaf falls
/// back to the environment. The continuation is never silently overwritten.
///
/// A direct resolve goes through [`resolve_escape`], so a leaf bound for a glossy tray
/// reflects there instead of reading the paper. A stone with tools pushes its leaf instead,
/// floor-bound or not, so that [`first_hit`] can find a re-entry into the cavity first; a
/// pushed leaf that misses the stone meets the tray at the branch miss, which calls
/// [`resolve_escape`] as well.
fn emit_outside(
    work: &mut Worklist,
    sum: &mut Vec3,
    prep: &WhittedPrep<'_>,
    tools: &[ToolPrimitive],
    leaf: Branch,
    observer: Vec3,
) {
    if tools.is_empty() || work.len >= WORKLIST_CAPACITY - 1 {
        resolve_escape(work, sum, prep, leaf, observer);
    } else {
        work.push(leaf, prep.max_depth);
    }
}

/// Fresnel-split one surface interaction into its reflected and transmitted branches.
#[expect(
    clippy::too_many_arguments,
    reason = "flat scalar chain state mirrors the WGSL twin's parameter list"
)]
fn split_at_surface(
    work: &mut Worklist,
    sum: &mut Vec3,
    prep: &WhittedPrep<'_>,
    tools: &[ToolPrimitive],
    branch: Branch,
    point: Vec3,
    normal: Vec3,
    observer: Vec3,
    n: f32,
) {
    let cos_i = (-dot(branch.ray.dir, normal)).clamp(0.0, 1.0);
    let eta = if branch.inside { n } else { 1.0 / n };
    let reflectance = fresnel_unpolarised_f32(cos_i, eta);
    let reflection = restarted(point, reflect(branch.ray.dir, normal));
    let depth = branch.depth.saturating_add(1);

    if branch.inside {
        if let Some(transmitted) = refract(branch.ray.dir, normal, eta) {
            let leaf = branch.child(
                restarted(point, transmitted),
                1.0 - reflectance,
                false,
                depth,
            );
            emit_outside(work, sum, prep, tools, leaf, observer);
        }
        work.push(
            branch.child(reflection, reflectance, true, depth),
            prep.max_depth,
        );
    } else {
        let glare = if branch.primary { prep.glare } else { 1.0 };
        let leaf = branch.child(reflection, reflectance * glare, false, depth);
        emit_outside(work, sum, prep, tools, leaf, observer);
        if let Some(transmitted) = refract(branch.ray.dir, normal, eta) {
            work.push(
                branch.child(
                    restarted(point, transmitted),
                    1.0 - reflectance,
                    true,
                    depth,
                ),
                prep.max_depth,
            );
        }
    }
}

/// One intersection of the stone with its entering/exiting side, or a miss.
pub(super) type Intersection = Option<(HitRecord, Option<bool>)>;

#[cfg(test)]
thread_local! {
    /// Camera-ray intersections computed inside a chain (tests only; thread-local so
    /// parallel tests cannot disturb each other's deltas).
    pub(super) static PRIMARY_INTERSECTIONS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

/// A scene with no planes is an unobstructed environment (a miss), not an intersection. The
/// result does not depend on the chain's refractive index, so one tap computes it once for all
/// its chains.
pub(super) fn first_hit(
    scene: &FrameScene<'_>,
    tools: &[ToolPrimitive],
    plane_soa: &PlanesSoA32,
    ray: Ray,
) -> Intersection {
    if scene.planes.is_empty() {
        None
    } else {
        intersect_stone_soa_side(ray, plane_soa, scene.planes.len(), tools)
    }
}

/// Traces a single scalar-index RGB chain, intersecting the camera ray itself. The
/// optional hit is always the camera's first intersection, never a later branch's.
/// Production traces through [`trace_chain_shared_primary`]; this form is the reference
/// the tests compare it against.
#[cfg(test)]
#[expect(
    clippy::too_many_arguments,
    reason = "the frame scene, prepared state and one camera ray stay flat to mirror the WGSL twin"
)]
pub(super) fn trace_chain(
    scene: &FrameScene<'_>,
    tools: &[ToolPrimitive],
    plane_soa: &PlanesSoA32,
    prep: &WhittedPrep<'_>,
    camera_ray: Ray,
    n: f32,
    observer: Vec3,
    primary_hit_out: Option<&mut Option<HitRecord>>,
) -> Vec3 {
    trace_chain_with_cutoff(
        scene,
        tools,
        plane_soa,
        prep,
        camera_ray,
        n,
        observer,
        primary_hit_out,
        CUTOFF,
        None,
    )
}

/// `trace_chain` for a caller that already intersected `camera_ray` with [`first_hit`]
/// (the tap's other chains share it); `primary` is that result, a miss included.
#[expect(
    clippy::too_many_arguments,
    reason = "trace_chain's parameters plus the shared camera-ray intersection"
)]
pub(super) fn trace_chain_shared_primary(
    scene: &FrameScene<'_>,
    tools: &[ToolPrimitive],
    plane_soa: &PlanesSoA32,
    prep: &WhittedPrep<'_>,
    camera_ray: Ray,
    n: f32,
    observer: Vec3,
    primary_hit_out: Option<&mut Option<HitRecord>>,
    primary: Intersection,
) -> Vec3 {
    trace_chain_with_cutoff(
        scene,
        tools,
        plane_soa,
        prep,
        camera_ray,
        n,
        observer,
        primary_hit_out,
        CUTOFF,
        Some(primary),
    )
}

/// The production cutoff is fixed; an explicit cutoff also lets analytic tests
/// measure energy conservation independently of intentional pruning. `primary` is
/// `None` to intersect the camera ray here, or the precomputed [`first_hit`] result.
#[expect(
    clippy::too_many_arguments,
    reason = "trace_chain's parameters plus the explicit cutoff and shared intersection"
)]
pub(super) fn trace_chain_with_cutoff(
    scene: &FrameScene<'_>,
    tools: &[ToolPrimitive],
    plane_soa: &PlanesSoA32,
    prep: &WhittedPrep<'_>,
    camera_ray: Ray,
    n: f32,
    observer: Vec3,
    mut primary_hit_out: Option<&mut Option<HitRecord>>,
    cutoff: f32,
    primary: Option<Intersection>,
) -> Vec3 {
    if let Some(out) = primary_hit_out.as_deref_mut() {
        *out = None;
    }
    let mut sum = Vec3::ZERO;
    let mut work = Worklist::new(cutoff);
    work.push(Branch::camera(camera_ray), prep.max_depth);

    while let Some(mut branch) = work.pop() {
        // A caller-supplied result replaces only the camera ray's intersection; every
        // later branch intersects here.
        let hit = primary.filter(|_| branch.primary).unwrap_or_else(|| {
            #[cfg(test)]
            {
                if branch.primary {
                    PRIMARY_INTERSECTIONS.with(|count| count.set(count.get() + 1));
                }
            }
            first_hit(scene, tools, plane_soa, branch.ray)
        });
        let Some((hit, entering)) = hit else {
            // A miss resolves through `resolve_escape`: the lit environment, or a glossy tray's
            // reflection.
            resolve_escape(&mut work, &mut sum, prep, branch, observer);
            continue;
        };
        if branch.primary
            && let Some(out) = primary_hit_out.as_deref_mut()
        {
            *out = Some(hit);
        }
        // At close CSG boundaries the restart offset may cross another face.
        // The intersection's side is authoritative for this entire segment.
        if let Some(entering) = entering {
            branch.inside = !entering;
        }
        if branch.inside {
            branch.length += hit.t;
            branch.axial += prep.absorption.axial(branch.ray.dir, hit.t);
            branch.weight =
                prep.absorption.transmission(branch.length, branch.axial) * branch.fresnel;
        }
        if matches!(
            scene.facet_finishes.get(hit.facet_idx),
            Some(FacetFinish::Frosted)
        ) {
            sum += branch.weight * prep.frost.get(hit.facet_idx).copied().unwrap_or(Vec3::ZERO);
            continue;
        }
        let point = scale_add(branch.ray.dir, hit.t, branch.ray.origin);
        let normal = if branch.inside {
            -hit.normal
        } else {
            hit.normal
        };
        split_at_surface(
            &mut work, &mut sum, prep, tools, branch, point, normal, observer, n,
        );
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optics::raytracer::environment::TrayMirror;

    #[test]
    fn cavity_overflow_preserves_the_continuation_and_resolves_the_leaf() {
        let prep = WhittedPrep::for_tests(1.5, Vec3::ZERO, Vec3::new(0.3, 0.7, 0.9), 64, 1);
        let ray = Ray {
            origin: Vec3::ZERO,
            dir: Vec3::Z,
        };
        let branch = Branch {
            inside: true,
            depth: 1,
            primary: false,
            ..Branch::camera(ray)
        };
        let mut work = Worklist::new(CUTOFF);
        for _ in 0..WORKLIST_CAPACITY - 1 {
            work.push(branch, prep.max_depth);
        }
        let tools = [ToolPrimitive::ball(Vec3::ZERO, 0.2)];
        let mut sum = Vec3::ZERO;
        let leaf = branch.child(ray, 0.25, false, 2);
        emit_outside(&mut work, &mut sum, &prep, &tools, leaf, Vec3::NEG_Z);
        assert_eq!(work.len, WORKLIST_CAPACITY - 1);
        assert!((sum - Vec3::new(0.3, 0.7, 0.9) * 0.25).abs().max_element() < 1e-7);
        work.push(branch, prep.max_depth);
        assert_eq!(work.len, WORKLIST_CAPACITY);
        assert!(work.pop().expect("continuation").inside);
    }

    fn escaping_leaf() -> Branch {
        let ray = Ray {
            origin: Vec3::new(0.0, 1.0, 0.0),
            dir: Vec3::new(0.3, -1.0, 0.1).normalize(),
        };
        Branch {
            depth: 1,
            primary: false,
            ..Branch::camera(ray)
        }
    }

    #[test]
    fn a_floor_bound_leaf_pushes_its_reflected_child_when_there_is_room() {
        let mirror = TrayMirror::new(-1.0, 0.5).expect("mirror");
        let color = Vec3::new(0.3, 0.7, 0.9);
        let prep = WhittedPrep::for_tests(1.5, Vec3::ZERO, color, 64, 1);
        let prep = WhittedPrep {
            env: prep.env.with_mirror(Some(mirror)),
            ..prep
        };
        let leaf = escaping_leaf();
        let floor = mirror.hit(leaf.ray.origin, leaf.ray.dir).expect("floor");
        let mut work = Worklist::new(0.0);
        let mut sum = Vec3::ZERO;
        resolve_escape(&mut work, &mut sum, &prep, leaf, Vec3::Y);
        assert_eq!(work.len, 1);
        let child = work.pop().expect("child");
        assert_eq!(child.depth, 2);
        assert!(!child.inside && !child.primary);
        assert!(
            (child.weight - Vec3::splat(floor.fresnel))
                .abs()
                .max_element()
                < 1e-7
        );
        assert!(child.ray.dir.y > 0.0);
        assert!((sum - color * (1.0 - floor.fresnel)).abs().max_element() < 1e-6);
    }

    #[test]
    fn a_stone_with_tools_pushes_a_floor_bound_leaf_unchanged() {
        let mirror = TrayMirror::new(-1.0, 0.5).expect("mirror");
        let color = Vec3::new(0.3, 0.7, 0.9);
        let prep = WhittedPrep::for_tests(1.5, Vec3::ZERO, color, 64, 1);
        let prep = WhittedPrep {
            env: prep.env.with_mirror(Some(mirror)),
            ..prep
        };
        let leaf = escaping_leaf();
        assert!(mirror.hit(leaf.ray.origin, leaf.ray.dir).is_some());
        let tools = [ToolPrimitive::ball(Vec3::ZERO, 0.2)];
        let mut work = Worklist::new(0.0);
        let mut sum = Vec3::ZERO;
        emit_outside(&mut work, &mut sum, &prep, &tools, leaf, Vec3::Y);
        // The cavity may re-enter the stone first: nothing is resolved, the leaf waits.
        assert_eq!(work.len, 1);
        assert_eq!(sum, Vec3::ZERO);
        let pushed = work.pop().expect("leaf");
        assert_eq!(pushed.ray.dir, leaf.ray.dir);
        assert_eq!(pushed.depth, leaf.depth);
        assert_eq!(pushed.weight, leaf.weight);
    }

    #[test]
    fn a_reflected_child_past_the_depth_cap_resolves_instead_of_dropping() {
        let mirror = TrayMirror::new(-1.0, 0.5).expect("mirror");
        let color = Vec3::new(0.3, 0.7, 0.9);
        let prep = WhittedPrep::for_tests(1.5, Vec3::ZERO, color, 64, 1);
        let prep = WhittedPrep {
            env: prep.env.with_mirror(Some(mirror)),
            ..prep
        };
        let leaf = Branch {
            depth: prep.max_depth,
            ..escaping_leaf()
        };
        let mut work = Worklist::new(0.0);
        let mut sum = Vec3::ZERO;
        resolve_escape(&mut work, &mut sum, &prep, leaf, Vec3::Y);
        // The child would sit one past the cap: it resolves as the environment, so the paper
        // and the mirror share together are the whole leaf.
        assert_eq!(work.len, 0);
        assert!((sum - color).abs().max_element() < 1e-6, "{sum:?}");
    }

    #[test]
    fn a_full_worklist_resolves_the_reflected_leaf_instead_of_dropping_it() {
        let mirror = TrayMirror::new(-1.0, 0.5).expect("mirror");
        let color = Vec3::new(0.3, 0.7, 0.9);
        let prep = WhittedPrep::for_tests(1.5, Vec3::ZERO, color, 64, 1);
        let prep = WhittedPrep {
            env: prep.env.with_mirror(Some(mirror)),
            ..prep
        };
        let leaf = escaping_leaf();
        let held = Branch {
            inside: true,
            ..leaf
        };
        let mut work = Worklist::new(0.0);
        for _ in 0..WORKLIST_CAPACITY - 1 {
            work.push(held, prep.max_depth);
        }
        let mut sum = Vec3::ZERO;
        resolve_escape(&mut work, &mut sum, &prep, leaf, Vec3::Y);
        // The last slot stays free, and the whole radiance (paper plus mirror) is accounted
        // for: a constant environment makes the two terms add up to the leaf's weight.
        assert_eq!(work.len, WORKLIST_CAPACITY - 1);
        assert!((sum - color).abs().max_element() < 1e-6, "{sum:?}");
    }
}
