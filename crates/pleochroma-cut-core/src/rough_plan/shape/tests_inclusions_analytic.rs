//! Inclusions of a block, cylinder or pebble, kept on the model: adding, removing, weighing,
//! scaling, editing the base under them, the mesh they make and the plans around them. The
//! mesh-level behaviour is in `mesh/tests.rs`; inclusions of an imported mesh are in
//! `tests_inclusion.rs`.

use glam::DVec3;

use super::{
    BoxFace, BoxState, HullError, InclusionFault, InclusionSet, MeshError, PrimitivePose,
    PrimitiveShape, RoughBase, RoughCut, RoughMesh, RoughModel, ShapeError,
    hull::{add_inclusion_mesh, import_hull, inclusion_list, remove_inclusion},
    inclusions::cached_meshes,
    mesh_fixture::{CUBE_OBJ, box_hull, parse_obj, stone_enters_box},
    primitive_mesh,
};
use crate::rough_plan::{
    Axis, CandidateDesign, DesignHull, PlanInput, PlanPath, RoughLayout,
    orientation::fit_to_crystal_length_of,
    plan,
    tests::{box_design, settings_with},
};

/// Where the inclusion of [`with_box`] is.
const INCLUSION_BOX: ([f64; 3], [f64; 3]) = ([6.0; 3], [14.0; 3]);

/// The three kinds of rough that keep their inclusions on the model, each in a 20 mm cube.
fn kinds() -> [(&'static str, RoughBase); 3] {
    [
        (
            "block",
            RoughBase::Block {
                x_mm: 20.0,
                y_mm: 20.0,
                z_mm: 20.0,
            },
        ),
        (
            "cylinder",
            RoughBase::Cylinder {
                diameter_mm: 20.0,
                length_mm: 20.0,
                axis: Axis::Z,
            },
        ),
        (
            "pebble",
            RoughBase::Pebble {
                x_mm: 20.0,
                y_mm: 20.0,
                z_mm: 20.0,
            },
        ),
    ]
}

/// The box `size_mm` centred at `centre_mm` as a closed shell.
fn shell(centre_mm: [f64; 3], size_mm: f64) -> RoughMesh {
    primitive_mesh(
        PrimitiveShape::Box {
            size_mm: [size_mm; 3],
        },
        PrimitivePose::at(centre_mm),
    )
    .expect("a box")
}

/// `base` with an 8 mm cube of inclusion in the middle (no margin) and the given `cuts`.
fn with_box_cut(base: RoughBase, cuts: Vec<RoughCut>) -> RoughModel {
    RoughModel::new(base, cuts)
        .with_inclusions_added(&[shell([10.0; 3], 8.0)], 0.0)
        .expect("the inclusion fits")
}

/// `base` with an 8 mm cube of inclusion in the middle (no margin).
fn with_box(base: RoughBase) -> RoughModel {
    with_box_cut(base, Vec::new())
}

fn plain(base: RoughBase) -> RoughModel {
    RoughModel::new(base, Vec::new())
}

fn relative(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs()
}

fn designs() -> Vec<CandidateDesign> {
    vec![
        box_design(1),
        CandidateDesign {
            entry_id: 2,
            width: 1.0,
            length: 1.6,
            height: 0.6,
            volume: 0.5,
            orientation: None,
        },
    ]
}

fn hulls() -> Vec<DesignHull> {
    vec![box_hull(1, 1.0, 1.0, 1.0), box_hull(2, 1.0, 0.6, 1.6)]
}

fn run(model: &RoughModel) -> Vec<RoughLayout> {
    let settings = settings_with(3, 0.3, 0.2, 1.0);
    let designs = designs();
    let hulls = hulls();
    let input = PlanInput {
        model,
        settings: &settings,
        designs: &designs,
        hulls: &hulls,
    };
    plan(&input, &mut |_| true).expect("not cancelled")
}

fn stones_in_inclusion(layouts: &[RoughLayout]) -> usize {
    let hulls = hulls();
    layouts
        .iter()
        .flat_map(|layout| layout.stones.iter().map(move |stone| (layout, stone)))
        .filter(|&(layout, stone)| stone_enters_box(layout, stone, &hulls, INCLUSION_BOX))
        .count()
}

#[test]
fn a_rough_with_an_inclusion_weighs_what_it_weighed_before_and_counts_the_inclusion() {
    for (name, base) in kinds() {
        let before = plain(base);
        let after = with_box(base);
        let (p, w) = (
            before.measure().expect("measures"),
            after.measure().expect("measures"),
        );
        // The outer mesh has the corners of the plane polytope, so the gross volume is the
        // polytope's (to rounding, not to the bit).
        assert!(
            relative(w.volume_mm3, p.volume_mm3) < 1e-9,
            "{name}: {} against {}",
            w.volume_mm3,
            p.volume_mm3
        );
        assert_eq!(w.inclusion_count, 1, "{name}");
        assert!((w.inclusion_mm3 - 512.0).abs() < 1e-6, "{name}: {w:?}");
        assert_eq!(p.inclusion_count, 0, "{name}");
        assert_eq!(p.inclusion_mm3.to_bits(), 0.0_f64.to_bits(), "{name}");
        // The planes and the bounding box do not move.
        assert_eq!(w.extents_mm, p.extents_mm, "{name}");
        assert_eq!(w.plane_count, p.plane_count, "{name}");
        assert_eq!(w.vertex_count, p.vertex_count, "{name}");
        assert_eq!(
            after.solid().expect("solid").planes,
            before.solid().expect("solid").planes,
            "{name}"
        );
        // What stones can use is the rest.
        let mesh = after.mesh().expect("an inclusion makes a mesh rough");
        assert!(
            (mesh.volume() - (w.volume_mm3 - 512.0)).abs() < 1e-6,
            "{name}: {}",
            mesh.volume()
        );
        assert_eq!(
            mesh.box_state([7.0; 3], [13.0; 3], 0.0),
            BoxState::Air,
            "{name}"
        );
        assert_eq!(after.inclusion_count(), 1, "{name}");
        let list = after.inclusion_infos();
        assert_eq!(list.len(), 1, "{name}");
        assert!((list[0].volume_mm3 - 512.0).abs() < 1e-6, "{name}");
        assert!(
            (list[0].centre_mm - DVec3::splat(10.0)).length() < 1e-9,
            "{name}"
        );
        assert!(
            (list[0].extents_mm - DVec3::splat(8.0)).length() < 1e-9,
            "{name}"
        );
    }
}

#[test]
fn a_rough_without_inclusions_is_what_it_always_was() {
    for (name, base) in kinds() {
        let original = plain(base);
        assert!(original.mesh().is_none(), "{name}");
        assert_eq!(original.mesh_checked(), Ok(None), "{name}");
        assert_eq!(original.inclusion_count(), 0, "{name}");
        // Adding the only inclusion and removing it gives the very same model.
        let back = with_box(base).with_inclusion_removed(0).expect("removes");
        assert_eq!(back, original, "{name}");
        assert!(back.mesh().is_none(), "{name}");
        assert!(back.inclusions.is_empty(), "{name}");
        // An empty set measures to the bit as a model that never had one.
        let measured = original.measure().expect("measures");
        let again = back.measure().expect("measures");
        assert_eq!(
            measured.volume_mm3.to_bits(),
            again.volume_mm3.to_bits(),
            "{name}"
        );
        assert_eq!(measured.extents_mm, again.extents_mm, "{name}");
    }
}

#[test]
fn an_uncut_block_with_an_inclusion_plans_on_the_shaped_path() {
    let settings = settings_with(3, 0.3, 0.2, 1.0);
    let designs = designs();
    let path_of = |model: &RoughModel| {
        PlanInput {
            model,
            settings: &settings,
            designs: &designs,
            hulls: &[],
        }
        .path()
    };
    let (_, block) = kinds()[0];
    assert_eq!(path_of(&plain(block)), PlanPath::PlainBlock);
    assert_eq!(path_of(&with_box(block)), PlanPath::Shaped);
    let back = with_box(block).with_inclusion_removed(0).expect("removes");
    assert_eq!(path_of(&back), PlanPath::PlainBlock);
    // The other bases were shaped already.
    for (_, base) in &kinds()[1..] {
        assert_eq!(path_of(&plain(*base)), PlanPath::Shaped);
        assert_eq!(path_of(&with_box(*base)), PlanPath::Shaped);
    }
}

#[test]
fn no_stone_enters_the_inclusion_of_any_kind() {
    for (name, base) in kinds() {
        // The control: the plain rough is filled right through the middle.
        assert!(
            stones_in_inclusion(&run(&plain(base))) > 0,
            "{name}: the plain rough's plan never uses the middle"
        );
        let model = with_box(base);
        let layouts = run(&model);
        assert!(
            layouts.iter().any(|layout| layout.stone_count() > 0),
            "{name}: the rough holds stones around the inclusion"
        );
        assert_eq!(
            stones_in_inclusion(&layouts),
            0,
            "{name}: a stone reaches into the inclusion"
        );
        // And the yield is taken against the gross volume, as for a mesh rough.
        let gross = model.measure().expect("measures").volume_mm3;
        for layout in &layouts {
            let expected = layout.total_volume_mm3 / gross;
            assert!(
                (layout.yield_fraction - expected).abs() < 1e-9,
                "{name}: yield {} against {expected}",
                layout.yield_fraction
            );
        }
    }
}

#[test]
fn every_stone_of_a_cylinder_or_pebble_with_an_inclusion_stays_in_the_material() {
    // The outer mesh coincides with the planes of the polytope, so a stone rests on both: the
    // plan must still place stones, and each one must be clear of the mesh.
    for (name, base) in &kinds()[1..] {
        let model = with_box(*base);
        let mesh = model.mesh().expect("mesh");
        let layouts = run(&model);
        assert!(
            layouts.iter().any(|layout| layout.stone_count() > 0),
            "{name}: no layout holds a stone"
        );
        for layout in layouts.iter().filter(|layout| !layout.exact_fit) {
            // A box-model stone is its box; an exact fit's box is its bounding box, which may
            // reach out of a curved rough, so those are checked against the inclusion above.
            for stone in &layout.stones {
                let half = DVec3::from(stone.stone_size_mm) * 0.5;
                let centre = DVec3::from(stone.pose.center_mm);
                let state =
                    mesh.box_state((centre - half).to_array(), (centre + half).to_array(), 0.0);
                assert_eq!(
                    state,
                    BoxState::Clear,
                    "{name}: a stone leaves the material"
                );
            }
        }
    }
}

#[test]
fn a_plan_around_an_inclusion_is_the_same_twice() {
    for (name, base) in kinds() {
        let model = with_box(base);
        assert_eq!(run(&model), run(&model), "{name}");
        // A model built from the same shells by another route plans the same.
        let rebuilt = plain(base).with_inclusion_set(
            InclusionSet::from_parts(&model.inclusions.to_parts()).expect("reads back"),
        );
        assert_eq!(rebuilt, model, "{name}");
        assert_eq!(run(&rebuilt), run(&model), "{name}");
    }
}

#[test]
fn edge_and_corner_cuts_still_work_with_inclusions() {
    let (_, block) = kinds()[0];
    let cuts = vec![
        RoughCut::Edge {
            faces: [BoxFace::Top, BoxFace::Right],
            setbacks_mm: [3.0, 3.0],
        },
        RoughCut::Corner {
            faces: [BoxFace::Bottom, BoxFace::Left, BoxFace::Back],
            setbacks_mm: [2.0, 2.0, 2.0],
        },
    ];
    let before = RoughModel::new(block, cuts.clone());
    let after = with_box_cut(block, cuts);
    // Neither cut is touched: the same planes, in the same order, bit for bit.
    assert_eq!(after.halfspaces(), before.halfspaces());
    let (p, w) = (
        before.measure().expect("measures"),
        after.measure().expect("measures"),
    );
    let expected = 8000.0 - 90.0 - 8.0 / 6.0;
    assert!((p.volume_mm3 - expected).abs() < 1e-6, "{p:?}");
    assert!(relative(w.volume_mm3, p.volume_mm3) < 1e-9, "{w:?}");
    assert!((w.inclusion_mm3 - 512.0).abs() < 1e-6, "{w:?}");
    let layouts = run(&after);
    assert!(layouts.iter().any(|layout| layout.stone_count() > 0));
    assert_eq!(stones_in_inclusion(&layouts), 0);
    // The cuts stay Block only, with or without inclusions.
    let (_, cylinder) = kinds()[1];
    let refused = RoughModel::new(
        cylinder,
        vec![RoughCut::Edge {
            faces: [BoxFace::Top, BoxFace::Right],
            setbacks_mm: [1.0, 1.0],
        }],
    )
    .with_inclusions_added(&[shell([10.0; 3], 4.0)], 0.0)
    .expect("an inclusion does not look at the cuts");
    assert_eq!(
        refused.measure(),
        Err(ShapeError::CutNotForBase { index: 0 })
    );
}

#[test]
fn a_face_cut_through_an_inclusion_counts_the_part_inside_the_cut() {
    // x <= 15 slices the inclusion [12.5, 17.5] in x: 2.5 of its 5 mm remain,
    // 2.5 x 5 x 5 = 62.5 mm^3. A face cut's depth is measured from the polytope's outermost
    // point along the normal, and only the block reaches x = 20: the cylinder's 64-sided prism
    // has its rim points at 10 + 10 cos(pi / 64) = 19.988 and the pebble's vertices are on its
    // ellipsoid, so each kind gets the depth that puts the plane at x = 15.
    for (name, base) in kinds() {
        let top = base
            .vertices(false)
            .expect("the base has vertices")
            .iter()
            .map(|vertex| vertex.x)
            .fold(f64::NEG_INFINITY, f64::max);
        let cut = RoughCut::Face {
            normal: [1.0, 0.0, 0.0],
            depth_mm: top - 15.0,
        };
        let before = RoughModel::new(base, vec![cut]);
        let after = before
            .with_inclusions_added(&[shell([15.0, 10.0, 10.0], 5.0)], 0.0)
            .expect("a cut that slices a shell is legal");
        let (p, w) = (
            before.measure().expect("measures"),
            after.measure().expect("measures"),
        );
        assert!(
            relative(w.volume_mm3, p.volume_mm3) < 1e-9,
            "{name}: {w:?} {p:?}"
        );
        assert!((w.inclusion_mm3 - 62.5).abs() < 1e-6, "{name}: {w:?}");
        assert_eq!(w.inclusion_count, 1, "{name}");
    }
}

#[test]
fn a_size_edit_that_pushes_a_shell_out_of_the_material_is_an_error() {
    for (name, base) in kinds() {
        let model = with_box(base);
        let smaller = match base {
            RoughBase::Block { y_mm, z_mm, .. } => RoughBase::Block {
                x_mm: 4.0,
                y_mm,
                z_mm,
            },
            RoughBase::Cylinder {
                length_mm, axis, ..
            } => RoughBase::Cylinder {
                diameter_mm: 6.0,
                length_mm,
                axis,
            },
            RoughBase::Pebble { y_mm, z_mm, .. } => RoughBase::Pebble {
                x_mm: 6.0,
                y_mm,
                z_mm,
            },
            RoughBase::Hull { .. } => unreachable!("not a kind under test"),
        };
        let mut edited = model.clone();
        edited.base = smaller;
        let error = edited.solid().expect_err("the shell no longer fits");
        assert!(
            matches!(error, ShapeError::Inclusion { index: 0, .. }),
            "{name}: {error:?}"
        );
        assert_eq!(edited.measure(), Err(error.clone()), "{name}");
        assert_eq!(
            edited.mesh_checked().map(|_| ()),
            Err(error.clone()),
            "{name}"
        );
        assert!(
            edited.mesh().is_none(),
            "{name}: a reader gets no mesh, not a panic"
        );
        assert!(error.to_string().contains("Inclusion 1"), "{name}: {error}");
        // The planner's context refuses it too.
        let settings = settings_with(3, 0.3, 0.2, 1.0);
        assert!(crate::rough_plan::shaped::ShapedCtx::new(&edited, &settings).is_err());
        // Nothing was dropped: the edit back makes the rough what it was.
        edited.base = base;
        assert_eq!(edited, model, "{name}");
        assert!(edited.measure().is_ok(), "{name}");
    }
}

#[test]
fn a_shell_through_the_new_surface_and_one_outside_it_are_told_apart() {
    let (_, block) = kinds()[0];
    let model = with_box(block);
    let mut through = model.clone();
    through.base = RoughBase::Block {
        x_mm: 12.0,
        y_mm: 20.0,
        z_mm: 20.0,
    };
    // The wall at x = 12 cuts the shell [6, 14].
    assert_eq!(
        through.solid().map(|_| ()),
        Err(ShapeError::Inclusion {
            index: 0,
            reason: InclusionFault::ReachesSurface,
        })
    );
    let mut outside = model;
    outside.base = RoughBase::Block {
        x_mm: 4.0,
        y_mm: 20.0,
        z_mm: 20.0,
    };
    assert_eq!(
        outside.solid().map(|_| ()),
        Err(ShapeError::Inclusion {
            index: 0,
            reason: InclusionFault::Outside,
        })
    );
}

#[test]
fn the_mesh_follows_the_base_of_a_model_that_shares_its_set() {
    // Undo entries and plan jobs clone the model, and the clones share the set: a mesh kept in
    // the set would be the old rough's.
    let (_, block) = kinds()[0];
    let model = with_box(block);
    let first = model.mesh().expect("mesh");
    assert!((first.bounds().1.x - 20.0).abs() < 1e-9);

    let mut bigger = model.clone();
    bigger.base = RoughBase::Block {
        x_mm: 30.0,
        y_mm: 20.0,
        z_mm: 20.0,
    };
    assert_eq!(bigger.inclusions, model.inclusions);
    let grown = bigger.mesh().expect("mesh");
    assert!((grown.bounds().1.x - 30.0).abs() < 1e-9);
    assert!((grown.gross_volume() - 12_000.0).abs() < 1e-6);
    let measure = bigger.measure().expect("measures");
    assert!((measure.volume_mm3 - 12_000.0).abs() < 1e-6);

    let mut smaller = model.clone();
    smaller.base = RoughBase::Block {
        x_mm: 16.0,
        y_mm: 20.0,
        z_mm: 20.0,
    };
    let shrunk = smaller.mesh().expect("mesh");
    assert!((shrunk.bounds().1.x - 16.0).abs() < 1e-9);
    // Asking again, in any order, gives each its own.
    assert!((model.mesh().expect("mesh").bounds().1.x - 20.0).abs() < 1e-9);
    assert!((bigger.mesh().expect("mesh").bounds().1.x - 30.0).abs() < 1e-9);
    // The cuts are not part of what the mesh is.
    let mut cut = model.clone();
    cut.cuts.push(RoughCut::Face {
        normal: [1.0, 0.0, 0.0],
        depth_mm: 2.0,
    });
    assert_eq!(model.mesh(), cut.mesh());
}

#[test]
fn the_registry_of_derived_meshes_stays_small() {
    let (_, block) = kinds()[0];
    let model = with_box(block);
    for k in 0..40_u32 {
        let mut other = model.clone();
        other.base = RoughBase::Block {
            x_mm: f64::from(k).mul_add(0.25, 20.0),
            y_mm: 20.0,
            z_mm: 20.0,
        };
        assert!(other.mesh().is_some());
    }
    assert!(cached_meshes() <= 16, "{}", cached_meshes());
    // Whatever fell out is built again, the same.
    assert!(model.mesh().is_some());
}

#[test]
fn inclusions_scale_with_the_rough() {
    for (name, base) in kinds() {
        let model = with_box(base);
        let scaled = model.scaled(1.5);
        let (p, w) = (
            plain(base).scaled(1.5).measure().expect("measures"),
            scaled.measure().expect("measures"),
        );
        assert!(relative(w.volume_mm3, p.volume_mm3) < 1e-9, "{name}: {w:?}");
        assert!(
            relative(w.inclusion_mm3, 512.0 * 3.375) < 1e-9,
            "{name}: {w:?}"
        );
        assert_eq!(w.inclusion_count, 1, "{name}");
        // The shell still fits and sits where it did, scaled about the corner.
        let list = scaled.inclusion_infos();
        assert!(
            (list[0].centre_mm - DVec3::splat(15.0)).length() < 1e-9,
            "{name}"
        );
        // Scaling by one changes nothing, the set included.
        assert_eq!(model.scaled(1.0), model, "{name}");
        // A factor that cannot scale a mesh leaves the set as it was.
        assert_eq!(
            model.scaled(f64::NAN).inclusions,
            model.inclusions,
            "{name}"
        );
    }
}

#[test]
fn a_saved_set_reads_back_as_the_same_set() {
    for (name, base) in kinds() {
        let sphere = primitive_mesh(
            PrimitiveShape::Sphere { radius_mm: 1.5 },
            PrimitivePose::at([16.5, 10.0, 10.0]),
        )
        .expect("a sphere");
        let model = with_box(base)
            .with_inclusions_added(&[sphere], 0.3)
            .expect("a second one fits");
        assert_eq!(model.inclusion_count(), 2, "{name}");
        let parts = model.inclusions.to_parts();
        assert_eq!(parts.len(), 2, "{name}");
        let back = InclusionSet::from_parts(&parts).expect("reads back");
        assert_eq!(back, model.inclusions, "{name}");
        assert_eq!(back.id(), model.inclusions.id(), "{name}");
        assert_ne!(back.id(), 0, "{name}");
        let reread = plain(base).with_inclusion_set(back);
        assert_eq!(reread, model, "{name}");
        let (a, b) = (
            model.measure().expect("measures"),
            reread.measure().expect("measures"),
        );
        assert_eq!(a.volume_mm3.to_bits(), b.volume_mm3.to_bits(), "{name}");
        assert_eq!(
            a.inclusion_mm3.to_bits(),
            b.inclusion_mm3.to_bits(),
            "{name}"
        );
        // A different set is a different set.
        let one = model.with_inclusion_removed(1).expect("removes");
        assert_ne!(one.inclusions, model.inclusions, "{name}");
        assert_eq!(InclusionSet::EMPTY.id(), 0);
        assert!(InclusionSet::from_parts(&[]).expect("empty").is_empty());
        // A shell that is not a closed mesh is refused.
        let (points, tris) = &parts[0];
        let open = (points.clone(), tris[..tris.len() - 1].to_vec());
        assert!(matches!(
            InclusionSet::from_parts(&[open]),
            Err(HullError::Inclusion(_))
        ));
    }
}

#[test]
fn inclusions_are_added_in_order_and_removed_by_number() {
    let (_, block) = kinds()[0];
    let one = with_box(block);
    let two = one
        .with_inclusions_added(&[shell([3.5, 10.0, 10.0], 2.0)], 0.0)
        .expect("a second");
    let list = two.inclusion_infos();
    assert_eq!(list.len(), 2);
    assert!((list[0].volume_mm3 - 512.0).abs() < 1e-6);
    assert!((list[1].centre_mm - DVec3::new(3.5, 10.0, 10.0)).length() < 1e-9);
    // The model that was added to is unchanged.
    assert_eq!(one.inclusion_count(), 1);

    let rest = two.with_inclusion_removed(0).expect("removes");
    let list = rest.inclusion_infos();
    assert_eq!(list.len(), 1);
    assert!((list[0].volume_mm3 - 8.0).abs() < 1e-6);
    assert_eq!(two.with_inclusion_removed(2), Err(HullError::NoInclusion));
    assert_eq!(
        plain(block).with_inclusion_removed(0),
        Err(HullError::NoInclusion)
    );
    // Adding nothing changes nothing.
    assert_eq!(one.with_inclusions_added(&[], 0.5), Ok(one));
}

#[test]
fn an_inclusion_that_does_not_fit_is_refused_as_for_a_mesh_rough() {
    let (_, block) = kinds()[0];
    let model = plain(block);
    // Through the surface.
    assert_eq!(
        model.with_inclusions_added(&[shell([18.0, 10.0, 10.0], 8.0)], 0.0),
        Err(HullError::Inclusion(MeshError::InclusionReachesSurface(0)))
    );
    // Wholly outside.
    assert_eq!(
        model.with_inclusions_added(&[shell([40.0, 10.0, 10.0], 8.0)], 0.0),
        Err(HullError::Inclusion(MeshError::InclusionOutside(0)))
    );
    // 0.1 mm from the surface: fine as it is, too close for 0.3 mm of margin.
    let near = shell([4.1, 10.0, 10.0], 8.0);
    assert!(
        model
            .with_inclusions_added(std::slice::from_ref(&near), 0.0)
            .is_ok()
    );
    assert_eq!(
        model.with_inclusions_added(std::slice::from_ref(&near), 0.3),
        Err(HullError::InclusionTooClose)
    );
    // The margin pads it, and the weight is still the whole.
    let padded = model
        .with_inclusions_added(&[shell([10.0; 3], 4.0)], 0.3)
        .expect("fits with its margin");
    let measure = padded.measure().expect("measures");
    assert!((measure.volume_mm3 - 8000.0).abs() < 1e-6);
    assert!((measure.inclusion_mm3 - 4.6_f64.powi(3)).abs() < 1e-5);
    // One that crosses an inclusion the rough already has.
    let one = with_box(block);
    assert_eq!(
        one.with_inclusions_added(&[shell([12.0, 10.0, 10.0], 8.0)], 0.0),
        Err(HullError::Inclusion(MeshError::InclusionsOverlap))
    );
    // One that wraps an inclusion the rough already has is refused when it is added, not later
    // when the model is built (the 8 mm cube of `with_box` is wholly inside a 12 mm one).
    assert_eq!(
        one.with_inclusions_added(&[shell([10.0; 3], 12.0)], 0.0),
        Err(HullError::Inclusion(MeshError::InclusionsOverlap))
    );
    // Whatever Add accepts, the model builds.
    assert!(one.solid().is_ok());
    // Too many.
    let many = vec![shell([10.0; 3], 1.0); 65];
    assert_eq!(
        model.with_inclusions_added(&many, 0.0),
        Err(HullError::TooManyInclusions)
    );
    // A model whose inclusions no longer fit takes no more, and says why.
    let mut edited = one;
    edited.base = RoughBase::Block {
        x_mm: 4.0,
        y_mm: 20.0,
        z_mm: 20.0,
    };
    assert!(matches!(
        edited.with_inclusions_added(&[shell([2.0, 10.0, 10.0], 1.0)], 0.0),
        Err(HullError::Inclusion(_))
    ));
}

#[test]
fn an_imported_mesh_keeps_its_inclusions_in_its_own_mesh() {
    let (points, _) = parse_obj(CUBE_OBJ);
    let base = import_hull(&points).expect("a cube");
    let model = RoughModel::new(base, Vec::new());
    let body = shell([10.0; 3], 4.0);
    let with = model
        .with_inclusions_added(std::slice::from_ref(&body), 0.0)
        .expect("fits");
    // The same base the hull route gives, and the set stays empty.
    assert_eq!(Ok(with.base), add_inclusion_mesh(&base, &body, 0.0));
    assert!(with.inclusions.is_empty());
    assert_eq!(with.inclusion_count(), 1);
    let RoughBase::Hull { id, .. } = with.base else {
        panic!("not a hull");
    };
    assert_eq!(with.inclusion_infos(), inclusion_list(id));
    assert!(with.mesh().is_some());
    assert_eq!(with.with_inclusion_removed(0).map(|m| m.base), Ok(base));
    assert_eq!(remove_inclusion(&with.base, 0), Ok(base));
    // A set beside a hull is not a thing: it is an error, not a silent drop.
    let both = with.with_inclusion_set(InclusionSet::from_meshes(vec![body]).expect("a set"));
    assert_eq!(
        both.mesh_checked().map(|_| ()),
        Err(ShapeError::Inclusion {
            index: 0,
            reason: InclusionFault::NotForBase,
        })
    );
    assert!(both.solid().is_err());
}

#[test]
fn the_crystal_length_fit_ignores_inclusions() {
    let base = RoughBase::Block {
        x_mm: 24.0,
        y_mm: 12.0,
        z_mm: 8.0,
    };
    let before = plain(base);
    let after = before
        .with_inclusions_added(&[shell([12.0, 6.0, 4.0], 2.0)], 0.0)
        .expect("fits");
    let fit = fit_to_crystal_length_of(&before, None);
    assert!(fit.is_some());
    assert_eq!(fit_to_crystal_length_of(&after, None), fit);
}
