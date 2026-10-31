//! The final denoise stops for an epoch that ended: its flag is raised exactly when the
//! epoch ends, and a raised flag keeps the denoiser (and the permit and lock around it)
//! untouched.

use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use pleochroma_cut_core::templates::TEMPLATES;
use pleochroma_editor::EditorSession;
use pleochroma_web_core::scene::OwnedScene;

use super::{Follow, Shared, relock};
use crate::{
    app_state::AppState,
    design_state::{Installed, begin_solve, install, run_solve},
    platform::Platform,
    platform_mock::MockPlatform,
    render::{RenderView, scene},
    settings::RenderBackend,
};

const VIEW: RenderView = RenderView {
    width: 32,
    height: 24,
};

/// A built scene of the round brilliant template at [`VIEW`].
fn built_scene() -> OwnedScene {
    let platform: Arc<dyn Platform> = Arc::new(MockPlatform::default());
    let mut state = AppState::new(platform);
    let template = &TEMPLATES[0];
    state.session = EditorSession::from_template(
        template.fresh_spec(template.default_material_selection()),
        1,
    );
    state.settings.render.backend = RenderBackend::DeviceCpu;
    let job = begin_solve(&mut state).expect("a design with no answer gets a job");
    let response = run_solve(&job, &std::sync::atomic::AtomicBool::new(false));
    let installed = install(
        &mut state,
        job.generation,
        response,
        Duration::from_millis(5),
    );
    assert_eq!(installed, Installed::Applied);
    let prepared = scene::prepare(&state, VIEW).expect("a solved design prepares");
    OwnedScene::build(&prepared.key.spec, prepared.hdr).expect("builds")
}

/// A one-permit pool with epoch `id` begun and its scene installed, and the follow of it.
fn shared_with_epoch(id: u64) -> (Shared, Follow) {
    let shared = Shared::new(1);
    shared.backend.select(RenderBackend::DeviceCpu);
    let follow = begin_with_scene(&shared, id);
    (shared, follow)
}

fn begin_with_scene(shared: &Shared, id: u64) -> Follow {
    shared.begin_epoch(id, 8, true);
    shared.install_scene(id, Ok(built_scene()), 8);
    relock(&shared.inner)
        .snapshot()
        .expect("the installed epoch can be followed")
}

/// A flag that is raised when its epoch ends -- replaced, stopped or closed -- and not
/// before; each epoch has its own, so the next epoch's flag starts clear.
#[test]
fn an_epoch_raises_its_flag_exactly_when_it_ends() {
    let (shared, first) = shared_with_epoch(1);
    assert!(!first.obsolete.load(Ordering::Acquire));
    // A pause is not the end of the epoch.
    shared.update_control(|control| control.user_paused = true);
    assert!(!first.obsolete.load(Ordering::Acquire));
    shared.update_control(|control| control.user_paused = false);

    let second = begin_with_scene(&shared, 2);
    assert!(first.obsolete.load(Ordering::Acquire), "replaced");
    assert!(!second.obsolete.load(Ordering::Acquire), "a fresh flag");

    shared.stop();
    assert!(second.obsolete.load(Ordering::Acquire), "stopped");

    let third = begin_with_scene(&shared, 3);
    shared.close();
    assert!(third.obsolete.load(Ordering::Acquire), "closed");
}

/// A raised flag keeps the denoiser from starting even while the epoch still counts as
/// current (before the fix only the epoch id was checked, so the full filter ran and the
/// publish guard alone dropped the picture).
#[test]
fn a_raised_flag_never_reaches_the_denoiser() {
    let (shared, follow) = shared_with_epoch(1);
    follow.obsolete.store(true, Ordering::Release);
    shared.show_denoised(&follow, true);
    assert_eq!(shared.denoise_calls.load(Ordering::Relaxed), 0);
    assert!(shared.take_frame().is_none(), "nothing was published");
}

/// The same obsolete follow, with the pool's only permit held elsewhere, returns at once
/// instead of waiting for the permit, and without denoising.
#[test]
fn an_obsolete_epoch_does_not_wait_for_the_permit() {
    let (shared, follow) = shared_with_epoch(1);
    let _held = shared.policy.acquire(&|| false).expect("the one permit");
    shared.begin_epoch(2, 8, true);
    assert!(follow.obsolete.load(Ordering::Acquire));
    shared.show_denoised(&follow, true);
    assert_eq!(shared.denoise_calls.load(Ordering::Relaxed), 0);
}

/// The live epoch still denoises and publishes (the guard against over-cancelling).
#[test]
fn a_live_epoch_still_publishes_its_denoised_picture() {
    let (shared, follow) = shared_with_epoch(1);
    shared.show_denoised(&follow, true);
    assert_eq!(shared.denoise_calls.load(Ordering::Relaxed), 1);
    let frame = shared.take_frame().expect("the denoised picture");
    assert!(frame.denoised);
    assert_eq!(frame.rgba.len(), 32 * 24 * 4);
}
