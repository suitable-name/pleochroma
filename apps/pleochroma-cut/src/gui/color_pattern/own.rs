//! The color a stone has without a pattern, per direction: what a typical pattern is built on.
//!
//! A typical pattern on a uniaxial or biaxial stone never has a one-color zone (FABLE ui finding
//! 3, lead decision D-E): the stone's own color is its variant (the design's snapshot, else the
//! Material Default) and the shades of it are built direction by direction. `cut-core` cannot call
//! the color editor, so this module computes the `[L*, C*, h]` of each direction with
//! `lch_color::axis_lch_from_tensor` and hands them to `OwnColor`.

use pleochroma::optics::materials::{
    GemMaterial,
    variants::{ColorVariant, TensorRank, VariantProvenance, host_rank},
};
use pleochroma_cut_core::material::{
    VariantChoice,
    pattern::{OwnColor, PatternColor},
};
use pleochroma_editor::lch_color::{axis_lch_from_tensor, reference_path_mm};

/// How many principal directions a stone of `rank` has.
const fn directions(rank: TensorRank) -> usize {
    match rank {
        TensorRank::Isotropic => 1,
        TensorRank::Uniaxial => 2,
        TensorRank::Biaxial => 3,
    }
}

/// The variant a design's snapshot stands for, as a variant of `host` (a saved or custom color
/// is a user variant; the pattern copies its per-millimeter rows).
fn variant_of(choice: &VariantChoice, host: &GemMaterial) -> ColorVariant {
    ColorVariant {
        key: choice.key.clone(),
        label: choice.label.clone(),
        material: host.name.clone(),
        absorption: choice.absorption.clone(),
        unit: choice.unit,
        reference_path_mm: choice.reference_path_mm,
        provenance: VariantProvenance::User,
    }
}

/// The Material Default among `variants` when it is exactly the color `material` has (a stone
/// with a legacy color, a tint or a Custom Color is not the Material Default).
fn material_default(material: &GemMaterial, variants: &[ColorVariant]) -> Option<ColorVariant> {
    variants
        .first()
        .filter(|variant| {
            variant.provenance == VariantProvenance::MaterialDefault
                && variant.unit == material.absorption_unit
                && variant.absorption == material.absorption
        })
        .cloned()
}

/// The own color of the stone `material` (the stone as it looks without a pattern).
///
/// A stone without pleochroism has one color: `seed_lch`, the `[L*, C*, h]` its color solves to. A
/// uniaxial or biaxial one has one `[L*, C*, h]` per direction of its tensor, read the way the
/// render shows the stone (`lch_color::axis_lch_from_tensor` at `stone_width_mm`), and its own
/// color as a variant: `choice` (the variant the design carries), else the Material Default of
/// `variants` (the stone's `zone_variants`) when it is the stone's color, else none.
#[must_use]
pub fn own_color(
    material: &GemMaterial,
    choice: Option<&VariantChoice>,
    variants: &[ColorVariant],
    seed_lch: [f64; 3],
    stone_width_mm: f64,
) -> OwnColor {
    let rank = host_rank(material);
    if rank == TensorRank::Isotropic {
        return OwnColor::single(seed_lch);
    }
    let mut axes = axis_lch_from_tensor(
        &material.absorption,
        material.absorption_unit,
        rank,
        stone_width_mm,
        reference_path_mm(Some(stone_width_mm)),
    );
    // One entry per direction of the rank, whatever the reader returned.
    axes.truncate(directions(rank));
    while axes.len() < directions(rank) {
        axes.push(axes.first().copied().unwrap_or(seed_lch));
    }
    let variant = choice
        .map(|choice| variant_of(choice, material))
        .or_else(|| material_default(material, variants))
        .map(|variant| PatternColor::from_variant(&variant));
    OwnColor::per_axis(&axes, variant)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::color_pattern::zone_variants;
    use pleochroma_cut_core::material::pattern::typical_patterns;

    const SEED: [f64; 3] = [40.0, 45.0, 265.0];

    #[test]
    fn a_sapphire_has_a_color_per_direction_and_its_material_default_as_variant() {
        let sapphire = GemMaterial::sapphire();
        let variants = zone_variants(&sapphire, None);
        let own = own_color(&sapphire, None, &variants, SEED, 7.0);
        assert_eq!(own.axes().len(), 2);
        // The directions are read at the face-up path: a per-model-unit tensor is not read
        // 5 to 10 times too deep (the stone is blue, not black).
        assert!(
            own.axes().iter().all(|lch| lch[0] > 10.0),
            "{:?}",
            own.axes()
        );
        assert_eq!(
            own.variant().and_then(PatternColor::variant_key),
            Some("sapphire.default")
        );
    }

    #[test]
    fn the_variant_a_design_carries_is_the_own_color() {
        let sapphire = GemMaterial::sapphire();
        let variants = zone_variants(&sapphire, None);
        let yellow = variants
            .iter()
            .find(|variant| variant.key == "sapphire.yellow")
            .expect("the data file lists the yellow sapphire");
        let choice = VariantChoice::from_variant(yellow);
        let stone = sapphire.with_color_variant_tensor(choice.absorption.clone(), choice.unit);
        let own = own_color(&stone, Some(&choice), &variants, SEED, 7.0);
        assert_eq!(
            own.variant().and_then(PatternColor::variant_key),
            Some("sapphire.yellow")
        );
        assert_eq!(own.axes().len(), 2);
    }

    #[test]
    fn a_legacy_color_on_a_pleochroic_stone_has_no_variant_but_still_two_directions() {
        let tinted = GemMaterial::sapphire().with_body_color([0.1, 0.2, 0.4]);
        let variants = zone_variants(&tinted, None);
        let own = own_color(&tinted, None, &variants, SEED, 7.0);
        assert_eq!(own.variant(), None, "the tint is not the Material Default");
        assert_eq!(own.axes().len(), 2);
    }

    #[test]
    fn a_stone_without_pleochroism_has_one_color_and_builds_one_color_zones() {
        let diamond = GemMaterial::by_name("Diamond").expect("built in");
        let own = own_color(&diamond, None, &zone_variants(&diamond, None), SEED, 7.0);
        assert_eq!(own, OwnColor::single(SEED));
    }

    #[test]
    fn a_typical_pattern_on_a_sapphire_has_no_one_color_zone() {
        let sapphire = GemMaterial::sapphire();
        let variants = zone_variants(&sapphire, None);
        let own = own_color(&sapphire, None, &variants, SEED, 7.0);
        for entry in typical_patterns("Sapphire") {
            let pattern = entry.for_stone_on(&own, &variants);
            for zone in std::iter::once(&pattern.base).chain(&pattern.zones) {
                assert!(
                    !matches!(zone.color, PatternColor::Single(_)),
                    "{} / {}",
                    entry.label,
                    zone.name
                );
            }
        }
    }
}
