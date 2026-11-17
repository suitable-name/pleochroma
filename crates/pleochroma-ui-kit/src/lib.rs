//! The shared Slint UI kit of the Pleochroma apps.
//!
//! The crate holds `.slint` sources only: the dark `Theme` tokens, the `PleoPalette`
//! colors, the figure and glass components and the `Kit*` widgets (`ui/theme.slint`,
//! `ui/shared/*`, `ui/components/kit/*`). Pleochroma Cut, the coordinator and the Android
//! app all import them, so a packaged app crate no longer reaches into another app's
//! folder.
//!
//! An app's build script registers the kit as the Slint library [`LIBRARY`]:
//!
//! ```ignore
//! slint_build::CompilerConfiguration::new()
//!     .with_library_paths(pleochroma_ui_kit::library_paths())
//! ```
//!
//! and its `.slint` files import from it, for example
//! `import { Theme } from "@pleochroma-kit/theme.slint";`.

use std::{collections::HashMap, path::PathBuf};

/// The library name the kit is imported under (`@pleochroma-kit/...`).
pub const LIBRARY: &str = "pleochroma-kit";

/// The folder holding the kit's `.slint` files.
#[must_use]
pub fn ui_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

/// The library map for `CompilerConfiguration::with_library_paths`: [`LIBRARY`] to [`ui_dir`].
#[must_use]
pub fn library_paths() -> HashMap<String, PathBuf> {
    HashMap::from([(LIBRARY.to_string(), ui_dir())])
}
