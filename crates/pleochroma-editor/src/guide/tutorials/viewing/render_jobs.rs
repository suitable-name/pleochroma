//! Render jobs: saving a picture as a job from the export dialog, the Render Jobs window and its
//! queue, Start, Pause and Resume, and the script that renders the jobs on another computer.
//!
//! Both lessons wait for events the desktop raises from `gui/render_jobs` (see
//! `tutorials::viewing_events`). The Render Jobs window is drawn beside the guide panel, so
//! every step of the second lesson can show its own text while the window is open.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{VIEWPORT_TABS, event};
use crate::guide::{
    Guide, GuideCategory, GuideStep, StartingState,
    tutorials::viewing_events::{
        LIVE_RENDER_OPENED, RENDER_EXPORT_OPENED, RENDER_JOB_ADDED, RENDER_JOB_RESUMED,
        RENDER_JOBS_OPENED, RENDER_QUEUE_PAUSED, RENDER_QUEUE_STARTED, RENDER_SCRIPT_EXPORTED,
    },
};

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![render_jobs(), render_jobs_queue()]
}

/// The "open Live Render" step: the export dialog belongs to Live Render.
fn open_live_render() -> GuideStep {
    GuideStep::new(
        "Open Live Render",
        "The export dialog sits in the Live Render toolbar, so the picture you queue is the one you see there.",
    )
    .actions(["Click Live Render, the pill to the left of Edit, above the picture."])
    .check("The Live Render view opens with its toolbar and the Export button at the right. It starts on the flat Solid picture; Path-Traced above the picture shows the traced one.")
    .why("A job renders the finished stone under the lighting and material you see in Live Render.")
    .goal(event(LIVE_RENDER_OPENED), "The Live Render view")
    .highlight("live_render_tab")
    .allow(VIEWPORT_TABS)
}

/// The "open the Render Jobs window" step both lessons share: the header's Jobs chip.
fn open_jobs_window() -> GuideStep {
    GuideStep::new(
        "Open the Render Jobs window",
        "Every job you add waits in the Render Jobs window. It opens over the picture, beside this panel.",
    )
    .actions([
        "Close the export dialog with its Close button if it is still open.",
        "Click the Jobs button at the top of the window. It reads Jobs: 1 waiting. File, Render Jobs... opens the same window.",
    ])
    .check("A window titled Render Jobs lists your picture with the state Queued.")
    .why("The Jobs button shows only while a job is waiting or running. The list is kept in your library, so the jobs are still there the next time you start the program.")
    .goal(event(RENDER_JOBS_OPENED), "The Render Jobs window")
    .highlight("render_jobs_chip")
    .allow(VIEWPORT_TABS)
}

fn render_jobs() -> Guide {
    Guide::new(
        "output-render-jobs",
        "Render Jobs",
        "Save a picture as a render job from the export dialog, and find it in the Render Jobs window.",
        GuideCategory::Rendering,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Render now or render later",
            "A render job is a picture that waits in a list until you start it, so you can keep working, or leave the program rendering overnight.",
        )
        .actions([
            "Start Export in the export dialog renders at once. Add to Queue next to it saves the same settings as a job and renders nothing yet.",
            "A job keeps a frozen copy of everything it needs: the finished stone, its material and color, the lighting, the camera, the size and the samples. Changing the design afterwards does not change a job that was added before.",
            "The same button sits in the Export Tilt Video section of the Tilt Performance dialog.",
        ])
        .why("One job renders at a time. The queue never starts by itself, not even when the program starts.")
        .allow(VIEWPORT_TABS),
    )
    .step(open_live_render())
    .step(
        GuideStep::new(
            "Open the export dialog",
            "The export dialog holds every setting of a picture: the folder and name, the size and the samples.",
        )
        .actions(["Click Export in the toolbar above the picture."])
        .check("The Export High-Resolution Render dialog opens, with Close, Add to Queue and Start Export at the bottom.")
        .why("Set it up as you would for an export: the size, the sample count and, in the Advanced interface, the color space and where to render. A job freezes them.")
        .goal(event(RENDER_EXPORT_OPENED), "The export dialog")
        .highlight("render_export_button")
        .allow(VIEWPORT_TABS),
    )
    .step(
        GuideStep::new(
            "Add it to the queue",
            "Add to Queue saves the picture as a job instead of rendering it.",
        )
        .actions([
            "Keep the settings as they are, or change the size and the samples.",
            "Click Add to Queue, not Start Export.",
        ])
        .check("A message says Added 1 render job to the queue, and the Jobs button appears at the top of the window.")
        .why("The file name is worked out now, from the folder and the name template, and a date or time in the template is the time of Add to Queue. If a picture with that name exists when the job runs, (2) is added to the name. With saved lighting ticked under Also Render With Saved Lighting, each picture becomes a job of its own.")
        .goal(event(RENDER_JOB_ADDED), "A render job added")
        .highlight("add_to_queue_button")
        .allow(VIEWPORT_TABS),
    )
    .step(open_jobs_window())
    .step(
        GuideStep::new(
            "Read the queue",
            "Each row of the list is one job.",
        )
        .actions([
            "A row shows the job's number, its name, its state and what it renders, then its progress. The states are Queued, Running, Paused, Done, Failed and Cancelled.",
            "Only the buttons that make sense for the job's state are shown: Run Next, Up, Down, Pause, Resume, Restart, Cancel, Show and Delete.",
            "Delete and Clear Finished remove only the list entry. The pictures and videos they wrote stay on disk.",
            "Close, or Esc, closes the window. A job that is rendering goes on.",
        ])
        .why("The next lesson starts the queue, pauses it and exports it as a script. Every control is unlocked again."),
    )
}

fn render_jobs_queue() -> Guide {
    Guide::new(
        "output-render-jobs-queue",
        "Start, Pause and Resume the Queue",
        "Start the queue, pause it early, put the job back in line, and export the jobs as a script.",
        GuideCategory::Rendering,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Queue a picture",
            "The queue needs a job. The lesson before this one explains each setting; here only the way there matters.",
        )
        .actions([
            "Click Live Render, then Export in its toolbar.",
            "Click Add to Queue.",
            "Close the export dialog.",
        ])
        .check("A message says Added 1 render job to the queue.")
        .why("A picture is rendered as a whole, so a small size and a low sample count in the export dialog make a job that starts and stops quickly while you try the buttons.")
        .goal(event(RENDER_JOB_ADDED), "A render job added")
        .highlight("live_render_tab")
        .allow(VIEWPORT_TABS),
    )
    .step(open_jobs_window())
    .step(
        GuideStep::new(
            "Start the queue",
            "Start Queue renders the waiting jobs from the top of the list down, one at a time.",
        )
        .actions(["Click Start Queue."])
        .check("The job's state changes to Running, a progress bar fills, and the button now reads Pause Queue.")
        .why("Stop After This Job, next to it while a job renders, lets the running job finish and then stops the queue. While a picture or a video from its own dialog is rendering, Start Queue is dimmed: one export runs at a time.")
        .goal(event(RENDER_QUEUE_STARTED), "The queue started")
        .highlight("start_queue_button")
        .allow(VIEWPORT_TABS),
    )
    .step(
        GuideStep::new(
            "Pause the queue",
            "Pause Queue stops the job that is rendering and keeps the rest waiting.",
        )
        .actions(["Click Pause Queue."])
        .check("The job's state changes to Paused, and the button reads Start Queue again.")
        .why("The queue is stopped, so nothing starts by itself. A single job also has its own Pause button, which stops only that job and lets the queue go on with the next one.")
        .goal(event(RENDER_QUEUE_PAUSED), "The queue paused")
        .highlight("pause_queue_button")
        .allow(VIEWPORT_TABS),
    )
    .step(
        GuideStep::new(
            "Resume the job",
            "A paused job waits until you put it back in line.",
        )
        .actions(["Click Resume on the paused job's row."])
        .check("The job's state changes back to Queued.")
        .why("Resume also brings back a job that failed. Restart is different: it renders the job again from the beginning.")
        .goal(event(RENDER_JOB_RESUMED), "The job put back in line")
        .highlight("job_resume_button")
        .allow(VIEWPORT_TABS),
    )
    .step(
        GuideStep::new(
            "What pausing keeps",
            "Pausing works frame by frame.",
        )
        .actions([
            "A still picture starts again from the beginning when you resume it: it is rendered as a whole, so nothing is kept.",
            "A tilt video keeps every frame it finished. Resume continues with the first missing frame, in the same folder, and the video file is made once all frames are there.",
            "A paused video keeps its frames on disk until it is finished or you delete the folder. At large sizes that can be many gigabytes.",
            "If you close the program while a job renders, it asks first. If you go on, the job waits as Paused and the queue is stopped.",
        ])
        .why("Restart throws the finished frames of a video away and starts at frame one.")
        .allow(VIEWPORT_TABS),
    )
    .step(
        GuideStep::new(
            "Export a script",
            "Export Script writes the jobs that are not finished to a new folder, so another computer can render them.",
        )
        .actions([
            "Click Export Script... at the top of the window. A section opens at the foot of the window.",
            "Choose the Script Type and what to Render On, or keep Both and This Computer.",
            "Click Choose Folder and Export, and pick a folder.",
        ])
        .check("A message says Exported 1 job and the run-render-jobs script, and the section closes.")
        .why("The program makes a new folder named render-jobs- with the date and time, so nothing is ever overwritten. The button is dimmed when no job is unfinished.")
        .goal(event(RENDER_SCRIPT_EXPORTED), "A script exported")
        .highlight("export_script_button")
        .allow(VIEWPORT_TABS),
    )
    .step(
        GuideStep::new(
            "Run the script elsewhere",
            "The new folder holds everything another computer needs.",
        )
        .actions([
            "The folder holds run-render-jobs.ps1, run-render-jobs.sh, a jobs folder with one job file for each job, and a renders folder if you chose to put the finished files there.",
            "Copy the whole folder to the computer that should render. On Windows run powershell -ExecutionPolicy Bypass -File run-render-jobs.ps1. On macOS and Linux run sh run-render-jobs.sh.",
            "The script runs pleochroma-cli once for each job. If the tool is not on the PATH, set the environment variable PLEOCHROMA_CLI to its full path first.",
        ])
        .why("A tilt video that stopped part way continues where it stopped when you run the script again. Every control is unlocked again."),
    )
}
