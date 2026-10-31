//! Changing the layouts of the shown results: the locked stones listed with every layout, a stone
//! replaced by a re-fit or a hand turn, and the box poses the cutter chose applied for drawing.
//!
//! A locked stone is not part of a layout's saw plan (a plan that keeps the stone cuts the REST of
//! the rough). It is appended to the layout's stones after the saw plan's own, so everything that
//! reads a layout (the rows, the 3D view, the saved plan, whose checks stop at the saw plan's last
//! piece) keeps working, and the totals count it. [`locked_tail`] finds those stones again.
//!
//! Everything here is plain data in, plain data out.

use crate::gui::rough_plan::session::LayoutEdits;
use pleochroma_cut_core::rough_plan::{
    DesignHull, PlacedStone, RoughLayout, SingleFit, layout_from_single_fit,
    orientation::{PlanSettingsOverrides, StoneChoice},
};
use std::collections::BTreeMap;

/// How many stones at the end of `layout` are locked stones: the stones beyond the pieces of its
/// saw plan (an exact fit has one stone of its own).
#[must_use]
pub(in crate::gui::rough_plan) fn locked_tail(layout: &RoughLayout) -> usize {
    let planned = if layout.exact_fit {
        1
    } else {
        layout
            .cut_plan
            .slabs
            .iter()
            .flat_map(|slab| &slab.bars)
            .map(|bar| bar.pieces_mm.len())
            .sum::<usize>()
    };
    layout.stones.len().saturating_sub(planned)
}

/// The locked stones of result `result` (its stones whose choice is locked), in the layout's
/// order: the stones a re-plan keeps.
#[must_use]
pub(in crate::gui::rough_plan) fn locked_stones(
    layout: &RoughLayout,
    result: u32,
    choices: &PlanSettingsOverrides,
) -> Vec<PlacedStone> {
    layout
        .stones
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            u32::try_from(*index)
                .ok()
                .and_then(|stone| choices.stone_choice(result, stone))
                .is_some_and(|choice| choice.locked)
        })
        .map(|(_, stone)| *stone)
        .collect()
}

/// The choices that mark the locked stones a fresh list of results carries: the stones at the end
/// of every layout beyond its saw plan are the locked ones (see [`locked_tail`]).
#[must_use]
pub(in crate::gui::rough_plan) fn locks_of(
    layouts: &[RoughLayout],
) -> BTreeMap<(u32, u32), StoneChoice> {
    let mut locks = BTreeMap::new();
    for (result, layout) in layouts.iter().enumerate() {
        let tail = locked_tail(layout);
        let first = layout.stones.len() - tail;
        for stone in first..layout.stones.len() {
            if let (Ok(result), Ok(stone)) = (u32::try_from(result), u32::try_from(stone)) {
                locks.insert(
                    (result, stone),
                    StoneChoice {
                        locked: true,
                        ..StoneChoice::default()
                    },
                );
            }
        }
    }
    locks
}

/// The shown layouts: the planned ones `base` with every layout the cutter changed laid over its
/// result.
#[must_use]
pub(in crate::gui::rough_plan) fn with_edits(
    base: &[RoughLayout],
    edits: &LayoutEdits,
) -> Vec<RoughLayout> {
    base.iter()
        .enumerate()
        .map(|(index, layout)| {
            u32::try_from(index)
                .ok()
                .and_then(|result| edits.get(&result))
                .unwrap_or(layout)
                .clone()
        })
        .collect()
}

/// Sets the totals of `layout` from its stones. The yield is the volume over `rough_volume_mm3`
/// when that is known; otherwise the old yield scaled by the change of the volume.
pub(in crate::gui::rough_plan) fn retotal(layout: &mut RoughLayout, rough_volume_mm3: Option<f64>) {
    let old_volume = layout.total_volume_mm3;
    layout.total_carat = layout.stones.iter().map(|stone| stone.carat).sum();
    layout.total_volume_mm3 = layout.stones.iter().map(|stone| stone.volume_mm3).sum();
    match rough_volume_mm3.filter(|volume| *volume > 0.0) {
        Some(volume) => layout.yield_fraction = layout.total_volume_mm3 / volume,
        None if old_volume > 0.0 => {
            layout.yield_fraction *= layout.total_volume_mm3 / old_volume;
        }
        None => {}
    }
}

/// Appends the locked stones `locked` to `layout`, after its own, and counts them in the totals.
pub(in crate::gui::rough_plan) fn append_locked(
    layout: &mut RoughLayout,
    locked: &[PlacedStone],
    rough_volume_mm3: Option<f64>,
) {
    if locked.is_empty() {
        return;
    }
    layout.stones.extend(locked.iter().copied());
    retotal(layout, rough_volume_mm3);
}

/// Puts `stone` in place of stone `index` of `layout` (a re-fit of its piece) and recounts the
/// totals. Returns whether the layout has such a stone.
pub(in crate::gui::rough_plan) fn replace_stone(
    layout: &mut RoughLayout,
    index: usize,
    stone: PlacedStone,
    rough_volume_mm3: Option<f64>,
) -> bool {
    let Some(slot) = layout.stones.get_mut(index) else {
        return false;
    };
    *slot = stone;
    retotal(layout, rough_volume_mm3);
    true
}

/// The layout of an exact single-stone fit after the stone was turned: a new layout of the one
/// stone (its piece is the stone's box in its new pose), with the locked stones `tail` after it.
#[must_use]
pub(in crate::gui::rough_plan) fn rebuilt_exact_fit(
    fit: &SingleFit,
    hull: &DesignHull,
    rough_volume_mm3: f64,
    tail: &[PlacedStone],
) -> RoughLayout {
    let mut layout = layout_from_single_fit(fit, hull, rough_volume_mm3);
    append_locked(&mut layout, tail, Some(rough_volume_mm3));
    layout
}

/// `layout` with the box poses and free poses the cutter chose for result `result` applied to
/// its stones, as the 3D view and the thumbnails draw it. A layout nothing was chosen for is
/// returned as it is.
#[must_use]
pub(in crate::gui::rough_plan) fn posed_layout(
    layout: &RoughLayout,
    result: usize,
    choices: &PlanSettingsOverrides,
) -> RoughLayout {
    // The pose of an exact fit is part of its layout (a stone turned by hand is baked in).
    let Ok(result) = u32::try_from(result) else {
        return layout.clone();
    };
    if layout.exact_fit {
        return layout.clone();
    }
    let mut posed = layout.clone();
    for (index, stone) in posed.stones.iter_mut().enumerate() {
        let choice = u32::try_from(index)
            .ok()
            .and_then(|stone_index| choices.stone_choice(result, stone_index));
        if let Some(choice) = choice {
            stone.pose = choice.pose_for(&stone.pose);
        }
    }
    posed
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::rough_plan::{
        Axis, BarCut, CutOrder, CutPlan, SlabCut, fit::StonePose, orientation::StoneChoice,
    };

    fn stone(carat: f64) -> PlacedStone {
        PlacedStone {
            entry_id: 1,
            piece_origin_mm: [0.0; 3],
            piece_size_mm: [2.0; 3],
            stone_size_mm: [2.0; 3],
            table_axis: Axis::Y,
            carat,
            volume_mm3: carat * 10.0,
            pose: StonePose {
                center_mm: [1.0; 3],
                axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                mm_per_unit: 1.0,
            },
        }
    }

    /// Two pieces in one slab and one bar, two stones, a rough of 100 mm^3.
    fn sawn() -> RoughLayout {
        RoughLayout {
            cut_order: CutOrder::Xyz,
            stones: vec![stone(1.0), stone(2.0)],
            cut_plan: CutPlan {
                slabs: vec![SlabCut {
                    thickness_mm: 2.0,
                    bars: vec![BarCut {
                        width_mm: 2.0,
                        pieces_mm: vec![2.0, 2.0],
                    }],
                }],
            },
            total_carat: 3.0,
            total_volume_mm3: 30.0,
            yield_fraction: 0.3,
            exact_fit: false,
        }
    }

    #[test]
    fn stones_beyond_the_saw_plan_are_the_locked_ones() {
        let mut layout = sawn();
        assert_eq!(locked_tail(&layout), 0);
        append_locked(&mut layout, &[stone(0.5)], Some(100.0));
        assert_eq!(locked_tail(&layout), 1);
        assert_eq!(layout.stones.len(), 3);
        let mut exact = sawn();
        exact.exact_fit = true;
        exact.stones.truncate(1);
        assert_eq!(locked_tail(&exact), 0);
        exact.stones.push(stone(1.0));
        assert_eq!(locked_tail(&exact), 1);
    }

    #[test]
    fn a_locked_stone_counts_in_the_totals_and_the_yield() {
        let mut layout = sawn();
        append_locked(&mut layout, &[stone(0.5)], Some(100.0));
        assert!((layout.total_carat - 3.5).abs() < 1e-12);
        assert!((layout.total_volume_mm3 - 35.0).abs() < 1e-12);
        assert!((layout.yield_fraction - 0.35).abs() < 1e-12);
        // Nothing to append changes nothing.
        let before = layout.clone();
        append_locked(&mut layout, &[], Some(100.0));
        assert_eq!(layout, before);
    }

    #[test]
    fn without_a_rough_volume_the_yield_follows_the_volume() {
        let mut layout = sawn();
        append_locked(&mut layout, &[stone(3.0)], None);
        // The volume doubled, so the yield did.
        assert!((layout.yield_fraction - 0.6).abs() < 1e-12);
    }

    #[test]
    fn the_locked_stones_of_a_result_are_the_ones_whose_choice_says_so() {
        let layout = sawn();
        let mut choices = PlanSettingsOverrides::default();
        assert_eq!(locked_stones(&layout, 0, &choices).len(), 0);
        choices.set_stone_choice(
            0,
            1,
            StoneChoice {
                locked: true,
                ..StoneChoice::default()
            },
        );
        assert_eq!(locked_stones(&layout, 0, &choices), [layout.stones[1]]);
        assert!(
            locked_stones(&layout, 1, &choices).is_empty(),
            "the lock belongs to result 0"
        );
    }

    #[test]
    fn the_stones_beyond_the_saw_plan_of_every_result_are_marked_locked() {
        let mut with_lock = sawn();
        append_locked(&mut with_lock, &[stone(0.5), stone(0.7)], Some(100.0));
        let locks = locks_of(&[sawn(), with_lock]);
        assert_eq!(locks.keys().copied().collect::<Vec<_>>(), [(1, 2), (1, 3)]);
        assert!(locks.values().all(|choice| choice.locked));
        assert!(locks_of(&[sawn()]).is_empty());
    }

    #[test]
    fn the_changed_layouts_are_laid_over_the_planned_ones() {
        let planned = vec![sawn(), sawn()];
        let mut changed = sawn();
        changed.total_carat = 9.0;
        let edits = LayoutEdits::from([(1, changed.clone())]);
        let shown = with_edits(&planned, &edits);
        assert_eq!(shown[0], planned[0]);
        assert_eq!(shown[1], changed);
        assert_eq!(with_edits(&planned, &LayoutEdits::new()), planned);
    }

    #[test]
    fn a_replaced_stone_is_recounted() {
        let mut layout = sawn();
        assert!(replace_stone(&mut layout, 1, stone(4.0), Some(100.0)));
        assert!((layout.total_carat - 5.0).abs() < 1e-12);
        assert!(!replace_stone(&mut layout, 9, stone(1.0), Some(100.0)));
        assert_eq!(layout.stones.len(), 2);
    }

    #[test]
    fn an_exact_fit_is_drawn_as_it_is() {
        let mut exact = sawn();
        exact.exact_fit = true;
        let mut choices = PlanSettingsOverrides::default();
        choices.set_stone_choice(
            0,
            0,
            StoneChoice {
                pose: 1,
                ..StoneChoice::default()
            },
        );
        assert_eq!(posed_layout(&exact, 0, &choices), exact);
    }

    #[test]
    fn the_chosen_poses_are_applied_for_drawing_and_nothing_else_changes() {
        let layout = sawn();
        let mut choices = PlanSettingsOverrides::default();
        assert_eq!(posed_layout(&layout, 0, &choices), layout);
        choices.set_stone_choice(
            0,
            1,
            StoneChoice {
                pose: 1,
                ..StoneChoice::default()
            },
        );
        let posed = posed_layout(&layout, 0, &choices);
        assert_eq!(posed.stones[0], layout.stones[0], "stone 0 was not chosen");
        assert_ne!(posed.stones[1].pose, layout.stones[1].pose);
        assert_eq!(posed.stones[1].carat, layout.stones[1].carat);
        // Another result has no such choice.
        assert_eq!(posed_layout(&layout, 1, &choices), layout);
    }
}
