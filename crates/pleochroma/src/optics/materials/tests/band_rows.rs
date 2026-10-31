//! Tests of the body-color band rows `GemMaterial::with_body_color_bands` reads (written, not run,
//! by the band-codec lane): a row needs a positive finite centre, width and amplitude.

use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor, body_color_bands},
    materials::GemMaterial,
    zoning::{ZoneAbsorption, ZonedAbsorption, ZoningError},
};

#[test]
fn rows_with_a_non_positive_centre_are_dropped() {
    for centre in [0.0, -10.0, f32::NAN, f32::INFINITY] {
        for width in [45.0, 2000.0] {
            let rows = [[centre, width, 0.2], [540.0, 2000.0, 0.1]];
            let material = GemMaterial::sapphire().with_body_color_bands(&rows, 1.0);
            let kept = &material.absorption.o_ray;
            assert_eq!(kept.len(), 1, "centre {centre}, width {width}");
            assert_eq!(kept[0].center_nm, 540.0);
            // Every kept band evaluates to a finite number (no `1e7 / 0`).
            assert!(kept[0].evaluate(550.0).is_finite());
        }
    }
    // A set that is only unusable rows is a colourless stone.
    let none = GemMaterial::sapphire().with_body_color_bands(&[[0.0, 2000.0, 0.2]], 1.0);
    assert!(none.absorption.o_ray.is_empty());
}

#[test]
fn valid_seven_band_rows_are_unchanged() {
    let bands = body_color_bands([0.2; 7]);
    assert_eq!(bands.len(), 7);
    let rows: Vec<[f32; 3]> = bands
        .iter()
        .map(|b| [b.center_nm, b.width_nm, b.peak])
        .collect();
    let material = GemMaterial::sapphire().with_body_color_bands(&rows, 1.0);
    let back = &material.absorption.o_ray;
    assert_eq!(back.len(), bands.len());
    for (before, after) in bands.iter().zip(back) {
        assert_eq!(before.center_nm, after.center_nm);
        assert_eq!(before.width_nm, after.width_nm);
        assert_eq!(before.peak, after.peak);
        assert_eq!(before.shape, after.shape);
    }
}

#[test]
fn a_zero_centre_band_fails_validation() {
    for centre in [0.0, -10.0] {
        let zoned =
            ZonedAbsorption::new(ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![
                AbsorptionBand::energy(centre, 4000.0, 0.2),
            ])));
        assert_eq!(
            zoned.validate(),
            Err(ZoningError::BadBands { zone: None }),
            "centre {centre}"
        );
    }
}
