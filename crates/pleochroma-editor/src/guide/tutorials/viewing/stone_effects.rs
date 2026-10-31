//! The Stone tab's effects: inclusion haze, frosted girdle, edge rounding and stone width, each
//! seen in the Path-Traced view.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{VIEWPORT, event, path_traced_view};
use crate::guide::{
    Group, Guide, GuideCategory, GuideStep, Perform, StartingState,
    tutorials::viewing_events::{
        STONE_EDGE_ROUNDING_CHANGED, STONE_GIRDLE_FROSTED_CHANGED, STONE_HAZE_CHANGED,
        STONE_WIDTH_SET,
    },
};

/// The Stone tab's controls (the `DesignSettings` group gates them) and Undo.
const EFFECTS: &[Group] = &[Group::DesignSettings, Group::History];

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![stone_effects()]
}

fn stone_effects() -> Guide {
    Guide::new(
        "viewing-stone-effects",
        "Stone Effects",
        "Add haze, a frosted girdle, rounded edges and a stone width to the picture, and see each one in Path-Traced.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "What the Stone tab can change",
            "The Stone tab holds four effects that change how the stone looks in a render. They belong to the design, not to the program.",
        )
        .actions([
            "Inclusion Haze is a milky veil inside the stone, from clean to milky.",
            "Frosted Girdle draws the girdle band ground (matt) instead of polished.",
            "Edge Rounding softens the edges between facets from perfectly sharp toward a faint rounded glint.",
            "Stone Width sets how wide the stone is across the girdle in millimeters. A wider stone looks deeper in color.",
            "Open the Stone tab with the gem button above the picture (or Ctrl+Alt+1): the effects are under Material Effects, and Stone Width is under Size.",
        ])
        .why("They are saved with the design, so it opens the way you left it. Each change is one step in the history and Undo takes it back.")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Look at the stone in Path-Traced",
            "The effects show where light travels through the stone, so the Path-Traced view is where to look.",
        )
        .actions([
            "Click Path-Traced in the row of view pills above the picture, and give the picture a moment to settle.",
        ])
        .check("The picture shows the stone lit with traced light, and the pills show Path-Traced as chosen, with Solid chosen as well or on its own.")
        .why("The Solid view is flat-shaded and does not show haze, a frosted girdle or rounded edges. From Solid, the click gives Solid combined with Path-Traced, which shows them; click Solid to turn the flat layer off.")
        .goal(path_traced_view(), "The Path-Traced view")
        .perform(Perform::ViewMode(1))
        .highlight("view_modes")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Add inclusion haze",
            "Tiny inclusions scatter the light inside a stone and give it a milky veil, the way silk does in a sapphire.",
        )
        .actions([
            "Open the Stone tab if it is not open, and find Inclusion Haze under Material Effects.",
            "Drag the Inclusion Haze slider about a third of the way to the right and let go.",
        ])
        .check("The value beside the slider changes from Off to a number, and the picture clouds over after a moment.")
        .why("The slider is finer at the clean end: a quarter of the way is already a light silk. The picture follows the slider at once; the change is written to the design when you let go.")
        .goal(event(STONE_HAZE_CHANGED), "Inclusion Haze changed")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Frost the girdle",
            "A cut stone's girdle is often left ground instead of polished.",
        )
        .actions([
            "In the Stone tab, switch Frosted Girdle on.",
        ])
        .check("The girdle band of the picture turns matt, and the switch shows on.")
        .why("Only the girdle changes. The facets of the crown and the pavilion stay polished.")
        .goal(event(STONE_GIRDLE_FROSTED_CHANGED), "Frosted Girdle changed")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Round the edges",
            "A real polished stone has edges that are not perfectly sharp.",
        )
        .actions([
            "In the Stone tab, drag the Edge Rounding slider a little to the right and let go.",
        ])
        .check("The value beside the slider changes from Off to a small number, and the edges between facets catch a faint glint.")
        .why("A small value is enough: the slider runs from sharp edges to a soft rounding, and a large one makes the facets look worn.")
        .goal(event(STONE_EDGE_ROUNDING_CHANGED), "Edge Rounding changed")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Set the stone width",
            "The width changes the picture's color, not its shape.",
        )
        .actions([
            "In the Stone tab, scroll to Size and click the Stone Width (mm) field.",
            "Type a width in millimeters, for example 8, and press Enter. You can type arithmetic too, for example 6.5+0.25.",
        ])
        .check("The line under the field says Saved in the design, with your width.")
        .why("A bigger stone is physically deeper in color: twice the width, twice the optical density. The built-in colors are tuned for about 7 mm. This is the same value as the Girdle Diameter in the Preform tab.")
        .goal(event(STONE_WIDTH_SET), "Stone Width set")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "One step each, saved with the design",
            "Every effect you set is its own step in the history.",
        )
        .actions([
            "Click Undo to take the stone width back, and Undo again for the edge rounding, the frosted girdle and the haze. The Stone tab follows each one.",
            "Save the design: the effects and the width are in the file, and they come back when you open it.",
            "Reset This Tab at the foot of the Stone tab asks you to confirm, then puts every effect back to off as one step. It never changes the material or the shape.",
        ])
        .why("The same effects apply to the Render and to the exports of this design. A design from the library shows its effects for looking only: open it in the Edit tab to keep them.")
        .allow(EFFECTS),
    )
}
