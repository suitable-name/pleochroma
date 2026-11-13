//! Synthetic validation of the rough-colour-from-photos feature: zone boundaries, the forward
//! model against the main tracer, determinism, and the colour recovery of the fit.
//!
//! ```text
//! cargo test -p pleochroma-cut-core --test zoning_validation
//! ```
//!
//! The quick checks run by default. The full recovery matrix and the slow runs are `#[ignore]`d;
//! run them on request with `scripts/run_tests_on_request.sh`.

#![allow(
    clippy::needless_range_loop,
    clippy::many_single_char_names,
    clippy::too_many_lines,
    reason = "test code"
)]

mod boundary;
mod cross_check;
mod determinism;
mod recovery;
mod synth;
