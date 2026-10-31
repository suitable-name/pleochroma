//! The inclusion and play-of-color choice a design file states, in the form the library stores
//! (`diagram_inclusion_effects`): the text of the two `[render_effects]` tables, which the
//! cached-render fingerprints hash. A file that states neither reads as `None`.

use pleochroma_formats::native::design::{self, DesignFile};

/// The text of the `[render_effects.inclusions]` and `[render_effects.structure]` tables `file`
/// states; `None` when it states neither.
#[must_use]
pub fn inclusion_effects_of(file: &DesignFile) -> Option<String> {
    let effects = file.render_effects.as_ref()?;
    if effects.inclusions.is_none() && effects.structure.is_none() {
        return None;
    }
    Some(format!("{:?}|{:?}", effects.inclusions, effects.structure))
}

/// [`inclusion_effects_of`] for a stored design file's bytes.
///
/// The bytes are a `.psdd` file; `None` for bytes that are not a readable
/// design file. Used to backfill the table for a library that predates it and to follow a
/// mirrored design's attached file.
#[must_use]
pub fn inclusion_effects_of_bytes(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let file = design::parse(text).ok()?;
    inclusion_effects_of(&file)
}
