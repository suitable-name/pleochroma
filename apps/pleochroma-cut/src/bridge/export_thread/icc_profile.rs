//! Re-export of the ICC profile builder in `pleochroma::render_setup::icc_profile`, which the
//! browser app shares so both embed byte-identical wide-gamut profiles.

pub use pleochroma::render_setup::icc_profile::build;
