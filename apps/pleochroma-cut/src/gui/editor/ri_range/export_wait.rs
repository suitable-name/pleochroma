//! The cutting-sheet export's wait for the traced RI range.
//!
//! A design that chose "Pleochroma estimate" stores the choice only, never a number. The sheet's
//! number is the traced range of the design as it is when the sheet is written
//! ([`super::latest`]). When the worker has no result yet, the export shows "Computing the RI
//! range..." (`RiFieldModel.export_waiting`, the confirm dialog of `global_dialogs.slint`) with
//! Cancel. Cancel writes the sheet without the range; a result that lands first writes it with.
//!
//! [`details_then`] is the one call the export makes: it hands the header details to the rest of
//! the export once they are settled, at once for every other choice.

use super::{latest, when_ready};
use crate::{MainWindow, RiFieldModel, gui::editor::cut_sheet::sheet_details};
use pleochroma_cut_core::{
    Design,
    native::{DesignMetadata, RiRangeChoice},
};
use pleochroma_editor::{cut_sheet::SheetDetails, ri_range::RiStatement};
use slint::ComponentHandle as _;
use std::cell::RefCell;

/// What runs with the settled details.
type Continuation<C> = Box<dyn FnOnce(&C, SheetDetails)>;

/// An export that waits for the traced range.
struct Pending<C> {
    /// The details without the estimate.
    details: SheetDetails,
    /// The rest of the export.
    then: Continuation<C>,
}

thread_local! {
    /// The export that waits, if any. The confirm dialog is modal, so there is at most one.
    static PENDING: RefCell<Option<Pending<MainWindow>>> = const { RefCell::new(None) };
}

/// Takes the waiting export out of `slot` and gives it `statement` as its RI range.
///
/// `None` when nothing waits: the export was cancelled or has already run. Whichever of Cancel
/// and the traced result comes first takes it; the other finds the slot empty and does nothing.
fn settle<C>(
    slot: &RefCell<Option<Pending<C>>>,
    statement: Option<RiStatement>,
) -> Option<(Continuation<C>, SheetDetails)> {
    let pending = slot.borrow_mut().take()?;
    let mut details = pending.details;
    details.ri_range = statement;
    Some((pending.then, details))
}

/// Runs `then` with the header details of `design`'s cutting sheet, once the RI range is settled.
///
/// Every choice but the estimate settles at once ([`sheet_details`]). The estimate takes the
/// newest traced range ([`latest`]); without one the export waits behind "Computing the RI
/// range..." until the worker lands it ([`when_ready`]) or the user cancels, which writes the
/// sheet without the range.
pub(in crate::gui::editor) fn details_then(
    ui: &MainWindow,
    design: &Design,
    metadata: &DesignMetadata,
    date_text: &str,
    then: impl FnOnce(&MainWindow, SheetDetails) + 'static,
) {
    if metadata.ri_range_choice() != RiRangeChoice::Estimate {
        then(ui, sheet_details(design, metadata, date_text, None));
        return;
    }
    if let Some(range) = latest() {
        then(
            ui,
            sheet_details(design, metadata, date_text, range.statement()),
        );
        return;
    }
    PENDING.with(|slot| {
        *slot.borrow_mut() = Some(Pending {
            details: sheet_details(design, metadata, date_text, None),
            then: Box::new(then),
        });
    });
    ui.global::<RiFieldModel>().set_export_waiting(true);
    when_ready(ui, |ui, range| finish(ui, range.statement()));
}

/// Cancel of the waiting export: the sheet is written without the range.
///
/// Answers `RiFieldModel.cancel_export_wait`. Nothing happens when no export waits.
pub(in crate::gui::editor) fn cancel(ui: &MainWindow) {
    finish(ui, None);
}

/// Closes the wait and runs the export with `statement` on its "RI range" row.
fn finish(ui: &MainWindow, statement: Option<RiStatement>) {
    let Some((then, details)) = PENDING.with(|slot| settle(slot, statement)) else {
        return;
    };
    ui.global::<RiFieldModel>().set_export_waiting(false);
    then(ui, details);
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_editor::ri_range::RiSource;
    use std::{cell::Cell, rc::Rc};

    fn traced() -> RiStatement {
        RiStatement {
            low: 1.63,
            high: None,
            source: RiSource::Estimate,
        }
    }

    /// A waiting export whose continuation records the range it was given.
    fn waiting(seen: &Rc<Cell<Option<Option<RiStatement>>>>) -> RefCell<Option<Pending<()>>> {
        let seen = Rc::clone(seen);
        RefCell::new(Some(Pending {
            details: SheetDetails::default(),
            then: Box::new(move |_, details| seen.set(Some(details.ri_range))),
        }))
    }

    #[test]
    fn the_traced_range_lands_on_the_waiting_sheet() {
        let seen = Rc::new(Cell::new(None));
        let slot = waiting(&seen);
        let (then, details) = settle(&slot, Some(traced())).expect("an export waits");
        then(&(), details);
        assert_eq!(seen.get(), Some(Some(traced())));
    }

    #[test]
    fn cancel_writes_the_sheet_without_the_range() {
        let seen = Rc::new(Cell::new(None));
        let slot = waiting(&seen);
        let (then, details) = settle(&slot, None).expect("an export waits");
        then(&(), details);
        assert_eq!(seen.get(), Some(None));
    }

    #[test]
    fn the_second_of_cancel_and_the_result_finds_nothing_to_do() {
        let seen = Rc::new(Cell::new(None));
        let slot = waiting(&seen);
        assert!(settle(&slot, None).is_some());
        // The result lands after Cancel: the export has already run, so nothing runs twice.
        assert!(settle(&slot, Some(traced())).is_none());
        assert!(settle(&RefCell::<Option<Pending<()>>>::new(None), None).is_none());
    }
}
