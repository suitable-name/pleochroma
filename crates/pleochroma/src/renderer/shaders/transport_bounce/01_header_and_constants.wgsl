// Shared per-bounce physics: the scene bindings, the helpers of a bounce (intersection, NEE,
// environment sampling, the uniaxial and biaxial dispatch) and `transport_bounce_step`, one bounce
// of the megakernel loop.
//
// `transport_main` (`spectral_transport.wgsl`) and `wavefront_bounce` (`wavefront_transport.wgsl`)
// both call `transport_bounce_step`, so both run the same compiled arithmetic and cannot drift
// apart. This file is concatenated ahead of both. `transport_main` keeps only the per-ray
// prologue, the bounce loop and the output tail.
//
// # `transport_bounce_step`
//
// One call is one iteration of the bounce loop. Its exits are a `u32` status:
// `BOUNCE_STATUS_TERMINATE` where the loop ends, `BOUNCE_STATUS_CONTINUE` otherwise. The
// per-channel `continue`s inside the inner loops are unchanged.
//
// Read-only per-ray values (wavelengths, hoisted constants, RNG seed) are passed by value. Mutable
// per-bounce state (ray origin, direction, wave normal, `inside_gem`, `is_extraordinary`, Stokes,
// radiance, path pdf, split radiance, compat, `path_escaped`, `pending_light_mis`) is passed as
// `ptr<function, T>`. The arithmetic and RNG draws are the same, so the result is identical by
// construction: only where each value lives differs.
//
// Each call reads only its arguments and shares nothing across threads, so the same inputs always
// give the same outputs, whatever other rays are in flight.

// Spectral estimator for isotropic, uniaxial and biaxial media: the GPU port of
// `optics::raytracer::trace_spectral_ray`.
//
// Biaxial "mode A" is the faster root of `biaxial_wave_indices`, mode B the slower.
// `is_extraordinary` selects between them, as in the uniaxial path. An air-to-crystal entry
// resolves both wave normals with a two-iteration fixed point (`biaxial_resolve_entry_mode`) from
// the hero channel, shared by all companion channels. Both biaxial modes walk off. Whether a
// material is trusted for rendering is decided by `GemMaterial::gpu_supported`, not here.
//
// For isotropic materials `is_anisotropic` and `is_biaxial` are false, which reduces the path to
// the isotropic case. The pleochroic absorption is computed in full (eigen-polarizations and the
// quadratic form), not a bare `spectral_absorption(lambda)`, so the CPU rounding matches too.
//
// Uniaxial interfaces use the closed-form `entry_solve_pair` and `internal_solve` of
// `transport_physics/` (after `optics::raytracer::uniaxial_fresnel`). The exception is the
// degenerate case where the wave normal is parallel to the optic axis (`uniaxial_nondegenerate`
// false), where a scalar Fresnel per mode is exact.
// A companion channel fails chromatic termination when its direction does not match the stored
// hero-driven direction (`final_refr_dir`, `final_dir_k`). Those stored values are compared, not
// recomputed, because a recomputation can miss the match by a few ULP.
// For biaxial materials the optional third band set (`beta_ray`) enters through
// `pleochroic_channel_alpha_biaxial`.
//
// Stochastic decisions use one locally computed probability for both the branch test and the
// division by it, so float differences between threads cannot change the outcome. Precision is
// `f32`; `f32::mul_add` maps to `fma`, and powers are multiplication chains rather than `pow()`.
//
// The output buffers are always bound (final XYZ and the per-channel debug arrays). WGSL binding
// follows static reachability, so a runtime flag would not save any bindings.
//
// State lives in the function's locals, and each thread writes only its own output slot.
//
// Exit-event splitting: at a crystal-to-air exit, a companion channel whose direction differs from
// the hero's gets its own transmission and one bounded environment probe
// (`try_split_exit_channel`). That goes into `split_radiance`, which is committed to `radiance`
// only if the path escapes. The `compat` bitmask tracks which channels still share an MIS
// family. An interior mismatch narrows it (`narrow_compat`), and exits never do. Russian
// roulette rescales
// `split_radiance` with `stokes`. The final XYZ normalizes each channel over its own family.
//
// `MATERIAL_CLASS` is an `override` constant set per pipeline. It removes code the stone cannot
// reach: with `is_anisotropic` or `is_biaxial` forced false, the per-ray arrays gated on them are
// dead, so the compiler drops them and their registers. `MATERIAL_CLASS_GENERIC`, the default and
// the self-tests, passes the runtime value through unchanged.
override MATERIAL_CLASS: u32 = 0u;
// `ORIENTED_INCLUSIONS` false removes the oriented-inclusion and Bragg estimators, for a stone
// without them (`material_class::NO_ORIENTED`). RADV (AMD Vulkan) returns garbage for uniaxial
// stones while that code is reachable, so the constant also works around the driver.
override ORIENTED_INCLUSIONS: bool = true;
// `ZONED_ORIENTED` false removes the zoned oriented estimator for an unzoned stone
// (`material_class::NO_ZONED_ORIENTED`). Same RADV issue as above.
override ZONED_ORIENTED: bool = true;
// `SCATTER_NEE` false removes the scattering-point NEE for a zoned stone with oriented inclusions,
// where it never runs (`material_class::NO_SCATTER_NEE`). RADV returns NaN for the zoned rim while
// the code is reachable.
override SCATTER_NEE: bool = true;
// Quartz and Rutile need their independent e-ray curves. A material without a curve specializes
// the hero index to the ordinary-index sum. Unspecialized pipelines pick the formula from the
// runtime material flag.
override EXTRAORDINARY_DISPERSION: bool = true;
// `GATED_LOOP` true selects the gated loop of `spectral_transport.wgsl` (`transport_bounce_front`
// and `transport_bounce_back`, `08f`). A bounce with a heavy arm (anything but total internal
// reflection) parks after its surface phase, and parked lanes of a warp run their arms together.
// Each sample performs the same operations and draws, only the lane interleaving changes. The
// default keeps the plain loop, which the text-patched variants (beam reference, path-cache
// completion) rely on. Only the megakernel reads it.
override GATED_LOOP: bool = false;
const MATERIAL_CLASS_GENERIC: u32 = 0u;
const MATERIAL_CLASS_ISOTROPIC: u32 = 1u;
const MATERIAL_CLASS_UNIAXIAL: u32 = 2u;
const MATERIAL_CLASS_BIAXIAL: u32 = 3u;

const SPECTRUM_MIN: f32 = 380.0;
const SPECTRUM_SPAN: f32 = 400.0;
const RR_FLOOR: f32 = 0.05;
// Clamp of the reflect-vs-transmit selection probability. It is tighter than `R_UNPOL_MIN` and
// `R_UNPOL_MAX` (in `transport_physics/`) because this is the one site where the probability both
// selects a branch and divides `stokes` (`1/r_unpol`, `1/(1-r_unpol)`).
const R_UNPOL_SELECT_MIN: f32 = 0.02;
const R_UNPOL_SELECT_MAX: f32 = 0.98;
// `RAY_EPS`, `R_UNPOL_MIN`/`MAX`, the stream salts and `hash_u32` are defined in
// `transport_physics/01_prelude_polarization.wgsl`.

// Matches `optics::raytracer::{BRADFORD_XYZ_TO_LMS, BRADFORD_LMS_TO_XYZ}`. `params.white_balance`
// is a scale in Bradford LMS space, so it must be applied between the two transforms, not to XYZ.
// `environment.wgsl` holds its own copy of these constants.
const BRADFORD_XYZ_TO_LMS = mat3x3<f32>(
    vec3<f32>(0.8951, -0.7502, 0.0389),
    vec3<f32>(0.2664, 1.7135, -0.0685),
    vec3<f32>(-0.1614, 0.0367, 1.0296),
);
const BRADFORD_LMS_TO_XYZ = mat3x3<f32>(
    vec3<f32>(0.986993, 0.432305, -0.008529),
    vec3<f32>(-0.147054, 0.518360, 0.040043),
    vec3<f32>(0.159963, 0.049291, 0.968487),
);

fn apply_von_kries_white_balance(xyz: vec3<f32>, lms_scale: vec3<f32>) -> vec3<f32> {
    let lms = BRADFORD_XYZ_TO_LMS * xyz;
    return BRADFORD_LMS_TO_XYZ * (lms * lms_scale);
}

