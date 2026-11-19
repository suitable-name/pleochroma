//! The texts of the Stone tab's geometric refractive-index hint.

use pleochroma_cut_core::optics_hints::GeometricRiHint;

/// The hint's index, rounded up to two decimals so "or more" is never understated.
///
/// A value within a rounding error of a whole hundredth stays on it (`1.5` is not `1.51`).
fn index_text(n_geo: f64) -> String {
    let hundredths = (n_geo * 100.0 - 1e-6).ceil();
    format!("{:.2}", hundredths / 100.0)
}

/// The chip text and the hover text of the geometric hint.
///
/// The chip reads "Pavilion needs RI 1.50 or more (P2)"; the hover says where the number comes
/// from and what it leaves out. The hint is for light entering through the table only, so these
/// texts never go on a cutting sheet.
#[must_use]
pub fn hint_texts(hint: &GeometricRiHint) -> (String, String) {
    let index = index_text(hint.n_geo);
    let chip = format!("Pavilion needs RI {index} or more ({})", hint.tier);
    let hover = format!(
        "From the pavilion angles alone: {tier} ({angle:.2}\u{b0}) is the shallowest pavilion tier. \
         In a material below about RI {index}, light entering through the table leaks out through \
         {tier}. Tilted views and light entering through the crown are not counted; the traced \
         range replaces this hint once it is computed.",
        tier = hint.tier,
        angle = hint.angle_deg,
    );
    (chip, hover)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(angle_deg: f64) -> GeometricRiHint {
        GeometricRiHint {
            n_geo: 1.0 / angle_deg.to_radians().sin(),
            tier: "P2".to_owned(),
            angle_deg,
        }
    }

    #[test]
    fn the_chip_names_the_index_and_the_tier() {
        let (chip, _) = hint_texts(&hint(41.9));
        assert_eq!(chip, "Pavilion needs RI 1.50 or more (P2)");
    }

    #[test]
    fn the_hover_explains_the_hint_and_its_limits() {
        let (_, hover) = hint_texts(&hint(41.9));
        assert_eq!(
            hover,
            "From the pavilion angles alone: P2 (41.90\u{b0}) is the shallowest pavilion tier. \
             In a material below about RI 1.50, light entering through the table leaks out \
             through P2. Tilted views and light entering through the crown are not counted; \
             the traced range replaces this hint once it is computed."
        );
    }

    #[test]
    fn the_index_rounds_up_but_an_exact_hundredth_stays() {
        assert_eq!(index_text(1.4975), "1.50");
        assert_eq!(index_text(1.501), "1.51");
        assert_eq!(index_text(1.5), "1.50");
        assert_eq!(index_text(1.5 + 1e-12), "1.50");
    }
}
