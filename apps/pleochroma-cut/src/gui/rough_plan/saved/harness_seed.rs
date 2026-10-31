//! A saved Rough Planner plan with a stored rough color, for the tutorial videos (feature
//! `ui-harness` only).
//!
//! The Rough Color lessons need a saved plan whose rough is a mesh and that already has a color:
//! the wizard restores a stored fit and photos from such a plan, and the planner offers Stone
//! Orientation and Use Color for it. A real one comes from a wizard run on photos of a rough on a
//! camera rig, which no video can film, so this writes the same rows the wizard's Accept and the
//! planner's Save write, from numbers generated here:
//!
//! * the plan: a non-convex C-shaped mesh rough (20 mm cube with a notch, so the planner keeps its
//!   mesh and Rough Color... is enabled), two layouts of two library designs, saved through
//!   [`store::replace_plan`] with the planner's own writer ([`serialize_checked_plan`]);
//! * the color: an amethyst base zone and a citrine half space, stored through
//!   `rough_colour::store::save_rough_colour_with_photos` together with a fixed fit report and two
//!   small synthetic backlit photos (a soft vignette of one color), as Accept stores them.
//!
//! Everything is deterministic; nothing is read from a file. The designs are the first two of the
//! library that have measured extents (else the first two), so the plan opens against any library.

use super::{
    convert::CandidateSource,
    dto::SavedDesignDto,
    format::{RankedLayout, SerializeInput, design_shape, serialize_checked_plan},
    naming::current_unix_time,
    store,
};
use crate::gui::rough_colour::store as colour_store;
use glam::DVec3;
use pleochroma::{
    color::body_color::Illuminant,
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
    },
};
use pleochroma_cut_core::{
    carat_weight,
    material::built_in_specific_gravity,
    rough_plan::{
        Axis, BarCut, CutOrder, CutPlan, PlacedStone, PlanSettings, RoughLayout, RoughModel,
        SlabCut,
        colour_fit::solve::{
            ChoiceReason, ColourFit, LovoEntry, LovoReport, ModelComparison, ModelFit, ModelKind,
            ModelScore, ZonePrediction,
        },
        fit::StonePose,
        import_mesh,
        photometry::{PixelMask, ResampledImage, WorkingGrid},
        zoned_plan::RoughColour,
    },
};
use pleochroma_vault::db::sqlite::Database;
use std::sync::{Mutex, PoisonError};

/// The name of the seeded plan, as the Saved Plans list shows it.
pub const PLAN_NAME: &str = "Amethyst Rough";

/// The material of the seeded plan.
const MATERIAL: &str = "Amethyst";

/// The vertices of the C-shaped rough: a 20 mm cube with a 10 x 10 x 20 mm notch in its `+x` face.
const C_SHAPE_POINTS: [[f64; 3]; 16] = [
    [0.0, 0.0, 0.0],
    [20.0, 0.0, 0.0],
    [20.0, 5.0, 0.0],
    [10.0, 5.0, 0.0],
    [10.0, 15.0, 0.0],
    [20.0, 15.0, 0.0],
    [20.0, 20.0, 0.0],
    [0.0, 20.0, 0.0],
    [0.0, 0.0, 20.0],
    [20.0, 0.0, 20.0],
    [20.0, 5.0, 20.0],
    [10.0, 5.0, 20.0],
    [10.0, 15.0, 20.0],
    [20.0, 15.0, 20.0],
    [20.0, 20.0, 20.0],
    [0.0, 20.0, 20.0],
];

/// The faces of the C-shaped rough (1-based vertex numbers, counter-clockwise seen from outside);
/// quads are fanned into triangles.
const C_SHAPE_FACES: [&[u32]; 20] = [
    &[9, 10, 11],
    &[9, 11, 12],
    &[9, 12, 13],
    &[9, 13, 16],
    &[13, 14, 15],
    &[13, 15, 16],
    &[1, 3, 2],
    &[1, 4, 3],
    &[1, 5, 4],
    &[1, 8, 5],
    &[5, 7, 6],
    &[5, 8, 7],
    &[1, 2, 10, 9],
    &[2, 3, 11, 10],
    &[3, 4, 12, 11],
    &[4, 5, 13, 12],
    &[5, 6, 14, 13],
    &[6, 7, 15, 14],
    &[7, 8, 16, 15],
    &[8, 1, 9, 16],
];

/// The kerf and the allowance per side the layouts were cut with, mm.
const KERF_MM: f64 = 0.3;
const ALLOWANCE_MM: f64 = 0.2;

/// The rig views whose synthetic photos are stored, and their size in working pixels.
const PHOTO_VIEWS: [u32; 2] = [0, 1];
const PHOTO_WIDTH: usize = 32;
const PHOTO_HEIGHT: usize = 24;

/// Full-resolution photo pixels per working pixel: the seeded photos stand for 3200 x 2400 pixel
/// camera images (the rig the wizard video fills in, `wizard::harness_prime`), so the zone
/// outlines the wizard projects land on the small working picture.
const PHOTO_SCALE: f64 = 100.0;

/// Writes the plan and its color into the library database at `database_path` and returns the
/// plan's name. The app must not be running on that file. A plan of the same name is replaced.
///
/// # Errors
///
/// The database cannot be opened, the library has no design, or a write fails; the sentence says
/// which.
pub fn seed_rough_color_plan(database_path: &str) -> Result<String, String> {
    let database = Database::new(Some(database_path))
        .map_err(|e| format!("cannot open {database_path}: {e:#}"))?;
    let designs = pick_designs(&database)?;
    let model = c_shape_model()?;
    let volume = model
        .measure()
        .map_err(|e| format!("the seeded rough has no volume: {e:?}"))?
        .volume_mm3;
    let settings = PlanSettings {
        max_designs: 6,
        max_stones: 0,
        kerf_mm: KERF_MM,
        allowance_mm: ALLOWANCE_MM,
        skin_mm: 0.0,
        min_width_mm: 1.0,
        specific_gravity: built_in_specific_gravity(MATERIAL)
            .map_or(2.65, |gravity| gravity.representative),
    };
    let (first, second) = layouts(designs.ids, volume, settings.specific_gravity);
    let created = current_unix_time();
    let database = Mutex::new(database);
    let library_id = store::library_stamp(&database);
    let ranked = [
        RankedLayout {
            rank: 1,
            layout: &first,
        },
        RankedLayout {
            rank: 2,
            layout: &second,
        },
    ];
    let text = serialize_checked_plan(&SerializeInput {
        name: PLAN_NAME,
        created_at: created,
        library_id,
        model: &model,
        material_name: MATERIAL,
        weighed_ct: None,
        settings: &settings,
        candidate_source: CandidateSource::Library,
        designs: &designs.records,
        layouts: &ranked,
    })?;
    let plan_id = store::replace_plan(&database, PLAN_NAME, &text)?;

    let colour = rough_colour(created)?;
    let photos: Vec<(u32, ResampledImage)> = PHOTO_VIEWS
        .iter()
        .map(|&view| (view, backlit_photo(view)))
        .collect();
    let views: Vec<(u32, &ResampledImage)> =
        photos.iter().map(|(view, image)| (*view, image)).collect();
    let guard = database.lock().unwrap_or_else(PoisonError::into_inner);
    colour_store::save_rough_colour_with_photos(&guard, plan_id, &colour, &views)
        .map_err(|e| format!("cannot store the rough color of the seeded plan: {e:#}"))?;
    Ok(PLAN_NAME.to_owned())
}

/// The designs the seeded layouts use.
struct Designs {
    /// The entry ids of the first and the second design (equal when the library has only one).
    ids: [i64; 2],
    /// The records the plan file stores for them, one per distinct design.
    records: Vec<SavedDesignDto>,
}

/// The first two designs of the library that have measured extents, else the first two.
fn pick_designs(database: &Database) -> Result<Designs, String> {
    let all = database
        .all_entry_ids()
        .map_err(|e| format!("cannot list the library designs: {e:#}"))?;
    let extents = database
        .solid_extents_for(&all)
        .map_err(|e| format!("cannot read the design measurements: {e:#}"))?;
    let measured: Vec<i64> = all
        .iter()
        .copied()
        .filter(|id| extents.get(id).and_then(|row| row.extents).is_some())
        .collect();
    let pool = if measured.is_empty() { &all } else { &measured };
    let Some(&first) = pool.first() else {
        return Err("the library has no design to plan the seeded rough with".to_owned());
    };
    let second = pool.get(1).copied().unwrap_or(first);
    let mut distinct = vec![first];
    if second != first {
        distinct.push(second);
    }
    let titles = database
        .entry_titles_for(&distinct)
        .map_err(|e| format!("cannot read the design titles: {e:#}"))?;
    let records = distinct
        .iter()
        .map(|&entry_id| {
            let shape = extents
                .get(&entry_id)
                .and_then(|row| row.extents)
                .map(|found| design_shape(&found))
                .unwrap_or_default();
            SavedDesignDto {
                entry_id,
                title: titles
                    .get(&entry_id)
                    .cloned()
                    .unwrap_or_else(|| format!("Design {entry_id}")),
                fingerprint: shape.fingerprint,
                width_caliper: shape.width_caliper,
                extents_version: shape.extents_version,
            }
        })
        .collect();
    Ok(Designs {
        ids: [first, second],
        records,
    })
}

/// The C-shaped mesh rough, imported as the planner imports an OBJ file.
fn c_shape_model() -> Result<RoughModel, String> {
    let points: Vec<DVec3> = C_SHAPE_POINTS
        .iter()
        .map(|&[x, y, z]| DVec3::new(x, y, z))
        .collect();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    for face in C_SHAPE_FACES {
        for pair in face[1..].windows(2) {
            triangles.push([face[0] - 1, pair[0] - 1, pair[1] - 1]);
        }
    }
    let (base, _note) = import_mesh(&points, &triangles)
        .map_err(|e| format!("the seeded mesh rough cannot be imported: {e:?}"))?;
    Ok(RoughModel::new(base, Vec::new()))
}

/// One stone: the `piece` box at `origin`, 0.4 mm smaller on every axis, its table facing `+y`.
fn stone(entry_id: i64, origin: [f64; 3], piece: [f64; 3], specific_gravity: f64) -> PlacedStone {
    let size = piece.map(|side| (-2.0_f64).mul_add(ALLOWANCE_MM, side));
    let volume_mm3 = 0.45 * size[0] * size[1] * size[2];
    PlacedStone {
        entry_id,
        piece_origin_mm: origin,
        piece_size_mm: piece,
        stone_size_mm: size,
        table_axis: Axis::Y,
        carat: carat_weight(volume_mm3, specific_gravity),
        volume_mm3,
        pose: StonePose {
            center_mm: [0, 1, 2].map(|i| 0.5f64.mul_add(piece[i], origin[i])),
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            mm_per_unit: 5.0,
        },
    }
}

/// A layout of `stones` cut in the order Y, X, Z with `plan` as its saw plan.
fn layout(stones: Vec<PlacedStone>, plan: Vec<SlabCut>, rough_volume_mm3: f64) -> RoughLayout {
    let total_volume_mm3: f64 = stones.iter().map(|s| s.volume_mm3).sum();
    let total_carat: f64 = stones.iter().map(|s| s.carat).sum();
    RoughLayout {
        cut_order: CutOrder::Yxz,
        stones,
        cut_plan: CutPlan { slabs: plan },
        total_carat,
        total_volume_mm3,
        yield_fraction: total_volume_mm3 / rough_volume_mm3,
        exact_fit: false,
    }
}

/// The two layouts of the plan: three stones (designs a, b, a) and two (a, b).
///
/// The pieces lie in the solid left slab of the C (x below 10 mm); every figure follows from the
/// piece boxes, the kerf and the allowance, so the loader's cross-checks pass.
fn layouts(ids: [i64; 2], rough_volume_mm3: f64, gravity: f64) -> (RoughLayout, RoughLayout) {
    let [a, b] = ids;
    let piece_a = stone(a, [0.0, 0.0, 0.0], [6.0, 8.0, 7.0], gravity);
    let piece_b = stone(b, [0.0, 0.0, 7.0 + KERF_MM], [6.0, 8.0, 6.4], gravity);
    let piece_c = stone(a, [0.0, 8.0 + KERF_MM, 0.0], [5.0, 6.0, 6.0], gravity);
    let first_slab = SlabCut {
        thickness_mm: 8.0,
        bars: vec![BarCut {
            width_mm: 6.0,
            pieces_mm: vec![7.0, 6.4],
        }],
    };
    let second_slab = SlabCut {
        thickness_mm: 6.0,
        bars: vec![BarCut {
            width_mm: 5.0,
            pieces_mm: vec![6.0],
        }],
    };
    let three = layout(
        vec![piece_a, piece_b, piece_c],
        vec![first_slab.clone(), second_slab],
        rough_volume_mm3,
    );
    let two = layout(vec![piece_a, piece_b], vec![first_slab], rough_volume_mm3);
    (three, two)
}

/// An amethyst base zone and a citrine half space (`z` above 4 mm): the color of an ametrine
/// rough, as the wizard's Accept stores zones, with the fit report of the fit that found it.
fn rough_colour(created: i64) -> Result<RoughColour, String> {
    let amethyst = ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![
        AbsorptionBand::new(540.0, 70.0, 0.22),
        AbsorptionBand::new(430.0, 40.0, 0.04),
    ]));
    let citrine = ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
        430.0, 55.0, 0.30,
    )]));
    let mut zoned = ZonedAbsorption::new(amethyst);
    zoned.zones.push(Zone {
        shape: ZoneShape::HalfSpace {
            normal: DVec3::Z,
            offset: 4.0,
        },
        absorption: citrine,
    });
    let fit = fixed_fit(&zoned);
    RoughColour::new(zoned, &fit, created)
        .map_err(|e| format!("the seeded rough color cannot be stored: {e}"))
}

/// One predicted face-up colour of the fit report.
const fn prediction(zone: usize, size_mm: f64, illuminant: Illuminant) -> ZonePrediction {
    let lab = if zone == 0 {
        [48.0, 24.0, -26.0]
    } else {
        [78.0, 9.0, 54.0]
    };
    ZonePrediction {
        zone,
        size_mm,
        path_mm: size_mm,
        illuminant,
        lab,
        lab_sigma: [1.1, 1.4, 1.6],
        delta_e_radius: 1.6,
        metamer_spread: 2.4,
        metamer_spread_unverified: 2.7,
    }
}

/// A converged two-zone fit report for `zoned`: the numbers a clean eight-view fit would give.
fn fixed_fit(zoned: &ZonedAbsorption) -> ColourFit {
    let mut tensors = vec![zoned.base.tensor.clone()];
    tensors.extend(
        zoned
            .zones
            .iter()
            .map(|zone| zone.absorption.tensor.clone()),
    );
    let mut predictions = Vec::new();
    for illuminant in [Illuminant::D65, Illuminant::Planckian(2856.0)] {
        for zone in 0..2 {
            for size in [7.0, 12.0] {
                predictions.push(prediction(zone, size, illuminant));
            }
        }
    }
    let model_fit = ModelFit {
        kind: ModelKind::SmoothBasis,
        host_id: None,
        param_names: Vec::new(),
        params: Vec::new(),
        view_ids: vec![0, 1],
        log_gains: vec![0.0, 0.0],
        covariance: Vec::new(),
        zone_tensors: tensors,
        cost: 1.0,
        data_cost: 0.8,
        prior_terms: Vec::new(),
        birge_ratio: 1.0,
        chi2: 10.0,
        n_data: 100,
        effective_params: 7.0,
        aic: 24.0,
        iterations: 5,
        iterations_per_stage: vec![2, 2, 1],
        converged: true,
        seed_costs: Vec::new(),
        at_bound: Vec::new(),
        predictions,
    };
    ColourFit {
        version: 1,
        n_zones: 2,
        fits: vec![model_fit],
        chosen_index: 0,
        comparison: ModelComparison {
            scores: vec![ModelScore {
                kind: ModelKind::SmoothBasis,
                chi2: 10.0,
                effective_params: 7.0,
                aic: 24.0,
            }],
            noise_scale2: 1.0,
            likelihood_ratio: None,
            chosen: ModelKind::SmoothBasis,
            reason: ChoiceReason::OnlyOneModel,
        },
        roughness: None,
        alignment: None,
        lovo: Some(LovoReport {
            entries: vec![
                LovoEntry {
                    view: 0,
                    pixels: 1000,
                    delta_e: 1.2,
                    delta_e_full_fit_gain: 1.0,
                    gain: 1.0,
                },
                LovoEntry {
                    view: 1,
                    pixels: 900,
                    delta_e: 1.9,
                    delta_e_full_fit_gain: 1.5,
                    gain: 1.0,
                },
            ],
            median_delta_e: 1.55,
            max_delta_e: 1.9,
        }),
        residuals: Vec::new(),
        structured_score: 0.1,
        residual_rms: 1.1,
        suggest_zoning: false,
        warnings: Vec::new(),
    }
}

/// A small synthetic backlit photo of the stone: one violet color with a soft vignette, a little
/// different for every `view`, valid everywhere.
fn backlit_photo(view: u32) -> ResampledImage {
    let shade = (-0.04_f32).mul_add(view as f32, 1.0);
    let mut values = Vec::with_capacity(PHOTO_WIDTH * PHOTO_HEIGHT);
    for y in 0..PHOTO_HEIGHT {
        for x in 0..PHOTO_WIDTH {
            let dx = (x as f32 + 0.5) / PHOTO_WIDTH as f32 - 0.5;
            let dy = (y as f32 + 0.5) / PHOTO_HEIGHT as f32 - 0.5;
            let falloff = (-0.6_f32).mul_add(dx.mul_add(dx, dy * dy), 1.0);
            values.push([
                0.42 * falloff * shade,
                0.26 * falloff * shade,
                0.55 * falloff * shade,
            ]);
        }
    }
    let count = PHOTO_WIDTH * PHOTO_HEIGHT;
    ResampledImage {
        grid: WorkingGrid {
            width: PHOTO_WIDTH,
            height: PHOTO_HEIGHT,
            origin: [0.0, 0.0],
            scale: PHOTO_SCALE,
        },
        values,
        variance: vec![[1e-4; 3]; count],
        coverage: vec![1.0; count],
        mask: PixelMask::new(PHOTO_WIDTH, PHOTO_HEIGHT),
    }
}
