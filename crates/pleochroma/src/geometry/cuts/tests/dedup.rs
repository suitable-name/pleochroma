//! `.asc`-reconstruction plane dedup coverage: bin-edge straddling, first-
//! occurrence-wins, and the normal/offset tolerance boundaries.

use glam::{DVec3, Vec3};
use pleochroma_formats::asc::parse_asc;

use crate::geometry::{
    cuts::{
        StandardGemCuts,
        asc_schedule::{dedup_planes, normals_coincide},
    },
    plane::GpuFacetPlane,
};

/// Two offsets that are close but straddle a quantization bin's rounding boundary.
/// Built at the edge between bins `0` and `1` (`0.5 * QUANT`) `+/- 1e-7`. A
/// `(v / QUANT).round()` bin lookup would keep both, though they are only `2e-7`
/// apart; the tolerance compare must merge them.
#[test]
fn dedup_merges_planes_straddling_a_quantization_bin_edge() {
    const QUANT: f32 = 1.0 / 2048.0;
    let bin_edge = 0.5 * QUANT;
    let normal = Vec3::new(0.0, 0.0, 1.0);
    let planes = vec![
        GpuFacetPlane::new(normal, bin_edge - 1e-7),
        GpuFacetPlane::new(normal, bin_edge + 1e-7),
    ];
    let deduped = dedup_planes(planes);
    assert_eq!(
        deduped.len(),
        1,
        "two near-identical offsets straddling a bin edge must merge into one plane"
    );
}

/// The first occurrence is the one that survives, not the last -- callers that
/// keep the earlier metadata (name, source tier) alongside the plane depend on
/// this.
#[test]
fn dedup_keeps_the_first_occurrence() {
    const QUANT: f32 = 1.0 / 2048.0;
    let bin_edge = 0.5 * QUANT;
    let normal = Vec3::new(0.0, 0.0, 1.0);
    let first = GpuFacetPlane::new(normal, bin_edge - 1e-7);
    let second = GpuFacetPlane::new(normal, bin_edge + 1e-7);
    let deduped = dedup_planes(vec![first, second]);
    assert_eq!(deduped, vec![first]);
}

/// Two planes with genuinely different offsets (well beyond the dedup tolerance)
/// must both survive -- the tolerance compare must not over-merge.
#[test]
fn dedup_keeps_genuinely_distinct_planes() {
    const QUANT: f32 = 1.0 / 2048.0;
    let normal = Vec3::new(0.0, 0.0, 1.0);
    let planes = vec![
        GpuFacetPlane::new(normal, 0.0),
        GpuFacetPlane::new(normal, 10.0 * QUANT),
    ];
    let deduped = dedup_planes(planes.clone());
    assert_eq!(
        deduped, planes,
        "genuinely distinct planes must both be kept"
    );
}

/// Two planes with the same offset but different (non-parallel) normals are
/// different half-spaces, not duplicates, regardless of how close their offsets
/// are.
#[test]
fn dedup_keeps_planes_with_different_normals_even_at_the_same_offset() {
    let planes = vec![
        GpuFacetPlane::new(Vec3::new(0.0, 0.0, 1.0), 1.0),
        GpuFacetPlane::new(Vec3::new(1.0, 0.0, 0.0), 1.0),
    ];
    let deduped = dedup_planes(planes.clone());
    assert_eq!(deduped, planes);
}

/// Two facets at the same offset whose normals are genuinely ~1 degree
/// apart -- not a near-bit-identical duplicate -- must both survive.
///
/// A looser epsilon (`1 - 1/2048`, ~1.79 degrees) would silently collapse these
/// into a single half-space; the `1 - cos <= 2e-7` rule of [`normals_coincide`]
/// (~0.036 degrees) does not.
#[test]
fn dedup_keeps_planes_one_degree_apart() {
    let normal_a = Vec3::new(0.0, 0.0, 1.0);
    let angle = 1.0f32.to_radians();
    let normal_b = Vec3::new(angle.sin(), 0.0, angle.cos());
    let planes = vec![
        GpuFacetPlane::new(normal_a, -1.0),
        GpuFacetPlane::new(normal_b, -1.0),
    ];
    let deduped = dedup_planes(planes.clone());
    assert_eq!(
        deduped, planes,
        "planes one degree apart must not be merged into a single half-space"
    );
}

/// The first occurrence survives even when a later duplicate has the smaller `|d|`.
/// Substituting the tighter plane would break per-tier attribution in callers that
/// walk `from_asc_schedule`'s output positionally (see [`dedup_planes`]).
#[test]
fn dedup_keeps_the_first_occurrence_even_when_a_later_duplicate_is_tighter() {
    let normal = Vec3::new(0.0, 0.0, 1.0);
    let looser_first = GpuFacetPlane::new(normal, -1.0 - 1e-7);
    let tighter_second = GpuFacetPlane::new(normal, -1.0);
    let deduped = dedup_planes(vec![looser_first, tighter_second]);
    assert_eq!(
        deduped,
        vec![looser_first],
        "the first-arriving plane must survive even though the second is tighter"
    );
}

/// A shallow-angle tier with many index positions keeps every facet as its own
/// plane. Guards against a `1 - 1e-5` dot-product bound merging adjacent facets
/// at 1, 2 and 3.5 degrees, each of which is a distinct facet.
#[test]
fn dedup_keeps_widely_spaced_shallow_tier_facets() {
    for &angle in &[1.0f64, 2.0, 3.5] {
        let indices: Vec<String> = (0..96).map(|i| i.to_string()).collect();
        let text = format!(
            "GemCad 5.0\ng 96 0.0\ny 1 n\nI 1.54\nH Test\na {angle:.6} 0.5 {}\n",
            indices.join(" ")
        );
        let schedule = parse_asc(&text).expect("valid .asc text");
        let planes = StandardGemCuts::from_asc_schedule(&schedule);
        assert_eq!(
            planes.len(),
            96,
            "angle {angle}: all 96 index positions must survive as distinct planes"
        );
    }
}

/// The offset tolerance must not depend on the order of a tier's mast revisions.
/// Adjacent masts (1.0, 1.0004, 1.0008) pair up within tolerance, but the two ends
/// do not. Reading the revisions forwards or backwards keeps the same two end values.
#[test]
fn dedup_offset_tolerance_is_order_independent_for_a_tiers_own_revision_history() {
    let mk = |masts: &[f64]| {
        use std::fmt::Write;
        let mut text = String::from("GemCad 5.0\ng 96 0.0\ny 1 n\nI 1.54\nH Test\n");
        for m in masts {
            let _ = writeln!(text, "a 41.000000 {m:.6} 0");
        }
        let schedule = parse_asc(&text).expect("valid .asc text");
        StandardGemCuts::from_asc_schedule(&schedule)
            .iter()
            .map(|p| -p.d)
            .collect::<Vec<f32>>()
    };
    let forward = mk(&[1.0, 1.0004, 1.0008]);
    let reversed = mk(&[1.0008, 1.0004, 1.0]);
    assert_eq!(
        forward.len(),
        2,
        "the two masts 4e-4 apart from each other, on opposite ends of the chain, must both survive"
    );
    assert_eq!(
        reversed.len(),
        forward.len(),
        "reading the same revision history backwards must keep the same number of planes"
    );
    let mut forward_sorted = forward.clone();
    forward_sorted.sort_by(f32::total_cmp);
    let mut reversed_sorted = reversed.clone();
    reversed_sorted.sort_by(f32::total_cmp);
    assert_eq!(
        forward_sorted, reversed_sorted,
        "forward {forward:?} and reversed {reversed:?} must keep the same set of mast values"
    );
}

/// Deterministic generic `(angle, index)` sample `k`: the angle walks the crown
/// range `5..85` degrees on a golden-ratio sequence, the index a 96-tooth wheel.
fn sample_angle_index(k: u32) -> (f64, u32) {
    let angle = 80.0f64.mul_add((f64::from(k) * 0.618_033_988_749_895).fract(), 5.0);
    (angle, (k * 37) % 96)
}

/// A duplicate must always merge, whatever the `f32` normalisation rounding. A raw
/// dot-product bound tight enough to separate real neighbours can miss the copy when
/// the squared length falls just below `1`.
#[test]
fn an_index_listed_twice_survives_as_exactly_one_plane() {
    for k in 0..2000 {
        let (angle, index) = sample_angle_index(k);
        let text = format!(
            "GemCad 5.0\ng 96 0.0\ny 1 n\nI 1.54\nH Test\na {angle:.6} 0.5 {index} {index}\n"
        );
        let schedule = parse_asc(&text).expect("valid .asc text");
        let planes = StandardGemCuts::from_asc_schedule(&schedule);
        assert_eq!(
            planes.len(),
            1,
            "angle {angle:.6}, index {index}: the twice-listed index must yield one plane"
        );
    }
}

/// The coincidence rule is about direction only: it normalises in `f64`, never
/// matches a zero normal, and separates directions a degree apart.
#[test]
fn normals_coincide_compares_directions_only() {
    let n = DVec3::new(0.3, 0.5, 0.8);
    assert!(normals_coincide(n, n));
    assert!(normals_coincide(n, n * 0.999_999_9));
    assert!(!normals_coincide(DVec3::ZERO, DVec3::ZERO));
    assert!(!normals_coincide(DVec3::X, DVec3::Y));
    let one_degree = 1.0f64.to_radians();
    assert!(!normals_coincide(
        DVec3::Z,
        DVec3::new(one_degree.sin(), 0.0, one_degree.cos())
    ));
}
