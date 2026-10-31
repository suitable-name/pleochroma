//! The saved plan with a stored rough color that the Rough Color tutorial videos open.
//!
//! The wizard restores a stored fit and photos from a saved plan, and the planner offers Stone
//! Orientation and Use Color only for one, but a color comes from photos of a rough on a camera rig
//! that no video can film. So before the app launches for a `library-rough-color-*` lesson, this
//! writes the plan the planner's Save and the wizard's Accept would have written, from numbers
//! generated in code (`harness_support::seed_rough_color_plan`). Other lessons get nothing.

use super::profile::Profile;
use pleochroma_cut::harness_support;

/// The lessons that start from the seeded plan: the Rough Color lessons of the library section.
const LESSON_PREFIX: &str = "library-rough-color-";

/// Whether the lesson `guide_id` needs the seeded plan.
#[must_use]
pub fn wanted(guide_id: &str) -> bool {
    guide_id.starts_with(LESSON_PREFIX)
}

/// The name the seeded plan has in the Saved Plans list.
#[must_use]
pub const fn plan_name() -> &'static str {
    harness_support::ROUGH_COLOR_SEED_PLAN_NAME
}

/// Writes the plan and its color into the profile's library database; the app must not be
/// running and the library (fixtures or the configured copy) must be in place. Returns the plan's
/// name.
///
/// # Errors
///
/// The database lies outside the profile, or the seed fails (no design in the library, a write).
pub fn seed(profile: &Profile) -> Result<String, String> {
    let database = harness_support::database_path();
    if !profile.contains(&database) {
        return Err(format!(
            "refusing to seed a saved plan outside the profile: {}",
            database.display()
        ));
    }
    harness_support::seed_rough_color_plan()
}
