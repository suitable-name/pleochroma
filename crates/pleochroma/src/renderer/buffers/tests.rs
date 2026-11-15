//! CPU-side pins for the WGSL layout invariants documented across this module tree, plus
//! the material/facet-finish encoding functions' mapping behaviour.

use core::mem::offset_of;

use super::{
    CameraUniform, DispersionParams, GpuAbsorptionBand, GpuGemMaterial, GpuTransportParams,
    GpuWavefrontParams, band_shape, encode_facet_finishes, facet_finish,
    material::{empty_bands, encode_bands},
};
use crate::optics::{absorption::AbsorptionBand, raytracer::FacetFinish};

/// `bytemuck::Pod` rules out uninitialized padding, not WGSL offset rules; the `offset_of!`
/// assertions in each sibling module check those. This pins the documented sizes as a `#[test]`.
#[test]
fn struct_sizes_match_documented_wgsl_layout() {
    assert_eq!(size_of::<CameraUniform>(), 112);
    assert_eq!(size_of::<DispersionParams>(), 112);
    assert_eq!(size_of::<GpuAbsorptionBand>(), 16);
    // The zone table, the biaxial curves (80) and the inclusion block (304) are appended
    // (see `renderer::buffers::zoning`, `material` and `inclusions`).
    assert_eq!(size_of::<GpuGemMaterial>(), 592 + 2128 + 80 + 304);
    assert_eq!(size_of::<GpuTransportParams>(), 128);
    assert_eq!(size_of::<GpuWavefrontParams>(), 16);
}

/// The transport uniform's glare slot sits at offset 72, defaults to the neutral `1.0`
/// and clamps what a caller sets.
#[test]
fn transport_params_surface_glare_slot_defaults_to_one_and_clamps() {
    assert_eq!(offset_of!(GpuTransportParams, surface_glare), 72);
    let base = GpuTransportParams::new(1, 1, 0, 1, 0.0, 6500.0, 1.0, 1.0, 0.0, 0.0, [1.0; 3]);
    assert_eq!(base.surface_glare.to_bits(), 1.0f32.to_bits());
    assert_eq!(
        base.with_surface_glare(0.4).surface_glare.to_bits(),
        0.4f32.to_bits()
    );
    assert_eq!(
        base.with_surface_glare(-1.0).surface_glare.to_bits(),
        0.0f32.to_bits()
    );
    assert_eq!(
        base.with_surface_glare(9.0).surface_glare.to_bits(),
        1.0f32.to_bits()
    );
}

/// The head-shadow cone slots sit at 76 / 80 and default to the 16 degree literals.
#[test]
fn transport_params_head_shadow_slots_default_to_the_16_degree_cone() {
    assert_eq!(offset_of!(GpuTransportParams, head_shadow_outer_cos), 76);
    assert_eq!(offset_of!(GpuTransportParams, head_shadow_inner_cos), 80);
    let base = GpuTransportParams::new(1, 1, 0, 1, 0.0, 6500.0, 1.0, 1.0, 0.0, 0.0, [1.0; 3]);
    assert_eq!(
        base.head_shadow_outer_cos.to_bits(),
        0.951_056_5f32.to_bits()
    );
    assert_eq!(
        base.head_shadow_inner_cos.to_bits(),
        0.970_295_7f32.to_bits()
    );
    let wide = base.with_head_shadow([0.5, 0.75]);
    assert_eq!(wide.head_shadow_outer_cos.to_bits(), 0.5f32.to_bits());
    assert_eq!(wide.head_shadow_inner_cos.to_bits(), 0.75f32.to_bits());
}

/// The four light-tent slots sit at 96..112 and default to the light tent's own values
/// (the identity of the tent formula); `with_tent` / `with_tent_of` carry a preset's.
#[test]
fn transport_params_tent_slots_default_to_the_light_tent() {
    use crate::optics::raytracer::{LightingPreset, TentParams};

    assert_eq!(offset_of!(GpuTransportParams, tent_walls), 96);
    assert_eq!(offset_of!(GpuTransportParams, tent_cards), 100);
    assert_eq!(offset_of!(GpuTransportParams, tent_spark), 104);
    assert_eq!(offset_of!(GpuTransportParams, tent_ground), 108);
    let base = GpuTransportParams::new(1, 1, 0, 1, 0.0, 6500.0, 1.0, 1.0, 0.0, 0.0, [1.0; 3]);
    assert_eq!(base.tent_walls.to_bits(), 1.0f32.to_bits());
    assert_eq!(base.tent_cards.to_bits(), 1.0f32.to_bits());
    assert_eq!(base.tent_spark.to_bits(), 1.0f32.to_bits());
    assert_eq!(base.tent_ground.to_bits(), 0.02f32.to_bits());
    assert_eq!(offset_of!(GpuTransportParams, tent_flat), 84);
    assert_eq!(base.tent_flat.to_bits(), 0.0f32.to_bits());
    let tray = base.with_tent_of(LightingPreset::WhiteTray.studio(1.0, 0.0, 0.0));
    let expected = LightingPreset::WhiteTray.params().tent;
    assert_eq!(tray.tent_walls.to_bits(), expected.walls.to_bits());
    assert_eq!(tray.tent_ground.to_bits(), expected.ground.to_bits());
    assert_eq!(tray.tent_flat.to_bits(), expected.flat.to_bits());
    // The light tent's own preset keeps the identity.
    let tent = base.with_tent_of(LightingPreset::LightTent.studio(1.0, 0.0, 0.0));
    assert_eq!(
        [
            tent.tent_walls,
            tent.tent_cards,
            tent.tent_spark,
            tent.tent_ground
        ],
        TentParams::DEFAULT.to_array()
    );
}

/// The HDR exposure slot sits at offset 92. It defaults to the `1.0` the shader skips, carries
/// an HDR source's exposure, and stays `1.0` for the analytic rig (whose exposure is
/// `studio_exposure`).
#[test]
fn transport_params_hdr_exposure_slot_defaults_to_one_and_follows_the_map() {
    use crate::{
        optics::raytracer::{EnvironmentSource, LightingPreset},
        renderer::env_map::EnvironmentMap,
    };

    assert_eq!(offset_of!(GpuTransportParams, hdr_exposure), 92);
    let base = GpuTransportParams::new(1, 1, 0, 2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, [1.0; 3]);
    assert_eq!(base.hdr_exposure.to_bits(), 1.0f32.to_bits());
    let map = EnvironmentMap::uniform(4, 2, [1.0, 1.0, 1.0]);
    let hdr = base.with_hdr_of(EnvironmentSource::hdr_at(&map, 2.5));
    assert_eq!(hdr.hdr_exposure.to_bits(), 2.5f32.to_bits());
    let unit = hdr.with_hdr_of(EnvironmentSource::hdr(&map));
    assert_eq!(unit.hdr_exposure.to_bits(), 1.0f32.to_bits());
    let studio = hdr.with_hdr_of(LightingPreset::Daylight.studio(3.0, 0.0, 0.0));
    assert_eq!(studio.hdr_exposure.to_bits(), 1.0f32.to_bits());
}

/// The HDR rotation row sits at 112..128, defaults to the identity `(1, 0)` with zero pads,
/// and `with_hdr_of` carries an HDR source's rotation (and resets it for the analytic rig).
#[test]
fn transport_params_hdr_rotation_row_defaults_to_identity_and_follows_the_map() {
    use crate::{
        optics::raytracer::{EnvironmentSource, LightingPreset},
        renderer::env_map::{EnvRotation, EnvironmentMap},
    };

    assert_eq!(offset_of!(GpuTransportParams, hdr_rot_cos), 112);
    assert_eq!(offset_of!(GpuTransportParams, hdr_rot_sin), 116);
    let base = GpuTransportParams::new(1, 1, 0, 2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, [1.0; 3]);
    assert_eq!(base.hdr_rot_cos.to_bits(), 1.0f32.to_bits());
    assert_eq!(base.hdr_rot_sin.to_bits(), 0.0f32.to_bits());
    let bytes = bytemuck::bytes_of(&base);
    assert!(bytes[120..128].iter().all(|&b| b == 0));
    let map = EnvironmentMap::uniform(4, 2, [1.0, 1.0, 1.0]);
    let turn = EnvRotation::from_yaw(0.7);
    let turned = base.with_hdr_of(EnvironmentSource::hdr_at(&map, 2.0).with_hdr_rotation(turn));
    assert_eq!(turned.hdr_rot_cos.to_bits(), turn.cos_sin()[0].to_bits());
    assert_eq!(turned.hdr_rot_sin.to_bits(), turn.cos_sin()[1].to_bits());
    assert_eq!(turned.hdr_exposure.to_bits(), 2.0f32.to_bits());
    let manual = base.with_hdr_rotation(turn);
    assert_eq!(manual.hdr_rot_cos.to_bits(), turned.hdr_rot_cos.to_bits());
    let unit = turned.with_hdr_of(EnvironmentSource::hdr(&map));
    assert_eq!(unit.hdr_rot_cos.to_bits(), 1.0f32.to_bits());
    assert_eq!(unit.hdr_rot_sin.to_bits(), 0.0f32.to_bits());
    let studio = turned.with_hdr_of(LightingPreset::Daylight.studio(3.0, 0.0, 0.0));
    assert_eq!(studio.hdr_rot_cos.to_bits(), 1.0f32.to_bits());
    assert_eq!(studio.hdr_rot_sin.to_bits(), 0.0f32.to_bits());
}

/// `offset_of!` pins for `GpuAbsorptionBand` and the band arrays of `GpuGemMaterial`. The GPU
/// side is `renderer::gpu::layout_check`; this is the CPU half, run by every `cargo test`.
#[test]
fn gpu_absorption_band_offsets_match_documented_wgsl_layout() {
    assert_eq!(offset_of!(GpuAbsorptionBand, center_nm), 0);
    assert_eq!(offset_of!(GpuAbsorptionBand, width_nm), 4);
    assert_eq!(offset_of!(GpuAbsorptionBand, peak), 8);
    assert_eq!(offset_of!(GpuAbsorptionBand, shape), 12);
}

#[test]
fn gpu_gem_material_band_array_offsets_match_documented_wgsl_layout() {
    assert_eq!(offset_of!(DispersionParams, a_axis_and_flag), 96);
    assert_eq!(offset_of!(GpuGemMaterial, o_ray_bands), 132);
    assert_eq!(offset_of!(GpuGemMaterial, e_ray_bands), 260);
    assert_eq!(offset_of!(GpuGemMaterial, scattering_sigma_s), 388);
    assert_eq!(offset_of!(GpuGemMaterial, beta_ray_bands), 408);
    assert_eq!(offset_of!(GpuGemMaterial, absorption_path_scale), 536);
}

/// `material::encode_bands` maps each [`crate::optics::absorption::BandShape`] variant to the
/// matching [`band_shape`] discriminant and leaves the other band fields alone. A shape that fell
/// back to the wavelength default would make energy-domain bands render differently on the GPU.
#[test]
fn encode_bands_maps_shape_correctly() {
    let bands = vec![
        AbsorptionBand::new(550.0, 20.0, 2.0),
        AbsorptionBand::energy(620.0, 200.0, 1.5),
    ];
    let (encoded, count) = encode_bands(&bands);
    assert_eq!(count, 2);
    assert_eq!(encoded[0].shape, band_shape::GAUSSIAN_WAVELENGTH);
    assert_eq!(encoded[1].shape, band_shape::GAUSSIAN_ENERGY);
    // Untouched slots keep the wavelength-domain default.
    assert_eq!(encoded[2].shape, band_shape::GAUSSIAN_WAVELENGTH);

    let (empty, empty_count) = empty_bands();
    assert_eq!(empty_count, 0);
    assert!(
        empty
            .iter()
            .all(|b| b.shape == band_shape::GAUSSIAN_WAVELENGTH)
    );
}

/// [`encode_facet_finishes`]'s default-fallback semantics must match
/// `trace_spectral_ray_with_finish`'s lookup exactly: an empty slice, or one shorter
/// than `num_planes`, defaults every uncovered index to `facet_finish::POLISHED`.
#[test]
fn encode_facet_finishes_defaults_to_polished() {
    assert_eq!(encode_facet_finishes(&[], 4), vec![0, 0, 0, 0]);

    let finishes = vec![FacetFinish::Frosted, FacetFinish::Polished];
    assert_eq!(
        encode_facet_finishes(&finishes, 4),
        vec![
            facet_finish::FROSTED,
            facet_finish::POLISHED,
            facet_finish::POLISHED,
            facet_finish::POLISHED,
        ]
    );
}

/// Exit collection is honoured only where the kernels and the CPU tracer run it, and its bit is
/// independent of the debug-write bit.
#[test]
fn the_collect_bit_is_set_only_where_exit_collection_runs() {
    use super::{studio_model, transport_env_mode};
    use crate::optics::raytracer::Estimator;
    let params = |env_mode: u32| {
        GpuTransportParams::new(1, 8, 0, env_mode, 0.0, 6500.0, 1.0, 1.0, 0.0, 0.0, [1.0; 3])
    };
    let rig = params(transport_env_mode::STUDIO_RIG);
    assert_eq!(rig.estimator(), Estimator::Branching);
    let collect = rig.with_estimator(Estimator::ExitCollection, true);
    assert_eq!(collect.estimator(), Estimator::ExitCollection);
    assert_eq!(GpuTransportParams::COLLECT_BIT, 4);
    assert_eq!(
        collect.write_debug_buffers,
        GpuTransportParams::DEBUG_WRITES_BIT | GpuTransportParams::COLLECT_BIT
    );
    // The furnace has no light sampling either.
    assert_eq!(
        params(transport_env_mode::UNIFORM_FURNACE)
            .with_estimator(Estimator::ExitCollection, true)
            .estimator(),
        Estimator::ExitCollection
    );
    // A rough surface, a stone with tools, the sun, an HDR map, and the estimator taken back.
    for refused in [
        rig.with_estimator(Estimator::ExitCollection, false),
        rig.with_tool_count(1)
            .with_estimator(Estimator::ExitCollection, true),
        rig.with_studio_model(studio_model::DAYLIGHT_SUN)
            .with_estimator(Estimator::ExitCollection, true),
        params(transport_env_mode::HDR_MAP).with_estimator(Estimator::ExitCollection, true),
        collect.with_estimator(Estimator::Branching, true),
    ] {
        assert_eq!(refused.estimator(), Estimator::Branching);
        assert_eq!(
            refused.write_debug_buffers & GpuTransportParams::COLLECT_BIT,
            0
        );
    }
    assert_eq!(
        collect.with_debug_buffers_disabled().write_debug_buffers,
        GpuTransportParams::COLLECT_BIT
    );
}
