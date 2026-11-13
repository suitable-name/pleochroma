//! The rule the viewport's local-only still views share: whether a remote lane is involved in this
//! frame.
//!
//! The instant re-light and the beam jobs use it, and so does the adaptive viewport
//! (`adaptive-sampling` feature).

use crate::settings::model::LiveComputeTarget;

/// Whether a remote lane is involved in this frame: a remote that owns the image, an epoch the
/// viewport combines with, or a remote-only live target. The viewport's local-only still views
/// (the adaptive passes, the stored light paths, the beams) do not run then.
#[must_use]
pub(super) const fn remote_involved(
    remote_active: bool,
    has_epoch: bool,
    target: LiveComputeTarget,
) -> bool {
    remote_active || has_epoch || matches!(target, LiveComputeTarget::RemoteOnly)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_lane_is_any_remote_ownership_epoch_or_remote_only_target() {
        assert!(!remote_involved(false, false, LiveComputeTarget::LocalOnly));
        assert!(!remote_involved(false, false, LiveComputeTarget::Both));
        assert!(remote_involved(true, false, LiveComputeTarget::Both));
        assert!(remote_involved(false, true, LiveComputeTarget::Both));
        assert!(remote_involved(false, false, LiveComputeTarget::RemoteOnly));
    }
}
