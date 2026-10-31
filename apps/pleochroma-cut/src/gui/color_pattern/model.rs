//! The pure half of the Color Pattern editor: how a zone is named and worded, the parameter and
//! Size rows of the selected zone, the axis choices and the numbers behind the sliders.
//!
//! No Slint here, so the rules are unit-testable without a window.
//!
//! # Units
//!
//! A pattern that "Scales With Stone Size" stores its zone lengths for a stone one unit wide; the
//! sliders show them as a percentage of the width (`0.35` is `35 % of width`). A pattern that is
//! "Fixed in mm" stores millimeters and the sliders show millimeters. Angles are stored in radians
//! and shown in degrees.

use crate::gui::rough_colour::wizard::zone_rows::{ShapeKind, scalar_parameters};
use glam::DVec3;
use pleochroma::optics::zoning::ZoneShape;
use pleochroma_cut_core::{
    material::pattern::{ColorPattern, PatternAxis, PatternZone},
    rough_plan::colour_fit::zones::{ZoneParameter, parameter_value},
};
use std::f64::consts::FRAC_PI_2;

/// The sentence under a disabled "Add Zone".
pub(super) const ADD_ZONE_HINT: &str = "A pattern holds at most 4 zones besides the base.";

/// A slider value of `1` (fully soft) is a boundary this wide, as a fraction of the stone width.
pub(super) const MAX_SOFTNESS_FRACTION: f64 = 0.5;

/// The sliders' index of the Custom axis; then come "Follows Crystal Axis" and, only for zones
/// from the Rough Color wizard (they keep their own frame), "Imported".
pub(super) const AXIS_CUSTOM: i32 = 3;
/// The index of the "Follows Crystal Axis" entry: the zones turn with the stone's Crystal Axis.
pub(super) const AXIS_FOLLOWS: i32 = 4;
/// The index of the "Imported" axis entry, present only for a pattern with an imported frame.
pub(super) const AXIS_IMPORTED: i32 = 5;

/// Colors a new zone starts with, in turn (`[L*, C*, h]`). They are solved in the background.
const NEW_ZONE_LCH: [[f64; 3]; 4] = [
    [50.0, 40.0, 310.0],
    [85.0, 70.0, 90.0],
    [60.0, 35.0, 0.0],
    [55.0, 35.0, 140.0],
];

/// A narrowing of `f64` to the `f32` the Slint properties hold.
#[expect(
    clippy::cast_possible_truncation,
    reason = "Slint model properties are f32; the values are small slider readings"
)]
pub(super) const fn f32_of(value: f64) -> f32 {
    value as f32
}

/// The `f64` value of a Slint `f32` property.
pub(super) fn f64_of(value: f32) -> f64 {
    f64::from(value)
}

/// What a stored length is multiplied by to get the number a slider shows.
pub(super) const fn length_scale(relative: bool) -> f64 {
    if relative { 100.0 } else { 1.0 }
}

/// The unit a length slider shows.
pub(super) const fn length_unit(relative: bool) -> &'static str {
    if relative { "% of width" } else { "mm" }
}

/// The shape kind of `shape`; `None` for a freeform shell, which the editor cannot make or edit.
pub(super) const fn kind_of(shape: &ZoneShape) -> Option<ShapeKind> {
    match shape {
        ZoneShape::HalfSpace { .. } => Some(ShapeKind::HalfSpace),
        ZoneShape::Slab { .. } => Some(ShapeKind::Slab),
        ZoneShape::CoaxialCylinder { .. } => Some(ShapeKind::Cylinder),
        ZoneShape::CoaxialPrism { .. } => Some(ShapeKind::Prism),
        ZoneShape::Sector { .. } => Some(ShapeKind::Sector),
        ZoneShape::MeshShell { .. } => None,
    }
}

/// The shape a new zone of `kind` starts with, for a stone one unit wide, in the pattern frame
/// (the pattern axis is `+Y`): a plane through the axis, a stripe, a solid rod, a triangular rod
/// and a quarter-turn wedge, all about the axis.
pub(super) const fn default_shape(kind: ShapeKind) -> ZoneShape {
    match kind {
        ShapeKind::HalfSpace => ZoneShape::HalfSpace {
            normal: DVec3::X,
            offset: 0.0,
        },
        ShapeKind::Slab => ZoneShape::Slab {
            normal: DVec3::X,
            offset_min: -0.125,
            offset_max: 0.125,
        },
        ShapeKind::Cylinder => ZoneShape::CoaxialCylinder {
            axis_point: DVec3::ZERO,
            axis_dir: DVec3::Y,
            r_in: 0.0,
            r_out: 0.25,
        },
        ShapeKind::Prism => ZoneShape::CoaxialPrism {
            axis_point: DVec3::ZERO,
            axis_dir: DVec3::Y,
            n_sides: 3,
            r_in: 0.0,
            r_out: 0.25,
            phase: 0.0,
        },
        ShapeKind::Sector => ZoneShape::Sector {
            axis_point: DVec3::ZERO,
            axis_dir: DVec3::Y,
            angle_from: 0.0,
            angle_to: FRAC_PI_2,
        },
    }
}

/// `shape` with every length multiplied by `factor` (angles and directions stay).
pub(super) fn scale_shape(shape: &ZoneShape, factor: f64) -> ZoneShape {
    let mut out = shape.clone();
    match &mut out {
        ZoneShape::HalfSpace { offset, .. } => *offset *= factor,
        ZoneShape::Slab {
            offset_min,
            offset_max,
            ..
        } => {
            *offset_min *= factor;
            *offset_max *= factor;
        }
        ZoneShape::CoaxialCylinder {
            axis_point,
            r_in,
            r_out,
            ..
        }
        | ZoneShape::CoaxialPrism {
            axis_point,
            r_in,
            r_out,
            ..
        } => {
            *axis_point *= factor;
            *r_in *= factor;
            *r_out *= factor;
        }
        ZoneShape::Sector { axis_point, .. } => *axis_point *= factor,
        ZoneShape::MeshShell { vertices, .. } => {
            for vertex in vertices {
                for coordinate in vertex {
                    *coordinate *= factor;
                }
            }
        }
    }
    out
}

/// Sets one scalar parameter of `shape` to the stored value `value`; `false` when the shape has no
/// such parameter or the value cannot be stored (a prism's side count is rounded and kept
/// between 3 and 12).
pub(super) fn set_param(shape: &mut ZoneShape, parameter: ZoneParameter, value: f64) -> bool {
    if !value.is_finite() {
        return false;
    }
    match (shape, parameter) {
        (ZoneShape::HalfSpace { offset, .. }, ZoneParameter::Offset) => *offset = value,
        (ZoneShape::Slab { offset_min, .. }, ZoneParameter::OffsetMin) => *offset_min = value,
        (ZoneShape::Slab { offset_max, .. }, ZoneParameter::OffsetMax) => *offset_max = value,
        (
            ZoneShape::CoaxialCylinder { r_in, .. } | ZoneShape::CoaxialPrism { r_in, .. },
            ZoneParameter::RIn,
        ) => *r_in = value,
        (
            ZoneShape::CoaxialCylinder { r_out, .. } | ZoneShape::CoaxialPrism { r_out, .. },
            ZoneParameter::ROut,
        ) => *r_out = value,
        (ZoneShape::CoaxialPrism { phase, .. }, ZoneParameter::Phase) => *phase = value,
        (ZoneShape::CoaxialPrism { n_sides, .. }, ZoneParameter::NSides) => {
            let rounded = value.round();
            let Some(sides) = (3_u32..=12).find(|n| (f64::from(*n) - rounded).abs() < 0.5) else {
                return false;
            };
            *n_sides = sides;
        }
        (ZoneShape::Sector { angle_from, .. }, ZoneParameter::AngleFrom) => *angle_from = value,
        (ZoneShape::Sector { angle_to, .. }, ZoneParameter::AngleTo) => *angle_to = value,
        _ => return false,
    }
    true
}

/// A stored value as a slider shows it.
pub(super) fn to_display(parameter: ZoneParameter, stored: f64, relative: bool) -> f64 {
    match parameter {
        ZoneParameter::Offset
        | ZoneParameter::OffsetMin
        | ZoneParameter::OffsetMax
        | ZoneParameter::RIn
        | ZoneParameter::ROut => stored * length_scale(relative),
        ZoneParameter::Phase | ZoneParameter::AngleFrom | ZoneParameter::AngleTo => {
            stored.to_degrees()
        }
        _ => stored,
    }
}

/// The value to store for a slider reading: the inverse of [`to_display`].
pub(super) fn from_display(parameter: ZoneParameter, shown: f64, relative: bool) -> f64 {
    match parameter {
        ZoneParameter::Offset
        | ZoneParameter::OffsetMin
        | ZoneParameter::OffsetMax
        | ZoneParameter::RIn
        | ZoneParameter::ROut => shown / length_scale(relative),
        ZoneParameter::Phase | ZoneParameter::AngleFrom | ZoneParameter::AngleTo => {
            shown.to_radians()
        }
        _ => shown,
    }
}

/// One parameter slider of the selected zone (Advanced mode), in the numbers the slider shows.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ParamSpec {
    /// Which parameter of the shape it edits.
    pub parameter: ZoneParameter,
    /// The caption, Title Case.
    pub label: &'static str,
    /// The current value.
    pub value: f64,
    /// The slider's lowest value.
    pub min: f64,
    /// The slider's highest value.
    pub max: f64,
    /// The slider's step.
    pub step: f64,
    /// The unit after the number ("% of width", "mm", a degree sign, or nothing).
    pub unit: &'static str,
    /// What the number means, one plain sentence.
    pub hint: &'static str,
}

impl ParamSpec {
    /// The value as read out, without the unit.
    pub(super) fn text(&self) -> String {
        match self.parameter {
            ZoneParameter::NSides
            | ZoneParameter::Phase
            | ZoneParameter::AngleFrom
            | ZoneParameter::AngleTo => format!("{:.0}", self.value),
            _ => format!("{:.1}", self.value),
        }
    }
}

/// The reach of an offset or radius in slider units: the whole stone width or twice the width in
/// millimeters.
fn reach(relative: bool, width_mm: f64) -> f64 {
    if relative {
        100.0
    } else {
        2.0 * width_mm.max(1.0)
    }
}

/// The slider step of a length.
const fn length_step(relative: bool) -> f64 {
    if relative { 1.0 } else { 0.05 }
}

/// The parameter sliders of `zone`, in display order (empty for the base zone and a freeform
/// shell).
pub(super) fn param_specs(zone: &PatternZone, relative: bool, width_mm: f64) -> Vec<ParamSpec> {
    let reach = reach(relative, width_mm);
    let step = length_step(relative);
    let unit = length_unit(relative);
    let is_prism = matches!(zone.shape, ZoneShape::CoaxialPrism { .. });
    scalar_parameters(&zone.shape)
        .into_iter()
        .filter_map(|parameter| {
            let stored = parameter_value(&zone.shape, parameter)?;
            let value = to_display(parameter, stored, relative);
            let length = |label, min, hint| ParamSpec {
                parameter,
                label,
                value,
                min,
                max: reach,
                step,
                unit,
                hint,
            };
            Some(match parameter {
                ZoneParameter::Offset => length(
                    "Position",
                    -reach,
                    "Where the plane cuts the stone, measured from the center along its direction.",
                ),
                ZoneParameter::OffsetMin => length(
                    "Lower Edge",
                    -reach,
                    "The lower edge of the stripe, measured from the center.",
                ),
                ZoneParameter::OffsetMax => length(
                    "Upper Edge",
                    -reach,
                    "The upper edge of the stripe, measured from the center.",
                ),
                ZoneParameter::RIn => length(
                    if is_prism {
                        "Inner Apothem"
                    } else {
                        "Inner Radius"
                    },
                    0.0,
                    "Leave this at 0 for a solid zone. Above 0 the zone is a hollow wall.",
                ),
                ZoneParameter::ROut => length(
                    if is_prism {
                        "Outer Apothem"
                    } else {
                        "Outer Radius"
                    },
                    0.0,
                    "How far the zone reaches from its axis.",
                ),
                ZoneParameter::Phase => ParamSpec {
                    parameter,
                    label: "Rotation",
                    value,
                    min: -180.0,
                    max: 180.0,
                    step: 1.0,
                    unit: "\u{b0}",
                    hint: "Turns the prism about its axis.",
                },
                ZoneParameter::NSides => ParamSpec {
                    parameter,
                    label: "Sides",
                    value,
                    min: 3.0,
                    max: 12.0,
                    step: 1.0,
                    unit: "",
                    hint: "How many sides the prism has.",
                },
                ZoneParameter::AngleFrom => ParamSpec {
                    parameter,
                    label: "Start Angle",
                    value,
                    min: -360.0,
                    max: 360.0,
                    step: 1.0,
                    unit: "\u{b0}",
                    hint: "Where the wedge starts, turning counter-clockwise.",
                },
                ZoneParameter::AngleTo => ParamSpec {
                    parameter,
                    label: "End Angle",
                    value,
                    min: -360.0,
                    max: 720.0,
                    step: 1.0,
                    unit: "\u{b0}",
                    hint: "Where the wedge ends. It must be after the start and at most a full turn later.",
                },
                _ => return None,
            })
        })
        .collect()
}

/// The Simple mode's one Size slider of a zone.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SizeSpec {
    /// The current value.
    pub value: f64,
    /// The slider's lowest value.
    pub min: f64,
    /// The slider's highest value.
    pub max: f64,
    /// The slider's step.
    pub step: f64,
    /// The value as read out, with its unit.
    pub text: String,
}

/// The Size slider of `shape`, or `None` for a shape that has none.
///
/// The size is the outer radius (or apothem) of a rod, the thickness of a stripe, the position of a
/// plane and the angle span of a wedge.
pub(super) fn size_spec(shape: &ZoneShape, relative: bool, width_mm: f64) -> Option<SizeSpec> {
    let scale = length_scale(relative);
    let reach = reach(relative, width_mm);
    let step = if relative { 1.0 } else { 0.1 };
    let min_length = if relative { 2.0 } else { 0.1 };
    let length_text = |value: f64| {
        if relative {
            format!("{value:.0} % of width")
        } else {
            format!("{value:.1} mm")
        }
    };
    match shape {
        ZoneShape::CoaxialCylinder { r_out, .. } | ZoneShape::CoaxialPrism { r_out, .. } => {
            let value = r_out * scale;
            Some(SizeSpec {
                value,
                min: min_length,
                max: reach,
                step,
                text: length_text(value),
            })
        }
        ZoneShape::Slab {
            offset_min,
            offset_max,
            ..
        } => {
            let value = (offset_max - offset_min) * scale;
            Some(SizeSpec {
                value,
                min: min_length,
                max: reach,
                step,
                text: length_text(value),
            })
        }
        ZoneShape::HalfSpace { offset, .. } => {
            let value = offset * scale;
            Some(SizeSpec {
                value,
                min: -reach / 2.0,
                max: reach / 2.0,
                step,
                text: length_text(value),
            })
        }
        ZoneShape::Sector {
            angle_from,
            angle_to,
            ..
        } => {
            let value = (angle_to - angle_from).to_degrees();
            Some(SizeSpec {
                value,
                min: 5.0,
                max: 360.0,
                step: 1.0,
                text: format!("{value:.0}\u{b0}"),
            })
        }
        ZoneShape::MeshShell { .. } => None,
    }
}

/// `shape` with its Size slider set to `shown`, or `None` for a shape that has none.
pub(super) fn with_size(shape: &ZoneShape, shown: f64, relative: bool) -> Option<ZoneShape> {
    if !shown.is_finite() {
        return None;
    }
    let scale = length_scale(relative);
    let mut out = shape.clone();
    match &mut out {
        ZoneShape::CoaxialCylinder { r_in, r_out, .. }
        | ZoneShape::CoaxialPrism { r_in, r_out, .. } => {
            *r_out = shown / scale;
            if *r_in >= *r_out {
                *r_in = 0.0;
            }
        }
        ZoneShape::Slab {
            offset_min,
            offset_max,
            ..
        } => {
            let centre = f64::midpoint(*offset_min, *offset_max);
            let half = shown / scale / 2.0;
            *offset_min = centre - half;
            *offset_max = centre + half;
        }
        ZoneShape::HalfSpace { offset, .. } => *offset = shown / scale,
        ZoneShape::Sector {
            angle_from,
            angle_to,
            ..
        } => *angle_to = *angle_from + shown.to_radians(),
        ZoneShape::MeshShell { .. } => return None,
    }
    Some(out)
}

/// The title of zone `index` in the list: "Base" or "Zone 1".
pub(super) fn zone_title(index: usize) -> String {
    if index == 0 {
        "Base".to_owned()
    } else {
        format!("Zone {index}")
    }
}

/// The shape column of the zone list.
pub(super) fn shape_label(index: usize, zone: &PatternZone) -> String {
    if index == 0 {
        return "Everywhere else".to_owned();
    }
    match &zone.shape {
        ZoneShape::CoaxialPrism { n_sides, .. } => format!("{n_sides}-Sided Prism"),
        ZoneShape::MeshShell { .. } => "Freeform".to_owned(),
        other => kind_of(other).map_or_else(String::new, |kind| kind.name().to_owned()),
    }
}

/// One short line of the numbers of a zone, for the list.
pub(super) fn zone_summary(index: usize, zone: &PatternZone, relative: bool) -> String {
    if index == 0 {
        return "Fills what the other zones do not cover".to_owned();
    }
    let scale = length_scale(relative);
    let length = |stored: f64| {
        if relative {
            format!("{:.0} %", stored * scale)
        } else {
            format!("{:.1} mm", stored * scale)
        }
    };
    match &zone.shape {
        ZoneShape::HalfSpace { offset, .. } => format!("At {}", length(*offset)),
        ZoneShape::Slab {
            offset_min,
            offset_max,
            ..
        } => format!("From {} to {}", length(*offset_min), length(*offset_max)),
        ZoneShape::CoaxialCylinder { r_in, r_out, .. } => {
            if *r_in > 0.0 {
                format!("Wall from {} to {}", length(*r_in), length(*r_out))
            } else {
                format!("Radius {}", length(*r_out))
            }
        }
        ZoneShape::CoaxialPrism { r_in, r_out, .. } => {
            if *r_in > 0.0 {
                format!("Wall from {} to {}", length(*r_in), length(*r_out))
            } else {
                format!("Apothem {}", length(*r_out))
            }
        }
        ZoneShape::Sector {
            angle_from,
            angle_to,
            ..
        } => format!("{:.0}\u{b0} wedge", (angle_to - angle_from).to_degrees()),
        ZoneShape::MeshShell { .. } => "A freeform shell".to_owned(),
    }
}

/// The text of the pattern in the material dialog: "Watermelon · 2 zones · scales with stone size".
pub(super) fn pattern_summary(pattern: &ColorPattern) -> String {
    let name = pattern
        .template
        .map_or("Custom Pattern", |template| template.label());
    let count = pattern.zones.len() + 1;
    format!(
        "{name} \u{b7} {count} zone{} \u{b7} {}",
        if count == 1 { "" } else { "s" },
        if pattern.relative_to_stone {
            "scales with stone size"
        } else {
            "fixed in mm"
        }
    )
}

/// A zone name not yet used in `pattern`: "Zone 1", "Zone 2" and so on.
pub(super) fn new_zone_name(pattern: &ColorPattern) -> String {
    // At most `zones + 1` names are taken, so one of the first `zones + 2` candidates is free.
    let candidates = pattern.zones.len() + 2;
    (1..=candidates)
        .map(|n| format!("Zone {n}"))
        .find(|name| {
            name != &pattern.base.name && pattern.zones.iter().all(|zone| &zone.name != name)
        })
        .unwrap_or_else(|| format!("Zone {candidates}"))
}

/// The color a new zone starts with: the `n`-th of the palette, wrapping.
pub(super) const fn new_zone_lch(n: usize) -> [f64; 3] {
    NEW_ZONE_LCH[n % NEW_ZONE_LCH.len()]
}

// ---- axis ------------------------------------------------------------------------------------

/// The entries of the axis menu: the three presets, Custom, "Follows Crystal Axis", and "Imported"
/// for an imported frame.
pub(super) fn axis_labels(axis: &PatternAxis) -> Vec<&'static str> {
    let mut labels = vec![
        "Table Normal",
        "Across",
        "Across Turned",
        "Custom",
        "Follows Crystal Axis",
    ];
    if matches!(axis, PatternAxis::Frame { .. }) {
        labels.push("Imported");
    }
    labels
}

/// Whether two angles are the same to a millionth of a radian.
fn same_angle(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

/// The axis menu's index of `axis`: 0 Table Normal, 1 Across, 2 Across Turned, 3 Custom, 4
/// Follows Crystal Axis, 5 Imported.
pub(super) fn axis_index(axis: &PatternAxis) -> i32 {
    match *axis {
        PatternAxis::Frame { .. } => AXIS_IMPORTED,
        PatternAxis::FollowsCrystalAxis => AXIS_FOLLOWS,
        PatternAxis::Fixed { tilt_rad, turn_rad } => {
            let is = |preset: PatternAxis| {
                matches!(preset, PatternAxis::Fixed { tilt_rad: t, turn_rad: u }
                    if same_angle(t, tilt_rad) && same_angle(u, turn_rad))
            };
            if is(PatternAxis::TABLE_NORMAL) {
                0
            } else if is(PatternAxis::ACROSS) {
                1
            } else if is(PatternAxis::ACROSS_TURNED) {
                2
            } else {
                AXIS_CUSTOM
            }
        }
    }
}

/// The axis of menu entry `index` that is picked outright (0 to 2, and 4 "Follows Crystal Axis");
/// `None` for Custom, Imported and anything else.
pub(super) const fn axis_for_index(index: i32) -> Option<PatternAxis> {
    match index {
        0 => Some(PatternAxis::TABLE_NORMAL),
        1 => Some(PatternAxis::ACROSS),
        2 => Some(PatternAxis::ACROSS_TURNED),
        AXIS_FOLLOWS => Some(PatternAxis::FollowsCrystalAxis),
        _ => None,
    }
}

/// Tilt and turn of `axis` in degrees (zero for an imported frame, and for a pattern that
/// follows the Crystal Axis: its tilt and turn are the stone's).
pub(super) const fn tilt_turn_deg(axis: &PatternAxis) -> (f64, f64) {
    match *axis {
        PatternAxis::Fixed { tilt_rad, turn_rad } => (tilt_rad.to_degrees(), turn_rad.to_degrees()),
        PatternAxis::Frame { .. } | PatternAxis::FollowsCrystalAxis => (0.0, 0.0),
    }
}

/// The axis with the given tilt and turn in degrees.
pub(super) const fn axis_from_deg(tilt_deg: f64, turn_deg: f64) -> PatternAxis {
    PatternAxis::Fixed {
        tilt_rad: tilt_deg.to_radians(),
        turn_rad: turn_deg.to_radians(),
    }
}

// ---- softness --------------------------------------------------------------------------------

/// The boundary slider (0 sharp to 1 soft) for a stored softness.
pub(super) fn softness_to_slider(softness: f32, relative: bool, width_mm: f64) -> f32 {
    let full = MAX_SOFTNESS_FRACTION * if relative { 1.0 } else { width_mm.max(1.0) };
    f32_of((f64::from(softness) / full).clamp(0.0, 1.0))
}

/// The stored softness for a boundary slider reading.
pub(super) fn softness_from_slider(slider: f32, relative: bool, width_mm: f64) -> f32 {
    let full = MAX_SOFTNESS_FRACTION * if relative { 1.0 } else { width_mm.max(1.0) };
    f32_of(f64::from(slider).clamp(0.0, 1.0) * full)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::material::pattern::{TemplateId, template};

    #[test]
    fn every_default_shape_is_a_zone_the_editor_can_size() {
        for kind in ShapeKind::ALL {
            let shape = default_shape(kind);
            assert_eq!(kind_of(&shape), Some(kind));
            assert!(size_spec(&shape, true, 7.0).is_some(), "{kind:?}");
        }
    }

    #[test]
    fn the_size_slider_round_trips_in_percent_and_in_millimeters() {
        let shape = default_shape(ShapeKind::Cylinder);
        let spec = size_spec(&shape, true, 7.0).unwrap();
        assert!((spec.value - 25.0).abs() < 1e-9);
        assert_eq!(spec.text, "25 % of width");
        let bigger = with_size(&shape, 40.0, true).unwrap();
        assert!(
            matches!(bigger, ZoneShape::CoaxialCylinder { r_out, .. } if (r_out - 0.4).abs() < 1e-9)
        );
        let millimeters = with_size(&shape, 3.0, false).unwrap();
        assert!(
            matches!(millimeters, ZoneShape::CoaxialCylinder { r_out, .. } if (r_out - 3.0).abs() < 1e-9)
        );
    }

    #[test]
    fn a_stripe_keeps_its_center_when_it_is_resized() {
        let shape = ZoneShape::Slab {
            normal: DVec3::X,
            offset_min: 0.1,
            offset_max: 0.3,
        };
        let wider = with_size(&shape, 40.0, true).unwrap();
        assert!(
            matches!(wider, ZoneShape::Slab { offset_min, offset_max, .. }
            if (offset_min - 0.0).abs() < 1e-9 && (offset_max - 0.4).abs() < 1e-9)
        );
    }

    #[test]
    fn a_prism_takes_between_three_and_twelve_sides() {
        let mut shape = default_shape(ShapeKind::Prism);
        assert!(set_param(&mut shape, ZoneParameter::NSides, 6.2));
        assert!(matches!(shape, ZoneShape::CoaxialPrism { n_sides: 6, .. }));
        assert!(!set_param(&mut shape, ZoneParameter::NSides, 2.0));
        assert!(!set_param(&mut shape, ZoneParameter::NSides, 13.0));
        assert!(!set_param(&mut shape, ZoneParameter::NSides, f64::NAN));
        assert!(matches!(shape, ZoneShape::CoaxialPrism { n_sides: 6, .. }));
    }

    #[test]
    fn display_values_convert_both_ways() {
        for (parameter, stored) in [
            (ZoneParameter::ROut, 0.35),
            (ZoneParameter::AngleTo, 2.0),
            (ZoneParameter::NSides, 5.0),
        ] {
            for relative in [true, false] {
                let shown = to_display(parameter, stored, relative);
                assert!((from_display(parameter, shown, relative) - stored).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn the_axis_menu_knows_the_presets() {
        assert_eq!(axis_index(&PatternAxis::TABLE_NORMAL), 0);
        assert_eq!(axis_index(&PatternAxis::ACROSS), 1);
        assert_eq!(axis_index(&PatternAxis::ACROSS_TURNED), 2);
        assert_eq!(axis_index(&axis_from_deg(30.0, 10.0)), AXIS_CUSTOM);
        let frame = PatternAxis::Frame {
            rotation: [0.0, 0.0, 0.0, 1.0],
            translation: [1.0, 0.0, 0.0],
        };
        assert_eq!(axis_index(&frame), AXIS_IMPORTED);
        assert_eq!(axis_labels(&frame).len(), 6);
        assert_eq!(axis_labels(&PatternAxis::ACROSS).len(), 5);
    }

    #[test]
    fn the_axis_menu_offers_follows_crystal_axis() {
        let labels = axis_labels(&PatternAxis::TABLE_NORMAL);
        let at = usize::try_from(AXIS_FOLLOWS).expect("a small index");
        assert_eq!(labels[at], "Follows Crystal Axis");
        assert_eq!(
            axis_index(&PatternAxis::FollowsCrystalAxis),
            AXIS_FOLLOWS,
            "the mode lights up its own entry"
        );
        assert_eq!(
            axis_for_index(AXIS_FOLLOWS),
            Some(PatternAxis::FollowsCrystalAxis)
        );
        assert_eq!(axis_for_index(AXIS_CUSTOM), None, "Custom is not a preset");
        assert_eq!(axis_for_index(AXIS_IMPORTED), None);
        assert_eq!(tilt_turn_deg(&PatternAxis::FollowsCrystalAxis), (0.0, 0.0));
    }

    #[test]
    fn softness_maps_between_the_slider_and_the_stored_value() {
        let stored = softness_from_slider(0.5, true, 7.0);
        assert!((stored - 0.25).abs() < 1e-6);
        assert!((softness_to_slider(stored, true, 7.0) - 0.5).abs() < 1e-6);
        let millimeters = softness_from_slider(1.0, false, 8.0);
        assert!((millimeters - 4.0).abs() < 1e-6);
    }

    #[test]
    fn the_summary_names_the_template_the_zones_and_the_size_rule() {
        let pattern = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
        assert_eq!(
            pattern_summary(&pattern),
            "Watermelon \u{b7} 2 zones \u{b7} scales with stone size"
        );
        assert_eq!(new_zone_name(&pattern), "Zone 1");
    }
}
