//! Lock on a block that has inclusions of its own: the locked pieces join them.

use super::{PrimitivePose, PrimitiveShape, RoughBase, RoughCut, RoughModel, primitive_mesh};
use crate::rough_plan::{
    Axis, PlacedStone, PlanInput, PlanPath, RoughLayout,
    fit::StonePose,
    orientation::{LockError, rough_with_locked_pieces},
    plan,
    tests::{box_design, settings_with},
};

fn block_model(cuts: Vec<RoughCut>) -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: 12.0,
            y_mm: 12.0,
            z_mm: 12.0,
        },
        cuts,
    )
}

/// The piece of a locked stone: the cube of edge `size` centred at `centre`.
fn stone_at(centre: [f64; 3], size: f64) -> PlacedStone {
    PlacedStone {
        entry_id: 1,
        piece_origin_mm: centre.map(|c| size.mul_add(-0.5, c)),
        piece_size_mm: [size; 3],
        stone_size_mm: [size; 3],
        table_axis: Axis::Y,
        carat: 1.0,
        volume_mm3: size * size * size,
        pose: StonePose {
            center_mm: centre,
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            mm_per_unit: size,
        },
    }
}

/// `model` with a cube of inclusion of edge `size` centred at `centre`.
fn with_inclusion(model: &RoughModel, centre: [f64; 3], size: f64) -> RoughModel {
    let shell = primitive_mesh(
        PrimitiveShape::Box { size_mm: [size; 3] },
        PrimitivePose::at(centre),
    )
    .expect("a box");
    model
        .with_inclusions_added(&[shell], 0.0)
        .expect("the inclusion fits")
}

/// Whether the stones' bounding boxes are apart along some axis (up to `slack` mm).
fn apart(a: &PlacedStone, b: &PlacedStone, slack: f64) -> bool {
    let span = |stone: &PlacedStone, axis: usize| {
        let half = stone.stone_size_mm[axis] * 0.5;
        (
            stone.pose.center_mm[axis] - half,
            stone.pose.center_mm[axis] + half,
        )
    };
    (0..3).any(|axis| {
        let ((a_lo, a_hi), (b_lo, b_hi)) = (span(a, axis), span(b, axis));
        a_lo >= b_hi - slack || b_lo >= a_hi - slack
    })
}

#[test]
fn locking_a_stone_on_a_block_with_an_inclusion_keeps_both_out() {
    let settings = settings_with(1, 0.0, 0.0, 1.0);
    let model = with_inclusion(&block_model(Vec::new()), [8.0; 3], 3.0);
    let locked_stone = stone_at([1.0; 3], 2.0);

    let locked = rough_with_locked_pieces(&model, &[locked_stone]).expect("the piece locks");
    // The rough keeps its base, its frame and its own inclusion; the piece joins it.
    assert!(matches!(locked.model.base, RoughBase::Block { .. }));
    assert_eq!(locked.offset_mm, [0.0; 3]);
    assert_eq!(locked.model.cuts, model.cuts);
    assert_eq!(locked.model.inclusion_count(), 2);
    assert_eq!(model.inclusion_count(), 1, "the original is unchanged");
    let (before, after) = (
        model.measure().expect("measures"),
        locked.model.measure().expect("measures"),
    );
    assert_eq!(after.inclusion_count, 2);
    assert!((after.volume_mm3 - before.volume_mm3).abs() < 1e-6 * before.volume_mm3);

    let designs = [box_design(1)];
    let input = PlanInput {
        model: &locked.model,
        settings: &settings,
        designs: &designs,
        hulls: &[],
    };
    assert_eq!(input.path(), PlanPath::Shaped);
    let replanned = plan(&input, &mut |_| true).expect("not cancelled");
    assert_ne!(replanned, [] as [RoughLayout; 0]);
    let inclusion_stone = stone_at([8.0; 3], 3.0);
    for stone in &replanned[0].stones {
        assert!(
            apart(stone, &locked_stone, 1e-3),
            "{stone:?} meets the locked piece"
        );
        assert!(
            apart(stone, &inclusion_stone, 1e-3),
            "{stone:?} meets the inclusion"
        );
    }
}

#[test]
fn a_lock_next_to_an_inclusion_that_a_cut_slices_still_locks() {
    // The cut x <= 10 slices the inclusion [8.5, 11.5]; as a hull the rough would have the
    // shell through its surface, which is why the pieces join the inclusions instead.
    let cut = RoughCut::Face {
        normal: [1.0, 0.0, 0.0],
        depth_mm: 2.0,
    };
    let model = with_inclusion(&block_model(vec![cut]), [10.0, 6.0, 6.0], 3.0);
    let locked = rough_with_locked_pieces(&model, &[stone_at([1.0; 3], 2.0)]).expect("locks");
    assert_eq!(locked.model.inclusion_count(), 2);
    assert_eq!(locked.model.cuts, model.cuts);
    assert!(locked.model.measure().is_ok());
}

#[test]
fn a_locked_piece_that_meets_an_inclusion_is_refused() {
    let model = with_inclusion(&block_model(Vec::new()), [8.0; 3], 3.0);
    let error = rough_with_locked_pieces(&model, &[stone_at([8.0; 3], 2.0)])
        .expect_err("the piece lies inside the inclusion");
    assert!(matches!(error, LockError::Shell(_)), "{error:?}");
    // The piece is named as a locked stone; the rough has one inclusion, not two.
    let text = error.to_string();
    assert!(text.contains("locked stone 1"), "{text}");
    assert!(!text.contains("Inclusion 2"), "{text}");
}

#[test]
fn a_block_without_inclusions_locks_as_it_always_did() {
    // The hull route, offset and frame included, is untouched.
    let model = block_model(Vec::new());
    let locked = rough_with_locked_pieces(&model, &[stone_at([1.0; 3], 2.0)]).expect("locks");
    assert!(matches!(locked.model.base, RoughBase::Hull { .. }));
    assert!(locked.model.inclusions.is_empty());
    assert_eq!(locked.model.inclusion_count(), 1);
}
