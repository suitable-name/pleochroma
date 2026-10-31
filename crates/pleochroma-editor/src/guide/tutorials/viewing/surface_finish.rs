//! Surface Finish in the editor: how well the facets are polished, how that differs from the
//! haze inside a stone, and which way the polishing scratches point (manual chapter "Surface
//! Finish").
//!
//! Every lesson starts from the Rich Teaching Design. A grade is a design edit, so the steps wait
//! for it with a [`check`] on the design (see `effects_common.rs`). What only changes the view,
//! the Show Surface Finish pill and the Faceting Machine in Preferences, has no trace to wait
//! for, so those steps are reading steps. The Preferences dialog covers the guide panel while it
//! is open, so its step carries every instruction it needs.

#![allow(
    clippy::too_many_lines,
    reason = "a tutorial is its step text; splitting it would only scatter the wording"
)]

use super::{
    ADVANCED, VIEWPORT, check,
    effects_common::{
        EFFECTS, SURFACE, crown_commercial, crown_ideal, crown_prepolish, path_traced,
        pavilion_angle_fixed, pavilion_prepolish,
    },
    event,
};
use crate::guide::{
    Guide, GuideCategory, GuideStep, StartingState, tutorials::viewing_events::STONE_HAZE_CHANGED,
};

/// The lessons of this file, in browser order.
pub fn guides() -> Vec<Guide> {
    vec![polish_quality(), scratch_direction()]
}

fn polish_quality() -> Guide {
    Guide::new(
        "materials-polish-quality",
        "Polish Quality and Haze",
        "Give the crown a real polish, make it rougher, add haze inside the stone and see how the two differ.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Two kinds of softness",
            "A real stone is never a perfect mirror. This lesson tells apart two things that soften a picture: how well the facets are polished, and haze inside the stone.",
        )
        .actions([
            "Polish quality belongs to the facets. Every tier has a Surface Finish grade. Ideal is a perfect mirror. Mirror, Fine, Commercial and Prepolish are real polishes, from a roughness of 1 nm for Mirror up to 25 nm for Prepolish.",
            "Ground (Fine) and Ground (Coarse) are frosted finishes. A ground facet scatters light like frost.",
            "Haze belongs to the whole stone. The Inclusion Haze slider in the Stone tab scatters light inside the stone, like fine inclusions.",
        ])
        .why("A grade is saved with its tier and the haze with the design. A design with no grade is the perfect polish it always was.")
        .allow(VIEWPORT),
    )
    .step(path_traced(
        "A polish scatters a little light as a soft halo, and the Path-Traced view is where light shows it.",
    ))
    .step(
        GuideStep::new(
            "Polish the crown",
            "A grade is chosen for a whole tier at once.",
        )
        .actions([
            "Click the Crown Main row in the tier table.",
            "In the inspector's Tier tab, find Surface Finish and open its list. Choose Commercial.",
            "You can also right-click the row and choose Surface Finish, then Commercial.",
        ])
        .check("The picture gains a soft halo around the crown facets, and the summary under the list reads Commercial.")
        .why("Commercial is the polish of an 8k diamond lap, about 6 nm. If the inspector shows no Surface Finish, switch the interface to Advanced with the Simple | Advanced pill in the header.")
        .goal(
            check("Crown Main polished to Commercial", crown_commercial),
            "Crown Main set to Commercial",
        )
        // The inspector's Tier form is where the grade is chosen: the guide makes room for it.
        .highlight("inspector_tier")
        .allow(SURFACE),
    )
    .step(
        GuideStep::new(
            "Make it rougher",
            "A rougher polish spreads the halo wider.",
        )
        .actions([
            "With Crown Main still selected, open the Surface Finish list again and choose Prepolish.",
        ])
        .check("The halo around the crown facets is wider and the summary reads Prepolish.")
        .why("Prepolish is a 3k lap, about 25 nm. Each grade is one step in the history, so Undo takes it back.")
        .goal(
            check("Crown Main polished to Prepolish", crown_prepolish),
            "Crown Main set to Prepolish",
        )
        .highlight("inspector_tier")
        .allow(SURFACE),
    )
    .step(
        GuideStep::new(
            "Add haze inside",
            "Haze is another kind of softness, and it comes from inside the stone.",
        )
        .actions([
            "Open the Stone tab of the Parameters dock (View > Stone Properties, or Ctrl+Alt+1).",
            "Drag Inclusion Haze a little to the right.",
        ])
        .check("The whole stone turns a little milky, not only the crown. The facets keep their own halo.")
        .why("Haze scatters light on its way through the stone. A finish scatters it only where the light meets the facet.")
        .goal(event(STONE_HAZE_CHANGED), "Inclusion Haze changed")
        .highlight("stone_tab")
        .allow(EFFECTS),
    )
    .step(
        GuideStep::new(
            "Back to a perfect polish",
            "Ideal is the grade of a tier that states no finish.",
        )
        .actions([
            "Click the Crown Main row, then open the Surface Finish list in the inspector and choose Ideal.",
        ])
        .check("The halo around the crown goes away, and the summary under the list is empty.")
        .why("The figures of Checks, the Verdict and Compare ignore Surface Finish, as they ignore Inclusions and Play-of-Color. The pictures show them, the numbers do not.")
        .goal(
            check("Crown Main back to Ideal", crown_ideal),
            "Crown Main set to Ideal",
        )
        .highlight("inspector_tier")
        .allow(SURFACE),
    )
}

fn scratch_direction() -> Guide {
    Guide::new(
        "materials-scratch-direction",
        "Scratch Direction",
        "See which way the polishing scratches point, change them with the Faceting Machine preference and fix one direction for a tier.",
        GuideCategory::Materials,
    )
    .starting(StartingState::Template(5))
    .step(
        GuideStep::new(
            "Scratches have a direction",
            "A polished facet carries fine scratches, and a pre-polish shows them most. They point the way the lap was turning where the stone touched it.",
        )
        .actions([
            "The stone travels across the lap while it is polished, so the scratches on one facet fan out instead of running parallel.",
            "Where they point depends on your machine. With the arm pointing at the lap centre the scratches run parallel to the girdle. With the mast to the side they run at an angle to it.",
            "That is why the direction is a setting of your faceting machine in Preferences, and not a part of the design. The same design shows the scratches of each cutter's machine.",
        ])
        .why("A grade, however, is part of the design. Only the direction comes from the machine.")
        .allow(VIEWPORT),
    )
    .step(path_traced(
        "The scratches show as a halo stretched across them, and the Path-Traced view is where light shows it.",
    ))
    .step(
        GuideStep::new(
            "Polish the pavilion",
            "A pre-polish shows its scratches best.",
        )
        .actions([
            "Click the Pavilion Main row in the tier table.",
            "In the inspector's Tier tab, open the Surface Finish list and choose Prepolish.",
        ])
        .check("The pavilion facets gain a halo that is stretched in one direction, and the summary reads Prepolish.")
        .why("A Mirror polish has almost no scratches to show. A ground finish has none with a direction, because it scatters round.")
        .goal(
            check("Pavilion Main polished to Prepolish", pavilion_prepolish),
            "Pavilion Main set to Prepolish",
        )
        // The inspector's Tier form is where the grade is chosen: the guide makes room for it.
        .highlight("inspector_tier")
        .allow(SURFACE),
    )
    .step(
        GuideStep::new(
            "Show Surface Finish",
            "A view aid draws the grades and the scratch directions over the stone.",
        )
        .actions([
            "Click Surface Finish in the row of pills above the picture. In the inspector, the Show Surface Finish checkbox does the same.",
            "Look at the pavilion facets: each finished facet has a dot tinted by its grade, and a short tick along the direction of its scratches.",
        ])
        .check("The Surface Finish pill is lit, and the pavilion facets show dots with ticks.")
        .why("The overlay is for looking only. It is not saved, and it is hidden in the Diagram view.")
        .highlight("solid_viewport")
        .allow(VIEWPORT),
    )
    .step(
        GuideStep::new(
            "Set the Faceting Machine",
            "While the Preferences dialog is open this panel stays visible at the top right of the window, so you can keep reading it.",
        )
        .actions([
            "Choose Edit, then Preferences... (or press Ctrl+,). Find the Faceting Machine section.",
            "Click Mast to the Side and, under it, set the Mast Position. The small picture of the lap seen from above shows the white ticks turn.",
            "Click Arm Points at the Lap Centre to go back, then choose a Sweep Range: Narrow, Typical or Wide. A wider range fans the scratches out more.",
            "Click Reset to the Plain Machine if you only wanted to look, then click Done.",
        ])
        .check("After Done, the ticks over the stone follow the machine you left set.")
        .why("The machine is saved at once with your other preferences. Designs you open later use it too.")
        .allow(ADVANCED),
    )
    .step(
        GuideStep::new(
            "Fix one direction",
            "For one tier, you can ignore the machine and name the direction yourself.",
        )
        .actions([
            "Click the Pavilion Main row. In the inspector, under Surface Finish, tick Custom Angle.",
            "Drag the Angle slider. The ticks of this tier turn as you let go.",
            "No Sweep, next to it, gives the scratches one direction instead of a fan.",
        ])
        .check("Custom Angle is ticked and an Angle slider appears under it; the ticks of the pavilion all point the same way.")
        .why("Custom Angle is saved with the tier. Untick it and the tier follows the machine again.")
        .goal(
            check("Pavilion Main with a Custom Angle", pavilion_angle_fixed),
            "Custom Angle ticked on Pavilion Main",
        )
        .highlight("inspector_tier")
        .allow(SURFACE),
    )
    .step(
        GuideStep::new(
            "Ground facets have no direction",
            "A ground finish scatters like frost, in every direction alike.",
        )
        .actions([
            "Choose Ground (Fine) for Pavilion Main in the Surface Finish list. The Custom Angle and No Sweep boxes disappear, because there is no direction left to set.",
            "Frosted Girdle in the Stone tab is a ground finish of the girdle. Choose Prepolish again to get the scratches back, or click Undo.",
        ])
        .why("Ground (Fine) is about a 1200 grit finish and Ground (Coarse) about 600. Every control is unlocked again."),
    )
}
