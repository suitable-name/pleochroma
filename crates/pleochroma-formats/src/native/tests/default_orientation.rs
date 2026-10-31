//! A custom snapshot's default crystal-axis orientation in the text of a design file.

use super::*;

/// The three keys are written only when set, round trip, and a snapshot without them writes the
/// bytes it always did.
#[test]
fn an_unset_default_orientation_writes_no_key_and_the_bytes_do_not_move() {
    let mut native = sample_file();
    native.material = native.material.with_custom(Some(trigonal_snapshot()));
    let text = write(&native);
    assert!(!text.contains("c_axis"), "{text}");
    let custom = read(&text).material.custom.expect("snapshot loads back");
    assert_eq!(custom.default_orientation(), (None, None, None));
    // Reading and writing again is byte-identical.
    let mut again = sample_file();
    again.material = again.material.with_custom(Some(custom));
    assert_eq!(write(&again), text);

    // Attaching all three `None` is the same as attaching nothing.
    let mut explicit = sample_file();
    explicit.material = explicit.material.with_custom(Some(
        trigonal_snapshot().with_default_orientation(None, None, None),
    ));
    assert_eq!(write(&explicit), text);
}

#[test]
fn a_default_orientation_round_trips_with_the_shortest_decimal() {
    let mut native = sample_file();
    let snapshot = trigonal_snapshot().with_default_orientation(Some(30.0), Some(45.5), Some(20.0));
    native.material = native.material.with_custom(Some(snapshot.clone()));
    let text = write(&native);
    assert!(text.contains("c_axis_tilt_deg = 30.0"), "{text}");
    assert!(text.contains("c_axis_azimuth_deg = 45.5"), "{text}");
    assert!(text.contains("c_axis_second_azimuth_deg = 20.0"), "{text}");
    let custom = read(&text).material.custom.expect("snapshot loads back");
    assert_eq!(custom, snapshot);
    assert_eq!(
        custom.default_orientation(),
        (Some(30.0), Some(45.5), Some(20.0))
    );
}
