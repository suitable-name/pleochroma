//! Filmed routes of the tier table's own controls that the tiers lessons teach: the Quick Add
//! items of the Add Tier menu above the table and the angle cell's key and wheel nudges.
//!
//! A Tier form edit that lands on the same value is a different gesture, so none of these sets a
//! value through the form, and every route requires the visible result before it returns.
//!
//! It also holds the tier menus. A row's actions (Adopt, Pin, Add Anchor, Delete, ...) are items
//! of the row's "Actions for Tier N" menu; Table, Girdle, Culet and Concave Tier are items of the
//! "More Ways to Add a Tier" menu beside Add Tier; Adopt All and Select are items of the "Tier
//! List Menu". Each menu is a popup window, whose items report their place inside the popup, so
//! every item is looked up through `PopupGeometry`, which maps it to the main window.
//!
//! A row fits the dock's width, so the table does not scroll sideways. `Film::reveal_table_button`
//! keeps its sideways wheel turns as a fallback for a control that is not on view.

use crate::{
    debug_log,
    director::Film,
    pacing,
    popup_geometry::PopupGeometry,
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

/// The component every tier menu (a row's, Add Tier's and the list's) draws its items on.
const MENU_SURFACE: &str = "TierMenuSurface";

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
    /// Safe click and wheel area of the tier table: its scroll view clipped to the editor dock and
    /// kept 2 px clear of the left edge and of each scroll bar in the tree.
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

    /// The zero-based indices of the tier rows that are in the tree now. Rows scrolled out of the
    /// table leave the tree, so these are the rows on view (and one touching each edge).
    pub(crate) fn tier_rows_on_view(&self) -> Vec<i32> {
        let view = self.view();
        let Some(table) = view
            .root
            .query_descendants()
            .match_inherits(TABLE_COMPONENT)
            .find_first()
        else {
            return Vec::new();
        };
        table
            .query_descendants()
            .match_accessible_role(uiprobe::AccessibleRole::ListItem)
            .find_all()
            .into_iter()
            .filter_map(|row| {
                let label = row.accessible_label()?;
                let number = label.as_str().strip_prefix("Tier ")?.split(',').next()?;
                number.parse::<i32>().ok().map(|number| number - 1)
            })
            .collect()
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

    /// Opens the row's "..." menu ("Actions for Tier N") and returns its item `label` (a prefix
    /// of the item's accessible name). The row actions (Adopt, Pin, Add Anchor, Complete orbit,
    /// Detach, move, Duplicate, Surface Finish, Delete) live in this menu, not on the row; the
    /// menu is a popup, so the item is looked up in the whole window and not clipped to the table.
    pub(crate) fn tier_row_menu_item(&mut self, index: i32, label: &str) -> Result<Spot, String> {
        let menu_label = format!("Actions for Tier {}", index + 1);
        let button = self
            .tier_row_spot(index)?
            .handle
            .as_ref()
            .ok_or("tier row has no element handle")?
            .query_descendants()
            .match_accessible_role(uiprobe::AccessibleRole::Button)
            .find_all()
            .into_iter()
            .find(|control| control.accessible_label().is_some_and(|s| s == menu_label))
            .map(|control| Spot::of(&control))
            .ok_or_else(|| format!("{menu_label:?}: the row's menu button is missing"))?;
        self.click_and_invoke(&button)?;
        self.open_tier_menu_item(&menu_label, label)
    }

    /// Opens the "More Ways to Add a Tier" menu beside Add Tier and returns its item `label`
    /// ("+ Add Concave Tier", "Table", "Girdle", "Culet", ...).
    pub(crate) fn add_tier_menu_item(&mut self, label: &str) -> Result<Spot, String> {
        self.toolbar_menu_item("More Ways to Add a Tier", label)
    }

    /// Opens the "Tier List Menu" above the table and returns its item `label` ("Adopt All" or
    /// "Select").
    pub(crate) fn tier_list_menu_item(&mut self, label: &str) -> Result<Spot, String> {
        self.toolbar_menu_item("Tier List Menu", label)
    }

    /// Clicks the menu button `menu` of the bar above the tier table and returns the item `label`
    /// of the menu it opens.
    fn toolbar_menu_item(&mut self, menu: &str, label: &str) -> Result<Spot, String> {
        let button = targets::role_label(
            &self.view(),
            menu,
            uiprobe::AccessibleRole::Button,
            Some(TOOLBAR_COMPONENT),
        )
        .ok_or_else(|| format!("{menu:?}: the menu button above the tier table is missing"))?;
        self.click_and_invoke(&button)?;
        self.open_tier_menu_item(menu, label)
    }

    /// The item `label` of the tier menu `menu` that was just opened, in main-window coordinates.
    /// The exact name is tried first ("Pin" also starts other names), then `label` as a prefix
    /// ("Remove this tier" of "Remove this tier (Delete).").
    fn open_tier_menu_item(&mut self, menu: &str, label: &str) -> Result<Spot, String> {
        let ui = self.ui;
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            PopupGeometry::find(ui, MENU_SURFACE).is_ok()
        }) {
            return Err(format!(
                "{menu:?} did not open its menu: {}",
                PopupGeometry::find(ui, MENU_SURFACE)
                    .err()
                    .unwrap_or_default()
            ));
        }
        pump(80);
        self.hold(pacing::MENU_HOLD)?;
        let popup = PopupGeometry::find(ui, MENU_SURFACE)?;
        let role = uiprobe::AccessibleRole::Button;
        popup.control(ui, label, role).or_else(|exact| {
            popup.control_start(ui, label, role).map_err(|prefix| {
                format!("{menu:?}: the menu has no {label:?} item ({exact}; {prefix})")
            })
        })
    }

    /// Turns the wheel over the tier table until `find` names a button that is on view, and
    /// returns that button with the number of turns it took. `find` gets the table's safe area
    /// and usually calls [`Self::tier_row_control`], which checks the button is on view. The
    /// table is looked at before the first turn, so a visible button costs no scrolling; the
    /// pinned ACTIONS buttons normally need none.
    ///
    /// `row` is the tier row the button belongs to. The pinned buttons never scroll sideways, so
    /// a button that is not on view may belong to a row cut off at the top or bottom of the
    /// table (under the sideways bar, say): the wheel is then turned vertically, by about the
    /// cut-off distance, until the row is whole, before any sideways turn.
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

    /// Films the Quick Add item `label` (Girdle, Table or Culet) of the Add Tier menu above the
    /// tier table (the chevron beside Add Tier opens it), and requires exactly one new tier of
    /// that name.
    pub(super) fn quick_add_tier(&mut self, label: &str) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        let before = editor.get_tiers().row_count();
        let item = self.add_tier_menu_item(label)?;
        self.click_and_invoke(&item)?;
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
