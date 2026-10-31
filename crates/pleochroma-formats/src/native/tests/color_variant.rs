//! A design's `[material.color_variant]` table in the text of a design file.

use super::*;

fn band(center_nm: f64, width_nm: f64, peak: f64, shape: &str) -> ColorVariantBandTable {
    ColorVariantBandTable {
        center_nm,
        width_nm,
        peak,
        shape: shape.to_owned(),
    }
}

fn variant(beta: bool) -> ColorVariantTable {
    ColorVariantTable {
        key: "sapphire.padparadscha".to_owned(),
        label: "Padparadscha".to_owned(),
        unit: COLOR_VARIANT_UNIT_PER_MM.to_owned(),
        reference_path_mm: 7.5,
        fallback_rgb: Some([0.2, 0.4, 2.8]),
        o_ray: vec![
            band(550.0, 1500.5, 0.35, COLOR_VARIANT_SHAPE_ENERGY),
            band(420.0, 42.25, 0.125, COLOR_VARIANT_SHAPE_WAVELENGTH),
        ],
        e_ray: vec![band(600.0, 1700.0, 0.0625, COLOR_VARIANT_SHAPE_ENERGY)],
        beta_ray: beta.then(|| vec![band(500.0, 40.0, 0.2, COLOR_VARIANT_SHAPE_WAVELENGTH)]),
        pleochroic: true,
        unknown: toml::Table::new(),
    }
}

/// The variant table round trips through the text of a file, with and without a beta ray, and
/// sits under `[material]` as `[material.color_variant]`.
#[test]
fn a_color_variant_round_trips_through_toml_text() {
    for beta in [false, true] {
        let mut native = sample_file();
        native.material = native.material.with_color_variant(Some(variant(beta)));
        let text = write(&native);
        assert!(text.contains("[material.color_variant]"), "{text}");
        assert!(text.contains("shape = \"energy\""), "{text}");
        assert_eq!(text.contains("beta_ray"), beta, "{text}");
        assert!(text.contains("fallback_rgb"), "{text}");
        let parsed = read(&text);
        assert_eq!(parsed.material.color_variant, Some(variant(beta)));
        assert_eq!(parsed, native);
    }
}

/// No variant: the table is not written at all, and a file without it (every file written
/// before the table existed) loads back as `None`.
#[test]
fn a_material_without_a_color_variant_omits_the_table_and_loads_as_none() {
    let text = write(&sample_file());
    assert!(!text.contains("color_variant"));
    assert_eq!(read(&text).material.color_variant, None);
}

/// A hand-written table with the optional `beta_ray` and the arrays left out reads as an
/// empty colorless variant, and an integer where a float is expected still reads.
#[test]
fn a_sparse_hand_written_variant_table_reads() {
    let text = hand_written(1, BLOCK_PREFORM, "").replace(
        "name = \"Diamond\"\n",
        "name = \"Diamond\"\n\n[material.color_variant]\nkey = \"custom\"\n\
         label = \"Custom Color\"\nunit = \"model_unit\"\nreference_path_mm = 7\n\
         pleochroic = false\n",
    );
    let variant = read(&text).material.color_variant.expect("the table loads");
    assert_eq!(variant.key, "custom");
    assert_eq!(variant.unit, COLOR_VARIANT_UNIT_MODEL_UNIT);
    assert!(variant.o_ray.is_empty() && variant.e_ray.is_empty());
    assert_eq!(variant.beta_ray, None);
    // A table from before the fallback triple was recorded inside it reads as "no record".
    assert_eq!(variant.fallback_rgb, None);
}

/// With no recorded fallback triple the key is not written at all.
#[test]
fn a_variant_without_a_recorded_fallback_omits_the_key() {
    let mut native = sample_file();
    let mut table = variant(false);
    table.fallback_rgb = None;
    native.material = native.material.with_color_variant(Some(table.clone()));
    let text = write(&native);
    assert!(!text.contains("fallback_rgb"), "{text}");
    assert_eq!(read(&text).material.color_variant, Some(table));
}
