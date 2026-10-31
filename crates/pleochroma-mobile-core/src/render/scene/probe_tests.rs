//! The Render driver's poll must not describe the whole scene while nothing changed: a
//! counter in `prepare` sees how often it does.
//!
//! The scene is the round brilliant template at a tiny sample target, like the render
//! tests; every call below is on the test thread, so the per-thread counter sees exactly
//! the driver's own `prepare` calls.

use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use pleochroma_cut_core::templates::TEMPLATES;
use pleochroma_editor::EditorSession;

use super::prepare_calls;
use crate::{
    app_state::AppState,
    design_state::{Installed, begin_solve, install, run_solve},
    layout::Destination,
    platform::Platform,
    platform_mock::MockPlatform,
    settings::RenderBackend,
    viewport::{
        ViewMode,
        render_view::{Maintain, maintain},
    },
};

/// A state with the Render mode on show over the round brilliant template, not solved.
fn render_on_show() -> AppState {
    let platform: Arc<dyn Platform> = Arc::new(MockPlatform::default());
    let mut app = AppState::new(platform);
    let template = &TEMPLATES[0];
    app.session = EditorSession::from_template(
        template.fresh_spec(template.default_material_selection()),
        1,
    );
    app.settings.render.live_spp_compact = 8;
    app.settings.render.denoise = false;
    app.settings.render.backend = RenderBackend::DeviceCpu;
    app.document.origin = crate::document::DocumentOrigin::New;
    app.destination = Destination::Design;
    app.viewport.set_view(390.0, 320.0, 1.0, true);
    app.viewport.set_mode(ViewMode::Render);
    app
}

fn solve_now(app: &mut AppState) {
    let job = begin_solve(app).expect("a design with no answer gets a job");
    let response = run_solve(&job, &AtomicBool::new(false));
    let installed = install(app, job.generation, response, Duration::from_millis(5));
    assert_eq!(installed, Installed::Applied);
}

/// The time of poll `n`, 100 ms apart (the shown render's poll rate).
fn at(n: u64) -> Duration {
    Duration::from_secs(10) + Duration::from_millis(100 * n)
}

/// Polls an unchanged, converged render: after the start, no poll describes the scene
/// (before the probe every poll called `prepare` through `needs_restart`).
#[test]
fn polls_of_an_unchanged_render_never_describe_the_scene() {
    let mut app = render_on_show();
    solve_now(&mut app);
    assert_eq!(maintain(&mut app, at(0)), Maintain::Started);
    let after_start = prepare_calls();
    assert!(after_start >= 1, "the start described the scene");
    for poll in 1..=50 {
        assert_eq!(maintain(&mut app, at(poll)), Maintain::Idle);
    }
    assert_eq!(prepare_calls(), after_start, "50 polls, no description");
}

/// A change the scene reads still restarts, and the polls after that are free again.
#[test]
fn a_changed_input_restarts_once_and_the_polls_after_it_are_free() {
    let mut app = render_on_show();
    solve_now(&mut app);
    assert_eq!(maintain(&mut app, at(0)), Maintain::Started);
    assert_eq!(maintain(&mut app, at(1)), Maintain::Idle);

    // The lighting is read by the scene description: the probe notices and the full
    // comparison finds the scene changed.
    app.settings.lighting.exposure += 0.5;
    let before = prepare_calls();
    assert_eq!(maintain(&mut app, at(2)), Maintain::Started);
    assert!(prepare_calls() > before, "the change was described");

    let after_restart = prepare_calls();
    for poll in 3..=30 {
        assert_eq!(maintain(&mut app, at(poll)), Maintain::Idle);
    }
    assert_eq!(prepare_calls(), after_restart);
}

/// The scene description carries the design's color variant through the resolved render
/// material (`render_material`), not only through the design's generation: with the generation
/// held, a variant on the design still makes another description, so no later key could treat the
/// recolored stone as the one already traced.
#[test]
fn the_scene_key_follows_the_designs_color_variant() {
    use super::prepare;
    use crate::{design_props::select_material, render::RenderView};
    use pleochroma::optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::AbsorptionUnit,
    };
    use pleochroma_cut_core::material::VariantChoice;

    let mut app = render_on_show();
    select_material(&mut app, "Sapphire").expect("Sapphire is built in");
    solve_now(&mut app);
    let view = RenderView {
        width: 64,
        height: 48,
    };
    let key_of = |app: &AppState| {
        prepare(app, view)
            .expect("the solved design describes a scene")
            .key
    };
    let plain = key_of(&app);

    // Set on the design directly: the generation does not move, so only the material can tell
    // the two descriptions apart.
    let choice = |peak: f32| VariantChoice {
        key: "sapphire.test".to_owned(),
        label: "Test Pink".to_owned(),
        absorption: AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::new(550.0, 40.0, peak)],
            vec![AbsorptionBand::new(430.0, 40.0, 0.12)],
        ),
        unit: AbsorptionUnit::PerMm,
        reference_path_mm: 7.0,
    };
    let sapphire = app.session.design.material.clone();
    app.session.design.material = sapphire.clone().with_color_variant(Some(choice(0.35)));
    let recolored = key_of(&app);
    assert_eq!(recolored.generation, plain.generation, "test premise");
    assert_ne!(
        recolored.spec, plain.spec,
        "the variant is in the scene description"
    );

    app.session.design.material = sapphire.with_color_variant(Some(choice(0.36)));
    assert_ne!(
        key_of(&app).spec,
        recolored.spec,
        "so is another amplitude of the same band"
    );
}

/// A failed start (an unsolved design) records its attempt: unchanged polls do not
/// describe the scene again, and a change of the design does.
#[test]
fn a_failed_start_is_not_described_again_until_the_design_changes() {
    let mut app = render_on_show();
    assert!(matches!(maintain(&mut app, at(0)), Maintain::Blocked(_)));
    let after_failure = prepare_calls();
    for poll in 1..=30 {
        assert!(matches!(maintain(&mut app, at(poll)), Maintain::Blocked(_)));
    }
    assert_eq!(prepare_calls(), after_failure, "30 polls, no description");

    // The design gets its answer (the solve): the attempt is made again.
    solve_now(&mut app);
    assert_eq!(maintain(&mut app, at(31)), Maintain::Started);
    assert!(prepare_calls() > after_failure);
}

/// A new design generation after a converged render restarts through the probe, and the
/// failed start it ends in (nothing solved yet) is not described again by later polls.
#[test]
fn a_new_generation_restarts_and_a_failed_restart_is_recorded() {
    let mut app = render_on_show();
    solve_now(&mut app);
    assert_eq!(maintain(&mut app, at(0)), Maintain::Started);

    app.session.mark_unsaved();
    let before = prepare_calls();
    assert!(matches!(maintain(&mut app, at(1)), Maintain::Blocked(_)));
    assert!(prepare_calls() > before, "the new generation was described");

    let after_failure = prepare_calls();
    for poll in 2..=20 {
        assert_ne!(maintain(&mut app, at(poll)), Maintain::Started);
    }
    assert_eq!(prepare_calls(), after_failure);
}
