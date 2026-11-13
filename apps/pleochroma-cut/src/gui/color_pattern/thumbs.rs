//! The little pictures of the Start From gallery: a pattern as it looks in one stone.
//!
//! A radial pattern (rods, wedges, planes through the axis) is drawn as the cut through the stone,
//! the way the Slice view shows it. An axial one, whose zones only vary along the pattern axis
//! (layers across it), would show a single flat color in a cut, so it is drawn as the Crystal
//! view's stack of cuts instead. Both are drawn in the colors the pattern has in this stone
//! ([`SwatchStone`]) on a transparent background, so the gallery's tile shows through.

use super::{
    colors::SwatchStone,
    cross_section::{render_rod_on, render_slice_disc, scene},
};
use crate::gui::rough_colour::wizard::raster::Rgba;
use pleochroma::optics::zoning::ZoneShape;
use pleochroma_cut_core::material::pattern::ColorPattern;

/// The slice position a radial thumbnail is cut at: the middle of the stone.
const THUMB_SLICE_POS: f64 = 0.5;

/// Whether the zones of `pattern` vary only along the pattern axis: it has zones and every one is
/// a half-space or a slab whose normal is the pattern axis (a layer across it).
pub(super) fn is_axial(pattern: &ColorPattern) -> bool {
    !pattern.zones.is_empty()
        && pattern.zones.iter().all(|zone| match &zone.shape {
            ZoneShape::HalfSpace { normal, .. } | ZoneShape::Slab { normal, .. } => {
                normal.y.abs() > 0.999
            }
            _ => false,
        })
}

/// A `size` x `size` picture of `pattern` in `stone`, on a transparent background: the Crystal
/// view's rod for an [axial](is_axial) pattern, the cut through the middle of the stone
/// otherwise.
pub(super) fn pattern_thumbnail(
    pattern: &ColorPattern,
    stone: &SwatchStone<'_>,
    size: usize,
) -> Rgba {
    let drawn = scene(pattern, stone);
    if is_axial(pattern) {
        render_rod_on(&drawn, size, size, [0, 0, 0, 0])
    } else {
        render_slice_disc(&drawn, THUMB_SLICE_POS, size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::material::pattern::{PatternAxis, TemplateId, template};

    /// A 7 mm stone with no material at hand.
    const STONE: SwatchStone<'static> = SwatchStone {
        path_mm: 10.0,
        width_mm: 7.0,
        host: None,
    };

    #[test]
    fn layers_across_the_axis_are_axial_and_rods_and_planes_through_it_are_not() {
        let axial = |id| is_axial(&template(id, PatternAxis::TABLE_NORMAL));
        assert!(axial(TemplateId::Banded));
        assert!(axial(TemplateId::Bicolor));
        assert!(axial(TemplateId::Tricolor));
        assert!(!axial(TemplateId::Watermelon));
        assert!(!axial(TemplateId::Split));
        assert!(!axial(TemplateId::Sectors));
        assert!(!axial(TemplateId::Uniform), "no zones to stack");
    }

    #[test]
    fn a_radial_thumbnail_is_a_disc_and_an_axial_one_a_rod_on_transparency() {
        let watermelon = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
        let disc = pattern_thumbnail(&watermelon, &STONE, 48);
        assert_eq!((disc.width, disc.height), (48, 48));
        assert_eq!(disc.pixel(0, 0)[3], 0);
        assert_eq!(disc.pixel(24, 24)[3], 255);

        let banded = template(TemplateId::Banded, PatternAxis::TABLE_NORMAL);
        let rod = pattern_thumbnail(&banded, &STONE, 48);
        assert_eq!((rod.width, rod.height), (48, 48));
        assert_eq!(rod.pixel(0, 0)[3], 0);
        assert_eq!(rod.pixel(24, 24)[3], 255);
    }
}
