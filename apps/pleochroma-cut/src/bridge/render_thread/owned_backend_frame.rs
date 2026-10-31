//! Owned scene snapshot for bounded background preparation and calibration.
use super::{
    BackendFrame, Camera, EnvironmentSource, FacetFinish, Fluorescence, GemMaterial, GpuFacetPlane,
    SurfaceScene, ToolPrimitive,
};
use pleochroma::{
    optics::raytracer::surface::{EdgeAbrasion, FacetSurface, FacetingMachine},
    renderer::env_map::EnvironmentMap,
};
use std::sync::Arc;

pub(in crate::bridge::render_thread) struct OwnedFrame {
    camera: Camera,
    planes: Vec<GpuFacetPlane>,
    tools: Vec<ToolPrimitive>,
    finishes: Vec<FacetFinish>,
    material: GemMaterial,
    fluorescence: Fluorescence,
    surfaces: Vec<FacetSurface>,
    machine: FacetingMachine,
    edge: EdgeAbrasion,
    environment: EnvironmentSource<'static>,
    hdr: Option<Arc<EnvironmentMap>>,
    dimensions: (u32, u32),
    pose: (f32, f32, f32),
    max_bounces: u32,
    sample_offset: u32,
    spp: u32,
}

impl OwnedFrame {
    pub(in crate::bridge::render_thread) fn new(
        frame: &BackendFrame<'_>,
        shared_hdr: Option<&Arc<EnvironmentMap>>,
    ) -> Self {
        let (environment, hdr) = match frame.environment {
            EnvironmentSource::HdrMap(map) => (
                pleochroma::optics::raytracer::LightingPreset::Daylight.studio(1.0, 0.0, 0.0),
                Some(
                    shared_hdr
                        .filter(|shared| std::ptr::eq(shared.as_ref(), map))
                        .cloned()
                        .unwrap_or_else(|| Arc::new(map.clone())),
                ),
            ),
            EnvironmentSource::Studio {
                preset,
                exposure,
                light_yaw,
                light_pitch,
                backdrop,
                surface_glare,
                head_shadow_deg,
            } => (
                EnvironmentSource::Studio {
                    preset,
                    exposure,
                    light_yaw,
                    light_pitch,
                    backdrop,
                    surface_glare,
                    head_shadow_deg,
                },
                None,
            ),
        };
        Self {
            camera: Camera {
                origin: frame.camera.origin,
                forward: frame.camera.forward,
                right: frame.camera.right,
                up: frame.camera.up,
                fov_tan: frame.camera.fov_tan,
            },
            planes: frame.planes.to_vec(),
            tools: frame.tools.to_vec(),
            finishes: frame.facet_finishes.to_vec(),
            material: frame.material.clone(),
            fluorescence: frame.fluorescence.clone(),
            surfaces: frame.surface.facet_surfaces.to_vec(),
            machine: frame.surface.machine,
            edge: frame.surface.edge,
            environment,
            hdr,
            dimensions: (frame.width, frame.height),
            pose: (frame.yaw, frame.pitch, frame.distance),
            max_bounces: frame.max_bounces,
            sample_offset: frame.sample_offset,
            spp: frame.spp,
        }
    }

    pub(in crate::bridge::render_thread) fn as_frame(&self) -> BackendFrame<'_> {
        BackendFrame {
            width: self.dimensions.0,
            height: self.dimensions.1,
            yaw: self.pose.0,
            pitch: self.pose.1,
            distance: self.pose.2,
            camera: &self.camera,
            planes: &self.planes,
            tools: &self.tools,
            fluorescence: &self.fluorescence,
            facet_finishes: &self.finishes,
            surface: SurfaceScene {
                facet_surfaces: &self.surfaces,
                machine: self.machine,
                edge: self.edge,
            },
            material: &self.material,
            max_bounces: self.max_bounces,
            environment: self
                .hdr
                .as_deref()
                .map_or(self.environment, EnvironmentSource::HdrMap),
            spp: self.spp,
            sample_offset: self.sample_offset,
        }
    }
}
