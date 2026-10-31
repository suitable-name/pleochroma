//! Moves the visible floating guide when it covers an extra-route pointer target.

use super::{Film, targets::Spot};
use pleochroma_cut::GuideModel;
use slint::ComponentHandle;
use std::time::Duration;

#[expect(clippy::too_many_lines, reason = "one filmed route step by step")]
pub(super) fn ensure_guide_clear_of(film: &mut Film<'_>, target: &Spot) -> Result<(), String> {
    let guide = film.ui.global::<GuideModel>();
    if !guide.get_open() {
        return Ok(());
    }
    let guide_id = guide.get_guide_id().to_string();
    let step_index = guide.get_step_index();
    let view = film.view();
    let floating = view
        .root
        .query_descendants()
        .match_inherits("GuideOverlayLayer")
        .find_first()
        .and_then(|layer| {
            layer
                .query_descendants()
                .match_inherits("GuidePanel")
                .find_first()
        });
    let panel = view
        .root
        .query_descendants()
        .match_inherits("GuidePanel")
        .find_first()
        .ok_or("open guide has no visible GuidePanel")?;
    let panel_spot = Spot::of(&panel);
    if !intersects(&panel_spot, target) {
        return Ok(());
    }
    let floating = floating.ok_or_else(|| {
        format!(
            "GuidePanel blocks the target but is docked and cannot be moved (guide={guide_id:?}, step={step_index})"
        )
    })?;
    // This id is the GuidePanel header's TouchArea; finding it under GuideOverlayLayer proves
    // this is the draggable floating mount rather than the docked editor column.
    let handle = floating
        .query_descendants()
        .match_id("header_drag")
        .find_first()
        .or_else(|| {
            floating
                .query_descendants()
                .match_id("GuidePanel::header_drag")
                .find_first()
        })
        .ok_or("floating GuidePanel has no header_drag handle")?;
    let header = Spot::of(&handle);
    if !header.usable(view.size) || header.w < 8.0 || header.h < 8.0 {
        return Err(format!(
            "floating GuidePanel drag handle is unusable: {}x{} at ({}, {})",
            header.w, header.h, header.x, header.y
        ));
    }
    let parameters = view
        .root
        .query_descendants()
        .match_inherits("ParametersPanel")
        .find_first()
        .map(|element| Spot::of(&element));
    let margin = 12.0;
    let right = view.size.0 - panel_spot.w - margin;
    let bottom = view.size.1 - panel_spot.h - margin;
    let xs = [margin, (view.size.0 - panel_spot.w) / 2.0, right];
    // Bottom rows first: a top-row spot parks the guide over the menu bar, title and header.
    let ys = [bottom, (view.size.1 - panel_spot.h) / 2.0, margin];
    let destination = ys
        .into_iter()
        .flat_map(|y| xs.into_iter().map(move |x| (x, y)))
        .map(|(x, y)| Spot {
            x,
            y,
            w: panel_spot.w,
            h: panel_spot.h,
            handle: None,
        })
        .find(|candidate| {
            candidate.x >= margin
                && candidate.y >= margin
                && candidate.x + candidate.w <= view.size.0 - margin
                && candidate.y + candidate.h <= view.size.1 - margin
                && !intersects(candidate, target)
                && !parameters
                    .as_ref()
                    .is_some_and(|bounds| intersects(candidate, bounds))
        })
        .ok_or_else(|| {
            format!(
                "no fully visible GuidePanel position clears the target and ParametersPanel (window={:?}, panel={}x{})",
                view.size, panel_spot.w, panel_spot.h
            )
        })?;
    let start = header.center();
    let end = (
        start.0 + destination.x - panel_spot.x,
        start.1 + destination.y - panel_spot.y,
    );
    film.gesture(start, end, None, false, false)?;
    film.hold(Duration::from_millis(180))?;

    let guide = film.ui.global::<GuideModel>();
    if !guide.get_open()
        || guide.get_guide_id().as_str() != guide_id
        || guide.get_step_index() != step_index
    {
        return Err(format!(
            "GuidePanel relocation changed the active step: guide={:?}, step={}, open={}",
            guide.get_guide_id(),
            guide.get_step_index(),
            guide.get_open()
        ));
    }
    let moved = film
        .view()
        .root
        .query_descendants()
        .match_inherits("GuideOverlayLayer")
        .find_first()
        .and_then(|layer| {
            layer
                .query_descendants()
                .match_inherits("GuidePanel")
                .find_first()
        })
        .map(|element| Spot::of(&element))
        .ok_or("floating GuidePanel disappeared after relocation")?;
    if intersects(&moved, target) || moved.x == panel_spot.x && moved.y == panel_spot.y {
        return Err(format!(
            "GuidePanel did not relocate clear of target: before=({}, {}), after=({}, {}), target=({}, {}, {}, {})",
            panel_spot.x, panel_spot.y, moved.x, moved.y, target.x, target.y, target.w, target.h
        ));
    }
    Ok(())
}

fn intersects(a: &Spot, b: &Spot) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}
