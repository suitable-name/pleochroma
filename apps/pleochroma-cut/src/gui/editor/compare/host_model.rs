/// Empties `handle`'s surface: no session, no images, no optical figures, `status` on the
/// status line.
pub(super) fn reset_surface(handle: &SurfaceHandle, status: &str) {
    handle.with_model(|model| {
        model.set_is_open(false);
        model.set_embedded_open(false);
        model.set_optimizer_inline_open(false);
        model.set_after_waiting(false);
        model.set_before_unavailable(false);
        model.set_can_keep(false);
        model.set_show_keep_discard(false);
        model.set_before_image(Image::default());
        model.set_after_image(Image::default());
        model.set_overlay_image(Image::default());
        model.set_status(status.into());
        clear_strip(&model);
    });
}

/// Pushes `strip` into the metrics strip's properties of `model`.
fn push_strip(model: &CompareModel<'_>, strip: &StripText) {
    let rows: Vec<CompareMetricRow> = strip
        .rows
        .iter()
        .map(|row| CompareMetricRow {
            label: row.label.into(),
            before: row.before.as_str().into(),
            after: row.after.as_str().into(),
            change: row.change.as_str().into(),
            verdict: row.tone.word().into(),
            tone: row.tone.code(),
        })
        .collect();
    model.set_metric_rows(ModelRc::new(VecModel::from(rows)));
    model.set_metric_summary(strip.summary.as_str().into());
    model.set_metric_note(strip.note.as_str().into());
    model.set_tilt_available(strip.tilt_available);
    model.set_tilt_running(strip.tilt_running);
    model.set_tilt_done(strip.tilt_done);
    model.set_tilt_progress(strip.tilt_progress);
}

/// Empties the metrics strip of `model`.
pub(super) fn clear_strip(model: &CompareModel<'_>) {
    push_strip(model, &StripText::default());
}
