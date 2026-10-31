//! The identity test (the web app's pipeline entry points reproduce the desktop's
//! pre-move frame bytes) and the new pure helpers' tests.

use super::{
    CameraPose, FacetOverlay, PlanJob, PreviewPipeline, RedrawRequest, StoneGeometryBuf,
    WorkerMemory, build_planned_frame, camera, render_request,
    view::{self, RasterLimits, ReplanBasis, ReplanInputs},
};
use crate::{
    diagram2d::PanelKind,
    facet_map::{FacetKind, FacetMap},
    live_update,
    live_update::Clock,
    mesh_cache::MeshCache,
    raster::SolidRasterizer,
};
use pleochroma::geometry::{ToolPrimitive, meet_solver::MeetConstraint};
use pleochroma_cut_core::{
    ConstraintTier, Design, PreformSpec, ScheduleMeta,
    design::{ConcaveTier, ConcaveTool, ToolMotion},
};
use std::{collections::BTreeSet, sync::Arc};

/// A clock that never advances: every fixture here is pinned (no solver call), so
/// the budget check never runs -- the desktop's `Instant` clock would give the
/// same result.
struct ZeroClock;

impl Clock for ZeroClock {
    fn now_ms(&self) -> f64 {
        0.0
    }
}

fn fnv(bytes: &[u8]) -> u64 {
    crate::mesh_cache::fnv1a_64(bytes.iter().copied())
}

fn fnv_u32(values: &[u32]) -> u64 {
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    fnv(&bytes)
}

fn rbc_tier(name: &str, angle_deg: f64, indices: &[f64], mast: f64) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: indices.to_vec(),
        constraint: MeetConstraint::ScaleReference(mast),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// The desktop pins' fixture (`preview_state::tests::pins::pin_design`), verbatim.
fn pin_design() -> Design {
    const GIRDLE: [f64; 16] = [
        0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
        90.0,
    ];
    const BREAK: [f64; 16] = [
        95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0, 83.0,
        85.0,
    ];
    const MAIN: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
    const STAR: [f64; 8] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gemcad_version: "GemCad 5.0".to_string(),
            gear_teeth: 96,
            gear_reference_angle: 0.0,
            symmetry_order: 8,
            mirror: true,
            refractive_index: 1.54,
            headers: Vec::new(),
            footnotes: Vec::new(),
        },
        vec![
            rbc_tier("Table", 0.0, &[], 0.32),
            rbc_tier("Star", 15.0, &STAR, 0.45),
            rbc_tier("Crown Main", 34.5, &MAIN, 0.59),
            rbc_tier("Upper Girdle", 41.0, &BREAK, 0.67),
            rbc_tier("Girdle", 90.0, &GIRDLE, 1.0),
            rbc_tier("Pavilion Main", -41.0, &MAIN, 0.67),
            rbc_tier("Lower Girdle", -42.5, &BREAK, 0.68),
            rbc_tier("Culet", -0.0, &[], 0.88),
        ],
    )
}

const PIN_CAMERA: CameraPose = CameraPose {
    roll: 0.0,
    yaw: 0.6,
    pitch: 0.45,
    distance: 2.4,
};

fn pin_overlay() -> FacetOverlay {
    FacetOverlay {
        hovered: Some(20),
        selected_facet: Some(21),
        multi_selected: vec![30, 31],
        provisional: Vec::new(),
        moved: Vec::new(),
    }
}

// The desktop's pins, recorded from its pipeline BEFORE the move
// (`apps/pleochroma-cut/src/gui/solid_preview/preview_state/tests/pins.rs`):
// `[solid image, solid pick, edges image, diagram image, diagram pick, tooth]`.
//
// Re-pinned 2026-10-05: slot 3 (the diagram's colour image) of both diagram pins moved,
// and nothing else did (the solid image, every pick buffer and the tooth buffer are the
// recorded ones, so the geometry and the layout are unchanged). The cause is the
// canonical facet labelling of 2026-10-04: a facet's on-diagram label is now its tier
// code plus its index (`C2 0`, `P1 0`, `Table`), no longer the tier's own name
// (`Crown Main 0`). Only text pixels differ. See
// `the_diagram_labels_are_the_canonical_tier_codes`, which pins the label text itself.
//
// Re-pinned 2026-10-06 (cutting-order lane): the table's code became `T`, so its diagram
// label is `T` where it was `Table`; slot 3 (the diagram's colour image) of `PIN_DIAGRAM`
// moved by the pixels of that one label and nothing else did. `PIN_SINGLE_PANEL` is
// unchanged: its enlarged panel 1 is the pavilion panel, which never draws the table.
const PIN_SOLID: [u64; 6] = [0x0f10_7ca8_5f93_74bf, 0x5f3d_a2da_7a33_a030, 0, 0, 0, 0];
const PIN_BOTH: [u64; 6] = [
    0x0f10_7ca8_5f93_74bf,
    0x5f3d_a2da_7a33_a030,
    0x21d0_25ae_bd67_13d0,
    0,
    0,
    0,
];
const PIN_DIAGRAM: [u64; 6] = [
    0x20dd_549d_dad6_8174,
    0xa8f1_2ee3_d05d_e3be,
    0,
    0x3885_c661_6f2d_25a1,
    0xd1a3_9e15_eccb_fb54,
    0xb0c8_4221_f413_1c25,
];
const PIN_SINGLE_PANEL: [u64; 6] = [
    0x61f0_28b7_7429_a350,
    0x2c7a_1a3e_c524_5679,
    0,
    0xcec8_3723_ef1e_4e76,
    0x4cf4_6279_7dc9_2530,
    0x0f8f_adba_ccdc_0009,
];

/// The job exactly as the desktop's `submit_preview_replan_for` builds it for
/// the pins (`pin_job` there): full solve, tier 2 selected, generation 3.
fn desktop_job(view_mode: u8, size: (u32, u32), enlarged_panel: i32) -> PlanJob {
    let design = pin_design();
    let n_d = design.effective_refractive_index();
    PlanJob {
        design: Arc::new(design),
        dirty: BTreeSet::new(),
        last_solved: None,
        camera: PIN_CAMERA,
        size,
        selected_tier: Some(2),
        n_d,
        view_mode,
        generation: 3,
        show_preform: true,
        enlarged_panel,
        tier_cutoff: None,
        cut_steps: None,
    }
}

/// The same replan as the web app's `src/views` builds it: [`view::plan_job`]
/// from [`ReplanInputs`] in their UI encodings.
fn web_job(view_mode: u8, size: (u32, u32), enlarged_panel: i32) -> PlanJob {
    view::plan_job(ReplanInputs {
        design: Arc::new(pin_design()),
        generation: 3,
        basis: ReplanBasis::FullSolve,
        camera: PIN_CAMERA,
        size,
        selected_tier: Some(2),
        custom_materials: &[],
        view_mode,
        show_preform: true,
        enlarged_panel,
        tier_cutoff: -1,
    })
}

/// The web app's path: [`PreviewPipeline::replan`] then
/// [`PreviewPipeline::update_overlay`], hashed from the pipeline's own buffers.
fn web_hashes(view_mode: u8, size: (u32, u32), enlarged_panel: i32) -> [u64; 6] {
    let mut pipeline = PreviewPipeline::new();
    pipeline
        .replan(
            web_job(view_mode, size, enlarged_panel),
            live_update::DEFAULT_PREVIEW_BUDGET,
            &ZeroClock,
        )
        .expect("a planned request always resolves");
    let frame = pipeline
        .update_overlay(pin_overlay())
        .expect("an overlay after a frame always resolves");
    let diagram = frame.diagram.as_ref();
    [
        fnv(pipeline.solid_rgba()),
        fnv_u32(&frame.pick.pick),
        if frame.has_edges {
            fnv(pipeline.edges_rgba())
        } else {
            0
        },
        diagram.map_or(0, |d| fnv(&d.color)),
        diagram.map_or(0, |d| fnv_u32(&d.pick)),
        diagram.map_or(0, |d| fnv_u32(&d.tooth)),
    ]
}

/// The desktop's path, called directly: its PLAN worker's `build_planned_frame`
/// and its RENDER worker's `render_request` on worker-owned state.
fn desktop_hashes(view_mode: u8, size: (u32, u32), enlarged_panel: i32) -> [u64; 6] {
    let mut mesh_cache = MeshCache::default();
    let mut rasterizer = SolidRasterizer::new(1, 1);
    let mut edges_rasterizer = SolidRasterizer::new(1, 1);
    let mut memory = WorkerMemory::default();
    let planned = build_planned_frame(
        desktop_job(view_mode, size, enlarged_panel),
        live_update::DEFAULT_PREVIEW_BUDGET,
        &ZeroClock,
    );
    render_request(
        &mut mesh_cache,
        &mut rasterizer,
        &mut edges_rasterizer,
        &mut memory,
        RedrawRequest::Planned(Box::new(planned)),
    )
    .expect("a planned request always resolves");
    let frame = render_request(
        &mut mesh_cache,
        &mut rasterizer,
        &mut edges_rasterizer,
        &mut memory,
        RedrawRequest::UpdateFacetOverlay(pin_overlay()),
    )
    .expect("an overlay after a frame always resolves");
    let diagram = frame.diagram.as_ref();
    [
        fnv(&rasterizer.color),
        fnv_u32(&frame.pick.pick),
        if frame.has_edges {
            fnv(&edges_rasterizer.color)
        } else {
            0
        },
        diagram.map_or(0, |d| fnv(&d.color)),
        diagram.map_or(0, |d| fnv_u32(&d.pick)),
        diagram.map_or(0, |d| fnv_u32(&d.tooth)),
    ]
}

#[test]
fn web_plan_job_matches_the_desktop_job_field_by_field() {
    let web = web_job(3, (360, 180), 1);
    let desktop = desktop_job(3, (360, 180), 1);
    assert_eq!(web.dirty, desktop.dirty);
    assert_eq!(web.last_solved.is_none(), desktop.last_solved.is_none());
    assert_eq!(web.camera, desktop.camera);
    assert_eq!(web.size, desktop.size);
    assert_eq!(web.selected_tier, desktop.selected_tier);
    assert_eq!(web.n_d.to_bits(), desktop.n_d.to_bits());
    assert_eq!(web.view_mode, desktop.view_mode);
    assert_eq!(web.generation, desktop.generation);
    assert_eq!(web.show_preform, desktop.show_preform);
    assert_eq!(web.enlarged_panel, desktop.enlarged_panel);
    assert_eq!(web.tier_cutoff, desktop.tier_cutoff);
    assert_eq!(web.cut_steps, desktop.cut_steps);
    assert_eq!(web.cut_limit(), desktop.cut_limit());
    assert_eq!(web.design.tiers, desktop.design.tiers);
}

/// The desktop's Cut slider: `cut_steps` plans the stone after that many steps, `0`
/// being the preform alone, and wins over the web's `tier_cutoff`.
#[test]
fn a_cut_steps_job_plans_the_stone_after_that_many_steps() {
    let plan = |steps: Option<usize>, tier_cutoff: Option<usize>| {
        let mut job = desktop_job(0, (160, 120), -1);
        job.cut_steps = steps;
        job.tier_cutoff = tier_cutoff;
        build_planned_frame(job, live_update::DEFAULT_PREVIEW_BUDGET, &ZeroClock)
    };
    let finished = plan(None, None);
    let preform = finished.design.preform.planes().len();
    let rough = plan(Some(0), None);
    assert_eq!(
        rough.planes.len(),
        preform,
        "the rough is the preform alone"
    );
    assert_eq!(
        rough.solved.as_ref().map(Vec::len),
        Some(finished.design.tiers.len()),
        "the masts of the whole design are chained forward, not a truncated list"
    );
    // The steps follow `Design::cutting_order()`: the pavilion section first, so the first
    // step of this top-down fixture is the girdle tier (stored index 4), sixteen planes.
    let girdle_only = plan(Some(1), None);
    assert_eq!(
        girdle_only.planes.len(),
        preform + 16,
        "step one is the girdle, sixteen planes"
    );
    let five = plan(Some(5), None);
    assert!(five.planes.len() > girdle_only.planes.len());
    assert!(five.planes.len() < finished.planes.len());
    let all = plan(Some(finished.design.tiers.len()), None);
    assert_eq!(all.planes, finished.planes);
    assert_eq!(
        plan(Some(0), Some(3)).planes.len(),
        preform,
        "cut_steps wins over tier_cutoff"
    );
    assert_eq!(
        plan(None, Some(0)).planes.len(),
        preform + 1,
        "the web's tier_cutoff is unchanged"
    );
}

/// The rough must still mesh: a preform alone is a closed solid, so the Cut slider's
/// first position shows a stone, never the finished gem or an empty view.
#[test]
fn the_rough_meshes_to_a_closed_solid() {
    let mut job = desktop_job(0, (160, 120), -1);
    job.cut_steps = Some(0);
    let planned = build_planned_frame(job, live_update::DEFAULT_PREVIEW_BUDGET, &ZeroClock);
    let mut cache = MeshCache::default();
    assert!(
        cache.get_or_build(&planned.planes).is_some(),
        "the preform's own planes close"
    );
}

/// The web and the desktop pipeline entry points produce the same frames, and on
/// Windows those frames are the pinned ones. The pins hold Windows bits: the
/// rasteriser's projection (`tan`) and facet angles (`sin`, `cos`) and the diagram
/// layout's `sin_cos` run through the platform math library, which rounds a few
/// arguments differently under glibc, so a boundary pixel moves and the hashes differ
/// on Linux (seen 2026-10-01). Off Windows only the web-versus-desktop equality is
/// checked, which holds on any platform since both run on the same library.
#[test]
fn web_frames_reproduce_the_desktop_pins() {
    for (view_mode, size, enlarged, pin) in [
        (0, (160, 120), -1, PIN_SOLID),
        (2, (160, 120), -1, PIN_BOTH),
        (3, (360, 180), -1, PIN_DIAGRAM),
        (3, (240, 240), 1, PIN_SINGLE_PANEL),
    ] {
        let desktop = desktop_hashes(view_mode, size, enlarged);
        let web = web_hashes(view_mode, size, enlarged);
        assert_eq!(
            web, desktop,
            "view mode {view_mode}: web and desktop paths differ"
        );
        if cfg!(windows) {
            assert_eq!(
                web, pin,
                "view mode {view_mode}: differs from the pre-move pin"
            );
        }
    }
}

/// The text the diagram draws on a facet is its canonical tier code plus the facet's
/// index (`C2 0`), or the bare code for a tier of one facet (`T`, `Culet`), never the tier's own
/// name (`Crown Main 0`). The diagram pins above hash the pixels of these labels, so they
/// moved when the labels changed (2026-10-04, and again when the table's code became `T`);
/// this pins the text itself. The codes are numbered in cutting order, which for this
/// fixture (stored top-down, every block in cutting order) gives the same numbers as before.
#[test]
fn the_diagram_labels_are_the_canonical_tier_codes() {
    let design = pin_design();
    let solved = design.solve().expect("every tier is pinned");
    let map = FacetMap::from_design(&design, &solved);
    let first_label = |tier: usize| map.facet_label(map.facets_of_tier(tier)[0] as usize);
    // `pin_design`'s tiers, in order: Table, Star, Crown Main, Upper Girdle, Girdle,
    // Pavilion Main, Lower Girdle, Culet.
    assert_eq!(first_label(0), "T");
    assert_eq!(first_label(1), "C1 6");
    assert_eq!(first_label(2), "C2 0");
    assert_eq!(first_label(3), "C3 95");
    assert_eq!(first_label(4), "G1 0");
    assert_eq!(first_label(5), "P1 0");
    assert_eq!(first_label(6), "P2 95");
    assert_eq!(first_label(7), "Culet");
}

#[test]
fn a_reproject_keeps_the_planned_style_and_follows_the_camera() {
    let mut pipeline = PreviewPipeline::new();
    assert!(
        pipeline.reproject(PIN_CAMERA, (64, 48), 0, None).is_none(),
        "nothing planned yet"
    );
    pipeline
        .replan(
            web_job(0, (64, 48), -1),
            live_update::DEFAULT_PREVIEW_BUDGET,
            &ZeroClock,
        )
        .expect("planned");
    let before = fnv(pipeline.solid_rgba());
    let turned = camera::orbit_step(PIN_CAMERA, 40.0, 0.0);
    let frame = pipeline
        .reproject(turned, (64, 48), 0, None)
        .expect("reprojected");
    assert!(frame.has_solid);
    assert_eq!(
        frame.generation, 3,
        "a reproject keeps the planned generation"
    );
    assert_ne!(
        fnv(pipeline.solid_rgba()),
        before,
        "the pose changed the pixels"
    );
    assert!(
        pipeline.memory().style.selected.iter().any(|&s| s),
        "the planned selection tint survives a camera move"
    );
}

/// The desktop's reproject planes are the planned ones round-tripped through a non-idempotent
/// `normalize()`, so they can differ by an ULP: that must not read as a new design and wipe the
/// tier highlight; a real plane move still must.
#[test]
fn a_reproject_with_ulp_noisy_planes_keeps_the_selection() {
    let mut pipeline = PreviewPipeline::new();
    pipeline
        .replan(
            web_job(0, (64, 48), -1),
            live_update::DEFAULT_PREVIEW_BUDGET,
            &ZeroClock,
        )
        .expect("planned");
    let planned = pipeline.memory().planes.clone().expect("planes remembered");
    let noisy: Vec<(glam::Vec3, f32)> = planned
        .iter()
        .map(|&(normal, m)| {
            let bump = |v: f32| f32::from_bits(v.to_bits().wrapping_add(1));
            (
                glam::Vec3::new(bump(normal.x), normal.y, bump(normal.z)),
                bump(m),
            )
        })
        .collect();
    let mut reproject = |planes: &[(glam::Vec3, f32)]| {
        pipeline
            .render(RedrawRequest::Reproject {
                geometry: StoneGeometryBuf::from_halfspaces(planes),
                camera: camera::orbit_step(PIN_CAMERA, 40.0, 0.0),
                size: (64, 48),
                view_mode: 0,
                gear: None,
            })
            .expect("reprojected");
        pipeline.memory().style.selected.iter().any(|&s| s)
    };
    assert!(
        reproject(&noisy),
        "an ULP of plane noise keeps the highlight"
    );
    assert!(reproject(&planned), "and so does the exact round trip back");
    let mut moved = planned;
    moved[0].1 += 0.05;
    assert!(!reproject(&moved), "a genuinely different stone resets it");
}

/// A camera-only `Reproject` of the stone the last planned frame labelled (same planes, only
/// the pose differs) hands the Solid hover its facet-name table unchanged, in every view mode.
#[test]
fn a_camera_only_reproject_keeps_the_hover_labels() {
    let mut pipeline = PreviewPipeline::new();
    pipeline
        .replan(
            web_job(0, (64, 48), -1),
            live_update::DEFAULT_PREVIEW_BUDGET,
            &ZeroClock,
        )
        .expect("planned");
    let labels = pipeline.memory().diagram.hover_text.clone();
    assert!(
        labels.iter().any(|text| !text.is_empty()),
        "the planned frame names its facets"
    );
    let planes = pipeline.memory().planes.clone().expect("planes remembered");
    for (view_mode, turn) in [(0u8, 40.0), (3, 80.0)] {
        let frame = pipeline
            .render(RedrawRequest::Reproject {
                geometry: StoneGeometryBuf::from_halfspaces(&planes),
                camera: camera::orbit_step(PIN_CAMERA, turn, 0.0),
                size: (64, 48),
                view_mode,
                gear: None,
            })
            .expect("reprojected");
        assert_eq!(
            frame.hover_text, labels,
            "view mode {view_mode}: the hover table the frame hands the UI is the planned one"
        );
        assert_eq!(pipeline.memory().diagram.hover_text, labels);
    }
}

/// The other side of the same rule, pinned so the cause of an empty hover is on record: a
/// `Reproject` of a DIFFERENT stone (what New, Load and Solve send -- they never plan a frame)
/// forgets the old stone's labels and has no `Design` to build new ones from, so the frame's
/// hover table is empty and stays empty until a `Planned` frame follows. The desktop must send
/// that frame after such a redraw (see `refresh_viewport`).
#[test]
fn a_reproject_of_another_stone_has_no_labels_until_a_planned_frame() {
    let mut pipeline = PreviewPipeline::new();
    pipeline
        .replan(
            web_job(0, (64, 48), -1),
            live_update::DEFAULT_PREVIEW_BUDGET,
            &ZeroClock,
        )
        .expect("planned");
    let mut planes = pipeline.memory().planes.clone().expect("planes remembered");
    planes[0].1 += 0.05;
    let frame = pipeline
        .render(RedrawRequest::Reproject {
            geometry: StoneGeometryBuf::from_halfspaces(&planes),
            camera: PIN_CAMERA,
            size: (64, 48),
            view_mode: 0,
            gear: None,
        })
        .expect("reprojected");
    assert!(
        frame.hover_text.is_empty(),
        "no design, so no names for the new stone's facets"
    );
    // The pick buffer is of the new stone: it names facet ids the empty table cannot resolve.
    assert!(frame.pick.pick.iter().any(|&id| id != 0));
    pipeline
        .replan(
            web_job(0, (64, 48), -1),
            live_update::DEFAULT_PREVIEW_BUDGET,
            &ZeroClock,
        )
        .expect("planned again");
    assert!(
        pipeline
            .memory()
            .diagram
            .hover_text
            .iter()
            .any(|text| !text.is_empty()),
        "a planned frame brings the names back"
    );
}

fn queued_planned_request() -> (RedrawRequest, Vec<(glam::Vec3, f32)>) {
    let frame = build_planned_frame(
        web_job(0, (64, 48), -1),
        live_update::DEFAULT_PREVIEW_BUDGET,
        &ZeroClock,
    );
    let planes = frame.planes.clone();
    (RedrawRequest::Planned(Box::new(frame)), planes)
}

/// A queued `Planned` request survives a newer camera `Reproject` (of ANOTHER plane set, as a
/// stale render context sends): the merged request is still the plan, at the newer pose, and the
/// frame it renders carries the labels.
#[test]
fn a_queued_planned_request_survives_a_later_reproject() {
    let (planned, mut planes) = queued_planned_request();
    planes.pop();
    let turned = camera::orbit_step(PIN_CAMERA, 40.0, 0.0);
    let merged = RedrawRequest::coalesce(
        planned,
        RedrawRequest::Reproject {
            geometry: StoneGeometryBuf::from_halfspaces(&planes),
            camera: turned,
            size: (80, 60),
            view_mode: 0,
            gear: None,
        },
    );
    let RedrawRequest::Planned(frame) = &merged else {
        panic!("the Planned request must not be replaced by a Reproject");
    };
    assert_eq!((frame.camera, frame.size), (turned, (80, 60)));
    let mut pipeline = PreviewPipeline::new();
    let rendered = pipeline.render(merged).expect("rendered");
    assert!(
        rendered.hover_text.iter().any(|text| !text.is_empty()),
        "the Design-carrying frame still fills the label table"
    );
}

/// A facet overlay arriving behind a queued `Planned` request is folded into it, and a newer
/// `Planned` request replaces it without losing that overlay.
#[test]
fn a_queued_planned_request_absorbs_a_later_facet_overlay() {
    let (planned, _) = queued_planned_request();
    let overlay = FacetOverlay {
        hovered: Some(3),
        ..FacetOverlay::default()
    };
    let merged = RedrawRequest::coalesce(planned, RedrawRequest::UpdateFacetOverlay(overlay));
    let (newer, _) = queued_planned_request();
    let merged = RedrawRequest::coalesce(merged, newer);
    let mut pipeline = PreviewPipeline::new();
    let rendered = pipeline.render(merged).expect("rendered");
    assert!(rendered.hover_text.iter().any(|text| !text.is_empty()));
    assert_eq!(pipeline.memory().style.hovered, Some(3));
    assert_eq!(pipeline.memory().diagram.style.hovered, Some(3));
}

/// Without a queued plan the newer request simply wins, as before.
#[test]
fn a_reproject_behind_nothing_planned_still_replaces_the_queued_request() {
    let overlay = RedrawRequest::UpdateFacetOverlay(FacetOverlay::default());
    let reproject = RedrawRequest::Reproject {
        geometry: StoneGeometryBuf::default(),
        camera: PIN_CAMERA,
        size: (8, 8),
        view_mode: 0,
        gear: None,
    };
    assert!(matches!(
        RedrawRequest::coalesce(overlay, reproject),
        RedrawRequest::Reproject { .. }
    ));
}

#[test]
fn a_replan_frame_carries_geometry_at_the_request_size_and_pose() {
    let mut pipeline = PreviewPipeline::new();
    let frame = pipeline
        .replan(
            web_job(0, (64, 48), -1),
            live_update::DEFAULT_PREVIEW_BUDGET,
            &ZeroClock,
        )
        .expect("planned");
    let geometry = frame
        .geometry
        .expect("a closed design always carries geometry");
    assert_eq!(geometry.size, (64, 48), "the size the raster used");
    assert_eq!(geometry.camera, PIN_CAMERA, "the pose the raster used");
    assert!(!geometry.corner_points.is_empty());
    assert_eq!(
        geometry.facet_centroids.len(),
        frame.stone.planes.len(),
        "one centroid slot per plane"
    );
    assert!(geometry.facet_centroids.iter().any(Option::is_some));
    assert!(geometry.bounding_radius > 0.0);
    assert!((geometry.bounding_radius - frame.mesh_bounding_radius).abs() < 1e-12);
}

#[test]
fn a_reproject_frame_carries_geometry_shared_with_the_replan() {
    let mut pipeline = PreviewPipeline::new();
    let planned = pipeline
        .replan(
            web_job(0, (64, 48), -1),
            live_update::DEFAULT_PREVIEW_BUDGET,
            &ZeroClock,
        )
        .expect("planned")
        .geometry
        .expect("planned geometry");
    let turned = camera::orbit_step(PIN_CAMERA, 40.0, 0.0);
    let reprojected = pipeline
        .reproject(turned, (80, 60), 0, None)
        .expect("reprojected")
        .geometry
        .expect("a reproject carries geometry too");
    assert_eq!(reprojected.size, (80, 60));
    assert_eq!(reprojected.camera, turned);
    assert!(
        Arc::ptr_eq(&planned.corner_points, &reprojected.corner_points)
            && Arc::ptr_eq(&planned.facet_centroids, &reprojected.facet_centroids),
        "the corner and centroid tables are built once per mesh, not per frame"
    );
}

include!("tests/viewport_helpers.rs");
include!("tests/concave_frames.rs");
