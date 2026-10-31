//! The full crystal frame of a biaxial material (`GemMaterial::alpha_axis`): the second
//! direction changes nothing while it is unset, nothing for a uniaxial material, and
//! nothing when it states the arbitrary completion itself (turn zero); a turned frame
//! reads different absorption at a fixed wave normal and renders differently.

use glam::Vec3;
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::{
        birefringence::{AbsorptionTensor3, alpha_axis_from_turn},
        materials::GemMaterial,
        raytracer::{LightingPreset, Ray, hash_u32, trace_spectral_ray},
    },
};

const SEEDS: [u32; 6] = [1, 2, 42, 4242, 7919, 0xC0FF_EE00];

fn oblique_ray() -> Ray {
    Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    }
}

/// The x/y/z bit patterns of one trace per seed.
fn trace_bits(material: &GemMaterial) -> Vec<[u32; 3]> {
    let planes = StandardGemCuts::standard_round_brilliant();
    SEEDS
        .iter()
        .map(|&seed| {
            let xyz = trace_spectral_ray(
                oblique_ray(),
                &planes,
                material,
                12,
                LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
                seed,
                (hash_u32(seed) as f32) / 4_294_967_295.0,
                None,
            );
            [xyz.x.to_bits(), xyz.y.to_bits(), xyz.z.to_bits()]
        })
        .collect()
}

/// A uniaxial material ignores a stated second direction: its trace is bit-identical.
#[test]
fn a_uniaxial_material_with_an_alpha_axis_traces_bit_identically() {
    for name in ["Sapphire", "Ruby", "Quartz"] {
        let plain = GemMaterial::by_name(name).expect("built-in material");
        assert!(
            plain.biaxial_delta_beta_alpha.is_none(),
            "{name} is uniaxial"
        );
        let mut framed = plain.clone();
        framed.alpha_axis = Some(alpha_axis_from_turn(plain.c_axis, 37.0));
        assert_eq!(trace_bits(&plain), trace_bits(&framed), "{name}");
    }
}

/// A biaxial material whose stated alpha direction is the turn-zero completion is the
/// unframed material, bit for bit.
#[test]
fn a_biaxial_material_with_the_turn_zero_alpha_axis_traces_bit_identically() {
    for name in ["Tanzanite", "Topaz", "Alexandrite"] {
        let plain = GemMaterial::by_name(name).expect("built-in material");
        assert!(
            plain.biaxial_delta_beta_alpha.is_some(),
            "{name} is biaxial"
        );
        let mut framed = plain.clone();
        framed.alpha_axis = Some(alpha_axis_from_turn(plain.c_axis, 0.0));
        assert_eq!(trace_bits(&plain), trace_bits(&framed), "{name}");
    }
}

/// A framed Tanzanite tensor turned by 90 degrees about gamma reads a different
/// absorption at a fixed wave normal: the principal axes the eigenmodes are scored
/// against moved with the frame.
#[test]
fn a_framed_tanzanite_tensor_turned_ninety_degrees_absorbs_differently() {
    let tanzanite = GemMaterial::by_name("Tanzanite").expect("built-in material");
    let lambda = 550.0f32;
    let wave_normal = Vec3::new(0.5, 0.4, 0.6).normalize();
    // Strongly trichroic synthetic coefficients isolate the frame from the fitted bands.
    let (alpha, beta, gamma) = (1.0f32, 4.0f32, 9.0f32);

    let absorption_at_turn = |turn_deg: f32| {
        let mut turned = tanzanite.clone();
        turned.alpha_axis = Some(alpha_axis_from_turn(tanzanite.c_axis, turn_deg));
        let indicatrix = turned
            .biaxial_indicatrix(lambda)
            .expect("tanzanite is biaxial");
        let tensor = AbsorptionTensor3::biaxial_framed(
            alpha,
            beta,
            gamma,
            turned.c_axis,
            turned.alpha_axis.expect("set above"),
        );
        let (slow, fast) = indicatrix.eigen_polarizations(wave_normal);
        (tensor.quadratic_form(slow), tensor.quadratic_form(fast))
    };

    let (base_slow, base_fast) = absorption_at_turn(0.0);
    let (turned_slow, turned_fast) = absorption_at_turn(90.0);
    assert!(
        (base_slow - turned_slow).abs() > 1e-3 || (base_fast - turned_fast).abs() > 1e-3,
        "turning the frame by 90 degrees must change the absorption (turn 0: {base_slow}/{base_fast}, \
         turn 90: {turned_slow}/{turned_fast})"
    );
}

/// End to end: a turned Tanzanite renders differently from the unframed one.
#[test]
fn a_turned_biaxial_material_renders_differently() {
    let plain = GemMaterial::by_name("Tanzanite").expect("built-in material");
    let mut turned = plain.clone();
    turned.alpha_axis = Some(alpha_axis_from_turn(plain.c_axis, 90.0));
    assert_ne!(trace_bits(&plain), trace_bits(&turned));
}
