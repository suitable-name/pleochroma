//! The face-up tone of a pleochroic stone: the per-ray principal paths, and the tone built on
//! them (axis-aware, unlike the orientation-mean `(2 T_o + T_e) / 3` it replaced).

use std::f32::consts::FRAC_PI_2;

use glam::Vec3;

use super::{
    FaceUpTone, evaluate_gem_optical_metrics_with_tone,
    prepared::PreparedEval,
    ray_trace::{EntryRay, RayFate, trace_entry},
};
use crate::{
    geometry::{cuts::StandardGemCuts, tool::StoneGeometry},
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::{GemMaterial, OpticalCharacter},
        raytracer::{Camera, LightingPreset},
    },
};

/// A tourmaline-like uniaxial stone: the ordinary ray absorbs orange, the extraordinary one
/// blue, so the face-up color depends on which way `c_axis` points.
///
/// Its birefringence is zero, so the light-return trace is the single-mode one and the axis
/// moves only the absorption (with real birefringence the two-mode trace makes the returned
/// light depend on the axis too).
fn tourmaline_like(c_axis: Vec3) -> GemMaterial {
    let mut material = GemMaterial::sapphire();
    material.birefringence_delta = 0.0;
    material.absorption = AbsorptionTensor::uniaxial(
        vec![AbsorptionBand::new(580.0, 60.0, 0.5)],
        vec![AbsorptionBand::new(450.0, 50.0, 0.5)],
    );
    material.c_axis = c_axis;
    material
}

/// A trichroic stone whose three principal axes absorb at clearly different wavelengths;
/// `gamma_axis` places the whole principal frame.
fn trichroic(gamma_axis: Vec3) -> GemMaterial {
    let mut material = GemMaterial::by_name("Tanzanite").expect("Tanzanite resolves");
    material.optical_character = OpticalCharacter::BiaxialPositive;
    material.biaxial_delta_beta_alpha = Some(0.0025);
    material.birefringence_delta = 0.009;
    material.absorption = AbsorptionTensor::biaxial(
        vec![AbsorptionBand::new(600.0, 50.0, 0.5)],
        vec![AbsorptionBand::new(510.0, 40.0, 0.5)],
        vec![AbsorptionBand::new(430.0, 40.0, 0.5)],
    );
    material.c_axis = gamma_axis;
    material
}

fn table_up_tone(material: &GemMaterial) -> FaceUpTone {
    evaluate_gem_optical_metrics_with_tone(
        &StandardGemCuts::standard_round_brilliant(),
        material,
        0.0,
        FRAC_PI_2,
        LightingPreset::Daylight.studio(1.0, 0.85, 0.95),
    )
    .1
}

#[test]
fn the_face_up_tone_depends_on_the_axis_direction() {
    let along_view = table_up_tone(&tourmaline_like(Vec3::Y));
    let across_view = table_up_tone(&tourmaline_like(Vec3::X));
    assert_ne!(along_view.srgb, across_view.srgb);
    assert!(along_view.returned_fraction > 0.0);
    // The geometry is the same, only the axis differs.
    assert_eq!(
        along_view.returned_fraction.to_bits(),
        across_view.returned_fraction.to_bits()
    );
    assert_eq!(
        along_view.mean_path_units.to_bits(),
        across_view.mean_path_units.to_bits()
    );
}

#[test]
fn rotating_a_biaxial_frame_changes_the_tone() {
    let material = trichroic(Vec3::Y);
    assert!(material.biaxial_indicatrix(589.3).is_some());
    let gamma_up = table_up_tone(&material);
    let gamma_sideways = table_up_tone(&trichroic(Vec3::X));
    assert_ne!(gamma_up.srgb, gamma_sideways.srgb);
}

#[test]
fn only_a_pleochroic_material_prepares_a_principal_frame() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let environment = LightingPreset::Daylight.studio(1.0, 0.85, 0.95);
    let colourless = PreparedEval::new(
        StoneGeometry::planes_only(&planes),
        &GemMaterial::diamond(),
        environment,
    )
    .expect("planes");
    assert!(!colourless.is_pleochroic());
    let tourmaline = tourmaline_like(Vec3::Y);
    let prepared = PreparedEval::new(
        StoneGeometry::planes_only(&planes),
        &tourmaline,
        environment,
    )
    .expect("planes");
    assert!(prepared.is_pleochroic());
    assert!(prepared.stone().principal.is_none());
    assert!(prepared.stone_with_principal().principal.is_some());
}

/// Every upward-exiting ray's three principal paths add up to its total path (the weights of
/// each segment sum to 1), for a tilted uniaxial axis; the plain arena reports zeros.
#[test]
fn the_principal_paths_split_the_total_path() {
    let planes = StandardGemCuts::emerald_cut();
    let material = tourmaline_like(Vec3::new(0.3, 1.0, 0.2).normalize());
    let prepared = PreparedEval::new(
        StoneGeometry::planes_only(&planes),
        &material,
        LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95),
    )
    .expect("planes");
    let camera = Camera::new(0.4, 0.9, 2.4, 42.0);
    let mut checked = 0;
    for gy in 0..48 {
        for gx in 0..48 {
            let ray = camera.generate_ray(gx as f32, gy as f32, 48.0, 48.0, 0.0, 0.0);
            let Some(hit) = prepared.stone().intersect(ray) else {
                continue;
            };
            let entry = EntryRay {
                point: ray.origin + hit.t * ray.dir,
                dir: ray.dir,
                normal: hit.normal,
                cos_i: (-ray.dir).dot(hit.normal).clamp(0.0, 1.0),
                facet: hit.facet_idx,
            };
            let plain = trace_entry(entry, prepared.stone(), prepared.nd, None);
            let split = trace_entry(entry, prepared.stone_with_principal(), prepared.nd, None);
            let (RayFate::ExitedUpward(plain), RayFate::ExitedUpward(split)) = (plain, split)
            else {
                continue;
            };
            assert_eq!(plain.principal_path, [0.0; 3], "pixel {gx},{gy}");
            // The split changes nothing about the trace itself.
            assert_eq!(plain.path_len.to_bits(), split.path_len.to_bits());
            let total: f32 = split.principal_path.iter().sum();
            assert!(
                (total - split.path_len).abs() <= 1e-4 * split.path_len.max(1.0),
                "pixel {gx},{gy}: {:?} sums to {total}, path {}",
                split.principal_path,
                split.path_len
            );
            assert!(split.principal_path.iter().all(|&d| d >= -1e-5));
            checked += 1;
        }
    }
    assert!(checked > 0, "no ray exited upward");
}
