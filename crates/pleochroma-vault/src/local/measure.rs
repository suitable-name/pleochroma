//! Measured metadata for a design about to be stored: proportions, volume and shape.
//!
//! The geometry (the facet planes and the solid they bound) is not this crate's: the
//! caller supplies it through [`SolidSource`], so the desktop's database-angle
//! reconstruction, the Android core's solver and the browser's solver all share the
//! one rule that turns a measured solid into the stored text fields.

use crate::model::{DEFAULT_SHAPES, detail::FacetingDiagramDetail};

/// The figures of a design's solid that the stored proportions are made of.
///
/// All lengths are in the solid's own (mast) units; only their ratios are stored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolidMeasure {
    /// The larger of the two horizontal extents.
    pub length_axis: f64,
    /// The smaller of the two horizontal extents.
    pub width_axis: f64,
    /// Full vertical extent, table to culet.
    pub total_height: f64,
    /// Crown height above the girdle band, when the stone has a live girdle.
    pub crown_height: Option<f64>,
    /// Pavilion depth below the girdle band, when the stone has a live girdle.
    pub pavilion_depth: Option<f64>,
    /// Volume of the solid, in cubed mast units.
    pub volume: f64,
}

/// Where a design's measured solid comes from.
///
/// Implemented by whoever holds the geometry code; the importer never builds planes
/// itself.
pub trait SolidSource {
    /// Measures the solid `detail` describes.
    ///
    /// `None` when the design does not read, solve or close: every stored field is then
    /// left as it was, because a number that could not be measured is never invented.
    fn measure(&mut self, detail: &FacetingDiagramDetail) -> Option<SolidMeasure>;

    /// The number of girdle facets of the solid the last [`Self::measure`] returned.
    ///
    /// Called only after a successful `measure`, and only when the design names no
    /// shape of its own.
    fn girdle_sides(&mut self) -> usize;
}

/// Fills the proportion fields and, when the file named none, the shape, from the
/// solid `source` measures.
///
/// A design without an angle table is left untouched. Values are written with plain
/// `f64::to_string()`; the volume is stored dimensionless (`Vol/W^3`), as a printed
/// diagram sheet states it. A shape the file itself names outranks the girdle
/// classification.
pub fn apply_measured_metadata(detail: &mut FacetingDiagramDetail, source: &mut impl SolidSource) {
    if detail.angle_settings_table.is_empty() {
        return;
    }
    let Some(m) = source.measure(detail) else {
        return;
    };
    let w = m.width_axis;
    if w < 1e-9 {
        return;
    }

    detail.lw_ratio = Some((m.length_axis / w).to_string());
    detail.hw_ratio = Some((m.total_height / w).to_string());
    detail.cw_ratio = m.crown_height.map(|c| (c / w).to_string());
    detail.pw_ratio = m.pavilion_depth.map(|p| (p / w).to_string());
    detail.volume = Some((m.volume / (w * w * w)).to_string());

    detail.shape = detail
        .shape
        .take()
        .or_else(|| classify_shape_by_sides(source.girdle_sides(), m.length_axis / w));
}

/// The shape the girdle outline makes unambiguous, `None` otherwise.
///
/// A blank shape is correctable by the user, whereas a confidently wrong one looks
/// authoritative and would quietly poison the library's shape filter.
///
/// Classifies by girdle facet count, not schedule fold count: a shape name describes
/// the silhouette. With `sides` the girdle facet count and `lw` the measured
/// length/width ratio (at least 1.0): twelve or more sides and `lw <= 1.03` is Round;
/// exactly 3/4/6/8 sides is Triangle/Square/Hexagon/Octagon within the length/width
/// bounds those outlines measure (`1.20` for the three- and six-sided ones, which carry
/// an inherent `2/sqrt(3)`, `1.03` for the square and octagon); anything else is not
/// guessed. The chosen name is looked up in [`DEFAULT_SHAPES`] rather than returned as
/// a literal, so a rename there cannot leave a label the vocabulary no longer knows.
#[must_use]
pub fn classify_shape_by_sides(sides: usize, lw: f64) -> Option<String> {
    const ROUND_LW_MAX: f64 = 1.03;
    const POLYGON_LW_MAX: f64 = 1.20;
    /// Below this, a many-sided outline is not confidently "round": a 10-sided outline
    /// is as plausibly a decagon as a coarse circle, so it gets no shape.
    const ROUND_MIN_SIDES: usize = 12;

    let name = match sides {
        3 if lw <= POLYGON_LW_MAX => "Triangle",
        4 if lw <= ROUND_LW_MAX => "Square",
        6 if lw <= POLYGON_LW_MAX => "Hexagon",
        8 if lw <= ROUND_LW_MAX => "Octagon",
        n if n >= ROUND_MIN_SIDES && lw <= ROUND_LW_MAX => "Round",
        _ => return None,
    };
    DEFAULT_SHAPES
        .iter()
        .find(|s| **s == name)
        .map(|s| (*s).to_string())
}

/// Gives the designer line a design file's `designer` and `source_citation` fields
/// leave empty, in the scraped catalogue's `Designer; Citation` form.
///
/// The list and the free-text search then find the designer. A detail that already has a
/// designer line, or names no designer, is left as it was.
pub fn fill_designer_info(detail: &mut FacetingDiagramDetail) {
    if detail.designer_info.is_some() {
        return;
    }
    let designer = detail
        .designer
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if designer.is_empty() {
        return;
    }
    let citation = detail
        .source_citation
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    detail.designer_info = Some(if citation.is_empty() {
        designer.to_owned()
    } else {
        format!("{designer}; {citation}")
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::angle::AngleSetting;

    /// A source that measures a fixed solid.
    struct Fixed {
        measure: Option<SolidMeasure>,
        sides: usize,
        asked_for_sides: bool,
    }

    impl SolidSource for Fixed {
        fn measure(&mut self, _detail: &FacetingDiagramDetail) -> Option<SolidMeasure> {
            self.measure
        }

        fn girdle_sides(&mut self) -> usize {
            self.asked_for_sides = true;
            self.sides
        }
    }

    const ROUND: SolidMeasure = SolidMeasure {
        length_axis: 10.0,
        width_axis: 10.0,
        total_height: 6.0,
        crown_height: Some(1.5),
        pavilion_depth: Some(4.0),
        volume: 500.0,
    };

    fn detail_with_rows() -> FacetingDiagramDetail {
        FacetingDiagramDetail {
            angle_settings_table: vec![AngleSetting::default()],
            ..FacetingDiagramDetail::default()
        }
    }

    fn fixed(measure: Option<SolidMeasure>, sides: usize) -> Fixed {
        Fixed {
            measure,
            sides,
            asked_for_sides: false,
        }
    }

    #[test]
    fn a_measured_solid_fills_the_ratios_the_dimensionless_volume_and_the_shape() {
        let mut detail = detail_with_rows();
        apply_measured_metadata(&mut detail, &mut fixed(Some(ROUND), 16));
        assert_eq!(detail.lw_ratio.as_deref(), Some("1"));
        assert_eq!(detail.hw_ratio.as_deref(), Some("0.6"));
        assert_eq!(detail.cw_ratio.as_deref(), Some("0.15"));
        assert_eq!(detail.pw_ratio.as_deref(), Some("0.4"));
        assert_eq!(detail.volume.as_deref(), Some("0.5"));
        assert_eq!(detail.shape.as_deref(), Some("Round"));
    }

    #[test]
    fn a_design_without_an_angle_table_or_a_measurable_solid_is_left_untouched() {
        let mut empty = FacetingDiagramDetail::default();
        let mut source = fixed(Some(ROUND), 16);
        apply_measured_metadata(&mut empty, &mut source);
        assert_eq!(
            format!("{empty:?}"),
            format!("{:?}", FacetingDiagramDetail::default())
        );

        let mut unmeasurable = detail_with_rows();
        apply_measured_metadata(&mut unmeasurable, &mut fixed(None, 16));
        assert_eq!(
            format!("{unmeasurable:?}"),
            format!("{:?}", detail_with_rows())
        );

        let mut flat = detail_with_rows();
        let zero_width = SolidMeasure {
            width_axis: 0.0,
            ..ROUND
        };
        apply_measured_metadata(&mut flat, &mut fixed(Some(zero_width), 16));
        assert_eq!(format!("{flat:?}"), format!("{:?}", detail_with_rows()));
    }

    #[test]
    fn a_missing_crown_or_pavilion_stays_none_rather_than_fabricated() {
        let mut detail = detail_with_rows();
        let no_girdle = SolidMeasure {
            crown_height: None,
            pavilion_depth: None,
            ..ROUND
        };
        apply_measured_metadata(&mut detail, &mut fixed(Some(no_girdle), 16));
        assert_eq!(detail.cw_ratio, None);
        assert_eq!(detail.pw_ratio, None);
        assert!(detail.lw_ratio.is_some());
    }

    #[test]
    fn a_shape_the_file_names_outranks_the_girdle_and_the_girdle_is_not_even_counted() {
        let mut detail = detail_with_rows();
        detail.shape = Some("Heart".to_owned());
        let mut source = fixed(Some(ROUND), 16);
        apply_measured_metadata(&mut detail, &mut source);
        assert_eq!(detail.shape.as_deref(), Some("Heart"));
        assert!(!source.asked_for_sides);
    }

    #[test]
    fn the_girdle_outline_rule_names_only_the_unambiguous_shapes() {
        assert_eq!(classify_shape_by_sides(16, 1.0).as_deref(), Some("Round"));
        assert_eq!(classify_shape_by_sides(12, 1.03).as_deref(), Some("Round"));
        assert_eq!(classify_shape_by_sides(11, 1.0), None);
        assert_eq!(classify_shape_by_sides(16, 1.6), None);
        assert_eq!(classify_shape_by_sides(0, 1.0), None);
        assert_eq!(
            classify_shape_by_sides(3, 1.15).as_deref(),
            Some("Triangle")
        );
        assert_eq!(
            classify_shape_by_sides(6, 1.1547).as_deref(),
            Some("Hexagon")
        );
        assert_eq!(classify_shape_by_sides(4, 1.0).as_deref(), Some("Square"));
        assert_eq!(classify_shape_by_sides(4, 1.1), None);
        assert_eq!(classify_shape_by_sides(8, 1.0).as_deref(), Some("Octagon"));
        assert_eq!(classify_shape_by_sides(8, 1.1), None);
        assert_eq!(classify_shape_by_sides(5, 1.0), None);
    }

    #[test]
    fn the_designer_line_joins_designer_and_citation_and_never_overwrites() {
        let mut detail = FacetingDiagramDetail {
            designer: Some(" Ada ".to_owned()),
            source_citation: Some("Facet Journal 4".to_owned()),
            ..FacetingDiagramDetail::default()
        };
        fill_designer_info(&mut detail);
        assert_eq!(
            detail.designer_info.as_deref(),
            Some("Ada; Facet Journal 4")
        );

        detail.source_citation = None;
        detail.designer_info = None;
        fill_designer_info(&mut detail);
        assert_eq!(detail.designer_info.as_deref(), Some("Ada"));

        detail.designer_info = Some("typed by hand".to_owned());
        fill_designer_info(&mut detail);
        assert_eq!(detail.designer_info.as_deref(), Some("typed by hand"));

        let mut anonymous = FacetingDiagramDetail::default();
        fill_designer_info(&mut anonymous);
        assert_eq!(anonymous.designer_info, None);
    }
}
