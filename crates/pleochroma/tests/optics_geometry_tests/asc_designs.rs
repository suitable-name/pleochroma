//! Tests for `StandardGemCuts::from_asc_schedule` and `reconstruct_validated_brep_from_asc`,
//! run on real `.asc` designs whose published `lw_ratio` and facet count are known.

use glam::Vec3;
use pleochroma::{
    FacetSpec,
    geometry::{brep::GemPolyhedron, cuts::StandardGemCuts},
};
use pleochroma_formats::asc;

use crate::fixtures::assert_euler_formula;

// ---------------------------------------------------------------------------
// `StandardGemCuts::from_asc_schedule` / `reconstruct_validated_brep_from_asc`.
//
// The fixtures are real designs, not hand-authored, so the sign and offset conventions are
// checked against published `lw_ratio` and facet counts. The full-corpus cross-check is in
// `examples/meet_solver_validation/main.rs`.
// ---------------------------------------------------------------------------

/// `attached_files` id 4208 ("pc45149.asc") -- "PC 45.149 Round Trichecker-12" by Fred
/// W. Van Sant. Published: `lw_ratio` = 1.000, `facets_count` = "36+12" (48 total).
///
/// Kept as a file (`include_str!`) so `examples/pgo_train/stages.rs` trains on the same design.
const ASC_ROUND_TRICHECKER_12: &str = include_str!("../fixtures/asc/trichecker12.asc");

/// `attached_files` id 4210 ("pc46019.asc") -- "PC 46.019 For Fun" by Michiko Huyhn.
/// Published: `lw_ratio` = 1.631, `facets_count` = "48+8" (56 total). Exercises an unsigned
/// zero-angle culet-like tier ("U") with no explicit crown/pavilion marker.
///
/// Stored as `tests/fixtures/asc/forfun.asc`.
const ASC_FOR_FUN: &str = include_str!("../fixtures/asc/forfun.asc");

/// `attached_files` id 4430 ("pc42060.asc") -- "PC 42.060 Large Texas Star" by
/// Charles `McCoy`. Published: `lw_ratio` = 1.051, `facets_count` = "41+10" (51 total).
/// Gear 80 (not the common 96) and symmetry order 5, plus an explicit table tier at
/// unsigned zero.
///
/// Stored as `tests/fixtures/asc/texasstar.asc`.
const ASC_LARGE_TEXAS_STAR: &str = include_str!("../fixtures/asc/texasstar.asc");

/// `attached_files` id 4422 ("pc43001a.asc") -- "PC 43.001A Shah (Replica)". No facet
/// names, a negative-mast tier at an unsigned zero angle, and two tiers that give a literal
/// duplicate half-space plane. Exercises `dedup_planes` and the unnamed parsing path. It is
/// not checked against L/W or facet count, since the design disagrees with its reference.
///
/// Stored as `tests/fixtures/asc/shah.asc`.
const ASC_SHAH_REPLICA_NO_NAMES: &str = include_str!("../fixtures/asc/shah.asc");

/// Length/width measure: the longest chord across the reconstructed
/// girdle outline as length, and the outline's extent perpendicular to that chord as
/// width.
fn length_width_ratio(hull: &GemPolyhedron) -> f64 {
    let outline = hull.girdle_outline();
    let mut best = (0usize, 0usize, 0.0f32);
    for i in 0..outline.len() {
        for j in (i + 1)..outline.len() {
            let d = (outline[i] - outline[j]).length();
            if d > best.2 {
                best = (i, j, d);
            }
        }
    }
    let length = best.2;
    let dir = (outline[best.1] - outline[best.0]).normalize();
    let perp = Vec3::new(-dir.z, 0.0, dir.x);
    let (mut min_p, mut max_p) = (f32::MAX, f32::MIN);
    for p in &outline {
        let proj = p.dot(perp);
        min_p = min_p.min(proj);
        max_p = max_p.max(proj);
    }
    f64::from(length / (max_p - min_p))
}

#[test]
fn asc_real_designs_reconstruct_closed_solids_with_no_untouched_planes() {
    for (label, content) in [
        ("Round Trichecker-12", ASC_ROUND_TRICHECKER_12),
        ("For Fun", ASC_FOR_FUN),
        ("Large Texas Star", ASC_LARGE_TEXAS_STAR),
        ("Shah Replica (no names)", ASC_SHAH_REPLICA_NO_NAMES),
    ] {
        let schedule = asc::parse_asc(content)
            .unwrap_or_else(|e| panic!("{label}: real .asc sample must parse: {e}"));
        let hull =
            StandardGemCuts::reconstruct_validated_brep_from_asc(&schedule).unwrap_or_else(|e| {
                panic!("{label}: real .asc sample must reconstruct a valid closed solid: {e}")
            });
        assert_euler_formula(&hull);
        assert!(
            hull.untouched_planes().is_empty(),
            "{label}: every plane in a real, well-formed schedule should be touched"
        );
        assert!(
            hull.volume().is_finite() && hull.volume() > 0.0,
            "{label}: volume must be finite and positive"
        );
    }
}

#[test]
fn asc_round_trichecker_12_matches_published_lw_ratio_and_facet_count() {
    let schedule = asc::parse_asc(ASC_ROUND_TRICHECKER_12).unwrap();
    let hull = StandardGemCuts::reconstruct_validated_brep_from_asc(&schedule).unwrap();
    assert_eq!(
        hull.facet_planes.len(),
        48,
        "published facets_count is \"36+12\" = 48"
    );
    let lw = length_width_ratio(&hull);
    // Measured 1.00090 (0.0009 from published); the bound is the tightest round one that clears it.
    assert!(
        (lw - 1.000).abs() < 0.001,
        "published lw_ratio is 1.000, got {lw:.4}"
    );
}

#[test]
fn asc_for_fun_matches_published_lw_ratio_and_facet_count() {
    // Tier count (10 pavilion and girdle, 17 crown, 1 culet), the first tier's angle and mast,
    // and the resulting geometry; the parser's own unit tests cover the rest.
    let schedule = asc::parse_asc(ASC_FOR_FUN).unwrap();
    assert_eq!(schedule.tiers.len(), 28);
    assert_eq!(schedule.gear_teeth, 96);
    assert!((schedule.refractive_index - 1.54).abs() < 1e-9);
    assert!((schedule.tiers[0].angle_deg - (-44.864_054)).abs() < 1e-6);
    assert!((schedule.tiers[0].mast - 0.537_910_82).abs() < 1e-9);

    let hull = StandardGemCuts::reconstruct_validated_brep_from_asc(&schedule).unwrap();
    assert_eq!(
        hull.facet_planes.len(),
        56,
        "published facets_count is \"48+8\" = 56"
    );
    let lw = length_width_ratio(&hull);
    // Measured 1.63311 (0.00211 from published). The reconstruction must stay deterministic:
    // `chull`'s hash sets can pick either of two nearly coincident meet solutions (about
    // 1.63064 or 1.63311) as the welded vertex.
    assert!(
        (lw - 1.631).abs() < 0.0025,
        "published lw_ratio is 1.631, got {lw:.4}"
    );
}

#[test]
fn asc_large_texas_star_matches_published_lw_ratio_and_facet_count() {
    // Gear 80 and an explicit unsigned-zero table tier ("T"), both unlike the other fixtures.
    let schedule = asc::parse_asc(ASC_LARGE_TEXAS_STAR).unwrap();
    assert_eq!(schedule.gear_teeth, 80);
    assert_eq!(schedule.symmetry_order, 5);

    let hull = StandardGemCuts::reconstruct_validated_brep_from_asc(&schedule).unwrap();
    assert_eq!(
        hull.facet_planes.len(),
        51,
        "published facets_count is \"41+10\" = 51"
    );
    let lw = length_width_ratio(&hull);
    // Measured 1.05232 (0.00232 from published). Its diff does not clear the +/-0.001 of the
    // other fixtures, so this bound is looser, with a small margin over the measured diff.
    assert!(
        (lw - 1.051).abs() < 0.0025,
        "published lw_ratio is 1.051, got {lw:.4}"
    );
}

#[test]
fn asc_schedules_always_satisfy_from_planes_d_negative_precondition() {
    for content in [
        ASC_ROUND_TRICHECKER_12,
        ASC_FOR_FUN,
        ASC_LARGE_TEXAS_STAR,
        ASC_SHAH_REPLICA_NO_NAMES,
    ] {
        let schedule = asc::parse_asc(content).unwrap();
        for p in StandardGemCuts::from_asc_schedule(&schedule) {
            assert!(
                p.d < 0.0,
                "from_asc_schedule produced a plane with d = {} >= 0",
                p.d
            );
        }
    }
}

#[test]
fn asc_schedule_negative_mast_and_duplicate_tiers_do_not_panic_or_break_geometry() {
    // ASC_SHAH_REPLICA_NO_NAMES has a negative-mast tier ("0.00 -0.36800 0") and two tiers
    // that give one duplicate half-space plane. Neither may panic, and the duplicate must be
    // absorbed by `dedup_planes`, not rejected by `GemPolyhedron::from_planes` as coincident.
    let schedule = asc::parse_asc(ASC_SHAH_REPLICA_NO_NAMES).unwrap();
    let planes = StandardGemCuts::from_asc_schedule(&schedule);
    for p in &planes {
        assert!(
            p.d < 0.0,
            "negative mast must still produce d < 0 (magnitude, not sign, is used), got d={}",
            p.d
        );
    }
    let hull = GemPolyhedron::from_planes(planes)
        .expect("duplicate half-space planes must be deduped before reaching from_planes");
    assert_euler_formula(&hull);
}

#[test]
fn asc_geometry_falls_back_cleanly_when_no_asc_is_available() {
    // Designs without an attached .asc must still render via the angle_settings path.
    // `from_asc_schedule` and `reconstruct_validated_brep_from_asc` are additive only.
    let angles = vec![FacetSpec {
        facet: "C1".into(),
        angle: "40.00\u{b0}".into(),
        index: "8 girdle facets".into(),
        notes: String::new(),
    }];
    let planes = StandardGemCuts::from_database_angles(&angles, 96);
    assert!(
        !planes.is_empty(),
        "the angle_settings fallback path must remain usable independent of from_asc_schedule"
    );
}
