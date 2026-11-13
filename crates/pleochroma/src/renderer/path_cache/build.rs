//! The deterministic branch tree of one camera ray, and its conversion to exit records.
//!
//! A tree is traced for one hero wavelength of the comb: the geometry follows the hero, and the
//! Stokes vector of all eight comb channels rides along, exactly as in the path tracer's own
//! bounce loop. Each node also sums the optical-depth coefficients of its interior segments; a
//! leaf's record stores them and the comb weights with the absorption divided out (`record` says
//! why). The cache needs three kinds of tree per position:
//!
//! - the **reference** tree at 560 nm, whose leaves become merged records (one direction, eight
//!   weights);
//! - the **neighbour** trees at 510 and 610 nm, whose matching leaves give the dispersion
//!   derivative of the direction;
//! - **per-bin** trees, one per comb wavelength, for the branches whose facet sequence is not the
//!   same at all three (these become `PER_BIN` records, one wavelength each).
//!
//! Two leaves are the same branch when their signatures agree: the hash of the ordered facet ids,
//! event kinds and eigenmode labels. A branch is merged only if every ancestor and the leaf itself
//! exist in both neighbour trees and no channel took a different total-reflection decision than
//! the hero anywhere on the way. Branches whose weight is within a factor of
//! [`NEAR_CUTOFF_FACTOR`] of the cutoff are exempt from the presence test, since a branch that
//! just fell under the cutoff in a neighbour tree is not a dispersion split.

use glam::{Vec2, Vec3};

use super::{
    bounce::{Child, EventKind, PathState, expand, prepare_bounce_input, segment_principal_depths},
    record::{
        BIN_SHIFT, BOUNCE_SHIFT, COMB_LEN, COMB_NM, DEPTH_LEN, ExitRecord, FLAG_DIRECT,
        FLAG_EXTRAORDINARY, FLAG_GLARE, FLAG_PER_BIN, FLAG_TIR_DOMINATED, REFERENCE_SLOT,
        oct_encode,
    },
    signature::{BRANCH_SIG_SEED, branch_step, fold_sig},
};
use crate::{
    geometry::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        polarization::StokesVector,
        raytracer::{
            Ray,
            absorption::spectral_absorption,
            camera::HitRecord,
            intersect_stone::intersect_stone_soa_side,
            refraction::{RayMaterialContext, RayWavelengthCache},
            transport::PreparedMaterial,
        },
    },
    simd::PlanesSoA32,
};

/// Dispersion (`n_G - n_B`, gemological) above which every branch gets per-bin records.
pub const DISPERSION_GATE_BG: f32 = 0.030;

/// Slot of the lower derivative wavelength (510 nm).
pub const LOWER_SLOT: usize = REFERENCE_SLOT - 1;
/// Slot of the upper derivative wavelength (610 nm).
pub const UPPER_SLOT: usize = REFERENCE_SLOT + 1;

/// A branch lighter than this many cutoffs may be missing from a neighbour tree.
pub const NEAR_CUTOFF_FACTOR: f32 = 4.0;

/// Largest octahedral step between the neighbour directions that still counts as smooth.
const MAX_OCT_STEP: f32 = 0.25;

/// Hard cap on the nodes of one tree; the rest of the energy goes to the residual.
pub const MAX_NODES_PER_TREE: usize = 4096;

/// Offset of a restarted ray along its direction, as in the path tracer's bounce loop.
const RESTART: f32 = 1.0e-4;

const NO_PARENT: u32 = u32::MAX;

/// Gemological dispersion `|n(430.8 nm) - n(686.7 nm)|` of a material.
#[must_use]
pub fn dispersion_b_minus_g(material: &GemMaterial) -> f32 {
    (material.dispersion.evaluate(430.8) - material.dispersion.evaluate(686.7)).abs()
}

/// `true` when the material's indices differ between 510 and 610 nm.
#[must_use]
pub fn is_dispersive(material: &GemMaterial) -> bool {
    let (n_lo, n_hi) = (
        material.dispersion.evaluate(510.0),
        material.dispersion.evaluate(610.0),
    );
    let (e_lo, e_hi) = (
        material.extraordinary_index_at(510.0, n_lo),
        material.extraordinary_index_at(610.0, n_hi),
    );
    (n_lo - n_hi).abs() > 1.0e-6 || (e_lo - e_hi).abs() > 1.0e-6
}

/// The principal absorption coefficients of `material` at each of `lambdas`: the band sums the
/// tracer's wavelength cache builds its tensors from.
///
/// Slots `DEPTH_O` (the `o_ray` bands), `DEPTH_E` (`e_ray`) and `DEPTH_BETA` (`beta_ray`, zero
/// without one) of `record`.
#[must_use]
pub fn principal_alphas(
    material: &GemMaterial,
    lambdas: &[f32; COMB_LEN],
) -> [[f32; DEPTH_LEN]; COMB_LEN] {
    let absorption = &material.absorption;
    std::array::from_fn(|k| {
        let lambda = lambdas[k];
        [
            spectral_absorption(&absorption.o_ray, lambda),
            spectral_absorption(&absorption.e_ray, lambda),
            absorption
                .beta_ray
                .as_deref()
                .map_or(0.0, |bands| spectral_absorption(bands, lambda)),
        ]
    })
}

/// `true` when a band of `material` has a peak that is not zero: the material absorbs somewhere.
///
/// A colorless stone (no bands, or only zero-peak bands, as `GemMaterial::new_custom` makes)
/// has no optical depth, so its records carry zero depth.
fn absorbs(material: &GemMaterial) -> bool {
    let absorption = &material.absorption;
    absorption
        .o_ray
        .iter()
        .chain(&absorption.e_ray)
        .chain(absorption.beta_ray.iter().flatten())
        .any(|band| band.peak.abs() > 0.0)
}

/// `depth . alphas`, the operation order of `ExitRecord::optical_depth`.
fn optical_depth(depth: &[f32; DEPTH_LEN], alphas: &[f32; DEPTH_LEN]) -> f32 {
    depth[0].mul_add(alphas[0], depth[1].mul_add(alphas[1], depth[2] * alphas[2]))
}

/// `true` when no coefficient is above zero.
fn is_clear(depth: &[f32; DEPTH_LEN]) -> bool {
    depth.iter().all(|&d| d <= 0.0)
}

/// The absorption-free weight of an absorbed `weight` whose branch has optical depth `tau`.
///
/// Zero for a weight that is not positive, and for a product that left the `f32` range (a channel
/// absorbed to nothing carries no information about its smooth weight; it shades as nothing).
fn smooth_weight(weight: f32, tau: f32) -> f32 {
    if weight.is_nan() || weight <= 0.0 {
        return 0.0;
    }
    let smooth = weight * tau.exp();
    if smooth.is_finite() {
        smooth.min(1.0)
    } else {
        0.0
    }
}

/// The path tracer's material context and wavelength cache for one hero wavelength of the comb.
pub struct HeroContext<'a> {
    /// The per-ray material context (comb wavelengths, hero slot).
    pub(crate) ctx: RayMaterialContext<'a>,
    /// Quantities cached per comb (indices, absorption tensors, zone tables).
    pub(crate) cache: RayWavelengthCache,
}

/// How a tree decides a branch is too light to follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PruneBy {
    /// The largest channel intensity (merged records).
    MaxChannel,
    /// The intensity of one channel (per-bin records).
    Hero(usize),
}

#[derive(Clone, Copy)]
struct Node {
    id: u32,
    ray: Ray,
    state: PathState,
    prev_plane: Option<Vec3>,
    depth: u32,
    sig: u64,
    branch: [u32; 2],
    diverged: bool,
    glare: bool,
    total_reflections: u8,
    partial_reflections: u8,
    optical: [f32; DEPTH_LEN],
}

/// What a tree remembers of each node.
#[derive(Clone, Copy, Debug)]
pub struct NodeInfo {
    /// Signature of the facet sequence up to and including this node.
    pub sig: u64,
    /// Index of the parent node (`u32::MAX` for the root).
    pub parent: u32,
    /// A channel took a different total-reflection decision than the hero on the way here.
    pub diverged: bool,
    /// Largest channel intensity when the node was created.
    pub weight: f32,
}

/// A branch that left the stone (or the camera ray that never entered it).
#[derive(Clone, Copy, Debug)]
pub struct Leaf {
    /// Index of the node that exits.
    pub node: u32,
    /// Branch signature.
    pub sig: u64,
    /// The two-lane branch signature (the tracer's, `signature::branch_step`).
    pub branch: [u32; 2],
    /// Exit direction at the tree's hero wavelength.
    pub dir: Vec3,
    /// Stokes intensity of every comb channel, absorption included.
    pub weight: [f32; COMB_LEN],
    /// The branch's optical-depth coefficients (`bounce::segment_principal_depths`, summed).
    pub depth: [f32; DEPTH_LEN],
    /// Record flags (without the bin).
    pub flags: u32,
}

/// A branch dropped under the cutoff.
#[derive(Clone, Copy, Debug)]
pub struct Pruned {
    /// Node the dropped branch would have been a child of.
    pub parent: u32,
    /// Mean of the eight channel intensities.
    pub mean: f32,
    /// Intensity of the tree's hero channel.
    pub hero: f32,
}

/// The branch tree of one ray at one hero wavelength.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    /// Every node, parents before children.
    pub nodes: Vec<NodeInfo>,
    /// Every leaf, in discovery order.
    pub leaves: Vec<Leaf>,
    /// Every dropped branch.
    pub pruned: Vec<Pruned>,
    /// Spectral-mean weight of the branches that reached the bounce cap still inside the stone.
    ///
    /// The path tracer loses the same energy, so it is neither a record nor a residual.
    pub truncated: f32,
    node_sigs: Vec<u64>,
    leaf_index: Vec<(u64, u32)>,
}

impl Tree {
    fn finish(&mut self) {
        self.node_sigs = self.nodes.iter().map(|n| n.sig).collect();
        self.node_sigs.sort_unstable();
        self.node_sigs.dedup();
        self.leaf_index = self
            .leaves
            .iter()
            .enumerate()
            .map(|(i, leaf)| (leaf.sig, u32::try_from(i).unwrap_or(u32::MAX)))
            .collect();
        self.leaf_index.sort_unstable();
    }

    /// `true` when a node with this signature exists.
    #[must_use]
    pub fn has_node(&self, sig: u64) -> bool {
        self.node_sigs.binary_search(&sig).is_ok()
    }

    /// The leaf with this signature, if any.
    #[must_use]
    pub fn find_leaf(&self, sig: u64) -> Option<&Leaf> {
        let at = self
            .leaf_index
            .binary_search_by_key(&sig, |&(s, _)| s)
            .ok()?;
        self.leaves.get(self.leaf_index[at].1 as usize)
    }

    /// The signature of the leaf carrying the most energy (0 when there is none).
    #[must_use]
    pub fn dominant_signature(&self) -> u64 {
        self.leaves
            .iter()
            .max_by(|a, b| mean_of(&a.weight).total_cmp(&mean_of(&b.weight)))
            .map_or(0, |leaf| leaf.sig)
    }
}

fn mean_of(weights: &[f32; COMB_LEN]) -> f32 {
    weights.iter().sum::<f32>() / COMB_LEN as f32
}

fn mix_sig(parent: u64, facet: usize, kind: EventKind, is_ext: bool) -> u64 {
    let word = ((facet as u64) << 8) | (kind.code() << 1) | u64::from(is_ext);
    let mixed = (parent ^ word.wrapping_mul(0x9E37_79B9_7F4A_7C15)).rotate_left(23);
    mixed.wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ (mixed >> 29)
}

/// What one position produces.
#[derive(Clone, Debug)]
pub struct PositionOutput {
    /// The exit records, merged first, then per-bin by wavelength.
    pub records: Vec<ExitRecord>,
    /// Summed spectral-mean weight of the dropped branches.
    pub residual: f32,
    /// Signature of the dominant branch (0 for a camera ray that misses the stone).
    pub signature: u64,
}

/// What a bounce expanded into, with the geometry the children need.
struct Expanded {
    children: Vec<Child>,
    hit_point: Vec3,
    plane_normal: Vec3,
    segment: [f32; DEPTH_LEN],
}

/// Everything a tree trace reads.
pub struct TraceKit<'s, 'a> {
    planes: &'s [GpuFacetPlane],
    plane_soa: &'s PlanesSoA32,
    heroes: [HeroContext<'a>; COMB_LEN],
    max_bounces: u32,
    cutoff: f32,
    dispersive: bool,
    all_per_bin: bool,
    absorbing: bool,
    comb_alphas: [[f32; DEPTH_LEN]; COMB_LEN],
}

impl<'s, 'a> TraceKit<'s, 'a> {
    /// Builds the eight hero contexts of the material's comb.
    ///
    /// `dispersion_gate` is the gemological dispersion above which every branch gets per-bin
    /// records ([`DISPERSION_GATE_BG`]).
    #[must_use]
    pub fn new(
        planes: &'s [GpuFacetPlane],
        plane_soa: &'s PlanesSoA32,
        prepared: &PreparedMaterial<'a>,
        (max_bounces, cutoff): (u32, f32),
        dispersion_gate: f32,
    ) -> Self {
        let heroes = std::array::from_fn(|hero| {
            let (ctx, cache) = prepared.ray_context(COMB_NM, hero);
            HeroContext { ctx, cache }
        });
        Self {
            planes,
            plane_soa,
            heroes,
            max_bounces,
            cutoff,
            dispersive: is_dispersive(prepared.material),
            all_per_bin: dispersion_b_minus_g(prepared.material) > dispersion_gate,
            absorbing: absorbs(prepared.material),
            comb_alphas: principal_alphas(prepared.material, &COMB_NM),
        }
    }

    /// `true` when every branch gets per-bin records (the material gate).
    #[must_use]
    pub const fn all_per_bin(&self) -> bool {
        self.all_per_bin
    }

    /// A record's comb weights with its absorption applied at the comb wavelengths: what the tree
    /// carried for the branch.
    ///
    /// For the position probe and tests; phase B evaluates the absorption at the sample's
    /// wavelengths instead.
    #[must_use]
    pub fn comb_weights(&self, record: &ExitRecord) -> [f32; COMB_LEN] {
        let smooth = record.weights();
        if !record.has_depth() {
            return smooth;
        }
        std::array::from_fn(|k| smooth[k] * (-record.optical_depth(self.comb_alphas[k])).exp())
    }

    /// The absorption-free comb weights of `leaf`. A leaf without depth keeps its weights bit for
    /// bit.
    fn smooth_weights(&self, leaf: &Leaf) -> [f32; COMB_LEN] {
        if is_clear(&leaf.depth) {
            return leaf.weight;
        }
        std::array::from_fn(|k| {
            smooth_weight(
                leaf.weight[k],
                optical_depth(&leaf.depth, &self.comb_alphas[k]),
            )
        })
    }

    /// Traces the branch tree of `primary` for the hero wavelength in comb slot `hero`.
    #[must_use]
    pub fn trace(&self, primary: Ray, hero: usize, prune: PruneBy) -> Tree {
        let mut tree = Tree::default();
        tree.nodes.push(NodeInfo {
            sig: 0,
            parent: NO_PARENT,
            diverged: false,
            weight: 1.0,
        });
        let root = Node {
            id: 0,
            ray: primary,
            state: PathState {
                k: primary.dir,
                inside: false,
                is_ext: false,
                stokes: [StokesVector::unpolarized(1.0); COMB_LEN],
            },
            prev_plane: None,
            depth: 0,
            sig: 0,
            branch: BRANCH_SIG_SEED,
            diverged: false,
            glare: false,
            total_reflections: 0,
            partial_reflections: 0,
            optical: [0.0; DEPTH_LEN],
        };
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if node.depth >= self.max_bounces {
                tree.truncated += mean_of(&node.state.stokes.map(|s| s.intensity()));
                continue;
            }
            let hit = intersect_stone_soa_side(node.ray, self.plane_soa, self.planes.len(), &[]);
            let Some((hit, entering)) = hit else {
                if !node.state.inside {
                    tree.leaves.push(leaf_of(&node));
                }
                continue;
            };
            let facet = hit.facet_idx;
            let Expanded {
                children,
                hit_point,
                plane_normal,
                segment,
            } = self.expand_node(&node, hit, entering, hero);
            let optical: [f32; DEPTH_LEN] = std::array::from_fn(|p| node.optical[p] + segment[p]);
            for child in children {
                let next = successor(&node, &child, facet, (hit_point, plane_normal), optical);
                let weights = next.state.stokes.map(|s| s.intensity());
                let heaviest = weights.iter().fold(0.0_f32, |a, &w| a.max(w));
                let hero_weight = weights[hero];
                let judged = match prune {
                    PruneBy::MaxChannel => heaviest,
                    PruneBy::Hero(channel) => weights[channel],
                };
                if judged.is_nan() {
                    continue;
                }
                if judged < self.cutoff || tree.nodes.len() >= MAX_NODES_PER_TREE {
                    tree.pruned.push(Pruned {
                        parent: node.id,
                        mean: mean_of(&weights),
                        hero: hero_weight,
                    });
                    continue;
                }
                let id = u32::try_from(tree.nodes.len()).unwrap_or(u32::MAX);
                tree.nodes.push(NodeInfo {
                    sig: next.sig,
                    parent: node.id,
                    diverged: next.diverged,
                    weight: heaviest,
                });
                stack.push(Node { id, ..next });
            }
        }
        tree.finish();
        tree
    }

    /// One facet event: the path tracer's per-hit set-up, then the physics of the bounce.
    fn expand_node(
        &self,
        node: &Node,
        hit: HitRecord,
        entering: Option<bool>,
        hero: usize,
    ) -> Expanded {
        let hit_point = node.ray.origin + hit.t * node.ray.dir;
        let input = prepare_bounce_input(
            &self.heroes[hero],
            self.planes,
            node.state,
            node.ray,
            &hit,
            entering,
            node.prev_plane,
        );
        Expanded {
            children: expand(&input),
            hit_point,
            plane_normal: input.plane_normal,
            segment: if self.absorbing {
                segment_principal_depths(&self.heroes[hero], &node.state, entering, hit.t)
            } else {
                [0.0; DEPTH_LEN]
            },
        }
    }

    /// The records, residual and dominant signature of one camera ray.
    #[must_use]
    pub fn position(&self, primary: Ray) -> PositionOutput {
        if self.all_per_bin {
            return self.per_bin_only(primary);
        }
        let reference = self.trace(primary, REFERENCE_SLOT, PruneBy::MaxChannel);
        let neighbours = self.dispersive.then(|| {
            (
                self.trace(primary, LOWER_SLOT, PruneBy::MaxChannel),
                self.trace(primary, UPPER_SLOT, PruneBy::MaxChannel),
            )
        });
        let merge = self.merge(&reference, neighbours.as_ref());
        let mut records = merge.records;
        let mut residual = merge.residual;
        if merge.bins_needed {
            for hero in 0..COMB_LEN {
                let tree = self.trace(primary, hero, PruneBy::Hero(hero));
                residual += per_bin_records(
                    &tree,
                    hero,
                    (&merge.ok_leaf_sigs, &merge.ok_node_sigs),
                    &self.comb_alphas[hero],
                    &mut records,
                );
            }
        }
        PositionOutput {
            records,
            residual,
            signature: reference.dominant_signature(),
        }
    }

    /// Every branch as per-bin records (the dispersion gate): no merging, no neighbour trees.
    fn per_bin_only(&self, primary: Ray) -> PositionOutput {
        let mut records = Vec::new();
        let mut residual = 0.0;
        let mut signature = 0;
        for hero in 0..COMB_LEN {
            let tree = self.trace(primary, hero, PruneBy::Hero(hero));
            if hero == REFERENCE_SLOT {
                signature = tree.dominant_signature();
            }
            residual += per_bin_records(
                &tree,
                hero,
                (&[], &[]),
                &self.comb_alphas[hero],
                &mut records,
            );
        }
        PositionOutput {
            records,
            residual,
            signature,
        }
    }

    /// Merged records of the reference tree, and which branches still need per-bin records.
    fn merge(&self, reference: &Tree, neighbours: Option<&(Tree, Tree)>) -> Merge {
        let near = NEAR_CUTOFF_FACTOR * self.cutoff;
        let mut node_ok = vec![true; reference.nodes.len()];
        for (i, info) in reference.nodes.iter().enumerate() {
            let parent_ok = if info.parent == NO_PARENT {
                true
            } else {
                node_ok[info.parent as usize]
            };
            let present =
                neighbours.is_none_or(|(lo, hi)| lo.has_node(info.sig) && hi.has_node(info.sig));
            node_ok[i] = parent_ok && !info.diverged && (present || info.weight < near);
        }

        let mut records = Vec::new();
        let mut ok_leaf_sigs = Vec::new();
        let mut bins_needed = node_ok.iter().any(|&ok| !ok);
        for leaf in &reference.leaves {
            let mut ok = node_ok[leaf.node as usize];
            let mut ddir = Vec2::ZERO;
            if ok && let Some((lo, hi)) = neighbours {
                match (lo.find_leaf(leaf.sig), hi.find_leaf(leaf.sig)) {
                    (Some(a), Some(b)) => {
                        let step = oct_encode(b.dir) - oct_encode(a.dir);
                        if step.abs().max_element() > MAX_OCT_STEP {
                            ok = false;
                        } else {
                            ddir = step;
                        }
                    }
                    _ => ok = reference.nodes[leaf.node as usize].weight < near,
                }
            }
            if ok {
                records.push(
                    ExitRecord::new(
                        leaf.dir,
                        ddir,
                        self.smooth_weights(leaf),
                        leaf.flags,
                        fold_sig(leaf.branch),
                    )
                    .with_depth(leaf.depth),
                );
                ok_leaf_sigs.push(leaf.sig);
            } else {
                bins_needed = true;
            }
        }
        ok_leaf_sigs.sort_unstable();

        let mut ok_node_sigs: Vec<u64> = reference
            .nodes
            .iter()
            .zip(&node_ok)
            .filter(|&(_, &ok)| ok)
            .map(|(info, _)| info.sig)
            .collect();
        ok_node_sigs.sort_unstable();
        let residual: f32 = reference
            .pruned
            .iter()
            .filter(|p| node_ok[p.parent as usize])
            .map(|p| p.mean)
            .sum();
        Merge {
            records,
            residual,
            ok_leaf_sigs,
            ok_node_sigs,
            bins_needed,
        }
    }
}

struct Merge {
    records: Vec<ExitRecord>,
    residual: f32,
    ok_leaf_sigs: Vec<u64>,
    ok_node_sigs: Vec<u64>,
    bins_needed: bool,
}

/// Appends the per-bin records of `tree` (hero `bin`) that the merged records do not cover, and
/// returns the share of the bin's dropped weight the merged residual does not count.
///
/// `bin_alphas` are the principal absorption coefficients at the bin's comb wavelength: a record's
/// weight is the leaf's bin weight with its absorption there divided out, and the record carries
/// the leaf's optical-depth coefficients.
fn per_bin_records(
    tree: &Tree,
    bin: usize,
    (ok_leaf_sigs, ok_node_sigs): (&[u64], &[u64]),
    bin_alphas: &[f32; DEPTH_LEN],
    records: &mut Vec<ExitRecord>,
) -> f32 {
    for leaf in &tree.leaves {
        let weight = leaf.weight[bin];
        if weight <= 0.0 || ok_leaf_sigs.binary_search(&leaf.sig).is_ok() {
            continue;
        }
        let smooth = if is_clear(&leaf.depth) {
            weight
        } else {
            smooth_weight(weight, optical_depth(&leaf.depth, bin_alphas))
        };
        if smooth <= 0.0 {
            continue;
        }
        let mut only_bin = [0.0; COMB_LEN];
        only_bin[bin] = smooth;
        let flags = leaf.flags | FLAG_PER_BIN | (u32::try_from(bin).unwrap_or(0) << BIN_SHIFT);
        records.push(
            ExitRecord::new(leaf.dir, Vec2::ZERO, only_bin, flags, fold_sig(leaf.branch))
                .with_depth(leaf.depth),
        );
    }
    tree.pruned
        .iter()
        .filter(|p| {
            let parent_sig = tree.nodes[p.parent as usize].sig;
            ok_node_sigs.binary_search(&parent_sig).is_err()
        })
        .map(|p| p.hero / COMB_LEN as f32)
        .sum()
}

/// The next node of `child`, before its id is assigned.
///
/// `optical` is the parent's optical-depth coefficients plus the segment that ended at this
/// bounce.
fn successor(
    node: &Node,
    child: &Child,
    facet: usize,
    (hit_point, plane_normal): (Vec3, Vec3),
    optical: [f32; DEPTH_LEN],
) -> Node {
    let interior = child.state.inside && child.kind != EventKind::Transmit;
    Node {
        id: 0,
        ray: Ray {
            origin: hit_point + child.s * RESTART,
            dir: child.s,
        },
        state: child.state,
        prev_plane: Some(plane_normal),
        depth: node.depth + 1,
        sig: mix_sig(node.sig, facet, child.kind, child.state.is_ext),
        branch: branch_step(
            node.branch,
            u32::try_from(facet).unwrap_or(u32::MAX),
            child.kind,
            child.state.is_ext,
        ),
        diverged: node.diverged || child.diverged,
        glare: node.depth == 0 && child.kind == EventKind::Reflect && !child.state.inside,
        total_reflections: node
            .total_reflections
            .saturating_add(u8::from(interior && child.kind == EventKind::TotalReflect)),
        partial_reflections: node
            .partial_reflections
            .saturating_add(u8::from(interior && child.kind == EventKind::Reflect)),
        optical,
    }
}

fn leaf_of(node: &Node) -> Leaf {
    let mut flags = 0;
    if node.depth == 0 {
        flags |= FLAG_DIRECT;
    }
    if node.glare {
        flags |= FLAG_GLARE;
    }
    if node.state.is_ext {
        flags |= FLAG_EXTRAORDINARY;
    }
    if node.total_reflections > 0 && node.partial_reflections == 0 {
        flags |= FLAG_TIR_DOMINATED;
    }
    let bounces = node
        .total_reflections
        .saturating_add(node.partial_reflections);
    flags |= u32::from(bounces) << BOUNCE_SHIFT;
    Leaf {
        node: node.id,
        sig: node.sig,
        branch: node.branch,
        dir: node.ray.dir.normalize_or_zero(),
        weight: node.state.stokes.map(|s| s.intensity()),
        depth: node.optical,
        flags,
    }
}
