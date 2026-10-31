# 28. Play-of-Color and Schiller

Everything in this chapter is part of every build of the program. A design that does not switch the effect on renders, scores and saves exactly as it did before.

## What You Will Do

A precious opal flashes patches of spectral color that change as you move the stone. A labradorite flashes one electric blue, green or gold across a whole face, and only from the right angle. A moonstone glows with a sheen that floats on the stone. None of these colors comes from absorption, and none comes from a dye. They come from the fine structure inside the stone. This chapter shows how to:

- read where the colors come from, and why the flash moves;
- switch **Play-of-Color** (opal) or **Schiller** (labradorite) on in the Stone tab and shape it;
- change the arrangement of the patches with **Reseed**;
- understand the moonstone's adularescence, which belongs to the inclusion presets;
- give a custom material its own structure.

Chapter 27 covers inclusions with a direction, Chapter 25 the Crystal Axis, and Chapter 29 the Material Editor.

## The Idea

### Color From Structure

A **diffraction** structure reflects only the wavelengths that fit its spacing. Where the spacing repeats evenly, the reflections from every layer add up for one wavelength and cancel for the rest. The wavelength that is reflected is given by the **Bragg condition**: roughly twice the spacing, times the refractive index, times the cosine of the angle between the light and the layers' normal.

Two consequences hold for both stones:

- **The color depends on the angle.** The wavelength gets shorter as the light meets the layers more obliquely, so a patch that is red straight on turns orange, then yellow, then green as you tilt the stone. That is why the colors move.
- **The color is narrow.** Only a thin band of wavelengths is reflected, so the colors are pure, much purer than a dye.

### Precious Opal

Precious opal is a close-packed array of tiny silica spheres, 150 to 300 nm across, stacked in orderly patches and surrounded by water-bearing silica. The spacing of the planes of spheres sets the color straight on: spheres of about 200 nm reflect blue (about 470 nm), spheres of about 270 nm reflect red (about 630 nm). Larger spheres therefore give the reds, which is why red is the rarest color of opal. Each orderly patch has its own orientation, so the stone is a mosaic of patches that flash at different angles. A second, weaker reflection at half the wavelength is included when it falls inside the visible range.

### Labradorite

Labradorite has stacked lamellae that formed as the crystal cooled, with a period of 150 to 250 nm. A period of about 190 nm reflects yellow (about 600 nm); 150 nm gives blue and 250 nm a deep red. The lamellae all run the same way in the crystal, so there is one direction in which the whole face flashes at once. That direction is fixed in the crystal, and the flash follows the **Crystal Axis**: turn the stone's axis and the flash moves with it. The program takes the lamella planes as tilted a little from one of the crystal's cleavage faces, which is an approximation: the real orientation changes with the composition of the stone.

### Moonstone

A moonstone's adularescence is a soft, floating sheen. How it forms is still debated and the numbers are not settled, so the program models it as a family of broad plates (the **Adularescence** preset of Chapter 27), not as a diffraction structure. You find it in the **Inclusions** group of the Stone tab. Moonstone has no Play-of-Color group.

## The Materials

The built-in materials for this chapter are:

| Material | Optical character | What it shows |
|---|---|---|
| **Precious Opal (White)** | Isotropic, index 1.45 | A pale body and a milky veil, with the play-of-color of its patches. |
| **Precious Opal (Black)** | Isotropic, index 1.45 | A dark neutral body, which makes the colors stand out as on a black opal. |
| **Labradorite** | Biaxial (+), indices about 1.559, 1.5625 and 1.567 | The schiller, fixed in the crystal. |
| **Moonstone** | Biaxial (−), indices about 1.518, 1.523 and 1.526 | Adularescence from the Inclusions group. |

The older **Opal** is unchanged: a plain colorless body opal with no play-of-color. Choose it when you want the plain stone.

The dispersion of Labradorite is a placeholder of the right size taken from moonstone, since no measured value was found. The indices are values from the published ranges.

## In the Stone Tab

For a material that has a structure, the Stone tab shows a **Play-of-Color** group (opal) or a **Schiller** group (labradorite) under the Inclusions group. Switch on **Show Play-of-Color** (or **Show Schiller**). The first time you do, the settings start from the material's own values. Each choice below is saved in the design as one undoable step.

- **Pattern**: how big the patches are. **Pinfire** is many tiny points of color, **Harlequin** medium patches, **Broad Flash** a few large flashes. An opal starts as Harlequin, a labradorite as Broad Flash.
- **Set Patch Size** and **Size**: set the width of one patch yourself (in model units, where the stone is about 1 across) instead of the pattern's own.
- **Color Range, Low End** and **High End**: the shortest and the longest wavelength the patches reflect when seen straight on, in nanometres. Around 450 is blue, 550 green, 650 red. An opal starts at 440 to 680, a labradorite at 470 to 740. The low end is never above the high end.
- **Purity**: how pure each flash is. A small number is the purer, narrower color; a large one a broad, washed color. The slider runs from 0.01 to 0.3.
- **Coverage**: the share of the stone that flashes, from 0 to 100 percent. An opal starts at 80 percent, a labradorite at 90.
- **Reseed**: draws a new arrangement of the patches with the same settings. The **Seed** number beside it is saved with the design, so a design always shows the same patches. One step you can undo.

With the view on **Ray-Traced** the group adds "Ray-Traced ignores Play-of-Color. Switch to Path-Traced to see it." A custom material with its own structure is listed under the group's header as part of the material, and edited in the Material Editor.

### What to Expect

- Tilt the stone with the tilt tools or a tilt video (Chapter 14) and the colors of an opal shift toward blue; the patches of a labradorite flash and go dark.
- A labradorite shows its flash only within a cone around the lamella direction. Turn the Crystal Axis and the cone moves.
- The play-of-color needs light to enter the stone. A black body makes the colors stand out, a white body adds a milky veil.

## A Custom Material With a Structure

In the Material Editor (Chapter 29) the **Play-of-Color** section lets a custom material behave like an opal or a labradorite:

- **Imitates**: **Opal** (the lattice patches point every way) or **Labradorite** (one direction fixed in the crystal, following the Crystal Axis).
- **Pattern**, **Custom Size** and **Size**.
- **Shortest Color** and **Longest Color**, in nanometres. The longest must not be shorter than the shortest.
- **Purity** and **Coverage**, in percent.
- **Reseed** and **Reset To Defaults**, which loads the usual values of the kind the material imitates.

A structure that names no kind is saved but not rendered; the section says so and shows "Stored, Not Rendered". A custom material's structure is saved with the material and travels in the `[material.custom]` table of a design that uses it. The design keeps its own choice, so a design can still change the pattern and the seed.

## Where It Shows and Where It Does Not

- **Path-Traced, Solid + Path-Traced, exports, render jobs, remote workers, the web viewer and the command-line tool** draw it. The graphics card draws it as well.
- **Ray-Traced** ignores it.
- **The figures ignore it.** Brilliance, Fire, Scintillation, the Verdict, Compare, Optimize and the tilt curves treat the stone as if it had no play-of-color. When it is on, the Checks tooltip, the Verdict and the Compare header add "Figures ignore Play-of-Color." (or "Schiller", together with anything else that is on).
- Following the light through many small patches may take longer than a plain stone to converge; the cost has not been measured yet. The graphics card and a remote worker are there for final renders.

## Files

A design's choice is saved in an optional `[render_effects.structure]` table: `pattern`, `seed`, `size` (only when set), `color_low_nm`, `color_high_nm`, `purity` and `coverage`. A custom material's structure is stored with the material as text, and as the `structure` key in `[material.custom]`; the kind it imitates travels in it. All of it is written only when used, so a design without it is byte for byte what an older build wrote (Chapter 11).

## Limitations

- The patch sizes, the purity and the coverage are looks you choose. The real size of the patches of an opal is not published, so they are not claimed to be physical values.
- Thin-film colors (the rainbow of a crack in a stone) and the glitter of sunstone are not modeled yet.
- The lamella orientation of labradorite is approximate.
- Black Opal is a dark body, not a separate zone: it does not get darker toward the pavilion.
- Play-of-color and schiller work on a stone with color zones (Chapter 24): the zones color the body, the structure adds the flashes. The two were checked against each other on the CPU and the graphics card; the combination has not been compared with a real stone.

## Troubleshooting

| Symptom | Cause | What to do |
|---|---|---|
| The Stone tab has no **Play-of-Color** or **Schiller** group | The material has no structure. | Pick **Precious Opal (White)**, **Precious Opal (Black)** or **Labradorite**, or a custom material that imitates one. |
| The group is there but the stone looks plain | **Show Play-of-Color** is off, or the view is Ray-Traced. | Switch it on; use Path-Traced. |
| The colors are washed out | **Purity** is high, or **Coverage** is low. | Lower the Purity number, raise the Coverage. |
| The patches are too small or too large | The pattern does not suit the stone. | Try another **Pattern**, or **Set Patch Size**. |
| A labradorite is dark from the front | The face-up direction is outside the flash cone. | Turn the Crystal Axis (Chapter 25), or tilt the stone. |
| A different arrangement every time I reopen the design | It is not; the seed is saved. | Check you have not pressed **Reseed**. |
| The picture is slow to settle | Following light through many patches may take longer. | Use the graphics card or a remote worker (Chapter 10). |

## Next Steps

Chapter 27 covers the inclusions that scatter light, Chapter 25 the Crystal Axis, Chapter 26 the surface finish, Appendix C the built-in materials, and Chapter 22 the tutorials: Opal Play-of-Color and Labradorite Flash.
