//! The once-per-session library scan for designs whose previews are missing or stale (see
//! `gui::batch::preview`).

use crate::MainWindow;
use pleochroma_vault::db::sqlite::Database;
use slint::Weak;
use std::{
    sync::{Mutex, PoisonError},
    thread,
};

/// Every entry id whose cached preview is missing. A design counts as missing when either
/// view (front or top) is absent, or when its picture was rendered with other parameters
/// than the current ones (`preview_size` and `preview_spp` feed the fingerprint). The check
/// is one blob-free `LEFT JOIN` query (`Database::entry_ids_missing_previews`); reading the
/// cached PNGs per row instead took seconds and hundreds of MB over the catalogue. Intended to
/// run off the UI thread (see [`spawn_missing_preview_scan`]); this function spawns no thread.
///
/// The fingerprint is built from the row's persisted material state (name, stone size and
/// source: `FingerprintMaterial::from_state`), never from a design file, which the scan does
/// not open. A design whose file names a material or a size therefore counts as missing until
/// a batch has resolved it and persisted that state (`gui::batch::catalogue_material`).
#[cfg(test)]
fn scan_missing_preview_ids(db: &Mutex<Database>, preview_size: u32, preview_spp: u32) -> Vec<i64> {
    scan_missing_preview_ids_for_engine(
        db,
        preview_size,
        preview_spp,
        pleochroma::renderer::engine::EngineSpec::default(),
    )
}

fn scan_missing_preview_ids_for_engine(
    db: &Mutex<Database>,
    preview_size: u32,
    preview_spp: u32,
    engine: pleochroma::renderer::engine::EngineSpec,
) -> Vec<i64> {
    let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
    // A frosted design's picture is stored under a fingerprint naming its frosting, which the
    // vault's scan cannot see per row: `frosting_scan` judges each design by its own.
    // A patterned material's picture names its zones too: read once for every row.
    let zones = crate::gui::batch::frosting_scan::zones_hash_table(&guard);
    // ... and so does a custom material's stored default crystal axis (a design's own axis,
    // which `outdated_ids` hands the closure, takes precedence).
    let axes = crate::gui::batch::frosting_scan::axis_key_table(&guard);
    // ... and a design's stored surface finish and abraded edge (the vault's side tables),
    // with the faceting machine the scratches are drawn for.
    let machine = crate::settings::model::current_faceting_machine();
    let outdated = crate::gui::batch::frosting_scan::outdated_ids_with_effects(
        &guard,
        Some(&machine),
        |state, frosting, design_axis, effects| {
            crate::bridge::preview_render::cache_fingerprint(
                if engine.is_ray() {
                    crate::bridge::preview_render::CacheKind::RayPreview {
                        size: preview_size,
                        taps: engine.ray.sanitized().aa_taps,
                        fire: engine.ray.fire,
                    }
                } else {
                    crate::bridge::preview_render::CacheKind::Preview {
                        size: preview_size,
                        spp: preview_spp,
                        max_bounces: super::engine::PREVIEW_MAX_BOUNCES,
                    }
                },
                crate::bridge::preview_render::FingerprintMaterial::from_state(state)
                    .with_frosting(frosting)
                    .with_zones(crate::gui::batch::frosting_scan::zones_hash_in(
                        &zones,
                        state.name.as_deref(),
                    ))
                    .with_axis(design_axis.or_else(|| {
                        crate::gui::batch::frosting_scan::axis_key_in(&axes, state.name.as_deref())
                    }))
                    .with_effects(effects),
            )
        },
        |db, fingerprint| db.entry_ids_missing_previews(fingerprint),
    );
    drop(guard);
    outdated
}

/// Runs [`scan_missing_preview_ids`] on its own worker thread and reports the result
/// back to `on_result` on the UI event loop: `gui::mod` calls this once per session,
/// shortly after the initial diagram list loads, and shows a confirmation prompt only
/// if the returned list is non-empty.
pub(super) fn spawn_missing_preview_scan(
    ui_weak: Weak<MainWindow>,
    db: std::sync::Arc<Mutex<Database>>,
    preview_size: u32,
    preview_spp: u32,
    engine: pleochroma::renderer::engine::EngineSpec,
    on_result: impl FnOnce(&MainWindow, Vec<i64>) + Send + 'static,
) {
    #[cfg(feature = "ui-harness")]
    let pending = crate::harness_support::begin_startup_preview_scan();
    thread::spawn(move || {
        let ids = scan_missing_preview_ids_for_engine(&db, preview_size, preview_spp, engine);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            on_result(&ui, ids);
            #[cfg(feature = "ui-harness")]
            pending.store(false, std::sync::atomic::Ordering::Release);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bridge::preview_render::{CacheKind, FingerprintMaterial, cache_fingerprint},
        gui::batch::{
            preview::engine::{
                BatchContext, PREVIEW_MAX_BOUNCES, preview_fingerprint, resolve_design,
            },
            test_records::{design_file_text, store_design_file},
        },
    };
    use pleochroma::{optics::materials::GemMaterial, render_setup};
    use pleochroma_cut_core::material::MaterialSelection;
    use pleochroma_vault::model::preview::{PreviewMaterialSource, PreviewMaterialState};
    use std::{collections::BTreeSet, sync::atomic::AtomicBool};

    const SIZE: u32 = 160;
    const SPP: u32 = 4;

    /// A design whose file names Sapphire at a 9 mm girdle diameter.
    fn sapphire_design(db: &Mutex<Database>) -> i64 {
        let selection = MaterialSelection {
            name: Some("Sapphire".to_string()),
            ..MaterialSelection::none()
        };
        let text = design_file_text(selection, Some(9.0), None);
        let guard = db.lock().expect("the database lock is not poisoned");
        store_design_file(&guard, "sapphire.psdd", text.into_bytes())
    }

    /// A design whose file names a material and a girdle diameter is resolved in them,
    /// its previews are stored under a fingerprint that names them, and the scan then finds
    /// it current. A fingerprint that names only the material's name reads as stale.
    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "one sequential fixture-and-assert scenario; splitting would scatter the shared setup"
    )]
    fn a_design_files_material_and_size_are_part_of_the_stored_fingerprint() {
        let db = Mutex::new(Database::new(Some(":memory:")).expect("in-memory database opens"));
        let entry_id = sapphire_design(&db);
        let gpu_retired = AtomicBool::new(false);
        let angle_table_entries = Mutex::new(BTreeSet::new());
        let ctx = BatchContext {
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            ray_draft: false,
            db: &db,
            material_candidates: &[],
            custom_materials: &[],
            preview_size: SIZE,
            preview_spp: SPP,
            solid: false,
            gpu_retired: &gpu_retired,
            angle_table_entries: &angle_table_entries,
        };

        let resolved = resolve_design(&ctx, entry_id).expect("the design resolves");
        assert_eq!(
            resolved.material,
            render_setup::material_for_stone(
                GemMaterial::by_name("Sapphire").expect("a built-in"),
                9.0,
                &resolved.planes
            ),
            "drawn in the file's own material at the file's own size"
        );
        let state = db
            .lock()
            .expect("the database lock is not poisoned")
            .get_preview_material_state(entry_id)
            .expect("the state reads");
        assert_eq!(
            state,
            PreviewMaterialState {
                name: Some("Sapphire".to_string()),
                girdle_mm: Some(9.0),
                source: PreviewMaterialSource::DesignFile,
            }
        );
        assert_eq!(scan_missing_preview_ids(&db, SIZE, SPP), vec![entry_id]);

        let updated_at = db
            .lock()
            .expect("the database lock is not poisoned")
            .entry_updated_at(entry_id)
            .expect("the stamp reads");
        let store = |fingerprint: &str| {
            db.lock()
                .expect("the database lock is not poisoned")
                .save_preview_images(
                    entry_id,
                    Some(b"front".as_slice()),
                    Some(b"top".as_slice()),
                    1,
                    fingerprint,
                    updated_at,
                )
                .expect("the images save")
        };

        // Stored the way a finished batch stores them: current.
        assert!(store(&preview_fingerprint(
            &ctx,
            &state,
            0,
            0,
            None,
            crate::bridge::preview_render::EffectsKey::NONE,
        )));
        assert_eq!(scan_missing_preview_ids(&db, SIZE, SPP), Vec::<i64>::new());

        let ray = pleochroma::renderer::engine::EngineSpec {
            engine: pleochroma::renderer::engine::RenderEngine::RayTraced,
            ..Default::default()
        };
        assert_eq!(
            scan_missing_preview_ids_for_engine(&db, SIZE, SPP, ray),
            vec![entry_id]
        );
        let ray_fingerprint = cache_fingerprint(
            CacheKind::RayPreview {
                size: SIZE,
                taps: 8,
                fire: true,
            },
            FingerprintMaterial::from_state(&state),
        );
        assert!(store(&ray_fingerprint));
        assert_eq!(
            scan_missing_preview_ids_for_engine(&db, SIZE, SPP, ray),
            [] as [i64; 0]
        );
        assert_eq!(scan_missing_preview_ids(&db, SIZE, SPP), vec![entry_id]);
        let draft = cache_fingerprint(
            CacheKind::RayDraft {
                size: SIZE,
                taps: 8,
                fire: true,
            },
            FingerprintMaterial::from_state(&state),
        );
        assert!(store(&draft));
        assert_eq!(
            scan_missing_preview_ids_for_engine(&db, SIZE, SPP, ray),
            vec![entry_id]
        );

        // Stored with the name alone in the fingerprint, so the scan lists it again.
        let legacy = cache_fingerprint(
            CacheKind::Preview {
                size: SIZE,
                spp: SPP,
                max_bounces: PREVIEW_MAX_BOUNCES,
            },
            FingerprintMaterial::picked(Some("Sapphire")),
        );
        assert!(store(&legacy));
        assert_eq!(scan_missing_preview_ids(&db, SIZE, SPP), vec![entry_id]);
    }
}
