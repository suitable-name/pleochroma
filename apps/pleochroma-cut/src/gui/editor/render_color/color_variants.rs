//! The saved-color actions of the design color editor: "Save as Variant...", "Update" for a
//! snapshot of a saved variant that changed, and "Nearest Variant" for a legacy color.
//!
//! * **Save as Variant**: the editor's per-direction colors are saved as a variant of the design's
//!   material in the vault (`gui::optics::color_variant_store`) and the design then uses it, as one
//!   undoable edit labelled `Sapphire (<Name>)`. A refusal (a blank name, a color the material does
//!   not accept, a vault failure) is shown as the error toast and nothing changes.
//! * **Update**: a design holds a snapshot of the saved variant it picked. When the variant was
//!   edited since, the Stone tab offers "Variant Changed: Update", which copies the stored color
//!   into the design again (one undoable edit).
//! * **Nearest Variant**: a legacy single color on a pleochroic material (an old file) can be
//!   replaced by the variant of the material whose face-up color is closest to it (CIEDE2000), the
//!   material's own color included.
//!
//! The decisions are pure functions ([`nearest_entry`], [`updated_selection`], [`persist`]) so they
//! are unit-testable without a window; [`setup`] wires them to the Slint callbacks.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
};

use pleochroma::{
    color::body_color::{Illuminant, body_colors, delta_e_2000, lch_to_lab},
    optics::{
        absorption::{AbsorptionTensor, legacy_rgb_bands},
        materials::{
            AbsorptionUnit, GemMaterial,
            variants::{
                ColorVariant, MATERIAL_DEFAULT_LABEL, VariantProvenance, built_in_variants,
                face_up_swatch, face_up_swatch_path,
            },
        },
    },
};
use pleochroma_cut_core::{
    MaterialSelection,
    material::{VariantChoice, color::swatch_path_units},
};
use pleochroma_editor::{
    lch_color::{self, LchSolve, axis_rows},
    material::{
        ColorEntry, legacy_color_on_pleochroic_host, listed_variants, selection_with_entry,
    },
};
use pleochroma_vault::db::sqlite::Database;
use slint::{ComponentHandle, SharedString};
use tracing::warn;

use super::{MaterialEdit, SharedAxes, lock_axes};
use crate::{
    BodyColorEditorModel, EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{body_color_combo::host_material, state::EditorState},
        optics::color_variant_store::{
            delete_user_color_variant, load_user_color_variant, save_user_color_variant,
            saved_variants, suggested_label,
        },
        rough_colour::material_zones::registered_vault,
        show_toast,
    },
};

/// The sentence for a design that cannot be read right now (an edit is in progress).
const BUSY: &str = "The design is busy right now. Try again in a moment.";

/// The sentence for a missing vault (the main window registers it at start).
const NO_VAULT: &str = "The saved colors are not available right now.";

/// The oriented face-up D65 lab color of `tensor` (in `unit`) on `host` in a stone
/// `stone_width_mm` wide (`variants::face_up_swatch`, the color the combo chip shows).
fn face_up_lab(
    host: &GemMaterial,
    tensor: &AbsorptionTensor,
    unit: AbsorptionUnit,
    stone_width_mm: f64,
) -> [f64; 3] {
    face_up_swatch(tensor, unit, stone_width_mm, host).lab
}

/// A stone width as the `f32` the face-up path functions take.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a stone width in millimetres is a small value; f32 is the precision the render uses"
)]
const fn width_f32(stone_width_mm: f64) -> f32 {
    stone_width_mm as f32
}

/// The entry of `host` whose face-up color is closest (CIEDE2000) to `legacy_lab`, with its label
/// and its distance.
///
/// The candidates are the material's own color and `variants` (the combo's built-in and saved
/// variants, [`listed_variants`]). Each is seen face-up in a stone `stone_width_mm` wide, in its
/// own unit ([`face_up_lab`]), the same stone the legacy color is read for ([`legacy_lab`]). `None`
/// only without any candidate, which never happens: the material's own color is always one.
fn nearest_entry(
    host: &GemMaterial,
    variants: &[ColorVariant],
    legacy_lab: [f64; 3],
    stone_width_mm: f64,
) -> Option<(ColorEntry, String, f64)> {
    let default = (
        ColorEntry::MaterialDefault,
        MATERIAL_DEFAULT_LABEL.to_owned(),
        face_up_lab(host, &host.absorption, host.absorption_unit, stone_width_mm),
    );
    let others = variants.iter().map(|variant| {
        (
            ColorEntry::Variant {
                key: variant.key.clone(),
            },
            variant.label.clone(),
            face_up_lab(host, &variant.absorption, variant.unit, stone_width_mm),
        )
    });
    let mut best: Option<(ColorEntry, String, f64)> = None;
    for (entry, label, lab) in std::iter::once(default).chain(others) {
        let distance = delta_e_2000(legacy_lab, lab);
        if best
            .as_ref()
            .is_none_or(|(_, _, nearest)| distance < *nearest)
        {
            best = Some((entry, label, distance));
        }
    }
    best
}

/// The face-up lab color of the design's legacy color in a stone `stone_width_mm` wide, at the
/// path the render shows it face-up: its band rows (per millimetre) at the per-millimetre face-up
/// path, else its triple (a per-model-unit color) at the width-scaled swatch path. `None` without
/// a legacy color.
///
/// It is read for the same stone as the candidates of [`nearest_entry`], so the nearest variant
/// does not change with the stone's size for the wrong reason.
fn legacy_lab(material: &MaterialSelection, stone_width_mm: f64) -> Option<[f64; 3]> {
    let width = width_f32(stone_width_mm);
    let has_bands = material
        .body_color_bands_override
        .as_ref()
        .is_some_and(|rows| !rows.is_empty());
    if has_bands {
        let path_mm = face_up_swatch_path(AbsorptionUnit::PerMm, width);
        return lch_color::lch_from_material(material, path_mm).map(lch_to_lab);
    }
    let triple = material.body_color_override?;
    let tensor = AbsorptionTensor::isotropic(legacy_rgb_bands(triple));
    Some(
        body_colors(&tensor, swatch_path_units(width), Illuminant::D65)
            .unpolarised
            .lab,
    )
}

/// `material` with the stored color of `stored` copied in, when `material` holds a snapshot of that
/// very variant; `None` when it does not (another key, no variant) or the snapshot is already
/// current (no undo step is added).
fn updated_selection(
    material: &MaterialSelection,
    stored: &ColorVariant,
) -> Option<MaterialSelection> {
    let held = material.color_variant_override.as_ref()?;
    if held.key != stored.key {
        return None;
    }
    let next = material
        .clone()
        .with_color_variant(Some(VariantChoice::from_variant(stored)));
    (next != *material).then_some(next)
}

/// Saves the editor's colors `choice` as a variant of `host` called `name` in `vault`, and returns
/// the choice a design stores for it: the new key, the trimmed name, the same tensor.
///
/// # Errors
///
/// The sentence to show the cutter: a blank name, a color the material does not accept, a vault
/// failure.
fn persist(
    vault: &Mutex<Database>,
    host: &GemMaterial,
    name: &str,
    choice: &VariantChoice,
) -> Result<VariantChoice, String> {
    let label = name.trim().to_owned();
    let variant = ColorVariant {
        key: String::new(),
        label: label.clone(),
        material: host.name.clone(),
        absorption: choice.absorption.clone(),
        unit: choice.unit,
        reference_path_mm: choice.reference_path_mm,
        provenance: VariantProvenance::User,
    };
    let key = {
        let guard = vault.lock().unwrap_or_else(PoisonError::into_inner);
        save_user_color_variant(&guard, &variant).map_err(|error| error.to_string())?
    };
    Ok(VariantChoice {
        key,
        label,
        ..choice.clone()
    })
}

/// The design's selection, its host material and the width of its stone (mm), or the sentence for
/// why they cannot be read now.
fn design_snapshot(
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> Result<(MaterialSelection, GemMaterial, f64), String> {
    let st = state.try_borrow().map_err(|_| BUSY.to_owned())?;
    let ctx = RenderContext::lock(render_ctx);
    let host = host_material(&st.design, &ctx.custom_materials);
    drop(ctx);
    let host = host.ok_or_else(|| "This design's material could not be resolved.".to_owned())?;
    let width = lch_color::stone_width_mm(st.design.girdle_diameter_mm);
    Ok((st.design.material.clone(), host, width))
}

/// The labels the host's variants already carry: the built-in ones and the saved ones.
fn taken_labels(host_name: &str) -> Vec<String> {
    built_in_variants(host_name)
        .into_iter()
        .chain(saved_variants(host_name))
        .map(|variant| variant.label)
        .collect()
}

/// Saves `choice` as a variant of `host` called `name` and hands the saved choice to `apply` (the
/// design edit). When `apply` refuses (the design is busy) the saved color is removed again, so
/// nothing is saved and a retry does not leave a second variant with the same name.
fn save_then_apply(
    vault: &Mutex<Database>,
    host: &GemMaterial,
    name: &str,
    choice: &VariantChoice,
    apply: impl FnOnce(VariantChoice) -> Result<(), String>,
) -> Result<(), String> {
    let chosen = persist(vault, host, name, choice)?;
    let key = chosen.key.clone();
    apply(chosen).inspect_err(|_| {
        let guard = vault.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(error) = delete_user_color_variant(&guard, &key) {
            warn!("The refused color '{key}' could not be removed again: {error:#}");
        }
    })
}

/// "Save as Variant": the editor's finished rows saved for the design's material, then used on
/// the design as one undoable edit. When the design refuses the edit nothing is saved
/// ([`save_then_apply`]).
fn save_design_variant(
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    edit: &MaterialEdit,
    axes: &SharedAxes,
    name: &str,
) -> Result<(), String> {
    let (host_known, rank, width, solves) = {
        let editor = lock_axes(axes);
        (
            editor.host_known,
            editor.rank,
            editor.stone_width_mm,
            editor.solves.clone(),
        )
    };
    if !host_known {
        return Err(
            "Choose this design's material first: a variant belongs to a material.".to_owned(),
        );
    }
    let solves: Vec<LchSolve> = solves.into_iter().flatten().collect();
    if solves.is_empty() || solves.len() != axis_rows(rank).len() {
        return Err("Wait for the color to finish matching, then save it.".to_owned());
    }
    let choice = lch_color::custom_choice(rank, &solves, width)
        .ok_or_else(|| "The colors of the directions could not be combined.".to_owned())?;
    let (_, host, _) = design_snapshot(state, render_ctx)?;
    let vault = registered_vault().ok_or_else(|| NO_VAULT.to_owned())?;
    save_then_apply(&vault, &host, name, &choice, |chosen| {
        edit(&|material| lch_color::recoloured_with_choice(material, chosen.clone()))
    })
}

/// "Update": the stored color of the saved variant the design holds a snapshot of, copied into the
/// design. Returns the variant's name.
fn update_stale_variant(
    state: &Rc<RefCell<EditorState>>,
    edit: &MaterialEdit,
) -> Result<String, String> {
    let key = state
        .try_borrow()
        .map_err(|_| BUSY.to_owned())?
        .design
        .material
        .color_variant_override
        .as_ref()
        .map(|choice| choice.key.clone())
        .ok_or_else(|| "This design does not use a saved color.".to_owned())?;
    let vault = registered_vault().ok_or_else(|| NO_VAULT.to_owned())?;
    let stored = {
        let guard = vault.lock().unwrap_or_else(PoisonError::into_inner);
        load_user_color_variant(&guard, &key).map_err(|error| error.to_string())?
    }
    .ok_or_else(|| "The saved color no longer exists. The design keeps its own copy.".to_owned())?;
    edit(&|material| updated_selection(material, &stored))?;
    Ok(stored.label)
}

/// "Nearest Variant": the legacy color of the design replaced by the nearest variant of its
/// material. Returns the label of the one used.
fn convert_to_nearest(
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    edit: &MaterialEdit,
) -> Result<String, String> {
    let (material, host, width) = design_snapshot(state, render_ctx)?;
    if !legacy_color_on_pleochroic_host(&material, &host) {
        return Err("This design has no single legacy color to replace.".to_owned());
    }
    let lab = legacy_lab(&material, width)
        .ok_or_else(|| "The legacy color of this design could not be read.".to_owned())?;
    let variants = listed_variants(&host, &saved_variants(&host.name));
    let (entry, label, _) = nearest_entry(&host, &variants, lab, width)
        .ok_or_else(|| "This material has no variant to choose from.".to_owned())?;
    edit(&|current| {
        let next = selection_with_entry(current.clone(), &entry, current, &variants);
        (next != *current).then_some(next)
    })?;
    Ok(label)
}

/// Registers "Save as Variant...", the suggested name, "Update" and "Nearest Variant".
pub(super) fn setup(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    edit: &MaterialEdit,
    axes: &SharedAxes,
) {
    let editor = ui.global::<BodyColorEditorModel>();

    // The name field starts on the coarse color of the first direction, unique among the labels
    // of the material.
    let (state_name, ctx_name, axes_name) =
        (Rc::clone(state), Arc::clone(render_ctx), Arc::clone(axes));
    editor.on_suggest_variant_name(move || {
        let families: Vec<&'static str> = lock_axes(&axes_name)
            .lch
            .iter()
            .map(|row| lch_color::color_family(*row))
            .collect();
        let taken = design_snapshot(&state_name, &ctx_name)
            .map(|(_, host, _)| taken_labels(&host.name))
            .unwrap_or_default();
        SharedString::from(suggested_label(&families, &taken))
    });

    let (state_save, ctx_save, edit_save, axes_save, weak) = (
        Rc::clone(state),
        Arc::clone(render_ctx),
        Rc::clone(edit),
        Arc::clone(axes),
        ui.as_weak(),
    );
    editor.on_save_variant_requested(move |name| {
        let Some(ui) = weak.upgrade() else {
            return false;
        };
        match save_design_variant(
            &state_save,
            &ctx_save,
            &edit_save,
            &axes_save,
            name.as_str(),
        ) {
            Ok(()) => true,
            Err(message) => {
                show_toast(&ui, &message, "error");
                false
            }
        }
    });

    let (state_update, edit_update, weak) = (Rc::clone(state), Rc::clone(edit), ui.as_weak());
    ui.global::<EditorModel>()
        .on_update_body_color_variant(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            match update_stale_variant(&state_update, &edit_update) {
                Ok(label) => show_toast(
                    &ui,
                    &format!("Updated this design to the saved color '{label}'."),
                    "success",
                ),
                Err(message) => show_toast(&ui, &message, "error"),
            }
        });

    let (state_near, ctx_near, edit_near, weak) = (
        Rc::clone(state),
        Arc::clone(render_ctx),
        Rc::clone(edit),
        ui.as_weak(),
    );
    ui.global::<EditorModel>()
        .on_convert_to_nearest_variant(move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            match convert_to_nearest(&state_near, &ctx_near, &edit_near) {
                Ok(label) => show_toast(
                    &ui,
                    &format!("This design now uses the color variant '{label}'."),
                    "success",
                ),
                Err(message) => show_toast(&ui, &message, "error"),
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::optics::color_variant_store::user_color_variants;
    use pleochroma::optics::absorption::AbsorptionBand;
    use pleochroma_cut_core::material::{CUSTOM_VARIANT_KEY, CUSTOM_VARIANT_LABEL};

    fn sapphire() -> MaterialSelection {
        MaterialSelection {
            name: Some("Sapphire".to_owned()),
            ..MaterialSelection::none()
        }
    }

    /// A pleochroic uniaxial tensor with one energy band per ray of the given peaks.
    fn tensor(o_peak: f32, e_peak: f32) -> AbsorptionTensor {
        AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::energy(540.0, 3000.0, o_peak)],
            vec![AbsorptionBand::energy(450.0, 3000.0, e_peak)],
        )
    }

    fn saved(key: &str, label: &str, o_peak: f32, e_peak: f32) -> ColorVariant {
        ColorVariant {
            key: key.to_owned(),
            label: label.to_owned(),
            material: "Sapphire".to_owned(),
            absorption: tensor(o_peak, e_peak),
            unit: AbsorptionUnit::PerMm,
            reference_path_mm: 7.0,
            provenance: VariantProvenance::User,
        }
    }

    fn custom_choice(absorption: AbsorptionTensor) -> VariantChoice {
        VariantChoice {
            key: CUSTOM_VARIANT_KEY.to_owned(),
            label: CUSTOM_VARIANT_LABEL.to_owned(),
            absorption,
            unit: AbsorptionUnit::PerMm,
            reference_path_mm: 7.0,
        }
    }

    fn memory_vault() -> Mutex<Database> {
        Mutex::new(Database::new(Some(":memory:")).expect("in-memory vault"))
    }

    #[test]
    fn the_nearest_entry_is_the_variant_whose_face_up_color_is_closest() {
        let host = GemMaterial::sapphire();
        let dark = saved("user.dark", "Dark", 3.0, 3.0);
        let pale = saved("user.pale", "Pale", 0.05, 0.05);
        let variants = [dark.clone(), pale.clone()];
        // A legacy color that is exactly the pale variant's face-up color picks it.
        let lab = face_up_lab(&host, &pale.absorption, pale.unit, 7.0);
        let (entry, label, distance) =
            nearest_entry(&host, &variants, lab, 7.0).expect("a candidate");
        assert_eq!(
            entry,
            ColorEntry::Variant {
                key: "user.pale".to_owned()
            }
        );
        assert_eq!(label, "Pale");
        assert!(distance < 1e-9, "{distance}");
        // Exactly the dark one's color picks the dark one.
        let lab = face_up_lab(&host, &dark.absorption, dark.unit, 7.0);
        let (entry, _, _) = nearest_entry(&host, &variants, lab, 7.0).expect("a candidate");
        assert_eq!(
            entry,
            ColorEntry::Variant {
                key: "user.dark".to_owned()
            }
        );
    }

    #[test]
    fn the_materials_own_color_is_a_candidate_and_wins_for_its_own_face_up_color() {
        let host = GemMaterial::sapphire();
        let variants = [saved("user.dark", "Dark", 3.0, 3.0)];
        let lab = face_up_lab(&host, &host.absorption, host.absorption_unit, 7.0);
        let (entry, label, _) = nearest_entry(&host, &variants, lab, 7.0).expect("a candidate");
        assert_eq!(entry, ColorEntry::MaterialDefault);
        assert_eq!(label, "Material Default");
        // With no variants at all the material's own color is the only choice.
        let (only, _, _) = nearest_entry(&host, &[], [50.0, 0.0, 0.0], 7.0).expect("a candidate");
        assert_eq!(only, ColorEntry::MaterialDefault);
    }

    #[test]
    fn a_legacy_triple_and_bands_have_a_lab_color_and_no_color_has_none() {
        assert!(legacy_lab(&sapphire(), 7.0).is_none());
        let triple = sapphire().with_body_color(Some([0.2, 0.4, 2.8]));
        assert!(legacy_lab(&triple, 7.0).is_some());
        let banded = sapphire().with_body_color_bands(
            Some([0.2, 0.4, 2.8]),
            Some(vec![[460.0, 2500.0, 0.25]]),
            None,
        );
        assert!(legacy_lab(&banded, 7.0).is_some());
    }

    /// Review finding 12: the legacy color and the candidates are read for the SAME stone, so a
    /// wide stone darkens the legacy color like it darkens the candidates (it is not left at the
    /// 7 mm look while the candidates are compared at the stone width).
    #[test]
    fn the_legacy_color_is_read_for_the_stone_width_of_the_candidates() {
        let triple = sapphire().with_body_color(Some([0.2, 0.4, 2.8]));
        let at = |width| legacy_lab(&triple, width).expect("a legacy color")[0];
        assert!(at(14.0) < at(7.0), "a wider stone is darker");
        let banded = sapphire().with_body_color_bands(
            Some([0.2, 0.4, 2.8]),
            Some(vec![[460.0, 2500.0, 0.25]]),
            None,
        );
        let at = |width| legacy_lab(&banded, width).expect("a legacy color")[0];
        assert!(at(14.0) < at(7.0), "so is the band form");
        // The candidates follow the width too.
        let host = GemMaterial::sapphire();
        let dark = saved("user.dark", "Dark", 3.0, 3.0);
        let wide = face_up_lab(&host, &dark.absorption, dark.unit, 14.0);
        let narrow = face_up_lab(&host, &dark.absorption, dark.unit, 7.0);
        assert!(wide[0] <= narrow[0]);
    }

    /// The conversion picks a variant and keeps the pleochroism: applying the nearest entry gives a
    /// design with a variant (or the material's own color) and no legacy color left.
    #[test]
    fn applying_the_nearest_entry_replaces_the_legacy_color() {
        let host = GemMaterial::sapphire();
        let variant = saved("user.pale", "Pale", 0.05, 0.05);
        let legacy = sapphire().with_body_color(Some([0.2, 0.4, 2.8]));
        assert!(legacy_color_on_pleochroic_host(&legacy, &host));
        let lab = face_up_lab(&host, &variant.absorption, variant.unit, 7.0);
        let (entry, _, _) =
            nearest_entry(&host, std::slice::from_ref(&variant), lab, 7.0).expect("a candidate");
        let next = selection_with_entry(legacy.clone(), &entry, &legacy, &[variant]);
        assert_eq!(next.body_color_override, None);
        assert_eq!(
            next.color_variant_override.as_ref().map(|c| c.key.as_str()),
            Some("user.pale")
        );
        assert!(!legacy_color_on_pleochroic_host(&next, &host));
        // An isotropic host never has the legacy note, so the action is refused there.
        assert!(!legacy_color_on_pleochroic_host(
            &legacy,
            &GemMaterial::diamond()
        ));
    }

    #[test]
    fn update_copies_the_stored_color_only_into_a_snapshot_of_that_variant() {
        let before = saved("user.1", "Mine", 0.6, 0.3);
        let design = sapphire().with_color_variant(Some(VariantChoice::from_variant(&before)));
        // Current: nothing to update.
        assert_eq!(updated_selection(&design, &before), None);
        let edited = saved("user.1", "Mine", 1.5, 0.3);
        let next = updated_selection(&design, &edited).expect("an update");
        assert_eq!(
            next.color_variant_override.as_ref().map(|c| &c.absorption),
            Some(&edited.absorption)
        );
        assert_eq!(next.name, design.name, "nothing but the color changes");
        // A rename alone (same tensor) refreshes the label the design shows.
        let mut renamed = before;
        renamed.label = "Deeper".to_owned();
        let next = updated_selection(&design, &renamed).expect("a rename is an update");
        assert_eq!(
            next.color_variant_override
                .as_ref()
                .map(|c| c.label.as_str()),
            Some("Deeper")
        );
        // Another variant's key, or no variant, is never touched.
        let other = saved("user.2", "Other", 1.5, 0.3);
        assert_eq!(updated_selection(&design, &other), None);
        assert_eq!(updated_selection(&sapphire(), &edited), None);
    }

    #[test]
    fn a_saved_variant_becomes_a_choice_with_its_new_key_and_the_trimmed_name() {
        let vault = memory_vault();
        let host = GemMaterial::sapphire();
        let choice = custom_choice(tensor(0.6, 0.3));
        let chosen = persist(&vault, &host, "  Dusk Rose  ", &choice).expect("saved");
        assert!(chosen.key.starts_with("user."));
        assert_eq!(chosen.label, "Dusk Rose");
        assert_eq!(chosen.absorption, choice.absorption);
        assert_eq!(chosen.unit, choice.unit);
        assert_eq!(
            chosen.reference_path_mm, choice.reference_path_mm,
            "the saved variant keeps the stone width the colors were solved for"
        );
        // It is stored under that key, for the host's material, and the design can use it.
        let stored = load_user_color_variant(&vault.lock().unwrap(), &chosen.key)
            .expect("read")
            .expect("stored");
        assert_eq!(stored.material, "Sapphire");
        assert_eq!(stored.label, "Dusk Rose");
        let design =
            lch_color::recoloured_with_choice(&sapphire(), chosen.clone()).expect("a change");
        assert_eq!(design.color_variant_override, Some(chosen));
        assert!(!variant_is_stale_for_test(&design, &stored));
        // A second save of the same colors is another variant, not the first one replaced.
        let again = persist(&vault, &host, "Another", &choice).expect("saved");
        assert_ne!(again.key, design.color_variant_override.unwrap().key);
    }

    /// Whether `stored` differs from the design's snapshot (the combo's stale rule).
    fn variant_is_stale_for_test(design: &MaterialSelection, stored: &ColorVariant) -> bool {
        updated_selection(design, stored).is_some()
    }

    #[test]
    fn the_refusals_of_the_store_reach_the_cutter_as_sentences() {
        let vault = memory_vault();
        let host = GemMaterial::sapphire();
        let blank = persist(&vault, &host, "   ", &custom_choice(tensor(0.6, 0.3)))
            .expect_err("a blank name");
        assert_eq!(blank, "Give the color a name before saving it");
        // A single color for a pleochroic host does not fit it.
        let flat = custom_choice(AbsorptionTensor::isotropic(vec![AbsorptionBand::energy(
            540.0, 3000.0, 0.6,
        )]));
        let refused = persist(&vault, &host, "Flat", &flat).expect_err("a rank mismatch");
        assert!(refused.contains("does not fit Sapphire"), "{refused}");
    }

    /// Review finding 7: the design edit comes with the save. When the design refuses the edit
    /// nothing stays saved, so the retry does not leave a second variant with the same name.
    #[test]
    fn a_refused_design_edit_saves_nothing() {
        let vault = memory_vault();
        let host = GemMaterial::sapphire();
        let choice = custom_choice(tensor(0.6, 0.3));
        let stored = |vault: &Mutex<Database>| {
            user_color_variants(&vault.lock().unwrap(), "Sapphire")
                .expect("read")
                .len()
        };
        let refused = save_then_apply(&vault, &host, "Rosewater", &choice, |_| {
            Err("The design is busy right now.".to_owned())
        })
        .expect_err("the edit was refused");
        assert_eq!(refused, "The design is busy right now.");
        assert_eq!(stored(&vault), 0, "the refused color was taken back");
        // A retry once the edit works saves exactly one.
        let mut applied = None;
        save_then_apply(&vault, &host, "Rosewater", &choice, |chosen| {
            applied = Some(chosen);
            Ok(())
        })
        .expect("saved and applied");
        assert_eq!(stored(&vault), 1);
        assert_eq!(applied.expect("applied").label, "Rosewater");
        // A refusal of the store itself (a blank name) never reaches the design.
        let mut reached = false;
        let blank = save_then_apply(&vault, &host, " ", &choice, |_| {
            reached = true;
            Ok(())
        });
        assert!(blank.is_err() && !reached);
    }

    #[test]
    fn the_suggested_name_avoids_the_labels_the_material_already_has() {
        let taken = taken_labels("Sapphire");
        assert!(taken.iter().any(|label| label == "Material Default"));
        let suggestion = suggested_label(&["Blue"], &["Blue".to_owned()]);
        assert_eq!(suggestion, "Blue 2");
        // Without a vault there are no saved labels, only the built-in ones.
        assert!(taken_labels("No Such Material").is_empty());
    }
}
