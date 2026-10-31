//! The cutter's stone choices in the library: the side table beside a saved plan.
//!
//! The library keeps them in `stone_pose_choice` (see the vault's `stone_choice_store`): the box
//! pose, a free pose (as the JSON of [`FreePoseDto`], the same text a file's `[axis.stones.free_pose]`
//! holds), the lock and the Reason. A plan that has none writes nothing here.
//!
//! The results of a saved plan are indexed `(layout, stone)` as the plan lists them (best rank
//! first). The window's settings are indexed by the result row on screen, so a save that covers
//! only some of the rows moves the choices to the new positions ([`remap_to_saved`]).

use super::axis::{FreePoseDto, free_pose_from_dto, free_pose_to_dto, is_rich};
use pleochroma_cut_core::rough_plan::orientation::StoneChoice;
use pleochroma_vault::{db::sqlite::Database, model::zoning::StoneChoiceRow};
use std::collections::BTreeMap;

/// The stone choices of a plan: `(layout, stone)` to the choice.
pub(super) type StoneChoices = BTreeMap<(u32, u32), StoneChoice>;

/// The library row of a choice.
///
/// # Errors
///
/// A message when the free pose cannot be written as text.
pub(super) fn row_of(
    layout: u32,
    stone: u32,
    choice: &StoneChoice,
) -> Result<StoneChoiceRow, String> {
    let free_pose_json = choice
        .free_pose
        .as_ref()
        .map(|pose| {
            serde_json::to_string(&free_pose_to_dto(pose))
                .map_err(|e| format!("A free pose could not be written: {e}"))
        })
        .transpose()?;
    Ok(StoneChoiceRow {
        layout_index: layout,
        stone_index: stone,
        pose: choice.pose,
        free_pose_json,
        locked: choice.locked,
        reason: choice.reason.clone(),
    })
}

/// The choice a library row holds.
///
/// # Errors
///
/// A message when the stored free pose cannot be read or is not usable.
pub(super) fn choice_of(row: &StoneChoiceRow) -> Result<StoneChoice, String> {
    let free_pose = row
        .free_pose_json
        .as_deref()
        .map(|text| {
            let dto: FreePoseDto = serde_json::from_str(text).map_err(|e| {
                format!(
                    "The free pose of stone {} of result {} cannot be read: {e}",
                    row.stone_index + 1,
                    row.layout_index + 1
                )
            })?;
            free_pose_from_dto(&dto, "free_pose")
        })
        .transpose()?;
    Ok(StoneChoice {
        pose: row.pose,
        free_pose,
        locked: row.locked,
        reason: row.reason.clone(),
    })
}

/// The stored choices of plan `plan_id`.
///
/// # Errors
///
/// A message when the library cannot be read or a stored free pose is damaged.
pub(super) fn load_stone_choices(db: &Database, plan_id: i64) -> Result<StoneChoices, String> {
    let rows = db
        .stone_choices(plan_id)
        .map_err(|e| format!("Could not read the stone choices of plan #{plan_id}: {e:#}"))?;
    rows.iter()
        .map(|row| Ok(((row.layout_index, row.stone_index), choice_of(row)?)))
        .collect()
}

/// Stores `stones` among the choices of plan `plan_id`: the choices are merged by `(layout,
/// stone)`, so a choice the color wizard stored for the same plan stays unless the planner has a
/// choice for that very stone, which wins.
///
/// # Errors
///
/// A message when a write fails.
pub(super) fn save_stone_choices(
    db: &Database,
    plan_id: i64,
    stones: &StoneChoices,
) -> Result<(), String> {
    for (&(layout, stone), choice) in stones {
        let row = row_of(layout, stone, choice)?;
        db.set_stone_choice(plan_id, &row)
            .map_err(|e| format!("The stone choices could not be stored: {e:#}"))?;
    }
    Ok(())
}

/// Clears the stored choices of the first `saved_layouts` layouts of plan `plan_id` that `stones`
/// no longer holds: a stone the cutter set back to the default (no pose, lock or Reason) must not
/// keep an older stored row. The rows of layouts past `saved_layouts` (the color wizard's pose rows
/// of layouts the save did not cover) stay.
///
/// # Errors
///
/// A message when the library cannot be read or a write fails.
pub(super) fn clear_removed_stone_choices(
    db: &Database,
    plan_id: i64,
    stones: &StoneChoices,
    saved_layouts: usize,
) -> Result<(), String> {
    let stored = load_stone_choices(db, plan_id)?;
    for &(layout, stone) in stored.keys() {
        let in_saved_layout = usize::try_from(layout).is_ok_and(|layout| layout < saved_layouts);
        if !in_saved_layout || stones.contains_key(&(layout, stone)) {
            continue;
        }
        let row = row_of(layout, stone, &StoneChoice::default())?;
        db.set_stone_choice(plan_id, &row)
            .map_err(|e| format!("The stone choices could not be stored: {e:#}"))?;
    }
    Ok(())
}

/// Stores the choices of `stones` that a version 4 pose table cannot hold (a free pose, a lock or
/// a Reason), leaving every other row as it is. Used by an import, after the poses.
///
/// # Errors
///
/// A message when a write fails.
pub(super) fn save_rich_stone_choices(
    db: &Database,
    plan_id: i64,
    stones: &StoneChoices,
) -> Result<(), String> {
    for (&(layout, stone), choice) in stones.iter().filter(|(_, choice)| is_rich(choice)) {
        let row = row_of(layout, stone, choice)?;
        db.set_stone_choice(plan_id, &row)
            .map_err(|e| format!("The stone choices could not be stored: {e:#}"))?;
    }
    Ok(())
}

/// Moves the choices of the result rows to the positions of a saved plan: `ranks` lists, for each
/// layout the plan stores in order, the 1-based rank (row number) it had on screen. Choices of a
/// row that is not saved are dropped.
#[must_use]
pub(super) fn remap_to_saved(stones: &StoneChoices, ranks: &[usize]) -> StoneChoices {
    let mut out = StoneChoices::new();
    for (position, rank) in ranks.iter().enumerate() {
        let (Ok(row), Ok(position)) = (
            u32::try_from(rank.saturating_sub(1)),
            u32::try_from(position),
        ) else {
            continue;
        };
        for (&(_, stone), choice) in stones.range((row, 0)..=(row, u32::MAX)) {
            out.insert((position, stone), choice.clone());
        }
    }
    out
}
