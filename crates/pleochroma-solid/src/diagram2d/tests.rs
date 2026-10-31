//! Unit tests for the panel visibility predicates, the index-wheel screen
//! mapping, full-frame rendering, and meet-marker point resolution.

use super::{
    fill::meet_marker_points,
    layout::{crown_visible, pavilion_visible, profile_visible, wheel_direction},
    *,
};
use glam::{DVec3, Vec3};
use pleochroma::geometry::stone_metrics::{SolidMesh, SolidStatus, build_solid_mesh};

/// "Visible" here means "selected by the panel's own facet-visibility
/// predicate" (see the parent module's doc comment) -- a cube's front profile face
/// still fully occludes its back face once rasterized, exactly like
/// `raster.rs`'s own crown/pavilion visibility test does before its depth
/// test runs. This is the count the "4 visible side facets in
/// profile and 1 in crown" describes.
#[test]
fn cube_visibility_predicates_select_one_crown_facet_and_four_profile_facets() {
    let cube_normals = [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ];
    let crown_count = cube_normals.iter().filter(|&&n| crown_visible(n)).count();
    let pavilion_count = cube_normals
        .iter()
        .filter(|&&n| pavilion_visible(n))
        .count();
    let profile_count = cube_normals.iter().filter(|&&n| profile_visible(n)).count();

    assert_eq!(crown_count, 1, "only +Y should read as crown-facing");
    assert_eq!(pavilion_count, 1, "only -Y should read as pavilion-facing");
    assert_eq!(
        profile_count, 4,
        "the four vertical faces (+-X, +-Z) should all read as profile-facing"
    );
}

/// Gear 96, no reference-angle offset: index 0 must land at 12 o'clock
/// (straight up) on the crown panel, and index 24 (a quarter turn) at 3
/// o'clock -- see the parent module's doc comment for why crown sweeps clockwise.
#[test]
fn crown_index_wheel_places_index_0_at_top_and_index_24_at_three_oclock() {
    let gear_teeth = 96.0f32;
    let phi0 = 2.0 * std::f32::consts::PI * 0.0 / gear_teeth;
    let phi24 = 2.0 * std::f32::consts::PI * 24.0 / gear_teeth;

    let (right0, up0) = wheel_direction(phi0, false);
    assert!(
        right0.abs() < 1e-5,
        "index 0 must have no horizontal offset"
    );
    assert!(up0 > 0.99, "index 0 must point straight up (screen_up > 0)");

    let (right24, up24) = wheel_direction(phi24, false);
    assert!(
        right24 > 0.99,
        "index 24 must point straight right (3 o'clock)"
    );
    assert!(up24.abs() < 1e-5, "index 24 must have no vertical offset");
}

/// The pavilion panel mirrors the horizontal axis, so the same index lands on
/// the OPPOSITE side (9 o'clock instead of 3 o'clock) while index 0 stays at
/// the top on both panels.
#[test]
fn pavilion_index_wheel_mirrors_the_horizontal_axis() {
    let gear_teeth = 96.0f32;
    let phi24 = 2.0 * std::f32::consts::PI * 24.0 / gear_teeth;
    let (right24, up24) = wheel_direction(phi24, true);
    assert!(
        right24 < -0.99,
        "index 24 must point left (9 o'clock) when mirrored"
    );
    assert!(up24.abs() < 1e-5);
}

/// A closed box's crown/pavilion/profile panels each pick back the expected
/// facet id at their own panel center -- the pick-buffer round trip the
/// hover/click wiring depends on.
#[test]
fn pick_buffer_round_trips_each_panels_center_to_the_right_facet() {
    let mesh = match build_solid_mesh(&[
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::NEG_Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ]) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("box fixture must close: {other:?}"),
    };
    let config = DiagramConfig {
        width: 300,
        height: 120,
        gear_teeth: 96,
        gear_reference_angle: 0.0,
        symmetry_order: 8,
        mirror: true,
    };
    let style = DiagramStyle::default();
    let frame = render_diagram(&mesh, &config, &style);

    // Panel centers sit at column midpoints (300 / 3 = 100px per column).
    assert_eq!(
        frame.pick_at(50, 60),
        Some(2),
        "crown panel center must show the +Y facet (id 2)"
    );
    assert_eq!(
        frame.pick_at(150, 60),
        Some(3),
        "pavilion panel center must show the -Y facet (id 3)"
    );
    assert_eq!(
        frame.pick_at(250, 60),
        Some(4),
        "profile panel center must show the nearer of +-Z (id 4, +Z)"
    );

    assert_eq!(frame.panel_at(50, 60), Some(PanelKind::Crown));
    assert_eq!(frame.panel_at(150, 60), Some(PanelKind::Pavilion));
    assert_eq!(frame.panel_at(250, 60), Some(PanelKind::Profile));
}

/// The end view looks from world `-X`: the profile centre shows the `+Z` face (id 4), the end
/// view's centre shows the `-X` face (id 1), and the picked facet ids are the box's own.
#[test]
fn the_end_view_looks_from_minus_x_and_keeps_the_facet_ids() {
    let mesh = match build_solid_mesh(&[
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::NEG_Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ]) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("box fixture must close: {other:?}"),
    };
    let config = DiagramConfig {
        width: 200,
        height: 120,
        gear_teeth: 96,
        gear_reference_angle: 0.0,
        symmetry_order: 8,
        mirror: true,
    };
    let style = DiagramStyle::default();
    let profile = render_diagram_single_panel(&mesh, &config, &style, PanelKind::Profile);
    assert_eq!(profile.pick_at(100, 60), Some(4), "profile: the +Z face");
    let end = render_diagram_end_view(&mesh, &config, &style);
    assert_eq!(end.pick_at(100, 60), Some(1), "end view: the -X face");
    assert_eq!(end.panel_at(100, 60), Some(PanelKind::Profile));
    assert!(end.layout.enlarged, "one panel fills the frame");
}

#[test]
fn an_empty_or_zero_sized_config_never_panics() {
    let mesh = SolidMesh::default();
    let config = DiagramConfig {
        width: 0,
        height: 0,
        gear_teeth: 96,
        gear_reference_angle: 0.0,
        symmetry_order: 8,
        mirror: false,
    };
    let frame = render_diagram(&mesh, &config, &DiagramStyle::default());
    assert_eq!(frame.width, 0);
    assert_eq!(frame.pick_at(0, 0), None);
}

/// A frame of any size draws. The tooth numbers were pulled inside their column with
/// `f32::clamp`, which panics when its bounds cross: a 16 px frame has 6 px columns, a
/// two-digit number is 11 px long, `clamp(0.0, -6.0)`. The render worker lost that frame, and
/// with it the Diagram, whenever a viewport was momentarily this small (a window or dock being
/// dragged narrow, a view not laid out yet). Sizes on both sides of the large-frame height
/// and of the column width a number needs.
#[test]
fn a_tiny_frame_draws_without_panicking() {
    let mesh = box_mesh();
    let style = DiagramStyle::default();
    for (width, height) in [
        (1, 1),
        (2, 2),
        (5, 5),
        (6, 40),
        (16, 16),
        (30, 10),
        (33, 300),
        (299, 299),
        (300, 300),
        (301, 40),
        (900, 300),
    ] {
        let config = DiagramConfig {
            width,
            height,
            ..box_config()
        };
        let three = render_diagram(&mesh, &config, &style);
        assert_eq!((three.width, three.height), (width, height));
        assert_eq!(three.layout.panels.len(), 3);
        for panel in [PanelKind::Crown, PanelKind::Pavilion, PanelKind::Profile] {
            let single = render_diagram_single_panel(&mesh, &config, &style, panel);
            assert_eq!((single.width, single.height), (width, height));
        }
    }
}

/// The enlarged panel must fill the WHOLE frame width, not the one-third
/// column [`render_diagram`] gives it -- otherwise "enlarge this panel"
/// would just be a relabeled crop of the existing image.
#[test]
fn render_diagram_single_panel_fills_the_whole_frame() {
    let mesh = match build_solid_mesh(&[
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::NEG_Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ]) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("box fixture must close: {other:?}"),
    };
    let config = DiagramConfig {
        width: 300,
        height: 120,
        gear_teeth: 96,
        gear_reference_angle: 0.0,
        symmetry_order: 8,
        mirror: false,
    };
    let style = DiagramStyle::default();
    let frame = render_diagram_single_panel(&mesh, &config, &style, PanelKind::Crown);

    // The crown panel's own center, now the FRAME's center (150, 60) rather
    // than a one-third column's (50, 60) -- see `render_diagram`'s own
    // matching assertion at column-center (50, 60).
    assert_eq!(
        frame.pick_at(150, 60),
        Some(2),
        "the enlarged panel's own center must show the +Y facet"
    );
    assert_eq!(frame.panel_at(150, 60), Some(PanelKind::Crown));
    // "Fills the whole frame" in the sense the API can actually express:
    // `panel_at` tags only pixels a facet paints (see its own doc comment), so a
    // background pixel is `None` by contract, not `Some`. What must hold is that
    // no pixel anywhere belongs to a DIFFERENT panel -- in the three-column
    // layout, x = 200 would have been the pavilion column.
    for y in 0..config.height {
        for x in 0..config.width {
            let tag = frame.panel_at(x, y);
            assert!(
                tag.is_none() || tag == Some(PanelKind::Crown),
                "pixel ({x}, {y}) is tagged {tag:?}, but only the enlarged crown panel may paint in a single-panel frame"
            );
        }
    }
}

/// [`meet_marker_points`] must find the actual shared vertices between two
/// touching facets, not merely trust that a candidate pair (from
/// `FacetMap::meeting_facet_pairs`) really touches.
#[test]
fn meet_marker_points_finds_the_shared_edge_between_two_touching_facets() {
    let mesh = match build_solid_mesh(&[
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::NEG_Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ]) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("box fixture must close: {other:?}"),
    };
    // Facet 0 (+X) and facet 2 (+Y) share the edge x=1, y=0.6, z in [-1, 1] --
    // two corners, (1, 0.6, -1) and (1, 0.6, 1).
    let points = meet_marker_points(&mesh, &[(0, 2)], 1e-4);
    assert_eq!(points.len(), 2, "got: {points:?}");
    for expected_z in [-1.0, 1.0] {
        assert!(
            points.iter().any(|p| (p.x - 1.0).abs() < 1e-6
                && (p.y - 0.6).abs() < 1e-6
                && (p.z - expected_z).abs() < 1e-6),
            "expected a marker at z={expected_z}, got: {points:?}"
        );
    }

    // An unrelated pair (facets that never touch) must find nothing.
    assert_eq!(meet_marker_points(&mesh, &[(0, 1)], 1e-4).len(), 0);
}

/// A one-piece "tool" mesh: a square at `y = 0` with facet id 0 whose piece normal is
/// `(0, normal_y, 0)`. Built by hand so the test pins the visibility rule, not any
/// tool tessellation.
fn tool_piece_mesh(normal_y: f64) -> SolidMesh {
    let normal = DVec3::new(0.0, normal_y, 0.0);
    let ring = vec![
        DVec3::new(-0.5, 0.0, -0.5),
        DVec3::new(-0.5, 0.0, 0.5),
        DVec3::new(0.5, 0.0, 0.5),
        DVec3::new(0.5, 0.0, -0.5),
    ];
    let mut mesh = SolidMesh::default();
    for &corner in &ring {
        mesh.positions.push(corner);
        mesh.normals.push(normal);
        mesh.facet_id.push(0);
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    mesh.rings.push((0, ring));
    mesh.piece_normals = Some(vec![normal]);
    mesh.edge_visible = Some(vec![vec![true; 4]]);
    mesh
}

#[test]
fn diagram_hides_undercut_tool_pieces_from_the_opposite_panel() {
    use pleochroma::geometry::meet_solver::Block;
    let config = DiagramConfig {
        width: 300,
        height: 120,
        gear_teeth: 96,
        gear_reference_angle: 0.0,
        symmetry_order: 8,
        mirror: true,
    };
    let pavilion_tool = DiagramStyle {
        tool_facet_block: vec![Some(Block::Pavilion)],
        ..DiagramStyle::default()
    };
    let crown_tool = DiagramStyle {
        tool_facet_block: vec![Some(Block::Crown)],
        ..DiagramStyle::default()
    };

    // An undercut wall of a pavilion tool: its normal points UP, which the crown
    // panel's normal test alone would accept.
    let undercut = tool_piece_mesh(1.0);
    let hidden = render_diagram(&undercut, &config, &pavilion_tool);
    assert_eq!(
        hidden.pick_at(50, 60),
        None,
        "a pavilion tool's upward-facing piece must not show on the crown panel"
    );
    let shown = render_diagram(&undercut, &config, &crown_tool);
    assert_eq!(
        shown.pick_at(50, 60),
        Some(0),
        "the same piece does show for a crown-block tool"
    );
    // A flat facet (no block entry) with the same normal is unaffected.
    let flat = render_diagram(&undercut, &config, &DiagramStyle::default());
    assert_eq!(flat.pick_at(50, 60), Some(0));

    // The ordinary pavilion wall: facing down, in its own block's panel.
    let wall = tool_piece_mesh(-1.0);
    let on_pavilion = render_diagram(&wall, &config, &pavilion_tool);
    assert_eq!(on_pavilion.pick_at(150, 60), Some(0));
    let on_crown_block = render_diagram(&wall, &config, &crown_tool);
    assert_eq!(
        on_crown_block.pick_at(150, 60),
        None,
        "a crown tool's piece never shows on the pavilion panel"
    );
}

/// The closed box the layout and outline tests draw: facet 0 is +X, 2 is +Y (crown),
/// 3 is -Y (pavilion), 4 is +Z.
fn box_mesh() -> SolidMesh {
    match build_solid_mesh(&[
        (DVec3::X, 1.0),
        (DVec3::NEG_X, 1.0),
        (DVec3::Y, 0.6),
        (DVec3::NEG_Y, 0.6),
        (DVec3::Z, 1.0),
        (DVec3::NEG_Z, 1.0),
    ]) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("box fixture must close: {other:?}"),
    }
}

fn box_config() -> DiagramConfig {
    DiagramConfig {
        width: 300,
        height: 120,
        gear_teeth: 96,
        gear_reference_angle: 1.5,
        symmetry_order: 8,
        mirror: true,
    }
}

/// How many pixels of `frame` are exactly `rgb` (alpha ignored).
fn pixels_of_color(frame: &DiagramFrame, rgb: [u8; 3]) -> usize {
    let (pixels, _remainder) = frame.color.as_chunks::<4>();
    pixels.iter().filter(|px| px[..3] == rgb).count()
}

#[test]
fn the_three_panel_frame_carries_its_layout() {
    let config = box_config();
    let frame = render_diagram(&box_mesh(), &config, &DiagramStyle::default());
    let layout = &frame.layout;

    assert!(!layout.enlarged);
    assert_eq!(layout.gear_teeth, 96);
    assert!((layout.gear_reference_angle - 1.5).abs() < 1e-6);
    let kinds: Vec<PanelKind> = layout.panels.iter().map(|panel| panel.kind).collect();
    assert_eq!(
        kinds,
        [PanelKind::Crown, PanelKind::Pavilion, PanelKind::Profile]
    );
    // Columns of 100 px: the centres are the column midpoints, and the clip rectangles
    // tile the frame without a gap or an overlap.
    for (index, panel) in layout.panels.iter().enumerate() {
        assert!((panel.center_x - 100.0_f32.mul_add(index as f32, 50.0)).abs() < 1e-3);
        assert!(panel.scale > 0.0 && panel.wheel_radius_px > 0.0);
        let column = i32::try_from(index).expect("a panel index fits in i32");
        assert_eq!(panel.clip.0, 100 * column);
        assert_eq!(panel.clip.2, 100 * (column + 1) - 1);
    }
    assert_eq!(layout.panel(PanelKind::Pavilion), Some(&layout.panels[1]));
}

#[test]
fn panel_at_finds_the_column_a_pixel_is_in() {
    let frame = render_diagram(&box_mesh(), &box_config(), &DiagramStyle::default());
    let layout = &frame.layout;
    let kind_at = |x: f32, y: f32| layout.panel_at(x, y).map(|panel| panel.kind);
    assert_eq!(kind_at(0.0, 0.0), Some(PanelKind::Crown));
    assert_eq!(kind_at(99.9, 119.9), Some(PanelKind::Crown));
    assert_eq!(kind_at(100.0, 60.0), Some(PanelKind::Pavilion));
    assert_eq!(kind_at(250.0, 60.0), Some(PanelKind::Profile));
    assert_eq!(kind_at(299.9, 0.0), Some(PanelKind::Profile));
    assert_eq!(kind_at(300.0, 60.0), None, "right of the frame");
    assert_eq!(kind_at(-0.5, 60.0), None, "left of the frame");
    assert_eq!(kind_at(50.0, 120.0), None, "below the frame");
    assert_eq!(DiagramLayout::default().panel_at(10.0, 10.0), None);
}

#[test]
fn an_enlarged_frame_carries_one_panel_spanning_the_frame() {
    let frame = render_diagram_single_panel(
        &box_mesh(),
        &box_config(),
        &DiagramStyle::default(),
        PanelKind::Pavilion,
    );
    let layout = &frame.layout;
    assert!(layout.enlarged);
    assert_eq!(layout.panels.len(), 1);
    assert!(layout.panel(PanelKind::Crown).is_none());
    let panel = layout
        .panel(PanelKind::Pavilion)
        .expect("the enlarged panel");
    assert!(
        (panel.center_x - 150.0).abs() < 1e-3,
        "the frame's own centre"
    );
    assert_eq!(panel.clip.0, 0);
    assert_eq!(panel.clip.2, 299);
    assert_eq!(layout.gear_teeth, 96);
}

#[test]
fn an_empty_frame_has_no_panels() {
    let config = DiagramConfig {
        width: 0,
        height: 0,
        ..box_config()
    };
    let frame = render_diagram(&SolidMesh::default(), &config, &DiagramStyle::default());
    assert_eq!(frame.layout.panels.len(), 0);
}

#[test]
fn project_point_lands_on_the_pixels_the_panel_drew() {
    let frame = render_diagram(&box_mesh(), &box_config(), &DiagramStyle::default());
    let layout = &frame.layout;
    let crown = layout.panel(PanelKind::Crown).expect("crown");
    let pavilion = layout.panel(PanelKind::Pavilion).expect("pavilion");
    let profile = layout.panel(PanelKind::Profile).expect("profile");

    // The axis is the centre of the crown and pavilion panels.
    let (x, y, _) = project_point(DVec3::ZERO, crown);
    assert!((x - crown.center_x).abs() < 1e-4 && (y - crown.center_y).abs() < 1e-4);

    // Crown looks down -Y with +X up the screen and +Z to the right; the pavilion
    // mirrors the horizontal axis; the profile has +X to the right and +Y up.
    let side = DVec3::new(0.5, 0.0, 0.25);
    let (cx, cy, _) = project_point(side, crown);
    assert!((cx - 0.25_f32.mul_add(crown.scale, crown.center_x)).abs() < 1e-3);
    assert!((cy - 0.5_f32.mul_add(-crown.scale, crown.center_y)).abs() < 1e-3);
    let (px, py, _) = project_point(side, pavilion);
    assert!((px - 0.25_f32.mul_add(-pavilion.scale, pavilion.center_x)).abs() < 1e-3);
    assert!((py - cy + (crown.center_y - pavilion.center_y)).abs() < 1e-3);
    let (sx, sy, _) = project_point(DVec3::new(0.5, 0.3, 0.0), profile);
    assert!((sx - 0.5_f32.mul_add(profile.scale, profile.center_x)).abs() < 1e-3);
    assert!((sy - 0.3_f32.mul_add(-profile.scale, profile.center_y)).abs() < 1e-3);

    // A point on the +Y face, projected on the crown panel, picks that facet back.
    let (fx, fy, _) = project_point(DVec3::new(0.2, 0.6, 0.2), crown);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the projected point lies inside the frame, so it is a small non-negative pixel"
    )]
    let (crown_col, crown_row) = (fx as u32, fy as u32);
    assert_eq!(frame.pick_at(crown_col, crown_row), Some(2));
    let (bx, by, _) = project_point(DVec3::new(0.2, -0.6, 0.2), pavilion);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the projected point lies inside the frame, so it is a small non-negative pixel"
    )]
    let (pavilion_col, pavilion_row) = (bx as u32, by as u32);
    assert_eq!(frame.pick_at(pavilion_col, pavilion_row), Some(3));
}

#[test]
fn a_panel_kind_shows_the_facets_its_visibility_predicate_selects() {
    assert!(PanelKind::Crown.shows(Vec3::Y));
    assert!(!PanelKind::Crown.shows(Vec3::NEG_Y));
    assert!(PanelKind::Pavilion.shows(Vec3::NEG_Y));
    assert!(!PanelKind::Pavilion.shows(Vec3::Y));
    assert!(PanelKind::Profile.shows(Vec3::X));
    assert!(PanelKind::Profile.shows(Vec3::new(0.3, 0.9, 0.3).normalize()));
    assert!(!PanelKind::Profile.shows(Vec3::Y));
    // A girdle facet (no vertical component) is on neither wheel panel.
    assert!(!PanelKind::Crown.shows(Vec3::X) && !PanelKind::Pavilion.shows(Vec3::X));
}

#[test]
fn a_moved_facet_is_outlined_in_the_moved_color() {
    let mesh = box_mesh();
    let config = box_config();
    let plain = DiagramStyle::default();
    let moved_color = plain.moved_color;
    assert_eq!(
        pixels_of_color(&render_diagram(&mesh, &config, &plain), moved_color),
        0,
        "nothing is outlined before a drag moves a tier"
    );

    let moved = DiagramStyle {
        moved: vec![2],
        ..DiagramStyle::default()
    };
    assert!(
        pixels_of_color(&render_diagram(&mesh, &config, &moved), moved_color) > 0,
        "the crown facet in `moved` gets the moved outline"
    );

    // A selection keeps its own outline on a facet that is also in `moved`.
    let selected_too = DiagramStyle {
        moved: vec![2],
        selected: vec![false, false, true, false, false, false],
        ..DiagramStyle::default()
    };
    assert_eq!(
        pixels_of_color(&render_diagram(&mesh, &config, &selected_too), moved_color),
        0,
        "the selected outline wins the facet's edges"
    );
}

/// A viewport-sized frame (at least `LARGE_FRAME_MIN_HEIGHT` tall) draws each caption
/// scaled up and under its own panel, inside the panel's column, clear of the top edge the
/// viewport's border and toolbar cover; the tooth numbers stay inside their columns.
#[test]
fn a_large_frame_sets_each_caption_under_its_panel_and_scaled_up() {
    let config = DiagramConfig {
        width: 850,
        height: 856,
        ..box_config()
    };
    let style = DiagramStyle::default();
    let frame = render_diagram(&box_mesh(), &config, &style);
    let scale = layout::text_scale(config.height);
    assert_eq!(
        scale, 2,
        "an 856 px frame draws its text at twice the glyph size"
    );

    for panel in &frame.layout.panels {
        let top = layout::caption_y(panel, config.height);
        let (_, text_h) = text_size("PAVILION", scale);
        // Under the panel's own picture, inside the frame, and not on the top edge.
        assert!(
            top > panel.center_y,
            "{:?} caption sits under the panel",
            panel.kind
        );
        assert!(top + text_h as f32 <= config.height as f32 - 7.0);
        // The caption's pixels (the wheel color) exist in the caption's own rows.
        let (left, _, right, _) = panel.clip;
        let found = (0..config.height)
            .filter(|&y| (y as f32) >= top - 1.0 && (y as f32) <= top + text_h as f32)
            .any(|y| {
                (left..=right).any(|x| {
                    let idx = (y * frame.width + x.cast_unsigned()) as usize * 4;
                    frame.color[idx..idx + 3] == style.wheel_color
                })
            });
        assert!(found, "{:?} caption is drawn in its rows", panel.kind);
    }
    // Nothing of the wheel color touches the top 7 rows of the frame: the old caption row
    // (y = 2) is empty, so the viewport border cannot cut a half-hidden caption.
    for y in 0..7usize {
        for x in 0..config.width as usize {
            let idx = (y * config.width as usize + x) * 4;
            assert_ne!(frame.color[idx..idx + 3], style.wheel_color, "({x}, {y})");
        }
    }
}

/// A stone much deeper below the girdle plane than above it (a steep crown angle moves the
/// bounding box, not the profile's centre): in a viewport-sized frame the whole profile, down
/// to the deepest point, stays above the profile caption instead of running through it.
#[test]
fn a_large_frame_keeps_a_deep_profile_above_its_caption() {
    let mesh = match build_solid_mesh(&[
        (DVec3::X, 0.5),
        (DVec3::NEG_X, 0.5),
        (DVec3::Y, 0.2),
        (DVec3::NEG_Y, 1.6),
        (DVec3::Z, 0.5),
        (DVec3::NEG_Z, 0.5),
    ]) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("deep-box fixture must close: {other:?}"),
    };
    let config = DiagramConfig {
        width: 850,
        height: 856,
        ..box_config()
    };
    let frame = render_diagram(&mesh, &config, &DiagramStyle::default());
    let profile = frame.layout.panel(PanelKind::Profile).expect("profile");
    let caption_top = layout::caption_y(profile, config.height);
    for (name, y) in [("deepest", -1.6), ("highest", 0.2)] {
        let (_, screen_y, _) = layout::profile_project(DVec3::new(0.0, y, 0.0), profile);
        assert!(
            screen_y < caption_top,
            "the {name} point of the profile ({screen_y}) is above the caption ({caption_top})"
        );
        assert!(screen_y > 0.0, "the {name} point is inside the frame");
    }
}

/// A small frame (a thumbnail, the pinned test frames) keeps the original layout: caption
/// text at its glyph size on row 2 and the original tick-ring radius.
#[test]
fn a_small_frame_keeps_the_original_caption_row_and_wheel() {
    let frame = render_diagram(&box_mesh(), &box_config(), &DiagramStyle::default());
    assert_eq!(layout::text_scale(120), 1);
    for panel in &frame.layout.panels {
        assert!((layout::caption_y(panel, 120) - 2.0).abs() < 1e-6);
    }
    let crown = frame.layout.panel(PanelKind::Crown).expect("crown");
    // 300x120: 100 px columns, a 10 px caption row, so a 100 px box and a 0.86 ring.
    assert!((crown.wheel_radius_px - 43.0).abs() < 1e-3);
}
