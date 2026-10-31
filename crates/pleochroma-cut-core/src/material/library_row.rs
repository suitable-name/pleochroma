//! A custom material of the design library as plain values ([`LibraryMaterial`]), and the two
//! things every front end does with it: build the [`GemMaterial`] it describes
//! ([`LibraryMaterial::gem_material`]) and the [`CustomMaterialSnapshot`] a saved design file
//! carries for it ([`LibraryMaterial::snapshot`]).
//!
//! `pleochroma-vault`'s `CustomMaterialRow` holds the same data, but that crate must not depend
//! on `pleochroma` and this crate must not depend on a database. So the caller copies the row's
//! fields one for one into a [`LibraryMaterial`] and everything past that point is shared: the
//! desktop editor and the command line build the same stone from the same row.
//!
//! The crystal system and optical character are stored as the variant's `Debug` name (for
//! example `"Trigonal"`); [`crystal_system_name`] and [`crystal_system_from_name`] (and the two
//! for the optical character) are the one place that spelling is written down.

use super::{
    ColorMode, CustomStructure, PerAxisDispersion, custom_structure_from_json,
    custom_structure_to_json, inclusion_families_from_json, inclusion_families_to_json,
    pattern::{bands_from_rows, rows_from_bands},
    per_axis_dispersion_from_json, structure_spec_from_json, with_custom_physics,
    with_custom_structure,
};
use crate::{
    crystal_axis::CrystalOrientation,
    native::{
        CustomMaterialSnapshot, RayBandsTable, color_recipe_dto, dispersion_dto_from_json,
        dispersion_model_dto, dispersion_model_from_json,
    },
};
use pleochroma::optics::{
    absorption::AbsorptionTensor,
    inclusions::{Family, bragg::StructureSpec},
    materials::{AbsorptionUnit, CrystalSystem, GemMaterial, OpticalCharacter},
};

/// Every crystal system, for reading a stored name back.
const CRYSTAL_SYSTEMS: [CrystalSystem; 7] = [
    CrystalSystem::Cubic,
    CrystalSystem::Tetragonal,
    CrystalSystem::Hexagonal,
    CrystalSystem::Trigonal,
    CrystalSystem::Orthorhombic,
    CrystalSystem::Monoclinic,
    CrystalSystem::Triclinic,
];

/// Every optical character, for reading a stored name back.
const OPTICAL_CHARACTERS: [OpticalCharacter; 5] = [
    OpticalCharacter::Isotropic,
    OpticalCharacter::UniaxialPositive,
    OpticalCharacter::UniaxialNegative,
    OpticalCharacter::BiaxialPositive,
    OpticalCharacter::BiaxialNegative,
];

/// A crystal system's `Debug` name, e.g. `"Trigonal"`: what a library row stores.
#[must_use]
pub const fn crystal_system_name(system: CrystalSystem) -> &'static str {
    match system {
        CrystalSystem::Cubic => "Cubic",
        CrystalSystem::Tetragonal => "Tetragonal",
        CrystalSystem::Hexagonal => "Hexagonal",
        CrystalSystem::Trigonal => "Trigonal",
        CrystalSystem::Orthorhombic => "Orthorhombic",
        CrystalSystem::Monoclinic => "Monoclinic",
        CrystalSystem::Triclinic => "Triclinic",
    }
}

/// Inverse of [`crystal_system_name`]. `None` for anything unrecognised.
///
/// A hand-edited database, or a variant a later build adds, gives such a name: the caller
/// treats that like an absent field and keeps what `GemMaterial::new_custom` infers.
#[must_use]
pub fn crystal_system_from_name(name: &str) -> Option<CrystalSystem> {
    CRYSTAL_SYSTEMS
        .iter()
        .copied()
        .find(|system| crystal_system_name(*system) == name)
}

/// An optical character's `Debug` name, e.g. `"UniaxialNegative"`: what a library row stores.
#[must_use]
pub const fn optical_character_name(character: OpticalCharacter) -> &'static str {
    match character {
        OpticalCharacter::Isotropic => "Isotropic",
        OpticalCharacter::UniaxialPositive => "UniaxialPositive",
        OpticalCharacter::UniaxialNegative => "UniaxialNegative",
        OpticalCharacter::BiaxialPositive => "BiaxialPositive",
        OpticalCharacter::BiaxialNegative => "BiaxialNegative",
    }
}

/// Inverse of [`optical_character_name`], with the same "unrecognised is `None`" contract as
/// [`crystal_system_from_name`].
#[must_use]
pub fn optical_character_from_name(name: &str) -> Option<OpticalCharacter> {
    OPTICAL_CHARACTERS
        .iter()
        .copied()
        .find(|character| optical_character_name(*character) == name)
}

/// Whether `character` is one of the two biaxial optical characters (the only ones a
/// `biaxial_delta_beta_alpha` means anything for).
#[must_use]
pub const fn is_biaxial(character: OpticalCharacter) -> bool {
    matches!(
        character,
        OpticalCharacter::BiaxialPositive | OpticalCharacter::BiaxialNegative
    )
}

/// The biaxial optical character a stored `n_beta - n_alpha` implies, by the optic-sign rule.
///
/// A biaxial crystal has three principal indices `alpha < beta < gamma`. It is optically
/// POSITIVE when `beta` lies nearer `alpha` (`gamma` is then the acute bisectrix) and NEGATIVE
/// when `beta` lies nearer `gamma`. With `delta_beta_alpha = beta - alpha` and
/// `birefringence = gamma - alpha` (the full spread, as `GemMaterial::birefringence_delta`
/// holds it for a biaxial material) that reads
///
/// ```text
/// positive:  beta - alpha < gamma - beta   <=>   2 * delta_beta_alpha < birefringence
/// negative:  otherwise
/// ```
///
/// The boundary `2 * delta_beta_alpha == birefringence` (`beta` exactly halfway) is NEGATIVE:
/// positive needs `beta` strictly nearer `alpha`. The sign of `birefringence` is ignored (the
/// uniaxial inference of `GemMaterial::new_custom` reads it signed, a biaxial spread is a
/// magnitude), so its absolute value stands for `gamma - alpha`.
///
/// `None` when the pair cannot describe a biaxial material: a delta that is not finite, zero or
/// negative, a birefringence that is not finite, or a delta larger than the birefringence
/// (`beta` would lie beyond `gamma`). A delta equal to the birefringence (`beta` on `gamma`) is
/// still NEGATIVE, by the rule above. The caller then keeps the character it already has,
/// exactly as before this rule existed.
///
/// The library row and the file snapshot both call this when a stored delta comes with no
/// readable optical-character name (rows written before the names were stored, hand-edited
/// files, the web protocol's custom-material spec): the delta is only ever written for a
/// biaxial material, so its presence is evidence of one. A name that parses always wins.
#[must_use]
pub const fn biaxial_character_from_deltas(
    delta_beta_alpha: f64,
    birefringence: f64,
) -> Option<OpticalCharacter> {
    let gamma_minus_alpha = birefringence.abs();
    let usable = delta_beta_alpha.is_finite()
        && gamma_minus_alpha.is_finite()
        && delta_beta_alpha > 0.0
        && delta_beta_alpha <= gamma_minus_alpha;
    if !usable {
        return None;
    }
    Some(if 2.0 * delta_beta_alpha < gamma_minus_alpha {
        OpticalCharacter::BiaxialPositive
    } else {
        OpticalCharacter::BiaxialNegative
    })
}

/// One custom material of the design library, as plain values: the fields of
/// `pleochroma_vault::model::material::CustomMaterialRow` that describe the material, copied
/// one for one by the caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LibraryMaterial<'a> {
    /// The material's name.
    pub name: &'a str,
    /// The refractive index (the `n_d` of the model when `dispersion_model_json` is set).
    pub refractive_index: f32,
    /// The `n_F - n_C` figure (the model's own when `dispersion_model_json` is set).
    pub dispersion: f32,
    /// The birefringence.
    pub birefringence: f32,
    /// The body colour as an absorption triple.
    pub absorption_rgb: [f32; 3],
    /// The stored [`crystal_system_name`], or `None` for "infer it from the birefringence".
    pub crystal_system: Option<&'a str>,
    /// The stored [`optical_character_name`], or `None` for "infer it from the birefringence".
    pub optical_character: Option<&'a str>,
    /// `n_beta - n_alpha` of a biaxial material, or `None`.
    pub biaxial_delta_beta_alpha: Option<f32>,
    /// The specific gravity, or `None` when none was recorded.
    pub specific_gravity: Option<f32>,
    /// The JSON of the material's [`ColorMode`] (fantasy payload and physics recipe), or
    /// `None`.
    pub color_recipe_json: Option<&'a str>,
    /// The JSON of the dispersion model (`dispersion_model_to_json`), or `None` for the plain
    /// refractive-index-and-dispersion path.
    pub dispersion_model_json: Option<&'a str>,
    /// The JSON of the seven-band body colour ([`absorption_bands_to_json`]), or `None` for "no
    /// bands": the material is then coloured from [`Self::absorption_rgb`] alone.
    pub absorption_bands_json: Option<&'a str>,
    /// The default tilt of the material's crystal axis from the stone's vertical, in degrees
    /// (0 to 90), or `None`. Read with [`Self::c_axis_azimuth_deg`] by
    /// [`Self::default_orientation`].
    pub c_axis_tilt_deg: Option<f32>,
    /// The default azimuth of the material's crystal axis, in degrees (0 to 360), or `None`.
    pub c_axis_azimuth_deg: Option<f32>,
    /// The default Second Axis turn of a biaxial material, in degrees (0 to 180), or `None`.
    pub c_axis_second_azimuth_deg: Option<f32>,
    /// The JSON of the per-axis dispersion curves (`per_axis_dispersion_to_json`), or `None` for
    /// the constant birefringence offset.
    pub per_axis_dispersion_json: Option<&'a str>,
    /// The JSON of the oriented inclusion families (`inclusion_families_to_json`), or `None`
    /// for "no inclusions".
    pub inclusion_families_json: Option<&'a str>,
    /// The JSON of the play-of-color choice (`structure_spec_to_json`), or `None` for "no
    /// structure".
    pub structure_json: Option<&'a str>,
}

/// Stone width (mm) assumed for a banded custom material when nothing sets the real one: the
/// physics colour's default. The bands are per millimetre, the stone is about two model units
/// wide, so the path scale is half of this. A design with a girdle diameter overrides it.
const BANDED_DEFAULT_STONE_WIDTH_MM: f32 = 7.0;

/// `material` coloured by the body-colour band rows of a library row (the one place the path
/// scale of a banded custom material is chosen, shared by the editor's save and by loading).
#[must_use]
pub fn with_library_bands(material: GemMaterial, rows: &[[f32; 3]]) -> GemMaterial {
    material.with_body_color_bands(rows, BANDED_DEFAULT_STONE_WIDTH_MM / 2.0)
}

/// `material` with a stored default crystal orientation baked into its own axes: the Crystal
/// Axis becomes `c_axis`, and the Second Axis of a biaxial material becomes `alpha_axis`.
///
/// This is the one place a custom material's default axis (precedence step 2: the design's own
/// setting beats it, the built-in's axis loses to it) reaches the stone, so every consumer that
/// holds the material (the live render, thumbnails, tilt, scoring, the web and the command
/// line) sees it without a lookup of its own. `None` and an isotropic material are returned
/// untouched, so a material without a default is byte-identical to what it was.
#[must_use]
pub fn with_default_orientation(
    mut material: GemMaterial,
    orientation: Option<CrystalOrientation>,
) -> GemMaterial {
    let Some(orientation) = orientation else {
        return material;
    };
    if material.optical_character == OpticalCharacter::Isotropic {
        return material;
    }
    let (gamma, alpha) = orientation.to_override();
    material.c_axis = gamma;
    if material.biaxial_delta_beta_alpha.is_some() {
        material.alpha_axis = alpha;
    }
    material
}

/// The JSON text a library row stores for body-colour band rows
/// (`[centre_nm, width_nm, amplitude_per_mm]` each), or `None` for no rows.
///
/// Callers pass rows produced by `pattern::rows_from_bands` or the seven-band solver, which keep
/// only bands whose row decodes to the same shape. The domain of a stored row is inferred from
/// its width (above 500 is an energy band, otherwise a wavelength band), a legacy encoding; a
/// row that would change shape under that rule is a caller bug (a versioned representation is
/// pending, owner decision D5).
#[must_use]
pub fn absorption_bands_to_json(rows: &[[f32; 3]]) -> Option<String> {
    // A row carries no shape tag, so the shape check itself lives in `rows_from_bands`; here a
    // row must at least have a width the `> 500` rule can decide on.
    debug_assert!(
        rows.iter().all(|r| r[1].is_finite() && r[1] > 0.0),
        "band rows must have a finite positive width so the `> 500` rule decides their shape"
    );
    (!rows.is_empty())
        .then(|| serde_json::to_string(rows).ok())
        .flatten()
}

/// Inverse of [`absorption_bands_to_json`].
///
/// `None` for blank, malformed or empty text and for rows with a non-finite number or a
/// non-positive centre, width or amplitude (a centre of zero would divide by zero in an energy
/// band): the caller then colours the material from its legacy triple, exactly as before the
/// column existed. The same rule as `GemMaterial::with_body_color_bands` and `usable` in
/// `pattern/convert.rs`; a test keeps the three in lock-step.
#[must_use]
pub fn absorption_bands_from_json(text: &str) -> Option<Vec<[f32; 3]>> {
    let rows: Vec<[f32; 3]> = serde_json::from_str(text.trim()).ok()?;
    let usable = !rows.is_empty()
        && rows
            .iter()
            .all(|r| r.iter().all(|v| v.is_finite()) && r[0] > 0.0 && r[1] > 0.0 && r[2] > 0.0);
    usable.then_some(rows)
}

/// The per-ray body-color rows of a custom material whose color depends on the direction.
///
/// One `[centre_nm, width_nm, amplitude_per_mm]` list per ray, the rows of the isotropic form (the
/// same width rule, see [`absorption_bands_to_json`]).
///
/// A uniaxial material has `o` and `e`; a biaxial one also has `beta` (alpha is `o`, gamma is `e`,
/// as everywhere else). Stored beside the isotropic form in the same column, see
/// [`absorption_ray_bands_to_json`].
#[derive(Debug, Clone, PartialEq)]
pub struct RayBandRows {
    /// The ordinary ray (alpha for a biaxial material).
    pub o: Vec<[f32; 3]>,
    /// The extraordinary ray (gamma for a biaxial material).
    pub e: Vec<[f32; 3]>,
    /// The beta direction of a biaxial material, `None` for a uniaxial one.
    pub beta: Option<Vec<[f32; 3]>>,
}

impl RayBandRows {
    /// Whether this is the three-direction form (a biaxial material).
    #[must_use]
    pub const fn is_biaxial(&self) -> bool {
        self.beta.is_some()
    }

    /// The per-millimetre tensor of the rows: `uniaxial` for two directions, `biaxial` for
    /// three. Rows that are not usable are dropped (the rule of `with_body_color_bands`).
    #[must_use]
    pub fn to_tensor(&self) -> AbsorptionTensor {
        let o = bands_from_rows(&self.o);
        let e = bands_from_rows(&self.e);
        match &self.beta {
            Some(beta) => AbsorptionTensor::biaxial(o, bands_from_rows(beta), e),
            None => AbsorptionTensor::uniaxial(o, e),
        }
    }

    /// The rows of a pleochroic tensor, `None` for an isotropic one or when a band could not be
    /// written as a row without changing its shape (`rows_from_bands`).
    #[must_use]
    pub fn from_tensor(tensor: &AbsorptionTensor) -> Option<Self> {
        if !tensor.is_pleochroic {
            return None;
        }
        let rows = |bands: &[pleochroma::optics::absorption::AbsorptionBand]| {
            let rows = rows_from_bands(bands);
            (rows.len() == bands.len()).then_some(rows)
        };
        Some(Self {
            o: rows(&tensor.o_ray)?,
            e: rows(&tensor.e_ray)?,
            beta: match &tensor.beta_ray {
                Some(beta) => Some(rows(beta)?),
                None => None,
            },
        })
    }

    /// The rows of a design file's `[material.custom.absorption_ray_bands_per_mm]` table, `None`
    /// when a ray has no usable rows (the rule of [`absorption_ray_bands_from_json`]: a stored
    /// table a hand edit emptied or broke is ignored, the material is then colored from its
    /// other rows or its triple).
    #[must_use]
    pub fn from_table(table: &RayBandsTable) -> Option<Self> {
        let rays = Self {
            o: table.o_rows(),
            e: table.e_rows(),
            beta: table.beta_rows(),
        };
        let beta_ok = rays.beta.as_deref().is_none_or(usable_rows);
        (usable_rows(&rays.o) && usable_rows(&rays.e) && beta_ok).then_some(rays)
    }

    /// The design-file table of these rows (the inverse of [`Self::from_table`]).
    #[must_use]
    pub fn to_table(&self) -> RayBandsTable {
        RayBandsTable::from_rows(&self.o, &self.e, self.beta.as_deref())
    }

    /// Whether this form suits a material of `character`: two directions for a uniaxial one,
    /// three for a biaxial one, never an isotropic one.
    #[must_use]
    pub const fn fits(&self, character: OpticalCharacter) -> bool {
        match character {
            OpticalCharacter::Isotropic => false,
            OpticalCharacter::UniaxialPositive | OpticalCharacter::UniaxialNegative => {
                !self.is_biaxial()
            }
            OpticalCharacter::BiaxialPositive | OpticalCharacter::BiaxialNegative => {
                self.is_biaxial()
            }
        }
    }
}

/// The JSON object of a stored [`RayBandRows`]: `{"o": [...], "e": [...], "beta": [...]}`. An older
/// build reads the column as a list of rows, finds an object, and colors the material from its
/// legacy triple, as before the column held anything.
#[derive(serde::Serialize, serde::Deserialize)]
struct RayBandsJson {
    o: Vec<[f32; 3]>,
    e: Vec<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    beta: Option<Vec<[f32; 3]>>,
}

/// Whether `rows` is a usable band list: not empty, every number finite, every centre, width and
/// amplitude positive (the rule of [`absorption_bands_from_json`]).
fn usable_rows(rows: &[[f32; 3]]) -> bool {
    !rows.is_empty()
        && rows
            .iter()
            .all(|r| r.iter().all(|v| v.is_finite()) && r[0] > 0.0 && r[1] > 0.0 && r[2] > 0.0)
}

/// The JSON text a library row stores for per-ray body-color rows, or `None` when a ray has no
/// usable rows (a colorless ray next to a colored one carries a negligible stand-in band, so no
/// ray is empty).
#[must_use]
pub fn absorption_ray_bands_to_json(rays: &RayBandRows) -> Option<String> {
    let beta_ok = rays.beta.as_deref().is_none_or(usable_rows);
    (usable_rows(&rays.o) && usable_rows(&rays.e) && beta_ok)
        .then(|| {
            serde_json::to_string(&RayBandsJson {
                o: rays.o.clone(),
                e: rays.e.clone(),
                beta: rays.beta.clone(),
            })
            .ok()
        })
        .flatten()
}

/// Inverse of [`absorption_ray_bands_to_json`].
///
/// `None` for blank or malformed text, a plain row list (the isotropic form, read by
/// [`absorption_bands_from_json`]) and any ray without usable rows. A row saved before per-ray
/// colors therefore reads as the isotropic color it always was.
#[must_use]
pub fn absorption_ray_bands_from_json(text: &str) -> Option<RayBandRows> {
    let json: RayBandsJson = serde_json::from_str(text.trim()).ok()?;
    let beta_ok = json.beta.as_deref().is_none_or(usable_rows);
    (usable_rows(&json.o) && usable_rows(&json.e) && beta_ok).then_some(RayBandRows {
        o: json.o,
        e: json.e,
        beta: json.beta,
    })
}

/// `material` coloured by the per-ray rows of a library row.
///
/// Only the absorption tensor and its unit change (per millimetre, with the same path scale as
/// [`with_library_bands`]); every other field, the material's pleochroic character included, is
/// kept. The caller checks [`RayBandRows::fits`] first.
#[must_use]
pub fn with_library_ray_bands(material: GemMaterial, rays: &RayBandRows) -> GemMaterial {
    let mut material = material.with_color_variant_tensor(rays.to_tensor(), AbsorptionUnit::PerMm);
    material.absorption_path_scale = BANDED_DEFAULT_STONE_WIDTH_MM / 2.0;
    material
}

impl LibraryMaterial<'_> {
    /// The [`GemMaterial`] this row describes.
    ///
    /// `GemMaterial::new_custom`'s own two-variant inference (cubic or trigonal, isotropic or
    /// uniaxial from the sign of the birefringence) is applied first; the crystal system,
    /// optical character and biaxial delta the row stores then replace what it inferred. A row
    /// saved before those fields existed has none of them, so it renders exactly as it always
    /// did.
    ///
    /// A row that stores a biaxial delta but no readable optical-character name (empty, `None`
    /// or unrecognised) takes its biaxial character from [`biaxial_character_from_deltas`]
    /// (the optic-sign rule on the delta and the birefringence), since the delta is only ever
    /// stored for a biaxial material; a delta that rule rejects leaves the inferred character.
    ///
    /// A row whose `dispersion_model_json` holds a usable Sellmeier or Cauchy model builds its
    /// curve with `GemMaterial::new_custom_with_dispersion`; a row without one (or with one that
    /// fails `DispersionModel::validate`) takes the `new_custom` path from `refractive_index`
    /// and `dispersion`. A stored physics recipe renders from its `resolved_bands` while physics
    /// is the active colour mode.
    ///
    /// Stored per-axis dispersion curves and inclusion families are applied last, through
    /// [`with_custom_physics`] (the same function a design file's snapshot uses); a row without
    /// them is exactly what it was before the columns existed.
    #[must_use]
    pub fn gem_material(&self) -> GemMaterial {
        let mut material = self
            .dispersion_model_json
            .and_then(dispersion_model_from_json)
            .map_or_else(
                || {
                    GemMaterial::new_custom(
                        self.name,
                        self.refractive_index,
                        self.dispersion,
                        self.birefringence,
                        self.absorption_rgb,
                    )
                },
                |model| {
                    GemMaterial::new_custom_with_dispersion(
                        self.name,
                        model,
                        self.birefringence,
                        self.absorption_rgb,
                    )
                },
            );
        if let Some(system) = self.crystal_system.and_then(crystal_system_from_name) {
            material.crystal_system = system;
        }
        // A name that parses wins; without one, the stored delta is evidence of a biaxial
        // material and the optic-sign rule picks which of the two.
        let named = self.optical_character.and_then(optical_character_from_name);
        let from_delta = || {
            self.biaxial_delta_beta_alpha.and_then(|delta| {
                biaxial_character_from_deltas(f64::from(delta), f64::from(self.birefringence))
            })
        };
        if let Some(character) = named.or_else(from_delta) {
            material.optical_character = character;
        }
        if let Some(delta) = self.biaxial_delta_beta_alpha {
            material.biaxial_delta_beta_alpha = Some(delta);
        }
        let physics = self
            .color_recipe_json
            .and_then(ColorMode::from_json)
            .filter(ColorMode::is_physics);
        if let Some(mode) = physics {
            // Physics bands are per millimetre (`with_chromophore_absorption` tags them so).
            material = material.with_chromophore_absorption(mode.resolve_tensor());
        } else if let Some(rays) = self
            .absorption_ray_bands()
            .filter(|rays| rays.fits(material.optical_character))
        {
            // Per-ray colour of a material whose colour depends on the direction. A row whose
            // rays no longer fit the material's optical character (it was edited afterwards)
            // is coloured from its triple below, never from the wrong number of directions.
            material = with_library_ray_bands(material, &rays);
        } else if let Some(rows) = self.absorption_bands() {
            // The seven-band colour replaces the triple's three bands (the triple stays what an
            // older build shows). The scale gives the bands per millimetre a physical stone;
            // a design with a girdle diameter overrides it at render time.
            material = with_library_bands(material, &rows);
        }
        let orientation = self.default_orientation();
        let material = with_default_orientation(material, orientation);
        let material = with_custom_physics(
            material,
            self.per_axis_dispersion(),
            self.inclusion_families().as_deref(),
            orientation,
        );
        with_custom_structure(material, self.custom_structure().as_ref())
    }

    /// The usable per-axis dispersion curves, if the row stores any.
    #[must_use]
    pub fn per_axis_dispersion(&self) -> Option<PerAxisDispersion> {
        self.per_axis_dispersion_json
            .and_then(per_axis_dispersion_from_json)
    }

    /// The usable inclusion families in the crystal frame, if the row stores any.
    #[must_use]
    pub fn inclusion_families(&self) -> Option<Vec<Family>> {
        self.inclusion_families_json
            .and_then(inclusion_families_from_json)
    }

    /// The usable play-of-color choice, if the row stores one.
    ///
    /// Applied by [`Self::gem_material`] only when the stored text names the kind of built-in
    /// structure the material imitates (see [`Self::custom_structure`]).
    #[must_use]
    pub fn structure_spec(&self) -> Option<StructureSpec> {
        self.structure_json.and_then(structure_spec_from_json)
    }

    /// The usable play-of-color choice with the structure kind it names, if the row stores one.
    #[must_use]
    pub fn custom_structure(&self) -> Option<CustomStructure> {
        self.structure_json.and_then(custom_structure_from_json)
    }

    /// The default crystal-axis orientation of this material: `Some` only when the tilt and the
    /// azimuth are both stored and finite (the Second Axis turn rides along when stored). `None`
    /// is the material's built-in axis, untouched. A built-in material has no library row, so it
    /// never has a default.
    #[must_use]
    pub fn default_orientation(&self) -> Option<CrystalOrientation> {
        let tilt = self.c_axis_tilt_deg.filter(|v| v.is_finite())?;
        let azimuth = self.c_axis_azimuth_deg.filter(|v| v.is_finite())?;
        let second = self.c_axis_second_azimuth_deg.filter(|v| v.is_finite());
        Some(CrystalOrientation::from_angles(
            f64::from(tilt),
            f64::from(azimuth),
            second.map(f64::from),
        ))
    }

    /// The usable body-colour band rows, if the row stores any (the isotropic form).
    #[must_use]
    pub fn absorption_bands(&self) -> Option<Vec<[f32; 3]>> {
        self.absorption_bands_json
            .and_then(absorption_bands_from_json)
    }

    /// The usable per-ray body-colour rows, if the row stores them instead (a material whose
    /// colour depends on the direction).
    #[must_use]
    pub fn absorption_ray_bands(&self) -> Option<RayBandRows> {
        self.absorption_bands_json
            .and_then(absorption_ray_bands_from_json)
    }

    /// The [`CustomMaterialSnapshot`] a design file carries for this material, so a design
    /// saved under a custom material does not reload as Diamond on a machine without it.
    ///
    /// The snapshot holds the numbers the cutter typed (refractive index, dispersion,
    /// birefringence, specific gravity) rather than anything re-derived from the resolved
    /// material, and the dispersion model when one is stored and valid. The crystal system and
    /// optical character go in as plain text, and the biaxial delta as a number; the open path
    /// (`gem_material_from_custom_snapshot`) restores all three, so a biaxial material keeps its
    /// crystal optics on a machine without the library row. An empty or unrecognised name
    /// leaves what the birefringence implies, unless a usable biaxial delta is stored with it
    /// ([`biaxial_character_from_deltas`] then picks the biaxial character on the open path).
    ///
    /// Both colour payloads travel: the top-level body colour, and, when the row holds a
    /// recipe, the typed [`ColorMode`] with its fallback colour recorded exactly as the
    /// top-level colour was written. An older build that edits the top-level colour later makes
    /// the two differ, which the open path detects. A per-ray colour (rows of a material whose
    /// colour depends on the direction) is written in its own table, next to the seven-band form
    /// a plain row would use.
    #[must_use]
    pub fn snapshot(&self) -> CustomMaterialSnapshot {
        let mut snapshot = CustomMaterialSnapshot::new(
            f64::from(self.refractive_index),
            f64::from(self.dispersion),
            f64::from(self.birefringence),
            self.specific_gravity.map(f64::from),
            self.crystal_system.unwrap_or_default(),
            self.optical_character.unwrap_or_default(),
        )
        .with_biaxial_delta(self.biaxial_delta_beta_alpha.map(f64::from))
        .with_body_color(Some(self.absorption_rgb))
        .with_dispersion_model(
            self.dispersion_model_json
                .and_then(dispersion_dto_from_json),
        );
        if let Some(mode) = self
            .color_recipe_json
            .and_then(ColorMode::from_json)
            .filter(|mode| mode.last_recipe.is_some())
        {
            let mut dto = color_recipe_dto(&mode);
            if let Some(top_level) = snapshot.absorption_rgb {
                dto.fallback_rgb = top_level;
            }
            snapshot = snapshot.with_color_recipe(Some(dto));
        }
        if let Some(rows) = self.absorption_bands() {
            snapshot = snapshot.with_absorption_bands(&rows);
        }
        // A per-ray color travels in its own table, so the stone is as pleochroic on a machine
        // without the library row as it is here (the open path applies it with
        // `with_library_ray_bands` when the rays fit the optical character).
        if let Some(rays) = self.absorption_ray_bands() {
            snapshot = snapshot.with_absorption_ray_bands(Some(rays.to_table()));
        }
        if self.c_axis_tilt_deg.is_some() && self.c_axis_azimuth_deg.is_some() {
            snapshot = snapshot.with_default_orientation(
                self.c_axis_tilt_deg,
                self.c_axis_azimuth_deg,
                self.c_axis_second_azimuth_deg,
            );
        }
        // The render-physics data travels as the canonical re-serialisation of what parsed, so
        // a stored text this build cannot use is not carried into the file.
        snapshot = match self.per_axis_dispersion() {
            Some(PerAxisDispersion::Uniaxial(e)) => {
                snapshot.with_axis_dispersion(Some(dispersion_model_dto(&e)), None, None)
            }
            Some(PerAxisDispersion::Biaxial([alpha, gamma])) => snapshot.with_axis_dispersion(
                None,
                Some(dispersion_model_dto(&alpha)),
                Some(dispersion_model_dto(&gamma)),
            ),
            None => snapshot,
        };
        snapshot.with_inclusion_families(
            self.inclusion_families()
                .and_then(|families| inclusion_families_to_json(&families)),
            self.custom_structure()
                .and_then(|structure| custom_structure_to_json(&structure)),
        )
    }
}

#[cfg(test)]
mod tests;
