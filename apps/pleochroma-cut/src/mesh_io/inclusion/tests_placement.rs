//! Inclusions of a block, a cylinder and a pebble: typed positions and sizes, mesh files placed
//! by their centre, and the refusal texts that say what to change.

use super::{
    placement::{MAX_INCLUSION_SIZE_MM, placed_centre, rough_centre_mm, used_bbox_centre},
    *,
};
use crate::mesh_io::{fixtures::CUBE_OBJ, parse_obj_mesh};
use pleochroma_cut_core::rough_plan::Axis;

const TOLERANCE: f64 = 1e-9;

fn block(x_mm: f64, y_mm: f64, z_mm: f64) -> RoughBase {
    RoughBase::Block { x_mm, y_mm, z_mm }
}

fn cylinder() -> RoughBase {
    RoughBase::Cylinder {
        diameter_mm: 12.0,
        length_mm: 40.0,
        axis: Axis::X,
    }
}

fn pebble() -> RoughBase {
    RoughBase::Pebble {
        x_mm: 30.0,
        y_mm: 24.0,
        z_mm: 20.0,
    }
}

fn model_of(base: RoughBase) -> RoughModel {
    RoughModel::new(base, Vec::new())
}

fn centre_of(mesh: &RoughMesh) -> DVec3 {
    let (lo, hi) = mesh.bounds();
    (lo + hi) * 0.5
}

fn near(a: DVec3, b: DVec3) -> bool {
    (a - b).abs().max_element() < TOLERANCE
}

/// The cube of the fixture file scaled to `side` mm, at the origin of its own file.
fn cube_file(side: f64) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let (points, triangles) = parse_obj_mesh(CUBE_OBJ).expect("parses");
    (
        points.iter().map(|&p| p * (side / 20.0)).collect(),
        triangles,
    )
}

fn sphere(diameter: &str, placement: Placement, base: &RoughBase) -> Result<RoughMesh, String> {
    primitive_in_rough(InclusionKind::Sphere, [diameter, "", ""], placement, base)
}

#[test]
fn the_kind_box_maps_its_positions_and_falls_back_to_a_file() {
    assert_eq!(InclusionKind::from_index(0), InclusionKind::MeshFile);
    assert_eq!(InclusionKind::from_index(1), InclusionKind::Sphere);
    assert_eq!(InclusionKind::from_index(2), InclusionKind::Ellipsoid);
    assert_eq!(InclusionKind::from_index(3), InclusionKind::Box);
    assert_eq!(InclusionKind::from_index(-1), InclusionKind::MeshFile);
    assert_eq!(InclusionKind::from_index(4), InclusionKind::MeshFile);
    assert_eq!(InclusionKind::default(), InclusionKind::MeshFile);
}

#[test]
fn an_empty_position_is_the_rough_centre_and_a_comma_is_a_decimal_point() {
    assert_eq!(parse_placement("", " ", ""), Ok([None; 3]));
    assert_eq!(
        parse_placement("12,5", " 3 ", ""),
        Ok([Some(12.5), Some(3.0), None])
    );
    assert_eq!(
        parse_placement("-2", "0", "7.25"),
        Ok([Some(-2.0), Some(0.0), Some(7.25)])
    );
    for (x, y, z, axis) in [
        ("abc", "", "", "X"),
        ("", "NaN", "", "Y"),
        ("", "", "1e9", "Z"),
        ("inf", "", "", "X"),
    ] {
        let message = parse_placement(x, y, z).expect_err("not a position");
        assert!(message.contains(&format!("{axis} position")), "{message}");
        assert!(message.contains("empty"), "{message}");
    }
}

#[test]
fn the_rough_centre_is_the_middle_of_the_uncut_bounding_box() {
    let slab = block(30.0, 20.0, 10.0);
    assert!(near(rough_centre_mm(&slab), DVec3::new(15.0, 10.0, 5.0)));
    assert_eq!(centre_text(&slab), "15.0, 10.0, 5.0");
    assert!(near(
        rough_centre_mm(&cylinder()),
        DVec3::new(20.0, 6.0, 6.0)
    ));
    assert!(near(
        rough_centre_mm(&pebble()),
        DVec3::new(15.0, 12.0, 10.0)
    ));
    let typed = placed_centre(&slab, [Some(1.0), None, Some(2.0)]);
    assert!(near(typed, DVec3::new(1.0, 10.0, 2.0)));
}

#[test]
fn a_mesh_file_is_centred_in_every_kind_of_rough_by_default() {
    let (points, triangles) = cube_file(4.0);
    for base in [block(20.0, 20.0, 20.0), cylinder(), pebble()] {
        let mesh = inclusion_in_rough_frame(&base, &points, &triangles, [None; 3]).expect("a cube");
        assert!(near(centre_of(&mesh), rough_centre_mm(&base)), "{base:?}");
        let (lo, hi) = mesh.bounds();
        assert!(near(hi - lo, DVec3::splat(4.0)), "{base:?}");
    }
}

#[test]
fn a_mesh_file_is_placed_by_its_bounding_box_centre_whatever_its_own_coordinates() {
    let base = block(20.0, 20.0, 20.0);
    let (points, triangles) = cube_file(4.0);
    let far: Vec<DVec3> = points
        .iter()
        .map(|&p| p + DVec3::new(100.0, -50.0, 7.5))
        .collect();
    for file in [&points, &far] {
        let mesh = inclusion_in_rough_frame(&base, file, &triangles, [Some(8.0), None, Some(12.0)])
            .expect("a cube");
        assert!(near(centre_of(&mesh), DVec3::new(8.0, 10.0, 12.0)));
    }
}

#[test]
fn a_vertex_no_face_uses_does_not_move_the_centre() {
    let (mut points, triangles) = cube_file(4.0);
    let centre = used_bbox_centre(&points, &triangles).expect("faces");
    assert!(near(centre, DVec3::splat(2.0)));
    points.push(DVec3::splat(1.0e6));
    assert_eq!(used_bbox_centre(&points, &triangles), Some(centre));
    assert_eq!(used_bbox_centre(&points, &[]), None);
    let message = inclusion_in_rough_frame(&block(20.0, 20.0, 20.0), &points, &[], [None; 3])
        .expect_err("no faces");
    assert!(message.contains("no faces"), "{message}");
}

#[test]
fn a_block_a_cylinder_and_a_pebble_take_an_inclusion_and_give_it_back() {
    for base in [block(20.0, 20.0, 20.0), cylinder(), pebble()] {
        let model = model_of(base);
        let mesh = sphere("4", [None; 3], &base).expect("a sphere");
        let added = add_inclusion(&model, &mesh, DEFAULT_MARGIN_MM).expect("inside the rough");
        assert_eq!(added.base, base, "the base stays what it was");
        assert_eq!(added.inclusion_count(), 1);
        assert_eq!(added.inclusions.len(), 1);
        assert!(
            model.inclusions.is_empty(),
            "the model passed in is unchanged"
        );
        let infos = added.inclusion_infos();
        assert_eq!(infos.len(), 1);
        assert!(
            infos[0].volume_mm3 > 30.0,
            "{base:?}: {}",
            infos[0].volume_mm3
        );
        let gone = remove_inclusion(&added, 0).expect("it is there");
        assert_eq!(gone, model, "{base:?}");
        let message = remove_inclusion(&gone, 0).expect_err("nothing left");
        assert!(message.contains("no such inclusion"), "{message}");
    }
}

#[test]
fn a_second_inclusion_goes_in_after_the_first() {
    let base = block(30.0, 20.0, 20.0);
    let first = sphere("4", [Some(8.0), None, None], &base).expect("a sphere");
    let second = sphere("4", [Some(22.0), None, None], &base).expect("a sphere");
    let once = add_inclusion(&model_of(base), &first, 0.3).expect("fits");
    let twice = add_inclusion(&once, &second, 0.3).expect("fits");
    let infos = twice.inclusion_infos();
    assert_eq!(infos.len(), 2);
    assert!((infos[0].centre_mm.x - 8.0).abs() < 1e-6, "{infos:?}");
    assert!((infos[1].centre_mm.x - 22.0).abs() < 1e-6, "{infos:?}");
    let without_first = remove_inclusion(&twice, 0).expect("it is there");
    assert!((without_first.inclusion_infos()[0].centre_mm.x - 22.0).abs() < 1e-6);
}

#[test]
fn a_sphere_contains_what_was_typed_and_not_much_more() {
    let base = block(20.0, 20.0, 20.0);
    let mesh = sphere("4", [None; 3], &base).expect("a sphere");
    assert!(near(centre_of(&mesh), DVec3::splat(10.0)));
    let (lo, hi) = mesh.bounds();
    let extent = (hi - lo).min_element();
    assert!((4.0 - TOLERANCE..4.4).contains(&extent), "{extent}");
    let moved = sphere("4", [Some(5.0), Some(6.0), None], &base).expect("a sphere");
    assert!(near(centre_of(&moved), DVec3::new(5.0, 6.0, 10.0)));
}

#[test]
fn a_box_is_exact_and_an_ellipsoid_contains_its_axes() {
    let base = block(20.0, 20.0, 20.0);
    let boxed =
        primitive_in_rough(InclusionKind::Box, ["4", "6", "8"], [None; 3], &base).expect("a box");
    let (lo, hi) = boxed.bounds();
    assert!(near(lo, DVec3::new(8.0, 7.0, 6.0)), "{lo}");
    assert!(near(hi, DVec3::new(12.0, 13.0, 14.0)), "{hi}");
    let round = primitive_in_rough(
        InclusionKind::Ellipsoid,
        ["4", "6,5", "8"],
        [None; 3],
        &base,
    )
    .expect("an ellipsoid");
    let (lo, hi) = round.bounds();
    let size = hi - lo;
    for (got, typed) in [(size.x, 4.0), (size.y, 6.5), (size.z, 8.0)] {
        assert!(
            (typed - TOLERANCE..typed * 1.1).contains(&got),
            "{got} for {typed}"
        );
    }
    assert!(near(centre_of(&round), DVec3::splat(10.0)));
}

#[test]
fn a_size_must_be_typed_and_a_number_in_range() {
    let base = block(20.0, 20.0, 20.0);
    let empty = sphere("", [None; 3], &base).expect_err("no size");
    assert!(
        empty.contains("diameter") && empty.contains("Type"),
        "{empty}"
    );
    for bad in ["abc", "0", "-3", "inf", "NaN", "2001"] {
        let message = sphere(bad, [None; 3], &base).expect_err(bad);
        assert!(message.contains("diameter"), "{bad}: {message}");
        assert!(message.contains("above 0"), "{bad}: {message}");
    }
    // The largest size is a size (the rough decides whether it fits, when it is added).
    let max = format!("{MAX_INCLUSION_SIZE_MM}");
    let huge = sphere(&max, [None; 3], &base).expect("a size, though no rough holds it");
    let (lo, hi) = huge.bounds();
    assert!((hi - lo).min_element() >= MAX_INCLUSION_SIZE_MM - TOLERANCE);
    let message = add_inclusion(&model_of(base), &huge, 0.0).expect_err("it holds the rough");
    assert!(
        message.contains("not inside the rough's material"),
        "{message}"
    );
    let message = primitive_in_rough(InclusionKind::Box, ["4", "", "4"], [None; 3], &base)
        .expect_err("one edge is missing");
    assert!(message.contains("Y size"), "{message}");
    let message = primitive_in_rough(InclusionKind::MeshFile, ["4", "4", "4"], [None; 3], &base)
        .expect_err("a file is not typed");
    assert!(message.contains("Sphere"), "{message}");
}

#[test]
fn a_typed_inclusion_that_does_not_fit_is_refused_in_words_that_say_what_to_change() {
    let base = block(20.0, 20.0, 20.0);
    let model = model_of(base);
    // Outside the rough.
    let outside = sphere("4", [Some(100.0), None, None], &base).expect("a sphere");
    let message = add_inclusion(&model, &outside, 0.0).expect_err("outside");
    assert!(
        message.contains("not inside the rough's material"),
        "{message}"
    );
    assert!(message.contains("check the position"), "{message}");
    // Through the surface: no advice about a notch in a mesh.
    let through = sphere("24", [None; 3], &base).expect("a sphere");
    let message = add_inclusion(&model, &through, 0.0).expect_err("through the surface");
    assert!(message.contains("reaches the surface"), "{message}");
    assert!(!message.contains("notch"), "{message}");
    assert!(!message.contains("mesh"), "{message}");
    // Fits as typed, not with its margin.
    let snug = sphere("16", [None; 3], &base).expect("a sphere");
    add_inclusion(&model, &snug, 0.0).expect("fits without a margin");
    let message = add_inclusion(&model, &snug, 2.0).expect_err("too close with a margin");
    assert!(message.contains("closer to the surface"), "{message}");
    assert!(message.contains("Lower the margin"), "{message}");
    assert!(!message.contains("notch"), "{message}");
}

#[test]
fn two_inclusions_that_cross_are_refused() {
    let base = block(30.0, 20.0, 20.0);
    let model = model_of(base);
    let first = sphere("8", [Some(12.0), None, None], &base).expect("a sphere");
    let crossing = sphere("8", [Some(15.0), None, None], &base).expect("a sphere");
    let once = add_inclusion(&model, &first, 0.0).expect("fits");
    let message = add_inclusion(&once, &crossing, 0.0).expect_err("it crosses the first");
    assert!(message.contains("another inclusion"), "{message}");
    assert!(!message.contains("reaches the surface"), "{message}");
    assert!(!message.contains("notch"), "{message}");
    // One that wraps the first is refused too, when it is added.
    let wrapping = sphere("14", [Some(12.0), None, None], &base).expect("a sphere");
    let message = add_inclusion(&once, &wrapping, 0.0).expect_err("it wraps the first");
    assert!(message.contains("another inclusion"), "{message}");
}

#[test]
fn an_imported_mesh_keeps_its_own_words() {
    let (points, _triangles) = parse_obj_mesh(CUBE_OBJ).expect("parses");
    let base = pleochroma_cut_core::rough_plan::import_hull(&points).expect("a cube");
    let model = model_of(base);
    let (small, tris) = cube_file(4.0);
    let inside: Vec<DVec3> = small.iter().map(|&p| p + DVec3::splat(8.0)).collect();
    let mesh =
        inclusion_in_rough_frame(&base, &inside, &tris, [Some(1.0), None, None]).expect("a cube");
    // The typed position is not used for a mesh rough: the file's own coordinates are.
    assert!(near(centre_of(&mesh), DVec3::splat(10.0)));
    let added = add_inclusion(&model, &mesh, 0.0).expect("inside the rough");
    assert!(added.inclusions.is_empty());
    let (far, tris) = cube_file(4.0);
    let far: Vec<DVec3> = far.iter().map(|&p| p + DVec3::splat(100.0)).collect();
    let mesh = inclusion_in_rough_frame(&base, &far, &tris, [None; 3]).expect("a cube");
    let message = add_inclusion(&model, &mesh, 0.0).expect_err("outside the file's rough");
    assert!(message.contains("same coordinates and unit"), "{message}");
}
