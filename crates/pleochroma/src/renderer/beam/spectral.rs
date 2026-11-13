//! Spectral intervals: initial intervals, end-node traces, validity checks, bin-edge regions (beam
//! guide §9.2).
//!
//! A beam carries an interval of spectral nodes whose topology is computed at the interval's
//! centre node. This module decides which intervals a material starts with
//! ([`initial_intervals`]), traces the two end nodes as their own heroes ([`trace_end_nodes`]),
//! judges whether the interval is still valid ([`check_interval`], [`check_diverged`]) and forms
//! the kept region of a leaf inside the true region for **every** wavelength of its bins
//! ([`shrink_leaf`], the coverage contract of §12.3).
//!
//! # How the regions are formed
//!
//! The leaf polygon is snapped at the interval's centre node. For each of its edges that is a real
//! branch boundary (the exact ray just outside the edge leaves the branch) the true boundary is
//! located along the edge's inward normal at the outer bin edges (`EndIntersection`) or at every
//! node bin edge (`PerNode`), by bisection on [`BranchFollower::follows`]. The polygon is then
//! clipped by the line parallel to the edge through the innermost crossing. The crossings at the
//! two edges and at the centre must be monotone, otherwise the interval splits. The band cut away
//! is left to the residual and counted in `residual_band_area`.

use glam::{DVec2, DVec3};

use super::{
    BeamSettings, RegionMode,
    beam::{
        Beam, EndNodeTrace, GROUP, IntervalSplit, Leaf, SampleStates, SpectralInterval,
        SpectralNode,
    },
    geom::{Tolerances, polygon::ConvexPolygon},
    interact::InteractKit,
    leaf::WEIGHT_ABS_TOLERANCE,
    propagate::retrace_state,
    retrace::{RetraceKit, exact_ray_at},
    snap::exit_facet,
    stats::{BeamStats, DropReason},
};
use crate::{
    optics::{materials::GemMaterial, polarization::StokesVector},
    renderer::path_cache::build::{DISPERSION_GATE_BG, dispersion_b_minus_g, is_dispersive},
};

/// The first bracket half-width of a boundary search, in chord tolerances.
const BRACKET_CHORDS: f64 = 16.0;
/// The probe distance outside an edge that decides whether it is a branch boundary, in chord
/// tolerances.
const PROBE_CHORDS: f64 = 8.0;
/// Slack of the monotonicity check of the crossings, in chord tolerances.
const SLACK_CHORDS: f64 = 3.0;
/// A bin-edge boundary shift at or below this many chord tolerances counts as no shift. One chord
/// tolerance is the coverage contract's own bound (§12.3): a region kept past the true boundary by
/// more would over-count, so larger shifts always shrink the region.
const SHIFT_SLACK_CHORDS: f64 = 1.0;
/// How far outside an edge a moved boundary is searched for, in pixels.
const OUT_REACH_PX: f64 = 64.0;
/// The bisection step cap of `crossing`; any finite `f64` bracket halves below any chord first.
const MAX_BISECT_STEPS: u32 = 1100;
/// The relative safety margin added to a measured shift.
const SHIFT_MARGIN: f64 = 0.05;
/// Edges longer than this (pixels) are sampled five times, shorter ones three times.
const LONG_EDGE_PX: f64 = 8.0;
/// Sample positions along a short edge.
const SHORT_SAMPLES: [f64; 3] = [0.08, 0.5, 0.92];
/// Sample positions along a long edge.
const LONG_SAMPLES: [f64; 5] = [0.08, 0.29, 0.5, 0.71, 0.92];
/// The cap on the samples of one edge after adaptive refinement.
const MAX_EDGE_SAMPLES: usize = 33;

// ---- initial intervals ----

/// The node centres of an `n_lambda` set, in nm.
fn node_centres(n_lambda: u32) -> impl Iterator<Item = f32> {
    (0..n_lambda)
        .map(move |k| SpectralNode(u16::try_from(k).unwrap_or(u16::MAX)).centre_nm(n_lambda))
}

/// Every principal index of `material` at `lambda_nm`, as bits: the base (ordinary, or biaxial
/// `n_beta`) index, the uniaxial extraordinary index and, for a biaxial material with its own
/// principal curves, `n_alpha` and `n_gamma` (`0` where the material has none). Offset-only
/// biaxial indices follow the base index, so they need no entry of their own.
fn principal_index_bits(material: &GemMaterial, lambda_nm: f32) -> [u32; 4] {
    let n_o = material.dispersion.evaluate(lambda_nm);
    let n_e = material.extraordinary_index_at(lambda_nm, n_o);
    let [n_alpha, n_gamma] = match (
        material.biaxial_delta_beta_alpha,
        &material.biaxial_principal_dispersion,
    ) {
        (Some(_), Some([alpha, gamma])) => [
            alpha.evaluate(lambda_nm).to_bits(),
            gamma.evaluate(lambda_nm).to_bits(),
        ],
        _ => [0, 0],
    };
    [n_o.to_bits(), n_e.to_bits(), n_alpha, n_gamma]
}

/// `true` when every principal index of `material` is bitwise the same at all of `lambdas`: the
/// geometry of any mode then does not depend on the wavelength across them.
///
/// The one constancy rule of the beam tracer: the interval classifier ([`strictly_constant`]) and
/// the exit-leaf direction shortcut both use it. A material whose dispersion lives only in its
/// extraordinary index, or only in a biaxial `n_alpha`/`n_gamma` curve, is never constant.
#[must_use]
pub fn indices_constant_at(material: &GemMaterial, lambdas: impl IntoIterator<Item = f32>) -> bool {
    let mut lambdas = lambdas.into_iter();
    let Some(first) = lambdas.next() else {
        return true;
    };
    let reference = principal_index_bits(material, first);
    lambdas.all(|lambda| principal_index_bits(material, lambda) == reference)
}

/// `true` when every principal index of `material` is bitwise equal at all `n_lambda` node
/// centres: the geometry then does not depend on the wavelength at all (§5.5, §9.2).
#[must_use]
pub fn strictly_constant(material: &GemMaterial, n_lambda: u32) -> bool {
    indices_constant_at(material, node_centres(n_lambda))
}

/// The intervals a material starts with (§9.2 table): one interval of all nodes when the material
/// is non-dispersive in the strict sense, single nodes above the dispersion gate, groups of 8
/// otherwise.
#[must_use]
pub fn initial_intervals(material: &GemMaterial, n_lambda: u32) -> Vec<SpectralInterval> {
    if strictly_constant(material, n_lambda) {
        return vec![SpectralInterval::all(n_lambda)];
    }
    let last = u16::try_from(n_lambda).unwrap_or(u16::MAX);
    if is_dispersive(material) && dispersion_b_minus_g(material) >= DISPERSION_GATE_BG {
        return (0..last).map(SpectralInterval::single).collect();
    }
    (0..last)
        .step_by(GROUP)
        .map(|first| SpectralInterval {
            first,
            last: (first + 7).min(last.saturating_sub(1)),
        })
        .collect()
}

/// `beam` (an outside primary beam) carrying `spectrum` instead: the sample states get unit
/// unpolarized Stokes vectors for the groups of the new interval.
#[must_use]
pub fn retarget(beam: &Beam, spectrum: SpectralInterval) -> Beam {
    let mut out = beam.clone();
    out.spectrum = spectrum;
    for state in &mut out.state.points {
        state.stokes = vec![[StokesVector::unpolarized(1.0); GROUP]; spectrum.groups()];
    }
    out
}

// ---- end-node traces and validity ----

/// The interval's end nodes traced as their own heroes at every sample point of `beam` (§9.2): the
/// exact state of a ray along the beam's branch at the node's own wavelength.
#[must_use]
pub fn trace_end_nodes(beam: &Beam, kit: &InteractKit<'_, '_>) -> [EndNodeTrace; 2] {
    [beam.spectrum.first, beam.spectrum.last].map(|node| {
        let probe = Beam {
            facet: beam.facet,
            polygon: beam.polygon,
            apex: beam.apex,
            image_to_plane: beam.image_to_plane,
            inside: beam.inside,
            sig: beam.sig,
            prefix: beam.prefix,
            bounces: beam.bounces,
            spectrum: SpectralInterval::single(node),
            state: SampleStates::default(),
        };
        let states = beam
            .state
            .points
            .iter()
            .map(|point| {
                // A cancelled frame is dropped by its caller: stop retracing.
                if kit.cancelled() {
                    None
                } else {
                    retrace_state(point.image, &probe, kit)
                }
            })
            .collect();
        EndNodeTrace {
            node,
            lambda_nm: SpectralNode(node).centre_nm(kit.retrace.n_lambda),
            states,
        }
    })
}

/// `true` when the two Stokes vectors agree within `tolerance` in every component.
fn stokes_agree(a: &StokesVector, b: &StokesVector, tolerance: f32) -> bool {
    (a.i - b.i).abs() <= tolerance
        && (a.q - b.q).abs() <= tolerance
        && (a.u - b.u).abs() <= tolerance
        && (a.v - b.v).abs() <= tolerance
}

/// The validity checks of §9.2 on a beam of a multi-node interval, given its end-node traces.
///
/// 1. **Topology:** an end node has an exact state at every sample point (the retracer, the child
///    set and the `diverged` rule of the replay all accept the branch), in the same medium and
///    mode as the shared state.
/// 2. **State check:** the end node's own-hero Stokes vector equals the shared one within
///    `τ_w · max(I) + τ_abs`, the tolerance of the weight interpolation check (§8.4).
///
/// A single-node interval is always valid here. The third check (no `diverged` child) is
/// [`check_diverged`].
///
/// # Errors
///
/// The [`IntervalSplit`] cause of the first failing check.
pub fn check_interval(
    beam: &Beam,
    ends: &[EndNodeTrace; 2],
    settings: &BeamSettings,
) -> Result<(), IntervalSplit> {
    if beam.spectrum.len() < 2 {
        return Ok(());
    }
    for end in ends {
        if end.states.len() != beam.state.points.len() || !beam.spectrum.contains(end.node) {
            return Err(IntervalSplit::Topology);
        }
        let index = usize::from(end.node - beam.spectrum.first);
        let (group, slot) = (index / GROUP, index % GROUP);
        let shared = |i: usize| beam.state.points[i].stokes.get(group).map(|g| g[slot]);
        let peak = (0..beam.state.points.len())
            .filter_map(|i| shared(i).map(|s| s.i.abs()))
            .fold(0.0_f32, f32::max);
        let tolerance = settings
            .weight_tolerance
            .mul_add(peak, WEIGHT_ABS_TOLERANCE);
        for (i, own) in end.states.iter().enumerate() {
            let point = &beam.state.points[i];
            let Some(own) = own else {
                return Err(IntervalSplit::Topology);
            };
            if own.inside != point.inside || own.is_ext != point.is_ext {
                return Err(IntervalSplit::Topology);
            }
            let (Some(mine), Some(theirs)) = (own.stokes.first().map(|g| g[0]), shared(i)) else {
                return Err(IntervalSplit::Topology);
            };
            if !stokes_agree(&mine, &theirs, tolerance) {
                return Err(IntervalSplit::StateCheck);
            }
        }
    }
    Ok(())
}

/// The `diverged` rule (§8.5) from the counters: `Err(Diverged)` when a piece was dropped as
/// `Diverged` between the snapshots `before` and `after`.
///
/// # Errors
///
/// [`IntervalSplit::Diverged`].
pub const fn check_diverged(before: &BeamStats, after: &BeamStats) -> Result<(), IntervalSplit> {
    let slot = DropReason::Diverged.slot();
    if after.drops[slot] > before.drops[slot] {
        Err(IntervalSplit::Diverged)
    } else {
        Ok(())
    }
}

// ---- the exact region of a branch ----

/// Tells whether the exact ray through an image point follows a branch at a wavelength.
pub struct BranchFollower<'k> {
    /// The retracer.
    kit: RetraceKit<'k>,
    /// The branch.
    prefix: super::beam::BranchPrefix,
}

impl<'k> BranchFollower<'k> {
    /// A follower of the branch `prefix`.
    #[must_use]
    pub const fn new(kit: RetraceKit<'k>, prefix: super::beam::BranchPrefix) -> Self {
        Self { kit, prefix }
    }

    /// `true` when the exact ray through `image_point` at `lambda_nm` follows the whole prefix:
    /// the retracer finds it (no total reflection on a transmit step) and every inside segment
    /// leaves the stone through the facet the next step names.
    #[must_use]
    pub fn follows(&self, image_point: DVec2, lambda_nm: f32) -> bool {
        let Ok(ray) = exact_ray_at(image_point, lambda_nm, &self.prefix, &self.kit) else {
            return false;
        };
        let steps = self.prefix.steps();
        for (i, &step) in steps.iter().enumerate().skip(1) {
            let (from, to): (DVec3, DVec3) = (ray.hits[i - 1], ray.hits[i]);
            let Some(dir) = (to - from).try_normalize() else {
                return false;
            };
            let expected = self.kit.stone.facet_of_original(step >> 8);
            if expected.is_none() || exit_facet(self.kit.stone, from, dir) != expected {
                return false;
            }
        }
        true
    }
}

/// The wavelengths whose regions bound the kept region: the two outer bin edges
/// (`EndIntersection`) or every node bin edge (`PerNode`).
fn edge_wavelengths(spectrum: SpectralInterval, mode: RegionMode, n_lambda: u32) -> Vec<f32> {
    match mode {
        RegionMode::EndIntersection => {
            let (lo, hi) = spectrum.bin_edges_nm(n_lambda);
            vec![lo, hi]
        }
        RegionMode::PerNode => {
            let mut out: Vec<f32> = (spectrum.first..=spectrum.last)
                .map(|n| SpectralNode(n).bin_nm(n_lambda).0)
                .collect();
            out.push(SpectralNode(spectrum.last).bin_nm(n_lambda).1);
            out
        }
    }
}

/// The signed distance along `n` from `p` to the boundary of the branch at `lambda`: positive when
/// the boundary lies inside the polygon (the region shrinks), negative when it lies outside.
///
/// `reach` bounds the search inward (the polygon's extent); a boundary beyond it returns `reach`.
fn crossing(
    follower: &BranchFollower<'_>,
    lambda: f32,
    (p, n): (DVec2, DVec2),
    reach: f64,
    chord: f64,
) -> f64 {
    let at = |s: f64| follower.follows(n.mul_add(DVec2::splat(s), p), lambda);
    let h0 = BRACKET_CHORDS * chord;
    let reach = reach.max(h0);
    let (mut false_end, mut true_end) = match (at(-h0), at(h0)) {
        (false, true) => (-h0, h0),
        (true, true) => {
            let (mut known_true, mut s) = (-h0, -2.0 * h0);
            loop {
                if s < -OUT_REACH_PX {
                    return -OUT_REACH_PX;
                }
                if at(s) {
                    known_true = s;
                    s *= 2.0;
                } else {
                    break (s, known_true);
                }
            }
        }
        (false, false) => {
            let (mut known_false, mut s) = (h0, 2.0 * h0);
            loop {
                if s > reach {
                    if at(reach) {
                        break (known_false, reach);
                    }
                    return reach;
                }
                if at(s) {
                    break (known_false, s);
                }
                known_false = s;
                s *= 2.0;
            }
        }
        (true, false) => return reach,
    };
    // Bisection: every step halves the bracket, so a few dozen steps reach any sane chord; the cap
    // only bounds a zero chord. The test is `!(gap > chord)`, so a NaN chord stops at once.
    for _ in 0..MAX_BISECT_STEPS {
        if (true_end - false_end).partial_cmp(&chord) != Some(std::cmp::Ordering::Greater) {
            break;
        }
        let mid = false_end.midpoint(true_end);
        if at(mid) {
            true_end = mid;
        } else {
            false_end = mid;
        }
    }
    true_end
}

/// `true` when the sequence is monotone within `tol` and the centre value 0 lies between its ends.
fn is_monotone(seq: &[f64], tol: f64) -> bool {
    let (Some(&first), Some(&last)) = (seq.first(), seq.last()) else {
        return true;
    };
    let rising = seq.windows(2).all(|w| w[1] >= w[0] - tol);
    let falling = seq.windows(2).all(|w| w[1] <= w[0] + tol);
    (rising || falling) && first.min(last) <= tol && first.max(last) >= -tol
}

/// Everything one edge scan reads.
struct Scan<'a, 'k> {
    follower: &'a BranchFollower<'k>,
    wavelengths: &'a [f32],
    centre_nm: f32,
    chord: f64,
    reach: f64,
    /// The cancel poll; a cancelled scan returns early and its result is discarded.
    cancelled: &'a dyn Fn() -> bool,
}

impl Scan<'_, '_> {
    /// The per-wavelength shifts of the edge `a -> b` (counter-clockwise polygon, inward normal on
    /// the left), or `None` when the edge is not a branch boundary.
    fn edge(&self, start: DVec2, end: DVec2) -> Result<Option<Vec<f64>>, IntervalSplit> {
        let delta = end - start;
        let len = delta.length();
        if !(len.is_finite() && len > 0.0) {
            return Ok(None);
        }
        let normal = DVec2::new(-delta.y, delta.x) / len;
        let middle = delta.mul_add(DVec2::splat(0.5), start);
        let probe = normal.mul_add(DVec2::splat(-PROBE_CHORDS * self.chord), middle);
        // An edge is skipped only when its outside follows the branch at the centre wavelength and
        // at every bin-edge wavelength: a boundary that moves across a neighbouring piece at a bin
        // edge must still clip the piece behind it.
        if self.follower.follows(probe, self.centre_nm)
            && self
                .wavelengths
                .iter()
                .all(|&lambda| self.follower.follows(probe, lambda))
        {
            return Ok(None);
        }
        let positions: &[f64] = if len > LONG_EDGE_PX {
            &LONG_SAMPLES
        } else {
            &SHORT_SAMPLES
        };
        let sample = |t: f64| -> Result<(f64, Vec<f64>), IntervalSplit> {
            let point = delta.mul_add(DVec2::splat(t), start);
            let seq: Vec<f64> = self
                .wavelengths
                .iter()
                .map(|&lambda| {
                    crossing(
                        self.follower,
                        lambda,
                        (point, normal),
                        self.reach,
                        self.chord,
                    )
                })
                .collect();
            if is_monotone(&seq, SLACK_CHORDS * self.chord) {
                Ok((t, seq))
            } else {
                Err(IntervalSplit::NotMonotone)
            }
        };
        let mut samples = Vec::with_capacity(MAX_EDGE_SAMPLES);
        for &t in positions {
            if (self.cancelled)() {
                return Ok(None);
            }
            samples.push(sample(t)?);
        }
        // Adaptive refinement: insert the midpoint of the neighbouring pair whose shifts differ
        // most while that exceeds the chord, until the cap or the chord spacing.
        while samples.len() < MAX_EDGE_SAMPLES && !(self.cancelled)() {
            let widest = samples
                .windows(2)
                .enumerate()
                .filter(|(_, w)| (w[1].0 - w[0].0) * len > self.chord)
                .map(|(k, w)| {
                    let spread = w[0]
                        .1
                        .iter()
                        .zip(&w[1].1)
                        .fold(0.0_f64, |acc, (a, b)| acc.max((a - b).abs()));
                    (k, spread)
                })
                .filter(|&(_, spread)| spread > self.chord)
                .max_by(|a, b| a.1.total_cmp(&b.1));
            let Some((k, _)) = widest else { break };
            let mid = samples[k].0.midpoint(samples[k + 1].0);
            samples.insert(k + 1, sample(mid)?);
        }
        let mut shifts = vec![f64::NEG_INFINITY; self.wavelengths.len()];
        for (_, seq) in &samples {
            for (shift, s) in shifts.iter_mut().zip(seq) {
                *shift = shift.max(*s);
            }
        }
        Ok(Some(shifts))
    }
}

/// The node groups of a leaf and the pair of wavelength indices bounding each.
fn node_groups(spectrum: SpectralInterval, mode: RegionMode) -> Vec<(SpectralInterval, usize)> {
    match mode {
        RegionMode::EndIntersection => vec![(spectrum, 0)],
        RegionMode::PerNode => (spectrum.first..=spectrum.last)
            .map(|n| (SpectralInterval::single(n), usize::from(n - spectrum.first)))
            .collect(),
    }
}

/// `polygon` clipped to the half-plane `n . x >= n . a + offset`; pieces below the sliver area are
/// dropped.
fn clip_inward(polygons: &[ConvexPolygon], a: DVec2, n: DVec2, offset: f64) -> Vec<ConvexPolygon> {
    let normal = DVec3::new(-n.x, -n.y, 0.0);
    let plane = -(n.dot(a) + offset);
    polygons
        .iter()
        .flat_map(|p| p.clip_capped(normal, plane, 0.0).0)
        .filter(|p| p.area() >= Tolerances::DEFAULT.sliver_area_px2)
        .collect()
}

/// The leaf of `source` restricted to the nodes `nodes` over `polygon`: the vertex data are the
/// Wachspress interpolation of the source's (exact at the source's own vertices).
#[expect(
    clippy::cast_possible_truncation,
    reason = "weights are far inside the f32 range; the tracer works in f32"
)]
fn restricted_leaf(source: &Leaf, nodes: SpectralInterval, polygon: &ConvexPolygon) -> Leaf {
    let all = source.nodes();
    let offset = usize::from(nodes.first - source.spectrum.first);
    let count = nodes.len();
    let (points, n) = polygon.points_2d();
    let (old_points, old_n) = source.image.points_2d();
    let mut dirs = Vec::with_capacity(n * count);
    let mut weights = Vec::with_capacity(n * count);
    for &q in &points[..n] {
        let same = old_points[..old_n].iter().position(|&o| o == q);
        let w = source.image.wachspress(q);
        for k in 0..count {
            let node = offset + k;
            if let Some(v) = same {
                dirs.push(source.dirs[v * all + node]);
                weights.push(source.weights[v * all + node]);
                continue;
            }
            let mut dir = DVec3::ZERO;
            let mut weight = 0.0_f64;
            for (v, &wv) in w.iter().enumerate().take(old_n) {
                dir = source.dirs[v * all + node]
                    .as_dvec3()
                    .mul_add(DVec3::splat(wv), dir);
                weight = f64::from(source.weights[v * all + node]).mul_add(wv, weight);
            }
            dirs.push(if source.reflection {
                dir.as_vec3()
            } else {
                dir.normalize_or_zero().as_vec3()
            });
            weights.push(weight.max(0.0) as f32);
        }
    }
    Leaf {
        tile: source.tile,
        image: *polygon,
        dirs,
        weights,
        spectrum: nodes,
        sig: source.sig,
        prefix: source.prefix,
        glare: source.glare,
        reflection: source.reflection,
        tau_omega: source.tau_omega,
        pre_clip: Some(source.pre_clip.unwrap_or(source.image)),
    }
}

/// `true` when the centroid of `leaf` follows its branch at the centre wavelength of its interval.
///
/// The topology test of [`shrink_leaf`], shared with `leaf::exit_into`, which bisects a
/// single-node exit piece that fails it. A reflection leaf has no branch and always passes.
#[must_use]
pub fn centroid_follows(leaf: &Leaf, retrace: RetraceKit<'_>) -> bool {
    leaf.reflection
        || BranchFollower::new(retrace, leaf.prefix).follows(
            leaf.image.centroid().truncate(),
            SpectralNode(leaf.spectrum.centre()).centre_nm(retrace.n_lambda),
        )
}

/// The kept leaves of `leaf` (§9.2, §12.3): the leaf over polygons that lie inside the true region
/// of every wavelength of the interval's bins.
///
/// `EndIntersection` gives one leaf with the interval's two outer bin-edge regions intersected;
/// `PerNode` one leaf per node with that node's own two bin-edge regions. A leaf whose edges do
/// not move beyond the chord tolerance comes back unchanged. The image area cut away is added to
/// `stats.residual_band_area` (the residual covers it).
///
/// # Errors
///
/// [`IntervalSplit::NotMonotone`] when a boundary does not move monotonically with the wavelength
/// (the crossings at the centre and at the bin edges are not ordered), and
/// [`IntervalSplit::Topology`] when a transmitted leaf's centroid is off its branch at the
/// centre wavelength (a reflection leaf is returned as it is). A single-node leaf is clipped by
/// `leaf::exit_into`, which bisects a piece that fails either check instead of dropping it.
pub fn shrink_leaf(
    leaf: &Leaf,
    kit: &InteractKit<'_, '_>,
    mode: RegionMode,
    stats: &mut BeamStats,
) -> Result<Vec<Leaf>, IntervalSplit> {
    let n_lambda = kit.retrace.n_lambda;
    let spectrum = leaf.spectrum;
    let follower = BranchFollower::new(kit.retrace, leaf.prefix);
    let centre_nm = SpectralNode(spectrum.centre()).centre_nm(n_lambda);
    if leaf.reflection {
        return Ok(vec![leaf.clone()]);
    }
    if !centroid_follows(leaf, kit.retrace) {
        // A transmitted leaf whose centroid leaves its branch at the centre wavelength may have a
        // different nearest facet or critical-angle decision: never a silent success. A multi-node
        // interval splits; a single node reaches here only through `exit_into`, which has already
        // bisected an off-branch piece.
        return Err(IntervalSplit::Topology);
    }
    let wavelengths = edge_wavelengths(spectrum, mode, n_lambda);
    let (points, n) = leaf.image.points_2d();
    let ring = &points[..n];
    let reach = ring
        .iter()
        .flat_map(|p| ring.iter().map(move |q| p.distance(*q)))
        .fold(0.0_f64, f64::max);
    let chord = kit.settings.chord_tolerance_px;
    let cancelled = || kit.cancelled();
    let scan = Scan {
        follower: &follower,
        wavelengths: &wavelengths,
        centre_nm,
        chord,
        reach,
        cancelled: &cancelled,
    };
    let mut shifts = Vec::with_capacity(n);
    for i in 0..n {
        if kit.cancelled() {
            // The caller drops everything of a cancelled frame; skip the remaining scans.
            return Ok(vec![leaf.clone()]);
        }
        shifts.push(scan.edge(ring[i], ring[(i + 1) % n])?);
    }
    if kit.cancelled() {
        return Ok(vec![leaf.clone()]);
    }
    let groups = node_groups(spectrum, mode);
    // Decide first whether any boundary moved, so a stationary leaf never materializes
    // replacements it would discard.
    let moved = groups.iter().any(|(_, w0)| {
        shifts
            .iter()
            .flatten()
            .any(|shift| shift[*w0].max(shift[w0 + 1]) > SHIFT_SLACK_CHORDS * chord)
    });
    if !moved {
        return Ok(vec![leaf.clone()]);
    }
    let mut out = Vec::new();
    let mut kept_area = 0.0;
    for (nodes, w0) in &groups {
        let mut polygons = vec![leaf.image];
        for (i, shift) in shifts.iter().enumerate() {
            let Some(shift) = shift else { continue };
            let worst = shift[*w0].max(shift[w0 + 1]);
            if worst <= SHIFT_SLACK_CHORDS * chord {
                continue;
            }
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let d = b - a;
            let normal = DVec2::new(-d.y, d.x) / d.length();
            let offset = worst.mul_add(1.0 + SHIFT_MARGIN, chord);
            polygons = clip_inward(&polygons, a, normal, offset);
        }
        for polygon in polygons {
            kept_area = polygon.area().mul_add(
                f64::from(u32::try_from(nodes.len()).unwrap_or(1)),
                kept_area,
            );
            out.push(restricted_leaf(leaf, *nodes, &polygon));
        }
    }
    let total = leaf.image.area() * f64::from(u32::try_from(spectrum.len()).unwrap_or(1));
    let band = (total - kept_area).max(0.0) / f64::from(u32::try_from(spectrum.len()).unwrap_or(1));
    stats.residual_band_area += band;
    Ok(out)
}

/// The largest weight of a leaf, for drop accounting.
#[must_use]
pub fn leaf_peak(leaf: &Leaf) -> f64 {
    leaf.weights
        .iter()
        .copied()
        .map(f64::from)
        .fold(0.0, f64::max)
}
