//! Accuracy, off-identity, layout and variant tests of the optics lookup tables.

use super::{
    EXP_TABLE_MIN_ARG, EXP_VALUES, ExpTable, FRESNEL_REL_TOLERANCE, FresnelAmplitudes, FresnelSide,
    GPU_BRANCH_DATA, GPU_EXP_BASE, GPU_HEADER_ENTRY, GPU_HEADER_EXIT, OPTICS_TABLES_BINDING,
    OPTICS_TABLES_WGSL, OpticsTableSet, OpticsTables, beer_lambert_factors, closed_form_reflection,
    default_optics_tables, exp_table_variant, fresnel_amplitudes, gpu::OPTICS_TABLES_CHECK_WGSL,
    material_index_range, optics_tables_variant, pack_disabled_gpu_words, pack_gpu_words,
    reflection_amplitudes,
};
use crate::optics::materials::GemMaterial;
use naga::{
    front::wgsl,
    valid::{Capabilities, ValidationFlags, Validator},
};

/// The bound on the reflectance (absolute) and the transmission (relative) errors.
const REQUIRED_BOUND: f64 = 1e-4;

/// A deterministic pseudo-random sequence in `[0, 1)` (64-bit LCG, top 24 bits).
struct Lcg(u64);

impl Lcg {
    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f64) / 16_777_216.0
    }
}

/// The four amplitudes from their definitions, in `f64`.
fn exact_amplitudes(n1: f64, n2: f64, cos_i: f64, cos_t: f64) -> [f64; 4] {
    let sum_s = n1.mul_add(cos_i, n2 * cos_t);
    let sum_p = n2.mul_add(cos_i, n1 * cos_t);
    [
        n1.mul_add(cos_i, -(n2 * cos_t)) / sum_s,
        n2.mul_add(cos_i, -(n1 * cos_t)) / sum_p,
        2.0 * n1 * cos_i / sum_s,
        2.0 * n1 * cos_i / sum_p,
    ]
}

/// Worst errors of a table set against the definitions over a sweep.
#[derive(Default, Debug)]
struct Worst {
    amplitude_abs: f64,
    reflectance_abs: f64,
    transmission_rel: f64,
    served: usize,
}

impl Worst {
    fn take(&mut self, got: FresnelAmplitudes, exact: [f64; 4]) {
        let got_all = [got.r_s, got.r_p, got.t_s, got.t_p].map(f64::from);
        for (g, e) in got_all.iter().zip(&exact).take(2) {
            self.amplitude_abs = self.amplitude_abs.max((g - e).abs());
        }
        let r_got = 0.5 * got_all[0].mul_add(got_all[0], got_all[1] * got_all[1]);
        let r_exact = 0.5 * exact[0].mul_add(exact[0], exact[1] * exact[1]);
        self.reflectance_abs = self.reflectance_abs.max((r_got - r_exact).abs());
        for (g, e) in got_all.iter().zip(&exact).skip(2) {
            self.transmission_rel = self
                .transmission_rel
                .max((g - e).abs() / e.abs().max(1e-12));
        }
        self.served += 1;
    }
}

/// Sweeps entry and exit over `(n, u)` points that avoid the build's own probe points and
/// returns the worst errors. Fails if any in-range point is not served.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the probe values are rounded to f32, the precision the tracer works in"
)]
fn sweep(set: &OpticsTableSet, n_lo: f32, n_hi: f32, points: usize) -> Worst {
    let mut rng = Lcg(0x5EED_0001);
    let mut worst = Worst::default();
    for _ in 0..points {
        let n = (f64::from(n_hi) - f64::from(n_lo)).mul_add(rng.unit(), f64::from(n_lo));
        let n = n as f32;
        let u = rng.unit() as f32;
        let other = (1.0 - f64::from(u).mul_add(-f64::from(u), 1.0) / (f64::from(n) * f64::from(n)))
            .max(0.0)
            .sqrt() as f32;
        // Entry: air to glass, the key is cos_i.
        let entry = set
            .fresnel(1.0, n, u, other)
            .expect("an in-range entry must be served");
        worst.take(
            entry,
            exact_amplitudes(1.0, f64::from(n), f64::from(u), f64::from(other)),
        );
        // Exit: glass to air, the key is cos_t, the other cosine is cos_i.
        let exit = set
            .fresnel(n, 1.0, other, u)
            .expect("an in-range exit must be served");
        worst.take(
            exit,
            exact_amplitudes(f64::from(n), 1.0, f64::from(other), f64::from(u)),
        );
    }
    worst
}

fn builtin(name: &str) -> GemMaterial {
    GemMaterial::by_name(name).expect("a built-in material")
}

#[test]
fn exp_table_relative_error_is_below_the_bound() {
    let table = ExpTable::new();
    assert_eq!(table.values().len(), EXP_VALUES);
    let mut rng = Lcg(7);
    let mut worst = 0.0f64;
    for k in 0..200_000 {
        // Half uniform over the whole range, half dense near zero where most depths live.
        let a = if k % 2 == 0 {
            -f64::from(-EXP_TABLE_MIN_ARG) * rng.unit()
        } else {
            -4.0 * rng.unit()
        };
        #[expect(clippy::cast_possible_truncation, reason = "probe rounded to f32")]
        let a = a as f32;
        let got = f64::from(table.exp(a));
        let exact = f64::from(a).exp();
        worst = worst.max(((got - exact) / exact).abs());
    }
    println!("exp table worst relative error: {worst:.3e}");
    assert!(worst < 2.5e-5, "worst relative error {worst:e}");
    assert!(worst < REQUIRED_BOUND);
    assert_eq!(table.exp(0.0), 1.0);
}

#[test]
fn exp_outside_the_table_range_is_the_standard_exponential() {
    let table = ExpTable::new();
    for a in [0.5f32, 3.0, -88.0, -200.0, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(table.exp(a).to_bits(), a.exp().to_bits(), "a = {a}");
    }
    assert!(table.exp(f32::NAN).is_nan());
}

#[test]
fn exp_array_falls_back_to_the_polynomial_for_the_whole_array() {
    let table = ExpTable::new();
    let mut args = [-0.3f32, -1.0, -2.5, -0.01, -7.0, -0.2, -0.9, -12.0];
    let inside = table.exp_array(args);
    for (lane, value) in inside.iter().enumerate() {
        assert_eq!(value.to_bits(), table.exp(args[lane]).to_bits());
    }
    args[5] = 0.7;
    let outside = table.exp_array(args);
    let polynomial = crate::simd::exp_f32x8(args);
    assert_eq!(outside.map(f32::to_bits), polynomial.map(f32::to_bits));
}

#[test]
fn fresnel_tables_meet_the_error_bounds_for_builtin_and_custom_materials() {
    let custom = GemMaterial::new_custom("tables probe", 1.76, 0.02, 0.0, [0.5, 1.0, 1.5]);
    let materials = [
        builtin("Diamond"),
        builtin("Zircon"),
        builtin("Tanzanite"),
        custom,
    ];
    for material in &materials {
        let (n_lo, n_hi) = material_index_range(material).expect("a finite index range");
        let set = OpticsTableSet::for_material(material);
        let worst = sweep(&set, n_lo, n_hi, 40_000);
        println!(
            "{}: n in [{n_lo:.3}, {n_hi:.3}], entry {:?} exit {:?}, {worst:?}",
            material.name,
            set.entry()
                .map(|b| (b.index_cells(), b.key_cells(), b.max_rel_error())),
            set.exit()
                .map(|b| (b.index_cells(), b.key_cells(), b.max_rel_error())),
        );
        assert_eq!(worst.served, 80_000, "{}", material.name);
        assert!(
            worst.amplitude_abs < REQUIRED_BOUND,
            "{}: {worst:?}",
            material.name
        );
        assert!(
            worst.reflectance_abs < REQUIRED_BOUND,
            "{}: {worst:?}",
            material.name
        );
        assert!(
            worst.transmission_rel < REQUIRED_BOUND,
            "{}: {worst:?}",
            material.name
        );
    }
}

#[test]
fn every_branch_measures_below_the_build_tolerance() {
    for name in ["Diamond", "Zircon", "Tanzanite"] {
        let set = OpticsTableSet::for_material(&builtin(name));
        for (side, branch) in [
            (FresnelSide::Entry, set.entry()),
            (FresnelSide::Exit, set.exit()),
        ] {
            let branch = branch.unwrap_or_else(|| panic!("{name} {side:?} must be built"));
            assert_eq!(branch.side(), side);
            assert!(
                branch.max_rel_error() <= FRESNEL_REL_TOLERANCE,
                "{name} {side:?}: {}",
                branch.max_rel_error()
            );
            assert_eq!(
                branch.values().len(),
                2 * (branch.index_cells() + 1) * (branch.key_cells() + 1)
            );
        }
    }
}

#[test]
fn the_index_range_covers_the_material_and_stays_above_one() {
    let diamond = builtin("Diamond");
    let (n_lo, n_hi) = material_index_range(&diamond).expect("range");
    for nm in [380.0f32, 450.0, 550.0, 650.0, 780.0] {
        let n = diamond.dispersion.evaluate(nm);
        assert!(
            n_lo < n && n < n_hi,
            "n({nm}) = {n} outside [{n_lo}, {n_hi}]"
        );
    }
    assert!(n_lo > 1.0);
}

#[test]
fn fresnel_declines_what_no_table_serves() {
    let set = OpticsTableSet::for_index_range(1.70, 1.80);
    let cos_t = 0.8f32;
    // Glass index outside the built range.
    assert!(set.fresnel(1.0, 2.4, 0.9, cos_t).is_none());
    assert!(set.fresnel(2.4, 1.0, 0.9, cos_t).is_none());
    // Neither side is air.
    assert!(set.fresnel(1.5, 1.76, 0.9, cos_t).is_none());
    // Air to air, and the wrong way round for the index.
    assert!(set.fresnel(1.0, 1.0, 0.9, cos_t).is_none());
    // A key outside [0, 1] and NaN.
    assert!(set.fresnel(1.0, 1.76, 1.2, cos_t).is_none());
    assert!(set.fresnel(1.0, 1.76, -0.1, cos_t).is_none());
    assert!(set.fresnel(1.0, f32::NAN, 0.5, cos_t).is_none());
    assert!(set.fresnel(1.0, 1.76, f32::NAN, cos_t).is_none());
    // A set built without a Fresnel branch serves nothing.
    assert!(
        OpticsTableSet::exponential_only()
            .fresnel(1.0, 1.76, 0.9, cos_t)
            .is_none()
    );
    // An empty or inverted range builds no branch.
    assert!(OpticsTableSet::for_index_range(1.8, 1.7).entry().is_none());
    assert!(OpticsTableSet::for_index_range(0.9, 1.0).exit().is_none());
}

/// The closed form exactly as the tracer's inline expressions write it.
fn inline_reflection(n1: f32, n2: f32, cos_i: f32, cos_t: f32) -> (f32, f32) {
    let r_s = f32::mul_add(n2, -cos_t, n1 * cos_i) / f32::mul_add(n2, cos_t, n1 * cos_i);
    let r_p = f32::mul_add(n1, -cos_t, n2 * cos_i) / f32::mul_add(n1, cos_t, n2 * cos_i);
    (r_s, r_p)
}

/// The transmission amplitudes exactly as the tracer's inline expressions write them.
fn inline_transmission(n1: f32, n2: f32, cos_i: f32, cos_t: f32) -> (f32, f32) {
    (
        (2.0 * n1 * cos_i) / f32::mul_add(n2, cos_t, n1 * cos_i),
        (2.0 * n1 * cos_i) / f32::mul_add(n1, cos_t, n2 * cos_i),
    )
}

#[test]
fn with_the_tables_off_every_helper_is_bit_identical_to_the_closed_forms() {
    let mut rng = Lcg(99);
    let indices = [1.0f32, 1.33, 1.544, 1.76, 2.417, 3.0];
    let mut checked = 0;
    for &n1 in &indices {
        for &n2 in &indices {
            for _ in 0..400 {
                #[expect(clippy::cast_possible_truncation, reason = "probe rounded to f32")]
                let (cos_i, cos_t) = (rng.unit() as f32, rng.unit() as f32);
                let (r_s, r_p) = inline_reflection(n1, n2, cos_i, cos_t);
                let (t_s, t_p) = inline_transmission(n1, n2, cos_i, cos_t);
                let reflection = reflection_amplitudes(None, n1, n2, cos_i, cos_t);
                assert_eq!(
                    (reflection.0.to_bits(), reflection.1.to_bits()),
                    (r_s.to_bits(), r_p.to_bits())
                );
                let all = fresnel_amplitudes(None, n1, n2, cos_i, cos_t);
                assert_eq!(
                    [all.r_s, all.r_p, all.t_s, all.t_p].map(f32::to_bits),
                    [r_s, r_p, t_s, t_p].map(f32::to_bits)
                );
                let closed = closed_form_reflection(n1, n2, cos_i, cos_t);
                assert_eq!(closed.0.to_bits(), r_s.to_bits());
                assert_eq!(closed.1.to_bits(), r_p.to_bits());
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 36 * 400);
}

#[test]
fn with_the_tables_off_the_exponential_is_the_polynomial_bit_for_bit() {
    let mut rng = Lcg(5);
    for _ in 0..2_000 {
        let mut args = [0.0f32; 8];
        for slot in &mut args {
            #[expect(clippy::cast_possible_truncation, reason = "probe rounded to f32")]
            let value = (-12.0 * rng.unit()) as f32;
            *slot = value;
        }
        assert_eq!(
            beer_lambert_factors(None, args).map(f32::to_bits),
            crate::simd::exp_f32x8(args).map(f32::to_bits)
        );
    }
}

#[test]
fn a_set_that_does_not_serve_an_interface_falls_back_to_the_closed_form() {
    let set = OpticsTableSet::for_index_range(1.70, 1.80);
    // Glass index 2.4 is outside the set: bit-identical to the closed form.
    let (n1, n2, cos_i, cos_t) = (1.0f32, 2.4f32, 0.7f32, 0.9f32);
    let tabled = reflection_amplitudes(Some(&set), n1, n2, cos_i, cos_t);
    let closed = inline_reflection(n1, n2, cos_i, cos_t);
    assert_eq!(
        (tabled.0.to_bits(), tabled.1.to_bits()),
        (closed.0.to_bits(), closed.1.to_bits())
    );
}

#[test]
fn the_process_default_is_off() {
    // Nothing in the test binary sets the default; this must hold for every other test's
    // bit-identity assumptions.
    assert_eq!(default_optics_tables(), OpticsTables::Off);
    assert_eq!(OpticsTables::default(), OpticsTables::Off);
}

#[test]
fn the_gpu_buffer_follows_the_documented_layout() {
    let set = OpticsTableSet::for_material(&builtin("Diamond"));
    let words = pack_gpu_words(&set);
    assert_eq!(words[0], 1.0);
    assert_eq!(words[1], 128.0);
    const {
        assert!(GPU_EXP_BASE + EXP_VALUES <= GPU_HEADER_ENTRY);
        assert!(GPU_HEADER_EXIT + 8 <= GPU_BRANCH_DATA);
    }
    assert_eq!(
        &words[GPU_EXP_BASE..GPU_EXP_BASE + EXP_VALUES],
        set.exp_table().values().as_slice()
    );
    let mut next = GPU_BRANCH_DATA;
    for (header, branch) in [
        (GPU_HEADER_ENTRY, set.entry().expect("entry")),
        (GPU_HEADER_EXIT, set.exit().expect("exit")),
    ] {
        assert_eq!(words[header], 1.0);
        assert_eq!(words[header + 1], branch.n_lo());
        assert_eq!(words[header + 2], branch.n_hi());
        assert_eq!(words[header + 3], branch.inv_dn());
        assert_eq!(words[header + 4], branch.index_cells() as f32);
        assert_eq!(words[header + 5], branch.key_cells() as f32);
        assert_eq!(words[header + 6], next as f32);
        let end = next + branch.values().len();
        assert_eq!(&words[next..end], branch.values());
        next = end;
    }
    assert_eq!(words.len(), next);
    let disabled = pack_disabled_gpu_words();
    assert_eq!(disabled.len(), GPU_BRANCH_DATA);
    assert!(disabled.iter().all(|w| *w == 0.0));
    let without = pack_gpu_words(&OpticsTableSet::exponential_only());
    assert_eq!(without.len(), GPU_BRANCH_DATA);
    assert_eq!(without[GPU_HEADER_ENTRY], 0.0);
    assert_eq!(without[GPU_HEADER_EXIT], 0.0);
}

#[test]
fn the_exp_variant_routes_every_call_and_refuses_a_drifted_unit() {
    let unit = "fn exp_poly(x_in: f32) -> f32 {\n    return x_in;\n}\n\
                fn user(a: f32) -> f32 {\n    return exp_poly(-a) + exp_poly(-2.0 * a);\n}\n";
    let variant = exp_table_variant(unit).expect("a unit with one definition");
    assert!(variant.contains("fn exp_poly(x_in: f32) -> f32 {"));
    assert_eq!(
        variant.matches("exp_poly(").count(),
        2,
        "definition + the fallback call"
    );
    assert!(variant.contains("return ot_exp_of(-a) + ot_exp_of(-2.0 * a);"));
    assert!(variant.contains(&format!("@binding({OPTICS_TABLES_BINDING})")));
    assert!(variant.contains("fn ot_exp_of(x: f32) -> f32 {"));
    // Already a variant, no definition, two definitions.
    assert!(exp_table_variant(&variant).is_none());
    assert!(exp_table_variant("fn other() {}").is_none());
    assert!(exp_table_variant(&format!("{unit}{unit}")).is_none());
}

/// Parses and validates `source` with naga, the front end wgpu uses at pipeline creation.
fn validate(name: &str, source: &str) -> Result<(), String> {
    let module =
        wgsl::parse_str(source).map_err(|e| format!("{name}: {}", e.emit_to_string(source)))?;
    Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .map(|_| ())
        .map_err(|e| format!("{name}: {}", e.emit_to_string(source)))
}

#[test]
fn the_table_kernels_validate() {
    let source = format!(
        "{OPTICS_TABLES_WGSL}
{OPTICS_TABLES_CHECK_WGSL}"
    );
    if let Err(error) = validate("optics tables + check kernels", &source) {
        panic!("{error}");
    }
}

#[test]
fn the_tables_text_holds_no_function_scope_array_and_no_unitialised_var() {
    // WGSL rules of this codebase: no function-local arrays, every `var` initialised.
    for (name, text) in [
        ("optics_tables.wgsl", OPTICS_TABLES_WGSL),
        ("optics_tables_check.wgsl", OPTICS_TABLES_CHECK_WGSL),
    ] {
        for line in text.lines().map(str::trim) {
            if line.starts_with("//") {
                continue;
            }
            if line.starts_with("var ") {
                assert!(line.contains('='), "{name}: uninitialised var: {line}");
                assert!(
                    !line.contains("array<"),
                    "{name}: function-scope array: {line}"
                );
            }
        }
    }
}

#[test]
fn the_variants_of_the_shipped_units_validate() {
    let units: [(&str, &str); 6] = [
        (
            "spectral_transport.generated.wgsl",
            include_str!(concat!(
                env!("OUT_DIR"),
                "/spectral_transport.generated.wgsl"
            )),
        ),
        (
            "wavefront_transport.generated.wgsl",
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wavefront_transport.generated.wgsl"
            )),
        ),
        (
            "transport_functions.generated.wgsl",
            include_str!(concat!(
                env!("OUT_DIR"),
                "/transport_functions.generated.wgsl"
            )),
        ),
        (
            "ew_generate.generated.wgsl",
            include_str!(concat!(env!("OUT_DIR"), "/ew_generate.generated.wgsl")),
        ),
        (
            "ew_scatter.generated.wgsl",
            include_str!(concat!(env!("OUT_DIR"), "/ew_scatter.generated.wgsl")),
        ),
        (
            "ew_surface.generated.wgsl",
            include_str!(concat!(env!("OUT_DIR"), "/ew_surface.generated.wgsl")),
        ),
    ];
    for (name, source) in units {
        let variant = exp_table_variant(source)
            .unwrap_or_else(|| panic!("{name} does not define exp_poly exactly once"));
        assert!(
            variant.contains("ot_exp_of("),
            "{name}: the variant routes no call"
        );
        if let Err(error) = validate(name, &variant) {
            panic!("{error}");
        }
        // The units with the bounce code also take the Fresnel sites; the standalone function
        // unit has no reflect/transmit decision and so declines.
        let has_bounce = name != "transport_functions.generated.wgsl";
        let full = optics_tables_variant(source);
        assert_eq!(full.is_some(), has_bounce, "{name}");
        if let Some(full) = full {
            assert!(
                full.contains("ot_amplitudes("),
                "{name}: no Fresnel site routed"
            );
            if let Err(error) = validate(name, &full) {
                panic!("{error}");
            }
        }
        // The shipped text is untouched: the variant is built at run time only.
        assert!(
            !source.contains("ot_exp_of("),
            "{name}: shipped unit changed"
        );
        assert!(
            !source.contains("optics_tables"),
            "{name}: shipped unit changed"
        );
    }
}

#[test]
fn the_full_variant_routes_the_fresnel_groups_and_refuses_a_partial_unit() {
    let exp = "fn exp_poly(x_in: f32) -> f32 {\n    return x_in;\n}\n";
    let decision = "let r_s = fma(n2_decision, -cos_t, n1 * cos_i) / fma(n2_decision, cos_t, n1 * cos_i);\n\
                    let r_p = fma(n1, -cos_t, n2_decision * cos_i) / fma(n1, cos_t, n2_decision * cos_i);\n";
    let reflect = "let r_s_k = fma(n2k, -cos_t_k, n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);\n\
                   let r_p_k = fma(n1k, -cos_t_k, n2k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);\n";
    let transmit = "let t_s_k = (2.0 * n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);\n\
                    let t_p_k = (2.0 * n1k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);\n";
    let unit = format!("{exp}{decision}{reflect}{transmit}");
    let variant = optics_tables_variant(&unit).expect("a complete unit");
    for routed in [
        "let ot_amp = ot_amplitudes(n1, n2_decision, cos_i, cos_t);",
        "let r_p = ot_amp.r_p;",
        "let ot_amp_k = ot_amplitudes(n1k, n2k, cos_i, cos_t_k);",
        "let r_p_k = ot_amp_k.r_p;",
        "let ot_amp_t_k = ot_amplitudes(n1k, n2k, cos_i, cos_t_k);",
        "let t_p_k = ot_amp_t_k.t_p;",
    ] {
        assert!(variant.contains(routed), "missing: {routed}");
    }
    assert!(
        !variant.contains("let r_s = fma("),
        "the shipped statement is gone"
    );
    // A unit missing a group, one with unequal pair counts, and an already routed one.
    assert!(optics_tables_variant(&format!("{exp}{decision}{reflect}")).is_none());
    assert!(
        optics_tables_variant(&format!("{unit}{}", transmit.lines().next().unwrap_or("")))
            .is_none()
    );
    assert!(optics_tables_variant(&variant).is_none());
    // The exponential-only variant needs no Fresnel site.
    assert!(exp_table_variant(exp).is_some());
}
