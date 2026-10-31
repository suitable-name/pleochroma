# 27. Inclusions and Silk

Everything in this chapter is part of every build of the program. A design that picks no inclusion preset renders, scores and saves exactly as it did before. The older **Inclusion Haze** slider of the Stone tab (Chapter 2) stays as it is: an even milky veil. This chapter is about inclusions that have a direction.

## What You Will Do

Many gems hold fine needles or plates that all point the same way. Rutile needles in sapphire, tubes in tourmaline, fibres in a cat's-eye, plates in moonstone: they scatter light differently depending on which way the light travels relative to them. That is what gives a sapphire its velvety body, a moonstone its sheen and a cabochon its star or eye. This chapter shows how to:

- read what a needle or a plate does to light, and why a faceted stone shows a sheen where a cabochon shows a star;
- pick an inclusion preset for a stone in the Stone tab, and tune its amount, color, spread and turn;
- see which way the inclusions run;
- build your own inclusion families for a custom material.

Chapter 25 explains the Crystal Axis, which the inclusions follow. Chapter 28 covers the other effect that comes from the inside of a stone, the flashes of color of opal and labradorite.

## The Idea

### Needles and Plates

Two kinds of inclusion are modeled:

- **Needles** are long, thin and parallel. A needle scatters light that crosses it and ignores light that travels along it, so a stone full of parallel needles is clear when you look along them and hazy when you look across them. The scattered light does not leave in all directions: it leaves on a **cone** around the needle's axis. That cone is the physical source of the star in a star sapphire and the eye in a cat's-eye.
- **Plates** are thin flat flakes, all facing the same way. Each plate acts as a tiny mirror, so the stone flashes when a plate faces the light and the eye at once. Moonstone's adularescence is of this kind.

A third kind, **Random Needles**, points in every direction. It gives a rutilated quartz its look: the same scattering strength whichever way you look.

### Why Blue or Grey

Fine inclusions scatter short wavelengths more than long ones, so very fine silk looks bluish-white. Coarse inclusions scatter every wavelength alike and look grey or white. The **Silk Color** setting chooses between them (see below).

### Faceted Stones and Cabochons, Honestly

This matters for what you will see.

- **A faceted stone** shows silk as a soft sheen that turns with the stone and follows the Crystal Axis, a velvety, lower-contrast body, and bright streaks where a cone of scattered light reaches the eye through a facet. A faceted star sapphire shows fragments of the star, a broken star that moves from facet to facet as you turn the stone. That is what real faceted silky corundum shows, and it is what the program draws.
- **A true star or eye needs a curved dome.** The star is the place on a dome where the scattering cones meet the mirror condition for the eye. The program builds stones out of flat facets and subtractive tools, and has no convex dome shape yet. A cabochon made of many small facets approximates one and shows a segmented star. A true dome, for a smooth star and eye, is planned for a later version.
- **Cubic materials** (star garnet, star spinel) have no crystal frame in which to say where the needles run, so they offer no inclusion presets for now. The presets are for materials with a crystal axis, plus Rutilated Quartz.

## The Presets

Each built-in material that has presets offers them in the Stone tab. The directions are in the crystal; the amounts are aesthetic starting points, since no table of typical amounts exists.

| Material | Presets | What they are |
|---|---|---|
| Sapphire, Ruby | **Fine Silk** | Three needle sets in the basal plane, 60 degrees apart, fine and bluish. A velvety body. |
| | **Rutile Silk** | The same directions, coarser and stronger. |
| | **Star Silk** | The same directions with a narrow cone: sharp streaks, the broken star of a faceted star sapphire. |
| Chrysoberyl (Yellow), Alexandrite | **Cat's-Eye Silk** | One needle set along the crystal's c direction. Needs the **Second Axis** (see below). |
| Tourmaline | **Growth Tubes** | One coarse needle set along the Crystal Axis. |
| Emerald, Aquamarine, Morganite | **Rain** | One needle set along the Crystal Axis: the growth tubes called rain. |
| Quartz | **Rutilated** | Random needles. |
| Moonstone | **Adularescence** | One broad set of plates. |
| Every other material | none | Only the even Inclusion Haze. |

Rutile needles in corundum lie in the basal plane, parallel to the three a directions that are 60 degrees apart, which is where the six-rayed star comes from. A four-rayed star means only two of the three sets formed. The chrysoberyl needles lie usually along the crystallographic c direction and only occasionally along a. In the optical frame of a biaxial crystal the crystallographic c direction of chrysoberyl is the direction of the smallest index (the alpha direction), not the Crystal Axis of the program, which is the direction of the largest index. That is why the Cat's-Eye preset follows the Second Axis and not the Crystal Axis alone.

The strengths are anchored on the plain haze values the program already uses for the same materials, so a preset at the usual density is a comparable amount of scattering.

## In the Stone Tab

Open the **Stone** tab of the Parameters dock (Chapter 2). The **Inclusions** group is under **Edge Rounding**. It shows the presets of the stone's material as buttons, **None** first. **None** is the plain stone. The settings below show once a preset is on, and each change is one undoable edit that is saved in the design:

- **Density**: how many inclusions the stone holds, from 0 to 1. About 0.35 is the usual amount.
- **Set Silk Turn** and **Silk Turn**: see below.
- **Sheen**: how widely the light spreads off the inclusions. **Preset's Own** keeps the preset's width, **Soft** makes the cone wider (a diffuse glow), **Sharp** narrower (sharp streaks).
- **Silk Color**: **Preset's Own**, **Grey** (white scattered light), **Slightly Blue** or **Blue** (the bluish look of fine silk). **Fine Silk** is blue by default.
- **Show Silk Directions**: see below.

A preset that needs a Second Axis you have not set is dimmed, and its tooltip says "Not available yet: this preset needs a Second Axis. Set the Crystal Axis for this stone and set its Second Axis first." An amber note under the group explains why when a preset is not available. With the view on **Ray-Traced**, the group adds "Ray-Traced ignores Inclusions. Switch to Path-Traced to see them."

A custom material has no presets. If it has inclusion families of its own (next section) they are listed under the group's header as part of the material, and edited in the Material Editor.

### Silk Turn

Needles that lie in the basal plane of a corundum have an in-plane direction too: which of the three a directions is the first. The **Silk Turn** is a turn, from 0 to 180 degrees, of the whole set of inclusions about the Crystal Axis. For a uniaxial material (Sapphire, Ruby, Tourmaline, the beryls, Quartz) it is the same stored turn as the Second Axis of Chapter 25, used only for the inclusions: the optics of a uniaxial stone ignore it, so setting it does not change doubling or color. For a two-axis material (Chrysoberyl, Alexandrite) the inclusions follow the real Second Axis.

**Set Silk Turn** is off by default and then the inclusions follow the stone's Second Axis, if it has one. Switch it on to give the turn by hand with the slider. With the inclusions off, a Second Axis on a uniaxial stone changes nothing at all, as before.

### Cat's-Eye Needs a Second Axis

The needles of **Cat's-Eye Silk** run along the chrysoberyl's alpha direction, and that direction is only fixed once the Second Axis of the stone is. Without it the program would have to guess one of the two directions perpendicular to the Crystal Axis, and the eye would point somewhere arbitrary. So the preset is offered only when the stone's **Crystal Axis** and **Second Axis** are both set (Chapter 25), by a design setting, a color-named preset or a material default. Set them, then pick the preset.

### Show Silk Directions

**Show Silk Directions** draws amber lines over the stone along the needles (or the normals of the plates), so you can see which way they run. It uses the same overlay as the Crystal Axis and switches that overlay on for the session. It is a view aid: it is not saved.

## Inclusion Families for a Custom Material

A custom material can carry up to four inclusion **families** of its own. Open the Material Editor (Chapter 29); the dialog has an **Inclusion Families** section. **Add Family** adds one; **Remove** takes one away. Each family has:

- **Kind**: **Needles**, **Plates** or **Random Needles**.
- **Needle Axis** (or **Plate Normal**): **Along the Crystal Axis**, **In the Basal Plane**, **Along the Alpha Axis** or **Tilted**. The basal plane and the tilted direction show a **Turn** in degrees, and the tilted one also a **Tilt**, the angle from the crystal axis. Random Needles have no direction.
- **Density**: how strongly the family scatters. The design's Density slider scales it further.
- **Color**: **Grey**, **Slightly Blue**, **Blue** or **Strong Blue**.
- **Cone Width** (**Gloss Width** for plates): a small value gives a sharp streak, a large one a soft glow.
- **Forwardness** for needles: how far the scattered light is thrown forward along the cone.

Under the list a note says what the families do for the material's character, and warns when a direction is not available for it (the alpha direction needs a biaxial material). The families are saved with the material, in your catalogue and in the `[material.custom]` table of any design that uses it. They are expressed in the material's own crystal frame, so they follow the stone's Crystal Axis when a design sets one.

## Where the Inclusions Show and Where They Do Not

- **Path-Traced, Solid + Path-Traced, exports, render jobs, remote workers, the web viewer and the command-line tool** draw the inclusions. The graphics card draws them too.
- **Ray-Traced** ignores them.
- **The figures ignore them.** Brilliance, Fire, Scintillation, the Verdict, Compare, Optimize and the tilt curves are computed as if the stone were clear. When an inclusion preset is on, the Checks tooltip, the Verdict and the Compare header add "Figures ignore Inclusions." (naming together everything that is on, for example "Figures ignore Inclusions and Surface Finish."). A figure for how much light the silk takes may follow in a later version.
- **Zoned stones** (Chapter 24) show inclusions too.

## Files

A design's inclusion choice is saved in an optional `[render_effects.inclusions]` table: `preset`, `density`, `turn_deg` (only when a Silk Turn is set), `sheen` and `color` (only when not the preset's own). A custom material's families are stored with the material and, as the `inclusions` text key, in `[material.custom]`. All of it is written only when used, so a design without it is byte for byte what an older build wrote (Chapter 11).

## Limitations

- The presets cover materials with a crystal axis and Rutilated Quartz. Cubic materials have none for now.
- A faceted stone shows sheen and the fragments of a star. A true star or eye needs a dome shape that is not yet in the program.
- The amounts are chosen to look right, not measured; adjust **Density** to a stone you know.
- **Cat's-Eye Silk** needs the Second Axis.
- The even **Inclusion Haze** is a separate setting and adds to the inclusions; with a preset on, the haze takes the preset's own forward-scattering strength instead of the fixed one.
- Optimize, the sweeps and the tilt curves do not count the silk.

## Troubleshooting

| Symptom | Cause | What to do |
|---|---|---|
| The **Inclusions** group says "This material has no inclusion presets." | The material is isotropic or has no entry. | Pick another material, or add families to a custom material. |
| **Cat's-Eye Silk** is dimmed | The stone's Crystal Axis or Second Axis is not set. | Set both in the **Crystal Axis** section (Chapter 25). |
| The preset shows no change | The view is Ray-Traced, or Density is near 0. | Switch to Path-Traced; raise the Density. |
| The silk looks grey, not blue | **Silk Color** is Grey, or the preset is coarse. | Pick **Blue**. |
| A sapphire shows no six rays | A faceted stone shows broken fragments, not a smooth star. | See "Faceted Stones and Cabochons, Honestly". |

## Next Steps

Chapter 25 covers the Crystal Axis and the Second Axis, Chapter 26 the surface finish, Chapter 28 play-of-color and schiller, Chapter 29 the Material Editor, and Chapter 22 the tutorials: Silk in a Sapphire.
