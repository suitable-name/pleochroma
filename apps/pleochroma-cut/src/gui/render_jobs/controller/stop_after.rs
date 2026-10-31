//! Stop After This Job: the queue finishes the job that renders and then starts nothing more.

use super::JobController;

impl JobController {
    /// Stop After This Job, or its undo: while the queue runs a job, toggles whether the
    /// queue stops once that job ends (Done, Failed or Cancelled alike). The job itself is
    /// not interrupted, unlike Pause Queue.
    pub fn toggle_stop_after_current(&self) {
        let now_set = {
            let mut state = self.state.borrow_mut();
            if !state.queue_running || state.current.is_none() {
                return;
            }
            state.stop_after_current = !state.stop_after_current;
            state.stop_after_current
        };
        self.toast(
            if now_set {
                "The queue stops when this job is done."
            } else {
                "The queue goes on after this job."
            },
            "info",
        );
        self.refresh();
    }
}
