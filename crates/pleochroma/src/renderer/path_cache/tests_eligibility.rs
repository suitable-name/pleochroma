//! Eligibility: a stone the cache cannot reproduce gets no cache, and the render is then the path
//! tracer's, byte for byte.

use glam::Vec3;

use super::{
    CacheOptions, PathCacheFrame, PathCacheOff, StoneTraits, check_eligibility,
    tests_support::{Rig, glass},
};
use crate::{
    geometry::tool::ToolPrimitive,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            FacetFinish, pixel_rotations,
            surface::{EdgeAbrasion, FacetSurface, SurfaceScene},
        },
    },
    renderer::{
        adaptive::ConvergenceMap,
        cpu_frame::{
            trace_live_tiles_surface, trace_live_tiles_surface_cached,
            trace_pixels_interleaved_surface, trace_pixels_interleaved_surface_cached,
        },
        frame_scene::FrameScene,
    },
};

const SIZE: u32 = 8;

fn plain_traits(material: &GemMaterial) -> StoneTraits<'_> {
    StoneTraits {
        material,
        facet_finishes: &[],
        tools: &[],
        fluorescence: Fluorescence::none(),
        surface: SurfaceScene::IDEAL,
        depth_of_field: false,
    }
}

fn bits(pixels: &[Vec3]) -> Vec<[u32; 3]> {
    pixels
        .iter()
        .map(|pixel| pixel.to_array().map(f32::to_bits))
        .collect()
}

#[test]
fn a_plain_stone_is_eligible() {
    for name in [
        "Diamond", "Sapphire", "Ruby", "Emerald", "Amethyst", "Zircon",
    ] {
        let material = GemMaterial::by_name(name).expect("a built-in material");
        assert_eq!(
            check_eligibility(&plain_traits(&material)),
            Ok(()),
            "{name}"
        );
    }
    assert_eq!(check_eligibility(&plain_traits(&glass())), Ok(()));
}

#[test]
fn the_opal_family_is_not() {
    for name in [
        "Precious Opal (White)",
        "Precious Opal (Black)",
        "Labradorite",
        "Moonstone",
    ] {
        let material = GemMaterial::by_name(name).expect("a built-in material");
        assert!(
            check_eligibility(&plain_traits(&material)).is_err(),
            "{name} has a look the cache cannot reproduce"
        );
    }
}

#[test]
fn a_scattering_medium_is_not() {
    let hazy = glass().with_scattering(0.4, 0.3);
    assert_eq!(
        check_eligibility(&plain_traits(&hazy)),
        Err(PathCacheOff::ScatteringMedium)
    );
}

#[test]
fn every_reason_has_its_own_sentence() {
    let reasons = [
        PathCacheOff::ScatteringMedium,
        PathCacheOff::OrientedInclusions,
        PathCacheOff::MaterialLook,
        PathCacheOff::FrostedFacets,
        PathCacheOff::RoughPolish,
        PathCacheOff::AbradedEdges,
        PathCacheOff::ConcaveTiers,
        PathCacheOff::Fluorescence,
        PathCacheOff::DepthOfField,
        PathCacheOff::ZonedColor,
        PathCacheOff::TrayReflection,
    ];
    let mut seen = Vec::new();
    for reason in reasons {
        let text = reason.message();
        assert!(text.starts_with("Path cache off: "), "{text}");
        assert_eq!(reason.to_string(), text);
        assert!(!seen.contains(&text), "{text} is used twice");
        seen.push(text);
    }
    assert_eq!(
        PathCacheOff::ScatteringMedium.message(),
        "Path cache off: the material scatters light"
    );
}

#[test]
fn frosted_rough_abraded_concave_and_depth_of_field_are_each_named() {
    let material = glass();
    let finishes = [FacetFinish::Polished, FacetFinish::Frosted];
    let frosted = StoneTraits {
        facet_finishes: &finishes,
        ..plain_traits(&material)
    };
    assert_eq!(
        check_eligibility(&frosted),
        Err(PathCacheOff::FrostedFacets)
    );

    let ground = [FacetSurface::IDEAL, FacetSurface::ground(0.3, 1.0)];
    let ground_scene = SurfaceScene {
        facet_surfaces: &ground,
        ..SurfaceScene::IDEAL
    };
    let ground_traits = StoneTraits {
        surface: ground_scene,
        ..plain_traits(&material)
    };
    assert_eq!(
        check_eligibility(&ground_traits),
        Err(PathCacheOff::FrostedFacets)
    );

    let polished = [FacetSurface::polished(4.0, 1.0)];
    let rough_traits = StoneTraits {
        surface: SurfaceScene {
            facet_surfaces: &polished,
            ..SurfaceScene::IDEAL
        },
        ..plain_traits(&material)
    };
    assert_eq!(
        check_eligibility(&rough_traits),
        Err(PathCacheOff::RoughPolish)
    );

    let ideal = [FacetSurface::IDEAL; 4];
    let ideal_traits = StoneTraits {
        surface: SurfaceScene {
            facet_surfaces: &ideal,
            ..SurfaceScene::IDEAL
        },
        ..plain_traits(&material)
    };
    assert_eq!(check_eligibility(&ideal_traits), Ok(()));

    let abraded = StoneTraits {
        surface: SurfaceScene {
            edge: EdgeAbrasion {
                width: 0.05,
                alpha: 0.2,
                halo_rq_nm: 0.0,
            },
            ..SurfaceScene::IDEAL
        },
        ..plain_traits(&material)
    };
    assert_eq!(check_eligibility(&abraded), Err(PathCacheOff::AbradedEdges));

    let tools = [ToolPrimitive::ball(Vec3::new(0.0, 0.5, 0.0), 0.2)];
    let concave = StoneTraits {
        tools: &tools,
        ..plain_traits(&material)
    };
    assert_eq!(check_eligibility(&concave), Err(PathCacheOff::ConcaveTiers));

    let blurred = StoneTraits {
        depth_of_field: true,
        ..plain_traits(&material)
    };
    assert_eq!(check_eligibility(&blurred), Err(PathCacheOff::DepthOfField));
}

/// The frame of the rig with the given finishes.
fn scene_with<'a>(
    rig: &'a Rig,
    finishes: &'a [FacetFinish],
    material: &'a GemMaterial,
) -> FrameScene<'a> {
    FrameScene {
        facet_finishes: finishes,
        material,
        ..rig.scene(SIZE, 8)
    }
}

/// Asserts that the cache build is refused with `expected`, and that the cached entry points with
/// no cache equal the plain ones bit for bit.
fn assert_no_cache_and_identical(
    scene: &FrameScene<'_>,
    rig: &Rig,
    extras: (&[ToolPrimitive], &Fluorescence, SurfaceScene<'_>),
    expected: PathCacheOff,
    label: &str,
) {
    let built = PathCacheFrame::new(scene, &rig.soa, extras, CacheOptions::default());
    assert_eq!(built.as_ref().err(), Some(&expected), "{label}");
    let cache = built.ok();
    assert!(cache.is_none(), "{label}: no frame is built");

    let plain = trace_pixels_interleaved_surface(scene, extras, &rig.soa, 0, 1, 0, 3);
    let off = trace_pixels_interleaved_surface_cached(
        scene,
        extras,
        &rig.soa,
        (0, 1),
        (0, 3),
        cache.as_ref(),
    );
    assert_eq!(bits(&plain), bits(&off), "{label}: full-frame samples");
    assert!(
        plain.iter().any(|pixel| pixel.max_element() > 0.0),
        "{label}: a lit frame"
    );
}

#[test]
fn opal_frosted_and_concave_stones_get_no_cache_and_render_as_if_off() {
    let none: &[ToolPrimitive] = &[];
    let fluorescence = Fluorescence::none();

    let opal = GemMaterial::by_name("Precious Opal (White)").expect("a built-in material");
    let rig = Rig::brilliant(opal);
    let scene = rig.scene(SIZE, 8);
    assert_no_cache_and_identical(
        &scene,
        &rig,
        (none, fluorescence, SurfaceScene::IDEAL),
        PathCacheOff::MaterialLook,
        "opal",
    );

    let rig = Rig::brilliant(glass());
    let finishes = vec![FacetFinish::Frosted; rig.planes.len()];
    let material = rig.material.clone();
    let scene = scene_with(&rig, &finishes, &material);
    assert_no_cache_and_identical(
        &scene,
        &rig,
        (none, fluorescence, SurfaceScene::IDEAL),
        PathCacheOff::FrostedFacets,
        "frosted",
    );

    let scene = rig.scene(SIZE, 8);
    let tools = [ToolPrimitive::ball(Vec3::new(0.0, 0.3, 0.0), 0.25)];
    assert_no_cache_and_identical(
        &scene,
        &rig,
        (&tools, fluorescence, SurfaceScene::IDEAL),
        PathCacheOff::ConcaveTiers,
        "concave",
    );
}

#[test]
fn with_no_cache_the_live_pass_is_the_plain_pass_bit_for_bit() {
    let rig = Rig::brilliant(glass());
    let scene = rig.scene(12, 8);
    let extras = (&[][..], Fluorescence::none(), SurfaceScene::IDEAL);
    let live = ConvergenceMap::new(12, 12, 4).live_tiles();
    let pixels = 12 * 12;

    let (mut sum_plain, mut half_plain) = (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
    trace_live_tiles_surface(
        &scene,
        extras,
        &rig.soa,
        &live,
        3,
        4,
        &mut sum_plain,
        &mut half_plain,
    );
    let (mut sum_off, mut half_off) = (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
    trace_live_tiles_surface_cached(
        &scene,
        extras,
        &rig.soa,
        &live,
        (3, 4),
        (&mut sum_off, &mut half_off),
        None,
    );
    assert_eq!(bits(&sum_plain), bits(&sum_off));
    assert_eq!(bits(&half_plain), bits(&half_off));

    // A cache built for another frame size is ignored the same way.
    let other = rig.scene(8, 8);
    let wrong = PathCacheFrame::new(&other, &rig.soa, extras, CacheOptions::default())
        .expect("the glass stone is eligible");
    let (mut sum_wrong, mut half_wrong) = (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
    trace_live_tiles_surface_cached(
        &scene,
        extras,
        &rig.soa,
        &live,
        (3, 4),
        (&mut sum_wrong, &mut half_wrong),
        Some(&wrong),
    );
    assert_eq!(bits(&sum_plain), bits(&sum_wrong));
    assert_eq!(bits(&half_plain), bits(&half_wrong));
}

#[test]
fn an_eligible_stone_builds_tiles_only_when_a_pixel_of_them_is_shaded() {
    let rig = Rig::brilliant(glass());
    let scene = rig.scene(SIZE, 8);
    let extras = (&[][..], Fluorescence::none(), SurfaceScene::IDEAL);
    let options = CacheOptions {
        budget_bytes: Some(1 << 20),
        ..CacheOptions::default()
    };
    let frame = PathCacheFrame::new(&scene, &rig.soa, extras, options)
        .expect("the glass stone is eligible");
    assert!(frame.matches(SIZE, SIZE));
    assert!(!frame.matches(SIZE + 1, SIZE));
    assert_eq!(frame.budget_bytes(), Some(1 << 20));
    assert_eq!(frame.tile_count(), 1, "an 8 x 8 frame is one 16 x 16 tile");
    assert_eq!(frame.built_tiles(), 0);
    let rot = pixel_rotations(4 * SIZE + 4);
    let _ = frame.shade_sample((4, 4), 0, &rot);
    assert_eq!(frame.built_tiles(), 1);
    assert!(frame.built_bytes() > 0);
    // A pixel off the frame is never shaded and builds nothing.
    assert!(frame.shade_sample((SIZE, 0), 0, &rot).is_none());
    assert!(frame.tile(1).is_none());
}
