//! Hardware twin tests of the optics lookup tables: the WGSL lookups against their CPU twins.
//!
//! Skipped without an adapter (`PLEOCHROMA_REQUIRE_GPU=1` makes the absence a failure). The CPU
//! side is the real [`ExpTable::exp`] and [`OpticsTableSet::fresnel`]; the GPU side runs
//! `ot_exp_table` and `ot_fresnel` of `transport_physics/optics_tables.wgsl` through the test
//! kernels of `transport_functions/optics_tables_check.wgsl`, reading the very buffer
//! [`pack_gpu_words`] produces. The twin tolerance is the one the other twins use: no bit
//! exactness (WGSL may contract to fused multiply-adds), a few units in the last place.

use super::{
    EXP_TABLE_MIN_ARG, FresnelAmplitudes, OPTICS_TABLES_BINDING, OPTICS_TABLES_WGSL,
    OpticsTableSet, fresnel_amplitudes, gpu::OPTICS_TABLES_CHECK_WGSL, material_index_range,
    pack_disabled_gpu_words, pack_gpu_words,
};
use crate::{
    optics::materials::GemMaterial,
    renderer::gpu::{GpuContext, compute, test_support::require_gpu},
};

/// Relative tolerance of a GPU result against its CPU twin (about eighty ULPs of `f32`).
///
/// WGSL may contract the argument reduction of the exponential into a fused multiply-add, which
/// moves the fraction by up to half an ULP of the (up to 126) exponent: 2.6e-6 relative.
const TWIN_REL_TOL: f32 = 1e-5;
/// Absolute floor under which a difference is rounding noise (values near zero).
const TWIN_ABS_FLOOR: f32 = 1e-6;

/// Runs `entry` of the test kernels over `cases`, with `tables` at binding 170, and returns the
/// `out_len` result floats.
fn run_kernel(
    ctx: &GpuContext,
    entry: &str,
    tables: &[f32],
    bindings: (u32, u32),
    cases: &[f32],
    out_len: usize,
) -> Vec<f32> {
    let source = format!("{OPTICS_TABLES_WGSL}\n{OPTICS_TABLES_CHECK_WGSL}");
    let table_buffer = compute::upload(
        &ctx.device,
        "optics tables",
        tables,
        wgpu::BufferUsages::STORAGE,
    );
    let case_buffer = compute::upload(&ctx.device, entry, cases, wgpu::BufferUsages::STORAGE);
    let out_buffer = compute::zeroed_buffer::<f32>(
        &ctx.device,
        entry,
        out_len,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(&ctx.device, entry, &source, entry);
    let bind_group = compute::bind_buffers(
        &ctx.device,
        entry,
        &pipeline,
        &[
            (OPTICS_TABLES_BINDING, &table_buffer),
            (bindings.0, &case_buffer),
            (bindings.1, &out_buffer),
        ],
    );
    let invocations = if bindings.0 == 171 {
        cases.len()
    } else {
        cases.len() / 4
    };
    let groups = u32::try_from(invocations.div_ceil(64)).expect("the case count fits in u32");
    compute::dispatch_and_wait(
        &ctx.device,
        &ctx.queue,
        &pipeline,
        &bind_group,
        (groups, 1, 1),
    );
    compute::readback::<f32>(&ctx.device, &ctx.queue, &out_buffer, out_len)
}

fn close(cpu: f32, gpu: f32) -> bool {
    (cpu - gpu).abs() <= TWIN_ABS_FLOOR.max(TWIN_REL_TOL * cpu.abs())
}

#[test]
fn the_gpu_exponential_matches_the_cpu_table() {
    let Some(ctx) = require_gpu() else {
        return;
    };
    let table = OpticsTableSet::exponential_only();
    let words = pack_gpu_words(&table);
    let cases: Vec<f32> = (0..4_000u32)
        .map(|k| EXP_TABLE_MIN_ARG * (k as f32 / 3_999.0).powf(1.7))
        .collect();
    let gpu = run_kernel(&ctx, "ot_exp_main", &words, (171, 172), &cases, cases.len());
    for (a, got) in cases.iter().zip(&gpu) {
        let want = table.exp_table().exp(*a);
        assert!(close(want, *got), "a = {a}: cpu {want:e}, gpu {got:e}");
    }
}

#[test]
fn the_gpu_fresnel_lookup_matches_the_cpu_table() {
    let Some(ctx) = require_gpu() else {
        return;
    };
    for name in ["Diamond", "Zircon", "Tanzanite"] {
        let material = GemMaterial::by_name(name).expect("a built-in material");
        let (n_lo, n_hi) = material_index_range(&material).expect("a finite index range");
        let set = OpticsTableSet::for_material(&material);
        let words = pack_gpu_words(&set);
        let cases = fresnel_cases(n_lo, n_hi);
        let gpu = run_kernel(
            &ctx,
            "ot_fresnel_main",
            &words,
            (173, 174),
            &cases,
            2 * cases.len(),
        );
        let mut served = 0usize;
        for (idx, case) in cases.as_chunks::<4>().0.iter().enumerate() {
            let &[n1, n2, cos_i, cos_t] = case;
            let tabled = set.fresnel(n1, n2, cos_i, cos_t);
            assert_eq!(
                gpu[8 * idx] == 1.0,
                tabled.is_some(),
                "{name}: served flag differs for {case:?}"
            );
            served += usize::from(tabled.is_some());
            // The amplitudes: the table where it serves, else the closed forms, on both sides.
            let cpu = fresnel_amplitudes(Some(&set), n1, n2, cos_i, cos_t);
            let got = [
                gpu[8 * idx + 1],
                gpu[8 * idx + 2],
                gpu[8 * idx + 4],
                gpu[8 * idx + 5],
            ];
            let want = [cpu.r_s, cpu.r_p, cpu.t_s, cpu.t_p];
            for (slot, (w, g)) in want.iter().zip(&got).enumerate() {
                assert!(
                    close(*w, *g),
                    "{name}: slot {slot} for {case:?}: cpu {w:e}, gpu {g:e}"
                );
            }
        }
        assert!(
            served > cases.as_chunks::<4>().0.len() / 2,
            "{name}: too few served cases ({served})"
        );
    }
}

#[test]
fn a_disabled_buffer_serves_nothing_on_the_gpu() {
    let Some(ctx) = require_gpu() else {
        return;
    };
    let words = pack_disabled_gpu_words();
    let cases = [1.0f32, 1.76, 0.8, 0.9, 1.76, 1.0, 0.9, 0.8];
    let gpu = run_kernel(&ctx, "ot_fresnel_main", &words, (173, 174), &cases, 16);
    assert_eq!(gpu[0], 0.0, "entry case");
    assert_eq!(gpu[8], 0.0, "exit case");
    // Not served: the amplitudes are the closed forms.
    for (idx, case) in cases.as_chunks::<4>().0.iter().enumerate() {
        let want = FresnelAmplitudes::closed_form(case[0], case[1], case[2], case[3]);
        let got = [
            gpu[8 * idx + 1],
            gpu[8 * idx + 2],
            gpu[8 * idx + 4],
            gpu[8 * idx + 5],
        ];
        for (w, g) in [want.r_s, want.r_p, want.t_s, want.t_p].iter().zip(&got) {
            assert!(close(*w, *g), "case {idx}: cpu {w:e}, gpu {g:e}");
        }
    }
}

/// Cases `(n1, n2, cos_i, cos_t)` over the index range: entries and exits that a table serves,
/// plus interfaces it must decline (no air side, index outside the range, key outside `[0, 1]`).
#[expect(
    clippy::cast_possible_truncation,
    reason = "the probe values are rounded to f32, the precision of the kernels"
)]
fn fresnel_cases(n_lo: f32, n_hi: f32) -> Vec<f32> {
    let mut cases = Vec::new();
    for j in 0..=12u32 {
        let n = (n_hi - n_lo).mul_add(j as f32 / 12.0, n_lo);
        for i in 0..=60u32 {
            let u = (i as f32 + 0.37) / 61.0;
            let other = (1.0
                - f64::from(u).mul_add(-f64::from(u), 1.0) / (f64::from(n) * f64::from(n)))
            .max(0.0)
            .sqrt() as f32;
            cases.extend_from_slice(&[1.0, n, u, other]);
            cases.extend_from_slice(&[n, 1.0, other, u]);
        }
    }
    // Declined: both sides glass, outside the range, key out of range.
    cases.extend_from_slice(&[1.5, n_hi, 0.5, 0.6]);
    cases.extend_from_slice(&[1.0, n_hi + 1.0, 0.5, 0.6]);
    cases.extend_from_slice(&[n_lo + 1.0, 1.0, 0.5, 0.6]);
    cases.extend_from_slice(&[1.0, n_lo, 1.5, 0.6]);
    cases.extend_from_slice(&[n_lo, 1.0, 0.5, 1.5]);
    cases
}
