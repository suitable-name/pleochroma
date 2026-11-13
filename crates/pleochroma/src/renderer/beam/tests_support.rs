//! Test rigs for the beam tracer: scenes, the Pentalume planes, `dump_svg`.
//!
//! The rigs mirror the path cache's `Rig` (a stone, its intersection arena, its material and a
//! camera) for the three stones the beam tests use: the unit cube, the standard round brilliant
//! and the Pentalume Sapphire of the benchmark scene S2. The frame builders give the 16, 32 and
//! 64 pixel square test frames, and [`dump_svg`] writes polygons to the temp directory for a
//! failing test to leave behind. Never call [`dump_svg`] in a passing test.

use std::fmt::Write as _;

use glam::{DVec2, DVec4, Vec3};

use crate::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, DEFAULT_FOV_DEG, Ray, build_plane_soa},
    },
    renderer::{frame_scene::FrameScene, test_stats::smooth_environment},
    simd::PlanesSoA32,
};

/// The side lengths in pixels of the square test frames.
pub const FRAME_SIZES: [u32; 3] = [16, 32, 64];

/// The bounce limit of the test frames.
pub const MAX_BOUNCES: u32 = 12;

/// Shows the beam tracer's debug log on the test output; with `--nocapture` it streams live.
///
/// The filter is `pleochroma::beam=debug` (a line per primary beam of a tile, per traced and per
/// finished batch), or the value of `PLEOCHROMA_BEAM_LOG` when set. Only the first call in a test
/// binary installs it; later calls do nothing.
pub fn show_beam_log() {
    let filter = std::env::var("PLEOCHROMA_BEAM_LOG")
        .unwrap_or_else(|_| "pleochroma::beam=debug".to_owned());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        .with_test_writer()
        .try_init();
}

/// The planes of the Pentalume Sapphire as `[nx, ny, nz, d]` (normal and offset).
///
/// Copied from `benchmarks/render_accel/jobs/s2_sapphire.job.json`, key `scene.planes`, in file
/// order (57 planes). The values are the job file's numbers: single precision values written
/// with the digits of their double precision expansion, so the conversion with
/// [`DVec4::as_vec4`] gives back exactly the `f32` the job loads.
pub const PENTALUME_PLANES: &[[f64; 4]] = &[
    [1.0, 0.0, 0.0, -1.25],
    [-1.0, 0.0, 0.0, -1.25],
    [0.0, 0.0, 1.0, -1.25],
    [0.0, 0.0, -1.0, -1.25],
    [0.0, 1.0, 0.0, -0.369_875_222_444_534_3],
    [0.0, -1.0, 0.0, -0.994_890_332_221_984_9],
    [
        0.681_998_372_077_941_9,
        -0.731_353_759_765_625,
        0.0,
        -0.713_080_883_026_123,
    ],
    [
        0.210_749_074_816_703_8,
        -0.731_353_759_765_625,
        0.648_618_996_143_341_1,
        -0.713_080_883_026_123,
    ],
    [
        -0.551_748_275_756_835_9,
        -0.731_353_759_765_625,
        0.400_868_564_844_131_47,
        -0.713_080_883_026_123,
    ],
    [
        -0.551_748_275_756_835_9,
        -0.731_353_759_765_625,
        -0.400_868_624_448_776_25,
        -0.713_080_883_026_123,
    ],
    [
        0.210_749_149_322_509_77,
        -0.731_353_700_160_980_2,
        -0.648_618_936_538_696_3,
        -0.713_080_883_026_123,
    ],
    [
        0.525_414_526_462_554_9,
        -0.760_405_957_698_822,
        0.381_735_980_510_711_67,
        -0.766_193_568_706_512_5,
    ],
    [
        -0.200_690_507_888_793_95,
        -0.760_405_957_698_822,
        0.617_661_774_158_477_8,
        -0.766_193_568_706_512_5,
    ],
    [
        -0.649_448_037_147_522,
        -0.760_405_957_698_822,
        -5.677_654_968_394_563e-08,
        -0.766_193_568_706_512_5,
    ],
    [
        -0.200_690_284_371_376_04,
        -0.760_406_017_303_466_8,
        -0.617_661_893_367_767_3,
        -0.766_193_568_706_512_5,
    ],
    [
        0.525_414_466_857_910_2,
        -0.760_405_957_698_822,
        -0.381_736_010_313_034_06,
        -0.766_193_568_706_512_5,
    ],
    [
        0.660_659_432_411_193_8,
        -0.719_339_787_960_052_5,
        -0.214_661_210_775_375_37,
        -0.795_507_311_820_983_9,
    ],
    [
        0.660_659_432_411_193_8,
        -0.719_339_787_960_052_5,
        0.214_661_255_478_858_95,
        -0.795_507_311_820_983_9,
    ],
    [
        0.408_309_966_325_759_9,
        -0.719_339_787_960_052_5,
        0.561_990_439_891_815_2,
        -0.795_507_311_820_983_9,
    ],
    [
        -3.036_448_248_394_663e-08,
        -0.719_339_787_960_052_5,
        0.694_658_398_628_234_9,
        -0.795_507_311_820_983_9,
    ],
    [
        -0.408_310_085_535_049_44,
        -0.719_339_787_960_052_5,
        0.561_990_380_287_170_4,
        -0.795_507_311_820_983_9,
    ],
    [
        -0.660_659_372_806_549_1,
        -0.719_339_787_960_052_5,
        0.214_661_270_380_020_14,
        -0.795_507_311_820_983_9,
    ],
    [
        -0.660_659_313_201_904_3,
        -0.719_339_787_960_052_5,
        -0.214_661_404_490_470_9,
        -0.795_507_311_820_983_9,
    ],
    [
        -0.408_309_817_314_147_95,
        -0.719_339_787_960_052_5,
        -0.561_990_559_101_104_7,
        -0.795_507_311_820_983_9,
    ],
    [
        8.283_718_599_955_137e-09,
        -0.719_339_787_960_052_5,
        -0.694_658_398_628_234_9,
        -0.795_507_311_820_983_9,
    ],
    [
        0.408_310_115_337_371_8,
        -0.719_339_787_960_052_5,
        -0.561_990_320_682_525_6,
        -0.795_507_311_820_983_9,
    ],
    [1.0, -4.371_138_828_673_793e-08, 0.0, -1.0],
    [
        0.309_016_972_780_227_66,
        -4.371_138_828_673_793e-08,
        0.951_056_540_012_359_6,
        -1.0,
    ],
    [
        -0.809_017_002_582_55,
        -4.371_138_828_673_793e-08,
        0.587_785_243_988_037_1,
        -1.0,
    ],
    [
        -0.809_016_942_977_905_3,
        -4.371_138_828_673_793e-08,
        -0.587_785_303_592_681_9,
        -1.0,
    ],
    [
        0.309_017_121_791_839_6,
        -4.371_138_828_673_793e-08,
        -0.951_056_480_407_714_8,
        -1.0,
    ],
    [
        0.809_017_002_582_55,
        -4.371_138_828_673_793e-08,
        0.587_785_243_988_037_1,
        -1.129_999_995_231_628_4,
    ],
    [
        -0.309_017_062_187_194_8,
        -4.371_139_183_945_161e-08,
        0.951_056_540_012_359_6,
        -1.129_999_995_231_628_4,
    ],
    [
        -1.0,
        -4.371_138_828_673_793e-08,
        -8.742_277_657_347_586e-08,
        -1.129_999_995_231_628_4,
    ],
    [
        -0.309_016_704_559_326_17,
        -4.371_139_183_945_161e-08,
        -0.951_056_659_221_649_2,
        -1.129_999_995_231_628_4,
    ],
    [
        0.809_016_942_977_905_3,
        -4.371_138_828_673_793e-08,
        -0.587_785_303_592_681_9,
        -1.129_999_995_231_628_4,
    ],
    [
        0.559_192_895_889_282_2,
        0.829_037_606_716_156,
        0.0,
        -0.594_426_989_555_358_9,
    ],
    [
        0.172_800_093_889_236_45,
        0.829_037_606_716_156,
        0.531_824_052_333_831_8,
        -0.594_426_989_555_358_9,
    ],
    [
        -0.452_396_571_636_199_95,
        0.829_037_606_716_156,
        0.328_685_343_265_533_45,
        -0.594_426_989_555_358_9,
    ],
    [
        -0.452_396_541_833_877_56,
        0.829_037_606_716_156,
        -0.328_685_373_067_855_83,
        -0.594_426_989_555_358_9,
    ],
    [
        0.172_800_183_296_203_6,
        0.829_037_606_716_156,
        -0.531_824_052_333_831_8,
        -0.594_426_989_555_358_9,
    ],
    [
        0.404_508_501_291_275,
        0.866_025_388_240_814_2,
        0.293_892_621_994_018_55,
        -0.601_806_104_183_197,
    ],
    [
        -0.154_508_531_093_597_4,
        0.866_025_447_845_459,
        0.475_528_270_006_179_8,
        -0.601_806_104_183_197,
    ],
    [
        -0.5,
        0.866_025_388_240_814_2,
        -4.371_138_828_673_793e-08,
        -0.601_806_104_183_197,
    ],
    [
        -0.154_508_337_378_501_9,
        0.866_025_388_240_814_2,
        -0.475_528_299_808_502_2,
        -0.601_806_104_183_197,
    ],
    [
        0.404_508_471_488_952_64,
        0.866_025_388_240_814_2,
        -0.293_892_651_796_340_94,
        -0.601_806_104_183_197,
    ],
    [
        0.517_982_542_514_801,
        0.838_670_551_776_886,
        -0.168_302_699_923_515_32,
        -0.634_631_514_549_255_4,
    ],
    [
        0.517_982_542_514_801,
        0.838_670_551_776_886,
        0.168_302_729_725_837_7,
        -0.634_631_514_549_255_4,
    ],
    [
        0.320_130_795_240_402_2,
        0.838_670_551_776_886,
        0.440_622_240_304_946_9,
        -0.634_631_514_549_255_4,
    ],
    [
        -2.380_692_976_089_449e-08,
        0.838_670_551_776_886,
        0.544_639_050_960_540_8,
        -0.634_631_514_549_255_4,
    ],
    [
        -0.320_130_884_647_369_4,
        0.838_670_551_776_886,
        0.440_622_180_700_302_1,
        -0.634_631_514_549_255_4,
    ],
    [
        -0.517_982_542_514_801,
        0.838_670_611_381_530_8,
        0.168_302_759_528_160_1,
        -0.634_631_514_549_255_4,
    ],
    [
        -0.517_982_482_910_156_2,
        0.838_670_551_776_886,
        -0.168_302_834_033_966_06,
        -0.634_631_514_549_255_4,
    ],
    [
        -0.320_130_705_833_435_06,
        0.838_670_551_776_886,
        -0.440_622_329_711_914_06,
        -0.634_631_514_549_255_4,
    ],
    [
        6.494_755_844_244_082e-09,
        0.838_670_551_776_886,
        -0.544_639_050_960_540_8,
        -0.634_631_514_549_255_4,
    ],
    [
        0.320_130_914_449_691_8,
        0.838_670_551_776_886,
        -0.440_622_150_897_979_74,
        -0.634_631_514_549_255_4,
    ],
    [0.0, 1.0, 0.0, -0.349_999_994_039_535_5],
];

/// The camera distance of the Pentalume pose of scene S2.
pub const PENTALUME_DISTANCE: f32 = 3.625_719_5;

/// The roll of the Pentalume pose of scene S2 in radians (`camera_roll`, 0.1 pi).
pub const PENTALUME_ROLL: f32 = std::f32::consts::PI * 0.1;

/// A stone, its intersection arena, its material and a camera.
pub struct BeamRig {
    /// The facet planes.
    pub planes: Vec<GpuFacetPlane>,
    /// The planes in the intersector's layout.
    pub soa: PlanesSoA32,
    /// The stone's material.
    pub material: GemMaterial,
    /// The camera of the test pose.
    pub camera: Camera,
}

impl BeamRig {
    /// A rig from its parts; the intersection arena is built from `planes`.
    #[must_use]
    pub fn new(planes: Vec<GpuFacetPlane>, material: GemMaterial, camera: Camera) -> Self {
        let soa = build_plane_soa(&planes);
        Self {
            planes,
            soa,
            material,
            camera,
        }
    }

    /// The unit cube seen from straight above, smaller than the frame.
    #[must_use]
    pub fn cube(material: GemMaterial) -> Self {
        Self::new(
            cube_planes(),
            material,
            Camera::new(0.0, std::f32::consts::FRAC_PI_2, 8.0, 24.0),
        )
    }

    /// The standard round brilliant seen from the default test pose.
    #[must_use]
    pub fn brilliant(material: GemMaterial) -> Self {
        Self::new(
            StandardGemCuts::standard_round_brilliant(),
            material,
            Camera::new(0.35, 0.28, 5.0, 18.0),
        )
    }

    /// The Pentalume Sapphire planes seen from the table, in the pose of scene S2.
    #[must_use]
    pub fn pentalume(material: GemMaterial) -> Self {
        Self::new(
            pentalume_planes(),
            material,
            Camera::new(
                0.0,
                std::f32::consts::FRAC_PI_2,
                PENTALUME_DISTANCE,
                DEFAULT_FOV_DEG,
            )
            .with_roll(PENTALUME_ROLL),
        )
    }

    /// A square frame of `size` pixels under the smooth studio environment.
    #[must_use]
    pub fn scene(&self, size: u32, max_bounces: u32) -> FrameScene<'_> {
        FrameScene {
            camera: &self.camera,
            width: size,
            height: size,
            planes: &self.planes,
            facet_finishes: &[],
            material: &self.material,
            max_bounces,
            environment: smooth_environment(),
        }
    }

    /// The ray through the centre of pixel `(x, y)` of the square frame of `size` pixels.
    #[must_use]
    pub fn pixel_ray(&self, size: u32, (x, y): (u32, u32)) -> Ray {
        self.camera
            .generate_ray(x as f32, y as f32, size as f32, size as f32, 0.0, 0.0)
    }
}

/// The planes of the unit cube.
#[must_use]
pub fn cube_planes() -> Vec<GpuFacetPlane> {
    [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ]
    .iter()
    .map(|&normal| GpuFacetPlane::new(normal, -1.0))
    .collect()
}

/// The planes of the Pentalume Sapphire, from [`PENTALUME_PLANES`].
#[must_use]
pub fn pentalume_planes() -> Vec<GpuFacetPlane> {
    PENTALUME_PLANES
        .iter()
        .map(|&[nx, ny, nz, d]| {
            let plane = DVec4::new(nx, ny, nz, d).as_vec4();
            GpuFacetPlane::new(plane.truncate(), plane.w)
        })
        .collect()
}

/// A colorless, non-dispersive isotropic glass.
#[must_use]
pub fn glass() -> GemMaterial {
    GemMaterial::new_custom("beam test glass", 1.6, 0.0, 0.0, [0.0; 3])
}

/// A dispersive, diamond-like isotropic material.
#[must_use]
pub fn diamond() -> GemMaterial {
    GemMaterial::new_custom("beam test diamond", 2.417, 0.0256, 0.0, [0.0; 3])
}

/// A uniaxial, sapphire-like material.
#[must_use]
pub fn sapphire() -> GemMaterial {
    GemMaterial::new_custom("beam test sapphire", 1.768, 0.0106, 0.008, [0.0; 3])
}

/// The D1 branch filter: the signatures of every first-surface reflection.
///
/// There is one signature per original facet index `0..facets`. A reference sample whose signature
/// is in this set is a depth-1 reflection; the `kept` callback `|sig, _| sig_in(&set, sig)`
/// removes exactly those branches, so `reference(None) − reference(keep_d1)` is the D1 part the
/// beams must equal (§16.2).
#[must_use]
pub fn d1_reflection_signatures(facets: u32) -> Vec<u64> {
    use crate::renderer::path_cache::{
        bounce::EventKind,
        signature::{BRANCH_SIG_SEED, branch_step, pack_sig},
    };
    let mut sigs: Vec<u64> = (0..facets)
        .map(|f| pack_sig(branch_step(BRANCH_SIG_SEED, f, EventKind::Reflect, false)))
        .collect();
    sigs.sort_unstable();
    sigs
}

/// `true` when `sig` is in the sorted `set` (for a `kept` callback built from
/// [`d1_reflection_signatures`]).
#[must_use]
pub fn sig_in(set: &[u64], sig: u64) -> bool {
    set.binary_search(&sig).is_ok()
}

/// The SVG text of `polygons`: one closed outline each, the view box fitted to all of them.
#[must_use]
pub fn svg_text(polygons: &[Vec<DVec2>]) -> String {
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for point in polygons.iter().flatten() {
        lo = lo.min(*point);
        hi = hi.max(*point);
    }
    if lo.x > hi.x {
        lo = DVec2::ZERO;
        hi = DVec2::ONE;
    }
    let extent = (hi - lo).max(DVec2::splat(1e-9));
    let margin = 0.02 * extent.max_element();
    let stroke = 0.002 * extent.max_element();
    let mut text = String::new();
    let _ = writeln!(
        text,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{} {} {} {}\">",
        lo.x - margin,
        lo.y - margin,
        2.0f64.mul_add(margin, extent.x),
        2.0f64.mul_add(margin, extent.y),
    );
    for polygon in polygons {
        let points: Vec<String> = polygon.iter().map(|p| format!("{},{}", p.x, p.y)).collect();
        let _ = writeln!(
            text,
            "<polygon points=\"{}\" fill=\"none\" stroke=\"black\" stroke-width=\"{stroke}\"/>",
            points.join(" ")
        );
    }
    text.push_str("</svg>\n");
    text
}

/// Writes `polygons` to `beam_<name>.svg` in the temp directory, for a failing test.
///
/// Characters of `name` that are not ASCII letters, digits, `-` or `_` become `_`. A write
/// failure is reported on stderr and otherwise ignored. Never call this in a passing test.
pub fn dump_svg(name: &str, polygons: &[Vec<DVec2>]) {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = std::env::temp_dir().join(format!("beam_{clean}.svg"));
    match std::fs::write(&path, svg_text(polygons)) {
        Ok(()) => eprintln!("beam test: wrote {}", path.display()),
        Err(error) => eprintln!("beam test: could not write {}: {error}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use glam::DVec2;

    use super::{
        BeamRig, FRAME_SIZES, MAX_BOUNCES, PENTALUME_DISTANCE, PENTALUME_PLANES, cube_planes,
        diamond, glass, pentalume_planes, sapphire, svg_text,
    };

    /// The distance of the line of `ray` from the origin.
    fn miss_distance(ray: crate::optics::raytracer::Ray) -> f32 {
        ray.origin.cross(ray.dir).length()
    }

    #[test]
    fn the_pentalume_literal_has_the_planes_of_the_job_file() {
        assert_eq!(PENTALUME_PLANES.len(), 57);
        assert_eq!(pentalume_planes().len(), 57);
        // Unit normals (single precision rounding of the job file).
        for [nx, ny, nz, _] in PENTALUME_PLANES {
            let length = (nx * nx + ny * ny + nz * nz).sqrt();
            assert!((length - 1.0).abs() < 1e-6, "normal length {length}");
        }
        // The last plane is the table at d = -0.35 and the first the +X girdle plane.
        assert_eq!(
            PENTALUME_PLANES[56],
            [0.0, 1.0, 0.0, -0.349_999_994_039_535_5]
        );
        assert_eq!(PENTALUME_PLANES[0], [1.0, 0.0, 0.0, -1.25]);
    }

    #[test]
    fn the_rigs_have_their_planes_and_look_at_the_origin() {
        let cube = BeamRig::cube(glass());
        let brilliant = BeamRig::brilliant(diamond());
        let pentalume = BeamRig::pentalume(sapphire());
        assert_eq!(cube_planes().len(), 6);
        assert_eq!(cube.planes.len(), 6);
        assert!(brilliant.planes.len() > 20);
        assert_eq!(pentalume.planes.len(), 57);
        for rig in [&cube, &brilliant, &pentalume] {
            // The ray through the exact centre of the frame passes through the origin.
            let ray = rig.pixel_ray(16, (8, 8));
            assert!(miss_distance(ray) < 1e-3, "miss {}", miss_distance(ray));
        }
        let distance = pentalume.camera.origin.length();
        assert!(
            (distance - PENTALUME_DISTANCE).abs() < 1e-4,
            "distance {distance}"
        );
    }

    #[test]
    fn the_frames_have_the_requested_size() {
        let rig = BeamRig::brilliant(glass());
        for size in FRAME_SIZES {
            let scene = rig.scene(size, MAX_BOUNCES);
            assert_eq!((scene.width, scene.height), (size, size));
            assert_eq!(scene.max_bounces, MAX_BOUNCES);
            assert_eq!(scene.planes.len(), rig.planes.len());
        }
        assert_eq!(FRAME_SIZES, [16, 32, 64]);
    }

    #[test]
    fn the_svg_text_has_one_polygon_per_outline_and_a_view_box() {
        let square = vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(2.0, 0.0),
            DVec2::new(2.0, 2.0),
            DVec2::new(0.0, 2.0),
        ];
        let triangle = vec![
            DVec2::new(0.5, 0.5),
            DVec2::new(1.5, 0.5),
            DVec2::new(1.0, 1.5),
        ];
        let text = svg_text(&[square, triangle]);
        assert!(text.starts_with("<svg "), "{text}");
        assert!(text.contains("viewBox="), "{text}");
        assert_eq!(text.matches("<polygon ").count(), 2);
        assert!(text.contains("points=\"0,0 2,0 2,2 0,2\""), "{text}");
        assert!(text.trim_end().ends_with("</svg>"));
        // No polygons: still a well-formed document.
        let empty = svg_text(&[]);
        assert_eq!(empty.matches("<polygon ").count(), 0);
        assert!(empty.contains("viewBox="));
    }
}
