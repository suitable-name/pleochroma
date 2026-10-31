//! Tutorial video harness for `pleochroma-cut` (custom `main`, `harness = false`): plays every
//! built-in tutorial in the real application and records it as a video.
//!
//! It drives the real `MainWindow` with all its Rust wiring on Slint's testing backend with the
//! software renderer, inside a throw-away profile (the same isolation as the `ui_harness` and
//! `ui_screens` targets), starts each tutorial from the tutorial browser and finishes it step by
//! step the way a learner would. A drawn mouse cursor glides to what each step is about, a ripple
//! shows the click, long background work (solves, renders) is sped up or cut. One scenario is one
//! tutorial (`video:<tutorial-id>`); the app is relaunched fresh for each.
//!
//! Run (ffmpeg must be on `PATH`, or named by `UIPROBE_FFMPEG`; debug mode needs no ffmpeg):
//!
//! ```text
//! cargo tutorial-videos
//! cargo tutorial-videos -- welcome worked   (only tutorials whose id contains one of these)
//! PLEOCHROMA_HARNESS_LIBRARY=C:\path\to\facet_diagrams.sqlite cargo tutorial-videos
//! cargo tutorial-videos --features gpu      (faster live renders)
//! ```
//!
//! Arguments that do not start with `-` select tutorials by id substring (the harness filters
//! scenarios by name). The run needs Slint debug info (the alias's environment sets it).
//!
//! Environment variables:
//!
//! - `PLEOCHROMA_VIDEOS_OUT`: the output folder (default `<manifest>/../../videos`, resolved
//!   before the profile moves the working directory).
//! - `PLEOCHROMA_VIDEOS_4K`: keep the 3840 x 2160 capture instead of encoding 1920 x 1080.
//! - `PLEOCHROMA_VIDEOS_FPS`: the frame rate, 24 to 60 (default 30).
//! - `PLEOCHROMA_VIDEOS_ALLOW_INJECT`: rejected; recordings require visible user input.
//! - `PLEOCHROMA_VIDEOS_DEBUG`: debug mode. No video is recorded (no recorder, no ffmpeg); each
//!   step writes before/after screenshots and a step log to `<out>/debug/<id>/` (see
//!   `DEBUG_MODE.md`).
//! - `PLEOCHROMA_HARNESS_LIBRARY`: a `facet_diagrams.sqlite` to copy into the profile (it is
//!   never opened itself). Without it a small fixture library is used.
//! - `UIPROBE_JOBS=serial`: one tutorial at a time; `UIPROBE_FFMPEG`: the ffmpeg executable.
//!
//! Per tutorial, in its browser section's folder `<ss>-<section>/` (for example
//! `05-solving-and-checking/`): `<nn>-<id>.mp4` (1920 x 1080, 30 fps, H.264), `<nn>-<id>.srt`
//! (one cue group per step) and `<nn>-<id>.chapters.txt` (a `YouTube` chapter list), where `<ss>`
//! is the section's place and `<nn>` the tutorial's place in the browser order, so the folders
//! and files sorted by name are the upload order; `playlist.md` (at the top of the output folder)
//! lists number, section, title and file. Each is announced as
//! `tutorial-videos: wrote videos/<ss>-<section>/<nn>-<id>.mp4 (<mm:ss>, <n> frames)`. A tutorial whose run hits a
//! step the recorder cannot show, a skipped step, a timeout, a panic, an error toast or private
//! text leaves the previous video untouched and reports a finding `video` in
//! `target/tutorial-videos/report.md`. Privacy-checked failure screenshots are saved under the
//! output folder's `failures` directory when the app is open. The video is written as
//! `<id>.mp4.part` and renamed when it is complete.
//!
//! Privacy: no frame may show a path with the user's name; every step's visible labels, values
//! and placeholders are scanned for the user's name and for profile path fragments, and a hit
//! abandons the video.
//!
//! Look: the software renderer draws with the system fonts and draws no window frame, so a video
//! shows the window content only. The window is 1920 x 1080 logical pixels at scale 2.0.

mod compare_capture;
mod config;
mod crystal_axis_targets;
mod debug_driver;
mod debug_images;
mod debug_log;
mod debug_model;
mod debug_overlays;
mod debug_rec;
mod director;
mod event_extras;
mod failure_capture;
mod jobs_targets;
mod numbering;
mod pacing;
mod perform_actions;
mod perform_inputs;
mod perform_routes;
mod popup_geometry;
mod routes;
mod script_lookup;
mod scripts;
mod section_folders;
mod stage;
mod startup;
#[path = "../ui_harness/support/mod.rs"]
mod support;
mod targets;

use config::Config;
use pleochroma_editor::guide::GuideCatalog;
use std::{cell::RefCell, process::ExitCode, rc::Rc};
use support::{Ctx, HarnessApp, SettingsSeed, profile::print_shutdown_summary};
use uiprobe::{
    Harness, Jobs,
    backend::Renderer,
    profile::Profile as HarnessProfile,
    report::{Finding, Format},
    take_panics,
};

/// The app, shared by the scenario closures and the teardown (one UI thread).
type SharedApp = Rc<RefCell<HarnessApp>>;

fn main() -> ExitCode {
    // Read before the profile moves the working directory and redirects the user variables.
    let config = Rc::new(Config::from_env());
    if !config.debug
        && let Err(error) = numbering::write_playlist(&config.out_dir)
    {
        eprintln!("tutorial-videos: playlist.md not written: {error}");
    }
    let harness = Harness::new()
        .profile(
            HarnessProfile::isolated()
                .name_prefix("pleochroma-tutorial-videos")
                .guard_paths(|| {
                    vec![
                        pleochroma_cut::harness_support::settings_path(),
                        pleochroma_cut::harness_support::database_path(),
                    ]
                }),
        )
        .renderer(Renderer::Software)
        // Worker processes while cores are idle (each its own profile); the tutorials are
        // independent. `UIPROBE_JOBS=serial` runs them one after another.
        .jobs(Jobs::Auto)
        .report_dir(uiprobe::target_dir!().join("tutorial-videos"))
        .formats([Format::Markdown]);

    let app: SharedApp = Rc::new(RefCell::new(HarnessApp::empty()));
    let mut harness = harness;
    let ids: Vec<String> = GuideCatalog::new()
        .all()
        .iter()
        .map(|guide| guide.id.clone())
        .collect();
    for id in ids {
        harness = register(harness, &app, &config, id);
    }
    let teardown_app = Rc::clone(&app);
    harness
        .teardown(move || {
            teardown_app.borrow_mut().shutdown();
            print_shutdown_summary();
        })
        .run()
}

/// Registers one tutorial as a scenario.
fn register(harness: Harness, app: &SharedApp, config: &Rc<Config>, id: String) -> Harness {
    let app = Rc::clone(app);
    let config = Rc::clone(config);
    let name = format!("video:{id}");
    harness.scenario(name, move |cx| {
        let mut guard = app.borrow_mut();
        let _ = take_panics();
        // The toast readers serve the crate's own end-of-scenario `check_health`.
        let reader = Rc::clone(&app);
        cx.on_error_notice("MainWindow", move || {
            reader.try_borrow().ok().and_then(|a| a.error_notice())
        });
        let clearer = Rc::clone(&app);
        cx.clear_notice_with(move || {
            if let Ok(a) = clearer.try_borrow() {
                a.clear_notice();
            }
        });
        let (report, profile) = cx.parts();
        let mut ctx = Ctx {
            app: &mut guard,
            profile,
            report,
            seed: SettingsSeed::ExistingInstall,
        };
        let outcome = director::film(&mut ctx, &config, &id);
        // A refused configuration or failed launch can return before a window exists.
        // Keep that original finding instead of panicking while collecting diagnostics.
        if ctx.app.is_open() {
            if outcome.is_err()
                && let Err(reason) = failure_capture::save_failure(ctx.app.ui(), &config, &id)
            {
                eprintln!("tutorial-videos: {id}: {reason}");
            }
            ctx.check_health(&id);
        }
        if config.debug {
            debug_log::finish(
                ctx.app.is_open().then(|| ctx.app.ui()),
                &config,
                &id,
                &outcome,
            );
        }
        guard.clear_notice();
        outcome.map_err(|reason| {
            reason.strip_prefix("video-stage: ").map_or_else(
                || Finding::simple("video", format!("{id}: {reason}")),
                |detail| Finding::simple("video-stage", format!("{id}: {detail}")),
            )
        })
    })
}
