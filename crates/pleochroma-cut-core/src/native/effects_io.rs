//! The render-physics parts of the design file: the inclusion and structure subtables of
//! `[render_effects]` and the surface columns of a tier record, converted between the plain
//! tables of `pleochroma_formats::native` and this crate's types.
//!
//! Everything here is written only when set, so a design that does not use the features keeps
//! its bytes. A name this build does not know (an inclusion preset, a pattern, a grade) reads as
//! "not set" for that part; the rest of the file still loads.

use crate::design::{
    Design, InclusionEffects, StructureEffects, SurfaceFinish, SurfaceGrade, TierSurface,
    effects_spec::{
        canonical_preset, pattern_from_word, sheen_from_word, sheen_word, silk_color_from_word,
        silk_color_word,
    },
    surface::DEFAULT_CUSTOM_RQ_NM,
};
use pleochroma_formats::native::{
    SurfaceColumns, SurfaceFacetTable,
    design::{InclusionEffectsTable, StructureEffectsTable},
};

/// The file table of an inclusion choice.
pub(super) fn inclusion_table(effects: &InclusionEffects) -> InclusionEffectsTable {
    InclusionEffectsTable {
        preset: effects.preset.to_owned(),
        density: effects.density,
        turn_deg: effects.turn_deg,
        sheen: effects.sheen.map(|sheen| sheen_word(sheen).to_owned()),
        color: effects.color.map(|color| silk_color_word(color).to_owned()),
        unknown: toml::Table::new(),
    }
}

/// The inclusion choice a file table states, `None` for a preset the catalogue does not have.
pub(super) fn inclusions_from_table(table: &InclusionEffectsTable) -> Option<InclusionEffects> {
    Some(
        InclusionEffects {
            preset: canonical_preset(&table.preset)?,
            density: table.density,
            turn_deg: table.turn_deg,
            sheen: table.sheen.as_deref().and_then(sheen_from_word),
            color: table.color.as_deref().and_then(silk_color_from_word),
        }
        .sanitised(),
    )
}

/// The file table of a play-of-color choice.
pub(super) fn structure_table(effects: &StructureEffects) -> StructureEffectsTable {
    StructureEffectsTable {
        pattern: effects.pattern.label().to_owned(),
        seed: effects.seed,
        size: effects.size,
        color_low_nm: effects.color_low_nm,
        color_high_nm: effects.color_high_nm,
        purity: effects.purity,
        coverage: effects.coverage,
        unknown: toml::Table::new(),
    }
}

/// The play-of-color choice a file table states, `None` for a pattern this build does not know.
pub(super) fn structure_from_table(table: &StructureEffectsTable) -> Option<StructureEffects> {
    Some(
        StructureEffects {
            pattern: pattern_from_word(&table.pattern)?,
            seed: table.seed,
            size: table.size,
            color_low_nm: table.color_low_nm,
            color_high_nm: table.color_high_nm,
            purity: table.purity,
            coverage: table.coverage,
        }
        .sanitised(),
    )
}

/// The values of one finish as file columns: `(grade word, Rq, angle, anisotropy, no sweep,
/// ground alpha)`. The Rq and anisotropy are written for a Custom grade only.
type FinishColumns = (
    Option<String>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    bool,
    Option<f64>,
);

fn finish_columns(finish: &SurfaceFinish) -> FinishColumns {
    let custom = finish.grade == SurfaceGrade::Custom;
    (
        (finish.grade != SurfaceGrade::Ideal).then(|| finish.grade.label().to_owned()),
        custom.then_some(finish.rq_nm),
        finish.lap_deg,
        (custom && finish.aniso > 1.0).then_some(finish.aniso),
        finish.no_sweep,
        finish.ground_alpha,
    )
}

fn finish_from_columns(columns: &FinishColumns) -> SurfaceFinish {
    let (grade, rq_nm, lap_deg, aniso, no_sweep, ground_alpha) = columns;
    let grade = grade
        .as_deref()
        .and_then(SurfaceGrade::from_label)
        .unwrap_or(SurfaceGrade::Ideal);
    SurfaceFinish {
        grade,
        rq_nm: rq_nm.unwrap_or(if grade == SurfaceGrade::Custom {
            DEFAULT_CUSTOM_RQ_NM
        } else {
            0.0
        }),
        aniso: aniso.unwrap_or(1.0),
        lap_deg: *lap_deg,
        no_sweep: *no_sweep,
        ground_alpha: *ground_alpha,
    }
    .sanitised()
}

/// The surface columns of a tier record for `surface` (nothing written for `None`).
pub(super) fn surface_columns(surface: Option<&TierSurface>) -> SurfaceColumns {
    let Some(surface) = surface else {
        return SurfaceColumns::default();
    };
    let (grade, rq_nm, lap_deg, aniso, no_sweep, ground_alpha) = finish_columns(&surface.finish);
    SurfaceColumns {
        surface: grade,
        rq_nm,
        lap_deg,
        aniso,
        no_sweep,
        ground_alpha,
        facets: surface
            .facets
            .iter()
            .map(|(at, finish)| {
                let (grade, rq_nm, lap_deg, aniso, no_sweep, ground_alpha) = finish_columns(finish);
                SurfaceFacetTable {
                    at: *at,
                    surface: grade,
                    rq_nm,
                    lap_deg,
                    aniso,
                    no_sweep,
                    ground_alpha,
                }
            })
            .collect(),
    }
}

/// The surface a tier record's columns state, `None` when they state nothing.
pub(super) fn tier_surface_from_columns(columns: &SurfaceColumns) -> Option<TierSurface> {
    TierSurface {
        finish: finish_from_columns(&(
            columns.surface.clone(),
            columns.rq_nm,
            columns.lap_deg,
            columns.aniso,
            columns.no_sweep,
            columns.ground_alpha,
        )),
        facets: columns
            .facets
            .iter()
            .map(|facet| {
                (
                    facet.at,
                    finish_from_columns(&(
                        facet.surface.clone(),
                        facet.rq_nm,
                        facet.lap_deg,
                        facet.aniso,
                        facet.no_sweep,
                        facet.ground_alpha,
                    )),
                )
            })
            .collect(),
    }
    .normalised()
}

/// Restores `design.tier_surfaces` from `raw` (the tiers' surface columns, captured before the
/// file's `tiers` array was consumed), each entry keyed by the id its tier ended up with. A
/// tier whose columns state nothing is ideal, which is also what a file saved before surface
/// finish existed reads as. Must run after the tier ids are applied.
pub(super) fn apply_tier_surfaces(design: &mut Design, raw: &[SurfaceColumns]) {
    design.tier_surfaces.clear();
    for (columns, &id) in raw.iter().zip(&design.tier_ids) {
        if let Some(surface) = tier_surface_from_columns(columns) {
            design.tier_surfaces.insert(id, surface);
        }
    }
}
