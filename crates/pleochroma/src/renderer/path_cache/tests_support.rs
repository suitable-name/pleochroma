//! Shared fixtures of the path cache tests.

use glam::Vec3;

use crate::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{
        materials::GemMaterial,
        raytracer::{BACKDROP_GREY, Camera, LightingPreset, build_plane_soa},
    },
    renderer::frame_scene::FrameScene,
    simd::PlanesSoA32,
};

/// A stone, its intersection arena, its material and a camera.
pub(super) struct Rig {
    pub(super) planes: Vec<GpuFacetPlane>,
    pub(super) soa: PlanesSoA32,
    pub(super) material: GemMaterial,
    pub(super) camera: Camera,
}

impl Rig {
    pub(super) fn new(planes: Vec<GpuFacetPlane>, material: GemMaterial, camera: Camera) -> Self {
        let soa = build_plane_soa(&planes);
        Self {
            planes,
            soa,
            material,
            camera,
        }
    }

    /// The standard round brilliant seen from the default test pose.
    pub(super) fn brilliant(material: GemMaterial) -> Self {
        Self::new(
            StandardGemCuts::standard_round_brilliant(),
            material,
            Camera::new(0.35, 0.28, 5.0, 18.0),
        )
    }

    /// A unit cube seen from straight above, smaller than the frame.
    pub(super) fn cube_from_above(material: GemMaterial) -> Self {
        let planes: Vec<GpuFacetPlane> = [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ]
        .iter()
        .map(|&normal| GpuFacetPlane::new(normal, -1.0))
        .collect();
        Self::new(
            planes,
            material,
            Camera::new(0.0, std::f32::consts::FRAC_PI_2, 8.0, 24.0),
        )
    }

    /// A square frame of `size` pixels.
    pub(super) fn scene(&self, size: u32, max_bounces: u32) -> FrameScene<'_> {
        FrameScene {
            camera: &self.camera,
            width: size,
            height: size,
            planes: &self.planes,
            facet_finishes: &[],
            material: &self.material,
            max_bounces,
            environment: LightingPreset::LightTent
                .studio(1.0, 0.4, 0.35)
                .with_backdrop(BACKDROP_GREY),
        }
    }
}

/// A colorless, non-dispersive isotropic glass.
pub(super) fn glass() -> GemMaterial {
    GemMaterial::new_custom("cache test glass", 1.6, 0.0, 0.0, [0.0; 3])
}

/// A colorless, non-dispersive uniaxial crystal.
pub(super) fn uniaxial_glass() -> GemMaterial {
    GemMaterial::new_custom("cache test uniaxial", 1.76, 0.0, 0.008, [0.0; 3])
}

/// A colorless isotropic glass far above the dispersion gate.
pub(super) fn dispersive_glass() -> GemMaterial {
    GemMaterial::new_custom("cache test dispersive", 1.8, 0.12, 0.0, [0.0; 3])
}

/// The angle between two directions, accurate for nearly parallel ones.
pub(super) fn angle_between(a: Vec3, b: Vec3) -> f32 {
    a.cross(b).length().atan2(a.dot(b))
}
