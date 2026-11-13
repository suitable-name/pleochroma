//! Off-UI-thread native file pickers. This is the only place `rfd::FileDialog` is
//! constructed, so no picker dialog blocks the UI thread.
//!
//! [`pick`] shows a [`PickerRequest`]'s dialog on a background thread and delivers the
//! result to a continuation on the UI thread via `Weak::upgrade_in_event_loop`. A
//! `#[cfg(test)]` hook ([`set_test_pick_answer`]) injects a chosen path, or a
//! cancellation, without an OS dialog.
//!
//! # Why a request struct, not a caller-supplied `rfd::FileDialog` builder
//!
//! A builder closure would need `rfd::` at every call site, defeating the single
//! construction site. Each picker's shape (kind, filters, starting directory, default
//! file name) is plain data instead, and only [`pick`] (via the private
//! [`build_and_show`]) turns it into a real `rfd::FileDialog`.
//!
//! # Callers
//!
//! `gui::editor::native_io::picker` wraps [`pick`] as its own `pick_file`/`PickKind`.
//! A Slint callback cannot await a dialog, so a dialog whose result fills a field
//! (the Parameters panel's HDR map, the remote worker's certificate folder) fills it
//! from its continuation on the UI thread.

use crate::MainWindow;
use slint::{ComponentHandle, Weak};
use std::{cell::RefCell, collections::HashMap, path::PathBuf};

/// One filter row a picker offers: a label and the extensions (no leading dot) it
/// accepts. Mirrors `rfd::FileDialog::add_filter`.
///
/// Owned rather than `'static`: most filters are literals, but
/// `gui::library::detail::attachments` builds one from an attachment's runtime
/// extension, which a `'static` field would need to leak.
pub(super) struct PickerFilter {
    pub label: String,
    pub extensions: Vec<String>,
}

impl PickerFilter {
    /// A single extension that doubles as the filter's label: `single("asc")` is
    /// `label: "asc", extensions: ["asc"]`. Build the struct literal for another label.
    #[must_use]
    pub fn single(extension: impl Into<String>) -> Self {
        let extension = extension.into();
        Self {
            extensions: vec![extension.clone()],
            label: extension,
        }
    }
}

/// Which native dialog [`pick`] should show.
pub(super) enum PickerKind {
    /// A single-file "Open" picker.
    OpenFile,
    /// A single-file "Save As" picker.
    SaveFile,
    /// A folder picker.
    PickFolder,
}

/// Everything one native dialog needs, as plain data: each `rfd::FileDialog` builder
/// method the app uses, as a field. See the module doc comment.
pub(super) struct PickerRequest {
    pub kind: PickerKind,
    /// `rfd::FileDialog::set_title`. `None` leaves the OS's own default title.
    pub title: Option<String>,
    /// `rfd::FileDialog::add_filter`, applied in order -- the FIRST filter is
    /// the dialog's own default/leading one, matching `rfd`'s own convention.
    pub filters: Vec<PickerFilter>,
    /// `rfd::FileDialog::set_file_name` -- a `SaveFile` picker's suggested
    /// name. Ignored for `OpenFile`/`PickFolder`.
    pub default_file_name: Option<String>,
    /// `rfd::FileDialog::set_directory` -- the starting directory. `None`
    /// leaves the OS's own last-used-directory memory in place.
    pub starting_dir: Option<PathBuf>,
}

/// A picker answer (`None` when cancelled), wrapped so [`TEST_PICK_ANSWER`] stays a
/// single `Option` rather than `Option<Option<PathBuf>>` (`clippy::option_option`).
struct TestPickAnswer(Option<PathBuf>);

/// A stashed [`pick`] continuation, named to keep [`PENDING_PICKS`] under clippy's
/// `type_complexity` lint.
type PickContinuation = Box<dyn FnOnce(&MainWindow, Option<PathBuf>)>;

thread_local! {
    /// Test hook: when `Some`, the NEXT [`pick`] call resolves to this answer
    /// with no OS dialog and no spawned thread, consumed (taken) exactly once.
    /// `TestPickAnswer(None)` stubs a dismissed picker.
    static TEST_PICK_ANSWER: RefCell<Option<TestPickAnswer>> = const { RefCell::new(None) };
    /// Continuations waiting for a [`pick`] answer, UI thread only. `on_done` is not
    /// `Send`, so only the `u64` key and the `Send` answer cross the thread boundary.
    static PENDING_PICKS: RefCell<HashMap<u64, PickContinuation>> = RefCell::new(HashMap::new());
    static NEXT_PICK_KEY: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Forgets the picker continuations a previous main window left parked (each closes over that
/// window's state, often the design library handle). A picker dialog still open then finds
/// nothing to answer. The test answer is left alone. See `gui::window_release`.
pub(super) fn release_window_state() {
    let pending = PENDING_PICKS.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
    drop(pending);
}

/// Names this module's holder in `out` while it is still set. See
/// `gui::window_release::live_holders`.
#[cfg(feature = "ui-harness")]
pub(super) fn note_holders(out: &mut Vec<&'static str>) {
    if PENDING_PICKS.with(|cell| !cell.borrow().is_empty()) {
        out.push("pickers PENDING_PICKS (parked continuations)");
    }
}

/// Test-only: see [`TEST_PICK_ANSWER`]'s own doc comment.
#[cfg(test)]
pub(super) fn set_test_pick_answer(answer: Option<PathBuf>) {
    TEST_PICK_ANSWER.with(|cell| *cell.borrow_mut() = Some(TestPickAnswer(answer)));
}

/// Pure precedence check, split out of [`pick`] purely so a test can exercise
/// the hook without a live `MainWindow`: `Some` (consuming the stashed answer)
/// when a test has stubbed one, `None` otherwise (the ordinary "show a real
/// picker" path).
fn take_test_pick_answer() -> Option<TestPickAnswer> {
    TEST_PICK_ANSWER.with(|cell| cell.borrow_mut().take())
}

/// Short name of a picker kind, for the UI harness's "unexpected file dialog" report.
#[cfg(feature = "ui-harness")]
const fn kind_label(kind: &PickerKind) -> &'static str {
    match kind {
        PickerKind::OpenFile => "open file",
        PickerKind::SaveFile => "save file",
        PickerKind::PickFolder => "pick folder",
    }
}

/// The `ui-harness` replacement for the native dialog: the next queued answer of
/// `crate::harness_support` (an empty queue records an "unexpected file dialog" failure
/// and answers "cancelled"). `None` means the real dialog is shown, which is also what a
/// build the harness does not drive (`crate::harness_seams_active`) does.
#[cfg(feature = "ui-harness")]
fn harness_answer(request: &PickerRequest) -> Option<TestPickAnswer> {
    if !crate::harness_seams_active() {
        return None;
    }
    let title = request.title.as_deref().unwrap_or("<untitled>");
    let detail = format!(
        "{title} ({}, suggested name {:?})",
        kind_label(&request.kind),
        request.default_file_name
    );
    Some(TestPickAnswer(crate::harness_support::next_picker_answer(
        &detail,
    )))
}

/// Feature-off twin of [`harness_answer`]: never answers, so [`pick`] shows the real dialog.
#[cfg(not(feature = "ui-harness"))]
const fn harness_answer(_request: &PickerRequest) -> Option<TestPickAnswer> {
    None
}

/// Turns a [`PickerRequest`] into a real `rfd::FileDialog` and shows it,
/// returning the chosen path (`None` on cancel/dismiss) -- the ONLY function
/// in this app that constructs one. Always called from a background thread
/// (see [`pick`]), never the UI thread.
fn build_and_show(request: &PickerRequest) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new();
    if let Some(title) = &request.title {
        dialog = dialog.set_title(title);
    }
    for filter in &request.filters {
        let extensions: Vec<&str> = filter.extensions.iter().map(String::as_str).collect();
        dialog = dialog.add_filter(&filter.label, &extensions);
    }
    if let Some(name) = &request.default_file_name {
        dialog = dialog.set_file_name(name);
    }
    if let Some(dir) = &request.starting_dir {
        dialog = dialog.set_directory(dir);
    }
    match request.kind {
        PickerKind::OpenFile => dialog.pick_file(),
        PickerKind::SaveFile => dialog.save_file(),
        PickerKind::PickFolder => dialog.pick_folder(),
    }
}

/// The one entry point every native file and folder picker uses: shows `request`'s
/// dialog on a background thread and delivers the result (`None` on cancel/dismiss)
/// to `on_done` on the UI thread. The caller must not hold a borrow of its own state
/// across this call: pick first, then borrow. `on_done` never runs if the window
/// closed while the dialog was open.
pub(super) fn pick(
    ui: &MainWindow,
    request: PickerRequest,
    on_done: impl FnOnce(&MainWindow, Option<PathBuf>) + 'static,
) {
    if let Some(TestPickAnswer(answer)) = take_test_pick_answer() {
        on_done(ui, answer);
        return;
    }
    // `ui-harness` builds: answered from the harness's queue, no OS dialog, no thread.
    if let Some(TestPickAnswer(answer)) = harness_answer(&request) {
        on_done(ui, answer);
        return;
    }
    let key = NEXT_PICK_KEY.with(|c| {
        let key = c.get();
        c.set(key + 1);
        key
    });
    PENDING_PICKS.with(|cell| {
        cell.borrow_mut().insert(key, Box::new(on_done));
    });
    let ui_weak: Weak<MainWindow> = ui.as_weak();
    std::thread::spawn(move || {
        let picked = build_and_show(&request);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            if let Some(on_done) = PENDING_PICKS.with(|cell| cell.borrow_mut().remove(&key)) {
                on_done(&ui, picked);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::{TEST_PICK_ANSWER, TestPickAnswer, set_test_pick_answer, take_test_pick_answer};
    use std::path::PathBuf;

    /// The test hook is consumed exactly once, and `None` stubs a dismissed picker.
    #[test]
    fn pick_test_hook_is_consumed_exactly_once() {
        // Cleared first: the thread-local may hold an answer another test left behind.
        // (The real dialog path needs a display and is not exercised here.)
        TEST_PICK_ANSWER.with(|cell| *cell.borrow_mut() = None);
        assert!(take_test_pick_answer().is_none());

        set_test_pick_answer(Some(PathBuf::from("chosen.asc")));
        let answer = take_test_pick_answer().expect("a stashed answer must be returned");
        assert_eq!(answer.0, Some(PathBuf::from("chosen.asc")));
        // Consumed: a second take before another `set` sees nothing stashed.
        assert!(take_test_pick_answer().is_none());

        // `Some(None)` stubs a dismissed picker -- distinct from "no hook at all".
        set_test_pick_answer(None);
        assert_eq!(take_test_pick_answer().expect("stashed").0, None);
    }

    /// Pins the wrapper's single field, so a rename shows up in this module's tests.
    #[test]
    fn test_pick_answer_wraps_the_answer_directly() {
        assert_eq!(TestPickAnswer(None).0, None);
    }
}
