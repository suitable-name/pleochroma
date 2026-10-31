//! [`StoneRenderEffects`]: the per-design stone effects of the live and exported render.
//!
//! Inclusion haze (`sigma_s`), crystal-axis orientation, frosted girdle and facet-edge
//! rounding used to be app-level settings, so opening another design rendered it with the
//! previous design's values. They belong to the stone, so they live on the [`Design`]
//! (`Design::render_effects`), travel in the native file's optional `[render_effects]`
//! table, and change only through the undoable `Edit::SetRenderEffects`.
//!
//! # Compatibility
//!
//! A design without the table (an old file, an imported `.asc`) has
//! [`StoneRenderEffects::default`]: every effect off, which is the plain render. `.asc` has
//! no field for these, so a re-imported `.asc` gets the defaults.
//!
//! [`Design`]: super::Design

use super::effects_spec::{EdgeAbrasionEffects, InclusionEffects, StructureEffects};

/// Largest inclusion `sigma_s` the file may hold (the app slider reaches 2.0).
pub const MAX_INCLUSION_SIGMA_S: f64 = 3.0;

/// Largest facet-edge rounding radius, in model units (the furnace-verified ceiling).
pub const MAX_EDGE_ROUNDING_RADIUS: f64 = 0.03;

/// The per-design stone render effects. `Default` is "everything off".
///
/// The last three fields (oriented inclusions, play-of-color and abraded edges) are the
/// render-physics additions: each is off by default and written to the file only when set.
/// The renderer's own specs come from [`Self::inclusion_spec`], [`Self::structure_spec`] and
/// [`Self::edge_abrasion`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StoneRenderEffects {
    /// Inclusion / subsurface scattering `sigma_s`, per model unit. `0.0` is off.
    pub inclusion_sigma_s: f64,
    /// Whether the crystal-axis orientation override is on ("as cut" when off).
    pub c_axis_override_enabled: bool,
    /// Tilt of the override axis from the table normal, `0.0..=90.0` degrees.
    pub c_axis_tilt_deg: f64,
    /// Azimuth of the override axis around the table normal, `0.0..=360.0` degrees.
    pub c_axis_azimuth_deg: f64,
    /// Turn of the Second Axis (the alpha direction of a biaxial stone) about the Crystal
    /// Axis, `0.0..=180.0` degrees. `None` is "not set": the material's own completion.
    /// Written to the file only when `Some`.
    pub c_axis_second_azimuth_deg: Option<f64>,
    /// Bruted (frosted) girdle finish instead of polished.
    pub girdle_frosted: bool,
    /// Facet-edge rounding radius in model units, `0.0..=0.03`. `0.0` is off.
    pub edge_rounding_radius: f64,
    /// The oriented-inclusion choice (`[render_effects.inclusions]`); `None` is off.
    pub inclusions: Option<InclusionEffects>,
    /// The play-of-color choice (`[render_effects.structure]`); `None` is off.
    pub structure: Option<StructureEffects>,
    /// The abraded edges (`edge_abrasion_*`); the default is off.
    pub edge_abrasion: EdgeAbrasionEffects,
}

impl StoneRenderEffects {
    /// The inclusion spec the renderer takes, `None` when no preset is chosen.
    #[must_use]
    pub fn inclusion_spec(&self) -> Option<pleochroma::optics::inclusions::spec::InclusionSpec> {
        self.inclusions.as_ref().map(InclusionEffects::to_spec)
    }

    /// The play-of-color spec the renderer takes, `None` when none is chosen.
    #[must_use]
    pub fn structure_spec(&self) -> Option<pleochroma::optics::inclusions::bragg::StructureSpec> {
        self.structure.as_ref().map(StructureEffects::to_spec)
    }

    /// The abraded-edge setting the renderer takes (inactive for the default).
    #[must_use]
    pub const fn edge_abrasion(&self) -> pleochroma::optics::raytracer::surface::EdgeAbrasion {
        self.edge_abrasion.to_abrasion()
    }

    /// These effects with the render-physics parts (inclusions, play-of-color, abraded edges)
    /// taken from `other`.
    ///
    /// [`Self::from_render_values`] builds the seven app-level values only, so a caller that
    /// rebuilds the effects from its controls uses this to keep what the controls do not carry.
    #[must_use]
    pub const fn with_physics_of(mut self, other: &Self) -> Self {
        self.inclusions = other.inclusions;
        self.structure = other.structure;
        self.edge_abrasion = other.edge_abrasion;
        self
    }

    /// Whether this is exactly the default (nothing to write to a file).
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// These effects with every value inside its legal range; anything not a number
    /// becomes the default for that field.
    #[must_use]
    pub fn sanitised(self) -> Self {
        let finite = |v: f64| if v.is_finite() { v } else { 0.0 };
        Self {
            inclusion_sigma_s: finite(self.inclusion_sigma_s).clamp(0.0, MAX_INCLUSION_SIGMA_S),
            c_axis_override_enabled: self.c_axis_override_enabled,
            c_axis_tilt_deg: finite(self.c_axis_tilt_deg).clamp(0.0, 90.0),
            c_axis_azimuth_deg: finite(self.c_axis_azimuth_deg).clamp(0.0, 360.0),
            c_axis_second_azimuth_deg: self
                .c_axis_second_azimuth_deg
                .filter(|v| v.is_finite())
                .map(|v| v.clamp(0.0, 180.0)),
            girdle_frosted: self.girdle_frosted,
            edge_rounding_radius: finite(self.edge_rounding_radius)
                .clamp(0.0, MAX_EDGE_ROUNDING_RADIUS),
            inclusions: self.inclusions.map(InclusionEffects::sanitised),
            structure: self.structure.map(StructureEffects::sanitised),
            edge_abrasion: self.edge_abrasion.sanitised(),
        }
    }

    /// Builds effects from the `f32` values the render context holds, rounded to six
    /// decimals so a value read back from a TOML file compares equal to the one written.
    #[must_use]
    pub fn from_render_values(
        inclusion_sigma_s: f32,
        c_axis_override_enabled: bool,
        c_axis_tilt_deg: f32,
        c_axis_azimuth_deg: f32,
        c_axis_second_azimuth_deg: Option<f32>,
        girdle_frosted: bool,
        edge_rounding_radius: f32,
    ) -> Self {
        let round6 = |v: f32| (f64::from(v) * 1e6).round() / 1e6;
        Self {
            inclusion_sigma_s: round6(inclusion_sigma_s),
            c_axis_override_enabled,
            c_axis_tilt_deg: round6(c_axis_tilt_deg),
            c_axis_azimuth_deg: round6(c_axis_azimuth_deg),
            c_axis_second_azimuth_deg: c_axis_second_azimuth_deg.map(round6),
            girdle_frosted,
            edge_rounding_radius: round6(edge_rounding_radius),
            ..Self::default()
        }
        .sanitised()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_off_and_sanitising_limits_ranges() {
        assert!(StoneRenderEffects::default().is_default());
        let wild = StoneRenderEffects {
            inclusion_sigma_s: 99.0,
            c_axis_tilt_deg: f64::NAN,
            c_axis_azimuth_deg: 720.0,
            edge_rounding_radius: -1.0,
            ..StoneRenderEffects::default()
        }
        .sanitised();
        assert_eq!(wild.inclusion_sigma_s.to_bits(), 3.0f64.to_bits());
        assert_eq!(wild.c_axis_tilt_deg.to_bits(), 0.0f64.to_bits());
        assert_eq!(wild.c_axis_azimuth_deg.to_bits(), 360.0f64.to_bits());
        assert_eq!(wild.edge_rounding_radius.to_bits(), 0.0f64.to_bits());
    }

    #[test]
    fn the_second_azimuth_is_clamped_to_a_half_turn_and_dropped_when_not_a_number() {
        let with = |v: Option<f64>| {
            StoneRenderEffects {
                c_axis_second_azimuth_deg: v,
                ..StoneRenderEffects::default()
            }
            .sanitised()
            .c_axis_second_azimuth_deg
        };
        assert_eq!(with(None), None);
        assert_eq!(with(Some(f64::NAN)), None);
        assert_eq!(with(Some(f64::INFINITY)), None);
        assert_eq!(with(Some(-5.0)), Some(0.0));
        assert_eq!(with(Some(200.0)), Some(180.0));
        assert_eq!(with(Some(42.5)), Some(42.5));
        assert!(
            !StoneRenderEffects {
                c_axis_second_azimuth_deg: Some(0.0),
                ..StoneRenderEffects::default()
            }
            .is_default(),
            "a set second azimuth is not the default, even at zero"
        );
    }
}
