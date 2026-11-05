//! The look a material shows only through a design effect: precious opal's play-of-color,
//! labradorite's schiller, moonstone's adularescence, white opal's milky body and labradorite's
//! translucent one.
//!
//! These effects are design choices, not part of the material (see
//! `feldspars_and_precious_opal`), so a stone switched to one of these materials used to render
//! as clear glass until the effect was turned on by hand. Choosing the material now switches its
//! look on, wherever a material is chosen (the Stone tab, the command line's `--material`), when
//! the design has none of it yet: [`MaterialLook::of`] is the one rule both read.

use super::GemMaterial;
use crate::optics::inclusions::bragg::StructureSpec;

/// The inclusion preset whose sheen is Moonstone's look, in the catalogue's spelling.
pub const MOONSTONE_PRESET: &str = "Adularescence";

/// What choosing a material switches on when the design has none of it yet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaterialLook {
    /// The play-of-color or schiller structure: the material's own default
    /// ([`StructureSpec::default_for`]).
    pub structure: Option<StructureSpec>,
    /// The inclusion preset whose sheen is the stone's look.
    pub inclusion_preset: Option<&'static str>,
    /// The haze (`sigma_s` per model unit) of a stone with a milky or translucent body: the
    /// material's [`GemMaterial::recommended_scattering`].
    pub haze_sigma_s: Option<f32>,
}

impl MaterialLook {
    /// The look of the material called `name`: Precious Opal (White) and (Black) and
    /// Labradorite get their structure, Moonstone its adularescence, the white opal also its
    /// milky potch and the labradorite its translucent body. Every other material has none.
    #[must_use]
    pub fn of(name: &str) -> Self {
        let hazy_body = matches!(name, "Precious Opal (White)" | "Labradorite");
        Self {
            structure: StructureSpec::default_for(name),
            inclusion_preset: (name == "Moonstone").then_some(MOONSTONE_PRESET),
            haze_sigma_s: hazy_body
                .then(|| GemMaterial::by_name(name).map(|m| m.recommended_scattering().0))
                .flatten()
                .filter(|sigma_s| *sigma_s > 0.0),
        }
    }

    /// Whether choosing the material switches nothing on.
    #[must_use]
    pub const fn is_none(&self) -> bool {
        self.structure.is_none() && self.inclusion_preset.is_none() && self.haze_sigma_s.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optics::inclusions::catalogue;

    #[test]
    fn the_structural_color_stones_have_a_look_and_the_others_none() {
        for name in [
            "Precious Opal (White)",
            "Precious Opal (Black)",
            "Labradorite",
        ] {
            let look = MaterialLook::of(name);
            assert_eq!(look.structure, StructureSpec::default_for(name), "{name}");
            assert!(look.structure.is_some(), "{name} has a structure");
        }
        let moonstone = MaterialLook::of("Moonstone");
        assert_eq!(moonstone.inclusion_preset, Some(MOONSTONE_PRESET));
        assert!(moonstone.structure.is_none());
        assert!(
            catalogue::find_preset("Moonstone", MOONSTONE_PRESET).is_some(),
            "the catalogue offers the preset for Moonstone"
        );
        let white = MaterialLook::of("Precious Opal (White)");
        assert!(
            white.haze_sigma_s.is_some_and(|s| s > 1.0),
            "white opal is milky"
        );
        let labradorite = MaterialLook::of("Labradorite");
        assert!(
            labradorite
                .haze_sigma_s
                .is_some_and(|s| s > 0.0 && s < white.haze_sigma_s.unwrap_or(0.0)),
            "labradorite is translucent, less hazy than white opal (more hides its schiller)"
        );
        assert!(
            MaterialLook::of("Precious Opal (Black)")
                .haze_sigma_s
                .is_none()
        );
        for name in ["Sapphire", "Opal", "Diamond", "Not A Material"] {
            assert!(MaterialLook::of(name).is_none(), "{name} has no look");
        }
    }
}
