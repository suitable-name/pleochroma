//! Read-only observations from the real Solid and Diagram hover callbacks.

use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy)]
pub enum HoverViewport {
    Solid,
    Diagram,
}

#[derive(Debug, Clone)]
pub struct HoverObservation {
    pub sequence: u64,
    pub logical_x: f32,
    pub logical_y: f32,
    pub scale: f32,
    pub pick_dimensions: Option<(u32, u32)>,
    pub facet_id: Option<u32>,
    pub label_count: Option<usize>,
    pub resolved_label: Option<String>,
}

/// What the Solid sink knew about one frame it was handed.
#[derive(Debug, Clone, Copy)]
pub struct FrameFacts {
    pub generation: u64,
    /// `true` for a `Planned` frame, `false` for a camera/overlay redraw (`Reproject`,
    /// `UpdateFacetOverlay`).
    pub planned: bool,
    /// Whether the frame carried solved masts.
    pub has_masts: bool,
    /// Entries in the frame's hover-label table.
    pub hover_len: usize,
    /// Planes of the stone the frame drew.
    pub plane_count: usize,
    /// Whether the frame carried a Diagram image (it was drawn at view mode 3 from a closed
    /// stone).
    pub has_diagram: bool,
}

/// One step of the plan -> render -> sink path the label table travels.
#[derive(Debug, Clone, Copy)]
pub enum FrameEvent {
    /// A `Planned` request was handed to the render gate.
    PlannedSubmitted,
    /// A `Planned` request still waiting in the render gate was replaced by a
    /// `Reproject`/overlay request before the render worker took it: it never renders.
    PlannedOverwritten,
    /// The plan worker discarded a job below the generation floor.
    PlanDroppedByFloor,
    /// The sink dropped a planned frame as older than the one already shown.
    PlannedSuperseded(FrameFacts),
    /// The sink applied a frame.
    Applied(FrameFacts),
}

/// Counts of the events above, cumulative for the process (not cleared with the hover
/// observations: the frames that matter happen before the first hover).
#[derive(Debug, Clone, Default)]
pub struct FrameDiagnostics {
    pub planned_submitted: u64,
    pub planned_overwritten: u64,
    pub plans_dropped_by_floor: u64,
    pub planned_superseded: u64,
    pub planned_applied: u64,
    pub planned_without_masts: u64,
    pub other_applied: u64,
    /// Non-planned frames that arrived with an empty label table right after a frame
    /// that had labels.
    pub label_resets: u64,
    pub last_planned: Option<FrameFacts>,
    pub last_other: Option<FrameFacts>,
    pub last_reset: Option<FrameFacts>,
    last_hover_len: usize,
}

impl FrameDiagnostics {
    #[must_use]
    pub fn describe(&self) -> String {
        let facts = |label: &str, facts: Option<FrameFacts>| {
            facts.map_or_else(
                || format!("{label}: none"),
                |f| {
                    format!(
                        "{label}: generation {}, labels {}, planes {}, masts {}, diagram {}",
                        f.generation, f.hover_len, f.plane_count, f.has_masts, f.has_diagram
                    )
                },
            )
        };
        format!(
            "; frames: planned submitted {} (replaced in the render gate by a non-planned request {}, dropped by the generation floor {}, superseded in the sink {}), planned applied {} ({} without masts), non-planned applied {}, label resets by a non-planned frame {}; {}; {}; {}",
            self.planned_submitted,
            self.planned_overwritten,
            self.plans_dropped_by_floor,
            self.planned_superseded,
            self.planned_applied,
            self.planned_without_masts,
            self.other_applied,
            self.label_resets,
            facts("last planned", self.last_planned),
            facts("last non-planned", self.last_other),
            facts("last reset", self.last_reset),
        )
    }
}

static FRAMES: OnceLock<Mutex<FrameDiagnostics>> = OnceLock::new();

fn frames() -> &'static Mutex<FrameDiagnostics> {
    FRAMES.get_or_init(|| Mutex::new(FrameDiagnostics::default()))
}

/// Records one step of the label table's path; see [`FrameEvent`].
pub fn record_frame_event(event: FrameEvent) {
    let mut frames = frames()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match event {
        FrameEvent::PlannedSubmitted => frames.planned_submitted += 1,
        FrameEvent::PlannedOverwritten => frames.planned_overwritten += 1,
        FrameEvent::PlanDroppedByFloor => frames.plans_dropped_by_floor += 1,
        FrameEvent::PlannedSuperseded(_) => frames.planned_superseded += 1,
        FrameEvent::Applied(facts) => {
            if facts.planned {
                frames.planned_applied += 1;
                if !facts.has_masts {
                    frames.planned_without_masts += 1;
                }
                frames.last_planned = Some(facts);
            } else {
                frames.other_applied += 1;
                frames.last_other = Some(facts);
                if frames.last_hover_len > 0 && facts.hover_len == 0 {
                    frames.label_resets += 1;
                    frames.last_reset = Some(facts);
                }
            }
            frames.last_hover_len = facts.hover_len;
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HoverDiagnosticSnapshot {
    pub solid_invocations: u64,
    pub diagram_invocations: u64,
    pub solid: Option<HoverObservation>,
    pub diagram: Option<HoverObservation>,
    pub frames: FrameDiagnostics,
}

impl HoverDiagnosticSnapshot {
    /// One line that tells why a hover sweep found nothing: whether the callback ran at all,
    /// whether a pick buffer and label table existed, and where the last probe landed.
    #[must_use]
    pub fn describe_solid(&self) -> String {
        let Some(last) = &self.solid else {
            return format!(
                "the Solid hover callback never ran (invocations={}, diagram={}): pointer moves did not reach the viewport",
                self.solid_invocations, self.diagram_invocations
            );
        };
        format!(
            "Solid hover callback ran {} times; last at ({:.0}, {:.0}) scale {}: pick buffer {}, labels {}, facet {}, label {:?}{}{}",
            self.solid_invocations,
            last.logical_x,
            last.logical_y,
            last.scale,
            last.pick_dimensions
                .map_or_else(|| "none".to_owned(), |(w, h)| format!("{w}x{h}")),
            last.label_count
                .map_or_else(|| "none".to_owned(), |count| count.to_string()),
            last.facet_id
                .map_or_else(|| "none".to_owned(), |id| id.to_string()),
            last.resolved_label,
            table_mismatch(last),
            self.frames.describe(),
        )
    }
}

impl HoverDiagnosticSnapshot {
    /// The Diagram counterpart of [`Self::describe_solid`]: whether the Diagram's hover
    /// callback ran, against which pick buffer and label table, and how many frames of the
    /// process carried a Diagram at all. A sweep that finds no facet in the Diagram says here
    /// whether the pick table was missing (no Diagram frame ever landed), empty, or simply
    /// never under the pointer.
    #[must_use]
    pub fn describe_diagram(&self) -> String {
        let Some(last) = &self.diagram else {
            return format!(
                "; the Diagram hover callback never ran (invocations={})",
                self.diagram_invocations
            );
        };
        format!(
            "; Diagram hover callback ran {} times; last at ({:.0}, {:.0}) scale {}: pick buffer {}, labels {}, facet {}, label {:?}{}",
            self.diagram_invocations,
            last.logical_x,
            last.logical_y,
            last.scale,
            last.pick_dimensions
                .map_or_else(|| "none".to_owned(), |(w, h)| format!("{w}x{h}")),
            last.label_count
                .map_or_else(|| "none".to_owned(), |count| count.to_string()),
            last.facet_id
                .map_or_else(|| "none".to_owned(), |id| id.to_string()),
            last.resolved_label,
            table_mismatch(last),
        )
    }
}

/// Names a disagreement between the pick buffer and the label table of one observation:
/// the facet id the pick buffer returned (the highest id the probe saw) against the number
/// of labels the table holds. An empty table next to a pick buffer means the pick came from
/// a camera/stone redraw (`RedrawRequest::Reproject`) of a stone no planned frame has
/// labelled yet; a non-empty table shorter than the id means the two describe different
/// stones. Empty when they agree or when either side was not observed.
fn table_mismatch(observation: &HoverObservation) -> String {
    let (Some(facet), Some(labels)) = (observation.facet_id, observation.label_count) else {
        return String::new();
    };
    let label_count = u64::try_from(labels).unwrap_or(u64::MAX);
    if label_count == 0 {
        format!(
            "; MISMATCH: pick id {facet} but the label table is empty (the stone on screen was redrawn without a planned frame, so no facet has a name)"
        )
    } else if u64::from(facet) >= label_count {
        format!(
            "; MISMATCH: pick id {facet} is outside the {labels}-entry label table (pick buffer and labels describe different stones)"
        )
    } else {
        String::new()
    }
}

static OBSERVATIONS: OnceLock<Mutex<HoverDiagnosticSnapshot>> = OnceLock::new();

fn observations() -> &'static Mutex<HoverDiagnosticSnapshot> {
    OBSERVATIONS.get_or_init(|| Mutex::new(HoverDiagnosticSnapshot::default()))
}

/// Clears the hover observations. The frame counters ([`FrameDiagnostics`]) are kept: they
/// describe frames applied before the first hover.
pub fn clear_hover_diagnostics() {
    *observations()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = HoverDiagnosticSnapshot::default();
}

pub fn hover_diagnostic_snapshot() -> HoverDiagnosticSnapshot {
    let mut snapshot = observations()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    snapshot.frames = frames()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    snapshot
}

#[expect(
    clippy::too_many_arguments,
    reason = "harness-only probe; the arguments are the observation's fields"
)]
pub fn record_hover_diagnostic(
    viewport: HoverViewport,
    logical_x: f32,
    logical_y: f32,
    scale: f32,
    pick_dimensions: Option<(u32, u32)>,
    facet_id: Option<u32>,
    label_count: Option<usize>,
    resolved_label: Option<String>,
) {
    let mut snapshot = observations()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (sequence, slot) = match viewport {
        HoverViewport::Solid => {
            snapshot.solid_invocations += 1;
            (snapshot.solid_invocations, &mut snapshot.solid)
        }
        HoverViewport::Diagram => {
            snapshot.diagram_invocations += 1;
            (snapshot.diagram_invocations, &mut snapshot.diagram)
        }
    };
    *slot = Some(HoverObservation {
        sequence,
        logical_x,
        logical_y,
        scale,
        pick_dimensions,
        facet_id,
        label_count,
        resolved_label,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(facet_id: Option<u32>, label_count: Option<usize>) -> HoverObservation {
        HoverObservation {
            sequence: 1,
            logical_x: 0.0,
            logical_y: 0.0,
            scale: 2.0,
            pick_dimensions: Some((10, 10)),
            facet_id,
            label_count,
            resolved_label: Some(String::new()),
        }
    }

    #[test]
    fn an_empty_table_next_to_a_pick_id_is_named() {
        let text = table_mismatch(&observation(Some(130), Some(0)));
        assert!(
            text.contains("MISMATCH") && text.contains("empty"),
            "{text}"
        );
    }

    #[test]
    fn an_id_past_the_table_is_named() {
        let text = table_mismatch(&observation(Some(40), Some(33)));
        assert!(text.contains("33-entry"), "{text}");
    }

    #[test]
    fn agreeing_tables_and_missing_sides_say_nothing() {
        assert_eq!(table_mismatch(&observation(Some(5), Some(33))).len(), 0);
        assert_eq!(table_mismatch(&observation(None, Some(0))).len(), 0);
        assert_eq!(table_mismatch(&observation(Some(5), None)).len(), 0);
    }
}
