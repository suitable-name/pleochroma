//! White-furnace energy-conservation tests for transport: a colorless gem in a
//! spatially uniform environment must converge to that environment's radiance.
//!
//! Covers a birefringent gem with a polished and with a frosted girdle, and a
//! concave, dispersive stone.

use glam::Vec3;
use pleochroma::{
    geometry::{StoneGeometry, ToolPrimitive, cuts::StandardGemCuts},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            EnvironmentSource, refraction::exit_split_truncated_energy_take, trace_spectral_ray,
            trace_spectral_ray_geom, trace_spectral_ray_with_finish,
        },
    },
    renderer::env_map::EnvironmentMap,
};

use crate::fixtures::{bruted_girdle_finishes, furnace_mean_xyz, uniform_furnace_target};

/// Birefringent (uniaxial) colorless gem in a spatially uniform environment, traced at
/// several bounce caps.
///
/// Guards the two anisotropic energy paths that isotropic furnaces never reach: the
/// internal o/e mode coupling in `apply_internal_mode_coupling`, which must not scale
/// `stokes` per internal reflection (it would diverge with the cap), and the entry split
/// in `apply_refract_bounce`, which must not divide `stokes` by the 0.5 mode-selection
/// probability (a flat ~50 % brightness error that a cross-cap check alone would miss).
///
/// The test runs on the CPU only, so a default `cargo test` covers it. Each cap's mean
/// must match the analytic radiance; the finiteness, ceiling and cross-cap drift checks
/// guard the compounding case directly.
#[test]
fn birefringent_white_furnace_energy_conservation_holds() {
    const L0: f32 = 2.5;
    const SAMPLES_PER_PIXEL: u32 = 64;
    // Largest allowed drift of a higher-cap mean from the cap-12 mean, relative to the
    // cap-12 value per XYZ component. Covers the cap-12 truncation (about 1.3 percent:
    // paths still inside the gem at the cap are dropped) and sampling noise (about 0.1
    // percent), with five standard deviations of headroom. A compounding gain exceeds it
    // by orders of magnitude.
    const CROSS_CAP_TOLERANCE: f32 = 0.02;
    // Largest allowed relative error of each cap's mean against the analytic target: the
    // same cap-12 truncation and noise, with five standard deviations of headroom. Far
    // below the ~48-50 percent error of an unguarded entry-split regression, and tight
    // enough to catch a branch mis-weighted by a few percent.
    const ANALYTIC_TOLERANCE: f32 = 0.02;
    // Generous finite bound on plausible brightness: above the analytic target, far below
    // what an unbounded per-bounce gain reaches at high caps.
    const SANITY_CEILING: f32 = 50.0 * L0;
    // Truncated (12), moderately deep (64) and effectively untruncated (256) caps.
    const BOUNCE_CAPS: [u32; 3] = [12, 64, 256];

    let planes = StandardGemCuts::standard_round_brilliant();
    // Colorless, non-dispersive and uniaxial: a nonzero `birefringence_delta` selects the
    // `is_anisotropic` path that `apply_internal_mode_coupling` acts on. `new_custom`
    // sets `crystal_system: Trigonal` and `optical_character: UniaxialPositive` for such
    // a delta. The empty absorption bands (last argument) keep the material colorless.
    let material = GemMaterial::new_custom(
        "CPU birefringent furnace probe",
        1.5,
        0.0,
        0.03,
        [0.0, 0.0, 0.0],
    );
    assert!(
        material.birefringence_delta.abs() > 1e-4,
        "test assumes this material is genuinely birefringent"
    );
    let env_map = EnvironmentMap::uniform(1, 1, [L0, L0, L0]);

    // Analytic radiance of the uniform environment: the truth anchor for the furnace checks.
    let target = uniform_furnace_target(L0);

    let mut means: Vec<(u32, Vec3)> = Vec::with_capacity(BOUNCE_CAPS.len());
    for &max_bounces in &BOUNCE_CAPS {
        // `trace_spectral_ray` always enables internal mode coupling, as production does.
        let (mean, count) = furnace_mean_xyz(SAMPLES_PER_PIXEL, 0x4257_4946, |ray, seed, hero| {
            trace_spectral_ray(
                ray,
                &planes,
                &material,
                max_bounces,
                EnvironmentSource::hdr(&env_map),
                seed,
                hero,
                None,
            )
        });
        let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
        let (ex, ey, ez) = (
            rel_err(mean.x, target.x),
            rel_err(mean.y, target.y),
            rel_err(mean.z, target.z),
        );
        println!(
            "[birefringent furnace] max_bounces={max_bounces} mean={mean:?} \
             analytic_target={target:?} rel_err_vs_target=({ex:.4}, {ey:.4}, {ez:.4}) over \
             {count} samples",
        );
        assert!(
            mean.x.is_finite() && mean.y.is_finite() && mean.z.is_finite(),
            "max_bounces={max_bounces}: birefringent furnace mean must stay finite, got {mean:?} \
             -- a non-finite value here is the exact signature of the internal-mode-coupling \
             energy-doubling bug this test guards against"
        );
        assert!(
            mean.x <= SANITY_CEILING && mean.y <= SANITY_CEILING && mean.z <= SANITY_CEILING,
            "max_bounces={max_bounces}: birefringent furnace mean {mean:?} exceeds the sanity \
             ceiling {SANITY_CEILING} -- this is the signature of unbounded per-bounce energy \
             growth, exactly what the internal-mode-coupling bug produced"
        );
        assert!(
            ex <= ANALYTIC_TOLERANCE && ey <= ANALYTIC_TOLERANCE && ez <= ANALYTIC_TOLERANCE,
            "max_bounces={max_bounces}: birefringent furnace mean {mean:?} strays from the \
             analytic target {target:?} by rel_err=({ex}, {ey}, {ez}), tolerance=\
             {ANALYTIC_TOLERANCE} -- with both the internal-mode-coupling and entry-split \
             energy-conservation bugs fixed, a lossless closed system must return its own \
             uniform radiance to within ordinary sampling noise"
        );
        means.push((max_bounces, mean));
    }

    let (low_cap, low_mean) = means[0];
    for &(hi_cap, hi_mean) in &means[1..] {
        let drift = |lo: f32, hi: f32| (hi - lo).abs() / lo.abs().max(1e-6);
        let (dx, dy, dz) = (
            drift(low_mean.x, hi_mean.x),
            drift(low_mean.y, hi_mean.y),
            drift(low_mean.z, hi_mean.z),
        );
        assert!(
            dx <= CROSS_CAP_TOLERANCE && dy <= CROSS_CAP_TOLERANCE && dz <= CROSS_CAP_TOLERANCE,
            "a lossless closed system's expected brightness must not depend on the bounce \
             cap: max_bounces={low_cap} gave mean={low_mean:?}, max_bounces={hi_cap} gave \
             mean={hi_mean:?}, drift=({dx}, {dy}, {dz}), tolerance={CROSS_CAP_TOLERANCE} -- \
             this is exactly the property the original per-internal-bounce `stokes *= 2.0` \
             broke (each additional internal bounce compounded the brightness further)"
        );
    }
}

/// Frosted girdle combined with a birefringent material, so a girdle entry takes the
/// anisotropic branch of `apply_frosted_bounce` (`entering_anisotropic` in
/// `optics::raytracer::scattering`) and still needs the 50/50 mode split.
///
/// Same material as `birefringent_white_furnace_energy_conservation_holds`, with the girdle
/// from `bruted_girdle_finishes`. A single bounce cap: this checks the entry-split weight,
/// not compounding.
#[test]
fn frosted_girdle_birefringent_white_furnace_energy_conservation_holds() {
    const L0: f32 = 2.5;
    const SAMPLES_PER_PIXEL: u32 = 64;
    // Measured at this sample budget: 1.65 percent truncation loss at cap 12 (none at
    // cap 256) plus about 0.15 percent noise; five standard deviations of headroom.
    const ANALYTIC_TOLERANCE: f32 = 0.025;

    let planes = StandardGemCuts::standard_round_brilliant();
    let finishes = bruted_girdle_finishes(planes.len());
    // Birefringent, so a girdle entry can reach the `entering_anisotropic` branch.
    let material = GemMaterial::new_custom(
        "CPU frosted birefringent furnace probe",
        1.5,
        0.0,
        0.03,
        [0.0, 0.0, 0.0],
    );
    assert!(
        material.birefringence_delta.abs() > 1e-4,
        "test assumes this material is genuinely birefringent"
    );
    let env_map = EnvironmentMap::uniform(1, 1, [L0, L0, L0]);

    let (mean, count) = furnace_mean_xyz(SAMPLES_PER_PIXEL, 0x4652_4247, |ray, seed, hero| {
        trace_spectral_ray_with_finish(
            ray,
            &planes,
            &finishes,
            &material,
            12,
            EnvironmentSource::hdr(&env_map),
            seed,
            hero,
            None,
        )
    });

    // Analytic radiance of the uniform environment.
    let target = uniform_furnace_target(L0);

    let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
    let (ex, ey, ez) = (
        rel_err(mean.x, target.x),
        rel_err(mean.y, target.y),
        rel_err(mean.z, target.z),
    );
    println!(
        "[frosted-girdle birefringent furnace] mean={mean:?} target={target:?} \
         rel_err=({ex:.4}, {ey:.4}, {ez:.4}) over {count} samples"
    );
    assert!(
        ex <= ANALYTIC_TOLERANCE && ey <= ANALYTIC_TOLERANCE && ez <= ANALYTIC_TOLERANCE,
        "frosted-girdle birefringent furnace should still converge to the uniform \
         environment's own radiance (mean={mean:?}, target={target:?}, rel_err=({ex}, {ey}, \
         {ez}), tolerance={ANALYTIC_TOLERANCE}) -- a residual here is the signature of \
         apply_frosted_bounce's anisotropic entry-split bug (dividing stokes by its own \
         0.5 mode-selection probability on top of an already-full-share diffuse-transmitted \
         intensity)"
    );
}

/// Main check for the concave transport shortcuts: a dispersive stone with a table groove.
///
/// The stone must return the uniform environment's radiance. A missed re-entry across the
/// groove (bounce loop, HG probe, NEE shadow, exit-split probe) shows up as lost energy.
///
/// The fixture is built from [`ToolPrimitive::cylinder`]: the groove runs along X, its
/// axis 0.12 above the table plane (y = 0.32), radius 0.3, removing a 0.18-deep, about
/// 0.55-wide channel across the table.
///
/// The gate is the analytic tolerance plus the exit-split truncation.
/// `try_split_exit_channel` drops a dispersive channel whose exit ray re-enters the stone,
/// which is common across a groove. The dropped energy is printed and added to the bound.
/// The truncated fraction is the dropped intensity per traced sample over the eight
/// channels that share a sample's unit throughput. The counter exists in debug builds
/// only, so a release build reports `0` and gets the bare tolerance.
#[test]
fn concave_fixture_white_furnace_energy_conservation_holds() {
    const L0: f32 = 2.5;
    const SAMPLES_PER_PIXEL: u32 = 64;
    // Cap 64 has no truncation, so this bound is generous for the planar facets.
    const ANALYTIC_TOLERANCE: f32 = 0.025;
    const MAX_BOUNCES: u32 = 64;

    let planes = StandardGemCuts::standard_round_brilliant();
    let tools = [ToolPrimitive::cylinder(
        Vec3::new(0.0, 0.32 + 0.12, 0.0),
        Vec3::X,
        0.3,
        2.0,
    )];
    let geom = StoneGeometry {
        planes: &planes,
        tools: &tools,
    };
    assert!(!geom.is_convex(), "the fixture must be concave");
    // Dispersive, so exit events split channels and the truncation counter has something
    // to report; colorless and isotropic otherwise.
    let material =
        GemMaterial::new_custom("Concave furnace probe", 1.5, 0.02, 0.0, [0.0, 0.0, 0.0]);
    let env_map = EnvironmentMap::uniform(1, 1, [L0, L0, L0]);
    let target = uniform_furnace_target(L0);

    let _ = exit_split_truncated_energy_take();
    let (mean, count) = furnace_mean_xyz(SAMPLES_PER_PIXEL, 0x434F_4E43, |ray, seed, hero| {
        trace_spectral_ray_geom(
            ray,
            geom,
            &material,
            Fluorescence::none(),
            MAX_BOUNCES,
            EnvironmentSource::hdr(&env_map),
            seed,
            hero,
            None,
        )
    });
    let truncated = exit_split_truncated_energy_take();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the truncated-energy fraction is a small ratio compared in f32"
    )]
    let truncated_fraction = (truncated / (f64::from(count) * 8.0)) as f32;

    let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
    let (ex, ey, ez) = (
        rel_err(mean.x, target.x),
        rel_err(mean.y, target.y),
        rel_err(mean.z, target.z),
    );
    let tolerance = ANALYTIC_TOLERANCE + truncated_fraction;
    println!(
        "[concave furnace] mean={mean:?} target={target:?} rel_err=({ex:.4}, {ey:.4}, {ez:.4}) \
         over {count} samples; exit-split truncated energy={truncated:.1} \
         (~{:.2}% of throughput), tolerance={tolerance:.4}",
        truncated_fraction * 100.0
    );
    assert!(
        mean.x.is_finite() && mean.y.is_finite() && mean.z.is_finite(),
        "concave furnace mean must stay finite, got {mean:?}"
    );
    assert!(
        ex <= tolerance && ey <= tolerance && ez <= tolerance,
        "concave furnace should converge to the uniform environment's radiance \
         (mean={mean:?}, target={target:?}, rel_err=({ex}, {ey}, {ez}), tolerance={tolerance}, \
         exit-split truncation={truncated_fraction}) -- a residual beyond the measured \
         truncation is a missed re-entry or a double-counted exit in the concave transport"
    );
}
