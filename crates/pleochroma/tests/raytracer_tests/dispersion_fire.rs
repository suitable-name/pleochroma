//! Image-level dispersion "fire" test: chromaticity variance across a rendered
//! pixel grid must be measurably higher for a dispersive material than for an
//! otherwise-identical flat-index one.

use glam::Vec3;
use pleochroma::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset, hash_u32, trace_spectral_ray},
    },
};

/// Returns the CIE xy chromaticity of each pixel of a `grid` x `grid` render of `material`.
///
/// Each pixel averages `samples_per_pixel` spectral samples, each with its own seed, so the
/// noise averages down. Pixels below a minimum luminance are skipped: they carry only
/// illuminant chromaticity and would dilute the measurement.
fn render_pixel_grid_chromaticities(
    camera: &Camera,
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    grid: usize,
    samples_per_pixel: u32,
    seed_salt: u32,
) -> Vec<(f32, f32)> {
    let dim = grid as f32;
    let mut chromaticities = Vec::new();

    for iy in 0..grid {
        for ix in 0..grid {
            let ray = camera.generate_ray(ix as f32, iy as f32, dim, dim, 0.5, 0.5);
            let mut xyz_sum = Vec3::ZERO;
            for s in 0..samples_per_pixel {
                let pixel_id = u32::try_from(iy * grid + ix).expect("a pixel id fits in u32");
                let seed = hash_u32(seed_salt ^ hash_u32(pixel_id ^ hash_u32(s ^ 0x51ED_270B)));
                xyz_sum += trace_spectral_ray(
                    ray,
                    planes,
                    material,
                    12,
                    LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
                    seed,
                    (hash_u32(seed) as f32) / 4_294_967_295.0,
                    None,
                );
            }
            let xyz_avg = xyz_sum / samples_per_pixel as f32;
            let sum = xyz_avg.x + xyz_avg.y + xyz_avg.z;
            if sum > 1e-4 && xyz_avg.y > 0.015 {
                chromaticities.push((xyz_avg.x / sum, xyz_avg.y / sum));
            }
        }
    }
    chromaticities
}

fn variance(values: &[f32]) -> f32 {
    let n = values.len() as f32;
    let mean = values.iter().sum::<f32>() / n;
    values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n
}

/// Dispersion "fire" must show at image level: a single ray traces one path, so its chromatic
/// spread reflects the illuminant and CMF shape more than dispersion. This renders a small
/// grid, averages the samples per pixel, and compares the variance of the chromaticity across
/// pixels between Cubic Zirconia and a flat material (`dispersion_delta = 0.0`) with the same
/// mean index, which controls for Fresnel magnitude.
///
/// A flat material renders a uniform hue: with one index at every wavelength all channels agree,
/// so `spectral_mis_weight` is exactly 1 (see
/// `spectral_mis_weight_is_exactly_unity_when_all_channels_agree`). A dispersive material's
/// weight concentrates each sample on its hero wavelength once refraction diverges, which
/// widens the spread across the image.
#[test]
fn test_image_level_chromatic_spread_reveals_dispersion_fire_vs_flat_material() {
    const GRID: usize = 20;
    const SAMPLES_PER_PIXEL: u32 = 64;

    let planes = StandardGemCuts::standard_round_brilliant();
    let dispersive = GemMaterial::by_name("Cubic Zirconia").unwrap();
    let nd_cz = dispersive.dispersion.evaluate(589.3);
    // Same mean index, isotropic and absorption-free like Cubic Zirconia: dispersion is the
    // only difference from `dispersive`.
    let flat = GemMaterial::new_custom("Flat CZ-index reference", nd_cz, 0.0, 0.0, [0.0, 0.0, 0.0]);

    // A moderately oblique view so pavilion facets and internal TIR bounces are
    // actually in frame (a straight-down face-up view sees mostly the flat table
    // facet, with far fewer dispersive refraction events per ray).
    let camera = Camera::new(0.35, 0.55, 3.1, 42.0);

    let chroma_dispersive = render_pixel_grid_chromaticities(
        &camera,
        &planes,
        &dispersive,
        GRID,
        SAMPLES_PER_PIXEL,
        0x1111_1111,
    );
    let chroma_flat = render_pixel_grid_chromaticities(
        &camera,
        &planes,
        &flat,
        GRID,
        SAMPLES_PER_PIXEL,
        0x2222_2222,
    );

    assert!(
        chroma_dispersive.len() > 20 && chroma_flat.len() > 20,
        "expected a reasonable number of gem-covered pixels in the {}x{} grid (dispersive={}, flat={})",
        GRID,
        GRID,
        chroma_dispersive.len(),
        chroma_flat.len()
    );

    let x_dispersive: Vec<f32> = chroma_dispersive.iter().map(|&(x, _)| x).collect();
    let y_dispersive: Vec<f32> = chroma_dispersive.iter().map(|&(_, y)| y).collect();
    let x_flat: Vec<f32> = chroma_flat.iter().map(|&(x, _)| x).collect();
    let y_flat: Vec<f32> = chroma_flat.iter().map(|&(_, y)| y).collect();

    let var_dispersive_x = variance(&x_dispersive);
    let var_dispersive_y = variance(&y_dispersive);
    let var_flat_x = variance(&x_flat);
    let var_flat_y = variance(&y_flat);

    println!(
        "[fire demonstration] pixels: dispersive={} flat={} | chromaticity variance: dispersive (x={:.3e}, y={:.3e}) flat (x={:.3e}, y={:.3e}) | ratio (x={:.2}x, y={:.2}x)",
        chroma_dispersive.len(),
        chroma_flat.len(),
        var_dispersive_x,
        var_dispersive_y,
        var_flat_x,
        var_flat_y,
        var_dispersive_x / var_flat_x.max(1e-12),
        var_dispersive_y / var_flat_y.max(1e-12)
    );

    assert!(
        var_dispersive_x > var_flat_x * 2.0,
        "Cubic Zirconia's across-image x-chromaticity variance ({var_dispersive_x:.3e}) should clearly exceed the flat reference material's ({var_flat_x:.3e}) -- dispersion should visibly widen the spread of hues across the rendered image"
    );
    assert!(
        var_dispersive_y > var_flat_y * 2.0,
        "Cubic Zirconia's across-image y-chromaticity variance ({var_dispersive_y:.3e}) should clearly exceed the flat reference material's ({var_flat_y:.3e}) -- dispersion should visibly widen the spread of hues across the rendered image"
    );

    // The flat material's spread must also be small in absolute terms, not only smaller than
    // the dispersive one. CIE xy spans about a unit square, so a variance below 1e-4 is a
    // standard deviation under about 1 %: an essentially uniform hue.
    assert!(
        var_flat_x < 1e-4 && var_flat_y < 1e-4,
        "flat (non-dispersive) material should render with essentially uniform hue across the image (var_x={var_flat_x:.3e}, var_y={var_flat_y:.3e})"
    );
}
