//! Conversions between the on-disk mirror types of [`pleochroma_formats::native`] and this
//! crate's editor types.
//!
//! The editor types are [`Design`], [`ConstraintTier`], [`MaterialSelection`] and
//! [`PreformSpec`]. `pleochroma`'s [`pleochroma::geometry::meet_solver::MeetConstraint`] is
//! converted too. These need the editor types, so they live here, not in the format crate.
//!
//! These are free functions, not `From`/`Into` impls. Most pairs involve two foreign types
//! (the mirror types and `MeetConstraint`), and the orphan rules do not allow a trait impl
//! between two types that are both foreign to this crate.

use crate::{
    design::{ConcaveTier, ConcaveTool, ConstraintTier, Design, TierId, TierTarget, ToolMotion},
    material::{
        ColorMode, MaterialSelection, RayBandRows, VariantChoice, biaxial_character_from_deltas,
        color::nearest_legacy_rgb, crystal_system_from_name, is_biaxial,
        optical_character_from_name, with_library_bands, with_library_ray_bands,
    },
    preform::{PlaneSet, PreformShape, PreformSpec},
};
use glam::DVec3;
use pleochroma::{
    color::{body_color::Illuminant, principal_axes::face_up_body_color},
    geometry::{meet_solver::MeetConstraint, stone_metrics::ExternalProportions},
    optics::materials::{
        AbsorptionUnit, GemMaterial,
        variants::{
            MAX_VARIANT_BANDS, VariantBandRow, VariantBandShape, VariantTensorRows, face_up_swatch,
            face_up_swatch_path,
        },
    },
};
use pleochroma_formats::native::{
    COLOR_VARIANT_SHAPE_ENERGY, COLOR_VARIANT_SHAPE_WAVELENGTH, COLOR_VARIANT_UNIT_MODEL_UNIT,
    COLOR_VARIANT_UNIT_PER_MM, ColorRecipeDto, ColorVariantBandTable, ColorVariantTable,
    ConcaveTierTable, CustomMaterialSnapshot, MaterialTable, NativeMeetConstraint,
    NativePreformShape, NativeTierTarget, PreformTable, SourceTable, TierTable,
};
use std::sync::Arc;

pub(super) fn native_meet_constraint_from(constraint: &MeetConstraint) -> NativeMeetConstraint {
    match constraint {
        MeetConstraint::MeetExisting => NativeMeetConstraint::MeetExisting,
        MeetConstraint::MeetNamed(names) => NativeMeetConstraint::MeetNamed {
            names: names.clone(),
        },
        MeetConstraint::ScaleReference(mast) => {
            NativeMeetConstraint::ScaleReference { mast: *mast }
        }
    }
}

pub(super) fn meet_constraint_from_native(constraint: NativeMeetConstraint) -> MeetConstraint {
    match constraint {
        NativeMeetConstraint::MeetExisting => MeetConstraint::MeetExisting,
        NativeMeetConstraint::MeetNamed { names } => MeetConstraint::MeetNamed(names),
        NativeMeetConstraint::ScaleReference { mast } => MeetConstraint::ScaleReference(mast),
    }
}

/// Mirrors a [`TierTarget`] into its on-disk [`NativeTierTarget`].
#[must_use]
pub(super) const fn native_tier_target_from(target: TierTarget) -> NativeTierTarget {
    match target {
        TierTarget::DepthMm(mm) => NativeTierTarget::DepthMm { mm },
        TierTarget::GirdleThicknessMm(mm) => NativeTierTarget::GirdleThicknessMm { mm },
        TierTarget::TableWidthMm(mm) => NativeTierTarget::TableWidthMm { mm },
    }
}

/// The inverse of [`native_tier_target_from`].
#[must_use]
pub(super) const fn tier_target_from_native(target: NativeTierTarget) -> TierTarget {
    match target {
        NativeTierTarget::DepthMm { mm } => TierTarget::DepthMm(mm),
        NativeTierTarget::GirdleThicknessMm { mm } => TierTarget::GirdleThicknessMm(mm),
        NativeTierTarget::TableWidthMm { mm } => TierTarget::TableWidthMm(mm),
    }
}

/// Mirrors a [`PreformShape`] into its on-disk [`NativePreformShape`]. A plane set is
/// written as its `[nx, ny, nz, m]` rows, in order, with its source note.
pub(super) fn native_preform_shape_from(shape: &PreformShape) -> NativePreformShape {
    match shape {
        PreformShape::Block => NativePreformShape::Block,
        PreformShape::Cylinder { sides } => NativePreformShape::Cylinder { sides: *sides },
        PreformShape::Planes(set) => NativePreformShape::Planes {
            planes: set
                .planes
                .iter()
                .map(|&(n, m)| [n.x, n.y, n.z, m])
                .collect(),
            source: set.source.clone(),
        },
    }
}

/// The inverse of [`native_preform_shape_from`]; the rows come back bit for bit.
pub(super) fn preform_shape_from_native(shape: &NativePreformShape) -> PreformShape {
    match shape {
        NativePreformShape::Block => PreformShape::Block,
        NativePreformShape::Cylinder { sides } => PreformShape::Cylinder { sides: *sides },
        NativePreformShape::Planes { planes, source } => PreformShape::Planes(Arc::new(PlaneSet {
            planes: planes
                .iter()
                .map(|&[nx, ny, nz, m]| (DVec3::new(nx, ny, nz), m))
                .collect(),
            source: source.clone(),
        })),
    }
}

/// `y_offset` mirrors [`Design::preform_y_offset`]. It is a design-level anchor that
/// `PreformSpec` does not carry, so it rides alongside the mirrored spec fields.
#[must_use]
pub(super) fn preform_table_from_spec(spec: &PreformSpec, y_offset: f64) -> PreformTable {
    PreformTable::new(
        native_preform_shape_from(&spec.shape),
        spec.half_width,
        spec.length_over_width,
        spec.depth,
    )
    .with_y_offset(y_offset)
}

/// The spec's size fields are read back exactly as the table holds them (a plane set's
/// are its recorded bounding box; they are not recomputed from the planes).
#[must_use]
pub(super) fn preform_spec_from_table(table: &PreformTable) -> PreformSpec {
    PreformSpec {
        shape: preform_shape_from_native(&table.shape),
        half_width: table.half_width,
        length_over_width: table.length_over_width,
        depth: table.depth,
    }
}

#[must_use]
pub(super) fn material_table_from_selection(selection: &MaterialSelection) -> MaterialTable {
    let table = MaterialTable::new(
        selection.name.clone(),
        selection.specific_gravity_override,
        selection.refractive_index_override,
    );
    let mut table = if let Some(choice) = &selection.color_variant_override {
        // The variant wins on load. The legacy fields written next to it are only the fallback
        // an older build shows: the nearest triple of the variant's own color, not the
        // selection's possibly stale triple or bands. The triple is also recorded in the
        // variant table, so a reader can tell when an older build changed the top-level one.
        let fallback = legacy_triple_for_variant(choice, selection.name.as_deref());
        table
            .with_body_color_override(Some(body_color_to_table(fallback)))
            .with_color_variant(Some(color_variant_to_table(choice, fallback)))
    } else {
        table
            .with_body_color_override(selection.body_color_override.map(body_color_to_table))
            .with_body_color_bands_override(
                selection
                    .body_color_bands_override
                    .as_ref()
                    .filter(|b| !b.is_empty())
                    .map(|rows| rows.iter().copied().map(body_color_to_table).collect()),
                selection
                    .absorption_path_scale_override
                    .map(f32_to_f64_short),
            )
    };
    table.color_pattern_override = selection.color_pattern_override.as_ref().map(|pattern| {
        // The pattern has no non-string map keys or fallible custom serializer.
        serde_json::to_string(pattern).expect("ColorPattern is JSON serializable")
    });
    table
}

/// `f32` -> `f64` through the shortest round-trip decimal (see [`body_color_to_table`]).
fn f32_to_f64_short(v: f32) -> f64 {
    v.to_string()
        .parse::<f64>()
        .unwrap_or_else(|_| f64::from(v))
}

/// The legacy `body_color_override` triple an older build shows for `choice`: the nearest
/// triple ([`nearest_legacy_rgb`]) to the variant's face-up D65 color.
///
/// The face-up color is the tensor over [`face_up_swatch_path`] for `(unit, reference_path_mm)`.
/// That path is a stone as wide as the reference, never a light path. It is seen through the
/// default principal frame of the built-in host `host_name` (see [`face_up_swatch`]). A host
/// that is not a built-in has no frame, so the unpolarised color is read. A non-positive or
/// non-finite width reads at the default stone width.
fn legacy_triple_for_variant(choice: &VariantChoice, host_name: Option<&str>) -> [f32; 3] {
    let width = if choice.reference_path_mm.is_finite() && choice.reference_path_mm > 0.0 {
        choice.reference_path_mm
    } else {
        0.0
    };
    let host = host_name.and_then(|name| {
        GemMaterial::built_in()
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
    });
    let color = host.map_or_else(
        || {
            let path = face_up_swatch_path(choice.unit, width);
            face_up_body_color(&choice.absorption, None, path, Illuminant::D65)
        },
        |host| face_up_swatch(&choice.absorption, choice.unit, f64::from(width), host),
    );
    nearest_legacy_rgb(color.lab)
}

/// `choice` as its on-disk `[material.color_variant]` table, with `fallback` (the legacy triple
/// written next to it) recorded as its `fallback_rgb`. Every `f32` goes through its shortest
/// round-trip decimal, so the file reads `peak = 0.35` and loads back bit for bit.
fn color_variant_to_table(choice: &VariantChoice, fallback: [f32; 3]) -> ColorVariantTable {
    let rows = VariantTensorRows::from_tensor(&choice.absorption);
    let bands = |rows: &[VariantBandRow]| -> Vec<ColorVariantBandTable> {
        rows.iter().map(variant_band_to_table).collect()
    };
    ColorVariantTable {
        key: choice.key.clone(),
        label: choice.label.clone(),
        unit: match choice.unit {
            AbsorptionUnit::PerMm => COLOR_VARIANT_UNIT_PER_MM,
            AbsorptionUnit::ModelUnit => COLOR_VARIANT_UNIT_MODEL_UNIT,
        }
        .to_owned(),
        reference_path_mm: f32_to_f64_short(choice.reference_path_mm),
        fallback_rgb: Some(body_color_to_table(fallback)),
        o_ray: bands(&rows.o_ray),
        e_ray: bands(&rows.e_ray),
        beta_ray: rows.beta_ray.as_deref().map(bands),
        pleochroic: rows.pleochroic,
        unknown: toml::Table::new(),
    }
}

fn variant_band_to_table(row: &VariantBandRow) -> ColorVariantBandTable {
    ColorVariantBandTable {
        center_nm: f32_to_f64_short(row.center_nm),
        width_nm: f32_to_f64_short(row.width_nm),
        peak: f32_to_f64_short(row.peak),
        shape: match row.shape {
            VariantBandShape::Wavelength => COLOR_VARIANT_SHAPE_WAVELENGTH,
            VariantBandShape::Energy => COLOR_VARIANT_SHAPE_ENERGY,
        }
        .to_owned(),
    }
}

/// The first thing in `table` that a newer build may have introduced and this build cannot read:
/// a unit or a band shape whose name it does not know.
fn unknown_variant_name(table: &ColorVariantTable) -> Option<String> {
    if !matches!(
        table.unit.as_str(),
        COLOR_VARIANT_UNIT_PER_MM | COLOR_VARIANT_UNIT_MODEL_UNIT
    ) {
        return Some(format!("the unit '{}'", table.unit));
    }
    let rays = [
        ("o_ray", Some(&table.o_ray)),
        ("e_ray", Some(&table.e_ray)),
        ("beta_ray", table.beta_ray.as_ref()),
    ];
    rays.into_iter().find_map(|(ray, bands)| {
        bands?.iter().enumerate().find_map(|(index, band)| {
            (!matches!(
                band.shape.as_str(),
                COLOR_VARIANT_SHAPE_WAVELENGTH | COLOR_VARIANT_SHAPE_ENERGY
            ))
            .then(|| format!("{ray} band {}: the shape '{}'", index + 1, band.shape))
        })
    })
}

/// The inverse of [`color_variant_to_table`]. `None` when the table uses a unit or band shape
/// this build does not know. The design then opens with the legacy triple written next to the
/// table (a `warn!` says so), rather than refusing to open.
///
/// # Errors
///
/// [`pleochroma_formats::native::design::DesignFileError::InvalidField`] for
/// `material.color_variant` when a ray has more bands than the GPU holds
/// ([`MAX_VARIANT_BANDS`]), a value is not finite, a band's width is not positive or its peak is
/// negative, or the reference path is not positive.
fn color_variant_from_table(
    table: &ColorVariantTable,
) -> Result<Option<VariantChoice>, pleochroma_formats::native::design::DesignFileError> {
    if let Some(unknown) = unknown_variant_name(table) {
        tracing::warn!(
            label = %table.label,
            "the color variant uses {unknown}, which this version does not know: \
             the design opens with the legacy color written next to it"
        );
        return Ok(None);
    }
    let unit = match table.unit.as_str() {
        COLOR_VARIANT_UNIT_PER_MM => AbsorptionUnit::PerMm,
        COLOR_VARIANT_UNIT_MODEL_UNIT => AbsorptionUnit::ModelUnit,
        other => {
            return Err(color_variant_file_error(format!(
                "the unit '{other}' is not '{COLOR_VARIANT_UNIT_PER_MM}' or \
                 '{COLOR_VARIANT_UNIT_MODEL_UNIT}'"
            )));
        }
    };
    let reference_path_mm = narrow(table.reference_path_mm);
    if !reference_path_mm.is_finite() || reference_path_mm <= 0.0 {
        return Err(color_variant_file_error(format!(
            "the reference path {} mm is not a positive number",
            table.reference_path_mm
        )));
    }
    let rows = VariantTensorRows {
        o_ray: variant_rays_from_table("o_ray", &table.o_ray)?,
        e_ray: variant_rays_from_table("e_ray", &table.e_ray)?,
        beta_ray: table
            .beta_ray
            .as_deref()
            .map(|rows| variant_rays_from_table("beta_ray", rows))
            .transpose()?,
        pleochroic: table.pleochroic,
    };
    Ok(Some(VariantChoice {
        key: table.key.clone(),
        label: table.label.clone(),
        absorption: rows.to_tensor(),
        unit,
        reference_path_mm,
    }))
}

fn variant_rays_from_table(
    ray: &str,
    bands: &[ColorVariantBandTable],
) -> Result<Vec<VariantBandRow>, pleochroma_formats::native::design::DesignFileError> {
    if bands.len() > MAX_VARIANT_BANDS {
        // The GPU buffer holds `MAX_VARIANT_BANDS` per ray and drops the rest, so a longer table
        // would render differently on the CPU and the GPU.
        return Err(color_variant_file_error(format!(
            "{ray} has {} bands, at most {MAX_VARIANT_BANDS} are allowed",
            bands.len()
        )));
    }
    bands
        .iter()
        .enumerate()
        .map(|(index, band)| {
            let shape = match band.shape.as_str() {
                COLOR_VARIANT_SHAPE_WAVELENGTH => VariantBandShape::Wavelength,
                COLOR_VARIANT_SHAPE_ENERGY => VariantBandShape::Energy,
                other => {
                    return Err(color_variant_file_error(format!(
                        "{ray} band {}: the shape '{other}' is not \
                         '{COLOR_VARIANT_SHAPE_WAVELENGTH}' or '{COLOR_VARIANT_SHAPE_ENERGY}'",
                        index + 1
                    )));
                }
            };
            let row = VariantBandRow {
                center_nm: narrow(band.center_nm),
                width_nm: narrow(band.width_nm),
                peak: narrow(band.peak),
                shape,
            };
            let usable = row.center_nm.is_finite()
                && row.center_nm > 0.0
                && row.width_nm.is_finite()
                && row.width_nm > 0.0
                && row.peak.is_finite()
                && row.peak >= 0.0;
            if usable {
                Ok(row)
            } else {
                Err(color_variant_file_error(format!(
                    "{ray} band {}: the centre, width and peak must be finite, the centre and \
                     width positive and the peak not negative",
                    index + 1
                )))
            }
        })
        .collect()
}

const fn color_variant_file_error(
    reason: String,
) -> pleochroma_formats::native::design::DesignFileError {
    pleochroma_formats::native::design::DesignFileError::InvalidField {
        field: "material.color_variant",
        reason,
    }
}

/// Whether the variant table of `table` still matches the file's color. True when no fallback
/// triple is recorded in it (a file from before that field existed, where the variant wins), or
/// when the top-level `body_color_override` still equals the recorded triple as `f32` bits.
///
/// A different or removed top-level triple means an older build changed the color after the
/// variant was written. The cutter's edit then wins over the variant.
fn variant_is_current(table: &MaterialTable) -> bool {
    let Some(recorded) = table
        .color_variant
        .as_ref()
        .and_then(|variant| variant.fallback_rgb)
    else {
        return true;
    };
    let bits = |rgb: [f64; 3]| rgb.map(narrow).map(f32::to_bits);
    table
        .body_color_override
        .is_some_and(|top| bits(top) == bits(recorded))
}

pub(super) fn material_selection_from_table(
    table: &MaterialTable,
) -> Result<MaterialSelection, pleochroma_formats::native::design::DesignFileError> {
    let pattern = table
        .color_pattern_override
        .as_deref()
        .map(
            |text| -> Result<_, pleochroma_formats::native::design::DesignFileError> {
                let pattern =
                    crate::material::pattern::decode_pattern(text).map_err(pattern_file_error)?;
                crate::material::pattern::apply_color_pattern(
                    GemMaterial::diamond(),
                    &pattern,
                    7.0,
                )
                .map_err(pattern_file_error)?;
                Ok(pattern)
            },
        )
        .transpose()?;
    let variant = table
        .color_variant
        .as_ref()
        .map(color_variant_from_table)
        .transpose()?
        .flatten()
        .filter(|variant| {
            let current = variant_is_current(table);
            if !current {
                tracing::warn!(
                    label = %variant.label,
                    "the color variant was changed by an older version (the color next to it \
                     differs from the one recorded inside it): the design opens with that color"
                );
            }
            current
        });
    // A variant wins over the legacy fields written next to it, so they are dropped on load.
    let legacy = variant.is_none();
    Ok(MaterialSelection {
        name: table.name.clone(),
        specific_gravity_override: table.specific_gravity_override,
        refractive_index_override: table.refractive_index_override,
        body_color_override: table
            .body_color_override
            .filter(|_| legacy)
            .map(body_color_from_table),
        body_color_bands_override: table
            .body_color_bands_override
            .as_ref()
            .filter(|b| legacy && !b.is_empty())
            .map(|rows| rows.iter().copied().map(body_color_from_table).collect()),
        absorption_path_scale_override: table
            .body_color_bands_override
            .as_ref()
            .filter(|b| legacy && !b.is_empty())
            .and(table.absorption_path_scale_override)
            .map(narrow),
        color_pattern_override: pattern,
        color_variant_override: variant,
    })
}

fn pattern_file_error(
    error: impl std::fmt::Display,
) -> pleochroma_formats::native::design::DesignFileError {
    pleochroma_formats::native::design::DesignFileError::InvalidField {
        field: "material.color_pattern_override",
        reason: error.to_string(),
    }
}

/// A stored `f64` material value at the `f32` precision the renderer and `GemMaterial` use.
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the renderer's material precision; stored values are in the normal float range"
)]
pub(super) const fn narrow(value: f64) -> f32 {
    value as f32
}

/// `f32` triple -> the `f64` triple `MaterialTable` stores, each component through its
/// shortest round-trip decimal. A plain `f64::from(0.2f32)` would serialise as
/// `0.20000000298023224`, which a cutter would see for a color picked as "0.2". The decimal
/// identifies the `f32` uniquely, so [`body_color_from_table`] returns the identical bits.
fn body_color_to_table(rgb: [f32; 3]) -> [f64; 3] {
    rgb.map(|v| {
        v.to_string()
            .parse::<f64>()
            .unwrap_or_else(|_| f64::from(v))
    })
}

/// The inverse of [`body_color_to_table`]: nearest `f32` per component, which for a
/// value written by that function is the original `f32` exactly.
fn body_color_from_table(rgb: [f64; 3]) -> [f32; 3] {
    rgb.map(narrow)
}

/// Mirrors an [`ExternalProportions`] into its on-disk [`SourceTable`] row.
#[must_use]
pub(super) const fn source_table_from_proportions(props: &ExternalProportions) -> SourceTable {
    SourceTable {
        vol_w3: props.vol_w3,
        lw: props.lw,
        cw: props.cw,
        pw: props.pw,
        hw: props.hw,
    }
}

/// The inverse of [`source_table_from_proportions`].
#[must_use]
pub(super) const fn external_proportions_from_source(table: &SourceTable) -> ExternalProportions {
    ExternalProportions {
        vol_w3: table.vol_w3,
        lw: table.lw,
        cw: table.cw,
        pw: table.pw,
        hw: table.hw,
    }
}

/// Mirrors one editor tier into its on-disk row.
///
/// The imported meet and the `.asc` file's original `G` note ride along, so a reload can
/// restore the "Adopt Meet" action and re-export the file's real note text.
///
/// `note`, `cheater_offset_deg`, `tier_id` and `target` are this tier's entries from the
/// [`Design`] maps. They are keyed by position or by [`TierId`], so the caller looks each up
/// by the tier's position and passes it in.
///
/// `tier.indices` is written as authored, never shifted. The shift that
/// `Design::to_asc_schedule_from_solved_with_cheater_offsets` bakes into an exported `.asc`
/// must not be read back as the tier's base indices on the next load.
#[must_use]
pub(super) fn tier_table_from_tier(
    tier: &ConstraintTier,
    note: Option<String>,
    cheater_offset_deg: Option<f64>,
    tier_id: Option<TierId>,
    target: Option<TierTarget>,
) -> TierTable {
    TierTable::new(
        tier.name.clone(),
        native_meet_constraint_from(&tier.constraint),
        tier.detached.clone(),
    )
    .with_indices(Some(tier.indices.clone()))
    .with_imported_meet(tier.imported_meet.as_ref().map(native_meet_constraint_from))
    .with_original_notes(tier.original_notes.clone())
    .with_note(note)
    .with_cheater_offset_deg(cheater_offset_deg)
    .with_tier_id(tier_id.map(TierId::value))
    .with_target(target.map(native_tier_target_from))
}

/// Optional data a caller may attach to a save beyond a bare design.
///
/// Carries a custom material snapshot, a history trail and [`Self::custom_catalogue`]. It is
/// one type rather than extra positional parameters on [`super::catalogue_asc`] and
/// [`super::DesignExtras`]. [`super::catalogue_asc_extended`] takes it, and
/// `DesignExtras::from(&extras)` carries the snapshot and history into a design file. The
/// default has an empty `custom_catalogue`, which uses the built-in materials only (see
/// [`Design::effective_refractive_index`]).
#[derive(Debug, Clone, Default)]
pub struct SaveExtras<'a> {
    /// See [`CustomMaterialSnapshot`]. `Some` only when `design.material.name` names a custom
    /// material (not a built-in [`GemMaterial::by_name`] preset) that the caller's catalogue
    /// resolved. This crate has no catalogue to check that, so the caller must get it right.
    pub custom_material: Option<&'a CustomMaterialSnapshot>,
    /// A bounded, human-readable trail of edits to this design, oldest first (see
    /// [`crate::edit::History::description_log`]). An empty slice writes no `[history]` table
    /// (see [`pleochroma_formats::native::design::DesignFile::with_history`]).
    pub history_entries: &'a [String],
    /// The caller's resolved custom materials, for example the editor's catalogue. They are
    /// consulted via [`Design::effective_refractive_index_with`] for the `.asc`'s `I` line when
    /// `design.material.name` names a custom entry. An empty slice uses the built-ins only.
    pub custom_catalogue: &'a [GemMaterial],
}

/// How a native snapshot's color reads: which payload the file really carries.
#[derive(Debug, Clone, PartialEq)]
pub enum SnapshotColor {
    /// No recipe: the top-level `absorption_rgb` is the color (fantasy, or a file written
    /// before physics color existed).
    Fantasy,
    /// A recipe whose fallback color still matches the top-level `absorption_rgb`, so the
    /// recipe is the color.
    Physics(ColorMode),
    /// A recipe, but the top-level `absorption_rgb` differs from the fallback the DTO recorded:
    /// an older build edited the color. Treated as fantasy-edited; the caller asks the user
    /// whether to keep the physics recipe (the [`ColorMode`] is that recipe).
    EditedElsewhere(ColorMode),
}

/// Reads `snapshot`'s color payload: see [`SnapshotColor`]. An unreadable recipe JSON is
/// treated as [`SnapshotColor::Fantasy`] (the fallback color is still there).
#[must_use]
pub fn snapshot_color(snapshot: &CustomMaterialSnapshot) -> SnapshotColor {
    let Some(dto) = &snapshot.color_recipe else {
        return SnapshotColor::Fantasy;
    };
    let Some(mode) = ColorMode::from_json(&dto.recipe_json) else {
        return SnapshotColor::Fantasy;
    };
    // A fantasy-active material renders from the top-level color, so the parked recipe is
    // ignored.
    if !mode.is_physics() {
        return SnapshotColor::Fantasy;
    }
    // Compared as `f32`, the precision `with_body_color` stores them at.
    let recorded = dto.fallback_rgb.map(narrow);
    let top_level = snapshot.body_color().unwrap_or([0.0; 3]);
    #[expect(
        clippy::float_cmp,
        reason = "both sides are f32 values stored at the same precision; equality means the snapshot was not edited"
    )]
    let unchanged = top_level == recorded;
    if unchanged {
        SnapshotColor::Physics(mode)
    } else {
        SnapshotColor::EditedElsewhere(mode)
    }
}

/// The DTO recording `mode` for the native file; `fallback_rgb` is the exact value the caller
/// writes as the top-level `absorption_rgb` ([`ColorMode::fallback_rgb`]).
#[must_use]
pub fn color_recipe_dto(mode: &ColorMode) -> ColorRecipeDto {
    ColorRecipeDto {
        recipe_json: mode.to_json(),
        fallback_rgb: mode.fallback_rgb().map(|v| {
            v.to_string()
                .parse::<f64>()
                .unwrap_or_else(|_| f64::from(v))
        }),
    }
}

/// Reconstructs a session-local [`GemMaterial`] from a design file's custom-material snapshot.
///
/// Used for a material name that resolves against no catalogue this build has (see
/// [`CustomMaterialSnapshot`]). The caller registers the result under the design's
/// `material.name`, so the real optics survive instead of falling back to
/// [`GemMaterial::diamond`].
///
/// The body color is [`CustomMaterialSnapshot::absorption_rgb`]. A snapshot with none restores
/// colorless (`[0.0; 3]`). The crystal system and optical character come from the snapshot's
/// name strings when recognised, as a library row's do. An empty or unknown name keeps what
/// [`GemMaterial::new_custom`] infers from the sign of `birefringence_delta`. The exception is a
/// usable [`CustomMaterialSnapshot::biaxial_delta_beta_alpha`], which sets the biaxial character
/// by [`biaxial_character_from_deltas`], because the delta is only written for biaxial
/// materials. A parsed name still wins, and a delta that the rule rejects changes nothing.
///
/// A biaxial material saved without its delta restores with the biaxial character and no delta.
/// The biaxial-only physics (per-axis curves, second-axis data) stays off until the material is
/// saved again from a library row that has the delta.
///
/// Per-ray rows ([`CustomMaterialSnapshot::absorption_ray_bands_per_mm`]) that fit the optical
/// character color the stone per direction with [`with_library_ray_bands`], as a library row
/// does. They win over the isotropic rows.
///
/// A physics recipe renders from its stored `resolved_bands`, never a re-resolve. If an older
/// build edited the top-level color ([`SnapshotColor::EditedElsewhere`]), this restores the
/// edited color. Use [`gem_material_from_custom_snapshot_keeping_recipe`] to keep the recipe.
///
/// A usable [`CustomMaterialSnapshot::dispersion_model`] rebuilds the material from that curve
/// (`GemMaterial::new_custom_with_dispersion`). Without one, or when
/// [`pleochroma::optics::dispersion::DispersionModel::validate`] refuses it, the Cauchy fit of
/// `mean_ri` and `dispersion_delta` is used.
#[must_use]
pub fn gem_material_from_custom_snapshot(
    name: &str,
    snapshot: &CustomMaterialSnapshot,
) -> GemMaterial {
    let birefringence = narrow(snapshot.birefringence_delta);
    let color = snapshot.body_color().unwrap_or([0.0, 0.0, 0.0]);
    let mut mat = snapshot
        .dispersion_model
        .as_ref()
        .and_then(super::dispersion_dto::dispersion_model_from_dto)
        .map_or_else(
            || {
                GemMaterial::new_custom(
                    name,
                    narrow(snapshot.mean_ri),
                    narrow(snapshot.dispersion_delta),
                    birefringence,
                    color,
                )
            },
            |model| GemMaterial::new_custom_with_dispersion(name, model, birefringence, color),
        );
    // Names win over `new_custom`'s inference, as in a library row
    // (`LibraryMaterial::gem_material`). Without a usable name, a biaxial delta picks the
    // biaxial character.
    if let Some(system) = crystal_system_from_name(&snapshot.crystal_system) {
        mat.crystal_system = system;
    }
    let named = optical_character_from_name(&snapshot.optical_character);
    let from_delta = || {
        snapshot
            .biaxial_delta_beta_alpha
            .and_then(|delta| biaxial_character_from_deltas(delta, snapshot.birefringence_delta))
    };
    if let Some(character) = named.or_else(from_delta) {
        mat.optical_character = character;
    }
    mat.biaxial_delta_beta_alpha = if is_biaxial(mat.optical_character) {
        snapshot.biaxial_delta_beta_alpha.map(narrow)
    } else {
        None
    };
    // Per-ray rows win over the isotropic rows, as in a library row, but only when they fit the
    // optical character. A table that does not fit is ignored, never applied with the wrong
    // number of rays. The bands are per millimetre, and the render setup picks the real scale.
    let rays = snapshot
        .absorption_ray_bands_per_mm
        .as_ref()
        .and_then(RayBandRows::from_table)
        .filter(|rays| rays.fits(mat.optical_character));
    if let Some(rays) = rays {
        mat = with_library_ray_bands(mat, &rays);
    } else if let Some(rows) = snapshot.absorption_bands() {
        mat = with_library_bands(mat, &rows);
    }
    if let SnapshotColor::Physics(mode) = snapshot_color(snapshot) {
        mat = mat.with_chromophore_absorption(mode.resolve_tensor());
    }
    // The default crystal axis, when the file carries both angles. It is baked in as a library
    // row bakes it, so the axis renders without the row.
    let (tilt, azimuth, second) = snapshot.default_orientation();
    let orientation = tilt.zip(azimuth).map(|(tilt, azimuth)| {
        crate::crystal_axis::CrystalOrientation::from_angles(
            f64::from(tilt),
            f64::from(azimuth),
            second.map(f64::from),
        )
    });
    let mat = crate::material::with_default_orientation(mat, orientation);
    // The render-physics data is applied last, as a library row applies it: the per-axis curves,
    // the inclusion families in the material's frame and a named structure. Without them, the
    // material is unchanged.
    super::axis_curves::with_snapshot_physics(mat, snapshot, orientation)
}

/// Like [`gem_material_from_custom_snapshot`], but a recipe wins even when an older build
/// edited the top-level color (the user's "keep physics recipe" choice).
#[must_use]
pub fn gem_material_from_custom_snapshot_keeping_recipe(
    name: &str,
    snapshot: &CustomMaterialSnapshot,
) -> GemMaterial {
    let mut mat = gem_material_from_custom_snapshot(name, snapshot);
    if let SnapshotColor::EditedElsewhere(mode) = snapshot_color(snapshot) {
        mat = mat.with_chromophore_absorption(mode.resolve_tensor());
    }
    mat
}

/// A concave tier as its file record: the tool and motion become the standard's
/// own strings, everything else is copied, so the conversion is lossless in both
/// directions (see [`concave_tier_from_table`]).
#[must_use]
pub(super) fn concave_tier_table_from_tier(tier: &ConcaveTier) -> ConcaveTierTable {
    ConcaveTierTable {
        name: tier.name.clone(),
        angle_deg: tier.angle_deg,
        indices: tier.indices.clone(),
        instructions: tier.instructions.clone(),
        tool: tier.tool.code().to_owned(),
        tool_azimuth_deg: tier.tool_azimuth_deg,
        displacement: tier.displacement,
        diameter_ratio: tier.diameter_ratio,
        tool_angle_deg: tier.tool_angle_deg,
        motion: tier.motion.word().to_owned(),
        // The id is the design's, not the tier's: `concave_tier_tables` attaches it.
        concave_tier_id: None,
        unknown: toml::Table::new(),
    }
}

/// The inverse of [`concave_tier_table_from_tier`]. The error is the reason a
/// human can act on: a tool code or motion this build does not know is refused
/// rather than replaced by a default, since a guessed tool cuts a different stone.
/// Range checks (angles, indices, diameter) are the caller's
/// [`Design::validate_concave_tiers`], which needs the design's gear.
pub(super) fn concave_tier_from_table(table: ConcaveTierTable) -> Result<ConcaveTier, String> {
    let tool = table
        .tool
        .parse::<ConcaveTool>()
        .map_err(|e| e.to_string())?;
    let motion = ToolMotion::from_word(&table.motion).ok_or_else(|| {
        format!(
            "unknown tool motion {:?} (expected reciprocating or plunge)",
            table.motion
        )
    })?;
    Ok(ConcaveTier {
        name: table.name,
        angle_deg: table.angle_deg,
        indices: table.indices,
        instructions: table.instructions,
        tool,
        tool_azimuth_deg: table.tool_azimuth_deg,
        displacement: table.displacement,
        diameter_ratio: table.diameter_ratio,
        tool_angle_deg: table.tool_angle_deg,
        motion,
    })
}

/// Every concave tier of `design` as file records, in stored order, each carrying its
/// stable id (the cutting-mode marks of a concave step hang on it, so it must survive a
/// reopen). A tier the design has no id for yet is written without one and gets a fresh
/// id on load.
#[must_use]
pub(super) fn concave_tier_tables(design: &Design) -> Vec<ConcaveTierTable> {
    design
        .concave_tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| ConcaveTierTable {
            concave_tier_id: design
                .concave_tier_ids
                .get(index)
                .copied()
                .map(TierId::value),
            ..concave_tier_table_from_tier(tier)
        })
        .collect()
}
