//! The inclusion, play-of-color and abraded-edge choices of a design: the three optional parts of
//! [`super::StoneRenderEffects`] that the render-physics features add.
//!
//! Each is `Copy` (like the rest of the effects) and converts to the renderer's own spec type
//! with a `to_spec` method: [`InclusionEffects::to_spec`] gives the [`InclusionSpec`],
//! [`StructureEffects::to_spec`] the [`StructureSpec`] and [`EdgeAbrasionEffects::to_abrasion`]
//! the [`EdgeAbrasion`]. A material is given the first two in this order:
//! `with_inclusion_spec`, then `with_structure_spec`.
//!
//! An inclusion preset is a name from the catalogue (`'static`), so the type stays `Copy`; a
//! name the catalogue does not know (a file from a newer build) reads as "no inclusions".

use pleochroma::optics::{
    inclusions::{
        bragg::{DEFAULT_SEED, StructurePattern, StructureSpec},
        catalogue,
        spec::{DEFAULT_DENSITY, InclusionSpec, Sheen, SilkColor},
    },
    raytracer::surface::EdgeAbrasion,
};

/// Largest band width of the abraded edge, in model units.
pub const MAX_EDGE_ABRASION_WIDTH: f64 = 0.1;

/// Largest polish roughness of the abraded edge's rim, in nanometres (the packed word's range).
pub const MAX_EDGE_ABRASION_HALO_RQ_NM: f64 = 100.0;

/// The shortest and longest wavelength a play-of-color range may name, in nanometres.
const COLOR_RANGE_NM: (f64, f64) = (300.0, 1200.0);

/// The catalogue's own spelling of the preset called `name` (ignoring case), or `None` when no
/// material has such a preset.
#[must_use]
pub fn canonical_preset(name: &str) -> Option<&'static str> {
    catalogue::PRESET_MATERIALS
        .iter()
        .find_map(|material| catalogue::find_preset(material, name))
        .map(|preset| preset.name)
}

/// The words a [`Sheen`] has in the design file.
#[must_use]
pub const fn sheen_word(sheen: Sheen) -> &'static str {
    match sheen {
        Sheen::Soft => "Soft",
        Sheen::Sharp => "Sharp",
    }
}

/// The [`Sheen`] a design file word names (ignoring case).
#[must_use]
pub fn sheen_from_word(word: &str) -> Option<Sheen> {
    [Sheen::Soft, Sheen::Sharp]
        .into_iter()
        .find(|sheen| sheen_word(*sheen).eq_ignore_ascii_case(word.trim()))
}

/// The words a [`SilkColor`] has in the design file.
#[must_use]
pub const fn silk_color_word(color: SilkColor) -> &'static str {
    match color {
        SilkColor::Grey => "Grey",
        SilkColor::SlightlyBlue => "Slightly Blue",
        SilkColor::Blue => "Blue",
    }
}

/// The [`SilkColor`] a design file word names (ignoring case).
#[must_use]
pub fn silk_color_from_word(word: &str) -> Option<SilkColor> {
    [SilkColor::Grey, SilkColor::SlightlyBlue, SilkColor::Blue]
        .into_iter()
        .find(|color| silk_color_word(*color).eq_ignore_ascii_case(word.trim()))
}

/// The [`StructurePattern`] a design file word names (ignoring case).
#[must_use]
pub fn pattern_from_word(word: &str) -> Option<StructurePattern> {
    [
        StructurePattern::Pinfire,
        StructurePattern::Harlequin,
        StructurePattern::BroadFlash,
    ]
    .into_iter()
    .find(|pattern| pattern.label().eq_ignore_ascii_case(word.trim()))
}

const fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() { value } else { fallback }
}

/// A design's inclusion choice: the catalogue preset, its density and the look controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InclusionEffects {
    /// The catalogue preset, in the catalogue's own spelling (see [`canonical_preset`]).
    pub preset: &'static str,
    /// `0.0..=1.0`; `0.35` is the catalogue strength.
    pub density: f64,
    /// The Silk Turn in degrees about the c axis, `0.0..=180.0`; `None` follows the stored
    /// Second Axis.
    pub turn_deg: Option<f64>,
    /// `None` keeps the preset's own cone width.
    pub sheen: Option<Sheen>,
    /// `None` keeps the preset's own color.
    pub color: Option<SilkColor>,
}

impl InclusionEffects {
    /// The choice of the preset called `preset` at the catalogue density, `None` when the
    /// catalogue has no such preset.
    #[must_use]
    pub fn of_preset(preset: &str) -> Option<Self> {
        Some(Self {
            preset: canonical_preset(preset)?,
            density: f64::from(DEFAULT_DENSITY),
            turn_deg: None,
            sheen: None,
            color: None,
        })
    }

    /// This choice with every value inside its legal range.
    #[must_use]
    pub fn sanitised(self) -> Self {
        Self {
            density: finite_or(self.density, 0.0).clamp(0.0, 1.0),
            turn_deg: self
                .turn_deg
                .filter(|turn| turn.is_finite())
                .map(|turn| turn.clamp(0.0, 180.0)),
            ..self
        }
    }

    /// The renderer's spec for this choice.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the spec is f32; the design keeps f64 so a TOML value round-trips exactly"
    )]
    pub fn to_spec(&self) -> InclusionSpec {
        InclusionSpec {
            preset: self.preset.to_owned(),
            density: self.density as f32,
            turn_deg: self.turn_deg.map(|turn| turn as f32),
            sheen: self.sheen,
            color: self.color,
        }
    }
}

/// A design's play-of-color choice (`[render_effects.structure]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StructureEffects {
    /// The layout.
    pub pattern: StructurePattern,
    /// The seed of the domain map, stored with the design; a Reseed draws a new one.
    pub seed: u32,
    /// The domain cell size in model units, or `None` for the pattern's own.
    pub size: Option<f64>,
    /// The shortest normal-incidence Bragg wavelength, nanometres.
    pub color_low_nm: f64,
    /// The longest normal-incidence Bragg wavelength, nanometres.
    pub color_high_nm: f64,
    /// The relative bandwidth ("Color Purity"), `0.005..=1.0`.
    pub purity: f64,
    /// The fraction of domains that reflect ("Coverage"), `0.0..=1.0`.
    pub coverage: f64,
}

impl StructureEffects {
    /// The choice that `spec` stands for (the material's default from
    /// [`StructureSpec::default_for`], for instance).
    #[must_use]
    pub fn from_spec(spec: &StructureSpec) -> Self {
        Self {
            pattern: spec.pattern,
            seed: spec.seed,
            size: spec.size.map(f64::from),
            color_low_nm: f64::from(spec.color_low_nm),
            color_high_nm: f64::from(spec.color_high_nm),
            purity: f64::from(spec.purity),
            coverage: f64::from(spec.coverage),
        }
    }

    /// This choice with every value inside its legal range: finite numbers, the color range
    /// ordered and inside 300 to 1200 nm, a positive cell size or none.
    #[must_use]
    pub fn sanitised(self) -> Self {
        let clamp_nm = |nm: f64, fallback: f64| {
            finite_or(nm, fallback).clamp(COLOR_RANGE_NM.0, COLOR_RANGE_NM.1)
        };
        let low = clamp_nm(self.color_low_nm, 440.0);
        let high = clamp_nm(self.color_high_nm, 680.0);
        Self {
            size: self.size.filter(|size| size.is_finite() && *size > 0.0),
            color_low_nm: low.min(high),
            color_high_nm: low.max(high),
            purity: finite_or(self.purity, 0.05).clamp(0.005, 1.0),
            coverage: finite_or(self.coverage, 0.8).clamp(0.0, 1.0),
            ..self
        }
    }

    /// The renderer's spec for this choice.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the spec is f32; the design keeps f64 so a TOML value round-trips exactly"
    )]
    pub fn to_spec(&self) -> StructureSpec {
        StructureSpec {
            pattern: self.pattern,
            seed: self.seed,
            size: self.size.map(|size| size as f32),
            color_low_nm: self.color_low_nm as f32,
            color_high_nm: self.color_high_nm as f32,
            purity: self.purity as f32,
            coverage: self.coverage as f32,
        }
    }

    /// The seed a Reseed button draws after `seed`: a fixed odd-multiplier step, so the
    /// sequence is deterministic and never returns to the same value quickly.
    #[must_use]
    pub const fn reseeded(seed: u32) -> u32 {
        let next = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        if next == seed { DEFAULT_SEED } else { next }
    }
}

/// A design's abraded-edge setting (`edge_abrasion_*` keys of `[render_effects]`).
///
/// `Default` is "off": a zero width.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EdgeAbrasionEffects {
    /// The band width in model units, `0.0..=0.1`; `0.0` switches the band off.
    pub width: f64,
    /// The GGX roughness of the band, `0.0..=1.0`.
    pub alpha: f64,
    /// The polish roughness (nm Rq) at the band's rim, `0.0..=100.0`; `0.0` is off.
    pub halo_rq_nm: f64,
}

impl EdgeAbrasionEffects {
    /// This setting with every value inside its legal range.
    #[must_use]
    pub const fn sanitised(self) -> Self {
        Self {
            width: finite_or(self.width, 0.0).clamp(0.0, MAX_EDGE_ABRASION_WIDTH),
            alpha: finite_or(self.alpha, 0.0).clamp(0.0, 1.0),
            halo_rq_nm: finite_or(self.halo_rq_nm, 0.0).clamp(0.0, MAX_EDGE_ABRASION_HALO_RQ_NM),
        }
    }

    /// The renderer's setting.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the renderer's setting is f32; the design keeps f64 so a TOML value round-trips exactly"
    )]
    pub const fn to_abrasion(&self) -> EdgeAbrasion {
        EdgeAbrasion {
            width: self.width as f32,
            alpha: self.alpha as f32,
            halo_rq_nm: self.halo_rq_nm as f32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_names_resolve_in_the_catalogues_spelling() {
        assert_eq!(canonical_preset("fine silk"), Some("Fine Silk"));
        assert_eq!(canonical_preset("Cat's-Eye Silk"), Some("Cat's-Eye Silk"));
        assert_eq!(canonical_preset("Rutilated"), Some("Rutilated"));
        assert_eq!(canonical_preset("Adularescence"), Some("Adularescence"));
        assert_eq!(canonical_preset("Rain"), Some("Rain"));
        assert_eq!(canonical_preset("Growth Tubes"), Some("Growth Tubes"));
        assert_eq!(canonical_preset("No Such Preset"), None);
    }

    #[test]
    fn the_words_of_the_look_controls_round_trip() {
        for sheen in [Sheen::Soft, Sheen::Sharp] {
            assert_eq!(sheen_from_word(sheen_word(sheen)), Some(sheen));
        }
        for color in [SilkColor::Grey, SilkColor::SlightlyBlue, SilkColor::Blue] {
            assert_eq!(silk_color_from_word(silk_color_word(color)), Some(color));
        }
        for pattern in [
            StructurePattern::Pinfire,
            StructurePattern::Harlequin,
            StructurePattern::BroadFlash,
        ] {
            assert_eq!(pattern_from_word(pattern.label()), Some(pattern));
        }
        assert_eq!(sheen_from_word("Glossy"), None);
    }

    #[test]
    fn sanitising_keeps_values_in_range() {
        let wild = InclusionEffects {
            density: f64::NAN,
            turn_deg: Some(400.0),
            ..InclusionEffects::of_preset("Fine Silk").expect("preset")
        }
        .sanitised();
        assert_eq!(wild.density.to_bits(), 0.0f64.to_bits());
        assert_eq!(wild.turn_deg, Some(180.0));

        let structure = StructureEffects {
            color_low_nm: 900.0,
            color_high_nm: 100.0,
            purity: 0.0,
            coverage: 3.0,
            size: Some(-1.0),
            ..StructureEffects::from_spec(&StructureSpec::default_for("Labradorite").expect("spec"))
        }
        .sanitised();
        assert!(structure.color_low_nm <= structure.color_high_nm);
        assert_eq!(structure.color_low_nm.to_bits(), 300.0f64.to_bits());
        assert_eq!(structure.color_high_nm.to_bits(), 900.0f64.to_bits());
        assert_eq!(structure.purity.to_bits(), 0.005f64.to_bits());
        assert_eq!(structure.coverage.to_bits(), 1.0f64.to_bits());
        assert_eq!(structure.size, None);

        let edge = EdgeAbrasionEffects {
            width: 5.0,
            alpha: -1.0,
            halo_rq_nm: f64::INFINITY,
        }
        .sanitised();
        assert_eq!(edge.width.to_bits(), MAX_EDGE_ABRASION_WIDTH.to_bits());
        assert_eq!(edge.alpha.to_bits(), 0.0f64.to_bits());
        assert_eq!(edge.halo_rq_nm.to_bits(), 0.0f64.to_bits());
    }

    #[test]
    fn the_specs_carry_the_design_values() {
        let inclusions = InclusionEffects {
            density: 0.5,
            turn_deg: Some(30.0),
            sheen: Some(Sheen::Sharp),
            color: Some(SilkColor::Blue),
            ..InclusionEffects::of_preset("Rutile Silk").expect("preset")
        };
        let spec = inclusions.to_spec();
        assert_eq!(spec.preset, "Rutile Silk");
        assert_eq!(spec.density.to_bits(), 0.5f32.to_bits());
        assert_eq!(spec.turn_deg, Some(30.0));
        assert_eq!(spec.sheen, Some(Sheen::Sharp));
        assert_eq!(spec.color, Some(SilkColor::Blue));

        let default = StructureSpec::default_for("Precious Opal (White)").expect("spec");
        assert_eq!(StructureEffects::from_spec(&default).to_spec(), default);
        assert_eq!(default.seed, DEFAULT_SEED);

        assert!(!EdgeAbrasionEffects::default().to_abrasion().active());
    }

    #[test]
    fn a_reseed_changes_the_seed_deterministically() {
        let next = StructureEffects::reseeded(DEFAULT_SEED);
        assert_ne!(next, DEFAULT_SEED);
        assert_eq!(StructureEffects::reseeded(DEFAULT_SEED), next);
    }
}
