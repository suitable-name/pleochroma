//! The state of the open Color Pattern editor: one [`Session`] at a time, inside the [`Host`] that
//! owns the window, the two background workers and the handles to the vault and the render
//! context.
//!
//! The host lives in a thread-local, like the Rough Color wizard's: Slint callbacks run on the UI
//! thread and reach it through [`with_host`]. No `RefCell` borrow is held across a call that can
//! run another callback: handlers read what they need, drop the borrow, then act.

use super::{
    colors::{SolveCache, SolveWorker, SwatchStone},
    cross_section::Handle,
    model::{AXIS_CUSTOM, AXIS_IMPORTED, axis_index},
    preview::PreviewWorker,
    stone::StonePatternApply,
    undo::UndoStack,
};
use crate::{ColorPatternWindow, MainWindow, bridge::render_thread::RenderContext};
use pleochroma::optics::materials::{
    GemMaterial,
    variants::{ColorVariant, TensorRank},
};
use pleochroma_cut_core::material::pattern::{
    ColorPattern, ColorRef, OwnColor, PatternError, ZoneRankFit, zone_rank_fit,
};
use pleochroma_vault::db::sqlite::Database;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// The sentence of the Preview area: where a pattern shows.
pub(super) const PREVIEW_NOTE: &str =
    "Patterns show in Path-Traced views. Ray-Traced shows the base color.";

/// The vault and the render context the editor saves into and refreshes.
#[derive(Clone)]
pub(super) struct Deps {
    pub db: Arc<Mutex<Database>>,
    pub render_ctx: Arc<Mutex<RenderContext>>,
}

/// A handle that is being dragged in the Cross-Section.
#[derive(Debug, Clone, Copy)]
pub(super) struct Drag {
    /// The handle at the moment the drag began.
    pub handle: Handle,
    /// The drag has changed the pattern (its undo step is recorded).
    pub started: bool,
}

/// One open editor: the pattern being edited and everything around it.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent UI-session flags, not a state machine"
)]
pub(super) struct Session {
    /// The saved material name, or the stone session's target label.
    pub material_name: String,
    /// A stone session writes through this guarded port, never to a material row.
    pub stone_apply: Option<Rc<StonePatternApply>>,
    /// The snapshot the stone session opened on; library previews have no editor design.
    pub preview_design: Option<pleochroma_cut_core::Design>,
    /// Whether the stone session writes a design rather than a view-only preview.
    pub targets_design: bool,
    /// The pattern as edited.
    pub pattern: ColorPattern,
    /// The pattern as last stored (or opened); the window is "dirty" while they differ.
    pub baseline: ColorPattern,
    /// Undo and Redo.
    pub undo: UndoStack,
    /// The zone the property panel shows: 0 is the base.
    pub selected_zone: usize,
    /// The stone width a pattern is resolved at, millimeters.
    pub stone_width_mm: f64,
    /// The reference light path colors are solved and shown for, millimeters.
    pub path_mm: f64,
    /// The most colors a picked zone color may have, by the rank of the material: one on a cubic
    /// or amorphous one, Along and Across on a uniaxial one (or a birefringent, non-cubic custom
    /// material), Alpha, Beta and Gamma on a biaxial one.
    pub rank: TensorRank,
    /// The color variants of the material that fit it (the Material Default first): what the
    /// Variant source lists.
    pub variants: Vec<ColorVariant>,
    /// The cutter chose the Custom axis, so Tilt and Turn are editable even where they equal a
    /// preset.
    pub custom_axis: bool,
    /// Colors solved so far.
    pub cache: SolveCache,
    /// A solve is running.
    pub solving: bool,
    /// The color the body-color editor was opened for.
    pub pending_pick: Option<ColorRef>,
    /// 0 Crystal, 1 Current Design, 2 Cross-Section.
    pub preview_mode: i32,
    /// The Cross-Section's position along the axis, 0 to 1.
    pub slice_pos: f32,
    /// The presets the cutter saved.
    pub saved_presets: Vec<(String, ColorPattern)>,
    /// A sentence about where the pattern came from (zones of the Rough Color wizard).
    pub import_note: Option<String>,
    /// The "Discard the changes?" bar is open.
    pub confirm_discard: bool,
    /// The preset name field is open.
    pub naming_preset: bool,
    /// The handles over the Cross-Section as last drawn.
    pub handles: Vec<Handle>,
    /// The handle being dragged.
    pub drag: Option<Drag>,
    /// The handle under the pointer.
    pub hover: Option<usize>,
    /// The material the Current Design preview renders the pattern in.
    pub base_material: GemMaterial,
    /// The color the stone has without a pattern, per direction: what a typical pattern is built
    /// on (not the current base zone, which a typical pattern makes paler).
    pub own: OwnColor,
}

impl Session {
    /// The stone the swatches are drawn for: the session's light path for picked colors, its
    /// stone width and material for a color variant.
    pub(super) const fn swatch_stone(&self) -> SwatchStone<'_> {
        SwatchStone {
            path_mm: self.path_mm,
            width_mm: self.stone_width_mm,
            host: Some(&self.base_material),
        }
    }

    /// Whether the pattern differs from the stored one.
    pub(super) fn dirty(&self) -> bool {
        self.pattern != self.baseline
    }

    /// The zone the property panel shows, kept inside the pattern.
    pub(super) fn selected(&self) -> usize {
        self.selected_zone.min(self.pattern.zones.len())
    }

    /// The axis menu's current entry: the Custom one while the cutter chose it (or the axis is
    /// no preset), else the preset the axis equals, or "Imported".
    pub(super) fn axis_menu_index(&self) -> i32 {
        let index = axis_index(&self.pattern.axis);
        if self.custom_axis && index != AXIS_IMPORTED {
            AXIS_CUSTOM
        } else {
            index
        }
    }

    /// The sentence under the property panel; empty when the pattern can be applied and saved.
    pub(super) fn message(&self) -> String {
        if !self.pattern.unsolved_colors().is_empty() {
            return if self.solving {
                "Solving colors. This takes a moment.".to_owned()
            } else {
                PatternError::NeedsColor {
                    zone: String::new(),
                }
                .to_string()
            };
        }
        if let Err(error) = self.pattern.to_zoned() {
            return error.to_string();
        }
        self.rank_message()
    }

    /// The sentence for a zone whose color does not fit the material, or an empty one. A one-color
    /// zone on a pleochroic material is the legacy case: it renders as saved and is not a problem.
    fn rank_message(&self) -> String {
        let fits = |color| zone_rank_fit(color, &self.base_material, self.rank);
        for zone in std::iter::once(&self.pattern.base).chain(&self.pattern.zones) {
            match fits(&zone.color) {
                ZoneRankFit::Fits | ZoneRankFit::Legacy => {}
                ZoneRankFit::TooRich => {
                    return format!(
                        "The zone \u{201c}{}\u{201d} has more colors than this material can show. Choose its form again.",
                        zone.name
                    );
                }
                ZoneRankFit::BadVariant(_) => {
                    return format!(
                        "The variant in the zone \u{201c}{}\u{201d} does not fit this material. Choose another one.",
                        zone.name
                    );
                }
            }
        }
        String::new()
    }

    /// Whether Apply and Save may run.
    pub(super) fn can_store(&self) -> bool {
        self.message().is_empty()
    }
}

/// The window, its workers and the handles it needs. One exists once the editor was opened.
pub(super) struct Host {
    /// The window.
    pub window: ColorPatternWindow,
    /// The main window (it owns the dialog the editor was opened from).
    pub main: slint::Weak<MainWindow>,
    /// The vault and the render context.
    pub deps: Deps,
    /// The open session, if any.
    pub session: RefCell<Option<Session>>,
    /// The thread that solves colors.
    pub solver: SolveWorker,
    /// The thread that renders the Current Design preview.
    pub preview: PreviewWorker,
}

thread_local! {
    /// The one editor window of the app session, once it was opened.
    static HOST: RefCell<Option<Rc<Host>>> = const { RefCell::new(None) };
    /// The vault and render context [`super::entry::setup_pattern_callbacks`] was given.
    static DEPS: RefCell<Option<Deps>> = const { RefCell::new(None) };
}

/// Remembers the handles the dialog's buttons open the editor with.
pub(super) fn set_deps(deps: Deps) {
    DEPS.with(|cell| *cell.borrow_mut() = Some(deps));
}

/// The remembered handles, or `None` before the dialog's callbacks were set up.
pub(super) fn deps() -> Option<Deps> {
    DEPS.with(|cell| cell.borrow().clone())
}

/// Forgets the remembered handles (the main window they belonged to is gone).
pub(super) fn clear_deps() {
    let old = DEPS.with(|cell| cell.borrow_mut().take());
    drop(old);
}

/// Stores the new host.
pub(super) fn set_host(host: &Rc<Host>) {
    HOST.with(|cell| *cell.borrow_mut() = Some(Rc::clone(host)));
}

/// Takes the host away (the window is dropped with it).
pub(super) fn take_host() -> Option<Rc<Host>> {
    HOST.with(|cell| cell.borrow_mut().take())
}

/// Runs `f` with the host, or returns `None` when the window was never created.
pub(super) fn with_host<R>(f: impl FnOnce(&Rc<Host>) -> R) -> Option<R> {
    let host = HOST.with(|cell| cell.borrow().clone())?;
    Some(f(&host))
}

/// [`with_host`] for callbacks that return nothing.
pub(super) fn on_host(f: impl FnOnce(&Rc<Host>)) {
    let _ = with_host(f);
}

/// Runs `f` on the open session, or does nothing when no editor is open.
///
/// The session is borrowed for the call only: `f` must not call anything that borrows it again.
pub(super) fn with_session<R>(host: &Host, f: impl FnOnce(&mut Session) -> R) -> Option<R> {
    let mut guard = host.session.try_borrow_mut().ok()?;
    guard.as_mut().map(f)
}
