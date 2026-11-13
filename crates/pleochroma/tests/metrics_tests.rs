use pleochroma::{
    color::metrics::{
        PROFILE_ANGLES_DEG, TILT_ANGLES_DEG, camera_view_basis,
        evaluate_angular_profile_at_azimuth, evaluate_full_axis_profile_at_azimuth,
        evaluate_gem_optical_metrics,
    },
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, LightingPreset},
    },
};

/// The light pose the metrics tests score under unless a test varies it: the editor's
/// default preset at the canonical key-light pose.
const fn studio() -> EnvironmentSource<'static> {
    LightingPreset::RingLights.studio(1.0, 0.85, 0.95)
}

/// Comparison tests read several camera poses: one pose can put two stones on the same floor
/// and decide nothing.
const COMPARISON_POSES: [(f32, f32); 4] = [(0.0, 1.4), (0.0, 0.45), (0.6, 0.9), (0.8, 0.05)];

/// Checks that the metrics' observer basis matches the render camera's (`Camera::new`).
///
/// That includes the `world_up` choice near the poles. A mismatch scores a differently rolled
/// frame, most visibly at steep tilt.
fn assert_bases_match(yaw: f32, pitch: f32) {
    let camera = Camera::new(yaw, pitch, 2.4, 42.0);
    let (forward, right, up) = camera_view_basis(yaw, pitch);

    let eps = 1e-4;
    assert!(
        (camera.forward - forward).length() < eps,
        "forward mismatch at yaw={yaw}, pitch={pitch}: camera={:?} metrics={:?}",
        camera.forward,
        forward
    );
    assert!(
        (camera.right - right).length() < eps,
        "right mismatch at yaw={yaw}, pitch={pitch}: camera={:?} metrics={:?}",
        camera.right,
        right
    );
    assert!(
        (camera.up - up).length() < eps,
        "up mismatch at yaw={yaw}, pitch={pitch}: camera={:?} metrics={:?}",
        camera.up,
        up
    );
}

#[test]
fn metrics_camera_basis_matches_render_camera_at_shallow_and_typical_angles() {
    for &yaw in &[0.0f32, 0.6, 1.5, -1.2] {
        assert_bases_match(yaw, 0.0);
        assert_bases_match(yaw, 45f32.to_radians());
    }
}

#[test]
fn metrics_camera_basis_matches_render_camera_near_the_pole() {
    // The profile sweeps reach 90 deg, so both the steep case and the exact pole must agree
    // between the metrics basis and the render camera.
    for &yaw in &[0.0f32, 0.6, 2.1] {
        assert_bases_match(yaw, 85f32.to_radians());
        assert_bases_match(yaw, 90f32.to_radians());
        assert_bases_match(yaw, -90f32.to_radians());
    }
}

#[test]
fn metrics_camera_basis_matches_render_camera_past_the_pole() {
    // Past +/-90 deg of pitch the render camera flips `world_up` so the image does not
    // mirror; the metrics basis must follow.
    for &yaw in &[0.0f32, 0.6, 2.1] {
        assert_bases_match(yaw, 100f32.to_radians());
        assert_bases_match(yaw, 135f32.to_radians());
        assert_bases_match(yaw, -110f32.to_radians());
    }
}

#[test]
fn metrics_camera_basis_right_up_forward_are_orthonormal() {
    // The shared basis helper must return an orthonormal set on every `world_up` branch.
    for &pitch_deg in &[0.0f32, 5.0, 45.0, 85.0, 90.0] {
        let (forward, right, up) = camera_view_basis(0.3, pitch_deg.to_radians());
        assert!((forward.length() - 1.0).abs() < 1e-4);
        assert!((right.length() - 1.0).abs() < 1e-4);
        assert!((up.length() - 1.0).abs() < 1e-4);
        assert!(forward.dot(right).abs() < 1e-3);
        assert!(forward.dot(up).abs() < 1e-3);
        assert!(right.dot(up).abs() < 1e-3);
    }
}

// ---------------------------------------------------------------------------------------
// Fire and Scintillation are measured from the traced facet geometry, so both must respond
// to the cut, not just the material. See src/color/metrics/mod.rs for the method. The
// assertions check orderings and physical ranges, not pinned readings.
// ---------------------------------------------------------------------------------------

#[test]
fn fire_differs_between_two_cuts_of_the_same_material() {
    // The measurement traces the facet geometry, so a different cut of the same material
    // (round brilliant vs. emerald cut) must report a different fire_index at some pose.
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();
    let diamond = GemMaterial::diamond();

    let differing_poses = COMPARISON_POSES
        .iter()
        .filter(|&&(yaw, pitch)| {
            let m_srb = evaluate_gem_optical_metrics(&srb, &diamond, yaw, pitch, studio());
            let m_ec = evaluate_gem_optical_metrics(&ec, &diamond, yaw, pitch, studio());
            println!(
                "diamond fire_index at yaw {yaw} pitch {pitch}: SRB={:.3} emerald_cut={:.3}",
                m_srb.fire_index, m_ec.fire_index
            );
            m_srb.fire_index.to_bits() != m_ec.fire_index.to_bits()
        })
        .count();
    assert!(
        differing_poses > 0,
        "fire_index must differ between the round brilliant and the emerald cut for the same \
         material at some pose -- the old closed-form formula could never do this"
    );
}

#[test]
fn fire_is_higher_for_a_high_dispersion_material_than_a_low_dispersion_one() {
    // On a fixed cut, a material with higher dispersion (larger n_F - n_C) must measure
    // higher fire than a low-dispersion one.
    let srb = StandardGemCuts::standard_round_brilliant();
    // Exact-name lookup, so the entry is certainly Cubic Zirconia.
    let cz = GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name == "Cubic Zirconia")
        .unwrap();
    let quartz = GemMaterial::by_name("Quartz").unwrap();

    let (mut cz_total, mut quartz_total) = (0.0f32, 0.0f32);
    for &(yaw, pitch) in &COMPARISON_POSES {
        cz_total += evaluate_gem_optical_metrics(&srb, &cz, yaw, pitch, studio()).fire_index;
        quartz_total +=
            evaluate_gem_optical_metrics(&srb, &quartz, yaw, pitch, studio()).fire_index;
    }
    println!(
        "SRB fire_index summed over poses: Cubic Zirconia={cz_total:.3} Quartz={quartz_total:.3}"
    );
    assert!(
        cz_total > quartz_total,
        "Cubic Zirconia (high dispersion, {cz_total:.3}) must measure higher fire than Quartz \
         (low dispersion, {quartz_total:.3}) on the same cut"
    );
}

#[test]
fn scintillation_differs_between_two_cuts_of_the_same_material() {
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();
    let diamond = GemMaterial::diamond();

    let differing_poses = COMPARISON_POSES
        .iter()
        .filter(|&&(yaw, pitch)| {
            let m_srb = evaluate_gem_optical_metrics(&srb, &diamond, yaw, pitch, studio());
            let m_ec = evaluate_gem_optical_metrics(&ec, &diamond, yaw, pitch, studio());
            println!(
                "diamond scintillation_pct at yaw {yaw} pitch {pitch}: SRB={:.3} emerald_cut={:.3}",
                m_srb.scintillation_pct, m_ec.scintillation_pct
            );
            m_srb.scintillation_pct.to_bits() != m_ec.scintillation_pct.to_bits()
        })
        .count();
    assert!(
        differing_poses > 0,
        "scintillation_pct must differ between the round brilliant and the emerald cut for \
         the same material at some pose"
    );
}

#[test]
fn the_metrics_follow_the_lighting_but_windowing_does_not() {
    // Windowing is decided before any light is consulted -- a ray that leaks through the
    // pavilion leaks under every lighting -- so it is bit-identical across presets. The ISO
    // hemisphere radiates into every upward direction, so whatever a studio rig counts as
    // a returned ray the ISO hemisphere counts too (same head shadow, same traced rays):
    // brilliance can only be higher and extinction lower under it. Every share is a share
    // of the rays that hit the stone, so together they cannot exceed the whole.
    let srb = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    let iso = LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95);

    for &(yaw, pitch) in &COMPARISON_POSES {
        let rig = evaluate_gem_optical_metrics(&srb, &diamond, yaw, pitch, studio());
        let uniform = evaluate_gem_optical_metrics(&srb, &diamond, yaw, pitch, iso);

        assert_eq!(
            rig.windowing_pct.to_bits(),
            uniform.windowing_pct.to_bits(),
            "windowing must not depend on the lighting (yaw {yaw}, pitch {pitch})"
        );
        assert!(
            uniform.brilliance_pct >= rig.brilliance_pct,
            "the ISO hemisphere ({:.3}) must return at least what the studio rig ({:.3}) does \
             (yaw {yaw}, pitch {pitch})",
            uniform.brilliance_pct,
            rig.brilliance_pct
        );
        assert!(
            uniform.extinction_pct <= rig.extinction_pct,
            "the ISO hemisphere ({:.3}) must extinguish no more than the studio rig ({:.3}) \
             (yaw {yaw}, pitch {pitch})",
            uniform.extinction_pct,
            rig.extinction_pct
        );
        for m in [rig, uniform] {
            assert!(
                m.brilliance_pct + m.extinction_pct + m.windowing_pct <= 100.0 + 1e-3,
                "brilliance, extinction and windowing are shares of one whole (got {:.3})",
                m.brilliance_pct + m.extinction_pct + m.windowing_pct
            );
        }
    }
}

#[test]
fn a_different_lighting_preset_changes_what_is_returned() {
    // The preset is part of what the metrics describe: the ring-lights rig and the light
    // tent light the same stone from different sources, so at some pose they must not
    // agree on how much light is returned.
    let srb = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    let tent = LightingPreset::LightTent.studio(1.0, 0.85, 0.95);

    let differing_poses = COMPARISON_POSES
        .iter()
        .filter(|&&(yaw, pitch)| {
            let ring = evaluate_gem_optical_metrics(&srb, &diamond, yaw, pitch, studio());
            let in_tent = evaluate_gem_optical_metrics(&srb, &diamond, yaw, pitch, tent);
            ring.brilliance_pct.to_bits() != in_tent.brilliance_pct.to_bits()
        })
        .count();
    assert!(
        differing_poses > 0,
        "brilliance must depend on the lighting preset at some pose"
    );
}

/// Ceiling of the `cv / (1 + cv)` squash. `scintillation_pct` stays below 100 by construction
/// (see `scintillation.rs`), but a printed value can round to 100.0, so tests keep this margin.
const SCINTILLATION_CEILING_MARGIN: f32 = 0.5;

#[test]
fn scintillation_is_not_a_pure_function_of_windowing_and_extinction() {
    // A fixed function of windowing_pct and extinction_pct (such as
    // 100.0 - (0.6 * windowing + 0.4 * extinction)) would carry no information of its own.
    // Scintillation depends on where the light returns from, so it must depart from such a
    // formula somewhere, while staying clear of the ceiling (a saturated 100 would differ
    // from everything trivially).
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();
    let cz = GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.name == "Cubic Zirconia")
        .unwrap();

    let mut departures = 0;
    for (cut_name, planes) in [("standard_round_brilliant", &srb), ("emerald_cut", &ec)] {
        for &(yaw, pitch) in &COMPARISON_POSES {
            let m = evaluate_gem_optical_metrics(planes, &cz, yaw, pitch, studio());
            println!(
                "{cut_name} Cubic Zirconia at yaw {yaw} pitch {pitch}: windowing={:.3} \
                 extinction={:.3} scintillation={:.3}",
                m.windowing_pct, m.extinction_pct, m.scintillation_pct
            );
            assert!(
                m.scintillation_pct < 100.0 - SCINTILLATION_CEILING_MARGIN,
                "{cut_name}: scintillation_pct must not be saturated at the ceiling, got {:.3}",
                m.scintillation_pct
            );
            let formula =
                0.4f32.mul_add(-m.extinction_pct, 0.6f32.mul_add(-m.windowing_pct, 100.0));
            if (m.scintillation_pct - formula).abs() > 1.0 {
                departures += 1;
            }
        }
    }
    assert!(
        departures > 0,
        "scintillation_pct tracks a fixed function of windowing and extinction at every pose \
         -- it would carry no information of its own"
    );
}

#[test]
fn no_built_in_material_saturates_scintillation_on_either_cut() {
    // The display mapping `cv / (1 + cv)` maps [0, inf) onto [0, 1), so it never reaches 100.
    // A plain clamp of `cv * 100` would pin most stones at exactly 100 and stop them from
    // being told apart.
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();

    let mut min_seen = f32::MAX;
    let mut max_seen = f32::MIN;

    for material in GemMaterial::all_materials() {
        for (cut_name, planes) in [("standard_round_brilliant", &srb), ("emerald_cut", &ec)] {
            // The editor's default render pose (camera yaw 0.60, pitch 0.45).
            let m = evaluate_gem_optical_metrics(planes, &material, 0.60, 0.45, studio());

            assert!(
                (m.scintillation_pct - 100.0).abs() > 0.01,
                "{} on {}: scintillation_pct saturated at the ceiling ({:.5}); the CV -> \
                 display mapping should never reach exactly 100%",
                material.name,
                cut_name,
                m.scintillation_pct
            );

            min_seen = min_seen.min(m.scintillation_pct);
            max_seen = max_seen.max(m.scintillation_pct);
        }
    }

    println!(
        "scintillation_pct spread across all built-in materials x both cuts: min={min_seen:.3} max={max_seen:.3}"
    );
    // The metric must tell stones apart: not every material/cut pair may read alike.
    assert!(
        max_seen > min_seen,
        "scintillation_pct is identical ({min_seen:.3}) for every built-in material on both \
         cuts; the metric should discriminate between different stones"
    );
}

#[test]
fn fire_and_scintillation_are_finite_and_in_range_for_every_material_on_both_cuts() {
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();

    for material in GemMaterial::all_materials() {
        for (cut_name, planes) in [("standard_round_brilliant", &srb), ("emerald_cut", &ec)] {
            let m = evaluate_gem_optical_metrics(planes, &material, 0.0, 1.4, studio());

            assert!(
                m.fire_index.is_finite() && m.fire_index >= 0.1,
                "{} on {}: fire_index must be finite and >= 0.1 (documented floor), got {}",
                material.name,
                cut_name,
                m.fire_index
            );
            // Loose ceiling across all angles: a value past it is a bug, not fire.
            assert!(
                m.fire_index < 1000.0,
                "{} on {}: fire_index suspiciously large ({}), expected a measured value \
                 well under 1000 for a built-in material",
                material.name,
                cut_name,
                m.fire_index
            );

            assert!(
                m.scintillation_pct.is_finite() && (0.0..=100.0).contains(&m.scintillation_pct),
                "{} on {}: scintillation_pct must be finite and in [0, 100], got {}",
                material.name,
                cut_name,
                m.scintillation_pct
            );
        }
    }
}

#[test]
#[ignore = "manual probe"]
fn debug_sweep_fire_energy_weighted() {
    let srb = StandardGemCuts::standard_round_brilliant();
    let ec = StandardGemCuts::emerald_cut();

    println!(
        "{:<22} {:>12} {:>12} {:>12} {:>12}",
        "material", "SRB@1.4", "SRB@0.45", "EC@1.4", "EC@0.45"
    );
    for m in GemMaterial::all_materials() {
        let srb_14 = evaluate_gem_optical_metrics(&srb, &m, 0.0, 1.4, studio());
        let srb_045 = evaluate_gem_optical_metrics(&srb, &m, 0.0, 0.45, studio());
        let ec_14 = evaluate_gem_optical_metrics(&ec, &m, 0.0, 1.4, studio());
        let ec_045 = evaluate_gem_optical_metrics(&ec, &m, 0.0, 0.45, studio());
        println!(
            "{:<22} {:>12.5} {:>12.5} {:>12.5} {:>12.5}",
            m.name, srb_14.fire_index, srb_045.fire_index, ec_14.fire_index, ec_045.fire_index
        );
    }
}

#[test]
#[ignore = "manual probe"]
fn debug_investigate_diamond_vs_quartz_ec() {
    let ec = StandardGemCuts::emerald_cut();
    let diamond = GemMaterial::diamond();
    let quartz = GemMaterial::by_name("Quartz").unwrap();

    println!("--- pitch 0.45 ---");
    let _ = evaluate_gem_optical_metrics(&ec, &diamond, 0.0, 0.45, studio());
    let _ = evaluate_gem_optical_metrics(&ec, &quartz, 0.0, 0.45, studio());
    println!("--- pitch 1.4 ---");
    let _ = evaluate_gem_optical_metrics(&ec, &diamond, 0.0, 1.4, studio());
    let _ = evaluate_gem_optical_metrics(&ec, &quartz, 0.0, 1.4, studio());
}

/// [`TILT_ANGLES_DEG`] has 181 points in exact 1 degree steps over `-90..=90`, with the
/// shared table-up point (tilt 0, pitch 90) at the midpoint. No raytracing is involved.
#[test]
fn tilt_angles_deg_spans_the_full_axis_in_exact_one_degree_steps() {
    assert_eq!(TILT_ANGLES_DEG.len(), 181);
    assert_eq!(TILT_ANGLES_DEG[0], -90.0);
    assert_eq!(TILT_ANGLES_DEG[90], 0.0);
    assert_eq!(TILT_ANGLES_DEG[180], 90.0);
    for i in 0..180 {
        assert_eq!(
            TILT_ANGLES_DEG[i + 1] - TILT_ANGLES_DEG[i],
            1.0,
            "step between index {i} and {} must be exactly 1 degree",
            i + 1
        );
    }
}

/// The 181-point full-axis sweep (tilt -90..=90 from table-up) must agree exactly with the
/// 19-point elevation sweep of `evaluate_angular_profile_at_azimuth` at every shared angle.
/// Both run the same `sample_elevation_sweep` loop, so this checks the index mapping and
/// signs, not the physics.
///
/// The map is `pitch = 90 - |tilt|`, with azimuth the positive one for `tilt >= 0` and
/// +180 degrees for `tilt < 0`. A pitch `deg` at the positive azimuth is tilt index
/// `180 - deg`; at the negative azimuth it is index `deg`.
///
/// The pitch-90 pole is checked once, against the positive-azimuth sweep only: it is
/// evaluated at the positive azimuth by convention, and `Camera::new` is only
/// azimuth-independent there up to floating-point error.
#[test]
fn full_axis_profile_agrees_with_the_five_degree_sweep_at_every_shared_angle() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    let positive_azimuth_deg = 45.0f32;

    let (full_b, full_e, full_w) =
        evaluate_full_axis_profile_at_azimuth(&planes, &diamond, positive_azimuth_deg, studio());
    let (pos_b, pos_e, pos_w) = evaluate_angular_profile_at_azimuth(
        &planes,
        &diamond,
        positive_azimuth_deg.to_radians(),
        studio(),
    );
    let (neg_b, neg_e, neg_w) = evaluate_angular_profile_at_azimuth(
        &planes,
        &diamond,
        (positive_azimuth_deg + 180.0).to_radians(),
        studio(),
    );

    // Shared table-up pole: TILT_ANGLES_DEG[90] must be the POSITIVE azimuth's own
    // pitch-90 (last entry, PROFILE_ANGLES_DEG[18] == 90.0) sample, bit-identical
    // (both are the exact same evaluate_gem_optical_metrics call).
    assert_eq!(full_b[90], pos_b[18]);
    assert_eq!(full_e[90], pos_e[18]);
    assert_eq!(full_w[90], pos_w[18]);

    for (i, &deg) in PROFILE_ANGLES_DEG.iter().enumerate() {
        if (deg - 90.0).abs() < 1e-6 {
            continue; // the shared table-up pole, already checked above
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "profile angles are whole degrees in 0..=180"
        )]
        let p = deg as usize;
        let positive_idx = 180 - p; // TILT_ANGLES_DEG[180 - p] == p (pitch, positive azimuth)
        let negative_idx = p; // TILT_ANGLES_DEG[p] == p (pitch, negative azimuth)

        assert_eq!(
            full_b[positive_idx], pos_b[i],
            "brilliance positive half at pitch {deg} deg"
        );
        assert_eq!(
            full_e[positive_idx], pos_e[i],
            "extinction positive half at pitch {deg} deg"
        );
        assert_eq!(
            full_w[positive_idx], pos_w[i],
            "windowing positive half at pitch {deg} deg"
        );

        assert_eq!(
            full_b[negative_idx], neg_b[i],
            "brilliance negative half at pitch {deg} deg"
        );
        assert_eq!(
            full_e[negative_idx], neg_e[i],
            "extinction negative half at pitch {deg} deg"
        );
        assert_eq!(
            full_w[negative_idx], neg_w[i],
            "windowing negative half at pitch {deg} deg"
        );
    }
}

/// Every sample of the full-axis sweep lies in `0..=100`. This is only a floor: an unfilled
/// `0.0` slot passes it, so the exact-agreement test above does the real checking.
#[test]
fn full_axis_profile_values_are_in_range_for_every_axis() {
    use pleochroma::color::metrics::PROFILE_AZIMUTHS_DEG;

    let planes = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();

    for &azimuth_deg in &PROFILE_AZIMUTHS_DEG {
        let (b, e, w) =
            evaluate_full_axis_profile_at_azimuth(&planes, &diamond, azimuth_deg, studio());
        assert_eq!(b.len(), 181);
        assert_eq!(e.len(), 181);
        assert_eq!(w.len(), 181);
        for i in 0..181 {
            assert!(
                (0.0..=100.0).contains(&b[i]),
                "brilliance[{i}] = {} out of range",
                b[i]
            );
            assert!(
                (0.0..=100.0).contains(&e[i]),
                "extinction[{i}] = {} out of range",
                e[i]
            );
            assert!(
                (0.0..=100.0).contains(&w[i]),
                "windowing[{i}] = {} out of range",
                w[i]
            );
        }
    }
}
