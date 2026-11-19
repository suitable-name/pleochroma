//! Editing queued render jobs: the edit dialog, copies to other materials, and their wiring.

pub(in crate::gui) mod copy;
mod footer;
pub(in crate::gui) mod load;
pub(in crate::gui) mod material;
pub(in crate::gui) mod naming;
pub(in crate::gui) mod save;
mod seed;
#[cfg(test)]
mod tests_copy;
#[cfg(test)]
mod tests_material;
#[cfg(test)]
mod tests_save;
pub(in crate::gui) mod video_curves;
mod wiring;

pub(in crate::gui) use wiring::setup;
