//! Walks of the lessons in `viewing/render_jobs.rs`.

use super::{Sim, event, guide_named, read, viewing_events as events, walk};

#[test]
fn queueing_a_picture_walks_from_live_render_to_the_jobs_window() {
    walk(
        "output-render-jobs",
        vec![
            read(),
            event(events::LIVE_RENDER_OPENED),
            event(events::RENDER_EXPORT_OPENED),
            event(events::RENDER_JOB_ADDED),
            event(events::RENDER_JOBS_OPENED),
            read(),
        ],
    );
}

#[test]
fn the_queue_lesson_walks_start_pause_resume_and_the_script() {
    walk(
        "output-render-jobs-queue",
        vec![
            event(events::RENDER_JOB_ADDED),
            event(events::RENDER_JOBS_OPENED),
            event(events::RENDER_QUEUE_STARTED),
            event(events::RENDER_QUEUE_PAUSED),
            event(events::RENDER_JOB_RESUMED),
            read(),
            event(events::RENDER_SCRIPT_EXPORTED),
            read(),
        ],
    );
}

#[test]
fn a_render_jobs_step_is_not_met_by_another_render_jobs_event() {
    let all = [
        events::RENDER_EXPORT_OPENED,
        events::RENDER_JOB_ADDED,
        events::RENDER_JOBS_OPENED,
        events::RENDER_QUEUE_STARTED,
        events::RENDER_QUEUE_PAUSED,
        events::RENDER_JOB_RESUMED,
        events::RENDER_SCRIPT_EXPORTED,
    ];
    let mut checked = 0;
    for id in ["output-render-jobs", "output-render-jobs-queue"] {
        let guide = guide_named(id);
        for step in &guide.steps {
            let own: Vec<&str> = step
                .goal
                .events()
                .into_iter()
                .filter(|name| all.contains(name))
                .collect();
            if own.is_empty() {
                continue;
            }
            for other in all.iter().filter(|name| !own.contains(*name)) {
                let mut sim = Sim::start(&guide.starting_state);
                sim.fire(other);
                assert!(
                    !sim.met(&step.goal),
                    "{id}, {:?}: {other} must not finish the step",
                    step.title
                );
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 9, "the steps that wait for a render jobs event");
}
