//! The one sentence that tells a cutter how the crystal axis runs in the rough, in the words
//! every surface of the app uses (the phone's Cutting Mode, the cutting sheet).
//!
//! The axis is described the way the Stone tab sets it: how far it leans from the table
//! normal and toward which azimuth (see the module documentation of [`super`] for the
//! convention), plus the turn of the second axis of a biaxial stone when one is set.

use super::CrystalOrientation;

/// Tilts within this many degrees of 0 or 90 are said as "straight through the table" and
/// "flat in the table plane".
const SNAP_DEG: f64 = 0.5;

/// `Orient the rough so the crystal axis (c-axis) runs ...` for `orientation`.
///
/// - Tilt 0: `runs straight through the table.`
/// - Tilt 90: `lies flat in the table plane, along azimuth 33°.`
/// - Anything between: `leans 40° from the table normal, toward azimuth 120°.`
///
/// A Second Axis turn adds `The second axis is turned 37° about it.` The azimuth runs from
/// the model's `+X` side toward `+Z` (clockwise seen from above the table).
#[must_use]
pub fn orientation_sentence(orientation: &CrystalOrientation) -> String {
    orientation_sentence_with_index(orientation, None)
}

/// [`orientation_sentence`] with the azimuth also named as an index of the design's index gear.
///
/// The index reads `toward azimuth 120° (index 32).`, as the cutting sheet prints it. `index` is
/// the text of the index (`"32"`); it is ignored for an axis through the table, which has no
/// azimuth. With `None` this is exactly [`orientation_sentence`], so the phone and the desktop
/// share the words.
#[must_use]
pub fn orientation_sentence_with_index(
    orientation: &CrystalOrientation,
    index: Option<&str>,
) -> String {
    let tilt = orientation.axis.tilt_deg();
    let azimuth = orientation.axis.azimuth_deg();
    let at = index.map_or_else(String::new, |index| format!(" (index {index})"));
    let course = if tilt <= SNAP_DEG {
        "runs straight through the table.".to_owned()
    } else if tilt >= 90.0 - SNAP_DEG {
        format!("lies flat in the table plane, along azimuth {azimuth:.0}\u{b0}{at}.")
    } else {
        format!(
            "leans {tilt:.0}\u{b0} from the table normal, toward azimuth {azimuth:.0}\u{b0}{at}."
        )
    };
    let second = orientation
        .second_turn_deg
        .map_or_else(String::new, |turn| {
            format!(" The second axis is turned {turn:.0}\u{b0} about it.")
        });
    format!("Orient the rough so the crystal axis (c-axis) {course}{second}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_shapes_of_the_sentence() {
        let through = CrystalOrientation::from_angles(0.0, 0.0, None);
        assert_eq!(
            orientation_sentence(&through),
            "Orient the rough so the crystal axis (c-axis) runs straight through the table."
        );
        let flat = CrystalOrientation::from_angles(90.0, 33.0, None);
        assert_eq!(
            orientation_sentence(&flat),
            "Orient the rough so the crystal axis (c-axis) lies flat in the table plane, along azimuth 33\u{b0}."
        );
        let leaning = CrystalOrientation::from_angles(40.0, 120.0, None);
        assert_eq!(
            orientation_sentence(&leaning),
            "Orient the rough so the crystal axis (c-axis) leans 40\u{b0} from the table normal, toward azimuth 120\u{b0}."
        );
    }

    #[test]
    fn a_second_axis_turn_is_named() {
        let turned = CrystalOrientation::from_angles(90.0, 0.0, Some(37.0));
        assert!(
            orientation_sentence(&turned)
                .ends_with(" The second axis is turned 37\u{b0} about it.")
        );
    }

    #[test]
    fn a_line_past_the_equator_reads_as_its_canonical_form() {
        // Tilt 150 from the normal is the same line as tilt 30 on the opposite azimuth.
        let a = CrystalOrientation::from_angles(150.0, 0.0, None);
        let b = CrystalOrientation::from_angles(30.0, 180.0, None);
        assert_eq!(orientation_sentence(&a), orientation_sentence(&b));
    }
}
