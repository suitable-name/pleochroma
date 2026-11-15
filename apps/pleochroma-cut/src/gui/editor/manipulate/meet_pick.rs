//! Pick mode: choosing the corner a tier is cut to by clicking it in the Solid viewport.
//!
//! The geometry and every word come from `pleochroma_editor::manipulate::meet_pick`; this module
//! is the desktop wiring around it.
//!
//! # Life of the mode
//!
//! Arming ([`toggled`]) refuses a tier that cannot take a pick, switches the Slice tool off,
//! parks the Cut slider on "before the tier" (the stone the tier is cut from) and hides the drag
//! handles. Every landed frame ([`refresh`]) re-places the corner rings on the frame. A click on a
//! ring ([`click`]) or Enter on the lit ring ([`accept`]) applies the pick through the session as
//! one undo step; every other exit only restores the view ([`leave`]).
//!
//! # What ends the mode
//!
//! Escape or the Cancel button (the model clears `pick_mode`), a tier selection change, the
//! Diagram view, a change of the design from elsewhere (an edit, Undo, Redo), another design
//! replacing this one, the Cut slider moved by hand, and the Slice tool being switched on.
//! [`pending_exit`] decides it from plain values.
//!
//! # Staying cheap
//!
//! The corners of the stone cut before the tier are computed once per (design generation, solved
//! masts generation, tier) and kept in the session ([`CornerCache`]); a landed frame only
//! projects them. While the mode is off, the tier that is selected and already has a pick keeps
//! its own ring on screen, from the same cache.

use super::{CTX, SESSION, Shared, diagram, handles, set_hint, slice, tier_label};
use crate::{
    EditorModel, MainWindow, ManipulateModel, MeetPickRing, SolidPreviewModel,
    gui::{
        editor::{
            callbacks::{pick_margin, pick_pixels_to_logical, resubmit_facet_overlay},
            state::EditorState,
            view::{refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::cut_slider::CutPosition,
    },
};
use pleochroma::geometry::meet_solver::MeetConstraint;
use pleochroma_cut_core::{Design, PickCorner, TierMeetPick};
use pleochroma_editor::{
    manipulate::{
        ScreenPoint,
        meet_pick::{
            PickRing, PickRingKind, RING_HIT_RADIUS_PX, accessible_ring, hint_armed,
            hint_none_visible, hint_ring, hit_ring, join_names, pick_rings, plate_detail,
            step_ring, tier_name, toast_cleared, toast_picked,
        },
    },
    view_model::meet_pick_row::{meet_pick_blocked_text, meet_pick_row},
};
use slint::{ComponentHandle as _, Model as _, ModelRc, VecModel};
use std::{
    collections::BTreeSet,
    rc::Rc,
    sync::{PoisonError, atomic::Ordering},
};

/// How far (pixels) from a ring's centre a facet of its corner is looked for when the corner's
/// facets are outlined.
const OUTLINE_RADIUS_PX: i32 = 3;

/// Why pick mode ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Exit {
    /// Escape, Cancel, the pill, `M` or the command.
    Cancel,
    /// A corner was picked.
    Picked,
    /// Another tier was selected, or the selection was cleared.
    Selection,
    /// The viewport went to the Diagram view.
    Diagram,
    /// The design changed from elsewhere (an edit, Undo, Redo).
    Edited,
    /// Another design replaced this one.
    Replaced,
    /// The Cut slider was moved by hand.
    SliderMoved,
    /// The Slice tool was switched on.
    Slice,
}

impl Exit {
    /// Whether the Cut slider goes back to where it stood before the mode. A new design starts
    /// uncut, and a slider the user moved stays where the user put it.
    const fn restores_cut(self) -> bool {
        !matches!(self, Self::Replaced | Self::SliderMoved)
    }
}

/// What pick mode remembers from the moment it was armed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Armed {
    /// The tier being cut to a corner.
    pub(super) tier: usize,
    /// The design generation at arming; an edit moves it.
    pub(super) generation: u64,
    /// The design epoch at arming; a replacement of the design moves it.
    pub(super) epoch: u64,
    /// The Cut slider value the mode parked the slider on.
    pub(super) parked: i32,
}

/// What the world looks like now, for [`pending_exit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Now {
    /// Whether the viewport shows the Diagram view.
    pub(super) diagram: bool,
    /// The selected tier, if any.
    pub(super) selected: Option<usize>,
    /// The design generation.
    pub(super) generation: u64,
    /// The design epoch.
    pub(super) epoch: u64,
    /// The Cut slider value.
    pub(super) cutoff: i32,
}

/// Why the mode must end now, `None` while it can go on. Checked in the order a user would
/// notice them.
#[must_use]
pub(super) fn pending_exit(armed: &Armed, now: &Now) -> Option<Exit> {
    if now.diagram {
        Some(Exit::Diagram)
    } else if now.selected != Some(armed.tier) {
        Some(Exit::Selection)
    } else if now.epoch != armed.epoch {
        Some(Exit::Replaced)
    } else if now.generation != armed.generation {
        Some(Exit::Edited)
    } else if now.cutoff != armed.parked {
        Some(Exit::SliderMoved)
    } else {
        None
    }
}

/// The Cut slider value that draws the stone `tier` is cut from: every cutting step before the
/// tier's own. `None` when the design has no flat tier there.
#[must_use]
pub(super) fn parked_cutoff(design: &Design, tier: usize) -> Option<i32> {
    let step = slice::tier_step(design, tier)?;
    Some(CutPosition::from_steps(step, design.preview_step_count()).to_model())
}

/// The slider value to put back when `exit` ends the mode: the saved one, unless the exit keeps
/// the slider where it is or somebody moved it off the parked value.
#[must_use]
pub(super) fn restored_cutoff(exit: Exit, current: i32, parked: i32, saved: i32) -> Option<i32> {
    (exit.restores_cut() && current == parked && current != saved).then_some(saved)
}

/// The sentence in the Meet Point row while the mode is armed. `corners` is how many corners
/// the tier can be cut to, once known.
#[must_use]
pub(super) fn armed_note(names: &[String], corners: Option<usize>) -> String {
    let lead = "Click a ringed corner in the view.";
    let Some(count) = corners else {
        return lead.to_owned();
    };
    let plural = if count == 1 { "corner" } else { "corners" };
    match names {
        [] => lead.to_owned(),
        [only] => format!("{lead} {only} has {count} {plural}; the pick says which one."),
        _ => format!(
            "{lead} {} meet in {count} {plural}; the pick says which one.",
            join_names(names)
        ),
    }
}

/// The corners of the stone cut before one tier, valid for one design and one set of solved
/// masts.
struct CornerCache {
    tier: usize,
    design_generation: u64,
    solved_generation: u64,
    corners: Rc<Vec<PickCorner>>,
}

/// An armed pick mode.
struct PickMode {
    armed: Armed,
    /// The Cut slider value before the mode parked it.
    saved_cutoff: i32,
    /// The tier's display name.
    label: String,
    /// The tier names of the design, by tier index (empty for none).
    names: Vec<String>,
    /// The Meets names of the tier, for the row's sentence.
    meet_names: Vec<String>,
    /// How many corners the tier can be cut to, once computed.
    corner_count: Option<usize>,
    /// The rings of the last frame, in keyboard order.
    rings: Vec<PickRing>,
    /// The corner (index into the cached corners) the pointer or keyboard is on.
    lit_corner: Option<usize>,
    /// The facet ids outlined for the lit ring.
    outlined: Vec<u32>,
}

/// The session state of pick mode, a field of the manipulation session.
pub(super) struct PickSession {
    mode: Option<PickMode>,
    cache: Option<CornerCache>,
}

impl PickSession {
    /// No mode, no cache.
    pub(super) const fn new() -> Self {
        Self {
            mode: None,
            cache: None,
        }
    }
}

/// Whether pick mode is armed. The drag handles stay hidden while it is.
pub(super) fn is_armed() -> bool {
    SESSION.with(|cell| cell.borrow().pick.mode.is_some())
}

/// Moves the Cut slider to `cutoff` and asks for the redraw, unless it is there already.
fn set_cutoff(ui: &MainWindow, cutoff: i32) {
    let solid = ui.global::<SolidPreviewModel>();
    if solid.get_tier_cutoff() != cutoff {
        solid.set_tier_cutoff(cutoff);
        solid.invoke_tier_cutoff_changed();
    }
}

/// The wording of why the selected tier cannot be armed, or the data arming needs.
fn arming_inputs(ui: &MainWindow, ctx: &Shared) -> Result<(Armed, PickMode), Option<String>> {
    let selected = usize::try_from(ui.global::<EditorModel>().get_selected_tier_index())
        .map_err(|_| None::<String>)?;
    let st = ctx.state.try_borrow().map_err(|_| None::<String>)?;
    let design = &st.design;
    let flat = design.tiers.get(selected).ok_or(None::<String>)?;
    if let Some(block) = design.meet_pick_blocked(selected) {
        return Err(Some(meet_pick_blocked_text(design, selected, block)));
    }
    let parked = parked_cutoff(design, selected).ok_or(None::<String>)?;
    let saved = ui.global::<SolidPreviewModel>().get_tier_cutoff();
    let armed = Armed {
        tier: selected,
        generation: st.current_generation(),
        epoch: st.design_epoch.load(Ordering::Relaxed),
        parked,
    };
    let meet_names = match &flat.constraint {
        MeetConstraint::MeetNamed(names) => names.clone(),
        _ => Vec::new(),
    };
    Ok((
        armed,
        PickMode {
            armed,
            saved_cutoff: saved,
            label: tier_label(flat, selected),
            names: tier_names(design),
            meet_names,
            corner_count: None,
            rings: Vec::new(),
            lit_corner: None,
            outlined: Vec::new(),
        },
    ))
}

/// The name of every flat tier, empty for a tier without one.
fn tier_names(design: &Design) -> Vec<String> {
    design
        .tiers
        .iter()
        .map(|flat| {
            flat.names()
                .first()
                .map_or_else(String::new, |name| (*name).to_owned())
        })
        .collect()
}

/// `ManipulateModel.pick_toggled`: `pick_mode` already holds its new value.
pub(super) fn toggled(ui: &MainWindow, ctx: &Shared) {
    if ui.global::<ManipulateModel>().get_pick_mode() {
        arm(ui, ctx);
    } else {
        leave(ui, ctx, Exit::Cancel);
    }
}

/// Starts pick mode for the selected tier, or says why not and leaves `pick_mode` off.
fn arm(ui: &MainWindow, ctx: &Shared) {
    let model = ui.global::<ManipulateModel>();
    if is_armed() {
        return;
    }
    if diagram::is_diagram_view(ui) {
        model.set_pick_mode(false);
        return;
    }
    let (armed, mode) = match arming_inputs(ui, ctx) {
        Ok(inputs) => inputs,
        Err(reason) => {
            model.set_pick_mode(false);
            if let Some(reason) = reason {
                show_toast(ui, &reason, "info");
            }
            return;
        }
    };
    // The two tools exclude each other.
    if model.get_slice_mode() {
        model.set_slice_mode(false);
        slice::cancel_gesture(ui);
        slice::discard(ui, ctx, slice::DiscardReason::User);
    }
    set_cutoff(ui, armed.parked);
    SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        session.hover_hint_active = false;
        session.pick.mode = Some(mode);
    });
    model.set_pick_lit(-1);
    handles::refresh_handles(ui, ctx);
    refresh(ui, ctx);
    publish_row_now(ui, ctx);
}

/// Ends pick mode: clears the rings and the outline, puts the Cut slider back when `exit` asks
/// for it and brings the drag handles back. Does nothing but clear `pick_mode` when the mode is
/// not armed.
pub(super) fn leave(ui: &MainWindow, ctx: &Shared, exit: Exit) {
    let model = ui.global::<ManipulateModel>();
    let taken = SESSION.with(|cell| cell.borrow_mut().pick.mode.take());
    model.set_pick_mode(false);
    let Some(mode) = taken else {
        return;
    };
    model.set_pick_lit(-1);
    model.set_pick_rings(ModelRc::new(VecModel::<MeetPickRing>::default()));
    if !mode.outlined.is_empty() {
        resubmit_facet_overlay(&ctx.preview_state, |overlay| overlay.multi_selected.clear());
    }
    let current = ui.global::<SolidPreviewModel>().get_tier_cutoff();
    if let Some(cutoff) = restored_cutoff(exit, current, mode.armed.parked, mode.saved_cutoff) {
        set_cutoff(ui, cutoff);
    }
    handles::refresh_handles(ui, ctx);
    slice::resting_hint(ui);
    refresh(ui, ctx);
    publish_row_now(ui, ctx);
}

/// A tier-list selection change is about to replan the committed design: the mode cannot
/// outlive it.
pub(super) fn leave_for_selection(ui: &MainWindow) {
    let Some(ctx) = CTX.with(|cell| cell.borrow().clone()) else {
        return;
    };
    leave(ui, &ctx, Exit::Selection);
}

/// The corners of the stone cut before `tier`, from the cache or computed once for this design
/// and these masts. Empty when the design cannot say (masts not aligned, no closed stone).
fn corners_for(ctx: &Shared, st: &EditorState, tier: usize) -> Rc<Vec<PickCorner>> {
    let design_generation = st.current_generation();
    let Some((solved_generation, masts_len)) = handles::solved_summary(ctx) else {
        return Rc::default();
    };
    if !handles::masts_aligned(st.design.tiers.len(), masts_len) {
        return Rc::default();
    }
    let cached = SESSION.with(|cell| {
        cell.borrow()
            .pick
            .cache
            .as_ref()
            .filter(|c| {
                c.tier == tier
                    && c.design_generation == design_generation
                    && c.solved_generation == solved_generation
            })
            .map(|c| Rc::clone(&c.corners))
    });
    if let Some(corners) = cached {
        return corners;
    }
    let corners = handles::aligned_masts(ctx, st.design.tiers.len())
        .and_then(|masts| st.design.pick_corners(&masts, tier))
        .map_or_else(Rc::default, Rc::new);
    SESSION.with(|cell| {
        cell.borrow_mut().pick.cache = Some(CornerCache {
            tier,
            design_generation,
            solved_generation,
            corners: Rc::clone(&corners),
        });
    });
    corners
}

/// The rings of `corners` on the frame on screen. `None` without a frame, or on a Diagram frame.
fn project(
    ctx: &Shared,
    corners: &[PickCorner],
    names: &[String],
    current: Option<&TierMeetPick>,
) -> Option<Vec<PickRing>> {
    let geometry = ctx
        .geometry
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()?;
    if geometry.diagram.is_some() {
        return None;
    }
    let pick = ctx.pick.lock().unwrap_or_else(PoisonError::into_inner);
    let owners = ctx
        .facet_owners
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let rings = pick_rings(
        corners,
        &geometry,
        pick.as_ref()?,
        names,
        |id| owners.flat.get(id as usize).copied().flatten(),
        current,
    );
    drop(owners);
    drop(pick);
    Some(rings)
}

/// Pushes `rings` to the model in logical viewport coordinates, with each ring's plate texts.
fn publish_rings(ui: &MainWindow, rings: &[PickRing], tier: &str, lit: Option<usize>) {
    let scale = ui.window().scale_factor();
    let margin = pick_margin(ui);
    let items: Vec<MeetPickRing> = rings
        .iter()
        .map(|ring| {
            let (x, y) = pick_pixels_to_logical((ring.at.x, ring.at.y), scale, margin);
            MeetPickRing {
                x,
                y,
                kind: ring.kind.code(),
                label: ring.plate_title().into(),
                sub: plate_detail(ring, tier).into(),
            }
        })
        .collect();
    let model = ui.global::<ManipulateModel>();
    model.set_pick_rings(ModelRc::new(VecModel::from(items)));
    model.set_pick_lit(lit.and_then(|i| i32::try_from(i).ok()).unwrap_or(-1));
}

/// Re-places the rings for the frame that just landed. Armed: all the corners the tier can be
/// cut to, and the exit checks. Not armed: the ring of the selected tier's own pick, if it has
/// one.
pub(super) fn refresh(ui: &MainWindow, ctx: &Shared) {
    if is_armed() {
        refresh_armed(ui, ctx);
    } else {
        refresh_resting(ui, ctx);
    }
}

/// The armed half of [`refresh`].
fn refresh_armed(ui: &MainWindow, ctx: &Shared) {
    let Some(armed) = SESSION.with(|cell| cell.borrow().pick.mode.as_ref().map(|m| m.armed)) else {
        return;
    };
    let Ok(st) = ctx.state.try_borrow() else {
        return;
    };
    let now = Now {
        diagram: diagram::is_diagram_view(ui),
        selected: usize::try_from(ui.global::<EditorModel>().get_selected_tier_index()).ok(),
        generation: st.current_generation(),
        epoch: st.design_epoch.load(Ordering::Relaxed),
        cutoff: ui.global::<SolidPreviewModel>().get_tier_cutoff(),
    };
    if let Some(exit) = pending_exit(&armed, &now) {
        drop(st);
        leave(ui, ctx, exit);
        return;
    }
    let corners = corners_for(ctx, &st, armed.tier);
    let current = st.design.tier_meet_pick(armed.tier).copied();
    drop(st);
    let names = SESSION.with(|cell| {
        cell.borrow()
            .pick
            .mode
            .as_ref()
            .map(|m| m.names.clone())
            .unwrap_or_default()
    });
    let rings = project(ctx, &corners, &names, current.as_ref()).unwrap_or_default();
    let first_count = SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        let mode = session.pick.mode.as_mut()?;
        mode.rings = rings;
        let first = mode.corner_count.is_none() && !corners.is_empty();
        if !corners.is_empty() {
            mode.corner_count = Some(corners.len());
        }
        Some(first)
    });
    sync_lit(ui, ctx);
    if first_count == Some(true) {
        publish_row_now(ui, ctx);
    }
}

/// The resting half of [`refresh`]: the green ring of the selected tier's pick.
fn refresh_resting(ui: &MainWindow, ctx: &Shared) {
    let model = ui.global::<ManipulateModel>();
    let clear = |ui: &MainWindow| {
        let model = ui.global::<ManipulateModel>();
        if model.get_pick_rings().row_count() > 0 {
            model.set_pick_rings(ModelRc::new(VecModel::<MeetPickRing>::default()));
        }
        model.set_pick_lit(-1);
    };
    if model.get_dragging() || model.get_slice_mode() || diagram::is_diagram_view(ui) {
        clear(ui);
        return;
    }
    let Ok(selected) = usize::try_from(ui.global::<EditorModel>().get_selected_tier_index()) else {
        clear(ui);
        return;
    };
    let Ok(st) = ctx.state.try_borrow() else {
        return;
    };
    let Some(pick) = st.design.tier_meet_pick(selected).copied() else {
        drop(st);
        clear(ui);
        return;
    };
    let Some(flat) = st.design.tiers.get(selected) else {
        drop(st);
        clear(ui);
        return;
    };
    let label = tier_label(flat, selected);
    let names = tier_names(&st.design);
    let corners = corners_for(ctx, &st, selected);
    drop(st);
    let rings: Vec<PickRing> = project(ctx, &corners, &names, Some(&pick))
        .unwrap_or_default()
        .into_iter()
        .filter(|ring| ring.kind == PickRingKind::Current)
        .collect();
    if rings.is_empty() {
        clear(ui);
    } else {
        publish_rings(ui, &rings, &label, None);
    }
}

/// The facet ids of the frame on screen that belong to the corner's tiers and lie at the ring.
fn facets_at(ctx: &Shared, ring: &PickRing) -> Vec<u32> {
    let guard = ctx.pick.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(pick) = guard.as_ref() else {
        return Vec::new();
    };
    let owners = ctx
        .facet_owners
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a pixel coordinate of a frame that fits in memory is far below i32::MAX"
    )]
    let (cx, cy) = (ring.at.x.round() as i32, ring.at.y.round() as i32);
    let mut ids: Vec<u32> = Vec::new();
    for dy in -OUTLINE_RADIUS_PX..=OUTLINE_RADIUS_PX {
        for dx in -OUTLINE_RADIUS_PX..=OUTLINE_RADIUS_PX {
            if dx * dx + dy * dy > OUTLINE_RADIUS_PX * OUTLINE_RADIUS_PX {
                continue;
            }
            let (Ok(x), Ok(y)) = (u32::try_from(cx + dx), u32::try_from(cy + dy)) else {
                continue;
            };
            let Some(id) = pick.facet_at(x, y) else {
                continue;
            };
            let tier = owners.flat.get(id as usize).copied().flatten();
            if tier.is_some_and(|tier| ring.tiers.contains(&tier)) && !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    drop(owners);
    drop(guard);
    ids
}

/// Brings everything that follows the lit ring in line: the model's lit index, the corner's
/// outline and the hint line.
fn sync_lit(ui: &MainWindow, ctx: &Shared) {
    let Some((rings, lit_corner, label, names, outlined)) = SESSION.with(|cell| {
        cell.borrow().pick.mode.as_ref().map(|m| {
            (
                m.rings.clone(),
                m.lit_corner,
                m.label.clone(),
                m.names.clone(),
                m.outlined.clone(),
            )
        })
    }) else {
        return;
    };
    let lit = lit_corner.and_then(|corner| rings.iter().position(|r| r.corner == corner));
    publish_rings(ui, &rings, &label, lit);
    let wanted = lit
        .and_then(|i| rings.get(i))
        .filter(|ring| ring.pickable())
        .map(|ring| facets_at(ctx, ring))
        .unwrap_or_default();
    if wanted != outlined {
        let ids = wanted.clone();
        resubmit_facet_overlay(&ctx.preview_state, move |overlay| {
            overlay.multi_selected = ids;
        });
        SESSION.with(|cell| {
            if let Some(mode) = cell.borrow_mut().pick.mode.as_mut() {
                mode.outlined = wanted;
            }
        });
    }
    let text = match lit.and_then(|i| rings.get(i).map(|ring| (i, ring))) {
        Some((i, ring)) => format!(
            "{} {}",
            hint_ring(ring, &label, &names),
            accessible_ring(i, rings.len(), ring)
        ),
        None if rings.is_empty() => hint_none_visible(&label),
        None => hint_armed(&label),
    };
    set_hint(ui, &text);
}

/// The ring under the logical pointer `(x, y)`, as an index into the mode's rings.
fn ring_at(ui: &MainWindow, x: f32, y: f32) -> Option<usize> {
    let pointer = handles::pointer_to_pick(ui, x, y);
    let radius = RING_HIT_RADIUS_PX
        * ui.window().scale_factor()
        * ui.global::<ManipulateModel>().get_handle_scale();
    SESSION.with(|cell| {
        let session = cell.borrow();
        let mode = session.pick.mode.as_ref()?;
        hit_ring(&mode.rings, ScreenPoint::new(pointer.x, pointer.y), radius)
    })
}

/// Lights the ring at `index` (or none) and refreshes what follows it.
fn set_lit(ui: &MainWindow, ctx: &Shared, index: Option<usize>) {
    let changed = SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        let mode = session.pick.mode.as_mut()?;
        let corner = index
            .and_then(|i| mode.rings.get(i))
            .map(|ring| ring.corner);
        let changed = mode.lit_corner != corner;
        mode.lit_corner = corner;
        Some(changed)
    });
    if changed == Some(true) {
        sync_lit(ui, ctx);
    }
}

/// `ManipulateModel.pick_hover`: the pointer moved over the viewport in pick mode.
pub(super) fn hover(ui: &MainWindow, ctx: &Shared, x: f32, y: f32) {
    if is_armed() {
        set_lit(ui, ctx, ring_at(ui, x, y));
    }
}

/// `ManipulateModel.pick_click`: a left release that moved little. Cuts the tier to the ring
/// under the pointer; a miss changes nothing.
pub(super) fn click(ui: &MainWindow, ctx: &Shared, x: f32, y: f32) {
    if !is_armed() {
        return;
    }
    if let Some(index) = ring_at(ui, x, y) {
        accept_ring(ui, ctx, index);
    }
}

/// `ManipulateModel.pick_step`: the arrow keys walk the rings.
pub(super) fn step(ui: &MainWindow, ctx: &Shared, delta: i32) {
    let next = SESSION.with(|cell| {
        let session = cell.borrow();
        let mode = session.pick.mode.as_ref()?;
        let lit = mode
            .lit_corner
            .and_then(|corner| mode.rings.iter().position(|r| r.corner == corner));
        step_ring(&mode.rings, lit, delta)
    });
    if next.is_some() {
        set_lit(ui, ctx, next);
    }
}

/// `ManipulateModel.pick_accept`: Enter picks the lit ring.
pub(super) fn accept(ui: &MainWindow, ctx: &Shared) {
    let lit = SESSION.with(|cell| {
        let session = cell.borrow();
        let mode = session.pick.mode.as_ref()?;
        mode.lit_corner
            .and_then(|corner| mode.rings.iter().position(|r| r.corner == corner))
    });
    if let Some(index) = lit {
        accept_ring(ui, ctx, index);
    }
}

/// Cuts the armed tier to the ring at `index`: one session edit, then the view goes back, the
/// panel and the picture follow the new design and a toast says what happened.
fn accept_ring(ui: &MainWindow, ctx: &Shared, index: usize) {
    let picked = SESSION.with(|cell| {
        let session = cell.borrow();
        let mode = session.pick.mode.as_ref()?;
        let ring = mode.rings.get(index)?;
        ring.pickable().then(|| {
            (
                mode.armed,
                ring.corner,
                ring.names.clone(),
                mode.label.clone(),
            )
        })
    });
    let Some((armed, corner_index, names, label)) = picked else {
        return;
    };
    let corner = SESSION.with(|cell| {
        cell.borrow()
            .pick
            .cache
            .as_ref()
            .filter(|c| c.tier == armed.tier)
            .and_then(|c| c.corners.get(corner_index).cloned())
    });
    let Some(corner) = corner else {
        return;
    };
    let outcome = {
        let Ok(mut st) = ctx.state.try_borrow_mut() else {
            return;
        };
        if st.current_generation() == armed.generation {
            st.pick_meet_corner(armed.tier, &corner)
                .map(|_| ())
                .map_err(|error| Some(error.to_string()))
        } else {
            Err(None)
        }
    };
    match outcome {
        Ok(()) => {}
        Err(None) => {
            leave(ui, ctx, Exit::Edited);
            return;
        }
        Err(Some(reason)) => {
            show_toast(ui, &reason, "error");
            return;
        }
    }
    // The slider goes back before the replan below, so the picture is of the restored cut.
    leave(ui, ctx, Exit::Picked);
    if let Ok(st) = ctx.state.try_borrow() {
        refresh_editor_panel_stale(ui, &ctx.render_ctx, &st, &BTreeSet::new());
        submit_preview_replan(
            ui,
            &ctx.render_ctx,
            &ctx.preview_state,
            &ctx.solid_last_solved,
            &st,
            BTreeSet::new(),
            false,
        );
    }
    show_toast(ui, &toast_picked(&label, &names), "info");
}

/// `EditorModel.meet_pick_clear`: forgets the picked corner of tier `index` as one undo step.
fn clear_pick(ui: &MainWindow, ctx: &Shared, index: usize) {
    let (outcome, label) = {
        let Ok(mut st) = ctx.state.try_borrow_mut() else {
            return;
        };
        let label = st
            .design
            .tiers
            .get(index)
            .map_or_else(|| tier_name(&[], index), |flat| tier_label(flat, index));
        (st.clear_meet_pick(index), label)
    };
    match outcome {
        Ok(Some(_)) => {
            if let Ok(st) = ctx.state.try_borrow() {
                refresh_editor_panel_stale(ui, &ctx.render_ctx, &st, &BTreeSet::new());
                submit_preview_replan(
                    ui,
                    &ctx.render_ctx,
                    &ctx.preview_state,
                    &ctx.solid_last_solved,
                    &st,
                    BTreeSet::new(),
                    false,
                );
            }
            show_toast(ui, &toast_cleared(&label), "info");
        }
        Ok(None) => {}
        Err(error) => show_toast(ui, &error.to_string(), "error"),
    }
}

/// Pushes the Meet Point row and the pill's availability for the selected tier of `st`.
///
/// The row's note is the armed sentence while pick mode is armed for that tier. Called with the
/// other selection-fed fields (`view::push_selected_tier_chips`) and whenever pick mode changes.
pub(in crate::gui::editor) fn publish_row(ui: &MainWindow, st: &EditorState) {
    let editor = ui.global::<EditorModel>();
    let manipulate = ui.global::<ManipulateModel>();
    let selected = usize::try_from(editor.get_selected_tier_index())
        .ok()
        .filter(|&index| index < st.design.tiers.len());
    let Some(index) = selected else {
        editor.set_meet_pick_state(0);
        editor.set_meet_pick_chip("".into());
        editor.set_meet_pick_note("".into());
        manipulate.set_pick_available(false);
        manipulate.set_pick_blocked_reason("".into());
        return;
    };
    let solved = CTX
        .with(|cell| cell.borrow().clone())
        .and_then(|ctx| handles::aligned_masts(&ctx, st.design.tiers.len()))
        .unwrap_or_default();
    let row = meet_pick_row(&st.design, &solved, index);
    let armed_note = SESSION.with(|cell| {
        let session = cell.borrow();
        session
            .pick
            .mode
            .as_ref()
            .filter(|mode| mode.armed.tier == index)
            .map(|mode| armed_note(&mode.meet_names, mode.corner_count))
    });
    editor.set_meet_pick_state(row.state.code());
    editor.set_meet_pick_chip(row.chip.into());
    editor.set_meet_pick_note(armed_note.unwrap_or(row.note).into());
    let blocked = st.design.meet_pick_blocked(index);
    manipulate.set_pick_available(blocked.is_none());
    manipulate.set_pick_blocked_reason(
        blocked
            .map(|block| meet_pick_blocked_text(&st.design, index, block))
            .unwrap_or_default()
            .into(),
    );
}

/// [`publish_row`] for a caller that holds no editor state borrow.
fn publish_row_now(ui: &MainWindow, ctx: &Shared) {
    if let Ok(st) = ctx.state.try_borrow() {
        publish_row(ui, &st);
    }
}

/// Wires the pick-mode callbacks of `ManipulateModel` and the row's Clear button.
pub(super) fn setup(ui: &MainWindow, ctx: &Shared) {
    let manipulate = ui.global::<ManipulateModel>();
    let ui_weak = ui.as_weak();
    let toggle_ctx = ctx.clone();
    manipulate.on_pick_toggled(move || {
        if let Some(ui) = ui_weak.upgrade() {
            toggled(&ui, &toggle_ctx);
        }
    });
    let ui_weak = ui.as_weak();
    let hover_ctx = ctx.clone();
    manipulate.on_pick_hover(move |x: f32, y: f32| {
        if let Some(ui) = ui_weak.upgrade() {
            hover(&ui, &hover_ctx, x, y);
        }
    });
    let ui_weak = ui.as_weak();
    let click_ctx = ctx.clone();
    manipulate.on_pick_click(move |x: f32, y: f32| {
        if let Some(ui) = ui_weak.upgrade() {
            click(&ui, &click_ctx, x, y);
        }
    });
    let ui_weak = ui.as_weak();
    let step_ctx = ctx.clone();
    manipulate.on_pick_step(move |delta: i32| {
        if let Some(ui) = ui_weak.upgrade() {
            step(&ui, &step_ctx, delta);
        }
    });
    let ui_weak = ui.as_weak();
    let accept_ctx = ctx.clone();
    manipulate.on_pick_accept(move || {
        if let Some(ui) = ui_weak.upgrade() {
            accept(&ui, &accept_ctx);
        }
    });
    let ui_weak = ui.as_weak();
    let clear_ctx = ctx.clone();
    ui.global::<EditorModel>()
        .on_meet_pick_clear(move |index: i32| {
            if let (Some(ui), Ok(index)) = (ui_weak.upgrade(), usize::try_from(index)) {
                clear_pick(&ui, &clear_ctx, index);
            }
        });
}
