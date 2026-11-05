//! Typical color zoning: the patterns that gemmologists know a species for, ready to apply in
//! one step.
//!
//! [`typical_patterns`] lists, for a material name, the patterns a stone of that species is
//! typically zoned in (a sapphire in hexagonal growth bands, a tourmaline as a watermelon, an
//! amethyst in sectors). Each entry builds a [`ColorPattern`] from the stone's own color, so a
//! blue sapphire bands in its own blue; a second hue appears only where the species is known for
//! one (the yellow rim of a parti-color sapphire, the pink and green of a tourmaline).
//!
//! The patterns are the same kind of thing a template is ([`super::template`]): geometry in the
//! pattern frame for a stone one unit wide (`+Y` is the crystal's c axis), `relative_to_stone`,
//! colors as `[L*, C*, h]` only. The application solves the colors before it applies the pattern
//! ([`ColorPattern::unsolved_colors`]). The pattern axis is "Follows Crystal Axis", so the zones
//! stay on the crystal when the stone's axis moves; a cubic species (fluorite), which has no
//! Crystal Axis, gets a fixed axis in the table plane instead, so its bands stand upright
//! ([`TypicalPattern::axis`]).
//!
//! # Variants
//!
//! A builder also gets the color variants of the stone's material (valid for the stone: the caller
//! filters them with `ColorVariant::validate_for`). Where a zone's intended color is one the species
//! has a variant for, that zone is a [`PatternColor::Variant`] instead of an `[L*, C*, h]` seed:
//! the rubellite core and verdelite rim of a tourmaline, the blue core and yellow rim of a
//! parti-color sapphire, the amethyst and citrine of an ametrine. A small table (`SHADE_VARIANTS`)
//! says which variant key supplies which shade; a variant a research lane adds later plugs in as
//! one row. A species without a matching variant keeps the `[L*, C*, h]` seed, so a stone with no
//! variants builds the same patterns as before.
//!
//! Species are matched by the exact name of the built-in material, ASCII case ignored. A custom
//! material carries no species, so it has no typical zoning of its own; it can still be zoned by
//! hand with the Color Pattern editor.
//!
//! # Pleochroic stones
//!
//! A pattern built on a uniaxial or biaxial stone never has a one-color ([`PatternColor::Single`])
//! zone: that zone would render without pleochroism. The stone's own color is an [`OwnColor`]: the
//! `[L*, C*, h]` of each of its directions, and, where the stone has one, the variant that is its
//! color (the design's variant snapshot, else the Material Default). A zone in the stone's own
//! color takes that variant as it is; a paler or stronger shade is built direction by direction
//! ([`PatternColor::Dichroic`] or [`PatternColor::Trichroic`]), and a fixed color (the pink of a
//! watermelon) repeats in every direction. The caller computes the per-direction `[L*, C*, h]`
//! (this crate has no color-editor dependency). A stone without pleochroism keeps one color per
//! zone, so [`OwnColor::single`] builds the patterns the first release built.

use super::{
    ColorPattern, PATTERN_BASE_FORMAT_VERSION, PatternAxis, PatternColor, PatternZone, TemplateId,
    ZoneColor,
    templates::{GREEN, PALE, PINK, single, template, wedge, zone_with},
};
use glam::DVec3;
use pleochroma::optics::{materials::variants::ColorVariant, zoning::ZoneShape};
use std::f64::consts::{FRAC_PI_3, TAU};

/// The `[L*, C*, h]` a typical pattern starts from when a template is built without a stone: a
/// blue sapphire.
pub(super) const DEFAULT_OWN_COLOR: [f64; 3] = [40.0, 45.0, 265.0];

/// Amethyst purple, as the Color Pattern window's Ametrine preset has it. Deeper since 2026-11-03
/// (it was L* 48 C* 42): dealer photos of ametrine show a lavender half more saturated than the
/// yellow one, 40-45 % of the stone, where the Split rendered 21 % purple in a mostly yellow
/// stone (L* 44 C* 48 with the citrine at L* 86 C* 45 overshot to 69 %).
const AMETHYST: [f64; 3] = [46.0, 45.0, 305.0];
/// Citrine yellow, as the Color Pattern window's Ametrine preset has it. Softer and more golden
/// since 2026-11-03 (it was L* 82 C* 72 h 88), the yellow half of the dealer photos.
const CITRINE: [f64; 3] = [84.0, 58.0, 85.0];
/// The yellow rim of a parti-color sapphire.
const PARTI_YELLOW: [f64; 3] = [80.0, 60.0, 90.0];
/// The blue core of a parti-color sapphire that is yellow itself.
const PARTI_BLUE: [f64; 3] = [42.0, 45.0, 265.0];

/// The least chroma of a "stronger" shade, so a nearly colorless stone still shows its zones.
const MIN_STRONG_CHROMA: f64 = 15.0;
/// The most chroma a shade is asked for (the solver takes the nearest color the model reaches).
const MAX_CHROMA: f64 = 100.0;

/// The apothems of the three growth zones of a hexagonal banding, as fractions of the stone
/// width: `(r_in, r_out)`, innermost first.
const GROWTH_ZONES: [(f64, f64); 3] = [(0.06, 0.13), (0.22, 0.29), (0.38, 0.45)];
/// The apothem of the 6-sided core of a parti-color sapphire and of a beryl.
const HEX_CORE_APOTHEM: f64 = 0.30;
/// Where the color band of a sapphire sits across the axis, as fractions of the stone width.
const COLOR_BAND: (f64, f64) = (0.06, 0.22);

/// A color of a typical pattern that a color variant of the species can supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shade {
    /// The pink core of a watermelon tourmaline.
    Rubellite,
    /// The green rim of a watermelon tourmaline.
    Verdelite,
    /// The blue of a parti-color sapphire.
    Blue,
    /// The yellow of a yellow sapphire (the core of a parti-color stone that is yellow itself).
    Yellow,
    /// The yellow rim of a parti-color sapphire: a deeper yellow than a yellow stone's, since the
    /// rim is a thin outer zone that light crosses for a few millimetres only.
    PartiYellow,
    /// The purple of an ametrine.
    Amethyst,
    /// The yellow of an ametrine.
    Citrine,
}

/// Which variant supplies which shade, by variant key; the first row of a shade that the stone's
/// variants hold wins. The tourmaline and sapphire keys are entries of the data files
/// (`data/color_variants/`); the parti-color's blue is the Sapphire's own color, its Material
/// Default (`sapphire.default`). The Ametrine has no body-color variants (its color is its zones),
/// so its two rows only match variants a caller supplies; a variant that is not there leaves its
/// zone on the `[L*, C*, h]` seed.
const SHADE_VARIANTS: [(Shade, &str); 8] = [
    (Shade::Rubellite, "tourmaline.rubellite"),
    (Shade::Verdelite, "tourmaline.verdelite"),
    (Shade::Blue, "sapphire.default"),
    (Shade::Yellow, "sapphire.yellow"),
    // The Yellow variant over a parti rim reads a faint cream (Light Tent C* 4), where real parti
    // sapphires show clear yellow, and green where it overlaps the blue (2026-11-03).
    (Shade::PartiYellow, "sapphire.golden_yellow"),
    (Shade::PartiYellow, "sapphire.yellow"),
    (Shade::Amethyst, "ametrine.amethyst"),
    (Shade::Citrine, "ametrine.citrine"),
];

/// The zone color of the variant that supplies `shade`, if the stone has it.
fn shade_color(variants: &[ColorVariant], shade: Shade) -> Option<PatternColor> {
    SHADE_VARIANTS
        .iter()
        .filter(|(row, _)| *row == shade)
        .find_map(|(_, key)| variants.iter().find(|variant| variant.key == *key))
        .map(PatternColor::from_variant)
}

/// The stone's own color, as a typical pattern is built on it (see the module notes).
///
/// It holds one `[L*, C*, h]` per principal direction of the stone: one for a stone without
/// pleochroism, Along and Across for a uniaxial one, Alpha, Beta and Gamma for a biaxial one. That
/// count is the form of every zone the pattern builds.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnColor {
    /// One to three entries, in slot order.
    axes: Vec<[f64; 3]>,
    /// The variant that is the stone's color, when it has one.
    variant: Option<PatternColor>,
}

impl OwnColor {
    /// A stone with one color in every direction, `lch` (`[L*, C*, h]`).
    #[must_use]
    pub fn single(lch: [f64; 3]) -> Self {
        Self {
            axes: vec![lch],
            variant: None,
        }
    }

    /// A stone with one `[L*, C*, h]` per direction: the first three of `axes` count (Along and
    /// Across, or Alpha, Beta and Gamma), and an empty list is the default blue sapphire.
    ///
    /// `variant` is the color variant that is the stone's own color, if it has one: a zone in the
    /// stone's own color is that variant.
    #[must_use]
    pub fn per_axis(axes: &[[f64; 3]], variant: Option<PatternColor>) -> Self {
        let mut axes: Vec<[f64; 3]> = axes.iter().copied().take(3).collect();
        if axes.is_empty() {
            axes.push(DEFAULT_OWN_COLOR);
        }
        Self { axes, variant }
    }

    /// The `[L*, C*, h]` of the first direction (the only one of a stone without pleochroism, the
    /// Along or Alpha color otherwise): what the hue rules of a pattern look at.
    #[must_use]
    pub fn lch(&self) -> [f64; 3] {
        self.axes.first().copied().unwrap_or(DEFAULT_OWN_COLOR)
    }

    /// The `[L*, C*, h]` of every direction, in slot order (one to three entries).
    #[must_use]
    pub fn axes(&self) -> &[[f64; 3]] {
        &self.axes
    }

    /// How many directions the stone's own color has: 1, 2 or 3.
    #[must_use]
    pub fn directions(&self) -> usize {
        self.axes.len().min(3)
    }

    /// The variant that is the stone's own color, if it has one.
    #[must_use]
    pub const fn variant(&self) -> Option<&PatternColor> {
        self.variant.as_ref()
    }

    /// The stone's own color as a zone color: its variant when it has one, else its colors
    /// direction by direction.
    fn color(&self) -> PatternColor {
        self.variant
            .clone()
            .unwrap_or_else(|| self.shade(|lch| lch))
    }

    /// A color of the stone's form (one, two or three colors) whose color in each direction is
    /// `shade` applied to the stone's own color in that direction.
    fn shade(&self, shade: impl Fn([f64; 3]) -> [f64; 3]) -> PatternColor {
        let unsolved = |lch: [f64; 3]| ZoneColor::unsolved(shade(lch));
        match self.axes.as_slice() {
            [along, across] => PatternColor::Dichroic {
                along: unsolved(*along),
                across: unsolved(*across),
            },
            [alpha, beta, gamma, ..] => PatternColor::Trichroic {
                alpha: unsolved(*alpha),
                beta: unsolved(*beta),
                gamma: unsolved(*gamma),
            },
            _ => single(shade(self.lch())),
        }
    }

    /// A color of the stone's form that is `lch` in every direction (a fixed color of a species,
    /// the pink of a watermelon).
    fn flat(&self, lch: [f64; 3]) -> PatternColor {
        self.shade(|_| lch)
    }
}

/// The builder of one typical pattern: the stone's own color, the color variants of its material
/// and the pattern axis.
type Build = fn(&OwnColor, &[ColorVariant], PatternAxis) -> ColorPattern;

/// One typical pattern of a species.
#[derive(Debug, Clone, Copy)]
pub struct TypicalPattern {
    /// The name shown in the menus ("Hexagonal Banding").
    pub label: &'static str,
    /// The template id stored in the pattern; the pattern "came from" this entry while it is set.
    /// Unique within one species' list.
    pub template: TemplateId,
    /// The pattern for a stone with the given own color and color variants, oriented by the axis.
    pub build: Build,
    /// How the pattern sits in the stone: [`PatternAxis::FollowsCrystalAxis`] for a species with a
    /// c axis, a fixed upright axis ([`PatternAxis::ACROSS`]) for a cubic one.
    pub axis: PatternAxis,
}

impl TypicalPattern {
    const fn new(template: TemplateId, build: Build) -> Self {
        Self {
            label: template.label(),
            template,
            build,
            axis: PatternAxis::FollowsCrystalAxis,
        }
    }

    /// An entry of a cubic species (fluorite): the pattern axis lies in the table plane, so layers
    /// stand upright and show as stripes face up. A cubic stone has no Crystal Axis (the render
    /// ignores it), so a following pattern would keep its layers parallel to the table for good,
    /// and face up the eye would look through all of them at once.
    const fn upright(template: TemplateId, build: Build) -> Self {
        Self {
            axis: PatternAxis::ACROSS,
            ..Self::new(template, build)
        }
    }

    /// The pattern for a stone with the own color `own` and the color variants `variants`.
    ///
    /// The variants are those of the stone's material (valid for the stone; zones whose color is one
    /// of them become [`PatternColor::Variant`]). The zones turn with the stone's Crystal Axis
    /// ([`PatternAxis::FollowsCrystalAxis`]), except for a cubic species, whose layers stand
    /// upright ([`Self::axis`]). Its `[L*, C*, h]` colors are not solved yet.
    ///
    /// A stone with pleochroism has one color per direction in `own` (see [`OwnColor`]), so the
    /// pattern has no one-color zone.
    #[must_use]
    pub fn for_stone_on(&self, own: &OwnColor, variants: &[ColorVariant]) -> ColorPattern {
        (self.build)(own, variants, self.axis)
    }

    /// [`Self::for_stone_on`] for a stone without pleochroism whose own color is `own_lch`
    /// (`[L*, C*, h]`): every zone has one color.
    #[must_use]
    pub fn for_stone(&self, own_lch: [f64; 3], variants: &[ColorVariant]) -> ColorPattern {
        self.for_stone_on(&OwnColor::single(own_lch), variants)
    }
}

static CORUNDUM: [TypicalPattern; 3] = [
    TypicalPattern::new(TemplateId::HexagonalBanding, hexagonal_banding),
    TypicalPattern::new(TemplateId::PartiColor, parti_color),
    TypicalPattern::new(TemplateId::ColorBand, color_band),
];
static RUBY: [TypicalPattern; 1] = [TypicalPattern::new(
    TemplateId::HexagonalBanding,
    hexagonal_banding,
)];
/// Emerald grows in documented six-sided color zones (the Swat and Zhen'an basal slices, the
/// trapiche type), so it keeps Hexagonal Banding.
static EMERALD: [TypicalPattern; 2] = [
    TypicalPattern::new(TemplateId::HexagonalBanding, hexagonal_banding),
    TypicalPattern::new(TemplateId::CoreAndRim, hexagonal_core_and_rim),
];
/// The other beryls (aquamarine, morganite, heliodor): no Hexagonal Banding. Their crystals zone
/// from core to rim (morganite cores in aquamarine rims, the Erongo aquamarines), but no
/// six-sided color bands were found in their stones or crystals.
static BERYL: [TypicalPattern; 1] = [TypicalPattern::new(
    TemplateId::CoreAndRim,
    hexagonal_core_and_rim,
)];
static TOURMALINE: [TypicalPattern; 4] = [
    TypicalPattern::new(TemplateId::Watermelon, tourmaline_watermelon),
    TypicalPattern::new(TemplateId::ReverseWatermelon, tourmaline_reverse_watermelon),
    TypicalPattern::new(TemplateId::Bicolor, tourmaline_bicolor),
    TypicalPattern::new(TemplateId::Tricolor, tourmaline_tricolor),
];
static AMETHYST_PATTERNS: [TypicalPattern; 2] = [
    TypicalPattern::new(TemplateId::Sectors, amethyst_sectors),
    TypicalPattern::new(TemplateId::Split, amethyst_split),
];
static CITRINE_PATTERNS: [TypicalPattern; 2] = [
    TypicalPattern::new(TemplateId::Split, ametrine_split),
    TypicalPattern::new(TemplateId::Sectors, ametrine_sectors),
];
static FLUORITE: [TypicalPattern; 2] = [
    TypicalPattern::upright(TemplateId::Banded, fluorite_banded),
    TypicalPattern::upright(TemplateId::Tricolor, fluorite_tricolor),
];
static TOPAZ: [TypicalPattern; 1] = [TypicalPattern::new(TemplateId::Bicolor, topaz_bicolor)];

/// The typical patterns of the species named `material_name`, in menu order; empty for a species
/// that has none (and for every custom material).
///
/// | Species | Patterns |
/// |---|---|
/// | Sapphire | Hexagonal Banding, Parti Color, Color Band |
/// | Ruby | Hexagonal Banding |
/// | Emerald | Hexagonal Banding, Core and Rim |
/// | Other beryls (Aquamarine, Morganite, Heliodor, ...) | Core and Rim |
/// | Tourmaline | Watermelon, Reverse Watermelon, Bicolor, Tricolor |
/// | Amethyst | Sectors, Split |
/// | Citrine, Ametrine | Split, Sectors |
/// | Fluorite | Banded, Tricolor |
/// | Topaz | Bicolor |
#[must_use]
pub fn typical_patterns(material_name: &str) -> &'static [TypicalPattern] {
    match material_name.trim().to_ascii_lowercase().as_str() {
        "sapphire" | "padparadscha" => &CORUNDUM,
        "ruby" => &RUBY,
        "emerald" => &EMERALD,
        "aquamarine" | "morganite" | "heliodor" | "goshenite" | "beryl" => &BERYL,
        "tourmaline" => &TOURMALINE,
        "amethyst" => &AMETHYST_PATTERNS,
        "citrine" | "ametrine" => &CITRINE_PATTERNS,
        "fluorite" => &FLUORITE,
        "topaz" => &TOPAZ,
        _ => &[],
    }
}

/// The entry of `material_name`'s typical patterns that a stone's pattern came from.
///
/// It is the one whose template id is `template`. `None` for a pattern made by hand, from another
/// template, or for a species without typical patterns.
#[must_use]
pub fn typical_index(material_name: &str, template: Option<TemplateId>) -> Option<usize> {
    let template = template?;
    typical_patterns(material_name)
        .iter()
        .position(|entry| entry.template == template)
}

// ---- colors ---------------------------------------------------------------------------------

/// The hue of `lch` moved by `degrees`.
fn rotated(lch: [f64; 3], degrees: f64) -> [f64; 3] {
    [lch[0], lch[1], (lch[2] + degrees).rem_euclid(360.0)]
}

/// A paler tint of the same hue: lighter, with less chroma.
fn paler(own: [f64; 3]) -> [f64; 3] {
    [
        (97.0 - own[0]).max(0.0).mul_add(0.5, own[0]),
        own[1] * 0.45,
        own[2],
    ]
}

/// A stronger shade of the same hue: a little darker, with more chroma.
fn stronger(own: [f64; 3]) -> [f64; 3] {
    [
        own[0] * 0.85,
        (own[1] * 1.3).clamp(MIN_STRONG_CHROMA, MAX_CHROMA),
        own[2],
    ]
}

/// The same hue, almost colorless.
fn nearly_clear(own: [f64; 3]) -> [f64; 3] {
    [94.0, (own[1] * 0.1).min(4.0), own[2]]
}

/// Whether `own` is a visible color of a hue between `from` and `to` degrees.
fn has_hue(own: [f64; 3], from: f64, to: f64) -> bool {
    own[1] >= 10.0 && (from..=to).contains(&own[2])
}

/// The pink and the green of a tourmaline: the stone's own color replaces the one it already is,
/// and each of the others is the rubellite or verdelite variant when the stone has it.
fn tourmaline_pair(own: &OwnColor, variants: &[ColorVariant]) -> (PatternColor, PatternColor) {
    let pink = || shade_color(variants, Shade::Rubellite).unwrap_or_else(|| own.flat(PINK));
    let green = || shade_color(variants, Shade::Verdelite).unwrap_or_else(|| own.flat(GREEN));
    let lch = own.lch();
    if has_hue(lch, 90.0, 190.0) {
        (pink(), own.color())
    } else if has_hue(lch, 320.0, 360.0) || has_hue(lch, 0.0, 30.0) {
        (own.color(), green())
    } else {
        (pink(), green())
    }
}

/// The purple and the yellow of an ametrine: the amethyst and citrine variants when the stone has
/// them, else the Ametrine preset's `[L*, C*, h]` (in every direction of the stone).
fn ametrine_pair(own: &OwnColor, variants: &[ColorVariant]) -> (PatternColor, PatternColor) {
    (
        shade_color(variants, Shade::Amethyst).unwrap_or_else(|| own.flat(AMETHYST)),
        shade_color(variants, Shade::Citrine).unwrap_or_else(|| own.flat(CITRINE)),
    )
}

// ---- geometry ---------------------------------------------------------------------------------

/// A prism around the pattern axis, `sides` sided, between two apothems.
const fn prism(sides: u32, r_in: f64, r_out: f64) -> ZoneShape {
    ZoneShape::CoaxialPrism {
        axis_point: DVec3::ZERO,
        axis_dir: DVec3::Y,
        n_sides: sides,
        r_in,
        r_out,
        phase: 0.0,
    }
}

/// A pattern from its parts, stamped with the template it came from.
fn assemble(
    id: TemplateId,
    axis: PatternAxis,
    parts: (PatternZone, Vec<PatternZone>),
) -> ColorPattern {
    ColorPattern {
        version: PATTERN_BASE_FORMAT_VERSION,
        template: Some(id),
        base: parts.0,
        zones: parts.1,
        softness: 0.0,
        axis,
        relative_to_stone: true,
    }
    .with_required_version()
}

/// `pattern` with the colors of its zones replaced, base first (picked `[L*, C*, h]` seeds or
/// variants). Geometry, names and the rest stay. The version follows the colors.
fn recolored_with(
    mut pattern: ColorPattern,
    colors: impl IntoIterator<Item = PatternColor>,
) -> ColorPattern {
    let parts = std::iter::once(&mut pattern.base).chain(pattern.zones.iter_mut());
    for (part, color) in parts.zip(colors) {
        part.color = color;
    }
    pattern.with_required_version()
}

/// The base and zones of a hexagonal banding: a pale body with three 6-sided growth zones.
pub(super) fn hexagonal_banding_parts(own: &OwnColor) -> (PatternZone, Vec<PatternZone>) {
    (
        PatternZone::base("Pale Body", own.shade(paler)),
        GROWTH_ZONES
            .iter()
            .enumerate()
            .map(|(index, (r_in, r_out))| {
                zone_with(
                    &format!("Growth Zone {}", index + 1),
                    prism(6, *r_in, *r_out),
                    own.shade(stronger),
                )
            })
            .collect(),
    )
}

/// The base and zones of a parti-color stone: a 6-sided core in the stone's own color inside a
/// rim of a contrasting hue.
pub(super) fn parti_color_parts(own: &OwnColor) -> (PatternZone, Vec<PatternZone>) {
    let rim = if has_hue(own.lch(), 60.0, 120.0) {
        PARTI_BLUE
    } else {
        PARTI_YELLOW
    };
    (
        PatternZone::base("Rim", own.flat(rim)),
        vec![zone_with(
            "Core",
            prism(6, 0.0, HEX_CORE_APOTHEM),
            own.color(),
        )],
    )
}

/// The base and zones of a color band: the body in the stone's own color and one stronger band
/// through the axis, off center.
pub(super) fn color_band_parts(own: &OwnColor) -> (PatternZone, Vec<PatternZone>) {
    (
        PatternZone::base("Body", own.color()),
        vec![zone_with(
            "Color Band",
            ZoneShape::Slab {
                normal: DVec3::X,
                offset_min: COLOR_BAND.0,
                offset_max: COLOR_BAND.1,
            },
            own.shade(stronger),
        )],
    )
}

/// Wedges of 60 degrees at every second sixth of a turn around the axis: the sector zoning of
/// a rhombohedral crystal.
fn three_sectors(
    background: PatternColor,
    sector: &PatternColor,
) -> (PatternZone, Vec<PatternZone>) {
    (
        PatternZone::base("Background", background),
        (0..3_u8)
            .map(|k| {
                let from = TAU / 3.0 * f64::from(k);
                zone_with(
                    &format!("Sector {}", k + 1),
                    wedge(from, from + FRAC_PI_3),
                    sector.clone(),
                )
            })
            .collect(),
    )
}

// ---- the entries --------------------------------------------------------------------------------

fn hexagonal_banding(
    own: &OwnColor,
    _variants: &[ColorVariant],
    axis: PatternAxis,
) -> ColorPattern {
    assemble(
        TemplateId::HexagonalBanding,
        axis,
        hexagonal_banding_parts(own),
    )
}

/// A 6-sided core in the stone's own color inside a rim of a contrasting hue; the rim, and the core
/// of a stone that is blue or yellow itself, are the Yellow and Blue variants when the stone has
/// them. A stone whose own color is a variant keeps it for the core.
fn parti_color(own: &OwnColor, variants: &[ColorVariant], axis: PatternAxis) -> ColorPattern {
    let (mut rim, mut zones) = parti_color_parts(own);
    let lch = own.lch();
    let (rim_shade, core_shade) = if has_hue(lch, 60.0, 120.0) {
        (Shade::Blue, Some(Shade::Yellow))
    } else {
        (
            Shade::PartiYellow,
            has_hue(lch, 230.0, 300.0).then_some(Shade::Blue),
        )
    };
    if let Some(color) = shade_color(variants, rim_shade) {
        rim.color = color;
    }
    if own.variant().is_none()
        && let Some(color) = core_shade.and_then(|shade| shade_color(variants, shade))
    {
        zones[0].color = color;
    }
    assemble(TemplateId::PartiColor, axis, (rim, zones))
}

fn color_band(own: &OwnColor, _variants: &[ColorVariant], axis: PatternAxis) -> ColorPattern {
    assemble(TemplateId::ColorBand, axis, color_band_parts(own))
}

/// A beryl's 6-sided core, stronger in color than the pale rim around it.
fn hexagonal_core_and_rim(
    own: &OwnColor,
    _variants: &[ColorVariant],
    axis: PatternAxis,
) -> ColorPattern {
    assemble(
        TemplateId::CoreAndRim,
        axis,
        (
            PatternZone::base("Rim", own.shade(paler)),
            vec![zone_with(
                "Core",
                prism(6, 0.0, HEX_CORE_APOTHEM),
                own.shade(stronger),
            )],
        ),
    )
}

fn tourmaline_watermelon(
    own: &OwnColor,
    variants: &[ColorVariant],
    axis: PatternAxis,
) -> ColorPattern {
    let (pink, green) = tourmaline_pair(own, variants);
    recolored_with(template(TemplateId::Watermelon, axis), [green, pink])
}

fn tourmaline_reverse_watermelon(
    own: &OwnColor,
    variants: &[ColorVariant],
    axis: PatternAxis,
) -> ColorPattern {
    // The pair is (pink, green): the pink rim and the green core as they are.
    let colors = <[PatternColor; 2]>::from(tourmaline_pair(own, variants));
    recolored_with(template(TemplateId::ReverseWatermelon, axis), colors)
}

fn tourmaline_bicolor(
    own: &OwnColor,
    variants: &[ColorVariant],
    axis: PatternAxis,
) -> ColorPattern {
    let (pink, green) = tourmaline_pair(own, variants);
    recolored_with(template(TemplateId::Bicolor, axis), [green, pink])
}

fn tourmaline_tricolor(
    own: &OwnColor,
    variants: &[ColorVariant],
    axis: PatternAxis,
) -> ColorPattern {
    let (pink, green) = tourmaline_pair(own, variants);
    recolored_with(
        template(TemplateId::Tricolor, axis),
        [own.flat(PALE), pink, green],
    )
}

fn amethyst_sectors(own: &OwnColor, _variants: &[ColorVariant], axis: PatternAxis) -> ColorPattern {
    assemble(
        TemplateId::Sectors,
        axis,
        three_sectors(own.shade(paler), &own.color()),
    )
}

fn amethyst_split(own: &OwnColor, _variants: &[ColorVariant], axis: PatternAxis) -> ColorPattern {
    recolored_with(
        template(TemplateId::Split, axis),
        [own.shade(paler), own.color()],
    )
}

/// Purple and yellow halves, as the Ametrine preset has them (the stone's own color only says how
/// many directions a color has: citrine and ametrine are defined by the two colors); the amethyst
/// and citrine variants when the stone has them.
fn ametrine_split(own: &OwnColor, variants: &[ColorVariant], axis: PatternAxis) -> ColorPattern {
    // The pair is (amethyst, citrine): the purple half and the yellow half as they are.
    let colors = <[PatternColor; 2]>::from(ametrine_pair(own, variants));
    recolored_with(template(TemplateId::Split, axis), colors)
}

/// Amethyst sectors in a citrine body: the sector zoning of ametrine.
fn ametrine_sectors(own: &OwnColor, variants: &[ColorVariant], axis: PatternAxis) -> ColorPattern {
    let (amethyst, citrine) = ametrine_pair(own, variants);
    assemble(TemplateId::Sectors, axis, three_sectors(citrine, &amethyst))
}

fn fluorite_banded(own: &OwnColor, _variants: &[ColorVariant], axis: PatternAxis) -> ColorPattern {
    let band = own.shade(stronger);
    recolored_with(
        template(TemplateId::Banded, axis),
        [
            own.shade(paler),
            band.clone(),
            band.clone(),
            band.clone(),
            band,
        ],
    )
}

fn fluorite_tricolor(
    own: &OwnColor,
    _variants: &[ColorVariant],
    axis: PatternAxis,
) -> ColorPattern {
    recolored_with(
        template(TemplateId::Tricolor, axis),
        [
            own.shade(paler),
            own.shade(stronger),
            own.shade(|lch| rotated(stronger(lch), 150.0)),
        ],
    )
}

fn topaz_bicolor(own: &OwnColor, _variants: &[ColorVariant], axis: PatternAxis) -> ColorPattern {
    recolored_with(
        template(TemplateId::Bicolor, axis),
        [own.shade(nearly_clear), own.color()],
    )
}
