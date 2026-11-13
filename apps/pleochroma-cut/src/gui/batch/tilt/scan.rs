//! The manually triggered scan for designs with missing or outdated tilt curves.
//!
//! It never runs automatically (see [`spawn_missing_tilt_curve_scan`]).

use crate::MainWindow;
use pleochroma_vault::db::sqlite::Database;
use slint::Weak;
use std::{
    sync::{Arc, Mutex, PoisonError},
    thread,
};

/// The entry ids whose tilt curves are missing, or stored under a fingerprint the design no
/// longer has. It runs off the UI thread (see [`spawn_missing_tilt_curve_scan`]).
///
/// The fingerprint compared with each row is built from the design's persisted material
/// state, as the preview scan builds it (`FingerprintMaterial::from_state`).
fn scan_missing_tilt_curve_ids(db: &Mutex<Database>) -> Vec<i64> {
    let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
    // A frosted design's curves are stored under a fingerprint naming its frosting:
    // `gui::batch::frosting_scan` judges each design by its own, as the preview scan does.
    let zones = crate::gui::batch::frosting_scan::zones_hash_table(&guard);
    // ... and so does a custom material's stored default crystal axis (a design's own axis,
    // which `outdated_ids` hands the closure, takes precedence).
    let axes = crate::gui::batch::frosting_scan::axis_key_table(&guard);
    let outdated = crate::gui::batch::frosting_scan::outdated_ids(
        &guard,
        |state, frosting, design_axis| {
            crate::bridge::preview_render::cache_fingerprint(
                crate::bridge::preview_render::CacheKind::TiltCurves,
                crate::bridge::preview_render::FingerprintMaterial::from_state(state)
                    .with_frosting(frosting)
                    .with_zones(crate::gui::batch::frosting_scan::zones_hash_in(
                        &zones,
                        state.name.as_deref(),
                    ))
                    .with_axis(design_axis.or_else(|| {
                        crate::gui::batch::frosting_scan::axis_key_in(&axes, state.name.as_deref())
                    })),
            )
        },
        |db, fingerprint| db.entry_ids_missing_tilt_curves(fingerprint),
    );
    drop(guard);
    outdated
}

/// Runs [`scan_missing_tilt_curve_ids`] on its own worker thread and reports the result
/// back to `on_result` on the UI event loop.
///
/// A full-catalogue run is long, so the scan is never offered unprompted. It is
/// started only by the explicit triggers `super::wiring::setup_tilt_batch_callbacks` wires:
/// the filter panel's "Compute missing tilt curves" button and a manual re-scan.
pub(super) fn spawn_missing_tilt_curve_scan(
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    on_result: impl FnOnce(&MainWindow, Vec<i64>) + Send + 'static,
) {
    thread::spawn(move || {
        let ids = scan_missing_tilt_curve_ids(&db);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| on_result(&ui, ids));
    });
}
