//! The high-resolution export flow.
//!
//! The flow resolves the export directory (prompting once if unset), validates the request, and
//! captures a `SceneSnapshot` that is independent of the live viewport's size and accumulation
//! buffer. It fans the snapshot out into one render per selected export-usable preset plus the
//! base current-view render, runs them one at a time via `export_thread::spawn_export`, and names
//! each file through the configurable template.
//!
//! Split into [`queue`] (the `ExportQueue`/`ExportJob` engine and its pure scene-transform
//! helpers) and [`wiring`] (the Slint callback registrations).
//!
//! Jobs run sequentially: the GPU backend is one shared adapter, so two exports would serialize
//! on it anyway. One job at a time also keeps the state simple: a single `ExportHandle` is the
//! current one for cancel, and one set of export state (`export_active_count`, the preview) is
//! live. The cost is wall-clock time; [`queue::ExportQueue`] reports progress across the whole
//! queue so that the time does not look like a stall.
//!
//! `ExportQueue` lives behind `Arc<Mutex<_>>`, not `Rc<RefCell<_>>`: `spawn_export`'s
//! `on_progress`/`on_done` closures must be `Send` (captured by the export worker's
//! `thread::spawn`, even though actually invoked back on the UI thread via
//! `upgrade_in_event_loop`), so anything they capture must be `Send` too. Every field
//! `ExportQueue` holds (paths, strings, `SceneSnapshot`s, `ExportHandle`) already is.

mod advanced;
// `pub(in crate::gui)`: the render job queue (`gui::render_jobs`) reuses the export's
// folder resolution, fan-out, naming and scene helpers instead of copying them.
pub(in crate::gui) mod queue;
pub(in crate::gui) mod wiring;

pub(in crate::gui) use wiring::setup_render_export_callbacks;
