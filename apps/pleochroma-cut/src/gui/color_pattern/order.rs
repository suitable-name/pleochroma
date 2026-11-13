//! The order of the layer list and the zones the cutter hid, as pure rules.
//!
//! A pattern stores its zones bottom to top: the base first (index 0), later zones cover earlier
//! ones. The window lists them the way image editors do, the top layer first and the base last, so
//! a list row (a view index) and a stored index are mirror images of each other. Hiding a layer
//! (the eye) only lasts for the open session: [`Hidden`] holds the stored indices, and
//! [`Remap`] keeps them on the right zones when an edit moves, adds or removes zones.

/// The mirror of `index` in a pattern of `zones` zones besides the base: a stored index becomes
/// the list row that shows it, and a list row becomes the stored index it shows. An index past the
/// last one maps to 0.
pub(super) const fn flip(index: usize, zones: usize) -> usize {
    zones.saturating_sub(index)
}

/// The stored index of list row `row` (`ColorPatternModel.zone_rows` order), or `None` for a row
/// the list does not have.
pub(super) fn row_to_stored(row: i32, zones: usize) -> Option<usize> {
    let row = usize::try_from(row).ok().filter(|row| *row <= zones)?;
    Some(flip(row, zones))
}

/// The list row of stored zone `stored` (the base is the last row).
pub(super) const fn stored_to_row(stored: usize, zones: usize) -> usize {
    flip(stored, zones)
}

/// What an edit did to the stored indices of the zones, so the hidden ones stay on their zones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Remap {
    /// The zones keep their indices (a parameter, a color, a name).
    Keep,
    /// The zones are not the same ones any more (a template, a preset, an undo).
    Clear,
    /// A zone was inserted at this stored index; the ones from there up move up by one.
    Insert(usize),
    /// The zone at this stored index was removed; the ones above it move down by one.
    Remove(usize),
    /// The zone at `from` was taken out and put back at `to`.
    Move {
        /// The stored index the zone had.
        from: usize,
        /// The stored index it has now.
        to: usize,
    },
}

/// The zones hidden in the preview, by stored index (the base cannot be hidden). Session-only:
/// nothing here is saved, and the pattern itself is never changed by it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct Hidden(Vec<usize>);

impl Hidden {
    /// Whether stored zone `stored` is hidden.
    pub(super) fn contains(&self, stored: usize) -> bool {
        self.0.contains(&stored)
    }

    /// Hides stored zone `stored`, or shows it again when it is hidden; the base (0) is never
    /// hidden.
    pub(super) fn toggle(&mut self, stored: usize) {
        if stored == 0 {
            return;
        }
        if let Some(at) = self.0.iter().position(|hidden| *hidden == stored) {
            self.0.remove(at);
        } else {
            self.0.push(stored);
            self.0.sort_unstable();
        }
    }

    /// Forgets hidden indices that no zone has any more (a pattern of `zones` zones).
    pub(super) fn retain_within(&mut self, zones: usize) {
        self.0.retain(|hidden| *hidden <= zones);
    }

    /// Follows the zones through `remap`.
    pub(super) fn apply(&mut self, remap: Remap) {
        match remap {
            Remap::Keep => {}
            Remap::Clear => self.0.clear(),
            Remap::Insert(at) => {
                for hidden in &mut self.0 {
                    if *hidden >= at {
                        *hidden += 1;
                    }
                }
            }
            Remap::Remove(at) => {
                self.0.retain(|hidden| *hidden != at);
                for hidden in &mut self.0 {
                    if *hidden > at {
                        *hidden -= 1;
                    }
                }
            }
            Remap::Move { from, to } => {
                for hidden in &mut self.0 {
                    *hidden = moved(*hidden, from, to);
                }
                self.0.sort_unstable();
            }
        }
    }
}

/// Where index `index` ends up when the zone at `from` is taken out and put back at `to`.
const fn moved(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < to && index > from && index <= to {
        index - 1
    } else if to < from && index >= to && index < from {
        index + 1
    } else {
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_the_stored_order_upside_down() {
        // Two zones besides the base: rows are Zone 2, Zone 1, Base.
        assert_eq!(row_to_stored(0, 2), Some(2));
        assert_eq!(row_to_stored(1, 2), Some(1));
        assert_eq!(row_to_stored(2, 2), Some(0));
        assert_eq!(row_to_stored(3, 2), None);
        assert_eq!(row_to_stored(-1, 2), None);
        for stored in 0..=2 {
            assert_eq!(flip(stored_to_row(stored, 2), 2), stored);
        }
        // A pattern with only its base: the one row is the base.
        assert_eq!(row_to_stored(0, 0), Some(0));
        assert_eq!(stored_to_row(0, 0), 0);
    }

    #[test]
    fn the_base_is_always_the_last_row() {
        for zones in 0..=4 {
            assert_eq!(stored_to_row(0, zones), zones);
            assert_eq!(row_to_stored(i32::try_from(zones).unwrap(), zones), Some(0));
        }
    }

    #[test]
    fn the_base_cannot_be_hidden_and_a_toggle_twice_shows_the_zone_again() {
        let mut hidden = Hidden::default();
        hidden.toggle(0);
        assert!(!hidden.contains(0));
        hidden.toggle(2);
        assert!(hidden.contains(2) && !hidden.contains(1));
        hidden.toggle(2);
        assert!(!hidden.contains(2));
    }

    #[test]
    fn hidden_zones_follow_their_zone_through_an_edit() {
        let mut hidden = Hidden::default();
        hidden.toggle(2);
        // A zone is added below it: it is zone 3 now.
        hidden.apply(Remap::Insert(1));
        assert!(hidden.contains(3) && !hidden.contains(2));
        // The zone under it is deleted: zone 2 again.
        hidden.apply(Remap::Remove(1));
        assert!(hidden.contains(2));
        // It moves up to the top of three zones, and another one moves down past it.
        hidden.apply(Remap::Move { from: 2, to: 3 });
        assert!(hidden.contains(3));
        hidden.apply(Remap::Move { from: 3, to: 1 });
        assert!(hidden.contains(1));
        // Deleting the hidden zone itself forgets it.
        hidden.apply(Remap::Remove(1));
        assert!(!hidden.contains(1));
    }

    #[test]
    fn a_zone_that_another_one_passes_shifts_by_one() {
        let mut hidden = Hidden::default();
        hidden.toggle(2);
        // Zone 1 goes to the top (index 3): the hidden zone 2 is zone 1 now.
        hidden.apply(Remap::Move { from: 1, to: 3 });
        assert!(hidden.contains(1));
        // Zone 3 goes to the bottom (index 1): the hidden zone 1 is zone 2 again.
        hidden.apply(Remap::Move { from: 3, to: 1 });
        assert!(hidden.contains(2));
    }

    #[test]
    fn a_new_pattern_shows_every_zone_and_a_shorter_one_forgets_the_missing() {
        let mut hidden = Hidden::default();
        hidden.toggle(1);
        hidden.toggle(3);
        hidden.retain_within(2);
        assert!(hidden.contains(1) && !hidden.contains(3));
        hidden.apply(Remap::Clear);
        assert!(!hidden.contains(1));
    }
}
