//! The filmed routes of the Facet Chips lesson: the cursor moves to the control under Facets in
//! the Tier tab (the position box and + Add, the x and detach icons on a chip, the teeth box and
//! Rotate, Mirror) and clicks it or types into it. A Tier form edit that lands on the same
//! indices is a different gesture, so nothing is set through a model callback or a form field,
//! and every route requires the visible result (the chips) before it returns.

use crate::{
    director::Film,
    support::app::pump,
    targets::{self, Spot},
};
use pleochroma_cut::EditorModel;
use slint::{ComponentHandle, Model};
use std::time::Duration;

/// The accessible label of the strip of chips (`TierFacetsSection`).
const CHIP_STRIP: &str = "Facets of This Tier";

/// The accessible labels of the boxes and buttons of `TierFacetsSection`. A `LineEdit` is labelled
/// by its `accessible-label`; a `TipButton` by its `reader_label` (its `hint` is only the
/// description), or its `text` when it has no reader label.
const ADD_BOX: &str = "Position of the Facet to Add";
const ADD_BUTTON: &str = "Add Facet";
const ROTATE_BOX: &str = "Number of Teeth to Rotate By";
const ROTATE_UP_BUTTON: &str = "Rotate Up";
const MIRROR_BUTTON: &str = "Mirror";

/// How often the inspector is scrolled up the page, or the strip along its chips, before the
/// icon is given up on.
const REVEAL_TRIES: usize = 14;

/// Whether `inner` lies wholly inside `outer`, give or take a pixel of rounding.
const fn wholly_inside(outer: &Spot, inner: &Spot) -> bool {
    inner.x >= outer.x - 1.0
        && inner.y >= outer.y - 1.0
        && inner.x + inner.w <= outer.x + outer.w + 1.0
        && inner.y + inner.h <= outer.y + outer.h + 1.0
}

/// A clipped `ScrollView` area with actual visible bars removed. Fluent 1.18.1 sets the vertical
/// bar width and horizontal bar height to 14px in `widgets/fluent/scrollview.slint` (lines 190,
/// 204); use those measured theme dimensions as conservative fallbacks, with a 2px inset.
///
/// `sideways_possible` says whether the area can ever show a sideways bar: the bottom 16 px are
/// kept clear for a bar missing from the tree only then. The chip strip lays its chips out to the
/// strip's width less its own gutter, so it never scrolls sideways and its bottom row is whole
/// right to the edge; reserving 16 px there made every icon of the last row look cut off, and the
/// wheel turns meant to bring it in only bubbled up and scrolled the inspector instead.
fn safe_scroll_area(
    scroll: &uiprobe::ElementHandle,
    clip: &Spot,
    window: (f32, f32),
    sideways_possible: bool,
) -> Option<Spot> {
    let viewport = Spot::of(scroll).clipped_to(clip, window)?;
    let bars: Vec<_> = scroll
        .query_descendants()
        .match_inherits("ScrollBar")
        .find_all()
        .into_iter()
        .map(|bar| Spot::of(&bar))
        .filter(|bar| {
            bar.w > 0.0
                && bar.h > 0.0
                && bar.x < viewport.x + viewport.w
                && bar.x + bar.w > viewport.x
                && bar.y < viewport.y + viewport.h
                && bar.y + bar.h > viewport.y
        })
        .collect();
    let mut left = viewport.x;
    let mut right = viewport.x + viewport.w;
    let mut top = viewport.y;
    let mut bottom = viewport.y + viewport.h;
    let mut vertical_found = false;
    let mut horizontal_found = false;
    for bar in bars {
        if bar.h > bar.w {
            vertical_found = true;
            if bar.center().0 >= viewport.center().0 {
                right = right.min(bar.x - 2.0);
            } else {
                left = left.max(bar.x + bar.w + 2.0);
            }
        } else if bar.w > bar.h {
            horizontal_found = true;
            if bar.center().1 >= viewport.center().1 {
                bottom = bottom.min(bar.y - 2.0);
            } else {
                top = top.max(bar.y + bar.h + 2.0);
            }
        }
    }
    if !vertical_found {
        right -= 16.0;
    }
    if !horizontal_found && sideways_possible {
        bottom -= 16.0;
    }
    let safe = Spot {
        x: left + 2.0,
        y: top + 2.0,
        w: (right - left - 4.0).max(0.0),
        h: (bottom - top - 4.0).max(0.0),
        handle: None,
    };
    [safe.x, safe.y, safe.w, safe.h]
        .into_iter()
        .all(f32::is_finite)
        .then_some(safe)
        .filter(|safe| safe.w >= 24.0 && safe.h >= 16.0)
}

/// Where the detach icon of one chip, its strip's scrolling area and the inspector's scrolling
/// area are on screen right now.
struct ChipPlace {
    icon: Spot,
    inspector_safe: Spot,
    chip_safe: Spot,
}

/// Why `chip_place` found no icon.
enum ChipMiss {
    /// The strip or the inspector's area is outside the window: scroll and retry.
    NotInView,
    /// The strip is on view but the chip's row is not in it: the strip scrolls (vertically, past
    /// three rows), down when the chip comes after the chips shown (`down`) or up when it comes
    /// before them. `seen` are the labels of the chips shown now.
    StripScroll {
        down: bool,
        chip_safe: Spot,
        seen: Vec<String>,
    },
    /// The strip is fully in view but the row or icon is not unique.
    Missing(String),
}

/// Whether the accessible label `label` of a chip names the chip at index position `position`.
fn names_chip(label: &str, position: &str) -> bool {
    label.strip_prefix("Position ").is_some_and(|rest| {
        rest.starts_with(position)
            && matches!(
                &rest[position.len()..],
                "." | ". Frosted."
                    | ", detached from its symmetric family."
                    | ", detached from its symmetric family. Frosted."
            )
    })
}

impl Film<'_> {
    /// Films clicking the detach icon on the chip of the facet at index position `position` of the
    /// tier called `tier`, then requires the chip to read as detached.
    pub(super) fn detach_chip(&mut self, tier: &str, position: f32) -> Result<(), String> {
        self.open_tier_chips(tier, false)?;
        let (label, detached) = self
            .chip_at(position)
            .ok_or_else(|| format!("{tier}: the Tier tab has no chip at position {position}"))?;
        if detached {
            return Err(format!("{tier}: the chip at {label} is already detached"));
        }
        let icon = self.reveal_chip_icon(
            &label,
            &format!("Detach the facet at position {label} from"),
        )?;
        self.click_and_invoke(&icon)?;
        self.settle()?;
        if !self.chip_at(position).is_some_and(|(_, now)| now) {
            return Err(format!(
                "{tier}: clicking the detach icon did not detach the chip at {label}"
            ));
        }
        Ok(())
    }

    /// Films the lesson's first step: clicks the row of `tier`, types `text` in the position box
    /// under Facets and clicks + Add, then requires a chip at index position `position`.
    pub(super) fn add_chip_facet(
        &mut self,
        tier: &str,
        position: f32,
        text: &str,
    ) -> Result<(), String> {
        self.open_tier_chips(tier, true)?;
        let before = self.chip_positions();
        if before.iter().any(|at| (at - position).abs() < 1e-3) {
            return Err(format!("{tier}: there is already a chip at {position}"));
        }
        let field = self
            .input_spot(ADD_BOX)?
            .ok_or_else(|| format!("{tier}: the {ADD_BOX:?} box is not visible"))?;
        self.click_and_type(&field, text)?;
        let button = self.reveal_inspector_button(ADD_BUTTON)?;
        self.click_and_invoke(&button)?;
        self.settle()?;
        let after = self.chip_positions();
        if after.len() <= before.len() || !after.iter().any(|at| (at - position).abs() < 1e-3) {
            return Err(format!(
                "{tier}: clicking {ADD_BUTTON:?} did not put a chip at {position} ({} chips before, {} after)",
                before.len(),
                after.len()
            ));
        }
        Ok(())
    }

    /// Films clicking the x on the chip at index position `position` of `tier`, then requires
    /// the chip to be gone.
    pub(super) fn remove_chip(&mut self, tier: &str, position: f32) -> Result<(), String> {
        self.open_tier_chips(tier, false)?;
        let (label, _) = self
            .chip_at(position)
            .ok_or_else(|| format!("{tier}: the Tier tab has no chip at position {position}"))?;
        let icon = self.reveal_chip_icon(
            &label,
            &format!("Remove the facet at position {label} from"),
        )?;
        self.click_and_invoke(&icon)?;
        self.settle()?;
        if self.chip_at(position).is_some() {
            return Err(format!(
                "{tier}: clicking the x did not remove the chip at {label}"
            ));
        }
        Ok(())
    }

    /// Films typing `teeth` in the teeth box under Facets and clicking the first Rotate button
    /// (up to higher index numbers), then requires the chips to have moved.
    pub(super) fn rotate_chips_up(&mut self, tier: &str, teeth: &str) -> Result<(), String> {
        self.open_tier_chips(tier, false)?;
        let before = self.chip_positions();
        let field = self
            .input_spot(ROTATE_BOX)?
            .ok_or_else(|| format!("{tier}: the {ROTATE_BOX:?} box is not visible"))?;
        self.click_and_type(&field, teeth)?;
        let button = self.reveal_inspector_button(ROTATE_UP_BUTTON)?;
        self.click_and_invoke(&button)?;
        self.settle()?;
        self.require_moved_chips(tier, &before, ROTATE_UP_BUTTON)
    }

    /// Films clicking Mirror under Facets, then requires the chips to have moved.
    pub(super) fn mirror_chips(&mut self, tier: &str) -> Result<(), String> {
        self.open_tier_chips(tier, false)?;
        let before = self.chip_positions();
        let button = self.reveal_inspector_button(MIRROR_BUTTON)?;
        self.click_and_invoke(&button)?;
        self.settle()?;
        self.require_moved_chips(tier, &before, MIRROR_BUTTON)
    }

    /// Fails unless the chips of the selected tier now sit at other positions than `before`.
    fn require_moved_chips(&self, tier: &str, before: &[f32], clicked: &str) -> Result<(), String> {
        if self.chip_positions() == before {
            return Err(format!(
                "{tier}: clicking {clicked:?} left the chips where they were"
            ));
        }
        Ok(())
    }

    /// Makes the Tier tab the open inspector tab and `tier` the selected row. The row is clicked
    /// when `click_row` is set (the lesson's own "click the row" action) or when it is not the
    /// selected one yet.
    fn open_tier_chips(&mut self, tier: &str, click_row: bool) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        if editor.get_inspector_tab() != 0 {
            self.film_inspector_tab(0)?;
        }
        let selected = usize::try_from(editor.get_selected_tier_index())
            .ok()
            .and_then(|at| editor.get_tiers().row_data(at));
        if click_row || selected.is_none_or(|row| !row.name.trim().eq_ignore_ascii_case(tier)) {
            self.film_select_tier(tier)?;
        }
        Ok(())
    }

    /// Lets the click take effect and holds a frame so the result is on film.
    fn settle(&mut self) -> Result<(), String> {
        pump(80);
        self.rec.hold(Duration::from_millis(150))
    }

    /// The index positions of the chips of the selected tier, in chip order.
    fn chip_positions(&self) -> Vec<f32> {
        self.ui
            .global::<EditorModel>()
            .get_selected_tier_chips()
            .iter()
            .map(|chip| chip.position)
            .collect()
    }

    /// Reveals an Inspector button with the shared exact-label, bounded form scroller. It is
    /// looked for where the inspector is now first (the box beside it was just typed in).
    fn reveal_inspector_button(&mut self, label: &str) -> Result<Spot, String> {
        self.inspector_form_control(label, uiprobe::AccessibleRole::Button)?
            .ok_or_else(|| format!("Inspector button {label:?} is not uniquely visible"))
    }

    /// The label and the detached state of the chip of the selected tier at index position
    /// `position`.
    fn chip_at(&self, position: f32) -> Option<(String, bool)> {
        self.ui
            .global::<EditorModel>()
            .get_selected_tier_chips()
            .iter()
            .find(|chip| (chip.position - position).abs() < 1e-3)
            .map(|chip| (chip.label.to_string(), chip.detached))
    }

    /// What to do about a chip the strip does not show although the strip is on view: scroll the
    /// strip towards it when the model puts it before or after the chips shown, otherwise a miss.
    fn chip_miss(
        &self,
        position: &str,
        row_labels: &[String],
        chip_safe: &Spot,
        detail: String,
    ) -> ChipMiss {
        let chips: Vec<String> = self
            .ui
            .global::<EditorModel>()
            .get_selected_tier_chips()
            .iter()
            .map(|chip| chip.label.to_string())
            .collect();
        let target = chips.iter().position(|label| label == position);
        let seen: Vec<usize> = chips
            .iter()
            .enumerate()
            .filter(|(_, label)| row_labels.iter().any(|row| names_chip(row, label)))
            .map(|(at, _)| at)
            .collect();
        if let (Some(target), Some(&first), Some(&last)) = (target, seen.first(), seen.last())
            && (target > last || target < first)
        {
            return ChipMiss::StripScroll {
                down: target > last,
                chip_safe: chip_safe.clone(),
                seen: row_labels.to_vec(),
            };
        }
        ChipMiss::Missing(detail)
    }

    /// Where the icon whose label starts with `prefix` is, with its strip and the inspector's
    /// scrolling area; `None` while the inspector does not show them.
    fn chip_place(&self, position: &str, prefix: &str) -> Result<ChipPlace, ChipMiss> {
        let view = self.view();
        let inspector = view
            .root
            .query_descendants()
            .match_inherits("EditorInspector")
            .find_first()
            .ok_or(ChipMiss::NotInView)?;
        let inspector_scroll = inspector
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .ok_or(ChipMiss::NotInView)?;
        let dock = targets::run(&view, &crate::targets::Query::Id("EditorView::dock_scroll"))
            .ok_or(ChipMiss::NotInView)?;
        let inspector_safe = safe_scroll_area(&inspector_scroll, &dock, view.size, true)
            .ok_or(ChipMiss::NotInView)?;
        let strip_handle = inspector
            .query_descendants()
            .find_all()
            .into_iter()
            .find(|element| {
                element
                    .accessible_label()
                    .is_some_and(|label| label == CHIP_STRIP)
            })
            .ok_or(ChipMiss::NotInView)?;
        let strip = Spot::of(&strip_handle);
        let chip_scroll = strip_handle
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .ok_or(ChipMiss::NotInView)?;
        let chip_safe =
            safe_scroll_area(&chip_scroll, &strip, view.size, false).ok_or(ChipMiss::NotInView)?;
        let row_labels: Vec<String> = strip_handle
            .query_descendants()
            .match_accessible_role(uiprobe::AccessibleRole::ListItem)
            .find_all()
            .into_iter()
            .filter_map(|element| element.accessible_label().map(|label| label.to_string()))
            .collect();
        // Rows outside the inspector's viewport or the strip's own scrolled area are not in the
        // tree, so a missing chip is a real miss only while the strip is on view (as much of it
        // as the viewport can hold); a strip only partly on view, with its first row in the tree
        // and the next ones below the viewport, is a reason to scroll, not a failure.
        let overlap = (strip.y + strip.h).min(inspector_safe.y + inspector_safe.h)
            - strip.y.max(inspector_safe.y);
        let strip_in_view = overlap >= strip.h.min(inspector_safe.h) - 2.0;
        let miss = |detail: String| {
            if row_labels.is_empty() || !strip_in_view {
                ChipMiss::NotInView
            } else {
                self.chip_miss(position, &row_labels, &chip_safe, detail)
            }
        };
        let rows: Vec<_> = strip_handle
            .query_descendants()
            .match_accessible_role(uiprobe::AccessibleRole::ListItem)
            .find_all()
            .into_iter()
            .filter(|element| {
                element
                    .accessible_label()
                    .is_some_and(|label| names_chip(&label, position))
            })
            .collect();
        if rows.len() != 1 {
            return Err(miss(format!(
                "{} rows matched position {position:?}; row labels seen: {row_labels:?}",
                rows.len()
            )));
        }
        let icons: Vec<_> = rows[0]
            .query_descendants()
            .match_accessible_role(uiprobe::AccessibleRole::Button)
            .find_all()
            .into_iter()
            .filter(|element| {
                element
                    .accessible_label()
                    .is_some_and(|label| label.starts_with(prefix))
            })
            .collect();
        if icons.len() != 1 {
            return Err(miss(format!(
                "{} icons matched {prefix:?} in the row; row labels seen: {row_labels:?}",
                icons.len()
            )));
        }
        let icon = Spot::of(&icons[0]);
        Ok(ChipPlace {
            icon,
            inspector_safe,
            chip_safe,
        })
    }

    /// Scrolls until the icon whose label starts with `prefix` can be clicked: the inspector up
    /// to the Facets section, then the strip of chips along to the chip. Both are wheel turns
    /// over the control, as a learner would make them.
    fn reveal_chip_icon(&mut self, position: &str, prefix: &str) -> Result<Spot, String> {
        self.scroll_inspector(3000.0)?;
        // The chips shown when the strip was last turned: unchanged after a turn means the strip
        // has nothing more to show that way, so the inspector is what hides the chip.
        let mut last_strip_seen: Option<Vec<String>> = None;
        for _ in 0..REVEAL_TRIES {
            let place = match self.chip_place(position, prefix) {
                Ok(place) => place,
                Err(ChipMiss::StripScroll {
                    down,
                    chip_safe,
                    seen,
                }) => {
                    if last_strip_seen.as_ref() == Some(&seen) {
                        self.scroll_inspector(if down { -100.0 } else { 100.0 })?;
                    } else {
                        self.wheel_over(&chip_safe, if down { -60.0 } else { 60.0 })?;
                        pump(60);
                        self.rec.hold(Duration::from_millis(150))?;
                    }
                    last_strip_seen = Some(seen);
                    continue;
                }
                Err(ChipMiss::NotInView) => {
                    // The Facets strip is not wholly in the inspector viewport: scroll towards it
                    // (down while it is below or not yet in the tree, up once a turn went past it).
                    let (viewport, strip) = {
                        let view = self.view();
                        let inspector = view
                            .root
                            .query_descendants()
                            .match_inherits("EditorInspector")
                            .find_first();
                        let viewport = inspector
                            .as_ref()
                            .and_then(|inspector| {
                                inspector
                                    .query_descendants()
                                    .match_inherits("ScrollView")
                                    .find_first()
                            })
                            .map(|scroll| Spot::of(&scroll));
                        let strip = inspector.and_then(|inspector| {
                            inspector
                                .query_descendants()
                                .find_all()
                                .into_iter()
                                .find(|element| {
                                    element
                                        .accessible_label()
                                        .is_some_and(|label| label == CHIP_STRIP)
                                })
                                .map(|element| Spot::of(&element))
                        });
                        (viewport, strip)
                    };
                    let height = viewport.as_ref().map_or(110.0, |viewport| viewport.h);
                    let step = (height * 0.65).max(24.0);
                    let above = match (&viewport, &strip) {
                        (Some(viewport), Some(strip)) => strip.y < viewport.y,
                        _ => false,
                    };
                    self.scroll_inspector(if above { step } else { -step })?;
                    continue;
                }
                Err(ChipMiss::Missing(detail)) => {
                    return Err(format!(
                        "the chip at position {position:?} has no unique row-scoped {prefix:?} icon ({detail})"
                    ));
                }
            };
            // The strip can be taller than the inspector viewport, so only the icon's own rows
            // have to be on view vertically.
            let icon_y_visible = place.icon.y >= place.inspector_safe.y
                && place.icon.y + place.icon.h <= place.inspector_safe.y + place.inspector_safe.h;
            let icon_visible = wholly_inside(&place.chip_safe, &place.icon) && icon_y_visible;
            if icon_visible {
                return Ok(place.icon);
            } else if !icon_y_visible {
                let delta = place.icon.center().1 - place.inspector_safe.center().1;
                let wheel = if delta.abs() < 2.0 {
                    if place.icon.y < place.inspector_safe.y {
                        100.0
                    } else {
                        -100.0
                    }
                } else {
                    -delta.clamp(-160.0, 160.0)
                };
                self.scroll_inspector(wheel)?;
            } else {
                // The strip scrolls vertically (its chips wrap), so an icon half cut by the
                // strip's edge is brought in with the wheel up or down.
                let delta = place.icon.center().1 - place.chip_safe.center().1;
                if delta.abs() < 2.0 || place.icon.x < place.chip_safe.x - 1.0 {
                    return Err(format!(
                        "the {prefix:?} icon is clipped without a safe reveal direction"
                    ));
                }
                self.wheel_over(&place.chip_safe, -delta.clamp(-60.0, 60.0))?;
                pump(60);
                self.rec.hold(Duration::from_millis(150))?;
            }
        }
        Err(format!(
            "the chip icon {prefix:?} could not be scrolled into view in {REVEAL_TRIES} turns"
        ))
    }
}
