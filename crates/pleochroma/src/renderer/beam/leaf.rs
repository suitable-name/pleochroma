//! Leaves: exit data and exit-direction interpolation (beam guide §11.1–11.2), the first-surface
//! reflection leaf (§8.6) and the GPU packing of leaves (§14.7 item 1).
//!
//! A reflection leaf holds, per vertex of its image polygon, the **unnormalised** mirrored camera
//! direction (an affine function of the image point, so Wachspress interpolation in image space
//! is exact, §11.2) and, per spectral node, the reflected Stokes intensity from
//! `path_cache::bounce`'s `expand_at`. The surface-glare factor is a lighting parameter and is
//! applied by the fragment stage when the leaf's `glare` flag is set.

use std::sync::OnceLock;

use glam::{DMat3, DVec2, DVec3, Vec3};

use super::{
    BeamSettings,
    beam::{
        Beam, GPU_LEAF_MAX_VERTICES, GROUP, GpuLeafHeader, GpuLeafNode, GpuLeafVertex,
        LEAF_FLAG_DELTAS, LEAF_FLAG_GLARE, LEAF_FLAG_REFLECTION, Leaf, PackedLeaves, SampleState,
        SampleStates, SpectralInterval, SpectralNode,
    },
    geom::{
        homography::{PlaneBasis, camera_direction_map},
        polygon::{ConvexPolygon, MAX_VERTICES},
    },
    interact::{InteractKit, IntervalHeroes, build_subnodes},
    lighting::tau_omega,
    primary::{TileRect, image_polygon},
    propagate::retrace_state,
    retrace::{RetraceKit, exact_ray},
    spectral::{centroid_follows, indices_constant_at, shrink_leaf, strictly_constant},
    stats::{BeamStats, DropReason, TILE_ALLOWANCE},
    stone::BeamStone,
    subnodes::{SubnodeCaches, fold_weights_counted},
};
use crate::{
    geometry::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, Ray, transport::PreparedMaterial},
    },
    renderer::path_cache::{
        bounce::{EventKind, ExplicitHit, PathState, expand_at},
        build::HeroContext,
        record::f32_to_f16_bits,
        signature::branch_step,
    },
};

/// The slot of a group's hero: its centre node, index 3 of 8 (beam guide §8.1).
pub const GROUP_HERO: usize = 3;

/// The fewest spectral nodes a render uses (one group).
pub const MIN_NODES: u32 = 8;

/// The most spectral nodes a render uses.
pub const MAX_NODES: u32 = 1024;

/// `τ_abs`, the absolute part of the weight interpolation check (beam guide §8.4).
pub const WEIGHT_ABS_TOLERANCE: f32 = 1.0e-6;

/// The smallest image area (square pixels) a beam is still bisected at, at the entry, inside and
/// at the exit: below it a failed check is a drop (the residual samples that region), not another
/// split.
///
/// A sixteenth of a pixel: smaller pieces add no accuracy a pixel average resolves, and without a
/// floor a beam that keeps failing near the critical angle is halved down to `max_split_depth`
/// (2^24 pieces), almost all of them drops in the end.
pub const SPLIT_FLOOR_PX2: f64 = 1.0 / 16.0;

/// The code of a merged reflect event in a branch step word (`branch_step`: `(facet << 8) | (code
/// << 1) | is_ext`).
const REFLECT_CODE: u32 = 1;

/// A weight sum as `f32`, the precision the tracer's radiance has.
#[expect(
    clippy::cast_possible_truncation,
    reason = "weights are far inside the f32 range; the tracer works in f32"
)]
const fn to_f32(v: f64) -> f32 {
    v as f32
}

/// The spectral node count a render with `settings` uses.
///
/// It is `spectral_nodes` clamped to `MIN_NODES..=MAX_NODES` and raised to a multiple of 8,
/// so a stored job with an odd value never breaks the grouping.
#[must_use]
pub fn node_count(settings: &BeamSettings) -> u32 {
    settings
        .spectral_nodes
        .clamp(MIN_NODES, MAX_NODES)
        .next_multiple_of(MIN_NODES)
}

/// Everything the reflection leaves of a frame share: the planes, the prepared material, one hero
/// context per group of 8 nodes, the camera's direction map and the recorded `τ_ω`.
pub struct ReflectionKit<'a> {
    /// The renderer's facet planes, indexed by original facet.
    planes: &'a [GpuFacetPlane],
    /// The prepared material (kept to build contexts for intervals off the group grid).
    prepared: PreparedMaterial<'a>,
    /// The hero context of every group of the full node set, hero in slot [`GROUP_HERO`].
    heroes: Vec<HeroContext<'a>>,
    /// The spectral node count `N_λ`.
    n_lambda: u32,
    /// The map `(sx, sy, 1) → unnormalised camera direction`, in `f64`.
    direction_map: DMat3,
    /// The exit-direction tolerance the leaves record.
    tau_omega: f32,
    /// The absorption sub-node caches, built on the first tile of the frame and shared (cloned,
    /// not rebuilt) by every later tile; the key is the `absorption_subnodes` they were built for.
    subnodes: OnceLock<(u32, Option<SubnodeCaches<'a>>)>,
}

/// The hero contexts of the groups of one interval: shared with the kit or built for it.
enum GroupHeroes<'k, 'a> {
    /// A slice of the kit's contexts (the interval is on the group grid).
    Shared(&'k [HeroContext<'a>]),
    /// Contexts built for an interval off the grid.
    Own(Vec<HeroContext<'a>>),
}

impl<'a> GroupHeroes<'_, 'a> {
    /// The context of group `g` of the interval.
    fn get(&self, g: usize) -> Option<&HeroContext<'a>> {
        match self {
            Self::Shared(contexts) => contexts.get(g),
            Self::Own(contexts) => contexts.get(g),
        }
    }
}

/// The hero context of the comb `lambdas`.
fn hero_for<'a>(prepared: &PreparedMaterial<'a>, lambdas: [f32; GROUP]) -> HeroContext<'a> {
    let (ctx, cache) = prepared.ray_context(lambdas, GROUP_HERO);
    HeroContext { ctx, cache }
}

impl<'a> ReflectionKit<'a> {
    /// Prepares the material and the hero contexts of `n_lambda` nodes for a frame of `size`
    /// pixels.
    ///
    /// `n_lambda` must be a multiple of 8 (see [`node_count`]). The recorded `τ_ω` comes from
    /// `lighting::tau_omega` of `environment`.
    #[must_use]
    pub fn new(
        material: &'a GemMaterial,
        planes: &'a [GpuFacetPlane],
        camera: &Camera,
        size: (u32, u32),
        environment: &EnvironmentSource<'_>,
        n_lambda: u32,
    ) -> Self {
        Self::with_prepared(
            PreparedMaterial::new(material),
            planes,
            camera,
            size,
            (environment, n_lambda),
        )
    }

    /// [`ReflectionKit::new`] over an already prepared material, so one export prepares the optics
    /// tables once for all stages.
    ///
    /// The hero contexts and everything else are exactly those of `new` for the material
    /// `prepared` was built from.
    #[must_use]
    pub fn with_prepared(
        prepared: PreparedMaterial<'a>,
        planes: &'a [GpuFacetPlane],
        camera: &Camera,
        size: (u32, u32),
        (environment, n_lambda): (&EnvironmentSource<'_>, u32),
    ) -> Self {
        let full = SpectralInterval::all(n_lambda);
        let heroes = (0..full.groups())
            .map(|g| hero_for(&prepared, full.group_lambdas(g, n_lambda)))
            .collect();
        Self {
            planes,
            prepared,
            heroes,
            n_lambda,
            direction_map: camera_direction_map(camera, size.0.max(1), size.1.max(1)),
            tau_omega: tau_omega(environment),
            subnodes: OnceLock::new(),
        }
    }

    /// The sub-node caches `settings` ask for on `material`: the absorption caches
    /// ([`build_subnodes`]) or, for a colorless dispersive stone, the CIE-only caches
    /// ([`SubnodeCaches::for_dispersive`]); `None` when the fold is off.
    fn caches_for(
        &self,
        material: &GemMaterial,
        settings: &BeamSettings,
    ) -> Option<SubnodeCaches<'a>> {
        build_subnodes(
            &self.prepared,
            &self.heroes,
            material,
            self.n_lambda,
            settings,
        )
        .or_else(|| {
            SubnodeCaches::for_dispersive(
                &self.prepared,
                material,
                self.n_lambda,
                settings.absorption_subnodes,
            )
        })
    }

    /// The sub-node caches of the frame: built once on the first call and cloned afterwards (a
    /// clone shares the contexts). Settings that ask for another sub-node count than the cached
    /// build was made for get a fresh build instead, so a kit never reads the wrong caches.
    fn shared_subnodes(
        &self,
        material: &GemMaterial,
        settings: &BeamSettings,
    ) -> Option<SubnodeCaches<'a>> {
        let key = settings.absorption_subnodes;
        let cached = self
            .subnodes
            .get_or_init(|| (key, self.caches_for(material, settings)));
        if cached.0 == key {
            cached.1.clone()
        } else {
            self.caches_for(material, settings)
        }
    }

    /// The spectral node count the kit was built for.
    #[must_use]
    pub const fn n_lambda(&self) -> u32 {
        self.n_lambda
    }

    /// The prepared material the kit reads (its optics tables are the ones the caller handed in
    /// through [`ReflectionKit::with_prepared`]).
    #[must_use]
    pub const fn prepared(&self) -> &PreparedMaterial<'a> {
        &self.prepared
    }

    /// The `τ_ω` the kit's leaves record.
    #[must_use]
    pub const fn tau_omega(&self) -> f32 {
        self.tau_omega
    }

    /// The interaction kit of one tile built on this kit's material and hero contexts.
    ///
    /// `retrace` is the exact-ray retracer of the frame, `settings` the beam settings in force,
    /// `tile` the tile the beams belong to and `max_bounces` the path tracer's bounce limit.
    ///
    /// The absorption sub-node caches are built by the first kit of the frame and shared by the
    /// kits that follow (the same contexts, not a rebuild), so a frame pays for them once.
    #[must_use]
    pub fn interact_kit<'k>(
        &'k self,
        retrace: RetraceKit<'k>,
        settings: &'k BeamSettings,
        tile: TileRect,
        max_bounces: u32,
    ) -> InteractKit<'k, 'a> {
        let subnodes = self.shared_subnodes(retrace.material, settings);
        InteractKit {
            retrace,
            planes: self.planes,
            prepared: &self.prepared,
            heroes: &self.heroes,
            settings,
            tile,
            tau_omega: self.tau_omega,
            max_bounces,
            subnodes,
            interval_heroes: IntervalHeroes::default(),
            cancel: None,
            allowance: TILE_ALLOWANCE,
        }
    }

    /// The hero contexts for the groups of `spectrum`.
    fn heroes_for(&self, spectrum: SpectralInterval) -> GroupHeroes<'_, 'a> {
        let first = usize::from(spectrum.first);
        let groups = spectrum.groups();
        let nodes = self.n_lambda as usize;
        if first.is_multiple_of(GROUP) && usize::from(spectrum.last) < nodes {
            let start = first / GROUP;
            if let Some(shared) = self.heroes.get(start..start + groups) {
                return GroupHeroes::Shared(shared);
            }
        }
        GroupHeroes::Own(
            (0..groups)
                .map(|g| hero_for(&self.prepared, spectrum.group_lambdas(g, self.n_lambda)))
                .collect(),
        )
    }
}

/// The exact first-surface reflection at one sample point of a primary beam.
struct Reflected {
    /// The unnormalised mirrored camera direction.
    direction: Vec3,
    /// The reflected Stokes intensity per node of the beam's interval.
    weights: Vec<f32>,
    /// The eigenmode label after the event.
    is_ext: bool,
}

/// `d` mirrored in the plane with unit normal `n`: `d − 2 (d·n) n`.
fn mirror(d: DVec3, n: DVec3) -> DVec3 {
    n.mul_add(DVec3::splat(-2.0 * d.dot(n)), d)
}

/// The first-surface reflection at the sample point `state` of `beam`.
///
/// The hit point is the state's ray meeting the carrier plane (the geometry decides, §13.5); the
/// physics is `expand_at`'s reflected child per group of 8 nodes.
fn reflect_at(
    kit: &ReflectionKit<'_>,
    stone: &BeamStone,
    beam: &Beam,
    heroes: &GroupHeroes<'_, '_>,
    state: &SampleState,
) -> Option<Reflected> {
    let original = stone.original_facet(beam.facet);
    let (plane_normal, plane_offset) = stone.plane(beam.facet);
    let unit_normal = plane_normal.try_normalize()?;
    let origin = state.segment_origin.as_dvec3();
    let direction = state.ray.dir.as_dvec3();
    let denominator = plane_normal.dot(direction);
    if denominator.abs() <= f64::EPSILON {
        return None;
    }
    let t = (plane_offset - plane_normal.dot(origin)) / denominator;
    if !(t.is_finite() && t > 0.0) {
        return None;
    }
    let hit = ExplicitHit {
        point: direction.mul_add(DVec3::splat(t), origin).as_vec3(),
        facet: original as usize,
        normal: Vec3::from(kit.planes.get(original as usize)?.normal),
    };
    let ray = Ray {
        origin: state.segment_origin,
        dir: state.ray.dir,
    };
    let nodes = beam.spectrum.len();
    let mut weights = Vec::with_capacity(nodes);
    let mut is_ext = false;
    for g in 0..beam.spectrum.groups() {
        let path = PathState {
            k: state.k,
            inside: state.inside,
            is_ext: state.is_ext,
            stokes: *state.stokes.get(g)?,
        };
        let expansion = expand_at(
            heroes.get(g)?,
            kit.planes,
            path,
            ray,
            hit,
            Some(!state.inside),
            state.prev_plane,
        );
        let child = expansion
            .children
            .iter()
            .find(|c| c.kind == EventKind::Reflect && c.state.inside == state.inside)?;
        if g == 0 {
            is_ext = child.state.is_ext;
        }
        let count = (nodes - g * GROUP).min(GROUP);
        weights.extend(child.state.stokes[..count].iter().map(|s| s.i));
    }
    Some(Reflected {
        direction: mirror(kit.direction_map * state.image.extend(1.0), unit_normal).as_vec3(),
        weights,
        is_ext,
    })
}

/// The external first-surface reflection of a primary beam as a leaf (beam guide §8.6, §11.1).
///
/// Per vertex of the image polygon the leaf stores the unnormalised mirrored camera direction
/// (identical for every node, a reflection does not disperse) and, per node, the Stokes intensity
/// of the reflected child, without glare. `glare` and `reflection` are set, the prefix holds one
/// reflect step, the signature is `branch_step(beam.sig, original_facet, Reflect, is_ext)` and
/// `tau_omega` is the kit's. The leaf's `tile` is 0; the caller sets it.
///
/// `None` for a beam that is not an outside primary beam, whose states are incomplete, whose ray
/// does not meet the carrier plane in front of the camera, or when `hero` was built for another
/// node count than `settings` asks for.
#[must_use]
pub fn reflection_leaf(
    beam: &Beam,
    stone: &BeamStone,
    hero: &ReflectionKit<'_>,
    settings: &BeamSettings,
) -> Option<Leaf> {
    if beam.inside || beam.bounces != 0 || hero.n_lambda != node_count(settings) {
        return None;
    }
    let count = beam.state.vertices;
    let vertex_states: Vec<&SampleState> = (0..count)
        .map(|i| beam.state.vertex(i))
        .collect::<Option<_>>()?;
    let image_points: Vec<DVec2> = vertex_states.iter().map(|s| s.image).collect();
    let image = ConvexPolygon::from_image_points(&image_points)?;
    let (kept, kept_count) = image.points_2d();
    if kept_count != count || kept[..count] != image_points[..] {
        return None;
    }
    let heroes = hero.heroes_for(beam.spectrum);
    let nodes = beam.spectrum.len();
    let mut dirs = Vec::with_capacity(count * nodes);
    let mut weights = Vec::with_capacity(count * nodes);
    let mut is_ext = false;
    for (v, state) in vertex_states.iter().enumerate() {
        let reflected = reflect_at(hero, stone, beam, &heroes, state)?;
        if v == 0 {
            is_ext = reflected.is_ext;
        }
        dirs.extend(std::iter::repeat_n(reflected.direction, nodes));
        weights.extend_from_slice(&reflected.weights);
    }
    let original = stone.original_facet(beam.facet);
    let word = (original << 8) | (REFLECT_CODE << 1) | u32::from(is_ext);
    Some(Leaf {
        tile: 0,
        image,
        dirs,
        weights,
        spectrum: beam.spectrum,
        sig: branch_step(beam.sig, original, EventKind::Reflect, is_ext),
        prefix: beam.prefix.push(word)?,
        glare: true,
        reflection: true,
        tau_omega: hero.tau_omega,
        pre_clip: None,
    })
}

/// `true` when the leaf's Wachspress weights match the exact reflection at the beam's centroid and
/// edge midpoints within `τ_w · max(I_vertices) + τ_abs` for every node (beam guide §8.4).
///
/// A beam without those check states passes; a check point whose exact reflection cannot be
/// computed fails.
#[must_use]
pub fn reflection_interpolates(
    beam: &Beam,
    stone: &BeamStone,
    hero: &ReflectionKit<'_>,
    settings: &BeamSettings,
    leaf: &Leaf,
) -> bool {
    let nodes = leaf.nodes();
    let peaks: Vec<f32> = (0..nodes)
        .map(|node| {
            leaf.weights
                .chunks_exact(nodes)
                .map(|row| row[node])
                .fold(0.0_f32, f32::max)
        })
        .collect();
    let heroes = hero.heroes_for(beam.spectrum);
    beam.state
        .points
        .iter()
        .skip(beam.state.vertices)
        .all(|state| {
            reflect_at(hero, stone, beam, &heroes, state).is_some_and(|exact| {
                let coordinates = leaf.image.wachspress(state.image);
                exact.weights.iter().enumerate().all(|(node, &value)| {
                    let interpolated = weight_from(leaf, &coordinates, node);
                    let tolerance = settings
                        .weight_tolerance
                        .mul_add(peaks[node], WEIGHT_ABS_TOLERANCE);
                    (value - interpolated).abs() <= tolerance
                })
            })
        })
}

/// The exit direction of `leaf` at the image point `image_point` for node `node` of the leaf's
/// interval: the Wachspress interpolation of the vertex directions in image space, normalised.
///
/// For a reflection leaf the interpolated quantity is the unnormalised direction and the result is
/// exact (beam guide §11.2). A zero interpolation gives the zero vector.
#[must_use]
pub fn exit_direction(leaf: &Leaf, image_point: DVec2, node: usize) -> Vec3 {
    direction_from(leaf, &leaf.image.wachspress(image_point), node)
}

/// [`exit_direction`] from precomputed Wachspress coordinates of the point. The coordinates do not
/// depend on the node, so a check computes them once per point.
fn direction_from(leaf: &Leaf, weights: &[f64; MAX_VERTICES], node: usize) -> Vec3 {
    let nodes = leaf.nodes();
    let mut sum = DVec3::ZERO;
    for (v, &w) in weights.iter().enumerate().take(leaf.image.len()) {
        if let Some(d) = leaf.dirs.get(v * nodes + node) {
            sum = d.as_dvec3().mul_add(DVec3::splat(w), sum);
        }
    }
    sum.normalize_or_zero().as_vec3()
}

/// The weight `I` of `leaf` at the image point `image_point` for node `node`: the Wachspress
/// interpolation of the vertex weights, clamped at 0 (beam guide §5.3 item 8).
#[must_use]
pub fn exit_weight(leaf: &Leaf, image_point: DVec2, node: usize) -> f32 {
    weight_from(leaf, &leaf.image.wachspress(image_point), node)
}

/// [`exit_weight`] from precomputed Wachspress coordinates of the point.
fn weight_from(leaf: &Leaf, weights: &[f64; MAX_VERTICES], node: usize) -> f32 {
    let nodes = leaf.nodes();
    let mut sum = 0.0_f64;
    for (v, &w) in weights.iter().enumerate().take(leaf.image.len()) {
        if let Some(&i) = leaf.weights.get(v * nodes + node) {
            sum = f64::from(i).mul_add(w, sum);
        }
    }
    to_f32(sum.max(0.0))
}

/// A count as `u32`, saturating (counts of a batch are far below `u32::MAX`).
fn count_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Two `f32` as one word of two `f16`: `low` in the low 16 bits, `high` in the high 16 bits.
fn pack_pair(low: f32, high: f32) -> u32 {
    u32::from(f32_to_f16_bits(low)) | (u32::from(f32_to_f16_bits(high)) << 16)
}

/// Splits the vertex ring `0..points.len()` into rings of at most [`GPU_LEAF_MAX_VERTICES`]
/// vertices, as lists of the original vertex indices.
///
/// A longer ring is cut along its longest diagonal (the first one on ties) and both halves are cut
/// again; the halves keep the original vertices, so the pieces tile the polygon exactly.
fn split_vertex_ring(points: &[DVec2]) -> Vec<Vec<usize>> {
    let mut pieces = Vec::new();
    split_ring_into(points, (0..points.len()).collect(), &mut pieces);
    pieces
}

/// The recursive step of [`split_vertex_ring`].
fn split_ring_into(points: &[DVec2], ring: Vec<usize>, pieces: &mut Vec<Vec<usize>>) {
    let n = ring.len();
    if n <= GPU_LEAF_MAX_VERTICES {
        pieces.push(ring);
        return;
    }
    let mut best: Option<(f64, usize, usize)> = None;
    for i in 0..n {
        for j in (i + 2)..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            let d = points[ring[i]].distance_squared(points[ring[j]]);
            if best.is_none_or(|(bd, _, _)| d > bd) {
                best = Some((d, i, j));
            }
        }
    }
    let Some((_, i, j)) = best else {
        pieces.push(ring);
        return;
    };
    let first: Vec<usize> = ring[i..=j].to_vec();
    let mut second: Vec<usize> = ring[j..].to_vec();
    second.extend_from_slice(&ring[..=i]);
    split_ring_into(points, first, pieces);
    split_ring_into(points, second, pieces);
}

/// The index of the hero node (the interval's centre) within the interval.
fn hero_index(leaf: &Leaf) -> usize {
    usize::from(leaf.spectrum.centre() - leaf.spectrum.first)
}

/// `true` when some vertex's direction differs between nodes of the interval.
fn has_node_deltas(leaf: &Leaf, hero: usize) -> bool {
    let nodes = leaf.nodes();
    leaf.dirs.chunks_exact(nodes).any(|row| {
        let reference = row[hero];
        row.iter().any(|&d| d != reference)
    })
}

/// Appends `leaf` (pre-split into pieces of at most 8 vertices) to `packed`.
fn push_leaf(packed: &mut PackedLeaves, leaf: &Leaf, stats: &mut BeamStats) {
    let (points, count) = leaf.image.points_2d();
    let pieces = split_vertex_ring(&points[..count]);
    stats.gpu_presplits += u64::try_from(pieces.len().saturating_sub(1)).unwrap_or(0);
    let nodes = leaf.nodes();
    let hero = hero_index(leaf);
    let deltas = has_node_deltas(leaf, hero);
    let mut flags = 0;
    if leaf.glare {
        flags |= LEAF_FLAG_GLARE;
    }
    if leaf.reflection {
        flags |= LEAF_FLAG_REFLECTION;
    }
    if deltas {
        flags |= LEAF_FLAG_DELTAS;
    }
    for piece in &pieces {
        let first_vertex = count_u32(packed.vertices.len());
        for &v in piece {
            let first_node = count_u32(packed.nodes.len());
            let hero_dir = leaf
                .dirs
                .get(v * nodes + hero)
                .copied()
                .unwrap_or(Vec3::ZERO);
            packed.vertices.push(GpuLeafVertex {
                image: points[v].as_vec2().to_array(),
                hero_dir: hero_dir.to_array(),
                first_node,
            });
            for node in 0..nodes {
                let weight = leaf.weights.get(v * nodes + node).copied().unwrap_or(0.0);
                let delta = if deltas {
                    leaf.dirs
                        .get(v * nodes + node)
                        .map_or(Vec3::ZERO, |&d| d - hero_dir)
                } else {
                    Vec3::ZERO
                };
                packed.nodes.push(GpuLeafNode {
                    weight_dx: pack_pair(weight, delta.x),
                    dy_dz: pack_pair(delta.y, delta.z),
                });
            }
        }
        packed.headers.push(GpuLeafHeader {
            sig: leaf.sig,
            first_vertex,
            vertex_count: count_u32(piece.len()),
            first_node: u32::from(leaf.spectrum.first),
            node_count: count_u32(nodes),
            flags,
            tau_omega: leaf.tau_omega,
        });
    }
}

/// Packs the leaves of a batch into the GPU layout, counting the pre-splits in `stats`
/// (`gpu_presplits`: one per extra piece).
///
/// `leaves` pairs each tile index with the tile's leaves in canonical order. A leaf with more than
/// `GPU_LEAF_MAX_VERTICES` vertices is first split into pieces of at most 8 vertices, cut along
/// the longest diagonal so the pieces tile the polygon. Weights and direction deltas are `f16`
/// through `record::f32_to_f16_bits`; the delta word is zero and the `LEAF_FLAG_DELTAS` bit clear
/// when the interval is not dispersive (every node shares the hero direction). The CSR lists the
/// tiles in the order given, with the pieces of each tile in leaf order.
#[must_use]
pub fn pack_leaves_counted<L: AsRef<[Leaf]>>(
    leaves: &[(u32, L)],
    stats: &mut BeamStats,
) -> PackedLeaves {
    let mut packed = PackedLeaves::default();
    for (tile, tile_leaves) in leaves {
        packed.tiles.push(*tile);
        packed.tile_offsets.push(count_u32(packed.headers.len()));
        for leaf in tile_leaves.as_ref() {
            push_leaf(&mut packed, leaf, stats);
        }
    }
    packed.tile_offsets.push(count_u32(packed.headers.len()));
    packed
}

/// [`pack_leaves_counted`] without the statistics.
#[must_use]
pub fn pack_leaves<L: AsRef<[Leaf]>>(leaves: &[(u32, L)]) -> PackedLeaves {
    pack_leaves_counted(leaves, &mut BeamStats::default())
}

/// The exit leaves of a transmitted beam (beam guide §8.3.1, §11.1, §11.2).
///
/// `transmitted` lies on its exit facet with `inside == false`, and its sample states are the
/// exact transmitted children (exit direction S, Stokes after the exit). The result is one or
/// more leaves, subdivided until the exit-direction check passes within `kit.tau_omega` and the
/// weight check within `τ_w`, or the depth cap is reached, whose rest is a drop. Each leaf
/// carries the `τ_ω`, the branch signature and the prefix.
///
/// Per vertex of the image polygon and per node the leaf holds the exit direction `S` of the
/// sample state (for a dispersive interval the exact exit ray of every other node, through
/// `retrace::exact_ray`) and the Stokes intensity of the state. The checks are the exit-direction
/// angle at the centroid and the edge midpoints against the Wachspress interpolation (at most
/// `τ_ω`, §11.2) and the weight check of §8.4. A beam that fails is bisected in image space,
/// the new sample points retraced with `propagate::retrace_state`, up to `max_split_depth`;
/// what still fails there is a [`DropReason::Interpolation`] drop with energy
/// `image area × peak weight`.
/// Leaves come in the depth-first order of the bisection, with `tile` 0 (the caller sets it),
/// `glare` and `reflection` false, the beam's `sig` and `prefix` and the kit's `τ_ω`.
#[must_use]
pub fn exit_leaves(
    transmitted: &Beam,
    kit: &InteractKit<'_, '_>,
    stats: &mut BeamStats,
) -> Vec<Leaf> {
    exit_leaves_checked(transmitted, kit, stats).0
}

/// [`exit_leaves`] together with whether the beam passed whole.
///
/// The flag is `true` when the beam itself passed the exit check (the result is its one leaf, no
/// subdivision); it is `false` when the beam was subdivided, whatever number of pieces survived. A
/// split can leave a single accepted piece while the rest was dropped at the floor or the depth
/// cap, which the leaf count alone cannot tell from a whole pass.
#[must_use]
pub fn exit_leaves_checked(
    transmitted: &Beam,
    kit: &InteractKit<'_, '_>,
    stats: &mut BeamStats,
) -> (Vec<Leaf>, bool) {
    let mut out = Vec::new();
    if transmitted.inside {
        return (out, false);
    }
    let flat = flat_spectrum(kit, transmitted.spectrum);
    // A single-node interval of a dispersive stone gets its bin-edge clipping inside the
    // acceptance, so a piece the clipping refuses is bisected, never refused whole; a multi-node
    // interval is clipped by the frame, which splits the interval on a refusal.
    let clip = transmitted.spectrum.len() == 1
        && !strictly_constant(kit.retrace.material, kit.retrace.n_lambda);
    let whole = exit_into(transmitted, kit, (flat, clip), 0, stats, &mut out);
    (out, whole)
}

/// `true` when every principal index of the material (base, extraordinary, biaxial curves) is
/// bitwise the same at every node of `spectrum`, so one exit direction serves the whole interval.
/// The base index alone would miss a dispersion that shows only in the other axes.
fn flat_spectrum(kit: &InteractKit<'_, '_>, spectrum: SpectralInterval) -> bool {
    indices_constant_at(
        kit.retrace.material,
        (spectrum.first..=spectrum.last)
            .map(|node| SpectralNode(node).centre_nm(kit.retrace.n_lambda)),
    )
}

/// The Stokes intensity per node of `spectrum` at a sample state, with the absorption sub-node
/// fold applied (beam guide §9.1) when the state carries sub-node optical depths.
///
/// A state with an empty `depth` (the fold is off) gives the plain Stokes intensities, bit for
/// bit; see [`fold_weights_counted`] for the folded form.
fn state_weights(
    state: &SampleState,
    spectrum: SpectralInterval,
    n_lambda: u32,
) -> Option<Vec<f32>> {
    state_weights_counted(state, spectrum, n_lambda).map(|(weights, _)| weights)
}

/// [`state_weights`] together with the number of nodes the fold could not recover: nodes whose
/// stored intensity underflowed to zero although a sub-node of theirs transmits. `0` when the fold
/// is off or nothing was lost.
fn state_weights_counted(
    state: &SampleState,
    spectrum: SpectralInterval,
    n_lambda: u32,
) -> Option<(Vec<f32>, u64)> {
    let nodes = spectrum.len();
    let mut out = Vec::with_capacity(nodes);
    for g in 0..nodes.div_ceil(GROUP) {
        let row = state.stokes.get(g)?;
        let count = (nodes - g * GROUP).min(GROUP);
        out.extend(row[..count].iter().map(|s| s.i));
    }
    let lost = if state.depth.is_empty() {
        0
    } else {
        fold_weights_counted(&mut out, &state.depth, spectrum, n_lambda)
    };
    Some((out, lost))
}

/// The exit direction per node at a sample state: the state's own `S` for the interval's centre
/// node (and for every node of a flat spectrum), the exact exit ray of the node otherwise (the
/// centre node's direction where the retracer refuses).
fn state_directions(
    state: &SampleState,
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    flat: bool,
) -> Vec<Vec3> {
    let nodes = beam.spectrum.len();
    let hero = state.ray.dir;
    if flat {
        return vec![hero; nodes];
    }
    let centre = beam.spectrum.centre();
    (0..nodes)
        .map(|n| {
            let node = beam
                .spectrum
                .first
                .saturating_add(u16::try_from(n).unwrap_or(u16::MAX));
            if node == centre {
                hero
            } else {
                exact_ray(state.image, SpectralNode(node), &beam.prefix, &kit.retrace)
                    .map_or(hero, |ray| ray.dir.as_vec3())
            }
        })
        .collect()
}

/// The largest Stokes intensity over the sample states of `beam` (an upper bound for drop
/// accounting, §12.2).
#[must_use]
pub fn peak_weight(beam: &Beam) -> f64 {
    beam.state
        .points
        .iter()
        .flat_map(|state| state.stokes.iter().flatten())
        .map(|s| f64::from(s.i))
        .fold(0.0, f64::max)
}

/// The image points of the sample states of `image`: vertices, centroid, edge midpoints, and the
/// number of vertices (§6.4).
#[must_use]
pub fn sample_points(image: &ConvexPolygon) -> (Vec<DVec2>, usize) {
    let (v, n) = image.points_2d();
    let mut points: Vec<DVec2> = v[..n].to_vec();
    points.push(image.centroid().truncate());
    points.extend((0..n).map(|i| (v[i] + v[(i + 1) % n]) * 0.5));
    (points, n)
}

/// One half of `beam` over the image piece `piece`: the carrier polygon through `image_to_plane`
/// and exact states retraced along the beam's prefix.
fn half_beam(
    beam: &Beam,
    piece: &ConvexPolygon,
    kit: &InteractKit<'_, '_>,
    basis: &PlaneBasis,
) -> Option<Beam> {
    let polygon = piece.to_plane(beam.image_to_plane, basis.frame())?;
    let (points, vertices) = sample_points(piece);
    let states = points
        .iter()
        .map(|&q| retrace_state(q, beam, kit))
        .collect::<Option<Vec<_>>>()?;
    Some(Beam {
        facet: beam.facet,
        polygon,
        apex: beam.apex,
        image_to_plane: beam.image_to_plane,
        inside: beam.inside,
        sig: beam.sig,
        prefix: beam.prefix,
        bounces: beam.bounces,
        spectrum: beam.spectrum,
        state: SampleStates {
            points: states,
            vertices,
        },
    })
}

/// Bisects any beam (inside or exit) across the longest edge of its image polygon (§7.4's seam is
/// a straight image line) and retraces the sample states of both halves exactly.
///
/// The halves keep the apex, mapping, signature, prefix and spectrum of `beam`. `None` when the
/// image polygon cannot be split, a half cannot be carried to the plane, or a new point cannot be
/// retraced (it straddles a topological boundary).
#[must_use]
pub fn split_retraced(beam: &Beam, kit: &InteractKit<'_, '_>) -> Option<(Beam, Beam)> {
    let (first, second) = image_polygon(beam)?.split_longest_edge()?;
    let basis = PlaneBasis::new(kit.retrace.stone.plane(beam.facet));
    let first = half_beam(beam, &first, kit, &basis)?;
    let second = half_beam(beam, &second, kit, &basis)?;
    Some((first, second))
}

/// The leaf of `beam` over its image polygon `image`, from the exact states at the vertices, and
/// the nodes the leaf cannot carry.
///
/// The mask has one entry per node of the beam's interval (all `false` when nothing was lost). A
/// node is marked when a vertex state reports unrecoverable nodes (its stored intensity
/// underflowed to zero behind a deep node centre) and the node's weight there is zero. The mark
/// is conservative: a node that is zero at such a vertex for another reason is left to the
/// residual too, which costs samples and never bias.
fn exit_leaf_of(
    beam: &Beam,
    image: &ConvexPolygon,
    kit: &InteractKit<'_, '_>,
    flat: bool,
) -> Option<(Leaf, Vec<bool>)> {
    let nodes = beam.spectrum.len();
    let count = beam.state.vertices;
    let mut dirs = Vec::with_capacity(count * nodes);
    let mut weights = Vec::with_capacity(count * nodes);
    let mut unrecoverable = vec![false; nodes];
    for v in 0..count {
        let state = beam.state.vertex(v)?;
        dirs.extend(state_directions(state, beam, kit, flat));
        let (row, lost) = state_weights_counted(state, beam.spectrum, kit.retrace.n_lambda)?;
        if lost > 0 {
            for (mark, &weight) in unrecoverable.iter_mut().zip(&row) {
                *mark |= weight <= 0.0;
            }
        }
        weights.extend(row);
    }
    let leaf = Leaf {
        tile: 0,
        image: *image,
        dirs,
        weights,
        spectrum: beam.spectrum,
        sig: beam.sig,
        prefix: beam.prefix,
        glare: false,
        reflection: false,
        tau_omega: kit.tau_omega,
        pre_clip: None,
    };
    Some((leaf, unrecoverable))
}

/// The maximal runs of the nodes of `spectrum` that the mask `unrecoverable` leaves alive, in node
/// order, as intervals; a node past the mask counts as alive.
fn live_runs(spectrum: SpectralInterval, unrecoverable: &[bool]) -> Vec<SpectralInterval> {
    let node_at = |offset: usize| {
        spectrum
            .first
            .saturating_add(u16::try_from(offset).unwrap_or(u16::MAX))
    };
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    for offset in 0..=spectrum.len() {
        let alive = offset < spectrum.len() && !unrecoverable.get(offset).copied().unwrap_or(false);
        match (alive, start) {
            (true, None) => start = Some(offset),
            (false, Some(first)) => {
                runs.push(SpectralInterval {
                    first: node_at(first),
                    last: node_at(offset - 1),
                });
                start = None;
            }
            _ => {}
        }
    }
    runs
}

/// The columns `offset..offset + width` of every row of `nodes` entries in `rows`.
fn columns_of<T: Copy>(rows: &[T], nodes: usize, (offset, width): (usize, usize)) -> Vec<T> {
    rows.chunks_exact(nodes)
        .flat_map(|row| row[offset..offset + width].iter().copied())
        .collect()
}

/// `leaf` over the nodes of `run` only, which must lie inside the leaf's interval.
fn restricted_to(leaf: &Leaf, run: SpectralInterval) -> Leaf {
    let columns = (usize::from(run.first - leaf.spectrum.first), run.len());
    Leaf {
        tile: leaf.tile,
        image: leaf.image,
        dirs: columns_of(&leaf.dirs, leaf.nodes(), columns),
        weights: columns_of(&leaf.weights, leaf.nodes(), columns),
        spectrum: run,
        sig: leaf.sig,
        prefix: leaf.prefix,
        glare: leaf.glare,
        reflection: leaf.reflection,
        tau_omega: leaf.tau_omega,
        pre_clip: leaf.pre_clip,
    }
}

/// The leaves a built leaf stands for once its unrecoverable nodes are left out: the leaf itself
/// when none is marked, otherwise one leaf per run of live nodes (none when every node is lost).
///
/// The kept regions of the residual follow the leaves' intervals, so the nodes left out here are
/// sampled by the residual and the image stays unbiased.
fn live_leaves(leaf: Leaf, unrecoverable: &[bool]) -> Vec<Leaf> {
    if !unrecoverable.iter().any(|&mark| mark) {
        return vec![leaf];
    }
    live_runs(leaf.spectrum, unrecoverable)
        .into_iter()
        .map(|run| restricted_to(&leaf, run))
        .collect()
}

/// The largest vertex weight of every node of `leaf`.
fn vertex_peaks(leaf: &Leaf) -> Vec<f32> {
    let nodes = leaf.nodes();
    (0..nodes)
        .map(|node| {
            leaf.weights
                .chunks_exact(nodes)
                .map(|row| row[node])
                .fold(0.0_f32, f32::max)
        })
        .collect()
}

/// The angle between two vectors in radians; infinite when either is (nearly) zero.
fn angle_between(a: Vec3, b: Vec3) -> f64 {
    if a.length_squared() < 0.25 || b.length_squared() < 0.25 {
        return f64::INFINITY;
    }
    let (a, b) = (a.as_dvec3(), b.as_dvec3());
    a.cross(b).length().atan2(a.dot(b))
}

/// `true` when the exact exit direction and weight at every check point (centroid, edge
/// midpoints) match the leaf's interpolation: the angle within `τ_ω` and the weight within
/// `τ_w · max(I_vertices) + τ_abs` for every node (§11.2, §8.4).
///
/// The nodes marked in `unrecoverable` are not part of the result and are not checked; for a flat
/// interval the direction is checked at the hero node, or at the first live
/// node when the hero is lost.
fn exit_interpolates(
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    leaf: &Leaf,
    (flat, unrecoverable): (bool, &[bool]),
) -> bool {
    let nodes = leaf.nodes();
    let peaks = vertex_peaks(leaf);
    let alive = |node: usize| !unrecoverable.get(node).copied().unwrap_or(false);
    let hero = usize::from(beam.spectrum.centre() - beam.spectrum.first);
    let direction_nodes: Vec<usize> = if flat {
        let node = if alive(hero) {
            Some(hero)
        } else {
            (0..nodes).find(|&node| alive(node))
        };
        node.into_iter().collect()
    } else {
        (0..nodes).filter(|&node| alive(node)).collect()
    };
    beam.state
        .points
        .iter()
        .skip(beam.state.vertices)
        .all(|state| {
            let Some(exact_weights) = state_weights(state, beam.spectrum, kit.retrace.n_lambda)
            else {
                return false;
            };
            let exact_dirs = state_directions(state, beam, kit, flat);
            let coordinates = leaf.image.wachspress(state.image);
            let weights_ok = (0..nodes).filter(|&node| alive(node)).all(|node| {
                let tolerance = kit
                    .settings
                    .weight_tolerance
                    .mul_add(peaks[node], WEIGHT_ABS_TOLERANCE);
                (exact_weights[node] - weight_from(leaf, &coordinates, node)).abs() <= tolerance
            });
            weights_ok
                && direction_nodes.iter().all(|&node| {
                    angle_between(exact_dirs[node], direction_from(leaf, &coordinates, node))
                        <= f64::from(leaf.tau_omega)
                })
        })
}

/// `true` when the exit Fresnel step of the interval's end nodes, traced as their own heroes at
/// the vertices of `beam`, equals the leaf's shared weight there within `τ_w · peak + τ_abs`.
///
/// The per-node state check of [`check_interval`](super::spectral::check_interval) bounds the
/// deviation of a beam's incoming Stokes vector; the transmitted Stokes vector at the exit applies
/// the Fresnel step of the centre node to every node, which near the critical angle can differ
/// between the nodes of a bin by more than the tolerance. Nodes in `unrecoverable` are not
/// compared. A vertex an end node cannot retrace fails the check.
fn end_nodes_agree(
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    leaf: &Leaf,
    unrecoverable: &[bool],
) -> bool {
    let nodes = leaf.nodes();
    let peaks = vertex_peaks(leaf);
    [beam.spectrum.first, beam.spectrum.last]
        .into_iter()
        .all(|node| {
            let offset = usize::from(node - beam.spectrum.first);
            if unrecoverable.get(offset).copied().unwrap_or(false) {
                return true;
            }
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
            let tolerance = kit
                .settings
                .weight_tolerance
                .mul_add(peaks[offset], WEIGHT_ABS_TOLERANCE);
            (0..beam.state.vertices).all(|v| {
                if kit.cancelled() {
                    return true;
                }
                let Some(point) = beam.state.vertex(v) else {
                    return false;
                };
                let Some(own) = retrace_state(point.image, &probe, kit) else {
                    return false;
                };
                let exact = state_weights(&own, probe.spectrum, kit.retrace.n_lambda)
                    .and_then(|row| row.first().copied());
                let shared = leaf.weights.get(v * nodes + offset).copied();
                exact
                    .zip(shared)
                    .is_some_and(|(exact, shared)| (exact - shared).abs() <= tolerance)
            })
        })
}

/// The recursive step of [`exit_leaves`].
///
/// A single-node piece whose centroid leaves its branch at the node's wavelength, or whose
/// bin-edge regions are not monotone (`IntervalSplit::NotMonotone` from `shrink_leaf`), is
/// bisected to the floor like a piece that fails the interpolation check: never accepted and
/// never refused whole, so a coarse render refined later and a fresh render decide alike. With
/// `clip` set the accepted single-node leaves come back clipped; a multi-node interval is clipped
/// by the frame, which splits the interval on a refusal.
///
/// A single-node piece the clipping leaves nothing of is bisected too, while it can be: the scan
/// of `shrink_leaf` is sampled, so a half can keep a region where the whole kept none, and an
/// emptied piece leaves no leaf whose pre-clip polygon a later refinement could bisect. At the
/// floor it stays emptied, counted in `residual_band_area`.
///
/// Stops at once when the kit is cancelled, and drops the beam as `DepthCap` once the tile's
/// attempted work reached the kit's allowance ([`TILE_ALLOWANCE`] unless the render gives a
/// budget). Returns `true` when `beam` itself passed the check and was pushed as one leaf.
fn exit_into(
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    (flat, clip): (bool, bool),
    depth: u32,
    stats: &mut BeamStats,
    out: &mut Vec<Leaf>,
) -> bool {
    if kit.cancelled() {
        return false;
    }
    let Some(image) = image_polygon(beam) else {
        stats.record_drop(DropReason::Sliver, 0.0);
        return false;
    };
    let area = image.area();
    stats.record_attempt();
    if stats.allowance_spent_at(kit.allowance) {
        stats.record_drop(DropReason::DepthCap, area * peak_weight(beam));
        return false;
    }
    let can_split = depth < kit.settings.max_split_depth && area > SPLIT_FLOOR_PX2;
    let built = exit_leaf_of(beam, &image, kit, flat);
    // Set when the piece passed the interpolation check but is refused whole for its topology (its
    // centroid leaves the branch, or its bin-edge regions are not monotone); such a piece is
    // bisected like an interpolation failure and keeps the drop reason `shrink_leaf` gave it.
    let mut refused = false;
    // Set when the clipping left nothing of a single-node piece that can still be bisected.
    let mut emptied = false;
    if let Some((leaf, unrecoverable)) = built
        && exit_interpolates(beam, kit, &leaf, (flat, &unrecoverable))
        && {
            // A single node has no interval to split on a topology failure, so the piece is
            // bisected here like an interpolation failure; a multi-node interval is split by
            // `shrink_leaf`.
            refused = beam.spectrum.len() == 1 && !centroid_follows(&leaf, kit.retrace);
            !refused
        }
    {
        if !flat && beam.spectrum.len() > 1 && !end_nodes_agree(beam, kit, &leaf, &unrecoverable) {
            // The nodes of the interval differ at the exit Fresnel step: a `Diverged` drop makes
            // the caller split the interval, as for a beam that fails the state check. Bisecting
            // the image would not help, so the beam is not split here.
            stats.record_drop(DropReason::Diverged, 0.0);
            return false;
        }
        let whole = !unrecoverable.iter().any(|&mark| mark);
        let band_before = stats.residual_band_area;
        match clipped_leaves(live_leaves(leaf, &unrecoverable), clip, kit, stats) {
            // The halves are clipped on their own, so the band `shrink_leaf` counted for the whole
            // piece is taken back. A lost node (`!whole`) is not a clipping result.
            Some(leaves) if clip && whole && can_split && leaves.is_empty() => {
                stats.residual_band_area = band_before;
                emptied = true;
            }
            Some(leaves) => {
                if !whole {
                    // The lost nodes stay out of every kept region, so the residual samples them.
                    // No energy is known for them, so it is a counter of its own, not a drop.
                    stats.lost_nodes += 1;
                }
                out.extend(leaves);
                return whole;
            }
            None => refused = true,
        }
    }
    split_or_end(
        beam,
        kit,
        (flat, clip),
        (depth, area),
        (can_split, refused, emptied),
        stats,
        out,
    );
    false
}

/// The tail of [`exit_into`]: bisects a piece that was not accepted, or ends it.
///
/// `depth` and `area` are the piece's split depth and image area. `can_split` says the depth
/// and the area floor allow a bisection, `refused` that the piece was refused for its topology,
/// and `emptied` that the clipping left nothing of it. A piece that cannot be bisected is a drop,
/// unless it was emptied: then it stays emptied, with the band it had before.
fn split_or_end(
    beam: &Beam,
    kit: &InteractKit<'_, '_>,
    (flat, clip): (bool, bool),
    (depth, area): (u32, f64),
    (can_split, refused, emptied): (bool, bool, bool),
    stats: &mut BeamStats,
    out: &mut Vec<Leaf>,
) {
    let halves = can_split.then(|| split_retraced(beam, kit)).flatten();
    if let Some((first, second)) = halves {
        exit_into(&first, kit, (flat, clip), depth + 1, stats, out);
        exit_into(&second, kit, (flat, clip), depth + 1, stats, out);
    } else if emptied {
        // The piece cannot be bisected after all (a new point straddles a boundary): it stays
        // emptied, with the band it had before.
        stats.residual_band_area += area;
    } else {
        // At the floor or the depth cap: a piece refused for its topology keeps the reason
        // `shrink_leaf` gave it, now with its energy.
        let reason = if refused {
            DropReason::BracketFailure
        } else {
            DropReason::Interpolation
        };
        stats.record_drop(reason, area * peak_weight(beam));
    }
}

/// The leaves of an accepted piece after the bin-edge clipping when `clip` asks for it, else as
/// they are.
///
/// `clip` is set for a single-node interval of a dispersive stone. `None` when the clipping
/// refuses a leaf (`IntervalSplit::NotMonotone`; a single-node leaf reaching the clipping is on
/// its branch, so `Topology` cannot occur): [`exit_into`] then bisects the piece. An empty list
/// means the clipping left nothing of the piece; [`exit_into`] bisects such a piece while it can.
fn clipped_leaves(
    live: Vec<Leaf>,
    clip: bool,
    kit: &InteractKit<'_, '_>,
    stats: &mut BeamStats,
) -> Option<Vec<Leaf>> {
    if !clip {
        return Some(live);
    }
    let mut out = Vec::with_capacity(live.len());
    for leaf in &live {
        out.extend(shrink_leaf(leaf, kit, kit.settings.region_mode, stats).ok()?);
    }
    Some(out)
}

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests {
    use super::*;
    use crate::{
        optics::{
            absorption::{AbsorptionBand, AbsorptionTensor},
            polarization::StokesVector,
        },
        renderer::beam::{
            beam::BranchPrefix,
            tests_support::{BeamRig, MAX_BOUNCES, diamond, glass},
        },
    };

    /// The side of the test frame in pixels.
    const FRAME: u32 = 16;

    /// The spectral node count of the tests.
    const N_LAMBDA: u32 = 48;

    /// The sub-node caches two kits of one `ReflectionKit` hold, as `(first, second)`.
    fn two_kits(
        material: GemMaterial,
        settings: (&BeamSettings, &BeamSettings),
    ) -> (
        Option<SubnodeCaches<'static>>,
        Option<SubnodeCaches<'static>>,
    ) {
        // The caches borrow the material, so the test leaks it for the length of the process.
        let material: &'static GemMaterial = Box::leak(Box::new(material));
        let rig = BeamRig::cube(material.clone());
        let stone = BeamStone::new(&rig.planes, false).expect("a closed stone");
        let scene = rig.scene(FRAME, MAX_BOUNCES);
        let planes: &'static [GpuFacetPlane] = Box::leak(rig.planes.clone().into_boxed_slice());
        let reflection = ReflectionKit::new(
            material,
            planes,
            &rig.camera,
            (FRAME, FRAME),
            &scene.environment,
            N_LAMBDA,
        );
        let tile = TileRect {
            x0: 0,
            y0: 0,
            x1: FRAME,
            y1: FRAME,
        };
        let retrace = RetraceKit {
            camera: &rig.camera,
            size: (FRAME, FRAME),
            stone: &stone,
            material,
            n_lambda: N_LAMBDA,
        };
        let first = reflection
            .interact_kit(retrace, settings.0, tile, MAX_BOUNCES)
            .subnodes;
        let second = reflection
            .interact_kit(retrace, settings.1, tile, MAX_BOUNCES)
            .subnodes;
        (first, second)
    }

    /// A glass-like material with an absorption band.
    fn banded() -> GemMaterial {
        let mut material = glass();
        material.absorption =
            AbsorptionTensor::isotropic(vec![AbsorptionBand::new(600.0, 8.0, 2.0)]);
        material
    }

    /// `flat_spectrum` of the whole interval for a kit of `material`.
    fn flat_for(material: GemMaterial) -> bool {
        let material: &'static GemMaterial = Box::leak(Box::new(material));
        let rig = BeamRig::cube(material.clone());
        let stone = BeamStone::new(&rig.planes, false).expect("a closed stone");
        let scene = rig.scene(FRAME, MAX_BOUNCES);
        let planes: &'static [GpuFacetPlane] = Box::leak(rig.planes.clone().into_boxed_slice());
        let reflection = ReflectionKit::new(
            material,
            planes,
            &rig.camera,
            (FRAME, FRAME),
            &scene.environment,
            N_LAMBDA,
        );
        let tile = TileRect {
            x0: 0,
            y0: 0,
            x1: FRAME,
            y1: FRAME,
        };
        let retrace = RetraceKit {
            camera: &rig.camera,
            size: (FRAME, FRAME),
            stone: &stone,
            material,
            n_lambda: N_LAMBDA,
        };
        let settings = BeamSettings::default();
        let kit = reflection.interact_kit(retrace, &settings, tile, MAX_BOUNCES);
        flat_spectrum(&kit, SpectralInterval::all(N_LAMBDA))
    }

    /// A dispersion only in the extraordinary index makes the interval non-flat.
    #[test]
    fn an_extraordinary_only_dispersion_gets_per_node_exit_directions() {
        assert!(flat_for(glass()), "an isotropic constant index is flat");
        let mut uniaxial = glass();
        uniaxial.uniaxial_extraordinary_dispersion =
            Some(crate::optics::dispersion::DispersionModel::Cauchy {
                a: 1.62,
                b: 0.01,
                c: 0.0,
            });
        assert!(!flat_for(uniaxial));
    }

    #[test]
    fn the_sub_node_caches_are_built_once_per_frame_and_shared_by_every_tile() {
        let settings = BeamSettings::default();
        let (first, second) = two_kits(banded(), (&settings, &settings));
        let (first, second) = (first.expect("absorbs"), second.expect("absorbs"));
        assert!(first.shares_storage(&second));
    }

    #[test]
    fn colorless_dispersive_stones_get_the_cie_caches_and_glass_none() {
        let settings = BeamSettings::default();
        let (first, second) = two_kits(diamond(), (&settings, &settings));
        let (first, second) = (first.expect("diamond disperses"), second.expect("cached"));
        assert!(first.shares_storage(&second));
        let (glass_first, glass_second) = two_kits(glass(), (&settings, &settings));
        assert!(glass_first.is_none() && glass_second.is_none());
    }

    #[test]
    fn settings_for_another_sub_node_count_get_their_own_build() {
        let four = BeamSettings::default();
        let two = BeamSettings {
            absorption_subnodes: 2,
            ..BeamSettings::default()
        };
        let one = BeamSettings {
            absorption_subnodes: 1,
            ..BeamSettings::default()
        };
        let (first, second) = two_kits(banded(), (&four, &two));
        let (first, second) = (first.expect("four"), second.expect("two"));
        assert_eq!(first.sub_nodes(), 4);
        assert_eq!(second.sub_nodes(), 2);
        assert!(!first.shares_storage(&second));
        let (_, off) = two_kits(banded(), (&four, &one));
        assert!(off.is_none(), "one sub-node tracks no depths");
    }

    /// The Stokes rows of one group with the intensities `values` (zero past them).
    fn group_row(values: &[f32]) -> Vec<[StokesVector; GROUP]> {
        vec![std::array::from_fn(|slot| {
            StokesVector::unpolarized(values.get(slot).copied().unwrap_or(0.0))
        })]
    }

    /// A sample state of `values` intensities and the depth vector `depth`.
    fn state_of(values: &[f32], depth: Vec<f32>) -> SampleState {
        SampleState {
            image: DVec2::ZERO,
            ray: Ray {
                origin: Vec3::ZERO,
                dir: Vec3::NEG_Y,
            },
            k: Vec3::NEG_Y,
            segment_origin: Vec3::ZERO,
            prev_plane: None,
            inside: false,
            is_ext: false,
            stokes: group_row(values),
            depth,
            path_length: 0.0,
        }
    }

    /// A triangle leaf over the nodes `first..=last` whose three vertices all carry `row` as
    /// weights and the direction `+y` at every node.
    fn triangle_leaf(first: u16, row: &[f32]) -> Leaf {
        let image = ConvexPolygon::from_image_points(&[
            DVec2::new(0.0, 0.0),
            DVec2::new(4.0, 0.0),
            DVec2::new(0.0, 4.0),
        ])
        .expect("a triangle");
        Leaf {
            tile: 0,
            image,
            dirs: vec![Vec3::Y; 3 * row.len()],
            weights: row.repeat(3),
            spectrum: SpectralInterval {
                first,
                last: first + u16::try_from(row.len()).expect("a few nodes") - 1,
            },
            sig: [0, 0],
            prefix: BranchPrefix::EMPTY,
            glare: false,
            reflection: false,
            tau_omega: 1.0e-4,
            pre_clip: None,
        }
    }

    /// A node whose stored intensity underflowed behind a deep centre is reported lost by the
    /// state's weights, and the others are folded as before.
    #[test]
    fn an_underflowed_centre_node_over_a_transparent_sub_node_is_reported_lost() {
        let spectrum = SpectralInterval {
            first: 17,
            last: 19,
        };
        // Two sub-nodes per node: the six sub-node depths, then the three centre depths; node 18
        // has its centre at depth 120 and transparent sub-nodes.
        let depth = vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 120.0, 0.0];
        let state = state_of(&[0.5, 0.0, 0.25], depth);
        let (weights, lost) =
            state_weights_counted(&state, spectrum, N_LAMBDA).expect("one group of Stokes data");
        assert_eq!(lost, 1);
        assert!(weights[0] > 0.0 && weights[2] > 0.0);
        assert_eq!(weights[1].to_bits(), 0.0_f32.to_bits());
        // Without sub-node depths nothing is reported.
        let plain = state_of(&[0.5, 0.0, 0.25], Vec::new());
        let (_, none) = state_weights_counted(&plain, spectrum, N_LAMBDA).expect("one group");
        assert_eq!(none, 0);
    }

    /// The leaves built over the live runs exclude the lost node.
    #[test]
    fn the_leaves_of_an_exit_with_a_lost_node_exclude_that_node() {
        let leaf = triangle_leaf(17, &[0.5, 0.0, 0.25]);
        let leaves = live_leaves(leaf.clone(), &[false, true, false]);
        assert_eq!(leaves.len(), 2);
        assert_eq!(
            leaves.iter().map(|l| l.spectrum).collect::<Vec<_>>(),
            vec![SpectralInterval::single(17), SpectralInterval::single(19)]
        );
        assert!(leaves.iter().all(|l| !l.spectrum.contains(18)));
        assert_eq!(leaves[0].weights, vec![0.5; 3]);
        assert_eq!(leaves[1].weights, vec![0.25; 3]);
        assert_eq!(leaves[0].dirs.len(), 3);
        // A lost end node leaves one run; every node lost leaves none; none lost keeps the leaf.
        let ends = live_leaves(leaf.clone(), &[true, false, false]);
        assert_eq!(
            ends.iter().map(|l| l.spectrum).collect::<Vec<_>>(),
            vec![SpectralInterval {
                first: 18,
                last: 19
            }]
        );
        assert_eq!(ends[0].weights, vec![0.0, 0.25, 0.0, 0.25, 0.0, 0.25]);
        assert_eq!(
            live_leaves(leaf.clone(), &[true, true, true]),
            Vec::<Leaf>::new()
        );
        assert_eq!(live_leaves(leaf.clone(), &[false; 3]), vec![leaf]);
    }
}
