//! Internal surface interactions (beam guide §8.5): partial reflection and transmission, the
//! snapped TIR split, the `diverged` rule, the depth cap and pruning.
//!
//! # How a hit is handled
//!
//! [`split_tir`] classifies the sample points of a beam with the exact retracer. A beam whose
//! points agree is returned whole; otherwise the model boundary (the circle of critical incidence
//! through the apex, or a chord between the polygon's edge crossings) is snapped onto the exact
//! critical curve with [`snap_boundary`] and the beam is cut with [`split_snapped`]. Bracket
//! failures bisect the beam (at most twice) before the region is dropped.
//!
//! [`internal_hit`] then expands every sample point of every piece with `expand_at` at the exact
//! hit. A TIR piece yields the reflected children; a partial piece yields the reflected children
//! and a transmitted beam whose exit leaves come from [`exit_leaves`]. Sample points that sit on
//! the critical curve itself (the snapped vertices) can land on either side of the `f32` decision
//! of `expand`; such points are forced to the class of their piece (a missing transmitted child is
//! the exact limit `T = 0`, an extra one is dropped) when they are within [`NEAR_CRITICAL`] of the
//! critical angle, and anything else is a topology disagreement that subdivides the piece.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

use glam::{DVec2, DVec3, Vec3};

use super::{
    beam::{
        Beam, BoundaryKind, BranchPrefix, Classifier, GROUP, Leaf, SampleState, SampleStates,
        SeamStore, SpectralInterval, SpectralNode,
    },
    birefringence::{
        beam_near_axis, is_anisotropic, mode_index, mode_of, ray_is_total_reflection, refit_apex,
        state_is_total_reflection,
    },
    geom::{
        Tolerances,
        homography::{apply, inverse},
        polygon::{ConvexPolygon, MAX_VERTICES},
        polyline::orient2,
    },
    leaf::{GROUP_HERO, SPLIT_FLOOR_PX2, WEIGHT_ABS_TOLERANCE, exit_leaves},
    primary::{TileRect, image_polygon},
    propagate::retrace_state,
    retrace::{RetraceKit, exact_ray_at, is_total_reflection},
    snap::{SnapRequest, snap_boundary, split_snapped},
    stats::{BeamStats, DropReason},
    subnodes::{SubnodeCaches, accumulate_depths},
};
use crate::{
    geometry::GpuFacetPlane,
    optics::{
        absorption::AbsorptionBand,
        materials::GemMaterial,
        polarization::StokesVector,
        raytracer::{Ray, transport::PreparedMaterial},
    },
    renderer::{
        accel_beam::BeamSettings,
        path_cache::{
            bounce::{
                Child, EventKind, Expansion, ExplicitHit, PathState, SegmentDepths, expand_at,
            },
            build::HeroContext,
            signature::branch_step,
        },
    },
};

/// The class id of the partial-reflection side of the critical curve.
const PARTIAL_CLASS: u32 = 0;
/// The class id of the total-internal-reflection side of the critical curve.
const TIR_CLASS: u32 = 1;
/// Bisections allowed after a bracket failure before a region is dropped (§7.4 step 8).
const MAX_TIR_RETRIES: u32 = 2;
/// A sample point within this much of `η² sin² θ = 1` is on the critical curve for the purposes of
/// the class coercion (the `f32` decision of `expand` and the chord tolerance both live here).
pub const NEAR_CRITICAL: f64 = 1.0e-3;
/// The event code of a reflection in a branch step word.
const REFLECT_CODE: u32 = 1;
/// The event code of a transmission in a branch step word.
const TRANSMIT_CODE: u32 = 3;
/// The angular step of the model arc's vertices, in radians.
const ARC_STEP_RAD: f64 = 0.3;
/// The fewest segments of the model arc.
const MIN_ARC_SEGMENTS: usize = 2;
/// The most segments of the model arc.
const MAX_ARC_SEGMENTS: usize = 16;
/// How far inside the circle (fraction of the radius) the side probe of the arc lies.
const INNER_FRACTION: f64 = 0.05;

/// Everything the interactions of one tile share.
pub struct InteractKit<'k, 'a> {
    /// The exact-ray retracer.
    pub retrace: RetraceKit<'k>,
    /// The renderer's facet planes, indexed by original facet (for `expand_at`).
    pub planes: &'a [GpuFacetPlane],
    /// The prepared material (contexts for intervals off the group grid).
    pub prepared: &'k PreparedMaterial<'a>,
    /// The hero context of every 8-node group of the full node set.
    pub heroes: &'k [HeroContext<'a>],
    /// The beam settings (tolerances, cutoff, depth cap).
    pub settings: &'k BeamSettings,
    /// The tile the beams belong to.
    pub tile: TileRect,
    /// The exit-direction tolerance leaves record (`lighting::tau_omega`).
    pub tau_omega: f32,
    /// The path tracer's `max_bounces`: a branch with more events contributes zero (§5.4).
    pub max_bounces: u32,
    /// The absorption sub-node caches of the tile's beam trees (§9.1). `None` when the depths are
    /// not tracked (see [`build_subnodes`]): every state's `depth` then stays empty.
    pub subnodes: Option<SubnodeCaches<'a>>,
    /// The hero contexts of the intervals off the group grid, built once per interval.
    pub interval_heroes: IntervalHeroes<'a>,
    /// The render's cancel token, polled inside the entry, interaction and exit-leaf recursions;
    /// `None` never cancels. Set with [`InteractKit::with_cancel`].
    pub cancel: Option<&'k AtomicBool>,
    /// The attempted work one tile may spend before its pending beams go to the residual
    /// ([`TILE_ALLOWANCE`](super::stats::TILE_ALLOWANCE) unless the render gives a budget, see
    /// `pipeline::tile_budget`).
    pub allowance: u64,
}

impl<'k> InteractKit<'k, '_> {
    /// This kit with `allowance` units of attempted work per tile.
    #[must_use]
    pub const fn with_allowance(mut self, allowance: u64) -> Self {
        self.allowance = allowance;
        self
    }

    /// This kit with `cancel` polled inside its recursions.
    #[must_use]
    pub const fn with_cancel(mut self, cancel: &'k AtomicBool) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// `true` once the kit's cancel token is set; always `false` without one.
    #[must_use]
    pub fn cancelled(&self) -> bool {
        self.cancel
            .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
    }
}

/// The hero contexts of the groups of one interval: the kit's own for an interval that fills whole
/// groups of the full node set, a cached set built for the interval otherwise.
pub enum KitHeroes<'h, 'a> {
    /// A slice of the kit's contexts (hero in slot [`GROUP_HERO`]).
    Shared(&'h [HeroContext<'a>]),
    /// Contexts built for an interval off the group grid, hero at the interval's centre node.
    Own(Arc<[HeroContext<'a>]>),
}

impl<'a> KitHeroes<'_, 'a> {
    /// The context of group `g` of the interval.
    #[must_use]
    pub fn get(&self, g: usize) -> Option<&HeroContext<'a>> {
        match self {
            Self::Shared(contexts) => contexts.get(g),
            Self::Own(contexts) => contexts.get(g),
        }
    }
}

/// The hero context sets of [`IntervalHeroes`], keyed by an interval's `(first, last)` nodes.
type HeroSets<'a> = BTreeMap<(u16, u16), Arc<[HeroContext<'a>]>>;

/// The per-interval cache of the hero contexts of intervals off the group grid (halves and single
/// nodes of a split interval): each is built once instead of at every beam.
#[derive(Default)]
pub struct IntervalHeroes<'a> {
    /// The built sets, keyed by the interval's `(first, last)` nodes.
    sets: Mutex<HeroSets<'a>>,
}

/// `true` when `spectrum` fills whole 8-node groups of the full node set: the kit's contexts
/// (hero in slot [`GROUP_HERO`]) serve it.
#[must_use]
pub fn on_group_grid(spectrum: SpectralInterval) -> bool {
    usize::from(spectrum.first).is_multiple_of(GROUP)
        && (usize::from(spectrum.last) + 1).is_multiple_of(GROUP)
}

/// The comb slot of the hero of group `g` of `spectrum`.
///
/// On the group grid every group keeps [`GROUP_HERO`], and the shared geometry is the hero of the
/// centre's group. That is the centre node only when the centre sits in slot [`GROUP_HERO`]; an
/// on-grid interval of several groups with the centre elsewhere only occurs for materials whose
/// geometry does not depend on the node. Off the grid the group holding the centre uses the
/// centre node's slot, since the topology is the centre node's (§9.2); the other groups keep
/// [`GROUP_HERO`].
#[must_use]
pub fn hero_slot(spectrum: SpectralInterval, g: usize) -> usize {
    if on_group_grid(spectrum) {
        return GROUP_HERO;
    }
    let start = usize::from(spectrum.first) + g * GROUP;
    let centre = usize::from(spectrum.centre());
    if centre >= start && centre < start + GROUP {
        centre - start
    } else {
        GROUP_HERO
    }
}

/// The node whose geometry group `g` of `spectrum` is replayed with: the group's hero slot,
/// clamped to the interval.
#[must_use]
pub fn hero_node(spectrum: SpectralInterval, g: usize) -> u16 {
    let node = (usize::from(spectrum.first) + g * GROUP + hero_slot(spectrum, g))
        .min(usize::from(spectrum.last));
    u16::try_from(node).unwrap_or(spectrum.last)
}

impl<'a> IntervalHeroes<'a> {
    /// The set built for `spectrum`, if any.
    fn find(&self, spectrum: SpectralInterval) -> Option<Arc<[HeroContext<'a>]>> {
        let sets = self.sets.lock().unwrap_or_else(PoisonError::into_inner);
        sets.get(&(spectrum.first, spectrum.last)).cloned()
    }

    /// Remembers the set built for `spectrum` (the first set built for an interval stays).
    fn insert(&self, spectrum: SpectralInterval, set: &Arc<[HeroContext<'a>]>) {
        self.sets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry((spectrum.first, spectrum.last))
            .or_insert_with(|| Arc::clone(set));
    }
}

impl<'a> InteractKit<'_, 'a> {
    /// Hero contexts of the groups of `spectrum`: the kit's for an interval on the group grid,
    /// otherwise a set built once per interval (hero at the centre node, see [`hero_slot`]).
    #[must_use]
    pub fn group_heroes(&self, spectrum: SpectralInterval) -> KitHeroes<'_, 'a> {
        let groups = spectrum.groups();
        if on_group_grid(spectrum) {
            let first = usize::from(spectrum.first) / GROUP;
            if let Some(shared) = self.heroes.get(first..first + groups) {
                return KitHeroes::Shared(shared);
            }
        }
        if let Some(set) = self.interval_heroes.find(spectrum) {
            return KitHeroes::Own(set);
        }
        let built: Arc<[HeroContext<'a>]> = (0..groups)
            .map(|g| {
                let lambdas = spectrum.group_lambdas(g, self.retrace.n_lambda);
                let (ctx, cache) = self.prepared.ray_context(lambdas, hero_slot(spectrum, g));
                HeroContext { ctx, cache }
            })
            .collect();
        self.interval_heroes.insert(spectrum, &built);
        KitHeroes::Own(built)
    }

    /// Adds one interior segment's optical depth to the sub-node depths of `state` (§9.1), if the
    /// kit tracks them; `wave` is the segment's `(k, is_ext)` before the bounce.
    ///
    /// Returns `false` when the depth layout of `state` does not match `spectrum` (the caller
    /// treats the state as unusable); `true` otherwise, also when nothing is tracked.
    pub fn add_depth(
        &self,
        state: &mut SampleState,
        depths: &SegmentDepths,
        wave: (Vec3, bool),
        spectrum: SpectralInterval,
    ) -> bool {
        self.subnodes
            .as_ref()
            .is_none_or(|caches| accumulate_depths(state, depths, wave, spectrum, caches))
    }
}

/// `true` when `bands` has a band that absorbs.
fn bands_absorb(bands: &[AbsorptionBand]) -> bool {
    bands.iter().any(|band| band.peak > 0.0)
}

/// The absorption sub-node caches of a frame, or `None` when the depths are not tracked.
///
/// Tracked when `settings.absorption_subnodes >= 2` and the material absorbs (a band with a
/// positive peak, or a zoned material). A dispersive material that does not absorb gets only the
/// CIE-weight sub-node quadrature from the reflection kit
/// (`SubnodeCaches::for_dispersive`). Only a non-dispersive colorless material (glass) has none
/// and renders as without sub-nodes. With one sub-node per node the fold is the identity.
#[must_use]
pub fn build_subnodes<'a>(
    prepared: &PreparedMaterial<'a>,
    heroes: &[HeroContext<'a>],
    material: &GemMaterial,
    n_lambda: u32,
    settings: &BeamSettings,
) -> Option<SubnodeCaches<'a>> {
    let tensor = &material.absorption;
    let absorbs = bands_absorb(&tensor.o_ray)
        || bands_absorb(&tensor.e_ray)
        || tensor.beta_ray.as_deref().is_some_and(bands_absorb)
        || heroes.iter().any(|hero| hero.cache.zoned.is_some());
    (settings.absorption_subnodes >= 2 && absorbs)
        .then(|| SubnodeCaches::new(prepared, n_lambda, settings.absorption_subnodes))
}

/// The outcome of one internal hit of a beam on its carrier facet.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InternalHit {
    /// The reflected child beams (partial and TIR), still inside, to propagate further.
    pub reflected: Vec<Beam>,
    /// The transmitted exit leaves (with the recorded `τ_ω` and prefixes).
    pub transmitted: Vec<Leaf>,
}

// ---- small helpers ----

/// An `f64` as `f32`, the precision of the tracer's states.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the sample states are f32 by design"
)]
const fn to_f32(v: f64) -> f32 {
    v as f32
}

/// A count as `f64` (counts here are tiny).
fn count_f64(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// The number of nodes of group `g` of an interval of `nodes` nodes.
fn group_nodes(nodes: usize, g: usize) -> usize {
    nodes.saturating_sub(g * GROUP).min(GROUP)
}

/// The group that supplies a beam's shared geometry: the one holding the interval's centre node.
fn geometry_group(beam: &Beam) -> usize {
    usize::from(beam.spectrum.centre() - beam.spectrum.first) / GROUP
}

/// The wavelength of the centre node of the beam's interval, in nm (the shared topology).
fn centre_lambda(beam: &Beam, kit: &InteractKit<'_, '_>) -> f32 {
    SpectralNode(beam.spectrum.centre()).centre_nm(kit.retrace.n_lambda)
}

/// The largest incoming Stokes intensity over the sample points, groups and nodes of `beam`.
fn incoming_peak(beam: &Beam) -> f64 {
    let nodes = beam.spectrum.len();
    let mut best = 0.0_f64;
    for state in &beam.state.points {
        for (g, group) in state.stokes.iter().enumerate() {
            for stokes in group.iter().take(group_nodes(nodes, g)) {
                best = best.max(f64::from(stokes.i));
            }
        }
    }
    best
}

/// The image points of the sample states of `image`: vertices, centroid, edge midpoints.
fn sample_points(image: &ConvexPolygon) -> (Vec<DVec2>, usize) {
    let (v, n) = image.points_2d();
    let mut points: Vec<DVec2> = v[..n].to_vec();
    points.push(image.centroid().truncate());
    points.extend((0..n).map(|i| (v[i] + v[(i + 1) % n]) * 0.5));
    (points, n)
}

// ---- the model of the critical curve (§8.5) ----

/// The circle in which the cone of critical incidence through an apex meets a facet plane (§8.5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelCircle {
    /// The foot of the apex on the plane, the circle's centre.
    pub centre: DVec3,
    /// The radius `h · tan θc`, `h` the distance of the apex from the plane.
    pub radius: f64,
}

/// The model circle of the apex `apex` on `plane = (normal, offset)` for the refractive index
/// `index` of the stone (the outside is air).
///
/// For isotropic media the rays through the apex at critical incidence form a cone about the
/// plane's normal; it meets the plane in the circle centred on the foot of the apex with radius
/// `h · tan θc = h / sqrt(n² − 1)`. Points inside the circle are below the critical angle. `None`
/// when `index <= 1` (no total reflection exists), the apex lies on the plane, or an input is not
/// finite.
#[must_use]
pub fn model_circle(apex: DVec3, plane: (DVec3, f64), index: f64) -> Option<ModelCircle> {
    let len = plane.0.length();
    if !(len.is_finite() && len > 0.0 && index.is_finite() && index > 1.0) {
        return None;
    }
    let unit = plane.0 / len;
    let signed = unit.dot(apex) - plane.1 / len;
    let radius = signed.abs() / index.mul_add(index, -1.0).sqrt();
    let centre = unit.mul_add(DVec3::splat(-signed), apex);
    (radius.is_finite() && radius > 0.0 && centre.is_finite())
        .then_some(ModelCircle { centre, radius })
}

/// The mirror image of `apex` in the plane `plane = (normal, offset)`: the apex of a reflected
/// pencil (exact for isotropic media, §8.5).
#[must_use]
pub fn mirror_apex(apex: DVec3, plane: (DVec3, f64)) -> DVec3 {
    let len = plane.0.length();
    let unit = plane.0 / len;
    let signed = unit.dot(apex) - plane.1 / len;
    unit.mul_add(DVec3::splat(-2.0 * signed), apex)
}

/// A model boundary polyline in image space and the class on each side of it.
struct ModelBoundary {
    /// From one parent edge to another.
    polyline: Vec<DVec2>,
    /// The classes left and right of the polyline's direction.
    sides: (u32, u32),
}

/// One crossing of the model circle with the image ring.
struct Crossing {
    /// The point on the ring edge, in image space.
    point: DVec2,
    /// The angle of the point around the circle's centre, in the plane.
    angle: f64,
}

/// `true` when `q` lies inside or on the counter-clockwise convex ring.
fn inside_ring(ring: &[DVec2], q: DVec2) -> bool {
    let n = ring.len();
    (0..n).all(|i| orient2(ring[i], ring[(i + 1) % n], q) >= 0.0)
}

/// The image-edge fraction `t` whose plane position lies the fraction `along` of the way along the
/// plane edge, for the homogeneous weights `wa` and `wb` of the edge's end points.
///
/// The forward map is `s = t·wb / ((1−t)·wa + t·wb)`; this is its inverse,
/// `t = s·wa / ((1−s)·wb + s·wa)`. `None` when the denominator is zero or nearly so (the point is
/// at infinity).
#[must_use]
pub fn image_fraction(along: f64, wa: f64, wb: f64) -> Option<f64> {
    let denominator = (1.0 - along).mul_add(wb, along * wa);
    (denominator.abs() > f64::EPSILON * (wa.abs() + wb.abs())).then(|| along * wa / denominator)
}

/// The crossings of the plane circle (`centre`, `radius`) with the edges of `ring`, using the
/// beam's `image_to_plane` for the plane position of the ring vertices (half-open edges, so a
/// crossing at a vertex is counted once). `None` when a vertex maps to infinity.
fn circle_crossings(
    ring: &[DVec2],
    image_to_plane: glam::DMat3,
    (centre, radius): (DVec2, f64),
) -> Option<Vec<Crossing>> {
    let hom: Vec<DVec3> = ring
        .iter()
        .map(|q| image_to_plane * q.extend(1.0))
        .collect();
    if hom
        .iter()
        .any(|p| !p.is_finite() || p.z.abs() <= f64::MIN_POSITIVE)
    {
        return None;
    }
    let plane: Vec<DVec2> = hom
        .iter()
        .map(|p| DVec2::new(p.x / p.z, p.y / p.z))
        .collect();
    let count = ring.len();
    let mut crossings = Vec::new();
    for i in 0..count {
        let next = (i + 1) % count;
        let edge = plane[next] - plane[i];
        let rel = plane[i] - centre;
        let qa = edge.length_squared();
        if qa <= 0.0 {
            continue;
        }
        let qb = 2.0 * rel.dot(edge);
        let qc = radius.mul_add(-radius, rel.length_squared());
        let disc = qb.mul_add(qb, -4.0 * qa * qc);
        if disc <= 0.0 {
            continue;
        }
        let root = disc.sqrt();
        for along in [(-qb - root) / (2.0 * qa), (-qb + root) / (2.0 * qa)] {
            if !(0.0..1.0).contains(&along) {
                continue;
            }
            // The edge parameter of the image point whose plane position is `along` the edge.
            let Some(image_param) = image_fraction(along, hom[i].z, hom[next].z) else {
                continue;
            };
            let point = (ring[next] - ring[i]).mul_add(DVec2::splat(image_param), ring[i]);
            let at = edge.mul_add(DVec2::splat(along), plane[i]) - centre;
            crossings.push(Crossing {
                point,
                angle: at.y.atan2(at.x),
            });
        }
    }
    Some(crossings)
}

/// The model boundary of the circle of critical incidence through the apex, as a polyline in
/// image space across the beam's image ring (§8.5 "initial chords"). `None` unless the circle
/// crosses the ring exactly twice.
fn circle_model(
    beam: &Beam,
    ring: &[DVec2],
    lambda_nm: f32,
    kit: &InteractKit<'_, '_>,
) -> Option<ModelBoundary> {
    let index = f64::from(kit.retrace.material.dispersion.evaluate(lambda_nm));
    let circle = model_circle(beam.apex, kit.retrace.stone.plane(beam.facet), index)?;
    let centre = beam.polygon.frame().to_2d(circle.centre);
    let radius = circle.radius;
    let back = inverse(beam.image_to_plane)?;
    let crossings = circle_crossings(ring, beam.image_to_plane, (centre, radius))?;
    let [first, second] = crossings.as_slice() else {
        return None;
    };
    let at =
        |angle: f64, scale: f64| centre + DVec2::new(angle.cos(), angle.sin()) * (radius * scale);
    for (from, to) in [(first, second), (second, first)] {
        let sweep = (to.angle - from.angle).rem_euclid(std::f64::consts::TAU);
        if sweep <= 0.0 {
            continue;
        }
        let mid_angle = 0.5_f64.mul_add(sweep, from.angle);
        let mid = apply(back, at(mid_angle, 1.0))?;
        if !inside_ring(ring, mid) {
            continue;
        }
        let mut segments = MIN_ARC_SEGMENTS;
        while segments < MAX_ARC_SEGMENTS && sweep / count_f64(segments) > ARC_STEP_RAD {
            segments += 1;
        }
        let mut polyline = vec![from.point];
        for k in 1..segments {
            let angle = (sweep / count_f64(segments)).mul_add(count_f64(k), from.angle);
            polyline.push(apply(back, at(angle, 1.0))?);
        }
        polyline.push(to.point);
        let inner = apply(back, at(mid_angle, 1.0 - INNER_FRACTION))?;
        let k = polyline.len() / 2;
        let tangent = polyline[k] - polyline[k - 1];
        let inner_left = tangent.perp_dot(inner - mid) > 0.0;
        let sides = if inner_left {
            (PARTIAL_CLASS, TIR_CLASS)
        } else {
            (TIR_CLASS, PARTIAL_CLASS)
        };
        return Some(ModelBoundary { polyline, sides });
    }
    None
}

/// The model boundary for the anisotropic or fallback case: the chord between the midpoints of
/// the two ring edges whose end vertices classify differently (§8.5). `None` unless exactly two
/// edges change class.
fn chord_model(ring: &[DVec2], flags: &[bool]) -> Option<ModelBoundary> {
    let n = ring.len();
    if flags.len() < n {
        return None;
    }
    let changes: Vec<usize> = (0..n).filter(|&i| flags[i] != flags[(i + 1) % n]).collect();
    let [entry, exit] = changes.as_slice() else {
        return None;
    };
    let mid = |edge: usize| (ring[edge] + ring[(edge + 1) % n]) * 0.5;
    let left_is_tir = flags[(exit + 1) % n];
    let sides = if left_is_tir {
        (TIR_CLASS, PARTIAL_CLASS)
    } else {
        (PARTIAL_CLASS, TIR_CLASS)
    };
    Some(ModelBoundary {
        polyline: vec![mid(*entry), mid(*exit)],
        sides,
    })
}

// ---- classification ----

/// Classifies the exact ray through an image point as partial (`0`) or total reflection (`1`) on
/// the beam's facet.
struct TirClassifier<'k> {
    kit: RetraceKit<'k>,
    prefix: BranchPrefix,
    lambda_nm: f32,
    facet: u32,
}

impl Classifier for TirClassifier<'_> {
    fn classify(&self, image_point: DVec2) -> Option<u32> {
        let ray = exact_ray_at(image_point, self.lambda_nm, &self.prefix, &self.kit).ok()?;
        Some(
            if ray_is_total_reflection(&ray, self.facet, self.lambda_nm, &self.kit) {
                TIR_CLASS
            } else {
                PARTIAL_CLASS
            },
        )
    }
}

/// For every sample point of `beam` (in the order of its states): `true` when the exact ray
/// meets the carrier facet past the critical angle at the interval's centre node (§8.5).
///
/// The exact ray comes from the retracer; a point the retracer refuses falls back to the state's
/// own direction. In a birefringent stone the test is the mode's own, at the wave normal `k`
/// (§10.1, §10.3).
#[must_use]
pub fn tir_flags(beam: &Beam, kit: &InteractKit<'_, '_>) -> Vec<bool> {
    let lambda_nm = centre_lambda(beam, kit);
    let uniaxial = is_anisotropic(kit.retrace.material);
    beam.state
        .points
        .iter()
        .map(
            |state| match exact_ray_at(state.image, lambda_nm, &beam.prefix, &kit.retrace) {
                Ok(ray) => ray_is_total_reflection(&ray, beam.facet, lambda_nm, &kit.retrace),
                Err(_) if uniaxial => state_is_total_reflection(
                    (state.k, state.is_ext),
                    beam.facet,
                    lambda_nm,
                    &kit.retrace,
                ),
                Err(_) => is_total_reflection(
                    state.ray.dir.as_dvec3(),
                    beam.facet,
                    lambda_nm,
                    &kit.retrace,
                ),
            },
        )
        .collect()
}

// ---- the TIR split ----

/// The pieces of a split beam by class.
#[derive(Default)]
struct TirSplit {
    partial: Vec<Beam>,
    tir: Vec<Beam>,
}

/// The piece of `parent` over the image polygon `image`: a carrier-plane polygon and freshly
/// retraced sample states (§7.3).
fn piece_beam(parent: &Beam, image: &ConvexPolygon, kit: &InteractKit<'_, '_>) -> Option<Beam> {
    let polygon = image.to_plane(parent.image_to_plane, *parent.polygon.frame())?;
    let (points, vertices) = sample_points(image);
    let states = points
        .iter()
        .map(|&q| retrace_state(q, parent, kit))
        .collect::<Option<Vec<_>>>()?;
    Some(Beam {
        facet: parent.facet,
        polygon,
        apex: parent.apex,
        image_to_plane: parent.image_to_plane,
        inside: parent.inside,
        sig: parent.sig,
        prefix: parent.prefix,
        bounces: parent.bounces,
        spectrum: parent.spectrum,
        state: SampleStates {
            points: states,
            vertices,
        },
    })
}

/// Bisects `beam` across the longest edge of its image polygon into two beams with fresh states.
fn bisect(beam: &Beam, kit: &InteractKit<'_, '_>) -> Option<(Beam, Beam)> {
    let (first, second) = image_polygon(beam)?.split_longest_edge()?;
    Some((
        piece_beam(beam, &first, kit)?,
        piece_beam(beam, &second, kit)?,
    ))
}

/// The snapped image polygons of the critical curve cut, each with its class (`true` = TIR).
/// `None` when no model snaps (a bracket failure) or the cut fails.
fn snap_split(
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    seams: &mut SeamStore,
    stats: &mut BeamStats,
    flags: &[bool],
) -> Option<Vec<(ConvexPolygon, bool)>> {
    let parent = image_polygon(beam)?;
    let (points, n) = parent.points_2d();
    let ring = &points[..n];
    let lambda_nm = centre_lambda(beam, kit);
    let classifier = TirClassifier {
        kit: kit.retrace,
        prefix: beam.prefix,
        lambda_nm,
        facet: beam.facet,
    };
    let tol = Tolerances {
        chord_px: kit.settings.chord_tolerance_px,
        ..Tolerances::DEFAULT
    };
    // The critical curve of an anisotropic mode is no circle (the e-ray's index depends on the
    // direction and its rays share no apex): the initial chords join the polygon's edge crossings,
    // found by the classification of the edges' end vertices, and the snap bisects them (§8.5).
    let circle = if is_anisotropic(kit.retrace.material) {
        None
    } else {
        circle_model(beam, ring, lambda_nm, kit)
    };
    let models = [circle, chord_model(ring, flags)];
    for model in models.into_iter().flatten() {
        let request = SnapRequest {
            parent,
            model: model.polyline,
            classifier: &classifier,
            sides: model.sides,
            boundary: BoundaryKind::Tir { facet: beam.facet },
            tile: kit.tile,
            lambda_nm,
            prefix: beam.prefix,
            cancel: kit.cancel,
        };
        let Ok(polyline) = snap_boundary(&request, &tol, seams, stats) else {
            continue;
        };
        let Some((left, right)) = split_snapped(&parent, &polyline, &tol) else {
            continue;
        };
        let mut out: Vec<(ConvexPolygon, bool)> = Vec::new();
        out.extend(left.into_iter().map(|p| (p, model.sides.0 == TIR_CLASS)));
        out.extend(right.into_iter().map(|p| (p, model.sides.1 == TIR_CLASS)));
        return Some(out);
    }
    None
}

/// The beams over the snapped polygons, slivers dropped (and counted). `None` when a piece cannot
/// be retraced or nothing is left.
fn build_pieces(
    beam: &Beam,
    polygons: &[(ConvexPolygon, bool)],
    kit: &InteractKit<'_, '_>,
    stats: &mut BeamStats,
) -> Option<Vec<(Beam, bool)>> {
    let peak = incoming_peak(beam);
    let mut slivers = Vec::new();
    let mut pieces = Vec::with_capacity(polygons.len());
    for (image, tir) in polygons {
        let area = image.area();
        if area < Tolerances::DEFAULT.sliver_area_px2 {
            slivers.push(area * peak);
            continue;
        }
        pieces.push((piece_beam(beam, image, kit)?, *tir));
    }
    if pieces.is_empty() {
        return None;
    }
    for energy in slivers {
        stats.record_drop(DropReason::Sliver, energy);
    }
    Some(pieces)
}

/// Splits `beam` into `out`, bisecting after a bracket failure (§7.4 step 8).
fn split_into(
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    seams: &mut SeamStore,
    stats: &mut BeamStats,
    retry: u32,
    out: &mut TirSplit,
) {
    let flags = tir_flags(beam, kit);
    if !flags.is_empty() && flags.iter().all(|&f| f) {
        out.tir.push(beam.clone());
        return;
    }
    if flags.iter().all(|&f| !f) {
        out.partial.push(beam.clone());
        return;
    }
    if let Some(polygons) = snap_split(beam, kit, seams, stats, &flags)
        && let Some(pieces) = build_pieces(beam, &polygons, kit, stats)
    {
        stats.tir_splits += 1;
        // The critical curve of an anisotropic mode is a topology change inside the beam (§10.2).
        stats.mode_splits += u64::from(is_anisotropic(kit.retrace.material));
        for (piece, tir) in pieces {
            if tir {
                out.tir.push(piece);
            } else {
                out.partial.push(piece);
            }
        }
        return;
    }
    let area = image_polygon(beam).map_or(0.0, |image| image.area());
    if beam_near_axis(beam, kit.retrace.material, kit.retrace.n_lambda) {
        // Not subdivided further near an optic axis: the residual covers it (§10.2, §10.3).
        stats.record_drop(DropReason::OpticAxis, area * incoming_peak(beam));
        return;
    }
    if retry < MAX_TIR_RETRIES
        && let Some((first, second)) = bisect(beam, kit)
    {
        split_into(&first, kit, seams, stats, retry + 1, out);
        split_into(&second, kit, seams, stats, retry + 1, out);
        return;
    }
    stats.record_drop(DropReason::BracketFailure, area * incoming_peak(beam));
}

/// Splits a beam on its facet at the exact critical curve (§8.5): `(partial, tir)` pieces whose
/// polygons tile the beam's polygon. A beam whose sample points all agree is returned whole on
/// its side.
///
/// The classification is the exact ray's at the interval's centre node ([`tir_flags`]). When the
/// points disagree the model boundary is snapped to the exact curve ([`snap_boundary`], kind
/// `BoundaryKind::Tir`), cut with [`split_snapped`], and each piece gets freshly retraced sample
/// states. A bracket failure bisects the beam and retries, at most twice; a region that still
/// fails is dropped (`BracketFailure`). Counts `tir_splits` per successful cut.
#[must_use]
pub fn split_tir(
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    seams: &mut SeamStore,
    stats: &mut BeamStats,
) -> (Vec<Beam>, Vec<Beam>) {
    let mut out = TirSplit::default();
    split_into(beam, kit, seams, stats, 0, &mut out);
    (out.partial, out.tir)
}

// ---- the hit ----

/// The expansion of one sample point at its exact hit on the carrier facet.
struct PointExp {
    /// The hit point, in `f32` as the expansion sees it.
    hit: Vec3,
    /// The plane-of-incidence normal of the geometry group.
    plane_normal: Vec3,
    /// The length of the segment from the state's origin to the hit.
    length: f32,
    /// `η² sin² θ − 1` at the point: zero on the critical curve.
    margin: f64,
    /// The grazing direction (tangent to the facet), the exact transmitted direction at `θc`.
    grazing: Vec3,
    /// One expansion per group of the beam's interval.
    groups: Vec<Expansion>,
}

/// One mode's children at every sample point and group (index `point * groups + group`).
struct ModeChildren {
    is_ext: bool,
    children: Vec<Child>,
}

/// The reflected and transmitted children of a piece, by mode.
struct Outcomes {
    reflect: Vec<ModeChildren>,
    transmit: Vec<ModeChildren>,
    /// Per sample point: its class was forced to the piece's class (on the critical curve).
    flipped: Vec<bool>,
}

/// Why a piece cannot be accepted as it is.
#[derive(Clone, Copy)]
enum Failure {
    /// Subdivide; at the depth cap the piece is dropped with this reason.
    Split(DropReason),
    /// The sample points of a birefringent piece disagree on the mode set (a relabelling flip,
    /// §10.2): subdivide, counted as a `mode_splits`; at the depth cap dropped as `DepthCap`.
    Modes,
    /// Drop now (a `diverged` child).
    Drop(DropReason),
}

/// What one piece turned into.
struct Accepted {
    /// The reflected child beams with their mode.
    reflected: Vec<(bool, Beam)>,
    /// The transmitted beams with their mode, to become exit leaves.
    transmitted: Vec<(bool, Beam)>,
    /// The drops to record.
    drops: Vec<(DropReason, f64)>,
    /// The reflected child beams whose apex was refitted instead of mirrored (§10.1).
    refits: u64,
}

/// The modes (`is_ext`) of the reflected (or transmitted) children of an expansion, in order.
fn modes_of(expansion: &Expansion, transmit: bool) -> Vec<bool> {
    expansion
        .children
        .iter()
        .filter(|c| (c.kind == EventKind::Transmit) == transmit)
        .map(|c| c.state.is_ext)
        .collect()
}

/// The child of the given family and mode.
fn find_child(expansion: &Expansion, transmit: bool, is_ext: bool) -> Option<Child> {
    expansion
        .children
        .iter()
        .find(|c| (c.kind == EventKind::Transmit) == transmit && c.state.is_ext == is_ext)
        .copied()
}

/// The transmitted child at the critical angle: zero intensity along the facet (`T = 0`, the
/// exact limit from below).
const fn grazing_transmit(point: &PointExp, is_ext: bool) -> Child {
    Child {
        state: PathState {
            k: point.grazing,
            inside: false,
            is_ext,
            stokes: [StokesVector::unpolarized(0.0); GROUP],
        },
        s: point.grazing,
        kind: EventKind::Transmit,
        diverged: false,
    }
}

/// The children of one mode at every point and group; a missing transmitted child (a point on the
/// critical curve) is the grazing limit.
fn mode_children(points: &[PointExp], transmit: bool, is_ext: bool) -> Option<ModeChildren> {
    let mut children = Vec::new();
    for point in points {
        for expansion in &point.groups {
            match find_child(expansion, transmit, is_ext) {
                Some(child) => children.push(child),
                None if transmit => children.push(grazing_transmit(point, is_ext)),
                None => return None,
            }
        }
    }
    Some(ModeChildren { is_ext, children })
}

/// Checks that every point and group of a piece returns one child set and collects the children
/// by mode (§8.1, §8.5). Points on the critical curve are coerced to the class of the piece.
fn gather(
    points: &[PointExp],
    tir: bool,
    geo: usize,
    birefringent: bool,
) -> Result<Outcomes, Failure> {
    let split = Failure::Split(DropReason::DepthCap);
    // Points that disagree on the modes of a birefringent piece are a relabelling flip (§10.2).
    let mismatch = if birefringent { Failure::Modes } else { split };
    if points
        .iter()
        .flat_map(|p| p.groups.iter())
        .any(|e| e.children.iter().any(|c| c.diverged))
    {
        return Err(Failure::Drop(DropReason::Diverged));
    }
    let reference = points
        .first()
        .and_then(|p| p.groups.get(geo))
        .ok_or(split)?;
    let reflect_modes = modes_of(reference, false);
    if reflect_modes.is_empty() {
        return Err(split);
    }
    let transmit_modes = if tir {
        Vec::new()
    } else {
        points
            .iter()
            .filter_map(|p| p.groups.get(geo))
            .map(|e| modes_of(e, true))
            .find(|modes| !modes.is_empty())
            .unwrap_or_default()
    };
    let mut flipped = vec![false; points.len()];
    for (index, point) in points.iter().enumerate() {
        let near = point.margin.abs() <= NEAR_CRITICAL;
        for expansion in &point.groups {
            if modes_of(expansion, false) != reflect_modes {
                return Err(mismatch);
            }
            if modes_of(expansion, true) != transmit_modes {
                if !near {
                    return Err(mismatch);
                }
                flipped[index] = true;
            }
        }
    }
    let reflect = reflect_modes
        .iter()
        .map(|&mode| mode_children(points, false, mode))
        .collect::<Option<Vec<_>>>()
        .ok_or(split)?;
    let transmit = transmit_modes
        .iter()
        .map(|&mode| mode_children(points, true, mode))
        .collect::<Option<Vec<_>>>()
        .ok_or(split)?;
    Ok(Outcomes {
        reflect,
        transmit,
        flipped,
    })
}

/// The largest Stokes intensity of `children` over points, groups and nodes.
fn children_peak(beam: &Beam, children: &[Child], groups: usize) -> f64 {
    let nodes = beam.spectrum.len();
    let mut best = 0.0_f64;
    for (index, child) in children.iter().enumerate() {
        let g = index % groups.max(1);
        for stokes in child.state.stokes.iter().take(group_nodes(nodes, g)) {
            best = best.max(f64::from(stokes.i));
        }
    }
    best
}

/// `true` when the exact Stokes intensity at the centroid and the edge midpoints matches the
/// Wachspress interpolation from the vertices within `τ_w · max(I_vertices) + τ_abs`, for every
/// node (§8.4). Check points on the critical curve (`flipped`) are exempt: their class is
/// ambiguous within the chord tolerance.
fn interpolates(
    beam: &Beam,
    image: &ConvexPolygon,
    children: &[Child],
    flipped: &[bool],
    tau_w: f32,
) -> bool {
    let vertices = beam.state.vertices;
    let points = beam.state.points.len();
    let groups = beam.spectrum.groups();
    if children.len() != points * groups || points <= vertices {
        return false;
    }
    let checks: Vec<[f64; MAX_VERTICES]> = beam.state.points[vertices..]
        .iter()
        .map(|s| image.wachspress(s.image))
        .collect();
    let nodes = beam.spectrum.len();
    for g in 0..groups {
        for slot in 0..group_nodes(nodes, g) {
            let value = |p: usize| {
                children
                    .get(p * groups + g)
                    .and_then(|c| c.state.stokes.get(slot))
                    .map(|s| f64::from(s.i))
            };
            let Some(corner): Option<Vec<f64>> = (0..vertices).map(value).collect() else {
                return false;
            };
            let peak = corner.iter().copied().fold(0.0, f64::max);
            let tolerance = f64::from(tau_w).mul_add(peak, f64::from(WEIGHT_ABS_TOLERANCE));
            for (k, weights) in checks.iter().enumerate() {
                let p = vertices + k;
                if flipped.get(p).copied().unwrap_or(false) {
                    continue;
                }
                let Some(exact) = value(p) else {
                    return false;
                };
                let mut interpolated = 0.0;
                for (w, v) in weights.iter().zip(&corner) {
                    interpolated = w.mul_add(*v, interpolated);
                }
                if (exact - interpolated).abs() > tolerance {
                    return false;
                }
            }
        }
    }
    true
}

/// The largest per-point spectral-mean intensity `expand` dropped below `MIN_PROBABILITY`.
fn dropped_intensity(points: &[PointExp]) -> f64 {
    points
        .iter()
        .map(|p| {
            let sum: f32 = p.groups.iter().map(|e| e.dropped).sum();
            f64::from(sum) / count_f64(p.groups.len().max(1))
        })
        .fold(0.0, f64::max)
}

/// The state of one tile's internal hit under construction.
struct Run<'r, 'k, 'a> {
    kit: &'r InteractKit<'k, 'a>,
    heroes: KitHeroes<'r, 'a>,
    stats: &'r mut BeamStats,
    /// The carrier facet's original index.
    original: u32,
    /// The carrier facet's unit outward normal.
    normal: DVec3,
    /// The refractive index at the interval's centre node.
    eta: f64,
    /// The stone is birefringent (uniaxial or biaxial): modes, refits and the optic-axis guard
    /// apply (§10).
    uniaxial: bool,
    /// The wavelength of the interval's centre node, in nm.
    lambda_nm: f32,
    /// Reflected children can still reach an exit within `max_bounces` (§5.4).
    reflect_alive: bool,
    /// Reflected children by mode (`is_ext`).
    reflected: [Vec<Beam>; 2],
    /// Exit leaves by mode.
    transmitted: [Vec<Leaf>; 2],
}

impl Run<'_, '_, '_> {
    /// Expands the sample `state` of `piece` at its exact hit on the carrier plane.
    fn expand_point(&self, piece: &Beam, state: &SampleState) -> Option<PointExp> {
        let (n, m) = self.kit.retrace.stone.plane(piece.facet);
        let origin = state.segment_origin.as_dvec3();
        let dir = state.ray.dir.as_dvec3();
        let denominator = n.dot(dir);
        if denominator.abs() <= f64::EPSILON {
            return None;
        }
        let t = (m - n.dot(origin)) / denominator;
        if !(t.is_finite() && t > 0.0) {
            return None;
        }
        let hit_f64 = dir.mul_add(DVec3::splat(t), origin);
        let hit = hit_f64.as_vec3();
        let facet = self.original as usize;
        let explicit = ExplicitHit {
            point: hit,
            facet,
            normal: Vec3::from(self.kit.planes.get(facet)?.normal),
        };
        let ray = Ray {
            origin: state.segment_origin,
            dir: state.ray.dir,
        };
        let groups = (0..piece.spectrum.groups())
            .map(|g| {
                let path = PathState {
                    k: state.k,
                    inside: state.inside,
                    is_ext: state.is_ext,
                    stokes: *state.stokes.get(g)?,
                };
                Some(expand_at(
                    self.heroes.get(g)?,
                    self.kit.planes,
                    path,
                    ray,
                    explicit,
                    Some(!state.inside),
                    state.prev_plane,
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        let plane_normal = groups.get(geometry_group(piece))?.plane_normal;
        let (cos, eta) = if self.uniaxial {
            // The mode's own index at the wave normal, the angle measured on `k` (§10.1).
            let index = mode_index(
                self.kit.retrace.material,
                self.lambda_nm,
                state.k,
                state.is_ext,
            );
            (state.k.as_dvec3().dot(self.normal), f64::from(index))
        } else {
            (dir.dot(self.normal), self.eta)
        };
        let margin = (eta * eta).mul_add((-cos).mul_add(cos, 1.0), -1.0);
        let unit = self.normal.as_vec3();
        let grazing = (state.ray.dir - unit * state.ray.dir.dot(unit)).normalize_or_zero();
        Some(PointExp {
            hit,
            plane_normal,
            length: to_f32(hit_f64.distance(origin)),
            margin,
            grazing,
            groups,
        })
    }

    /// The apex of a reflected child beam and whether it was refitted.
    ///
    /// The mirror image of the apex is exact when the piece's rays are mirrored rays (isotropic
    /// stones, the ordinary mode). A reflected extraordinary ray is not a mirror image (§10.1), so
    /// its apex is refitted from the exact `S` of the children at the sample points (§8.3
    /// procedure); a fit that misses `τ_dir` makes the piece subdivide.
    fn reflected_apex(
        &self,
        piece: &Beam,
        points: &[PointExp],
        mode: &ModeChildren,
    ) -> Result<(DVec3, bool), Failure> {
        let plane = self.kit.retrace.stone.plane(piece.facet);
        let refit = self.uniaxial
            && mode_of(
                self.kit.retrace.material,
                true,
                piece.state.points.first().is_some_and(|s| s.is_ext),
            )
            .needs_refit();
        if !refit {
            return Ok((mirror_apex(piece.apex, plane), false));
        }
        let split = Failure::Split(DropReason::DepthCap);
        let (groups, geo) = (piece.spectrum.groups(), geometry_group(piece));
        let hits: Vec<DVec3> = points.iter().map(|p| p.hit.as_dvec3()).collect();
        let dirs = (0..points.len())
            .map(|p| mode.children.get(p * groups + geo).map(|c| c.s.as_dvec3()))
            .collect::<Option<Vec<_>>>()
            .ok_or(split)?;
        refit_apex(&hits, &dirs, self.kit.settings.dir_tolerance_rad)
            .map(|apex| (apex, true))
            .ok_or(split)
    }

    /// The child beam of one mode: reflected (`apex` from [`Run::reflected_apex`], inside) or
    /// transmitted (the piece's apex, outside).
    fn derived_beam(
        &self,
        piece: &Beam,
        points: &[PointExp],
        mode: &ModeChildren,
        (transmit, apex): (bool, DVec3),
    ) -> Option<Beam> {
        let (kind, code) = if transmit {
            (EventKind::Transmit, TRANSMIT_CODE)
        } else {
            (EventKind::Reflect, REFLECT_CODE)
        };
        let word = (self.original << 8) | (code << 1) | u32::from(mode.is_ext);
        let prefix = piece.prefix.push(word)?;
        let mut child = Beam {
            facet: piece.facet,
            polygon: piece.polygon,
            apex,
            image_to_plane: piece.image_to_plane,
            inside: !transmit,
            sig: branch_step(piece.sig, self.original, kind, mode.is_ext),
            prefix,
            bounces: piece.bounces + 1,
            spectrum: piece.spectrum,
            state: SampleStates::default(),
        };
        // A reflected child's states are the retraced ones, so an inherited vertex equals the same
        // image point retraced later bit for bit (§7.3); the expansion still decides the
        // child set and the drops. A transmitted child keeps the expansion's states: a point on
        // the critical curve carries the grazing limit (`T = 0`), which has no retraced
        // counterpart (the retracer refuses total reflection on a transmit step).
        let states = if transmit {
            self.inherited_states(piece, points, mode)?
        } else {
            piece
                .state
                .points
                .iter()
                .map(|state| retrace_state(state.image, &child, self.kit))
                .collect::<Option<Vec<_>>>()?
        };
        child.state = SampleStates {
            points: states,
            vertices: piece.state.vertices,
        };
        Some(child)
    }

    /// The sample states of a child of `piece` built from the expansion's children (the states
    /// chained from the parent's `f32` hit).
    fn inherited_states(
        &self,
        piece: &Beam,
        points: &[PointExp],
        mode: &ModeChildren,
    ) -> Option<Vec<SampleState>> {
        let groups = piece.spectrum.groups();
        let geo = geometry_group(piece);
        piece
            .state
            .points
            .iter()
            .zip(points)
            .enumerate()
            .map(|(p, (state, point))| {
                let child = mode.children.get(p * groups + geo)?;
                let stokes = (0..groups)
                    .map(|h| mode.children.get(p * groups + h).map(|c| c.state.stokes))
                    .collect::<Option<Vec<_>>>()?;
                let mut next = SampleState {
                    image: state.image,
                    ray: Ray {
                        origin: point.hit,
                        dir: child.s,
                    },
                    k: child.state.k,
                    segment_origin: point.hit,
                    prev_plane: Some(point.plane_normal),
                    inside: false,
                    is_ext: child.state.is_ext,
                    stokes,
                    depth: state.depth.clone(),
                    path_length: state.path_length + point.length,
                };
                // The segment that ended at this hit, traced in the parent's wave.
                let segment = point.groups.get(geo)?.depths;
                self.kit
                    .add_depth(&mut next, &segment, (state.k, state.is_ext), piece.spectrum)
                    .then_some(next)
            })
            .collect()
    }

    /// Judges `piece`: its child beams and drops, or the reason it must be subdivided.
    fn judge(&self, piece: &Beam, image: &ConvexPolygon, tir: bool) -> Result<Accepted, Failure> {
        let split = Failure::Split(DropReason::DepthCap);
        let points = piece
            .state
            .points
            .iter()
            .map(|state| self.expand_point(piece, state))
            .collect::<Option<Vec<_>>>()
            .ok_or(split)?;
        let outcomes = gather(&points, tir, geometry_group(piece), self.uniaxial)?;
        let (area, groups) = (image.area(), piece.spectrum.groups());
        let cutoff = f64::from(self.kit.settings.cutoff);
        let mut accepted = Accepted {
            reflected: Vec::new(),
            transmitted: Vec::new(),
            drops: Vec::new(),
            refits: 0,
        };
        let dropped = dropped_intensity(&points);
        if dropped > 0.0 {
            accepted
                .drops
                .push((DropReason::MinProbability, area * dropped));
        }
        if self.reflect_alive {
            for mode in &outcomes.reflect {
                let peak = children_peak(piece, &mode.children, groups);
                if peak < cutoff {
                    accepted.drops.push((DropReason::Cutoff, area * peak));
                    continue;
                }
                let tau_w = self.kit.settings.weight_tolerance;
                if !interpolates(piece, image, &mode.children, &outcomes.flipped, tau_w) {
                    return Err(Failure::Split(DropReason::Interpolation));
                }
                let (apex, refit) = self.reflected_apex(piece, &points, mode)?;
                let beam = self
                    .derived_beam(piece, &points, mode, (false, apex))
                    .ok_or(split)?;
                accepted.refits += u64::from(refit);
                accepted.reflected.push((mode.is_ext, beam));
            }
        }
        for mode in &outcomes.transmit {
            let peak = children_peak(piece, &mode.children, groups);
            if peak < cutoff {
                accepted.drops.push((DropReason::Cutoff, area * peak));
                continue;
            }
            let beam = self
                .derived_beam(piece, &points, mode, (true, piece.apex))
                .ok_or(split)?;
            accepted.transmitted.push((mode.is_ext, beam));
        }
        Ok(accepted)
    }

    /// Records an accepted piece: its drops, reflected children and exit leaves.
    fn accept(&mut self, accepted: Accepted) {
        for (reason, energy) in accepted.drops {
            self.stats.record_drop(reason, energy);
        }
        if self.uniaxial {
            // Every reflected and transmitted mode is a child (§10.1).
            let children = accepted.reflected.len() + accepted.transmitted.len();
            self.stats.mode_children += u64::try_from(children).unwrap_or(u64::MAX);
            self.stats.apex_refits += accepted.refits;
        }
        for (is_ext, beam) in accepted.reflected {
            self.reflected[usize::from(is_ext)].push(beam);
        }
        for (is_ext, beam) in accepted.transmitted {
            let leaves = exit_leaves(&beam, self.kit, self.stats);
            self.transmitted[usize::from(is_ext)].extend(leaves);
        }
    }

    /// Bisects `piece` and handles both halves, or drops it at the depth cap or below
    /// [`SPLIT_FLOOR_PX2`].
    fn subdivide(
        &mut self,
        piece: &Beam,
        image: &ConvexPolygon,
        tir: bool,
        depth: u32,
        reason: DropReason,
    ) {
        if beam_near_axis(piece, self.kit.retrace.material, self.kit.retrace.n_lambda) {
            // Not subdivided further near an optic axis: the residual covers it (§10.2, §10.3).
            self.stats
                .record_drop(DropReason::OpticAxis, image.area() * incoming_peak(piece));
            return;
        }
        let halves = (depth < self.kit.settings.max_split_depth && image.area() > SPLIT_FLOOR_PX2)
            .then(|| bisect(piece, self.kit))
            .flatten();
        if let Some((first, second)) = halves {
            self.piece(&first, tir, depth + 1);
            self.piece(&second, tir, depth + 1);
        } else {
            self.stats
                .record_drop(reason, image.area() * incoming_peak(piece));
        }
    }

    /// Handles one piece on one side of the critical curve.
    ///
    /// Stops at once when the kit is cancelled, and drops the piece as `DepthCap` once the tile's
    /// attempted work reaches the kit's allowance.
    fn piece(&mut self, piece: &Beam, tir: bool, depth: u32) {
        if self.kit.cancelled() {
            return;
        }
        let Some(image) = image_polygon(piece) else {
            self.stats.record_drop(DropReason::Sliver, 0.0);
            return;
        };
        self.stats.record_attempt();
        if self.stats.allowance_spent_at(self.kit.allowance) {
            self.stats
                .record_drop(DropReason::DepthCap, image.area() * incoming_peak(piece));
            return;
        }
        match self.judge(piece, &image, tir) {
            Ok(accepted) => self.accept(accepted),
            Err(Failure::Split(reason)) => self.subdivide(piece, &image, tir, depth, reason),
            Err(Failure::Modes) => {
                self.stats.mode_splits += 1;
                self.subdivide(piece, &image, tir, depth, DropReason::DepthCap);
            }
            Err(Failure::Drop(reason)) => {
                self.stats
                    .record_drop(reason, image.area() * incoming_peak(piece));
            }
        }
    }
}

/// Expands one beam lying on its next facet (after the partition and projection).
///
/// It splits the beam at the snapped TIR curve, then emits the reflected children and the
/// transmitted leaves (§8.5). Pruned and capped energy goes to `stats` (§12.1, §12.2).
///
/// - **Pieces.** [`split_tir`] gives the partial and TIR pieces; each is expanded at every sample
///   point with `expand_at` at the exact hit. A piece is subdivided (up to `max_split_depth`, then
///   dropped as `Interpolation` or `DepthCap`) when its points disagree on the child set or a
///   reflected child fails the weight interpolation check (§8.4, `τ_w`, `τ_abs = 1e-6`).
/// - **Children.** A reflected child per mode has the mirror apex, `sig = branch_step(.., Reflect,
///   is_ext)` (partial reflection and TIR share one code), the prefix plus the step word,
///   `bounces + 1` and the exact states of the expansion. A partial piece also gives a transmitted
///   beam per mode (outside, the transmitted children as its states) that [`exit_leaves`] turns
///   into leaves. Children come in canonical order: ordinary mode first, then extraordinary, each
///   in piece order (partial pieces before TIR pieces).
/// - **Pruning.** A child whose largest per-node weight at any sample point is below
///   `settings.cutoff` is a `Cutoff` drop. Outcomes below `MIN_PROBABILITY` are `MinProbability`
///   drops of the pieces that are kept. A piece with a `diverged` child is dropped as `Diverged`.
/// - **`max_bounces`.** The hit has `bounces + 1` events; if that exceeds `max_bounces − 1` the
///   branch contributes zero (§5.4) and nothing is produced or counted. A reflected child can only
///   exit with one more event, so it is produced only when `bounces + 2 <= max_bounces − 1`.
#[must_use]
pub fn internal_hit(
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    seams: &mut SeamStore,
    stats: &mut BeamStats,
) -> InternalHit {
    let events = beam.bounces.saturating_add(1);
    let cap = kit.max_bounces.saturating_sub(1);
    if events > cap {
        return InternalHit::default();
    }
    let (partial, tir) = split_tir(beam, kit, seams, stats);
    let stone = kit.retrace.stone;
    let lambda_nm = centre_lambda(beam, kit);
    let mut run = Run {
        kit,
        heroes: kit.group_heroes(beam.spectrum),
        stats,
        original: stone.original_facet(beam.facet),
        normal: stone.plane(beam.facet).0.normalize(),
        eta: f64::from(kit.retrace.material.dispersion.evaluate(lambda_nm)),
        uniaxial: is_anisotropic(kit.retrace.material),
        lambda_nm,
        reflect_alive: events < cap,
        reflected: [Vec::new(), Vec::new()],
        transmitted: [Vec::new(), Vec::new()],
    };
    for piece in &partial {
        run.piece(piece, false, 0);
    }
    for piece in &tir {
        run.piece(piece, true, 0);
    }
    let [mut reflected, extra_reflected] = run.reflected;
    reflected.extend(extra_reflected);
    let [mut transmitted, extra_transmitted] = run.transmitted;
    transmitted.extend(extra_transmitted);
    InternalHit {
        reflected,
        transmitted,
    }
}

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests {
    use super::*;

    /// A reflected child of `is_ext` with unit intensity.
    fn reflected_child(is_ext: bool) -> Child {
        Child {
            state: PathState {
                k: Vec3::X,
                inside: true,
                is_ext,
                stokes: [StokesVector::unpolarized(1.0); GROUP],
            },
            s: Vec3::X,
            kind: EventKind::Reflect,
            diverged: false,
        }
    }

    /// A sample point whose single group returns reflected children of `modes`, `margin` away from
    /// the critical curve.
    fn point(modes: &[bool], margin: f64) -> PointExp {
        PointExp {
            hit: Vec3::ZERO,
            plane_normal: Vec3::Z,
            length: 1.0,
            margin,
            grazing: Vec3::X,
            groups: vec![Expansion {
                children: modes.iter().map(|&m| reflected_child(m)).collect(),
                dropped: 0.0,
                depths: SegmentDepths::NONE,
                plane_normal: Vec3::Z,
            }],
        }
    }

    /// The forward projective map of an image-edge fraction `t` to the plane fraction.
    fn plane_fraction(t: f64, wa: f64, wb: f64) -> f64 {
        t * wb / (1.0 - t).mul_add(wa, t * wb)
    }

    /// With unequal weights the plane midpoint belongs at image fraction 1/3, and the inverse
    /// round-trips the forward map for every fraction.
    #[test]
    fn the_image_fraction_inverts_the_projective_map_with_unequal_weights() {
        let (wa, wb) = (1.0, 2.0);
        let third = image_fraction(0.5, wa, wb).expect("regular weights");
        assert!((third - 1.0 / 3.0).abs() < 1e-14, "midpoint at {third}");
        assert!((plane_fraction(third, wa, wb) - 0.5).abs() < 1e-14);
        for k in 0..=10 {
            let along = f64::from(k) / 10.0;
            let t = image_fraction(along, wa, wb).expect("regular weights");
            assert!(
                (plane_fraction(t, wa, wb) - along).abs() < 1e-13,
                "s {along}"
            );
        }
        // Equal weights are the identity.
        assert_eq!(image_fraction(0.25, 3.0, 3.0), Some(0.25));
    }

    /// A vanishing denominator (the point at infinity) gives no crossing.
    #[test]
    fn the_image_fraction_refuses_a_singular_denominator() {
        assert_eq!(image_fraction(0.5, 1.0, -1.0), None);
        assert_eq!(image_fraction(0.5, 0.0, 0.0), None);
    }

    /// Points that disagree on the reflected mode set (a relabelling flip) never make a kept
    /// piece, not even on the critical curve: the piece subdivides as `Failure::Modes`, and at the
    /// depth cap `Run::subdivide` drops it (`DepthCap` or `OpticAxis`) for the residual
    /// (§10.2, §12.3).
    #[test]
    fn a_piece_straddling_a_relabelling_flip_is_never_kept() {
        let straddling = [point(&[false, true], 0.5), point(&[false], 0.0)];
        for tir in [false, true] {
            assert!(
                matches!(gather(&straddling, tir, 0, true), Err(Failure::Modes)),
                "birefringent, tir {tir}"
            );
            assert!(
                matches!(
                    gather(&straddling, tir, 0, false),
                    Err(Failure::Split(DropReason::DepthCap))
                ),
                "isotropic, tir {tir}"
            );
        }
        let agreeing = [point(&[false, true], 0.5), point(&[false, true], 0.0)];
        assert!(gather(&agreeing, true, 0, true).is_ok());
        let single = [point(&[false], 0.5), point(&[false], 0.0)];
        assert!(gather(&single, true, 0, true).is_ok());
    }
}
