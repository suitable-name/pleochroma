#[test]
fn shared_outlines_are_stamped_onto_any_style_and_absent_ones_change_nothing() {
    use super::{Outlines, SharedOutlines};
    use crate::raster::SolidStyle;
    use std::sync::Mutex;

    let plain = WorkerMemory::default();
    let untouched = plain.with_outlines(SolidStyle {
        provisional: vec![7],
        ..SolidStyle::default()
    });
    assert_eq!(
        untouched.provisional,
        vec![7],
        "no shared handle: the request's own style"
    );

    let shared: SharedOutlines = Arc::new(Mutex::new(Outlines::default()));
    let memory = WorkerMemory {
        outlines: Some(Arc::clone(&shared)),
        ..WorkerMemory::default()
    };
    // A style a `Planned` request built from scratch: empty outlines.
    let empty = memory.with_outlines(SolidStyle::default());
    assert!(empty.provisional.is_empty() && empty.moved.is_empty());

    *shared.lock().expect("lock") = Outlines {
        provisional: vec![3, 4],
        moved: vec![9],
    };
    let stamped = memory.with_outlines(SolidStyle::default());
    assert_eq!(stamped.provisional, vec![3, 4]);
    assert_eq!(stamped.moved, vec![9]);
    // Cleared by the app: the very next draw drops them, whatever request it serves.
    *shared.lock().expect("lock") = Outlines::default();
    let cleared = memory.with_outlines(stamped);
    assert!(cleared.provisional.is_empty() && cleared.moved.is_empty());
}

#[test]
fn a_pipeline_that_never_closed_a_solid_has_no_geometry() {
    let mut pipeline = PreviewPipeline::new();
    let frame = pipeline
        .render(RedrawRequest::Reproject {
            geometry: StoneGeometryBuf::from_halfspaces(&[
                (glam::Vec3::X, 1.0),
                (glam::Vec3::NEG_X, 1.0),
            ]),
            camera: PIN_CAMERA,
            size: (32, 32),
            view_mode: 0,
            gear: None,
        })
        .expect("a reproject always resolves");
    assert!(!frame.has_solid);
    assert!(frame.geometry.is_none());
}

#[test]
fn both_tracers_get_the_same_rolled_solid_edges_and_pick_geometry() {
    let snapshot = |mode| {
        let mut pipeline = PreviewPipeline::new();
        let mut job = desktop_job(mode, (160, 120), -1);
        job.camera.roll = 0.63;
        let frame = pipeline
            .replan(job, live_update::DEFAULT_PREVIEW_BUDGET, &ZeroClock)
            .expect("planned");
        assert!(frame.has_solid && frame.has_edges);
        assert!(frame.diagram.is_none());
        let geometry = frame.geometry.expect("closed geometry");
        assert_eq!(geometry.camera.roll, 0.63);
        (
            pipeline.edges_rgba().to_vec(),
            frame.pick.pick,
            pipeline.solid_rgba().to_vec(),
        )
    };
    let path = snapshot(2);
    let ray = snapshot(5);
    assert_eq!(
        path, ray,
        "the selected tracer must not change the solid layer"
    );
    assert!(ray.0.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0));
    assert!(ray.0.as_chunks::<4>().0.iter().any(|pixel| pixel[3] == 0));
}

#[test]
fn orbit_step_uses_the_desktop_signs_factor_and_wrap() {
    let pose = CameraPose {
        roll: 0.0,
        yaw: 1.0,
        pitch: 0.5,
        distance: 3.0,
    };
    let moved = camera::orbit_step(pose, 10.0, -5.0);
    assert!((moved.yaw - 0.92).abs() < 1e-6);
    assert!((moved.pitch - 0.46).abs() < 1e-6);
    assert_eq!(moved.distance, 3.0);
    let over_the_pole = camera::orbit_step(CameraPose { pitch: 3.1, ..pose }, 0.0, 10.0);
    assert!(over_the_pole.pitch < 0.0, "pitch wraps into [-pi, pi)");
}

#[test]
fn zoom_step_moves_closer_on_scroll_up_and_clamps_to_the_mesh() {
    assert!((camera::zoom_step(2.4, 100.0, 1.5) - 2.2).abs() < 1e-6);
    assert!((camera::zoom_step(2.4, -100.0, 1.5) - 2.6).abs() < 1e-6);
    assert!((camera::zoom_step(1.3, 1000.0, 1.5) - 1.2).abs() < 1e-6);
    assert!((camera::zoom_step(7.9, -1000.0, 1.5) - 8.0).abs() < 1e-5);
}

#[test]
fn standard_views_match_the_desktop_pose_pills() {
    use std::f32::consts::FRAC_PI_2;
    let pose = camera::RESET_POSE;
    let top = camera::standard_view(1, pose, 1.5);
    assert_eq!((top.yaw, top.pitch, top.distance), (0.0, FRAC_PI_2, 2.4));
    let right = camera::standard_view(4, pose, 1.5);
    assert_eq!((right.yaw, right.pitch), (FRAC_PI_2, 0.0));
    let front = camera::standard_view(99, pose, 1.5);
    assert_eq!((front.yaw, front.pitch), (0.0, 0.0));
    let fit = camera::standard_view(5, pose, 1.5);
    assert_eq!(
        (fit.yaw, fit.pitch),
        (pose.yaw, pose.pitch),
        "Fit keeps the angle"
    );
    let (min, max) = camera::orbit_distance_bounds(1.5);
    assert!(fit.distance >= min && fit.distance <= max);
}

const LIMITS: RasterLimits = RasterLimits {
    max_device_pixel_ratio: 1.5,
    min_edge: 64,
    max_edge: 960,
};

#[test]
fn raster_size_keeps_the_aspect_and_caps_the_longer_edge() {
    assert_eq!(
        view::raster_size_for_view(800.0, 600.0, 1.0, LIMITS),
        (800, 600)
    );
    assert_eq!(
        view::raster_size_for_view(400.0, 300.0, 2.0, LIMITS),
        (600, 450)
    );
    assert_eq!(
        view::raster_size_for_view(1600.0, 800.0, 1.0, LIMITS),
        (960, 480)
    );
    assert_eq!(
        view::raster_size_for_view(0.0, f32::NAN, 1.0, LIMITS),
        (64, 64)
    );
    assert_eq!(
        view::raster_size_for_view(300.0, 200.0, 0.5, LIMITS),
        (300, 200)
    );
}

#[test]
fn a_narrow_solid_view_is_drawn_at_the_minimum_aspect_and_a_wide_one_is_left_alone() {
    let four_thirds = 4.0 / 3.0;
    // 560 x 520 is narrower than 4:3: the height drops to 420 (560 / 4:3).
    assert_eq!(
        view::with_min_aspect((560, 520), four_thirds, 64),
        (560, 420)
    );
    // Tall: same width, a much shorter raster.
    assert_eq!(
        view::with_min_aspect((400, 800), four_thirds, 64),
        (400, 300)
    );
    // Already 4:3 or wider: untouched.
    assert_eq!(
        view::with_min_aspect((800, 600), four_thirds, 64),
        (800, 600)
    );
    assert_eq!(
        view::with_min_aspect((1000, 500), four_thirds, 64),
        (1000, 500)
    );
    // The minimum edge holds, and the height never grows.
    assert_eq!(view::with_min_aspect((64, 900), four_thirds, 64), (64, 64));
    assert_eq!(view::with_min_aspect((100, 60), 0.0, 64), (100, 60));
}

#[test]
fn diagram_raster_grows_a_small_view_to_the_minimum_width_and_keeps_the_aspect() {
    let limits = RasterLimits {
        max_edge: 1600,
        ..LIMITS
    };
    // Already wide enough: exactly `raster_size_for_view`.
    assert_eq!(
        view::diagram_raster_size(1300.0, 700.0, 1.0, limits, 1200),
        view::raster_size_for_view(1300.0, 700.0, 1.0, limits)
    );
    // A narrow view is drawn 1200 wide, same aspect.
    assert_eq!(
        view::diagram_raster_size(600.0, 400.0, 1.0, limits, 1200),
        (1200, 800)
    );
    // A tall view hits the long-edge cap first: uniformly scaled down, aspect kept.
    let (w, h) = view::diagram_raster_size(390.0, 700.0, 1.0, limits, 1200);
    assert_eq!(h, 1600);
    assert!(
        (w as f32 / h as f32 - 390.0 / 700.0).abs() < 0.01,
        "{w}x{h}"
    );
    // A zero-size view still gets a valid (positive) raster.
    let (w, h) = view::diagram_raster_size(0.0, 0.0, 1.0, limits, 1200);
    assert!(w >= limits.min_edge && h >= limits.min_edge);
}

#[test]
fn contain_pixel_maps_through_the_letterbox() {
    // A 100x50 image in a 200x200 view: scale 2, drawn at y 50..150.
    assert_eq!(
        view::contain_pixel(0.0, 50.0, 200.0, 200.0, 100, 50),
        Some((0, 0))
    );
    assert_eq!(
        view::contain_pixel(199.0, 149.0, 200.0, 200.0, 100, 50),
        Some((99, 49))
    );
    assert_eq!(
        view::contain_pixel(100.0, 20.0, 200.0, 200.0, 100, 50),
        None
    );
    assert_eq!(
        view::contain_pixel(100.0, 150.0, 200.0, 200.0, 100, 50),
        None
    );
    assert_eq!(view::contain_pixel(10.0, 10.0, 0.0, 200.0, 100, 50), None);
    // Same aspect: a plain scale.
    assert_eq!(
        view::contain_pixel(30.0, 20.0, 400.0, 300.0, 800, 600),
        Some((60, 40))
    );
}

/// The web's pointer -> pick-frame conversion: the fractional, unclamped image position
/// under a logical pointer, through the same letterboxing `contain_pixel` uses.
#[test]
fn contain_fit_converts_the_pointer_to_the_pick_frame_through_the_letterbox() {
    // A 100x50 raster in a 200x200 view: scale 2, bars above and below (drawn y 50..150).
    let fit = view::contain_fit(200.0, 200.0, 100, 50).expect("a real view");
    assert_eq!((fit.offset_x, fit.offset_y, fit.scale), (0.0, 50.0, 2.0));
    assert_eq!(fit.to_image(0.0, 50.0), (0.0, 0.0));
    assert_eq!(fit.to_image(100.0, 100.0), (50.0, 25.0));
    assert_eq!(fit.to_image(200.0, 150.0), (100.0, 50.0));
    assert_eq!(fit.to_view(50.0, 25.0), (100.0, 100.0));
    // Fractional pixels survive (a handle drag is measured in sub-pixels).
    assert_eq!(fit.to_image(101.0, 101.0), (50.5, 25.5));
    // A pointer in a bar maps outside the raster instead of being clamped, so a drag
    // that leaves the image keeps its travel.
    assert_eq!(fit.to_image(100.0, 20.0), (50.0, -15.0));
    assert_eq!(fit.to_image(100.0, 190.0), (50.0, 70.0));
    // Empty views and images have no placement.
    assert_eq!(view::contain_fit(0.0, 200.0, 100, 50), None);
    assert_eq!(view::contain_fit(200.0, 200.0, 0, 50), None);
}

/// The conversion agrees with `contain_pixel` everywhere inside the image, and the two
/// directions are exact inverses, for the awkward aspects a rounded raster gives.
#[test]
fn contain_fit_agrees_with_contain_pixel_and_round_trips() {
    // (view size, raster size): a 1.5x display, an odd width, a capped long edge.
    let cases = [
        ((811.3_f32, 519.7_f32), (1217_u32, 780_u32)),
        ((640.0, 360.0), (960, 540)),
        ((333.3, 900.1), (500, 1350)),
        ((1200.0, 500.0), (960, 400)),
    ];
    for ((view_w, view_h), (image_w, image_h)) in cases {
        let fit = view::contain_fit(view_w, view_h, image_w, image_h).expect("a real view");
        for step_x in 0..=20 {
            for step_y in 0..=20 {
                let x = view_w * step_x as f32 / 20.0;
                let y = view_h * step_y as f32 / 20.0;
                let (px, py) = fit.to_image(x, y);
                let inside = px >= 0.0 && py >= 0.0 && px < image_w as f32 && py < image_h as f32;
                let pixel = view::contain_pixel(x, y, view_w, view_h, image_w, image_h);
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "`inside` is only true for non-negative px and py within the image"
                )]
                let expected = inside.then(|| (px.floor() as u32, py.floor() as u32));
                assert_eq!(
                    pixel, expected,
                    "({x}, {y}) in {view_w}x{view_h} for {image_w}x{image_h}"
                );
                let (back_x, back_y) = fit.to_view(px, py);
                assert!(
                    (back_x - x).abs() < 1e-2 && (back_y - y).abs() < 1e-2,
                    "({x}, {y}) came back as ({back_x}, {back_y})"
                );
            }
        }
    }
}

#[test]
fn a_double_click_enlarges_the_panel_under_the_pointer_and_toggles_back() {
    assert_eq!(
        view::toggle_enlarged_panel(-1, Some(PanelKind::Pavilion)),
        1
    );
    assert_eq!(view::toggle_enlarged_panel(-1, None), -1);
    assert_eq!(
        view::toggle_enlarged_panel(1, Some(PanelKind::Pavilion)),
        -1
    );
    assert_eq!(view::toggle_enlarged_panel(2, None), -1);
    for panel in [PanelKind::Crown, PanelKind::Pavilion, PanelKind::Profile] {
        assert_eq!(
            super::panel_kind_from_index(view::panel_index(panel)),
            Some(panel)
        );
    }
}

#[test]
fn a_real_diagram_double_click_resolves_its_panel() {
    let mut pipeline = PreviewPipeline::new();
    let frame = pipeline
        .replan(
            web_job(3, (360, 180), -1),
            live_update::DEFAULT_PREVIEW_BUDGET,
            &ZeroClock,
        )
        .expect("planned");
    let diagram = frame.diagram.expect("the fixture closes");
    // The three panels sit side by side; the middle of each third is inside it.
    let panel_at_view = |x: f32| {
        view::contain_pixel(x, 180.0, 720.0, 360.0, diagram.width, diagram.height)
            .and_then(|(px, py)| diagram.panel_at(px, py))
    };
    let crown = panel_at_view(120.0);
    assert_eq!(crown, Some(PanelKind::Crown));
    assert_eq!(view::toggle_enlarged_panel(-1, crown), 0);
}

/// The Diagram view's panel layout rides on the frame's geometry, so an app places drag
/// handles on the pixels that were drawn; every other view mode carries none.
#[test]
fn a_diagram_frame_carries_the_layout_it_was_drawn_with() {
    let plan = |view_mode: u8, enlarged: i32| {
        PreviewPipeline::new()
            .replan(
                desktop_job(view_mode, (360, 180), enlarged),
                live_update::DEFAULT_PREVIEW_BUDGET,
                &ZeroClock,
            )
            .expect("planned")
    };
    let solid = plan(0, -1);
    assert!(
        solid.geometry.expect("geometry").diagram.is_none(),
        "only the Diagram view has panels"
    );

    let three = plan(3, -1);
    let drawn = three.diagram.expect("the fixture closes");
    let layout = three
        .geometry
        .expect("geometry")
        .diagram
        .expect("a Diagram frame carries its panel layout");
    assert_eq!(*layout, drawn.layout);
    assert_eq!(layout.panels.len(), 3);
    assert!(!layout.enlarged);
    assert_eq!(layout.gear_teeth, 96);

    let enlarged = plan(3, 1);
    let layout = enlarged
        .geometry
        .expect("geometry")
        .diagram
        .expect("an enlarged frame carries its layout too");
    assert!(layout.enlarged);
    assert_eq!(layout.panels.len(), 1);
    assert_eq!(layout.panels[0].kind, PanelKind::Pavilion);
}

/// A partly cut stone's frame says which tiers its facet ids number, so a pick or an
/// outline built from the frame uses the same facets; the finished stone says nothing.
#[test]
fn a_cut_frame_names_the_tiers_its_facet_ids_number() {
    let tier_count = pin_design().tiers.len();
    let geometry_at = |steps: Option<usize>| {
        let mut job = desktop_job(0, (160, 120), -1);
        job.cut_steps = steps;
        PreviewPipeline::new()
            .replan(job, live_update::DEFAULT_PREVIEW_BUDGET, &ZeroClock)
            .expect("planned")
            .geometry
            .expect("a cut stone still meshes")
    };
    assert!(geometry_at(None).visible_tiers.is_none());

    let rough = geometry_at(Some(0));
    let tiers = rough.visible_tiers.expect("the rough names its tiers");
    assert_eq!(tiers.len(), tier_count);
    assert!(tiers.iter().all(|&shown| !shown), "no tier is cut yet");

    let three = geometry_at(Some(3));
    let tiers = three
        .visible_tiers
        .expect("a partly cut stone names its tiers");
    let shown: Vec<usize> = tiers
        .iter()
        .enumerate()
        .filter(|&(_, &shown)| shown)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        shown,
        [4, 5, 6],
        "the first three steps of the cutting order are the girdle, the pavilion mains and \
         the lower girdles (stored at 4, 5, 6), not the first three stored tiers"
    );
}

#[test]
fn step_selection_follows_the_desktop_rule() {
    assert_eq!(view::step_selection(None, 1, 5), Some(0));
    assert_eq!(view::step_selection(None, -1, 5), Some(4));
    assert_eq!(view::step_selection(Some(3), 10, 5), Some(4));
    assert_eq!(view::step_selection(Some(3), -10, 5), Some(0));
    assert_eq!(view::step_selection(Some(2), 1, 5), Some(3));
    assert_eq!(view::step_selection(Some(2), 1, 0), Some(2));
}

#[test]
fn facets_of_tiers_collects_every_facet_of_the_multi_selection() {
    let table = [None, Some(0), Some(1), Some(0), Some(2)];
    assert_eq!(
        view::facets_of_tiers(&table, &BTreeSet::from([0, 2])),
        vec![1, 3, 4]
    );
    assert_eq!(
        view::facets_of_tiers(&table, &BTreeSet::new()),
        Vec::<u32>::new()
    );
}

#[test]
fn dirty_tiers_names_edited_tiers_and_refuses_structural_edits() {
    let before = pin_design();
    let mut after = before.clone();
    after.tiers[2].angle_deg += 0.5;
    after.tiers[5].name = "Renamed".to_string();
    assert_eq!(
        view::dirty_tiers(&before, &after),
        Some(BTreeSet::from([2, 5]))
    );
    assert_eq!(view::dirty_tiers(&before, &before), Some(BTreeSet::new()));

    let mut added = before.clone();
    added.tiers.push(rbc_tier("Extra", 20.0, &[], 0.5));
    assert_eq!(view::dirty_tiers(&before, &added), None);
    let mut regeared = before.clone();
    regeared.meta.gear_teeth = 80;
    assert_eq!(view::dirty_tiers(&before, &regeared), None);
}

#[test]
fn a_pinned_design_plans_without_the_solver_and_a_free_tier_does_not() {
    let mut design = pin_design();
    assert!(view::plans_without_solver(&design));
    design.tiers[3].constraint = MeetConstraint::MeetExisting;
    assert!(!view::plans_without_solver(&design));
}

#[test]
fn replan_basis_prefers_the_current_solve_then_the_cached_masts() {
    let design = pin_design();
    let solved = design.solve().expect("pinned");
    let masts = |basis: &ReplanBasis| match basis {
        ReplanBasis::Chain { last_solved, dirty } => Some((
            last_solved.iter().map(|t| t.mast).collect::<Vec<_>>(),
            dirty.clone(),
        )),
        ReplanBasis::FullSolve => None,
    };
    let expected: Vec<f64> = solved.iter().map(|t| t.mast).collect();
    assert_eq!(
        masts(&view::replan_basis(&design, Some(&solved), None)),
        Some((expected.clone(), BTreeSet::new()))
    );
    let mut edited = design.clone();
    edited.tiers[1].angle_deg += 1.0;
    assert_eq!(
        masts(&view::replan_basis(&edited, None, Some((&design, &solved)))),
        Some((expected, BTreeSet::from([1])))
    );
    let mut shorter = design.clone();
    shorter.tiers.pop();
    assert!(
        matches!(
            view::replan_basis(&shorter, Some(&solved), Some((&design, &solved))),
            ReplanBasis::FullSolve
        ),
        "misaligned masts never chain"
    );
}
