//! The render-physics keys of the design file: `[render_effects.inclusions]`,
//! `[render_effects.structure]`, the `edge_abrasion_*` keys and the tier surface columns. Each is
//! written only when set, so a file without them keeps its bytes.

use super::{tests::sample, *};
use crate::native::{SurfaceColumns, SurfaceFacetTable};

fn inclusions() -> InclusionEffectsTable {
    InclusionEffectsTable {
        preset: "Fine Silk".to_string(),
        density: 0.5,
        turn_deg: Some(30.0),
        sheen: None,
        color: Some("Slightly Blue".to_string()),
        unknown: toml::Table::new(),
    }
}

fn structure() -> StructureEffectsTable {
    StructureEffectsTable {
        pattern: "Harlequin".to_string(),
        seed: 12_345,
        size: None,
        color_low_nm: 440.0,
        color_high_nm: 680.0,
        purity: 0.05,
        coverage: 0.8,
        unknown: toml::Table::new(),
    }
}

#[test]
fn a_file_without_effects_has_no_render_effects_keys() {
    let text = to_string(&sample()).expect("serializes");
    assert!(!text.contains("render_effects"));
    assert!(!text.contains("edge_abrasion"));
    assert!(!text.contains("surface"));
}

#[test]
fn the_subtables_and_edge_keys_round_trip_and_stay_byte_identical() {
    let effects = RenderEffectsTable {
        edge_abrasion_width: 0.02,
        edge_abrasion_alpha: 0.35,
        inclusions: Some(inclusions()),
        structure: Some(structure()),
        ..RenderEffectsTable::default()
    };
    let file = sample().with_render_effects(effects);
    let text = to_string(&file).expect("serializes");
    assert!(text.contains("[render_effects.inclusions]"), "{text}");
    assert!(text.contains("[render_effects.structure]"), "{text}");
    assert!(text.contains("edge_abrasion_width = 0.02"), "{text}");
    assert!(
        !text.contains("edge_abrasion_halo_rq_nm"),
        "a zero key is not written"
    );
    assert!(!text.contains("sheen"), "an unset option is not written");
    let parsed = parse(&text).expect("parses");
    assert_eq!(parsed, file);
    assert_eq!(to_string(&parsed).expect("serializes"), text);
}

#[test]
fn a_subtable_alone_makes_the_effects_table_non_empty() {
    let only_structure = RenderEffectsTable {
        structure: Some(structure()),
        ..RenderEffectsTable::default()
    };
    assert!(!only_structure.is_empty());
    assert!(RenderEffectsTable::default().is_empty());
    assert!(
        sample()
            .with_render_effects(RenderEffectsTable::default())
            .render_effects
            .is_none()
    );
}

#[test]
fn missing_keys_of_the_subtables_take_their_defaults() {
    let mut text = to_string(&sample()).expect("serializes");
    text.push_str("\n[render_effects.inclusions]\npreset = \"Rain\"\n");
    let file = parse(&text).expect("parses");
    let table = file
        .render_effects
        .and_then(|effects| effects.inclusions)
        .expect("the table is read");
    assert_eq!(table.preset, "Rain");
    assert_eq!(table.density.to_bits(), 0.35f64.to_bits());
    assert_eq!(table.turn_deg, None);
}

#[test]
fn unknown_keys_of_the_subtables_survive() {
    let mut table = structure();
    table
        .unknown
        .insert("future_key".to_string(), toml::Value::Integer(7));
    let file = sample().with_render_effects(RenderEffectsTable {
        structure: Some(table),
        ..RenderEffectsTable::default()
    });
    let text = to_string(&file).expect("serializes");
    assert!(text.contains("future_key = 7"), "{text}");
    assert_eq!(parse(&text).expect("parses"), file);
}

#[test]
fn tier_surface_columns_round_trip_and_an_unset_tier_writes_none() {
    let columns = SurfaceColumns {
        surface: Some("Custom".to_string()),
        rq_nm: Some(12.5),
        lap_deg: Some(30.0),
        aniso: Some(2.5),
        no_sweep: true,
        ground_alpha: Some(0.4),
        facets: vec![
            SurfaceFacetTable {
                at: 24.0,
                surface: Some("Prepolish".to_string()),
                ..SurfaceFacetTable::default()
            },
            SurfaceFacetTable {
                at: 0.0,
                ..SurfaceFacetTable::default()
            },
        ],
    };
    let mut file = sample();
    file.tiers[1] = file.tiers[1].clone().with_surface(columns.clone());
    let text = to_string(&file).expect("serializes");
    assert!(text.contains("surface = \"Custom\""), "{text}");
    assert!(text.contains("surface_rq_nm = 12.5"), "{text}");
    assert!(text.contains("surface_lap_deg = 30.0"), "{text}");
    assert!(text.contains("surface_aniso = 2.5"), "{text}");
    assert!(text.contains("surface_no_sweep = true"), "{text}");
    assert!(text.contains("ground_alpha = 0.4"), "{text}");
    assert!(text.contains("surface_facets"), "{text}");
    let parsed = parse(&text).expect("parses");
    assert_eq!(parsed.tiers[1].surface_columns(), columns);
    assert_eq!(parsed.tiers[0].surface_columns(), SurfaceColumns::default());
    assert_eq!(to_string(&parsed).expect("serializes"), text);
    // Only the tier and its one named facet carry a grade word; the first tier carries nothing.
    assert_eq!(text.matches("surface = ").count(), 2, "{text}");
    assert_eq!(parsed.version, DESIGN_VERSION, "no version bump");
}
