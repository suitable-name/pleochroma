//! Spectral-space debug comparison, so that a CMF bug cannot masquerade as a transport bug.
//!
//! See [`run_spectral_debug`] for what this does and does not verify.

use crate::{
    optics::raytracer::{
        LightingPreset, apply_von_kries_white_balance, environment::environment_white_balance,
        illuminant_temperature_k, integrate_channels_to_xyz_families,
    },
    renderer::{
        buffers::{GpuGemMaterial, GpuTransportParams, transport_env_mode},
        gpu::{environment_check::build_hdr_test_map, frame},
    },
};
use glam::Vec3;

use super::{
    MaterialForDispatch, SceneBuffersForDispatch, TransportDispatch, all_polished_finishes,
    camera_params_for, dispatch_transport, dispatch_transport_for_class, round_brilliant_planes,
    test_camera, tier3_material,
};

/// Outcome of the spectral debug check.
#[derive(Debug, Clone)]
pub struct SpectralDebugResult {
    /// Total number of cases run.
    pub total_cases: usize,
    /// Largest ULP distance among the comparisons over budget; 0 when none is. The
    /// comparison re-integrates the GPU's per-channel `(radiance, lambdas, path_pdf)` debug
    /// output on the CPU and is checked against the GPU's own `out_xyz`.
    pub max_self_consistency_ulp: u32,
    /// Number of comparisons that exceeded the budget.
    pub over_budget_count: usize,
}

const SPECTRAL_DEBUG_ULP_BUDGET: u32 = 64;
const SPECTRAL_DEBUG_ABS_FLOOR: f32 = 1e-5;

/// Isolates the integration step from everything upstream, using the GPU's own per-channel
/// debug output (no reimplementation of the bounce loop).
///
/// # Scope
///
/// The CPU tracer has no hook for its internal per-channel `radiance`/`lambdas`/`path_pdf`,
/// so there is no independent CPU per-channel reference. What this does check: the GPU's
/// pre-integration `(radiance, lambdas, path_pdf)`, re-integrated on the CPU through
/// `integrate_channels_to_xyz_families`, must match the GPU's own `out_xyz`. Agreement means
/// any CPU/GPU divergence lies in the transport upstream; disagreement means the integration
/// step is ported wrongly. The per-channel radiance values are not verified here; they are
/// covered only indirectly, by the furnace anchor and Tier 3's final-XYZ comparison.
///
/// Runs twice: against `EnvironmentSource::Studio` (white balance applied, `env_mode ==
/// STUDIO_RIG`) and against `EnvironmentSource::HdrMap` (no white balance on either side,
/// `env_mode == HDR_MAP`), using `environment_check::build_hdr_test_map`. Both runs are pooled
/// into one [`SpectralDebugResult`], since the same integration step is isolated either way.
#[must_use]
pub fn run_spectral_debug(ctx: &crate::renderer::gpu::GpuContext) -> SpectralDebugResult {
    let mut max_ulp = 0u32;
    let mut over_budget = 0usize;
    let mut total_cases = 0usize;

    let studio_total = run_spectral_debug_studio_case(ctx, &mut max_ulp, &mut over_budget);
    total_cases += studio_total;
    let hdr_total = run_spectral_debug_hdr_case(ctx, &mut max_ulp, &mut over_budget);
    total_cases += hdr_total;

    SpectralDebugResult {
        total_cases,
        max_self_consistency_ulp: max_ulp,
        over_budget_count: over_budget,
    }
}

/// The `EnvironmentSource::Studio` half of [`run_spectral_debug`] -- white balance
/// applied on both the GPU kernel and the CPU recombination, `env_mode ==
/// STUDIO_RIG`. Returns the number of (pixel, sample) tuples it dispatched, folding its
/// own comparisons into `max_ulp`/`over_budget`.
fn run_spectral_debug_studio_case(
    ctx: &crate::renderer::gpu::GpuContext,
    max_ulp: &mut u32,
    over_budget: &mut usize,
) -> usize {
    let camera = test_camera();
    let (width, height, samples) = (16u32, 16u32, 8u32);
    let planes = round_brilliant_planes();
    let material = tier3_material();
    let gpu_material = GpuGemMaterial::encode(&material);
    let temp_k = illuminant_temperature_k(LightingPreset::Daylight);
    // The production scale (identity for the D65 Daylight rig), exactly as the frame
    // path derives it from the studio environment.
    let wb = environment_white_balance(LightingPreset::Daylight.studio(1.0, 0.0, 0.0));
    let camera_params = camera_params_for(&camera, width, height, samples);
    let params = GpuTransportParams::new(
        width * height,
        10,
        0,
        transport_env_mode::STUDIO_RIG,
        0.0,
        temp_k,
        1.0,
        1.0,
        0.0,
        0.0,
        wb.to_array(),
    );
    let total = (width * height * samples) as usize;
    let finishes = all_polished_finishes(planes.len());
    let dispatch = dispatch_transport(
        ctx,
        &camera_params,
        &params,
        &gpu_material,
        &planes,
        &finishes,
        total,
    );
    accumulate_spectral_debug_case(&dispatch, total, Some(wb), max_ulp, over_budget);
    total
}

/// The `EnvironmentSource::HdrMap` half of [`run_spectral_debug`] -- NO
/// white balance on either side (`env_mode == HDR_MAP` skips it in the kernel the same
/// way `trace_spectral_ray_inner`'s `EnvironmentSource::HdrMap { .. } => xyz` no-op does on
/// the CPU), so the recombination below applies none either. Otherwise identical in
/// shape to [`run_spectral_debug_studio_case`]; see that function's doc comment.
fn run_spectral_debug_hdr_case(
    ctx: &crate::renderer::gpu::GpuContext,
    max_ulp: &mut u32,
    over_budget: &mut usize,
) -> usize {
    let camera = test_camera();
    let (width, height, samples) = (16u32, 16u32, 8u32);
    let planes = round_brilliant_planes();
    let material = tier3_material();
    let gpu_material = GpuGemMaterial::encode(&material);
    let camera_params = camera_params_for(&camera, width, height, samples);
    // The `Studio`-only fields (`studio_*`, `white_balance`) are never read once
    // `env_mode == HDR_MAP`, so they are placeholders, as in the other HDR map dispatches.
    let params = GpuTransportParams::new(
        width * height,
        10,
        0,
        transport_env_mode::HDR_MAP,
        0.0,
        0.0,
        1.0,
        1.0,
        0.0,
        0.0,
        Vec3::ONE.to_array(),
    );
    let total = (width * height * samples) as usize;
    let finishes = all_polished_finishes(planes.len());
    let map = build_hdr_test_map();
    let hdr_env = crate::renderer::env_map_gpu::HdrEnvGpuData::upload(&ctx.device, &map);
    let dispatch = dispatch_transport_for_class(
        ctx,
        &camera_params,
        &params,
        MaterialForDispatch {
            encoded: &gpu_material,
            class: frame::material_class::GENERIC,
        },
        SceneBuffersForDispatch {
            planes: &planes,
            facet_finishes: &finishes,
            hdr_env: Some(&hdr_env),
            tray_shadow: None,
            tray_mirror: None,
        },
        total,
    );
    accumulate_spectral_debug_case(&dispatch, total, None, max_ulp, over_budget);
    total
}

/// Shared per-(pixel, sample) comparison loop for both [`run_spectral_debug_studio_case`]
/// and [`run_spectral_debug_hdr_case`] -- recombines the GPU's own pre-integration debug
/// output through the real CPU integration (plus white balance, when `wb` is `Some`) and
/// records each channel's ULP distance against the GPU's own `out_xyz`.
fn accumulate_spectral_debug_case(
    dispatch: &TransportDispatch,
    total: usize,
    wb: Option<Vec3>,
    max_ulp: &mut u32,
    over_budget: &mut usize,
) {
    for idx in 0..total {
        let mut radiance = [0.0f32; 8];
        let mut lambdas = [0.0f32; 8];
        let mut path_pdf = [0.0f32; 8];
        let mut compat = [0u8; 8];
        radiance.copy_from_slice(&dispatch.radiance[idx * 8..idx * 8 + 8]);
        lambdas.copy_from_slice(&dispatch.lambdas[idx * 8..idx * 8 + 8]);
        path_pdf.copy_from_slice(&dispatch.path_pdf[idx * 8..idx * 8 + 8]);
        for (c, &raw) in compat
            .iter_mut()
            .zip(&dispatch.compat[idx * 8..idx * 8 + 8])
        {
            *c = u8::try_from(raw).expect("the kernel's compat flags are small bit sets");
        }

        // `wb` is the kernel's white balance, applied as a Bradford (von Kries) transform so
        // the recombination matches `out_xyz`; the `HdrMap` case passes `None`, as the kernel
        // skips it there.
        //
        // The kernel integrates with `integrate_channels_to_xyz_families` (per-channel family
        // weights), so the recombination uses it too. `dispatch.compat` is the kernel's own
        // `compat[]` mask. The clamp mirrors `trace_spectral_ray_inner`: a narrow sample whose
        // Bradford round trip dips below zero would otherwise compare `-4.9e-4` against `0`.
        let integrated =
            integrate_channels_to_xyz_families(&radiance, &lambdas, &path_pdf, 0, compat);
        let recombined = wb
            .map_or(integrated, |wb| {
                apply_von_kries_white_balance(integrated, wb)
            })
            .max(Vec3::ZERO);
        let gpu_xyz = Vec3::new(
            dispatch.xyz[idx * 3],
            dispatch.xyz[idx * 3 + 1],
            dispatch.xyz[idx * 3 + 2],
        );

        for (cpu, gpu) in [
            (recombined.x, gpu_xyz.x),
            (recombined.y, gpu_xyz.y),
            (recombined.z, gpu_xyz.z),
        ] {
            let ulp = crate::renderer::gpu::ulp::ulp_distance(cpu, gpu);
            if !crate::renderer::gpu::ulp::within_tolerance(
                cpu,
                gpu,
                SPECTRAL_DEBUG_ULP_BUDGET,
                SPECTRAL_DEBUG_ABS_FLOOR,
            ) {
                *over_budget += 1;
                *max_ulp = (*max_ulp).max(ulp);
            }
        }
    }
}

impl SpectralDebugResult {
    /// Whether every compared value stayed within its budget.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.over_budget_count == 0
    }
}
