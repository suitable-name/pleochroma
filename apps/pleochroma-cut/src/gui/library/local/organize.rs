//! Organizing an existing local design: ignore, planner exclusion, rename, shape, tags and
//! delete. Import and export live in their own sibling modules.

use super::helpers::refresh_after_library_change;
use crate::{
    LibraryModel, MainWindow,
    bridge::{
        library::source::LibrarySource,
        render_thread::{PlanesOwner, RenderContext},
    },
    gui::show_toast,
};
use pleochroma::geometry::cuts::StandardGemCuts;
use pleochroma_vault::{db::sqlite::Database, model::metadata_update::MetadataUpdate};
use slint::{ComponentHandle, Model, SharedString};
use std::sync::{Arc, Mutex};

/// Wires the per-design "Ignore" toggle in `diagram_list.slint`.
///
/// Refuses while a remote library is browsed: `Database::set_diagram_ignored` writes the
/// local database, and the remote protocol has no request for it.
pub fn setup_ignore_toggle_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_ignore = Arc::clone(db);
    let source_ignore = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_toggle_ignored(move |id: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if source_ignore
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_remote()
            {
                show_toast(
                    &ui,
                    "Switch to the local library to ignore a design.",
                    "error",
                );
                return;
            }
            // The visible list carries the ignored state (`DiagramItem.ignored`, set by
            // `gui::library::search::fetch_diagram_list`), so no database read is needed.
            // A row missing from the list (a stale click) counts as not ignored, so the
            // click ignores it: ignoring is the common direction, and undoing is one click.
            let currently_ignored = ui
                .global::<LibraryModel>()
                .get_diagram_list()
                .iter()
                .find(|item| item.id == id)
                .is_some_and(|item| item.ignored);
            let new_ignored = !currently_ignored;
            let result = {
                let db = db_ignore
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                db.set_diagram_ignored(i64::from(id), new_ignored)
            };
            match result {
                Ok(()) => {
                    show_toast(
                        &ui,
                        if new_ignored {
                            "Design ignored."
                        } else {
                            "Design un-ignored."
                        },
                        "success",
                    );
                    refresh_after_library_change(&ui, &db_ignore, &source_ignore);
                }
                Err(e) => show_toast(&ui, &format!("Failed to update ignored flag: {e}"), "error"),
            }
        });
}

/// Wires the per-design "Exclude from planner" toggle in `diagram_list.slint`.
///
/// The mark only keeps the design out of the Rough Planner's candidates. Unlike "Ignore", the
/// design stays in the list and in every other batch. Refuses while a remote library is
/// browsed, as [`setup_ignore_toggle_callback`] does.
///
/// After the write the library list is refreshed, and `rough_plan::exclusions_changed`
/// tells an open planner window to re-read its excluded designs.
pub fn setup_planner_exclusion_toggle_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_exclude = Arc::clone(db);
    let source_exclude = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_toggle_planner_excluded(move |id: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if source_exclude
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_remote()
            {
                show_toast(
                    &ui,
                    "Switch to the local library to change which designs are planned.",
                    "error",
                );
                return;
            }
            // The visible row carries the current state (`DiagramItem.planner_excluded`), so no
            // database read is needed. A row missing from the list (a stale click) counts as
            // not excluded, so the click excludes it.
            let shown = ui
                .global::<LibraryModel>()
                .get_diagram_list()
                .iter()
                .find(|item| item.id == id);
            let new_excluded = !shown.as_ref().is_some_and(|item| item.planner_excluded);
            let title = shown.map_or_else(|| format!("#{id}"), |item| item.title.to_string());
            let result = {
                let db = db_exclude
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                db.set_planner_excluded(i64::from(id), new_excluded)
            };
            match result {
                Ok(()) => {
                    let message = if new_excluded {
                        format!("Excluded \"{title}\" from the rough planner.")
                    } else {
                        format!("\"{title}\" can be planned again.")
                    };
                    show_toast(&ui, &message, "success");
                    refresh_after_library_change(&ui, &db_exclude, &source_exclude);
                    crate::gui::rough_plan::exclusions_changed();
                }
                Err(e) => show_toast(
                    &ui,
                    &format!("Failed to change the planner exclusion: {e}"),
                    "error",
                ),
            }
        });
}

/// Wires up the detail header's inline rename (pencil icon -> `rename_diagram`).
///
/// Refuses while a remote library is browsed. The selected id then names a row in the
/// remote catalogue, and the remote protocol is read-only. Renaming a local row is not
/// offered either, since the selection is the remote one.
pub fn setup_rename_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_rename = Arc::clone(db);
    let source_rename = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_rename_diagram(move |new_title: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if source_rename
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_remote()
            {
                show_toast(
                    &ui,
                    "Switch to the local library to rename a design.",
                    "error",
                );
                return;
            }
            let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
            if entry_id < 0 {
                return;
            }
            let result = {
                let db = db_rename
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                db.rename_diagram_entry(i64::from(entry_id), &new_title)
            };
            match result {
                Ok(()) => {
                    let mut detail = ui.global::<LibraryModel>().get_current_detail();
                    detail.title = new_title;
                    ui.global::<LibraryModel>().set_current_detail(detail);
                    show_toast(&ui, "Renamed.", "success");
                    refresh_after_library_change(&ui, &db_rename, &source_rename);
                }
                Err(e) => show_toast(&ui, &format!("Rename failed: {e}"), "error"),
            }
        });
}

/// Sets `entry_id`'s shape to `new_shape`, called by [`setup_set_shape_callback`].
///
/// Edits exactly one column. `Database::update_diagram_metadata` issues a narrow `UPDATE`
/// naming only the fields it is given. `Database::save_diagram_detail` would replace the
/// whole detail row, zeroing anything not carried across.
///
/// The other fields of the [`MetadataUpdate`] are read back from the current record and
/// passed through unchanged. Stored doubles come back as the shortest text that parses to
/// the same value, so they survive exactly. The proportions are not recomputed: they are
/// measured at import (see `super::import::apply_measured_metadata`), and re-deriving them
/// on a shape edit could disagree with the stored values.
fn set_diagram_shape(db: &Database, entry_id: i64, new_shape: &str) -> Result<(), String> {
    let full = db
        .get_diagram_full(entry_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Diagram detail not found.".to_string())?;

    let update = MetadataUpdate {
        shape: Some(new_shape.to_string()),
        // Everything below is this design's existing value, round-tripped unchanged.
        designer_info: full.designer_info,
        refractive_index: full.refractive_index,
        index_gear: full.index_gear,
        facets_count: full.facets_count,
        symmetry_order: full.symmetry_order,
        mirror_symmetry: full.mirror_symmetry,
        lw_ratio: full.lw_ratio,
        hw_ratio: full.hw_ratio,
        cw_ratio: full.cw_ratio,
        pw_ratio: full.pw_ratio,
        volume: full.volume,
    };
    db.update_diagram_metadata(entry_id, &update)
        .map_err(|e| e.to_string())
}

/// Builds the shape picker's options and the index of `current` within them.
///
/// `current` is `None` or empty for an unclassified design, which gives index -1. Called from
/// `gui::library::detail::load_diagram_detail` whenever a local design opens.
///
/// `Database::get_unique_shapes` already returns the seeded `DEFAULT_SHAPES` plus every
/// shape present in the library, sorted. A `current` value missing from that list is added
/// too, so opening the picker never drops the design's own shape.
pub fn build_shape_picker_options(
    db: &Database,
    current: Option<&str>,
) -> (Vec<SharedString>, i32) {
    let mut shapes = db.get_unique_shapes().unwrap_or_default();
    let current = current.unwrap_or("").trim();
    if !current.is_empty() && !shapes.iter().any(|s| s == current) {
        shapes.push(current.to_string());
        shapes.sort();
    }
    let index = if current.is_empty() {
        -1
    } else {
        shapes
            .iter()
            .position(|s| s == current)
            .map_or(-1, |i| i32::try_from(i).unwrap_or(i32::MAX))
    };
    (shapes.into_iter().map(SharedString::from).collect(), index)
}

/// Wires up the detail header's shape picker (pencil icon next to the "Shape:" chip) to
/// `set_shape`.
///
/// Refuses while a remote library is browsed. `detail_header.slint` also shows the pencil
/// only for local designs, so this check is a backstop.
pub fn setup_set_shape_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_shape = Arc::clone(db);
    let source_shape = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_set_shape(move |new_shape: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if source_shape
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_remote()
            {
                show_toast(
                    &ui,
                    "Switch to the local library to change a design's shape.",
                    "error",
                );
                return;
            }
            let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
            if entry_id < 0 {
                return;
            }
            let result = {
                let db = db_shape
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                set_diagram_shape(&db, i64::from(entry_id), new_shape.as_str())
            };
            match result {
                Ok(()) => {
                    let mut detail = ui.global::<LibraryModel>().get_current_detail();
                    detail.shape = new_shape;
                    ui.global::<LibraryModel>().set_current_detail(detail);
                    show_toast(&ui, "Shape updated.", "success");
                    refresh_after_library_change(&ui, &db_shape, &source_shape);
                }
                Err(e) => show_toast(&ui, &format!("Shape update failed: {e}"), "error"),
            }
        });
}

/// Wires up the "Add tag..." context-menu entry (`diagram_list.slint`) to `add_tag_to_entry`.
///
/// That creates the tag if needed and attaches it to `entry_id`. A blank name is rejected by
/// the database and shown as a toast.
///
/// Refuses while a remote library is browsed.
pub fn setup_add_tag_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_tag = Arc::clone(db);
    let source_tag = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>().on_add_tag_to_entry(
        move |entry_id: i32, tag_name: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if source_tag
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_remote()
            {
                show_toast(&ui, "Switch to the local library to tag a design.", "error");
                return;
            }
            let result = {
                let db = db_tag
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                db.add_tag_to_entry(i64::from(entry_id), tag_name.as_str())
            };
            match result {
                Ok(tag) => {
                    show_toast(&ui, &format!("Tagged \"{}\".", tag.name), "success");
                    refresh_after_library_change(&ui, &db_tag, &source_tag);
                }
                Err(e) => show_toast(&ui, &format!("Failed to add tag: {e}"), "error"),
            }
        },
    );
}

/// Wires up a tag chip's remove control (`diagram_list.slint`) to `remove_tag_from_entry`.
///
/// The chip carries the tag's name, not its id, so the name is resolved with
/// `Database::tag_id_by_name`. A name that no longer resolves (the tag was deleted meanwhile)
/// is a no-op: there is nothing left to remove.
///
/// Refuses while a remote library is browsed.
pub fn setup_remove_tag_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_untag = Arc::clone(db);
    let source_untag = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>().on_remove_tag_from_entry(
        move |entry_id: i32, tag_name: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if source_untag
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_remote()
            {
                show_toast(
                    &ui,
                    "Switch to the local library to untag a design.",
                    "error",
                );
                return;
            }
            let result = {
                let db = db_untag
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match db.tag_id_by_name(tag_name.as_str()) {
                    Ok(Some(tag_id)) => db.remove_tag_from_entry(i64::from(entry_id), tag_id),
                    Ok(None) => Ok(()),
                    Err(e) => Err(e),
                }
            };
            match result {
                Ok(()) => refresh_after_library_change(&ui, &db_untag, &source_untag),
                Err(e) => show_toast(&ui, &format!("Failed to remove tag: {e}"), "error"),
            }
        },
    );
}

/// Wires up the two ways to delete a design.
///
/// The detail header's `delete_diagram` acts on the selected row, already confirmed inline.
/// The list's `delete_entry(id, title)` acts on any row, after a confirm dialog. Both end in
/// [`delete_entry_now`].
///
/// Refuses while a remote library is browsed. A remote id applied to the local database
/// would delete the wrong row.
pub fn setup_delete_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let db_delete = Arc::clone(db);
    let source_delete = Arc::clone(source);
    let render_ctx_delete = render_ctx.clone();
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>().on_delete_diagram(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
        delete_entry_now(
            &ui,
            &db_delete,
            &source_delete,
            &render_ctx_delete,
            entry_id,
        );
    });

    let db_entry = Arc::clone(db);
    let source_entry = Arc::clone(source);
    let render_ctx_entry = render_ctx.clone();
    let ui_weak = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_delete_entry(move |entry_id: i32, title: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // Refused before asking, so the cutter is never asked to confirm a delete
            // that cannot happen; `delete_entry_now` checks again on Delete, since the
            // browsed library may have changed while the dialog was open.
            if entry_id < 0 || refuse_delete_while_remote(&ui, &source_entry) {
                return;
            }
            let db = Arc::clone(&db_entry);
            let source = Arc::clone(&source_entry);
            let render_ctx = render_ctx_entry.clone();
            crate::gui::editor::native_io::ask_write_confirm(
                &ui,
                "Delete Design?",
                delete_question(title.as_str()),
                "Delete",
                // Never suppressible: a delete cannot be undone.
                None,
                move |ui| delete_entry_now(ui, &db, &source, &render_ctx, entry_id),
            );
        });
}

/// The question the context menu's delete asks, naming the design.
fn delete_question(title: &str) -> String {
    let title = title.trim();
    let name = if title.is_empty() {
        "This design".to_string()
    } else {
        format!("\u{201c}{title}\u{201d}")
    };
    format!(
        "{name} and the files attached to it will be removed from your library. \
         This cannot be undone."
    )
}

/// Shows the "switch to the local library" toast and returns `true` while a remote
/// library is browsed.
fn refuse_delete_while_remote(ui: &MainWindow, source: &Arc<Mutex<LibrarySource>>) -> bool {
    let remote = source
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_remote();
    if remote {
        show_toast(
            ui,
            "Switch to the local library to delete a design.",
            "error",
        );
    }
    remote
}

/// Removes the entry and clears whatever still shows it.
///
/// `Database::delete_diagram_entry` cascades to the detail, angle and attachment rows. The
/// detail pane is cleared when the entry is selected, and the 3D viewport when its planes are
/// loaded.
fn delete_entry_now(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i32,
) {
    if entry_id < 0 || refuse_delete_while_remote(ui, source) {
        return;
    }
    let result = {
        let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        db.delete_diagram_entry(i64::from(entry_id))
    };
    match result {
        Ok(()) => {
            // Only the selected design's detail pane is cleared. Deleting another row
            // leaves the selected design on screen.
            if ui.global::<LibraryModel>().get_selected_entry_id() == entry_id {
                crate::gui::library::detail::clear_current_detail_display(ui);
            }
            {
                let mut ctx = RenderContext::lock(render_ctx);
                // Only reset the traced geometry when the deleted row is the
                // one whose planes are actually loaded -- resetting unconditionally
                // would throw away an unrelated design being edited in the viewport.
                let owns_deleted_row = matches!(
                    ctx.planes_owner,
                    PlanesOwner::Catalogue { entry_id: owned } if owned == i64::from(entry_id)
                );
                if owns_deleted_row {
                    ctx.claim_active_planes(
                        std::sync::Arc::new(StandardGemCuts::standard_round_brilliant()),
                        None,
                        PlanesOwner::Builtin,
                    );
                    ctx.dirty = true;
                }
            }
            show_toast(ui, "Diagram deleted.", "info");
            refresh_after_library_change(ui, db, source);
        }
        Err(e) => show_toast(ui, &format!("Delete failed: {e}"), "error"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::library::local::helpers::test_support::{open_temp_db, temp_db_path_for_test};
    use pleochroma_vault::model::detail::FacetingDiagramDetail;

    /// `set_diagram_shape` must edit `shape` and nothing else.
    ///
    /// The vault tests show that `update_diagram_metadata` writes only the columns it is
    /// given. This checks that the wiring passes all of them: a field omitted from the
    /// `MetadataUpdate` literal would compile and silently blank that column.
    #[test]
    fn set_diagram_shape_changes_only_the_shape() {
        let path = temp_db_path_for_test("set_shape");
        let db_arc = open_temp_db(&path);
        let db = db_arc.lock().expect("lock temp db");

        let entry = pleochroma_vault::model::entry::FacetingDiagramEntry {
            title: "Shape edit probe".to_string(),
            url: "local://shape_probe.asc".to_string(),
            design_id: String::new(),
        };
        let detail = FacetingDiagramDetail {
            refractive_index: Some("1.762".to_string()),
            index_gear: Some("96".to_string()),
            facets_count: Some("57".to_string()),
            symmetry_order: Some("8".to_string()),
            mirror_symmetry: Some(true),
            lw_ratio: Some("1.234".to_string()),
            hw_ratio: Some("0.617".to_string()),
            cw_ratio: Some("0.145".to_string()),
            pw_ratio: Some("0.431".to_string()),
            volume: Some("0.187".to_string()),
            designer_info: Some("Somebody; Some Journal".to_string()),
            shape: Some("Oval".to_string()),
            ..FacetingDiagramDetail::default()
        };
        let entry_id = db
            .save_diagram_entry(&entry, pleochroma_vault::local::LOCAL_SOURCE_ID)
            .expect("save entry");
        db.save_diagram_detail(&detail, entry_id)
            .expect("save detail");

        set_diagram_shape(&db, entry_id, "Cushion").expect("set shape");

        let after = db
            .get_diagram_full(entry_id)
            .expect("read back")
            .expect("row exists");
        assert_eq!(after.shape.as_deref(), Some("Cushion"), "shape must change");
        // Every other field must be unchanged.
        assert_eq!(after.refractive_index.as_deref(), Some("1.762"));
        assert_eq!(after.index_gear.as_deref(), Some("96"));
        assert_eq!(after.facets_count.as_deref(), Some("57"));
        assert_eq!(after.symmetry_order.as_deref(), Some("8"));
        assert_eq!(after.mirror_symmetry, Some(true));
        assert_eq!(after.lw_ratio.as_deref(), Some("1.234"));
        assert_eq!(after.hw_ratio.as_deref(), Some("0.617"));
        assert_eq!(after.cw_ratio.as_deref(), Some("0.145"));
        assert_eq!(after.pw_ratio.as_deref(), Some("0.431"));
        assert_eq!(after.volume.as_deref(), Some("0.187"));
        assert_eq!(
            after.designer_info.as_deref(),
            Some("Somebody; Some Journal")
        );

        drop(db);
        drop(db_arc);
        std::fs::remove_file(&path).ok();
    }

    /// A shape edit must not change a stored double, not even its last digit.
    ///
    /// Text with only 15 significant digits would turn `0.30000000000000004` into `0.3`.
    #[test]
    fn set_diagram_shape_keeps_every_digit_of_a_stored_double() {
        let path = temp_db_path_for_test("set_shape_digits");
        let db_arc = open_temp_db(&path);
        let db = db_arc.lock().expect("lock temp db");

        let entry = pleochroma_vault::model::entry::FacetingDiagramEntry {
            title: "Digits probe".to_string(),
            url: "local://digits_probe.asc".to_string(),
            design_id: String::new(),
        };
        let entry_id = db
            .save_diagram_entry(&entry, pleochroma_vault::local::LOCAL_SOURCE_ID)
            .expect("save entry");
        db.save_diagram_detail(&FacetingDiagramDetail::default(), entry_id)
            .expect("save detail");
        // Both need 17 significant digits: 0.1 + 0.2, and 1 + 2^-52.
        let update = MetadataUpdate {
            shape: Some("Oval".to_string()),
            lw_ratio: Some("0.30000000000000004".to_string()),
            refractive_index: Some("1.0000000000000002".to_string()),
            ..MetadataUpdate::default()
        };
        db.update_diagram_metadata(entry_id, &update)
            .expect("store the doubles");

        let before = db
            .get_diagram_full(entry_id)
            .expect("read back")
            .expect("row exists");
        assert_eq!(before.lw_ratio.as_deref(), Some("0.30000000000000004"));
        assert_eq!(
            before.refractive_index.as_deref(),
            Some("1.0000000000000002")
        );

        set_diagram_shape(&db, entry_id, "Cushion").expect("set shape");

        let after = db
            .get_diagram_full(entry_id)
            .expect("read back")
            .expect("row exists");
        assert_eq!(after.shape.as_deref(), Some("Cushion"));
        assert_eq!(after.lw_ratio, before.lw_ratio);
        assert_eq!(after.refractive_index, before.refractive_index);

        drop(db);
        drop(db_arc);
        std::fs::remove_file(&path).ok();
    }

    /// The context menu's delete question names the design, and says so plainly when
    /// the row has no title.
    #[test]
    fn the_delete_question_names_the_design_and_warns_it_cannot_be_undone() {
        let named = delete_question("  Emerald Estuary 21 ");
        assert!(named.starts_with("\u{201c}Emerald Estuary 21\u{201d} and the files"));
        assert!(named.ends_with("This cannot be undone."));
        assert!(delete_question("").starts_with("This design and the files"));
    }
}
