//! The color forms of the zone color picker: how many colors a picked zone color has, by the rank
//! of the stone's material, and which slot a "Pick..." button stands for.
//!
//! A cubic or amorphous material shows one color, a uniaxial one an Along and an Across color, a
//! biaxial one an Alpha, a Beta and a Gamma color. The picker offers the form of the material's
//! rank; a zone that holds another form keeps it and lists it too, so a stored one-color zone on a
//! pleochroic material reads "Legacy: No Pleochroism" (owner decision D2) and renders as saved
//! until the cutter converts it.

use pleochroma::optics::materials::variants::TensorRank;
use pleochroma_cut_core::material::pattern::{ColorForm, ColorSlot, PatternColor};

/// `ColorPatternModel.color_kind`: one picked color.
pub(super) const KIND_SINGLE: i32 = 0;
/// `ColorPatternModel.color_kind`: Along and Across.
pub(super) const KIND_DICHROIC: i32 = 1;
/// `ColorPatternModel.color_kind`: Alpha, Beta and Gamma.
pub(super) const KIND_TRICHROIC: i32 = 2;
/// `ColorPatternModel.color_kind`: a color variant of the material.
pub(super) const KIND_VARIANT: i32 = 3;

/// The entry of the form menu for a one-color zone on a pleochroic material.
pub(super) const LEGACY_LABEL: &str = "Legacy: No Pleochroism";

/// The sentence under the form menu of an isotropic material (the plan's R2 wording): why a zone
/// has one color only and what to change for two.
pub(super) const ONE_COLOR_HINT: &str = "Two colors need a birefringent material. In the Gem Material Editor set a birefringence other than 0 and a crystal system other than Cubic (shown in Advanced mode), save the material, then reopen the pattern.";

/// The sentence under the form menu of a uniaxial material.
const UNIAXIAL_HINT: &str =
    "A uniaxial crystal shows one color along its axis (Along) and another across it (Across).";

/// The sentence under the form menu of a biaxial material.
const BIAXIAL_HINT: &str = "A biaxial crystal shows its own color along each of its three principal directions: Alpha, Beta and Gamma.";

/// The sentence under the form menu of a legacy one-color zone.
const LEGACY_HINT: &str = "This zone has one color, so it looks the same from every direction. It renders as it was saved. Choose the form of this material to give it one color per direction.";

/// `ColorPatternModel.color_kind` of a zone color.
pub(super) const fn kind_of(color: &PatternColor) -> i32 {
    match color {
        PatternColor::Single(_) => KIND_SINGLE,
        PatternColor::Dichroic { .. } => KIND_DICHROIC,
        PatternColor::Trichroic { .. } => KIND_TRICHROIC,
        PatternColor::Variant { .. } => KIND_VARIANT,
    }
}

/// The form of a picked color; `None` for a variant.
pub(super) const fn form_of(color: &PatternColor) -> Option<ColorForm> {
    match color {
        PatternColor::Single(_) => Some(ColorForm::Single),
        PatternColor::Dichroic { .. } => Some(ColorForm::Dichroic),
        PatternColor::Trichroic { .. } => Some(ColorForm::Trichroic),
        PatternColor::Variant { .. } => None,
    }
}

/// The form a material of `rank` asks of a picked color.
pub(super) const fn host_form(rank: TensorRank) -> ColorForm {
    match rank {
        TensorRank::Isotropic => ColorForm::Single,
        TensorRank::Uniaxial => ColorForm::Dichroic,
        TensorRank::Biaxial => ColorForm::Trichroic,
    }
}

/// The menu label of `form`.
const fn form_label(form: ColorForm) -> &'static str {
    match form {
        ColorForm::Single => "One Color",
        ColorForm::Dichroic => "Along and Across",
        ColorForm::Trichroic => "Alpha, Beta and Gamma",
    }
}

/// The entries of the form menu of one zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FormOptions {
    /// The form of each entry.
    pub forms: Vec<ColorForm>,
    /// The label of each entry.
    pub labels: Vec<&'static str>,
    /// The entry the zone has.
    pub index: usize,
}

/// The form menu for a picked `color` on a material of `rank`: the material's own form first, then
/// the form the zone has when that is another (the legacy one-color zone of a pleochroic material,
/// two colors on a biaxial one, any form the material cannot show). `None` for a variant.
pub(super) fn form_options(rank: TensorRank, color: &PatternColor) -> Option<FormOptions> {
    let current = form_of(color)?;
    let own = host_form(rank);
    let mut forms = vec![own];
    let mut labels = vec![form_label(own)];
    let index = usize::from(current != own);
    if current != own {
        forms.push(current);
        labels.push(
            if current == ColorForm::Single && rank != TensorRank::Isotropic {
                LEGACY_LABEL
            } else {
                form_label(current)
            },
        );
    }
    Some(FormOptions {
        forms,
        labels,
        index,
    })
}

/// The sentence under the form menu for a picked `color` on a material of `rank`.
pub(super) fn form_hint(rank: TensorRank, color: &PatternColor) -> &'static str {
    if form_of(color) == Some(ColorForm::Single) && rank != TensorRank::Isotropic {
        return LEGACY_HINT;
    }
    match rank {
        TensorRank::Isotropic => ONE_COLOR_HINT,
        TensorRank::Uniaxial => UNIAXIAL_HINT,
        TensorRank::Biaxial => BIAXIAL_HINT,
    }
}

/// The slot the "Pick..." button `which` stands for in a zone holding `color`: 0 the one color,
/// then in slot order 1 Along or Alpha, 2 Across or Beta, 3 Gamma. `None` for a button the zone's
/// form does not have (and for a variant, which has nothing to pick).
pub(super) const fn slot_for(color: &PatternColor, which: i32) -> Option<ColorSlot> {
    match (color, which) {
        (PatternColor::Single(_), 0) => Some(ColorSlot::Single),
        (PatternColor::Dichroic { .. }, 1) => Some(ColorSlot::Along),
        (PatternColor::Dichroic { .. }, 2) => Some(ColorSlot::Across),
        (PatternColor::Trichroic { .. }, 1) => Some(ColorSlot::Alpha),
        (PatternColor::Trichroic { .. }, 2) => Some(ColorSlot::Beta),
        (PatternColor::Trichroic { .. }, 3) => Some(ColorSlot::Gamma),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::material::pattern::{PatternTensorRows, ZoneColor};

    fn one() -> PatternColor {
        PatternColor::Single(ZoneColor::unsolved([50.0, 30.0, 100.0]))
    }

    fn two() -> PatternColor {
        PatternColor::Dichroic {
            along: ZoneColor::unsolved([50.0, 30.0, 100.0]),
            across: ZoneColor::unsolved([50.0, 30.0, 200.0]),
        }
    }

    fn three() -> PatternColor {
        PatternColor::Trichroic {
            alpha: ZoneColor::unsolved([50.0, 30.0, 100.0]),
            beta: ZoneColor::unsolved([50.0, 30.0, 150.0]),
            gamma: ZoneColor::unsolved([50.0, 30.0, 200.0]),
        }
    }

    fn variant() -> PatternColor {
        PatternColor::Variant {
            key: "sapphire.test".to_owned(),
            label: "Test".to_owned(),
            absorption: PatternTensorRows {
                o_ray: Vec::new(),
                e_ray: Vec::new(),
                beta_ray: None,
                pleochroic: true,
            },
        }
    }

    #[test]
    fn the_picker_offers_the_form_of_the_rank() {
        let iso = form_options(TensorRank::Isotropic, &one()).expect("custom");
        assert_eq!(iso.labels, ["One Color"]);
        assert_eq!(iso.index, 0);
        let uni = form_options(TensorRank::Uniaxial, &two()).expect("custom");
        assert_eq!(uni.labels, ["Along and Across"]);
        assert_eq!(uni.forms, [ColorForm::Dichroic]);
        let bi = form_options(TensorRank::Biaxial, &three()).expect("custom");
        assert_eq!(bi.labels, ["Alpha, Beta and Gamma"]);
    }

    #[test]
    fn a_one_color_zone_on_a_pleochroic_material_is_labelled_legacy() {
        for (rank, own) in [
            (TensorRank::Uniaxial, ColorForm::Dichroic),
            (TensorRank::Biaxial, ColorForm::Trichroic),
        ] {
            let options = form_options(rank, &one()).expect("custom");
            assert_eq!(options.forms, [own, ColorForm::Single]);
            assert_eq!(options.labels[1], "Legacy: No Pleochroism");
            assert_eq!(options.index, 1, "the zone has the legacy form");
            assert_eq!(form_hint(rank, &one()), LEGACY_HINT);
        }
        // Not legacy on a material that shows one color.
        assert_ne!(
            form_options(TensorRank::Isotropic, &one())
                .expect("custom")
                .labels,
            [LEGACY_LABEL]
        );
    }

    #[test]
    fn two_colors_on_a_biaxial_material_and_too_many_on_a_simple_one_stay_listed() {
        let bi = form_options(TensorRank::Biaxial, &two()).expect("custom");
        assert_eq!(bi.forms, [ColorForm::Trichroic, ColorForm::Dichroic]);
        assert_eq!(bi.labels[1], "Along and Across");
        let iso = form_options(TensorRank::Isotropic, &three()).expect("custom");
        assert_eq!(iso.forms, [ColorForm::Single, ColorForm::Trichroic]);
        assert_eq!(iso.index, 1);
    }

    #[test]
    fn a_variant_has_no_form_and_nothing_to_pick() {
        assert_eq!(form_options(TensorRank::Uniaxial, &variant()), None);
        assert_eq!(form_of(&variant()), None);
        assert_eq!(kind_of(&variant()), KIND_VARIANT);
        for which in 0..4 {
            assert_eq!(slot_for(&variant(), which), None);
        }
    }

    #[test]
    fn the_pick_buttons_map_to_the_slots_of_the_zone_form() {
        assert_eq!(slot_for(&one(), 0), Some(ColorSlot::Single));
        assert_eq!(slot_for(&one(), 1), None);
        assert_eq!(slot_for(&two(), 1), Some(ColorSlot::Along));
        assert_eq!(slot_for(&two(), 2), Some(ColorSlot::Across));
        assert_eq!(slot_for(&two(), 0), None);
        assert_eq!(slot_for(&three(), 1), Some(ColorSlot::Alpha));
        assert_eq!(slot_for(&three(), 2), Some(ColorSlot::Beta));
        assert_eq!(slot_for(&three(), 3), Some(ColorSlot::Gamma));
        assert_eq!(slot_for(&three(), 4), None);
    }

    #[test]
    fn the_host_form_follows_the_rank() {
        assert_eq!(host_form(TensorRank::Isotropic), ColorForm::Single);
        assert_eq!(host_form(TensorRank::Uniaxial), ColorForm::Dichroic);
        assert_eq!(host_form(TensorRank::Biaxial), ColorForm::Trichroic);
        assert_eq!(kind_of(&one()), KIND_SINGLE);
        assert_eq!(kind_of(&two()), KIND_DICHROIC);
        assert_eq!(kind_of(&three()), KIND_TRICHROIC);
        assert_eq!(form_hint(TensorRank::Isotropic, &one()), ONE_COLOR_HINT);
    }
}
