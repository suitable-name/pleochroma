//! The compute side of the pleochroma browser app, with no DOM and no Slint in it.
//!
//! Everything here except [`host`] builds and is tested natively, so the logic a Web
//! Worker runs is checked by `cargo test -p pleochroma-web-core` rather than only in a
//! browser.
//!
//! - [`protocol`]: the messages between the page and its Workers ([`protocol::ToWorker`],
//!   [`protocol::FromWorker`]), postcard-encoded into a transferred `ArrayBuffer`, and
//!   the [`protocol::PROTOCOL_VERSION`] checked on `Init`.
//! - [`scene`]: [`scene::SceneSpec`], the plain-data description of a render scene, and
//!   [`scene::OwnedScene`], which builds exactly the scene the desktop builds for the
//!   same settings and lends out a `FrameScene`.
//! - [`render`]: the per-chunk CPU trace ([`render::handle_trace_chunk`]), the full-frame
//!   [`render::Accumulator`] and the [`render::ChunkPlanner`] that sizes chunks.
//! - [`solve`]: [`solve::handle_solve`], the solve worker's job: a native-TOML design in,
//!   the solved masts, planes, status and warnings out.
//! - [`solve_error`]: [`solve_error::SolveError`], the typed reason a solve-role job
//!   produced no answer (superseded, cancelled, failed).
//! - [`display`]: pixels from an accumulation -- the live tone map, the settled
//!   denoise, the PNG export and its file name -- each the desktop's own call.
//! - [`settings`]: the persisted render settings and session payload, and
//!   [`settings::scene_spec`], which turns them into a [`scene::SceneSpec`] with the
//!   desktop's conversions.
//! - [`concave_form`]: the Tier tab's concave form without the window -- tool mapping, blank
//!   draft, the stale-draft guard and the save.
//! - [`custom_material`]: the Design settings dialog's custom-material fields turned into
//!   a `GemMaterial` and the snapshot a design file stores for it.
//! - [`design_state`]: the loaded design with its source, `[meta]`, attachments and the
//!   custom-material snapshots a save carries.
//! - [`cut_slider`]: the toolbar's Cut slider without the window -- handle position, stored
//!   cutoff, label and the cutting-order `cut_steps` the planner is asked for.
//! - [`deep_summary`]: a finished Deep Solve as the tab shows it (verdict line, tier rows, the
//!   Pin check).
//! - [`design_meta`]: the `.psdd` file's descriptive `[meta]` table as the page
//!   keeps and stamps it (ISO-8601 UTC time, UUID v4 from caller-supplied bytes).
//! - [`gpu_admission`]: the pure half of the GPU admission gate -- scene eligibility, the
//!   per-adapter record and key, the self-check scene.
//! - [`gpu_startup`]: the GPU start-up test's pure half -- what to do at page load, the
//!   progress of a run, and the sentence that says what the app renders with.
//! - [`gpu_ledger`]: the pool's book of full passes posted to the GPU Worker.
//! - [`guide`]: the guided walkthrough's tab-session state (open, step, collapsed,
//!   where the floating panel was dragged to).
//! - [`guide_event_log`]: the events a lesson step may wait for, remembered for the rest of the
//!   step (with the Deep Solve hand-off between two steps).
//! - [`guide_events`]: which tutorial events a change of the library query, the render panel's
//!   stone effects or the Auto list raises (a state compared before and after).
//! - [`guide_judge`]: a lesson step judged from state, and the web's reading of a step's stage.
//! - [`guide_perform`]: what a lesson's "Next does it for me" recipe becomes in the browser -- the
//!   page's own actions in order, or the gap that keeps the step's Skip Step button.
//! - [`guide_runner`]: the lesson runner's pure half -- the lessons the browser offers, a step as
//!   the panel renders it, the lock groups it leaves open, and a launch waiting for its design.
//! - [`tutorial_list`]: the tutorial browser's rows and progress line for a search.
//! - [`hdr`]: the browser's HDR limits and the render-worker count rule for a map.
//! - [`input`]: the size limits checked on a picked or dropped file before it is read.
//! - [`open_route`]: what the Open picker offers and the design file's name (which loader
//!   a file goes to is `pleochroma_editor::files::InputFileKind::classify`).
//! - [`rows`]: the tier table's row ids, the driven-angle refusal, the schedule notes and
//!   the history labels (pure helpers of the page's editor).
//! - [`new_design`]: the New Design dialog's template path without the window.
//! - [`library_record`]: the design library's records, keys, errors and import plan.
//! - [`library_measure`]: reading a stored design back and measuring it for the library.
//! - [`library_view`]: the library screen without the window -- query, rows, metadata edit,
//!   import report, the file an entry opens from, `.psdd` export.
//! - [`storage_keys`]: the `pleochroma.<name>.v<n>` storage key builders.
//! - [`tab_state`]: the design name and unsaved flag, which belong to one tab
//!   (`sessionStorage`) rather than to the shared settings.
//! - [`worker`]: [`worker::WorkerHandler`], the whole message-handling state machine a
//!   Worker runs; the `pleochroma-web-compute` crate only moves bytes in and out of it.
//! - [`host`] (wasm32 only): the page-side pool, [`host::WorkerPool`] with its
//!   [`host::RenderPool`] and [`host::SolveClient`].

pub mod concave_form;
pub mod custom_material;
pub mod cut_slider;
pub mod deep_summary;
pub mod design_color;
pub mod design_meta;
pub mod design_state;
pub mod display;
pub mod gpu_admission;
pub mod gpu_ledger;
pub mod gpu_startup;
pub mod guide;
pub mod guide_event_log;
pub mod guide_events;
pub mod guide_judge;
pub mod guide_perform;
pub mod guide_runner;
pub mod hdr;
#[cfg(target_arch = "wasm32")]
pub mod host;
pub mod input;
pub mod library_measure;
pub mod library_record;
pub mod library_view;
pub mod new_design;
pub mod open_route;
mod pattern_wire;
pub mod protocol;
pub mod render;
pub mod rows;
pub mod scene;
pub mod settings;
pub mod solve;
pub mod solve_error;
pub mod storage_keys;
pub mod tab_state;
pub mod tutorial_list;
pub mod worker;

/// Where the host finds the Worker script: the loader shim Trunk writes next to the
/// app for `apps/pleochroma-web/index.html`'s `data-type="worker"` link.
///
/// Trunk names worker outputs after the Cargo package, never with a content hash
/// (`trunk` 0.21's `hashed_wasm_base` skips hashing for `data-type="worker"`), and
/// `data-loader-shim` adds `<name>_loader.js`, a classic-worker script that runs
/// `importScripts("./pleochroma-web-compute.js")` and then instantiates
/// `./pleochroma-web-compute_bg.wasm`. The URL is relative, so it resolves against the
/// page and keeps working when the app is served from a sub-path.
pub const WORKER_LOADER_URL: &str = "./pleochroma-web-compute_loader.js";

/// Where the host finds the GPU Worker script: the loader shim Trunk writes next to the
/// app for `apps/pleochroma-web/index.html`'s `data-type="worker"` link for `pleochroma-web-gpu`.
pub const GPU_WORKER_LOADER_URL: &str = "./pleochroma-web-gpu_loader.js";
