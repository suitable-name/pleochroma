//! Shared visible UI gestures used by the scripted tutorial routes.
#[path = "history_routes.rs"]
mod history_routes;
use crate::{
    debug_log::{self, debug_check},
    director::Film,
    pacing,
    support::app::pump,
    targets::{self, Query, Resolved, Spot},
};
use pleochroma_cut::{
    EditorModel, GuideModel, ManipulateModel, SolidPreviewModel, SweepModel, VerdictModel,
};
use slint::{
    ComponentHandle, LogicalPosition, Model,
    platform::{PointerEventButton, WindowEvent},
};
use std::time::Duration;

/// Whether `outer` covers `inner` (a pixel of slack for rounding).
fn covers(outer: &Spot, inner: &Spot) -> bool {
    outer.x - 1.0 <= inner.x
        && outer.y - 1.0 <= inner.y
        && outer.x + outer.w + 1.0 >= inner.x + inner.w
        && outer.y + outer.h + 1.0 >= inner.y + inner.h
}

/// Collapses spots that expose one control twice (the same rectangle, or one inside the other,
/// such as a button and its caption) into the outer one.
fn distinct(spots: Vec<Spot>) -> Vec<Spot> {
    let keep: Vec<bool> = spots
        .iter()
        .enumerate()
        .map(|(at, spot)| {
            !spots.iter().enumerate().any(|(other_at, other)| {
                other_at != at && covers(other, spot) && (!covers(spot, other) || other_at < at)
            })
        })
        .collect();
    spots
        .into_iter()
        .zip(keep)
        .filter_map(|(spot, kept)| kept.then_some(spot))
        .collect()
}

/// The one spot of `spots`; zero or several are errors that name `what`.
fn one_of(mut spots: Vec<Spot>, what: &str) -> Result<Spot, String> {
    match spots.len() {
        0 => Err(format!("{what} is not visible")),
        1 => Ok(spots.remove(0)),
        count => {
            let places: Vec<String> = spots
                .iter()
                .map(|spot| format!("({:.0}, {:.0})", spot.x, spot.y))
                .collect();
            Err(format!(
                "{what} is ambiguous: {count} visible controls carry it, at {}",
                places.join(", ")
            ))
        }
    }
}

impl Film<'_> {
    pub(super) fn compare_variants(&mut self) -> Result<(), String> {
        history_routes::compare_variants(self)
    }
    pub(super) fn delete_variant(&mut self, name: &str) -> Result<(), String> {
        history_routes::delete_variant(self, name)
    }
    pub(super) fn save_variant(&mut self, name: &str) -> Result<(), String> {
        history_routes::save_variant(self, name)
    }
    pub(super) fn open_variant(&mut self, name: &str) -> Result<(), String> {
        history_routes::open_variant(self, name)
    }
    pub(super) fn history_jump(&mut self, position: i32) -> Result<(), String> {
        history_routes::history_jump(self, position)
    }

    pub(super) fn key(&mut self, chord: &str) -> Result<(), String> {
        self.rec
            .press_keys(self.ui.window(), chord)
            .map_err(|error| format!("key {chord:?}: {error:?}"))
    }

    /// Presses `chord` `count` times, holding a frame now and then so the movement shows.
    pub(super) fn key_times(&mut self, chord: &str, count: usize) -> Result<(), String> {
        for done in 1..=count {
            self.key(chord)?;
            if done % 16 == 0 {
                self.hold(Duration::from_millis(40))?;
            }
        }
        Ok(())
    }

    /// Returns the visible controls labelled `label` inside `component`.
    pub(super) fn scoped_spots(&self, component: &str, label: &str) -> Vec<Spot> {
        let view = self.view();
        let Some(root) = view
            .root
            .query_descendants()
            .match_inherits(component)
            .find_first()
        else {
            return Vec::new();
        };
        distinct(
            root.query_descendants()
                .find_all()
                .into_iter()
                .filter(|e| e.accessible_label().is_some_and(|text| text == label))
                .map(|e| Spot::of(&e))
                .filter(|spot| spot.usable(view.size))
                .collect(),
        )
    }

    /// Returns the unique control labelled `label` inside `component`.
    pub(crate) fn scoped_button(&self, component: &str, label: &str) -> Result<Spot, String> {
        one_of(
            self.scoped_spots(component, label),
            &format!("{label:?} in {component}"),
        )
    }

    pub(super) fn select_open_combo(&mut self, current: i32, wanted: i32) -> Result<(), String> {
        let key = if wanted < current { "Up" } else { "Down" };
        for _ in 0..(wanted - current).abs() {
            self.key(key)?;
        }
        self.key("Return")
    }

    pub(super) fn viewport(&self) -> Result<Spot, String> {
        targets::run(&self.view(), &Query::Id("SolidViewportView::drag_area"))
            .ok_or_else(|| "Solid viewport pointer surface is missing".into())
    }

    pub(super) fn pointer(&mut self, x: f32, y: f32) -> Result<(), String> {
        self.glide_to((x, y))?;
        self.hold(Duration::from_millis(30))
    }

    pub(crate) fn gesture(
        &mut self,
        start: (f32, f32),
        end: (f32, f32),
        handle: Option<i32>,
        cancel: bool,
        fine: bool,
    ) -> Result<(), String> {
        self.glide_to(start)?;
        let window = self.ui.window();
        if fine {
            window.dispatch_event(WindowEvent::KeyPressed {
                text: slint::platform::Key::Shift.into(),
            });
        }
        window.dispatch_event(WindowEvent::PointerPressed {
            position: LogicalPosition::new(start.0, start.1),
            button: PointerEventButton::Left,
        });
        debug_log::pointer("click", window, start, (0.0, 0.0), "press, drag begins");
        pump(30);
        let mut last = start;
        let gesture_result = (|| {
            if let Some(kind) = handle {
                let model = self.ui.global::<ManipulateModel>();
                if !model.get_dragging() || model.get_hovered_handle() != kind {
                    return Err(format!(
                        "pointer press did not grab handle {kind}: {}",
                        model.get_hint_text()
                    ));
                }
            }
            for i in 1..=16_u8 {
                let fraction = f32::from(i) / 16.0;
                let point = (
                    (end.0 - start.0).mul_add(fraction, start.0),
                    (end.1 - start.1).mul_add(fraction, start.1),
                );
                self.glide_to(point)?;
                last = point;
            }
            if cancel {
                self.key("Escape")?;
            }
            Ok(())
        })();
        debug_log::pointer("click", window, last, (0.0, 0.0), "release, drag ends");
        window.dispatch_event(WindowEvent::PointerReleased {
            position: LogicalPosition::new(last.0, last.1),
            button: PointerEventButton::Left,
        });
        if fine {
            window.dispatch_event(WindowEvent::KeyReleased {
                text: slint::platform::Key::Shift.into(),
            });
        }
        pump(60);
        self.cursor = last;
        gesture_result?;
        self.cursor = end;
        if self.ui.global::<ManipulateModel>().get_dragging() {
            return Err("pointer release left a handle drag active".into());
        }
        Ok(())
    }

    pub(super) fn drag_handle(
        &mut self,
        kind: i32,
        distance: f32,
        cancel: bool,
    ) -> Result<(), String> {
        self.drag_handle_with_modifiers(kind, distance, cancel, false)
    }

    pub(super) fn drag_handle_fine(&mut self, kind: i32, distance: f32) -> Result<(), String> {
        self.drag_handle_with_modifiers(kind, distance, false, true)
    }

    pub(super) fn drag_handle_with_modifiers(
        &mut self,
        kind: i32,
        distance: f32,
        cancel: bool,
        fine: bool,
    ) -> Result<(), String> {
        let model = self.ui.global::<ManipulateModel>();
        // Ready is the handle itself with a usable axis, not just the group flag: the tips are
        // placed by the frame that landed last and can lag the flag by a pump.
        let handle_ready = || {
            let (visible, tip) = match kind {
                0 => (
                    model.get_angle_visible(),
                    (model.get_angle_tip_x(), model.get_angle_tip_y()),
                ),
                1 => (
                    model.get_depth_visible(),
                    (model.get_depth_tip_x(), model.get_depth_tip_y()),
                ),
                2 => (
                    model.get_index_visible(),
                    (model.get_index_tip_x(), model.get_index_tip_y()),
                ),
                _ => return false,
            };
            model.get_handles_visible()
                && visible
                && (tip.0 - model.get_anchor_x()).hypot(tip.1 - model.get_anchor_y()) >= 1.0
        };
        let ready = pacing::wait_real(pacing::STEP_TIMEOUT, handle_ready);
        if !ready {
            let preview = self.ui.global::<SolidPreviewModel>();
            let editor = self.ui.global::<EditorModel>();
            return Err(format!(
                "drag handle {kind} is not ready: handles_visible={}, angle/depth/index visible={}/{}/{}, selected_tier={}, slice_mode={}, provisional={}, dragging={}, view_mode={}, has_solid={}, solving={}, hint={:?}",
                model.get_handles_visible(),
                model.get_angle_visible(),
                model.get_depth_visible(),
                model.get_index_visible(),
                editor.get_selected_tier_index(),
                model.get_slice_mode(),
                model.get_provisional_active(),
                model.get_dragging(),
                preview.get_view_mode(),
                preview.get_has_solid(),
                editor.get_solve_running(),
                model.get_hint_text()
            ));
        }
        if !matches!(kind, 0..=2) {
            return Err("invalid handle kind".into());
        }
        let viewport = self.viewport()?;
        let (x, y) = match kind {
            0 => (model.get_angle_tip_x(), model.get_angle_tip_y()),
            1 => (model.get_depth_tip_x(), model.get_depth_tip_y()),
            2 => (model.get_index_tip_x(), model.get_index_tip_y()),
            _ => return Err("invalid handle kind".into()),
        };
        let dx = x - model.get_anchor_x();
        let dy = y - model.get_anchor_y();
        let length = dx.hypot(dy);
        if length < 1.0 {
            return Err("drag handle has no usable axis".into());
        }
        let diagram = self.ui.global::<SolidPreviewModel>().get_view_mode() == 3;
        let zoom = if diagram { model.get_view_zoom() } else { 1.0 };
        let (pan_x, pan_y) = if diagram {
            let root = self
                .view()
                .root
                .query_descendants()
                .match_inherits("SolidViewportView")
                .find_first()
                .ok_or("Diagram viewport is missing")?;
            let root_pos = root.absolute_position();
            let root_size = root.size();
            let image = root
                .query_descendants()
                .match_inherits("Image")
                .find_all()
                .into_iter()
                .find(|item| {
                    let size = item.size();
                    root_size.width.mul_add(-zoom, size.width).abs() < 1.0
                        && root_size.height.mul_add(-zoom, size.height).abs() < 1.0
                })
                .ok_or("scaled Diagram image is missing for pan readback")?;
            let image_pos = image.absolute_position();
            (image_pos.x - root_pos.x, image_pos.y - root_pos.y)
        } else {
            (0.0, 0.0)
        };
        let start = (
            x.mul_add(zoom, viewport.x) + pan_x,
            y.mul_add(zoom, viewport.y) + pan_y,
        );
        let end = (
            (distance * dx / length).mul_add(zoom, start.0),
            (distance * dy / length).mul_add(zoom, start.1),
        );
        self.gesture(start, end, Some(kind), cancel, fine)
    }

    pub(crate) fn draw_slice(&mut self) -> Result<(), String> {
        let model = self.ui.global::<ManipulateModel>();
        let editor = self.ui.global::<EditorModel>();
        let preview = self.ui.global::<SolidPreviewModel>();
        if !model.get_slice_mode() {
            return Err("Slice is not enabled".into());
        }
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            self.ui.get_active_tab() == 0 && !editor.get_solve_running() && preview.get_has_solid()
        }) {
            return Err(format!(
                "slice viewport was not ready for a line: top_tab={}, view_mode={}, has_solid={}, solving={}, hint={:?}",
                self.ui.get_active_tab(),
                preview.get_view_mode(),
                preview.get_has_solid(),
                editor.get_solve_running(),
                model.get_hint_text()
            ));
        }
        let before = editor.get_tiers().row_count();
        let viewport = self.viewport()?;
        self.gesture(
            (
                viewport.w.mul_add(0.28, viewport.x),
                viewport.h.mul_add(0.40, viewport.y),
            ),
            (
                viewport.w.mul_add(0.72, viewport.x),
                viewport.h.mul_add(0.40, viewport.y),
            ),
            None,
            false,
            false,
        )?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            model.get_provisional_active()
                && model.get_handles_visible()
                && model.get_depth_visible()
                && editor.get_tiers().row_count() == before
        }) {
            return Err(format!(
                "slice line did not produce a ready uncommitted provisional facet: provisional={}, handles_visible={}, depth_visible={}, rows_before={}, rows_after={}, view_mode={}, cutoff={}, has_solid={}, hint={:?}",
                model.get_provisional_active(),
                model.get_handles_visible(),
                model.get_depth_visible(),
                before,
                editor.get_tiers().row_count(),
                preview.get_view_mode(),
                preview.get_tier_cutoff(),
                preview.get_has_solid(),
                model.get_hint_text()
            ));
        }
        Ok(())
    }

    pub(super) fn keep_slice(&mut self) -> Result<(), String> {
        let model = self.ui.global::<ManipulateModel>();
        if !model.get_provisional_active() {
            return Err("Keep requires a real provisional slice".into());
        }
        let editor = self.ui.global::<EditorModel>();
        let before = editor.get_tiers().row_count();
        let label = format!("Keep {}", model.get_provisional_label());
        self.press_label(label.trim_end())?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            editor.get_tiers().row_count() == before + 1
                && !model.get_provisional_active()
                && !model.get_slice_mode()
        }) {
            return Err(format!(
                "Keep did not commit exactly one slice: {}",
                model.get_hint_text()
            ));
        }
        if editor.get_selected_tier_index() < 0 {
            return Err("Keep did not select the committed tier".into());
        }
        Ok(())
    }

    pub(crate) fn cut_slice_in(&mut self) -> Result<(), String> {
        let model = self.ui.global::<ManipulateModel>();
        if !model.get_provisional_active() {
            return Err("Slice has no provisional facet to cut in".into());
        }
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            model.get_provisional_active()
                && model.get_handles_visible()
                && model.get_depth_visible()
        }) {
            return Err(format!(
                "provisional depth handle did not become ready before inward cut: provisional={}, handles_visible={}, depth_visible={}, hint={:?}",
                model.get_provisional_active(),
                model.get_handles_visible(),
                model.get_depth_visible(),
                model.get_hint_text()
            ));
        }
        self.drag_handle(1, -35.0, false)?;
        let guide = self.ui.global::<GuideModel>();
        let cut = pacing::wait_lapse(&mut self.rec, pacing::STEP_TIMEOUT, || {
            guide.get_step_done() && model.get_provisional_active()
        })?;
        if cut {
            return Ok(());
        }
        Err(format!(
            "inward depth drag did not cut the provisional facet into the stone: guide_done={}, provisional={}, handles_visible={}, depth_visible={}, view_mode={}, cutoff={}, has_solid={}, solving={}, hint={:?}",
            guide.get_step_done(),
            model.get_provisional_active(),
            model.get_handles_visible(),
            model.get_depth_visible(),
            self.ui.global::<SolidPreviewModel>().get_view_mode(),
            self.ui.global::<SolidPreviewModel>().get_tier_cutoff(),
            self.ui.global::<SolidPreviewModel>().get_has_solid(),
            self.ui.global::<EditorModel>().get_solve_running(),
            model.get_hint_text()
        ))
    }

    /// Clicks the unique visible control labelled `label`.
    pub(crate) fn press_label(&mut self, label: &str) -> Result<(), String> {
        let view = self.view();
        let spots = distinct(
            view.by_label(label)
                .iter()
                .map(Spot::of)
                .filter(|spot| spot.usable(view.size))
                .collect(),
        );
        let spot = one_of(spots, &format!("control {label:?}"))?;
        self.click_and_invoke(&spot)
    }

    /// Opens the selected tier's ANGLE cell with the lesson's real double-click gesture.
    #[expect(clippy::too_many_lines, reason = "one filmed route step by step")]
    pub(super) fn open_angle_editor(&mut self, tier: &str) -> Result<Spot, String> {
        let index = self.film_select_tier(tier)?;
        let editor = self.ui.global::<EditorModel>();
        debug_check(
            "selected tier row",
            &index,
            &editor.get_selected_tier_index(),
        );
        if editor.get_selected_tier_index() != index {
            return Err(format!(
                "{tier}: row {index} selection readback failed before opening angle editor"
            ));
        }
        let row_label = format!("Tier {},", index + 1);
        let view = self.view();
        let row = targets::run_label_start(&view, &row_label)
            .ok_or_else(|| format!("{tier}: selected tier row is missing before double-click"))?;
        let row_element = row
            .handle
            .as_ref()
            .ok_or("selected tier row has no handle")?;
        let angle_cells = row_element
            .query_descendants()
            .match_inherits("TierAngleCell")
            .find_all();
        if angle_cells.len() != 1 {
            return Err(format!(
                "{tier}: expected one ANGLE cell in row {index}, found {}",
                angle_cells.len()
            ));
        }
        let angle_cell = Spot::of(&angle_cells[0]);
        let safe = self.safe_tier_table_viewport()?;
        let (cx, cy) = angle_cell.center();
        let visible_w =
            (angle_cell.x + angle_cell.w).min(safe.x + safe.w) - angle_cell.x.max(safe.x);
        let visible_h =
            (angle_cell.y + angle_cell.h).min(safe.y + safe.h) - angle_cell.y.max(safe.y);
        if !(cx > safe.x
            && cx < safe.x + safe.w
            && cy > safe.y
            && cy < safe.y + safe.h
            && visible_w >= angle_cell.w * 0.75
            && visible_h >= angle_cell.h * 0.6)
        {
            return Err(format!(
                "{tier}: ANGLE cell is clipped by the table viewport"
            ));
        }
        let point = (cx, cy);
        self.glide_to(point)?;
        self.rec.hold(pacing::BEFORE_CLICK)?;
        let window = self.ui.window();
        let position = LogicalPosition::new(point.0, point.1);
        debug_log::pointer("click", window, point, (0.0, 0.0), "double click");
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
        pump(20);
        self.rec.hold(Duration::from_millis(80))?;
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
        self.cursor = point;
        self.rec.click()?;
        pump(60);
        let row = targets::run_label_start(&self.view(), &row_label)
            .ok_or_else(|| format!("{tier}: row disappeared after double-click"))?;
        let row = row
            .handle
            .as_ref()
            .ok_or("selected tier row has no handle")?;
        let inputs: Vec<_> = row
            .query_descendants()
            .match_inherits("TierAngleCell")
            .find_all()
            .into_iter()
            .flat_map(|cell| {
                cell.query_descendants()
                    .match_accessible_role(uiprobe::AccessibleRole::TextInput)
                    .find_all()
            })
            .map(|e| Spot::of(&e))
            .filter(|s| s.usable(self.view().size))
            .collect();
        if inputs.len() != 1 {
            return Err(format!(
                "{tier}: double-click exposed {} inline angle editors",
                inputs.len()
            ));
        }
        Ok(inputs[0].clone())
    }

    pub(super) fn inline_angle(&mut self, tier: &str, angle: &str) -> Result<(), String> {
        let input = self.open_angle_editor(tier)?;
        self.click_and_type(&input, angle)?;
        self.rec
            .press_keys(self.ui.window(), "Return")
            .map_err(|error| format!("keyboard input failed: {error:?}"))?;
        pump(80);
        Ok(())
    }

    pub(super) fn pin_solved_depth(&mut self, tier: &str) -> Result<(), String> {
        let index = self.film_select_tier(tier)?;
        let at = usize::try_from(index).map_err(|_| format!("Pin {tier:?}: no row index"))?;
        let editor = self.ui.global::<EditorModel>();
        let ready = pacing::wait_real(pacing::STEP_TIMEOUT, || {
            !editor.get_solve_running()
                && editor
                    .get_tiers()
                    .row_data(at)
                    .is_some_and(|row| !row.strategy_is_uncertain && !row.mast_full.is_empty())
        });
        if !ready {
            return Err(format!(
                "Pin {tier:?}, row {index}: current certain solved mast unavailable"
            ));
        }
        let before = editor
            .get_tiers()
            .row_data(at)
            .ok_or("Pin row disappeared")?;
        // Reveal the Pin button horizontally; its off-screen bounds can overlap another control.
        let (pin, turns) = self
            .reveal_table_button(index, |film, _area| {
                film.tier_row_control(index, uiprobe::AccessibleRole::Button, "Pin")
                    .ok()
            })
            .map_err(|why| format!("Pin {tier:?}, row {index}: {why}"))?;
        self.hold(pacing::RESULT_HOLD)?;
        self.click_and_invoke(&pin)?;
        let after = editor
            .get_tiers()
            .row_data(at)
            .ok_or("Pinned row disappeared")?;
        let mast = before
            .mast_full
            .parse::<f64>()
            .map_err(|_| "Pin mast is not numeric")?;
        let actual = after
            .constraint_text
            .parse::<f64>()
            .map_err(|_| "Pinned constraint is not numeric")?;
        if after.constraint_kind != 2
            || (mast - actual).abs() > 1e-8
            || before.name != after.name
            || before.angle_full != after.angle_full
            || before.indices_full != after.indices_full
        {
            return Err(format!(
                "Pin {tier:?}: did not preserve the row at solved mast {mast}, actual kind={}, value={actual}",
                after.constraint_kind
            ));
        }
        self.unturn_table(turns)
    }

    /// Opens the verdict and clicks `fix`, optionally solving first when the lesson requests it.
    pub(super) fn verdict_fix(&mut self, fix: &str, solve_first: bool) -> Result<(), String> {
        if solve_first {
            self.film_command(pleochroma_editor::guide::CMD_SOLVE)?;
        }
        let editor = self.ui.global::<EditorModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || !editor.get_solve_running()) {
            return Err("verdict fix: current solve did not finish".into());
        }
        // The verdict is worked out on a worker after the solve (and after the previous Fix):
        // until it lands it is marked out of date and every Fix button is dimmed.
        let verdict = self.ui.global::<VerdictModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            !verdict.get_stale() && !verdict.get_busy()
        }) {
            return Err(format!(
                "verdict fix: the verdict stayed out of date (stale={}, busy={})",
                verdict.get_stale(),
                verdict.get_busy()
            ));
        }
        let Resolved::Found(badge) = targets::resolve(&self.view(), "verdict_badge") else {
            return Err("verdict badge is not visible".into());
        };
        self.click_and_invoke(&badge)?;
        self.hold(pacing::MENU_HOLD)?;
        let prefix = format!("{fix} for Reason ");
        let labels: Vec<String> = self
            .view()
            .elements()
            .into_iter()
            .filter_map(|e| e.accessible_label().map(|s| s.to_string()))
            .filter(|s| s.starts_with(&prefix))
            .collect();
        let [label] = labels.as_slice() else {
            return Err(format!(
                "verdict fix {fix:?}: expected one reason-scoped action, found {}",
                labels.len()
            ));
        };
        // The verdict opens as a popup: its controls report popup-local positions, which the
        // popup's own geometry maps into the window (a click at the raw position lands beside
        // the popup and closes it).
        let popup = crate::popup_geometry::PopupGeometry::find(self.ui, "VerdictPopover")?;
        let bounds = popup.bounds();
        crate::debug_log::debug_check_with(
            "verdict popover bounds (window)",
            &"",
            &(bounds.x, bounds.y, bounds.w, bounds.h),
            true,
        );
        let button = popup.control(self.ui, label, uiprobe::AccessibleRole::Button)?;
        debug_check(
            "verdict before the Fix click (stale, busy)",
            &(false, false),
            &(verdict.get_stale(), verdict.get_busy()),
        );
        self.click_and_invoke(&button)?;
        let still_open =
            crate::popup_geometry::PopupGeometry::find(self.ui, "VerdictPopover").is_ok();
        debug_check(
            "verdict popover open after the Fix click",
            &false,
            &still_open,
        );
        let finished = pacing::wait_real(pacing::STEP_TIMEOUT, || !verdict.get_busy());
        // A fix reports what it changed in a toast ("Snapped 1 index position ... Undo (Ctrl+Z)
        // puts it back."), so only the finish is checked; the toast is shown for the record.
        crate::debug_log::debug_check_with(
            "verdict fix finished; toast after it",
            &"finished, with the fix's own toast",
            &(finished, self.ui.get_toast_message().to_string()),
            finished,
        );
        Ok(())
    }

    pub(super) fn open_sweep(&mut self) -> Result<(), String> {
        let sweep = self.ui.global::<SweepModel>();
        if sweep.get_dialog_open() {
            return Ok(());
        }
        // The Edit menu is native and its items cannot be pointed at inside the recorded window;
        // the lesson offers the in-window route: Ctrl+K and the palette row.
        self.film_command("tools.angle_sweep")?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || sweep.get_dialog_open()) {
            return Err("Angle Sweep dialog did not open".into());
        }
        self.hold(pacing::MENU_HOLD)?;
        Ok(())
    }
}
