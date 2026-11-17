//! Native file pickers that never block the UI thread.
//!
//! A picker dialog waits for the user, so it runs on a helper thread; the chosen path comes
//! back to the UI thread through [`slint::Weak::upgrade_in_event_loop`], the same way the
//! desktop app's pickers work. Cancelling calls nothing. This is the only place in the
//! coordinator window that builds an `rfd::FileDialog`.

use crate::gui::app::AppWindow;
use std::path::{Path, PathBuf};

/// Which dialog to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Choose one existing file.
    OpenFile,
    /// Choose where to save a file.
    SaveFile,
    /// Choose a folder.
    Folder,
}

/// What one dialog shows, as plain data.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// Which dialog.
    pub kind: Kind,
    /// The dialog's title.
    pub title: &'a str,
    /// The file filter: its label and the extensions (without the dot).
    pub filter: Option<(&'a str, &'a [&'a str])>,
    /// The file name a save dialog suggests.
    pub file_name: Option<&'a str>,
    /// The folder the dialog starts in, when it exists.
    pub start_in: Option<&'a Path>,
}

impl<'a> Request<'a> {
    /// A dialog of `kind` with `title` and nothing else.
    #[must_use]
    pub const fn new(kind: Kind, title: &'a str) -> Self {
        Self {
            kind,
            title,
            filter: None,
            file_name: None,
            start_in: None,
        }
    }

    /// The same dialog with a file filter.
    #[must_use]
    pub const fn filter(mut self, label: &'a str, extensions: &'a [&'a str]) -> Self {
        self.filter = Some((label, extensions));
        self
    }

    /// The same dialog with a suggested file name.
    #[must_use]
    pub const fn file_name(mut self, name: &'a str) -> Self {
        self.file_name = Some(name);
        self
    }

    /// The same dialog starting in `dir`.
    #[must_use]
    pub const fn start_in(mut self, dir: &'a Path) -> Self {
        self.start_in = Some(dir);
        self
    }
}

/// Shows the dialog on a helper thread and, when a path was chosen, calls `done` with the
/// window and the path on the UI thread.
///
/// # Errors
///
/// A message in plain words when the helper thread could not start.
pub fn pick<F>(weak: &slint::Weak<AppWindow>, request: &Request<'_>, done: F) -> Result<(), String>
where
    F: FnOnce(&AppWindow, PathBuf) + Send + 'static,
{
    pick_or_else(weak, request, done, |_| {})
}

/// Like [`pick`], and when the dialog was closed without a choice `cancelled` runs with the
/// window on the UI thread (to take back what the page showed while it was open).
///
/// # Errors
///
/// A message in plain words when the helper thread could not start.
pub fn pick_or_else<F, C>(
    weak: &slint::Weak<AppWindow>,
    request: &Request<'_>,
    done: F,
    cancelled: C,
) -> Result<(), String>
where
    F: FnOnce(&AppWindow, PathBuf) + Send + 'static,
    C: FnOnce(&AppWindow) + Send + 'static,
{
    let kind = request.kind;
    let title = request.title.to_owned();
    let filter = request.filter.map(|(label, extensions)| {
        (
            label.to_owned(),
            extensions
                .iter()
                .map(|extension| (*extension).to_owned())
                .collect::<Vec<_>>(),
        )
    });
    let file_name = request.file_name.map(str::to_owned);
    let start_in = request
        .start_in
        .filter(|dir| dir.is_dir())
        .map(Path::to_path_buf);
    let weak = weak.clone();
    std::thread::Builder::new()
        .name("file-picker".to_owned())
        .spawn(move || {
            let mut dialog = rfd::FileDialog::new().set_title(title);
            if let Some((label, extensions)) = filter {
                dialog = dialog.add_filter(label, extensions.as_slice());
            }
            if let Some(name) = file_name {
                dialog = dialog.set_file_name(name);
            }
            if let Some(dir) = start_in {
                dialog = dialog.set_directory(dir);
            }
            let chosen = match kind {
                Kind::OpenFile => dialog.pick_file(),
                Kind::SaveFile => dialog.save_file(),
                Kind::Folder => dialog.pick_folder(),
            };
            // A closed window has nobody to tell.
            if let Some(path) = chosen {
                let _ = weak.upgrade_in_event_loop(move |window| done(&window, path));
            } else {
                let _ = weak.upgrade_in_event_loop(move |window| cancelled(&window));
            }
        })
        .map(|_| ())
        .map_err(|e| format!("The file dialog could not open: {e}"))
}
