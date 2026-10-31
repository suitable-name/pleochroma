//! Shared physical scoring with native CUDA geometry and optical services.

use std::sync::Arc;

use super::{
    common::{DesignEval, EnvCore, EvalFailure, LiveTierGeometry, to_gpu_planes, yield_loss_pct},
    material_features::material_for_design,
    runtime::EnvExecution,
};
use pleochroma::{
    color::metrics::{FaceUpTone, FinishedStone},
    geometry::{
        SolvedTier, StoneGeometry,
        stone_metrics::{
            SolidMesh, SolidMetrics, SolidStatus, StoneProportions, build_solid_mesh_and_measure,
            build_solid_mesh_geom, measure_solid, mesh_volume,
        },
    },
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use pleochroma_cut_core::{
    DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, Design, ManufacturabilityWarning, ObjectiveComponents,
    ObjectiveWeights, ShapeTarget,
    design::{girdle_band_in, tier_plane_ranges},
    manufacturability::check_manufacturability_with_geometry,
    optimize::{ObjectiveFidelity, evaluate_objective_with_tone_finished_under_threaded},
    volumetric_yield,
};

struct EvaluationInput<'a> {
    design: &'a Design,
    solved: Option<Vec<SolvedTier>>,
    gem: &'a GemMaterial,
    weights: &'a ObjectiveWeights,
    fidelity: ObjectiveFidelity,
    preform_volume: Option<f64>,
    lighting: LightingPreset,
    shape_target: Option<ShapeTarget>,
}

impl EnvCore {
    pub(super) fn evaluate_candidate_design(
        &self,
        design: &Design,
    ) -> Result<DesignEval, EvalFailure> {
        score(
            EvaluationInput {
                design,
                solved: self.space.certified_solve(design),
                gem: &self.gem,
                weights: &self.settings.weights,
                fidelity: self.settings.fidelity,
                preform_volume: self.preform_volume,
                lighting: self.settings.lighting,
                shape_target: self.settings.shape_target,
            },
            self.execution.as_ref(),
        )
    }
}

/// Solves, closes, scores and measures `design` in `gem`.
///
/// `preform_volume` is [`super::common::preform_volume`] of `design`: the caller measures it once per
/// base design. The plane list, mesh and solid measurement of the candidate are built once
/// here and shared with the manufacturability check. The score uses the same core objective
/// path as optimization, including measured yield when requested and available.
pub(crate) fn evaluate_design(
    design: &Design,
    gem: &GemMaterial,
    weights: &ObjectiveWeights,
    fidelity: ObjectiveFidelity,
    preform_volume: Option<f64>,
    lighting: LightingPreset,
    shape_target: Option<ShapeTarget>,
) -> Result<DesignEval, EvalFailure> {
    score(
        EvaluationInput {
            design,
            solved: None,
            gem,
            weights,
            fidelity,
            preform_volume,
            lighting,
            shape_target,
        },
        None,
    )
}

pub(super) fn evaluate_configured(
    design: &Design,
    gem: &GemMaterial,
    settings: &super::common::CoreSettings,
    preform_volume: Option<f64>,
    execution: Option<&Arc<EnvExecution>>,
) -> Result<DesignEval, EvalFailure> {
    score(
        EvaluationInput {
            design,
            solved: None,
            gem,
            weights: &settings.weights,
            fidelity: settings.fidelity,
            preform_volume,
            lighting: settings.lighting,
            shape_target: settings.shape_target,
        },
        execution,
    )
}

fn score(
    input: EvaluationInput<'_>,
    execution: Option<&Arc<EnvExecution>>,
) -> Result<DesignEval, EvalFailure> {
    let mut prepared = prepare(input, execution)?;
    while !prepared.poll() {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    prepared.complete()
}

enum EvaluationOptics {
    Ready(ObjectiveComponents, FaceUpTone),
    Pending(Box<super::runtime::PendingOptics>),
}

pub(super) struct PreparedEvaluation {
    assembly: EvaluationAssembly,
    optics: EvaluationOptics,
}

impl PreparedEvaluation {
    pub(super) fn poll(&mut self) -> bool {
        match &mut self.optics {
            EvaluationOptics::Ready(..) => true,
            EvaluationOptics::Pending(work) => work.poll(),
        }
    }

    pub(super) fn complete(self) -> Result<DesignEval, EvalFailure> {
        let (components, tone) = match self.optics {
            EvaluationOptics::Ready(components, tone) => (components, tone),
            EvaluationOptics::Pending(work) => {
                let result = work.complete()?;
                (result.components, result.tone)
            }
        };
        self.assembly.finish(components, tone)
    }
}

impl EnvCore {
    pub(super) fn prepare_candidate_design(
        &self,
        design: &Design,
    ) -> Result<PreparedEvaluation, EvalFailure> {
        prepare(
            EvaluationInput {
                design,
                solved: self.space.certified_solve(design),
                gem: &self.gem,
                weights: &self.settings.weights,
                fidelity: self.settings.fidelity,
                preform_volume: self.preform_volume,
                lighting: self.settings.lighting,
                shape_target: self.settings.shape_target,
            },
            self.execution.as_ref(),
        )
    }
}

fn prepare(
    input: EvaluationInput<'_>,
    execution: Option<&Arc<EnvExecution>>,
) -> Result<PreparedEvaluation, EvalFailure> {
    let EvaluationInput {
        design,
        solved,
        gem,
        weights,
        fidelity,
        preform_volume,
        lighting,
        shape_target,
    } = input;
    // Certificates are keyed by exact full Design equality, never a hash alone.
    // Reuse the authored solve that the preservation gate already accepted.
    let solved = match solved {
        Some(solved) => solved,
        None => design.solve().map_err(EvalFailure::Unsolvable)?,
    };
    let planes = design.planes_from_solved(&solved);
    let tools = design.tools_for_scoring(&planes);
    let gpu_planes = to_gpu_planes(&planes);
    let geometry = StoneGeometry {
        planes: &gpu_planes,
        tools: &tools,
    };
    let (status, metrics) = if let Some(runtime) = execution {
        let solid = runtime.solid(&planes, &tools)?;
        (solid.status, solid.metrics)
    } else if tools.is_empty() {
        build_solid_mesh_and_measure(&planes)
    } else {
        let status = build_solid_mesh_geom(&planes, &tools);
        let metrics = if let SolidStatus::Closed(mesh) = &status {
            measure_solid(&planes).map(|mut metrics| {
                metrics.volume = mesh_volume(mesh);
                metrics
            })
        } else {
            None
        };
        (status, metrics)
    };
    let SolidStatus::Closed(mesh) = status else {
        return Err(EvalFailure::NotClosed);
    };

    // The physical absorption scale uses this candidate's width. Reuse the solid
    // measurement already built above instead of performing another plane arrangement.
    let prepared_material = material_for_design(
        design,
        gem,
        metrics.as_ref().map(|metrics| metrics.width_axis),
    );
    let finishes = design.facet_finishes_from_solved(&solved, &gpu_planes);
    let stone = FinishedStone::new(geometry, &finishes);
    let accelerated = execution
        .map(|runtime| runtime.prepare_optics(stone, &prepared_material, fidelity, lighting))
        .transpose()?
        .flatten();
    let optics = if let Some(pending) = accelerated {
        EvaluationOptics::Pending(Box::new(pending))
    } else {
        let (components, tone) = evaluate_objective_with_tone_finished_under_threaded(
            stone,
            &prepared_material,
            fidelity,
            lighting,
            Some(1),
        );
        EvaluationOptics::Ready(components, tone)
    };
    Ok(PreparedEvaluation {
        assembly: EvaluationAssembly {
            weights: *weights,
            shape_target,
            geometry: DesignMeasurements::prepare(
                design,
                &solved,
                &planes,
                &mesh,
                metrics.as_ref(),
                preform_volume,
            ),
        },
        optics,
    })
}

struct EvaluationAssembly {
    weights: ObjectiveWeights,
    shape_target: Option<ShapeTarget>,
    geometry: DesignMeasurements,
}

struct DesignMeasurements {
    masts: Vec<f64>,
    table_percent: Option<f64>,
    crown_to_pavilion: Option<f64>,
    yield_pct: Option<f32>,
    girdle_thickness: Option<f64>,
    vanishing_facets: usize,
    undersized_facets: usize,
    width_axis: Option<f64>,
    live_tier_geometry: Vec<LiveTierGeometry>,
}

impl DesignMeasurements {
    fn prepare(
        design: &Design,
        solved: &[SolvedTier],
        planes: &[(glam::DVec3, f64)],
        mesh: &SolidMesh,
        metrics: Option<&SolidMetrics>,
        preform_volume: Option<f64>,
    ) -> Self {
        // Keep CPU geometry assembly on the preparing pool job. The continuation retains
        // compact measurements only; candidate meshes and borrowed authoring inputs can drop.
        let (table_percent, crown_to_pavilion) = metrics.map_or((None, None), |metrics| {
            let proportions = StoneProportions::from_solid(metrics, mesh, planes);
            let ratio = match (proportions.crown_height, proportions.pavilion_depth) {
                (Some(crown), Some(depth)) if depth > 0.0 => Some(crown / depth),
                _ => None,
            };
            (proportions.table_percent, ratio)
        });
        let yield_pct = metrics
            .and_then(|finished| volumetric_yield(finished.volume, preform_volume?))
            .map(|fraction| (fraction * 100.0) as f32);
        let ranges = tier_plane_ranges(design, solved);
        let live_tier_geometry = live_geometry_by_tier(&ranges, mesh);
        let girdle_thickness =
            girdle_band_in(design, &ranges, planes, mesh).map(|band| band.min_thickness);

        let warnings = check_manufacturability_with_geometry(
            design,
            solved,
            Some(mesh),
            metrics,
            DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
        );
        let vanishing_facets = warnings
            .iter()
            .filter(|warning| matches!(warning, ManufacturabilityWarning::VanishingFacet { .. }))
            .count();
        let undersized_facets = warnings
            .iter()
            .filter(|warning| matches!(warning, ManufacturabilityWarning::UndersizedFacet { .. }))
            .count();

        Self {
            masts: solved.iter().map(|tier| tier.mast).collect(),
            table_percent,
            crown_to_pavilion,
            yield_pct,
            girdle_thickness,
            vanishing_facets,
            undersized_facets,
            width_axis: metrics.map(|m| m.width_axis),
            live_tier_geometry,
        }
    }
}

impl EvaluationAssembly {
    fn finish(
        self,
        components: ObjectiveComponents,
        tone: FaceUpTone,
    ) -> Result<DesignEval, EvalFailure> {
        let Self {
            weights,
            shape_target,
            geometry,
        } = self;
        let DesignMeasurements {
            masts,
            table_percent,
            crown_to_pavilion,
            yield_pct,
            girdle_thickness,
            vanishing_facets,
            undersized_facets,
            width_axis,
            live_tier_geometry,
        } = geometry;
        // Reject invalid optical results before composing yield and tone terms.
        let finite = [
            components.windowing_pct,
            components.extinction_pct,
            components.tilt_brilliance_pct,
        ]
        .iter()
        .all(|value| value.is_finite());
        if !finite {
            return Err(EvalFailure::NonFiniteScore);
        }

        let tone_lighter_loss = pleochroma_cut_core::ToneGoal::Lighter.loss_pct(&tone);
        let tone_deeper_loss = pleochroma_cut_core::ToneGoal::Deeper.loss_pct(&tone);
        let tone_lighter_loss_pct = Some(tone_lighter_loss);
        let tone_deeper_loss_pct = Some(tone_deeper_loss);
        let effective_yield_weight = if yield_pct.is_some() {
            weights.yield_weight
        } else {
            0.0
        };
        let effective_weights = ObjectiveWeights {
            yield_weight: effective_yield_weight,
            ..weights
        };
        let tone_loss = match weights.tone_goal {
            pleochroma_cut_core::ToneGoal::Lighter => tone_lighter_loss,
            pleochroma_cut_core::ToneGoal::Deeper => tone_deeper_loss,
        };
        let mut score = effective_weights.score_with_tone(
            &components,
            yield_pct.map_or(0.0, yield_loss_pct),
            tone_loss,
        );
        let shape_distance_pct = shape_target.map_or(f32::NAN, |target| {
            (100.0 * target.distance(table_percent, crown_to_pavilion)) as f32
        });
        if let Some(target) = shape_target {
            score += target.penalty(table_percent, crown_to_pavilion);
        }
        if !score.is_finite() {
            return Err(EvalFailure::NonFiniteScore);
        }

        Ok(DesignEval {
            score,
            components,
            tone_lighter_loss_pct,
            tone_deeper_loss_pct,
            tone,
            shape_distance_pct,
            masts,
            table_percent,
            crown_to_pavilion,
            yield_pct,
            girdle_thickness,
            vanishing_facets,
            undersized_facets,
            width_axis,
            live_tier_geometry,
        })
    }
}

fn live_geometry_by_tier(
    ranges: &[std::ops::Range<usize>],
    mesh: &SolidMesh,
) -> Vec<LiveTierGeometry> {
    let mut areas = vec![0.0_f64; ranges.len()];
    let mut moments = vec![[0.0_f64; 3]; ranges.len()];
    let mut total = 0.0_f64;
    for (facet, ring) in &mesh.rings {
        let Some((tier_index, _)) = ranges.iter().enumerate().find(|(_, r)| r.contains(facet))
        else {
            continue;
        };
        if ring.len() < 3 {
            continue;
        }
        let origin = ring[0];
        let mut area = 0.0;
        let mut moment = [0.0; 3];
        for i in 1..ring.len() - 1 {
            let a = ring[i] - origin;
            let b = ring[i + 1] - origin;
            let triangle_area = 0.5 * a.cross(b).length();
            if !triangle_area.is_finite() {
                continue;
            }
            let center = (origin + ring[i] + ring[i + 1]) / 3.0;
            area += triangle_area;
            moment[0] += center.x * triangle_area;
            moment[1] += center.y * triangle_area;
            moment[2] += center.z * triangle_area;
        }
        areas[tier_index] += area;
        for axis in 0..3 {
            moments[tier_index][axis] += moment[axis];
        }
        total += area;
    }
    ranges
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let area = areas[i];
            LiveTierGeometry {
                area_fraction: if total > 0.0 {
                    (area / total) as f32
                } else {
                    0.0
                },
                centroid: if area > 0.0 {
                    [
                        moments[i][0] / area,
                        moments[i][1] / area,
                        moments[i][2] / area,
                    ]
                } else {
                    [0.0; 3]
                },
                available: area > 0.0 && total > 0.0,
            }
        })
        .collect()
}
