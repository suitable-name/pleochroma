//! The edits of a Color Pattern, as pure functions: each takes the current pattern and returns the
//! changed one (`None`: nothing to change or not possible). The window handlers in `actions.rs`
//! put the result through the undo stack and the validation.
//!
//! Geometry is in the unit the pattern holds: for a stone one unit wide when it "Scales With Stone
//! Size", millimeters when it is "Fixed in mm" (`stone_width_mm` converts a default that is
//! written for one unit).

use super::{
    form::{form_of, host_form},
    model::{
        ADD_ZONE_HINT, default_shape, kind_of, new_zone_lch, new_zone_name, scale_shape, set_param,
        with_size,
    },
};
use crate::gui::rough_colour::wizard::zone_rows::ShapeKind;
use pleochroma::optics::{
    absorption::AbsorptionTensor,
    materials::variants::{ColorVariant, TensorRank},
    zoning::ZoneShape,
};
use pleochroma_cut_core::{
    material::pattern::{
        ColorForm, ColorPattern, MAX_PATTERN_ZONES, OwnColor, PatternAxis, PatternColor,
        PatternZone, TemplateId, TypicalPattern, ZoneColor, rows_carry_tensor, template,
    },
    rough_plan::colour_fit::zones::ZoneParameter,
};

/// How far a duplicated zone is shifted so both stay visible, as a fraction of the stone width.
const DUPLICATE_SHIFT: f64 = 0.15;

/// `pattern` with its size rule set to `relative`, converting the geometry at `stone_width_mm`
/// so the stone looks the same; `None` when it already has that rule or the width is unusable.
pub(super) fn with_scale_mode(
    pattern: &ColorPattern,
    relative: bool,
    stone_width_mm: f64,
) -> Option<ColorPattern> {
    if pattern.relative_to_stone == relative {
        return None;
    }
    if relative {
        pattern.to_relative(stone_width_mm)
    } else {
        pattern.to_fixed_mm(stone_width_mm)
    }
}

/// The pattern of template `id`: the current axis and size rule are kept (a Uniform template also
/// keeps the current base color).
///
/// A template's colors are one-color seeds, so on a material of `rank` above isotropic every
/// one-color zone becomes the form that rank asks of a picked color ([`at_host_form`]): a template
/// never makes a "Legacy: No Pleochroism" zone on a pleochroic stone.
pub(super) fn with_template(
    current: &ColorPattern,
    id: TemplateId,
    stone_width_mm: f64,
    rank: TensorRank,
) -> ColorPattern {
    let mut next = template(id, current.axis);
    if id == TemplateId::Uniform {
        next.base.color = current.base.color.clone();
    }
    let next = at_host_form(next, rank);
    with_scale_mode(&next, current.relative_to_stone, stone_width_mm).unwrap_or(next)
}

/// `pattern` with every one-color zone turned into the form a material of `rank` asks.
///
/// The colors are kept (see `ColorPattern::set_color_form`), as for a new zone. A zone that
/// already has two or three colors, or holds a variant, stays as it is; an isotropic `rank`
/// changes nothing.
fn at_host_form(mut pattern: ColorPattern, rank: TensorRank) -> ColorPattern {
    let form = host_form(rank);
    if form == ColorForm::Single {
        return pattern;
    }
    for index in 0..=pattern.zones.len() {
        let current = pattern.zone(index).and_then(|zone| form_of(&zone.color));
        if current == Some(ColorForm::Single) {
            pattern.set_color_form(index, form);
        }
    }
    pattern.with_required_version()
}

/// The typical pattern `entry` of the stone's species, built on the stone's own color `own` and
/// the color `variants` of its material (zones whose color the species has a variant for take
/// it): the current axis and size rule are kept (as for a template).
pub(super) fn with_typical(
    current: &ColorPattern,
    entry: &TypicalPattern,
    own: &OwnColor,
    variants: &[ColorVariant],
    stone_width_mm: f64,
) -> ColorPattern {
    let next = (entry.build)(own, variants, current.axis);
    with_scale_mode(&next, current.relative_to_stone, stone_width_mm).unwrap_or(next)
}

/// `preset` as the pattern of the open material: the current axis and size rule are kept.
pub(super) fn with_preset(
    current: &ColorPattern,
    preset: &ColorPattern,
    stone_width_mm: f64,
) -> ColorPattern {
    let mut next = preset.clone();
    next.axis = current.axis;
    with_scale_mode(&next, current.relative_to_stone, stone_width_mm).unwrap_or(next)
}

/// One more zone of `kind`, with an unsolved color; returns the pattern and the new zone's index.
///
/// # Errors
///
/// The sentence for a pattern that already holds four zones.
pub(super) fn add_zone(
    pattern: &ColorPattern,
    kind: ShapeKind,
    stone_width_mm: f64,
) -> Result<(ColorPattern, usize), String> {
    if pattern.zones.len() >= MAX_PATTERN_ZONES {
        return Err(ADD_ZONE_HINT.to_owned());
    }
    let mut next = pattern.clone();
    next.template = None;
    let shape = unit_shape(
        default_shape(kind),
        pattern.relative_to_stone,
        stone_width_mm,
    );
    let color = PatternColor::Single(ZoneColor::unsolved(new_zone_lch(pattern.zones.len())));
    next.zones.push(PatternZone {
        name: new_zone_name(pattern),
        shape,
        color,
    });
    let index = next.zones.len();
    Ok((next, index))
}

/// A shape written for a unit-wide stone, in the unit of a pattern that is `relative` or fixed.
fn unit_shape(shape: ZoneShape, relative: bool, stone_width_mm: f64) -> ZoneShape {
    if relative {
        shape
    } else {
        scale_shape(&shape, stone_width_mm)
    }
}

/// A copy of zone `index` right after it, shifted a little; returns the pattern and the copy's
/// index.
///
/// # Errors
///
/// The sentence for the base zone (which has nothing to copy) or a full pattern.
pub(super) fn duplicate_zone(
    pattern: &ColorPattern,
    index: usize,
    stone_width_mm: f64,
) -> Result<(ColorPattern, usize), String> {
    if pattern.zones.len() >= MAX_PATTERN_ZONES {
        return Err(ADD_ZONE_HINT.to_owned());
    }
    let Some(original) = index.checked_sub(1).and_then(|i| pattern.zones.get(i)) else {
        return Err("The base zone cannot be copied.".to_owned());
    };
    let shift = DUPLICATE_SHIFT
        * if pattern.relative_to_stone {
            1.0
        } else {
            stone_width_mm
        };
    let mut copy = original.clone();
    copy.name = format!("{} Copy", original.name);
    copy.shape = shifted(&original.shape, shift);
    let mut next = pattern.clone();
    next.template = None;
    next.zones.insert(index, copy);
    Ok((next, index + 1))
}

/// `shape` moved by `shift` along the plane's own `x` axis (offsets of planes, axis points of rods
/// and wedges).
fn shifted(shape: &ZoneShape, shift: f64) -> ZoneShape {
    let mut out = shape.clone();
    match &mut out {
        ZoneShape::HalfSpace { offset, .. } => *offset += shift,
        ZoneShape::Slab {
            offset_min,
            offset_max,
            ..
        } => {
            *offset_min += shift;
            *offset_max += shift;
        }
        ZoneShape::CoaxialCylinder { axis_point, .. }
        | ZoneShape::CoaxialPrism { axis_point, .. }
        | ZoneShape::Sector { axis_point, .. } => axis_point.x += shift,
        ZoneShape::MeshShell { .. } => {}
    }
    out
}

/// The pattern without zone `index`; `None` for the base zone or a zone that does not exist.
pub(super) fn delete_zone(pattern: &ColorPattern, index: usize) -> Option<ColorPattern> {
    let at = index.checked_sub(1)?;
    if at >= pattern.zones.len() {
        return None;
    }
    let mut next = pattern.clone();
    next.template = None;
    next.zones.remove(at);
    Some(next)
}

/// Zone `index` moved `delta` places in the stacking order (negative: up); returns the pattern and
/// the zone's new index, or `None` when it cannot move that far.
pub(super) fn move_zone(
    pattern: &ColorPattern,
    index: usize,
    delta: i32,
) -> Option<(ColorPattern, usize)> {
    let from = index.checked_sub(1)?;
    if from >= pattern.zones.len() {
        return None;
    }
    let to = if delta < 0 {
        from.checked_sub(1)?
    } else {
        from + 1
    };
    if to >= pattern.zones.len() {
        return None;
    }
    let mut next = pattern.clone();
    next.zones.swap(from, to);
    Some((next, to + 1))
}

/// Zone `index` called `name` (trimmed); `None` when the name is blank, unchanged or the zone does
/// not exist.
pub(super) fn rename_zone(
    pattern: &ColorPattern,
    index: usize,
    name: &str,
) -> Option<ColorPattern> {
    let name = name.trim();
    if name.is_empty() || pattern.zone(index)?.name == name {
        return None;
    }
    let mut next = pattern.clone();
    name.clone_into(&mut next.zone_mut(index)?.name);
    Some(next)
}

/// Zone `index` with a different shape; its color stays. `None` for the base zone, a zone that
/// does not exist or the shape it already has.
pub(super) fn change_shape(
    pattern: &ColorPattern,
    index: usize,
    kind: ShapeKind,
    stone_width_mm: f64,
) -> Option<ColorPattern> {
    if index == 0 || pattern.zone(index).and_then(|zone| kind_of(&zone.shape)) == Some(kind) {
        return None;
    }
    let mut next = pattern.clone();
    next.template = None;
    next.zone_mut(index)?.shape = unit_shape(
        default_shape(kind),
        pattern.relative_to_stone,
        stone_width_mm,
    );
    Some(next)
}

/// Parameter `parameter` of zone `index` set to the stored value `value`; `None` when the zone has
/// no such parameter.
pub(super) fn with_param(
    pattern: &ColorPattern,
    index: usize,
    parameter: ZoneParameter,
    value: f64,
) -> Option<ColorPattern> {
    if index == 0 {
        return None;
    }
    let mut next = pattern.clone();
    set_param(&mut next.zone_mut(index)?.shape, parameter, value).then_some(next)
}

/// The Size slider of zone `index` set to `shown`; `None` when the zone has no Size slider.
pub(super) fn with_zone_size(
    pattern: &ColorPattern,
    index: usize,
    shown: f64,
) -> Option<ColorPattern> {
    if index == 0 {
        return None;
    }
    let shape = with_size(
        &pattern.zone(index)?.shape,
        shown,
        pattern.relative_to_stone,
    )?;
    let mut next = pattern.clone();
    next.zone_mut(index)?.shape = shape;
    Some(next)
}

/// Zone `index` turned into a picked color of `form` (its colors are kept, see
/// `ColorPattern::set_color_form`); `None` when it already has that form, holds a variant or does
/// not exist.
pub(super) fn with_color_form(
    pattern: &ColorPattern,
    index: usize,
    form: ColorForm,
) -> Option<ColorPattern> {
    if form_of(&pattern.zone(index)?.color)? == form {
        return None;
    }
    let mut next = pattern.clone();
    next.set_color_form(index, form)
        .then(|| next.with_required_version())
}

/// Zone `index` turned into a copy of `variant`: a snapshot of its per-millimeter rows, so the
/// pattern renders without the variant library. `None` when the zone already is that copy.
pub(super) fn with_variant(
    pattern: &ColorPattern,
    index: usize,
    variant: &ColorVariant,
) -> Option<ColorPattern> {
    let color = PatternColor::from_variant(variant);
    if pattern.zone(index)?.color == color {
        return None;
    }
    let mut next = pattern.clone();
    next.zone_mut(index)?.color = color;
    Some(next.with_required_version())
}

/// Zone `index` with the variant it holds turned into the picked color of the same rank, with the
/// variant's band rows (`lch_of` seeds the sliders). A variant the rows cannot carry becomes a
/// color of its per-direction `[L*, C*, h]` (`axes_of`) with no rows yet, which the window then
/// solves (see `PatternColor::to_custom`).
///
/// A one-color variant (a colorless one, say) on a pleochroic host `rank` becomes the host's own
/// form with that color in every slot, never a legacy one-color zone: the zone keeps its look and
/// gains the host's pleochroism to edit. `None` when the zone holds a picked color already or does
/// not exist.
pub(super) fn with_custom_color(
    pattern: &ColorPattern,
    index: usize,
    rank: TensorRank,
    lch_of: impl Fn(&[[f32; 3]]) -> [f64; 3],
    axes_of: impl Fn(&AbsorptionTensor) -> Vec<[f64; 3]>,
) -> Option<ColorPattern> {
    let zone = pattern.zone(index)?;
    if zone.color.is_custom() {
        return None;
    }
    let color = zone.color.to_custom(lch_of, axes_of);
    let mut next = pattern.clone();
    next.zone_mut(index)?.color = color;
    if form_of(&next.zone(index)?.color) == Some(ColorForm::Single) {
        next.set_color_form(index, host_form(rank));
    }
    Some(next.with_required_version())
}

/// Whether turning zone `index`'s variant into a picked color has to solve its bands again,
/// because the rows cannot carry them (`with_custom_color`).
pub(super) fn custom_color_resolves(pattern: &ColorPattern, index: usize) -> bool {
    matches!(
        pattern.zone(index).map(|zone| &zone.color),
        Some(PatternColor::Variant { absorption, .. })
            if !rows_carry_tensor(&absorption.to_tensor())
    )
}

/// The pattern with `axis`; `None` when it is the axis it has.
pub(super) fn with_axis(pattern: &ColorPattern, axis: PatternAxis) -> Option<ColorPattern> {
    (pattern.axis != axis).then(|| ColorPattern {
        axis,
        ..pattern.clone()
    })
}

/// The pattern with the stored boundary softness `softness`; `None` when it is the softness it
/// has.
pub(super) fn with_softness(pattern: &ColorPattern, softness: f32) -> Option<ColorPattern> {
    ((pattern.softness - softness).abs() > f32::EPSILON).then(|| ColorPattern {
        softness,
        ..pattern.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watermelon() -> ColorPattern {
        template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL)
    }

    #[test]
    #[expect(
        clippy::suboptimal_flops,
        reason = "test arithmetic written as the formula"
    )]
    fn a_template_keeps_the_axis_and_the_size_rule() {
        let current = ColorPattern {
            axis: PatternAxis::ACROSS,
            ..watermelon()
        };
        let fixed = with_scale_mode(&current, false, 8.0).unwrap();
        let next = with_template(&fixed, TemplateId::CoreAndRim, 8.0, TensorRank::Isotropic);
        assert_eq!(next.axis, PatternAxis::ACROSS);
        assert!(!next.relative_to_stone);
        assert_eq!(next.template, Some(TemplateId::CoreAndRim));
        assert!(
            matches!(next.zones[0].shape, ZoneShape::CoaxialCylinder { r_out, .. }
            if (r_out - 0.3 * 8.0).abs() < 1e-9)
        );
    }

    #[test]
    fn a_typical_pattern_keeps_the_axis_and_the_size_rule_and_is_built_on_the_own_color() {
        use pleochroma_cut_core::material::pattern::typical_patterns;
        let entry = &typical_patterns("Sapphire")[2];
        let own = [40.0, 45.0, 265.0];
        // The default: relative to the stone, the axis of the pattern it replaces.
        let current = ColorPattern {
            axis: PatternAxis::ACROSS,
            ..watermelon()
        };
        let next = with_typical(&current, entry, &OwnColor::single(own), &[], 8.0);
        assert_eq!(next.axis, PatternAxis::ACROSS);
        assert!(next.relative_to_stone);
        assert_eq!(next.template, Some(TemplateId::ColorBand));
        assert_eq!(next.base_lch(), own);
        // A pattern fixed in mm gets the geometry in mm.
        let fixed = with_scale_mode(&current, false, 8.0).unwrap();
        let next = with_typical(&fixed, entry, &OwnColor::single(own), &[], 8.0);
        assert!(!next.relative_to_stone);
        assert!(
            matches!(next.zones[0].shape, ZoneShape::Slab { offset_min, .. }
            if 0.06f64.mul_add(-8.0, offset_min).abs() < 1e-9)
        );
    }

    #[test]
    fn the_uniform_template_keeps_the_base_color() {
        let mut current = watermelon();
        current.base.color =
            PatternColor::Single(ZoneColor::solved([40.0, 20.0, 10.0], Vec::new()));
        let next = with_template(&current, TemplateId::Uniform, 7.0, TensorRank::Isotropic);
        assert_eq!(next.zones.len(), 0);
        assert_eq!(next.base.color, current.base.color);
    }

    #[test]
    fn a_pattern_holds_four_zones_besides_the_base() {
        let mut pattern = watermelon();
        for _ in 0..3 {
            pattern = add_zone(&pattern, ShapeKind::Slab, 7.0).unwrap().0;
        }
        assert_eq!(pattern.zones.len(), 4);
        assert_eq!(
            add_zone(&pattern, ShapeKind::Slab, 7.0).unwrap_err(),
            ADD_ZONE_HINT
        );
        assert!(duplicate_zone(&pattern, 1, 7.0).is_err());
    }

    #[test]
    fn a_new_zone_has_an_unsolved_color_and_a_fresh_name() {
        let (next, index) = add_zone(&watermelon(), ShapeKind::Sector, 7.0).unwrap();
        assert_eq!(index, 2);
        assert_eq!(next.zones[1].name, "Zone 1");
        assert_eq!(next.unsolved_colors().len(), 3);
    }

    #[test]
    fn a_duplicate_sits_after_the_original_and_is_shifted() {
        let pattern = add_zone(&watermelon(), ShapeKind::HalfSpace, 7.0)
            .unwrap()
            .0;
        let (next, index) = duplicate_zone(&pattern, 2, 7.0).unwrap();
        assert_eq!(index, 3);
        assert_eq!(next.zones[2].name, "Zone 1 Copy");
        assert!(matches!(next.zones[2].shape, ZoneShape::HalfSpace { offset, .. } if offset > 0.1));
        assert!(duplicate_zone(&pattern, 0, 7.0).is_err());
    }

    #[test]
    fn the_base_zone_cannot_be_deleted_or_moved() {
        let pattern = watermelon();
        assert!(delete_zone(&pattern, 0).is_none());
        assert!(move_zone(&pattern, 0, 1).is_none());
        assert_eq!(delete_zone(&pattern, 1).unwrap().zones.len(), 0);
    }

    #[test]
    fn moving_swaps_neighbors_and_stops_at_the_ends() {
        let pattern = add_zone(&watermelon(), ShapeKind::Slab, 7.0).unwrap().0;
        assert!(move_zone(&pattern, 1, -1).is_none());
        assert!(move_zone(&pattern, 2, 1).is_none());
        let (moved, index) = move_zone(&pattern, 2, -1).unwrap();
        assert_eq!(index, 1);
        assert_eq!(moved.zones[0].name, "Zone 1");
        assert_eq!(moved.zones[1].name, "Core");
    }

    #[test]
    fn a_new_shape_keeps_the_zone_color() {
        let pattern = watermelon();
        let next = change_shape(&pattern, 1, ShapeKind::Cylinder, 7.0).unwrap();
        assert_eq!(kind_of(&next.zones[0].shape), Some(ShapeKind::Cylinder));
        assert_eq!(next.zones[0].color, pattern.zones[0].color);
        assert!(change_shape(&next, 1, ShapeKind::Cylinder, 7.0).is_none());
        assert!(change_shape(&pattern, 0, ShapeKind::Slab, 7.0).is_none());
    }

    #[test]
    fn two_colors_start_as_copies_and_end_as_the_along_color() {
        let pattern = watermelon();
        let two = with_color_form(&pattern, 1, ColorForm::Dichroic).unwrap();
        assert!(matches!(two.zones[0].color, PatternColor::Dichroic { .. }));
        assert!(with_color_form(&two, 1, ColorForm::Dichroic).is_none());
        let one = with_color_form(&two, 1, ColorForm::Single).unwrap();
        assert_eq!(one.zones[0].color, pattern.zones[0].color);
        assert!(with_color_form(&pattern, 9, ColorForm::Single).is_none());
    }

    #[test]
    fn three_colors_make_the_pattern_a_version_two_one_and_going_back_undoes_it() {
        use pleochroma_cut_core::material::pattern::{
            PATTERN_BASE_FORMAT_VERSION, PATTERN_FORMAT_VERSION,
        };
        let pattern = watermelon();
        assert_eq!(pattern.version, PATTERN_BASE_FORMAT_VERSION);
        let three = with_color_form(&pattern, 1, ColorForm::Trichroic).unwrap();
        assert!(matches!(
            three.zones[0].color,
            PatternColor::Trichroic { .. }
        ));
        assert_eq!(three.version, PATTERN_FORMAT_VERSION);
        let back = with_color_form(&three, 1, ColorForm::Single).unwrap();
        assert_eq!(back.version, PATTERN_BASE_FORMAT_VERSION);
    }

    /// A pleochroic per-millimeter variant of Sapphire.
    fn sapphire_variant() -> ColorVariant {
        use pleochroma::optics::{
            absorption::{AbsorptionBand, AbsorptionTensor, BandShape},
            materials::{AbsorptionUnit, variants::VariantProvenance},
        };
        // An energy sigma of 900 cm^-1 (`AbsorptionBand::energy` takes an FWHM, whose sigma of
        // 382 cm^-1 the row form cannot carry), so the rows read back as written.
        let band = |center_nm, peak| AbsorptionBand {
            center_nm,
            width_nm: 900.0,
            peak,
            shape: BandShape::GaussianEnergy,
        };
        ColorVariant {
            key: "sapphire.test".to_owned(),
            label: "Test".to_owned(),
            material: "Sapphire".to_owned(),
            absorption: AbsorptionTensor::uniaxial(vec![band(600.0, 0.2)], vec![band(500.0, 0.1)]),
            unit: AbsorptionUnit::PerMm,
            reference_path_mm: 7.0,
            provenance: VariantProvenance::User,
        }
    }

    #[test]
    fn a_variant_is_copied_into_the_zone_and_turned_back_into_a_picked_color() {
        let variant = sapphire_variant();
        let pattern = watermelon();
        let next = with_variant(&pattern, 1, &variant).unwrap();
        assert_eq!(next.zones[0].color.variant_key(), Some("sapphire.test"));
        assert_eq!(next.zones[0].shape, pattern.zones[0].shape);
        assert_eq!(next.base, pattern.base, "only the picked zone changes");
        // The variant makes it a version 2 pattern; picking the same one again changes nothing.
        assert_eq!(
            next.version,
            pleochroma_cut_core::material::pattern::PATTERN_FORMAT_VERSION
        );
        assert!(with_variant(&next, 1, &variant).is_none());
        assert!(with_variant(&pattern, 9, &variant).is_none());

        // "Custom" turns it into Along and Across with the variant's rows, and back to version 1.
        assert!(!custom_color_resolves(&next, 1));
        let custom = with_custom_color(
            &next,
            1,
            TensorRank::Uniaxial,
            |_| [50.0, 20.0, 80.0],
            |_| Vec::new(),
        )
        .unwrap();
        let PatternColor::Dichroic { along, across } = &custom.zones[0].color else {
            panic!("a uniaxial variant becomes Along and Across");
        };
        assert_eq!(along.bands, vec![[600.0, 900.0, 0.2]]);
        assert_eq!(across.bands, vec![[500.0, 900.0, 0.1]]);
        assert_eq!(along.lch, [50.0, 20.0, 80.0]);
        assert_eq!(
            custom.version,
            pleochroma_cut_core::material::pattern::PATTERN_BASE_FORMAT_VERSION
        );
        assert!(
            with_custom_color(
                &custom,
                1,
                TensorRank::Uniaxial,
                |_| [0.0; 3],
                |_| Vec::new()
            )
            .is_none()
        );
        // Variant zones have no form to change.
        assert!(with_color_form(&next, 1, ColorForm::Dichroic).is_none());
    }

    #[test]
    fn a_colorless_variant_becomes_the_form_of_a_pleochroic_host() {
        let mut colorless = sapphire_variant();
        colorless.absorption = AbsorptionTensor::isotropic(Vec::new());
        let next = with_variant(&watermelon(), 1, &colorless).unwrap();
        let custom = with_custom_color(
            &next,
            1,
            TensorRank::Uniaxial,
            |_| [50.0, 0.0, 0.0],
            |_| Vec::new(),
        )
        .unwrap();
        assert!(
            matches!(custom.zones[0].color, PatternColor::Dichroic { .. }),
            "a one-color result is not left as a legacy zone on a uniaxial host"
        );
        // On an isotropic host it stays one color.
        let single = with_custom_color(
            &next,
            1,
            TensorRank::Isotropic,
            |_| [50.0, 0.0, 0.0],
            |_| Vec::new(),
        )
        .unwrap();
        assert!(matches!(single.zones[0].color, PatternColor::Single(_)));
    }

    #[test]
    fn a_variant_the_rows_cannot_carry_keeps_its_color_and_asks_for_a_solve() {
        use pleochroma::optics::absorption::{AbsorptionBand, BandShape};
        let mut narrow = sapphire_variant();
        let band = AbsorptionBand {
            center_nm: 428.3,
            width_nm: 301.4,
            peak: 0.02,
            shape: BandShape::GaussianEnergy,
        };
        narrow.absorption = AbsorptionTensor::uniaxial(vec![band], vec![band]);
        let next = with_variant(&watermelon(), 1, &narrow).unwrap();
        assert!(custom_color_resolves(&next, 1));
        let custom = with_custom_color(
            &next,
            1,
            TensorRank::Uniaxial,
            |_| unreachable!("no rows to read"),
            |_| vec![[40.0, 30.0, 250.0], [60.0, 20.0, 80.0]],
        )
        .unwrap();
        let PatternColor::Dichroic { along, across } = &custom.zones[0].color else {
            panic!("a uniaxial variant becomes Along and Across");
        };
        assert_eq!(along.lch, [40.0, 30.0, 250.0]);
        assert_eq!(across.lch, [60.0, 20.0, 80.0]);
        assert!(!along.is_solved() && !across.is_solved());
    }

    #[test]
    fn a_typical_pattern_takes_the_variants_it_is_given() {
        use pleochroma_cut_core::material::pattern::typical_patterns;
        let mut yellow = sapphire_variant();
        yellow.key = "sapphire.yellow".to_owned();
        let entry = &typical_patterns("Sapphire")[1];
        let current = watermelon();
        let own = OwnColor::single([40.0, 45.0, 265.0]);
        let next = with_typical(&current, entry, &own, &[yellow], 8.0);
        assert_eq!(next.template, Some(TemplateId::PartiColor));
        assert_eq!(next.base.color.variant_key(), Some("sapphire.yellow"));
        assert!(next.relative_to_stone);
    }

    #[test]
    fn a_blank_name_and_the_same_name_change_nothing() {
        let pattern = watermelon();
        assert!(rename_zone(&pattern, 1, "  ").is_none());
        assert!(rename_zone(&pattern, 1, "Core").is_none());
        assert_eq!(
            rename_zone(&pattern, 1, " Heart ").unwrap().zones[0].name,
            "Heart"
        );
    }

    #[test]
    fn a_size_edit_changes_the_zone_and_nothing_else() {
        let pattern = watermelon();
        let next = with_zone_size(&pattern, 1, 50.0).unwrap();
        assert!(
            matches!(next.zones[0].shape, ZoneShape::CoaxialPrism { r_out, .. }
            if (r_out - 0.5).abs() < 1e-9)
        );
        assert_eq!(next.base, pattern.base);
    }

    /// The color kinds of a pattern's zones, base first: 1 Single, 2 Dichroic, 3 Trichroic.
    fn colors_per_zone(pattern: &ColorPattern) -> Vec<usize> {
        std::iter::once(&pattern.base)
            .chain(&pattern.zones)
            .map(|zone| match zone.color {
                PatternColor::Single(_) => 1,
                PatternColor::Dichroic { .. } => 2,
                PatternColor::Trichroic { .. } => 3,
                PatternColor::Variant { .. } => 0,
            })
            .collect()
    }

    #[test]
    fn a_template_on_a_uniaxial_host_has_dichroic_zones_only() {
        let current = watermelon();
        for id in TemplateId::ALL {
            let next = with_template(&current, id, 7.0, TensorRank::Uniaxial);
            assert!(
                colors_per_zone(&next).iter().all(|kind| *kind == 2),
                "{}: {:?}",
                id.label(),
                colors_per_zone(&next)
            );
            // Two colors per zone are a first-release pattern.
            assert_eq!(
                next.version,
                pleochroma_cut_core::material::pattern::PATTERN_BASE_FORMAT_VERSION
            );
            assert!(next.unsolved_colors().len() > next.zones.len());
        }
        // The colors a one-color zone had are kept in both slots.
        let next = with_template(&current, TemplateId::Bicolor, 7.0, TensorRank::Uniaxial);
        let PatternColor::Dichroic { along, across } = &next.zones[0].color else {
            panic!("two colors");
        };
        assert_eq!(along, across);
    }

    #[test]
    fn a_template_on_a_biaxial_host_has_trichroic_zones_and_is_a_version_two_pattern() {
        let next = with_template(
            &watermelon(),
            TemplateId::Tricolor,
            7.0,
            TensorRank::Biaxial,
        );
        assert!(colors_per_zone(&next).iter().all(|kind| *kind == 3));
        assert_eq!(
            next.version,
            pleochroma_cut_core::material::pattern::PATTERN_FORMAT_VERSION
        );
    }

    #[test]
    fn a_template_on_an_isotropic_host_keeps_one_color_per_zone() {
        let next = with_template(
            &watermelon(),
            TemplateId::Tricolor,
            7.0,
            TensorRank::Isotropic,
        );
        assert!(colors_per_zone(&next).iter().all(|kind| *kind == 1));
    }

    #[test]
    fn a_uniform_template_keeps_a_two_color_base_and_raises_a_one_color_one() {
        let mut current = watermelon();
        current.base.color = PatternColor::Dichroic {
            along: ZoneColor::unsolved([40.0, 20.0, 10.0]),
            across: ZoneColor::unsolved([60.0, 20.0, 10.0]),
        };
        let next = with_template(&current, TemplateId::Uniform, 7.0, TensorRank::Uniaxial);
        assert_eq!(next.base.color, current.base.color);
        // A one-color base on a biaxial host becomes three colors; an isotropic host never changes.
        let single = watermelon();
        let raised = with_template(&single, TemplateId::Uniform, 7.0, TensorRank::Biaxial);
        assert_eq!(colors_per_zone(&raised), [3]);
        let kept = with_template(&single, TemplateId::Uniform, 7.0, TensorRank::Isotropic);
        assert_eq!(colors_per_zone(&kept), [1]);
    }
}
