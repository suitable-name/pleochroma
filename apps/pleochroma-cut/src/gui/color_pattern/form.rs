//! The color forms of a zone: how many colors a picked zone color has, by the rank of the stone's
//! material, which color tiles the selected layer shows and which slot a tile stands for.
//!
//! A cubic or amorphous material shows one color, a uniaxial one an Along and an Across color, a
//! biaxial one an Alpha, a Beta and a Gamma color. A zone that holds another form keeps it: a
//! stored one-color zone on a pleochroic material is a legacy zone. It renders as saved, the
//! window says so, and "Convert" gives it the material's own form.

use pleochroma::optics::materials::variants::TensorRank;
use pleochroma_cut_core::material::pattern::{ColorForm, ColorSlot, PatternColor};

/// The form of a picked color; `None` for a variant.
pub(super) const fn form_of(color: &PatternColor) -> Option<ColorForm> {
    match color {
        PatternColor::Single(_) => Some(ColorForm::Single),
        PatternColor::Dichroic { .. } => Some(ColorForm::Dichroic),
        PatternColor::Trichroic { .. } => Some(ColorForm::Trichroic),
        PatternColor::Variant { .. } => None,
    }
}

/// The form a material of `rank` asks of a picked color.
pub(super) const fn host_form(rank: TensorRank) -> ColorForm {
    match rank {
        TensorRank::Isotropic => ColorForm::Single,
        TensorRank::Uniaxial => ColorForm::Dichroic,
        TensorRank::Biaxial => ColorForm::Trichroic,
    }
}

/// Whether `color` is a legacy one-color zone: one picked color on a material that shows more
/// than one.
pub(super) fn is_legacy(rank: TensorRank, color: &PatternColor) -> bool {
    form_of(color) == Some(ColorForm::Single) && rank != TensorRank::Isotropic
}

/// The note at the right of the Color row: what the material's rank allows.
pub(super) const fn rank_text(rank: TensorRank) -> &'static str {
    match rank {
        TensorRank::Isotropic => "One color \u{b7} isotropic",
        TensorRank::Uniaxial => "Pleochroic \u{b7} uniaxial",
        TensorRank::Biaxial => "Pleochroic \u{b7} biaxial",
    }
}

/// The slot the color tile `which` stands for in a zone holding `color`.
///
/// Slots run 0 for the one color, then 1 Along or Alpha, 2 Across or Beta, 3 Gamma. `None` for
/// a tile the zone's form does not have, and for a variant (which has no picked color to change
/// in place).
pub(super) const fn slot_for(color: &PatternColor, which: i32) -> Option<ColorSlot> {
    match (color, which) {
        (PatternColor::Single(_), 0) => Some(ColorSlot::Single),
        (PatternColor::Dichroic { .. }, 1) => Some(ColorSlot::Along),
        (PatternColor::Dichroic { .. }, 2) => Some(ColorSlot::Across),
        (PatternColor::Trichroic { .. }, 1) => Some(ColorSlot::Alpha),
        (PatternColor::Trichroic { .. }, 2) => Some(ColorSlot::Beta),
        (PatternColor::Trichroic { .. }, 3) => Some(ColorSlot::Gamma),
        _ => None,
    }
}

/// Which swatch of a zone a color tile paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SwatchPart {
    /// The zone's overall color (a one-color variant).
    Blend,
    /// The color, or the Along (Alpha) color.
    Primary,
    /// The Beta color of a three-color zone.
    Beta,
    /// The Across (Gamma) color.
    Across,
}

/// One color tile of the selected layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TileSpec {
    /// The caption on the tile.
    pub caption: &'static str,
    /// The tile's slot (`PatternColorTile.slot`): 0 the one color, 1 Along or Alpha, 2 Across or
    /// Beta, 3 Gamma.
    pub slot: i32,
    /// The swatch the tile shows.
    pub part: SwatchPart,
}

/// The two tiles of a uniaxial zone.
const TWO_TILES: [TileSpec; 2] = [
    TileSpec {
        caption: "Along c",
        slot: 1,
        part: SwatchPart::Primary,
    },
    TileSpec {
        caption: "Across c",
        slot: 2,
        part: SwatchPart::Across,
    },
];

/// The three tiles of a biaxial zone.
const THREE_TILES: [TileSpec; 3] = [
    TileSpec {
        caption: "\u{3b1}",
        slot: 1,
        part: SwatchPart::Primary,
    },
    TileSpec {
        caption: "\u{3b2}",
        slot: 2,
        part: SwatchPart::Beta,
    },
    TileSpec {
        caption: "\u{3b3}",
        slot: 3,
        part: SwatchPart::Across,
    },
];

/// The tile of a one-color zone, painted with `part`.
const fn one_tile(part: SwatchPart) -> TileSpec {
    TileSpec {
        caption: "Color",
        slot: 0,
        part,
    }
}

/// The color tiles of a zone holding `color`: one, two or three, by what the color has. A variant
/// shows the tiles of its own tensor.
pub(super) fn tile_specs(color: &PatternColor) -> Vec<TileSpec> {
    match color {
        PatternColor::Single(_) => vec![one_tile(SwatchPart::Primary)],
        PatternColor::Dichroic { .. } => TWO_TILES.to_vec(),
        PatternColor::Trichroic { .. } => THREE_TILES.to_vec(),
        PatternColor::Variant { absorption, .. } => {
            if absorption.beta_ray.is_some() {
                THREE_TILES.to_vec()
            } else if absorption.pleochroic {
                TWO_TILES.to_vec()
            } else {
                vec![one_tile(SwatchPart::Blend)]
            }
        }
    }
}

/// The title of the picker opened for tile `which` of a zone holding `color`: the direction the
/// color is seen along.
pub(super) fn picker_title(color: &PatternColor, which: i32) -> &'static str {
    tile_specs(color)
        .into_iter()
        .find(|tile| tile.slot == which)
        .map_or("Color", |tile| match tile.caption {
            "Along c" => "Along the c-axis",
            "Across c" => "Across the c-axis",
            "\u{3b1}" => "Along alpha (\u{3b1})",
            "\u{3b2}" => "Along beta (\u{3b2})",
            "\u{3b3}" => "Along gamma (\u{3b3})",
            _ => "Color",
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::material::pattern::{PatternTensorRows, ZoneColor};

    fn one() -> PatternColor {
        PatternColor::Single(ZoneColor::unsolved([50.0, 30.0, 100.0]))
    }

    fn two() -> PatternColor {
        PatternColor::Dichroic {
            along: ZoneColor::unsolved([50.0, 30.0, 100.0]),
            across: ZoneColor::unsolved([50.0, 30.0, 200.0]),
        }
    }

    fn three() -> PatternColor {
        PatternColor::Trichroic {
            alpha: ZoneColor::unsolved([50.0, 30.0, 100.0]),
            beta: ZoneColor::unsolved([50.0, 30.0, 150.0]),
            gamma: ZoneColor::unsolved([50.0, 30.0, 200.0]),
        }
    }

    fn variant(pleochroic: bool, beta: bool) -> PatternColor {
        PatternColor::Variant {
            key: "sapphire.test".to_owned(),
            label: "Test".to_owned(),
            absorption: PatternTensorRows {
                o_ray: Vec::new(),
                e_ray: Vec::new(),
                beta_ray: beta.then(Vec::new),
                pleochroic,
            },
        }
    }

    #[test]
    fn a_one_color_zone_on_a_pleochroic_material_is_legacy() {
        assert!(is_legacy(TensorRank::Uniaxial, &one()));
        assert!(is_legacy(TensorRank::Biaxial, &one()));
        assert!(!is_legacy(TensorRank::Isotropic, &one()));
        assert!(!is_legacy(TensorRank::Uniaxial, &two()));
        assert!(!is_legacy(TensorRank::Uniaxial, &variant(false, false)));
    }

    #[test]
    fn the_tiles_follow_the_colors_a_zone_has() {
        let captions = |color: &PatternColor| -> Vec<&str> {
            tile_specs(color).iter().map(|tile| tile.caption).collect()
        };
        assert_eq!(captions(&one()), ["Color"]);
        assert_eq!(captions(&two()), ["Along c", "Across c"]);
        assert_eq!(captions(&three()), ["\u{3b1}", "\u{3b2}", "\u{3b3}"]);
        assert_eq!(captions(&variant(false, false)), ["Color"]);
        assert_eq!(captions(&variant(true, false)), ["Along c", "Across c"]);
        assert_eq!(captions(&variant(true, true)).len(), 3);
        assert_eq!(
            tile_specs(&variant(false, false))[0].part,
            SwatchPart::Blend
        );
    }

    #[test]
    fn a_tile_stands_for_the_slot_it_names() {
        for color in [one(), two(), three()] {
            for tile in tile_specs(&color) {
                assert!(
                    slot_for(&color, tile.slot).is_some(),
                    "{} of {color:?}",
                    tile.caption
                );
            }
        }
        assert_eq!(slot_for(&one(), 1), None);
        assert_eq!(slot_for(&two(), 0), None);
        assert_eq!(slot_for(&two(), 1), Some(ColorSlot::Along));
        assert_eq!(slot_for(&two(), 2), Some(ColorSlot::Across));
        assert_eq!(slot_for(&three(), 1), Some(ColorSlot::Alpha));
        assert_eq!(slot_for(&three(), 2), Some(ColorSlot::Beta));
        assert_eq!(slot_for(&three(), 3), Some(ColorSlot::Gamma));
        assert_eq!(slot_for(&three(), 4), None);
        // A variant has nothing to change in place: a tile turns it into a picked color first.
        for which in 0..4 {
            assert_eq!(slot_for(&variant(true, true), which), None);
        }
    }

    #[test]
    fn the_host_form_follows_the_rank() {
        assert_eq!(host_form(TensorRank::Isotropic), ColorForm::Single);
        assert_eq!(host_form(TensorRank::Uniaxial), ColorForm::Dichroic);
        assert_eq!(host_form(TensorRank::Biaxial), ColorForm::Trichroic);
        assert_eq!(form_of(&variant(true, false)), None);
    }

    #[test]
    fn the_picker_names_the_direction() {
        assert_eq!(picker_title(&two(), 1), "Along the c-axis");
        assert_eq!(picker_title(&two(), 2), "Across the c-axis");
        assert_eq!(picker_title(&one(), 0), "Color");
        assert_eq!(picker_title(&three(), 2), "Along beta (\u{3b2})");
    }
}
