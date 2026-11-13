//! Planner integration of rough colour.
//!
//! Moves a rough's colour zones into the frame of each planned stone, chooses the pose that
//! shows the wanted colour face-up, adopts a stone as a design with a zoned custom material,
//! and holds the text and byte formats of the vault's side tables.
//!
//! Pure data and geometry: no database, no UI. The application layer
//! (`pleochroma-cut`'s `gui::rough_colour::store`) reads and writes the vault tables with these
//! codecs.
//!
//! * [`frame`]: the rough-to-stone rigid map ([`rough_to_stone_frame`], [`zones_in_stone_frame`])
//!   and a stone's box-symmetric poses ([`pose_variant`], [`candidate_poses`]);
//! * [`pose`]: the cheap face-up predictor and the pose choice ([`face_up_prediction`],
//!   [`choose_pose`], [`PoseGoal`]);
//! * [`adopt`]: the preview and adopted materials ([`stone_preview_material`],
//!   [`adopt_colour`], [`adopted_material`]) and the "relative to stone" scaling
//!   ([`resolve_relative`], [`make_relative`]);
//! * [`codec`]: JSON for the zones and the fit report, and the [`RoughColour`] record;
//! * [`photo`]: the cached working-resolution photos of a view.

pub mod adopt;
pub mod codec;
pub mod frame;
pub mod photo;
pub mod pose;
#[cfg(test)]
mod tests;

pub use adopt::{
    ADOPTED_SUFFIX, AdoptedColour, StonePlacement, adopt_colour, adopted_material,
    adopted_material_name, base_zone_bands, make_relative, resolve_relative,
    stone_preview_material,
};
pub use codec::{
    CodecError, MAX_TEXT_BYTES, RoughColour, ZONING_FORMAT_VERSION, check_version, decode_fit,
    decode_zoned, encode_fit, encode_zoned,
};
pub use frame::{
    DesignPlacement, POSE_COUNT, candidate_poses, pose_variant, rough_to_stone_frame,
    zones_in_stone_frame,
};
pub use photo::{
    MAX_CACHED_PIXELS, PhotoBlob, PhotoCodecError, PhotoEncoding, blobs_from_resampled,
    resampled_from_blobs,
};
pub use pose::{CROWN_FRACTION, FaceUp, PoseGoal, choose_pose, choose_poses, face_up_prediction};
