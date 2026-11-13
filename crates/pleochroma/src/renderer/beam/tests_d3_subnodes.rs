//! The absorption sub-nodes, the folded weights and the out-of-scope check (beam guide §9.1, §12.5,
//! §16.3).

use glam::{DVec2, Vec3};

use super::{
    beam::{GROUP, SampleState, SpectralInterval, SpectralNode},
    leaf::GROUP_HERO,
    subnodes::{
        DEFAULT_SUB_NODES, SubnodeCaches, accumulate_depths, depth_len, fold_factor, fold_weights,
        fold_weights_counted, out_of_scope, sub_wavelength, unrecoverable_nodes,
    },
    tests_support::{glass, sapphire},
};
use crate::{
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        raytracer::{
            LightingPreset, Ray, absorption::channel_absorption_alphas_assigned, cie_1931_cmf,
            spectral_absorption, transport::PreparedMaterial,
        },
    },
    renderer::path_cache::bounce::SegmentDepths,
};

/// The node count of the tests.
const N_LAMBDA: u32 = 48;

/// The narrow-band row's tolerance: the quadrature within 1 % of the dense reference (§16.3).
const NARROW_BAND_TOLERANCE: f64 = 0.01;

/// The relative tolerance of "equal to `exp(-αL)`" for a coefficient the tracer evaluates through
/// an eigenmode quadratic form (a few ULP of `f32`).
const CONSTANT_ALPHA_TOLERANCE: f32 = 1.0e-5;

/// The FWHM of the ruby-like band in nm.
const BAND_FWHM_NM: f32 = 15.0;

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

/// The depth data of a segment of `length` absorption-length units in the base zone.
fn segment(length: f32) -> SegmentDepths {
    let mut depths = SegmentDepths::NONE;
    depths.lengths_mm[0] = length;
    depths
}

/// A colorless glass with the single absorption band `band` for both eigenmodes.
fn banded_glass(band: AbsorptionBand) -> crate::optics::materials::GemMaterial {
    let mut material = glass();
    material.absorption = AbsorptionTensor::isotropic(vec![band]);
    material
}

/// The ruby-like band: 15 nm FWHM at 650 nm, optical depth 3 over one length unit.
fn ruby_band() -> AbsorptionBand {
    AbsorptionBand::new(650.0, BAND_FWHM_NM / 2.354_82, 3.0)
}

/// The scalar CIE weight the fold uses.
fn cie_sum(lambda_nm: f32) -> f64 {
    let c = cie_1931_cmf(lambda_nm);
    f64::from(c.x) + f64::from(c.y) + f64::from(c.z)
}

#[test]
fn the_sub_node_wavelengths_straddle_the_node_centre() {
    let node = SpectralNode(10);
    let centre = node.centre_nm(N_LAMBDA);
    // One sub-node is the node itself, bit for bit.
    assert_eq!(
        sub_wavelength(node, 0, 1, N_LAMBDA).to_bits(),
        centre.to_bits()
    );
    // Four sub-nodes: 2.083 nm apart, symmetric about the centre.
    let delta = 400.0_f32 / 48.0;
    let step = delta / 4.0;
    for s in 0..4_u8 {
        let expected = (f32::from(s) - 1.5).mul_add(step, centre);
        let got = sub_wavelength(node, usize::from(s), 4, N_LAMBDA);
        assert!((got - expected).abs() < 1e-4, "s={s}: {got} vs {expected}");
    }
    let sum: f32 = (0..4)
        .map(|s| sub_wavelength(node, s, 4, N_LAMBDA) - centre)
        .sum();
    assert!(sum.abs() < 1e-4, "sub-node offsets sum to {sum}");
}

#[test]
fn one_sub_node_reproduces_the_node_only_weights_bit_for_bit() {
    let material = banded_glass(ruby_band());
    let prepared = PreparedMaterial::new(&material);
    let caches = SubnodeCaches::new(&prepared, N_LAMBDA, 1);
    assert_eq!(caches.sub_nodes(), 1);
    assert!(caches.node_context(0).is_none());
    let spectrum = SpectralInterval::all(N_LAMBDA);
    let mut state = blank_state();
    assert!(accumulate_depths(
        &mut state,
        &segment(1.0),
        (Vec3::NEG_Y, false),
        spectrum,
        &caches
    ));
    assert_eq!(state.depth.len(), spectrum.len());
    assert_eq!(state.depth.len(), depth_len(spectrum.len(), 1));
    // Each depth is the tracer's alpha at the node centre times the length.
    for g in 0..spectrum.groups() {
        let comb = spectrum.group_lambdas(g, N_LAMBDA);
        let (ctx, cache) = prepared.ray_context(comb, GROUP_HERO);
        let alphas = channel_absorption_alphas_assigned(&ctx, &cache, Vec3::NEG_Y, false);
        for (slot, alpha) in alphas.iter().enumerate() {
            let expected = alpha.mul_add(1.0, 0.0);
            assert_eq!(state.depth[g * GROUP + slot].to_bits(), expected.to_bits());
        }
    }
    // The fold leaves the weights unchanged.
    let original: Vec<f32> = (0..spectrum.len())
        .map(|i| 0.01_f32.mul_add(f32::from(u8::try_from(i).unwrap_or(0)), 0.25))
        .collect();
    let mut folded = original.clone();
    fold_weights(&mut folded, &state.depth, spectrum, N_LAMBDA);
    for (a, b) in original.iter().zip(&folded) {
        assert_eq!(a.to_bits(), b.to_bits());
    }
    // An empty depth vector (the fold off) is the identity too.
    let mut off = original.clone();
    fold_weights(&mut off, &[], spectrum, N_LAMBDA);
    assert_eq!(off, original);
}

#[test]
fn a_constant_alpha_gives_the_beer_lambert_depth_at_every_sub_node() {
    // A band a billion nanometres wide is the constant 0.8 over the visible range.
    let alpha = 0.8_f32;
    let length = 2.0_f32;
    let material = banded_glass(AbsorptionBand::new(550.0, 1.0e9, alpha));
    let prepared = PreparedMaterial::new(&material);
    let caches = SubnodeCaches::new(&prepared, N_LAMBDA, DEFAULT_SUB_NODES);
    let spectrum = SpectralInterval::all(N_LAMBDA);
    let nodes = spectrum.len();
    let sub = caches.sub_nodes();
    assert_eq!(sub, 4);
    let mut state = blank_state();
    assert!(accumulate_depths(
        &mut state,
        &segment(length),
        (Vec3::NEG_Y, false),
        spectrum,
        &caches
    ));
    assert_eq!(state.depth.len(), depth_len(nodes, sub));
    assert_eq!(state.depth.len(), nodes * (sub + 1));
    let expected = alpha * length;
    for (i, &d) in state.depth.iter().enumerate() {
        assert!(
            (d - expected).abs() <= CONSTANT_ALPHA_TOLERANCE * expected,
            "entry {i}: {d} vs {expected}"
        );
        let t = (-d).exp();
        let t_expected = (-expected).exp();
        assert!(
            (t - t_expected).abs() <= 2.0 * CONSTANT_ALPHA_TOLERANCE * t_expected,
            "entry {i}: T {t} vs {t_expected}"
        );
    }
    // With a constant T the fold is the mean CIE ratio only.
    for i in [0, 17, 47] {
        let node = SpectralNode(u16::try_from(i).unwrap_or(0));
        let c_node = cie_sum(node.centre_nm(N_LAMBDA));
        let mean_ratio = (0..sub)
            .map(|s| cie_sum(sub_wavelength(node, s, sub, N_LAMBDA)) / c_node)
            .sum::<f64>()
            / 4.0;
        let factor = f64::from(fold_factor(&state.depth, i, spectrum, N_LAMBDA));
        assert!(
            (factor - mean_ratio).abs() <= 1.0e-4 * mean_ratio,
            "node {i}: {factor} vs {mean_ratio}"
        );
    }
}

#[test]
fn the_narrow_band_row_matches_a_dense_reference_within_one_percent() {
    let material = banded_glass(ruby_band());
    let prepared = PreparedMaterial::new(&material);
    let spectrum = SpectralInterval::all(N_LAMBDA);
    let delta = f64::from(400.0_f32 / 48.0);
    let reference: f64 = (0..400_u16)
        .map(|k| {
            let lambda = 380.5 + f32::from(k);
            let alpha = spectral_absorption(&material.absorption.o_ray, lambda);
            f64::from((-alpha).exp()) * cie_sum(lambda)
        })
        .sum();
    let mut errors = Vec::new();
    for sub_nodes in [1_u32, DEFAULT_SUB_NODES] {
        let caches = SubnodeCaches::new(&prepared, N_LAMBDA, sub_nodes);
        let sub = caches.sub_nodes();
        let mut state = blank_state();
        assert!(accumulate_depths(
            &mut state,
            &segment(1.0),
            (Vec3::NEG_Y, false),
            spectrum,
            &caches
        ));
        let mut quadrature = 0.0_f64;
        for i in 0..spectrum.len() {
            let node = SpectralNode(u16::try_from(i).unwrap_or(0));
            // The node depth: the dedicated entry, or the node's own depth for one sub-node.
            let d_node = if sub > 1 {
                state.depth[spectrum.len() * sub + i]
            } else {
                state.depth[i]
            };
            // The Stokes intensity of the state: F = 1 and the node transmittance.
            let mut weight = [(-d_node).exp()];
            let one_node = SpectralInterval::single(node.0);
            if sub > 1 {
                // Fold node `i` alone: its depth entries as an interval of one node.
                let mut entries: Vec<f32> = state.depth[i * sub..(i + 1) * sub].to_vec();
                entries.push(d_node);
                fold_weights(&mut weight, &entries, one_node, N_LAMBDA);
            }
            quadrature = (delta * cie_sum(node.centre_nm(N_LAMBDA)))
                .mul_add(f64::from(weight[0]), quadrature);
        }
        let error = (quadrature - reference).abs() / reference;
        eprintln!("narrow band row: S_lambda = {sub}: relative error {error:.3e}");
        errors.push(error);
    }
    assert!(
        errors[1] <= NARROW_BAND_TOLERANCE,
        "sub-node quadrature error {} exceeds {NARROW_BAND_TOLERANCE}",
        errors[1]
    );
}

#[test]
fn a_pleochroic_segment_equals_the_tracers_absorption_at_each_sub_node() {
    let mut material = sapphire();
    material.absorption = AbsorptionTensor::uniaxial(
        vec![AbsorptionBand::new(560.0, 40.0, 1.2)],
        vec![AbsorptionBand::new(560.0, 40.0, 0.3)],
    );
    let prepared = PreparedMaterial::new(&material);
    let caches = SubnodeCaches::new(&prepared, N_LAMBDA, DEFAULT_SUB_NODES);
    let spectrum = SpectralInterval::all(N_LAMBDA);
    let nodes = spectrum.len();
    let sub = caches.sub_nodes();
    let k = Vec3::new(0.3, -0.9, 0.2).normalize();
    let length = 1.7_f32;
    let mut per_mode = Vec::new();
    for is_ext in [false, true] {
        let mut state = blank_state();
        assert!(accumulate_depths(
            &mut state,
            &segment(length),
            (k, is_ext),
            spectrum,
            &caches
        ));
        for g in 0..spectrum.groups() {
            for s in 0..sub {
                // An independent context at the sub-node wavelengths: the tracer's own evaluation.
                let comb: [f32; GROUP] = std::array::from_fn(|slot| {
                    sub_wavelength(
                        SpectralNode(u16::try_from(g * GROUP + slot).unwrap_or(0)),
                        s,
                        sub,
                        N_LAMBDA,
                    )
                });
                let (ctx, cache) = prepared.ray_context(comb, GROUP_HERO);
                let alphas = channel_absorption_alphas_assigned(&ctx, &cache, k, is_ext);
                for (slot, alpha) in alphas.iter().enumerate() {
                    let expected = alpha.mul_add(length, 0.0);
                    let got = state.depth[(g * GROUP + slot) * sub + s];
                    assert_eq!(
                        got.to_bits(),
                        expected.to_bits(),
                        "mode {is_ext}, group {g}, sub {s}, slot {slot}"
                    );
                }
            }
        }
        assert_eq!(state.depth.len(), nodes * (sub + 1));
        per_mode.push(state.depth[27 * sub]);
    }
    eprintln!(
        "pleochroic depth at node 27 sub 0: ordinary {} extraordinary {}",
        per_mode[0], per_mode[1]
    );
}

#[test]
fn segments_accumulate_and_a_changed_layout_is_refused() {
    let material = banded_glass(ruby_band());
    let prepared = PreparedMaterial::new(&material);
    let caches = SubnodeCaches::new(&prepared, N_LAMBDA, DEFAULT_SUB_NODES);
    let spectrum = SpectralInterval::all(N_LAMBDA);
    let mut state = blank_state();
    // A zero-length segment creates the vector and adds nothing.
    assert!(accumulate_depths(
        &mut state,
        &SegmentDepths::NONE,
        (Vec3::NEG_Y, false),
        spectrum,
        &caches
    ));
    assert_eq!(state.depth.len(), depth_len(spectrum.len(), 4));
    assert!(state.depth.iter().all(|d| d.to_bits() == 0));
    // Two half segments equal the whole one.
    let mut halves = state.clone();
    for _ in 0..2 {
        assert!(accumulate_depths(
            &mut halves,
            &segment(0.5),
            (Vec3::NEG_Y, false),
            spectrum,
            &caches
        ));
    }
    let mut whole = state.clone();
    assert!(accumulate_depths(
        &mut whole,
        &segment(1.0),
        (Vec3::NEG_Y, false),
        spectrum,
        &caches
    ));
    for (a, b) in halves.depth.iter().zip(&whole.depth) {
        assert!((a - b).abs() <= 1.0e-6 * (1.0 + b.abs()), "{a} vs {b}");
    }
    // Another interval length is refused and leaves the vector alone.
    let before = whole.depth.clone();
    assert!(!accumulate_depths(
        &mut whole,
        &segment(1.0),
        (Vec3::NEG_Y, false),
        SpectralInterval::single(3),
        &caches
    ));
    assert_eq!(whole.depth, before);
    // An interval off the group grid reads the caches of its own nodes.
    let off_grid = SpectralInterval { first: 5, last: 12 };
    let mut shifted = blank_state();
    assert!(accumulate_depths(
        &mut shifted,
        &segment(1.0),
        (Vec3::NEG_Y, false),
        off_grid,
        &caches
    ));
    for i in 0..off_grid.len() {
        for s in 0..4 {
            assert_eq!(
                shifted.depth[i * 4 + s].to_bits(),
                whole.depth[(5 + i) * 4 + s].to_bits()
            );
        }
    }
}

#[test]
fn only_the_uv_lamp_presets_are_out_of_scope() {
    let mut out = 0;
    for preset in LightingPreset::ALL {
        let environment = preset.studio(1.0, 0.4, 0.35);
        assert_eq!(
            out_of_scope(&environment),
            preset.is_uv_lamp(),
            "{preset:?}"
        );
        out += usize::from(out_of_scope(&environment));
    }
    assert_eq!(out, 2);
}

/// The mean CIE ratio of `sub` sub-nodes of node `index` (the fold factor at zero depths).
fn mean_ratio(index: u16, sub: usize) -> f64 {
    let node = SpectralNode(index);
    let c_node = cie_sum(node.centre_nm(N_LAMBDA));
    (0..sub)
        .map(|s| cie_sum(sub_wavelength(node, s, sub, N_LAMBDA)) / c_node)
        .sum::<f64>()
        / f64::from(u16::try_from(sub).unwrap_or(1))
}

#[test]
fn a_deep_node_over_a_transparent_sub_node_keeps_its_interface_throughput() {
    // Centre depth 80, transparent sub-nodes, interface throughput 1.
    let node = 17_u16;
    let one_node = SpectralInterval::single(node);
    let entries = [0.0_f32, 0.0, 80.0];
    #[expect(clippy::cast_possible_truncation, reason = "exp(-80) is a normal f32")]
    let stokes = (-80.0_f64).exp() as f32;
    assert!(stokes > 0.0);
    let mut weight = [stokes];
    let lost = fold_weights_counted(&mut weight, &entries, one_node, N_LAMBDA);
    assert_eq!(lost, 0);
    let expected = mean_ratio(node, 2);
    let got = f64::from(weight[0]);
    assert!(
        (got - expected).abs() <= 1.0e-4 * expected,
        "folded {got} vs the interface throughput {expected} (the capped fold gave exp(-20) of it)"
    );
}

#[test]
fn an_underflowed_deep_node_is_counted_not_silently_lost() {
    let one_node = SpectralInterval::single(17);
    let before = unrecoverable_nodes();
    let mut weight = [0.0_f32];
    let lost = fold_weights_counted(&mut weight, &[0.0, 0.0, 120.0], one_node, N_LAMBDA);
    assert_eq!(lost, 1);
    assert_eq!(weight[0].to_bits(), 0.0_f32.to_bits());
    assert!(unrecoverable_nodes() > before);
    // A zero weight on a node that is not wide is an ordinary dead node.
    let mut dead = [0.0_f32];
    assert_eq!(
        fold_weights_counted(&mut dead, &[0.0, 0.0, 5.0], one_node, N_LAMBDA),
        0
    );
}

#[test]
fn a_node_within_the_historical_range_folds_bit_for_bit_as_before() {
    let one_node = SpectralInterval::single(30);
    for (d_node, subs) in [
        (1.0_f32, [1.0_f32, 1.0]),
        (5.0, [0.0, 2.5]),
        (59.0, [0.5, 0.0]),
    ] {
        let entries = [subs[0], subs[1], d_node];
        let original = 0.3_f32;
        let mut folded = [original];
        fold_weights(&mut folded, &entries, one_node, N_LAMBDA);
        let expected = original * fold_factor(&entries, 0, one_node, N_LAMBDA);
        assert_eq!(folded[0].to_bits(), expected.to_bits(), "depth {d_node}");
    }
    // Zero depths: the mean CIE ratio only.
    let mut flat = [1.0_f32];
    fold_weights(&mut flat, &[0.0, 0.0, 0.0], one_node, N_LAMBDA);
    let expected = mean_ratio(30, 2);
    assert!((f64::from(flat[0]) - expected).abs() <= 1.0e-5 * expected);
}
