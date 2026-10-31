//! The public `_finished` twins of the face-up metrics entry points: a list with no frosted
//! facet is the plain entry point bit for bit, a frosted list changes the figures
//! reproducibly, and the cache key follows the finishes.

use super::{
    FinishedStone, GemOpticalMetrics, MetricsCacheKey, compute_or_reuse_metrics_finished,
    compute_or_reuse_metrics_finished_cancellable, compute_or_reuse_metrics_geom,
    compute_or_reuse_pose_metrics_finished, compute_or_reuse_pose_metrics_geom,
    evaluate::placeholder_metrics, evaluate_angular_profile_at_azimuth_finished,
    evaluate_angular_profile_at_azimuth_finished_stepped, evaluate_angular_profile_at_azimuth_geom,
    evaluate_angular_profile_at_azimuth_geom_stepped, evaluate_angular_profile_finished,
    evaluate_angular_profile_finished_stepped, evaluate_angular_profile_geom,
    evaluate_angular_profile_geom_stepped,
    evaluate_full_axis_profile_at_azimuth_stepped_finished_threaded,
    evaluate_full_axis_profile_at_azimuth_stepped_geom_threaded,
    evaluate_gem_optical_metrics_finished, evaluate_gem_optical_metrics_finished_core,
    evaluate_gem_optical_metrics_geom, evaluate_gem_optical_metrics_geom_core,
    evaluate_gem_optical_metrics_with_tone_finished,
    evaluate_gem_optical_metrics_with_tone_finished_core,
    evaluate_gem_optical_metrics_with_tone_geom, finishes_hash,
};
use crate::{
    geometry::{
        cuts::StandardGemCuts,
        girdle_facet_finishes,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::{
        materials::GemMaterial,
        raytracer::{EnvironmentSource, FacetFinish, LightingPreset},
    },
};
use glam::Vec3;
use std::f32::consts::FRAC_PI_2;

type Profile19 = ([f32; 19], [f32; 19], [f32; 19]);
type Profile181 = ([f32; 181], [f32; 181], [f32; 181]);

/// The light pose these tests run under.
const fn studio() -> EnvironmentSource<'static> {
    LightingPreset::RingLights.studio(1.0, 0.85, 0.95)
}

fn metric_bits(m: GemOpticalMetrics) -> [u32; 6] {
    [
        m.brilliance_pct.to_bits(),
        m.fire_index.to_bits(),
        m.scintillation_pct.to_bits(),
        m.windowing_pct.to_bits(),
        m.extinction_pct.to_bits(),
        m.doubling_arcmin.to_bits(),
    ]
}

fn profile_bits(p: &Profile19) -> Vec<u32> {
    p.0.iter()
        .chain(&p.1)
        .chain(&p.2)
        .map(|v| v.to_bits())
        .collect()
}

fn long_profile_bits(p: &Profile181) -> Vec<u32> {
    p.0.iter()
        .chain(&p.1)
        .chain(&p.2)
        .map(|v| v.to_bits())
        .collect()
}

fn grooved_tools() -> [ToolPrimitive; 1] {
    [ToolPrimitive::cylinder(
        Vec3::new(0.0, 0.44, 0.0),
        Vec3::X,
        0.3,
        2.0,
    )]
}

/// An empty list and a list of only polished facets are the plain entry points, bit for bit,
/// for every pose twin, planar and grooved stone alike.
#[test]
fn an_unfrosted_list_is_the_plain_pose_entry_point_bit_for_bit() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let tools = grooved_tools();
    let material = GemMaterial::diamond();
    for geom in [
        StoneGeometry::planes_only(&planes),
        StoneGeometry {
            planes: &planes,
            tools: &tools,
        },
    ] {
        let all_polished = vec![FacetFinish::Polished; geom.planes.len() + geom.tools.len()];
        let empty: &[FacetFinish] = &[];
        for finishes in [empty, all_polished.as_slice()] {
            let stone = FinishedStone::new(geom, finishes);
            for (yaw, pitch) in [(0.0, FRAC_PI_2), (0.6, 0.45)] {
                let plain =
                    evaluate_gem_optical_metrics_geom(geom, &material, yaw, pitch, studio());
                let finished =
                    evaluate_gem_optical_metrics_finished(stone, &material, yaw, pitch, studio());
                assert_eq!(metric_bits(finished), metric_bits(plain));

                let plain_core =
                    evaluate_gem_optical_metrics_geom_core(geom, &material, yaw, pitch, studio());
                let finished_core = evaluate_gem_optical_metrics_finished_core(
                    stone,
                    &material,
                    yaw,
                    pitch,
                    studio(),
                );
                assert_eq!(metric_bits(finished_core), metric_bits(plain_core));

                let (plain_m, plain_tone) = evaluate_gem_optical_metrics_with_tone_geom(
                    geom,
                    &material,
                    yaw,
                    pitch,
                    studio(),
                );
                let (m, tone) = evaluate_gem_optical_metrics_with_tone_finished(
                    stone,
                    &material,
                    yaw,
                    pitch,
                    studio(),
                );
                assert_eq!(metric_bits(m), metric_bits(plain_m));
                assert_eq!(tone, plain_tone);

                let (core_m, core_tone) = evaluate_gem_optical_metrics_with_tone_finished_core(
                    stone,
                    &material,
                    yaw,
                    pitch,
                    studio(),
                );
                assert_eq!(core_tone, plain_tone);
                assert_eq!(
                    metric_bits(core_m)[0],
                    metric_bits(plain_m)[0],
                    "the Fire-less pass keeps brilliance"
                );
            }
        }
    }
}

/// The 19-point and 181-point profile twins, stepped and not, are the plain entry points with
/// no frosted facet.
#[test]
fn an_unfrosted_list_is_the_plain_profile_entry_point_bit_for_bit() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let geom = StoneGeometry::planes_only(&planes);
    let all_polished = vec![FacetFinish::Polished; planes.len()];
    let empty: &[FacetFinish] = &[];
    let plain19 = evaluate_angular_profile_at_azimuth_geom(geom, &material, 0.7, studio());
    let plain19_stepped = evaluate_angular_profile_at_azimuth_geom_stepped(
        geom,
        &material,
        0.7,
        studio(),
        &mut || true,
    )
    .expect("an agreeing hook completes");
    let plain_zero = evaluate_angular_profile_geom(geom, &material, studio());
    let plain_zero_stepped =
        evaluate_angular_profile_geom_stepped(geom, &material, studio(), &mut || true)
            .expect("an agreeing hook completes");
    let plain181 = evaluate_full_axis_profile_at_azimuth_stepped_geom_threaded(
        geom,
        &material,
        45.0,
        studio(),
        Some(2),
        &mut || true,
    )
    .expect("an agreeing hook completes");

    for finishes in [empty, all_polished.as_slice()] {
        let stone = FinishedStone::new(geom, finishes);
        let at = evaluate_angular_profile_at_azimuth_finished(stone, &material, 0.7, studio());
        assert_eq!(profile_bits(&at), profile_bits(&plain19));
        let at_stepped = evaluate_angular_profile_at_azimuth_finished_stepped(
            stone,
            &material,
            0.7,
            studio(),
            &mut || true,
        )
        .expect("an agreeing hook completes");
        assert_eq!(profile_bits(&at_stepped), profile_bits(&plain19_stepped));
        assert_eq!(profile_bits(&at_stepped), profile_bits(&plain19));

        let zero = evaluate_angular_profile_finished(stone, &material, studio());
        assert_eq!(profile_bits(&zero), profile_bits(&plain_zero));
        let zero_stepped =
            evaluate_angular_profile_finished_stepped(stone, &material, studio(), &mut || true)
                .expect("an agreeing hook completes");
        assert_eq!(
            profile_bits(&zero_stepped),
            profile_bits(&plain_zero_stepped)
        );

        let long = evaluate_full_axis_profile_at_azimuth_stepped_finished_threaded(
            stone,
            &material,
            45.0,
            studio(),
            Some(2),
            &mut || true,
        )
        .expect("an agreeing hook completes");
        assert_eq!(long_profile_bits(&long), long_profile_bits(&plain181));
    }
}

/// A frosted stone's figures differ from the polished stone's, stay finite, agree between the
/// full, Fire-less and tone passes where they must, and are the same bits on any thread.
#[test]
fn a_frosted_stone_changes_the_pose_figures_and_is_deterministic_across_threads() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let geom = StoneGeometry::planes_only(&planes);
    let everything = vec![FacetFinish::Frosted; planes.len()];
    let stone = FinishedStone::new(geom, &everything);
    let (yaw, pitch) = (0.6, 0.45);

    let polished = evaluate_gem_optical_metrics_geom(geom, &material, yaw, pitch, studio());
    let frosted = evaluate_gem_optical_metrics_finished(stone, &material, yaw, pitch, studio());
    assert_ne!(metric_bits(frosted), metric_bits(polished));
    assert!(
        metric_bits(frosted)
            .iter()
            .all(|&b| f32::from_bits(b).is_finite())
    );

    // The Fire-less pass keeps the three percentages and reports the placeholders.
    let core = evaluate_gem_optical_metrics_finished_core(stone, &material, yaw, pitch, studio());
    assert_eq!(
        core.brilliance_pct.to_bits(),
        frosted.brilliance_pct.to_bits()
    );
    assert_eq!(
        core.extinction_pct.to_bits(),
        frosted.extinction_pct.to_bits()
    );
    assert_eq!(
        core.windowing_pct.to_bits(),
        frosted.windowing_pct.to_bits()
    );
    let placeholder = placeholder_metrics();
    assert_eq!(core.fire_index.to_bits(), placeholder.fire_index.to_bits());
    assert_eq!(
        core.scintillation_pct.to_bits(),
        placeholder.scintillation_pct.to_bits()
    );

    // The tone pass is the same grid pass: identical metrics, and the Fire-less tone agrees.
    let (tone_metrics, tone) =
        evaluate_gem_optical_metrics_with_tone_finished(stone, &material, yaw, pitch, studio());
    assert_eq!(metric_bits(tone_metrics), metric_bits(frosted));
    let (core_metrics, core_tone) = evaluate_gem_optical_metrics_with_tone_finished_core(
        stone,
        &material,
        yaw,
        pitch,
        studio(),
    );
    assert_eq!(core_tone, tone);
    assert_eq!(
        core_metrics.brilliance_pct.to_bits(),
        frosted.brilliance_pct.to_bits()
    );

    let runs: Vec<[u32; 6]> = std::thread::scope(|scope| {
        #[expect(
            clippy::needless_collect,
            reason = "collect spawns every thread before the first join"
        )]
        let workers: Vec<_> = (0..3)
            .map(|_| {
                scope.spawn(|| {
                    metric_bits(evaluate_gem_optical_metrics_finished(
                        stone,
                        &material,
                        yaw,
                        pitch,
                        studio(),
                    ))
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a metrics worker finishes"))
            .collect()
    });
    for run in runs {
        assert_eq!(run, metric_bits(frosted));
    }
}

/// The 19-point profile of a frosted stone differs from the polished one, is reproducible, and
/// the stepped twins keep the hook contract.
#[test]
fn a_frosted_stones_angular_profile_differs_and_the_hook_still_stops_it() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let geom = StoneGeometry::planes_only(&planes);
    let finishes = girdle_facet_finishes(&planes);
    assert!(finishes.contains(&FacetFinish::Frosted));
    let stone = FinishedStone::new(geom, &finishes);

    let polished = evaluate_angular_profile_at_azimuth_geom(geom, &material, 0.0, studio());
    let frosted = evaluate_angular_profile_at_azimuth_finished(stone, &material, 0.0, studio());
    assert_ne!(profile_bits(&frosted), profile_bits(&polished));
    let again = evaluate_angular_profile_at_azimuth_finished(stone, &material, 0.0, studio());
    assert_eq!(profile_bits(&frosted), profile_bits(&again));
    let at_zero = evaluate_angular_profile_finished(stone, &material, studio());
    assert_eq!(profile_bits(&at_zero), profile_bits(&frosted));

    let mut asked = 0;
    let stepped =
        evaluate_angular_profile_finished_stepped(stone, &material, studio(), &mut || {
            asked += 1;
            true
        })
        .expect("an agreeing hook completes");
    assert_eq!(asked, 19);
    assert_eq!(profile_bits(&stepped), profile_bits(&frosted));

    let mut asked = 0;
    let cut_short =
        evaluate_angular_profile_finished_stepped(stone, &material, studio(), &mut || {
            asked += 1;
            asked < 5
        });
    assert!(cut_short.is_none());
    assert_eq!(asked, 5);
}

/// The long full-axis stepped twin on a frosted stone is reproducible and differs from the
/// polished sweep.
#[test]
fn a_frosted_full_axis_sweep_through_the_stepped_twin_differs_from_the_polished_one() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let geom = StoneGeometry::planes_only(&planes);
    let finishes = girdle_facet_finishes(&planes);
    let run = |stone: FinishedStone<'_>, threads: usize| {
        evaluate_full_axis_profile_at_azimuth_stepped_finished_threaded(
            stone,
            &material,
            0.0,
            studio(),
            Some(threads),
            &mut || true,
        )
        .expect("an agreeing hook completes")
    };
    let polished = run(FinishedStone::polished(geom), 1);
    let frosted = run(FinishedStone::new(geom, &finishes), 1);
    let threaded = run(FinishedStone::new(geom, &finishes), 3);
    assert_ne!(long_profile_bits(&frosted), long_profile_bits(&polished));
    assert_eq!(long_profile_bits(&frosted), long_profile_bits(&threaded));
}

/// A polished stone's cache key is the key it always had; a frosting change changes it.
#[test]
fn the_cache_key_follows_the_finishes_and_a_polished_key_is_unchanged() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let geom = StoneGeometry::planes_only(&planes);
    let key = |finishes: &[FacetFinish]| {
        MetricsCacheKey::new_finished(
            FinishedStone::new(geom, finishes),
            &material,
            0.6,
            0.45,
            studio(),
        )
    };
    let legacy = MetricsCacheKey::new_geom(geom, &material, 0.6, 0.45, studio());
    let plain = MetricsCacheKey::new(&planes, &material, 0.6, 0.45, studio());
    assert_eq!(legacy, plain);
    assert_eq!(key(&[]), legacy);
    assert_eq!(key(&vec![FacetFinish::Polished; planes.len()]), legacy);

    let girdle = girdle_facet_finishes(&planes);
    let everything = vec![FacetFinish::Frosted; planes.len()];
    assert_ne!(key(&girdle), legacy);
    assert_ne!(key(&everything), legacy);
    assert_ne!(key(&girdle), key(&everything));
    assert_eq!(key(&girdle), key(&girdle));
}

/// The fingerprint counts only which facets are frosted: zero for none, equal for lists that
/// mean the same stone, different for different frosting.
#[test]
fn the_finishes_hash_is_zero_for_a_polished_stone_and_canonical_otherwise() {
    assert_eq!(finishes_hash(&[]), 0);
    assert_eq!(finishes_hash(&[FacetFinish::Polished; 40]), 0);

    let mut one = vec![FacetFinish::Polished; 10];
    one[3] = FacetFinish::Frosted;
    let mut padded = one.clone();
    padded.extend([FacetFinish::Polished; 5]);
    let mut other = vec![FacetFinish::Polished; 10];
    other[4] = FacetFinish::Frosted;
    assert_ne!(finishes_hash(&one), 0);
    assert_eq!(finishes_hash(&one), finishes_hash(&padded));
    assert_ne!(finishes_hash(&one), finishes_hash(&other));
}

/// The polished cache wrappers and the `_finished` twins hand back the same bits.
#[test]
fn the_polished_cache_functions_are_the_finished_ones_with_no_frosting() {
    let planes = StandardGemCuts::emerald_cut();
    let material = GemMaterial::diamond();
    let geom = StoneGeometry::planes_only(&planes);
    let stone = FinishedStone::polished(geom);

    let (mut a, mut b) = (None, None);
    let pose_plain =
        compute_or_reuse_pose_metrics_geom(&mut a, geom, &material, 0.6, 0.4, studio());
    let pose_finished =
        compute_or_reuse_pose_metrics_finished(&mut b, stone, &material, 0.6, 0.4, studio());
    assert_eq!(metric_bits(pose_plain), metric_bits(pose_finished));

    let (mut c, mut d) = (None, None);
    let (m0, b0, e0, w0) =
        compute_or_reuse_metrics_geom(&mut c, geom, &material, 0.6, 0.4, studio());
    let (m1, b1, e1, w1) =
        compute_or_reuse_metrics_finished(&mut d, stone, &material, 0.6, 0.4, studio());
    assert_eq!(metric_bits(m0), metric_bits(m1));
    assert_eq!(profile_bits(&(b0, e0, w0)), profile_bits(&(b1, e1, w1)));
}

/// Frosting a facet, or unfrosting it, recomputes the pose metrics AND the profile; whatever
/// the cache serves equals the direct call for the same finishes.
#[test]
fn a_frosting_edit_invalidates_the_pose_and_the_profile() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let geom = StoneGeometry::planes_only(&planes);
    let everything = vec![FacetFinish::Frosted; planes.len()];
    let polished = FinishedStone::polished(geom);
    let frosted = FinishedStone::new(geom, &everything);
    let (yaw, pitch) = (0.6, 0.45);

    let mut cache = None;
    let first =
        compute_or_reuse_metrics_finished(&mut cache, polished, &material, yaw, pitch, studio());
    let second =
        compute_or_reuse_metrics_finished(&mut cache, frosted, &material, yaw, pitch, studio());
    let back =
        compute_or_reuse_metrics_finished(&mut cache, polished, &material, yaw, pitch, studio());

    let direct_polished =
        evaluate_gem_optical_metrics_finished(polished, &material, yaw, pitch, studio());
    let direct_frosted =
        evaluate_gem_optical_metrics_finished(frosted, &material, yaw, pitch, studio());
    let profile_polished = evaluate_angular_profile_finished(polished, &material, studio());
    let profile_frosted = evaluate_angular_profile_finished(frosted, &material, studio());

    assert_ne!(metric_bits(direct_polished), metric_bits(direct_frosted));
    assert_ne!(
        profile_bits(&profile_polished),
        profile_bits(&profile_frosted)
    );
    assert_eq!(metric_bits(first.0), metric_bits(direct_polished));
    assert_eq!(
        profile_bits(&(first.1, first.2, first.3)),
        profile_bits(&profile_polished)
    );
    assert_eq!(metric_bits(second.0), metric_bits(direct_frosted));
    assert_eq!(
        profile_bits(&(second.1, second.2, second.3)),
        profile_bits(&profile_frosted)
    );
    assert_eq!(metric_bits(back.0), metric_bits(direct_polished));
    assert_eq!(
        profile_bits(&(back.1, back.2, back.3)),
        profile_bits(&profile_polished)
    );

    // The pose-only entry follows the finishes too.
    let mut pose_cache = None;
    let pose_polished = compute_or_reuse_pose_metrics_finished(
        &mut pose_cache,
        polished,
        &material,
        yaw,
        pitch,
        studio(),
    );
    let pose_frosted = compute_or_reuse_pose_metrics_finished(
        &mut pose_cache,
        frosted,
        &material,
        yaw,
        pitch,
        studio(),
    );
    assert_eq!(metric_bits(pose_polished), metric_bits(direct_polished));
    assert_eq!(metric_bits(pose_frosted), metric_bits(direct_frosted));
}

/// Orbiting under equal finishes reuses the profile (the hook is never asked); a frosting
/// change recomputes it (the hook is asked once per pose). A finish list that only differs by
/// trailing polished entries is the same stone and keeps the entry.
#[test]
fn the_profile_is_reused_across_the_orbit_while_the_finishes_are_equal() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let geom = StoneGeometry::planes_only(&planes);
    let everything = vec![FacetFinish::Frosted; planes.len()];
    let mut padded = everything.clone();
    padded.extend([FacetFinish::Polished; 4]);
    let frosted = FinishedStone::new(geom, &everything);
    let frosted_padded = FinishedStone::new(geom, &padded);

    let mut cache = None;
    let mut asked = 0;
    let first = compute_or_reuse_metrics_finished_cancellable(
        &mut cache,
        frosted,
        &material,
        0.60,
        0.45,
        studio(),
        &mut || {
            asked += 1;
            true
        },
    )
    .expect("an agreeing hook completes");
    assert_eq!(asked, 19);

    let mut asked = 0;
    let moved = compute_or_reuse_metrics_finished_cancellable(
        &mut cache,
        frosted_padded,
        &material,
        0.75,
        0.30,
        studio(),
        &mut || {
            asked += 1;
            true
        },
    )
    .expect("a cached profile is served");
    assert_eq!(
        asked, 0,
        "equal finishes reuse the profile across the orbit"
    );
    assert_eq!(
        profile_bits(&(first.1, first.2, first.3)),
        profile_bits(&(moved.1, moved.2, moved.3))
    );
    let direct = evaluate_gem_optical_metrics_finished(frosted, &material, 0.75, 0.30, studio());
    assert_eq!(metric_bits(moved.0), metric_bits(direct));

    let mut asked = 0;
    let polished = compute_or_reuse_metrics_finished_cancellable(
        &mut cache,
        FinishedStone::polished(geom),
        &material,
        0.75,
        0.30,
        studio(),
        &mut || {
            asked += 1;
            true
        },
    )
    .expect("an agreeing hook completes");
    assert_eq!(asked, 19, "a frosting change recomputes the profile");
    let direct_profile =
        evaluate_angular_profile_finished(FinishedStone::polished(geom), &material, studio());
    assert_eq!(
        profile_bits(&(polished.1, polished.2, polished.3)),
        profile_bits(&direct_profile)
    );
}
