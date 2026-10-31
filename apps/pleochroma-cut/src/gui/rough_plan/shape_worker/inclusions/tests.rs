use super::*;
use crate::gui::rough_plan::shape_worker::{ShapeRequest, centred_mesh, evaluate_reusing};
use pleochroma_cut_core::rough_plan::{
    PrimitivePose, PrimitiveShape, RoughBase, RoughCut, primitive_mesh,
};

fn block() -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: 10.0,
            y_mm: 8.0,
            z_mm: 6.0,
        },
        Vec::new(),
    )
}

/// `model` with a 2 mm box inclusion centred at `centre` (the rough's frame, mm).
fn with_box_at(model: &RoughModel, centre: [f64; 3]) -> RoughModel {
    let inclusion = primitive_mesh(
        PrimitiveShape::Box { size_mm: [2.0; 3] },
        PrimitivePose::at(centre),
    )
    .expect("a box");
    model
        .with_inclusions_added(&[inclusion], 0.0)
        .expect("the inclusion fits")
}

fn layer_of(model: &RoughModel) -> SolidMesh {
    let planes = model.halfspaces().expect("valid model");
    inclusion_layer(model, &planes)
}

/// The lowest and highest corner of every ring of `mesh`.
fn bounds(mesh: &SolidMesh) -> (DVec3, DVec3) {
    let mut low = DVec3::splat(f64::INFINITY);
    let mut high = DVec3::splat(f64::NEG_INFINITY);
    for (_, ring) in &mesh.rings {
        for &p in ring {
            low = low.min(p);
            high = high.max(p);
        }
    }
    (low, high)
}

#[test]
fn a_rough_without_inclusions_has_no_layer() {
    assert!(layer_of(&block()).rings.is_empty());
}

#[test]
fn the_layer_is_exactly_the_shells_of_the_inclusions_in_the_centred_frame() {
    let model = with_box_at(&block(), [5.0, 4.0, 3.0]);
    let layer = layer_of(&model);
    let rough = model.mesh().expect("the derived mesh");
    assert_eq!(rough.inclusion_count(), 1);
    // One ring per triangle of the shell, each a triangle.
    assert_eq!(layer.rings.len(), rough.inclusions()[0].triangles().len());
    assert!(layer.rings.iter().all(|(_, ring)| ring.len() == 3));
    assert_eq!(
        layer.piece_normals.as_ref().map(Vec::len),
        Some(layer.rings.len())
    );
    let flags = layer.edge_visible.as_ref().expect("edge flags");
    assert_eq!(flags.len(), layer.rings.len());
    // The 2 mm box sits on the block's centre, which is the origin of the centred frame.
    let (low, high) = bounds(&layer);
    assert!((low + DVec3::ONE).length() < 1e-9, "{low:?}");
    assert!((high - DVec3::ONE).length() < 1e-9, "{high:?}");
    // The six flat faces are two triangles each: their seams are not drawn (12 flags, two
    // per seam), the twelve box edges are.
    let hidden = flags.iter().flatten().filter(|&&drawn| !drawn).count();
    assert_eq!(hidden, 12);
}

#[test]
fn the_rough_itself_does_not_draw_the_shells_and_keeps_its_plane_ids() {
    let plain = centred_mesh(&block()).expect("valid");
    let with = centred_mesh(&with_box_at(&block(), [5.0, 4.0, 3.0])).expect("valid");
    assert_eq!(with.facet_id, plain.facet_id);
    assert_eq!(with.rings.len(), plain.rings.len());
    assert!(with.rings.len() == 6 && with.facet_id.iter().all(|&id| id < 6));
}

#[test]
fn the_worker_hands_the_layer_on_beside_the_mesh() {
    let request = ShapeRequest {
        revision: 1,
        model: with_box_at(&block(), [5.0, 4.0, 3.0]),
        specific_gravity: 2.65,
        material_name: String::new(),
        weighed_ct: None,
        selected_cut: None,
    };
    let output = evaluate_reusing(&request, None)
        .0
        .outcome
        .expect("a valid model");
    assert_eq!(output.inclusions.rings.len(), 12);
    assert_eq!(
        output.mesh.rings.len(),
        6,
        "the shells are not in the rough"
    );
    let plain = ShapeRequest {
        model: block(),
        ..request
    };
    let output = evaluate_reusing(&plain, None).0.outcome.expect("valid");
    assert!(output.inclusions.rings.is_empty());
}

#[test]
fn a_cut_through_a_shell_clips_it_and_closes_it_with_the_cut_face() {
    // The box inclusion spans z 2..4 mm; the face cut keeps z <= 3 mm.
    let mut model = with_box_at(&block(), [5.0, 4.0, 3.0]);
    model.cuts.push(RoughCut::Face {
        normal: [0.0, 0.0, 1.0],
        depth_mm: 3.0,
    });
    let layer = layer_of(&model);
    let (low, high) = bounds(&layer);
    assert!((low.z + 1.0).abs() < 1e-9, "{low:?}");
    assert!(high.z.abs() < 1e-9, "the shell ends at the cut: {high:?}");
    let normals = layer.piece_normals.as_ref().expect("normals");
    assert!(
        normals.iter().any(|n| (*n - DVec3::Z).length() < 1e-9),
        "the cut closes the shell with a +z face"
    );
}

#[test]
fn a_cut_that_takes_a_shell_away_leaves_no_layer() {
    // The cut keeps z <= 1 mm; the shell spans z 2..4 mm.
    let mut model = with_box_at(&block(), [5.0, 4.0, 3.0]);
    model.cuts.push(RoughCut::Face {
        normal: [0.0, 0.0, 1.0],
        depth_mm: 5.0,
    });
    assert!(layer_of(&model).rings.is_empty());
}

#[test]
fn an_imported_mesh_rough_gets_the_same_layer() {
    use crate::gui::rough_plan::obj_import::{C_SHAPE_OBJ, mesh_base_of};
    let plain = RoughModel::new(mesh_base_of(C_SHAPE_OBJ), Vec::new());
    assert!(layer_of(&plain).rings.is_empty());
    // x 3..5 mm is well inside the cube, away from the notch in its +x face.
    let model = with_box_at(&plain, [4.0, 10.0, 10.0]);
    let layer = layer_of(&model);
    assert_eq!(layer.rings.len(), 12);
    let (low, high) = bounds(&layer);
    assert!(
        (high - low - DVec3::splat(2.0)).length() < 1e-9,
        "{low:?} {high:?}"
    );
}
