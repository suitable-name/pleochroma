//! Filmed route for the tier note's persistence example.

use crate::{director::Film, pacing};
use pleochroma_cut::EditorModel;
use slint::{ComponentHandle, Model};

const TIER: &str = "Pavilion Main";
const NOTE: &str = "check the meet here";

pub(super) fn drive(film: &mut Film<'_>, step: usize) -> Result<(), String> {
    if step != 2 {
        return Err(format!("unexpected Tier Notes script step {step}"));
    }

    film.rec
        .caption("Re-add a note to demonstrate that it follows its tier.");
    film.hold(pacing::reading_time(12))?;
    film.film_tier_note(TIER, NOTE)?;

    let before = film.film_select_tier(TIER)?;
    film.film_command("tiers.move_down")?;
    let after = film
        .ui
        .global::<EditorModel>()
        .get_tiers()
        .iter()
        .position(|row| row.name.as_str() == TIER)
        .and_then(|index| i32::try_from(index).ok())
        .ok_or("Pavilion Main disappeared after moving it")?;
    if after != before + 1 {
        return Err(format!(
            "Pavilion Main did not move down one row: before {before}, after {after}"
        ));
    }

    let rows = film.ui.global::<EditorModel>().get_tiers();
    let neighbor = rows
        .row_data(usize::try_from(before).map_err(|_| "invalid Pavilion Main row")?)
        .map(|row| row.name.to_string())
        .ok_or("the neighboring tier row is missing")?;
    if neighbor == TIER {
        return Err("the row below Pavilion Main is still Pavilion Main".into());
    }
    film.film_select_tier(&neighbor)?;
    let editor = film.ui.global::<EditorModel>();
    if !editor.get_selected_tier_note_text().is_empty() {
        return Err(format!(
            "neighboring tier {neighbor:?} inherited note {:?}",
            editor.get_selected_tier_note_text()
        ));
    }
    film.verify_tier_annotation(
        "Note for This Tier",
        "",
        editor.get_selected_tier_note_text().as_str(),
    )?;
    film.film_select_tier(TIER)?;
    let note = editor.get_selected_tier_note_text();
    if note != NOTE {
        return Err(format!(
            "Pavilion Main note did not persist after moving: expected {NOTE:?}, got {note:?}"
        ));
    }
    film.verify_tier_annotation("Note for This Tier", NOTE, note.as_str())?;
    film.hold(pacing::RESULT_HOLD)?;
    Ok(())
}
