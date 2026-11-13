//! The megakernel's per-dispatch uniform: [`GpuTransportParams`] plus its
//! [`studio_model`]/[`transport_env_mode`] discriminants.

use core::mem::offset_of;

use crate::{
    optics::raytracer::{DEFAULT_HEAD_SHADOW_COSINES, Estimator, TentParams},
    renderer::env_map::EnvRotation,
};

/// Per-dispatch kernel parameters for `shaders/spectral_transport.wgsl`'s
/// `transport_main` entry point.
///
/// `env_mode` selects the environment model (see [`transport_env_mode`]). `sample_offset` +
/// `camera.num_samples` select the sample range this dispatch traces, as the worker's
/// `first_sample`/`samples` request does. This lets `estimator_check` give the CPU and GPU
/// disjoint sample ranges. `white_balance` is the precomputed von-Kries scale
/// (`compute_illuminant_white_balance`). It is a diagonal in **Bradford LMS** space, not XYZ:
/// `apply_von_kries_white_balance` transforms to LMS, scales, and transforms back, as the CPU
/// side does.
///
/// # Layout
///
/// The struct is 128 bytes in eight 16-byte rows. The `offset_of!`/`size_of!` asserts after the
/// `impl` blocks pin every offset. `white_balance` (`vec3<f32>`) starts a row at offset 48, so
/// it needs no padding. The last four rows hold the lighting model, the head-shadow cone, the
/// light-tent parameters and the HDR map's yaw (`hdr_rot_cos`, `hdr_rot_sin`) with two pad words.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuTransportParams {
    /// Number of pixels in the dispatch.
    pub num_pixels: u32,
    /// Maximum number of internal bounces per path.
    pub max_bounces: u32,
    /// Index of the first sample this dispatch traces.
    pub sample_offset: u32,
    /// Environment model, see [`transport_env_mode`].
    pub env_mode: u32,
    /// Radiance of the uniform furnace environment (`env_mode == UNIFORM_FURNACE`).
    pub l0: f32,
    /// Color temperature of the studio rig's light, in kelvin.
    pub studio_temp_k: f32,
    /// Spot multiplier of the studio rig.
    pub studio_spot_mult: f32,
    /// Studio exposure: the analytic rig's (`env_mode == STUDIO_RIG`). An HDR map's exposure
    /// rides in [`Self::hdr_exposure`] instead.
    pub studio_exposure: f32,
    /// Azimuth of the studio key light.
    pub studio_light_yaw: f32,
    /// Pitch of the studio key light.
    pub studio_light_pitch: f32,
    /// Index of the first pixel this dispatch covers. The shader adds its `idx / num_samples` to
    /// it to get the global pixel for camera rays, while output slots stay dispatch-local. Zero
    /// for a whole-frame dispatch; `renderer::gpu::frame` uses it to split a frame into chunks.
    pub pixel_offset: u32,
    /// Flag bits. Bit 0 ([`Self::DEBUG_WRITES_BIT`]) enables the three per-channel debug output
    /// buffers (`out_radiance`, `out_lambdas`, `out_path_pdf`). [`Self::new`] sets it and
    /// [`Self::with_debug_buffers_disabled`] clears it, leaving only `out_xyz` written. Bit 1 is
    /// unused and must stay free. Bit 2 ([`Self::COLLECT_BIT`]) runs the exit-collection
    /// estimator, see [`Self::with_estimator`].
    pub write_debug_buffers: u32,
    /// Von Kries scale in Bradford LMS space (see the struct docs).
    pub white_balance: [f32; 3],
    /// Nonzero to sample the tabulated CIE D65 spectrum instead of `blackbody_spectrum` at
    /// `studio_temp_k` (zero, the default). Mirrors `sample_studio_environment_with_rig`'s
    /// `preset.uses_d65()` branch. Unused when `env_mode == transport_env_mode::UNIFORM_FURNACE`.
    pub studio_use_d65: u32,
    /// Which environment model to sample -- see [`studio_model`].
    pub studio_model: u32,
    /// Radiance of the backdrop card a camera ray sees where it misses the stone
    /// (`0.0`: none) -- mirrors `EnvironmentSource::Studio::backdrop`.
    pub backdrop: f32,
    /// Scale of the stone's first-surface specular reflection, `0.0..=1.0` (`1.0`:
    /// unchanged) -- mirrors `EnvironmentSource::Studio::surface_glare`.
    pub surface_glare: f32,
    /// Outer cosine of the observer head-shadow cone (the dot product at which the shadow
    /// starts to fade in): mirrors
    /// `head_shadow_cosines(EnvironmentSource::Studio::head_shadow_deg)[0]`. Default = the
    /// 16 degree cone's literal.
    pub head_shadow_outer_cos: f32,
    /// Inner cosine of the head-shadow cone: `head_shadow_cosines(..)[1]`.
    pub head_shadow_inner_cos: f32,
    /// Light-tent wall flattening (`TentParams::flat`; `0.0` = the tent's gradient). Offset 84.
    pub tent_flat: f32,
    /// Number of concave-tool records appended to the `planes` storage buffer after the real
    /// planes (`0` for a planar stone; see [`Self::with_tool_count`]). Offset 88.
    num_tools: u32,
    /// The HDR map's exposure (`EnvironmentSource::HdrMap::exposure`), read only by
    /// `env_mode == transport_env_mode::HDR_MAP` (`hdr_rgb_at_exposure` in
    /// `transport_bounce/04_studio_and_hdr_environment.wgsl`, the twin of
    /// `optics::raytracer::environment::hdr_rgb_at_exposure`). Defaults to `1.0`, which the
    /// shader skips: a uniform built without [`Self::with_hdr_of`] (the self-test harnesses)
    /// lights the map as decoded. Offset 92.
    pub hdr_exposure: f32,
    /// Light-tent wall scale (`TentParams::walls`; `1.0` = the tent). Only the
    /// `LIGHT_TENT` model reads the four `tent_*` fields.
    pub tent_walls: f32,
    /// Light-tent card strength (`TentParams::cards`; `1.0` = the tent, `0.0` = no cards).
    pub tent_cards: f32,
    /// Light-tent spark flag (`TentParams::spark`; `1.0` = on, `0.0` = off).
    pub tent_spark: f32,
    /// Light-tent ground radiance (`TentParams::ground`; `0.02` = the tent).
    pub tent_ground: f32,
    /// Cosine of the HDR map's yaw (`EnvRotation::cos_sin()[0]`), read only by
    /// `env_mode == transport_env_mode::HDR_MAP` (`hdr_rot` in
    /// `transport_bounce/04_studio_and_hdr_environment.wgsl`). Exactly `1.0` together with
    /// [`Self::hdr_rot_sin`] `== 0.0` is the identity, which the shader skips, so a uniform
    /// built without [`Self::with_hdr_rotation`] lights the map as decoded. Offset 112.
    pub hdr_rot_cos: f32,
    /// Sine of the HDR map's yaw (`EnvRotation::cos_sin()[1]`); see [`Self::hdr_rot_cos`].
    pub hdr_rot_sin: f32,
    /// Pad word completing the last 16-byte row; always zero.
    _pad_hdr_rot0: u32,
    /// Pad word completing the last 16-byte row; always zero.
    _pad_hdr_rot1: u32,
}

/// `studio_model` discriminants for [`GpuTransportParams`].
///
/// Mirrors [`crate::optics::raytracer::LightingModel`]'s `gpu_id` mapping:
/// - 0: `Studio` (classic analytic rig)
/// - 1: `IsoHemisphere` (uniform lit upper hemisphere)
/// - 2: `LightTent` (light tent + black cards)
/// - 3: `DaylightDome` (daylight sky, no sun)
/// - 4: `Aset` (ASET-style contrast view)
/// - 5: `DaylightSun` (daylight sky + direct sun)
pub mod studio_model {
    /// Identifier for studio.
    pub const STUDIO: u32 = 0;
    /// Identifier for iso hemisphere.
    pub const ISO_HEMISPHERE: u32 = 1;
    /// Identifier for light tent.
    pub const LIGHT_TENT: u32 = 2;
    /// Identifier for daylight dome.
    pub const DAYLIGHT_DOME: u32 = 3;
    /// Identifier for the ASET-style contrast view.
    pub const ASET: u32 = 4;
    /// Identifier for daylight sky plus direct sun.
    pub const DAYLIGHT_SUN: u32 = 5;
}

/// `env_mode` discriminants for [`GpuTransportParams`]. Must match
/// `shaders/spectral_transport.wgsl`'s own `params.env_mode` branch.
pub mod transport_env_mode {
    /// Identifier for uniform furnace.
    pub const UNIFORM_FURNACE: u32 = 0;
    /// Identifier for studio rig.
    pub const STUDIO_RIG: u32 = 1;
    /// `EnvironmentSource::HdrMap`.
    ///
    /// Sampled via `hdr_env_radiance_at` from the map's storage and uniform buffers (bindings
    /// 10/11), at [`super::GpuTransportParams::hdr_exposure`]. See `renderer::env_map_gpu`.
    pub const HDR_MAP: u32 = 2;
}

impl GpuTransportParams {
    /// Bit of [`Self::write_debug_buffers`] that asks for the per-channel debug writes.
    pub const DEBUG_WRITES_BIT: u32 = 1;
    /// Bit of [`Self::write_debug_buffers`] that turns the exit-collection estimator on
    /// (`PARAMS_COLLECT_BIT` in `shaders/transport_physics/01_prelude_polarization.wgsl`).
    pub const COLLECT_BIT: u32 = 4;

    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "one parameter per GpuTransportParams field, in field order, because \
                  that field order is itself the #[repr(C)] layout spectral_transport.wgsl's \
                  uniform buffer binds against; bundling them into a context struct here \
                  would just re-introduce this exact struct one level removed"
    )]
    /// Creates a new value from its components.
    pub const fn new(
        num_pixels: u32,
        max_bounces: u32,
        sample_offset: u32,
        env_mode: u32,
        l0: f32,
        studio_temp_k: f32,
        studio_spot_mult: f32,
        studio_exposure: f32,
        studio_light_yaw: f32,
        studio_light_pitch: f32,
        white_balance: [f32; 3],
    ) -> Self {
        Self {
            num_pixels,
            max_bounces,
            sample_offset,
            env_mode,
            l0,
            studio_temp_k,
            studio_spot_mult,
            studio_exposure,
            studio_light_yaw,
            studio_light_pitch,
            pixel_offset: 0,
            write_debug_buffers: Self::DEBUG_WRITES_BIT,
            white_balance,
            studio_use_d65: 0,
            studio_model: 0,
            backdrop: 0.0,
            surface_glare: 1.0,
            head_shadow_outer_cos: DEFAULT_HEAD_SHADOW_COSINES[0],
            head_shadow_inner_cos: DEFAULT_HEAD_SHADOW_COSINES[1],
            tent_flat: TentParams::DEFAULT.flat,
            num_tools: 0,
            hdr_exposure: 1.0,
            tent_walls: TentParams::DEFAULT.walls,
            tent_cards: TentParams::DEFAULT.cards,
            tent_spark: TentParams::DEFAULT.spark,
            tent_ground: TentParams::DEFAULT.ground,
            hdr_rot_cos: 1.0,
            hdr_rot_sin: 0.0,
            _pad_hdr_rot0: 0,
            _pad_hdr_rot1: 0,
        }
    }

    /// Returns a copy with the light-tent per-preset parameters (see [`TentParams`]).
    /// The default already is the light tent's own values.
    #[must_use]
    pub const fn with_tent(mut self, tent: TentParams) -> Self {
        self.tent_walls = tent.walls;
        self.tent_cards = tent.cards;
        self.tent_spark = tent.spark;
        self.tent_ground = tent.ground;
        self.tent_flat = tent.flat;
        self
    }

    /// Returns a copy with the tent parameters of `environment`'s lighting preset (the
    /// default for an HDR map, whose shader branch never reads them).
    #[must_use]
    pub const fn with_tent_of(
        self,
        environment: crate::optics::raytracer::EnvironmentSource<'_>,
    ) -> Self {
        match environment.lighting_preset() {
            Some(preset) => self.with_tent(preset.params().tent),
            None => self.with_tent(TentParams::DEFAULT),
        }
    }

    /// Returns a copy with the HDR map exposure `exposure` (see [`Self::hdr_exposure`]).
    #[must_use]
    pub const fn with_hdr_exposure(mut self, exposure: f32) -> Self {
        self.hdr_exposure = exposure;
        self
    }

    /// Returns a copy with the HDR map yaw `rotation` (see [`Self::hdr_rot_cos`]).
    #[must_use]
    pub const fn with_hdr_rotation(mut self, rotation: EnvRotation) -> Self {
        let cos_sin = rotation.cos_sin();
        self.hdr_rot_cos = cos_sin[0];
        self.hdr_rot_sin = cos_sin[1];
        self
    }

    /// Returns a copy with `environment`'s HDR exposure and rotation: the map's own for an
    /// HDR map, `1.0` and the identity for the analytic rig (whose shader branch never reads
    /// them, its exposure being `studio_exposure`). Every production dispatch builds its
    /// uniform with this beside [`Self::with_tent_of`], so no builder can forget the rotation.
    #[must_use]
    pub const fn with_hdr_of(
        self,
        environment: crate::optics::raytracer::EnvironmentSource<'_>,
    ) -> Self {
        match environment {
            crate::optics::raytracer::EnvironmentSource::HdrMap {
                exposure, rotation, ..
            } => self.with_hdr_exposure(exposure).with_hdr_rotation(rotation),
            crate::optics::raytracer::EnvironmentSource::Studio { .. } => self
                .with_hdr_exposure(1.0)
                .with_hdr_rotation(EnvRotation::IDENTITY),
        }
    }

    /// Returns a copy covering pixels `[pixel_offset, pixel_offset + num_pixels)` of a larger
    /// frame, see [`Self::pixel_offset`]. A builder rather than a `new` parameter keeps existing
    /// constructor calls unchanged.
    #[must_use]
    pub const fn with_pixel_offset(mut self, pixel_offset: u32) -> Self {
        self.pixel_offset = pixel_offset;
        self
    }

    /// Returns a copy that traces a concave stone with `num_tools` tool records packed after the
    /// planes in the `planes` binding. The count is a uniform, so a planar stone (`0`) takes the
    /// unchanged code path.
    #[must_use]
    pub const fn with_tool_count(mut self, num_tools: u32) -> Self {
        self.num_tools = num_tools;
        self
    }

    /// Number of concave-tool records in the `planes` buffer (see [`Self::with_tool_count`]).
    #[must_use]
    pub const fn tool_count(&self) -> u32 {
        self.num_tools
    }

    /// Returns a copy that skips `transport_main`'s three per-channel debug output
    /// writes -- see [`Self::write_debug_buffers`]'s doc comment. Only
    /// `GpuFrameRenderer::accumulate`'s production dispatch calls this.
    #[must_use]
    pub const fn with_debug_buffers_disabled(mut self) -> Self {
        self.write_debug_buffers &= !Self::DEBUG_WRITES_BIT;
        self
    }

    /// Returns a copy whose dispatch runs `estimator` at the partial Fresnel events: the GPU
    /// twin of `optics::raytracer::transport::PreparedMaterial::with_estimator`.
    ///
    /// [`Estimator::ExitCollection`] is honoured only where the CPU tracer honours it and the
    /// kernels implement it: over a surface the CPU tracer calls ideal (`ideal_surface`:
    /// `GpuSurfaceScene::is_ideal_for_cpu`), without a light-sampling technique (not an HDR
    /// map, not the Daylight + Sun rig) and for a convex stone (no tools: the kernels do not
    /// probe a leaf for re-entry). The CPU tracer declines exactly the same stones (a stone with
    /// tools runs the branching estimator on every engine), so a hybrid frame never mixes
    /// estimators. Any other dispatch runs [`Estimator::Branching`], which is every dispatch as
    /// it was. Call this LAST in a builder chain: it reads the environment
    /// mode, the studio model and the tool count.
    #[must_use]
    pub const fn with_estimator(mut self, estimator: Estimator, ideal_surface: bool) -> Self {
        let no_light_sampling = self.env_mode != transport_env_mode::HDR_MAP
            && !(self.env_mode == transport_env_mode::STUDIO_RIG
                && self.studio_model == studio_model::DAYLIGHT_SUN);
        let collect = matches!(estimator, Estimator::ExitCollection)
            && ideal_surface
            && no_light_sampling
            && self.num_tools == 0;
        if collect {
            self.write_debug_buffers |= Self::COLLECT_BIT;
        } else {
            self.write_debug_buffers &= !Self::COLLECT_BIT;
        }
        self
    }

    /// The estimator this dispatch runs (see [`Self::with_estimator`]).
    #[must_use]
    pub const fn estimator(&self) -> Estimator {
        if self.write_debug_buffers & Self::COLLECT_BIT == 0 {
            Estimator::Branching
        } else {
            Estimator::ExitCollection
        }
    }

    /// Returns a copy that samples the tabulated CIE D65 measured spectrum instead of
    /// `blackbody_spectrum` at `studio_temp_k` -- see [`Self::studio_use_d65`]'s doc
    /// comment. Only callers whose `EnvironmentSource::Studio` preset is
    /// `LightingPreset::Daylight` should pass `true` here.
    #[must_use]
    pub const fn with_studio_use_d65(mut self, use_d65: bool) -> Self {
        self.studio_use_d65 = if use_d65 { 1 } else { 0 };
        self
    }

    /// Returns a copy with the specified studio lighting model (see [`studio_model`]).
    #[must_use]
    pub const fn with_studio_model(mut self, model: u32) -> Self {
        self.studio_model = model;
        self
    }

    /// Returns a copy with the specified backdrop radiance (see [`Self::backdrop`]).
    #[must_use]
    pub const fn with_backdrop(mut self, backdrop: f32) -> Self {
        self.backdrop = backdrop;
        self
    }

    /// Returns a copy with the specified surface-glare scale (see
    /// [`Self::surface_glare`]), clamped to `0.0..=1.0`; NaN means `1.0`.
    #[must_use]
    pub const fn with_surface_glare(mut self, surface_glare: f32) -> Self {
        self.surface_glare = if surface_glare >= 1.0 || surface_glare.is_nan() {
            1.0
        } else if surface_glare > 0.0 {
            surface_glare
        } else {
            0.0
        };
        self
    }

    /// Returns a copy with the observer head-shadow cone `[outer, inner]` cosines (see
    /// [`Self::head_shadow_outer_cos`] and `optics::raytracer::head_shadow_cosines`).
    #[must_use]
    pub const fn with_head_shadow(mut self, cosines: [f32; 2]) -> Self {
        self.head_shadow_outer_cos = cosines[0];
        self.head_shadow_inner_cos = cosines[1];
        self
    }
}

const _: () = {
    assert!(offset_of!(GpuTransportParams, num_pixels) == 0);
    assert!(offset_of!(GpuTransportParams, max_bounces) == 4);
    assert!(offset_of!(GpuTransportParams, sample_offset) == 8);
    assert!(offset_of!(GpuTransportParams, env_mode) == 12);
    assert!(offset_of!(GpuTransportParams, l0) == 16);
    assert!(offset_of!(GpuTransportParams, studio_temp_k) == 20);
    assert!(offset_of!(GpuTransportParams, studio_spot_mult) == 24);
    assert!(offset_of!(GpuTransportParams, studio_exposure) == 28);
    assert!(offset_of!(GpuTransportParams, studio_light_yaw) == 32);
    assert!(offset_of!(GpuTransportParams, studio_light_pitch) == 36);
    assert!(offset_of!(GpuTransportParams, white_balance) == 48);
    assert!(offset_of!(GpuTransportParams, studio_use_d65) == 60);
    assert!(offset_of!(GpuTransportParams, studio_model) == 64);
    assert!(offset_of!(GpuTransportParams, backdrop) == 68);
    assert!(offset_of!(GpuTransportParams, surface_glare) == 72);
    assert!(offset_of!(GpuTransportParams, head_shadow_outer_cos) == 76);
    assert!(offset_of!(GpuTransportParams, head_shadow_inner_cos) == 80);
    assert!(offset_of!(GpuTransportParams, tent_flat) == 84);
    assert!(offset_of!(GpuTransportParams, num_tools) == 88);
    assert!(offset_of!(GpuTransportParams, hdr_exposure) == 92);
    assert!(offset_of!(GpuTransportParams, tent_walls) == 96);
    assert!(offset_of!(GpuTransportParams, tent_cards) == 100);
    assert!(offset_of!(GpuTransportParams, tent_spark) == 104);
    assert!(offset_of!(GpuTransportParams, tent_ground) == 108);
    assert!(offset_of!(GpuTransportParams, hdr_rot_cos) == 112);
    assert!(offset_of!(GpuTransportParams, hdr_rot_sin) == 116);
    assert!(offset_of!(GpuTransportParams, _pad_hdr_rot0) == 120);
    assert!(offset_of!(GpuTransportParams, _pad_hdr_rot1) == 124);
    assert!(size_of::<GpuTransportParams>() == 128);
};
