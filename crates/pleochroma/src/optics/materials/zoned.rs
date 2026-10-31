//! Zoned-absorption helpers of [`GemMaterial`]: the builder that installs
//! zones and the per-zone swatches the inspector shows.

use super::{AbsorptionUnit, GemMaterial};
use crate::{
    color::{
        BodyColor, Illuminant, body_colors,
        principal_axes::{PrincipalFrame, face_up_body_color},
    },
    optics::zoning::ZonedAbsorption,
    render_setup::{MODEL_UNIT_FACE_UP_PATH, effective_stone_width_mm},
};

/// The girdle width in model units the UNSIZED swatch rule assumes ([`zone_swatches`],
/// [`zone_swatches_face_up`]) to turn a stone width in millimetres into millimetres per model
/// unit.
///
/// The built-in cuts have a girdle radius of order one model unit (`GemMaterial::scattering_sigma_s`'s "useful range" note), i.e. a width of about 2.
/// Only the unsized swatches depend on it; a caller that holds a sized material (the render
/// path) passes its millimetres per model unit to [`zone_swatches_face_up_at_scale`] instead.
pub const ZONE_SWATCH_FALLBACK_MODEL_WIDTH: f64 = 2.0;

impl GemMaterial {
    /// Installs `zoning` on this material.
    ///
    /// The material becomes [`AbsorptionUnit::PerMm`] and its plain
    /// [`absorption`](Self::absorption) is set to the base zone's tensor, so a build or consumer
    /// that ignores zones still shows the base zone (the adopt path relies on this). The tracer
    /// itself ignores `absorption` for a zoned material and uses the zones.
    #[must_use]
    pub fn with_zoning(mut self, zoning: ZonedAbsorption) -> Self {
        self.absorption = zoning.base.tensor.clone();
        self.absorption_unit = AbsorptionUnit::PerMm;
        self.zoning = Some(zoning);
        self
    }
}

/// The sRGB swatches (each channel in `[0, 1]`, D65) of a zoned material's zones at the face-up
/// path length: the base zone first, then the shaped zones in order. Empty for a material
/// without zones.
///
/// This is the UNSIZED rule: the face-up path is `MODEL_UNIT_FACE_UP_PATH` model units times
/// `effective_stone_width_mm(width_mm) / ZONE_SWATCH_FALLBACK_MODEL_WIDTH` millimetres per model
/// unit (`width_mm` of `0.0` means the 7 mm default). The material's
/// `absorption_path_scale` is never read, so a stored scale (even exactly `1.0`) cannot change
/// the result. A caller that holds a sized material uses [`zone_swatches_at_scale`].
///
/// Callers: the rough-colour material zones (`gui::rough_colour::material_zones`).
#[must_use]
pub fn zone_swatches(material: &GemMaterial, width_mm: f32) -> Vec<[f32; 3]> {
    zone_swatches_over_path(material, unsized_path_mm(width_mm))
}

/// [`zone_swatches`] for a caller that holds a SIZED material.
///
/// `mm_per_unit` millimetres per
/// model unit (the render path's `absorption_path_scale`) replaces the width rule, so the path is
/// `MODEL_UNIT_FACE_UP_PATH * mm_per_unit` millimetres. A `mm_per_unit` that is not finite and
/// positive falls back to the unsized rule at the 7 mm default width.
#[must_use]
pub fn zone_swatches_at_scale(material: &GemMaterial, mm_per_unit: f32) -> Vec<[f32; 3]> {
    zone_swatches_over_path(material, scaled_path_mm(mm_per_unit))
}

/// [`zone_swatches`] with the crystal's orientation.
///
/// Each zone's pleochroic tensor is read
/// along the single face-up wave normal `k = -Y` through the material's own principal frame
/// (`material.c_axis`, or its biaxial indicatrix: the resolved orientation, see
/// `render_setup::material_for_stone_oriented`), the way the face-up tone reads a stone. A
/// zone whose tensor is not pleochroic gives exactly the [`zone_swatches`] color.
///
/// The path is the UNSIZED rule of [`zone_swatches`] (the stored `absorption_path_scale` is not
/// read). Callers: `view_color.rs` in the cut app's render thread and
/// `crystal_axis/presets.rs`. A caller that holds a sized material uses
/// [`zone_swatches_face_up_at_scale`].
#[must_use]
pub fn zone_swatches_face_up(material: &GemMaterial, width_mm: f32) -> Vec<[f32; 3]> {
    zone_swatches_face_up_over_path(material, unsized_path_mm(width_mm))
}

/// [`zone_swatches_face_up`] for a caller that holds a SIZED material.
///
/// The path is
/// `MODEL_UNIT_FACE_UP_PATH * mm_per_unit` millimetres, `mm_per_unit` being the material's
/// millimetres per model unit (its `absorption_path_scale` once sized). A `mm_per_unit` that is
/// not finite and positive falls back to the unsized rule at the 7 mm default width. A scale of
/// exactly `1.0` is a size like any other.
#[must_use]
pub fn zone_swatches_face_up_at_scale(material: &GemMaterial, mm_per_unit: f32) -> Vec<[f32; 3]> {
    zone_swatches_face_up_over_path(material, scaled_path_mm(mm_per_unit))
}

/// The face-up swatch color of `material`'s own absorption over `path`.
///
/// `path` is in the unit the tensor is in (model units for a [`AbsorptionUnit::ModelUnit`] material, millimetres for a
/// [`AbsorptionUnit::PerMm`] one, the swatch's existing path rule) with the crystal oriented
/// as the material says: the single wave normal `k = -Y`, unpolarised absorption through the
/// principal frame of `material.c_axis` / its biaxial indicatrix.
///
/// A material whose absorption is not pleochroic gives exactly
/// `body_colors(&material.absorption, path, ill).unpolarised`. This is the core function the
/// app's swatches call with the stone's orientation (`render_body_color`, `material_zones`).
#[must_use]
pub fn face_up_swatch(material: &GemMaterial, path: f64, ill: Illuminant) -> BodyColor {
    let frame = PrincipalFrame::for_pleochroic(material);
    face_up_body_color(&material.absorption, frame.as_ref(), path, ill)
}

/// The zone swatches over `path_mm` millimetres, orientation-free.
fn zone_swatches_over_path(material: &GemMaterial, path_mm: f64) -> Vec<[f32; 3]> {
    let Some(zoning) = material.zoning.as_ref() else {
        return Vec::new();
    };
    std::iter::once(&zoning.base)
        .chain(zoning.zones.iter().map(|zone| &zone.absorption))
        .map(|zone| {
            body_colors(&zone.tensor, path_mm, Illuminant::D65)
                .unpolarised
                .srgb
                .map(|v| f32::from(v) / 255.0)
        })
        .collect()
}

/// The zone swatches over `path_mm` millimetres, through the material's principal frame.
fn zone_swatches_face_up_over_path(material: &GemMaterial, path_mm: f64) -> Vec<[f32; 3]> {
    let Some(zoning) = material.zoning.as_ref() else {
        return Vec::new();
    };
    let frame = PrincipalFrame::of_material(material);
    std::iter::once(&zoning.base)
        .chain(zoning.zones.iter().map(|zone| &zone.absorption))
        .map(|zone| {
            face_up_body_color(&zone.tensor, Some(&frame), path_mm, Illuminant::D65)
                .srgb
                .map(|v| f32::from(v) / 255.0)
        })
        .collect()
}

/// The unsized path: millimetres per model unit from the stone width.
fn unsized_path_mm(width_mm: f32) -> f64 {
    let mm_per_unit =
        f64::from(effective_stone_width_mm(width_mm)) / ZONE_SWATCH_FALLBACK_MODEL_WIDTH;
    f64::from(MODEL_UNIT_FACE_UP_PATH) * mm_per_unit
}

/// The sized path: the face-up path over `mm_per_unit` millimetres per model unit, or the unsized
/// rule at the 7 mm default when `mm_per_unit` is not finite and positive.
fn scaled_path_mm(mm_per_unit: f32) -> f64 {
    if mm_per_unit.is_finite() && mm_per_unit > 0.0 {
        f64::from(MODEL_UNIT_FACE_UP_PATH) * f64::from(mm_per_unit)
    } else {
        unsized_path_mm(0.0)
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        zoning::ZoneAbsorption,
    };

    fn tourmaline_like(c_axis: Vec3) -> GemMaterial {
        let mut material = GemMaterial::sapphire();
        material.absorption = AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::new(580.0, 60.0, 0.5)],
            vec![AbsorptionBand::new(450.0, 50.0, 0.5)],
        );
        material.c_axis = c_axis;
        material
    }

    #[test]
    fn the_material_swatch_follows_the_resolved_axis() {
        let along = face_up_swatch(&tourmaline_like(Vec3::Y), 2.5, Illuminant::D65);
        let across = face_up_swatch(&tourmaline_like(Vec3::X), 2.5, Illuminant::D65);
        assert_ne!(along.srgb, across.srgb);
    }

    #[test]
    fn a_colorless_material_swatch_is_the_old_one() {
        let material = GemMaterial::diamond().with_body_color([0.3, 0.6, 0.2]);
        let new = face_up_swatch(&material, 2.5, Illuminant::D65);
        let old = body_colors(&material.absorption, 2.5, Illuminant::D65).unpolarised;
        assert_eq!(new.lab.map(f64::to_bits), old.lab.map(f64::to_bits));
        assert_eq!(new.srgb, old.srgb);
    }

    #[test]
    fn unzoned_materials_have_no_zone_swatches() {
        assert_eq!(
            zone_swatches_face_up(&GemMaterial::diamond(), 7.0),
            [] as [[f32; 3]; 0]
        );
    }

    /// A zoned `PerMm` material with one visible band in its base zone and the stored scale
    /// `scale`.
    fn zoned_probe(scale: f32) -> GemMaterial {
        let tensor = AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::new(580.0, 60.0, 0.2)],
            vec![AbsorptionBand::new(580.0, 60.0, 0.2)],
        );
        let mut material =
            GemMaterial::sapphire().with_zoning(ZonedAbsorption::new(ZoneAbsorption {
                tensor,
                unit: AbsorptionUnit::PerMm,
            }));
        material.absorption_path_scale = scale;
        material
    }

    fn base_color_at_path(material: &GemMaterial, path_mm: f64) -> BodyColor {
        let frame = PrincipalFrame::of_material(material);
        let zoning = material.zoning.as_ref().expect("zoned probe");
        face_up_body_color(&zoning.base.tensor, Some(&frame), path_mm, Illuminant::D65)
    }

    fn lab_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y) * (x - y))
            .sum::<f64>()
            .sqrt()
    }

    #[test]
    fn a_scale_of_exactly_one_is_a_size_not_a_sentinel() {
        let material = zoned_probe(1.0);
        let sized = zone_swatches_face_up_at_scale(&material, 1.0);
        let expected = base_color_at_path(&material, f64::from(MODEL_UNIT_FACE_UP_PATH))
            .srgb
            .map(|v| f32::from(v) / 255.0);
        assert_eq!(sized, vec![expected]);
        // The unsized rule at 7 mm is 3.5 mm per unit, a different path and so a different color.
        assert_ne!(sized, zone_swatches_face_up(&material, 7.0));
        // The orientation-free twin follows the same size.
        let plain = zone_swatches_at_scale(&material, 1.0);
        let plain_expected = body_colors(
            &material.zoning.as_ref().expect("zoned").base.tensor,
            f64::from(MODEL_UNIT_FACE_UP_PATH),
            Illuminant::D65,
        )
        .unpolarised
        .srgb
        .map(|v| f32::from(v) / 255.0);
        assert_eq!(plain, vec![plain_expected]);
    }

    #[test]
    fn continuity_around_one() {
        let material = zoned_probe(1.0);
        let scales = [0.999_f32, 1.0, 1.001];
        let labs = scales.map(|s| {
            base_color_at_path(&material, f64::from(MODEL_UNIT_FACE_UP_PATH) * f64::from(s)).lab
        });
        for pair in labs.windows(2) {
            let distance = lab_distance(pair[0], pair[1]);
            assert!(distance < 0.5, "neighbouring scales are {distance} apart");
        }
        // The swatches themselves are monotone per channel and within one 8-bit step.
        let swatches = scales.map(|s| zone_swatches_face_up_at_scale(&material, s)[0]);
        for channel in 0..3 {
            let [a, b, c] = swatches.map(|swatch| swatch[channel]);
            assert!(
                (a <= b && b <= c) || (a >= b && b >= c),
                "channel {channel} is not monotone: {a} {b} {c}"
            );
            assert!(
                (a - c).abs() <= 1.5 / 255.0,
                "channel {channel} jumps: {a} {c}"
            );
        }
    }

    #[test]
    fn the_unsized_rule_ignores_the_stored_scale() {
        let stored = zoned_probe(24.5);
        let one = zoned_probe(1.0);
        assert_eq!(
            zone_swatches_face_up(&stored, 7.0),
            zone_swatches_face_up(&one, 7.0)
        );
        assert_eq!(zone_swatches(&stored, 7.0), zone_swatches(&one, 7.0));
        // A scale that is not a size falls back to the unsized 7 mm rule.
        assert_eq!(
            zone_swatches_face_up_at_scale(&one, f32::NAN),
            zone_swatches_face_up(&one, 0.0)
        );
        assert_eq!(
            zone_swatches_face_up_at_scale(&one, 0.0),
            zone_swatches_face_up(&one, 7.0)
        );
    }
}
