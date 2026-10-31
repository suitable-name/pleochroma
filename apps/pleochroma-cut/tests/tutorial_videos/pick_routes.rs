//! Filmed clicks on a facet in the stone's picture: the pointer sweeps over the picture until the
//! hover panel names the facet that is wanted, and that spot is clicked, the way a learner points
//! at the stone and reads what is under the pointer before clicking.

use crate::{
    debug_log::{debug_check, debug_check_with},
    director::Film,
    pacing,
    targets::Spot,
};
use pleochroma_cut::{EditorModel, ManipulateModel, SolidPreviewModel};
use pleochroma_editor::guide::CameraFocus;
use slint::{ComponentHandle, Model};

/// The tier the drag handle and picking lessons start from.
const CROWN_MAIN: &str = "Crown Main";

impl Film<'_> {
    /// Hovers a Crown Main facet without clicking and keeps the identifying HUD readable.
    pub(crate) fn hover_crown_facet(&mut self) -> Result<(), String> {
        self.wait_for_pick_surface()?;
        let editor = self.ui.global::<EditorModel>();
        let before = editor.get_selected_tier_index();
        self.sweep_for_facet("Crown Main facet", |hover| {
            hover.contains(CROWN_MAIN) && hover.contains("°") && hover.contains("index")
        })?;
        let preview = self.ui.global::<SolidPreviewModel>();
        if preview.get_view_mode() == 3 {
            return Err(
                "Crown Main hover lesson requires the Solid, not Diagram, hover HUD".into(),
            );
        }
        let hover = preview.get_hover_text().to_string();
        let complete = hover.contains(CROWN_MAIN) && hover.contains("°") && hover.contains("index");
        debug_check_with("hover HUD names the facet", &CROWN_MAIN, &hover, complete);
        if !hover.contains(CROWN_MAIN) || !hover.contains("°") || !hover.contains("index") {
            return Err(format!("Crown Main hover HUD is incomplete: {hover:?}"));
        }
        if self.ui.get_active_tab() != 0 || self.ui.global::<ManipulateModel>().get_dragging() {
            return Err(
                "Crown Main hover HUD is not visible outside the active Solid viewport".into(),
            );
        }
        let selected = editor.get_selected_tier_index();
        debug_check("selected tier unchanged by the hover", &before, &selected);
        if editor.get_selected_tier_index() != before {
            return Err(format!(
                "hovering Crown Main changed the selected tier from {before} to {}",
                editor.get_selected_tier_index()
            ));
        }
        self.hold(std::time::Duration::from_secs(2))?;
        if editor.get_selected_tier_index() != before {
            return Err("Crown Main hover changed the tier selection during the hold".into());
        }
        Ok(())
    }

    /// Points at the stone until a facet of Crown Main is under the pointer, clicks it and
    /// requires the three handles to appear.
    pub(crate) fn pick_crown_facet(&mut self) -> Result<(), String> {
        self.wait_for_pick_surface()?;
        let editor = self.ui.global::<EditorModel>();
        let crown_row = (0..editor.get_tiers().row_count())
            .find(|&index| {
                editor
                    .get_tiers()
                    .row_data(index)
                    .is_some_and(|row| row.name == CROWN_MAIN)
            })
            .ok_or_else(|| format!("{CROWN_MAIN} row is missing before the facet pick"))?;
        let crown_row =
            i32::try_from(crown_row).map_err(|_| "Crown Main row index is too large")?;
        let spot = self.sweep_for_facet(&format!("{CROWN_MAIN} facet"), |hover| {
            hover.contains(CROWN_MAIN)
        })?;
        self.click_and_invoke(&spot)?;
        let manipulate = self.ui.global::<ManipulateModel>();
        let picked = pacing::wait_real(pacing::START_TIMEOUT, || {
            editor.get_selected_tier_index() == crown_row && manipulate.get_handles_visible()
        });
        debug_check(
            "row selected by the pick",
            &crown_row,
            &editor.get_selected_tier_index(),
        );
        debug_check(
            "handles visible after the pick",
            &true,
            &manipulate.get_handles_visible(),
        );
        if !picked {
            return Err(format!(
                "picked {CROWN_MAIN} did not select row {crown_row} and show handles: selected={}, handles_visible={} ({}), hover={:?}",
                editor.get_selected_tier_index(),
                manipulate.get_handles_visible(),
                pleochroma_cut::harness_support::handles_hidden_reason(self.ui),
                self.ui.global::<SolidPreviewModel>().get_hover_text()
            ));
        }
        Ok(())
    }

    /// Points at the stone until a facet of a tier other than `current` is under the pointer,
    /// clicks it and requires the selection to move to another tier.
    ///
    /// With no facet under the pointer the hover panel keeps reading the facet that was clicked
    /// last, so a text that names `current` is never taken for another tier.
    pub(crate) fn pick_other_tier_facet(&mut self, current: &str) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        let before = editor.get_selected_tier_index();
        let spot = self
            .sweep_for_facet(&format!("facet of a tier other than {current}"), |hover| {
                !hover.is_empty() && !hover.contains(current)
            })?;
        self.click_and_invoke(&spot)?;
        let changed = pacing::wait_real(pacing::START_TIMEOUT, || {
            editor.get_selected_tier_index() >= 0 && editor.get_selected_tier_index() != before
        });
        let after = editor.get_selected_tier_index();
        if !changed {
            return Err(format!(
                "clicking a facet of a tier other than {current} left selection at row {after} (was {before}); hover={:?}",
                self.ui.global::<SolidPreviewModel>().get_hover_text()
            ));
        }
        Ok(())
    }

    /// Moves the pointer over the stone's picture on a grid, from the camera poses that show the
    /// upper side, until the hover text satisfies `wanted`; the spot to click then. `what` names
    /// the facet for the error.
    fn sweep_for_facet(
        &mut self,
        what: &str,
        wanted: impl Fn(&str) -> bool,
    ) -> Result<Spot, String> {
        pleochroma_cut::harness_support::clear_hover_diagnostics();
        let preview = self.ui.global::<SolidPreviewModel>();
        let diagram = preview.get_view_mode() == 3;
        let focuses: &[CameraFocus] = if diagram {
            &[CameraFocus::Keep]
        } else {
            &[CameraFocus::Crown, CameraFocus::Whole, CameraFocus::Top]
        };
        for &focus in focuses {
            if !diagram {
                self.frame(focus, true, true)?;
            }
            self.wait_for_pick_surface()?;
            let viewport = self.viewport()?;
            let view = self.view();
            let obstructions = crate::targets::viewport_obstructions(&view);
            let covered =
                |candidate: &Spot| obstructions.iter().any(|spot| intersects(spot, candidate));
            if diagram {
                // The Diagram image is drawn on the solid worker: without a recorder no real
                // time passes between steps, so wait for it in real time.
                let ready = crate::pacing::wait_real(std::time::Duration::from_secs(10), || {
                    preview.get_has_diagram()
                });
                crate::debug_log::debug_check(
                    "Diagram image shown before the sweep",
                    &true,
                    &ready,
                );
            } else {
                self.wait_for_pick_buffer(&viewport, &covered)?;
            }
            // A coarse grid first (49 points), the fine one (361 points) only when it misses.
            for divisions in [8_u16, 20] {
                for row in 1..divisions {
                    for col in 1..divisions {
                        let x = viewport.x + viewport.w * f32::from(col) / f32::from(divisions);
                        let y = viewport.y + viewport.h * f32::from(row) / f32::from(divisions);
                        let candidate = Spot {
                            x,
                            y,
                            w: 1.0,
                            h: 1.0,
                            handle: None,
                        };
                        if covered(&candidate) {
                            continue;
                        }
                        let before = pleochroma_cut::harness_support::hover_diagnostic_snapshot()
                            .solid_invocations;
                        self.pointer(x, y)?;
                        let hover = if diagram {
                            preview.get_diagram_hover_text().to_string()
                        } else {
                            // The facet the callback resolved for this very move: the text
                            // property keeps the clicked facet's label on a miss.
                            let snapshot =
                                pleochroma_cut::harness_support::hover_diagnostic_snapshot();
                            if snapshot.solid_invocations > before {
                                snapshot
                                    .solid
                                    .and_then(|seen| seen.resolved_label)
                                    .unwrap_or_default()
                            } else {
                                String::new()
                            }
                        };
                        if wanted(hover.as_str()) {
                            return Ok(Spot {
                                x: x - 2.0,
                                y: y - 2.0,
                                w: 4.0,
                                h: 4.0,
                                handle: None,
                            });
                        }
                    }
                }
            }
        }
        let hover_diagnostics = pleochroma_cut::harness_support::hover_diagnostic_snapshot();
        Err(format!(
            "no {what} was found after visible viewport hover sweep: view_mode={}, has_solid={}, has_diagram={}, status={:?}, render_view_tab={}, solving={}, viewport=({}, {}); final hover: {}; callback pick diagnostics: {}{}{}",
            preview.get_view_mode(),
            preview.get_has_solid(),
            preview.get_has_diagram(),
            preview.get_status(),
            self.ui.get_render_view_tab(),
            self.ui.global::<EditorModel>().get_solve_running(),
            preview.get_viewport_width(),
            preview.get_viewport_height(),
            if diagram {
                preview.get_diagram_hover_text()
            } else {
                preview.get_hover_text()
            },
            hover_diagnostics.describe_solid(),
            hover_diagnostics.frames.describe(),
            if diagram {
                hover_diagnostics.describe_diagram()
            } else {
                String::new()
            }
        ))
    }

    /// Moves the pointer over the free part of the picture until the Solid hover callback has
    /// run against a pick buffer and a label table, i.e. until the frame that picking reads has
    /// landed. A sweep before that only ever sees misses.
    fn wait_for_pick_buffer(
        &mut self,
        viewport: &Spot,
        covered: &dyn Fn(&Spot) -> bool,
    ) -> Result<(), String> {
        let probe = [
            (0.5_f32, 0.5_f32),
            (0.4, 0.5),
            (0.6, 0.5),
            (0.5, 0.4),
            (0.5, 0.6),
        ]
        .into_iter()
        .map(|(fx, fy)| Spot {
            x: viewport.w.mul_add(fx, viewport.x),
            y: viewport.h.mul_add(fy, viewport.y),
            w: 1.0,
            h: 1.0,
            handle: None,
        })
        .find(|spot| !covered(spot))
        .ok_or("no free spot on the Solid viewport to probe the pick buffer")?;
        let started = std::time::Instant::now();
        let mut jiggle = 0.0_f32;
        loop {
            let snapshot = pleochroma_cut::harness_support::hover_diagnostic_snapshot();
            let ready = snapshot.solid.as_ref().is_some_and(|seen| {
                seen.pick_dimensions.is_some() && seen.label_count.is_some_and(|n| n > 0)
            });
            if ready {
                return Ok(());
            }
            if started.elapsed() > pacing::STEP_TIMEOUT {
                return Err(format!(
                    "the Solid pick buffer never became ready: {}",
                    snapshot.describe_solid()
                ));
            }
            jiggle = 1.0 - jiggle;
            self.pointer(probe.x + jiggle, probe.y)?;
        }
    }

    fn wait_for_pick_surface(&self) -> Result<(), String> {
        let preview = self.ui.global::<SolidPreviewModel>();
        let editor = self.ui.global::<EditorModel>();
        if pacing::wait_real(pacing::STEP_TIMEOUT, || {
            self.ui.get_active_tab() == 0 && preview.get_has_solid() && !editor.get_solve_running()
        }) {
            Ok(())
        } else {
            Err(format!(
                "facet pick surface did not become ready: top_tab={}, view_mode={}, has_solid={}, solving={}, viewport=({}, {})",
                self.ui.get_active_tab(),
                preview.get_view_mode(),
                preview.get_has_solid(),
                editor.get_solve_running(),
                preview.get_viewport_width(),
                preview.get_viewport_height()
            ))
        }
    }
}

fn intersects(a: &Spot, b: &Spot) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}
