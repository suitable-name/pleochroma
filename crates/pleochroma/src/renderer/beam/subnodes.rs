//! Absorption sub-nodes: optical depth at `S_λ` sub-nodes per node and the folded weights (beam
//! guide §9.1).
//!
//! The beam tracer's geometry lives on the fixed node set (48 nodes, 8.33 nm apart), but the
//! absorption of Cr, V and Fe stones has bands 10-30 nm wide, so `exp(-α(λ)·L)` is sampled at
//! `S_λ` **sub-nodes** per node (4 by default, 2.1 nm apart). The quadrature is
//! `Σ_node Σ_sub w · T(λ_sub) · F(λ_node) · L(λ_node) · c̄(λ_sub)`: Fresnel and the lighting at
//! the node, absorption and the CIE weights at the sub-nodes.
//!
//! # Layout of `SampleState::depth`
//!
//! For a beam interval of `n` nodes and `S_λ` sub-nodes the vector holds
//!
//! * `S_λ = 1`: `n` entries, the optical depth at each node (the sub-node is the node itself);
//! * `S_λ >= 2`: `n · (S_λ + 1)` entries: first `n · S_λ` sub-node depths, node-major
//!   (`depth[i · S_λ + s]`), then the `n` depths at the node centres (`depth[n · S_λ + i]`).
//!
//! The node-centre depths are needed because the Stokes intensity of a state already carries the
//! transmittance at the node centre `T(λ_node)`; the fold replaces it by the sub-node average
//! without recomputing the Fresnel part.
//!
//! # The fold
//!
//! The leaf weight of node `i` is the Stokes intensity `I = F · T(λ_node)`. The folded weight is
//! `I · m` with `m = (1/S_λ) Σ_s (c(λ_s) / c(λ_node)) · exp(d_node - d_s)`, where `d` are the
//! optical depths of the state and `c = x̄ + ȳ + z̄` is the sum of the CIE components. The
//! lighting stage keeps multiplying by `c̄(λ_node)` through `color::integrate_nodes_to_xyz`,
//! so the result is
//! `Σ_s w · T(λ_s) · F · c(λ_s)` summed over the three components with one scalar ratio: exact for
//! the luminance-like sum and with only a second-order error in the individual components (the
//! three CIE functions are smooth over one node bin). The ratio is folded into the weight so no
//! per-node CIE vector reaches the lighting stage and `integrate_nodes_to_xyz` stays unchanged.
//!
//! # Colorless dispersive stones
//!
//! A material without absorption bands has zero depths everywhere, and the fold reduces to the
//! mean
//! CIE ratio `(1/S_λ) Σ_s c(λ_s) / c(λ_node)`: the node-only CIE weights of a dispersive stone
//! such as diamond are off by that factor. The test
//! `the_cie_only_fold_of_a_colorless_stone_is_small_and_printed` reports its size. The caches are
//! therefore built for every material whose indices vary with the wavelength
//! ([`SubnodeCaches::for_dispersive`]); a non-dispersive material (glass) has one spectral
//! interval and is left exactly as it was, bit for bit.

use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use glam::Vec3;

use super::{
    beam::{GROUP, LAMBDA_SPAN_NM, SampleState, SpectralInterval, SpectralNode},
    leaf::GROUP_HERO,
    spectral::strictly_constant,
};
use crate::{
    optics::{
        materials::GemMaterial,
        raytracer::{
            EnvironmentSource, LightingPreset, absorption::channel_absorption_alphas_assigned,
            cie_1931_cmf, transport::PreparedMaterial,
        },
    },
    renderer::path_cache::{
        bounce::{SegmentDepths, ZONE_SLOTS},
        build::HeroContext,
        record::COMB_LEN,
    },
};

/// Default absorption sub-nodes per spectral node (`BeamSettings::absorption_subnodes`).
pub const DEFAULT_SUB_NODES: u32 = 4;

/// The most sub-nodes per node the caches build.
pub const MAX_SUB_NODES: u32 = 16;

/// The exponent `d_node - d_sub` above which the fold switches from the plain `f32` form to the
/// `f64` log-domain form. Below it `exp` stays well inside the `f32` range (it overflows near 88)
/// and the result is unchanged bit for bit; above it a nearly transparent sub-node behind a deep
/// node keeps its interface throughput.
const WIDE_FOLD_EXPONENT: f32 = 60.0;

thread_local! {
    /// The fixed wavelength factors of the fold of the grid last used on this thread.
    static RATIOS: RefCell<Option<RatioTable>> = const { RefCell::new(None) };
}

/// Nodes whose folded weight was zero while the fold needed more range than the stored throughput
/// holds (see [`unrecoverable_nodes`]).
static UNRECOVERABLE_NODES: AtomicU64 = AtomicU64::new(0);

/// The fixed wavelength factors of the fold for one grid: the ratios
/// `c(λ_sub) / c(λ_node)` depend only on `n_lambda` and `S_λ`, so they are built once per grid and
/// thread instead of per node and call. Every value is the one the direct evaluation gives.
struct RatioTable {
    /// The spectral node count of the grid.
    n_lambda: u32,
    /// The sub-nodes per node.
    sub: usize,
    /// `ratios[node * sub + s]`, or `None` for a node whose `c(λ_node)` is not positive.
    ratios: Vec<Option<f32>>,
}

impl RatioTable {
    /// Builds the table of `n_lambda` nodes with `sub` sub-nodes each.
    fn new(n_lambda: u32, sub: usize) -> Self {
        let mut ratios = Vec::with_capacity(usize::try_from(n_lambda).unwrap_or(0) * sub);
        for index in 0..n_lambda {
            let node = SpectralNode(u16::try_from(index).unwrap_or(u16::MAX));
            let c_node = cie_sum(node.centre_nm(n_lambda));
            for s in 0..sub {
                ratios.push(
                    (c_node > 0.0)
                        .then(|| cie_sum(sub_wavelength(node, s, sub, n_lambda)) / c_node),
                );
            }
        }
        Self {
            n_lambda,
            sub,
            ratios,
        }
    }

    /// The factors of node `node` (one per sub-node), `None` when `c(λ_node) <= 0` or the node is
    /// not on the grid.
    fn node(&self, node: usize) -> Option<&[Option<f32>]> {
        self.ratios.get(node * self.sub..(node + 1) * self.sub)
    }
}

/// Runs `f` with the ratio table of the grid `(n_lambda, sub)`, building it when the thread's last
/// table belongs to another grid.
fn with_ratios<R>(n_lambda: u32, sub: usize, f: impl FnOnce(&RatioTable) -> R) -> R {
    RATIOS.with(|cell| {
        let mut slot = cell.borrow_mut();
        if !slot
            .as_ref()
            .is_some_and(|t| t.n_lambda == n_lambda && t.sub == sub)
        {
            *slot = Some(RatioTable::new(n_lambda, sub));
        }
        match slot.as_ref() {
            Some(table) => f(table),
            None => f(&RatioTable::new(n_lambda, sub)),
        }
    })
}

/// How many folds so far (process-wide) left a node weight at zero although the node's sub-node
/// transmission is far above its node-centre transmission.
///
/// The Stokes intensity `F · exp(-d_node)` of such a node has underflowed in `f32`, and the
/// interface throughput `F` is not stored anywhere, so the fold cannot recover it. A diagnostic
/// only: it never feeds a render decision.
#[must_use]
pub fn unrecoverable_nodes() -> u64 {
    UNRECOVERABLE_NODES.load(Ordering::Relaxed)
}

/// The per-zone, per-channel absorption coefficients of a comb.
type CombAlphas = [[f32; COMB_LEN]; ZONE_SLOTS];

/// A small count as `f32` (counts here are at most a few hundred, exactly representable).
fn small_f32(n: usize) -> f32 {
    f32::from(u16::try_from(n).unwrap_or(u16::MAX))
}

/// The wavelength of sub-node `s` of `sub_nodes` of `node`: `λ_node + (s + ½ - S_λ/2) · Δ/S_λ`.
///
/// With `sub_nodes == 1` the result is the node centre, bit for bit.
#[must_use]
pub fn sub_wavelength(node: SpectralNode, s: usize, sub_nodes: usize, n_lambda: u32) -> f32 {
    let sub = small_f32(sub_nodes.max(1));
    let width = LAMBDA_SPAN_NM / f32::from(u16::try_from(n_lambda).unwrap_or(u16::MAX));
    let offset = (-0.5_f32).mul_add(sub, small_f32(s) + 0.5) * (width / sub);
    node.centre_nm(n_lambda) + offset
}

/// The number of entries `SampleState::depth` has for `nodes` nodes and `sub_nodes` sub-nodes
/// (see the module documentation for the layout).
#[must_use]
pub const fn depth_len(nodes: usize, sub_nodes: usize) -> usize {
    if sub_nodes <= 1 {
        nodes
    } else {
        nodes * (sub_nodes + 1)
    }
}

/// The wavelength caches of the absorption sub-nodes of one beam tree: per group of 8 nodes,
/// `S_λ` extra material contexts at the sub-node wavelengths and one at the node centres.
///
/// The groups are those of the full node set (nodes `8g..8g + 8`), so an interval that does not
/// start on a group boundary still reads the cache of the group its nodes belong to.
///
/// The contexts are shared (`Arc`), so a clone is cheap: a frame builds the caches once and every
/// tile's interaction kit holds a clone ([`Self::shares_storage`]).
#[derive(Clone)]
pub struct SubnodeCaches<'a> {
    /// The sub-nodes per node `S_λ`.
    sub_nodes: usize,
    /// The spectral node count `N_λ`.
    n_lambda: u32,
    /// The node-centre context of every group (empty when `sub_nodes == 1`).
    nodes: Arc<[HeroContext<'a>]>,
    /// The sub-node contexts, group-major: `subs[g * sub_nodes + s]`.
    subs: Arc<[HeroContext<'a>]>,
}

/// The hero context of the comb `lambdas`, hero in slot [`GROUP_HERO`].
fn context_for<'a>(prepared: &PreparedMaterial<'a>, lambdas: [f32; GROUP]) -> HeroContext<'a> {
    let (ctx, cache) = prepared.ray_context(lambdas, GROUP_HERO);
    HeroContext { ctx, cache }
}

impl<'a> SubnodeCaches<'a> {
    /// Builds the caches for `n_lambda` nodes (a multiple of 8) and `sub_nodes` sub-nodes per node
    /// (clamped to `1..=MAX_SUB_NODES`) on the frame's prepared material.
    #[must_use]
    pub fn new(prepared: &PreparedMaterial<'a>, n_lambda: u32, sub_nodes: u32) -> Self {
        let sub_nodes = usize::try_from(sub_nodes.clamp(1, MAX_SUB_NODES)).unwrap_or(1);
        let groups = usize::try_from(n_lambda).unwrap_or(0).div_ceil(GROUP);
        let last = usize::try_from(n_lambda.saturating_sub(1)).unwrap_or(0);
        let node_at = |g: usize, slot: usize| {
            SpectralNode(u16::try_from((g * GROUP + slot).min(last)).unwrap_or(u16::MAX))
        };
        let mut nodes = Vec::new();
        let mut subs = Vec::with_capacity(groups * sub_nodes);
        for g in 0..groups {
            if sub_nodes > 1 {
                let comb = std::array::from_fn(|i| node_at(g, i).centre_nm(n_lambda));
                nodes.push(context_for(prepared, comb));
            }
            for s in 0..sub_nodes {
                let comb =
                    std::array::from_fn(|i| sub_wavelength(node_at(g, i), s, sub_nodes, n_lambda));
                subs.push(context_for(prepared, comb));
            }
        }
        Self {
            sub_nodes,
            n_lambda,
            nodes: nodes.into(),
            subs: subs.into(),
        }
    }

    /// Builds the caches of a material whose geometry depends on the wavelength but that has no
    /// absorption band: the sub-node quadrature then reduces to the CIE weights (the fold with
    /// zero depths), which a dispersive colorless stone such as diamond needs.
    ///
    /// `None` when `sub_nodes < 2` (the fold is the identity) or the material's indices are
    /// bitwise the same at every node of `n_lambda` ([`strictly_constant`]): glass renders
    /// exactly as without sub-nodes.
    #[must_use]
    pub fn for_dispersive(
        prepared: &PreparedMaterial<'a>,
        material: &GemMaterial,
        n_lambda: u32,
        sub_nodes: u32,
    ) -> Option<Self> {
        (sub_nodes >= 2 && !strictly_constant(material, n_lambda))
            .then(|| Self::new(prepared, n_lambda, sub_nodes))
    }

    /// `true` when `other` is a clone of the same build (the contexts are the same allocations).
    #[must_use]
    pub fn shares_storage(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.nodes, &other.nodes) && Arc::ptr_eq(&self.subs, &other.subs)
    }

    /// The sub-nodes per node `S_λ`.
    #[must_use]
    pub const fn sub_nodes(&self) -> usize {
        self.sub_nodes
    }

    /// The spectral node count the caches were built for.
    #[must_use]
    pub const fn n_lambda(&self) -> u32 {
        self.n_lambda
    }

    /// The context of sub-node `s` of group `group` of the full node set.
    #[must_use]
    pub fn sub_context(&self, group: usize, s: usize) -> Option<&HeroContext<'a>> {
        (s < self.sub_nodes)
            .then(|| self.subs.get(group * self.sub_nodes + s))
            .flatten()
    }

    /// The node-centre context of group `group` (`None` when `S_λ == 1`).
    #[must_use]
    pub fn node_context(&self, group: usize) -> Option<&HeroContext<'a>> {
        self.nodes.get(group)
    }
}

/// The per-zone, per-channel absorption coefficients of a segment on the comb of `hero`.
///
/// The segment has wave normal `k_hat` and mode `is_ext`. The result is the tracer's own
/// evaluation (`zone_alphas` for a zoned material, the assigned-mode coefficients in zone slot 0
/// otherwise).
#[must_use]
pub fn segment_alphas(hero: &HeroContext<'_>, k_hat: Vec3, is_ext: bool) -> CombAlphas {
    let (ctx, cache) = (&hero.ctx, &hero.cache);
    if let Some(zoned) = cache.zoned.as_ref() {
        return zoned.zone_alphas(ctx, cache, k_hat, is_ext);
    }
    let mut out = [[0.0; COMB_LEN]; ZONE_SLOTS];
    out[0] = channel_absorption_alphas_assigned(ctx, cache, k_hat, is_ext);
    out
}

/// Adds the per-channel depths `channels` of group `group` to `target`, for the nodes of
/// `range = (first, last)`; `place` maps a node's index within the interval to its slot.
fn add_comb(
    target: &mut [f32],
    channels: &[f32; COMB_LEN],
    group: usize,
    range: (usize, usize),
    place: impl Fn(usize) -> usize,
) {
    for (slot, &channel) in channels.iter().enumerate() {
        let node = group * GROUP + slot;
        if node < range.0 || node > range.1 {
            continue;
        }
        if let Some(entry) = target.get_mut(place(node - range.0)) {
            *entry += channel;
        }
    }
}

/// Adds one interior segment's optical depth at every sub-node (and node centre) of `spectrum` to
/// `state.depth`.
///
/// `depths` are the segment's per-zone lengths from `prepare_bounce` / `expand_at`
/// (`Expansion::depths`); `wave` is the segment's wave normal `k` and eigenmode label **before**
/// the bounce (the state the segment was traced in, not the child's). The vector is created
/// (zero-filled, [`depth_len`] entries) on first use. A segment of zero length (outside the stone,
/// a scattering material) adds nothing. Returns `false`, changing nothing, when the vector already
/// has another length (the interval or `S_λ` changed under it) or a cache is missing.
pub fn accumulate_depths(
    state: &mut SampleState,
    depths: &SegmentDepths,
    wave: (Vec3, bool),
    spectrum: SpectralInterval,
    caches: &SubnodeCaches<'_>,
) -> bool {
    let nodes = spectrum.len();
    let sub = caches.sub_nodes;
    let needed = depth_len(nodes, sub);
    if state.depth.is_empty() {
        state.depth.resize(needed, 0.0);
    } else if state.depth.len() != needed {
        return false;
    }
    if depths.lengths_mm.iter().all(|&l| l <= 0.0) {
        return true;
    }
    let (k_hat, is_ext) = wave;
    let range = (usize::from(spectrum.first), usize::from(spectrum.last));
    for g in range.0 / GROUP..=range.1 / GROUP {
        for s in 0..sub {
            let Some(context) = caches.sub_context(g, s) else {
                return false;
            };
            let alphas = segment_alphas(context, k_hat, is_ext);
            let channels = depths.optical_depths_with(&alphas);
            add_comb(&mut state.depth, &channels, g, range, |i| i * sub + s);
        }
        if sub > 1 {
            let Some(context) = caches.node_context(g) else {
                return false;
            };
            let alphas = segment_alphas(context, k_hat, is_ext);
            let channels = depths.optical_depths_with(&alphas);
            add_comb(&mut state.depth, &channels, g, range, |i| nodes * sub + i);
        }
    }
    true
}

/// `x̄ + ȳ + z̄` at `lambda_nm`: the scalar CIE weight the fold uses.
fn cie_sum(lambda_nm: f32) -> f32 {
    let c = cie_1931_cmf(lambda_nm);
    c.x + c.y + c.z
}

/// The sub-node count `S_λ` of a depth vector of `nodes` nodes, `None` when the vector holds no
/// sub-node depths (empty, `S_λ = 1`, or a length that does not match the layout).
const fn sub_count(depth_len: usize, nodes: usize) -> Option<usize> {
    let Some(per) = depth_len.checked_div(nodes) else {
        return None;
    };
    if per < 3 || depth_len != per * nodes {
        None
    } else {
        Some(per - 1)
    }
}

/// The node-centre depth and the sub-node depths of node `index` in a depth vector of `S_λ = sub`.
fn node_depths(depth: &[f32], nodes: usize, sub: usize, index: usize) -> Option<(f32, &[f32])> {
    let d_node = *depth.get(nodes * sub + index)?;
    let subs = depth.get(index * sub..(index + 1) * sub)?;
    Some((d_node, subs))
}

/// `true` when some sub-node of the node is more than [`WIDE_FOLD_EXPONENT`] more transparent than
/// its centre.
fn is_wide(d_node: f32, subs: &[f32]) -> bool {
    subs.iter()
        .any(|&d_sub| d_node - d_sub > WIDE_FOLD_EXPONENT)
}

/// The fold factor from the cached ratios, in `f32` (the historical arithmetic) or, for a wide
/// node, in `f64`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the f64 sum is rounded once to the f32 factor the callers use"
)]
fn factor_from(ratios: &[Option<f32>], d_node: f32, subs: &[f32]) -> f32 {
    let count = small_f32(subs.len());
    if is_wide(d_node, subs) {
        let mut sum = 0.0_f64;
        for (ratio, &d_sub) in ratios.iter().zip(subs) {
            let ratio = f64::from(ratio.unwrap_or(0.0));
            sum = ratio.mul_add((f64::from(d_node) - f64::from(d_sub)).exp(), sum);
        }
        return (sum / f64::from(count)) as f32;
    }
    let mut sum = 0.0_f32;
    for (ratio, &d_sub) in ratios.iter().zip(subs) {
        let exponent = d_node - d_sub;
        sum = ratio.unwrap_or(0.0).mul_add(exponent.exp(), sum);
    }
    sum / count
}

/// The fold factor `m` of node `index` of `spectrum` (see the module documentation), `1.0` when
/// `depth` does not hold sub-node depths (empty, or `S_λ = 1`).
///
/// The factor multiplies the Stokes intensity of the node (which carries `T(λ_node)`) and gives
/// the sub-node quadrature `(1/S_λ) Σ_s T(λ_s) c(λ_s) / c(λ_node) · F`. The exponent is not
/// capped: beyond [`WIDE_FOLD_EXPONENT`] the sum is evaluated in `f64`, and the factor is `inf`
/// once it exceeds the `f32` range. [`fold_weights_counted`] never forms that factor for a wide
/// node but combines it with the weight in the log domain.
#[must_use]
pub fn fold_factor(depth: &[f32], index: usize, spectrum: SpectralInterval, n_lambda: u32) -> f32 {
    let nodes = spectrum.len();
    let Some(sub) = sub_count(depth.len(), nodes).filter(|_| index < nodes) else {
        return 1.0;
    };
    let Some((d_node, subs)) = node_depths(depth, nodes, sub, index) else {
        return 1.0;
    };
    let node = usize::from(spectrum.first).saturating_add(index);
    with_ratios(n_lambda, sub, |table| match table.node(node) {
        Some(ratios) if ratios.iter().all(Option::is_some) => factor_from(ratios, d_node, subs),
        _ => 1.0,
    })
}

/// The folded weight of a positive `weight` of a wide node: `(1/S_λ) Σ_s r_s · exp(ln w + d_node -
/// d_sub)` in `f64`, so the interface throughput survives a node depth beyond the `f32` range of
/// `exp(-d)`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the f64 sum is rounded once to the f32 weight the leaf stores"
)]
fn wide_weight(weight: f32, ratios: &[Option<f32>], d_node: f32, subs: &[f32]) -> f32 {
    let log_weight = f64::from(weight).ln() + f64::from(d_node);
    let mut sum = 0.0_f64;
    for (ratio, &d_sub) in ratios.iter().zip(subs) {
        let ratio = f64::from(ratio.unwrap_or(0.0));
        sum = ratio.mul_add((log_weight - f64::from(d_sub)).exp(), sum);
    }
    (sum / f64::from(small_f32(subs.len()))) as f32
}

/// Folds the sub-node absorption into the node weights `weights` (one per node of `spectrum`, the
/// Stokes intensities of a sample state) in place, from the state's `depth`.
///
/// `weights[i] *= fold_factor(depth, i, ..)`. Zero weights stay zero; a `depth` without sub-node
/// entries (empty, or `S_λ = 1`) leaves the weights bit for bit as they are. A node whose
/// sub-nodes are more than [`WIDE_FOLD_EXPONENT`] more transparent than its centre is folded
/// in the log domain, so its interface throughput is not capped away.
///
/// Returns the number of nodes left at zero although they are wide (see [`unrecoverable_nodes`]),
/// which this call also adds to that process-wide count.
pub fn fold_weights_counted(
    weights: &mut [f32],
    depth: &[f32],
    spectrum: SpectralInterval,
    n_lambda: u32,
) -> u64 {
    let nodes = spectrum.len();
    let Some(sub) = sub_count(depth.len(), nodes) else {
        return 0;
    };
    let mut lost = 0_u64;
    with_ratios(n_lambda, sub, |table| {
        for (i, weight) in weights.iter_mut().enumerate().take(nodes) {
            if weight.is_nan() || *weight < 0.0 {
                continue;
            }
            let Some((d_node, subs)) = node_depths(depth, nodes, sub, i) else {
                continue;
            };
            let Some(ratios) = table
                .node(usize::from(spectrum.first).saturating_add(i))
                .filter(|r| r.iter().all(Option::is_some))
            else {
                continue;
            };
            if *weight > 0.0 {
                *weight = if is_wide(d_node, subs) {
                    wide_weight(*weight, ratios, d_node, subs)
                } else {
                    *weight * factor_from(ratios, d_node, subs)
                };
            } else if is_wide(d_node, subs) {
                lost += 1;
            }
        }
    });
    UNRECOVERABLE_NODES.fetch_add(lost, Ordering::Relaxed);
    lost
}

/// Folds the sub-node absorption into the node weights; see [`fold_weights_counted`], whose count
/// this discards.
pub fn fold_weights(weights: &mut [f32], depth: &[f32], spectrum: SpectralInterval, n_lambda: u32) {
    fold_weights_counted(weights, depth, spectrum, n_lambda);
}

/// `true` when the lighting is outside the beam tracer's scope: the UV-lamp presets (guide §12.5).
///
/// `BeamFrame::new` answers such a frame with the silent `BeamOff::OutOfScopeLighting`.
#[must_use]
pub fn out_of_scope(environment: &EnvironmentSource<'_>) -> bool {
    environment
        .lighting_preset()
        .is_some_and(LightingPreset::is_uv_lamp)
}

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests {
    use glam::DVec2;

    use super::*;
    use crate::{
        optics::{
            absorption::{AbsorptionBand, AbsorptionTensor},
            raytracer::Ray,
        },
        renderer::beam::tests_support::{diamond, glass},
    };

    /// The node count of the tests.
    const N_LAMBDA: u32 = 48;

    /// The centre of the test band in nm.
    const BAND_CENTRE_NM: f32 = 600.0;

    /// The width (sigma) of the test band in nm.
    const BAND_SIGMA_NM: f32 = 8.0;

    /// The peak of the test band per length unit.
    const BAND_PEAK: f32 = 2.0;

    /// A sample state with no Stokes data and an empty depth vector.
    fn blank_state() -> SampleState {
        SampleState {
            image: DVec2::ZERO,
            ray: Ray {
                origin: Vec3::ZERO,
                dir: Vec3::NEG_Y,
            },
            k: Vec3::NEG_Y,
            segment_origin: Vec3::ZERO,
            prev_plane: None,
            inside: true,
            is_ext: false,
            stokes: Vec::new(),
            depth: Vec::new(),
            path_length: 0.0,
        }
    }

    /// The depth data of a segment of `length` units in the base zone.
    fn segment(length: f32) -> SegmentDepths {
        let mut depths = SegmentDepths::NONE;
        depths.lengths_mm[0] = length;
        depths
    }

    /// The depth vector `caches` accumulate for a banded segment.
    fn depths_of(caches: &SubnodeCaches<'_>) -> Vec<f32> {
        let mut state = blank_state();
        assert!(accumulate_depths(
            &mut state,
            &segment(1.5),
            (Vec3::NEG_Y, false),
            SpectralInterval::all(N_LAMBDA),
            caches
        ));
        state.depth
    }

    #[test]
    fn a_clone_of_the_caches_is_the_same_build_and_accumulates_the_same_bits() {
        let mut material = glass();
        material.absorption = AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
            BAND_CENTRE_NM,
            BAND_SIGMA_NM,
            BAND_PEAK,
        )]);
        let prepared = PreparedMaterial::new(&material);
        let built = SubnodeCaches::new(&prepared, N_LAMBDA, DEFAULT_SUB_NODES);
        let shared = built.clone();
        assert!(built.shares_storage(&shared));
        let fresh = SubnodeCaches::new(&prepared, N_LAMBDA, DEFAULT_SUB_NODES);
        assert!(!built.shares_storage(&fresh));
        let bits = |depth: Vec<f32>| depth.into_iter().map(f32::to_bits).collect::<Vec<_>>();
        let expected = bits(depths_of(&fresh));
        assert!(expected.iter().any(|&b| f32::from_bits(b) > 0.0));
        assert_eq!(bits(depths_of(&built)), expected);
        assert_eq!(bits(depths_of(&shared)), expected);
    }

    #[test]
    fn only_a_dispersive_material_gets_the_cie_only_caches() {
        let diamond = diamond();
        let glass = glass();
        let prepared_diamond = PreparedMaterial::new(&diamond);
        let prepared_glass = PreparedMaterial::new(&glass);
        let caches =
            SubnodeCaches::for_dispersive(&prepared_diamond, &diamond, N_LAMBDA, DEFAULT_SUB_NODES)
                .expect("diamond disperses");
        assert_eq!(
            caches.sub_nodes(),
            usize::try_from(DEFAULT_SUB_NODES).unwrap_or(0)
        );
        assert!(
            SubnodeCaches::for_dispersive(&prepared_glass, &glass, N_LAMBDA, DEFAULT_SUB_NODES)
                .is_none(),
            "glass keeps node-only weights, bit for bit"
        );
        assert!(
            SubnodeCaches::for_dispersive(&prepared_diamond, &diamond, N_LAMBDA, 1).is_none(),
            "one sub-node is the identity"
        );
    }

    #[test]
    fn the_cie_only_fold_of_a_colorless_stone_is_small_and_printed() {
        let spectrum = SpectralInterval::all(N_LAMBDA);
        let nodes = spectrum.len();
        let depth =
            vec![0.0_f32; depth_len(nodes, usize::try_from(DEFAULT_SUB_NODES).unwrap_or(4))];
        let mut worst = 0.0_f32;
        let mut sum = 0.0_f32;
        let mut middle = 0.0_f32;
        for i in 0..nodes {
            let factor = fold_factor(&depth, i, spectrum, N_LAMBDA);
            assert!(factor.is_finite() && factor > 0.0, "node {i}: {factor}");
            let bias = (factor - 1.0).abs();
            worst = worst.max(bias);
            sum += bias;
            if (nodes / 4..3 * nodes / 4).contains(&i) {
                middle = middle.max(bias);
            }
        }
        eprintln!(
            "CIE-only fold of a colorless stone: mean |m-1| {:e}, middle half max {middle:e}, \
             largest {worst:e} (the bias node-only weights have)",
            sum / small_f32(nodes)
        );
        assert!(worst > 0.0, "the fold must act on a colorless stone");
    }
}
