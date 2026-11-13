//! Byte-identity pins for the solid renderer and the 2D diagram.
//!
//! Every test hashes (FNV-1a, as `mesh_cache::hash_planes` does) the RGBA, depth and pick
//! buffers that `raster::SolidRasterizer` and `diagram2d::render_diagram` produce for a fixed
//! design, camera pose, size and style, plus the build output of
//! `facet_map::FacetMap::from_design`, read back through its public accessors (this crate
//! cannot reach its `pub(crate)` fields). Only public API is used, so the pins hold whichever
//! crate the modules live in.
//!
//! Three fixed designs:
//! - **CrackOtto-Step** (`pleochroma-cut-core`'s 103-tier probe fixture): the full
//!   pipeline (`Design::solve` -> `planes_from_solved` -> `build_solid_mesh` ->
//!   `MeshCache`/`SolidRasterizer` -> `diagram2d` -> `FacetMap::from_design`).
//! - **RBC-445** (`StandardGemCuts::standard_round_brilliant`): raw planes, no
//!   `Design`, exercised through the rasterizer and the diagram renderer (which
//!   only needs a `SolidMesh`, not a `Design`).
//! - **Box**: a minimal 6-plane cube, the cheapest possible closed solid, exercised
//!   through the rasterizer only.

use glam::Vec3;
use pleochroma::{
    geometry::{
        cuts::StandardGemCuts,
        meet_solver::{Block, MeetConstraint, classify_blocks, meet_tier_inputs_from_asc},
        stone_metrics::{SolidStatus, build_solid_mesh},
    },
    optics::raytracer::Camera,
};
use pleochroma_cut::gui::solid_preview::{
    diagram2d::{self, DiagramConfig, DiagramStyle, PanelKind},
    facet_map::FacetMap,
    mesh_cache::MeshCache,
    raster::{SolidRasterizer, SolidStyle},
};
use pleochroma_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};

/// FNV-1a over raw bytes, the same `mesh_cache::fnv1a_64` that `hash_planes` uses. Bit
/// patterns are hashed, not `std::hash::Hash`, so the pins are as deterministic as the cache key.
fn fnv1a(bytes: &[u8]) -> u64 {
    pleochroma_cut::gui::solid_preview::mesh_cache::fnv1a_64(bytes.iter().copied())
}

/// Folds sub-hashes into one pin, so each test has one constant.
fn combine(hashes: &[u64]) -> u64 {
    let bytes: Vec<u8> = hashes.iter().flat_map(|h| h.to_le_bytes()).collect();
    fnv1a(&bytes)
}

fn hash_f32_slice(values: &[f32]) -> u64 {
    let bytes: Vec<u8> = values
        .iter()
        .flat_map(|v| v.to_bits().to_le_bytes())
        .collect();
    fnv1a(&bytes)
}

fn hash_u32_slice(values: &[u32]) -> u64 {
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    fnv1a(&bytes)
}

/// A [`SolidRasterizer`] frame's RGBA + depth + pick buffers, combined into one hash.
fn hash_rasterizer(r: &SolidRasterizer) -> u64 {
    combine(&[
        fnv1a(&r.color),
        hash_f32_slice(&r.depth),
        hash_u32_slice(&r.pick),
    ])
}

/// A [`diagram2d::DiagramFrame`]'s RGBA and per-pixel pick, panel and tooth readback, combined
/// into one hash. `pick_at`, `panel_at` and `tooth_at` are the only way an external crate reads
/// those buffers (see the module doc).
fn hash_diagram_frame(frame: &diagram2d::DiagramFrame) -> u64 {
    let mut pick = Vec::with_capacity((frame.width * frame.height) as usize);
    let mut panel = Vec::with_capacity((frame.width * frame.height) as usize);
    let mut tooth = Vec::with_capacity((frame.width * frame.height) as usize);
    for y in 0..frame.height {
        for x in 0..frame.width {
            pick.push(frame.pick_at(x, y).map_or(0, |v| v + 1));
            panel.push(match frame.panel_at(x, y) {
                None => 0u32,
                Some(PanelKind::Crown) => 1,
                Some(PanelKind::Pavilion) => 2,
                Some(PanelKind::Profile) => 3,
            });
            tooth.push(frame.tooth_at(x, y).map_or(0, |v| v + 1));
        }
    }
    combine(&[
        fnv1a(&frame.color),
        hash_u32_slice(&pick),
        hash_u32_slice(&panel),
        hash_u32_slice(&tooth),
    ])
}

fn closed_mesh(planes: &[(Vec3, f32)]) -> pleochroma::geometry::stone_metrics::SolidMesh {
    let widened: Vec<(glam::DVec3, f64)> = planes
        .iter()
        .map(|&(n, m)| {
            (
                glam::DVec3::new(f64::from(n.x), f64::from(n.y), f64::from(n.z)),
                f64::from(m),
            )
        })
        .collect();
    match build_solid_mesh(&widened) {
        SolidStatus::Closed(mesh) => mesh,
        other => panic!("fixture must close: {other:?}"),
    }
}

fn rbc_planes() -> Vec<(Vec3, f32)> {
    StandardGemCuts::standard_round_brilliant()
        .into_iter()
        .map(|plane| (Vec3::from(plane.normal), -plane.d))
        .collect()
}

fn box_planes() -> Vec<(Vec3, f32)> {
    vec![
        (Vec3::X, 1.0),
        (Vec3::NEG_X, 1.0),
        (Vec3::Y, 0.6),
        (Vec3::NEG_Y, 0.6),
        (Vec3::Z, 1.0),
        (Vec3::NEG_Z, 1.0),
    ]
}

/// `pleochroma-cut-core`'s "CrackOtto-Step" fixture (PC 05.115, 103 tiers), built as a real
/// [`Design`]. The first tier of each crown, pavilion and girdle block without a scale
/// reference is given one, so the design solves.
const CRACKOTTO_STEP_ASC: &str =
    include_str!("../../../crates/pleochroma-cut-core/src/optimize_cost_probe_crackotto_step.asc");

fn crackotto_step_design() -> Design {
    let schedule =
        pleochroma_formats::asc::parse_asc(CRACKOTTO_STEP_ASC).expect("fixture must parse");
    let mut inputs = meet_tier_inputs_from_asc(&schedule);
    let blocks = classify_blocks(&inputs);
    for block in [Block::Crown, Block::Pavilion, Block::Girdle] {
        let anchored = inputs
            .iter()
            .zip(&blocks)
            .any(|(t, &b)| b == block && matches!(t.constraint, MeetConstraint::ScaleReference(_)));
        if anchored {
            continue;
        }
        if let Some(i) = (0..inputs.len()).find(|&i| blocks[i] == block) {
            inputs[i].constraint = MeetConstraint::ScaleReference(schedule.tiers[i].mast);
        }
    }
    let tiers = inputs
        .into_iter()
        .zip(&schedule.tiers)
        .map(|(input, original)| ConstraintTier {
            angle_deg: input.angle_deg,
            name: original.name.clone(),
            indices: input.indices,
            constraint: input.constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        })
        .collect();
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gemcad_version: schedule.gemcad_version.clone(),
            gear_teeth: schedule.gear_teeth,
            gear_reference_angle: schedule.gear_reference_angle,
            symmetry_order: schedule.symmetry_order,
            mirror: schedule.mirror,
            refractive_index: schedule.refractive_index,
            headers: schedule.headers.clone(),
            footnotes: schedule.footnotes,
        },
        tiers,
    )
}

fn pin_camera() -> Camera {
    Camera::new(0.6, 0.35, 3.0, 42.0)
}

const PIN_WIDTH: u32 = 200;
const PIN_HEIGHT: u32 = 150;

#[test]
fn solid_identity_pin_crackotto_step_full_pipeline() {
    let design = crackotto_step_design();
    assert_eq!(
        design.tiers.len(),
        103,
        "fixture must have its real tier count"
    );
    let solved = design.solve().expect("fixture must solve");
    let planes_f64 = design.planes_from_solved(&solved);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the rasterizer takes f32 planes; the fixture values are small"
    )]
    let planes: Vec<(Vec3, f32)> = planes_f64
        .into_iter()
        .map(|(n, m)| (Vec3::new(n.x as f32, n.y as f32, n.z as f32), m as f32))
        .collect();

    let mut cache = MeshCache::default();
    let prepared = cache.get_or_build(&planes).expect("must close");
    let style = SolidStyle::default();

    let mut plain = SolidRasterizer::new(PIN_WIDTH, PIN_HEIGHT);
    plain.render(&prepared.mesh, &pin_camera(), &style);
    let plain_hash = hash_rasterizer(&plain);

    let mut fast = SolidRasterizer::new(PIN_WIDTH, PIN_HEIGHT);
    fast.render_prepared(prepared, &pin_camera(), &style);
    let prepared_hash = hash_rasterizer(&fast);

    #[expect(
        clippy::cast_possible_truncation,
        reason = "DiagramConfig stores the reference angle as f32 render data"
    )]
    let diagram_config = DiagramConfig {
        width: PIN_WIDTH,
        height: PIN_HEIGHT,
        gear_teeth: design.meta.gear_teeth_abs(),
        gear_reference_angle: design.meta.gear_reference_angle as f32,
        symmetry_order: design.meta.symmetry_order,
        mirror: design.meta.mirror,
    };
    let diagram_style = DiagramStyle::default();
    let three_panel = diagram2d::render_diagram(&prepared.mesh, &diagram_config, &diagram_style);
    let three_panel_hash = hash_diagram_frame(&three_panel);
    let single_panel = diagram2d::render_diagram_single_panel(
        &prepared.mesh,
        &diagram_config,
        &diagram_style,
        PanelKind::Pavilion,
    );
    let single_panel_hash = hash_diagram_frame(&single_panel);

    let facet_map = FacetMap::from_design(&design, &solved);
    let mut facet_map_bytes: Vec<u8> = Vec::new();
    facet_map_bytes.extend_from_slice(&(facet_map.facet_count() as u64).to_le_bytes());
    facet_map_bytes.extend_from_slice(&(facet_map.preform_plane_count() as u64).to_le_bytes());
    for facet_id in 0..facet_map.facet_count() {
        facet_map_bytes.extend_from_slice(
            &facet_map
                .tier_of(facet_id)
                .map_or(u64::MAX, |t| t as u64)
                .to_le_bytes(),
        );
        facet_map_bytes.extend_from_slice(&facet_map.index_on_gear(facet_id).to_le_bytes());
        facet_map_bytes.extend_from_slice(facet_map.facet_label(facet_id).as_bytes());
    }
    let facet_map_hash = fnv1a(&facet_map_bytes);

    let combined = combine(&[
        plain_hash,
        prepared_hash,
        three_panel_hash,
        single_panel_hash,
        facet_map_hash,
    ]);
    // Each component is pinned on its own, so a failure names the one that moved. The values
    // depend on glam's DMat3 determinant and inverse bits (glam is pinned to `=0.34.1`), and
    // `facet_map` hashes the canonical labels of `design/labelling.rs`: codes are numbered per
    // letter in cutting order and the table is `T`.
    assert_eq!(
        combined, 0x1ff8_fcba_069a_a4e1,
        "CrackOtto-Step solid/diagram/facet_map pipeline hash changed -- \
         plain={plain_hash:#x} prepared={prepared_hash:#x} three_panel={three_panel_hash:#x} \
         single_panel={single_panel_hash:#x} facet_map={facet_map_hash:#x}"
    );
}

#[test]
fn solid_identity_pin_rbc_445() {
    let planes = rbc_planes();
    let mesh = closed_mesh(&planes);
    let style = SolidStyle::default();

    let mut cache = MeshCache::default();
    let prepared = cache.get_or_build(&planes).expect("RBC-445 must close");

    let mut plain = SolidRasterizer::new(PIN_WIDTH, PIN_HEIGHT);
    plain.render(&mesh, &pin_camera(), &style);
    let plain_hash = hash_rasterizer(&plain);

    let mut fast = SolidRasterizer::new(PIN_WIDTH, PIN_HEIGHT);
    fast.render_prepared(prepared, &pin_camera(), &style);
    let prepared_hash = hash_rasterizer(&fast);

    let diagram_config = DiagramConfig {
        width: PIN_WIDTH,
        height: PIN_HEIGHT,
        gear_teeth: 96,
        gear_reference_angle: 0.0,
        symmetry_order: 8,
        mirror: false,
    };
    let diagram_style = DiagramStyle::default();
    let three_panel = diagram2d::render_diagram(&mesh, &diagram_config, &diagram_style);
    let three_panel_hash = hash_diagram_frame(&three_panel);

    let combined = combine(&[plain_hash, prepared_hash, three_panel_hash]);
    assert_eq!(
        combined, 0x3a3d_abe7_d3e9_4c87,
        "RBC-445 solid/diagram hash changed -- plain={plain_hash:#x} \
         prepared={prepared_hash:#x} three_panel={three_panel_hash:#x}"
    );
}

#[test]
fn solid_identity_pin_box() {
    let planes = box_planes();
    let mesh = closed_mesh(&planes);
    let style = SolidStyle::default();
    let camera = Camera::new(0.4, 0.3, 4.0, 40.0);

    let mut plain = SolidRasterizer::new(PIN_WIDTH, PIN_HEIGHT);
    plain.render(&mesh, &camera, &style);
    let plain_hash = hash_rasterizer(&plain);

    let mut cache = MeshCache::default();
    let prepared = cache.get_or_build(&planes).expect("box must close");
    let mut fast = SolidRasterizer::new(PIN_WIDTH, PIN_HEIGHT);
    fast.render_prepared(prepared, &camera, &style);
    let prepared_hash = hash_rasterizer(&fast);

    let combined = combine(&[plain_hash, prepared_hash]);
    assert_eq!(
        combined, 0xdd3f_cd8d_48f7_3025,
        "Box solid hash changed -- plain={plain_hash:#x} prepared={prepared_hash:#x}"
    );
}
