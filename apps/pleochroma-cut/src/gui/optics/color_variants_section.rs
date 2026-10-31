//! The material editor's "Color Variants" section: the named colors of one material.
//!
//! The list shows the variants that ship with the program (read-only, with the first source line
//! and an "Estimated" mark when the numbers are an estimate) and the ones the cutter saved
//! ([`super::color_variant_store`]): those can be renamed and deleted. "Duplicate" saves an
//! editable per-millimeter copy of any row, so a built-in color becomes the cutter's own to rename
//! and delete. A design that picked a variant keeps its own copy of the color, so none of this
//! changes a design. Each row's chip is the oriented face-up color of the variant
//! (`variants::face_up_swatch`) for the stone width it is calibrated for; its tooltip names the
//! color of each direction.
//!
//! The rows are built by pure functions ([`rows_for`], [`copy_of`]) so the rules are unit-testable
//! without a window; [`setup`] wires them to `ColorVariantsModel`. The vault is never waited for
//! on the UI thread: a read that finds it busy keeps the saved colors last read, a write says it
//! is busy ([`with_vault`]).

use std::{
    cell::RefCell,
    sync::{Arc, Mutex, TryLockError},
};

use pleochroma::{
    color::body_color::{BodyColor, BodyColors, Illuminant, body_colors},
    optics::materials::{
        AbsorptionUnit, GemMaterial,
        variants::{
            ColorVariant, MATERIAL_DEFAULT_REFERENCE_PATH_MM, VariantProvenance, built_in_variants,
            face_up_swatch, face_up_swatch_path, tensor_rank,
        },
    },
};
use pleochroma_cut_core::material::pattern::per_mm_rows;
use pleochroma_editor::lch_color::{axis_rows, color_family, lch_of_body_color};
use pleochroma_vault::db::sqlite::Database;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::color_variant_store::{
    delete_user_color_variant, estimated_note, rename_user_color_variant, save_user_color_variant,
    unique_label, user_color_variants,
};
use crate::{
    ColorVariantRow, ColorVariantsModel, MainWindow, bridge::render_thread::RenderContext,
    gui::show_toast,
};

/// The sentence for a vault a worker holds right now.
const VAULT_BUSY: &str = "The saved colors are busy right now. Try again in a moment.";

/// One row of the list, as plain values.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    /// The variant behind the row.
    variant: ColorVariant,
    /// Where the color comes from.
    detail: String,
    /// The face-up color, sRGB bytes.
    swatch: [u8; 3],
    /// Saved by the cutter (rename and delete are offered).
    saved: bool,
    /// An estimated built-in variant.
    estimated: bool,
    /// The tooltip: the color of each direction and the estimate note.
    tip: String,
}

/// The color of each direction of `variant` as a sentence: "Along the Axis: Blue / Across the
/// Axis: Green" for a pleochroic one, "Face-up color: Blue." otherwise.
fn directions_line(variant: &ColorVariant, colors: &BodyColors) -> String {
    let tensor = &variant.absorption;
    if !tensor.is_pleochroic {
        return format!(
            "Face-up color: {}.",
            color_family(lch_of_body_color(&colors.unpolarised))
        );
    }
    let mut rays: Vec<&BodyColor> = vec![&colors.o_ray];
    if let Some(beta) = &colors.beta_ray {
        rays.push(beta);
    }
    rays.push(&colors.e_ray);
    axis_rows(tensor_rank(tensor))
        .iter()
        .zip(rays)
        .map(|(row, color)| format!("{}: {}", row.label, color_family(lch_of_body_color(color))))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// The list line under a variant's name.
fn detail_of(provenance: &VariantProvenance) -> String {
    match provenance {
        VariantProvenance::MaterialDefault => "The material's own color".to_owned(),
        VariantProvenance::User => "Saved by you".to_owned(),
        VariantProvenance::BuiltIn { sources, .. } => sources
            .iter()
            .flat_map(|source| source.lines())
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map_or_else(|| "Built in".to_owned(), str::to_owned),
    }
}

/// The row of `variant` on `host`.
///
/// The chip is the oriented face-up color for the stone width the variant is calibrated for (its
/// `reference_path_mm`, in the variant's own unit; 7 mm is the stone width, not the path); the
/// tooltip names the color of each direction at the same path.
fn row_of(host: &GemMaterial, variant: &ColorVariant) -> Row {
    let swatch = face_up_swatch(
        &variant.absorption,
        variant.unit,
        f64::from(variant.reference_path_mm),
        host,
    );
    let colors = body_colors(
        &variant.absorption,
        face_up_swatch_path(variant.unit, variant.reference_path_mm),
        Illuminant::D65,
    );
    let note = estimated_note(&variant.provenance);
    let mut tip = directions_line(variant, &colors);
    if let Some(note) = &note {
        tip.push(' ');
        tip.push_str(note);
    }
    Row {
        detail: detail_of(&variant.provenance),
        swatch: swatch.srgb,
        saved: variant.provenance == VariantProvenance::User,
        estimated: note.is_some(),
        tip,
        variant: variant.clone(),
    }
}

/// The rows of the material `host`: its built-in variants (the Material Default first, then the
/// data file's in file order), then the cutter's saved ones. A saved variant is listed even when
/// the material would no longer accept it, so it can always be renamed or deleted.
fn rows_for(host: &GemMaterial, saved: &[ColorVariant]) -> Vec<Row> {
    built_in_variants(&host.name)
        .iter()
        .chain(
            saved
                .iter()
                .filter(|variant| variant.material.eq_ignore_ascii_case(&host.name)),
        )
        .map(|variant| row_of(host, variant))
        .collect()
}

/// An editable saved copy of `variant`: no key yet (the store makes one), a "<label> Copy" name
/// that is unique among `taken`, provenance of the cutter's own.
///
/// Every saved variant is per millimeter: a per-model-unit source (a Material Default) is
/// converted with the render's own factor (`pattern::per_mm_rows`), and the copy is calibrated for
/// the 7 mm stone those numbers are tuned to. A per-millimeter source keeps its tensor and its
/// width bit for bit.
fn copy_of(variant: &ColorVariant, taken: &[String]) -> ColorVariant {
    let reference_path_mm = match variant.unit {
        AbsorptionUnit::ModelUnit => MATERIAL_DEFAULT_REFERENCE_PATH_MM,
        AbsorptionUnit::PerMm => variant.reference_path_mm,
    };
    ColorVariant {
        key: String::new(),
        label: unique_label(&format!("{} Copy", variant.label), taken),
        absorption: per_mm_rows(variant).to_tensor(),
        unit: AbsorptionUnit::PerMm,
        reference_path_mm,
        provenance: VariantProvenance::User,
        ..variant.clone()
    }
}

/// The materials whose variants can be managed, in list order: the custom materials, then the
/// built-in ones.
fn material_names(custom: &[GemMaterial]) -> Vec<String> {
    custom
        .iter()
        .chain(GemMaterial::built_in())
        .map(|material| material.name.clone())
        .collect()
}

/// The position of `wanted` in `names` (ASCII case ignored), the first entry when it is not there.
fn index_of(names: &[String], wanted: &str) -> usize {
    names
        .iter()
        .position(|name| name.eq_ignore_ascii_case(wanted.trim()))
        .unwrap_or(0)
}

/// The material called `name` among `custom` and the built-in ones (a one-color stand-in when it
/// is none of them, which only the chips of its rows need).
fn host_of(name: &str, custom: &[GemMaterial]) -> GemMaterial {
    custom
        .iter()
        .chain(GemMaterial::built_in())
        .find(|material| material.name.eq_ignore_ascii_case(name))
        .cloned()
        .unwrap_or_else(|| GemMaterial::new_custom(name, 1.5, 0.0, 0.0, [0.0; 3]))
}

/// What the section lists now.
struct Shown {
    /// The materials in the combo.
    materials: Vec<String>,
    /// The custom materials of the render context when the section was opened.
    custom: Vec<GemMaterial>,
    /// The material whose variants are listed.
    material: String,
    /// The saved variants last read from the vault (kept while a worker holds it).
    saved: Vec<ColorVariant>,
    /// The variants behind the rows, in row order.
    variants: Vec<ColorVariant>,
}

impl Shown {
    /// Nothing listed yet.
    const fn empty() -> Self {
        Self {
            materials: Vec::new(),
            custom: Vec::new(),
            material: String::new(),
            saved: Vec::new(),
            variants: Vec::new(),
        }
    }
}

thread_local! {
    /// The section's list as the Slint thread last pushed it, so a row index resolves against the
    /// list the cutter actually saw.
    static SHOWN: RefCell<Shown> = const { RefCell::new(Shown::empty()) };
}

/// The variant behind row `index` of the list last pushed.
fn shown_variant(index: i32) -> Option<ColorVariant> {
    let at = usize::try_from(index).ok()?;
    SHOWN.with_borrow(|shown| shown.variants.get(at).cloned())
}

/// The material listed now.
fn shown_material() -> String {
    SHOWN.with_borrow(|shown| shown.material.clone())
}

/// The sentence under the combo: why the list is short.
fn note_for(rows: &[Row]) -> &'static str {
    if rows.iter().any(|row| row.saved) {
        ""
    } else if rows.len() <= 1 {
        "This material has no color variants yet. Set a color for a design in the Stone tab and use Save as Variant to keep it here."
    } else {
        "None saved yet. Set a color for a design in the Stone tab and use Save as Variant to keep it here."
    }
}

/// Runs `work` on the vault without waiting for it: the UI thread never blocks on a vault a worker
/// holds (it answers [`VAULT_BUSY`] instead). A failure is returned as the sentence to show.
fn with_vault<T>(
    db: &Mutex<Database>,
    work: impl FnOnce(&Database) -> anyhow::Result<T>,
) -> Result<T, String> {
    let guard = match db.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => return Err(VAULT_BUSY.to_owned()),
    };
    work(&guard).map_err(|error| format!("{error:#}"))
}

/// Pushes the rows of the material now listed.
///
/// The saved variants are read without waiting for the vault ([`with_vault`]); a busy vault keeps
/// the list last read, so opening the dialog never freezes the window.
fn push_rows(ui: &MainWindow, db: &Mutex<Database>) {
    let material = shown_material();
    let read = with_vault(db, |db| user_color_variants(db, &material));
    let (host, saved) = SHOWN.with_borrow_mut(|shown| {
        if let Ok(list) = read {
            shown.saved = list;
        }
        (host_of(&material, &shown.custom), shown.saved.clone())
    });
    let rows = rows_for(&host, &saved);
    let note = note_for(&rows);
    let model = ui.global::<ColorVariantsModel>();
    model.set_rows(ModelRc::new(VecModel::from(
        rows.iter()
            .map(|row| ColorVariantRow {
                label: row.variant.label.as_str().into(),
                detail: row.detail.as_str().into(),
                swatch: slint::Color::from_rgb_u8(row.swatch[0], row.swatch[1], row.swatch[2]),
                saved: row.saved,
                estimated: row.estimated,
                tip: row.tip.as_str().into(),
            })
            .collect::<Vec<_>>(),
    )));
    model.set_note(note.into());
    SHOWN.with_borrow_mut(|shown| {
        shown.variants = rows.into_iter().map(|row| row.variant).collect();
    });
}

/// Lists the materials, selects `wanted` (the first when it is not listed) and pushes its rows.
fn open(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    db: &Mutex<Database>,
    wanted: &str,
) {
    let custom = RenderContext::lock(render_ctx).custom_materials.clone();
    let names = material_names(&custom);
    let at = index_of(&names, wanted);
    let model = ui.global::<ColorVariantsModel>();
    model.set_materials(ModelRc::new(VecModel::from(
        names
            .iter()
            .map(|name| SharedString::from(name.as_str()))
            .collect::<Vec<_>>(),
    )));
    model.set_material_index(i32::try_from(at).unwrap_or(0));
    SHOWN.with_borrow_mut(|shown| {
        shown.material = names.get(at).cloned().unwrap_or_default();
        shown.materials = names;
        shown.custom = Arc::unwrap_or_clone(custom);
        shown.saved.clear();
    });
    push_rows(ui, db);
}

/// Registers the callbacks of `ColorVariantsModel`.
pub(in crate::gui) fn setup(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    db: &Arc<Mutex<Database>>,
) {
    let model = ui.global::<ColorVariantsModel>();

    let (weak, ctx, vault) = (ui.as_weak(), Arc::clone(render_ctx), Arc::clone(db));
    model.on_opened(move |wanted| {
        if let Some(ui) = weak.upgrade() {
            open(&ui, &ctx, &vault, wanted.as_str());
        }
    });

    let (weak, vault) = (ui.as_weak(), Arc::clone(db));
    model.on_material_selected(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let chosen = usize::try_from(index)
            .ok()
            .and_then(|at| SHOWN.with_borrow(|shown| shown.materials.get(at).cloned()));
        if let Some(name) = chosen {
            SHOWN.with_borrow_mut(|shown| shown.material = name);
            push_rows(&ui, &vault);
        }
    });

    setup_rename(ui, db);
    setup_delete(ui, db);
    setup_duplicate(ui, db);
}

/// Registers "Rename" of a saved row.
fn setup_rename(ui: &MainWindow, db: &Arc<Mutex<Database>>) {
    let (weak, vault) = (ui.as_weak(), Arc::clone(db));
    ui.global::<ColorVariantsModel>()
        .on_rename_requested(move |index, name| {
            let Some(ui) = weak.upgrade() else {
                return false;
            };
            let Some(variant) = shown_variant(index).filter(is_saved) else {
                return false;
            };
            let result = with_vault(&vault, |db| {
                rename_user_color_variant(db, &variant.key, name.as_str())
            });
            match result {
                Ok(()) => {
                    push_rows(&ui, &vault);
                    show_toast(
                        &ui,
                        &format!(
                            "Renamed the color '{}' to '{}'.",
                            variant.label,
                            name.trim()
                        ),
                        "success",
                    );
                    true
                }
                Err(message) => {
                    show_toast(&ui, &message, "error");
                    false
                }
            }
        });
}

/// Registers "Delete" of a saved row.
fn setup_delete(ui: &MainWindow, db: &Arc<Mutex<Database>>) {
    let (weak, vault) = (ui.as_weak(), Arc::clone(db));
    ui.global::<ColorVariantsModel>()
        .on_delete_requested(move |index| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let Some(variant) = shown_variant(index).filter(is_saved) else {
                return;
            };
            match with_vault(&vault, |db| delete_user_color_variant(db, &variant.key)) {
                Ok(_) => {
                    push_rows(&ui, &vault);
                    show_toast(
                        &ui,
                        &format!(
                            "Deleted the color '{}'. Designs that use it keep their own copy.",
                            variant.label
                        ),
                        "info",
                    );
                }
                Err(message) => show_toast(&ui, &message, "error"),
            }
        });
}

/// Registers "Duplicate" of any row.
fn setup_duplicate(ui: &MainWindow, db: &Arc<Mutex<Database>>) {
    let (weak, vault) = (ui.as_weak(), Arc::clone(db));
    ui.global::<ColorVariantsModel>()
        .on_duplicate_requested(move |index| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let Some(variant) = shown_variant(index) else {
                return;
            };
            let taken: Vec<String> = SHOWN.with_borrow(|shown| {
                shown
                    .variants
                    .iter()
                    .map(|listed| listed.label.clone())
                    .collect()
            });
            let copy = copy_of(&variant, &taken);
            match with_vault(&vault, |db| save_user_color_variant(db, &copy)) {
                Ok(_) => {
                    push_rows(&ui, &vault);
                    show_toast(
                        &ui,
                        &format!("Saved '{}'. You can rename or delete it here.", copy.label),
                        "success",
                    );
                }
                Err(message) => show_toast(&ui, &message, "error"),
            }
        });
}

/// Whether `variant` is a saved one (its row offers rename and delete).
fn is_saved(variant: &ColorVariant) -> bool {
    variant.provenance == VariantProvenance::User
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::AbsorptionUnit,
    };

    fn sapphire_host() -> GemMaterial {
        GemMaterial::sapphire()
    }

    fn host_named(name: &str) -> GemMaterial {
        GemMaterial::new_custom(name, 1.5, 0.01, 0.0, [0.0; 3])
    }

    /// A saved Sapphire variant.
    fn saved(key: &str, label: &str) -> ColorVariant {
        ColorVariant {
            key: key.to_owned(),
            label: label.to_owned(),
            material: "Sapphire".to_owned(),
            absorption: AbsorptionTensor::uniaxial(
                vec![AbsorptionBand::energy(540.0, 3000.0, 0.6)],
                vec![AbsorptionBand::energy(450.0, 3000.0, 0.3)],
            ),
            unit: AbsorptionUnit::PerMm,
            reference_path_mm: 7.0,
            provenance: VariantProvenance::User,
        }
    }

    #[test]
    fn the_material_default_leads_and_the_saved_variants_follow_the_built_in_ones() {
        let rows = rows_for(&sapphire_host(), &[saved("user.1", "Mine")]);
        assert_eq!(rows[0].variant.label, "Material Default");
        assert!(!rows[0].saved, "the material's own color is read-only");
        assert_eq!(rows[0].detail, "The material's own color");
        let last = rows.last().expect("rows");
        assert_eq!(last.variant.key, "user.1");
        assert!(last.saved && !last.estimated);
        assert_eq!(last.detail, "Saved by you");
        assert!(
            rows[..rows.len() - 1].iter().all(|row| !row.saved),
            "built-in rows are never offered rename or delete"
        );
    }

    #[test]
    fn a_saved_variant_of_another_material_is_not_listed() {
        let mut other = saved("user.2", "Rubies");
        other.material = "Ruby".to_owned();
        let rows = rows_for(&sapphire_host(), &[other]);
        assert!(rows.iter().all(|row| row.variant.key != "user.2"));
    }

    #[test]
    fn a_saved_variant_that_no_longer_fits_the_material_is_still_listed() {
        // An isotropic color on a pleochroic material would not be offered in the combo, but it
        // must stay reachable here so it can be renamed or deleted.
        let mut flat = saved("user.3", "Flat");
        flat.absorption = AbsorptionTensor::isotropic(flat.absorption.o_ray.clone());
        let rows = rows_for(&sapphire_host(), &[flat]);
        assert!(rows.iter().any(|row| row.variant.key == "user.3"));
    }

    #[test]
    fn a_custom_material_lists_only_its_saved_variants() {
        let mut mine = saved("user.4", "Mine");
        mine.material = "My Own Corundum".to_owned();
        let rows = rows_for(&host_named("My Own Corundum"), &[mine]);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].saved);
    }

    #[test]
    fn every_row_has_a_tooltip_and_a_swatch_and_a_pleochroic_one_names_its_directions() {
        for row in rows_for(&sapphire_host(), &[saved("user.1", "Mine")]) {
            assert!(!row.tip.is_empty(), "{}", row.variant.label);
            assert!(!row.tip.contains("colour"), "{}", row.variant.label);
        }
        let mine = row_of(&sapphire_host(), &saved("user.1", "Mine"));
        assert!(mine.tip.contains("Along the Axis: "), "{}", mine.tip);
        assert!(mine.tip.contains(" / Across the Axis: "));
        let diamond = rows_for(&GemMaterial::diamond(), &[]);
        assert!(diamond[0].tip.starts_with("Face-up color: "));
    }

    #[test]
    fn an_estimated_built_in_is_marked_and_names_its_source() {
        let mut variant = saved("sapphire.estimated", "Estimated One");
        variant.provenance = VariantProvenance::BuiltIn {
            sources: vec!["Fe3+ bands of the corundum host; estimated".to_owned()],
            fit_record: None,
            estimated: true,
        };
        let row = row_of(&sapphire_host(), &variant);
        assert!(row.estimated && !row.saved);
        assert_eq!(row.detail, "Fe3+ bands of the corundum host; estimated");
        assert!(
            row.tip
                .ends_with("Estimated: Fe3+ bands of the corundum host; estimated"),
            "{}",
            row.tip
        );
        // A measured built-in carries its source but no mark.
        variant.provenance = VariantProvenance::BuiltIn {
            sources: vec!["Rossman spectrum".to_owned()],
            fit_record: None,
            estimated: false,
        };
        let measured = row_of(&sapphire_host(), &variant);
        assert!(!measured.estimated);
        assert!(!measured.tip.contains("Estimated"));
    }

    #[test]
    fn duplicating_makes_an_editable_copy_with_a_unique_name() {
        let builtin = rows_for(&sapphire_host(), &[])[0].variant.clone();
        let copy = copy_of(&builtin, &["Material Default".to_owned()]);
        assert_eq!(copy.key, "", "the store gives it a key");
        assert_eq!(copy.label, "Material Default Copy");
        assert_eq!(copy.provenance, VariantProvenance::User);
        assert_eq!(copy.material, builtin.material);
        // Review finding 11: Sapphire's own color is per model unit; its saved copy is per
        // millimeter at the 7 mm width, in the same pleochroic form.
        assert_eq!(builtin.unit, AbsorptionUnit::ModelUnit);
        assert_eq!(copy.unit, AbsorptionUnit::PerMm);
        assert!((copy.reference_path_mm - 7.0).abs() < 1e-6);
        assert!(copy.absorption.is_pleochroic);
        assert_eq!(
            tensor_rank(&copy.absorption),
            tensor_rank(&builtin.absorption)
        );
        let again = copy_of(&builtin, &["Material Default Copy".to_owned()]);
        assert_eq!(again.label, "Material Default Copy 2");
        let of_saved = copy_of(&saved("user.1", "Mine"), &[]);
        assert_eq!(of_saved.label, "Mine Copy");
    }

    /// The copy looks like the original in a 7 mm stone: the conversion keeps the color.
    #[test]
    fn a_converted_copy_has_the_color_of_the_original() {
        let host = sapphire_host();
        let original = rows_for(&host, &[])[0].variant.clone();
        let copy = copy_of(&original, &[]);
        let before = face_up_swatch(&original.absorption, original.unit, 7.0, &host).srgb;
        let after = face_up_swatch(&copy.absorption, copy.unit, 7.0, &host).srgb;
        for (b, a) in before.iter().zip(after) {
            assert!(b.abs_diff(a) <= 1, "{before:?} vs {after:?}");
        }
        // A per-millimeter source is copied bit for bit, width included.
        let mut mine = saved("user.1", "Mine");
        mine.reference_path_mm = 10.0;
        let again = copy_of(&mine, &[]);
        assert_eq!(again.absorption, mine.absorption);
        assert_eq!(again.unit, AbsorptionUnit::PerMm);
        assert!((again.reference_path_mm - 10.0).abs() < 1e-6);
    }

    /// A row's chip is the face-up color for the stone width the variant carries.
    #[test]
    fn a_rows_chip_is_the_face_up_color_at_its_calibrated_width() {
        let host = sapphire_host();
        let mut light = saved("user.1", "Mine");
        light.absorption = AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::energy(540.0, 3000.0, 0.03)],
            vec![AbsorptionBand::energy(450.0, 3000.0, 0.02)],
        );
        for width in [7.0_f32, 12.0] {
            light.reference_path_mm = width;
            let expected = face_up_swatch(&light.absorption, light.unit, f64::from(width), &host);
            assert_eq!(row_of(&host, &light).swatch, expected.srgb, "{width} mm");
        }
    }

    /// A vault a worker holds is not waited for: the answer is the busy sentence.
    #[test]
    fn a_busy_vault_is_reported_not_waited_for() {
        let vault = Mutex::new(Database::new(Some(":memory:")).expect("in-memory vault"));
        let held = vault.lock().expect("free");
        let busy = with_vault(&vault, |db| user_color_variants(db, "Sapphire"));
        assert_eq!(busy, Err(VAULT_BUSY.to_owned()));
        drop(held);
        let free = with_vault(&vault, |db| user_color_variants(db, "Sapphire"));
        assert_eq!(free, Ok(Vec::new()));
    }

    #[test]
    fn a_duplicate_of_a_built_in_is_saved_and_listed_as_a_saved_variant() {
        let db = Database::new(Some(":memory:")).expect("in-memory vault");
        let original = rows_for(&sapphire_host(), &[])[0].variant.clone();
        let copy = copy_of(&original, &[]);
        let key = save_user_color_variant(&db, &copy).expect("saved");
        let listed = user_color_variants(&db, "Sapphire").expect("read");
        let rows = rows_for(&sapphire_host(), &listed);
        let row = rows.last().expect("rows");
        assert_eq!(row.variant.key, key);
        assert!(row.saved);
        assert_eq!(row.variant.label, "Material Default Copy");
        // Renamed and deleted like any saved variant.
        rename_user_color_variant(&db, &key, "Renamed").expect("renamed");
        assert_eq!(
            user_color_variants(&db, "Sapphire").expect("read")[0].label,
            "Renamed"
        );
        assert!(delete_user_color_variant(&db, &key).expect("deleted"));
        assert!(
            user_color_variants(&db, "Sapphire")
                .expect("read")
                .is_empty()
        );
    }

    #[test]
    fn the_material_list_has_the_custom_ones_first_and_finds_a_name_ignoring_case() {
        let custom = vec![GemMaterial::new_custom("My Own", 1.5, 0.01, 0.0, [0.0; 3])];
        let names = material_names(&custom);
        assert_eq!(names[0], "My Own");
        assert!(names.iter().any(|name| name == "Sapphire"));
        assert_eq!(index_of(&names, "my own"), 0);
        let sapphire = index_of(&names, " sapphire ");
        assert_eq!(names[sapphire], "Sapphire");
        assert_eq!(index_of(&names, "No Such Material"), 0);
    }

    #[test]
    fn the_note_explains_an_empty_saved_list() {
        let none = rows_for(&sapphire_host(), &[]);
        assert!(note_for(&none).contains("Save as Variant"));
        let one = rows_for(&sapphire_host(), &[saved("user.1", "Mine")]);
        assert_eq!(note_for(&one), "");
        let custom_none = rows_for(&host_named("No Such Material"), &[]);
        assert!(note_for(&custom_none).contains("no color variants yet"));
    }
}
