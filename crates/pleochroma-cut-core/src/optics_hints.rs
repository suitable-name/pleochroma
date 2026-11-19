//! Pure critical-angle math shared by the material retarget proposal and the solid
//! preview's risk overlay, so both answer "how close is this facet to windowing" the same way.
//!
//! # Angle convention
//!
//! Every angle here is a [`crate::design::ConstraintTier::angle_deg`] value: signed degrees
//! from the girdle plane, negative for a pavilion facet and non-negative for a crown. A
//! sign-negative `-0.0` is the culet and `+0.0` the table; see that field's doc comment.
//! [`tier_margin_deg`], [`windowing_risk`] and [`retarget_angle_deg`] read the magnitude of
//! the angle and keep the input's sign on any angle they return.
//!
//! Windowing is a pavilion phenomenon: light entering through the crown that hits
//! a pavilion facet below the critical angle refracts straight out instead of
//! reflecting back to the eye. A caller is expected to apply these functions only
//! to pavilion tiers (`angle_deg < 0.0`); nothing here enforces it, since a crown
//! tier's own risk is a distinct, unmodeled concern left to crown-specific UI policy.
//!
//! # Domain
//!
//! `n` (a refractive index) is assumed `> 1.0`, which every gem material this crate
//! produces satisfies. [`critical_angle_deg`] is not meaningful outside that: `asin` of
//! `1 / n > 1` is `NaN`, and `n == 1.0` gives `90.0`. [`windowing_risk`] maps a `NaN`
//! margin to [`Risk::Windows`]; left alone, it would compare as [`Risk::Safe`].

use crate::design::{Design, hinge::tier_plane_ranges};
use pleochroma::geometry::{
    meet_solver::{Block, SolvedTier, classify_blocks},
    stone_metrics::{SolidStatus, build_solid_mesh},
};
use std::collections::BTreeSet;

/// The critical angle for total internal reflection at refractive index `n`.
///
/// In degrees: `asin(1/n)`. Below this angle (measured from the facet's own
/// normal), light hitting a facet from inside the stone refracts out instead of
/// reflecting -- see the module doc comment's "Domain" section for this
/// function's `n > 1.0` assumption.
#[must_use]
pub fn critical_angle_deg(n: f64) -> f64 {
    (1.0 / n).asin().to_degrees()
}

/// How many degrees `tier_angle_deg` sits above index `n`'s own
/// [`critical_angle_deg`].
///
/// `tier_angle_deg` is a tier's own authored angle from the girdle plane -- see
/// the module doc comment's "Angle convention" section. Positive means the facet
/// sits past the critical angle with margin to spare; negative means it has
/// already crossed below it (light windows through).
#[must_use]
pub fn tier_margin_deg(tier_angle_deg: f64, n: f64) -> f64 {
    tier_angle_deg.abs() - critical_angle_deg(n)
}

/// A tier's windowing risk at refractive index `n`, from its margin over the
/// critical angle (see [`tier_margin_deg`]) -- meaningful for a pavilion tier's
/// angle specifically (see the module doc comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// Margin at least 2 degrees -- comfortably past the critical angle.
    Safe,
    /// Margin in `[0, 2)` degrees -- past the critical angle, but thin enough
    /// that a small edit, a manufacturing tolerance, or a different material
    /// could tip it into windowing.
    Marginal,
    /// Margin below zero -- already below the critical angle for `n`: light
    /// leaks straight through this facet instead of reflecting.
    Windows,
}

/// Classifies a tier's windowing risk at index `n`: [`Risk::Windows`] below a zero margin,
/// [`Risk::Marginal`] up to 2 degrees, and [`Risk::Safe`] above that.
///
/// `n <= 1.0`, or a `NaN` `n` or `tier_angle_deg`, reports [`Risk::Windows`]. See the
/// module's "Domain" section.
#[must_use]
pub fn windowing_risk(tier_angle_deg: f64, n: f64) -> Risk {
    let margin = tier_margin_deg(tier_angle_deg, n);
    if margin.is_nan() {
        return Risk::Windows;
    }
    if margin < 0.0 {
        Risk::Windows
    } else if margin < 2.0 {
        Risk::Marginal
    } else {
        Risk::Safe
    }
}

/// The critical-angle-shift retarget proposal for one tier moving from
/// refractive index `n_from` to `n_to`.
///
/// The "critical-angle shift (deterministic, default)" algorithm:
/// `theta' = critical_angle_deg(n_to) + (theta - critical_angle_deg(n_from))`,
/// which preserves the tier's own margin over the critical angle exactly (see
/// this module's own tests). Reads and returns a signed `angle_deg` (see the
/// module doc comment's "Angle convention" section): the input's own sign is
/// preserved on the output, so this is safe to call with a pavilion tier's
/// (negative) `angle_deg` directly.
///
/// This is the pure math half. The slope limits are [`guard_retargeted_angle_deg`]. The
/// policy around it (flagging a tier that falls below the new critical angle, and the crown
/// "scale by ratio" alternative) lives in `pleochroma-editor`'s retarget code.
#[must_use]
pub fn retarget_angle_deg(tier_angle_deg: f64, n_from: f64, n_to: f64) -> f64 {
    // `is_sign_negative`, not `< 0.0`: this crate's own templates author a
    // pavilion culet at exactly `-0.0` to record its side (see
    // `ConstraintTier::standard_round_brilliant`). Since `-0.0 < 0.0` is `false`,
    // using `<` would drop that marker and retarget it to `+new_theta` (a crown
    // angle) instead of preserving the pavilion side.
    let sign = if tier_angle_deg.is_sign_negative() {
        -1.0
    } else {
        1.0
    };
    let theta = tier_angle_deg.abs();
    let new_theta = critical_angle_deg(n_to) + (theta - critical_angle_deg(n_from));
    sign * new_theta
}

/// The flattest slope, in degrees from the horizontal, a retarget leaves on a crown
/// or pavilion facet.
///
/// A shifted angle that would drop below this (or cross the horizontal altogether,
/// which turns a crown facet into a pavilion facet) is held here instead -- see
/// [`guard_retargeted_angle_deg`]. A facet that was already flatter than this keeps
/// its own, flatter angle as the floor.
pub const MIN_RETARGET_ANGLE_DEG: f64 = 1.0;

/// The steepest slope, in degrees from the horizontal, a retarget proposes.
///
/// Half a degree short of vertical, so the facet is never reclassified as a girdle
/// facet (see `pleochroma::geometry::meet_solver::classify_blocks`).
pub const MAX_RETARGET_ANGLE_DEG: f64 = 89.5;

/// Angles with a magnitude below this count as horizontal: the table (`+0.0`), the
/// culet (`-0.0`) and any other flat plane. A retarget never changes them.
pub const HORIZONTAL_ANGLE_EPS_DEG: f64 = 1e-9;

/// `true` for a flat facet: the table, the culet or any other horizontal plane.
#[must_use]
pub const fn is_horizontal_angle_deg(angle_deg: f64) -> bool {
    angle_deg > -HORIZONTAL_ANGLE_EPS_DEG && angle_deg < HORIZONTAL_ANGLE_EPS_DEG
}

/// What [`guard_retargeted_angle_deg`] did to a shifted angle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AngleGuard {
    /// The shifted angle was fine as it was.
    Within,
    /// The shift would have made the facet flatter than [`MIN_RETARGET_ANGLE_DEG`]
    /// (or flipped it to the other side of the girdle); it is held at the minimum.
    HeldAtMinimum,
    /// The shift would have made the facet steeper than [`MAX_RETARGET_ANGLE_DEG`];
    /// it is held at the maximum.
    HeldAtMaximum,
}

/// Keeps a shifted angle on the same side of the horizontal as the angle it came from,
/// and between the flattest and steepest slopes a retarget allows.
///
/// `old_deg` is the facet's current signed angle (see the module doc comment's "Angle
/// convention"); `shifted_deg` is what the shift formula produced. The result has the
/// sign of `old_deg` (a sign-negative zero counts as negative), a magnitude of at least
/// `min(MIN_RETARGET_ANGLE_DEG, |old_deg|)` and at most [`MAX_RETARGET_ANGLE_DEG`].
/// Without the floor a crown facet near the horizontal could be shifted past it and
/// turn into a pavilion-side plane. A non-finite `shifted_deg` leaves the facet where it is.
///
/// Callers must not pass a horizontal `old_deg` ([`is_horizontal_angle_deg`]): those
/// facets are never shifted at all.
#[must_use]
pub const fn guard_retargeted_angle_deg(old_deg: f64, shifted_deg: f64) -> (f64, AngleGuard) {
    if !shifted_deg.is_finite() {
        return (old_deg, AngleGuard::Within);
    }
    let sign = if old_deg.is_sign_negative() {
        -1.0
    } else {
        1.0
    };
    let magnitude = shifted_deg * sign;
    let old_magnitude = old_deg * sign;
    let floor = if old_magnitude < MIN_RETARGET_ANGLE_DEG {
        old_magnitude
    } else {
        MIN_RETARGET_ANGLE_DEG
    };
    if magnitude < floor {
        (sign * floor, AngleGuard::HeldAtMinimum)
    } else if magnitude > MAX_RETARGET_ANGLE_DEG {
        (sign * MAX_RETARGET_ANGLE_DEG, AngleGuard::HeldAtMaximum)
    } else {
        (shifted_deg, AngleGuard::Within)
    }
}

/// The crown's own windowing estimate.
///
/// How many degrees of margin remain at a PAVILION facet
/// (angle `pavilion_angle_deg`, from the girdle) for light that entered
/// through a CROWN facet (angle `crown_angle_deg`, from the girdle) rather
/// than straight down through the table, at refractive index `n`.
///
/// # Derivation (Snell's law)
///
/// Both angles are read as magnitudes (see the module doc comment's "Angle
/// convention"): a facet tilted `theta` degrees from the (horizontal) girdle
/// plane has its own normal tilted `theta` degrees from the vertical viewing
/// axis -- the same reading [`tier_margin_deg`] already gives the plain
/// (table-only) pavilion model.
///
/// Follow one ray, in the 2-D radial cross-section through one crown facet and
/// the pavilion facet on the same azimuthal index: it starts vertical (straight
/// down, in air), hits the crown facet at incidence angle `c = crown_angle_deg`
/// from the crown facet's own normal (since that normal sits `c` degrees off
/// vertical), and refracts per Snell's law,
/// `sin(c) = n * sin(r)`, so `r = asin(sin(c) / n)`.
///
/// Because refraction bends the ray TOWARD the normal (entering the denser
/// stone), the ray's new direction is tilted `(c - r)` degrees from vertical
/// (less than the crown facet's own `c`, never zero) -- a vertical ray only ever
/// picks up tilt by refracting through a facet that is not flat, and `r < c` for
/// any `n > 1`.
///
/// That ray then reaches the pavilion facet, whose own normal sits
/// `p = pavilion_angle_deg` degrees off vertical, tilted toward the SAME side
/// the crown bent the ray toward (the two facets face each other across one
/// radial cross-section). Resolving both directions as unit vectors in that
/// plane and taking their dot product gives the angle of incidence at the
/// pavilion directly: `theta_pavilion = p + (c - r)`. Compare that against
/// [`critical_angle_deg`] exactly as [`tier_margin_deg`] compares the plain
/// table-only incidence angle `p` -- entering through a tilted crown facet
/// only ever ADDS incidence angle over the straight-through-the-table case
/// (`c - r >= 0`), so this margin is never smaller than the table-only
/// pavilion margin at the same `p`.
///
/// # This is an estimate, not a full ray trace
///
/// This follows one ray in the crown and pavilion facets' shared meridian plane. It ignores
/// every other path light takes through a real stone: internal reflections off other facets,
/// the crown's curvature away from that meridian, dispersion (each wavelength refracts by a
/// different `r`), and facets not radially opposite. Callers should label the result an
/// estimate.
#[must_use]
pub fn crown_window_margin_deg(pavilion_angle_deg: f64, crown_angle_deg: f64, n: f64) -> f64 {
    let c = crown_angle_deg.abs();
    let p = pavilion_angle_deg.abs();
    let r = (c.to_radians().sin() / n).asin().to_degrees();
    (p + (c - r)) - critical_angle_deg(n)
}

/// [`crown_window_margin_deg`] classified into the same three [`Risk`] bands as
/// [`windowing_risk`] -- an ESTIMATE (see that function's own doc comment), not
/// a full ray trace.
///
/// Same `n <= 1.0`/`NaN` guard as [`windowing_risk`]: a `NaN` margin
/// reports [`Risk::Windows`], never [`Risk::Safe`].
#[must_use]
pub fn crown_windowing_risk(pavilion_angle_deg: f64, crown_angle_deg: f64, n: f64) -> Risk {
    let margin = crown_window_margin_deg(pavilion_angle_deg, crown_angle_deg, n);
    if margin.is_nan() {
        return Risk::Windows;
    }
    if margin < 0.0 {
        Risk::Windows
    } else if margin < 2.0 {
        Risk::Marginal
    } else {
        Risk::Safe
    }
}

/// The refractive index the pavilion alone asks for, read from its shallowest tier.
///
/// A cheap hint, not a trace: see [`geometric_ri_hint`].
#[derive(Debug, Clone, PartialEq)]
pub struct GeometricRiHint {
    /// `1 / sin(angle)` of the shallowest pavilion tier: below this index light entering
    /// through the table leaks out of that tier.
    pub n_geo: f64,
    /// The code of that tier (`P2`).
    pub tier: String,
    /// The tier's angle from the girdle plane, in degrees (positive).
    pub angle_deg: f64,
}

/// The geometric refractive-index hint of `design`, from its solve `solved`.
///
/// `n_geo = 1 / sin(a_min)`, where `a_min` is the smallest angle among the pavilion tiers that
/// still have facets on the finished stone. The culet and any other flat tier are not counted,
/// nor are the girdle and the crown. `None` when `solved` is not one entry per tier, when the
/// stone does not close, or when no pavilion tier is left.
///
/// The hint holds only for light entering through the table: tilted views and light entering
/// through the crown are not counted.
#[must_use]
pub fn geometric_ri_hint(design: &Design, solved: &[SolvedTier]) -> Option<GeometricRiHint> {
    if solved.len() != design.tiers.len() {
        return None;
    }
    let planes = design.planes_from_solved(solved);
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        return None;
    };
    let live: BTreeSet<usize> = mesh
        .rings
        .iter()
        .filter(|(_, ring)| ring.len() >= 3)
        .map(|&(plane, _)| plane)
        .collect();
    let ranges = tier_plane_ranges(design, solved);
    let blocks = classify_blocks(&design.meet_tier_inputs());
    let (index, angle_deg) = design
        .tiers
        .iter()
        .zip(&blocks)
        .enumerate()
        .filter(|(_, (tier, block))| {
            **block == Block::Pavilion && !is_horizontal_angle_deg(tier.angle_deg)
        })
        .filter(|(index, _)| {
            ranges
                .get(*index)
                .is_some_and(|range| range.clone().any(|plane| live.contains(&plane)))
        })
        .map(|(index, (tier, _))| (index, tier.angle_deg.abs()))
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    let n_geo = 1.0 / angle_deg.to_radians().sin();
    if !(n_geo.is_finite() && n_geo > 1.0) {
        return None;
    }
    let tier = design
        .tier_codes()
        .flat
        .get(index)
        .map(|info| info.code.clone())?;
    Some(GeometricRiHint {
        n_geo,
        tier,
        angle_deg,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Diamond's own well-known critical angle (~24.4 degrees) is a real sanity
    /// check on the formula itself, not just a boundary case.
    #[test]
    fn critical_angle_matches_the_well_known_diamond_figure() {
        let angle = critical_angle_deg(2.417);
        assert!((angle - 24.4).abs() < 0.05, "{angle}");
    }

    /// A lower-index material has a WIDER critical angle: quartz (~1.544) must
    /// report a larger critical angle than diamond (~2.417).
    #[test]
    fn critical_angle_increases_as_refractive_index_decreases() {
        assert!(critical_angle_deg(1.544) > critical_angle_deg(2.417));
    }

    /// Only the magnitude of the angle matters -- a crown tier at +30 and a
    /// pavilion tier at -30 must report the identical margin at the same n.
    #[test]
    fn tier_margin_is_symmetric_for_crown_and_pavilion_signs() {
        let n = 1.62;
        assert_eq!(tier_margin_deg(30.0, n), tier_margin_deg(-30.0, n));
    }

    // --- windowing_risk boundaries ---

    /// Exercises both boundaries (`Windows`/`Marginal` at margin 0, `Marginal`/`Safe`
    /// at margin 2) as half-open on the low side, using an `n` chosen so
    /// `critical_angle_deg(n)` is a clean 20 degrees.
    #[test]
    fn windowing_risk_boundaries() {
        let n = 1.0 / 20.0f64.to_radians().sin();
        assert!((critical_angle_deg(n) - 20.0).abs() < 1e-9);

        assert_eq!(windowing_risk(-19.0, n), Risk::Windows);
        assert_eq!(windowing_risk(-(20.0 - 1e-9), n), Risk::Windows);
        assert_eq!(windowing_risk(-20.0, n), Risk::Marginal);
        assert_eq!(windowing_risk(-21.999, n), Risk::Marginal);
        assert_eq!(windowing_risk(-22.0, n), Risk::Safe);
        assert_eq!(windowing_risk(-22.001, n), Risk::Safe);
    }

    /// Sign-agnostic (see the module doc comment) -- a caller decides whether to
    /// apply it to a crown tier at all.
    #[test]
    fn windowing_risk_reads_crown_tiers_the_same_way_as_pavilion() {
        let n = 1.0 / 20.0f64.to_radians().sin();
        assert_eq!(windowing_risk(20.0, n), windowing_risk(-20.0, n));
    }

    // --- retarget_angle_deg ---

    /// Retargeting a synthetic pavilion tier from one index to another must
    /// preserve its margin over the critical angle to `1e-9`.
    #[test]
    fn retarget_preserves_margin_on_a_synthetic_pavilion_tier() {
        let n_from = 2.417; // diamond
        let n_to = 1.544; // quartz
        let original_angle = -45.0; // a synthetic pavilion tier
        let margin_before = tier_margin_deg(original_angle, n_from);

        let new_angle = retarget_angle_deg(original_angle, n_from, n_to);
        assert!(
            new_angle < 0.0,
            "pavilion sign must be preserved: {new_angle}"
        );
        let margin_after = tier_margin_deg(new_angle, n_to);

        assert!(
            (margin_after - margin_before).abs() < 1e-9,
            "margin drifted: before={margin_before} after={margin_after}"
        );
    }

    /// Same property, but for a crown-signed (non-negative) angle -- the sign
    /// handling must be symmetric, not pavilion-only.
    #[test]
    fn retarget_preserves_margin_and_sign_on_a_crown_tier_too() {
        let n_from = 1.7;
        let n_to = 2.0;
        let original_angle = 35.0;
        let margin_before = tier_margin_deg(original_angle, n_from);

        let new_angle = retarget_angle_deg(original_angle, n_from, n_to);
        assert!(
            new_angle >= 0.0,
            "crown sign must be preserved: {new_angle}"
        );
        let margin_after = tier_margin_deg(new_angle, n_to);

        assert!((margin_after - margin_before).abs() < 1e-9);
    }

    /// Retargeting to the SAME index must be a no-op on the angle itself.
    #[test]
    fn retarget_to_the_same_index_is_a_no_op() {
        let n = 1.62;
        let angle = -38.5;
        assert!((retarget_angle_deg(angle, n, n) - angle).abs() < 1e-9);
    }

    /// A culet authored at exactly `-0.0` (this crate's own
    /// pavilion-side marker -- see `ConstraintTier::standard_round_brilliant`)
    /// must retarget to a NEGATIVE (pavilion) angle, not `+new_theta`: a plain
    /// `tier_angle_deg < 0.0` test is `false` for `-0.0`, which would silently drop
    /// the marker.
    #[test]
    fn retarget_preserves_the_negative_zero_pavilion_marker() {
        let n_from = 2.417;
        let n_to = 1.544;
        let new_angle = retarget_angle_deg(-0.0, n_from, n_to);
        assert!(
            new_angle.is_sign_negative(),
            "a -0.0 (pavilion) origin must retarget to a negative angle: {new_angle}"
        );
    }

    // --- crown_window_margin_deg / crown_windowing_risk ---

    /// A classic 41-degree pavilion / 34-degree crown round brilliant is a real,
    /// working (non-windowing) cut in both quartz and sapphire -- the "known-good
    /// case" this function must not falsely flag, in EITHER material.
    #[test]
    fn crown_window_reports_safe_for_a_standard_41_34_round_brilliant() {
        for n in [1.544_f64, 1.76_f64] {
            let risk = crown_windowing_risk(41.0, 34.0, n);
            assert_eq!(
                risk,
                Risk::Safe,
                "n={n}: margin={}",
                crown_window_margin_deg(41.0, 34.0, n)
            );
        }
    }

    /// A flat (0-degree) crown -- i.e. a table-only path -- must reduce exactly
    /// to the plain table-only pavilion margin: `r = asin(sin(0)/n) = 0`, so
    /// `theta_pavilion = p + (0 - 0) = p`, identical to [`tier_margin_deg`].
    #[test]
    fn crown_window_margin_reduces_to_the_table_only_margin_at_zero_crown_angle() {
        let n = 1.62;
        let p = 40.75;
        let table_only = tier_margin_deg(-p, n);
        let via_crown = crown_window_margin_deg(p, 0.0, n);
        assert!(
            (table_only - via_crown).abs() < 1e-9,
            "table_only={table_only} via_crown={via_crown}"
        );
    }

    /// Entering through ANY tilted crown facet must never report a SMALLER
    /// margin than the table-only path at the same pavilion angle -- the
    /// derivation's own `c - r >= 0` property (refraction only ever adds
    /// incidence angle here, never removes it).
    #[test]
    fn crown_window_margin_is_never_below_the_table_only_margin() {
        let n = 1.76;
        let p = 38.0;
        let table_only = tier_margin_deg(-p, n);
        for c in [5.0, 15.0, 25.0, 34.0, 40.0] {
            let via_crown = crown_window_margin_deg(p, c, n);
            assert!(
                via_crown >= table_only - 1e-9,
                "c={c}: via_crown={via_crown} < table_only={table_only}"
            );
        }
    }

    /// Sign-agnostic on the pavilion angle, matching [`tier_margin_deg`]'s own
    /// convention -- a pavilion tier's authored `angle_deg` is negative.
    #[test]
    fn crown_window_margin_is_symmetric_in_the_pavilion_angles_sign() {
        let n = 1.7;
        assert_eq!(
            crown_window_margin_deg(41.0, 34.0, n),
            crown_window_margin_deg(-41.0, 34.0, n)
        );
    }

    // --- guard_retargeted_angle_deg / is_horizontal_angle_deg ---

    #[test]
    fn horizontal_angles_are_the_table_the_culet_and_nothing_else() {
        assert!(is_horizontal_angle_deg(0.0));
        assert!(is_horizontal_angle_deg(-0.0));
        assert!(is_horizontal_angle_deg(1e-12));
        assert!(!is_horizontal_angle_deg(0.001));
        assert!(!is_horizontal_angle_deg(-34.5));
    }

    #[test]
    fn a_shift_inside_the_limits_is_left_alone() {
        assert_eq!(
            guard_retargeted_angle_deg(-41.0, -44.75),
            (-44.75, AngleGuard::Within)
        );
        assert_eq!(
            guard_retargeted_angle_deg(34.5, 38.1),
            (38.1, AngleGuard::Within)
        );
    }

    #[test]
    fn a_shift_across_the_horizontal_is_held_at_the_minimum_on_the_original_side() {
        // A crown facet shifted to a negative angle would become a pavilion plane.
        let (crown, guard) = guard_retargeted_angle_deg(5.0, -3.0);
        assert_eq!((crown, guard), (1.0, AngleGuard::HeldAtMinimum));
        // A pavilion facet shifted to a positive angle would become a crown plane.
        let (pavilion, guard) = guard_retargeted_angle_deg(-5.0, 3.0);
        assert_eq!((pavilion, guard), (-1.0, AngleGuard::HeldAtMinimum));
        // Landing exactly on zero is held too.
        assert_eq!(
            guard_retargeted_angle_deg(5.0, 0.0),
            (1.0, AngleGuard::HeldAtMinimum)
        );
    }

    #[test]
    fn a_facet_already_flatter_than_the_minimum_keeps_its_own_angle_as_the_floor() {
        let (angle, guard) = guard_retargeted_angle_deg(0.4, -2.0);
        assert!((angle - 0.4).abs() < 1e-12, "{angle}");
        assert_eq!(guard, AngleGuard::HeldAtMinimum);
    }

    #[test]
    fn a_shift_past_the_steepest_slope_is_held_at_the_maximum() {
        assert_eq!(
            guard_retargeted_angle_deg(-80.0, -95.0),
            (-MAX_RETARGET_ANGLE_DEG, AngleGuard::HeldAtMaximum)
        );
        assert_eq!(
            guard_retargeted_angle_deg(80.0, 120.0),
            (MAX_RETARGET_ANGLE_DEG, AngleGuard::HeldAtMaximum)
        );
    }

    #[test]
    fn a_non_finite_shift_leaves_the_facet_where_it_is() {
        assert_eq!(
            guard_retargeted_angle_deg(-41.0, f64::NAN),
            (-41.0, AngleGuard::Within)
        );
    }

    // --- geometric_ri_hint ---

    /// Pentalume Sapphire: P1 at 43, P2 at 40.5, P3 at 44 degrees, no culet.
    const PENTALUME: &str = "GemCad 5.0
g 80 0
y 5 y
I 1.7680773735046387
H Pentalume Sapphire
H Designed by Niklas Gussek
a -43 0.7130808923813132 0 n P1_Five_broad_mains 16 32 48 64
a -40.5 0.7661935481511087 8 n P2_Five_corner_guards 24 40 56 72
a -44 0.7955073176906 76 n P3_Ten_shoulder_splits 4 12 20 28 36 44 52 60 68
a 90 1 0 n G1_Five_long_sides 16 32 48 64 G Set stone size.
a 90 1.13 8 n G2_Five_clipping_flats 24 40 56 72
a 34 0.5944270003043362 0 n C1_Five_broad_bezels 16 32 48 64
a 30 0.6018060796608384 8 n C2_Five_corner_bezels 24 40 56 72
a 33 0.6346315320439263 76 n C3_Ten_shoulder_facets 4 12 20 28 36 44 52 60 68
a 0 0.35 n Table
";

    fn pentalume() -> Design {
        let schedule = pleochroma_formats::asc::parse_asc(PENTALUME).expect("the fixture parses");
        let preform = crate::preform::PreformSpec::cylinder_for_schedule(&schedule, 3.0, 1.0, 3.0);
        Design::from_asc_schedule(preform, &schedule)
    }

    /// The shallowest pavilion tier names the index: P2 at 40.5 degrees, not the first or the
    /// steepest one.
    #[test]
    fn the_hint_reads_the_shallowest_pavilion_tier() {
        let design = pentalume();
        let solved = design.solve().expect("every imported tier is pinned");
        let hint = geometric_ri_hint(&design, &solved).expect("a pavilion tier is left");
        assert_eq!(hint.tier, "P2");
        assert!((hint.angle_deg - 40.5).abs() < 1e-9, "{}", hint.angle_deg);
        let expected = 1.0 / 40.5_f64.to_radians().sin();
        assert!((hint.n_geo - expected).abs() < 1e-9, "{}", hint.n_geo);
    }

    /// The index follows `1 / sin(angle)`: at the critical angle of `n_geo` the tier sits exactly
    /// on the edge of windowing.
    #[test]
    fn the_hint_index_is_the_one_whose_critical_angle_is_the_tier_angle() {
        let design = pentalume();
        let solved = design.solve().expect("every imported tier is pinned");
        let hint = geometric_ri_hint(&design, &solved).expect("a pavilion tier is left");
        assert!((critical_angle_deg(hint.n_geo) - hint.angle_deg).abs() < 1e-9);
    }

    /// The culet is flat, not a windowing question: a template's hint never reads an angle near
    /// zero.
    #[test]
    fn the_culet_is_never_the_shallowest_tier() {
        let spec = crate::templates::TEMPLATES
            .iter()
            .find(|spec| {
                spec.tiers().iter().any(|tier| {
                    is_horizontal_angle_deg(tier.angle_deg) && tier.angle_deg.is_sign_negative()
                })
            })
            .expect("a built-in template has a culet");
        let design = Design::new(spec.preform.clone(), spec.schedule_meta(), spec.tiers());
        let solved = design.solve().expect("every template tier is pinned");
        let hint = geometric_ri_hint(&design, &solved).expect("the template has a pavilion");
        assert!(hint.angle_deg > 1.0, "{}", hint.angle_deg);
        assert!(hint.n_geo > 1.0 && hint.n_geo < 10.0, "{}", hint.n_geo);
    }

    /// A solve that is not one entry per tier gives no hint.
    #[test]
    fn a_misaligned_solve_gives_no_hint() {
        let design = pentalume();
        let solved = design.solve().expect("every imported tier is pinned");
        assert_eq!(geometric_ri_hint(&design, &solved[1..]), None);
    }
}
