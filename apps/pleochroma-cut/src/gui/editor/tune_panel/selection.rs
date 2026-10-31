//! Tune uses the editor's design-tier selection, shared with Solid and Diagram picking.

use super::{handles, is_tunable, order::TierInfo, tick_now, tune_showing, with_hub};
use crate::{EditorModel, GuideModel, TuneModel, gui::editor::callbacks::clear_selected_facet};
use slint::ComponentHandle;
use std::sync::atomic::Ordering;

/// A listed design tier, even if its driven/vertical angle cannot be tuned.
///
/// The disabled row still follows a picked facet, while its tuning controls stay hidden.
/// Flat table/culet and concave tiers are not in this list and clear its selection.
pub(super) fn listed_tier(infos: &[TierInfo], index: i32) -> Option<usize> {
    let tier = usize::try_from(index).ok()?;
    infos.iter().any(|info| info.index == tier).then_some(tier)
}

/// A click is usable only for the same design revision that supplied its row.
fn requested_tier(
    infos: &[TierInfo],
    index: i32,
    listed_revision: Option<(u64, u64)>,
    current_revision: (u64, u64),
) -> Option<usize> {
    listed_tier(infos, index)
        .filter(|&tier| listed_revision == Some(current_revision) && is_tunable(infos, tier))
}

/// A row click or Next Tier selects the same design tier in all views.
pub(super) fn select_tier(index: i32) {
    let Some((ui, deps)) = handles() else {
        return;
    };
    if !ui.global::<TuneModel>().get_active()
        || !tune_showing(&ui)
        || !ui.global::<GuideModel>().invoke_allows_advanced()
    {
        return;
    }
    let current = {
        let Ok(state) = deps.state.try_borrow() else {
            return;
        };
        (
            state.design_epoch.load(Ordering::Relaxed),
            state.current_generation(),
        )
    };
    // A reorder/delete can happen before the timer rebuilt the rows. Refuse that old row
    // instead of selecting whichever tier now happens to occupy its previous design index.
    let valid = with_hub(|hub| {
        requested_tier(
            &hub.infos,
            index,
            hub.list_key.map(|key| (key.epoch, key.generation)),
            current,
        )
        .is_some()
    })
    .unwrap_or(false);
    if !valid {
        tick_now();
        return;
    }
    clear_selected_facet(&ui, &deps.preview_state);
    let editor = ui.global::<EditorModel>();
    if editor.get_selected_tier_index() == index {
        // Like a same-row click in the tier table: finish coalescing and clear multi-select.
        editor.invoke_selected_tier_changed(index);
    } else {
        editor.set_selected_tier_index(index);
    }
    tick_now();
}

/// Keep the first-open default, but make it the shared selection too.
pub(super) fn select_initial() {
    let Some((ui, _)) = handles() else {
        return;
    };
    if ui.global::<EditorModel>().get_selected_tier_index() >= 0 {
        return;
    }
    let first = with_hub(|hub| {
        hub.order
            .iter()
            .copied()
            .find(|&tier| is_tunable(&hub.infos, tier))
    })
    .flatten();
    if let Some(first) = first.and_then(|tier| i32::try_from(tier).ok()) {
        select_tier(first);
    }
}

#[cfg(test)]
#[path = "selection_tests.rs"]
mod tests;
