//! Routing for concave stones (a stone with tools): which scenes the GPU traces with its
//! tool-aware kernels and why it declines the rest.
//!
//! Split from the plane-only [`super::scene_routes_to_gpu`] on purpose. That function, and
//! every [`super::GpuSceneRef`] (planes only), serves callers that cannot pass a tool list.
//! For them a stone with tools must keep declining, or the GPU would render it without its
//! concave facets. Callers that pass tools to [`super::GpuBackend::try_accumulate_geom`] (or
//! `GpuBatchItem::tools`) use [`scene_routes_to_gpu_with_tools`] instead.

use std::fmt;

use super::{Fluorescence, GemMaterial, LightingPreset, StoneGeometry};
use crate::geometry::tool::GPU_MAX_TOOL_PRIMITIVES;

/// Why a scene is not offered to the GPU at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuRouteDecline {
    /// This pipeline cannot read the spectral map on the backdrop.
    StoneShadow,
    /// This pipeline cannot trace the glossy tray: its floor bounce needs the tray variant of
    /// the megakernel, which the event wavefront, the legacy wavefront and some devices lack.
    TrayReflection,
    /// The material has a feature only the CPU supports (`GemMaterial::gpu_supported`).
    Material,
    /// The stone has more concave tools than the GPU kernels trace
    /// ([`GPU_MAX_TOOL_PRIMITIVES`]); the CPU tracer renders it.
    TooManyTools {
        /// Tool count of the stone.
        tools: usize,
    },
    /// The material fluoresces; fluorescence is single-wavelength CPU transport.
    Fluorescence,
    /// A UV lamp lights the scene; those spectra exist only on the CPU.
    UvLamp,
}

impl GpuRouteDecline {
    /// Shared status sentence when this GPU route cannot preserve the stone shadow.
    /// A static value lets interfaces recognize and clear only this routing note.
    pub const STONE_SHADOW_MESSAGE: &'static str =
        "Stone shadow is traced on the CPU on this graphics card.";

    /// Shared status sentence when this GPU route cannot trace the tray reflection.
    /// A static value lets interfaces recognize and clear only this routing note.
    pub const TRAY_REFLECTION_MESSAGE: &'static str =
        "Tray reflection is traced on the CPU on this graphics card.";
}

impl fmt::Display for GpuRouteDecline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StoneShadow => f.write_str(Self::STONE_SHADOW_MESSAGE),
            Self::TrayReflection => f.write_str(Self::TRAY_REFLECTION_MESSAGE),
            Self::Material => f.write_str("The material has features only the CPU tracer supports"),
            Self::TooManyTools { tools } => write!(
                f,
                "The stone has {tools} concave tools; the GPU traces at most \
                 {GPU_MAX_TOOL_PRIMITIVES}"
            ),
            Self::Fluorescence => f.write_str("Fluorescence is traced on the CPU only"),
            Self::UvLamp => f.write_str("UV lamp lighting is traced on the CPU only"),
        }
    }
}

/// Returns the CPU routing reason when this device cannot sample the studio shadow map.
#[must_use]
pub const fn scene_decline_with_tray_shadow(
    environment: crate::optics::raytracer::EnvironmentSource<'_>,
    backend_supports: bool,
) -> Option<GpuRouteDecline> {
    if environment.tray_shadow().is_some() && !backend_supports {
        Some(GpuRouteDecline::StoneShadow)
    } else {
        None
    }
}

/// Returns the CPU routing reason when this device cannot trace the glossy tray.
///
/// Declines only a scene whose environment carries a mirror that is read (a studio rig with a
/// backdrop, see [`crate::optics::raytracer::EnvironmentSource::tray_mirror`]) on a backend
/// that cannot run the tray variant of the megakernel. `backend_supports` is
/// `GpuBackend::supports_tray_mirror()`.
#[must_use]
pub const fn scene_decline_with_tray_mirror(
    environment: crate::optics::raytracer::EnvironmentSource<'_>,
    backend_supports: bool,
) -> Option<GpuRouteDecline> {
    if environment.tray_mirror().is_some() && !backend_supports {
        Some(GpuRouteDecline::TrayReflection)
    } else {
        None
    }
}

/// The reason the GPU declines this scene, `None` when it is offered to the GPU.
///
/// The GPU traces concave stones with up to [`GPU_MAX_TOOL_PRIMITIVES`] tools, so unlike
/// [`super::scene_routes_to_gpu`] a stone with tools is routed to it.
///
/// A pure function of the scene so it is unit-tested without an adapter.
#[must_use]
pub fn scene_decline_with_tools(
    material: &GemMaterial,
    geom: StoneGeometry<'_>,
    fluorescence: &Fluorescence,
    lighting: LightingPreset,
) -> Option<GpuRouteDecline> {
    if !material.gpu_supported() {
        Some(GpuRouteDecline::Material)
    } else if geom.tools.len() > GPU_MAX_TOOL_PRIMITIVES {
        Some(GpuRouteDecline::TooManyTools {
            tools: geom.tools.len(),
        })
    } else if !fluorescence.is_empty() {
        Some(GpuRouteDecline::Fluorescence)
    } else if lighting.is_uv_lamp() {
        Some(GpuRouteDecline::UvLamp)
    } else {
        None
    }
}

/// Whether a scene may be dispatched to the GPU when the caller passes the stone's tools.
///
/// See the module docs. Same rules as [`super::scene_routes_to_gpu`] except that concave
/// stones with at most [`GPU_MAX_TOOL_PRIMITIVES`] tools are accepted.
#[must_use]
pub fn scene_routes_to_gpu_with_tools(
    material: &GemMaterial,
    geom: StoneGeometry<'_>,
    fluorescence: &Fluorescence,
    lighting: LightingPreset,
) -> bool {
    scene_decline_with_tools(material, geom, fluorescence, lighting).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        geometry::{cuts::StandardGemCuts, tool::ToolPrimitive},
        optics::{
            absorption::AbsorptionBand,
            fluorescence::{EmissionBand, FluorescentEmitter},
            raytracer::EnvironmentSource,
        },
    };
    use glam::Vec3;

    fn tools(n: usize) -> Vec<ToolPrimitive> {
        (0..n)
            .map(|i| {
                let x = 0.01 * i as f32;
                ToolPrimitive::ball(Vec3::new(x, 0.3, 0.0), 0.05)
            })
            .collect()
    }

    fn decline(
        planes: &[crate::geometry::GpuFacetPlane],
        tools: &[ToolPrimitive],
    ) -> Option<GpuRouteDecline> {
        scene_decline_with_tools(
            &GemMaterial::diamond(),
            StoneGeometry { planes, tools },
            Fluorescence::none(),
            LightingPreset::Daylight,
        )
    }

    /// The stone-shadow decline needs a map that is read (a studio rig with a tray) and a
    /// device that cannot sample it; every other combination is offered to the GPU.
    #[test]
    fn the_stone_shadow_declines_only_a_device_that_cannot_read_the_map() {
        use crate::{
            optics::raytracer::environment::{TrayShadowKey, TrayShadowMap},
            renderer::env_map::EnvironmentMap,
        };
        let map = TrayShadowMap::from_parts(
            -0.9,
            [0.0; 2],
            3.0,
            1,
            vec![0.5; 20],
            vec![[0.5; 3]],
            TrayShadowKey(1),
        );
        let tray = LightingPreset::IsoHemisphere
            .studio(1.0, 0.85, 0.95)
            .with_backdrop(0.9);
        let shadowed = tray.with_tray_shadow(Some(&map));
        assert_eq!(
            scene_decline_with_tray_shadow(shadowed, false),
            Some(GpuRouteDecline::StoneShadow)
        );
        assert_eq!(scene_decline_with_tray_shadow(shadowed, true), None);
        // No map attached.
        assert_eq!(scene_decline_with_tray_shadow(tray, false), None);
        // A map on a rig without a tray is never read.
        let as_lit = LightingPreset::IsoHemisphere
            .studio(1.0, 0.85, 0.95)
            .with_tray_shadow(Some(&map));
        assert_eq!(scene_decline_with_tray_shadow(as_lit, false), None);
        // An HDR map is its own backdrop and drops the shadow.
        let panorama = EnvironmentMap::uniform(4, 2, [0.5; 3]);
        let hdr = EnvironmentSource::hdr(&panorama).with_tray_shadow(Some(&map));
        assert_eq!(scene_decline_with_tray_shadow(hdr, false), None);
    }

    /// The tray-reflection decline needs a mirror that is read (a studio rig with a tray) and a
    /// device that cannot run the tray variant; every other combination is offered to the GPU.
    #[test]
    fn the_tray_reflection_declines_only_a_device_that_cannot_trace_it() {
        use crate::{
            optics::raytracer::environment::TrayMirror, renderer::env_map::EnvironmentMap,
        };
        let mirror = TrayMirror::new(-0.9, 0.04);
        assert!(mirror.is_some());
        let tray = LightingPreset::IsoHemisphere
            .studio(1.0, 0.85, 0.95)
            .with_backdrop(0.9);
        let mirrored = tray.with_tray_mirror(mirror);
        assert_eq!(
            scene_decline_with_tray_mirror(mirrored, false),
            Some(GpuRouteDecline::TrayReflection)
        );
        assert_eq!(scene_decline_with_tray_mirror(mirrored, true), None);
        // No mirror attached.
        assert_eq!(scene_decline_with_tray_mirror(tray, false), None);
        // A mirror on a rig without a tray is never read.
        let as_lit = LightingPreset::IsoHemisphere
            .studio(1.0, 0.85, 0.95)
            .with_tray_mirror(mirror);
        assert_eq!(scene_decline_with_tray_mirror(as_lit, false), None);
        // An HDR map is its own backdrop and drops the mirror.
        let panorama = EnvironmentMap::uniform(4, 2, [0.5; 3]);
        let hdr = EnvironmentSource::hdr(&panorama).with_tray_mirror(mirror);
        assert_eq!(scene_decline_with_tray_mirror(hdr, false), None);
        // The sentence is exact and distinct from the shadow's.
        assert_eq!(
            GpuRouteDecline::TrayReflection.to_string(),
            "Tray reflection is traced on the CPU on this graphics card."
        );
        assert_ne!(
            GpuRouteDecline::TrayReflection.to_string(),
            GpuRouteDecline::StoneShadow.to_string()
        );
    }

    /// Tools within the limit route to the GPU; one more than the limit declines with its own
    /// reason.
    #[test]
    fn concave_stones_route_to_the_gpu_up_to_the_tool_limit() {
        let planes = StandardGemCuts::standard_round_brilliant();
        assert!(
            GemMaterial::diamond().gpu_supported(),
            "test premise: a supported material"
        );
        assert_eq!(decline(&planes, &[]), None);
        assert_eq!(decline(&planes, &tools(1)), None);
        assert_eq!(decline(&planes, &tools(GPU_MAX_TOOL_PRIMITIVES)), None);
        let over = tools(GPU_MAX_TOOL_PRIMITIVES + 1);
        assert_eq!(
            decline(&planes, &over),
            Some(GpuRouteDecline::TooManyTools {
                tools: GPU_MAX_TOOL_PRIMITIVES + 1
            })
        );
        assert!(scene_routes_to_gpu_with_tools(
            &GemMaterial::diamond(),
            StoneGeometry {
                planes: &planes,
                tools: &tools(3)
            },
            Fluorescence::none(),
            LightingPreset::Daylight
        ));
        assert!(!scene_routes_to_gpu_with_tools(
            &GemMaterial::diamond(),
            StoneGeometry {
                planes: &planes,
                tools: &over
            },
            Fluorescence::none(),
            LightingPreset::Daylight
        ));
    }

    #[test]
    fn the_decline_reason_names_the_limit() {
        let text = GpuRouteDecline::TooManyTools { tools: 65 }.to_string();
        assert!(text.contains("65"), "{text}");
        assert!(
            text.contains(&GPU_MAX_TOOL_PRIMITIVES.to_string()),
            "{text}"
        );
    }

    /// The CPU's own bound is above the GPU's, so the GPU decline is reachable by a stone
    /// the CPU tracer accepts, and it always has a CPU fallback. Both are constants, so this
    /// is a compile-time check.
    const _: () = assert!(GPU_MAX_TOOL_PRIMITIVES <= crate::geometry::tool::MAX_TOOL_PRIMITIVES);

    /// Everything that kept a plane-only stone on the CPU still does with tools present.
    #[test]
    fn fluorescence_and_uv_lamps_still_decline_a_concave_stone() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let t = tools(2);
        let stone = StoneGeometry {
            planes: &planes,
            tools: &t,
        };
        let material = GemMaterial::diamond();
        let emitter = FluorescentEmitter {
            excitation: vec![AbsorptionBand::new(410.0, 20.0, 1.0)],
            emission: vec![EmissionBand::new(694.0, 2.0, 1.0)],
            quantum_yield: 0.9,
        };
        let fluorescent = Fluorescence::new(vec![emitter]);
        assert_eq!(
            scene_decline_with_tools(&material, stone, &fluorescent, LightingPreset::Daylight),
            Some(GpuRouteDecline::Fluorescence)
        );
        for preset in LightingPreset::ALL {
            assert_eq!(
                scene_routes_to_gpu_with_tools(&material, stone, Fluorescence::none(), preset),
                !preset.is_uv_lamp(),
                "{preset:?}"
            );
        }
    }

    /// For a plane-only stone the two routing functions agree on every preset. The plane-only
    /// one still declines any tools.
    #[test]
    fn the_legacy_routing_agrees_without_tools_and_still_declines_tools() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        for preset in LightingPreset::ALL {
            assert_eq!(
                super::super::scene_routes_to_gpu(
                    &material,
                    StoneGeometry::planes_only(&planes),
                    Fluorescence::none(),
                    preset
                ),
                scene_routes_to_gpu_with_tools(
                    &material,
                    StoneGeometry::planes_only(&planes),
                    Fluorescence::none(),
                    preset
                ),
                "{preset:?}"
            );
        }
        let t = tools(1);
        assert!(!super::super::scene_routes_to_gpu(
            &material,
            StoneGeometry {
                planes: &planes,
                tools: &t
            },
            Fluorescence::none(),
            LightingPreset::Daylight
        ));
    }
}
