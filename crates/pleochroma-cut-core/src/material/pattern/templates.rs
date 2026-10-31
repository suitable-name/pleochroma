//! The built-in templates: [`template`] gives a pattern for a unit-wide stone.
//!
//! All geometry is in the pattern frame (the pattern axis is `+Y`) for a stone one unit wide
//! (half a unit from the center to the girdle), with `relative_to_stone = true`. The colors
//! are `[L*, C*, h]` only and carry no band rows: the application solves them with the
//! body-color solver when the template is chosen (`ColorPattern::unsolved_colors`).

use super::{
    ColorPattern, PATTERN_BASE_FORMAT_VERSION, PatternAxis, PatternColor, PatternZone, TemplateId,
    ZoneColor,
    typical::{
        DEFAULT_OWN_COLOR, OwnColor, color_band_parts, hexagonal_banding_parts, parti_color_parts,
    },
};
use glam::DVec3;
use pleochroma::optics::zoning::ZoneShape;
use std::f64::consts::{PI, TAU};

/// Rim green of a watermelon tourmaline.
pub(super) const GREEN: [f64; 3] = [55.0, 35.0, 140.0];
/// Core pink of a watermelon tourmaline.
pub(super) const PINK: [f64; 3] = [60.0, 35.0, 0.0];
/// A pale tint for backgrounds and middles.
pub(super) const PALE: [f64; 3] = [90.0, 8.0, 90.0];
/// Amethyst purple.
const PURPLE: [f64; 3] = [50.0, 40.0, 310.0];
/// Citrine yellow.
const YELLOW: [f64; 3] = [85.0, 70.0, 90.0];
/// Amber.
const AMBER: [f64; 3] = [70.0, 40.0, 70.0];
/// Violet.
const VIOLET: [f64; 3] = [45.0, 45.0, 300.0];

/// The apothem of a watermelon core, as a fraction of the stone width.
const WATERMELON_CORE_APOTHEM: f64 = 0.35;
/// The radius of a round core, as a fraction of the stone width.
const ROUND_CORE_RADIUS: f64 = 0.3;
/// Layers reach this far from the center, in stone widths (beyond any stone).
const LAYER_REACH: f64 = 1.0;

/// An unsolved one-color zone color of the picked `[L*, C*, h]`.
pub(super) const fn single(lch: [f64; 3]) -> PatternColor {
    PatternColor::Single(ZoneColor::unsolved(lch))
}

pub(super) fn zone(name: &str, shape: ZoneShape, lch: [f64; 3]) -> PatternZone {
    zone_with(name, shape, single(lch))
}

/// A zone with an already built color (a picked color, or a variant).
pub(super) fn zone_with(name: &str, shape: ZoneShape, color: PatternColor) -> PatternZone {
    PatternZone {
        name: name.to_owned(),
        shape,
        color,
    }
}

pub(super) fn base(name: &str, lch: [f64; 3]) -> PatternZone {
    PatternZone::base(name, single(lch))
}

/// The layer between two offsets along the pattern axis.
const fn layer(from: f64, to: f64) -> ZoneShape {
    ZoneShape::Slab {
        normal: DVec3::Y,
        offset_min: from,
        offset_max: to,
    }
}

pub(super) const fn wedge(from: f64, to: f64) -> ZoneShape {
    ZoneShape::Sector {
        axis_point: DVec3::ZERO,
        axis_dir: DVec3::Y,
        angle_from: from,
        angle_to: to,
    }
}

/// A watermelon: `rim` is the base color, `core` the triangular core's.
fn watermelon(rim: [f64; 3], core: [f64; 3]) -> (PatternZone, Vec<PatternZone>) {
    (
        base("Rim", rim),
        vec![zone(
            "Core",
            ZoneShape::CoaxialPrism {
                axis_point: DVec3::ZERO,
                axis_dir: DVec3::Y,
                n_sides: 3,
                r_in: 0.0,
                r_out: WATERMELON_CORE_APOTHEM,
                phase: 0.0,
            },
            core,
        )],
    )
}

/// The pattern of template `id` for a unit-wide stone, oriented by `axis`.
///
/// | Template | Zones (base first) |
/// |---|---|
/// | Uniform | the base only (no zones) |
/// | Bicolor | base + a half space (normal = the pattern axis, offset 0) |
/// | Tricolor | base (the middle) + 2 layers across the axis |
/// | Banded | base + 4 equally spaced layers |
/// | Watermelon | base (rim) + a solid 3-sided prism, apothem 0.35 of the width (core) |
/// | Reverse Watermelon | the same with the colors swapped |
/// | Core and Rim | base (rim) + a solid cylinder, radius 0.3 of the width |
/// | Split | base + a half space through the axis (as Ametrine) |
/// | Sectors | base + 2 sectors |
/// | Hexagonal Banding | base (pale) + 3 concentric 6-sided shells (a blue sapphire's colors) |
/// | Parti Color | base (yellow rim) + a solid 6-sided prism (blue core) |
/// | Color Band | base + one slab through the axis |
///
/// The last three are the starting points of the typical zoning of the corundum family; the
/// per-stone versions take the stone's own color ([`super::typical_patterns`]).
///
/// Every color is `[L*, C*, h]` only (no band rows), so [`ColorPattern::to_zoned`] refuses the
/// pattern until the application solved them ([`ColorPattern::unsolved_colors`]); a Uniform
/// pattern stores no zones and needs no solution to be the plain material.
#[must_use]
pub fn template(id: TemplateId, axis: PatternAxis) -> ColorPattern {
    let (base, zones) = match id {
        TemplateId::Uniform => (base("Body", PINK), Vec::new()),
        TemplateId::Bicolor => (
            base("Lower Half", GREEN),
            vec![zone(
                "Upper Half",
                ZoneShape::HalfSpace {
                    normal: DVec3::Y,
                    offset: 0.0,
                },
                PINK,
            )],
        ),
        TemplateId::Tricolor => (
            base("Middle", PALE),
            vec![
                zone("Top", layer(1.0 / 6.0, LAYER_REACH), PINK),
                zone("Bottom", layer(-LAYER_REACH, -1.0 / 6.0), GREEN),
            ],
        ),
        TemplateId::Banded => (
            base("Background", PALE),
            (0..4_u8)
                .map(|k| {
                    let center = 0.2_f64.mul_add(f64::from(k), -0.3);
                    zone(
                        &format!("Band {}", k + 1),
                        layer(center - 0.05, center + 0.05),
                        PURPLE,
                    )
                })
                .collect(),
        ),
        TemplateId::Watermelon => watermelon(GREEN, PINK),
        TemplateId::ReverseWatermelon => watermelon(PINK, GREEN),
        TemplateId::CoreAndRim => (
            base("Rim", AMBER),
            vec![zone(
                "Core",
                ZoneShape::CoaxialCylinder {
                    axis_point: DVec3::ZERO,
                    axis_dir: DVec3::Y,
                    r_in: 0.0,
                    r_out: ROUND_CORE_RADIUS,
                },
                VIOLET,
            )],
        ),
        TemplateId::Split => (
            base("Left Half", PURPLE),
            vec![zone(
                "Right Half",
                ZoneShape::HalfSpace {
                    normal: DVec3::X,
                    offset: 0.0,
                },
                YELLOW,
            )],
        ),
        TemplateId::Sectors => (
            base("Background", PALE),
            vec![
                zone("Sector 1", wedge(0.0, TAU / 3.0), PURPLE),
                zone("Sector 2", wedge(PI, PI + TAU / 3.0), YELLOW),
            ],
        ),
        TemplateId::HexagonalBanding => {
            hexagonal_banding_parts(&OwnColor::single(DEFAULT_OWN_COLOR))
        }
        TemplateId::PartiColor => parti_color_parts(&OwnColor::single(DEFAULT_OWN_COLOR)),
        TemplateId::ColorBand => color_band_parts(&OwnColor::single(DEFAULT_OWN_COLOR)),
    };
    ColorPattern {
        version: PATTERN_BASE_FORMAT_VERSION,
        template: Some(id),
        base,
        zones,
        softness: 0.0,
        axis,
        relative_to_stone: true,
    }
}
