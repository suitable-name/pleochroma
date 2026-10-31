//! Pins for the blocks appended to `GpuGemMaterial` after the zone table (the biaxial curves and
//! the inclusion block) and for the packed per-facet surface word's encoder.

use core::mem::offset_of;

use glam::Vec3;

use super::{
    GpuBiaxialCurves, GpuGemMaterial, GpuInclusionBlock, STRUCTURE_ROWS, dispersion_model_type,
    encode_facet_finishes, encode_facet_surfaces, facet_finish, family_kind,
};
use crate::optics::{
    dispersion::DispersionModel,
    inclusions::{FamilyKind, InclusionSet, ResolvedFamily},
    materials::GemMaterial,
    raytracer::{FacetFinish, surface::FacetSurface},
};

/// The offsets and sizes the WGSL structs are declared against.
#[test]
fn appended_blocks_sit_at_the_documented_offsets() {
    assert_eq!(offset_of!(GpuGemMaterial, zones), 592);
    assert_eq!(offset_of!(GpuGemMaterial, biaxial_curves), 2720);
    assert_eq!(offset_of!(GpuGemMaterial, inclusions), 2800);
    assert_eq!(size_of::<GpuGemMaterial>(), 3024);
    assert_eq!(size_of::<GpuBiaxialCurves>(), 80);
    assert_eq!(size_of::<GpuInclusionBlock>(), 224);
    assert_eq!(offset_of!(GpuInclusionBlock, fam_dir), 16);
    assert_eq!(offset_of!(GpuInclusionBlock, fam_params), 80);
    assert_eq!(offset_of!(GpuInclusionBlock, haze), 144);
    assert_eq!(offset_of!(GpuInclusionBlock, structure), 160);
    assert_eq!(offset_of!(GpuInclusionBlock, edge), 208);
    assert_eq!(STRUCTURE_ROWS, 3);
}

/// A material with neither feature uploads exactly the bytes of the old layout in front, and
/// zeros in both new blocks.
#[test]
fn a_plain_material_encodes_both_new_blocks_as_zero() {
    for name in ["Diamond", "Sapphire", "Alexandrite"] {
        let mut material = GemMaterial::by_name(name).expect("a built-in");
        material.biaxial_principal_dispersion = None;
        let gpu = GpuGemMaterial::encode(&material);
        assert_eq!(gpu.biaxial_curves, GpuBiaxialCurves::default(), "{name}");
        assert_eq!(gpu.inclusions, GpuInclusionBlock::default(), "{name}");
        let bytes = bytemuck::bytes_of(&gpu);
        assert!(bytes[2720..].iter().all(|&b| b == 0), "{name}");
    }
}

/// The three parameters of a curve in the encoding of `DispersionParams::param_a`.
fn encoded(model: &DispersionModel) -> (u32, [f32; 4], [f32; 4]) {
    match *model {
        DispersionModel::Sellmeier1 { b1, c1 } => (
            dispersion_model_type::SELLMEIER1,
            [b1, 0.0, 0.0, 0.0],
            [c1, 0.0, 0.0, 0.0],
        ),
        DispersionModel::Sellmeier3 { b, c } => (
            dispersion_model_type::SELLMEIER3,
            [b[0], b[1], b[2], 0.0],
            [c[0], c[1], c[2], 0.0],
        ),
        DispersionModel::Cauchy { a, b, c } => {
            (dispersion_model_type::CAUCHY, [a, b, c, 0.0], [0.0; 4])
        }
    }
}

/// The two curves of Alexandrite reach the block in their slots; the flag is set.
#[test]
fn biaxial_curves_encode_into_their_slots() {
    let material = GemMaterial::by_name("Alexandrite").expect("a built-in");
    let [alpha, gamma] = material
        .biaxial_principal_dispersion
        .expect("Alexandrite carries the published curves");
    let curves = GpuGemMaterial::encode(&material).biaxial_curves;
    let (alpha_type, alpha_a, alpha_b) = encoded(&alpha);
    let (gamma_type, gamma_a, gamma_b) = encoded(&gamma);
    assert_eq!(curves.types, [alpha_type, gamma_type, 1, 0]);
    assert_eq!(curves.alpha_a, alpha_a);
    assert_eq!(curves.alpha_b, alpha_b);
    assert_eq!(curves.gamma_a, gamma_a);
    assert_eq!(curves.gamma_b, gamma_b);
}

/// Curves on a material that is not biaxial are not uploaded (the CPU ignores them too).
#[test]
fn biaxial_curves_need_a_biaxial_material() {
    let mut material = GemMaterial::by_name("Alexandrite").expect("a built-in");
    material.biaxial_delta_beta_alpha = None;
    assert_eq!(
        GpuBiaxialCurves::encode(&material),
        GpuBiaxialCurves::default()
    );
}

fn needle(dir: Vec3, sigma: f32, p: u8, lobe: f32, rho: f32) -> ResolvedFamily {
    ResolvedFamily {
        kind: FamilyKind::Needle,
        dir,
        sigma,
        p,
        lobe,
        rho,
    }
}

/// Every family field lands in its slot, the count and the haze follow, the reserved rows stay
/// zero.
#[test]
fn inclusion_families_round_trip_into_the_block() {
    let set = InclusionSet::new(&[
        needle(Vec3::X, 0.5, 2, 0.1, 0.3),
        ResolvedFamily {
            kind: FamilyKind::Platelet,
            dir: Vec3::Z,
            sigma: 0.25,
            p: 0,
            lobe: 0.2,
            rho: 0.0,
        },
        ResolvedFamily {
            kind: FamilyKind::RandomNeedle,
            dir: Vec3::ZERO,
            sigma: 0.125,
            p: 4,
            lobe: 0.05,
            rho: 0.0,
        },
    ]);
    let mut material = GemMaterial::diamond().with_inclusions(set);
    material.scattering_sigma_s = 0.2;
    material.scattering_g = 0.4;
    let block = GpuInclusionBlock::encode(&material);

    assert_eq!(block.header, [3, 0, 0, 0]);
    assert_eq!(block.fam_dir[0], [1.0, 0.0, 0.0, family_kind::NEEDLE]);
    assert_eq!(block.fam_params[0], [0.5, 2.0, 0.1, 0.3]);
    assert_eq!(block.fam_dir[1], [0.0, 0.0, 1.0, family_kind::PLATELET]);
    assert_eq!(block.fam_params[1], [0.25, 0.0, 0.2, 0.0]);
    assert_eq!(block.fam_dir[2][3], family_kind::RANDOM_NEEDLE);
    assert_eq!(block.fam_params[2][1], 4.0);
    assert_eq!(block.fam_dir[3], [0.0; 4]);
    assert_eq!(block.fam_params[3], [0.0; 4]);
    assert_eq!(block.haze, [0.2, 0.4, 0.0, 0.0]);
    assert_eq!(block.structure, [[0.0; 4]; STRUCTURE_ROWS]);
    assert_eq!(block.edge, [0.0; 4]);
}

/// The reserved parts are filled through their own setters and touch nothing else.
#[test]
fn structure_and_edge_setters_fill_only_their_own_fields() {
    let rows = [[1.0, 2.0, 3.0, 4.0], [5.0; 4], [6.0; 4]];
    let block = GpuInclusionBlock::default()
        .with_structure(1, 77, rows)
        .with_edge(0.02, 0.3);
    assert_eq!(block.header, [0, 1, 77, 0]);
    assert_eq!(block.structure, rows);
    assert_eq!(block.edge, [0.02, 0.3, 0.0, 0.0]);
    assert_eq!(block.fam_dir, [[0.0; 4]; 4]);
    assert_eq!(block.haze, [0.0; 4]);
}

/// With no surfaces the surface encoder is the finish encoder, word for word.
#[test]
fn encoding_no_surfaces_equals_the_finish_encoding() {
    let finishes = [
        FacetFinish::Frosted,
        FacetFinish::Polished,
        FacetFinish::Frosted,
    ];
    assert_eq!(
        encode_facet_surfaces(&[], &finishes, 5),
        encode_facet_finishes(&finishes, 5)
    );
    assert_eq!(encode_facet_surfaces(&[], &[], 3), vec![0, 0, 0]);
    // An explicitly ideal surface is the same as none.
    assert_eq!(
        encode_facet_surfaces(&[FacetSurface::IDEAL; 3], &finishes, 3),
        encode_facet_finishes(&finishes, 3)
    );
}

/// A frosted finish beats a polish with roughness (the CPU grinds the facet with the default
/// lobe), but never a Ground surface's own lobe.
#[test]
fn a_frosted_finish_overrides_only_a_polish_with_roughness() {
    let polished = FacetSurface::polished(2.5, 1.0);
    let ground = FacetSurface::ground(0.3, 1.0);
    let words = encode_facet_surfaces(
        &[polished, ground],
        &[FacetFinish::Frosted, FacetFinish::Frosted],
        2,
    );
    assert_eq!(words, vec![facet_finish::FROSTED, ground.packed()]);
}

/// A facet with a surface carries its packed word; the kind is readable through the mask.
#[test]
fn surface_words_pass_through_and_keep_their_kind_bits() {
    let polished = FacetSurface::polished(2.5, 1.0);
    let ground = FacetSurface::ground(0.45, 2.0).with_custom_angle(37.0);
    let words = encode_facet_surfaces(
        &[polished, FacetSurface::IDEAL, ground],
        &[FacetFinish::Polished, FacetFinish::Frosted],
        4,
    );
    assert_eq!(words[0], polished.packed());
    assert_eq!(
        words[0] & facet_finish::KIND_MASK,
        facet_finish::POLISHED_ROUGH
    );
    // An ideal surface falls back to the finish: the frosted girdle stays word 1.
    assert_eq!(words[1], facet_finish::FROSTED);
    assert_eq!(words[2], ground.packed());
    assert_eq!(words[2] & facet_finish::KIND_MASK, facet_finish::GROUND);
    assert_eq!(words[3], facet_finish::POLISHED);
}
