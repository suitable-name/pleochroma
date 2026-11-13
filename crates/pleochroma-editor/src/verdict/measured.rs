//! The reasons the measured optics give (split from `verdict.rs` to keep it under 800 lines).

use super::{
    BRILLIANCE_CHECK_BELOW_PCT, BRILLIANCE_PROBLEM_BELOW_PCT, EXTINCTION_CHECK_PCT,
    EXTINCTION_PROBLEM_PCT, Level, Reason, ReasonKind, WINDOWING_CHECK_PCT, WINDOWING_PROBLEM_PCT,
    reason,
};
use crate::retarget::metrics::MetricColumn;

/// The reasons the measured optics give.
pub(super) fn optics_reasons(optics: &MetricColumn) -> Vec<Reason> {
    let mut reasons = Vec::new();
    let windowing = optics.windowing_pct;
    if windowing > WINDOWING_CHECK_PCT {
        let level = if windowing > WINDOWING_PROBLEM_PCT {
            Level::Problem
        } else {
            Level::Check
        };
        reasons.push(reason(
            ReasonKind::OpticalWindowing,
            level,
            format!(
                "About {windowing:.0} % of the stone leaks light straight out of the bottom (windowing)."
            ),
        ));
    }
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
