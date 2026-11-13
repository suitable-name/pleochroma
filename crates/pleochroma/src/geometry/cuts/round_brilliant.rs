//! Azimuth conversion and the 57-facet Standard Round Brilliant reference cut.

use std::f32::consts::PI;

use glam::Vec3;

use super::StandardGemCuts;
use crate::geometry::plane::GpuFacetPlane;

/// Facet-index range of the girdle band in [`StandardGemCuts::standard_round_brilliant`].
///
/// These are the 16 true girdle facets, pushed as step 5 of that function (see its
/// "5. 16 Girdle Facets" comment). The table, star, crown main and upper girdle break
/// facets (33 planes) come before them. A frosted-girdle variant can mark these indices
/// `optics::raytracer::FacetFinish::Frosted`. A test in `girdle` checks the range against
/// the planes.
pub const STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS: std::ops::Range<usize> = 33..49;

impl StandardGemCuts {
    /// Converts an index-wheel position to an azimuth (radians about the vertical axis).
    ///
    /// `index` and `gear_reference_angle` are in index-wheel tooth units, as used by
    /// [`pleochroma_formats::asc::AscTier::indices`] and
    /// [`pleochroma_formats::asc::AscSchedule::gear_reference_angle`], not degrees. The
    /// `.asc` format does not document this. Real files keep the reference angle within
    /// `[0, gear_teeth]`, which only a tooth-count unit explains.
    ///
    /// The reference angle is a plain additive offset on the index. It rotates the whole
    /// azimuth frame and never flips index growth, so existing scenes keep their handedness.
    /// At zero the result equals `2*pi*index/gear_teeth` exactly.
    ///
    /// `gear_teeth` must be the absolute tooth count (see
    /// [`pleochroma_formats::asc::AscSchedule::gear_teeth_abs`]) and non-zero. Callers guard this.
    #[must_use]
    pub fn index_to_azimuth(index: f32, gear_teeth: f32, gear_reference_angle: f32) -> f32 {
        2.0 * PI * (index + gear_reference_angle) / gear_teeth
    }

    /// Generates exact 3D half-space planes for a 57-facet Standard Round Brilliant (SRB)
    /// cut with standard ideal proportions (crown height ~15% diameter, table ~56% width,
    /// pavilion depth ~43%).
    #[must_use]
    pub fn standard_round_brilliant() -> Vec<GpuFacetPlane> {
        let mut planes = Vec::with_capacity(57);
        let gear_teeth = 96.0f32;

        // 1. Crown Table Facet (Top flat facet at Y = +0.32)
        planes.push(GpuFacetPlane::new(Vec3::new(0.0, 1.0, 0.0), -0.32));

        // 2. 8 Crown Star Facets (15.0°, index 6, 18, 30, 42, 54, 66, 78, 90)
        let star_angle = 15.0f32.to_radians();
        for &g in &[6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0] {
            let phi = Self::index_to_azimuth(g, gear_teeth, 0.0);
            let n = Vec3::new(
                star_angle.sin() * phi.cos(),
                star_angle.cos(),
                star_angle.sin() * phi.sin(),
            );
            planes.push(GpuFacetPlane::new(n, -0.45));
        }

        // 3. 8 Crown Kite / Main Facets (34.5°, index 96, 12, 24, 36, 48, 60, 72, 84)
        let crown_main_angle = 34.5f32.to_radians();
        for &g in &[0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0] {
            let phi = Self::index_to_azimuth(g, gear_teeth, 0.0);
            let n = Vec3::new(
                crown_main_angle.sin() * phi.cos(),
                crown_main_angle.cos(),
                crown_main_angle.sin() * phi.sin(),
            );
            planes.push(GpuFacetPlane::new(n, -0.59));
        }

        // 4. 16 Upper Girdle Break Facets (41.0°, index 95, 1, 11, 13, 23, 25, 35, 37, 47, 49, 59, 61, 71, 73, 83, 85)
        let upper_girdle_angle = 41.0f32.to_radians();
        for &g in &[
            95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0,
            83.0, 85.0,
        ] {
            let phi = Self::index_to_azimuth(g, gear_teeth, 0.0);
            let n = Vec3::new(
                upper_girdle_angle.sin() * phi.cos(),
                upper_girdle_angle.cos(),
                upper_girdle_angle.sin() * phi.sin(),
            );
            planes.push(GpuFacetPlane::new(n, -0.67));
        }

        // 5. 16 Girdle Facets (90.0° vertical cylinder of radius 1.0)
        for i in 0..16 {
            let phi = 2.0 * PI * (i as f32) / 16.0;
            let n = Vec3::new(phi.cos(), 0.0, phi.sin());
            planes.push(GpuFacetPlane::new(n, -1.0));
        }

        // 6. 8 Pavilion Main Facets (-41.0°, index 96, 12, 24, 36, 48, 60, 72, 84)
        let pav_main_angle = 41.0f32.to_radians();
        for &g in &[0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0] {
            let phi = Self::index_to_azimuth(g, gear_teeth, 0.0);
            let n = Vec3::new(
                pav_main_angle.sin() * phi.cos(),
                -pav_main_angle.cos(),
                pav_main_angle.sin() * phi.sin(),
            );
            planes.push(GpuFacetPlane::new(n, -0.67));
        }

        // 7. 16 Lower Girdle Break Facets (-42.5°, index 95, 1, 11, 13, 23, 25, 35, 37, 47, 49, 59, 61, 71, 73, 83, 85)
        let lower_girdle_angle = 42.5f32.to_radians();
        for &g in &[
            95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0,
            83.0, 85.0,
        ] {
            let phi = Self::index_to_azimuth(g, gear_teeth, 0.0);
            let n = Vec3::new(
                lower_girdle_angle.sin() * phi.cos(),
                -lower_girdle_angle.cos(),
                lower_girdle_angle.sin() * phi.sin(),
            );
            planes.push(GpuFacetPlane::new(n, -0.68));
        }

        // 8. Culet (Bottom point at Y = -0.88)
        planes.push(GpuFacetPlane::new(Vec3::new(0.0, -1.0, 0.0), -0.88));

        planes
    }
}
