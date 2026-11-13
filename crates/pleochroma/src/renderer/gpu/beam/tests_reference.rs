//! Tests of the WGSL reference variant.
//!
//! They cover the anchors and their counts, drift declines, naga validity of every variant unit,
//! and the untouched shipped sources. Two hardware tests (the `branch_step` pin and reference =
//! production on a constant-index material) skip without an adapter (`PLEOCHROMA_REQUIRE_GPU=1`
//! turns the skip into a failure).

use glam::Vec3;
use naga::{
    front::wgsl,
    valid::{Capabilities, ValidationFlags, Validator},
};

use super::reference_source::{
    BRANCH_SIG_SRC, RAY_SIG_BINDING, REFERENCE_PATCH_SRC, ReferenceUnit, anchors_of,
    reference_event_wavefront_sources, reference_megakernel_source, reference_variant,
};
use crate::{
    geometry::cuts::StandardGemCuts,
    renderer::{
        buffers::{
            GpuCameraParams, GpuGemMaterial, GpuTransportParams, encode_facet_finishes,
            transport_env_mode,
        },
        env_map_gpu::HdrEnvGpuData,
        gpu::{
            GpuContext, compute,
            estimator_check::{furnace_material, test_camera},
            event_wavefront::{EwKernel, compare::compare_frames},
            frame::{self, SHADER_SRC},
        },
    },
};

/// Environment variable that turns a missing adapter into a failure.
const REQUIRE_GPU_ENV: &str = "PLEOCHROMA_REQUIRE_GPU";

/// The shipped generated text of every unit the variant covers: the megakernel and the sixteen
/// event-wavefront kernels.
fn all_units() -> Vec<(ReferenceUnit, &'static str)> {
    let mut units = vec![(ReferenceUnit::Megakernel, SHADER_SRC)];
    units.extend(
        EwKernel::ALL
            .iter()
            .map(|&kernel| (ReferenceUnit::EventWavefront(kernel), kernel.source())),
    );
    units
}

fn validate(name: &str, source: &str) -> naga::Module {
    let module = wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("{name} does not parse: {}", e.emit_to_string(source)));
    Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name} does not validate: {}", e.emit_to_string(source)));
    module
}

/// The `@binding` numbers of the globals `entry` reaches in `source`.
fn reached_bindings(source: &str, entry: &str) -> Vec<u32> {
    let module = validate(entry, source);
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .expect("validated above");
    let index = module
        .entry_points
        .iter()
        .position(|ep| ep.name == entry)
        .expect("entry point exists");
    let uses = info.get_entry_point(index);
    let mut bindings: Vec<u32> = module
        .global_variables
        .iter()
        .filter(|(handle, _)| !uses[*handle].is_empty())
        .filter_map(|(_, var)| var.binding.as_ref().map(|b| b.binding))
        .collect();
    bindings.sort_unstable();
    bindings
}

fn variant_of(unit: ReferenceUnit, source: &str) -> String {
    reference_variant(unit, source).unwrap_or_else(|| panic!("{unit:?} has no reference variant"))
}

#[test]
fn every_anchor_occurs_its_expected_number_of_times_in_the_shipped_sources() {
    for (unit, source) in all_units() {
        let anchors = anchors_of(unit);
        assert!(!anchors.is_empty(), "{unit:?} has no anchors");
        for (what, anchor, expected) in anchors {
            assert_eq!(
                source.matches(anchor).count(),
                expected,
                "{unit:?}: {what} ({anchor:?})"
            );
        }
    }
}

#[test]
fn the_anchor_sets_have_the_planned_sizes() {
    // Twelve shared edits (the exact matches, the mismatch zeroing, the no-split guards, the
    // split_mask writes, the integration swap and the finalize hook) ...
    let shared = 12;
    // ... plus the megakernel's signature update, or the event wavefront's generate / tail edit.
    assert_eq!(anchors_of(ReferenceUnit::Megakernel).len(), shared + 1);
    for kernel in EwKernel::ALL {
        let own = usize::from(matches!(kernel, EwKernel::Generate | EwKernel::Tail));
        assert_eq!(
            anchors_of(ReferenceUnit::EventWavefront(kernel)).len(),
            shared + own,
            "{kernel:?}"
        );
    }
}

#[test]
fn the_cosine_match_sites_are_exactly_the_ones_the_variant_replaces() {
    // `directions_match` is called at 08e (two sites), 08d (one), in `narrow_compat` (one) and,
    // in the event wavefront, `ew_arm_uni_entry_transmit` (one more). A new site would show up
    // here before it could silently stay a tolerance test.
    for (unit, source) in all_units() {
        let transmit = matches!(
            unit,
            ReferenceUnit::EventWavefront(EwKernel::ArmUniEntryTransmit)
        );
        assert_eq!(
            source.matches("= directions_match(").count(),
            if transmit { 5 } else { 4 },
            "{unit:?}"
        );
    }
}

#[test]
fn a_missing_or_repeated_anchor_declines() {
    for (unit, source) in all_units() {
        for (what, anchor, _) in anchors_of(unit) {
            let missing = source.replacen(anchor, "", 1);
            assert!(
                reference_variant(unit, &missing).is_none(),
                "{unit:?}: a missing anchor must decline ({what})"
            );
            let repeated = format!("{source}\n{anchor}\n");
            assert!(
                reference_variant(unit, &repeated).is_none(),
                "{unit:?}: a repeated anchor must decline ({what})"
            );
        }
    }
}

#[test]
fn a_text_that_is_not_a_transport_unit_has_no_variant() {
    assert!(reference_variant(ReferenceUnit::Megakernel, "fn nothing() {}").is_none());
    assert!(
        reference_variant(
            ReferenceUnit::EventWavefront(EwKernel::Tail),
            "fn nothing() {}"
        )
        .is_none()
    );
}

#[test]
fn a_variant_cannot_be_patched_twice() {
    let once = variant_of(ReferenceUnit::Megakernel, SHADER_SRC);
    assert!(reference_variant(ReferenceUnit::Megakernel, &once).is_none());
}

#[test]
fn the_variants_hold_no_cosine_match_and_no_split_staging() {
    for (unit, source) in all_units() {
        let text = variant_of(unit, source);
        assert!(!text.contains("= directions_match("), "{unit:?}");
        assert!(!text.contains("split_mask | (1u << k)"), "{unit:?}");
        assert!(
            !text.contains("prefix_path_pdf_k * "),
            "{unit:?}: a mismatched channel keeps no path density"
        );
        for once in [
            "fn integrate_channels_to_xyz_family(",
            "fn integrate_channels_to_xyz_family_shipped(",
            "fn ref_integrate_plain(",
            "fn branch_step(",
            "fn ref_sig_event(",
            "fn beam_kept(",
            "fn ref_sig_for(",
            "fn ref_apply_kept(",
        ] {
            assert_eq!(text.matches(once).count(), 1, "{unit:?}: {once}");
        }
    }
}

#[test]
fn the_signature_lives_in_a_private_variable_in_the_megakernel_and_a_buffer_in_the_wavefront() {
    assert!((200..=219).contains(&RAY_SIG_BINDING));
    for (unit, source) in all_units() {
        let text = variant_of(unit, source);
        match unit {
            ReferenceUnit::Megakernel => {
                assert!(text.contains("var<private> ref_sig: vec2<u32>"));
                assert!(!text.contains("ray_sig"));
            }
            ReferenceUnit::EventWavefront(_) => {
                assert!(text.contains(&format!(
                    "@group(0) @binding({RAY_SIG_BINDING}) var<storage, read_write> ray_sig"
                )));
                assert!(!text.contains("var<private> ref_sig"));
            }
        }
    }
}

#[test]
fn the_shipped_sources_hold_none_of_the_variant() {
    for (unit, source) in all_units() {
        for marker in [
            "beam_kept",
            "ray_sig",
            "branch_step",
            "ref_sig",
            "ref_bits_eq",
            "BRANCH_SIG_SEED",
        ] {
            assert!(!source.contains(marker), "{unit:?} holds {marker}");
        }
        assert_ne!(variant_of(unit, source), source, "{unit:?}");
    }
}

#[test]
fn the_cached_sources_equal_a_fresh_build() {
    let mega = reference_megakernel_source().expect("the megakernel variant exists");
    assert_eq!(mega, variant_of(ReferenceUnit::Megakernel, SHADER_SRC));
    assert_eq!(
        reference_megakernel_source().as_deref(),
        Some(mega.as_str())
    );

    let kernels = reference_event_wavefront_sources().expect("the wavefront variants exist");
    assert_eq!(kernels.len(), EwKernel::ALL.len());
    for ((entry, text), kernel) in kernels.iter().zip(EwKernel::ALL) {
        assert_eq!(entry, kernel.entry_point());
        assert_eq!(
            *text,
            variant_of(ReferenceUnit::EventWavefront(kernel), kernel.source())
        );
    }
}

#[test]
fn branch_sig_wgsl_validates_alone() {
    let module = validate("branch_sig.wgsl", BRANCH_SIG_SRC);
    assert!(
        module
            .functions
            .iter()
            .any(|(_, f)| f.name.as_deref() == Some("branch_step"))
    );
}

#[test]
fn the_reference_patch_is_the_only_home_of_the_kept_hook() {
    assert_eq!(REFERENCE_PATCH_SRC.matches("fn beam_kept(").count(), 1);
    assert!(REFERENCE_PATCH_SRC.contains(
        "fn beam_kept(pixel: u32, image_point: vec2<f32>, sig: vec2<u32>, lambda: f32) -> bool"
    ));
    assert!(!BRANCH_SIG_SRC.contains("beam_kept"));
}

#[test]
fn every_variant_unit_validates() {
    for (unit, source) in all_units() {
        let text = variant_of(unit, source);
        validate(&format!("{unit:?} reference variant"), &text);
    }
}

#[test]
fn the_megakernel_variant_adds_no_binding() {
    let plain = reached_bindings(SHADER_SRC, "transport_main");
    let reference = reached_bindings(
        &variant_of(ReferenceUnit::Megakernel, SHADER_SRC),
        "transport_main",
    );
    assert_eq!(reference, plain);
}

#[test]
fn only_generate_tail_and_finalize_reach_the_signature_buffer() {
    for kernel in EwKernel::ALL {
        let text = variant_of(ReferenceUnit::EventWavefront(kernel), kernel.source());
        let reaches = reached_bindings(&text, kernel.entry_point()).contains(&RAY_SIG_BINDING);
        assert_eq!(
            reaches,
            matches!(
                kernel,
                EwKernel::Generate | EwKernel::Tail | EwKernel::Finalize
            ),
            "{kernel:?}"
        );
    }
}

fn context_or_skip(test_name: &str) -> Option<GpuContext> {
    match GpuContext::acquire() {
        Ok(ctx) => Some(ctx),
        Err(error) => {
            assert!(
                !std::env::var(REQUIRE_GPU_ENV).is_ok_and(|v| v == "1"),
                "{test_name}: {REQUIRE_GPU_ENV}=1 but no usable GPU adapter: {error}"
            );
            println!("skipping {test_name}: no GPU adapter");
            None
        }
    }
}

/// Runs `branch_step` on the two pinned inputs.
const PIN_KERNEL: &str = "
@group(0) @binding(0) var<storage, read_write> pin_out: array<u32>;

@compute @workgroup_size(1)
fn pin_main() {
    let first = branch_step(BRANCH_SIG_SEED, 7u, BRANCH_KIND_REFLECT, false);
    let second = branch_step(first, 12u, BRANCH_KIND_TRANSMIT, true);
    pin_out[0] = first.x;
    pin_out[1] = first.y;
    pin_out[2] = second.x;
    pin_out[3] = second.y;
}
";

/// CPU = WGSL signature pin: `branch_step(SEED, 7, Reflect, false)` and one step further
/// `(12, Transmit, true)` give the literals of the guide (section 14.3), the same ones the CPU
/// tests pin.
#[test]
fn the_wgsl_branch_step_reproduces_the_pinned_literals() {
    let name = "the_wgsl_branch_step_reproduces_the_pinned_literals";
    let Some(ctx) = context_or_skip(name) else {
        return;
    };
    let source = format!("{BRANCH_SIG_SRC}\n{PIN_KERNEL}");
    let out = compute::zeroed_buffer::<u32>(
        &ctx.device,
        "branch pin out",
        4,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let pipeline = compute::create_compute_pipeline(&ctx.device, "branch pin", &source, "pin_main");
    let bind_group = compute::bind_buffers(
        &ctx.device,
        "branch pin bind group",
        &pipeline,
        &[(0, &out)],
    );
    compute::dispatch_and_wait(&ctx.device, &ctx.queue, &pipeline, &bind_group, (1, 1, 1));
    let words: Vec<u32> = compute::readback(&ctx.device, &ctx.queue, &out, 4);
    assert_eq!(
        words,
        [0x0707_A745, 0x3F6E_6B9A, 0x5312_98E6, 0x62FA_FEB4],
        "the WGSL branch_step disagrees with the CPU pin"
    );
}

const FRAME_WIDTH: u32 = 24;
const FRAME_HEIGHT: u32 = 24;
/// [`FRAME_WIDTH`] and [`FRAME_HEIGHT`] as the camera uniform stores them.
const FRAME_SIZE_F32: f32 = 24.0;
const SAMPLES_PER_PIXEL: u32 = 16;

/// One megakernel dispatch of `source` over a small frame of the constant-index furnace stone;
/// the per-pixel sum of the samples' XYZ.
fn megakernel_frame(ctx: &GpuContext, source: &str) -> Vec<Vec3> {
    let material = GpuGemMaterial::encode(&furnace_material());
    let camera = test_camera();
    let camera_params = GpuCameraParams {
        origin: camera.origin.to_array(),
        fov_tan: camera.fov_tan,
        forward: camera.forward.to_array(),
        width: FRAME_SIZE_F32,
        right: camera.right.to_array(),
        height: FRAME_SIZE_F32,
        up: camera.up.to_array(),
        num_samples: SAMPLES_PER_PIXEL,
    };
    let params = GpuTransportParams::new(
        FRAME_WIDTH * FRAME_HEIGHT,
        12,
        0,
        transport_env_mode::UNIFORM_FURNACE,
        2.5,
        6500.0,
        1.0,
        1.0,
        0.0,
        0.0,
        [1.0, 1.0, 1.0],
    );
    let planes = StandardGemCuts::standard_round_brilliant();
    let finishes = encode_facet_finishes(&[], planes.len());
    let class = (frame::material_class::GENERIC & frame::material_class::KIND_MASK)
        | frame::material_class::feature_bits(&material);
    let pipeline = compute::create_compute_pipeline_with_constants(
        &ctx.device,
        "reference check",
        source,
        "transport_main",
        &frame::material_class::pipeline_constants(class),
    );
    let hdr = HdrEnvGpuData::dummy(&ctx.device);
    let tuples = (FRAME_WIDTH * FRAME_HEIGHT * SAMPLES_PER_PIXEL) as usize;
    let outputs = frame::TransportOutputs::new(&ctx.device, tuples);
    frame::encode_and_dispatch(
        &frame::TransportDispatchArgs {
            ctx,
            pipeline: &pipeline,
            camera_params: &camera_params,
            params: &params,
            material: &material,
            planes: &planes,
            facet_finishes: &finishes,
            outputs: &outputs,
            hdr_env: &hdr,
            tray_shadow: None,
        },
        tuples,
    );
    let xyz: Vec<f32> = compute::readback(&ctx.device, &ctx.queue, outputs.xyz(), tuples * 3);
    xyz.as_chunks::<{ 3 * SAMPLES_PER_PIXEL as usize }>()
        .0
        .iter()
        .map(|pixel| {
            pixel
                .as_chunks::<3>()
                .0
                .iter()
                .fold(Vec3::ZERO, |sum, s| sum + Vec3::from_array(*s))
        })
        .collect()
}

/// The reference variant equals production on a constant-index material (Cauchy b = c = 0):
/// every channel's indices are bitwise the hero's, so the exact match and the cosine test agree,
/// no channel terminates, and the plain integration equals the families one with `compat` all
/// ones. The frames must agree within the twin tolerance; they are expected to be bitwise equal.
#[test]
#[ignore = "over 60 s in the retest (2026-11-09); run on request: scripts/run_beam_tests.sh step 03g"]
fn the_reference_megakernel_equals_production_on_a_constant_index_material() {
    let name = "the_reference_megakernel_equals_production_on_a_constant_index_material";
    let Some(ctx) = context_or_skip(name) else {
        return;
    };
    let reference = reference_megakernel_source().expect("the megakernel variant exists");
    let production = megakernel_frame(&ctx, SHADER_SRC);
    let variant = megakernel_frame(&ctx, &reference);
    let diff = compare_frames(&production, &variant, SAMPLES_PER_PIXEL);
    println!("reference vs production: {diff:?}");
    assert!(
        production.iter().any(|pixel| pixel.length_squared() > 0.0),
        "the probe must see light"
    );
    assert!(
        diff.tol_passed(production.len()),
        "the reference variant moved a constant-index render: {diff:?}"
    );
}
