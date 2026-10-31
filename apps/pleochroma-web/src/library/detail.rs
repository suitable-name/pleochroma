//! The detail pane of the selected design: its fields, Save Changes, Open, Export `.psdd`
//! and Delete.

use super::{THUMBS, forget_thumb, notify, rebuild, reload, set_busy, with_state};
use crate::{
    LibraryModel,
    app::{
        Ctx,
        persist::schedule_save,
        push::{MessageKind, push_all},
        solve::auto_solve,
    },
    io::{
        download::{MIME_DESIGN, download_bytes},
        load::{self, Opened},
    },
    storage::library,
};
use pleochroma_editor::{files::InputFileKind, guide::viewing_events as events};
use pleochroma_web_core::{
    design_meta::iso8601_utc_from_epoch_ms,
    library_record::LibraryEntry,
    library_view::{
        DetailFields, apply_detail_edit, detail_fields, detail_info, export_psdd, open_candidate,
    },
};
use slint::ComponentHandle;

/// The selected entry, if there is one.
fn selected_entry() -> Option<LibraryEntry> {
    with_state(|state| {
        let id = state.selected.as_ref()?;
        state.entries.iter().find(|e| &e.design_id == id).cloned()
    })
}

/// Fills the pane from the selected entry (or hides it when nothing is selected), dropping any
/// edit that was not saved.
pub(super) fn push_selection(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<LibraryModel>();
    let Some(entry) = selected_entry() else {
        model.set_has_selection(false);
        model.set_detail_dirty(false);
        model.set_detail_error("".into());
        return;
    };
    let fields = detail_fields(&entry);
    model.set_detail_title(fields.title.into());
    model.set_detail_designer(fields.designer.into());
    model.set_detail_citation(fields.citation.into());
    model.set_detail_shape(fields.shape.into());
    model.set_detail_ri(fields.ri.into());
    model.set_detail_lw(fields.lw.into());
    model.set_detail_volume(fields.volume.into());
    model.set_detail_tags(fields.tags.into());
    model.set_detail_ignored(fields.ignored);
    model.set_detail_info(detail_info(&entry).into());
    model.set_detail_dirty(false);
    model.set_detail_error("".into());
    model.set_has_selection(true);
    push_thumb(ctx);
}

/// Shows the selected design's cached picture in the pane.
pub(super) fn push_thumb(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let Some(id) = with_state(|state| state.selected.clone()) else {
        return;
    };
    let thumb = THUMBS.with_borrow(|cache| cache.get(&id).cloned().flatten());
    let model = ui.global::<LibraryModel>();
    model.set_detail_has_thumb(thumb.is_some());
    model.set_detail_thumb(thumb.unwrap_or_default());
}

/// The fields as the pane's text boxes hold them now.
fn read_fields(model: &LibraryModel<'_>) -> DetailFields {
    DetailFields {
        title: model.get_detail_title().to_string(),
        designer: model.get_detail_designer().to_string(),
        citation: model.get_detail_citation().to_string(),
        shape: model.get_detail_shape().to_string(),
        ri: model.get_detail_ri().to_string(),
        lw: model.get_detail_lw().to_string(),
        volume: model.get_detail_volume().to_string(),
        tags: model.get_detail_tags().to_string(),
        ignored: model.get_detail_ignored(),
    }
}

/// Save Changes: validates the fields, then writes the edited entry.
pub(super) fn apply_detail(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<LibraryModel>();
    let Some(mut trial) = selected_entry() else {
        return;
    };
    let fields = read_fields(&model);
    if let Err(message) = apply_detail_edit(&mut trial, &fields) {
        model.set_detail_error(message.into());
        return;
    }
    model.set_detail_error("".into());
    let id = trial.design_id.clone();
    set_busy(ctx, Some("Saving changes..."));
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let written = library::update_entry(&id, move |entry| {
            // Checked above on a copy; the same fields give the same result here.
            let _ = apply_detail_edit(entry, &fields);
        })
        .await;
        match written {
            Ok(Some(updated)) => {
                let title = updated.item.title.clone();
                with_state(|state| {
                    if let Some(slot) = state
                        .entries
                        .iter_mut()
                        .find(|e| e.design_id == updated.design_id)
                    {
                        *slot = updated;
                    }
                });
                notify(
                    &ctx,
                    MessageKind::Success,
                    &format!("Saved the changes to \"{title}\"."),
                );
            }
            Ok(None) => notify(
                &ctx,
                MessageKind::Error,
                "That design is no longer in the library.",
            ),
            Err(error) => notify(&ctx, MessageKind::Error, &error.user_text()),
        }
        rebuild(&ctx);
        set_busy(&ctx, None);
    });
}

/// Installs the stored design `name` (read from the library entry `design_id`) as the open
/// design, after the unsaved-changes guard, and closes the library.
fn open_stored(ctx: &Ctx, name: String, bytes: Vec<u8>, design_id: String) {
    let c = ctx.clone();
    crate::editor::unsaved::confirm_discard(ctx, move || {
        install(&c, &name, &bytes, &design_id);
    });
}

/// [`open_stored`]'s second half, once nothing unsaved is at stake.
fn install(ctx: &Ctx, name: &str, bytes: &[u8], design_id: &str) {
    let loaded: Result<Opened, String> = {
        let mut app = ctx.state.borrow_mut();
        match InputFileKind::classify(name, bytes) {
            Some(InputFileKind::Design) => std::str::from_utf8(bytes)
                .map_err(|_| format!("\"{name}\" is not a text (UTF-8) design file."))
                .and_then(|text| load::load_design_file(&mut app, name, text)),
            Some(InputFileKind::Asc) => load::load_asc(name, bytes),
            Some(kind @ (InputFileKind::Gem | InputFileKind::Gcs)) => {
                load::load_converted(name, kind, bytes)
            }
            _ => Err(format!("\"{name}\" is not a design file.")),
        }
    };
    let Opened {
        mut design,
        notes,
        needs_attention,
    } = match loaded {
        Ok(opened) => opened,
        Err(message) => {
            notify(ctx, MessageKind::Error, &message);
            return;
        }
    };
    // The design keeps its library identity: a later Save to Library updates this entry
    // instead of adding a second one.
    if design.metadata.id.is_empty() {
        design_id.clone_into(&mut design.metadata.id);
    }
    ctx.state.borrow_mut().replace_design(design);
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<LibraryModel>().set_open(false);
        push_all(&ui, &ctx.state.borrow());
    }
    auto_solve(ctx);
    schedule_save(ctx);
    // A tutorial step may wait for a design to be loaded (once per Open Design). The event
    // also re-checks the step, so no separate progress check follows.
    crate::editor::guide::raise(ctx, events::DESIGN_REPLACED);
    let text = if notes.is_empty() {
        format!("Opened {name} from the library.")
    } else {
        format!("Opened {name} from the library. {}", notes.join(" "))
    };
    let kind = if needs_attention {
        MessageKind::Warning
    } else {
        MessageKind::Success
    };
    notify(ctx, kind, &text);
}

/// Open Design: reads the selected entry's design file and opens it.
pub(super) fn open_selected(ctx: &Ctx) {
    let Some(id) = with_state(|state| state.selected.clone()) else {
        return;
    };
    set_busy(ctx, Some("Opening the design..."));
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let record = library::get_design(&id).await;
        set_busy(&ctx, None);
        match record {
            Ok(Some(record)) => match open_candidate(&record.files) {
                Some(file) => open_stored(
                    &ctx,
                    file.name.clone(),
                    file.content.clone(),
                    record.entry.design_id.clone(),
                ),
                None => notify(
                    &ctx,
                    MessageKind::Error,
                    "This entry holds no design file to open.",
                ),
            },
            Ok(None) => notify(
                &ctx,
                MessageKind::Error,
                "That design is no longer in the library.",
            ),
            Err(error) => notify(&ctx, MessageKind::Error, &error.user_text()),
        }
    });
}

/// Export `.psdd`: downloads the selected design as a design file with the library's metadata.
pub(super) fn export_selected(ctx: &Ctx) {
    let Some(id) = with_state(|state| state.selected.clone()) else {
        return;
    };
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let record = match library::get_design(&id).await {
            Ok(Some(record)) => record,
            Ok(None) => {
                notify(
                    &ctx,
                    MessageKind::Error,
                    "That design is no longer in the library.",
                );
                return;
            }
            Err(error) => {
                notify(&ctx, MessageKind::Error, &error.user_text());
                return;
            }
        };
        let now = iso8601_utc_from_epoch_ms(js_sys::Date::now());
        match export_psdd(&record.entry, &record.files, &now) {
            Ok(exported) => {
                match download_bytes(&exported.file_name, MIME_DESIGN, exported.text.as_bytes()) {
                    Ok(()) => notify(
                        &ctx,
                        MessageKind::Success,
                        &format!("Downloaded {}.", exported.file_name),
                    ),
                    Err(message) => notify(&ctx, MessageKind::Error, &message),
                }
            }
            Err(message) => notify(&ctx, MessageKind::Error, &message),
        }
    });
}

/// Delete Design: asks, then removes the entry with its files, picture and side data.
pub(super) fn delete_selected(ctx: &Ctx) {
    let Some(entry) = selected_entry() else {
        return;
    };
    let c = ctx.clone();
    crate::editor::unsaved::confirm_action(
        ctx,
        "Delete Design",
        &format!(
            "Delete \"{}\" from the library? The design file stored here is removed; copies you \
             downloaded are not affected.",
            entry.item.title
        ),
        "Delete",
        move || {
            let ctx = c.clone();
            let id = entry.design_id.clone();
            let title = entry.item.title;
            set_busy(&ctx, Some("Deleting..."));
            wasm_bindgen_futures::spawn_local(async move {
                match library::delete_design(&id).await {
                    Ok(()) => {
                        forget_thumb(&id);
                        with_state(|state| {
                            if state.selected.as_deref() == Some(id.as_str()) {
                                state.selected = None;
                            }
                        });
                        notify(
                            &ctx,
                            MessageKind::Success,
                            &format!("Deleted \"{title}\" from the library."),
                        );
                        reload(&ctx).await;
                    }
                    Err(error) => {
                        notify(&ctx, MessageKind::Error, &error.user_text());
                        set_busy(&ctx, None);
                    }
                }
            });
        },
    );
}
