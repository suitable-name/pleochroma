// ---- concave stones -------------------------------------------------------------
//
// The fixture is the pin design's planes with one hand-made cylinder groove across the
// table, so this crate's tests need nothing from cut-core's tool resolver.

/// The pin design plus the concave tier the groove below stands for.
fn concave_design() -> Design {
    let mut design = pin_design();
    design.concave_tiers.push(ConcaveTier {
        name: "Groove".to_string(),
        angle_deg: 10.0,
        indices: vec![0.0],
        instructions: String::new(),
        tool: ConcaveTool::Cylinder,
        tool_azimuth_deg: 90.0,
        displacement: [0.0, 0.0, 0.0],
        diameter_ratio: 0.25,
        tool_angle_deg: None,
        motion: ToolMotion::Reciprocating,
    });
    design.ensure_concave_tier_ids();
    design
}

/// The pin design's flat planes plus a cylinder along `z` cutting 0.19 into the table
/// (`y = 0.32`), as one placement of concave tier 0.
fn concave_fixture() -> StoneGeometryBuf {
    let planned = build_planned_frame(
        desktop_job(0, (64, 48), -1),
        live_update::DEFAULT_PREVIEW_BUDGET,
        &ZeroClock,
    );
    StoneGeometryBuf {
        tools: vec![ToolPrimitive::cylinder(
            glam::Vec3::new(0.0, 0.25, 0.0),
            glam::Vec3::Z,
            0.12,
            2.0,
        )],
        placements: vec![(0, 0)],
        ..StoneGeometryBuf::from_halfspaces(&planned.planes)
    }
}

/// A camera above the stone, so the groove floor and walls face it.
fn concave_camera(yaw: f32, pitch: f32) -> CameraPose {
    CameraPose {
        roll: 0.0,
        yaw,
        pitch,
        distance: 2.4,
    }
}

fn reproject(
    pipeline: &mut PreviewPipeline,
    geometry: &StoneGeometryBuf,
    camera: CameraPose,
    size: (u32, u32),
    view_mode: u8,
) -> super::RenderedFrame {
    pipeline
        .render(RedrawRequest::Reproject {
            geometry: geometry.clone(),
            camera,
            size,
            view_mode,
            gear: Some((96, 0.0)),
        })
        .expect("a reproject always resolves")
}

/// `[solid image, solid pick, diagram image, diagram pick]` hashes of one frame.
fn frame_hashes(pipeline: &PreviewPipeline, frame: &super::RenderedFrame) -> [u64; 4] {
    let diagram = frame.diagram.as_ref();
    [
        fnv(pipeline.solid_rgba()),
        fnv_u32(&frame.pick.pick),
        diagram.map_or(0, |d| fnv(&d.color)),
        diagram.map_or(0, |d| fnv_u32(&d.pick)),
    ]
}

/// Golden hashes of [`concave_fixture`] for the three standard views, per
/// [`CONCAVE_VIEWS`]. Like the planar pins they hold the platform math library's bits,
/// so they are only compared on Windows; `None` until they are recorded there.
const CONCAVE_PINS: Option<[[u64; 4]; 3]> = None;

/// The three standard views: Solid, the three-panel Diagram and a square Diagram
/// (a `Reproject` carries no enlarged-panel choice, so all three panels show).
const CONCAVE_VIEWS: [(u8, (u32, u32)); 3] = [(0, (160, 120)), (3, (360, 180)), (3, (240, 240))];

#[test]
fn concave_fixture_frames_are_pinned() {
    let stone = concave_fixture();
    let planar = StoneGeometryBuf::planes_only(stone.planes.clone());
    let camera = concave_camera(0.6, 1.0);
    let mut recorded = Vec::new();
    for (view_mode, size) in CONCAVE_VIEWS {
        // Path A: a long-lived pipeline, hit twice so the second frame is served from
        // the mesh cache.
        let mut pipeline = PreviewPipeline::new();
        let _ = reproject(&mut pipeline, &stone, camera, size, view_mode);
        let again = reproject(&mut pipeline, &stone, camera, size, view_mode);
        let hashes = frame_hashes(&pipeline, &again);
        // Path B: the same request on fresh worker state.
        let mut fresh = PreviewPipeline::new();
        let first = reproject(&mut fresh, &stone, camera, size, view_mode);
        assert_eq!(
            frame_hashes(&fresh, &first),
            hashes,
            "view {view_mode}: a cache hit must draw the same bytes as a cold build"
        );
        // The groove changes the picture.
        let mut flat = PreviewPipeline::new();
        let flat_frame = reproject(&mut flat, &planar, camera, size, view_mode);
        assert_ne!(
            frame_hashes(&flat, &flat_frame)[..2],
            hashes[..2],
            "view {view_mode}: the groove must change the solid image"
        );
        recorded.push(hashes);
    }
    if cfg!(windows)
        && let Some(pins) = CONCAVE_PINS
    {
        assert_eq!(recorded, pins.to_vec());
    }
}

#[test]
fn pick_buffer_ids_on_concave_fixture_resolve_to_tier_and_placement() {
    let design = concave_design();
    let solved = design.solve().expect("every tier is pinned");
    let stone = concave_fixture();
    let tool_id = stone.planes.len();
    let map = FacetMap::from_design_with_tools(&design, &solved, &stone.placements);
    assert_eq!(map.facet_count(), tool_id + 1);

    let mut pipeline = PreviewPipeline::new();
    let frame = reproject(
        &mut pipeline,
        &stone,
        concave_camera(0.6, 1.0),
        (160, 120),
        0,
    );
    let mut seen_tool = false;
    let mut seen_flat = false;
    for y in 0..frame.pick.height {
        for x in 0..frame.pick.width {
            let Some(id) = frame.pick.facet_at(x, y) else {
                continue;
            };
            match map.kind_of(id as usize) {
                FacetKind::Concave { tier, placement } => {
                    assert_eq!((tier, placement), (0, 0));
                    assert_eq!(id as usize, tool_id);
                    seen_tool = true;
                }
                FacetKind::Flat => {
                    assert!((id as usize) < tool_id);
                    seen_flat = true;
                }
            }
        }
    }
    assert!(seen_tool, "the groove is visible from above");
    assert!(seen_flat);
    assert_eq!(
        map.hover_text(tool_id, 1.54),
        "Groove CYL θ 90.0° D 0.250",
        "a picked tool id reads as its tier name, tool code, theta and diameter"
    );
}

/// A planned frame of a design with concave tiers carries a concave-owner table parallel
/// to `facet_tier`: `Some(concave index)` for exactly the tool facets, `None` for flat ids,
/// while `facet_tier` itself still names no tier for a tool facet.
#[test]
fn planned_frame_carries_the_concave_owner_of_every_tool_facet() {
    let design = Design::concave_fixture();
    let n_d = design.effective_refractive_index();
    let job = PlanJob {
        design: Arc::new(design.clone()),
        dirty: BTreeSet::new(),
        last_solved: None,
        camera: PIN_CAMERA,
        size: (64, 48),
        selected_tier: None,
        n_d,
        view_mode: 3,
        generation: 1,
        show_preform: true,
        enlarged_panel: -1,
        tier_cutoff: None,
        cut_steps: None,
    };
    let mut pipeline = PreviewPipeline::new();
    let frame = pipeline
        .replan(job, live_update::DEFAULT_PREVIEW_BUDGET, &ZeroClock)
        .expect("a planned request always resolves");
    let solved = design.solve().expect("the fixture solves");
    let map = FacetMap::from_design_with_tools(&design, &solved, &frame.stone.placements);
    assert_eq!(frame.facet_concave_tier.len(), map.facet_count());
    assert_eq!(frame.facet_tier.len(), map.facet_count());
    let mut tool_facets = 0;
    for id in 0..map.facet_count() {
        match map.kind_of(id) {
            FacetKind::Concave { tier, .. } => {
                tool_facets += 1;
                assert_eq!(frame.facet_concave_tier[id], Some(tier), "facet {id}");
                assert_eq!(frame.facet_tier[id], None, "facet {id} has no flat tier");
            }
            FacetKind::Flat => {
                assert_eq!(frame.facet_concave_tier[id], None, "facet {id}");
                assert_eq!(frame.facet_tier[id], map.tier_of(id), "facet {id}");
            }
        }
    }
    assert!(tool_facets > 0, "the fixture's tools are placed");
    assert_eq!(
        frame.diagram_facet_concave_tier.as_deref(),
        Some(frame.facet_concave_tier.as_slice()),
        "a Diagram-mode frame carries the same table"
    );
}

#[test]
fn concave_raster_has_no_hairline_gaps() {
    let stone = concave_fixture();
    let mut pipeline = PreviewPipeline::new();
    for camera in [
        concave_camera(0.6, 1.0),
        concave_camera(2.2, 0.7),
        concave_camera(4.0, 1.3),
        PIN_CAMERA,
    ] {
        let frame = reproject(&mut pipeline, &stone, camera, (200, 150), 0);
        let (w, h) = (frame.pick.width as usize, frame.pick.height as usize);
        let at = |x: usize, y: usize| frame.pick.pick[y * w + x];
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let all_stone = at(x - 1, y) != 0
                    && at(x + 1, y) != 0
                    && at(x, y - 1) != 0
                    && at(x, y + 1) != 0;
                assert!(
                    at(x, y) != 0 || !all_stone,
                    "unset pick pixel ({x}, {y}) surrounded by stone at {camera:?}"
                );
            }
        }
    }
}

#[test]
fn a_reproject_with_different_tools_resets_the_remembered_style() {
    let stone = concave_fixture();
    let planar = StoneGeometryBuf::planes_only(stone.planes.clone());
    let mut pipeline = PreviewPipeline::new();
    let camera = concave_camera(0.6, 1.0);
    let _ = reproject(&mut pipeline, &stone, camera, (64, 48), 0);
    assert_eq!(pipeline.memory().tools.len(), 1);
    assert_eq!(pipeline.memory().geometry().as_ref(), Some(&stone));
    let _ = reproject(&mut pipeline, &planar, camera, (64, 48), 0);
    assert!(
        pipeline.memory().tools.is_empty(),
        "a planar stone forgets the earlier tools"
    );
    assert_eq!(pipeline.memory().geometry().as_ref(), Some(&planar));
}
