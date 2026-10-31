//! Visible slider and popup routes for the catalogue filter lessons.

use super::library_search_query::shortening_query;
use crate::{director::Film, pacing, targets};
use pleochroma_cut::LibraryModel;
use slint::{ComponentHandle, Model};
use std::time::Duration;

impl Film<'_> {
    /// Step "Type in the search box": types the shortest start of the first listed title that
    /// leaves a shorter list (not "PC ", which every design of the fixture library starts with),
    /// then waits for the list to settle on it.
    pub(super) fn film_library_search(&mut self) -> Result<(), String> {
        let library = self.ui.global::<LibraryModel>();
        let listed = library.get_diagram_list();
        let first = listed
            .row_data(0)
            .ok_or("Library search has no listed design")?;
        let rows: Vec<(String, String)> = listed
            .iter()
            .map(|row| (row.title.to_string(), row.designer.to_string()))
            .collect();
        let before_matched_count = library.get_matched_count();
        let query = shortening_query(first.title.as_str(), &rows).ok_or_else(|| {
            format!(
                "no start of {:?} leaves a shorter list than the {} listed designs",
                first.title,
                rows.len()
            )
        })?;
        let field = targets::text_input(&self.view(), "Search the Library")
            .ok_or("Library search input is missing")?;
        self.click_and_type(&field, &query)?;
        crate::debug_log::debug_check("search text", &query, &library.get_search_text());
        if library.get_search_text().as_str() != query {
            return Err("Library search text did not read back as typed".into());
        }
        // Settled: the list is the filtered one (shorter, first row matching) and the header's
        // count has dropped with it.
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            let rows = library.get_diagram_list();
            let first_matches = rows.row_data(0).is_some_and(|row| {
                row.title
                    .to_string()
                    .to_lowercase()
                    .contains(&query.to_lowercase())
            });
            first_matches
                && rows.row_count() > 0
                && library.get_matched_count() > 0
                && library.get_matched_count() < before_matched_count
        }) {
            return Err(format!(
                "Library search {query:?} text changed but filtered results did not settle: matched={}, rows={}",
                library.get_matched_count(),
                library.get_diagram_list().row_count()
            ));
        }
        Ok(())
    }

    /// Step "Clear it": the cross of the search box, then the whole catalogue is back.
    pub(super) fn film_library_search_cleared(&mut self) -> Result<(), String> {
        let library = self.ui.global::<LibraryModel>();
        let original_first = library
            .get_diagram_list()
            .row_data(0)
            .ok_or("Library list is empty before Clear Search")?
            .title
            .to_string();
        let total_count = library.get_total_count();
        let clear = targets::run_label(&self.view(), "Clear Search")
            .ok_or("Clear Search button is missing")?;
        self.click_and_invoke(&clear)?;
        if !library.get_search_text().is_empty() {
            return Err("Clear Search did not empty the search box".into());
        }
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            let rows = library.get_diagram_list();
            library.get_matched_count() == total_count
                && rows.row_count() > 0
                && rows
                    .row_data(0)
                    .is_some_and(|row| row.title.as_str() == original_first.as_str())
        }) {
            return Err(format!(
                "Clear Search text emptied but the full catalogue did not settle: matched={}, total={}, rows={}",
                library.get_matched_count(),
                total_count,
                library.get_diagram_list().row_count()
            ));
        }
        Ok(())
    }

    pub(super) fn film_library_ri_filter(&mut self) -> Result<(), String> {
        let filters =
            targets::run_label_start(&self.view(), "Filters").ok_or("Filters button is missing")?;
        self.click_and_invoke(&filters)?;
        self.hold(pacing::MENU_HOLD)?;
        let library = self.ui.global::<LibraryModel>();
        let popup = crate::popup_geometry::PopupGeometry::find(self.ui, "FilterPanel")?;
        let slider = popup.control(
            self.ui,
            "Refractive Index, lowest value to keep",
            uiprobe::AccessibleRole::Slider,
        )?;
        let (old, lower, upper) = (
            library.get_ri_filter_min(),
            library.get_ri_bounds_min(),
            library.get_ri_filter_max(),
        );
        let span = library.get_ri_bounds_max() - lower;
        if !span.is_finite() || span <= f32::EPSILON {
            return Err(format!(
                "Refractive Index slider has degenerate bounds {lower}..{}",
                library.get_ri_bounds_max()
            ));
        }
        let increase = (span * 0.2).min((upper - old).max(0.0));
        let target = if increase > f32::EPSILON {
            old + increase
        } else {
            old - (span * 0.2).min((old - lower).max(0.0))
        };
        if (target - old).abs() <= f32::EPSILON || target > upper {
            return Err(format!(
                "Refractive Index lower handle cannot move within {lower}..{upper} from {old}"
            ));
        }
        let handle = slider
            .handle
            .as_ref()
            .ok_or("Refractive Index slider has no accessible element handle")?;
        let view = self.view();
        let thumbs: Vec<_> = handle
            .query_descendants()
            .match_id("Slider::thumb")
            .find_all()
            .iter()
            .map(targets::Spot::of)
            .map(|spot| popup.map_spot(spot))
            .filter(|spot| spot.usable(view.size))
            .collect();
        if thumbs.len() != 1 || slider.w <= thumbs[0].w {
            return Err(format!(
                "Refractive Index slider has {} usable thumbs or no usable track",
                thumbs.len()
            ));
        }
        let thumb = thumbs[0].clone();
        let start = thumb.center();
        let end = (
            slider.x + thumb.w / 2.0 + (slider.w - thumb.w) * (target - lower) / span,
            start.1,
        );
        self.gesture(start, end, None, false, false)?;
        let changed = library.get_ri_filter_min();
        if (changed - old).abs() < f32::EPSILON
            || changed < lower
            || changed > library.get_ri_filter_max()
        {
            return Err(format!(
                "Refractive Index drag did not commit within bounds: old={old}, new={changed}, allowed={lower}..{}",
                library.get_ri_filter_max()
            ));
        }
        self.hold(Duration::from_secs(1))?;
        self.close_library_filters_outside_popup()
    }

    pub(super) fn close_library_filters_outside_popup(&mut self) -> Result<(), String> {
        let view = self.view();
        let header = view
            .root
            .query_descendants()
            .match_inherits("HeaderToolbar")
            .find_first()
            .map(|element| targets::Spot::of(&element))
            .ok_or("HeaderToolbar is missing while Filters is open")?;
        let popup = crate::popup_geometry::PopupGeometry::find(self.ui, "FilterPanel")?.bounds();
        let candidates = [
            (header.x + 12.0, header.y + header.h / 2.0),
            (header.x + header.w - 12.0, header.y + header.h / 2.0),
        ];
        let (x, y) = candidates
            .into_iter()
            .find(|(x, y)| {
                *x >= header.x
                    && *x <= header.x + header.w
                    && *y >= header.y
                    && *y <= header.y + header.h
                    && !(*x >= popup.x
                        && *x <= popup.x + popup.w
                        && *y >= popup.y
                        && *y <= popup.y + popup.h)
            })
            .ok_or("no HeaderToolbar point is outside the Advanced Filters panel")?;
        self.rec
            .click_at(self.ui.window(), (x, y), (0.0, 0.0))
            .map_err(|error| format!("click outside Advanced Filters: {error:?}"))?;
        self.cursor = (x, y);
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            crate::popup_geometry::PopupGeometry::find(self.ui, "FilterPanel").is_err()
                || targets::run_label_start(&self.view(), "Advanced Filters").is_none()
        }) {
            return Err("outside click did not close Advanced Filters".into());
        }
        Ok(())
    }
}
