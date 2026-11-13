//! Background batch computation of the catalogue's tilt-performance curves.
//!
//! The curves are `pleochroma_vault::model::tilt_curves::TiltPerformanceCurves`. The batch
//! follows the same shape as `gui::batch::preview`: two-level progress, cooperative
//! cancellation, per-design panic isolation and a `RenderContext::export_active` viewport pause.
//!
//! It is kept separate because the unit of work differs: one design's whole four-axis sweep
//! here, against independently dispatched views in preview. The Slint dialog is also its own
//! component (`tilt_batch_dialog.slint`, modelled on `preview_batch_dialog.slint`). Both
//! batches share the local/remote dispatch in `gui::batch::batch_queue`. This module uses
//! `gui::batch::preview`'s `RI_MATCH_TOLERANCE` and `target_ri_for_design`, and
//! `gui::batch::material_choice`, rather than re-deriving them.
//!
//! A design's four-axis sweep is far heavier than a preview image, so remote distribution
//! matters more here.
//!
//! Lane distribution follows `gui::batch::batch_queue`'s shared `WorkQueue`, as in
//! `gui::batch::preview` (see its `LiveComputeTarget`, `RemoteOnly` and `Both` modes). The
//! queue's unit here is one whole design. A single sweep is itself threaded
//! (`evaluate_full_axis_profile_at_azimuth_finished_threaded`), so the local lanes cap each
//! sweep at `available_parallelism / local_lane_count` threads (at least 1). Lanes times
//! inner threads never exceeds the cores; the lanes, not the inner threads, spread a
//! whole-catalogue run across the machine.
//!
//! The sweep honours a design's frosting: a frosted facet scatters light instead of
//! reflecting and refracting it specularly, so a frosted design's curves differ from its
//! polished ones. The batch resolves the frosted planes with its geometry
//! (`gui::batch::record_planes_with_frosting_for_batch`, as the preview batch does), the
//! local sweep uses the facet finishes they give, and a remote request carries the planes
//! in its scene so the worker sweeps the same stone. The stored fingerprint therefore names
//! the frosting (`frosting=`): setting or clearing it changes the curves.
//!
//! Cancellation is checked before each design any lane claims, and (local path only)
//! between each design's four axis sweeps in [`engine::sweep::compute_tilt_curves_locally`].
//! A `catch_unwind` wraps each per-design attempt on every lane, so one bad design counts
//! as `failed` instead of killing its lane. A design's tilt curves are all-or-nothing:
//! `TiltPerformanceCurves` needs all four axes to construct. A cancel observed mid-design
//! therefore abandons that design without saving anything; designs that finished stay saved.
//!
//! `TILT_CURVES` is a single blocking request/response (unlike `RENDER`'s progressive
//! `StreamEvent` stream). A remote dispatch checks `cancel` once before sending, then
//! blocks for the whole round trip, so `tilt_batch_remote_title` can only show the design
//! a dispatcher started most recently, never a sub-progress fraction.
//!
//! Progress is a COMPLETED COUNT (`tilt_batch_design_index`), not a current index, because
//! local and remote lanes may each be mid-design at once. `tilt_batch_local_active` counts
//! how many of the `tilt_batch_local_lane_total` local lanes have a design claimed. The
//! remote side runs several dispatchers too: `tilt_batch_remote_in_flight` counts the
//! designs on the remote, and `tilt_batch_remote_title` names the design started most
//! recently.
//!
//! The remote side runs `AppSettings::remote_batch_lanes` dispatchers at once
//! (`batch_queue::remote_lane_count`), all claiming from the same queue, so a fast remote
//! is never left waiting on one claim. A design stays all-or-nothing on the remote too: one
//! dispatcher sends the whole design in one request, so there is nothing to split.
//!
//! Split into [`scan`] (manual missing-curves scan), [`engine`] (per-design resolve,
//! compute and dispatch, plus the local lane runner), [`remote_lane`] (the remote
//! dispatcher loop) and [`wiring`] (`spawn_tilt_batch` and `setup_tilt_batch_callbacks`,
//! the UI glue), the same seams `gui::batch::preview` uses.

mod engine;
mod remote_lane;
mod scan;
mod wiring;

pub use engine::{save_tilt_curves_for_entry, tilt_curves_for_stone};
#[cfg(feature = "ui-harness")]
pub(in crate::gui) use wiring::note_holders;
pub(in crate::gui) use wiring::release_window_state;
pub use wiring::{offer_regeneration, setup_tilt_batch_callbacks};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::batch::test_records::{
        ASC_MAST, ASC_TEXT, ENTRY_ID, angle_table_planes, angle_table_record, assert_gem_geometry,
        gem_record, has_plane_at, record_with,
    };
    use pleochroma::{
        geometry::{GpuFacetPlane, cuts::StandardGemCuts, tool::StoneGeometry},
        optics::materials::GemMaterial,
    };
    use pleochroma_vault::{
        db::sqlite::Database,
        model::{
            entry::FullDiagramRecord,
            tilt_curves::{AxisTiltCurves, TILT_CURVE_AXIS_COUNT, TiltPerformanceCurves},
        },
    };
    use std::{
        collections::BTreeSet,
        sync::{Mutex, atomic::AtomicBool},
    };

    /// The tilt engine's own geometry step for `full`, plus the ids its batch
    /// context counted as angle-table fallbacks.
    fn tilt_planes(full: &FullDiagramRecord) -> (Option<Vec<GpuFacetPlane>>, BTreeSet<i64>) {
        let db = Mutex::new(Database::new(Some(":memory:")).expect("in-memory database opens"));
        let angle_table_entries = Mutex::new(BTreeSet::new());
        let ctx = engine::BatchContext {
            db: &db,
            material_candidates: &[],
            custom_materials: &[],
            angle_table_entries: &angle_table_entries,
        };
        let planes =
            engine::record_planes_with_frosting(&ctx, full).map(|(resolved, _)| resolved.planes);
        let fallbacks = angle_table_entries
            .into_inner()
            .expect("the fallback set is never poisoned in a test");
        (planes, fallbacks)
    }

    /// A record whose only design file is a `.gem` is swept with the `.gem`'s
    /// real masts, not the angle table's, and is not counted as a fallback.
    #[test]
    fn tilt_batch_sweeps_a_gem_only_record_from_its_design_file() {
        let full = gem_record();
        let (planes, fallbacks) = tilt_planes(&full);
        let planes = planes.expect("the .gem yields planes");
        assert_gem_geometry(&planes);
        assert_ne!(planes, angle_table_planes(&full));
        assert!(fallbacks.is_empty());
        // The detail view resolves the same record to the same stone.
        assert_eq!(
            planes,
            crate::gui::editor::resolve_catalogue_planes(&full).planes
        );
    }

    /// A record whose `.asc` masts differ from its angle table is swept from the
    /// `.asc`.
    #[test]
    fn tilt_batch_prefers_the_asc_over_a_differing_angle_table() {
        let full = record_with("design.asc", ASC_TEXT.as_bytes().to_vec());
        let (planes, fallbacks) = tilt_planes(&full);
        let planes = planes.expect("the .asc yields planes");
        assert!(has_plane_at(&planes, ASC_MAST));
        assert_ne!(planes, angle_table_planes(&full));
        assert!(fallbacks.is_empty());
    }

    /// A record with no design file keeps the angle-table geometry the batch
    /// always swept, and is counted as a fallback.
    #[test]
    fn tilt_batch_sweeps_a_record_without_a_design_file_from_its_angle_table() {
        let full = angle_table_record();
        let (planes, fallbacks) = tilt_planes(&full);
        assert_eq!(planes, Some(angle_table_planes(&full)));
        assert_eq!(fallbacks, BTreeSet::from([ENTRY_ID]));
    }

    /// A corrupt `.gem` falls back to the angle table without panicking.
    #[test]
    fn tilt_batch_falls_back_to_the_angle_table_for_a_corrupt_gem() {
        let full = record_with("broken.gem", vec![1, 2, 3]);
        let (planes, fallbacks) = tilt_planes(&full);
        assert_eq!(planes, Some(angle_table_planes(&full)));
        assert_eq!(fallbacks, BTreeSet::from([ENTRY_ID]));
    }

    /// The single-design entry point, called through this module's re-export. An already-set
    /// `cancel` flag returns before the local sweep starts, so the test stays fast while
    /// still proving the re-exported name resolves and returns a value.
    #[test]
    fn tilt_curves_for_stone_honours_an_already_set_cancel_flag() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        let cancel = AtomicBool::new(true);
        assert!(
            tilt_curves_for_stone(
                StoneGeometry::planes_only(&planes),
                &[],
                &material,
                None,
                &cancel,
                &mut |_| {}
            )
            .is_none()
        );
    }

    /// Runs the single-design entry point to completion, the call the Edit tab's
    /// "Compute Tilt Curves" button makes off the UI thread (see
    /// `setup_batch_tilt_for_open_design_callback`). Asserts a real, non-degenerate curve
    /// set, not merely `Some` of a default result.
    #[test]
    fn tilt_curves_for_stone_computes_a_real_local_sweep_for_the_open_design_path() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        let cancel = AtomicBool::new(false);
        let mut steps = 0usize;
        let curves = tilt_curves_for_stone(
            StoneGeometry::planes_only(&planes),
            &[],
            &material,
            None,
            &cancel,
            &mut |_| {
                steps += 1;
            },
        )
        .expect("a standard round brilliant must sweep successfully");
        assert_eq!(steps, 724, "one progress report per raytrace evaluation");
        assert!(
            curves
                .axes
                .iter()
                .any(|axis| axis.brilliance_pct.iter().any(|&v| v > 0.0)),
            "expected at least one nonzero brilliance sample across the swept axes -- \
             a real sweep, not a degenerate all-zero placeholder"
        );
    }

    fn blank_curves() -> TiltPerformanceCurves {
        let axis = AxisTiltCurves {
            brilliance_pct: [0.0; pleochroma_vault::model::tilt_curves::TILT_CURVE_POINTS_PER_AXIS],
            extinction_pct: [0.0; pleochroma_vault::model::tilt_curves::TILT_CURVE_POINTS_PER_AXIS],
            windowing_pct: [0.0; pleochroma_vault::model::tilt_curves::TILT_CURVE_POINTS_PER_AXIS],
        };
        TiltPerformanceCurves {
            axes: [axis; TILT_CURVE_AXIS_COUNT],
        }
    }

    /// `save_tilt_curves_for_entry` must return `false`, not panic, for an `entry_id` with no
    /// row: `diagram_tilt_curves.entry_id` is a foreign key into `diagram_entries`.
    #[test]
    fn save_tilt_curves_for_entry_fails_cleanly_for_a_nonexistent_entry() {
        let db = Database::new(Some(":memory:")).expect("in-memory database must open");
        let db = std::sync::Mutex::new(db);
        let saved = save_tilt_curves_for_entry(&db, 999_999, &blank_curves());
        assert!(!saved, "no diagram_entries row exists for this id");
    }
}
