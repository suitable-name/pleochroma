//! Which guide steps have a scripted route. Only the director asks, so this lives outside
//! `pure_tables.rs` (which the unit-test target `tutorial_videos_targets` includes too).

use crate::scripts::pure_tables::{MODULE_SCRIPTED, SCRIPTED};

/// Whether a step has a scripted route that must run before the director can advance it.
#[must_use]
pub fn has_script(guide_id: &str, step: usize) -> bool {
    SCRIPTED
        .iter()
        .any(|(id, index, _)| *id == guide_id && *index == step)
        || MODULE_SCRIPTED
            .iter()
            .any(|(id, count)| *id == guide_id && step < *count)
}
