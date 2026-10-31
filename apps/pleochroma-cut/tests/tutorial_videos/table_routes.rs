//! Filmed routes of the tier table's own controls that the tiers lessons teach: the Quick Add
//! buttons of the bar above the table and the ANGLE cell's key and wheel nudges.
//!
//! A Tier form edit that lands on the same value is a different gesture, so none of these sets a
//! value through the form, and every route requires the visible result before it returns.
//!
//! It also holds the sideways wheel turns of the table itself: the default dock is narrower than a
//! row, so the data columns scroll sideways. The row buttons (Adopt, Pin, Remove, ...) sit in the
//! pinned ACTIONS group at the right edge, which stays on view without scrolling, so the turns
//! are normally zero. Every route that clicks one of those buttons still goes through
//! `Film::reveal_table_button`, which looks first and only turns the table if a button is not
//! yet on view.

use crate::{
    debug_log,
    director::Film,
    support::app::pump,
    targets::{self, Query, Spot},
};
use pleochroma_cut::EditorModel;
use slint::{
    ComponentHandle, LogicalPosition, Model,
    platform::{Key, WindowEvent},
};
use std::time::Duration;

/// The component of the bar above the tier table, which holds the Quick Add buttons.
const TOOLBAR_COMPONENT: &str = "TierTableToolbar";

/// The component of the tier table, which holds every ANGLE cell.
const TABLE_COMPONENT: &str = "EditorTierTable";

/// What one key press or one wheel notch moves an angle by, in degrees.
const NUDGE_DEGREES: f64 = 0.1;

/// The wheel distance of one notch, in Slint's convention (positive scrolls up).
const NOTCH: f32 = 120.0;

/// How far a nudged angle may sit from the angle expected, in degrees.
const ANGLE_TOLERANCE: f64 = 1e-6;

/// How long each key press or wheel notch is held on screen.
const NUDGE_HOLD: Duration = Duration::from_millis(350);

/// How far one sideways wheel turn moves the tier table, in logical pixels.
const SIDEWAYS_TURN: f32 = 120.0;

/// How many sideways turns are made before a button is given up on. The row buttons are pinned in
/// the ACTIONS group and need none; the turns are a fallback for a button that is not on view.
const SIDEWAYS_TURNS: usize = 8;

/// How many vertical turns are made to bring a row that is cut off at the top or bottom of the
/// table's scrolling area whole into it, before the sideways turns.
const VERTICAL_TURNS: usize = 6;

impl Film<'_> {
    /// Safe click/wheel area clipped to the dock/window and clear of overlay scrollbars. Fluent
    /// 1.18.1's fallback is 14px: `widgets/fluent/scrollview.slint` gives each bar a 14px cross
    /// dimension. Keep an extra 2px margin (16px on the vertical edge); actual queried bar bounds
    /// take precedence whenever available.
    pub(crate) fn safe_tier_table_viewport(&self) -> Result<Spot, String> {
        let view = self.view();
        let table = view
            .root
            .query_descendants()
            .match_inherits(TABLE_COMPONENT)
            .find_first()
            .ok_or("the tier table is not visible")?;
        let scroll = table
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .ok_or("the tier table has no ScrollView")?;
        let dock = targets::run(&view, &Query::Id("EditorView::dock_scroll"))
            .ok_or("editor dock is missing")?;
        let viewport = Spot::of(&scroll)
            .clipped_to(&dock, view.size)
            .ok_or("tier table viewport is outside the editor dock")?;
        let bars: Vec<_> = scroll
            .query_descendants()
            .match_inherits("ScrollBar")
            .find_all()
            .into_iter()
            .map(|bar| Spot::of(&bar))
            .filter(|bar| {
                bar.x < viewport.x + viewport.w
                    && bar.x + bar.w > viewport.x
                    && bar.y < viewport.y + viewport.h
                    && bar.y + bar.h > viewport.y
            })
            .collect();
        let horizontal = bars
            .iter()
            .filter(|bar| bar.w > bar.h)
            .max_by(|a, b| a.h.total_cmp(&b.h));
        let vertical = bars
            .iter()
            .filter(|bar| bar.h > bar.w)
            .max_by(|a, b| a.w.total_cmp(&b.w));
        // Room is reserved for a bar only when that bar exists in the tree.
        let right = vertical.map_or(viewport.x + viewport.w, |bar| bar.x - 2.0);
        let bottom = horizontal.map_or(viewport.y + viewport.h, |bar| bar.y - 2.0);
        let safe = Spot {
            x: viewport.x + 2.0,
            y: viewport.y,
            w: (right - viewport.x - 2.0).max(0.0),
            h: (bottom - viewport.y).max(0.0),
            handle: None,
        };
        if ![safe.x, safe.y, safe.w, safe.h]
            .into_iter()
            .all(f32::is_finite)
            || safe.w < 24.0
            || safe.h < 16.0
        {
            return Err(format!(
                "tier table safe viewport is empty or too small ({:.1} x {:.1})",
                safe.w, safe.h
            ));
        }
        Ok(safe)
    }

    /// Resolves the exact accessible `ListItem` row inside the tier table, including when clipped.
    pub(crate) fn tier_row_spot(&self, index: i32) -> Result<Spot, String> {
        let view = self.view();
        let prefix = format!("Tier {},", index + 1);
        let table = view
            .root
            .query_descendants()
            .match_inherits(TABLE_COMPONENT)
            .find_first()
            .ok_or("the tier table is not visible")?;
        let rows: Vec<_> = table
            .query_descendants()
            .match_accessible_role(uiprobe::AccessibleRole::ListItem)
            .find_all()
            .into_iter()
            .filter(|row| {
                row.accessible_label()
                    .is_some_and(|s| s.starts_with(&prefix))
            })
            .collect();
        if rows.len() != 1 {
            return Err(format!(
                "{prefix:?}: expected one visible tier row, found {}",
                rows.len()
            ));
        }
        Ok(Spot::of(&rows[0]))
    }

    /// Resolves a control inside the exact accessible tier row; never matches a neighboring row.
    pub(crate) fn tier_row_control(
        &self,
        index: i32,
        role: uiprobe::AccessibleRole,
        label: &str,
    ) -> Result<Spot, String> {
        let prefix = format!("Tier {},", index + 1);
        let row = self.tier_row_spot(index)?;
        let row = row
            .handle
            .as_ref()
            .ok_or("tier row has no element handle")?;
        let controls: Vec<_> = row
            .query_descendants()
            .match_accessible_role(role)
            .find_all()
            .into_iter()
            .filter(|control| {
                control
                    .accessible_label()
                    .is_some_and(|s| s.starts_with(label))
            })
            .collect();
        if controls.len() != 1 {
            return Err(format!(
                "{prefix:?}: expected one {label:?} control, found {}",
                controls.len()
            ));
        }
        let control = Spot::of(&controls[0]);
        let safe = self.safe_tier_table_viewport()?;
        let (x, y) = control.center();
        let visible_w = (control.x + control.w).min(safe.x + safe.w) - control.x.max(safe.x);
        let visible_h = (control.y + control.h).min(safe.y + safe.h) - control.y.max(safe.y);
        if !(x > safe.x
            && x < safe.x + safe.w
            && y > safe.y
            && y < safe.y + safe.h
            && visible_w >= control.w * 0.75
            && visible_h >= control.h * 0.6)
        {
            return Err(format!(
                "{prefix:?}: {label:?} control center is clipped or under a scrollbar"
            ));
        }
        Ok(control)
    }

    /// Turns the wheel sideways over the tier table, one turn at a time, until `find` names a
    /// button that is on view, and returns that button with the number of turns it took. `find`
    /// is given the scrolling area of the table and picks from what is on screen (usually with
    /// [`Self::tier_row_control`], which checks the button is on view). The table is looked at before the first turn, so a button that is on
    /// view already costs no scrolling. The buttons (Adopt, Pin, Detach, Add Anchor, Move,
    /// Duplicate, Remove) sit in the pinned ACTIONS group at the right edge of the table and are
    /// on view without scrolling, so no turn is normally needed.
    ///
    /// `row` is the tier row the button belongs to. The pinned buttons never scroll sideways, so a
    /// button that is not on view belongs to a row that is cut off at the top or bottom of the
    /// table's scrolling area (a row half under the sideways bar, say): the wheel is then turned
    /// vertically, by about the cut-off distance, until the row is whole, before any sideways turn.
    pub(crate) fn reveal_table_button(
        &mut self,
        row: i32,
        find: impl Fn(&Self, &Spot) -> Option<Spot>,
    ) -> Result<(Spot, usize), String> {
        let mut turns = 0;
        let mut vertical_turns = 0;
        loop {
            let viewport = self.safe_tier_table_viewport()?;
            if let Some(button) = find(self, &viewport) {
                return Ok((button, turns));
            }
            if vertical_turns < VERTICAL_TURNS
                && let Ok(spot) = self.tier_row_spot(row)
            {
                let above = viewport.y - spot.y;
                let below = (spot.y + spot.h) - (viewport.y + viewport.h);
                if above > 0.5 || below > 0.5 {
                    // Positive turns the content down (a row cut off above comes in), negative up.
                    let delta = if above > 0.5 {
                        (above + 4.0).clamp(8.0, 80.0)
                    } else {
                        -(below + 4.0).clamp(8.0, 80.0)
                    };
                    self.wheel_over(&viewport, delta)?;
                    pump(40);
                    vertical_turns += 1;
                    continue;
                }
            }
            if turns == SIDEWAYS_TURNS {
                return Err(format!(
                    "the button is not on view in the tier table after {SIDEWAYS_TURNS} \
                     sideways turns of the wheel"
                ));
            }
            self.wheel_across(&viewport, -SIDEWAYS_TURN)?;
            pump(40);
            turns += 1;
        }
    }

    /// Turns the tier table back by `turns` sideways turns ([`Self::reveal_table_button`] reports
    /// them), so that a later route finds its first columns where it left them.
    pub(crate) fn unturn_table(&mut self, turns: usize) -> Result<(), String> {
        if turns == 0 {
            return Ok(());
        }
        for _ in 0..turns {
            let viewport = self.safe_tier_table_viewport()?;
            self.wheel_across(&viewport, SIDEWAYS_TURN)?;
        }
        pump(40);
        Ok(())
    }

    /// Films clicking the Quick Add button `label` (Girdle, Table or Culet) of the bar above the
    /// tier table, and requires exactly one new tier of that name.
    pub(super) fn quick_add_tier(&mut self, label: &str) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        let before = editor.get_tiers().row_count();
        let button = self.scoped_button(TOOLBAR_COMPONENT, label)?;
        self.click_and_invoke(&button)?;
        pump(80);
        let tiers = editor.get_tiers();
        let named = tiers
            .iter()
            .filter(|row| row.name.as_str() == label)
            .count();
        if tiers.row_count() != before + 1 || named != 1 {
            return Err(format!(
                "Quick Add {label}: expected one new tier called {label:?}, the table went from \
                 {before} to {} rows with {named} of that name",
                tiers.row_count()
            ));
        }
        self.hold(Duration::from_millis(150))
    }

    /// Films the lesson's key nudge: double-clicks the ANGLE cell of `tier`, presses Up `presses`
    /// times (0.1 degrees each) and Enter, and requires the angle to have moved that far.
    pub(super) fn nudge_angle_with_keys(&mut self, tier: &str, presses: u32) -> Result<(), String> {
        let input = self.open_angle_editor(tier)?;
        let row = self.ui.global::<EditorModel>().get_selected_tier_index();
        let before = self.angle_of_row(row)?;
        self.glide_to(input.center())?;
        self.rec.show_keys(&format!("Up × {presses}"));
        debug_log::keys(&format!("Up x {presses}"));
        let window = self.ui.window();
        for _ in 0..presses {
            window.dispatch_event(WindowEvent::KeyPressed {
                text: Key::UpArrow.into(),
            });
            window.dispatch_event(WindowEvent::KeyReleased {
                text: Key::UpArrow.into(),
            });
        }
        pump(80);
        self.rec.hold(NUDGE_HOLD)?;
        self.key("Return")?;
        pump(80);
        self.require_nudged(tier, row, before, f64::from(presses))
    }

    /// Films the lesson's wheel nudge: the cursor goes to the ANGLE cell of `tier`, Ctrl is held
    /// and the wheel turns `notches` times (up adds 0.1 degrees a notch, down takes it away).
    pub(super) fn nudge_angle_with_wheel(
        &mut self,
        tier: &str,
        notches: i32,
    ) -> Result<(), String> {
        let row = self.film_select_tier(tier)?;
        let before = self.angle_of_row(row)?;
        let cell = self.angle_cell_of_row(row)?;
        let (x, y) = cell.center();
        let position = LogicalPosition::new(x, y);
        self.glide_to((x, y))?;
        let window = self.ui.window();
        window.dispatch_event(WindowEvent::PointerMoved { position });
        debug_log::pointer(
            "wheel",
            window,
            (x, y),
            (0.0, 0.0),
            &format!("Ctrl + {notches} notches"),
        );
        let direction = if notches > 0 {
            "Wheel up"
        } else {
            "Wheel down"
        };
        self.rec
            .show_keys(&format!("Ctrl + {direction} × {}", notches.unsigned_abs()));
        window.dispatch_event(WindowEvent::KeyPressed {
            text: Key::Control.into(),
        });
        let delta_y = if notches > 0 { NOTCH } else { -NOTCH };
        for _ in 0..notches.unsigned_abs() {
            window.dispatch_event(WindowEvent::PointerScrolled {
                position,
                delta_x: 0.0,
                delta_y,
            });
        }
        window.dispatch_event(WindowEvent::KeyReleased {
            text: Key::Control.into(),
        });
        pump(80);
        self.rec.hold(NUDGE_HOLD)?;
        self.require_nudged(tier, row, before, f64::from(notches))
    }

    /// Exercises the tutorial's Undo and Redo after a visible angle adjustment, checking both the
    /// row model and its accessible ANGLE label after each real shortcut.
    pub(super) fn undo_redo_angle_nudges(
        &mut self,
        tier: &str,
        before: f64,
        after: f64,
    ) -> Result<(), String> {
        let row = self.ui.global::<EditorModel>().get_selected_tier_index();
        let readback = |film: &Self, expected: f64, action: &str| -> Result<(), String> {
            let model = film.angle_of_row(row)?;
            let spot = film.angle_cell_of_row(row)?;
            let label = spot
                .handle
                .as_ref()
                .and_then(uiprobe::ElementHandle::accessible_label)
                .ok_or("ANGLE cell has no accessible value readback")?;
            let shown = label
                .strip_prefix("Angle ")
                .and_then(|s| s.split(" degrees").next())
                .and_then(|s| s.parse::<f64>().ok())
                .ok_or_else(|| format!("ANGLE label is not a displayed number: {label:?}"))?;
            if (model - expected).abs() > ANGLE_TOLERANCE || (shown - expected).abs() > 0.0051 {
                return Err(format!(
                    "{tier}: {action} expected {expected:.2}, model reads {model}, ANGLE label reads {shown:.2}"
                ));
            }
            Ok(())
        };
        readback(self, after, "before Undo")?;
        self.key("Ctrl+Z")?;
        pump(80);
        self.rec.hold(NUDGE_HOLD)?;
        readback(self, before, "Undo")?;
        self.key("Ctrl+Y")?;
        pump(80);
        self.rec.hold(NUDGE_HOLD)?;
        readback(self, after, "Redo")
    }

    /// Fails unless the angle of table row `row` moved by `steps` nudges from `before`.
    fn require_nudged(&self, tier: &str, row: i32, before: f64, steps: f64) -> Result<(), String> {
        let after = self.angle_of_row(row)?;
        let expected = steps.mul_add(NUDGE_DEGREES, before);
        if (after - expected).abs() > ANGLE_TOLERANCE {
            return Err(format!(
                "{tier}: the angle reads {after} after {steps} nudge(s) from {before}, expected {expected}"
            ));
        }
        Ok(())
    }

    /// The angle of table row `row`, unrounded.
    fn angle_of_row(&self, row: i32) -> Result<f64, String> {
        let at = usize::try_from(row).map_err(|_| format!("table row {row} does not exist"))?;
        self.ui
            .global::<EditorModel>()
            .get_tiers()
            .row_data(at)
            .ok_or_else(|| format!("table row {row} disappeared"))?
            .angle_full
            .parse::<f64>()
            .map_err(|_| format!("the angle of table row {row} is not a number"))
    }

    /// The button that shows the angle of table row `row` and opens its editor, inside the tier
    /// table and level with the row.
    fn angle_cell_of_row(&self, row: i32) -> Result<Spot, String> {
        self.tier_row_control(row, uiprobe::AccessibleRole::Button, "Angle ")
    }
}
