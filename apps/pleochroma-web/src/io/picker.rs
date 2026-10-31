//! The file pickers: `rfd`'s async picker (on wasm32 a hidden `<input type="file" multiple>`),
//! so several files can be selected together.
//!
//! - [`pick_and_open`]: File > Open (designs and `.hdr` maps).
//! - [`pick_design_files`]: the design library's Import (design files only); the files go to
//!   the caller instead of being opened.

use super::{IncomingFile, open_files, oversize_message};
use crate::app::{
    Ctx,
    push::{MessageKind, show_message},
};
use pleochroma_web_core::open_route::OPEN_EXTENSIONS;

/// The filter label: the design file first, then the other formats still read.
const FILTER_LABEL: &str = "Designs (.psdd, .asc, .gem, .gcs) and .hdr Maps";

/// The design library's filter label (no environment maps).
const LIBRARY_FILTER_LABEL: &str = "Designs (.psdd, .asc, .gem, .gcs)";

/// The extensions the library imports: [`OPEN_EXTENSIONS`] without the last one (`hdr`).
const LIBRARY_EXTENSIONS: [&str; 4] = [
    OPEN_EXTENSIONS[0],
    OPEN_EXTENSIONS[1],
    OPEN_EXTENSIONS[2],
    OPEN_EXTENSIONS[3],
];

/// Shows the picker with one combined filter and reads the chosen files. A dismissed picker
/// gives an empty list and says nothing. Each file's `File.size` is checked against the page's
/// limits before its bytes are read, and an oversize file is named and skipped.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
async fn pick(ctx: &Ctx, label: &str, extensions: &[&str]) -> Vec<IncomingFile> {
    let Some(handles) = rfd::AsyncFileDialog::new()
        .add_filter(label, extensions)
        .pick_files()
        .await
    else {
        return Vec::new();
    };
    let mut files = Vec::with_capacity(handles.len());
    for handle in handles {
        let name = handle.file_name();
        if let Some(message) = oversize_message(&name, handle.inner().size()) {
            show_message(ctx, MessageKind::Error, &message);
            continue;
        }
        files.push(IncomingFile {
            name,
            bytes: handle.read().await,
        });
    }
    files
}

/// Shows the picker and opens whatever the user chose; a dismissed picker is not
/// an error and says nothing.
pub fn pick_and_open(ctx: &Ctx) {
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        // One combined filter (`OPEN_EXTENSIONS` lists `.psdd` first).
        let files = pick(&ctx, FILTER_LABEL, &OPEN_EXTENSIONS).await;
        if !files.is_empty() {
            open_files(&ctx, files);
        }
    });
}

/// Shows the picker for design files and hands the chosen ones to `then` (not called when the
/// picker is dismissed or every file was oversize).
pub fn pick_design_files(ctx: &Ctx, then: impl FnOnce(Vec<IncomingFile>) + 'static) {
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let files = pick(&ctx, LIBRARY_FILTER_LABEL, &LIBRARY_EXTENSIONS).await;
        if !files.is_empty() {
            then(files);
        }
    });
}
