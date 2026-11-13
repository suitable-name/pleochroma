//! Illuminant-dependent color shift tests.
//!
//! Ruby and alexandrite must render measurably redder under warm incandescent light than
//! under D65 daylight, driven by their narrow chromophore transmission windows.

use glam::Vec3;
use pleochroma::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{
        materials::GemMaterial,
        raytracer::{LightingPreset, Ray, hash_u32, trace_spectral_ray},
    },
};

/// Returns the CIE xy chromaticity of `material` under `lighting_preset`, averaged over
/// `samples` spectral samples through the same fixed ray.
///
/// Averaging suppresses Monte Carlo noise, so the comparison isolates the illuminant-driven
/// shift rather than sampling variance.
fn render_chromaticity_under_preset(
    material: &GemMaterial,
    planes: &[GpuFacetPlane],
    ray: Ray,
    lighting_preset: LightingPreset,
    samples: u32,
    seed_salt: u32,
) -> (f32, f32) {
    let mut xyz_sum = Vec3::ZERO;
    for i in 0..samples {
        let seed = hash_u32(seed_salt ^ hash_u32(i ^ 0x9E37_79B9));
        xyz_sum += trace_spectral_ray(
            ray,
            planes,
            material,
            12,
            lighting_preset.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
    }
    let xyz_avg = xyz_sum / samples as f32;
    let sum = xyz_avg.x + xyz_avg.y + xyz_avg.z;
    (xyz_avg.x / sum, xyz_avg.y / sum)
}

/// Real ruby renders noticeably REDDER under warm incandescent (3200 K) light than under
/// daylight (D65). Tungsten emits little blue, where ruby's narrower transmission window sits
/// (the Cr3+ bands at 410 nm and 550 nm, cited on the Ruby entry in
/// `GemMaterial::all_materials`).
/// Daylight's stronger blue lets more of that window through, so its color sits slightly
/// further toward blue. A model of broad fixed lobes has no such narrow window and cannot
/// reproduce the shift, so this test checks banded absorption beyond a bare R > B comparison.
#[test]
fn ruby_shifts_redder_under_incandescent_than_d65() {
    const SAMPLES: u32 = 192;

    let planes = StandardGemCuts::emerald_cut();
    let ruby = GemMaterial::ruby();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };

    let (x_d65, y_d65) = render_chromaticity_under_preset(
        &ruby,
        &planes,
        ray,
        LightingPreset::Daylight,
        SAMPLES,
        0xA5A5_0001,
    );
    let (x_inc, y_inc) = render_chromaticity_under_preset(
        &ruby,
        &planes,
        ray,
        LightingPreset::Incandescent,
        SAMPLES,
        0xA5A5_0002,
    );

    println!(
        "[ruby illuminant shift] D65 chroma=({x_d65:.4}, {y_d65:.4})  Incandescent chroma=({x_inc:.4}, {y_inc:.4})  dx={:+.4}",
        x_inc - x_d65
    );

    assert!(
        x_inc > x_d65 + 0.003,
        "Ruby's CIE x-chromaticity under Incandescent (3200K) ({x_inc:.4}) must be measurably \
         higher (redder) than under D65 Daylight ({x_d65:.4}) -- the narrow-window Cr3+ band \
         model should reproduce this well-known illuminant-dependent color shift"
    );
}

/// Alexandrite's defining color change (daylight green, incandescent red) comes from the same
/// narrow-window mechanism as ruby's shift: two Cr3+ bands straddle the ~580 nm and 415 nm
/// Neuhaus critical values (see the Alexandrite entry in `GemMaterial::all_materials`). The
/// trichroic absorption adds direction dependence on top of the illuminant dependence. At the
/// face-up view the ray runs down `c_axis`, mixing the alpha and beta principal spectra, and
/// this test checks that the illuminant dependence survives.
///
/// The margin over the +0.003 floor was last measured at dx = +0.0104 for the trichroic data,
/// before the GRR 874 entry change. That margin has not been re-measured since.
#[test]
fn alexandrite_shifts_redder_under_incandescent_than_d65() {
    const SAMPLES: u32 = 192;

    let planes = StandardGemCuts::emerald_cut();
    let alexandrite =
        GemMaterial::by_name("Alexandrite").expect("Alexandrite must be a built-in material");
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };

    let (x_d65, y_d65) = render_chromaticity_under_preset(
        &alexandrite,
        &planes,
        ray,
        LightingPreset::Daylight,
        SAMPLES,
        0xA5A5_0003,
    );
    let (x_inc, y_inc) = render_chromaticity_under_preset(
        &alexandrite,
        &planes,
        ray,
        LightingPreset::Incandescent,
        SAMPLES,
        0xA5A5_0004,
    );

    println!(
        "[alexandrite illuminant shift] D65 chroma=({x_d65:.4}, {y_d65:.4})  Incandescent chroma=({x_inc:.4}, {y_inc:.4})  dx={:+.4}",
        x_inc - x_d65
    );

    assert!(
        x_inc > x_d65 + 0.003,
        "Alexandrite's CIE x-chromaticity under Incandescent (3200K) ({x_inc:.4}) must be \
         measurably higher (redder) than under D65 Daylight ({x_d65:.4}) -- the defining \
         daylight-green / incandescent-red color change must survive the trichroic \
         absorption data"
    );
}
