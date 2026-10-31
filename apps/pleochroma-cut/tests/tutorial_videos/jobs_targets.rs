//! The outline targets of the render jobs lessons (`output-render-jobs`,
//! `output-render-jobs-queue`), in the shape of `targets::TABLE`. `targets::resolve` reads this
//! table after the main one.

use crate::targets::{Entry, Query};

/// One row for each render jobs `highlight_target`. The `source` column says where the app draws
/// the outline.
pub const TABLE: &[Entry] = &[
    Entry {
        name: "render_export_button",
        queries: &[Query::Label("Export")],
        source: "components/gem_viewport/toolbar_actions.slint (Export)",
    },
    Entry {
        name: "add_to_queue_button",
        queries: &[Query::Label("Add to Queue")],
        source: "components/export_dialog.slint (Add to Queue)",
    },
    Entry {
        name: "render_jobs_chip",
        queries: &[Query::LabelStart("Jobs"), Query::LabelStart("Rendering")],
        source: "components/render_jobs/jobs_chip.slint",
    },
    Entry {
        name: "start_queue_button",
        queries: &[Query::Label("Start Queue")],
        source: "components/render_jobs/jobs_panel.slint (Start Queue)",
    },
    Entry {
        name: "pause_queue_button",
        queries: &[Query::Label("Pause Queue")],
        source: "components/render_jobs/jobs_panel.slint (Pause Queue)",
    },
    Entry {
        name: "export_script_button",
        queries: &[Query::Label("Export Script...")],
        source: "components/render_jobs/jobs_panel.slint (Export Script...)",
    },
    Entry {
        name: "job_resume_button",
        queries: &[Query::Label("Resume")],
        source: "components/render_jobs/job_row.slint (Resume)",
    },
];
