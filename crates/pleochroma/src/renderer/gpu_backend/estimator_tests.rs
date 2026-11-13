//! The estimator a frame asks for (`GpuSceneRef::estimator`): how it resolves, that `None` is the
//! path a build without the field took, and where exit collection is honoured or ignored.
//!
//! The precedence tests need no adapter. The frame tests skip with a printed note when there is
//! none, unless `PLEOCHROMA_REQUIRE_GPU=1` (see `tests::acquire_or_skip`). They rely on the
//! library's process default staying [`Estimator::Branching`] (nothing in this crate's tests
//! changes it): the applications choose exit collection at start-up, the library does not.

use glam::Vec3;

use super::{GpuBackend, GpuPipelineKind, GpuSceneRef, tests::acquire_or_skip};
use crate::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts, tool::ToolPrimitive},
    optics::{
        materials::GemMaterial,
        raytracer::{
            BACKDROP_GREY, Camera, EnvironmentSource, Estimator, LightingPreset, default_estimator,
            environment::TrayMirror,
        },
    },
    renderer::{
        env_map::EnvironmentMap,
        gpu::{
            GpuFrameError, GpuFrameRenderer, GpuFrameScene,
            frame::{
                ChunkCursor, ChunkTurnOutcome, TurnRequest, classify_material, collect_applies,
                resolve_estimator,
            },
        },
    },
};

/// Frame edge in pixels.
const SIZE: u32 = 32;

/// Samples per pixel of every frame.
const SPP: u32 = 16;

/// The fixed scene pieces of a test: camera, stone and material.
struct Fixture {
    camera: Camera,
    planes: Vec<GpuFacetPlane>,
    material: GemMaterial,
}

impl Fixture {
    fn new() -> Self {
        Self {
            camera: Camera::new(0.35, 0.28, 5.0, 18.0),
            planes: StandardGemCuts::standard_round_brilliant(),
            material: GemMaterial::diamond(),
        }
    }

    fn scene<'a>(
        &'a self,
        environment: EnvironmentSource<'a>,
        estimator: Option<Estimator>,
    ) -> GpuSceneRef<'a> {
        GpuSceneRef {
            camera: &self.camera,
            width: SIZE,
            height: SIZE,
            planes: &self.planes,
            facet_finishes: &[],
            material: &self.material,
            max_bounces: 12,
            environment,
            estimator,
        }
    }

    /// The frame of samples `0..SPP` the backend traces for `scene` and `tools`.
    fn frame(
        &self,
        backend: &GpuBackend,
        environment: EnvironmentSource<'_>,
        tools: &[ToolPrimitive],
        estimator: Option<Estimator>,
    ) -> Vec<Vec3> {
        let scene = self.scene(environment, estimator);
        let mut accum = vec![Vec3::ZERO; (SIZE * SIZE) as usize];
        assert!(
            backend.try_accumulate_geom(&scene, tools, 0, SPP, &mut accum),
            "the backend declined a small frame (estimator {estimator:?})"
        );
        assert!(
            accum.iter().any(|pixel| pixel.length_squared() > 0.0),
            "the frame holds no light (estimator {estimator:?})"
        );
        accum
    }

    /// Requires exit collection to change nothing where it cannot apply: the frame asked to
    /// collect equals the frame asked to branch, bit for bit.
    fn assert_collect_is_ignored(
        &self,
        backend: &GpuBackend,
        environment: EnvironmentSource<'_>,
        tools: &[ToolPrimitive],
        what: &str,
    ) {
        let branching = self.frame(backend, environment, tools, Some(Estimator::Branching));
        let collect = self.frame(backend, environment, tools, Some(Estimator::ExitCollection));
        assert_eq!(
            branching, collect,
            "{what}: exit collection must fall back to the branching estimator here"
        );
    }
}

/// A sky whose radiance changes with the azimuth everywhere.
fn sky_map() -> EnvironmentMap {
    let (width, height) = (32_usize, 16_usize);
    let texels = (0..width * height)
        .map(|i| {
            let u = (i % width) as f32 / width as f32;
            let v = (i / width) as f32 / height as f32;
            [0.2 + u, 0.5f32.mul_add(v, 0.3), u.mul_add(v, 0.1)]
        })
        .collect();
    EnvironmentMap::from_rgb(width, height, texels).expect("self-consistent texels")
}

/// A studio rig without a light-sampling technique, over a convex stone: exit collection applies.
const fn studio() -> EnvironmentSource<'static> {
    LightingPreset::RingLights.studio(1.0, 0.85, 0.95)
}

/// The request names its estimator, else the renderer does, else the process does; the legacy
/// bounce wavefront never collects.
#[test]
fn the_asked_estimator_wins_over_the_renderers_over_the_process() {
    use Estimator::{Branching, ExitCollection};
    let mega = GpuPipelineKind::Megakernel;
    for process in [Branching, ExitCollection] {
        for own in [None, Some(Branching), Some(ExitCollection)] {
            for asked in [None, Some(Branching), Some(ExitCollection)] {
                let expected = asked.or(own).unwrap_or(process);
                assert_eq!(
                    resolve_estimator(asked, own, process, mega),
                    expected,
                    "asked {asked:?}, own {own:?}, process {process:?}"
                );
                assert_eq!(
                    resolve_estimator(asked, own, process, GpuPipelineKind::Wavefront),
                    Branching,
                    "the legacy wavefront never collects (asked {asked:?}, own {own:?}, \
                     process {process:?})"
                );
            }
        }
    }
}

/// Exit collection applies over an ideal surface, without a light-sampling technique, for a
/// convex stone and without a Tray Reflection: the rule of `GpuTransportParams::with_estimator`.
#[test]
fn collect_applies_follows_the_transport_rule() {
    use crate::renderer::buffers::{studio_model, transport_env_mode};
    let rig = transport_env_mode::STUDIO_RIG;
    let plain = studio_model::STUDIO;
    assert!(collect_applies(rig, plain, true, 0, false));
    assert!(collect_applies(
        transport_env_mode::UNIFORM_FURNACE,
        plain,
        true,
        0,
        false
    ));
    assert!(!collect_applies(rig, plain, false, 0, false), "surface");
    assert!(!collect_applies(rig, plain, true, 1, false), "tools");
    assert!(
        !collect_applies(rig, plain, true, 0, true),
        "Tray Reflection"
    );
    assert!(
        !collect_applies(transport_env_mode::HDR_MAP, plain, true, 0, false),
        "HDR map"
    );
    assert!(
        !collect_applies(rig, studio_model::DAYLIGHT_SUN, true, 0, false),
        "Daylight + Sun"
    );
    assert!(
        collect_applies(rig, studio_model::DAYLIGHT_DOME, true, 0, false),
        "the daylight dome has no light sampling"
    );
}

/// `Some(Branching)` renders the same as `None`, bit for bit.
#[test]
fn asking_for_branching_equals_asking_for_nothing_bit_for_bit() {
    let Some(backend) =
        acquire_or_skip("asking_for_branching_equals_asking_for_nothing_bit_for_bit")
    else {
        return;
    };
    assert_eq!(
        default_estimator(),
        Estimator::Branching,
        "the library's process default is branching"
    );
    let fixture = Fixture::new();
    let nothing = fixture.frame(&backend, studio(), &[], None);
    let branching = fixture.frame(&backend, studio(), &[], Some(Estimator::Branching));
    assert_eq!(nothing, branching);
}

/// On a convex stone under a studio rig the request's `ExitCollection` reaches the kernel.
#[test]
fn exit_collection_changes_a_convex_studio_frame() {
    let Some(backend) = acquire_or_skip("exit_collection_changes_a_convex_studio_frame") else {
        return;
    };
    let fixture = Fixture::new();
    let branching = fixture.frame(&backend, studio(), &[], Some(Estimator::Branching));
    let collect = fixture.frame(&backend, studio(), &[], Some(Estimator::ExitCollection));
    assert_ne!(branching, collect, "the request's estimator is ignored");
}

/// An HDR map is lit by light sampling, where exit collection does not apply.
#[test]
fn exit_collection_is_ignored_under_an_hdr_map() {
    let Some(backend) = acquire_or_skip("exit_collection_is_ignored_under_an_hdr_map") else {
        return;
    };
    let map = sky_map();
    Fixture::new().assert_collect_is_ignored(&backend, EnvironmentSource::hdr(&map), &[], "HDR");
}

/// The Daylight + Sun rig samples its sun, where exit collection does not apply.
#[test]
fn exit_collection_is_ignored_under_daylight_with_sun() {
    let Some(backend) = acquire_or_skip("exit_collection_is_ignored_under_daylight_with_sun")
    else {
        return;
    };
    Fixture::new().assert_collect_is_ignored(
        &backend,
        LightingPreset::DaylightSun.studio(1.0, 0.85, 0.95),
        &[],
        "Daylight + Sun",
    );
}

/// A stone with tools renders with the branching estimator on every engine.
#[test]
fn exit_collection_is_ignored_for_a_stone_with_tools() {
    let Some(backend) = acquire_or_skip("exit_collection_is_ignored_for_a_stone_with_tools") else {
        return;
    };
    let dimple = ToolPrimitive::ball(Vec3::new(0.2, 0.32, 0.1), 0.25);
    Fixture::new().assert_collect_is_ignored(&backend, studio(), &[dimple], "a stone with tools");
}

/// A collected leaf cannot follow a floor bounce, so a Tray Reflection renders branching.
#[test]
fn exit_collection_is_ignored_with_a_tray_reflection() {
    let Some(backend) = acquire_or_skip("exit_collection_is_ignored_with_a_tray_reflection") else {
        return;
    };
    if !backend.supports_tray_mirror() {
        println!("skipping exit_collection_is_ignored_with_a_tray_reflection: no tray variant");
        return;
    }
    let mirror = TrayMirror::new(-0.9, 0.5).expect("a finite plane and a positive reflectance");
    let environment = studio()
        .with_backdrop(BACKDROP_GREY)
        .with_tray_mirror(Some(mirror));
    assert!(environment.tray_mirror().is_some(), "the tray must be on");
    Fixture::new().assert_collect_is_ignored(&backend, environment, &[], "a Tray Reflection");
}

/// The legacy bounce wavefront never collects: a request that asks it to, where
/// collection applies, is declined (the caller traces the frame on the CPU, which collects);
/// everywhere else it renders, branching.
#[test]
fn the_legacy_wavefront_declines_a_frame_asked_to_collect() {
    let test = "the_legacy_wavefront_declines_a_frame_asked_to_collect";
    let renderer = GpuFrameRenderer::new();
    let mut renderer = match renderer {
        Ok(renderer) => renderer,
        Err(error) => {
            assert!(
                !super::tests::gpu_required(),
                "{test}: PLEOCHROMA_REQUIRE_GPU=1 but no usable GPU adapter: {error}"
            );
            println!("skipping {test}: no GPU adapter");
            return;
        }
    };
    renderer.set_pipeline_kind(GpuPipelineKind::Wavefront);
    if renderer.pipeline_kind() != GpuPipelineKind::Wavefront {
        println!("skipping {test}: this device stays on the megakernel");
        return;
    }
    let fixture = Fixture::new();
    let map = sky_map();
    let mut turn = |environment: EnvironmentSource<'_>, estimator: Option<Estimator>| {
        let scene = GpuFrameScene {
            camera: &fixture.camera,
            width: SIZE,
            height: SIZE,
            planes: &fixture.planes,
            facet_finishes: &[],
            material: &fixture.material,
            max_bounces: 4,
            environment,
        };
        let request = TurnRequest {
            scene: &scene,
            tools: &[],
            pipeline_class: classify_material(&fixture.material),
            sample_offset: 0,
            spp: 1,
            cancel: None,
            max_chunks: usize::MAX,
            surface: None,
            estimator,
        };
        let mut accum = vec![Vec3::ZERO; (SIZE * SIZE) as usize];
        renderer.accumulate_turn(&request, &mut accum, &mut ChunkCursor::default())
    };
    let declined = turn(studio(), Some(Estimator::ExitCollection));
    assert!(
        matches!(declined, Err(GpuFrameError::LegacyWavefrontCollect)),
        "expected LegacyWavefrontCollect, got {declined:?}"
    );
    for (what, environment, estimator) in [
        ("no estimator asked", studio(), None),
        ("branching asked", studio(), Some(Estimator::Branching)),
        (
            "collect asked under an HDR map, where it does not apply",
            EnvironmentSource::hdr(&map),
            Some(Estimator::ExitCollection),
        ),
    ] {
        let outcome = turn(environment, estimator);
        assert!(
            matches!(outcome, Ok(ChunkTurnOutcome::Done)),
            "{what}: expected the frame to render, got {outcome:?}"
        );
    }
}
