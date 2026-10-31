//! The Tune view's tier list, without a window: which tiers it lists, the tuning order they
//! come in, the order the cutter gave them, what "Next Tier" picks and the Baseline's angle of
//! each tier. Slint-free, so it is tested directly.

use pleochroma_cut_core::Design;
use pleochroma_editor::sweep::MAX_SWEEP_ANGLE_DEG;
use std::collections::BTreeSet;

/// Which part of the stone a tier is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Side {
    Pavilion,
    Crown,
    /// A vertical tier: it has no angle to tune.
    Girdle,
}

/// What the list needs to know of one tier.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TierInfo {
    /// The tier's position in the design.
    pub index: usize,
    /// "P1", "C2": its code (numbered in cutting order).
    pub code: String,
    /// The name a person reads; the code again when the tier has no name.
    pub name: String,
    /// Signed, as the tier stores it (a pavilion tier's is negative).
    pub angle_deg: f64,
    pub side: Side,
    /// The angle follows a relation (`P2 = P1 - 2`): it is not free to tune.
    pub driven: bool,
    /// The relation as a cutter reads it (`P1 - 2`) for a driven tier.
    pub relation: Option<String>,
}

impl TierInfo {
    /// Whether the cutter can tune the tier: a free crown or pavilion tier.
    pub(super) const fn tunable(&self) -> bool {
        !self.driven && !matches!(self.side, Side::Girdle)
    }
}

/// The side of a tier at `angle_deg`; `None` for a table or a culet, which have no angle to tune
/// and are not listed.
pub(super) fn side_of(angle_deg: f64) -> Option<Side> {
    let magnitude = angle_deg.abs();
    if magnitude < 1e-9 {
        None
    } else if magnitude > MAX_SWEEP_ANGLE_DEG {
        Some(Side::Girdle)
    } else if angle_deg.is_sign_negative() {
        Some(Side::Pavilion)
    } else {
        Some(Side::Crown)
    }
}

/// Every listed tier of `design` (a table and a culet are left out), in design order.
pub(super) fn tier_infos(design: &Design) -> Vec<TierInfo> {
    let names = pleochroma_editor::retarget::plan::tier_display_names(design);
    let codes = design.tier_codes().flat;
    design
        .tiers
        .iter()
        .enumerate()
        .filter_map(|(index, tier)| {
            let side = side_of(tier.angle_deg)?;
            let code = codes
                .get(index)
                .map(|label| label.code.clone())
                .unwrap_or_default();
            Some(TierInfo {
                index,
                name: names.get(index).cloned().unwrap_or_else(|| code.clone()),
                code,
                angle_deg: tier.angle_deg,
                side,
                driven: design.is_tier_driven(index),
                relation: design.relation_text(index),
            })
        })
        .collect()
}

/// A code's number for sorting (`C10` after `C2`), 0 when it has none.
fn code_number(code: &str) -> u32 {
    let digits: String = code.chars().filter(char::is_ascii_digit).collect();
    digits.parse().unwrap_or(0)
}

/// The main tier of a side: the one named "main", else the one whose code is `P1` or `C1`, else
/// the first of the side. `None` for a side with no tier.
fn main_of(infos: &[TierInfo], side: Side) -> Option<usize> {
    let of_side = || infos.iter().filter(move |info| info.side == side);
    let by_name = of_side().find(|info| info.name.to_lowercase().contains("main"));
    let wanted = match side {
        Side::Pavilion => "P1",
        Side::Crown => "C1",
        Side::Girdle => "G1",
    };
    by_name
        .or_else(|| of_side().find(|info| info.code == wanted))
        .or_else(|| of_side().next())
        .map(|info| info.index)
}

/// The tuning order: the pavilion main, the crown main, then the other crown tiers, the other
/// pavilion tiers and the girdle tiers, each group by code number and then name. The result is
/// the tiers' design positions.
pub(super) fn tuning_order(infos: &[TierInfo]) -> Vec<usize> {
    let pavilion_main = main_of(infos, Side::Pavilion);
    let crown_main = main_of(infos, Side::Crown);
    let mut order: Vec<usize> = pavilion_main.into_iter().chain(crown_main).collect();
    for side in [Side::Crown, Side::Pavilion, Side::Girdle] {
        let mut group: Vec<&TierInfo> = infos
            .iter()
            .filter(|info| info.side == side && !order.contains(&info.index))
            .collect();
        group.sort_by(|a, b| {
            code_number(&a.code)
                .cmp(&code_number(&b.code))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.index.cmp(&b.index))
        });
        order.extend(group.into_iter().map(|info| info.index));
    }
    order
}

/// `order` as the cutter reordered it: the tiers `custom` names first, in its order, then the
/// rest in `order`'s. A tier `custom` names that is gone is skipped.
pub(super) fn apply_custom_order(order: &[usize], custom: &[usize]) -> Vec<usize> {
    let mut result: Vec<usize> = custom
        .iter()
        .copied()
        .filter(|tier| order.contains(tier))
        .collect();
    let mut seen = BTreeSet::new();
    result.retain(|tier| seen.insert(*tier));
    result.extend(order.iter().copied().filter(|tier| !seen.contains(tier)));
    result
}

/// Moves `tier` one place up (`up`) or down in `order`; `false` when it is at that end or not
/// in the list.
pub(super) fn move_tier(order: &mut [usize], tier: usize, up: bool) -> bool {
    let Some(at) = order.iter().position(|&listed| listed == tier) else {
        return false;
    };
    let to = if up {
        match at.checked_sub(1) {
            Some(to) => to,
            None => return false,
        }
    } else {
        at + 1
    };
    if to >= order.len() {
        return false;
    }
    order.swap(at, to);
    true
}

/// What "Next Tier" picks: the first tier after `current` in `order` (then from the start of
/// the list) that can be tuned and has not been accepted; `None` when there is none other than
/// `current` itself.
pub(super) fn next_untuned(
    order: &[usize],
    current: Option<usize>,
    tunable: &dyn Fn(usize) -> bool,
    accepted: &BTreeSet<usize>,
) -> Option<usize> {
    let start = current
        .and_then(|tier| order.iter().position(|&listed| listed == tier))
        .map_or(0, |at| at + 1);
    order
        .iter()
        .cycle()
        .skip(start)
        .take(order.len())
        .copied()
        .find(|&tier| Some(tier) != current && tunable(tier) && !accepted.contains(&tier))
}

/// The Baseline's angle of each tier of `design` (signed, as stored), by position when the two
/// designs have the same tier at that position, else by the tier's name; `None` for a tier the
/// Baseline does not have.
pub(super) fn baseline_angles(baseline: &Design, design: &Design) -> Vec<Option<f64>> {
    design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let same_place = baseline
                .tiers
                .get(index)
                .filter(|before| before.name == tier.name);
            let found = same_place.or_else(|| {
                (!tier.name.is_empty())
                    .then(|| {
                        baseline
                            .tiers
                            .iter()
                            .find(|before| before.name == tier.name)
                    })
                    .flatten()
            });
            found.map(|before| before.angle_deg)
        })
        .collect()
}
