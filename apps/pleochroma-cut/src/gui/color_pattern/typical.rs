//! Typical zoning applied without the Color Pattern window: the colors of a typical pattern are
//! `[L*, C*, h]` only, so they are solved into band rows first.
//!
//! The solve is the body-color editor's own (`pleochroma_editor::lch_color::solve_lch`). It takes
//! a moment, so a caller runs [`solve_pattern_colors`] on a background thread.

use pleochroma_cut_core::material::pattern::ColorPattern;
use pleochroma_editor::lch_color::solve_lch;
use std::sync::atomic::AtomicBool;

/// `pattern` with the band rows of every unsolved color filled in. `solve` is asked once for each
/// distinct `[L*, C*, h]` (a typical pattern repeats its stronger shade in every band) and returns
/// the rows, or `None` when it could not (the whole call then gives `None`).
pub(super) fn solve_colors_with(
    mut pattern: ColorPattern,
    mut solve: impl FnMut([f64; 3]) -> Option<Vec<[f32; 3]>>,
) -> Option<ColorPattern> {
    let mut known: Vec<([f64; 3], Vec<[f32; 3]>)> = Vec::new();
    for at in pattern.unsolved_colors() {
        let lch = pattern.color(at)?.lch;
        let bands = if let Some((_, bands)) = known.iter().find(|(seen, _)| *seen == lch) {
            bands.clone()
        } else {
            let bands = solve(lch)?;
            known.push((lch, bands.clone()));
            bands
        };
        pattern.set_color_bands(at, bands);
    }
    Some(pattern)
}

/// [`solve_colors_with`] with the body-color solver at the light path `path_mm`. `None` when the
/// solve was cancelled (`cancel` raised) or could not finish.
pub fn solve_pattern_colors(
    pattern: ColorPattern,
    path_mm: f64,
    cancel: &AtomicBool,
) -> Option<ColorPattern> {
    solve_colors_with(pattern, |lch| {
        solve_lch(lch, path_mm, cancel).map(|solve| solve.bands)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::material::pattern::{OwnColor, PatternColor, typical_patterns};

    /// A deterministic stand-in for the solver's rows.
    fn rows() -> Vec<[f32; 3]> {
        vec![[500.0, 40.0, 0.25]]
    }

    #[test]
    fn every_color_of_a_pleochroic_stone_is_solved_per_direction_and_a_repeat_once() {
        let entry = &typical_patterns("Sapphire")[0];
        // A sapphire's Along and Across colors: every zone has two colors, never one.
        let own = OwnColor::per_axis(&[[33.0, 34.0, 289.0], [54.0, 21.0, 276.0]], None);
        let pattern = entry.for_stone_on(&own, &[]);
        assert!(!pattern.unsolved_colors().is_empty());
        let mut asked = Vec::new();
        let solved = solve_colors_with(pattern, |lch| {
            asked.push(lch);
            Some(rows())
        })
        .expect("solves");
        assert!(solved.unsolved_colors().is_empty());
        // The pale body and the stronger shade of the three growth zones, in two directions: four
        // distinct colors.
        assert_eq!(asked.len(), 4, "{asked:?}");
        solved.to_zoned().expect("valid").expect("zones");
        for zone in std::iter::once(&solved.base).chain(&solved.zones) {
            let PatternColor::Dichroic { along, across } = &zone.color else {
                panic!(
                    "two colors per zone on a pleochroic stone: {:?}",
                    zone.color
                );
            };
            assert_eq!((&along.bands, &across.bands), (&rows(), &rows()));
        }
    }

    #[test]
    fn every_color_of_a_stone_without_pleochroism_is_solved_and_a_repeat_once() {
        // Fluorite is cubic: one color per zone, the stronger shade repeated in every band.
        let entry = &typical_patterns("Fluorite")[0];
        let pattern = entry.for_stone([40.0, 45.0, 265.0], &[]);
        assert!(!pattern.unsolved_colors().is_empty());
        let mut asked = Vec::new();
        let solved = solve_colors_with(pattern, |lch| {
            asked.push(lch);
            Some(rows())
        })
        .expect("solves");
        assert!(solved.unsolved_colors().is_empty());
        assert_eq!(asked.len(), 2, "{asked:?}");
        solved.to_zoned().expect("valid").expect("zones");
        for zone in std::iter::once(&solved.base).chain(&solved.zones) {
            let PatternColor::Single(color) = &zone.color else {
                panic!("one color per zone");
            };
            assert_eq!(color.bands, rows());
        }
    }

    #[test]
    fn a_variant_zone_needs_no_solve_and_keeps_its_rows() {
        use pleochroma::optics::materials::GemMaterial;
        let variants = crate::gui::color_pattern::zone_variants(&GemMaterial::sapphire(), None);
        let entry = &typical_patterns("Sapphire")[1];
        let pattern = entry.for_stone([40.0, 45.0, 265.0], &variants);
        let rim = pattern.base.color.clone();
        assert!(matches!(rim, PatternColor::Variant { .. }));
        // The blue core of a blue stone is the Sapphire's own color, its Material Default.
        let core = pattern.zones[0].color.clone();
        assert_eq!(core.variant_key(), Some("sapphire.default"));
        let mut asked = Vec::new();
        let solved = solve_colors_with(pattern, |lch| {
            asked.push(lch);
            Some(rows())
        })
        .expect("solves");
        // Both zones are variants, so nothing is asked for and their rows stay as they are.
        assert!(asked.is_empty(), "{asked:?}");
        assert_eq!(solved.base.color, rim);
        assert_eq!(solved.zones[0].color, core);
        assert!(solved.unsolved_colors().is_empty());
        solved.to_zoned().expect("valid").expect("zones");
    }

    #[test]
    fn a_failed_solve_gives_no_pattern() {
        let entry = &typical_patterns("Tourmaline")[0];
        let pattern = entry.for_stone([40.0, 45.0, 265.0], &[]);
        assert_eq!(solve_colors_with(pattern, |_| None), None);
    }

    #[test]
    fn a_solved_pattern_is_left_as_it_is() {
        let entry = &typical_patterns("Topaz")[0];
        let once = solve_colors_with(entry.for_stone([60.0, 40.0, 60.0], &[]), |_| Some(rows()))
            .expect("solves");
        let twice = solve_colors_with(once.clone(), |_| panic!("nothing left to solve"));
        assert_eq!(twice, Some(once));
    }
}
