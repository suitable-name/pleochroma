//! The state of the open Color Pattern editor: one [`Session`] at a time, inside the [`Host`] that
//! owns the window, the two background workers and the handles to the vault and the render
//! context.
//!
//! The host lives in a thread-local, like the Rough Color wizard's: Slint callbacks run on the UI
//! thread and reach it through [`with_host`]. No `RefCell` borrow is held across a call that can
//! run another callback: handlers read what they need, drop the borrow, then act.

use super::{
    colors::{SolveCache, SolveWorker, SwatchStone, fill_from_cache},
    cross_section::Handle,
    model::{AXIS_CUSTOM, AXIS_IMPORTED, axis_index},
    order::{Hidden, Remap},
    picker::PickerState,
    preview::{OutlineKey, OutlineMm, PreviewWorker},
    stone::StonePatternApply,
    undo::{EditKey, UndoStack},
};
use crate::{ColorPatternWindow, MainWindow, bridge::render_thread::RenderContext};
use pleochroma::optics::materials::{
    GemMaterial,
    variants::{ColorVariant, TensorRank},
};
use pleochroma_cut_core::material::pattern::{
    ColorPattern, ColorRef, OwnColor, PatternAxis, PatternError, ZoneRankFit, zone_rank_fit,
};
use pleochroma_editor::lch_color::reference_path_mm;
use pleochroma_vault::db::sqlite::Database;
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Instant,
};

/// The sentence of the render status chip's tooltip: where a pattern shows.
pub(super) const PREVIEW_NOTE: &str =
    "Patterns show in Path-Traced views. Ray-Traced shows the base color.";

/// `ColorPatternModel.view`: the Current Design, Path-Traced.
pub(super) const VIEW_STONE: i32 = 0;
/// `ColorPatternModel.view`: the flat cut through the stone, with handles.
pub(super) const VIEW_SLICE: i32 = 1;
/// `ColorPatternModel.view`: the rod of stacked cuts.
pub(super) const VIEW_CRYSTAL: i32 = 2;

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
    /// The color the body-color editor was opened for, read back by `pick_seed` and `picked_color`;
    /// this window's own picker is [`Self::picker`].
    pub pending_pick: Option<ColorRef>,
    /// The window's color picker, while it is open.
    pub picker: Option<PickerState>,
    /// `VIEW_STONE`, `VIEW_SLICE` or `VIEW_CRYSTAL`.
    pub view: i32,
    /// The Cross-Section's position along the axis, 0 to 1.
    pub slice_pos: f32,
    /// The presets the cutter saved.
    pub saved_presets: Vec<(String, ColorPattern)>,
    /// A sentence about where the pattern came from (zones of the Rough Color wizard).
    pub import_note: Option<String>,
    /// A sentence about why the Current Design render failed.
    pub render_note: Option<String>,
    /// The "Discard the changes?" bar is open.
    pub confirm_discard: bool,
    /// The preset name field is open.
    pub naming_preset: bool,
    /// The "Start From..." gallery is open.
    pub gallery_open: bool,
    /// The handles over the Cross-Section as last drawn.
    pub handles: Vec<Handle>,
    /// The handle being dragged.
    pub drag: Option<Drag>,
    /// The handle under the pointer.
    pub hover: Option<usize>,
    /// The zones the cutter hid with the eye; session-only, the stored pattern never changes.
    pub hidden: Hidden,
    /// The material the Current Design preview renders the pattern in.
    pub base_material: GemMaterial,
    /// The color the stone has without a pattern, per direction: what a typical pattern is built
    /// on (not the current base zone, which a typical pattern makes paler).
    pub own: OwnColor,
    /// The last Current Design render, for the main picture and the inset.
    pub design_image: Option<SharedPixelBuffer<Rgba8Pixel>>,
    /// `ColorPatternModel.render_state`: 0 idle, 1 refining, 2 updating.
    pub render_state: i32,
    /// The stone outline of the slice as the outline worker last delivered it, with what it was
    /// made for. Never worked out on the UI thread: the outline solves the design.
    pub outline: Option<(OutlineKey, OutlineMm)>,
    /// The outline the worker is making.
    pub outline_pending: Option<OutlineKey>,
    /// The picture area the pictures were last drawn for, logical pixels (a resize redraws them).
    pub drawn_area: (f32, f32),
    /// Gallery thumbnails by (tile id, swatch key); a new session starts empty, so they follow the
    /// stone and its material.
    pub thumbs: HashMap<(String, u64), Image>,
    /// The picker's `LCh` planes by whole hue degree.
    pub planes: HashMap<i32, Image>,
    /// The picker's hue strip.
    pub hue_strip: Option<Image>,
}

impl Session {
    /// A session on `pattern` with every other setting at its start value: no design, Slice view,
    /// a 7 mm stone, nothing selected, hidden or open. The window sets what it knows on top.
    pub(super) fn blank(pattern: ColorPattern, base_material: GemMaterial, own: OwnColor) -> Self {
        Self {
            material_name: String::new(),
            stone_apply: None,
            preview_design: None,
            targets_design: false,
            baseline: pattern.clone(),
            pattern,
            undo: UndoStack::new(),
            selected_zone: 0,
            stone_width_mm: 7.0,
            path_mm: reference_path_mm(Some(7.0)),
            rank: TensorRank::Isotropic,
            variants: Vec::new(),
            custom_axis: false,
            cache: SolveCache::new(),
            solving: false,
            pending_pick: None,
            picker: None,
            view: VIEW_SLICE,
            slice_pos: 0.5,
            saved_presets: Vec::new(),
            import_note: None,
            render_note: None,
            confirm_discard: false,
            naming_preset: false,
            gallery_open: false,
            handles: Vec::new(),
            drag: None,
            hover: None,
            hidden: Hidden::default(),
            base_material,
            own,
            design_image: None,
            render_state: 0,
            outline: None,
            outline_pending: None,
            drawn_area: (0.0, 0.0),
            thumbs: HashMap::new(),
            planes: HashMap::new(),
            hue_strip: None,
        }
    }

    /// Replaces the pattern with `next` and selects `selected`, as one undo step (`key` joins a
    /// run of one control's changes); `remap` keeps the hidden zones on their zones. Colors the
    /// solver already knows are filled in. Returns whether anything changed.
    pub(super) fn commit(
        &mut self,
        mut next: ColorPattern,
        selected: usize,
        key: Option<EditKey>,
        remap: Remap,
        now: Instant,
    ) -> bool {
        fill_from_cache(&mut next, &self.cache);
        if next == self.pattern && selected == self.selected() {
            return false;
        }
        let before = std::mem::replace(&mut self.pattern, next);
        if before != self.pattern {
            self.undo.record(&before, key, now);
        }
        self.selected_zone = selected;
        self.hidden.apply(remap);
        self.hidden.retain_within(self.pattern.zones.len());
        self.confirm_discard = false;
        self.hover = None;
        self.pending_pick = None;
        self.picker = None;
        true
    }

    /// The pattern the pictures show: the pattern without the hidden zones, and the stored index
    /// of each of its zones (the base first), so a picture's zone index maps back to the list.
    pub(super) fn visible(&self) -> (ColorPattern, Vec<usize>) {
        let mut pattern = self.pattern.clone();
        let mut kept = vec![0];
        let zones = std::mem::take(&mut pattern.zones);
        for (at, zone) in zones.into_iter().enumerate() {
            let stored = at + 1;
            if !self.hidden.contains(stored) {
                pattern.zones.push(zone);
                kept.push(stored);
            }
        }
        (pattern, kept)
    }

    /// The tooltip of the render status chip: the import note, why a render failed, and where a
    /// pattern shows.
    pub(super) fn render_hint(&self) -> String {
        [
            self.import_note.as_deref(),
            self.render_note.as_deref(),
            Some(PREVIEW_NOTE),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
    }

    /// The axis the outline of the slice depends on, with whether the pattern has zones (a
    /// pattern without zones has no frame of its own).
    pub(super) const fn outline_axis(visible: &ColorPattern) -> (PatternAxis, bool) {
        (visible.axis, !visible.zones.is_empty())
    }

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

    /// Whether Save (and Done) may run.
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
    /// The threads that render the Current Design preview and work out the stone's outline.
    pub preview: PreviewWorker,
    /// Waits a moment after the last change of the picked color before it is solved.
    pub picker_timer: slint::Timer,
    /// Watches the window's size: the `narrow` flag and the pictures follow a resize.
    pub tick: slint::Timer,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::{
        color_pattern::{
            edit::{add_zone, reorder_zone},
            order::{row_to_stored, stored_to_row},
        },
        rough_colour::wizard::zone_rows::ShapeKind,
    };
    use pleochroma_cut_core::material::pattern::{TemplateId, template};

    /// A session on a watermelon with two more zones (three zones besides the base).
    fn session() -> Session {
        let mut pattern = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
        for _ in 0..2 {
            pattern = add_zone(&pattern, ShapeKind::Slab, 7.0).unwrap().0;
        }
        Session::blank(
            pattern,
            GemMaterial::sapphire(),
            OwnColor::single([50.0, 30.0, 100.0]),
        )
    }

    fn names(session: &Session) -> Vec<String> {
        session
            .pattern
            .zones
            .iter()
            .map(|zone| zone.name.clone())
            .collect()
    }

    #[test]
    fn the_selected_row_is_the_mirror_of_the_stored_index() {
        let mut session = session();
        session.selected_zone = 3;
        let zones = session.pattern.zones.len();
        // The top layer is the first row.
        assert_eq!(stored_to_row(session.selected(), zones), 0);
        session.selected_zone = 0;
        assert_eq!(stored_to_row(session.selected(), zones), zones);
        assert_eq!(row_to_stored(1, zones), Some(2));
    }

    #[test]
    fn a_reorder_is_one_undo_step_and_undo_restores_the_order() {
        let mut session = session();
        let before = names(&session);
        // Drag the bottom layer (stored 1) to the top (stored 3) in one commit.
        let (next, selected) = reorder_zone(&session.pattern, 1, 3).unwrap();
        assert!(session.commit(
            next,
            selected,
            None,
            Remap::Move { from: 1, to: 3 },
            Instant::now()
        ));
        assert_ne!(names(&session), before);
        assert_eq!(session.selected(), 3);
        let moved = session.pattern.clone();
        let back = session.undo.undo(&session.pattern).expect("one step");
        assert_eq!(names_of(&back), before);
        // The step was the only one.
        assert!(!session.undo.can_undo());
        // And redo brings the new order back.
        let again = session.undo.redo(&back).expect("redo");
        assert_eq!(again, moved);
    }

    fn names_of(pattern: &ColorPattern) -> Vec<String> {
        pattern.zones.iter().map(|zone| zone.name.clone()).collect()
    }

    #[test]
    fn hiding_a_zone_removes_it_from_the_pictures_but_never_from_the_pattern() {
        let mut session = session();
        let stored = session.pattern.clone();
        session.hidden.toggle(2);
        let (visible, kept) = session.visible();
        assert_eq!(visible.zones.len(), 2);
        assert_eq!(kept, [0, 1, 3]);
        assert_eq!(session.pattern, stored);
        assert!(!session.dirty());
    }

    #[test]
    fn a_hidden_zone_stays_hidden_when_a_zone_moves_past_it() {
        let mut session = session();
        session.hidden.toggle(2);
        let (next, selected) = reorder_zone(&session.pattern, 1, 3).unwrap();
        session.commit(
            next,
            selected,
            None,
            Remap::Move { from: 1, to: 3 },
            Instant::now(),
        );
        // The hidden zone is the one that was at 2; it is at 1 now.
        assert!(session.hidden.contains(1) && !session.hidden.contains(2));
    }

    #[test]
    fn the_render_hint_joins_the_notes() {
        let mut session = session();
        assert_eq!(session.render_hint(), PREVIEW_NOTE);
        session.render_note = Some("The open design does not solve.".to_owned());
        assert!(session.render_hint().starts_with("The open design"));
        assert!(session.render_hint().ends_with(PREVIEW_NOTE));
    }
}
