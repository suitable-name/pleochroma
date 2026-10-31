//! A custom material's stored default Crystal Axis, for the trial key.
//!
//! The axes baked into a custom material cannot show whether its default was "set" (a default of
//! tilt 0 reads like none), so the library row is the source (`stored_default_orientation`). A
//! trial recorded under one default must not read as the figures of another, so the key names it
//! (`record::trial_key`).
//!
//! The vault is held weakly in a `thread_local!` of its own (never the hub's, so a lookup can run
//! while the hub is borrowed). The lookup never waits for the library: when a worker holds it,
//! the rows last read stand in.

use crate::gui::optics::crystal_optics::stored_default_orientation;
use pleochroma_cut_core::{CrystalOrientation, MaterialSelection};
use pleochroma_vault::db::sqlite::Database;
use std::{
    cell::RefCell,
    sync::{Arc, Mutex, TryLockError, Weak},
};

/// A custom material's name and its stored default orientation.
type Defaults = Vec<(String, CrystalOrientation)>;

thread_local! {
    /// The vault the main window was built with, held weakly so it never keeps the database
    /// connection open past the window (`gui::window_release`).
    static VAULT: RefCell<Weak<Mutex<Database>>> = const { RefCell::new(Weak::new()) };
    /// The stored defaults as last read.
    static LAST: RefCell<Defaults> = const { RefCell::new(Vec::new()) };
}

/// Remembers the main window's vault for [`of`].
pub(in crate::gui::editor) fn register_vault(db: &Arc<Mutex<Database>>) {
    VAULT.with(|cell| *cell.borrow_mut() = Arc::downgrade(db));
    LAST.with(|cell| cell.borrow_mut().clear());
}

/// The stored default orientation of the custom material `selection` names: `None` for a
/// built-in material, one with no stored default, or no name.
#[must_use]
pub(super) fn of(selection: &MaterialSelection) -> Option<CrystalOrientation> {
    let name = selection.name.as_deref()?;
    refresh();
    LAST.with(|cell| find(&cell.borrow(), name))
}

/// The default of `name` in `defaults` (case-insensitive, as the catalogue lookup is).
fn find(defaults: &Defaults, name: &str) -> Option<CrystalOrientation> {
    defaults
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, orientation)| *orientation)
}

/// Reads the library's defaults again when it is free.
fn refresh() {
    let Some(db) = VAULT.with(|cell| cell.borrow().upgrade()) else {
        return;
    };
    let rows = match db.try_lock() {
        Ok(guard) => guard.get_custom_materials().unwrap_or_default(),
        Err(TryLockError::Poisoned(poisoned)) => poisoned
            .into_inner()
            .get_custom_materials()
            .unwrap_or_default(),
        // Busy: the rows last read stand.
        Err(TryLockError::WouldBlock) => return,
    };
    let defaults: Defaults = rows
        .iter()
        .filter_map(|row| {
            stored_default_orientation(&rows, &row.name).map(|found| (row.name.clone(), found))
        })
        .collect();
    LAST.with(|cell| *cell.borrow_mut() = defaults);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lookup_is_case_insensitive_and_names_only_stored_defaults() {
        let upright = CrystalOrientation::from_angles(0.0, 0.0, None);
        let defaults: Defaults = vec![("My Quartz".to_owned(), upright)];
        assert_eq!(find(&defaults, "my quartz"), Some(upright));
        assert_eq!(find(&defaults, "Other"), None);
    }

    #[test]
    fn without_a_vault_nothing_is_found() {
        let named = MaterialSelection {
            name: Some("My Quartz".to_owned()),
            ..MaterialSelection::none()
        };
        assert_eq!(of(&named), None);
        assert_eq!(of(&MaterialSelection::none()), None);
    }
}
