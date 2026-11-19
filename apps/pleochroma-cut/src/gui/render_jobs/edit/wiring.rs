//! The callbacks of the edit dialog and of the rows' Edit and Duplicate, and the dialog's session.
//!
//! One edit is open at a time. Its seed (the job as loaded and the dialog's first values) lives in
//! a thread-local; every control move recomputes the footer, the notes and `can_save` from it.
//! No borrow of the session is held across a save, a picker or a measurement.

use super::{
    footer::{FooterFacts, first_picture_field, footer_text},
    load::{self, load_job_for_edit},
    material::{material_edit, material_names, resolve_job_material},
    naming::{JobKindTag, name_from_template, reserve_excluding},
    save::{
        EditDeps, Requeue, SaveOutcome, deps, insert_job_after, install_deps, prepared_job,
        save_edit,
    },
    seed::{
        Controls, CurveKey, MeasuredCurves, Seed, curve_key, edit_from_controls, effective_axes,
        publish_notes, publish_static, read_controls, write_controls,
    },
    video_curves::spawn_measure,
};
use crate::{
    JobEditModel, MainWindow, RenderJobsModel,
    bridge::render_thread::RenderContext,
    gui::{
        pickers::{PickerFilter, PickerKind, PickerRequest, pick},
        render_jobs::controller::is_running,
        show_toast,
        tilt::video_export::params::pose_count,
    },
    settings::SettingsPersister,
};
use pleochroma_net::messages::{content_hash, hash_hex};
use pleochroma_render_jobs::{
    JobKind, RenderJobFile,
    edit::{JobEdit, edit_rule},
    job::OverlaySelection,
    state::JobState,
};
use pleochroma_vault::db::sqlite::{Database, ExpectedRow};
use slint::{ComponentHandle as _, Weak};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

thread_local! {
    /// The edit that is open (UI thread only).
    static SESSION: RefCell<Option<Seed>> = const { RefCell::new(None) };
}

/// The dialog's status as one refresh computed it.
struct Status {
    footer: String,
    can_save: bool,
    measuring: bool,
    message: String,
    refused: bool,
}

/// A curve measurement a refresh asks for: the job to measure, its axes, and what it is for.
struct MeasureRequest {
    id: i64,
    basis: RenderJobFile,
    axes: Vec<u32>,
    key: CurveKey,
}

/// Registers every `JobEditModel` callback and the rows' `edit_job` and `duplicate_job`.
pub(in crate::gui) fn setup(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    load::install_db(db);
    install_deps(EditDeps {
        db: Arc::clone(db),
        render_ctx: Arc::clone(render_ctx),
        settings_store: Arc::clone(settings_store),
    });
    let model = ui.global::<JobEditModel>();
    model.on_edited(with_ui(ui, refresh));
    model.on_browse_folder(with_ui(ui, browse_folder));
    let weak = ui.as_weak();
    model.on_save(move |as_new| {
        if let Some(ui) = weak.upgrade() {
            do_save(&ui, as_new, Requeue::NotAsked);
        }
    });
    model.on_cancel(with_ui(ui, close));
    model.on_rename_from_template(with_ui(ui, rename_from_template));
    model.on_locate_hdr(with_ui(ui, locate_hdr));
    let weak = ui.as_weak();
    model.on_material_chosen(move |name| {
        if let Some(ui) = weak.upgrade() {
            choose_material(&ui, name.as_str());
        }
    });
    model.on_load_again(with_ui(ui, load_again));
    let weak = ui.as_weak();
    model.on_requeue_answer(move |answer| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        match answer {
            1 => do_save(&ui, false, Requeue::SaveOnly),
            2 => do_save(&ui, false, Requeue::RenderAgain),
            _ => {}
        }
    });

    let rows = ui.global::<RenderJobsModel>();
    let weak = ui.as_weak();
    rows.on_edit_job(move |id| {
        if let Some(ui) = weak.upgrade() {
            open_dialog(&ui, i64::from(id));
        }
    });
    let weak = ui.as_weak();
    rows.on_duplicate_job(move |id| {
        if let Some(ui) = weak.upgrade() {
            duplicate(&ui, i64::from(id));
        }
    });
}

/// A callback without arguments that runs `f` on the window.
fn with_ui(ui: &MainWindow, f: fn(&MainWindow)) -> impl Fn() + 'static {
    let weak: Weak<MainWindow> = ui.as_weak();
    move || {
        if let Some(ui) = weak.upgrade() {
            f(&ui);
        }
    }
}

// ---- Opening and closing ---------------------------------------------------------------

/// Opens the dialog on job `id`. Returns whether it opened; a refusal shows a toast.
fn open_dialog(ui: &MainWindow, id: i64) -> bool {
    if id <= 0 {
        return false;
    }
    let Some(deps) = deps() else {
        return false;
    };
    let (meta, job) = match load_job_for_edit(ui, id) {
        Ok(loaded) => loaded,
        Err(sentence) => {
            show_toast(ui, &sentence, "error");
            return false;
        }
    };
    let Some(state) = JobState::parse(&meta.state) else {
        show_toast(
            ui,
            "This job has a state this version does not know.",
            "info",
        );
        return false;
    };
    if let Err(sentence) = edit_rule(state) {
        show_toast(ui, sentence, "info");
        return false;
    }
    if is_running(id) {
        show_toast(ui, "This job is rendering. Pause it first.", "info");
        return false;
    }
    let mut materials = {
        let ctx = RenderContext::lock(&deps.render_ctx);
        material_names(&ctx)
    };
    if !materials.contains(&job.scene.material.name) {
        materials.insert(0, job.scene.material.name.clone());
    }
    let expected = ExpectedRow {
        state: meta.state.clone(),
        frames_done: i64::from(meta.frames_done),
    };
    let seed = Seed::new(id, expected, state, job, materials);
    write_controls(ui, &seed.controls);
    publish_static(ui, &seed);
    SESSION.with(|cell| *cell.borrow_mut() = Some(seed));
    refresh(ui);
    ui.global::<JobEditModel>().set_open(true);
    true
}

/// Closes the dialog and forgets the session.
fn close(ui: &MainWindow) {
    let old = SESSION.with(|cell| cell.borrow_mut().take());
    drop(old);
    let model = ui.global::<JobEditModel>();
    model.set_requeue_question_open(false);
    model.set_open(false);
}

/// "Load the Job Again": drops the edits and seeds the dialog from the row again.
fn load_again(ui: &MainWindow) {
    let id = SESSION.with(|cell| cell.borrow().as_ref().map(|seed| seed.id));
    let Some(id) = id else {
        return;
    };
    if !open_dialog(ui, id) {
        close(ui);
    }
}

/// Duplicate: a queued copy of job `id` right after it, with its own output.
fn duplicate(ui: &MainWindow, id: i64) {
    if id <= 0 {
        return;
    }
    let job = match load_job_for_edit(ui, id) {
        Ok((_, job)) => job,
        Err(sentence) => {
            show_toast(ui, &sentence, "error");
            return;
        }
    };
    match insert_job_after(ui, id, job) {
        Ok(new_id) => show_toast(
            ui,
            &format!("Job {id} duplicated as job {new_id}."),
            "success",
        ),
        Err(sentence) => show_toast(ui, &sentence, "error"),
    }
}

// ---- Keeping the dialog in step with the controls ----------------------------------------

/// Resolves the material the combo now shows, once per change.
fn sync_material(seed: &mut Seed, now: &Controls) {
    if now.material_index == seed.material_index {
        return;
    }
    seed.material_index = now.material_index;
    seed.material_blocked = false;
    seed.message.clear();
    if now.material_index == seed.controls.material_index {
        seed.material_edit = None;
        seed.material_note.clear();
        return;
    }
    let Some(name) = usize::try_from(now.material_index)
        .ok()
        .and_then(|at| seed.materials.get(at))
        .cloned()
    else {
        return;
    };
    let Some(deps) = deps() else {
        return;
    };
    let resolved = {
        let ctx = RenderContext::lock(&deps.render_ctx);
        resolve_job_material(&ctx, &seed.job, &name)
    };
    match resolved {
        Ok(change) => {
            seed.material_note = if seed.job.engine.is_ray() {
                "A Ray-Traced job has no color zones; its effects stay as they are.".to_string()
            } else {
                "Haze, crystal axes, inclusions and color zones are reset to the new material."
                    .to_string()
            };
            seed.material_edit = Some(material_edit(change));
        }
        Err(sentence) => {
            seed.material_edit = None;
            seed.material_note.clone_from(&sentence);
            seed.message = sentence;
            seed.material_blocked = true;
        }
    }
}

/// The frames the video has after the edit; 1 for a still.
fn frames_after(seed: &Seed, now: &Controls, edit: &JobEdit) -> u32 {
    let JobKind::TiltVideo(video) = &seed.job.kind else {
        return 1;
    };
    if edit.sweep.is_none() && edit.axes.is_none() {
        return video.total_frames;
    }
    let (start, end, step) = edit
        .sweep
        .unwrap_or((video.start_deg, video.end_deg, video.step_deg));
    let axes: Vec<usize> = effective_axes(seed, now)
        .into_iter()
        .map(|axis| axis as usize)
        .collect();
    u32::try_from(pose_count(&axes, start, end, step)).unwrap_or(u32::MAX)
}

/// The curve measurement this edit needs, if it has not been made or asked for yet.
fn measurement_for(
    seed: &mut Seed,
    now: &Controls,
    edit: &JobEdit,
) -> Result<Option<MeasureRequest>, String> {
    let key = curve_key(seed, now);
    if seed.measuring.as_ref() == Some(&key) {
        return Ok(None);
    }
    if seed.measure_failed.as_ref() == Some(&key) {
        return Err(seed.message.clone());
    }
    let basis_edit = JobEdit {
        material: edit.material.clone(),
        zoning: edit.zoning.clone(),
        effects_material_inclusions: edit.effects_material_inclusions.clone(),
        fluorescence: edit.fluorescence.clone(),
        axes: edit.axes.clone(),
        overlay: Some(OverlaySelection::default()),
        ..JobEdit::default()
    };
    let basis = basis_edit
        .apply(&seed.job)
        .map_err(|refusal| refusal.to_string())?;
    seed.measuring = Some(key.clone());
    Ok(Some(MeasureRequest {
        id: seed.id,
        basis,
        axes: key.axes.clone(),
        key,
    }))
}

/// Works out the footer, the notes' inputs and `can_save` for the dialog as it stands.
fn analyse(seed: &mut Seed, now: &Controls) -> (Status, Option<MeasureRequest>, u32) {
    sync_material(seed, now);
    let built = edit_from_controls(seed, now);
    let (edit, mut error) = match built {
        Ok(edit) => (edit, None),
        Err(sentence) => (JobEdit::default(), Some(sentence)),
    };
    let job = &seed.job;
    let needs_curves = error.is_none() && edit.needs_curves(job) && edit.curves.is_none();
    let mut request = None;
    if needs_curves {
        match measurement_for(seed, now, &edit) {
            Ok(asked) => request = asked,
            Err(sentence) => error = Some(sentence),
        }
    }
    let measuring = needs_curves && error.is_none();
    if error.is_none()
        && !measuring
        && let Err(sentence) = prepared_job(&seed.job, &edit)
    {
        error = Some(sentence);
    }
    let edited = *now != seed.controls || seed.hdr_path.is_some();
    let total_frames = frames_after(seed, now, &edit);
    let facts = FooterFacts {
        state: seed.state,
        is_video: seed.is_video(),
        frames_done: u32::try_from(seed.expected.frames_done).unwrap_or(0),
        edited,
        only_label: edit.only_label_or_name(),
        resets: edit.resets_progress(&seed.job),
        field: first_picture_field(&seed.controls, now, seed.material_edit.is_some()),
        samples_moved: now.sample_exponent != seed.controls.sample_exponent,
        sweep_or_axes: edit.sweep.is_some() || edit.axes.is_some(),
        folder_changed: seed.is_video() && (edit.output.is_some() || edit.video_name.is_some()),
        measuring,
        total_frames,
    };
    let message = seed
        .refused
        .clone()
        .or(error.clone())
        .unwrap_or_else(|| seed.message.clone());
    let status = Status {
        footer: footer_text(&facts),
        can_save: edited && error.is_none() && !measuring && !seed.material_blocked,
        measuring,
        message,
        refused: seed.refused.is_some(),
    };
    (status, request, total_frames)
}

/// Recomputes everything the dialog shows from its controls.
fn refresh(ui: &MainWindow) {
    let availability = crate::gui::remote::current_availability(ui);
    let now = read_controls(ui);
    let analysed = SESSION.with(|cell| {
        let mut slot = cell.borrow_mut();
        let seed = slot.as_mut()?;
        let (status, request, total_frames) = analyse(seed, &now);
        publish_notes(ui, seed, &now, availability, total_frames);
        Some((status, request))
    });
    let Some((status, request)) = analysed else {
        return;
    };
    let model = ui.global::<JobEditModel>();
    model.set_footer_text(status.footer.as_str().into());
    model.set_can_save(status.can_save);
    model.set_measuring_curves(status.measuring);
    model.set_message(status.message.as_str().into());
    model.set_refused(status.refused);
    if let Some(request) = request {
        let MeasureRequest {
            id,
            basis,
            axes,
            key,
        } = request;
        spawn_measure(ui, basis, axes, move |ui, result| {
            on_measured(ui, id, &key, result);
        });
    }
}

/// A curve measurement ended: keeps its result when the dialog still wants it.
fn on_measured(
    ui: &MainWindow,
    id: i64,
    key: &CurveKey,
    result: Result<Vec<pleochroma_render_jobs::job::MetricCurvesData>, String>,
) {
    SESSION.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(seed) = slot.as_mut() else {
            return;
        };
        if seed.id != id || seed.measuring.as_ref() != Some(key) {
            return;
        }
        seed.measuring = None;
        match result {
            Ok(sets) => {
                seed.curves = Some(MeasuredCurves {
                    key: key.clone(),
                    sets,
                });
                seed.measure_failed = None;
            }
            Err(sentence) => {
                seed.measure_failed = Some(key.clone());
                seed.message = format!("The tilt curves could not be measured: {sentence}");
            }
        }
    });
    refresh(ui);
}

/// The material combo picked `name`.
fn choose_material(ui: &MainWindow, name: &str) {
    let index = SESSION.with(|cell| {
        cell.borrow().as_ref().and_then(|seed| {
            seed.materials
                .iter()
                .position(|candidate| candidate == name)
                .and_then(|at| i32::try_from(at).ok())
        })
    });
    if let Some(index) = index {
        ui.global::<JobEditModel>().set_material_index(index);
    }
    refresh(ui);
}

// ---- Pickers and the template ------------------------------------------------------------

/// The output folder picker.
fn browse_folder(ui: &MainWindow) {
    let start = ui.global::<JobEditModel>().get_output_folder().to_string();
    let request = PickerRequest {
        kind: PickerKind::PickFolder,
        title: Some("Output Folder".to_string()),
        filters: Vec::new(),
        default_file_name: None,
        starting_dir: (!start.is_empty()).then(|| PathBuf::from(start)),
    };
    pick(ui, request, |ui, picked| {
        if let Some(folder) = picked {
            let text = folder.to_string_lossy().into_owned();
            ui.global::<JobEditModel>()
                .set_output_folder(text.as_str().into());
            refresh(ui);
        }
    });
}

/// Rename from Template: the file name (or video name) from the current template.
fn rename_from_template(ui: &MainWindow) {
    let now = read_controls(ui);
    let named = SESSION.with(|cell| {
        cell.borrow().as_ref().map(|seed| {
            let edited = edit_from_controls(seed, &now)
                .ok()
                .and_then(|edit| edit.apply(&seed.job).ok());
            (edited.unwrap_or_else(|| seed.job.clone()), seed.is_video())
        })
    });
    let Some((job, is_video)) = named else {
        return;
    };
    match name_from_template(ui, &job) {
        Ok(name) => {
            let model = ui.global::<JobEditModel>();
            if is_video {
                model.set_video_name(name.as_str().into());
            } else {
                model.set_file_name(name.as_str().into());
            }
            refresh(ui);
        }
        Err(sentence) => ui
            .global::<JobEditModel>()
            .set_message(sentence.as_str().into()),
    }
}

/// Locate HDR Map: a new place is accepted only when the file has the job's hash.
fn locate_hdr(ui: &MainWindow) {
    let start = SESSION.with(|cell| {
        cell.borrow().as_ref().and_then(|seed| {
            let base = Path::new(&seed.job.output).parent()?.to_path_buf();
            seed.job
                .hdr_path(&base)
                .and_then(|path| path.parent().map(Path::to_path_buf))
        })
    });
    let request = PickerRequest {
        kind: PickerKind::OpenFile,
        title: Some("Locate HDR Map".to_string()),
        filters: vec![PickerFilter {
            label: "HDR map".to_string(),
            extensions: vec!["hdr".to_string(), "exr".to_string()],
        }],
        default_file_name: None,
        starting_dir: start,
    };
    pick(ui, request, |ui, picked| {
        let Some(path) = picked else {
            return;
        };
        let sentence = accept_hdr(&path);
        if let Some(sentence) = sentence {
            SESSION.with(|cell| {
                if let Some(seed) = cell.borrow_mut().as_mut() {
                    seed.message = sentence;
                }
            });
        }
        refresh(ui);
    });
}

/// Stores `path` as the job's map when its hash matches; otherwise the sentence why not.
fn accept_hdr(path: &Path) -> Option<String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => return Some(format!("That file could not be read: {error}")),
    };
    let hash = hash_hex(&content_hash(&bytes));
    SESSION.with(|cell| {
        let mut slot = cell.borrow_mut();
        let seed = slot.as_mut()?;
        let matches = seed
            .job
            .hdr
            .as_ref()
            .is_some_and(|source| source.sha256_hex == hash);
        if matches {
            seed.hdr_path = Some(path.to_path_buf());
            seed.message.clear();
            None
        } else {
            Some("That file is not the HDR map this job was made with.".to_string())
        }
    })
}

// ---- Saving ------------------------------------------------------------------------------

/// What a save needs, read from the session: the row, the guard values and the edit.
fn edit_for_save(
    seed: &Seed,
    now: &Controls,
    as_new: bool,
) -> Result<(i64, ExpectedRow, JobEdit), String> {
    let mut edit = edit_from_controls(seed, now)?;
    if edit.needs_curves(&seed.job) && edit.curves.is_none() {
        return Err("The tilt curves of this job's stone are not measured yet.".to_string());
    }
    if !as_new && let Some(wanted) = edit.output.take() {
        let own = PathBuf::from(&seed.job.output);
        edit.output = Some(reserve_excluding(&wanted, JobKindTag::of(&seed.job), &own));
    }
    Ok((seed.id, seed.expected.clone(), edit))
}

/// Save Changes (`as_new` false) or Save as New Job, with the answer to the question if asked.
fn do_save(ui: &MainWindow, as_new: bool, requeue: Requeue) {
    let now = read_controls(ui);
    let built = SESSION.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|seed| edit_for_save(seed, &now, as_new))
    });
    let (id, expected, edit) = match built {
        None => return,
        Some(Err(sentence)) => {
            ui.global::<JobEditModel>()
                .set_message(sentence.as_str().into());
            return;
        }
        Some(Ok(parts)) => parts,
    };
    let outcome = save_edit(ui, id, &expected, &edit, as_new, requeue);
    match outcome {
        SaveOutcome::Saved => {
            close(ui);
            show_toast(ui, &format!("Job {id} saved."), "success");
        }
        SaveOutcome::Created(new_id) => {
            close(ui);
            show_toast(ui, &format!("Saved as new job {new_id}."), "success");
        }
        SaveOutcome::RowChanged => {
            SESSION.with(|cell| {
                if let Some(seed) = cell.borrow_mut().as_mut() {
                    seed.refused = Some(
                        "This job changed while you were editing; nothing was changed.".to_string(),
                    );
                }
            });
            ui.global::<JobEditModel>().set_requeue_question_open(false);
            refresh(ui);
        }
        SaveOutcome::AskRequeue => ui.global::<JobEditModel>().set_requeue_question_open(true),
        SaveOutcome::Refused(sentence) => ui
            .global::<JobEditModel>()
            .set_message(sentence.as_str().into()),
    }
}
