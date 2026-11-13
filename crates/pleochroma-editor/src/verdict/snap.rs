//! The snap to meet points: the one safety rule for a snapped design ([`judge_snap`]), shared by
//! the verdict's fix and the CLI's `snap-meets`, and the fix's plan.
//!
//! [`pleochroma_cut_core::Design::snap_depths_to_meets`] moves depths; it does not judge the stone
//! that results. A snap changes meets by its nature, so the meet-preservation gate does not apply
//! and only the finished stone is judged (`retarget::validity::judge_stone`, as for a new table).
//! The rule refuses a snapped design whose stone breaks: it does not solve or close, its girdle
//! is lost, much thinner or runs to a knife edge, or its table or culet ends up on the wrong side
//! of the girdle. Facets the new depths cut away do not refuse it; they are reported, tier by tier
//! ([`CutAway`]), so the question asked before the snap is applied can name them.

use super::{
    FixPlan,
    fix::{invalid_text, tier_label},
    meets::listed,
    plural,
};
use crate::retarget::validity::{InvalidReason, analyze, judge_stone};
use pleochroma_cut_core::{
    DEFAULT_SNAP_MAX_MOVE_FRACTION, Design, Edit, design::cutting_order::flat_cutting_order,
};
use std::fmt;

/// Shown when no tier without a meet point is close enough to one for the snap to move it.
const NOTHING_TO_SNAP: &str = "No tier without a meet point is close enough to one to move it \
                               automatically. Set their depths by hand.";

/// One tier that loses facets to a snap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutAway {
    /// The tier's position in `design.tiers`.
    pub tier: usize,
    /// How many of its facets reached the stone before the snap and no longer do.
    pub lost: usize,
    /// How many facets the tier has.
    pub total: usize,
}

/// What a snap does to the stone, when [`judge_snap`] lets it through.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SnapOutcome {
    /// The tiers whose depth moved (whose tier differs between the two designs), in cutting
    /// order.
    pub moved: Vec<usize>,
    /// The tiers that lose facets to the new depths, in cutting order. Empty when none does.
    pub cut_away: Vec<CutAway>,
}

impl SnapOutcome {
    /// The cut-away tiers as a sentence's list, named the way the tier table names them:
    /// `P3 (all 10 facets) and C3 (3 of 10 facets)`. `None` when nothing is cut away.
    #[must_use]
    pub fn cut_away_list(&self, design: &Design) -> Option<String> {
        if self.cut_away.is_empty() {
            return None;
        }
        let items: Vec<String> = self
            .cut_away
            .iter()
            .map(|cut| {
                let label = tier_label(design, cut.tier);
                if cut.lost >= cut.total && cut.total == 1 {
                    format!("{label} (its facet)")
                } else if cut.lost >= cut.total {
                    format!("{label} (all {} facets)", cut.total)
                } else {
                    format!("{label} ({} of {} facets)", cut.lost, cut.total)
                }
            })
            .collect();
        Some(listed(&items))
    }
}

/// Why [`judge_snap`] refused a snapped design: one plain sentence for the cutter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapRefusal {
    /// The sentence, ending in what to do instead.
    pub sentence: String,
}

impl fmt::Display for SnapRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.sentence)
    }
}

impl std::error::Error for SnapRefusal {}

/// Whether a design snapped to its meet points may replace `design`: the safety rule both the
/// verdict's fix and the CLI apply (see the module docs).
///
/// `snapped` is `design` after [`pleochroma_cut_core::Design::snap_depths_to_meets`] (on a
/// clone). Both are solved here, so a caller runs this off the UI thread. A snap that moved
/// nothing is let through with an empty [`SnapOutcome`].
///
/// # Errors
///
/// A [`SnapRefusal`] when the two designs do not have the same tiers, when `design` itself has no
/// sound stone to compare with, or when the snapped stone breaks.
pub fn judge_snap(design: &Design, snapped: &Design) -> Result<SnapOutcome, SnapRefusal> {
    let refuse = |sentence: String| SnapRefusal { sentence };
    if snapped.tiers.len() != design.tiers.len() {
        return Err(refuse(
            "The snapped design does not have the same tiers. Snap the design again.".to_string(),
        ));
    }
    let order = flat_cutting_order(&design.tiers);
    let moved: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&tier| design.tiers.get(tier) != snapped.tiers.get(tier))
        .collect();
    let original = analyze(design, false).map_err(|reason| refuse(invalid_text(&reason)))?;
    let analysis = analyze(snapped, false);
    if let Some(breaking) = judge_stone(design, &original, &analysis)
        .iter()
        .find(|reason| !is_about_facets(reason))
    {
        return Err(refuse(format!(
            "Moving the depths to the meet points would break the stone: {} Set the depths by \
             hand.",
            invalid_text(breaking)
        )));
    }
    // `judge_stone` refuses a candidate that does not analyse, so the error arm is not reached.
    let cut_away = analysis.as_ref().map_or_else(
        |_| Vec::new(),
        |after| {
            order
                .iter()
                .filter_map(|&tier| {
                    let was = original.facets.tiers.get(tier)?;
                    let now = after.facets.tiers.get(tier)?;
                    (now.alive < was.alive).then_some(CutAway {
                        tier,
                        lost: was.alive - now.alive,
                        total: was.total,
                    })
                })
                .collect()
        },
    );
    Ok(SnapOutcome { moved, cut_away })
}

/// Whether `reason` is about facets only: a tier that loses some or all of its facets, or whose
/// facets become too small. The snap reports those instead of refusing (see the module docs).
const fn is_about_facets(reason: &InvalidReason) -> bool {
    matches!(
        reason,
        InvalidReason::TierLost { .. }
            | InvalidReason::TierPartlyLost { .. }
            | InvalidReason::FacetsTooSmall { .. }
    )
}

/// The verdict's [`super::FixAction::SnapToMeets`]: snaps a clone of `design`, judges it with
/// [`judge_snap`] and plans the moves as one batch of `Edit::SetConstraint` (one undo step), with
/// the question to ask before it is applied ([`FixPlan::confirm`]).
pub(super) fn snap_to_meets(design: &Design) -> Result<FixPlan, String> {
    let solved = design.solve().map_err(|error| {
        format!(
            "The design does not solve ({}).",
            error.to_string().trim_end_matches('.')
        )
    })?;
    let meets = design
        .cut_meets(&solved)
        .ok_or_else(|| "The facets do not enclose a stone.".to_string())?;
    let mut snapped = design.clone();
    let report = snapped
        .snap_depths_to_meets(&solved, DEFAULT_SNAP_MAX_MOVE_FRACTION * meets.size)
        .ok_or_else(|| "The facets do not enclose a stone.".to_string())?;
    // One step per moved tier: the constraint the snap gave it.
    let mut steps: Vec<Edit> = report
        .moved
        .iter()
        .filter_map(|moved| {
            snapped
                .tiers
                .get(moved.tier)
                .map(|tier| Edit::SetConstraint {
                    index: moved.tier,
                    constraint: tier.constraint.clone(),
                })
        })
        .collect();
    let Some(first_moved) = report.moved.first().map(|moved| moved.tier) else {
        return Err(NOTHING_TO_SNAP.to_string());
    };
    if steps.is_empty() {
        return Err(NOTHING_TO_SNAP.to_string());
    }
    let count = steps.len();
    let edit = if count == 1 {
        steps.remove(0)
    } else {
        Edit::Batch(steps)
    };
    // The stone judged is the one the edit makes, applied to a fresh copy.
    let mut candidate = design.clone();
    candidate
        .apply_edit(edit.clone())
        .map_err(|error| error.to_string())?;
    let outcome = judge_snap(design, &candidate).map_err(|refusal| refusal.sentence)?;
    let cut_away = outcome.cut_away_list(design);

    let mut confirm = format!(
        "Move the depth of {count} tier{} to the nearest meet point? This changes the design; \
         check the optics afterwards.",
        plural(count)
    );
    if let Some(list) = &cut_away {
        confirm = format!("{confirm} It cuts away {list}.");
    }

    let moved = labels_of(design, report.moved.iter().map(|moved| moved.tier));
    let mut sentences = vec![format!(
        "Moved the depth of {} to the nearest meet point.",
        listed(&moved)
    )];
    let still = labels_of(design, report.still_floating.iter().copied());
    if !still.is_empty() {
        sentences.push(format!(
            "{} still {} no meet point.",
            listed(&still),
            if still.len() == 1 { "has" } else { "have" }
        ));
    }
    if let Some(list) = &cut_away {
        sentences.push(format!("The new depths cut away {list}."));
    }
    Ok(FixPlan {
        edit,
        message: sentences.join(" "),
        select: Some(first_moved),
        confirm: Some(confirm),
    })
}

/// What a person reads for each of `tiers` (the tier table's names).
fn labels_of(design: &Design, tiers: impl IntoIterator<Item = usize>) -> Vec<String> {
    tiers
        .into_iter()
        .map(|tier| tier_label(design, tier))
        .collect()
}
