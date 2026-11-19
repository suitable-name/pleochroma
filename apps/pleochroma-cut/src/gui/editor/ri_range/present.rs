//! Putting the RI range into `RiRangeModel`: the geometric hint first, then the traced chip.
//!
//! The texts are pure functions of a range and the catalogue ([`traced_texts`]), so the tests
//! call them without a window. The chip says "(estimate)" wherever it shows the trace, and the
//! materials it lists follow `HIGH_BOUND_CALIBRATED`: the low bound only while the high bound is
//! uncalibrated.

use crate::{MainWindow, RiRangeModel};
use pleochroma::{geometry::meet_solver::SolvedTier, optics::materials::GemMaterial};
use pleochroma_cut_core::{Design, optics_hints::geometric_ri_hint};
use pleochroma_editor::{
    ri_range::{HIGH_BOUND_CALIBRATED, RiRange, SMALL_TILT_DEG, range_text},
    view_model::ri_hint::hint_texts,
};
use slint::ComponentHandle;
use std::sync::LazyLock;

/// How many catalogue materials the chip's hover names before "and N more".
const HOVER_MATERIALS: usize = 8;

/// The wavelength, in nanometers, a material's index is read at (sodium D).
const SODIUM_D_NM: f32 = 589.3;

/// The built-in materials with their index, ascending by index.
static CATALOGUE: LazyLock<Vec<(String, f64)>> = LazyLock::new(|| {
    let mut entries: Vec<(String, f64)> = GemMaterial::built_in()
        .iter()
        .map(|gem| {
            (
                gem.name.clone(),
                f64::from(gem.dispersion.evaluate(SODIUM_D_NM)),
            )
        })
        .collect();
    entries.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    entries
});

/// Shows the geometric hint (chip state 1), or hides the chip when the design has none.
pub(super) fn show_hint(ui: &MainWindow, design: &Design, solved: &[SolvedTier]) {
    let Some(hint) = geometric_ri_hint(design, solved) else {
        hide(ui);
        return;
    };
    let (text, hover) = hint_texts(&hint);
    let model = ui.global::<RiRangeModel>();
    model.set_chip_state(1);
    model.set_chip_text(text.into());
    model.set_chip_hover(hover.into());
}

/// Shows the traced range (chip state 2). A range with no low bound leaves the chip as it is.
pub(super) fn show_traced(ui: &MainWindow, range: &RiRange) {
    let Some((text, hover)) = traced_texts(range, &CATALOGUE, HIGH_BOUND_CALIBRATED) else {
        return;
    };
    let model = ui.global::<RiRangeModel>();
    model.set_chip_state(2);
    model.set_chip_text(text.into());
    model.set_chip_hover(hover.into());
}

/// Hides the chip.
pub(super) fn hide(ui: &MainWindow) {
    let model = ui.global::<RiRangeModel>();
    model.set_chip_state(0);
    model.set_chip_text("".into());
    model.set_chip_hover("".into());
}

/// An index as a catalogue lists it: three decimals, one trailing zero dropped ("1.654", "2.61").
fn index_text(n: f64) -> String {
    let text = format!("{n:.3}");
    text.strip_suffix('0')
        .map_or_else(|| text.clone(), str::to_owned)
}

/// The chip's text and hover for `range`, or `None` when it has no low bound.
///
/// `catalogue` is the materials with their index, ascending. The hover starts with what the trace
/// is, names the bounds, lists the first [`HOVER_MATERIALS`] materials the range allows and
/// counts the rest. `calibrated` is whether the high bound is judged (`HIGH_BOUND_CALIBRATED`).
pub(super) fn traced_texts(
    range: &RiRange,
    catalogue: &[(String, f64)],
    calibrated: bool,
) -> Option<(String, String)> {
    let statement = range.statement_as(calibrated)?;
    let chip = format!("RI {} (estimate)", range_text(&statement));
    let mut hover = format!(
        "Pleochroma estimate: traced at this design's angles, table up and tilted by \
         {SMALL_TILT_DEG:.0}\u{b0}. Below RI {:.2} the windowing passes the limit.",
        statement.low
    );
    if let Some(high) = statement.high {
        hover.push_str(&format!(
            " Above RI {high:.2} the brilliance or the extinction falls short."
        ));
    }
    let allowed: Vec<&(String, f64)> = catalogue
        .iter()
        .filter(|(_, n)| range.contains_as(*n, calibrated))
        .collect();
    let span = statement.high.map_or_else(
        || format!("from {:.2} up", statement.low),
        |high| format!("from {:.2} to {high:.2}", statement.low),
    );
    if allowed.is_empty() {
        hover.push_str(&format!("\n\nNo catalogue material lies {span}."));
    } else {
        let names: Vec<String> = allowed
            .iter()
            .take(HOVER_MATERIALS)
            .map(|(name, n)| format!("{name} {}", index_text(*n)))
            .collect();
        let rest = allowed.len().saturating_sub(HOVER_MATERIALS);
        let more = if rest > 0 {
            format!(" and {rest} more")
        } else {
            String::new()
        };
        hover.push_str(&format!(
            "\n\nCatalogue materials {span}: {}{more}.",
            names.join(", ")
        ));
    }
    hover.push_str("\n\nClick for the RI chart.");
    Some((chip, hover))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_editor::ri_range::{RiGridRow, THRESHOLD_VERSION};

    fn range(n_low: Option<f64>, n_high: Option<f64>) -> RiRange {
        let row = RiGridRow {
            n: 1.5,
            windowing_table_up: 3.0,
            windowing_tilt: 4.0,
            brilliance_tilt: 60.0,
            extinction_tilt: 20.0,
        };
        RiRange {
            n0: 1.768,
            own: row,
            rows: vec![row],
            n_low,
            n_high,
            threshold_version: THRESHOLD_VERSION,
        }
    }

    /// A small catalogue, ascending by index like the real one.
    fn catalogue() -> Vec<(String, f64)> {
        let mut entries: Vec<(String, f64)> = [
            ("Quartz", 1.544),
            ("Peridot", 1.654),
            ("Spinel", 1.718),
            ("Sapphire", 1.768),
            ("Almandine Garnet", 1.79),
            ("Zircon", 1.925),
            ("Cubic Zirconia", 2.15),
            ("Diamond", 2.417),
            ("Rutile", 2.61),
            ("Titanite", 2.0),
            ("Zircon Low", 1.81),
            ("Demantoid", 1.88),
            ("Sphene", 1.95),
            ("Hematite", 2.9),
        ]
        .into_iter()
        .map(|(name, n)| (name.to_owned(), n))
        .collect();
        entries.sort_by(|a, b| a.1.total_cmp(&b.1));
        entries
    }

    #[test]
    fn the_chip_says_estimate_and_the_low_bound_only_while_uncalibrated() {
        let range = range(Some(1.63), Some(2.31));
        let (chip, _) = traced_texts(&range, &catalogue(), false).expect("a low bound");
        assert_eq!(chip, "RI 1.63 and up (estimate)");
        let (chip, _) = traced_texts(&range, &catalogue(), true).expect("a low bound");
        assert_eq!(chip, "RI 1.63 to 2.31 (estimate)");
    }

    #[test]
    fn the_hover_lists_eight_materials_above_the_low_bound_and_counts_the_rest() {
        let (_, hover) =
            traced_texts(&range(Some(1.63), None), &catalogue(), false).expect("a low bound");
        assert!(hover.starts_with(
            "Pleochroma estimate: traced at this design's angles, table up and tilted by 10\u{b0}."
        ));
        assert!(hover.contains("Below RI 1.63 the windowing passes the limit."));
        // Thirteen materials are at or above 1.63; eight are named, five are counted.
        assert!(hover.contains("Catalogue materials from 1.63 up: Peridot 1.654, Spinel 1.718"));
        assert!(hover.contains(" and 5 more."), "{hover}");
        assert!(!hover.contains("Quartz"));
        assert!(hover.ends_with("Click for the RI chart."));
    }

    #[test]
    fn a_calibrated_high_bound_narrows_the_list() {
        let range = range(Some(1.63), Some(2.0));
        let (_, open) = traced_texts(&range, &catalogue(), false).expect("a low bound");
        let (_, both) = traced_texts(&range, &catalogue(), true).expect("a low bound");
        assert!(open.contains("Rutile") || open.contains(" more."));
        assert!(!both.contains("Rutile"));
        assert!(
            both.contains("Catalogue materials from 1.63 to 2.00: "),
            "{both}"
        );
        assert!(both.contains("Above RI 2.00"));
        // Nine materials work; the ninth (Titanite) is counted, not named.
        assert!(both.contains(" and 1 more."), "{both}");
    }

    #[test]
    fn a_range_without_a_low_bound_has_no_traced_chip() {
        assert!(traced_texts(&range(None, None), &catalogue(), false).is_none());
    }

    #[test]
    fn an_empty_catalogue_answer_is_said_plainly() {
        let (_, hover) =
            traced_texts(&range(Some(3.0), None), &catalogue(), false).expect("a low bound");
        assert!(hover.contains("No catalogue material lies from 3.00 up."));
        assert!(!hover.contains("more"));
    }

    #[test]
    fn indices_print_with_two_or_three_decimals() {
        assert_eq!(index_text(1.654), "1.654");
        assert_eq!(index_text(2.61), "2.61");
        assert_eq!(index_text(1.7), "1.70");
        assert_eq!(index_text(2.0), "2.00");
    }

    #[test]
    fn the_real_catalogue_is_ascending_and_holds_the_named_stones() {
        let names: Vec<&str> = CATALOGUE.iter().map(|(name, _)| name.as_str()).collect();
        assert!(names.contains(&"Sapphire"));
        assert!(names.contains(&"Nanosital"));
        assert!(CATALOGUE.windows(2).all(|pair| pair[0].1 <= pair[1].1));
    }
}
