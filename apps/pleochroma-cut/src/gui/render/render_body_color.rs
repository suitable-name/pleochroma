//! The Live Render toolbar's "Color" control: a quick color change that needs no custom
//! material.
//!
//! The control offers the same keyed list as the design's color combo, for the viewed material
//! (`pleochroma_editor::material::color_entries_for_design`): "Material Default", the material's
//! color variants (built in and saved), the nine generic presets (only for a material that looks
//! the same in every direction), a "Legacy" row for an old single color on a pleochroic stone, and
//! "Custom Color..." (the per-axis L*C*h editor). Every dot is the oriented face-up color of its
//! row (`variants::face_up_swatch`) for the current Stone Size. Where a pick goes depends on what
//! the view shows (see [`ColorTarget`]):
//!
//! * the editor's open design with the render material linked to it: the pick is that
//!   design's own color, the same undoable edit the Edit tab's color box makes (the editor side
//!   supplies it as a [`DesignColorPort`]; a color variant is stored as the design's variant
//!   snapshot);
//! * anything else (a library design, or a material preview from the Stone tab): a view-only
//!   setting, `RenderContext::view_body_color` (a [`ViewColor`]: a color variant snapshot, or for
//!   the colors saved before variants the legacy triple plus its seven-band rows), remembered in
//!   `AppSettings::render_body_color_variant` (or, for a legacy color,
//!   `render_body_color_override` and `render_body_color_bands`) and never written to a design.
//!
//! The render pipeline applies the setting in one place
//! (`RenderContext::tinted_material_override`), so the live view, high-resolution export,
//! tilt video and remote workers all render the same color. A variant replaces only the absorption
//! tensor (`GemMaterial::with_color_variant_tensor`), so a pleochroic stone keeps every direction;
//! a legacy color is one color and flattens it, exactly as it was saved. A material that defines
//! its own color (a physics recipe) is never recolored and the control is greyed out.
//!
//! Saved variants ("My Variants") come from the vault through [`DesignColorPort::saved_variants`];
//! the render context itself holds no vault, so a port that offers none lists the built-in variants
//! only.
//!
//! Everything decided here is a plain function ([`control_view`], [`shown_selection`],
//! [`color_list`], [`list_view`], [`pick_for_entry`], [`display_rgb_for_stone`]) so it is tested
//! without a window.

use std::{
    borrow::Cow,
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use pleochroma::{
    color::body_color::{Illuminant, body_colors},
    optics::{
        absorption::{AbsorptionTensor, legacy_rgb_bands},
        materials::{
            GemMaterial,
            body_color::BODY_COLOR_PRESETS,
            variants::{ColorVariant, TensorRank, face_up_swatch, host_rank},
        },
    },
};
use pleochroma_cut_core::{
    MaterialSelection,
    material::{CUSTOM_VARIANT_LABEL, VariantChoice, color::swatch_path_units},
};
use pleochroma_editor::material::{
    ColorEntry, ColorEntryItem, color_entries_for_design, entry_for_selection,
    legacy_color_on_pleochroic_host, listed_variants,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{
    MainWindow, ViewportModel,
    bridge::render_thread::{ColorTarget, RenderContext, ViewColor},
    gui::show_toast,
    settings::SettingsPersister,
};

/// A legacy body-color override: the absorption triple `GemMaterial::with_body_color` takes, or
/// `None` for the material's own color.
pub type ColorOverride = Option<[f32; 3]>;

/// The label of the row that shows a design's color when it matches no row.
const NO_COLOR_LABEL: &str = "Material Default";

/// The note under the list while the shown color is an old single color on a pleochroic stone.
const LEGACY_NOTE: &str = "Legacy Color: No Pleochroism";

/// Where the color picker starts while the control shows "Material Default".
const PICKER_START: slint::Color = slint::Color::from_rgb_u8(0xc0, 0x39, 0x2b);

/// What a clicked row of the control means.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// "Material Default": the material's own color.
    MaterialDefault,
    /// A legacy single color: one of the presets, or a quick-picker triple.
    Triple([f32; 3]),
    /// A color variant: the material it belongs to and its snapshot.
    Variant {
        /// The material the variant belongs to (`ColorVariant::material`).
        material: String,
        /// The snapshot a design or the view stores.
        choice: VariantChoice,
    },
}

/// A change of the open design's color, as the editor side applies it (one undoable edit).
#[derive(Debug, Clone, PartialEq)]
pub enum ColorChange {
    /// A legacy override (`None` = the material's own color): replaces the design's pattern and
    /// every other color field.
    Legacy(ColorOverride),
    /// A color variant snapshot (a picked variant or a Custom Color): replaces the design's
    /// pattern and the legacy color fields.
    Variant(VariantChoice),
}

impl Pick {
    /// The change this pick makes to the open design.
    #[must_use]
    pub fn change(&self) -> ColorChange {
        match self {
            Self::MaterialDefault => ColorChange::Legacy(None),
            Self::Triple(rgb) => ColorChange::Legacy(Some(*rgb)),
            Self::Variant { choice, .. } => ColorChange::Variant(choice.clone()),
        }
    }

    /// The view color this pick sets (`None` = the material's own color).
    #[must_use]
    pub fn view_color(&self) -> Option<ViewColor> {
        match self {
            Self::MaterialDefault => None,
            Self::Triple(rgb) => Some(ViewColor::triple_only(*rgb)),
            Self::Variant { material, choice } => {
                Some(ViewColor::from_variant(material.clone(), choice.clone()))
            }
        }
    }
}

/// The pick a list entry stands for. `variants` are the variants behind the list's rows
/// ([`ColorList::variants`]). `None` for "Custom" (the editor handles it), for a variant key the
/// list does not hold and for an out-of-range preset.
#[must_use]
pub fn pick_for_entry(entry: &ColorEntry, variants: &[ColorVariant]) -> Option<Pick> {
    match entry {
        ColorEntry::MaterialDefault => Some(Pick::MaterialDefault),
        ColorEntry::Preset(index) => BODY_COLOR_PRESETS
            .get(*index)
            .map(|preset| Pick::Triple(preset.absorption_rgb)),
        ColorEntry::Variant { key } => {
            variants
                .iter()
                .find(|variant| variant.key == *key)
                .map(|variant| Pick::Variant {
                    material: variant.material.clone(),
                    choice: VariantChoice::from_variant(variant),
                })
        }
        ColorEntry::Custom => None,
    }
}

/// The sRGB color (D65) a stone with this absorption triple shows at `stone_width_mm`
/// (`0.0` = no size set): the swatch path is
/// `MODEL_UNIT_FACE_UP_PATH` times the scale the render gives a per-model-unit
/// color at that size ([`swatch_path_units`], `width / 7 mm`), so the dot is the color the
/// face-up render shows (the calibration of `render_setup::MODEL_UNIT_FACE_UP_PATH`) and darkens
/// with the stone like the render does. With no size or 7 mm it is the 1-unit reference. The
/// toolbar's preset dots call this with the current Stone Size.
#[must_use]
pub fn display_rgb_for_stone(absorption: [f32; 3], stone_width_mm: f32) -> [u8; 3] {
    let tensor = AbsorptionTensor::isotropic(legacy_rgb_bands(absorption));
    body_colors(&tensor, swatch_path_units(stone_width_mm), Illuminant::D65)
        .unpolarised
        .srgb
}

/// The face-up sRGB color (D65) of a color variant at `stone_width_mm` on `host`: the dot the
/// toolbar shows for it.
///
/// It is the oriented face-up color of `variants::face_up_swatch` (the host's own crystal axis
/// through the table; the unpolarised color of a one-color tensor), in the variant's own unit and
/// for the stone's width: a built-in or saved variant and a Custom Color alike (a variant's
/// `reference_path_mm` is the stone width its numbers are calibrated for, not a path). `0.0` is
/// "no size set", the 7 mm default.
#[must_use]
pub fn variant_swatch(choice: &VariantChoice, stone_width_mm: f32, host: &GemMaterial) -> [u8; 3] {
    face_up_swatch(
        &choice.absorption,
        choice.unit,
        f64::from(stone_width_mm),
        host,
    )
    .srgb
}

/// The dot a color selection shows: the face-up color of its variant (on `host`, `None`: a
/// one-color stand-in), else of its legacy triple; `None` (a hollow ring) for the material's own
/// color.
#[must_use]
pub fn shown_swatch(
    selection: &MaterialSelection,
    stone_width_mm: f32,
    host: Option<&GemMaterial>,
) -> Option<[u8; 3]> {
    if let Some(choice) = &selection.color_variant_override {
        let host: Cow<'_, GemMaterial> =
            host.map_or_else(|| Cow::Owned(unresolved_host()), Cow::Borrowed);
        return Some(variant_swatch(choice, stone_width_mm, &host));
    }
    selection
        .body_color_override
        .map(|rgb| display_rgb_for_stone(rgb, stone_width_mm))
}

/// The color the control shows, as the color fields of a selection: the design's while picks go
/// to the design, the view color while they go to the view (and the material can be recolored at
/// all). A view color never applies to a material with its own color (a physics recipe, color
/// zones): none is shown.
#[must_use]
pub fn shown_selection(
    target: ColorTarget,
    own_color: bool,
    design: &MaterialSelection,
    view: &MaterialSelection,
) -> MaterialSelection {
    match (target, own_color) {
        (ColorTarget::Design, _) => design.clone(),
        (ColorTarget::View, false) => view.clone(),
        (ColorTarget::View, true) => MaterialSelection::none(),
    }
}

/// The stand-in host of a view whose material cannot be resolved: an isotropic material with no
/// name, so the list is Material Default, the presets and Custom.
fn unresolved_host() -> GemMaterial {
    GemMaterial::new_custom("", 1.5, 0.0, 0.0, [0.0; 3])
}

/// The keyed list the toolbar shows ([`color_list`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ColorList {
    /// The rows, in order: Material Default, the variants, the presets (isotropic hosts) or the
    /// Legacy row (an old single color on a pleochroic host), Custom last.
    pub items: Vec<ColorEntryItem>,
    /// The variants behind the rows (what [`pick_for_entry`] resolves keys against).
    pub variants: Vec<ColorVariant>,
    /// The host's absorption rank.
    pub rank: TensorRank,
    /// The host the list was built for (the stand-in of [`unresolved_host`] when the material
    /// cannot be resolved): the rows' dots are drawn on it.
    pub host: GemMaterial,
}

/// The list for `host` (`None`: the stand-in of [`unresolved_host`]) with the cutter's `saved`
/// variants (listed for their own material only) and the `shown` color (which decides whether the
/// Legacy row exists).
#[must_use]
pub fn color_list(
    host: Option<&GemMaterial>,
    saved: &[ColorVariant],
    shown: &MaterialSelection,
) -> ColorList {
    let host = host.cloned().unwrap_or_else(unresolved_host);
    ColorList {
        items: color_entries_for_design(&host, saved, shown),
        variants: listed_variants(&host, saved),
        rank: host_rank(&host),
        host,
    }
}

/// One row of the popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowView {
    /// The row's position in the list: what `render_color_chosen` hands back.
    pub index: usize,
    /// The label.
    pub label: String,
    /// The tooltip (the per-direction colors of a pleochroic entry).
    pub tip: String,
    /// The dot, `None` for a row with no color of its own (Custom).
    pub swatch: Option<[u8; 3]>,
}

/// The list grouped the way the popup draws it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListView {
    /// Material Default and the variants: text rows with a dot.
    pub rows: Vec<RowView>,
    /// The generic presets: round swatches (isotropic hosts only).
    pub presets: Vec<RowView>,
    /// The Legacy row of an old single color on a pleochroic host.
    pub legacy: Option<RowView>,
    /// The Custom row.
    pub custom: Option<RowView>,
}

/// Groups `list` for the popup. The dots of Material Default, the presets and the variants follow
/// `stone_width_mm` ([`variant_swatch`], [`display_rgb_for_stone`]) like the selected dot, so a dot
/// and the swatch it becomes when picked agree at every Stone Size; the Legacy row keeps its
/// preset's 7 mm dot.
#[must_use]
pub fn list_view(list: &ColorList, stone_width_mm: f32) -> ListView {
    let mut view = ListView::default();
    for (index, item) in list.items.iter().enumerate() {
        let mut row = RowView {
            index,
            label: item.label.clone(),
            tip: item.tooltip.clone(),
            swatch: item.swatch_rgb(),
        };
        match &item.entry {
            ColorEntry::MaterialDefault => {
                row.swatch = Some(
                    face_up_swatch(
                        &list.host.absorption,
                        list.host.absorption_unit,
                        f64::from(stone_width_mm),
                        &list.host,
                    )
                    .srgb,
                );
                view.rows.push(row);
            }
            ColorEntry::Variant { key } => {
                if let Some(variant) = list.variants.iter().find(|variant| variant.key == *key) {
                    row.swatch = Some(variant_swatch(
                        &VariantChoice::from_variant(variant),
                        stone_width_mm,
                        &list.host,
                    ));
                }
                view.rows.push(row);
            }
            ColorEntry::Preset(preset) if list.rank == TensorRank::Isotropic => {
                if let Some(preset) = BODY_COLOR_PRESETS.get(*preset) {
                    row.swatch = Some(display_rgb_for_stone(preset.absorption_rgb, stone_width_mm));
                }
                view.presets.push(row);
            }
            ColorEntry::Preset(_) => view.legacy = Some(row),
            ColorEntry::Custom => view.custom = Some(row),
        }
    }
    view
}

/// What [`control_view`] decides from.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlInput<'a> {
    /// Who a pick changes right now.
    pub target: ColorTarget,
    /// The traced material defines its own color (a physics recipe).
    pub physics: bool,
    /// The traced material has color zones (color zoning): its color is its zones, so no
    /// override applies to it.
    pub zoned: bool,
    /// The color shown ([`shown_selection`]).
    pub shown: &'a MaterialSelection,
    /// The rows of the list built for it ([`color_list`]).
    pub items: &'a [ColorEntryItem],
    /// The host the list was built for (`ColorList::host`): the dot of a variant is drawn on it.
    pub host: Option<&'a GemMaterial>,
    /// The stone size the render uses (`RenderContext::stone_width_mm`, `0.0` = not set), which
    /// the dot is shown at.
    pub stone_width_mm: f32,
}

/// What the control shows.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the control's display flags (enabled, physics, zoned, targets_design); they are independent switches"
)]
pub struct ControlView {
    /// The selected row: its position in the list ([`entry_for_selection`]).
    pub choice: usize,
    /// The shown color's name: a variant's own label ("Padparadscha", "Custom Color"), else the
    /// selected row's.
    pub summary: String,
    /// The color the selection renders in; `None` for "Material Default".
    pub swatch: Option<[u8; 3]>,
    /// False for a material that defines its own color (a physics recipe): the control is
    /// greyed out. A zoned material keeps the control open for its Color Zoning section.
    pub enabled: bool,
    /// Whether the body colors (Material Default, the rows, Custom Color) can be picked:
    /// false for a physics recipe and for color zones, whose color is the zones.
    pub colors_enabled: bool,
    /// A reason for `enabled == false`: a physics recipe.
    pub physics: bool,
    /// A reason for `enabled == false`: color zones.
    pub zoned: bool,
    /// A pick edits the open design (true) or only the view (false).
    pub targets_design: bool,
}

/// The control's state for the color in `input.shown`: for a design that carries a color variant
/// it is the variant's label and its face-up dot, never "Material Default".
#[must_use]
pub fn control_view(input: &ControlInput<'_>) -> ControlView {
    let choice = entry_for_selection(input.shown, input.items);
    let summary = match (&input.shown.color_variant_override, input.items.get(choice)) {
        (Some(variant), _) => variant.label.clone(),
        (None, Some(item)) if matches!(item.entry, ColorEntry::Custom) => {
            CUSTOM_VARIANT_LABEL.to_owned()
        }
        (None, Some(item)) => item.label.clone(),
        (None, None) => NO_COLOR_LABEL.to_owned(),
    };
    let own_color = input.physics || input.zoned;
    ControlView {
        choice,
        summary,
        swatch: shown_swatch(input.shown, input.stone_width_mm, input.host),
        enabled: !input.physics,
        colors_enabled: !own_color,
        physics: input.physics,
        zoned: input.zoned,
        targets_design: input.target == ColorTarget::Design,
    }
}

/// The note under the list for the shown color: "Legacy Color: No Pleochroism" while it is an old
/// single color on a pleochroic `host`, else empty.
#[must_use]
pub fn legacy_note(shown: &MaterialSelection, host: Option<&GemMaterial>) -> &'static str {
    if host.is_some_and(|host| legacy_color_on_pleochroic_host(shown, host)) {
        LEGACY_NOTE
    } else {
        ""
    }
}

/// The view color that applies to `host`: a variant that does not fit it
/// (`ViewVariant::fits`) is dormant, so nothing is shown for it (the render keeps the host's own
/// color). A legacy color always applies.
fn applicable_view_color(
    color: Option<ViewColor>,
    host: Option<&GemMaterial>,
) -> Option<ViewColor> {
    color.filter(|color| {
        color
            .variant
            .as_ref()
            .is_none_or(|variant| host.is_none_or(|host| variant.fits(host)))
    })
}

/// Applies `change` to the render context and marks the render dirty only when the color
/// the render pipeline applies actually moved (a dormant view color is not a scene change).
fn update_context(ctx: &mut RenderContext, change: impl FnOnce(&mut RenderContext)) {
    let before = ctx.effective_view_body_color();
    change(ctx);
    if ctx.effective_view_body_color() != before {
        ctx.dirty = true;
    }
}

/// The three settings entries a view color is saved in: the legacy triple, the legacy band rows
/// (`None` for no rows, and for an empty list) and the variant (the material it was picked for and
/// its snapshot). A variant color writes only the variant: the legacy entries are cleared with it,
/// like the builders of a design's selection do.
type PersistedParts = (
    ColorOverride,
    Option<Vec<[f32; 3]>>,
    Option<(String, VariantChoice)>,
);

/// The settings entries of `color` ([`PersistedParts`]).
fn persisted_parts(color: Option<&ViewColor>) -> PersistedParts {
    let Some(color) = color else {
        return (None, None, None);
    };
    if let Some(variant) = &color.variant {
        return (
            None,
            None,
            Some((variant.material.clone(), variant.choice.clone())),
        );
    }
    (
        Some(color.triple),
        color.band_rows().map(<[_]>::to_vec),
        None,
    )
}

/// The view color the settings entries restore: a variant wins over the legacy entries, and the
/// band rows only count beside a triple.
fn restored_view_color(
    triple: ColorOverride,
    bands: Option<Vec<[f32; 3]>>,
    variant: Option<(String, VariantChoice)>,
) -> Option<ViewColor> {
    if let Some((material, choice)) = variant {
        return Some(ViewColor::from_variant(material, choice));
    }
    let triple = triple?;
    Some(ViewColor {
        triple,
        bands: bands.filter(|rows| !rows.is_empty()),
        variant: None,
    })
}

/// Saves `color` (every key) in the settings file.
fn persist_view_color(color: Option<&ViewColor>) {
    if let Some(persister) = SettingsPersister::installed_for_this_thread() {
        let (triple, bands, variant) = persisted_parts(color);
        persister.update(|file| {
            file.settings.render_body_color_override = triple;
            file.settings.render_body_color_bands = bands;
            file.settings.set_render_view_variant(
                variant
                    .as_ref()
                    .map(|(material, choice)| (material.as_str(), choice)),
            );
        });
    }
}

/// Forgets the remembered view-only color (context and settings file). Called when a material
/// preview starts or ends, so a tint from an earlier preview never colors the next one.
pub fn clear_view_color(render_ctx: &Arc<Mutex<RenderContext>>) {
    update_context(&mut RenderContext::lock(render_ctx), |ctx| {
        ctx.view_body_color = None;
    });
    persist_view_color(None);
}

/// Reads the open design's material selection (`None`: it cannot be read right now).
pub type CurrentColorFn = dyn Fn() -> Option<MaterialSelection>;

/// Changes the open design's color as ONE undoable edit, or says why it could not.
pub type ApplyColorFn = dyn Fn(ColorChange) -> Result<(), String>;

/// The color variants the cutter saved for the material named.
pub type SavedVariantsFn = dyn Fn(&str) -> Vec<ColorVariant>;

/// What the editor side offers the control: reading and editing the open design's color. Kept
/// behind closures so this module needs nothing from the editor.
pub struct DesignColorPort {
    /// The open design's material selection (its color fields are what the control shows: a
    /// legacy color, or a color variant with its label and tensor). The outer `None` means it
    /// cannot be read right now (the editor state is busy); the control then keeps what it shows.
    pub current: Box<CurrentColorFn>,
    /// Changes the open design's color as ONE undoable edit (the Edit tab's color box does the
    /// same), or says why it could not.
    pub apply: Box<ApplyColorFn>,
    /// The color variants the cutter saved for the material named (the vault's "My Variants"; an
    /// empty list for a port with no vault). The list shows them for that material only.
    pub saved_variants: Box<SavedVariantsFn>,
}

/// The material whose colors the toolbar offers right now (the traced material without any color
/// applied): the host of the per-axis editor of the view-only color. `None` when the name
/// resolves to nothing.
#[must_use]
pub fn view_host(render_ctx: &Arc<Mutex<RenderContext>>) -> Option<GemMaterial> {
    RenderContext::lock(render_ctx).color_host()
}

/// The view-only color as the color fields of a selection (a variant snapshot, else the legacy
/// triple and band rows; none for the material's own color): what the per-axis editor of the view
/// seeds its rows from (`lch_color::seed_axis_lch`).
#[must_use]
pub fn view_color_selection(render_ctx: &Arc<Mutex<RenderContext>>) -> MaterialSelection {
    RenderContext::lock(render_ctx)
        .view_body_color
        .as_ref()
        .map_or_else(MaterialSelection::none, ViewColor::as_selection)
}

/// Pushes the grouped list into the Slint model.
fn push_list(ui: &MainWindow, view: &ListView, note: &str) {
    let model = ui.global::<ViewportModel>();
    let strings = |rows: &[&RowView], pick: fn(&RowView) -> &str| -> ModelRc<SharedString> {
        ModelRc::new(VecModel::from(
            rows.iter()
                .map(|row| SharedString::from(pick(row)))
                .collect::<Vec<_>>(),
        ))
    };
    let dot = |row: &RowView| {
        row.swatch
            .map_or(slint::Color::from_argb_u8(0, 0, 0, 0), |[r, g, b]| {
                slint::Color::from_rgb_u8(r, g, b)
            })
    };
    let index = |row: &RowView| i32::try_from(row.index).unwrap_or(i32::MAX);

    let rows: Vec<&RowView> = view.rows.iter().collect();
    model.set_render_color_row_labels(strings(&rows, |row| row.label.as_str()));
    model.set_render_color_row_tips(strings(&rows, |row| row.tip.as_str()));
    model.set_render_color_row_swatches(ModelRc::new(VecModel::from(
        rows.iter().map(|row| dot(row)).collect::<Vec<_>>(),
    )));
    model.set_render_color_row_has_swatch(ModelRc::new(VecModel::from(
        rows.iter()
            .map(|row| row.swatch.is_some())
            .collect::<Vec<_>>(),
    )));
    model.set_render_color_row_indexes(ModelRc::new(VecModel::from(
        rows.iter().map(|row| index(row)).collect::<Vec<_>>(),
    )));

    let presets: Vec<&RowView> = view.presets.iter().collect();
    model.set_render_color_preset_labels(strings(&presets, |row| row.label.as_str()));
    model.set_render_color_preset_tips(strings(&presets, |row| row.tip.as_str()));
    model.set_render_color_preset_swatches(ModelRc::new(VecModel::from(
        presets.iter().map(|row| dot(row)).collect::<Vec<_>>(),
    )));
    model.set_render_color_preset_first(presets.first().map_or(-1, |row| index(row)));

    model.set_render_color_legacy_index(view.legacy.as_ref().map_or(-1, index));
    model.set_render_color_legacy_label(
        view.legacy
            .as_ref()
            .map_or("", |row| row.label.as_str())
            .into(),
    );
    model.set_render_color_legacy_tip(
        view.legacy
            .as_ref()
            .map_or("", |row| row.tip.as_str())
            .into(),
    );
    model.set_render_color_legacy_swatch(
        view.legacy
            .as_ref()
            .map_or(slint::Color::from_argb_u8(0, 0, 0, 0), dot),
    );
    model.set_render_color_custom_index(view.custom.as_ref().map_or(-1, index));
    model.set_render_color_custom_tip(
        view.custom
            .as_ref()
            .map_or("", |row| row.tip.as_str())
            .into(),
    );
    model.set_render_color_legacy_note(note.into());
}

/// Pushes `view` into the Slint model.
fn push_view(ui: &MainWindow, view: &ControlView) {
    let model = ui.global::<ViewportModel>();
    model.set_render_color_index(i32::try_from(view.choice).unwrap_or(0));
    model.set_render_color_summary(view.summary.as_str().into());
    model.set_render_color_has_swatch(view.swatch.is_some());
    model.set_render_color_current(
        view.swatch
            .map_or(PICKER_START, |[r, g, b]| slint::Color::from_rgb_u8(r, g, b)),
    );
    model.set_render_color_enabled(view.enabled);
    model.set_render_color_colors_enabled(view.colors_enabled);
    model.set_render_color_physics(view.physics);
    model.set_render_color_zoned(view.zoned);
    model.set_render_color_targets_design(view.targets_design);
}

/// The list as the Slint thread last pushed it: the index `render_color_chosen` hands back is a
/// position in THIS list, never in one rebuilt after the material changed.
struct Remembered {
    items: Vec<ColorEntryItem>,
    variants: Vec<ColorVariant>,
}

thread_local! {
    /// The toolbar's list as the Slint thread last saw it (see [`Remembered`]).
    static REMEMBERED: RefCell<Option<Remembered>> = const { RefCell::new(None) };
}

/// Remembers `list` for [`remembered_entry`].
fn remember(list: &ColorList) {
    REMEMBERED.with_borrow_mut(|slot| {
        *slot = Some(Remembered {
            items: list.items.clone(),
            variants: list.variants.clone(),
        });
    });
}

/// The entry at list position `index` of the list last shown, with the variants behind it;
/// `None` before any list was shown or for a position outside it.
fn remembered_entry(index: i32) -> Option<(ColorEntry, Vec<ColorVariant>)> {
    let at = usize::try_from(index).ok()?;
    REMEMBERED.with_borrow(|slot| {
        let remembered = slot.as_ref()?;
        Some((
            remembered.items.get(at)?.entry.clone(),
            remembered.variants.clone(),
        ))
    })
}

/// What the control reads from the render context in one lock.
struct RenderColorState {
    /// Who a pick changes right now.
    target: ColorTarget,
    /// The traced material defines its own color (a physics recipe).
    physics: bool,
    /// The traced material has color zones.
    zoned: bool,
    /// The view-only color.
    view_color: Option<ViewColor>,
    /// The stone size the render uses (`0.0` = not set).
    stone_width_mm: f32,
    /// The strip of zone colors of a zoned material.
    zone_colors: Vec<[f32; 3]>,
    /// The material whose colors the control offers.
    host: Option<GemMaterial>,
}

impl RenderColorState {
    /// Reads the state under one lock of the render context.
    fn read(render_ctx: &Arc<Mutex<RenderContext>>) -> Self {
        let ctx = RenderContext::lock(render_ctx);
        Self {
            target: ctx.color_target(),
            physics: ctx.physics_color(),
            zoned: ctx.zoned_material(),
            view_color: ctx.view_body_color.clone(),
            stone_width_mm: ctx.stone_width_mm,
            zone_colors: ctx.zone_swatch_colors(),
            host: ctx.color_host(),
        }
    }
}

/// Recomputes the control from the render context and the open design.
fn refresh(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>, port: &DesignColorPort) {
    let RenderColorState {
        target,
        physics,
        zoned,
        view_color,
        stone_width_mm,
        zone_colors,
        host,
    } = RenderColorState::read(render_ctx);
    // The Material chip's strip of zone colors (empty: the single dot).
    ui.global::<ViewportModel>()
        .set_render_color_zone_swatches(ModelRc::new(VecModel::from(
            zone_colors
                .into_iter()
                .map(|[r, g, b]| slint::Color::from_rgb_f32(r, g, b))
                .collect::<Vec<_>>(),
        )));
    let design = if target == ColorTarget::Design {
        let Some(current) = (port.current)() else {
            return;
        };
        current
    } else {
        MaterialSelection::none()
    };
    let view = applicable_view_color(view_color, host.as_ref())
        .as_ref()
        .map_or_else(MaterialSelection::none, ViewColor::as_selection);
    let shown = shown_selection(target, physics || zoned, &design, &view);
    let saved = host
        .as_ref()
        .map_or_else(Vec::new, |host| (port.saved_variants)(&host.name));
    let list = color_list(host.as_ref(), &saved, &shown);
    remember(&list);
    push_list(
        ui,
        &list_view(&list, stone_width_mm),
        legacy_note(&shown, host.as_ref()),
    );
    push_view(
        ui,
        &control_view(&ControlInput {
            target,
            physics,
            zoned,
            shown: &shown,
            items: &list.items,
            host: Some(&list.host),
            stone_width_mm,
        }),
    );
    ui.global::<crate::Zoning>().invoke_refresh_stone_pattern();
}

/// Applies a pick where it belongs right now: the open design (one undo step) or the view
/// setting (remembered), then refreshes the control.
fn apply_pick(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    port: &DesignColorPort,
    pick: &Pick,
) {
    let (target, own_color) = {
        let ctx = RenderContext::lock(render_ctx);
        (
            ctx.color_target(),
            ctx.physics_color() || ctx.zoned_material(),
        )
    };
    if own_color {
        // The control is greyed out for these (physics recipe, color zones); never recolor
        // a material's own color.
        return;
    }
    match target {
        ColorTarget::Design => {
            if let Err(message) = (port.apply)(pick.change()) {
                show_toast(ui, &message, "error");
            }
        }
        ColorTarget::View => store_view_color(render_ctx, pick.view_color()),
    }
    refresh(ui, render_ctx, port);
}

/// Makes `color` the view-only color (context, dirty flag, settings file); a pattern scoped to
/// the preview gives way to it.
fn store_view_color(render_ctx: &Arc<Mutex<RenderContext>>, color: Option<ViewColor>) {
    persist_view_color(color.as_ref());
    update_context(&mut RenderContext::lock(render_ctx), |ctx| {
        let _ = ctx.set_view_color_pattern(None);
        ctx.view_body_color = color;
    });
}

/// The pick a finished view color stands for when the open design takes it instead.
fn pick_of_view_color(color: ViewColor) -> Pick {
    let ViewColor {
        triple, variant, ..
    } = color;
    match variant {
        Some(variant) => Pick::Variant {
            material: variant.material,
            choice: variant.choice,
        },
        None => Pick::Triple(triple),
    }
}

/// Applies a color from the color editor as the view-only color: a variant (the per-axis Custom
/// Color, or a picked variant) keeps every direction of a pleochroic stone, a legacy color is the
/// triple every consumer can show plus the seven-band rows, which win in the render. While the
/// open design takes the pick instead (the view changed under the popup) it is the design's own
/// edit, as for any pick.
pub fn apply_view_color(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    port: &DesignColorPort,
    color: ViewColor,
) {
    ui.global::<ViewportModel>().set_render_color_busy(false);
    let (target, own_color) = {
        let ctx = RenderContext::lock(render_ctx);
        (
            ctx.color_target(),
            ctx.physics_color() || ctx.zoned_material(),
        )
    };
    if own_color {
        return;
    }
    match target {
        ColorTarget::Design => apply_pick(ui, render_ctx, port, &pick_of_view_color(color)),
        ColorTarget::View => {
            store_view_color(render_ctx, Some(color));
            refresh(ui, render_ctx, port);
        }
    }
}

/// Applies the per-axis Custom Color `choice` (`lch_color::custom_choice`) as the view-only color
/// of the viewed material: [`apply_view_color`] with a variant color named after the host.
pub fn apply_view_choice(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    port: &DesignColorPort,
    choice: VariantChoice,
) {
    let material = view_host(render_ctx).map_or_else(String::new, |host| host.name);
    apply_view_color(
        ui,
        render_ctx,
        port,
        ViewColor::from_variant(material, choice),
    );
}

/// Wires the toolbar's "Color" control: pushes the list, restores the remembered view
/// color, and registers the model's callbacks. `port` is the editor's side of the design
/// color (see [`DesignColorPort`]).
pub fn setup_render_body_color_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    port: &Rc<DesignColorPort>,
) {
    // Restore the remembered view color (variant, or the legacy triple and band rows) and the
    // link state the context mirrors.
    let saved = SettingsPersister::installed_for_this_thread().and_then(|persister| {
        let settings = persister.snapshot().settings;
        let variant = settings.render_view_variant();
        restored_view_color(
            settings.render_body_color_override,
            settings.render_body_color_bands,
            variant,
        )
    });
    let linked = ui.global::<ViewportModel>().get_viewport_material_linked();
    update_context(&mut RenderContext::lock(render_ctx), |ctx| {
        ctx.view_body_color = saved;
        ctx.material_linked = linked;
    });

    let model = ui.global::<ViewportModel>();

    let (ui_weak, ctx, port_refresh) = (ui.as_weak(), Arc::clone(render_ctx), Rc::clone(port));
    model.on_render_color_refresh(move || {
        if let Some(ui) = ui_weak.upgrade() {
            refresh(&ui, &ctx, &port_refresh);
        }
    });

    // The view's material link moved: the render pipeline mirrors it (it decides whether the
    // view color or the design's own color applies), then the control follows.
    let (ui_weak, ctx, port_link) = (ui.as_weak(), Arc::clone(render_ctx), Rc::clone(port));
    model.on_render_color_context_changed(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let linked = ui.global::<ViewportModel>().get_viewport_material_linked();
        update_context(&mut RenderContext::lock(&ctx), |c| {
            c.material_linked = linked;
        });
        refresh(&ui, &ctx, &port_link);
    });

    // A row was clicked: the position is in the list the popup last showed.
    let (ui_weak, ctx, port_pick) = (ui.as_weak(), Arc::clone(render_ctx), Rc::clone(port));
    model.on_render_color_chosen(move |index| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let Some((entry, variants)) = remembered_entry(index) else {
            return;
        };
        let Some(pick) = pick_for_entry(&entry, &variants) else {
            return;
        };
        apply_pick(&ui, &ctx, &port_pick, &pick);
    });

    refresh(ui, render_ctx, port);
}

#[cfg(test)]
mod tests;
