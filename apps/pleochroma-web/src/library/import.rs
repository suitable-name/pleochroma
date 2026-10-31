//! Putting designs into the library: Import Designs (picked files) and Save to Library (the
//! open design).
//!
//! One path for both: `pleochroma_vault::local::import_bytes` parses the file (the importer the
//! desktop and Android use), the design is solved and measured (proportions, volume and shape
//! through the vault's one rule, [`SolvedSolid`] being the browser's geometry), a thumbnail is
//! drawn from the same solve, and `library_record::plan_import` settles it against the entry
//! already stored under the file's id or url (the desktop's dedupe rule: the same bytes are
//! skipped, other bytes replace the entry and keep its tags and marks).
//!
//! A small design is solved on the page; a large one in the analysis Worker, one file at a
//! time, with a browser task between files so the status line moves. A design the Worker
//! cannot solve (it was superseded, the Worker failed) is stored unmeasured: its proportions
//! are the file's own.

use super::{forget_thumb, next_turn, notify, refresh_if_open, reload, set_busy, thumbs};
use crate::{
    app::{Ctx, push::MessageKind, state::SolveState},
    io::{IncomingFile, save::BuiltDesign},
    storage::library::{self, StoredDesignRecord},
};
use pleochroma::geometry::{GpuFacetPlane, meet_solver::SolvedTier};
use pleochroma_cut_core::Design;
use pleochroma_editor::{
    files::InputFileKind,
    guide::viewing_events as events,
    solve_policy::{SolveCostEstimate, should_solve_synchronously_for},
};
use pleochroma_solid::preview::view::plans_without_solver;
use pleochroma_vault::local::{
    ImportFailure, ImportKind, ImportStage, ImportedAsc, ImportedBytes, apply_measured_metadata,
    fill_designer_info, import_bytes,
};
use pleochroma_web_core::{
    library_measure::{SolvedSolid, design_of_attachments},
    library_record::{ImportPlan, StoreError, StoreImport, plan_import, url_taken_by_other},
    library_view::{ImportReport, Severity, meta_id_of, open_candidate, thumbnail_stamp},
    open_route::design_file_name_for_asc,
    solve::{SolveRequest, SolveResponse, design_to_toml},
    storage_keys::sha256_hex,
};
use std::{cell::Cell, collections::HashSet, sync::atomic::AtomicBool};

thread_local! {
    /// The browser was asked to keep the site's data once this session.
    static ASKED_FOR_PERSISTENCE: Cell<bool> = const { Cell::new(false) };
}

/// What storing one design did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stored {
    Added,
    Updated,
    Identical,
}

/// Why one file did not make it into the library.
struct Refusal {
    /// The browser's storage is full; the rest of a batch is not tried.
    out_of_space: bool,
    /// For the notice.
    message: String,
}

impl Refusal {
    fn file(message: impl Into<String>) -> Self {
        Self {
            out_of_space: false,
            message: message.into(),
        }
    }
}

impl From<StoreError> for Refusal {
    fn from(error: StoreError) -> Self {
        Self {
            out_of_space: error.is_quota(),
            message: error.user_text(),
        }
    }
}

/// How a failed read reads to the person who picked the file.
fn failure_text(failure: ImportFailure) -> String {
    let ImportFailure { stage, message } = failure;
    match stage {
        ImportStage::Design => format!("this design file cannot be read: {message}"),
        ImportStage::Asc => format!("this .asc file cannot be read: {message}"),
        ImportStage::Convert => format!("this file cannot be converted: {message}"),
        ImportStage::ConvertedSchedule => {
            format!("the converted schedule cannot be read: {message}")
        }
    }
}

/// Asks the browser to keep this site's data, once per session (the first time a design is
/// stored). The answer only matters to the browser.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn ask_for_persistence() {
    if ASKED_FOR_PERSISTENCE.replace(true) {
        return;
    }
    let _ = library::request_persistence().await;
}

/// The solid of `design`: solved on the page when that is cheap, else in the analysis Worker.
/// `None` when it could not be solved at all (no Worker, superseded, timed out).
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn solid_of(design: &Design) -> Option<SolvedSolid> {
    if plans_without_solver(design)
        || should_solve_synchronously_for(SolveCostEstimate::of(design), None)
    {
        return Some(SolvedSolid::of_design(design, &AtomicBool::new(false)));
    }
    // A tilt sweep holds the analysis Worker (a request would end it): leave it unmeasured.
    if crate::metrics::tilt::is_running() {
        return None;
    }
    let toml = design_to_toml(design).ok()?;
    let client = crate::workers::pool().ok()?.analysis().ok()?;
    match client.solve(toml, SolveRequest::Solve).await {
        Ok(SolveResponse::Solved(outcome)) => Some(SolvedSolid::from_outcome(outcome)),
        _ => None,
    }
}

/// Settles `parsed` against the library and writes it, with `thumbnail` (a PNG) as its picture.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn store_parsed(
    parsed: ImportedAsc,
    meta_id: Option<String>,
    thumbnail: Option<Vec<u8>>,
) -> Result<Stored, Refusal> {
    let url = parsed.entry.url.clone();
    let mut existing: Option<StoredDesignRecord> = match &meta_id {
        Some(id) => library::get_design(id).await?,
        None => None,
    };
    let by_url = library::find_by_url(&url).await?;
    // The id found one entry and the url names another: replacing the first would leave two
    // entries on one url, and `find_by_url` would then pick between them arbitrarily.
    if let Some(owner) = url_taken_by_other(
        existing.as_ref().map(|record| &record.entry),
        by_url.as_ref(),
    ) {
        return Err(Refusal::file(format!(
            "another library entry (\"{}\") already uses this file name: rename the design or \
             delete that entry first",
            owner.item.title
        )));
    }
    if existing.is_none()
        && let Some(entry) = by_url
    {
        let files = library::get_files(&entry.design_id)
            .await?
            .unwrap_or_default();
        existing = Some(StoredDesignRecord { entry, files });
    }
    let plan = plan_import(
        parsed,
        meta_id.as_deref(),
        existing
            .as_ref()
            .map(|record| (&record.entry, record.files.as_slice())),
        js_sys::Date::now(),
        true,
    );
    let ImportPlan::Store(store) = plan else {
        return Ok(Stored::Identical);
    };
    let StoreImport {
        entry,
        attachments,
        replaced,
    } = *store;
    let stored = library::put_design(entry, &attachments).await?;
    ask_for_persistence().await;
    forget_thumb(&stored.design_id);
    // A picture that fails to store (quota) is only a missing picture.
    let written = match (thumbnail, open_candidate(&attachments)) {
        (Some(png), Some(file)) => {
            library::put_thumbnail(&stored.design_id, &png, &thumbnail_stamp(&file.content))
                .await
                .is_ok()
        }
        _ => false,
    };
    if !written {
        let _ = library::delete_thumbnail(&stored.design_id).await;
    }
    Ok(if replaced {
        Stored::Updated
    } else {
        Stored::Added
    })
}

/// What reading, measuring and storing one design came to.
struct Imported {
    stored: Stored,
    /// The design could not be solved here, so its proportions are the file's own.
    unmeasured: bool,
    /// What the converter noted.
    warnings: Vec<String>,
}

/// Measures `parsed` from `solid` (when there is one), draws its thumbnail and stores it.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn measure_and_store(
    imported: ImportedBytes,
    meta_id: Option<String>,
    design: Option<Design>,
    solid: Option<SolvedSolid>,
    custom: &[pleochroma::optics::materials::GemMaterial],
) -> Result<Imported, Refusal> {
    let ImportedBytes {
        mut parsed,
        warnings,
    } = imported;
    let unmeasured = design.is_some() && solid.is_none();
    let mut thumbnail = None;
    if let Some(mut solid) = solid {
        apply_measured_metadata(&mut parsed.detail, &mut solid);
        if let (Some(design), Some(masts)) = (&design, solid.tiers()) {
            thumbnail = thumbs::render_png(design, masts, custom);
        }
    }
    fill_designer_info(&mut parsed.detail);
    let stored = store_parsed(parsed, meta_id, thumbnail).await?;
    Ok(Imported {
        stored,
        unmeasured,
        warnings,
    })
}

/// Reads one picked file, measures it and stores it.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn import_one(file: &IncomingFile, kind: InputFileKind) -> Result<Imported, Refusal> {
    let import_kind = match kind {
        InputFileKind::Design => ImportKind::Design,
        InputFileKind::Gem => ImportKind::Gem,
        InputFileKind::Gcs => ImportKind::Gcs,
        _ => ImportKind::Asc,
    };
    let imported = import_bytes(&file.name, import_kind, &file.bytes, None, |_| {}, |_| {})
        .map_err(|failure| Refusal::file(failure_text(failure)))?;
    let design = design_of_attachments(&imported.parsed.detail.attached_files).ok();
    let solid = match &design {
        Some(design) => solid_of(design).await,
        None => None,
    };
    let meta_id = meta_id_of(&file.name, &file.bytes);
    measure_and_store(imported, meta_id, design, solid, &[]).await
}

/// Import Designs: stores the picked files one by one and ends with one notice.
pub(super) fn import_files(ctx: &Ctx, files: Vec<IncomingFile>) {
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let report = run(&ctx, files).await;
        if !report.is_empty() {
            let kind = match report.severity() {
                Severity::Success => MessageKind::Success,
                Severity::Warning => MessageKind::Warning,
                Severity::Error => MessageKind::Error,
            };
            notify(&ctx, kind, &report.summary());
        }
        reload(&ctx).await;
        // A tutorial step may wait for designs to be imported: something was stored (added or
        // replaced), as on the desktop; a skipped identical file or a refusal is not an import.
        if report.added + report.updated > 0 {
            crate::editor::guide::raise(&ctx, events::LIBRARY_IMPORTED);
        }
    });
}

/// Imports `files` in order, updating the busy line before each.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn run(ctx: &Ctx, files: Vec<IncomingFile>) -> ImportReport {
    let mut report = ImportReport::default();
    let total = files.len();
    let mut seen: HashSet<String> = HashSet::new();
    for (n, file) in files.into_iter().enumerate() {
        set_busy(
            ctx,
            Some(&format!("Importing {} of {total}: {}", n + 1, file.name)),
        );
        next_turn().await;
        let Some(kind) = InputFileKind::classify(&file.name, &file.bytes).filter(|k| k.is_design())
        else {
            report.failed.push((
                file.name,
                "not a design file (.psdd, .asc, .gem or .gcs)".to_owned(),
            ));
            continue;
        };
        if !seen.insert(sha256_hex(&file.bytes)) {
            report.identical += 1;
            continue;
        }
        match import_one(&file, kind).await {
            Ok(done) => {
                match done.stored {
                    Stored::Added => report.added += 1,
                    Stored::Updated => report.updated += 1,
                    Stored::Identical => report.identical += 1,
                }
                if done.unmeasured {
                    report.unmeasured += 1;
                }
                for warning in done.warnings {
                    report.notes.push(format!("{}: {warning}", file.name));
                }
            }
            Err(refusal) if refusal.out_of_space => {
                report.out_of_space = true;
                break;
            }
            Err(refusal) => report.failed.push((file.name, refusal.message)),
        }
    }
    report
}

/// Save to Library, second half: stores the open design's `.psdd` (`built`) with the solve
/// already in hand (`tiers`), so nothing is solved again.
pub fn add_open_design(ctx: &Ctx, built: BuiltDesign, tiers: Option<Vec<SolvedTier>>) {
    let BuiltDesign {
        text,
        asc_name,
        draft,
        metadata,
        ..
    } = built;
    let file_name = design_file_name_for_asc(&asc_name);
    // The design, its viewport planes and its materials, read in one short borrow.
    let (design, planes, custom) = {
        let mut app = ctx.state.borrow_mut();
        let planes: Option<Vec<GpuFacetPlane>> = match (&app.solve, app.generation()) {
            (
                SolveState::Solved {
                    generation, planes, ..
                },
                Some(current),
            ) if *generation == current => Some(planes.clone()),
            _ => None,
        };
        let custom = app.custom_materials.clone();
        let design = app.design.as_mut().map(|state| {
            // The stamped table is now the design's: the next save keeps its id.
            state.metadata = metadata;
            state.session.design.clone()
        });
        (design, planes, custom)
    };
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        set_busy(&ctx, Some("Adding the open design to the library..."));
        next_turn().await;
        let solid = match (planes, tiers) {
            (Some(planes), Some(tiers)) => Some(SolvedSolid::from_planes(planes).with_tiers(tiers)),
            _ => None,
        };
        let outcome = add_built(&text, &file_name, design, solid, &custom).await;
        match outcome {
            Ok(Stored::Added) => notify(
                &ctx,
                if draft {
                    MessageKind::Warning
                } else {
                    MessageKind::Success
                },
                &if draft {
                    format!("Added {file_name} to the library as a draft: it does not solve.")
                } else {
                    format!("Added {file_name} to the library.")
                },
            ),
            Ok(Stored::Updated) => notify(
                &ctx,
                MessageKind::Success,
                &format!("Updated {file_name} in the library."),
            ),
            Ok(Stored::Identical) => notify(
                &ctx,
                MessageKind::Info,
                &format!("{file_name} is already in the library, unchanged."),
            ),
            Err(refusal) => notify(&ctx, MessageKind::Error, &refusal.message),
        }
        set_busy(&ctx, None);
        refresh_if_open(&ctx);
    });
}

/// Parses the design file `text` and stores it, measured from `solid`.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn add_built(
    text: &str,
    file_name: &str,
    design: Option<Design>,
    solid: Option<SolvedSolid>,
    custom: &[pleochroma::optics::materials::GemMaterial],
) -> Result<Stored, Refusal> {
    let imported = import_bytes(
        file_name,
        ImportKind::Design,
        text.as_bytes(),
        None,
        |_| {},
        |_| {},
    )
    .map_err(|failure| Refusal::file(failure_text(failure)))?;
    let meta_id = meta_id_of(file_name, text.as_bytes());
    measure_and_store(imported, meta_id, design, solid, custom)
        .await
        .map(|done| done.stored)
}
