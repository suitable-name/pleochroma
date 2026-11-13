//! Re-export of `pleochroma::render_setup::sample_scale`, the exponent-to-sample-count mapping of
//! the Parameters panel's Target Samples slider.
//!
//! The mapping lives in the library so the browser app's slider maps samples the same way. This
//! path keeps the crate's imports unchanged.

pub use pleochroma::render_setup::sample_scale::*;
