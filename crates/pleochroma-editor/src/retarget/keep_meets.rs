//! Change the angles, keep the meets.
//!
//! This is the one step every retarget goes through: the user's Retarget (preview, verdict and
//! Apply), and a template created in a material other than its own
//! (`templates::create_from_template`).
//!
//! A retarget turns facets and re-anchors depths about the girdle, and its projection holds the
//! finished stone's junctions. A point a tier was cut to on the stone cut so far is not always
//! one of them (a later tier can cut it away, as the round brilliant's culet cuts away the
//! mains' point), so a meet-point design could come out with tiers that have no meet point.
//! [`keep_meets`] puts each tier that met a point of the original back at the same distance from
//! the point of the same three planes ([`Design::reanchor_meets`] with the original's
//! [`StoneAnalysis::meet_targets`]), and judges the RESULT with the Retarget's own validity gate
//! ([`judge`]). When the gate refuses the re-anchored stone, the changed stone is kept as it came
//! and the caller is told the meets were not kept: re-anchoring never turns an accepted retarget
//! into a refused one, and never changes how a stone is judged.

use super::validity::{InvalidReason, StoneAnalysis, analyze_controlled, judge};
use pleochroma::geometry::meet_solver::SolveControl;
use pleochroma_cut_core::{CutMeet, Design, MeetTarget};

/// What [`keep_meets`] gave.
#[derive(Debug, Clone)]
pub struct KeptMeets {
    /// The re-anchored design when the gate accepts it, else the changed design as it came.
    pub design: Design,
    /// The analysis of [`Self::design`].
    pub analysis: Result<StoneAnalysis, InvalidReason>,
    /// The gate's reasons against [`Self::design`] (empty: valid).
    pub reasons: Vec<InvalidReason>,
    /// Whether [`Self::design`] is the re-anchored stone, or nothing needed re-anchoring:
    /// `false` when the points could not be recomputed (a triple turned degenerate) or the gate
    /// refused the re-anchored stone.
    pub kept: bool,
}

/// Puts every tier that met a point of the original back on it after the angles changed, and
/// judges the result.
///
/// `changed` is `original_design` with its angles moved (and whatever depths the change itself
/// re-anchored); `original` is the ORIGINAL stone's analysis, with its meet baseline and
/// [`StoneAnalysis::meet_targets`] (`analyze(original_design, true)`). The steps: re-solve
/// `changed`, [`Design::reanchor_meets`] in cutting order, analyse and [`judge`] the result. The
/// re-anchored stone is kept when the gate accepts it; otherwise `changed` is analysed and
/// judged as it came. `control` is read by every solve.
#[must_use]
pub fn keep_meets(
    original_design: &Design,
    original: &StoneAnalysis,
    changed: Design,
    control: &SolveControl<'_>,
) -> KeptMeets {
    let judged = |design: Design, kept: bool| {
        let analysis = analyze_controlled(&design, false, control);
        let reasons = judge(original_design, original, &analysis);
        KeptMeets {
            design,
            analysis,
            reasons,
            kept,
        }
    };
    match reanchored(&changed, &original.meet_targets, control) {
        Reanchored::Moved(design) => {
            let result = judged(*design, true);
            if result.reasons.is_empty() {
                return result;
            }
            judged(changed, false)
        }
        Reanchored::Unneeded => judged(changed, true),
        Reanchored::Failed => judged(changed, false),
    }
}

/// Whether every tier of `targets` (recorded with [`Design::meet_targets`] before the angles
/// changed) still meets a point of the stone cut so far in `design`.
///
/// `true` for no targets. `false` when `design` does not solve or close.
#[must_use]
pub fn meets_kept(design: &Design, targets: &[MeetTarget]) -> bool {
    if targets.is_empty() {
        return true;
    }
    let Ok(solved) = design.solve() else {
        return false;
    };
    let Some(meets) = design.cut_meets(&solved) else {
        return false;
    };
    targets
        .iter()
        .all(|target| matches!(meets.tiers.get(target.tier), Some(CutMeet::Meets(_))))
}

/// What re-anchoring a changed design gave.
enum Reanchored {
    /// Some tier moved: the re-anchored design (boxed: a design is large, the other variants
    /// carry nothing).
    Moved(Box<Design>),
    /// Every tier is already where it belongs (or there is nothing to re-anchor).
    Unneeded,
    /// The design does not solve or close, a point could not be recomputed, or the solve was
    /// stopped.
    Failed,
}

/// `design` with every tier of `targets` put back on its point ([`Design::reanchor_meets`]).
fn reanchored(design: &Design, targets: &[MeetTarget], control: &SolveControl<'_>) -> Reanchored {
    if targets.is_empty() {
        return Reanchored::Unneeded;
    }
    if control.is_cancelled() {
        return Reanchored::Failed;
    }
    let Ok(solved) = design.solve_with(control) else {
        return Reanchored::Failed;
    };
    let mut candidate = design.clone();
    match candidate.reanchor_meets(targets, &solved) {
        None => Reanchored::Failed,
        Some(moves) if moves.is_empty() => Reanchored::Unneeded,
        Some(_) => Reanchored::Moved(Box::new(candidate)),
    }
}
