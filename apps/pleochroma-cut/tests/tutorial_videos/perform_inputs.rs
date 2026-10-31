//! Filmed low-level input primitives shared by the `Perform` routes: clicking a control, typing
//! into a field character-by-character, turning the mouse wheel, and finding an editable input
//! inside the (scrolling) Inspector.

use crate::{
    debug_log::{self, debug_check},
    director::Film,
    pacing,
    support::app::pump,
    targets::{self, Spot},
};
use pleochroma_cut::{EditorModel, GuideModel};
use slint::{ComponentHandle, LogicalPosition, platform::WindowEvent};
use std::time::Duration;

/// Time held between characters when typing into a field.
const KEY_HOLD: Duration = Duration::from_millis(55);
/// Time held after finishing a field or toggling a control before moving to the next one.
pub const FIELD_PAUSE: Duration = Duration::from_millis(200);

/// Time held after one sideways wheel turn, long enough for the content to be seen moving.
const WHEEL_ACROSS_HOLD: Duration = Duration::from_millis(120);

impl Film<'_> {
    /// Focuses the control at `spot` with a real pointer press/release and types `text`
    /// character-by-character into it.
    pub(crate) fn click_and_type(&mut self, spot: &Spot, text: &str) -> Result<(), String> {
        let window = self.ui.window();
        if !spot.usable(self.view().size) {
            return Err("input is outside the visible window".into());
        }
        if let Some(handle) = &spot.handle
            && handle.accessible_enabled() == Some(false)
        {
            return Err(format!("input {:?} is disabled", handle.accessible_label()));
        }
        let (cx, cy) = spot.center();
        debug_log::target(spot);
        self.rec.click_at(window, (cx, cy), (0.0, 0.0))?;
        self.cursor = (cx, cy);
        self.rec.press_keys(window, "Ctrl+A")?;
        self.rec.press_keys(window, "Backspace")?;
        pump(20);
        self.rec.type_text(window, text, KEY_HOLD)?;
        pump(20);
        let handle = spot
            .handle
            .as_ref()
            .ok_or("typed input has no accessible readback handle")?;
        let mut actual = handle.accessible_value().ok_or_else(|| {
            format!(
                "typed input {:?} has no accessible value readback",
                handle.accessible_label()
            )
        })?;
        if actual != text {
            // The model write after the last key can land a pump later; give it a moment
            // before calling it a mismatch.
            pacing::wait_real(Duration::from_millis(400), || {
                handle.accessible_value().is_some_and(|value| value == text)
            });
            actual = handle.accessible_value().unwrap_or_default();
        }
        debug_check("typed input value", &text, &actual);
        if actual != text {
            return Err(format!(
                "typed input {:?} readback mismatch: expected {text:?}, observed {actual:?}; \
                 clicked ({cx:.0}, {cy:.0}), element at [{:.0}, {:.0}, {:.0} x {:.0}] type={:?} \
                 role={:?} enabled={:?}; other inputs holding the text: [{}]; elements at the \
                 click point: [{}]",
                handle.accessible_label(),
                spot.x,
                spot.y,
                spot.w,
                spot.h,
                handle.type_name(),
                handle.accessible_role(),
                handle.accessible_enabled(),
                self.inputs_holding(text),
                self.describe_at((cx, cy)),
            ));
        }
        self.rec.hold(FIELD_PAUSE)
    }

    /// The labels and places of the visible text inputs whose value is exactly `text`: where
    /// typed text went when the field it was meant for stayed empty.
    fn inputs_holding(&self, text: &str) -> String {
        self.view()
            .root
            .query_descendants()
            .match_accessible_role(uiprobe::AccessibleRole::TextInput)
            .find_all()
            .into_iter()
            .filter(|element| {
                element
                    .accessible_value()
                    .is_some_and(|value| value == text)
            })
            .take(5)
            .map(|element| {
                let at = Spot::of(&element);
                format!(
                    "{:?} at [{:.0}, {:.0}, {:.0} x {:.0}]",
                    element.accessible_label(),
                    at.x,
                    at.y,
                    at.w,
                    at.h
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// The smallest few elements whose rectangle holds `point`, with type, role and label: what a
    /// click there can have landed on.
    pub(crate) fn describe_at(&self, point: (f32, f32)) -> String {
        let mut hits: Vec<(f32, String)> = self
            .view()
            .root
            .query_descendants()
            .find_all()
            .into_iter()
            .filter_map(|element| {
                let at = Spot::of(&element);
                (at.w > 0.0
                    && at.h > 0.0
                    && point.0 >= at.x
                    && point.0 <= at.x + at.w
                    && point.1 >= at.y
                    && point.1 <= at.y + at.h)
                    .then(|| {
                        (
                            at.w * at.h,
                            format!(
                                "{:?}/{:?}/{:?}",
                                element.type_name(),
                                element.accessible_role(),
                                element.accessible_label()
                            ),
                        )
                    })
            })
            .collect();
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));
        hits.into_iter()
            .take(6)
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join(" | ")
    }

    /// Role, label and enabled state of the Inspector's controls, for error messages.
    pub(crate) fn inspector_dump(&self) -> String {
        let Some(inspector) = self
            .view()
            .root
            .query_descendants()
            .match_inherits("EditorInspector")
            .find_first()
        else {
            return "the Inspector is not on screen".into();
        };
        inspector
            .query_descendants()
            .find_all()
            .into_iter()
            .filter(|element| {
                matches!(
                    element.accessible_role(),
                    Some(
                        uiprobe::AccessibleRole::Combobox
                            | uiprobe::AccessibleRole::TextInput
                            | uiprobe::AccessibleRole::Button
                            | uiprobe::AccessibleRole::Checkbox
                    )
                )
            })
            .take(40)
            .map(|element| {
                format!(
                    "{:?} {:?}{}",
                    element.accessible_role(),
                    element.accessible_label(),
                    if element.accessible_enabled() == Some(false) {
                        " (disabled)"
                    } else {
                        ""
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Whether the Inspector holds an element labelled exactly `label`: `None` when it does not,
    /// otherwise whether that element is enabled.
    pub(crate) fn inspector_label_state(&self, label: &str) -> Option<bool> {
        let inspector = self
            .view()
            .root
            .query_descendants()
            .match_inherits("EditorInspector")
            .find_first()?;
        inspector
            .query_descendants()
            .find_all()
            .into_iter()
            .find(|element| element.accessible_label().is_some_and(|name| name == label))
            .map(|element| element.accessible_enabled() != Some(false))
    }

    /// Clicks a button of the Inspector's pinned row (Add Tier, Save Tier, Add Concave Tier,
    /// Save Concave Tier). The row sits at the foot of the dock, so the button can be cut off by
    /// the dock's edge: the click goes to the part that is on view, and the wheel turns over the
    /// button (which does not scroll by itself) until enough of it is.
    pub(crate) fn click_pinned_commit(&mut self, spot: &Spot) -> Result<(), String> {
        let mut last = String::new();
        for _ in 0..6 {
            let view = self.view();
            let dock = targets::run(&view, &targets::Query::Id("EditorView::dock_scroll"))
                .ok_or("editor dock scroll area is missing")?;
            let fresh = spot.handle.as_ref().map_or_else(|| spot.clone(), Spot::of);
            let shown = fresh.clipped_to(&dock, view.size);
            if let Some(shown) = &shown
                && shown.w >= fresh.w * 0.8
                && shown.h >= fresh.h * 0.8
            {
                return self.click_and_invoke(shown);
            }
            last = format!(
                "button [{:.0}, {:.0}, {:.0} x {:.0}], dock [{:.0}, {:.0}, {:.0} x {:.0}], window {:?}",
                fresh.x, fresh.y, fresh.w, fresh.h, dock.x, dock.y, dock.w, dock.h, view.size
            );
            // The button lies below the dock: the content must move up (negative wheel).
            let wheel = if fresh.y >= dock.y + dock.h / 2.0 {
                -120.0
            } else {
                120.0
            };
            let target = shown.unwrap_or(dock);
            self.wheel_over(&target, wheel)?;
            pump(60);
        }
        Err(format!(
            "the pinned commit button never came fully into view of the dock: {last}"
        ))
    }

    /// One activation per control: the pointer press/release is the activation. The
    /// accessible handle supplies bounds and disabled-state readback only.
    pub(crate) fn click_and_invoke(&mut self, spot: &Spot) -> Result<(), String> {
        if !spot.usable(self.view().size) {
            return Err("control is outside the visible window".into());
        }
        if let Some(handle) = &spot.handle
            && handle.accessible_enabled() == Some(false)
        {
            let guide = self.ui.global::<GuideModel>();
            return Err(format!(
                "control {:?} is disabled; guide hint={:?}, tier table allowed={}, waiting={:?}",
                handle.accessible_label(),
                guide.get_locked_hint(),
                guide.invoke_allows_tier_table(),
                guide.get_current().waiting
            ));
        }
        let (x, y) = spot.center();
        let window = self.ui.window();
        debug_log::target(spot);
        self.rec.click_at(window, (x, y), (0.0, 0.0))?;
        self.cursor = (x, y);
        pump(60);
        Ok(())
    }

    /// Finds the visible editable input (`LineEdit`) labelled `label`.
    /// When `inspector` is true, searches only the Inspector subtree.
    pub(crate) fn input_spot_in(&self, label: &str, inspector: bool) -> Option<Spot> {
        let view = self.view();
        let window = view.size;
        let matches = if inspector {
            view.root
                .query_descendants()
                .match_inherits("EditorInspector")
                .find_first()?
                .query_descendants()
                .find_all()
        } else {
            view.by_label(label)
        };
        matches
            .into_iter()
            .filter(|e| e.accessible_label().is_some_and(|name| name == label))
            .filter(|e| e.accessible_role() == Some(uiprobe::AccessibleRole::TextInput))
            .map(|e| Spot::of(&e))
            .find(|s| s.usable(window))
    }

    /// Turns the mouse wheel over `spot` the way a learner would: the cursor glides to the centre
    /// of `spot`, then the recorder films the wheel ticks with a short hold after each so
    /// the content visibly scrolls.
    /// `delta_y` is in Slint's wheel convention (positive scrolls the content down towards its
    /// top). Lane scripts call this instead of `ElementHandle::scroll`, which moves nothing the
    /// viewer can see before the content jumps.
    pub(crate) fn wheel_over(&mut self, spot: &Spot, delta_y: f32) -> Result<(), String> {
        let window = self.ui.window();
        let (cx, cy) = spot.center();
        debug_log::target(spot);
        self.rec.scroll_at(window, (cx, cy), delta_y)?;
        self.cursor = (cx, cy);
        pump(20);
        Ok(())
    }

    /// Turns the mouse wheel sideways over `spot`, once: the cursor glides to the centre of
    /// `spot`, then the wheel turns by `delta_x` (negative moves the content to the left, so that
    /// later columns come into view) and a short hold lets the movement show.
    pub(crate) fn wheel_across(&mut self, spot: &Spot, delta_x: f32) -> Result<(), String> {
        let (cx, cy) = spot.center();
        self.glide_to((cx, cy))?;
        debug_log::target(spot);
        debug_log::pointer(
            "wheel",
            self.ui.window(),
            (cx, cy),
            (0.0, 0.0),
            &format!("sideways delta {delta_x}"),
        );
        let position = LogicalPosition::new(cx, cy);
        self.ui
            .window()
            .dispatch_event(WindowEvent::PointerMoved { position });
        self.ui
            .window()
            .dispatch_event(WindowEvent::PointerScrolled {
                position,
                delta_x,
                delta_y: 0.0,
            });
        pump(20);
        self.rec.hold(WHEEL_ACROSS_HOLD)
    }

    /// On a 1080 px window with DESIGN open the dock is taller than its viewport: the Inspector's
    /// header peeks over the lower edge and its form lies below it, where nothing of it can be
    /// seen or reached (the form's `ScrollView` is not on screen at all). A learner folds DESIGN
    /// with one click on its header, which hands the room to the Inspector; so does the route.
    /// Does nothing while the form is on screen or DESIGN is already folded.
    pub(crate) fn fold_design_for_inspector(&mut self) -> Result<(), String> {
        if self.ui.global::<EditorModel>().get_settings_collapsed() {
            return Ok(());
        }
        let view = self.view();
        let Some(inspector) = view
            .root
            .query_descendants()
            .match_inherits("EditorInspector")
            .find_first()
        else {
            return Ok(());
        };
        if inspector
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .is_some()
        {
            return Ok(());
        }
        let Some(dock) = targets::run(&view, &targets::Query::Id("EditorView::dock_scroll")) else {
            return Ok(());
        };
        let inspector = Spot::of(&inspector);
        if inspector.y + inspector.h <= dock.y + dock.h + 1.0 {
            return Ok(());
        }
        let header =
            targets::run_label(&view, "DESIGN").ok_or("the DESIGN section header is missing")?;
        self.click_and_invoke(&header)?;
        pump(120);
        if !self.ui.global::<EditorModel>().get_settings_collapsed() {
            return Err("a click on the DESIGN header did not fold the section".into());
        }
        self.hold(pacing::STAGE_HOLD)
    }

    pub(crate) fn scroll_inspector(&mut self, delta: f32) -> Result<(), String> {
        self.fold_design_for_inspector()?;
        let view = self.view();
        let inspector = view
            .root
            .query_descendants()
            .match_inherits("EditorInspector")
            .find_first()
            .ok_or("Inspector is missing while scrolling its form")?;
        let scroll = inspector
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .ok_or_else(|| {
                let at = Spot::of(&inspector);
                format!(
                    "Inspector form ScrollView is missing (inspector at [{:.0}, {:.0}, {:.0} x {:.0}], DESIGN folded: {}, tab {})",
                    at.x,
                    at.y,
                    at.w,
                    at.h,
                    self.ui.global::<EditorModel>().get_settings_collapsed(),
                    self.ui.global::<EditorModel>().get_inspector_tab()
                )
            })?;
        let dock = targets::run(&view, &targets::Query::Id("EditorView::dock_scroll"))
            .ok_or("editor dock scroll area is missing")?;
        let viewport = Spot::of(&scroll)
            .clipped_to(&dock, view.size)
            .ok_or("inspector scroll area is outside the editor dock")?;
        let scrollbar = scroll
            .query_descendants()
            .match_inherits("ScrollBar")
            .find_all()
            .into_iter()
            .map(|element| Spot::of(&element))
            .filter(|bar| bar.h > bar.w && bar.h >= 2.0)
            .filter_map(|bar| bar.clipped_to(&viewport, view.size))
            .filter(|bar| {
                let edge = (bar.x + bar.w - (viewport.x + viewport.w)).abs();
                edge <= 18.0 && bar.h >= viewport.h * 0.65
            })
            .max_by(|left, right| left.x.total_cmp(&right.x));
        // The Optimize tab grows the inspector by what its content needs, so the inner view is
        // as tall as its content and has nothing to scroll: the editor dock carries the whole
        // tab, and the wheel goes to the dock's own scroll bar.
        let inner_full = Spot::of(&scroll);
        let target = if inner_full.h > viewport.h + 1.0 {
            dock_wheel_target(&dock, view.size)
        } else if let Some(scrollbar) = scrollbar {
            scrollbar
        } else {
            // FormRow's verified 88 px label column is a safe wheel target when the style
            // omits a visible scrollbar. Never wheel over a focused input or slider.
            let label_width = viewport.w.min(88.0);
            if label_width < 4.0 {
                return Err(
                    "Inspector has no visible vertical scrollbar or safe label gutter".into(),
                );
            }
            Spot {
                x: viewport.x + label_width / 2.0 - 1.0,
                y: viewport.y,
                w: 2.0,
                h: viewport.h,
                handle: None,
            }
        };
        self.wheel_over(&target, delta)?;
        pump(100);
        self.rec.hold(std::time::Duration::from_millis(180))?;
        Ok(())
    }

    pub(crate) fn input_spot(&mut self, label: &str) -> Result<Option<Spot>, String> {
        self.inspector_form_control(label, uiprobe::AccessibleRole::TextInput)
    }

    /// Reveals a control inside the Inspector's scrolling form. Pinned commit buttons
    /// are outside this area and must be targeted separately. Both scroll boundaries
    /// matter: an accessible child can have window coordinates while clipped by either.
    #[expect(
        clippy::too_many_lines,
        reason = "one scroll-and-reveal loop step by step"
    )]
    pub(crate) fn inspector_form_control(
        &mut self,
        label: &str,
        role: uiprobe::AccessibleRole,
    ) -> Result<Option<Spot>, String> {
        self.fold_design_for_inspector()?;
        let Some(inspector) = self
            .view()
            .root
            .query_descendants()
            .match_inherits("EditorInspector")
            .find_first()
        else {
            return Ok(None);
        };
        if inspector
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .is_none()
        {
            return Ok(None);
        }
        self.scroll_inspector(3000.0)?;
        let mut saw_control = false;
        let mut last_y: Option<f32> = None;
        for _ in 0..32 {
            let view = self.view();
            let inspector = view
                .root
                .query_descendants()
                .match_inherits("EditorInspector")
                .find_first()
                .ok_or("Inspector disappeared while revealing its form")?;
            let scroll = inspector
                .query_descendants()
                .match_inherits("ScrollView")
                .find_first()
                .ok_or("Inspector form ScrollView disappeared")?;
            let dock = targets::run(&view, &targets::Query::Id("EditorView::dock_scroll"))
                .ok_or("editor dock scroll area is missing")?;
            let viewport = Spot::of(&scroll)
                .clipped_to(&dock, view.size)
                .ok_or("inspector form is outside the editor dock")?;
            let bars: Vec<_> = scroll
                .query_descendants()
                .match_inherits("ScrollBar")
                .find_all()
                .into_iter()
                .map(|element| Spot::of(&element))
                .filter(|bar| bar.w >= 2.0 && bar.h >= 2.0)
                .collect();
            let mut safe_right = viewport.x + viewport.w;
            let mut safe_left = viewport.x;
            let mut safe_bottom = viewport.y + viewport.h;
            let mut safe_top = viewport.y;
            let boundary_bars: Vec<_> = bars
                .iter()
                .filter(|bar| {
                    if bar.h > bar.w {
                        let right_edge = (bar.x + bar.w - (viewport.x + viewport.w)).abs() <= 18.0;
                        let left_edge = (bar.x - viewport.x).abs() <= 18.0;
                        bar.h >= viewport.h * 0.65
                            && bar.y < viewport.y + viewport.h
                            && bar.y + bar.h > viewport.y
                            && (right_edge || left_edge)
                    } else {
                        let bottom_edge = (bar.y + bar.h - (viewport.y + viewport.h)).abs() <= 18.0;
                        let top_edge = (bar.y - viewport.y).abs() <= 18.0;
                        bar.w >= viewport.w * 0.65
                            && bar.x < viewport.x + viewport.w
                            && bar.x + bar.w > viewport.x
                            && (bottom_edge || top_edge)
                    }
                })
                .collect();
            for bar in &boundary_bars {
                if bar.h > bar.w && bar.x < safe_right && bar.x + bar.w > viewport.x {
                    if bar.center().0 >= viewport.center().0 {
                        safe_right = safe_right.min(bar.x);
                    } else {
                        safe_left = safe_left.max(bar.x + bar.w);
                    }
                } else if bar.w >= bar.h && bar.y < safe_bottom && bar.y + bar.h > viewport.y {
                    if bar.center().1 >= viewport.center().1 {
                        safe_bottom = safe_bottom.min(bar.y);
                    } else {
                        safe_top = safe_top.max(bar.y + bar.h);
                    }
                }
            }
            // Fluent 1.18.1 overlays 14 px ScrollBars. The form only scrolls vertically, so a
            // bottom reserve would make the last control unreachable at the scroll end; keep
            // the right reserve only when no vertical bar element was found.
            let has_vertical_bar = bars.iter().any(|bar| bar.h > bar.w);
            if !has_vertical_bar && safe_right == viewport.x + viewport.w {
                safe_right -= 14.0;
            }
            let safe_click = |spot: &Spot| {
                let (x, y) = spot.center();
                x > safe_left
                    && x < safe_right
                    && y > safe_top
                    && y < safe_bottom
                    && !bars.iter().any(|bar| {
                        x >= bar.x && x <= bar.x + bar.w && y >= bar.y && y <= bar.y + bar.h
                    })
            };
            let horizontal_center_is_safe = |spot: &Spot| {
                let (x, y) = spot.center();
                x > safe_left
                    && x < safe_right
                    && !boundary_bars.iter().any(|bar| {
                        bar.h > bar.w
                            && x >= bar.x
                            && x <= bar.x + bar.w
                            && y >= bar.y
                            && y <= bar.y + bar.h
                    })
            };
            // Form controls must use exact labels: a prefix such as "Facet Name" also matches
            // the Meets value field "Facet Names This Facet Meets".
            if let Some(control) = targets::role_label(&view, label, role, Some("EditorInspector"))
            {
                saw_control = true;
                if !horizontal_center_is_safe(&control) {
                    return Err(format!(
                        "Inspector control {label:?} has no safe center point beside its scrollbar"
                    ));
                }
                // A control taller than the visible form (the inspector can shrink to a sliver
                // while the table's Steps/Mirror bar is open) can never fit: its centre in view
                // is then what a user clicks.
                let taller_than_view = control.h >= safe_bottom - safe_top;
                let fits_vertically = (control.y >= safe_top
                    && control.y + control.h <= safe_bottom)
                    || taller_than_view;
                let center_is_safe = safe_click(&control);
                if fits_vertically && center_is_safe {
                    return Ok(Some(control));
                }
                let unmoved = last_y.is_some_and(|y| (y - control.y).abs() <= 0.5);
                let visible_h = (control.y + control.h).min(safe_bottom) - control.y.max(safe_top);
                if unmoved && center_is_safe && visible_h >= control.h * 0.7 {
                    crate::debug_log::lookup(
                        "inspector control at scroll end",
                        &(label, safe_top, safe_bottom),
                        Some(&control),
                    );
                    return Ok(Some(control));
                }
                last_y = Some(control.y);
                let control_center = control.center().1;
                let viewport_center = f32::midpoint(safe_top, safe_bottom);
                let delta = (control_center - viewport_center).clamp(-180.0, 180.0);
                if delta.abs() >= 1.0 {
                    // Slint's wheel convention is opposite the content's motion.
                    self.scroll_inspector(-delta)?;
                    continue;
                }
            }
            // The control may not enter the accessibility tree until it approaches the
            // viewport. Advance by a bounded fraction of the freshly measured viewport.
            let delta = (viewport.h * 0.65).clamp(24.0, 120.0);
            self.scroll_inspector(-delta)?;
        }
        if saw_control {
            Err(format!(
                "Inspector control {label:?} never fit fully inside the form viewport"
            ))
        } else {
            Ok(None)
        }
    }
}

/// Where the wheel goes to scroll the editor dock itself: its own vertical scroll bar (the
/// rightmost tall one inside it), or a thin strip at its right edge when the style draws none.
pub fn dock_wheel_target(dock: &Spot, window: (f32, f32)) -> Spot {
    dock.handle
        .as_ref()
        .and_then(|handle| {
            handle
                .query_descendants()
                .match_inherits("ScrollBar")
                .find_all()
                .into_iter()
                .map(|element| Spot::of(&element))
                .filter(|bar| bar.h > bar.w && bar.h >= dock.h * 0.5)
                .filter_map(|bar| bar.clipped_to(dock, window))
                .max_by(|left, right| (left.x + left.w).total_cmp(&(right.x + right.w)))
        })
        .unwrap_or_else(|| Spot {
            x: dock.x + dock.w - 8.0,
            y: dock.y,
            w: 2.0,
            h: dock.h,
            handle: None,
        })
}
