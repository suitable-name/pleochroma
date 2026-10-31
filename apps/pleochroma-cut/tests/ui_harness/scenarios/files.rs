//! Scenarios: files -- Save As and reopen, a cancelled picker, `.asc` import, the Export
//! dialog and the startup restore prompt.
//!
//! File pickers never open: every scenario queues the picker's answer first
//! (`harness_support::queue_picker_path` / `queue_picker_cancel`) and checks afterwards that the
//! queue was consumed. Files are written only below `ctx.scratch_file(..)` and the profile's
//! settings directory.

use super::{
    Scenario,
    editor::{
        Card, create_template, duplicate_first_tier, show_edit_tab, tier_count, tier_signature,
    },
    existing_install,
    main_window::{select_render_view, select_top_tab},
};
use crate::support::{
    Ctx, HarnessApp, ProfileExt,
    app::{pump, settle},
    sample_designs,
};
use pleochroma_cut::{EditorModel, ExportModel, LibraryModel, harness_support};
use pleochroma_formats::native::design::{
    DESIGN_EXTENSION, FileKind, detect_kind, detect_kind_of_path,
};
use slint::{ComponentHandle, Model};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

/// File names directly inside `dir` (no recursion), empty when it cannot be read.
fn names_in(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Queues `path` for the next picker, runs Save As and waits until the file exists. Records a
/// finding and returns `None` when it never appears.
fn save_as(ctx: &mut Ctx<'_>, file_name: &str) -> Option<PathBuf> {
    let path = ctx.scratch_file(file_name);
    harness_support::queue_picker_path(path.clone());
    ctx.app.ui().global::<EditorModel>().invoke_save_native_as();
    let target = path.clone();
    let written = ctx.wait_for(
        &format!("Save As writes {file_name}"),
        Duration::from_secs(15),
        move || target.exists(),
    );
    // The file appears before the save is finished: the catalogue write-back still runs on the
    // writer thread and the UI tail (`finish_save_native_success`: dirty flag, saved path) lands
    // afterwards. Wait for that tail, or the next step replaces the design under a save that is
    // still in flight and the late tail marks the wrong design.
    if written {
        let ui = ctx.app.ui().clone_strong();
        let wanted = path.display().to_string();
        ctx.wait_for(
            &format!("Save As of {file_name} finishes"),
            Duration::from_secs(15),
            move || ui.global::<EditorModel>().get_last_saved_path().as_str() == wanted,
        );
    }
    pump(300);
    if written {
        crate::support::coverage::record_callback("EditorModel.save_native_as");
    }
    written.then_some(path)
}

/// Expects `path` to hold a design file: its header says so and its name says so.
fn expect_design_file(ctx: &mut Ctx<'_>, path: &Path) {
    let bytes = fs::read(path).unwrap_or_default();
    ctx.expect(
        !bytes.is_empty() && detect_kind(&bytes) == FileKind::Design,
        "saved-file-header",
        format!("{} does not start with the design header", path.display()),
    );
    ctx.expect(
        detect_kind_of_path(path) == FileKind::Design,
        "saved-file-name",
        format!("{} is not named like a design file", path.display()),
    );
}

/// The scenarios of this file.
pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "save_psdd_and_reopen",
            seed: existing_install,
            run: save_psdd_and_reopen,
        },
        Scenario {
            name: "save_cancelled_changes_nothing",
            seed: existing_install,
            run: save_cancelled_changes_nothing,
        },
        Scenario {
            name: "import_asc",
            seed: existing_install,
            run: import_asc,
        },
        Scenario {
            name: "export_dialog_opens",
            seed: existing_install,
            run: export_dialog_opens,
        },
        Scenario {
            name: "startup_restore_prompt",
            seed: existing_install,
            run: startup_restore_prompt,
        },
    ]
}

/// Template design -> Save As `a.psdd` -> a different design -> Open `a.psdd`. Proves the file is
/// a design file, and that reopening restores the same tier table and title.
fn save_psdd_and_reopen(ctx: &mut Ctx<'_>) {
    show_edit_tab(ctx.app);
    let created = create_template(ctx.app, Card::First);
    if !ctx.expect(
        created,
        "template-design",
        "no design could be created from a template",
    ) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    let tiers = tier_count(&ui);
    let signature = tier_signature(&ui);
    let title = editor.get_design_title().to_string();
    let Some(path) = save_as(ctx, &format!("a.{DESIGN_EXTENSION}")) else {
        return;
    };
    expect_design_file(ctx, &path);
    ctx.expect(
        harness_support::pending_picker_answers() == 0,
        "picker-consumed",
        "Save As did not ask the file picker",
    );
    ctx.expect(
        !editor.get_is_dirty(),
        "clean-after-save",
        "the design is still dirty after Save As",
    );
    ctx.check_health("Save As");

    // Replace the design, then open the file.
    let replaced = create_template(ctx.app, Card::Last);
    if !replaced && editor.get_unsaved_dialog_open() {
        editor.invoke_unsaved_dialog_discard();
        pump(300);
    }
    if tier_signature(&ui) == signature {
        ctx.report
            .note("save_psdd_and_reopen: the replacement design equals the saved one".to_owned());
    }
    harness_support::queue_picker_path(path.clone());
    editor.invoke_open_native();
    pump(150);
    // The replacement design is unsaved, so Open first asks whether to discard it
    // (`setup_open_native_callback`); the picker is asked only after the answer.
    if editor.get_unsaved_dialog_open() {
        editor.invoke_unsaved_dialog_discard();
        pump(150);
    }
    if ctx.wait_for(
        "Open restores the saved tier table",
        Duration::from_secs(15),
        || tier_signature(&ui) == signature,
    ) {
        crate::support::coverage::record_callback("EditorModel.open_native");
    }
    pump(300);
    ctx.expect(
        tier_count(&ui) == tiers,
        "reopen-tier-count",
        format!("{} tier(s) after Open, saved {tiers}", tier_count(&ui)),
    );
    ctx.expect(
        editor.get_design_title().as_str() == title,
        "reopen-title",
        format!(
            "title {:?} after Open, saved {title:?}",
            editor.get_design_title()
        ),
    );
    ctx.expect(
        harness_support::pending_picker_answers() == 0,
        "picker-consumed",
        "Open did not ask the file picker",
    );
    ctx.check_health("Open");
    ctx.sweep_main_at_sizes("Edit tab after reopening a saved design");
}

/// A cancelled Save As leaves the design dirty, writes no file and raises no error.
fn save_cancelled_changes_nothing(ctx: &mut Ctx<'_>) {
    show_edit_tab(ctx.app);
    let created = create_template(ctx.app, Card::First);
    if !ctx.expect(
        created,
        "template-design",
        "no design could be created from a template",
    ) {
        return;
    }
    duplicate_first_tier(ctx.app);
    pump(300);
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    ctx.expect(
        editor.get_is_dirty(),
        "dirty-before-save",
        "the design is not dirty after adding a tier",
    );
    let probe = ctx.scratch_file(&format!("cancelled.{DESIGN_EXTENSION}"));
    // The scratch directory is shared by the scenarios of the run: only files that appear after
    // the cancelled Save As count (an earlier scenario leaves `a.psdd` there).
    let before = probe.parent().map_or_else(Vec::new, names_in);
    harness_support::queue_picker_cancel();
    editor.invoke_save_native_as();
    // Was a fixed `settle(800)`: wait until the picker has consumed the cancel, then keep a short
    // fixed pause because "nothing is written" cannot be observed.
    let _ = crate::support::app::wait_until(Duration::from_millis(800), || {
        harness_support::pending_picker_answers() == 0
    });
    settle(200);
    ctx.expect(
        harness_support::pending_picker_answers() == 0,
        "picker-consumed",
        "Save As did not ask the file picker",
    );
    let suffix = format!(".{DESIGN_EXTENSION}");
    let written = probe.parent().map_or_else(Vec::new, |dir| {
        names_in(dir)
            .into_iter()
            .filter(|n| n.ends_with(&suffix) && !before.contains(n))
            .collect::<Vec<_>>()
    });
    ctx.expect(
        written.is_empty(),
        "cancelled-save-wrote",
        format!("a cancelled Save As left files behind: {written:?}"),
    );
    ctx.expect(
        editor.get_is_dirty() && editor.get_last_saved_path().is_empty(),
        "cancelled-save-state",
        "a cancelled Save As changed the dirty flag or the last saved path",
    );
    ctx.check_health("cancelled Save As");
}

/// Import of an `.asc` fixture through the picker. Proves a new library row appears.
fn import_asc(ctx: &mut Ctx<'_>) {
    let designs = sample_designs();
    let Some(first) = designs.first() else {
        ctx.expect(false, "fixtures", "there are no fixture designs");
        return;
    };
    let source = ctx.scratch_file(first.file_name);
    if let Err(e) = fs::write(&source, &first.asc) {
        ctx.expect(
            false,
            "fixtures",
            format!("cannot write the .asc fixture: {e}"),
        );
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let library = ui.global::<LibraryModel>();
    // The start-up list load lands on a worker thread: count the rows only once it has settled
    // (two reads 500 ms of real time apart agree), or the import is compared with a partial list.
    // The start-up load first sets "Database loaded: N ..." and then lists the rows on a second
    // worker: wait for both, so `before` is the library as it stands, not an empty first read.
    let _ = crate::support::app::wait_until(Duration::from_secs(15), || {
        let status = library.get_status_message();
        let listed = status
            .strip_prefix("Database loaded: ")
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|n| n.parse::<usize>().ok());
        listed.is_some_and(|n| library.get_diagram_list().row_count() == n)
    });
    let mut before = library.get_diagram_list().row_count();
    for _ in 0..20 {
        // Was `settle(500)` per round; the status/rows condition above already holds, so a
        // second read 100 ms later is enough to see a straggler.
        settle(100);
        pump(50);
        let now = library.get_diagram_list().row_count();
        if now == before {
            break;
        }
        before = now;
    }
    harness_support::queue_picker_path(source);
    library.invoke_pick_asc_file();
    if ctx.wait_for(
        "the imported design appears in the library",
        Duration::from_secs(15),
        || library.get_diagram_list().row_count() > before,
    ) {
        crate::support::coverage::record_callback("LibraryModel.pick_asc_file");
    }
    pump(300);
    ctx.expect(
        library.get_diagram_list().row_count() == before + 1,
        "import-row",
        format!(
            "{} row(s) after the import, {before} before",
            library.get_diagram_list().row_count()
        ),
    );
    ctx.expect(
        harness_support::pending_picker_answers() == 0,
        "picker-consumed",
        "Import did not ask the file picker",
    );
    ctx.check_health("import .asc");
    ctx.sweep_main_at_sizes("library after importing an .asc");
}

/// Opens the Export dialog on the Live Render view, sweeps it and closes it. No export is run.
fn export_dialog_opens(ctx: &mut Ctx<'_>) {
    select_top_tab(ctx.app, 0);
    select_render_view(ctx.app, 0);
    let ui = ctx.app.ui().clone_strong();
    let export = ui.global::<ExportModel>();
    let before = ctx.app.main_view().elements().len();
    export.set_is_open(true);
    pump(500);
    let after = ctx.app.main_view().elements().len();
    ctx.expect(
        after > before,
        "export-dialog-drawn",
        format!("opening the Export dialog added no elements ({before} -> {after})"),
    );
    ctx.check_health("Export dialog open");
    ctx.sweep_main_at_sizes("Export dialog");
    export.set_is_open(false);
    pump(300);
    let remaining = ctx.app.main_view().elements().len();
    ctx.expect(
        remaining < after,
        "export-dialog-closed",
        "closing the Export dialog removed no elements",
    );
    ctx.check_health("Export dialog closed");
}

/// Relaunches the app with a leftover autosave in the profile's settings directory (written
/// after the app data wipe a plain relaunch makes). Returns the autosave's path.
fn relaunch_with_leftover_autosave(ctx: &mut Ctx<'_>) -> Option<PathBuf> {
    ctx.app.shutdown();
    ctx.profile.wipe_app_data();
    ctx.profile.seed_settings(&ctx.seed);
    let dir = harness_support::settings_path()
        .parent()
        .map(Path::to_path_buf)?;
    let leftover = dir.join(format!("untitled.autosave.{DESIGN_EXTENSION}"));
    if !ctx.profile.contains(&leftover) {
        ctx.expect(
            false,
            "profile-isolation",
            format!(
                "the autosave path {} is outside the profile",
                leftover.display()
            ),
        );
        return None;
    }
    let written = fs::create_dir_all(&dir)
        .and_then(|()| fs::write(&leftover, "format = \"pleochroma-design\"\nversion = 1\n"));
    if let Err(e) = written {
        ctx.expect(false, "fixtures", format!("cannot write the autosave: {e}"));
        return None;
    }
    harness_support::clear_picker_answers();
    match HarnessApp::launch() {
        Ok(app) => *ctx.app = app,
        Err(e) => {
            ctx.report
                .add_simple("-", "launch", format!("cannot start with an autosave: {e}"));
            return None;
        }
    }
    Some(leftover)
}

/// A leftover autosave raises the "Recover Unsaved Work?" prompt at startup. Proves "Not Now"
/// closes it and keeps the file, and "Delete" closes it and removes the file (what the code
/// promises: the file is the user's unsaved work, only an explicit Delete discards it).
fn startup_restore_prompt(ctx: &mut Ctx<'_>) {
    for (button, delete) in [("Not Now", false), ("Delete", true)] {
        let Some(leftover) = relaunch_with_leftover_autosave(ctx) else {
            return;
        };
        let ui = ctx.app.ui().clone_strong();
        let editor = ui.global::<EditorModel>();
        let offered = ctx.wait_for(
            "the startup restore prompt appears",
            Duration::from_secs(5),
            || !editor.get_startup_restore_path().is_empty(),
        );
        if !offered {
            continue;
        }
        ctx.expect(
            editor.get_startup_restore_is_autosave(),
            "restore-is-autosave",
            "the prompt does not know the leftover file is an autosave",
        );
        let name = leftover
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        ctx.expect(
            editor.get_startup_restore_path().contains(name.as_str()),
            "restore-path",
            format!(
                "the prompt offers {:?}, expected a path ending in {name}",
                editor.get_startup_restore_path()
            ),
        );
        if !delete {
            ctx.sweep_main_at_sizes("startup restore prompt");
        }
        // The last matching button is the modal's: `click` takes the first, which may be a
        // control behind the prompt.
        if super::dialogs::click_button(&ctx.app.main_view(), &[button]).is_none() {
            // GAP: the prompt's button is not reachable by label; its callback is driven.
            if delete {
                editor.invoke_startup_restore_delete();
            } else {
                editor.invoke_startup_restore_dismiss();
            }
        }
        pump(300);
        ctx.expect(
            editor.get_startup_restore_path().is_empty(),
            "restore-prompt-closes",
            format!("the prompt is still showing after {button}"),
        );
        ctx.expect(
            leftover.exists() != delete,
            "restore-file",
            format!(
                "after {button} the autosave exists: {} (expected {})",
                leftover.exists(),
                !delete
            ),
        );
        ctx.check_health(&format!("startup restore: {button}"));
    }
}
