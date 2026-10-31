//! The Tune view in the Simple interface: which of the settings it hides are not at their
//! defaults.
//!
//! The Simple interface hides the mini sweep with its figure choice, Jump to Best, Pair With and
//! the Up and Down buttons. The two that keep a value are the tuning order (the cutter moved a
//! tier) and the figure the chart shows; the view says "Some advanced settings are in use.
//! Switch to Advanced to see them." whenever either is not what a fresh view starts with. The
//! rule is here, Slint-free, so it is tested without a window.

/// The figure the chart starts on (Brilliance, the first of `mini_sweep::FIGURES`).
pub(super) const DEFAULT_FIGURE: usize = 0;

/// Whether a setting the Simple interface hides is away from its default.
pub(super) const fn settings_in_use(order_changed: bool, figure: usize) -> bool {
    order_changed || figure != DEFAULT_FIGURE
}
