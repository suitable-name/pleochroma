//! Orientation-sign pleochroism tests.
//!
//! Tourmaline, Ruby and Alexandrite face-up luminance must be darker or brighter depending on
//! how their `c_axis` sits relative to the view axis. Aquamarine's face-up color is blue only
//! with `c_axis` in the table plane, and Sapphire's side-on hue rotates toward green relative
//! to face-up. Each test discriminates the sign, which a simple "looks dark" check cannot.

use glam::Vec3;
use pleochroma::{
    color::body_color::{lab_to_lch, xyz_to_lab},
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{
        birefringence::{AbsorptionTensor3, BirefringenceParams},
        materials::GemMaterial,
        raytracer::{
            LightingPreset, Ray, cie_1931_cmf, hash_u32, spectral_absorption, trace_spectral_ray,
        },
    },
};

/// Average CIE Y of `material` through a fixed ray under `lighting_preset`, over `samples`
/// spectral samples. Used where the signal is overall brightness (how much a `c_axis` choice
/// darkens or brightens the face-up view), not hue.
fn average_luminance(
    material: &GemMaterial,
    planes: &[GpuFacetPlane],
    ray: Ray,
    lighting_preset: LightingPreset,
    samples: u32,
    seed_salt: u32,
) -> f32 {
    let mut y_sum = 0.0f32;
    for i in 0..samples {
        let seed = hash_u32(seed_salt ^ hash_u32(i ^ 0x9E37_79B9));
        let xyz = trace_spectral_ray(
            ray,
            planes,
            material,
            12,
            lighting_preset.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        y_sum += xyz.y;
    }
    y_sum / samples as f32
}

/// Tourmaline's `c_axis` is `Vec3::X` (in the table plane), unlike the `Vec3::Y` default (see
/// `test_gem_materials_default_c_axis_to_y`). Cutters orient the table perpendicular to the
/// c-axis, because face-up down the closed ("dark ray", o-ray) axis is its worst view.
///
/// The test discriminates sign: it collapses or flips if the o-ray and e-ray naming were
/// swapped, or if the Mueller frame-rotation mirror regressed, since both change WHICH direction
/// reads dark.
///
/// The same cut, lighting and face-up ray are used for the real material and a clone forced to
/// `c_axis = Vec3::Y`. There the wave normal is parallel to `c_axis`, and the polarization
/// quadratic form reduces to the o-ray (see `birefringence::AbsorptionTensor3::quadratic_form`).
/// Luminance is averaged over 20,000 seeds, a margin about ten times the seed-to-seed noise.
#[test]
fn tourmaline_face_up_is_darker_with_c_axis_along_view_axis_than_in_table_plane() {
    const SAMPLES: u32 = 20_000;
    let planes = StandardGemCuts::standard_round_brilliant();
    let tourmaline_real =
        GemMaterial::by_name("Tourmaline").expect("Tourmaline must be a built-in material");
    assert_eq!(
        tourmaline_real.c_axis,
        Vec3::X,
        "test premise: Tourmaline's real (as-shipped) c_axis must be Vec3::X"
    );
    let mut tourmaline_wrong_way = tourmaline_real.clone();
    tourmaline_wrong_way.c_axis = Vec3::Y;

    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0), // fixed face-up ray, straight through the table
    };

    let lum_real = average_luminance(
        &tourmaline_real,
        &planes,
        ray,
        LightingPreset::RingLights,
        SAMPLES,
        0xB007_0001,
    );
    let lum_wrong_way = average_luminance(
        &tourmaline_wrong_way,
        &planes,
        ray,
        LightingPreset::RingLights,
        SAMPLES,
        0xB007_0001,
    );

    println!(
        "[tourmaline orientation] luminance c_axis=X (real, table-plane) = {lum_real:.6}, \
         c_axis=Y (forced, down the dark axis) = {lum_wrong_way:.6}, margin = {:+.6} ({:.2}% darker)",
        lum_real - lum_wrong_way,
        100.0 * (1.0 - lum_wrong_way / lum_real)
    );

    assert!(
        lum_wrong_way < lum_real * 0.97,
        "face-up down the closed/dark axis (c_axis=Y, forced) should be measurably DARKER \
         than the real cut-orientation override (c_axis=X) -- got luminance(Y)={lum_wrong_way:.6} \
         vs luminance(X)={lum_real:.6}"
    );
}

/// `[L*, C*, h]` of the unpolarised light that `material` transmits over `units` model units of
/// path along the face-up wave normal (straight down through the table, `-Y`), relative to an
/// equal-energy white. Half the light travels in each eigenmode (unpolarised light couples
/// equally into both), so the transmitted spectrum is the mean of the two modes' Beer-Lambert
/// transmittances, each with its own absorption from the polarization quadratic form at the
/// material's own `c_axis` -- what the interior transport (one assigned mode per path,
/// `assigned_mode_alpha`) averages to over many paths.
fn face_up_lch(material: &GemMaterial, units: f32) -> [f64; 3] {
    let k_hat = Vec3::NEG_Y;
    let o_hat = BirefringenceParams::ordinary_eigen_polarization(k_hat, material.c_axis);
    let e_hat = BirefringenceParams::extraordinary_eigen_polarization(k_hat, material.c_axis);
    let mut xyz = [0.0f64; 3];
    let mut white = [0.0f64; 3];
    for step in 0..=(780 - 380) {
        let lambda = 380.0f32 + step as f32;
        let tensor = AbsorptionTensor3::uniaxial(
            spectral_absorption(&material.absorption.o_ray, lambda),
            spectral_absorption(&material.absorption.e_ray, lambda),
            material.c_axis,
        );
        let transmittance = f32::midpoint(
            (-tensor.quadratic_form(o_hat) * units).exp(),
            (-tensor.quadratic_form(e_hat) * units).exp(),
        );
        for ((x, w), c) in xyz
            .iter_mut()
            .zip(white.iter_mut())
            .zip(cie_1931_cmf(lambda).to_array())
        {
            *x += f64::from(c * transmittance);
            *w += f64::from(c);
        }
    }
    lab_to_lch(xyz_to_lab(xyz, white))
}

/// Aquamarine's color is its e-ray (E parallel to c), while its o-ray is nearly colorless. So the
/// engaged ray decides whether the stone reads blue. As shipped, `c_axis` is `Vec3::X` (c in the
/// table plane, as Aquamarine is cut): the face-up wave normal is perpendicular to c, both
/// eigenmodes are engaged, and the unpolarized mix is light blue. A clone forced to `Vec3::Y`
/// has the wave normal parallel to c, where both eigenmodes collapse to the o-ray. That reads
/// practically colorless with a greenish cast.
///
/// This checks the face-up relation at one model unit of path. Hue and lightness discriminate.
/// C* is compared only at this path, because over longer paths the o-ray gains chroma from the
/// Fe2+ band tail and C* stops separating the two orientations. The expected values are measured
/// against an equal-energy white for a 50/50 mix of the two eigenmodes.
#[test]
fn aquamarine_face_up_sees_the_blue_ray_with_c_axis_in_table_plane() {
    let real = GemMaterial::by_name("Aquamarine").expect("Aquamarine must be a built-in material");
    assert_eq!(
        real.c_axis,
        Vec3::X,
        "test premise: Aquamarine's real (as-shipped) c_axis must be Vec3::X"
    );
    let mut wrong_way = real.clone();
    wrong_way.c_axis = Vec3::Y;

    let [l_real, c_real, h_real] = face_up_lch(&real, 1.0);
    let [l_wrong, c_wrong, h_wrong] = face_up_lch(&wrong_way, 1.0);

    println!(
        "[aquamarine orientation] face-up c_axis=X (real, table-plane): L* {l_real:.1} C* \
         {c_real:.1} h {h_real:.1}; c_axis=Y (forced, along the view axis): L* {l_wrong:.1} C* \
         {c_wrong:.1} h {h_wrong:.1}"
    );

    assert!(
        (230.0..=290.0).contains(&h_real),
        "face-up with c in the table plane must be blue, got hue {h_real:.1}"
    );
    assert!(
        h_wrong < 190.0,
        "face-up with c along the view axis sees the o-ray only (greenish, not blue), got hue \
         {h_wrong:.1}"
    );
    assert!(
        c_real > c_wrong + 8.0,
        "the blue face-up view must be more saturated than the o-ray-only view -- got C* \
         {c_real:.1} vs {c_wrong:.1}"
    );
    assert!(
        l_real < l_wrong - 5.0,
        "the e-ray's absorption must darken the face-up view -- got L* {l_real:.1} vs \
         {l_wrong:.1}"
    );
}

/// Ruby variant of the orientation-sign test, with a looser bound. Ruby keeps the `Vec3::Y`
/// default, so the real material is compared with a clone forced to `c_axis = Vec3::X` at the
/// same face-up ray. Ruby's o-ray to e-ray amplitude ratio (about 1.8, see the Ruby entry in
/// `GemMaterial::all_materials`) is milder than Tourmaline's (about 3), so the required margin
/// is smaller, while still well above the noise.
#[test]
fn ruby_face_up_is_darker_with_c_axis_along_view_axis_than_perpendicular_to_it() {
    const SAMPLES: u32 = 20_000;
    let planes = StandardGemCuts::standard_round_brilliant();
    let ruby_real = GemMaterial::by_name("Ruby").expect("Ruby must be a built-in material");
    assert_eq!(
        ruby_real.c_axis,
        Vec3::Y,
        "test premise: Ruby keeps the every-material default c_axis=Vec3::Y (no cut-orientation override)"
    );
    let mut ruby_rotated = ruby_real.clone();
    ruby_rotated.c_axis = Vec3::X;

    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };

    let lum_real = average_luminance(
        &ruby_real,
        &planes,
        ray,
        LightingPreset::RingLights,
        SAMPLES,
        0xB007_0002,
    );
    let lum_rotated = average_luminance(
        &ruby_rotated,
        &planes,
        ray,
        LightingPreset::RingLights,
        SAMPLES,
        0xB007_0002,
    );

    println!(
        "[ruby orientation] luminance c_axis=Y (real) = {lum_real:.6}, c_axis=X (forced) = \
         {lum_rotated:.6}, margin = {:+.6} ({:.2}% darker)",
        lum_rotated - lum_real,
        100.0 * (1.0 - lum_real / lum_rotated)
    );

    assert!(
        lum_real < lum_rotated * 0.99,
        "Ruby face-up down its own c_axis (Y, real) should be measurably darker than with \
         c_axis rotated into the table plane (X, forced) -- got luminance(Y)={lum_real:.6} vs \
         luminance(X)={lum_rotated:.6} (looser bound than Tourmaline's, per Ruby's milder ~1.8x \
         o:e ratio)"
    );
}

/// Alexandrite variant of the orientation-sign tests. Its `c_axis` is the `n_gamma` direction,
/// the crystallographic b axis, which carries the strongest (green) absorption band. That band
/// lies on the photopic luminance peak, so which directions a view engages shows up directly in
/// luminance. Face-up down `c_axis = Vec3::Y` (as shipped), the transverse polarizations sample
/// the alpha (red) and beta (yellow) spectra. The clone with `c_axis = Vec3::X` rotates gamma
/// into the table plane, where face-up polarizations engage it, and darkens the view. Its alpha
/// axis is set to `Vec3::Z` so that beta lies along the view. Swapping gamma's band onto another
/// slot flips or collapses the margin, which makes the test sign-discriminating for the alpha,
/// beta and gamma order.
#[test]
fn alexandrite_face_up_is_brighter_than_with_green_gamma_axis_rotated_into_table_plane() {
    const SAMPLES: u32 = 20_000;
    let planes = StandardGemCuts::standard_round_brilliant();
    let alexandrite_real =
        GemMaterial::by_name("Alexandrite").expect("Alexandrite must be a built-in material");
    assert_eq!(
        alexandrite_real.c_axis,
        Vec3::Y,
        "test premise: Alexandrite keeps the every-material default c_axis=Vec3::Y"
    );
    let mut alexandrite_rotated = alexandrite_real.clone();
    alexandrite_rotated.c_axis = Vec3::X;
    // Beta (X cross Z = -Y) along the view: face-up shows alpha and gamma.
    alexandrite_rotated.alpha_axis = Some(Vec3::Z);

    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };

    let lum_real = average_luminance(
        &alexandrite_real,
        &planes,
        ray,
        LightingPreset::RingLights,
        SAMPLES,
        0xB007_0003,
    );
    let lum_rotated = average_luminance(
        &alexandrite_rotated,
        &planes,
        ray,
        LightingPreset::RingLights,
        SAMPLES,
        0xB007_0003,
    );

    println!(
        "[alexandrite orientation] luminance c_axis=Y (real, gamma along view) = {lum_real:.6}, \
         c_axis=X, alpha=Z (forced, gamma and alpha in table plane) = {lum_rotated:.6}, margin = {:+.6} ({:.2}% darker)",
        lum_rotated - lum_real,
        100.0 * (1.0 - lum_rotated / lum_real)
    );

    assert!(
        lum_rotated < lum_real * 0.99,
        "rotating alexandrite's strong-595nm green gamma axis into the table plane should \
         measurably DARKEN the face-up view -- got luminance(X, forced)={lum_rotated:.6} vs \
         luminance(Y, real)={lum_real:.6}"
    );
}

/// Sapphire's face-up view is dominated by `alpha_o` (E perpendicular to c). With `c_axis` =
/// `Vec3::Y`, straight-down propagation is parallel to `c_axis`, the degenerate direction where
/// the ordinary and extraordinary eigenmodes coincide, so `AbsorptionTensor3::quadratic_form`
/// gives `alpha_o` for every polarization. A side-on view (propagation perpendicular to
/// `c_axis`) engages `alpha_e`.
///
/// This integrates `spectral_absorption` against the CIE 1931 CMFs with a flat illuminant, rather
/// than tracing a full polyhedron. A raytraced side-on ray is confounded: walk-off moves the
/// extraordinary ray onto another facet path, and Fresnel and background terms dominate at many
/// entry angles. The integral still uses the same `spectral_absorption` that
/// `raytracer::apply_absorption` calls on every bounce, at several path lengths.
///
/// The side-on transmission uses the 50/50 o/e average, which
/// `birefringence::effective_pleochroic_alpha` computes for unpolarized light perpendicular to
/// `c_axis`. That is what light entering a flat facet at normal incidence is.
#[test]
fn sapphire_side_on_hue_rotates_toward_green_relative_to_face_up() {
    fn spectral_chroma(bands_alpha: impl Fn(f32) -> f32, path_len: f32) -> (f32, f32) {
        let mut xyz = Vec3::ZERO;
        for step in 0..=(780 - 380) {
            let lambda = 380.0f32 + step as f32;
            let transmittance = (-bands_alpha(lambda) * path_len).exp();
            xyz += cie_1931_cmf(lambda) * transmittance;
        }
        let sum = xyz.x + xyz.y + xyz.z;
        (xyz.x / sum, xyz.y / sum)
    }

    let sapphire = GemMaterial::by_name("Sapphire").expect("Sapphire must be a built-in material");
    let o_bands = sapphire.absorption.o_ray.clone();
    let e_bands = sapphire.absorption.e_ray;

    for path_len in [0.5f32, 1.0, 1.5, 2.0] {
        let (x_face, y_face) = spectral_chroma(|l| spectral_absorption(&o_bands, l), path_len);
        let (x_side, y_side) = spectral_chroma(
            |l| {
                0.5f32.mul_add(
                    spectral_absorption(&e_bands, l),
                    0.5 * spectral_absorption(&o_bands, l),
                )
            },
            path_len,
        );

        println!(
            "[sapphire hue shift] path_len={path_len:.2}  face-up=({x_face:.5},{y_face:.5})  \
             side-on=({x_side:.5},{y_side:.5})  dx={:+.5} dy={:+.5}",
            x_side - x_face,
            y_side - y_face
        );

        // "Toward green" is a higher CIE y: the green locus sits well above blue. Sapphire is
        // still blue at every path length, so the claim is the direction of the shift, not the
        // destination. x is not constrained: at the shortest path the chromaticity sits near the
        // illuminant, where x can rise with y. Asserting on y alone keeps the claim to what the
        // test needs.
        assert!(
            y_side > y_face + 0.01,
            "path_len={path_len}: side-on chromaticity y ({y_side:.5}) should be measurably \
             higher (greener) than face-up's ({y_face:.5})"
        );
    }
}
