//! Copy to Materials: one new queued job per chosen material, each placed right after the one before it.
//!
//! A copy goes through the same loader, material resolver, template naming, output reservation and
//! insertion as every other new job. A still is copied in one go ([`copy_job_to_materials`]); a tilt
//! video measures its curves for each copy on a worker thread, one copy after the other, while the
//! dialog shows "Measuring… 2 of 3" and Cancel stops before the next copy. The dialog's ticks and
//! search text live here, not in Slint.

use super::{
    load::load_job_for_edit,
    material::{
        MaterialChange, material_edit, material_names, resolve_job_material, with_material_suffix,
    },
    naming::{JobKindTag, name_from_template, reserve_output},
    save::insert_job_after,
    video_curves::spawn_measure,
};
use crate::{
    JobCopyModel, JobCopyPreview, MainWindow, RenderJobsModel,
    bridge::render_thread::RenderContext,
    gui::render_jobs::controller::{job_changed, unix_now},
    settings::SettingsPersister,
};
use pleochroma_render_jobs::{
    RenderJobFile,
    edit::{EditRefusal, JobEdit},
    job::{CURVE_POINTS, JobKind, MetricCurvesData, new_job_token},
};
use pleochroma_vault::db::sqlite::Database;
use slint::{ComponentHandle as _, Model as _, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

/// What the dialog says is reset in each copy.
const RESET_NOTE: &str = "Haze, crystal axes and the material's own inclusions and color zones are reset to each new material; stone width, edge rounding, lighting, camera and output folder are kept.";

/// The sentence added for a tilt video.
const VIDEO_NOTE: &str = " The tilt curves are measured again for each copy, one after the other.";

/// Numbers the runs of a video copy, so a late result of a closed run is ignored.
static GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The render context the resolver reads (set by [`setup`]; the UI thread only).
    static CONTEXT: RefCell<Option<Arc<Mutex<RenderContext>>>> = const { RefCell::new(None) };
    /// The open dialog's state.
    static DIALOG: RefCell<Option<Dialog>> = const { RefCell::new(None) };
    /// The tilt video copy that is being made.
    static RUN: RefCell<Option<VideoRun>> = const { RefCell::new(None) };
}

/// What the open dialog knows: its source job, the choices and the ticks.
#[derive(Debug, Clone)]
struct Dialog {
    source: RenderJobFile,
    names: Vec<String>,
    ticked: Vec<String>,
    query: String,
}

impl Dialog {
    /// The material of the source job, which cannot be ticked.
    fn source_material(&self) -> &str {
        &self.source.scene.material.name
    }

    /// The ticked materials in checklist order.
    fn picked(&self) -> Vec<String> {
        self.names
            .iter()
            .filter(|name| self.ticked.contains(name))
            .cloned()
            .collect()
    }
}

/// A tilt video copy in progress: the materials, where the next copy goes, and what is made.
#[derive(Debug, Clone)]
pub(super) struct VideoRun {
    generation: u64,
    source: RenderJobFile,
    known: Vec<String>,
    materials: Vec<String>,
    next: usize,
    after: i64,
    made: Vec<i64>,
    stopping: bool,
}

impl VideoRun {
    /// A run over `materials` whose first copy goes right after job `source_id`.
    pub(super) const fn new(
        generation: u64,
        source_id: i64,
        source: RenderJobFile,
        known: Vec<String>,
        materials: Vec<String>,
    ) -> Self {
        Self {
            generation,
            source,
            known,
            materials,
            next: 0,
            after: source_id,
            made: Vec::new(),
            stopping: false,
        }
    }

    /// The material of the copy that is made next, if any is left.
    pub(super) fn current(&self) -> Option<&str> {
        self.materials.get(self.next).map(String::as_str)
    }

    /// Notes the new job `id`: the next copy goes after it.
    pub(super) fn record(&mut self, id: i64) {
        self.made.push(id);
        self.after = id;
        self.next += 1;
    }
}

/// What one step of a video run found.
enum Step {
    /// The run is over: finished, stopped, or failed with a sentence.
    Finish(Option<String>),
    /// A copy that needs no measuring was made; go on.
    Next,
    /// The curves of `staged` are measured on a worker thread.
    Measure {
        staged: RenderJobFile,
        axes: Vec<u32>,
        generation: u64,
    },
}

/// Registers `RenderJobsModel.copy_to_materials`, `open_copy_dialog` and every `JobCopyModel` callback.
pub(in crate::gui) fn setup(
    ui: &MainWindow,
    _db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    _settings_store: &Arc<SettingsPersister>,
) {
    CONTEXT.with(|slot| *slot.borrow_mut() = Some(Arc::clone(render_ctx)));

    let jobs = ui.global::<RenderJobsModel>();
    let weak = ui.as_weak();
    jobs.on_open_copy_dialog(move |id| {
        if let Some(ui) = weak.upgrade() {
            open_dialog(&ui, i64::from(id));
        }
    });
    let weak = ui.as_weak();
    jobs.on_copy_to_materials(move |id, materials| {
        if let Some(ui) = weak.upgrade() {
            start_copy(&ui, i64::from(id), to_strings(&materials));
        }
    });

    let copy = ui.global::<JobCopyModel>();
    let weak = ui.as_weak();
    copy.on_selection_changed(move |materials| {
        if let Some(ui) = weak.upgrade() {
            set_ticks(&ui, to_strings(&materials));
        }
    });
    let weak = ui.as_weak();
    copy.on_toggle(move |name| {
        if let Some(ui) = weak.upgrade() {
            toggle(&ui, name.as_str());
        }
    });
    let weak = ui.as_weak();
    copy.on_query_changed(move |query| {
        if let Some(ui) = weak.upgrade() {
            set_query(&ui, query.as_str());
        }
    });
    let weak = ui.as_weak();
    copy.on_copy(move |materials| {
        if let Some(ui) = weak.upgrade() {
            let id = i64::from(ui.global::<JobCopyModel>().get_job_id());
            start_copy(&ui, id, to_strings(&materials));
        }
    });
    let weak = ui.as_weak();
    copy.on_cancel(move || {
        if let Some(ui) = weak.upgrade() {
            cancel(&ui);
        }
    });
}

/// Copies still `source_id` to each of `materials`, in order after it, and returns the new job ids.
///
/// The synchronous form for pictures; the material check set calls it. A tilt video is an `Err`:
/// it measures its curves off the UI thread, so only the dialog copies it. A material equal to the
/// source's makes a plain duplicate, so a caller that does not want one leaves it out. The loader,
/// the resolver, the template name and the output reservation are the ones every new job uses.
pub(in crate::gui) fn copy_job_to_materials(
    ui: &MainWindow,
    source_id: i64,
    materials: &[String],
) -> Result<Vec<i64>, String> {
    let (_, source) = load_job_for_edit(ui, source_id)?;
    if matches!(source.kind, JobKind::TiltVideo(_)) {
        return Err(
            "A tilt video measures its tilt curves for each copy; copy it from the Copy to Materials dialog."
                .to_string(),
        );
    }
    copy_stills(ui, source_id, &source, materials)
}

/// Makes the copies of a still, one after the other, each after the one before.
fn copy_stills(
    ui: &MainWindow,
    source_id: i64,
    source: &RenderJobFile,
    materials: &[String],
) -> Result<Vec<i64>, String> {
    let known = with_context(material_names)?;
    let mut after = source_id;
    let mut made = Vec::with_capacity(materials.len());
    for material in materials {
        match copy_one(ui, source, material, &known, after, None) {
            Ok(id) => {
                made.push(id);
                after = id;
            }
            Err(message) => return Err(partial(&message, made.len(), materials.len())),
        }
    }
    Ok(made)
}

/// Makes one copy of `source` in `material` and inserts it after job `after`.
///
/// `curves` are the measured curves of a tilt video. The copy gets a new token, its own template
/// name reserved against the queue and the disk, and the queue refreshes for it.
fn copy_one(
    ui: &MainWindow,
    source: &RenderJobFile,
    material: &str,
    known: &[String],
    after: i64,
    curves: Option<Vec<MetricCurvesData>>,
) -> Result<i64, String> {
    let mut edit = plan_edit(source, material, known)?;
    if let Some(curves) = curves {
        edit.curves = Some(curves);
    }
    let mut job = apply_edit(source, &edit)?;
    job.output = planned_output(ui, &job)?.to_string_lossy().into_owned();
    job.token = new_job_token();
    job.created_at = unix_now();
    let id = insert_job_after(ui, after, job)?;
    job_changed(ui, id);
    Ok(id)
}

/// The edit that turns `source` into its copy in `material`: the resolved material and the new names.
pub(super) fn plan_edit(
    source: &RenderJobFile,
    material: &str,
    known: &[String],
) -> Result<JobEdit, String> {
    let change = with_context(|ctx| resolve_job_material(ctx, source, material))??;
    Ok(copy_edit(source, change, material, known))
}

/// The material edit plus the label and, for a video, the video name of the copy.
fn copy_edit(
    source: &RenderJobFile,
    change: MaterialChange,
    material: &str,
    known: &[String],
) -> JobEdit {
    let (label, video_name) = copy_names(source, material, known);
    let mut edit = material_edit(change);
    edit.label = Some(label);
    edit.video_name = video_name;
    edit
}

/// The label of the copy and, for a tilt video, its video name: " · <material>" replaced or appended.
pub(super) fn copy_names(
    source: &RenderJobFile,
    material: &str,
    known: &[String],
) -> (String, Option<String>) {
    let label = with_material_suffix(&source.label, material, known);
    let video_name = match &source.kind {
        JobKind::TiltVideo(video) => Some(with_material_suffix(&video.video_name, material, known)),
        JobKind::Still(_) => None,
    };
    (label, video_name)
}

/// Applies `edit` to `source` and turns a refusal into a sentence.
fn apply_edit(source: &RenderJobFile, edit: &JobEdit) -> Result<RenderJobFile, String> {
    edit.apply(source).map_err(refusal_text)
}

/// The sentence for a refused edit.
fn refusal_text(refusal: EditRefusal) -> String {
    match refusal {
        EditRefusal::Running => "The job is rendering. Pause it first.".to_string(),
        EditRefusal::RayRemote => "A Ray-Traced job renders on this computer only.".to_string(),
        EditRefusal::CurvesMissing => {
            "The tilt curves of the copy could not be measured.".to_string()
        }
        EditRefusal::Invalid(sentence) => sentence,
    }
}

/// The free output path of a new job: the template name of a picture, the frame folder of a video.
fn planned_output(ui: &MainWindow, job: &RenderJobFile) -> Result<PathBuf, String> {
    let (template_name, kind) = match &job.kind {
        JobKind::Still(_) => (name_from_template(ui, job)?, JobKindTag::Still),
        JobKind::TiltVideo(_) => (String::new(), JobKindTag::TiltVideo),
    };
    Ok(reserve_output(
        ui,
        &wanted_output(job, &template_name),
        kind,
    ))
}

/// The path a copy wants before it is reserved.
///
/// A picture goes beside the source with the template's file name (".png" when the template name has
/// no extension). A video's frame folder is the source's output folder plus the copy's own video name.
pub(super) fn wanted_output(job: &RenderJobFile, template_name: &str) -> PathBuf {
    let folder = Path::new(&job.output)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    match &job.kind {
        JobKind::Still(_) => {
            let name = if Path::new(template_name).extension().is_none() {
                format!("{template_name}.png")
            } else {
                template_name.to_string()
            };
            folder.join(name)
        }
        JobKind::TiltVideo(video) => folder.join(&video.video_name),
    }
}

/// `path`, or with " (2)", " (3)" ... before the extension when `taken` already holds it.
///
/// Only the preview uses this, for names that clash inside one list; the copies themselves reserve
/// one after the other, so the queue already holds the ones before.
pub(super) fn distinct_path(path: PathBuf, taken: &[PathBuf]) -> PathBuf {
    if !taken.contains(&path) {
        return path;
    }
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    for number in 2_u32.. {
        let candidate = path.with_file_name(format!("{stem} ({number}){extension}"));
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    path
}

/// Placeholder curves of the right shape, so an edit that needs curves applies before they exist.
pub(super) fn placeholder_curves(axes: usize) -> Vec<MetricCurvesData> {
    vec![
        MetricCurvesData {
            brilliance: vec![0.0; CURVE_POINTS],
            windowing: vec![0.0; CURVE_POINTS],
            extinction: vec![0.0; CURVE_POINTS],
        };
        axes
    ]
}

/// The axes of a tilt video, in the order it shows them; empty for a picture.
fn video_axes(job: &RenderJobFile) -> Vec<u32> {
    match &job.kind {
        JobKind::TiltVideo(video) => video.axes(),
        JobKind::Still(_) => Vec::new(),
    }
}

/// Runs `read` on the render context.
fn with_context<R>(read: impl FnOnce(&RenderContext) -> R) -> Result<R, String> {
    let shared = CONTEXT
        .with(|slot| slot.borrow().clone())
        .ok_or_else(|| "Copy to Materials is not set up.".to_string())?;
    let guard = shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Ok(read(&guard))
}

/// `message`, with how many copies were made before it when some were.
fn partial(message: &str, made: usize, total: usize) -> String {
    if made == 0 {
        message.to_string()
    } else {
        format!("{message} {made} of {total} copies were made.")
    }
}

/// The strings of a Slint list.
fn to_strings(list: &ModelRc<SharedString>) -> Vec<String> {
    list.iter().map(|item| item.to_string()).collect()
}

/// A Slint list of `values`.
fn string_model(values: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        values
            .iter()
            .map(|value| SharedString::from(value.as_str()))
            .collect::<Vec<_>>(),
    ))
}

/// Opens the dialog for job `id`; does nothing for a missing job.
fn open_dialog(ui: &MainWindow, id: i64) {
    if id <= 0 {
        return;
    }
    let model = ui.global::<JobCopyModel>();
    if model.get_busy() {
        return;
    }
    let loaded = load_job_for_edit(ui, id).and_then(|(meta, job)| {
        let names = with_context(material_names)?;
        Ok((meta, job, names))
    });
    model.set_message(SharedString::new());
    model.set_busy(false);
    model.set_busy_text(SharedString::new());
    model.set_query(SharedString::new());
    model.set_job_id(i32::try_from(id).unwrap_or(-1));
    match loaded {
        Ok((meta, job, names)) => {
            let is_video = matches!(job.kind, JobKind::TiltVideo(_));
            model.set_source_label(meta.label.as_str().into());
            model.set_source_summary(meta.summary.as_str().into());
            model.set_source_material(job.scene.material.name.as_str().into());
            model.set_is_video(is_video);
            model.set_reset_note(
                if is_video {
                    format!("{RESET_NOTE}{VIDEO_NOTE}")
                } else {
                    RESET_NOTE.to_string()
                }
                .into(),
            );
            model.set_materials(string_model(&names));
            DIALOG.with(|slot| {
                *slot.borrow_mut() = Some(Dialog {
                    source: job,
                    names,
                    ticked: Vec::new(),
                    query: String::new(),
                });
            });
            publish(ui);
        }
        Err(message) => {
            DIALOG.with(|slot| *slot.borrow_mut() = None);
            model.set_source_label(SharedString::new());
            model.set_source_summary(SharedString::new());
            model.set_source_material(SharedString::new());
            model.set_materials(string_model(&[]));
            model.set_choice_names(string_model(&[]));
            model.set_choice_ticked(ModelRc::new(VecModel::from(Vec::<bool>::new())));
            model.set_picked(string_model(&[]));
            model.set_preview(ModelRc::new(VecModel::from(Vec::<JobCopyPreview>::new())));
            model.set_message(message.into());
        }
    }
    model.set_open(true);
}

/// Closes the dialog and forgets its state.
fn close_dialog(ui: &MainWindow) {
    ui.global::<JobCopyModel>().set_open(false);
    DIALOG.with(|slot| *slot.borrow_mut() = None);
}

/// Writes the checklist, the ticked list and the preview of the new jobs into the model.
fn publish(ui: &MainWindow) {
    let Some(state) = DIALOG.with(|slot| slot.borrow().clone()) else {
        return;
    };
    let picked = state.picked();
    let needle = state.query.trim().to_lowercase();
    let shown: Vec<&String> = state
        .names
        .iter()
        .filter(|name| needle.is_empty() || name.to_lowercase().contains(&needle))
        .collect();
    let (preview, error) = preview_rows(ui, &state.source, &state.names, &picked);

    let model = ui.global::<JobCopyModel>();
    model.set_choice_names(ModelRc::new(VecModel::from(
        shown
            .iter()
            .map(|name| SharedString::from(name.as_str()))
            .collect::<Vec<_>>(),
    )));
    model.set_choice_ticked(ModelRc::new(VecModel::from(
        shown
            .iter()
            .map(|name| state.ticked.contains(name))
            .collect::<Vec<_>>(),
    )));
    model.set_picked(string_model(&picked));
    model.set_preview(ModelRc::new(VecModel::from(preview)));
    model.set_message(error.unwrap_or_default().into());
}

/// The new jobs as the dialog lists them, and the first sentence of a material that cannot be copied.
fn preview_rows(
    ui: &MainWindow,
    source: &RenderJobFile,
    known: &[String],
    picked: &[String],
) -> (Vec<JobCopyPreview>, Option<String>) {
    let mut rows = Vec::with_capacity(picked.len());
    let mut taken: Vec<PathBuf> = Vec::new();
    let mut error = None;
    for material in picked {
        match preview_one(ui, source, material, known, &taken) {
            Ok((row, path)) => {
                taken.push(path);
                rows.push(row);
            }
            Err(message) => {
                if error.is_none() {
                    error = Some(message);
                }
            }
        }
    }
    (rows, error)
}

/// One preview row and the path it would write.
fn preview_one(
    ui: &MainWindow,
    source: &RenderJobFile,
    material: &str,
    known: &[String],
    taken: &[PathBuf],
) -> Result<(JobCopyPreview, PathBuf), String> {
    let mut edit = plan_edit(source, material, known)?;
    if edit.needs_curves(source) {
        edit.curves = Some(placeholder_curves(video_axes(source).len()));
    }
    let staged = apply_edit(source, &edit)?;
    let path = distinct_path(planned_output(ui, &staged)?, taken);
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let row = JobCopyPreview {
        material: material.into(),
        label: staged.label.as_str().into(),
        file_name: file_name.into(),
    };
    Ok((row, path))
}

/// Flips the tick of `name`; the source's own material cannot be ticked.
fn toggle(ui: &MainWindow, name: &str) {
    let changed = DIALOG.with(|slot| {
        let mut slot = slot.borrow_mut();
        let state = slot.as_mut()?;
        if name == state.source_material() || !state.names.iter().any(|known| known == name) {
            return None;
        }
        if let Some(index) = state.ticked.iter().position(|ticked| ticked == name) {
            state.ticked.remove(index);
        } else {
            state.ticked.push(name.to_string());
        }
        Some(())
    });
    if changed.is_some() {
        publish(ui);
    }
}

/// Replaces the ticked materials as a whole; names the checklist does not hold are dropped.
fn set_ticks(ui: &MainWindow, materials: Vec<String>) {
    let changed = DIALOG.with(|slot| {
        let mut slot = slot.borrow_mut();
        let state = slot.as_mut()?;
        let mut ticked: Vec<String> = Vec::new();
        for material in materials {
            if material != state.source_material()
                && state.names.contains(&material)
                && !ticked.contains(&material)
            {
                ticked.push(material);
            }
        }
        state.ticked = ticked;
        Some(())
    });
    if changed.is_some() {
        publish(ui);
    }
}

/// Stores the search text and shows the matching rows.
fn set_query(ui: &MainWindow, query: &str) {
    let changed = DIALOG.with(|slot| {
        let mut slot = slot.borrow_mut();
        let state = slot.as_mut()?;
        state.query = query.to_string();
        Some(())
    });
    if changed.is_some() {
        publish(ui);
    }
}

/// Closes the dialog, or while a video's copies are being made stops before the next copy.
fn cancel(ui: &MainWindow) {
    let running = RUN.with(|slot| {
        if let Some(run) = slot.borrow_mut().as_mut() {
            run.stopping = true;
            true
        } else {
            false
        }
    });
    if running {
        ui.global::<JobCopyModel>()
            .set_busy_text("Stopping after this copy…".into());
    } else {
        close_dialog(ui);
    }
}

/// Copies job `id` to `materials`: a picture at once, a video one copy after the other.
fn start_copy(ui: &MainWindow, id: i64, materials: Vec<String>) {
    if id <= 0 || materials.is_empty() {
        return;
    }
    let model = ui.global::<JobCopyModel>();
    if model.get_busy() {
        return;
    }
    model.set_message(SharedString::new());
    let source = match load_job_for_edit(ui, id) {
        Ok((_, job)) => job,
        Err(message) => {
            model.set_message(message.into());
            return;
        }
    };
    if matches!(source.kind, JobKind::TiltVideo(_)) {
        start_video_run(ui, id, source, materials);
        return;
    }
    match copy_stills(ui, id, &source, &materials) {
        Ok(_) => close_dialog(ui),
        Err(message) => model.set_message(message.into()),
    }
}

/// Starts the run that copies a tilt video to `materials`.
fn start_video_run(ui: &MainWindow, id: i64, source: RenderJobFile, materials: Vec<String>) {
    let model = ui.global::<JobCopyModel>();
    let known = match with_context(material_names) {
        Ok(known) => known,
        Err(message) => {
            model.set_message(message.into());
            return;
        }
    };
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    RUN.with(|slot| {
        *slot.borrow_mut() = Some(VideoRun::new(generation, id, source, known, materials));
    });
    model.set_busy(true);
    run_next(ui);
}

/// Goes on with the video run until a copy has to wait for its curves, or the run is over.
fn run_next(ui: &MainWindow) {
    loop {
        match next_step(ui) {
            Step::Next => {}
            Step::Finish(error) => {
                finish(ui, error);
                return;
            }
            Step::Measure {
                staged,
                axes,
                generation,
            } => {
                spawn_measure(ui, staged, axes, move |ui, result| {
                    on_measured(ui, generation, result);
                });
                return;
            }
        }
    }
}

/// The next step of the run: stop, a copy made at once, or a measurement to wait for.
fn next_step(ui: &MainWindow) -> Step {
    let Some(run) = RUN.with(|slot| slot.borrow().clone()) else {
        return Step::Finish(None);
    };
    let Some(material) = run.current().map(str::to_string) else {
        return Step::Finish(None);
    };
    if run.stopping {
        return Step::Finish(None);
    }
    ui.global::<JobCopyModel>()
        .set_busy_text(format!("Measuring… {} of {}", run.next + 1, run.materials.len()).into());
    match plan_step(ui, &run, &material) {
        Ok(step) => step,
        Err(message) => Step::Finish(Some(message)),
    }
}

/// Makes the copy in `material` at once when it needs no curves, otherwise stages its measurement.
fn plan_step(ui: &MainWindow, run: &VideoRun, material: &str) -> Result<Step, String> {
    let mut edit = plan_edit(&run.source, material, &run.known)?;
    if !edit.needs_curves(&run.source) {
        let id = copy_one(ui, &run.source, material, &run.known, run.after, None)?;
        RUN.with(|slot| {
            if let Some(current) = slot.borrow_mut().as_mut() {
                current.record(id);
            }
        });
        return Ok(Step::Next);
    }
    let axes = video_axes(&run.source);
    edit.curves = Some(placeholder_curves(axes.len()));
    let staged = apply_edit(&run.source, &edit)?;
    Ok(Step::Measure {
        staged,
        axes,
        generation: run.generation,
    })
}

/// The curves of the copy in `generation` are measured: make the copy, then the next one.
fn on_measured(ui: &MainWindow, generation: u64, result: Result<Vec<MetricCurvesData>, String>) {
    let Some(run) = RUN.with(|slot| slot.borrow().clone()) else {
        return;
    };
    if run.generation != generation {
        return;
    }
    let Some(material) = run.current().map(str::to_string) else {
        finish(ui, None);
        return;
    };
    let made = result.and_then(|curves| {
        copy_one(
            ui,
            &run.source,
            &material,
            &run.known,
            run.after,
            Some(curves),
        )
    });
    match made {
        Ok(id) => {
            RUN.with(|slot| {
                if let Some(current) = slot.borrow_mut().as_mut() {
                    current.record(id);
                }
            });
            run_next(ui);
        }
        Err(message) => finish(ui, Some(message)),
    }
}

/// Ends the run: closes the dialog, or keeps it open with the sentence that stopped it.
fn finish(ui: &MainWindow, error: Option<String>) {
    let run = RUN.with(|slot| slot.borrow_mut().take());
    let model = ui.global::<JobCopyModel>();
    model.set_busy(false);
    model.set_busy_text(SharedString::new());
    let (made, total) = run.map_or((0, 0), |run| (run.made.len(), run.materials.len()));
    match error {
        Some(message) => model.set_message(partial(&message, made, total).into()),
        None => close_dialog(ui),
    }
}
