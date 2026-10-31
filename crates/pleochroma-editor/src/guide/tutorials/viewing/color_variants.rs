//! Color variants: picking a named color of a material for a design, setting the color of each
//! direction by hand and keeping it as a variant of your own, and taking the color of a zone of a
//! color pattern from a variant (manual chapters "Materials" and "Custom Materials and Color
//! Patterns").
//!
//! Both lessons are desktop-only: the web build has the Body Color list and the Custom editor,
//! but no Material Editor with a Color Variants section and no Color Pattern window (the offer
//! table marks them class C). Their goals read the design where it holds the result: the color a
//! design carries is a snapshot of the variant in its material selection, and a stone's own
//! pattern is its material selection's pattern override. What only happens on screen is the event
//! the Color Pattern window already reports (see [`crate::guide::tutorials::viewing_events`]).

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{VIEWPORT, check, event, path_traced_view};
use crate::guide::{
    Goal, GoalContext, Group, Guide, GuideCategory, GuideStep, Perform, StartingState,
    tutorials::viewing_events::{PATTERN_EDITOR_OPENED, PATTERN_SAVED, PATTERN_TEMPLATE_PICKED},
};
use pleochroma_cut_core::material::{CUSTOM_VARIANT_KEY, pattern::PatternColor};

/// The Stone tab's controls (the `DesignSettings` group gates them) and Undo.
const EFFECTS: &[Group] = &[Group::DesignSettings, Group::History];

/// The start of the key of a variant the cutter saved (`user.<uuid>`).
const SAVED_KEY_PREFIX: &str = "user.";

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![color_variants(), zone_colors_from_variants()]
}

/// The design carries a color picked from a material's list of variants (a built-in or a saved
/// one), not a Custom Color and not the material's own color.
fn variant_picked(ctx: &GoalContext<'_>) -> bool {
    ctx.design
        .material
        .color_variant_override
        .as_ref()
        .is_some_and(|choice| choice.key != CUSTOM_VARIANT_KEY)
}

/// The design carries a Custom Color made in the per-direction editor and applied.
fn custom_color_applied(ctx: &GoalContext<'_>) -> bool {
    ctx.design
        .material
        .color_variant_override
        .as_ref()
        .is_some_and(|choice| choice.key == CUSTOM_VARIANT_KEY)
}

/// The design carries a variant the cutter saved under a name of their own.
fn saved_variant_picked(ctx: &GoalContext<'_>) -> bool {
    ctx.design
        .material
        .color_variant_override
        .as_ref()
        .is_some_and(|choice| choice.key.starts_with(SAVED_KEY_PREFIX))
}

/// The stone's own color pattern has a zone (the base zone included) whose color is a variant.
fn zone_takes_variant(ctx: &GoalContext<'_>) -> bool {
    ctx.design
        .material
        .color_pattern_override
        .as_ref()
        .is_some_and(|pattern| {
            std::iter::once(&pattern.base)
                .chain(&pattern.zones)
                .any(|zone| matches!(zone.color, PatternColor::Variant { .. }))
        })
}

/// The step that picks `material` in the Stone tab.
fn pick_material(material: &str, intro: &'static str, why: &'static str) -> GuideStep {
    GuideStep::new(format!("Choose {material}"), intro)
        .actions([
            "Open the Stone tab of the Parameters dock (View > Stone Properties, or Ctrl+Alt+1)."
                .to_owned(),
            format!("Set Material to {material}. It applies at once."),
        ])
        .check(format!("The Stone tab shows {material}."))
        .why(why)
        .goal(
            Goal::Material(material.to_owned()),
            format!("the material set to {material}"),
        )
        .highlight("stone_tab")
        .allow(EFFECTS)
}

/// The step that switches the Path-Traced view on, where the color of each direction shows.
fn path_traced(intro: &'static str) -> GuideStep {
    GuideStep::new("Look at it in Path-Traced", intro)
        .actions([
            "Click Path-Traced in the row of view pills above the picture, and give the picture a moment to settle.",
        ])
        .check("The picture shows the stone lit with traced light, in the colors you chose.")
        .why("The Solid view is flat-shaded and shows no color of the material. From Solid, the click gives Solid combined with Path-Traced, which shows it too; click Solid to turn the flat layer off.")
        .goal(path_traced_view(), "The Path-Traced view")
        .perform(Perform::ViewMode(1))
        .highlight("view_modes")
        .allow(VIEWPORT)
}

fn color_variants() -> Guide {
    Guide::new(
        "materials-color-variants",
        "Color Variants",
        "Show a sapphire in one of its color variants, set the color of each direction yourself, and keep it as a variant of your own.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "What a color variant is",
            "A color variant is a named color of one material, such as a yellow or a pink sapphire. You pick it for a design without making a custom material.",
        )
        .actions([
            "A variant keeps everything else about the material and changes only how it absorbs light.",
            "A crystal that shows a different color in each direction (pleochroism) keeps that in every variant, so a variant is a color for each direction, not one flat color.",
            "Material Default is the material's own color. Many built-in variants are fitted to measured spectra, but almost all of them use a strength chosen for a typical face-up color, and the tip on each estimated one says which part is estimated.",
            "A variant you save yourself is listed after the built-in ones, for every design of that material.",
        ])
        .why("A design keeps its own copy of the color it uses, so renaming or deleting a saved variant later never changes a design.")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(pick_material(
        "Sapphire",
        "A sapphire is a crystal with one special direction, so its variants hold a color for each of the two directions.",
        "A material that looks the same in every direction, such as Diamond, lists a few generic colors instead of variants.",
    ))
    .step(
        GuideStep::new(
            "Pick a built-in variant",
            "The Body Color list of the Stone tab holds the material's own color, its color variants and Custom.",
        )
        .actions([
            "In the Stone tab find Body Color and open the list.",
            "A sapphire lists Material Default first, then its color variants, and Custom... last. The generic colors are not offered, because a sapphire shows a different color in each direction.",
            "Pick Yellow, or any other variant of the list. It applies at once.",
        ])
        .check("A swatch beside the list shows the face-up color, and the line under the list names the color Along the Axis and Across the Axis.")
        .why("It is one step in the history and Undo takes it back. Hover the swatch to read the same words as a tip.")
        .goal(check("a color variant chosen", variant_picked), "a color variant chosen")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(path_traced(
        "The color of each direction shows where light travels through the stone, so the Path-Traced view is where to look.",
    ))
    .step(
        GuideStep::new(
            "Set the Along color yourself",
            "Custom opens an editor with one row for each direction of the stone.",
        )
        .actions([
            "In the Stone tab set Body Color to Custom.... The color editor opens under the list.",
            "A sapphire has two rows, Along the Axis and Across the Axis. Each shows a swatch and the coarse color of that direction. Under the sliders, Face-Up and one swatch for each direction show the solved colors.",
            "Click the Along the Axis row, move Tone, Saturation and Hue to a color you like, and wait for Matching... to end.",
            "Click Apply Color.",
        ])
        .check("The list shows Custom, and the swatches show the color of each direction.")
        .why("A direction you do not touch keeps its color, so the stone keeps its color shading by direction. The color is the closest one the material model can reach at the size of the stone.")
        .goal(check("a Custom Color applied", custom_color_applied), "a Custom Color applied")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Save it as a variant",
            "A Custom Color can be kept under a name and picked again for any design of the same material.",
        )
        .actions([
            "Click Choose Color..., under the Body Color list, to open the editor again.",
            "Move a slider a little, so the colors are matched again, and wait for Matching... to end. Save as Variant... stays dimmed until they are.",
            "Click Save as Variant.... A name row opens with a suggested name. Type a name of your own, for example Sunset Sapphire, and click Save.",
        ])
        .check("The Body Color list shows your name, and the design uses that variant.")
        .why("The variant is saved on this computer for the material, and it is one step in the history. The design keeps its own copy of the color.")
        .goal(
            check("a saved variant on the design", saved_variant_picked),
            "a saved variant chosen",
        )
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Find it in Color Variants",
            "The Material Editor lists every color variant of a material, the built-in ones and your own.",
        )
        .actions([
            "Read this step, then click New/Edit Material... in the Stone tab. The Material Editor opens over the window.",
            "Scroll to Color Variants. Its Material list starts on the material of this stone. Your variant sits after the built-in ones and says Saved by you.",
            "Built-in variants are read-only, and the estimated ones carry an Estimated tag. Duplicate makes a copy of any variant that you can rename and delete. Rename and Delete work on your own, and Delete asks again first.",
            "Close the Material Editor with Cancel when you are done.",
        ])
        .why("Renaming or deleting a variant never changes a design that already uses it. Every control is unlocked again."),
    )
}

fn zone_colors_from_variants() -> Guide {
    Guide::new(
        "materials-zone-colors-from-variants",
        "Zone Colors From Variants",
        "Give the rim and the core of a tourmaline the colors of its Verdelite and Rubellite variants in the Color Pattern window.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "A zone color from a variant",
            "A zone of a color pattern can take its color from a color variant of the stone's material, instead of a color you pick.",
        )
        .actions([
            "Rubellite is the pink of a tourmaline and Verdelite its green. A variant has a color for each direction of the crystal, which one picked color cannot have.",
            "The zone keeps its own copy of the variant, so changing or deleting the variant later does not change the stone.",
            "This lesson zones a tourmaline like a watermelon: a Verdelite rim around a Rubellite core.",
        ])
        .why("A pattern that uses a variant can be opened only by a program that has color variants.")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(pick_material(
        "Tourmaline",
        "A tourmaline is a crystal with one special direction, and it has variants named for its colors.",
        "A zone takes its variants from the stone's own material, so the list of variants in the Color Pattern window changes with the material.",
    ))
    .step(
        GuideStep::new(
            "Open the Color Pattern window",
            "The Color Pattern window opens over the editor, so this step and the next two carry everything you need.",
        )
        .actions([
            "In the Stone tab find Color Zoning.",
            "Click Color Pattern.... The Color Pattern window opens.",
        ])
        .check("A window called Color Pattern opens with a list of ZONES, a Preview and the controls of the selected zone.")
        .why("Apply stores the pattern with this stone and keeps the window open, Done stores it and closes the window, and Cancel closes it and keeps the pattern as it was.")
        .goal(event(PATTERN_EDITOR_OPENED), "The Color Pattern window opened")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Start from the Watermelon template",
            "A template gives the stone its zones, and the variants then color them.",
        )
        .actions([
            "In the row of Templates click Watermelon, and wait for Solving colors... to end.",
        ])
        .check("The ZONES list shows a Rim and a Core.")
        .why("The template's zones start with one picked color each. The next step changes where their colors come from.")
        .goal(event(PATTERN_TEMPLATE_PICKED), "The Watermelon template chosen")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Take the zone colors from variants",
            "Source says where the color of the selected zone comes from: Custom, a color you pick, or Variant, a color variant of the material.",
        )
        .actions([
            "In ZONES click Core. Under Color, set Source to Variant.",
            "In the Variant list pick Rubellite. A split swatch beside the list shows its color along and across the axis. If the list has no Rubellite, pick any other variant of it.",
            "Click Rim, set Source to Variant and pick Verdelite.",
            "Click Apply.",
        ])
        .check("A toast says Applied the color pattern to this stone, and the stone shows a green rim around a pink core.")
        .why("The Variant list holds the material's own color first, then its variants, then the variants you saved. Setting Source back to Custom turns the zone into a color you pick, starting from the variant's colors.")
        .goal(
            Goal::All(vec![
                event(PATTERN_SAVED),
                check("a zone colored from a variant", zone_takes_variant),
            ]),
            "A zone colored from a variant, applied",
        )
        .allow(EFFECTS),
    )
    .step(path_traced(
        "The zones show where light travels through the stone, so the Path-Traced view is where to look.",
    ))
    .step(
        GuideStep::new(
            "Colors, Source and typical zoning",
            "A zone you color with Custom has a Colors menu next to its Source, and a stone offers typical patterns that use variants.",
        )
        .actions([
            "Click Done in the Color Pattern window if it is still open.",
            "Colors says how many colors a zone you pick has: One Color, Along and Across for a crystal with two directions, or Alpha, Beta and Gamma for three. A one-color zone on such a crystal reads Legacy: No Pleochroism and shows the same color in every direction.",
            "In the Stone tab, Typical Zoning for Tourmaline now colors its watermelon patterns with the Rubellite and Verdelite variants, when the material has them.",
        ])
        .why("Every control is unlocked again."),
    )
}
