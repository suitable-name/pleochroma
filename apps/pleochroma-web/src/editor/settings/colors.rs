//! The Design settings dialog's color combo and the cutter's saved color variants.
//!
//! The combo is the keyed list `pleochroma_web_core::design_color::DesignColors` builds
//! (Material Default, the material's built-in variants, the saved ones, the generic presets on a
//! one-color material, Custom last). The Slint combo hands only an index back, so the list the
//! user last saw is remembered here and an Apply resolves the index against THAT list, never
//! against one rebuilt after the material changed.
//!
//! The saved variants live in `IndexedDB` (`storage::color_variants`); they are read once when the
//! page starts ([`load_saved`]) and kept in memory, so building the combo never waits for the
//! browser. A variant saved from the Custom editor is added to the memory list as soon as the
//! store accepts it.

use std::cell::RefCell;

use crate::{
    app::{Ctx, diagnostics::console_warn},
    editor::inspector::refresh,
    storage::color_variants,
};
use pleochroma::optics::materials::{GemMaterial, variants::ColorVariant};
use pleochroma_cut_core::{Design, MaterialSelection};
use pleochroma_editor::material::with_body_color_choice;
use pleochroma_web_core::design_color::DesignColors;

thread_local! {
    /// The cutter's saved variants of this browser (every material), as last read or saved.
    static SAVED: RefCell<Vec<ColorVariant>> = const { RefCell::new(Vec::new()) };
    /// The combo list as the Slint thread last saw it (see the module doc).
    static SHOWN: RefCell<Option<DesignColors>> = const { RefCell::new(None) };
}

/// The saved variants of this browser.
fn saved_variants() -> Vec<ColorVariant> {
    SAVED.with_borrow(Clone::clone)
}

/// Adds `variant` to the in-memory saved list (replacing the one under its key).
pub(super) fn remember_saved(variant: ColorVariant) {
    SAVED.with_borrow_mut(|saved| {
        saved.retain(|existing| existing.key != variant.key);
        saved.push(variant);
    });
}

/// The combo for `design`, remembered for [`material_for_apply`]; the flag says whether its rows
/// differ from the ones shown before (the page then re-seeds the combo's index).
pub(super) fn shown(design: &Design, custom: &[GemMaterial]) -> (DesignColors, bool) {
    let saved = saved_variants();
    SHOWN.with_borrow_mut(|slot| {
        let colors = DesignColors::for_design(slot.as_ref(), design, custom, &saved);
        let changed = slot
            .as_ref()
            .is_none_or(|previous| previous.items != colors.items);
        *slot = Some(colors.clone());
        (colors, changed)
    })
}

/// The material an Apply of the whole material row produces: `material` (the parsed material/RI
/// row) with the color of combo row `color_index` applied on top. Before any list was shown the
/// index falls back to the fixed-index contract.
pub(super) fn material_for_apply(
    material: MaterialSelection,
    color_index: i32,
    current: &MaterialSelection,
) -> MaterialSelection {
    SHOWN.with_borrow(|slot| match slot {
        Some(colors) => colors.material_for_apply(material, color_index, current),
        None => with_body_color_choice(material, color_index, current),
    })
}

/// Reads the saved variants from `IndexedDB` (in the background) and refreshes the dialog when
/// they arrive. A browser without `IndexedDB` simply has none; unreadable records are logged.
pub(super) fn load_saved(ctx: &Ctx) {
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        match color_variants::list().await {
            Ok(saved) => {
                for sentence in &saved.skipped {
                    console_warn(sentence);
                }
                if saved.variants.is_empty() {
                    return;
                }
                SAVED.with_borrow_mut(|memory| {
                    // Anything saved while the read ran stays.
                    for variant in saved.variants {
                        if !memory.iter().any(|known| known.key == variant.key) {
                            memory.push(variant);
                        }
                    }
                });
                refresh(&ctx);
            }
            Err(error) => console_warn(&format!("saved colors are unavailable: {error}")),
        }
    });
}
