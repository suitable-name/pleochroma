//! Tests of the sorted wavefront (`super::sorted`).
//!
//! They cover the switch, the pool and round
//! arithmetic, the declines, the queue layout and the bindings against the shader text, and,
//! with an adapter, the twins against the megakernel, the accounting of regeneration and the
//! drain (every tuple finished exactly once), the exit-collection twin, the queues and the
//! surface phase the class twins never take (a scattering stone under an HDR map with a concave
//! tool, a zoned stone), and a decline that leaves the megakernel's image untouched.
//!
//! The GPU tests skip with a printed note when no adapter exists (`PLEOCHROMA_REQUIRE_GPU=1`
//! turns that into a failure). Each compiles one megakernel class and the sorted kernels of that
//! class (about six megakernel-sized pipelines; seconds from the pipeline cache).

use glam::Vec3;

use super::{
    GpuFrameRenderer, GpuFrameScene, GpuPipelineKind, classify_material,
    classify_material_with_tables, material_class,
    sorted::{
        DEFAULT_POOL_MIB, GpuSortedParams, MAX_POOL_SLOTS, MAX_ROUNDS, MIB, PREPARE_SHADER_SRC,
        PREPARE_SLOTS, SLOT_BYTES, SORTED_SHADER_SRC, SW_COUNTERS, SW_FLAG_CHECK, SW_I_ARM,
        SW_I_DRAIN, SW_I_FRONT, SW_INDIRECTS, SW_QUEUES, SortedArm, SortedDecline, SortedSettings,
        SortedSwitch, TRANSPORT_SLOTS, pool_slots_for, rounds_for, sorted_arms, sorted_decline,
        sorted_requested,
    },
};
use crate::{
    geometry::{cuts::StandardGemCuts, tool::ToolPrimitive},
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, Estimator, LightingPreset, hash_u32},
        tables::OpticsTables,
    },
    renderer::{
        buffers::{EwHit, EwRayChan, EwRayCore},
        gpu::{
            environment_check::build_hdr_test_map, event_wavefront::compare::compare_frames,
            merge_tests::renderer_or_skip, zoned_cases::ZonedImageCase,
        },
    },
};

/// Control word: rounds opened (`SW_C_ROUND` of `shaders/sorted_queues.wgsl`).
const C_ROUND: usize = 15;
/// Control word: the next tuple to start (`SW_C_NEXT`).
const C_NEXT: usize = 16;
/// Control word: the stored rays the last round's front kernel resumed (`SW_C_KEEP`).
const C_KEEP: usize = 17;
/// Control word: tuples the rounds started (`SW_C_GENERATED`).
const C_GENERATED: usize = 20;
/// Control word: rays queued for the front kernel when the drain began (`SW_C_DRAIN_FRONT_END`).
const C_DRAIN_FRONT_END: usize = 22;
/// Control word: paths left for the drain (`SW_C_DRAIN_TOTAL`).
const C_DRAIN_TOTAL: usize = 28;
/// Control word: paths finalized (`SW_C_FINALIZED`).
const C_FINALIZED: usize = 29;
/// Control word: paths the drain finished (`SW_C_DRAINED`).
const C_DRAINED: usize = 30;
/// Control word: wrapping sum of the finalized tuples (`SW_C_SUM`).
const C_SUM: usize = 31;
/// Control word: xor of the finalized tuples' hashes (`SW_C_XOR`).
const C_XOR: usize = 32;
/// Control word: pushes into a full queue (`SW_C_OVERFLOW`).
const C_OVERFLOW: usize = 33;

/// The test frame's width and height.
const SIZE: u32 = 32;
/// Samples per pixel of the test frame.
const SPP: u32 = 3;
/// The bounce cap of the test frame.
const MAX_BOUNCES: u32 = 12;
/// The tuples of the test frame: one chunk.
const TUPLES: u32 = SIZE * SIZE * SPP;

/// The value of the line `const NAME: u32 = Vu;` of `source`.
fn wgsl_const(source: &str, name: &str) -> u32 {
    let prefix = format!("const {name}: u32 = ");
    let value = source
        .lines()
        .find_map(|line| line.trim().strip_prefix(prefix.as_str()))
        .unwrap_or_else(|| panic!("no `const {name}: u32` in the shader text"));
    value
        .trim_end_matches(';')
        .trim_end_matches('u')
        .parse()
        .unwrap_or_else(|_| panic!("`const {name}` is no decimal u32: {value}"))
}

#[test]
fn the_switch_reads_off_on_and_a_pool_size() {
    assert_eq!(sorted_requested(None), None);
    assert_eq!(sorted_requested(Some("")), None);
    assert_eq!(sorted_requested(Some("0")), None);
    assert_eq!(sorted_requested(Some(" 1 ")), Some(DEFAULT_POOL_MIB * MIB));
    assert_eq!(sorted_requested(Some("16")), Some(16 * MIB));
    assert_eq!(sorted_requested(Some("1024")), Some(1024 * MIB));
    assert_eq!(sorted_requested(Some("4096")), Some(4096 * MIB));
    for off in ["2", "15", "4097", "on", "-1", "1.5"] {
        assert_eq!(sorted_requested(Some(off)), None, "{off}");
    }
}

#[test]
fn a_chunk_gets_the_smallest_of_its_tuples_the_budget_and_the_binding() {
    assert_eq!(
        SLOT_BYTES,
        size_of::<EwRayCore>() + size_of::<EwRayChan>() + size_of::<EwHit>() + SW_QUEUES * 4
    );
    assert_eq!(SLOT_BYTES, 712);
    let budget = DEFAULT_POOL_MIB * MIB;
    let binding = 2048 * MIB;
    assert_eq!(pool_slots_for(1000, budget, binding), 1000);
    assert_eq!(
        pool_slots_for(10_000_000, budget, binding),
        budget / SLOT_BYTES
    );
    assert_eq!(
        pool_slots_for(10_000_000, 4096 * MIB, 128 * MIB),
        128 * MIB / size_of::<EwRayChan>()
    );
    assert_eq!(
        pool_slots_for(usize::MAX, usize::MAX, usize::MAX),
        MAX_POOL_SLOTS
    );
    assert_eq!(pool_slots_for(0, budget, binding), 0);
}

#[test]
fn the_rounds_finish_a_full_pool_and_grow_with_the_tuples_per_slot() {
    // A pool as large as the chunk: one round per bounce and the first front.
    assert_eq!(rounds_for(3072, 3072, 12), 13);
    assert_eq!(rounds_for(100, 5000, 32), 33);
    // A cap over 32 leaves its longest paths to the drain.
    assert_eq!(rounds_for(100, 5000, 128), 33);
    // Twelve tuples per slot: eight rounds each while the pool is refilled, then the tail.
    assert_eq!(rounds_for(3072, 256, 12), 96 + 13);
    assert_eq!(rounds_for(1_000_000, 377_000, 32), 22 + 33);
    assert_eq!(rounds_for(0, 256, 12), 0);
    assert_eq!(rounds_for(256, 0, 12), 0);
    assert_eq!(rounds_for(4_000_000, 1, 32), MAX_ROUNDS);
}

#[test]
fn each_class_gets_the_arm_kernels_its_rays_can_choose() {
    let all = SortedArm::ALL.to_vec();
    assert_eq!(sorted_arms(material_class::UNIAXIAL), all);
    assert_eq!(
        sorted_arms(material_class::GENERIC | material_class::NO_ORIENTED),
        all
    );
    let plain = vec![SortedArm::Frosted, SortedArm::Tir, SortedArm::Generic];
    assert_eq!(sorted_arms(material_class::ISOTROPIC), plain);
    assert_eq!(
        sorted_arms(material_class::BIAXIAL | material_class::NO_EXTRAORDINARY_DISPERSION),
        plain
    );
    for (number, arm) in (1usize..).zip(&all) {
        assert_eq!(arm.index() + 1, number, "{arm:?}");
        assert!(
            SORTED_SHADER_SRC.contains(&format!("fn {}(", arm.entry_point())),
            "{arm:?}"
        );
    }
    for (name, number) in [
        ("ARM_FROSTED", 1),
        ("ARM_TIR", 2),
        ("ARM_UNI_ENTRY", 3),
        ("ARM_UNI_INTERNAL", 4),
        ("ARM_GENERIC", 5),
    ] {
        assert_eq!(wgsl_const(SORTED_SHADER_SRC, name), number, "{name}");
    }
}

#[test]
fn the_first_decline_reason_wins_and_a_capable_device_declines_nothing() {
    let capable = wgpu::Limits {
        max_storage_buffers_per_shader_stage: 32,
        max_buffers_and_acceleration_structures_per_shader_stage: 40,
        ..wgpu::Limits::default()
    };
    let class = material_class::UNIAXIAL;
    let mega = GpuPipelineKind::Megakernel;
    assert_eq!(sorted_decline(mega, class, &capable, true), None);
    assert_eq!(
        sorted_decline(
            GpuPipelineKind::Wavefront,
            class | material_class::OPTICS_TABLES,
            &capable,
            true
        ),
        Some(SortedDecline::LegacyWavefront)
    );
    assert_eq!(
        sorted_decline(
            mega,
            class | material_class::OPTICS_TABLES | material_class::TRAY_SHADOW,
            &capable,
            true
        ),
        Some(SortedDecline::OpticsTables)
    );
    assert_eq!(
        sorted_decline(mega, class | material_class::TRAY_SHADOW, &capable, true),
        Some(SortedDecline::TrayVariant)
    );
    assert_eq!(
        sorted_decline(mega, class | material_class::TRAY_MIRROR, &capable, true),
        Some(SortedDecline::TrayVariant)
    );
    assert_eq!(
        sorted_decline(mega, class, &wgpu::Limits::default(), true),
        Some(SortedDecline::DeviceLimits)
    );
    let short_total = wgpu::Limits {
        max_buffers_and_acceleration_structures_per_shader_stage: 20,
        ..capable
    };
    assert_eq!(
        sorted_decline(mega, class, &short_total, true),
        Some(SortedDecline::DeviceLimits)
    );
    assert_eq!(
        sorted_decline(mega, class, &capable, false),
        Some(SortedDecline::NoIndirectDispatch)
    );
    let bits = [
        SortedDecline::LegacyWavefront,
        SortedDecline::OpticsTables,
        SortedDecline::TrayVariant,
        SortedDecline::DeviceLimits,
        SortedDecline::NoIndirectDispatch,
        SortedDecline::AdaptivePass,
    ]
    .map(SortedDecline::bit);
    assert_eq!(bits.iter().fold(0, |acc, bit| acc | bit), 63);
}

#[test]
fn the_host_queue_layout_is_the_shaders() {
    for source in [SORTED_SHADER_SRC, PREPARE_SHADER_SRC] {
        for (name, host) in [
            ("SW_QUEUES", SW_QUEUES),
            ("SW_COUNTERS", SW_COUNTERS),
            ("SW_INDIRECTS", SW_INDIRECTS),
            ("SW_I_FRONT", SW_I_FRONT),
            ("SW_I_ARM", SW_I_ARM),
            ("SW_I_DRAIN", SW_I_DRAIN),
            ("SW_ARMS", SortedArm::ALL.len()),
            ("SW_C_ROUND", C_ROUND),
            ("SW_C_NEXT", C_NEXT),
            ("SW_C_KEEP", C_KEEP),
            ("SW_C_GENERATED", C_GENERATED),
            ("SW_C_DRAIN_FRONT_END", C_DRAIN_FRONT_END),
            ("SW_C_DRAIN_TOTAL", C_DRAIN_TOTAL),
            ("SW_C_FINALIZED", C_FINALIZED),
            ("SW_C_DRAINED", C_DRAINED),
            ("SW_C_SUM", C_SUM),
            ("SW_C_XOR", C_XOR),
            ("SW_C_OVERFLOW", C_OVERFLOW),
        ] {
            assert_eq!(wgsl_const(source, name) as usize, host, "{name}");
        }
        assert_eq!(wgsl_const(source, "SW_FLAG_CHECK"), SW_FLAG_CHECK);
        assert_eq!(wgsl_const(source, "SW_WORKGROUP"), 64);
    }
    let arms = SortedArm::ALL.len();
    let free = wgsl_const(PREPARE_SHADER_SRC, "SW_Q_FREE") as usize;
    assert_eq!(
        wgsl_const(PREPARE_SHADER_SRC, "SW_Q_ARM") as usize + 2 * arms,
        free
    );
    assert_eq!(free + 2, SW_QUEUES);
    assert_eq!(
        wgsl_const(PREPARE_SHADER_SRC, "SW_C_DRAIN_ARM_END") as usize + arms,
        C_DRAIN_TOTAL
    );
    assert_eq!(C_OVERFLOW + 1, SW_COUNTERS);
    assert_eq!(SW_I_ARM + arms, SW_I_DRAIN);
    assert_eq!(SW_I_DRAIN + 1, SW_INDIRECTS);
    assert_eq!(size_of::<GpuSortedParams>(), 16);
}

/// Every entry point must reach exactly the bindings the host binds: a binding reached but not
/// bound (or the reverse) is a wgpu validation error, a lost chunk.
#[test]
fn every_kernel_reaches_exactly_the_bindings_the_host_binds() {
    use naga::{
        front::wgsl,
        valid::{Capabilities, ValidationFlags, Validator},
    };
    let transport: Vec<&str> = ["sw_front_main", "sw_drain_main"]
        .into_iter()
        .chain(SortedArm::ALL.iter().map(|arm| arm.entry_point()))
        .collect();
    for (source, entries, slots) in [
        (SORTED_SHADER_SRC, transport, &TRANSPORT_SLOTS[..]),
        (
            PREPARE_SHADER_SRC,
            vec!["sw_prepare", "sw_prepare_drain"],
            &PREPARE_SLOTS[..],
        ),
    ] {
        let module = wgsl::parse_str(source).expect("the unit parses");
        let info = Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .expect("the unit validates");
        for entry in entries {
            let index = module
                .entry_points
                .iter()
                .position(|point| point.name == entry)
                .unwrap_or_else(|| panic!("no entry point {entry}"));
            let usage = info.get_entry_point(index);
            let mut reached: Vec<u32> = module
                .global_variables
                .iter()
                .filter(|(handle, _)| !usage[*handle].is_empty())
                .filter_map(|(_, global)| global.binding.as_ref())
                .map(|binding| binding.binding)
                .collect();
            reached.sort_unstable();
            assert_eq!(reached, slots, "{entry}");
        }
    }
}

/// On with the default pool and every path counted.
const fn checked() -> SortedSettings {
    SortedSettings {
        check: true,
        ..SortedSettings::with_pool(DEFAULT_POOL_MIB * MIB)
    }
}

/// The daylight studio of the twin scenes.
const fn daylight() -> EnvironmentSource<'static> {
    LightingPreset::Daylight.studio(1.0, 0.4, 0.35)
}

/// Renders the test frame of `material`, carved by `tools` (none: the plain brilliant), through
/// `class` under `environment` with the sorted wavefront switched as `switch`. The staged
/// megakernel is off, so `SortedSwitch::Off` is the plain megakernel whatever
/// `PLEOCHROMA_STAGED` says in the caller's shell.
fn frame_with_tools(
    renderer: &mut GpuFrameRenderer,
    material: &GemMaterial,
    tools: &[ToolPrimitive],
    class: u32,
    environment: EnvironmentSource<'_>,
    switch: SortedSwitch,
) -> Vec<Vec3> {
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let scene = GpuFrameScene {
        camera: &camera,
        width: SIZE,
        height: SIZE,
        planes: &planes,
        facet_finishes: &[],
        material,
        max_bounces: MAX_BOUNCES,
        environment,
    };
    renderer.set_sorted(switch);
    renderer.set_stage_ends(Some(Vec::new()));
    let mut accum = vec![Vec3::ZERO; (SIZE * SIZE) as usize];
    renderer
        .accumulate_via_pipeline_geom(&scene, tools, class, 0, SPP, &mut accum, None)
        .expect("the test frame renders on the GPU");
    accum
}

/// [`frame_with_tools`] for the plain brilliant.
fn frame(
    renderer: &mut GpuFrameRenderer,
    material: &GemMaterial,
    class: u32,
    environment: EnvironmentSource<'_>,
    switch: SortedSwitch,
) -> Vec<Vec3> {
    frame_with_tools(renderer, material, &[], class, environment, switch)
}

/// The sorted frame is the megakernel's twin (the rule of `event_wavefront::compare`).
fn assert_twins(name: &str, megakernel: &[Vec3], sorted: &[Vec3]) {
    let total: f32 = megakernel.iter().map(|pixel| pixel.y).sum();
    assert!(total > 0.0, "{name}: a black megakernel frame");
    let diff = compare_frames(megakernel, sorted, SPP);
    assert!(
        diff.twin_passed(megakernel.len()),
        "{name}: the sorted wavefront is no twin of the megakernel: {diff:?}"
    );
}

/// Two frames are the same, bit for bit.
fn assert_bitwise(name: &str, a: &[Vec3], b: &[Vec3]) {
    let differing = a
        .iter()
        .zip(b)
        .filter(|(x, y)| {
            x.to_array()
                .iter()
                .zip(y.to_array().iter())
                .any(|(p, q)| p.to_bits() != q.to_bits())
        })
        .count();
    assert_eq!(
        differing, 0,
        "{name}: two runs differ in {differing} pixels"
    );
}

/// The counters of a checked chunk of the test frame say every tuple was finished exactly once:
/// the count, the wrapping sum of the tuples and the xor of their hashes, no queue overflow, and
/// every path the drain was given finished by it. A path lost or doubled between two rounds
/// changes the count, the sum or the xor.
fn assert_finished_once(name: &str, counters: &[u32]) {
    let sum = (0..TUPLES).fold(0u32, u32::wrapping_add);
    let xor = (0..TUPLES).fold(0u32, |acc, tuple| acc ^ hash_u32(tuple));
    assert_eq!(counters[C_FINALIZED], TUPLES, "{name}: paths finalized");
    assert_eq!(counters[C_SUM], sum, "{name}: sum of the finalized tuples");
    assert_eq!(
        counters[C_XOR], xor,
        "{name}: xor of the finalized tuples' hashes"
    );
    assert_eq!(counters[C_OVERFLOW], 0, "{name}: pushes into a full queue");
    assert_eq!(
        counters[C_DRAINED], counters[C_DRAIN_TOTAL],
        "{name}: paths the drain finished"
    );
}

/// The sorted wavefront against the megakernel on `material`: a twin, the same bits when
/// repeated, every tuple finished once, and every path finished in the rounds (the pool holds
/// the whole frame, so the rounds give every tuple a whole path).
fn sorted_twin(test_name: &str, material_name: &str) {
    let Some(mut renderer) = renderer_or_skip(test_name) else {
        return;
    };
    let material = GemMaterial::by_name(material_name).expect("a built-in material");
    let class = classify_material(&material);
    let megakernel = frame(
        &mut renderer,
        &material,
        class,
        daylight(),
        SortedSwitch::Off,
    );
    let on = SortedSwitch::On(checked());
    let sorted = frame(&mut renderer, &material, class, daylight(), on);
    let counters = renderer.sorted_counters().expect("a sorted chunk ran");
    let again = frame(&mut renderer, &material, class, daylight(), on);
    assert_twins(material_name, &megakernel, &sorted);
    assert_bitwise(material_name, &sorted, &again);
    assert_finished_once(material_name, &counters);
    assert_eq!(
        counters[C_DRAIN_TOTAL], 0,
        "{material_name}: paths left for the drain with a whole-frame pool"
    );
    assert_eq!(
        counters[C_ROUND],
        MAX_BOUNCES + 1,
        "{material_name}: rounds"
    );
}

#[test]
fn the_sorted_wavefront_is_a_twin_of_the_megakernel_on_an_isotropic_stone() {
    sorted_twin(
        "the_sorted_wavefront_is_a_twin_of_the_megakernel_on_an_isotropic_stone",
        "Diamond",
    );
}

#[test]
fn the_sorted_wavefront_is_a_twin_of_the_megakernel_on_a_uniaxial_stone() {
    sorted_twin(
        "the_sorted_wavefront_is_a_twin_of_the_megakernel_on_a_uniaxial_stone",
        "Zircon",
    );
}

#[test]
fn the_sorted_wavefront_is_a_twin_of_the_megakernel_on_a_biaxial_stone() {
    sorted_twin(
        "the_sorted_wavefront_is_a_twin_of_the_megakernel_on_a_biaxial_stone",
        "Peridot",
    );
}

/// A pool of 256 slots for 3072 tuples, refilled about a dozen times. With enough rounds for any
/// path lengths every tuple ends in the rounds; with nine rounds the drain finishes queued rays
/// and tuples never started; with none it traces the whole frame. Every run finishes each tuple
/// exactly once and is the megakernel's twin, and the first and the last are twins of each
/// other.
#[test]
fn every_tuple_is_finished_once_with_regeneration_and_the_drain() {
    let name = "every_tuple_is_finished_once_with_regeneration_and_the_drain";
    let Some(mut renderer) = renderer_or_skip(name) else {
        return;
    };
    let material = GemMaterial::by_name("Zircon").expect("a built-in material");
    let class = classify_material(&material);
    let megakernel = frame(
        &mut renderer,
        &material,
        class,
        daylight(),
        SortedSwitch::Off,
    );
    let pool = 256u32;
    // Enough for any lengths: a full pool for ceil(tuples * (cap + 1) / pool) rounds, then a
    // whole path for the last tuples started.
    let enough = (TUPLES * (MAX_BOUNCES + 1)).div_ceil(pool) + MAX_BOUNCES + 1;
    let mut frames = Vec::new();
    for (rounds, label) in [
        (enough, "enough rounds"),
        (9, "nine rounds"),
        (0, "the drain alone"),
    ] {
        let settings = SortedSettings {
            pool_slots: Some(pool as usize),
            rounds: Some(rounds),
            ..checked()
        };
        let sorted = frame(
            &mut renderer,
            &material,
            class,
            daylight(),
            SortedSwitch::On(settings),
        );
        let counters = renderer.sorted_counters().expect("a sorted chunk ran");
        assert_finished_once(label, &counters);
        assert_eq!(counters[C_ROUND], rounds, "{label}: rounds");
        let unstarted = TUPLES - counters[C_NEXT];
        let queued = counters[C_DRAIN_TOTAL] - unstarted;
        if rounds == 0 {
            assert_eq!(
                counters[C_GENERATED], 0,
                "{label}: tuples the rounds started"
            );
            assert_eq!(
                counters[C_DRAIN_TOTAL], TUPLES,
                "{label}: paths of the drain"
            );
        } else if rounds == enough {
            assert_eq!(
                counters[C_DRAIN_TOTAL], 0,
                "{label}: paths left for the drain"
            );
            assert!(
                counters[C_GENERATED] > pool,
                "{label}: the pool was never refilled"
            );
        } else {
            assert!(
                unstarted > 0 && queued > 0,
                "{label}: the drain should take started and unstarted paths: {unstarted} \
                 unstarted, {queued} queued"
            );
        }
        assert_twins(label, &megakernel, &sorted);
        frames.push(sorted);
    }
    let diff = compare_frames(&frames[0], &frames[2], SPP);
    assert!(
        diff.twin_passed(frames[0].len()),
        "enough rounds against the drain alone: {diff:?}"
    );
}

/// Exit collection on the sorted wavefront: the estimator bit reaches every kernel (`ew_load`
/// and `transport_generate_ray` bind it), so the frame is the megakernel's twin under the same
/// estimator, under the ring lights of the megakernel's own collect test.
#[test]
fn the_sorted_wavefront_collects_like_the_megakernel() {
    let name = "the_sorted_wavefront_collects_like_the_megakernel";
    let Some(mut renderer) = renderer_or_skip(name) else {
        return;
    };
    renderer.set_estimator(Some(Estimator::ExitCollection));
    let material = GemMaterial::diamond();
    let class = classify_material(&material);
    let lights = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    let megakernel = frame(&mut renderer, &material, class, lights, SortedSwitch::Off);
    let sorted = frame(
        &mut renderer,
        &material,
        class,
        lights,
        SortedSwitch::On(checked()),
    );
    let counters = renderer.sorted_counters().expect("a sorted chunk ran");
    assert_finished_once("collect", &counters);
    assert_twins("collect", &megakernel, &sorted);
}

/// The queues and the surface phase the class twins never take: a scattering stone (a bounce
/// that ends in the medium sends its ray back to the front queue, so the front kernel resumes
/// stored rays) under an HDR map (the scattering-point next-event estimate and its MIS carry)
/// and carved by a concave tool, and a zoned stone (the megakernel's own surface phase). Each
/// runs with a pool of 256 slots and nine rounds, so the drain finishes rays the rounds left
/// queued and tuples never started; for the scattering stone the last round's front kernel
/// resumes stored rays and some of the queued rays wait for the front kernel. Every tuple is
/// finished once and each frame is the megakernel's twin.
#[test]
#[ignore = "over 60 s in retest step 02d (2026-11-11); run on request: scripts/run_tests_on_request.sh"]
fn scattering_concave_hdr_and_zoned_stones_are_twins_with_regeneration() {
    let name = "scattering_concave_hdr_and_zoned_stones_are_twins_with_regeneration";
    let Some(mut renderer) = renderer_or_skip(name) else {
        return;
    };
    let map = build_hdr_test_map();
    let tools = [ToolPrimitive::ball(Vec3::new(0.2, 0.32, 0.1), 0.25)];
    let scattering = GemMaterial::by_name("Spinel")
        .expect("a built-in material")
        .with_scattering(1.0, 0.3);
    let zoned = ZonedImageCase::Bicolour.material();
    let settings = SortedSettings {
        pool_slots: Some(256),
        rounds: Some(9),
        ..checked()
    };
    for (label, material, carving, environment) in [
        (
            "scattering, concave, HDR",
            &scattering,
            &tools[..],
            EnvironmentSource::hdr(&map),
        ),
        ("zoned", &zoned, &[][..], daylight()),
    ] {
        let class = classify_material(material);
        let megakernel = frame_with_tools(
            &mut renderer,
            material,
            carving,
            class,
            environment,
            SortedSwitch::Off,
        );
        let sorted = frame_with_tools(
            &mut renderer,
            material,
            carving,
            class,
            environment,
            SortedSwitch::On(settings),
        );
        let counters = renderer.sorted_counters().expect("a sorted chunk ran");
        assert_finished_once(label, &counters);
        let unstarted = TUPLES - counters[C_NEXT];
        let queued = counters[C_DRAIN_TOTAL] - unstarted;
        assert!(
            unstarted > 0 && queued > 0,
            "{label}: the drain should take started and unstarted paths: {unstarted} \
             unstarted, {queued} queued"
        );
        if material.scattering_sigma_s > 0.0 {
            assert!(
                counters[C_KEEP] > 0,
                "{label}: the last round's front kernel resumed no stored ray"
            );
            assert!(
                counters[C_DRAIN_FRONT_END] > 0,
                "{label}: no ray waited for the front kernel when the drain began"
            );
        }
        assert_twins(label, &megakernel, &sorted);
    }
}

/// A class with the optics tables declines: the chunk runs the megakernel's table variant, the
/// frame is the one the switch off gives, bit for bit, the decline is noted and no sorted buffer
/// is made.
#[test]
fn a_class_with_the_optics_tables_stays_on_the_megakernel() {
    let name = "a_class_with_the_optics_tables_stays_on_the_megakernel";
    let Some(mut renderer) = renderer_or_skip(name) else {
        return;
    };
    let material = GemMaterial::diamond();
    let class = classify_material_with_tables(&material, OpticsTables::On);
    if !material_class::uses_optics_tables(renderer.effective_class(class)) {
        println!("skipping {name}: this device runs no table kernels");
        return;
    }
    let off = frame(
        &mut renderer,
        &material,
        class,
        daylight(),
        SortedSwitch::Off,
    );
    let on = frame(
        &mut renderer,
        &material,
        class,
        daylight(),
        SortedSwitch::On(checked()),
    );
    assert_bitwise("tables", &off, &on);
    assert_ne!(
        renderer.sorted_declined() & SortedDecline::OpticsTables.bit(),
        0,
        "the decline was not noted"
    );
    assert!(
        renderer.sorted_counters().is_none(),
        "a declined class made the sorted buffers"
    );
}
