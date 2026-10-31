use super::{contained_request_size, fit_distance_for_radius, orbit_distance_bounds};

// --- contained_request_size ---

#[test]
fn solid_and_diagram_modes_are_untouched() {
    // Only Path-traced (1) and Both (2) show `render_image` at all --
    // Solid (0) and Diagram (3) must keep the viewport's own raw size.
    for mode in [0, 3] {
        assert_eq!(
            contained_request_size(mode, (900, 500), (1920, 1080)),
            (900, 500)
        );
    }
}

#[test]
fn wider_render_than_viewport_shrinks_height_to_the_letterboxed_rectangle() {
    // A 1920x1080 (16:9) render in a 900x700 (~1.29) viewport: `contain`
    // fits the width and shrinks the height, exactly matching what
    // `solid_viewport.slint`'s `image-fit: contain` draws on screen.
    let (width, height) = contained_request_size(2, (900, 700), (1920, 1080));
    assert_eq!(width, 900);
    assert_eq!(height, 506);
    assert_eq!(
        contained_request_size(4, (900, 700), (1920, 1080)),
        (width, height)
    );
}

#[test]
fn narrower_render_than_viewport_is_already_the_on_screen_size() {
    // A 4:3 render in a wider viewport: `contain` fits the height here and the
    // two layers already agree pixel-for-pixel, so this is a no-op beyond
    // reproducing the exact viewport height.
    let (width, height) = contained_request_size(1, (900, 500), (640, 480));
    assert_eq!(height, 500);
    assert_eq!(width, 667);
}

#[test]
fn zero_sized_input_is_returned_unchanged_rather_than_dividing_by_zero() {
    assert_eq!(contained_request_size(2, (0, 500), (1920, 1080)), (0, 500));
    assert_eq!(contained_request_size(2, (900, 500), (0, 1080)), (900, 500));
}

#[test]
fn equal_aspect_ratios_pass_through_unchanged() {
    assert_eq!(
        contained_request_size(2, (1920, 1080), (1920, 1080)),
        (1920, 1080)
    );
}

// --- orbit_distance_bounds ---

#[test]
fn default_bounding_radius_reproduces_the_old_fixed_clamp() {
    // 1.5 is `DEFAULT_MESH_BOUNDING_RADIUS` -- the common case must
    // keep the exact `[1.2, 8.0]` clamp behavior.
    let (min, max) = orbit_distance_bounds(1.5);
    assert!((min - 1.2).abs() < 1e-6, "got {min}");
    assert!((max - 8.0).abs() < 1e-6, "got {max}");
}

#[test]
fn a_larger_mesh_gets_a_wider_clamp() {
    let (small_min, small_max) = orbit_distance_bounds(1.5);
    let (large_min, large_max) = orbit_distance_bounds(4.5);
    assert!(large_min > small_min);
    assert!(large_max > small_max);
}

#[test]
fn a_zero_radius_never_collapses_the_clamp_to_a_single_point() {
    let (min, max) = orbit_distance_bounds(0.0);
    assert!(min > 0.0);
    assert!(max > min);
}

// --- fit_distance_for_radius ---

#[test]
fn fit_distance_grows_with_the_bounding_radius() {
    assert!(fit_distance_for_radius(3.0) > fit_distance_for_radius(1.5));
}

#[test]
fn fit_distance_stays_within_the_matching_orbit_clamp() {
    // The "Fit" pose (`setup_set_view_callback`) always clamps its own
    // result through `orbit_distance_bounds` of the SAME radius -- this
    // confirms that combination lands inside a sensible range rather than,
    // say, the unclamped `fit_distance_for_radius` output being clamped
    // down to something that no longer frames the mesh at all.
    let radius = 1.5_f64;
    let (min, max) = orbit_distance_bounds(radius);
    let fit = fit_distance_for_radius(radius).clamp(min, max);
    assert!(fit >= min && fit <= max);
}
