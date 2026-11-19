//! Load Job View: a queued job's look in the Live Render view, and Update Job from This View.

pub(in crate::gui) mod load;
pub(in crate::gui) mod session;
pub(in crate::gui) mod update;

pub(in crate::gui) use load::setup;

#[cfg(test)]
mod tests_update;
#[cfg(test)]
mod tests_view;
