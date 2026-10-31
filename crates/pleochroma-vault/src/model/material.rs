/// A user-defined custom gemstone material, exactly as stored in the
/// `custom_gem_materials` table.
///
/// A plain data row owned by `pleochroma-vault` -- deliberately does NOT depend on
/// `pleochroma`'s richer `GemMaterial` type, so this crate stays independently
/// publishable. Callers that need a `GemMaterial` convert this row via
/// `GemMaterial::new_custom(..)`, then apply `crystal_system`/`optical_character`/
/// `biaxial_delta_beta_alpha` on top.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomMaterialRow {
    /// Display name.
    pub name: String,
    /// Refractive index.
    pub refractive_index: f32,
    /// Dispersion value.
    pub dispersion: f32,
    /// Birefringence value.
    pub birefringence: f32,
    /// Per-channel absorption coefficients.
    pub absorption_rgb: [f32; 3],
    /// `pleochroma::optics::materials::CrystalSystem`'s variant name as plain text (e.g.
    /// `"Trigonal"`), or `None`. Stored as a string, not the enum, so this crate still
    /// doesn't depend on `pleochroma` -- `apps/pleochroma-cut` parses it back. `None`
    /// covers both "saved before this field existed" and "author never overrode it";
    /// either way the caller falls back to what `GemMaterial::new_custom(..)` infers
    /// from `birefringence` alone.
    pub crystal_system: Option<String>,
    /// `pleochroma::optics::materials::OpticalCharacter`'s variant name as plain text
    /// (e.g. `"UniaxialNegative"`), or `None`. Same string-not-enum reasoning and
    /// fallback contract as `crystal_system` above.
    pub optical_character: Option<String>,
    /// `pleochroma::optics::materials::GemMaterial::biaxial_delta_beta_alpha` --
    /// `n_beta - n_alpha` at the sodium D line -- carried straight through as `f32`
    /// since it's already a plain number. `None` for every isotropic/uniaxial material
    /// and for any row saved before biaxial authoring existed; only meaningful when
    /// `optical_character` is one of the two biaxial variants.
    pub biaxial_delta_beta_alpha: Option<f32>,
    /// `pleochroma`'s optional per-axis dispersion coefficients (the extraordinary curve of a
    /// uniaxial material, `GemMaterial::uniaxial_extraordinary_dispersion`, or the `n_alpha` and
    /// `n_gamma` curves of a biaxial one, `GemMaterial::biaxial_principal_dispersion`), stored
    /// as a JSON string -- same string-not-enum boundary reasoning as
    /// `crystal_system`/`optical_character`. The text is `{"e": <model>}` or
    /// `{"alpha": <model>, "gamma": <model>}`, each `<model>` the same table a
    /// `dispersion_model_json` holds. `None` covers the same two cases those fields do; the
    /// caller falls back to the material's ordinary constant-`birefringence_delta` behaviour.
    pub per_axis_dispersion_json: Option<String>,
    /// The material's specific gravity (density relative to water), or `None` for
    /// a row saved before this field existed or an author who never typed one.
    /// With this `None`, carat-weight estimates for a custom material stay empty
    /// unless the cutter separately overrides SG per design. A caller wanting a
    /// `GemMaterial`-side fallback should treat `None`
    /// the same way `crystal_system`/`optical_character` do -- infer or leave blank,
    /// never guess a number.
    pub specific_gravity: Option<f32>,
    /// Optional serialized `ColorRecipe` (as JSON) for physically based chromophore colors.
    /// `None` for fantasy materials.
    pub color_recipe_json: Option<String>,
    /// The material's dispersion curve as coefficients (a Sellmeier with one or three terms,
    /// or a Cauchy fit), as the JSON text the desktop app writes; opaque to this crate, which
    /// must not depend on `pleochroma`. `None` is the plain path (a row saved before this
    /// field existed, or one whose author typed a refractive index and an `n_F - n_C` figure):
    /// the caller builds the Cauchy fit of [`Self::refractive_index`] and [`Self::dispersion`]
    /// exactly as before. With a model present those two scalars still hold its `n_d` and
    /// `n_F - n_C`.
    pub dispersion_model_json: Option<String>,
    /// The body colour as seven-band absorption rows (`[centre_nm, width_nm,
    /// amplitude_per_mm]`), as the JSON text the desktop app writes (the path-aware L*C*h
    /// editor's result); opaque to this crate. `None` is "no bands": a row saved before this
    /// field existed or one coloured from a preset or the three-band picker -- the caller then
    /// colours the material from [`Self::absorption_rgb`] as before. With bands present
    /// `absorption_rgb` still holds the nearest legacy triple, so an older build shows a close
    /// colour.
    pub absorption_bands_json: Option<String>,
    /// The default tilt (degrees, 0 to 90) of the material's crystal axis from the stone's
    /// vertical, with [`Self::c_axis_azimuth_deg`]. `None` (both unset) is the material's
    /// built-in axis. A row saved before this field existed reads `None`.
    pub c_axis_tilt_deg: Option<f32>,
    /// The default azimuth (degrees, 0 to 360) of the crystal axis; see
    /// [`Self::c_axis_tilt_deg`].
    pub c_axis_azimuth_deg: Option<f32>,
    /// The default Second Axis turn (degrees, 0 to 180) of a biaxial material; `None` leaves the
    /// second direction at the built-in completion.
    pub c_axis_second_azimuth_deg: Option<f32>,
    /// The material's oriented inclusion families (silk, needles, platelets) in the crystal
    /// frame, as the JSON text the desktop app writes; opaque to this crate. `None` is "no
    /// inclusions": a row saved before this field existed or one whose author set none, which
    /// the caller builds exactly as before.
    pub inclusion_families_json: Option<String>,
    /// The material's play-of-color (domain structure) choice, as the JSON text the desktop app
    /// writes; opaque to this crate. `None` is "no structure", as for
    /// [`Self::inclusion_families_json`].
    pub structure_json: Option<String>,
}
