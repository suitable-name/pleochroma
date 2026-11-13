//! Scene-keyed GPU upload and admission for the tray buffer at binding 190: the spectral
//! tray-shadow map and the Tray Reflection's header words.

use super::{GpuFrameRenderer, GpuPipelineKind, material_class, tray_shadow_source};
use crate::{
    optics::{
        raytracer::environment::{
            TrayMirror,
            tray_shadow::{TrayShadowKey, TrayShadowMap, pack_gpu_words_with_mirror},
        },
        tables::{OpticsTables, default_optics_tables},
    },
    renderer::gpu::{MEGAKERNEL_STORAGE_BUFFERS, compute},
};

/// Buffer bindings (uniform and storage) of the shipped megakernel, bindings 0 through 14.
const MEGAKERNEL_TOTAL_BUFFERS: u32 = 15;

/// What the uploaded tray buffer holds: the shadow map's identity and the mirror's two header
/// values as raw bits (plane height and `R0`), so the key is `Eq` and bit-exact.
type TrayBufferKey = (Option<TrayShadowKey>, Option<[u32; 2]>);

/// The cache key of a (shadow, mirror) pair.
fn tray_buffer_key(map: Option<&TrayShadowMap>, mirror: Option<TrayMirror>) -> TrayBufferKey {
    (
        map.map(TrayShadowMap::key),
        mirror.map(|m| [m.plane_y().to_bits(), m.reflectance().to_bits()]),
    )
}

/// Tray buffer retained until the map identity or the mirror changes.
#[derive(Default)]
pub(super) struct TrayShadowGpu {
    key: Option<TrayBufferKey>,
    buffer: Option<wgpu::Buffer>,
}

impl TrayShadowGpu {
    /// Uploads the shadow and mirror pair unless the cached key already matches. A mirror-only
    /// scene uploads a header-only buffer (shadow flag off, no texels) with header words 10
    /// through 12 set.
    fn ensure(
        &mut self,
        device: &wgpu::Device,
        map: Option<&TrayShadowMap>,
        mirror: Option<TrayMirror>,
    ) {
        let key = tray_buffer_key(map, mirror);
        if self.key == Some(key) && self.buffer.is_some() {
            return;
        }
        self.buffer = Some(compute::upload(
            device,
            "tray shadow and reflection",
            &pack_gpu_words_with_mirror(map, mirror),
            wgpu::BufferUsages::STORAGE,
        ));
        self.key = Some(key);
    }
}

impl GpuFrameRenderer {
    /// Whether an ordinary production pass can bind the shadow and optional optics tables.
    #[must_use]
    pub fn supports_tray_shadow(&self) -> bool {
        let tables = if default_optics_tables() == OpticsTables::On {
            material_class::OPTICS_TABLES
        } else {
            0
        };
        self.supports_tray_shadow_class(
            self.effective_class(material_class::TRAY_SHADOW | tables),
            false,
        )
    }

    /// Exact class admission; no shadow bit is discarded when resources are insufficient.
    pub(super) fn supports_tray_shadow_class(&self, class: u32, live: bool) -> bool {
        let needed = MEGAKERNEL_STORAGE_BUFFERS
            + 1
            + u32::from(live)
            + u32::from(material_class::uses_optics_tables(class));
        // The combined uniform + storage count of the kernel: its fifteen buffers plus the same
        // optional extras (`set_pipeline_kind` checks the wavefront's combined count the same way).
        let needed_total = MEGAKERNEL_TOTAL_BUFFERS
            + 1
            + u32::from(live)
            + u32::from(material_class::uses_optics_tables(class));
        let limits = self.ctx.device.limits();
        self.pipeline_kind == GpuPipelineKind::Megakernel
            && limits.max_storage_buffers_per_shader_stage >= needed
            && limits.max_buffers_and_acceleration_structures_per_shader_stage >= needed_total
            && tray_shadow_source::megakernel_source_for(class).is_some()
            && tray_shadow_source::megakernel_live_source_for(class).is_some()
    }

    /// Whether an ordinary production pass can bind the Tray Reflection and the optics tables.
    ///
    /// Delegates to [`Self::supports_tray_shadow`]: the two features ride the same tray variant of
    /// the megakernel and the same binding 190, and the class admission ignores which tray bit is
    /// set, so a renderer that can run one can run the other.
    #[must_use]
    pub fn supports_tray_mirror(&self) -> bool {
        self.supports_tray_shadow()
    }

    /// Makes the current map and mirror available to subsequent ordinary and live dispatches.
    pub(super) fn prepare_tray_shadow(
        &mut self,
        map: Option<&TrayShadowMap>,
        mirror: Option<TrayMirror>,
    ) {
        if map.is_some() || mirror.is_some() {
            self.tray_shadow.ensure(&self.ctx.device, map, mirror);
        }
    }

    /// Buffer bound only by a pipeline carrying the scene's shadow or mirror bit.
    pub(super) const fn tray_shadow_for(&self, class: u32) -> Option<&wgpu::Buffer> {
        if material_class::uses_tray_variant(class) {
            self.tray_shadow.buffer.as_ref()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        geometry::cuts::StandardGemCuts,
        optics::{
            materials::GemMaterial,
            raytracer::{Camera, EnvironmentSource, LightingPreset},
        },
        renderer::{
            frame_scene::FrameScene,
            gpu::{
                environment_check::build_tray_shadow_check_map,
                event_wavefront::{EventWavefrontRenderer, EwError},
                merge_tests,
            },
        },
    };
    use glam::Vec3;

    /// Asserts that the legacy wavefront and the event wavefront decline a scene whose rig gets
    /// the tray feature `attach` adds, and that `supports` says the same, writing nothing.
    fn wavefront_declines<'t>(
        name: &str,
        attach: impl FnOnce(EnvironmentSource<'static>) -> EnvironmentSource<'t>,
        supports: fn(&GpuFrameRenderer) -> bool,
    ) {
        let Some(mut renderer) = merge_tests::renderer_or_skip(name) else {
            return;
        };
        let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        let rig = LightingPreset::LightTent
            .studio(1.0, 0.0, 0.5)
            .with_backdrop(0.23);
        let scene = FrameScene {
            camera: &camera,
            width: 2,
            height: 2,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 4,
            environment: attach(rig),
        };
        let mut output = vec![Vec3::ZERO; 4];
        // `set_pipeline_kind` stays on the megakernel on a device too small for the wavefront.
        renderer.set_pipeline_kind(GpuPipelineKind::Wavefront);
        if renderer.pipeline_kind() == GpuPipelineKind::Wavefront {
            assert!(!supports(&renderer));
            assert!(matches!(
                renderer.accumulate(&scene, 0, 1, &mut output),
                Err(super::super::GpuFrameError::TrayVariant)
            ));
            assert_eq!(output, vec![Vec3::ZERO; 4]);
        }
        let Some(ctx) = merge_tests::context_or_skip(name) else {
            return;
        };
        let mut event = EventWavefrontRenderer::new(ctx);
        let result = pollster::block_on(event.accumulate(&scene, 0, 1, &mut output));
        assert!(matches!(result, Err(EwError::TrayVariant)));
        assert_eq!(output, vec![Vec3::ZERO; 4]);
    }

    #[test]
    fn legacy_and_event_wavefront_decline_shadowed_scenes() {
        let shadow = build_tray_shadow_check_map();
        wavefront_declines(
            "legacy_and_event_wavefront_decline_shadowed_scenes",
            |rig| rig.with_tray_shadow(Some(&shadow)),
            GpuFrameRenderer::supports_tray_shadow,
        );
    }

    #[test]
    fn legacy_and_event_wavefront_decline_mirrored_scenes() {
        let mirror = TrayMirror::new(-1.0, 0.04).expect("valid mirror");
        wavefront_declines(
            "legacy_and_event_wavefront_decline_mirrored_scenes",
            |rig| rig.with_tray_mirror(Some(mirror)),
            GpuFrameRenderer::supports_tray_mirror,
        );
    }

    #[test]
    fn the_tray_buffer_key_follows_the_map_and_the_mirror_bits() {
        let a = TrayMirror::new(-1.0, 0.04);
        let b = TrayMirror::new(-1.0, 0.05);
        let c = TrayMirror::new(-1.5, 0.04);
        assert_eq!(tray_buffer_key(None, a), tray_buffer_key(None, a));
        assert_ne!(tray_buffer_key(None, a), tray_buffer_key(None, b));
        assert_ne!(tray_buffer_key(None, a), tray_buffer_key(None, c));
        assert_ne!(tray_buffer_key(None, a), tray_buffer_key(None, None));
    }
}
