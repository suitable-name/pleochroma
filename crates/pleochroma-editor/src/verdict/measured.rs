//! The reasons the measured optics give (split from `verdict.rs` to keep it under 800 lines).

use super::{
    BRILLIANCE_CHECK_BELOW_PCT, BRILLIANCE_PROBLEM_BELOW_PCT, EXTINCTION_CHECK_PCT,
    EXTINCTION_PROBLEM_PCT, Level, Reason, ReasonKind, WINDOWING_CHECK_PCT,
    WINDOWING_CHECK_READS_TILT, WINDOWING_PROBLEM_PCT, WINDOWING_TILT_CHECK_PCT,
    WINDOWING_TILT_PROBLEM_PCT, reason,
};
use crate::retarget::metrics::MetricColumn;

/// The windowing reason, when the figures call for one.
///
/// With `reads_tilt` on and a tilt mean that is the larger of the two readings, the tilted
/// limits judge it and the sentence names the tilt and gives the table-up figure. Otherwise
/// the table-up figure alone is judged, as it always was.
fn windowing_reason(table_up: f32, small_tilt: Option<f32>, reads_tilt: bool) -> Option<Reason> {
    let triggering_tilt = small_tilt
        .filter(|tilted| reads_tilt && *tilted > table_up && *tilted > WINDOWING_TILT_CHECK_PCT);
    if let Some(tilted) = triggering_tilt {
        let level = if tilted > WINDOWING_TILT_PROBLEM_PCT {
            Level::Problem
        } else {
            Level::Check
        };
        return Some(reason(
            ReasonKind::OpticalWindowing,
            level,
            format!(
                "About {tilted:.0} % of the stone leaks light straight out of the bottom when the stone is tilted by 10\u{b0} (windowing). Table up it is about {table_up:.0} %."
            ),
        ));
    }
    if table_up > WINDOWING_CHECK_PCT {
        let level = if table_up > WINDOWING_PROBLEM_PCT {
            Level::Problem
        } else {
            Level::Check
        };
        return Some(reason(
            ReasonKind::OpticalWindowing,
            level,
            format!(
                "About {table_up:.0} % of the stone leaks light straight out of the bottom (windowing)."
            ),
        ));
    }
    None
}

/// The reasons the measured optics give.
///
/// `small_tilt_windowing_pct` is the 10 degree windowing mean; it counts only while
/// [`WINDOWING_CHECK_READS_TILT`] is on.
pub(super) fn optics_reasons(
    optics: &MetricColumn,
    small_tilt_windowing_pct: Option<f32>,
) -> Vec<Reason> {
    optics_reasons_with(optics, small_tilt_windowing_pct, WINDOWING_CHECK_READS_TILT)
}

/// [`optics_reasons`] with the tilt switch given, so a test can try both states.
pub(super) fn optics_reasons_with(
    optics: &MetricColumn,
    small_tilt_windowing_pct: Option<f32>,
    reads_tilt: bool,
) -> Vec<Reason> {
    let mut reasons = Vec::new();
    reasons.extend(windowing_reason(
        optics.windowing_pct,
        small_tilt_windowing_pct,
        reads_tilt,
    ));
    let extinction = optics.extinction_pct;
    if extinction > EXTINCTION_CHECK_PCT {
        let level = if extinction > EXTINCTION_PROBLEM_PCT {
            Level::Problem
        } else {
            Level::Check
        };
        reasons.push(reason(
            ReasonKind::OpticalExtinction,
            level,
            format!("About {extinction:.0} % of the light is lost inside the stone (extinction)."),
        ));
    }
    let brilliance = optics.brilliance_pct;
    if brilliance < BRILLIANCE_CHECK_BELOW_PCT {
        let level = if brilliance < BRILLIANCE_PROBLEM_BELOW_PCT {
            Level::Problem
        } else {
            Level::Check
        };
        reasons.push(reason(
            ReasonKind::OpticalBrilliance,
            level,
            format!(
                "Only about {brilliance:.0} % of the light returns to the eye with the table up."
            ),
        ));
    }
    reasons
}
