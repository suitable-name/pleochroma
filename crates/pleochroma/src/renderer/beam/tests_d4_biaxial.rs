//! Biaxial birefringence in the beam tree (beam guide §10.3, §16.1, §16.2).
//!
//! Exact property tests: the retracer's biaxial entry, reflection and exit steps against the
//! tracer's own `expand_at` (bit for bit on `S` and `k`), the shared Poynting direction and the
//! 50/50 weights of the resolved modes, the optic axes, the state replay of entry children, and the
//! uniaxial and isotropic paths unchanged by the biaxial branches.
//!
//! Statistical rows: branch-filtered equivalence of tanzanite and peridot on the cube and the
//! brilliant under the studio rig on the CPU, with the optic-axis guard share printed and asserted.
//! The thresholds are named constants; numbers only a run can give are printed with `eprintln!`.

use std::sync::atomic::AtomicBool;

use glam::{DVec2, Vec3};

use super::{
    BeamSettings,
    beam::{BranchPrefix, GROUP, OPTIC_AXIS_GUARD_RAD, SpectralInterval, SpectralNode},
    birefringence::{
        Wave, beam_near_axis, beam_near_optic_axis, biaxial_entry_wave, biaxial_optic_axes,
        biaxial_ray, biaxial_reflect_wave, exit_wave, is_biaxial_crystal, is_uniaxial, mode_index,
        near_biaxial_axis, ray_is_total_reflection, state_is_total_reflection, uniaxial_ray,
    },
    entry::refract_entry,
    frame::{BeamFrame, BeamImage},
    geom::polygon::ConvexPolygon,
    interact::InteractKit,
    leaf::{ReflectionKit, node_count},
    primary::{TileRect, image_polygon, primary_beams},
    propagate::retrace_state,
    raster::rasterise_polygon,
    residual::KeptIndex,
    retrace::{RetraceKit, camera_direction, exact_ray_at, is_total_reflection},
    stats::{BeamStats, DropReason},
    stone::BeamStone,
    tests_support::{BeamRig, MAX_BOUNCES, diamond, glass, sapphire},
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{
        birefringence::BirefringenceParams,
        fluorescence::Fluorescence,
        materials::GemMaterial,
        polarization::StokesVector,
        raytracer::{
            BACKDROP_GREY, EnvironmentSource, LightingPreset, Ray, cranley_patterson_rotate,
            hash_u32, radical_inverse_base,
            surface::SurfaceScene,
            transport::{
                PreparedMaterial,
                reference::{KeptFn, trace_spectral_ray_reference_prepared},
            },
        },
    },
    renderer::{
        frame_scene::FrameScene,
        path_cache::{
            bounce::{EventKind, Expansion, ExplicitHit, PathState, expand_at},
            build::HeroContext,
        },
        test_stats::{assert_z_stats, compare_streams, draw},
    },
};

/// The side of the square test frame in pixels.
const FRAME: u32 = 32;
/// The tile side of the beam render in pixels.
const TILE_SIZE: u32 = 8;
/// The worker threads of the beam renders.
const THREADS: usize = 4;
/// The spectral node count of the tests.
const N_LAMBDA: u32 = 48;
/// The node whose centre wavelength the step tests run at.
const TEST_NODE: u16 = 24;
/// The size of one equivalence row: the reference samples per compared pixel, the most pixels
/// compared and the spectral nodes of the beams.
#[derive(Clone, Copy)]
struct Size {
    /// Reference samples per compared pixel.
    samples: u32,
    /// The most pixels compared per case.
    max_pixels: usize,
    /// The spectral nodes of the beams.
    nodes: u32,
}

impl Size {
    /// The default beam settings at the size's node count.
    fn settings(self) -> BeamSettings {
        BeamSettings {
            spectral_nodes: self.nodes,
            ..BeamSettings::default()
        }
    }
}

/// The full rows: 2048 samples, 16 pixels, 48 nodes. Their tests are `#[ignore]`d.
const FULL: Size = Size {
    samples: 2048,
    max_pixels: 16,
    nodes: N_LAMBDA,
};

/// The small row that stays in a default run: 256 samples, 8 pixels, 16 nodes (two groups). The
/// z-statistics are normalised by the estimated spread, so `Z_LIMIT` holds unchanged at the
/// smaller sample count; the row only guards against gross breakage. The 32 pixel frame stays
/// because the faceted stone's kept regions leave few interior pixels in a smaller one.
const SMALL: Size = Size {
    samples: 256,
    max_pixels: 8,
    nodes: 16,
};
/// The fewest fully covered interior pixels a case must offer.
const MIN_PIXELS: usize = 4;
/// Samples per candidate pixel for the "every sample hits the stone" screen.
const SCREEN_SAMPLES: u32 = 64;
/// The largest |z| allowed on interior pixels.
const Z_LIMIT: f64 = 5.0;
/// The tolerance of a pixel counting as fully covered, in square pixels.
const COVERAGE_EPS: f64 = 1.0e-9;
/// A piece covering less than one pixel by more than this marks a boundary pixel.
const PARTIAL_EPS: f64 = 1.0e-6;
/// The largest share of the stone's energy the optic-axis guard may send to the residual (§10.3).
const GUARD_SHARE_LIMIT: f64 = 1.0e-3;
/// The largest angle between a retraced and a propagated entry ray, in radians.
const RETRACE_ANGLE_TOLERANCE: f64 = 1.0e-5;
/// A slice of the guard angle, for the "just inside" and "just outside" checks (radians).
const GUARD_PROBE_RAD: f64 = 1.0e-4;
/// The largest gap between the two roots of Fresnel's equation at an optic axis, in index units
/// (the quadratic's discriminant is a difference of nearly equal `f32` values there).
const AXIS_GAP_LIMIT: f32 = 3.0e-3;
/// The most a resolved mode's entry child may weigh: half of the incident unit intensity.
const HALF_WEIGHT: f32 = 0.5;
/// The least: half the incident intensity times a transmittance above this (near-normal incidence).
const MIN_TRANSMITTANCE: f32 = 0.8;
/// The slack of the energy sums, in intensity units.
const ENERGY_SLACK: f32 = 1.0e-4;
/// The wave normals the internal step tests run: partial and total reflection in all four quadrants
/// of the facet's tangent plane, none on a principal axis of a built-in.
const INTERNAL_DIRECTIONS: [[f32; 3]; 4] = [
    [0.95, 0.2, 0.2],
    [0.5, 0.6, 0.62],
    [0.9, -0.3, 0.25],
    [0.7, 0.1, -0.6],
];

/// The whole test frame as one tile.
const fn whole_frame() -> TileRect {
    TileRect {
        x0: 0,
        y0: 0,
        x1: FRAME,
        y1: FRAME,
    }
}

/// The wavelength the step tests run at.
fn test_lambda() -> f32 {
    SpectralNode(TEST_NODE).centre_nm(N_LAMBDA)
}

/// The bit pattern of a vector.
fn bits3(v: Vec3) -> [u32; 3] {
    v.to_array().map(f32::to_bits)
}

/// A built-in material by name.
fn named(name: &str) -> GemMaterial {
    GemMaterial::by_name(name).unwrap_or_else(|| panic!("{name} must resolve"))
}

/// The tracer's hero context for the test wavelength (all eight channels equal, hero 0).
fn hero<'a>(prepared: &PreparedMaterial<'a>) -> HeroContext<'a> {
    let (ctx, cache) = prepared.ray_context([test_lambda(); GROUP], 0);
    HeroContext { ctx, cache }
}

/// Unit Stokes vectors for all channels.
fn unit_stokes() -> [StokesVector; GROUP] {
    [StokesVector::unpolarized(1.0); GROUP]
}

/// The angle between two directions in radians.
fn angle(a: Vec3, b: Vec3) -> f64 {
    let (a, b) = (a.as_dvec3(), b.as_dvec3());
    a.cross(b).length().atan2(a.dot(b))
}

// ---- the retracer's steps against the tracer's ----

/// The expansion of a wave normal `k` inside the unit cube hitting facet 0 (outward normal `+X`).
fn internal_expansion(hero: &HeroContext<'_>, rig: &BeamRig, k: Vec3, is_ext: bool) -> Expansion {
    let point = k * (1.0 / k.x);
    let state = PathState {
        k,
        inside: true,
        is_ext,
        stokes: unit_stokes(),
    };
    let hit = ExplicitHit {
        point,
        facet: 0,
        normal: Vec3::X,
    };
    // The segment ends where it starts: `expand_at` applies the Beer-Lambert absorption of the
    // interior segment `origin -> point`, and tanzanite absorbs strongly (a 2 unit segment keeps
    // about 1.3 % of the energy), which the energy sums of these tests must not see.
    let ray = Ray {
        origin: point,
        dir: k,
    };
    expand_at(hero, &rig.planes, state, ray, hit, Some(false), None)
}

/// The test directions as unit wave normals.
fn internal_waves() -> Vec<Vec3> {
    INTERNAL_DIRECTIONS
        .iter()
        .map(|&d| Vec3::from(d).normalize())
        .collect()
}

#[test]
fn the_retracers_biaxial_internal_steps_equal_the_tracers_bit_for_bit() {
    let (lambda, facing) = (test_lambda(), -Vec3::X);
    for name in ["Tanzanite", "Peridot"] {
        let rig = BeamRig::cube(named(name));
        assert!(is_biaxial_crystal(&rig.material), "{name}");
        let prepared = PreparedMaterial::new(&rig.material);
        let hero = hero(&prepared);
        let (mut reflections, mut exits) = (0_usize, 0_usize);
        for k in internal_waves() {
            for is_ext in [false, true] {
                let expansion = internal_expansion(&hero, &rig, k, is_ext);
                let wave = Wave { k, s: k, is_ext };
                for child in &expansion.children {
                    if child.kind == EventKind::Transmit {
                        let exit = exit_wave(&rig.material, lambda, wave, facing)
                            .expect("the tracer transmitted, so the retracer must");
                        assert_eq!(bits3(exit.k), bits3(child.state.k), "{name} exit k, {k:?}");
                        assert_eq!(bits3(exit.s), bits3(child.s), "{name} exit S, {k:?}");
                        assert_eq!(exit.is_ext, child.state.is_ext);
                        exits += 1;
                    } else {
                        let step = (true, child.state.is_ext);
                        let next = biaxial_reflect_wave(&rig.material, lambda, wave, facing, step);
                        assert_eq!(bits3(next.k), bits3(child.state.k), "{name} refl k, {k:?}");
                        assert_eq!(bits3(next.s), bits3(child.s), "{name} refl S, {k:?}");
                        assert_eq!(next.is_ext, child.state.is_ext);
                        reflections += 1;
                    }
                }
                let transmitted = expansion
                    .children
                    .iter()
                    .any(|c| c.kind == EventKind::Transmit);
                assert_eq!(
                    exit_wave(&rig.material, lambda, wave, facing).is_some(),
                    transmitted,
                    "{name}: the critical decision of {k:?}, ext {is_ext}"
                );
            }
        }
        eprintln!("{name}: {reflections} reflections, {exits} exits compared");
        assert!(reflections > 0 && exits > 0, "{name}");
    }
}

#[test]
fn relabelled_biaxial_children_share_one_poynting_direction_and_split_the_energy_evenly() {
    let rig = BeamRig::cube(named("Tanzanite"));
    let prepared = PreparedMaterial::new(&rig.material);
    let hero = hero(&prepared);
    let (mut pairs, mut total_reflections) = (0_usize, 0_usize);
    for k in internal_waves() {
        for is_ext in [false, true] {
            let expansion = internal_expansion(&hero, &rig, k, is_ext);
            let reflected: Vec<_> = expansion
                .children
                .iter()
                .filter(|c| c.kind != EventKind::Transmit)
                .collect();
            let exits: Vec<f32> = expansion
                .children
                .iter()
                .filter(|c| c.kind == EventKind::Transmit)
                .map(|c| c.state.stokes[0].i)
                .collect();
            let transmitted: f32 = exits.iter().sum();
            let [first, second] = reflected.as_slice() else {
                panic!(
                    "a biaxial stone relabels every reflection into both modes: {k:?}, parent ext \
                     {is_ext}: {} reflected children, {} exits {exits:?}",
                    reflected.len(),
                    exits.len()
                );
            };
            let context = format!("k {k:?}, parent ext {is_ext}, exits {exits:?}");
            assert_ne!(first.state.is_ext, second.state.is_ext, "{context}");
            assert_eq!(
                bits3(first.s),
                bits3(second.s),
                "the shared pre-relabel S: {context}"
            );
            assert_eq!(
                bits3(first.state.k),
                bits3(second.state.k),
                "the shared k: {context}"
            );
            // The fixed 50/50 split: both modes carry half of the reflected energy, bit for bit.
            assert_eq!(
                first.state.stokes[0].i.to_bits(),
                second.state.stokes[0].i.to_bits(),
                "the 50/50 split ({} vs {}): {context}",
                first.state.stokes[0].i,
                second.state.stokes[0].i
            );
            let sum = first.state.stokes[0].i + second.state.stokes[0].i + transmitted;
            assert!(
                sum <= 1.0 + ENERGY_SLACK,
                "energy created: {sum} (reflected {} + {}, transmitted {transmitted}): {context}",
                first.state.stokes[0].i,
                second.state.stokes[0].i
            );
            if exits.is_empty() {
                assert!(
                    (sum - 1.0).abs() <= ENERGY_SLACK,
                    "TIR loses energy: {sum}: {context}"
                );
                total_reflections += 1;
            }
            pairs += 1;
        }
    }
    eprintln!("biaxial pairs: {pairs} relabelled pairs, {total_reflections} total reflections");
    assert!(
        pairs > 0 && total_reflections > 0,
        "{pairs} relabelled pairs, {total_reflections} total reflections"
    );
}

/// The dense and original facet the camera ray through `point` enters.
fn entry_facet(rig: &BeamRig, stone: &BeamStone, point: DVec2) -> (u32, u32) {
    let dir = camera_direction(&rig.camera, (FRAME, FRAME), point).normalize();
    let origin = rig.camera.origin.as_dvec3();
    let mut best: Option<(f64, u32)> = None;
    for facet in 0..u32::try_from(stone.facet_count()).expect("few facets") {
        let (n, m) = stone.plane(facet);
        let denom = n.dot(dir);
        if denom >= 0.0 {
            continue;
        }
        let t = (m - n.dot(origin)) / denom;
        if t > 0.0 && best.is_none_or(|(bt, _)| t < bt) {
            best = Some((t, facet));
        }
    }
    let facet = best.expect("the camera ray meets the cube").1;
    (facet, stone.original_facet(facet))
}

#[test]
fn the_retracers_biaxial_entry_step_equals_the_tracers_refraction_bit_for_bit() {
    let lambda = test_lambda();
    for name in ["Tanzanite", "Peridot"] {
        let rig = BeamRig::cube(named(name));
        let stone = BeamStone::new(&rig.planes, false).expect("a closed stone");
        let kit = RetraceKit {
            camera: &rig.camera,
            size: (FRAME, FRAME),
            stone: &stone,
            material: &rig.material,
            n_lambda: N_LAMBDA,
        };
        let prepared = PreparedMaterial::new(&rig.material);
        let hero = hero(&prepared);
        for point in [
            DVec2::new(16.3, 15.7),
            DVec2::new(12.1, 18.4),
            DVec2::new(20.2, 11.9),
        ] {
            let (_, original) = entry_facet(&rig, &stone, point);
            let camera =
                exact_ray_at(point, lambda, &BranchPrefix::EMPTY, &kit).expect("camera ray");
            let k_in = camera.dir.as_vec3();
            for is_ext in [false, true] {
                let word = (original << 8) | (3 << 1) | u32::from(is_ext);
                let prefix = BranchPrefix::EMPTY.push(word).expect("room");
                let ray = exact_ray_at(point, lambda, &prefix, &kit).expect("the wave enters");
                assert!(ray.inside);
                assert_eq!(ray.is_ext, is_ext);
                let state = PathState {
                    k: k_in,
                    inside: false,
                    is_ext: false,
                    stokes: unit_stokes(),
                };
                let hit = ExplicitHit {
                    point: ray.hits[0].as_vec3(),
                    facet: original as usize,
                    normal: Vec3::from(rig.planes[original as usize].normal),
                };
                let segment = Ray {
                    origin: camera.origin.as_vec3(),
                    dir: k_in,
                };
                let expansion =
                    expand_at(&hero, &rig.planes, state, segment, hit, Some(true), None);
                let child = expansion
                    .children
                    .iter()
                    .find(|c| c.kind == EventKind::Transmit && c.state.is_ext == is_ext)
                    .expect("the tracer's transmitted child of this mode");
                assert_eq!(
                    bits3(ray.dir.as_vec3()),
                    bits3(child.s),
                    "{name} S, ext {is_ext}"
                );
                assert_eq!(
                    bits3(ray.k.as_vec3()),
                    bits3(child.state.k),
                    "{name} k, ext {is_ext}"
                );
                let wave = biaxial_entry_wave(&rig.material, lambda, k_in, hit.normal, is_ext)
                    .expect("entry cannot be totally reflected");
                assert_eq!(bits3(wave.s), bits3(child.s));
                assert_eq!(bits3(wave.k), bits3(child.state.k));
            }
        }
    }
}

#[test]
fn an_entry_gives_both_resolved_modes_at_half_weight_each() {
    for name in ["Tanzanite", "Peridot"] {
        let rig = BeamRig::cube(named(name));
        let prepared = PreparedMaterial::new(&rig.material);
        let hero = hero(&prepared);
        let k = Vec3::new(0.1, 0.05, -1.0).normalize();
        let state = PathState {
            k,
            inside: false,
            is_ext: false,
            stokes: unit_stokes(),
        };
        // Facet 4 has the outward normal `+Z`; the ray comes from above.
        let hit = ExplicitHit {
            point: Vec3::new(0.1, 0.05, 1.0),
            facet: 4,
            normal: Vec3::Z,
        };
        let segment = Ray {
            origin: Vec3::new(0.0, 0.0, 2.0),
            dir: k,
        };
        let expansion = expand_at(&hero, &rig.planes, state, segment, hit, Some(true), None);
        let transmitted: Vec<_> = expansion
            .children
            .iter()
            .filter(|c| c.kind == EventKind::Transmit)
            .collect();
        let modes: Vec<bool> = transmitted.iter().map(|c| c.state.is_ext).collect();
        assert_eq!(modes, vec![false, true], "{name}: modes A then B");
        for child in transmitted {
            let weight = child.state.stokes[0].i;
            eprintln!(
                "{name}: mode ext={} entry weight {weight}",
                child.state.is_ext
            );
            assert!(weight <= HALF_WEIGHT + ENERGY_SLACK, "{name}: {weight}");
            assert!(
                weight >= HALF_WEIGHT * MIN_TRANSMITTANCE,
                "{name}: {weight} is below half a near-normal transmittance"
            );
        }
        assert!(expansion.dropped <= 0.0, "{name}: {}", expansion.dropped);
    }
}

#[test]
fn both_resolved_modes_walk_off_and_enter_at_different_wave_normals() {
    let lambda = test_lambda();
    let k = Vec3::new(0.3, 0.4, -0.866).normalize();
    let normal = Vec3::Z;
    for name in ["Tanzanite", "Peridot"] {
        let material = named(name);
        let a = biaxial_entry_wave(&material, lambda, k, normal, false).expect("enters");
        let b = biaxial_entry_wave(&material, lambda, k, normal, true).expect("enters");
        assert!(
            angle(a.k, b.k) > 1.0e-6,
            "{name}: the modes share a wave normal"
        );
        let walk = angle(a.s, a.k).max(angle(b.s, b.k));
        eprintln!(
            "{name}: walk-off A {:.3e}, B {:.3e} rad",
            angle(a.s, a.k),
            angle(b.s, b.k)
        );
        assert!(walk > 1.0e-5, "{name}: neither mode walks off");
    }
}

// ---- the optic axes ----

#[test]
fn the_optic_axes_are_where_the_two_roots_of_fresnels_equation_meet() {
    let guard = OPTIC_AXIS_GUARD_RAD;
    for name in ["Tanzanite", "Peridot"] {
        let material = named(name);
        let indicatrix = material
            .biaxial_indicatrix(test_lambda())
            .expect("a biaxial material");
        let axes = biaxial_optic_axes(&indicatrix).expect("two optic axes");
        let beta = indicatrix.axes.y_axis;
        for axis in axes {
            assert!((axis.length() - 1.0).abs() < 1.0e-6, "{name}");
            assert!(
                axis.dot(beta).abs() < 1.0e-6,
                "{name}: in the alpha-gamma plane"
            );
            let (slow, fast) = indicatrix.wave_indices(axis);
            eprintln!("{name}: gap at an optic axis {:.3e}", slow - fast);
            assert!(slow - fast < AXIS_GAP_LIMIT, "{name}: {slow} / {fast}");
            assert!((fast - indicatrix.n_beta).abs() < AXIS_GAP_LIMIT, "{name}");
            // The guard covers both senses and stops at the guard angle.
            let across = beta.cross(axis).normalize();
            let tilted = |radians: f64| {
                let (s, c) = radians.sin_cos();
                (axis.as_dvec3() * c + across.as_dvec3() * s).as_vec3()
            };
            assert!(near_biaxial_axis(axis, &indicatrix));
            assert!(near_biaxial_axis(-axis, &indicatrix));
            assert!(near_biaxial_axis(
                tilted(guard - GUARD_PROBE_RAD),
                &indicatrix
            ));
            assert!(!near_biaxial_axis(
                tilted(guard + GUARD_PROBE_RAD),
                &indicatrix
            ));
        }
        assert!(
            axes[0].dot(axes[1]).abs() < 1.0 - 1.0e-3,
            "{name}: the two axes are distinct lines"
        );
        let generic =
            (indicatrix.axes.x_axis + indicatrix.axes.y_axis + indicatrix.axes.z_axis).normalize();
        let (slow, fast) = indicatrix.wave_indices(generic);
        eprintln!("{name}: gap in a generic direction {:.3e}", slow - fast);
        assert!(!near_biaxial_axis(generic, &indicatrix), "{name}");
    }
}

// ---- the other classes are untouched ----

/// The uniaxial index formula, the reference for the uniaxial branch of [`mode_index`].
fn reference_uniaxial_index(material: &GemMaterial, lambda_nm: f32, k: Vec3, is_ext: bool) -> f32 {
    let n_o = material.dispersion.evaluate(lambda_nm);
    if !is_ext {
        return n_o;
    }
    let n_e = material.extraordinary_index_at(lambda_nm, n_o);
    let theta = k.dot(material.c_axis).clamp(-1.0, 1.0).abs().acos();
    BirefringenceParams::effective_extraordinary_index(n_o, n_e, theta)
}

#[test]
fn uniaxial_and_isotropic_mode_indices_and_decisions_are_unchanged() {
    let lambda = test_lambda();
    let materials = [
        glass(),
        diamond(),
        sapphire(),
        named("Sapphire"),
        named("Tourmaline"),
    ];
    for material in materials {
        assert!(!is_biaxial_crystal(&material), "{}", material.name);
        for k in internal_waves() {
            for is_ext in [false, true] {
                assert_eq!(
                    mode_index(&material, lambda, k, is_ext).to_bits(),
                    reference_uniaxial_index(&material, lambda, k, is_ext).to_bits(),
                    "{} {k:?} ext {is_ext}",
                    material.name
                );
            }
        }
        let rig = BeamRig::cube(material);
        let stone = BeamStone::new(&rig.planes, false).expect("a closed stone");
        let kit = RetraceKit {
            camera: &rig.camera,
            size: (FRAME, FRAME),
            stone: &stone,
            material: &rig.material,
            n_lambda: N_LAMBDA,
        };
        let point = DVec2::new(16.3, 15.7);
        let (top, original) = entry_facet(&rig, &stone, point);
        let (normal, _) = stone.plane(top);
        let opposite = (0..u32::try_from(stone.facet_count()).expect("few facets"))
            .find(|&f| stone.plane(f).0.dot(normal) < -0.99)
            .expect("a cube has an opposite facet");
        let bottom = stone.original_facet(opposite);
        let steps = [
            (original << 8) | (3 << 1),
            (bottom << 8) | (1 << 1),
            (original << 8) | (3 << 1),
        ];
        let mut prefix = BranchPrefix::EMPTY;
        for (length, &word) in steps.iter().enumerate() {
            prefix = prefix.push(word).expect("room");
            if is_uniaxial(&rig.material) {
                // The dispatch of `exact_ray_at` is the uniaxial retracer, bit for bit.
                let direct = uniaxial_ray(point, lambda, &prefix, &kit);
                assert_eq!(exact_ray_at(point, lambda, &prefix, &kit), direct);
            }
            let Ok(ray) = exact_ray_at(point, lambda, &prefix, &kit) else {
                continue;
            };
            let facet = stone.facet_of_original(bottom).expect("the bottom facet");
            let decision = ray_is_total_reflection(&ray, facet, lambda, &kit);
            if is_uniaxial(&rig.material) {
                let state = (ray.k.as_vec3(), ray.is_ext);
                assert_eq!(
                    decision,
                    state_is_total_reflection(state, facet, lambda, &kit),
                    "{} after {length} steps",
                    rig.material.name
                );
            } else {
                assert_eq!(
                    decision,
                    is_total_reflection(ray.dir, facet, lambda, &kit),
                    "{} after {length} steps",
                    rig.material.name
                );
            }
        }
        // For a non-biaxial stone the biaxial guard equals the uniaxial one.
        let spectrum = SpectralInterval::all(N_LAMBDA);
        for beam in primary_beams(&stone, &rig.camera, (FRAME, FRAME), whole_frame(), spectrum) {
            assert_eq!(
                beam_near_axis(&beam, &rig.material, N_LAMBDA),
                beam_near_optic_axis(&beam, &rig.material),
                "{}",
                rig.material.name
            );
        }
    }
}

#[test]
fn a_biaxial_prefix_is_retraced_by_the_biaxial_retracer() {
    let rig = BeamRig::cube(named("Tanzanite"));
    let stone = BeamStone::new(&rig.planes, false).expect("a closed stone");
    let kit = RetraceKit {
        camera: &rig.camera,
        size: (FRAME, FRAME),
        stone: &stone,
        material: &rig.material,
        n_lambda: N_LAMBDA,
    };
    let lambda = test_lambda();
    let point = DVec2::new(16.3, 15.7);
    let (top, original) = entry_facet(&rig, &stone, point);
    let (normal, _) = stone.plane(top);
    let opposite = (0..u32::try_from(stone.facet_count()).expect("few facets"))
        .find(|&f| stone.plane(f).0.dot(normal) < -0.99)
        .expect("a cube has an opposite facet");
    let bottom = stone.original_facet(opposite);
    let mut retraced = 0_usize;
    for entry_mode in [0_u32, 1] {
        for reflect_mode in [0_u32, 1] {
            let prefix = BranchPrefix::EMPTY
                .push((original << 8) | (3 << 1) | entry_mode)
                .and_then(|p| p.push((bottom << 8) | (1 << 1) | reflect_mode))
                .expect("room");
            let ray = exact_ray_at(point, lambda, &prefix, &kit).expect("a biaxial retrace");
            assert_eq!(
                ray,
                biaxial_ray(point, lambda, &prefix, &kit).expect("same")
            );
            assert!(ray.inside);
            assert_eq!(ray.is_ext, reflect_mode == 1);
            retraced += 1;
        }
    }
    assert_eq!(retraced, 4);
}

// ---- the beam tree ----

/// Runs `f` with the interaction kit of a cube rig of `material`, built the way the frame builds it.
fn with_kit<R>(
    material: GemMaterial,
    f: impl FnOnce(&InteractKit<'_, '_>, &BeamStone, &BeamRig) -> R,
) -> R {
    let rig = BeamRig::cube(material);
    let stone = BeamStone::new(&rig.planes, false).expect("a closed stone");
    let settings = BeamSettings::default();
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let reflection = ReflectionKit::new(
        &rig.material,
        &rig.planes,
        &rig.camera,
        (FRAME, FRAME),
        &scene.environment,
        N_LAMBDA,
    );
    let retrace = RetraceKit {
        camera: &rig.camera,
        size: (FRAME, FRAME),
        stone: &stone,
        material: &rig.material,
        n_lambda: N_LAMBDA,
    };
    let kit = reflection.interact_kit(retrace, &settings, whole_frame(), MAX_BOUNCES);
    f(&kit, &stone, &rig)
}

#[test]
fn entry_children_come_in_both_resolved_modes_and_a_retrace_reproduces_their_states() {
    for name in ["Tanzanite", "Peridot"] {
        with_kit(named(name), |kit, stone, rig| {
            let spectrum = SpectralInterval::all(N_LAMBDA);
            let mut stats = BeamStats::default();
            let (mut mode_a, mut mode_b, mut compared) = (0_usize, 0_usize, 0_usize);
            for primary in
                primary_beams(stone, &rig.camera, (FRAME, FRAME), whole_frame(), spectrum)
            {
                for child in refract_entry(&primary, kit, &mut stats) {
                    let ext = child.state.points[0].is_ext;
                    if ext {
                        mode_b += 1;
                    } else {
                        mode_a += 1;
                    }
                    // The step word and the signature carry the resolved mode of the child.
                    let last = *child.prefix.steps().last().expect("one step");
                    assert_eq!(last & 1 == 1, ext);
                    for state in child.state.points.iter().take(4) {
                        assert_eq!(state.is_ext, ext);
                        let Some(again) = retrace_state(state.image, &child, kit) else {
                            continue;
                        };
                        assert_eq!(again.is_ext, state.is_ext);
                        assert!(angle(again.ray.dir, state.ray.dir) < RETRACE_ANGLE_TOLERANCE);
                        assert!(angle(again.k, state.k) < RETRACE_ANGLE_TOLERANCE);
                        compared += 1;
                    }
                }
            }
            eprintln!(
                "{name} entry: {mode_a} mode A and {mode_b} mode B children, {compared} states \
                 retraced, mode_children {}",
                stats.mode_children
            );
            assert!(mode_a > 0 && mode_b > 0, "{name}: both modes must enter");
            assert_eq!(
                stats.mode_children,
                u64::try_from(mode_a + mode_b).expect("small")
            );
            assert!(compared > 0, "{name}: nothing was retraced");
        });
    }
}

/// The full beam tree of a rig under `environment`.
fn render(rig: &BeamRig, environment: EnvironmentSource<'_>, label: &str, size: Size) -> BeamImage {
    let scene = FrameScene {
        environment,
        ..rig.scene(FRAME, MAX_BOUNCES)
    };
    let frame = BeamFrame::new(&scene, &size.settings())
        .unwrap_or_else(|off| panic!("{label}: beams are off: {off:?}"));
    let cancel = AtomicBool::new(false);
    let image = frame
        .render_beams_with_threads(TILE_SIZE, THREADS, &cancel)
        .expect("not cancelled");
    let stats = &image.stats;
    eprintln!("{label}: {}", stats.summary());
    eprintln!(
        "{label}: entry pieces {}, apex splits {}, TIR splits {}, mode children {}, apex refits \
         {}, mode splits {}, bracket failures {}",
        stats.entry_pieces,
        stats.apex_splits,
        stats.tir_splits,
        stats.mode_children,
        stats.apex_refits,
        stats.mode_splits,
        stats.bracket_failures
    );
    eprintln!(
        "{label}: drops by reason {:?}, dropped energy {:?}",
        stats.drops, stats.dropped_energy
    );
    image
}

// ---- the equivalence rows ----

/// The studio environment of the rows, with the grey tray under it.
fn environment() -> EnvironmentSource<'static> {
    LightingPreset::Daylight
        .studio(1.0, 0.4, 0.35)
        .with_backdrop(BACKDROP_GREY)
}

/// The reference tracer on the pixels of a rig.
struct Reference<'r> {
    rig: &'r BeamRig,
    prepared: PreparedMaterial<'r>,
    environment: EnvironmentSource<'r>,
    n_lambda: u32,
}

/// Which branches of a reference sample are zeroed.
#[derive(Clone, Copy)]
enum Filter<'i> {
    /// The plain reference tracer.
    Plain,
    /// The branches the index keeps.
    Kept(&'i KeptIndex),
    /// Everything (the screen for "every sample hits the stone").
    Everything,
}

impl Reference<'_> {
    /// Sample `i` of pixel `pixel` through the reference tracer with `filter`.
    fn sample(&self, pixel: (u32, u32), i: u32, filter: Filter<'_>) -> Vec3 {
        let salt = pixel.1 * FRAME + pixel.0;
        let (seed, hero_rand) = draw(i, salt);
        let unit = |h: u32| hash_u32(h) as f32 / 4_294_967_295.0;
        let jitter_x =
            cranley_patterson_rotate(radical_inverse_base(i, 2), unit(salt ^ 0x1357_9BDF)) - 0.5;
        let jitter_y =
            cranley_patterson_rotate(radical_inverse_base(i, 3), unit(salt ^ 0x2468_ACE0)) - 0.5;
        let ray = self.rig.camera.generate_ray(
            pixel.0 as f32,
            pixel.1 as f32,
            FRAME as f32,
            FRAME as f32,
            jitter_x,
            jitter_y,
        );
        let point = DVec2::new(
            f64::from(pixel.0) + f64::from(jitter_x),
            f64::from(pixel.1) + f64::from(jitter_y),
        );
        let n_lambda = self.n_lambda;
        let kept = |sig: u64, lambda: f32| match filter {
            Filter::Plain => false,
            Filter::Kept(index) => index.kept(point, sig, lambda, n_lambda),
            Filter::Everything => true,
        };
        let callback: Option<&KeptFn<'_>> = match filter {
            Filter::Plain => None,
            _ => Some(&kept as &KeptFn<'_>),
        };
        trace_spectral_ray_reference_prepared(
            ray,
            StoneGeometry::planes_only(&self.rig.planes),
            &self.rig.soa,
            &[],
            &self.prepared,
            Fluorescence::none(),
            MAX_BOUNCES,
            self.environment,
            seed,
            hero_rand,
            None,
            SurfaceScene::IDEAL,
            callback,
        )
        .xyz
    }

    /// The radiance of the kept branches: the plain sample minus the sample with them zeroed.
    fn kept_branches(&self, pixel: (u32, u32), i: u32, index: &KeptIndex) -> Vec3 {
        self.sample(pixel, i, Filter::Plain) - self.sample(pixel, i, Filter::Kept(index))
    }
}

/// The pixels whose square the primary beams cover completely, row-major, and the total image
/// area of the primary beams.
fn covered_pixels(rig: &BeamRig) -> (Vec<(u32, u32)>, f64) {
    let stone = BeamStone::new(&rig.planes, false).expect("a closed stone");
    let tile = whole_frame();
    let mut area = vec![0.0_f64; (FRAME * FRAME) as usize];
    let mut total = 0.0;
    let beams = primary_beams(
        &stone,
        &rig.camera,
        (FRAME, FRAME),
        tile,
        SpectralInterval::all(N_LAMBDA),
    );
    for beam in beams {
        let image = image_polygon(&beam).expect("an image polygon");
        total += image.area();
        for ((x, y), piece) in rasterise_polygon(&image, tile) {
            area[(y * FRAME + x) as usize] += piece.area();
        }
    }
    let pixels = (0..FRAME * FRAME)
        .filter(|&i| area[i as usize] >= 1.0 - COVERAGE_EPS)
        .map(|i| (i % FRAME, i / FRAME))
        .collect();
    (pixels, total)
}

/// The pixels some kept region covers only partly (sorted).
fn boundary_pixels(image: &BeamImage) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for (rect, index) in &image.kept {
        for region in index.regions() {
            let ring: Vec<DVec2> = region.polygon[..usize::from(region.len)]
                .iter()
                .map(|p| p.as_dvec2())
                .collect();
            let Some(polygon) = ConvexPolygon::from_image_points(&ring) else {
                continue;
            };
            for (pixel, piece) in rasterise_polygon(&polygon, *rect) {
                if piece.area() < 1.0 - PARTIAL_EPS {
                    out.push(pixel);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The kept index of the tile holding `pixel`.
fn index_of(image: &BeamImage, pixel: (u32, u32)) -> &KeptIndex {
    let columns = FRAME.div_ceil(TILE_SIZE);
    let tile = (pixel.1 / TILE_SIZE) * columns + pixel.0 / TILE_SIZE;
    &image.kept[tile as usize].1
}

/// Up to `max_pixels` of `candidates`, spread over the list, whose samples all hit the stone.
fn screened(
    reference: &Reference<'_>,
    candidates: Vec<(u32, u32)>,
    max_pixels: usize,
) -> Vec<(u32, u32)> {
    let stride = (candidates.len() / (2 * max_pixels)).max(1);
    candidates
        .into_iter()
        .step_by(stride)
        .filter(|&pixel| {
            (0..SCREEN_SAMPLES)
                .all(|i| reference.sample(pixel, i, Filter::Everything) == Vec3::ZERO)
        })
        .take(max_pixels)
        .collect()
}

/// The guard share, the counters and the branch-filtered z-test of one biaxial scene (§16.2).
fn check_biaxial(rig: &BeamRig, label: &str, size: Size) {
    assert!(
        is_biaxial_crystal(&rig.material),
        "{label}: not a biaxial material"
    );
    let env = environment();
    let image = render(rig, env, label, size);
    let stats = &image.stats;
    assert!(stats.leaves > 0, "{label}: no leaves");
    assert!(stats.entry_pieces > 0, "{label}: no refracted trees");
    assert!(stats.mode_children > 0, "{label}: no mode children");

    let (covered, area) = covered_pixels(rig);
    let guard = stats.dropped_energy[DropReason::OpticAxis.slot()];
    let share = guard / area.max(f64::MIN_POSITIVE);
    eprintln!(
        "{label}: optic-axis guard {} drops, share of the stone's energy {share:.3e} (limit \
         {GUARD_SHARE_LIMIT:.0e})",
        stats.drops[DropReason::OpticAxis.slot()]
    );
    assert!(share < GUARD_SHARE_LIMIT, "{label}: guard share {share}");

    let edges = boundary_pixels(&image);
    let candidates: Vec<(u32, u32)> = covered
        .into_iter()
        .filter(|pixel| edges.binary_search(pixel).is_err())
        .collect();
    let reference = Reference {
        rig,
        prepared: PreparedMaterial::new(&rig.material),
        environment: env,
        n_lambda: node_count(&size.settings()),
    };
    let pixels = screened(&reference, candidates, size.max_pixels);
    eprintln!("{label}: {} interior pixels compared", pixels.len());
    assert!(
        pixels.len() >= MIN_PIXELS,
        "{label}: only {} fully covered interior pixels",
        pixels.len()
    );
    let beam_value = |pixel: (u32, u32)| image.xyz[(pixel.1 * FRAME + pixel.0) as usize];
    let zs = compare_streams(
        pixels.len(),
        (size.samples, 2),
        |k, i| reference.kept_branches(pixels[k], i, index_of(&image, pixels[k])),
        |k, _| beam_value(pixels[k]),
    );
    assert_z_stats(&zs, Z_LIMIT, label);
    let total: f64 = pixels
        .iter()
        .map(|&p| f64::from(beam_value(p).element_sum()))
        .sum();
    assert!(total > 0.0, "{label}: the beams produced no light");
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn tanzanite_cube_branches_match_the_reference_under_the_studio_rig() {
    check_biaxial(
        &BeamRig::cube(named("Tanzanite")),
        "cube tanzanite studio",
        FULL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn tanzanite_brilliant_branches_match_the_reference_under_the_studio_rig() {
    check_biaxial(
        &BeamRig::brilliant(named("Tanzanite")),
        "brilliant tanzanite studio",
        FULL,
    );
}

#[test]
#[ignore = "beam equivalence: faceted-stone smoke, over 60 s; scripts/run_beam_tests.sh step 03b runs it"]
fn tanzanite_brilliant_branches_match_the_reference_under_the_studio_rig_smoke() {
    check_biaxial(
        &BeamRig::brilliant(named("Tanzanite")),
        "brilliant tanzanite studio (small)",
        SMALL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn peridot_cube_branches_match_the_reference_under_the_studio_rig() {
    check_biaxial(
        &BeamRig::cube(named("Peridot")),
        "cube peridot studio",
        FULL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn peridot_brilliant_branches_match_the_reference_under_the_studio_rig() {
    check_biaxial(
        &BeamRig::brilliant(named("Peridot")),
        "brilliant peridot studio",
        FULL,
    );
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn glass_and_diamond_renders_count_no_birefringent_events() {
    for (rig, label) in [
        (BeamRig::brilliant(glass()), "brilliant glass"),
        (BeamRig::brilliant(diamond()), "brilliant diamond"),
    ] {
        // The counters are zero for any isotropic stone: the small node count.
        let image = render(&rig, environment(), label, SMALL);
        let stats = &image.stats;
        assert_eq!(
            (stats.mode_children, stats.apex_refits, stats.mode_splits),
            (0, 0, 0),
            "{label}"
        );
        assert_eq!(stats.drops[DropReason::OpticAxis.slot()], 0, "{label}");
        assert!(stats.leaves > 0, "{label}: no leaves");
    }
}
