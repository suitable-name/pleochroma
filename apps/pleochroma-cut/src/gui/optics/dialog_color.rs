//! The body colour the material editor dialog is holding, beside [`super::physics_state`].
//!
//! The dialog's preset row is a legacy three-band triple; the path-aware L*C*h editor
//! (`pleochroma_editor::lch_color`) solves a seven-band colour next to the nearest triple. This
//! module remembers the band rows of that solve until "Save" writes them to the vault row
//! (`CustomMaterialRow::absorption_bands_json`): there is one dialog, so one slot. It is filled
//! from the selected material when the dialog's pre-fill is pushed, replaced by an editor
//! "Apply colour", and dropped when a preset swatch is chosen; Custom (keep) leaves it alone.
//! Physics state is never touched.

use std::sync::{Mutex, PoisonError};

use pleochroma_cut_core::{MaterialSelection, material::RayBandRows};

/// The colour of the dialog's material.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DialogColor {
    /// The seven-band rows `[centre_nm, width_nm, amplitude_per_mm]`, `None` for none.
    pub bands: Option<Vec<[f32; 3]>>,
    /// The legacy triple the preset row or the solve produced, `None` for a new material. With
    /// [`Self::rays`] it is the nearest legacy color of the face-up color, what an older build
    /// shows.
    pub triple: Option<[f32; 3]>,
    /// The per-direction rows of a material whose color depends on the direction (one row set per
    /// ray, see [`RayBandRows`]), `None` for the single color of [`Self::bands`]. Only one of the
    /// two is ever held.
    pub rays: Option<RayBandRows>,
}

static SLOT: Mutex<DialogColor> = Mutex::new(DialogColor {
    bands: None,
    triple: None,
    rays: None,
});

/// The colour the dialog holds now.
#[must_use]
pub fn current() -> DialogColor {
    SLOT.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Replaces the held colour (the dialog's pre-fill, or an editor "Apply colour").
pub fn set(color: DialogColor) {
    *SLOT.lock().unwrap_or_else(PoisonError::into_inner) = color;
}

/// A preset swatch was chosen: the bands go and the triple becomes the preset's. `None` is the
/// "Custom (keep)" swatch, which keeps the held color whole: triple AND bands (a kept color is
/// the stored one, and a re-save must write it back unchanged).
pub fn preset_chosen(triple: Option<[f32; 3]>) {
    let Some(triple) = triple else { return };
    let mut slot = SLOT.lock().unwrap_or_else(PoisonError::into_inner);
    slot.bands = None;
    slot.rays = None;
    slot.triple = Some(triple);
}

/// The per-direction color the dialog holds for a material of the given ray count (2 for a
/// uniaxial material, 3 for a biaxial one): the held [`RayBandRows`] when they have that many
/// directions, else `None` (an isotropic color, or rows of the other rank).
#[must_use]
pub fn rays_for(color: &DialogColor, directions: usize) -> Option<&RayBandRows> {
    color
        .rays
        .as_ref()
        .filter(|rays| rays.beta.is_some() == (directions == 3) && directions >= 2)
}

/// The held colour as a selection, which is what `lch_color::lch_from_material` seeds the
/// editor's sliders from.
#[must_use]
pub fn as_selection(color: &DialogColor) -> MaterialSelection {
    MaterialSelection::none().with_body_color_bands(color.triple, color.bands.clone(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The slot is one process-wide value and tests run in parallel: this lock serialises the
    // tests that touch it.
    static SLOT_GUARD: Mutex<()> = Mutex::new(());

    fn lock_slot() -> std::sync::MutexGuard<'static, ()> {
        SLOT_GUARD.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[test]
    fn a_preset_pick_drops_the_bands_and_sets_the_triple() {
        let _guard = lock_slot();
        set(DialogColor {
            bands: Some(vec![[460.0, 45.0, 0.25]]),
            triple: Some([0.1, 0.2, 0.3]),
            ..DialogColor::default()
        });
        preset_chosen(Some([0.4, 0.5, 0.6]));
        assert_eq!(current().bands, None);
        assert_eq!(current().triple, Some([0.4, 0.5, 0.6]));
        set(DialogColor::default());
        assert_eq!(current(), DialogColor::default());
    }

    fn two_directions() -> RayBandRows {
        RayBandRows {
            o: vec![[460.0, 2500.0, 0.25]],
            e: vec![[540.0, 2500.0, 0.1]],
            beta: None,
        }
    }

    #[test]
    fn a_preset_pick_drops_the_per_direction_rows_and_keep_leaves_them() {
        let _guard = lock_slot();
        let held = DialogColor {
            triple: Some([0.1, 0.2, 0.3]),
            rays: Some(two_directions()),
            ..DialogColor::default()
        };
        set(held.clone());
        preset_chosen(None);
        assert_eq!(current(), held);
        preset_chosen(Some([0.4, 0.5, 0.6]));
        assert_eq!(current().rays, None);
        assert_eq!(current().triple, Some([0.4, 0.5, 0.6]));
        set(DialogColor::default());
    }

    #[test]
    fn held_rows_are_offered_only_to_a_material_with_that_many_directions() {
        let held = DialogColor {
            rays: Some(two_directions()),
            ..DialogColor::default()
        };
        assert!(rays_for(&held, 2).is_some());
        assert!(
            rays_for(&held, 3).is_none(),
            "two rows do not fit a biaxial one"
        );
        assert!(rays_for(&held, 1).is_none());
        let three = DialogColor {
            rays: Some(RayBandRows {
                beta: Some(vec![[500.0, 2500.0, 0.2]]),
                ..two_directions()
            }),
            ..DialogColor::default()
        };
        assert!(rays_for(&three, 3).is_some());
        assert!(rays_for(&three, 2).is_none());
        assert!(rays_for(&DialogColor::default(), 2).is_none());
    }

    #[test]
    fn keep_keeps_bands_and_triple() {
        let _guard = lock_slot();
        let held = DialogColor {
            bands: Some(vec![[460.0, 45.0, 0.25], [620.0, 30.0, 0.1]]),
            triple: Some([0.1, 0.2, 0.3]),
            ..DialogColor::default()
        };
        set(held.clone());
        preset_chosen(None);
        assert_eq!(current(), held);
        set(DialogColor::default());
    }

    #[test]
    fn keep_after_a_preset_still_has_no_bands() {
        let _guard = lock_slot();
        set(DialogColor {
            bands: Some(vec![[460.0, 45.0, 0.25]]),
            triple: Some([0.1, 0.2, 0.3]),
            ..DialogColor::default()
        });
        preset_chosen(Some([0.4, 0.5, 0.6]));
        preset_chosen(None);
        assert_eq!(current().bands, None);
        assert_eq!(current().triple, Some([0.4, 0.5, 0.6]));
        set(DialogColor::default());
    }

    #[test]
    fn the_selection_carries_triple_and_bands() {
        // Pure: `as_selection` does not touch the slot.
        let color = DialogColor {
            bands: Some(vec![[460.0, 45.0, 0.25]]),
            triple: Some([0.1, 0.2, 0.3]),
            ..DialogColor::default()
        };
        let selection = as_selection(&color);
        assert_eq!(selection.body_color_override, Some([0.1, 0.2, 0.3]));
        assert_eq!(
            selection.body_color_bands_override,
            Some(vec![[460.0, 45.0, 0.25]])
        );
        assert_eq!(
            as_selection(&DialogColor::default()),
            MaterialSelection::none()
        );
    }
}
