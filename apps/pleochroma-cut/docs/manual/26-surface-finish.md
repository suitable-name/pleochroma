# 26. Surface Finish

Everything in this chapter is part of every build of the program. A design that sets no Surface Finish renders, scores and saves exactly as it did before: the default is **Ideal**, a perfect polish.

## What You Will Do

A real facet is never a perfect mirror. A good polish still scatters a little light into a soft halo around the bright reflections, a poor polish scatters a lot of it and looks hazy, and a ground facet scatters light like frost. This chapter shows how to:

- give a whole tier, or a single facet, a polish grade or a ground finish, and read what the grades mean;
- tell the program how your faceting machine moves the stone over the lap, because that decides which way the scratches of a polish point;
- fix the scratch direction of one tier by hand, or switch the sweep off;
- see the finish on the stone with **Show Surface Finish**;
- wear the edges of the stone with **Abraded Edges**.

Chapter 4 describes the tier form, Chapter 17 the Preferences dialog, Chapter 2 the Stone tab and Chapter 9 the renders.

## The Idea

### Polish Quality

A polished facet has tiny scratches and unevenness. The size of the unevenness is its **RMS roughness**, written **Rq** and measured in nanometres (a nanometre is a millionth of a millimetre). A mirror polish has an Rq near 1 nm, a rough pre-polish tens of nanometres. Light has a wavelength of 380 to 780 nm, so even a rough polish is smooth compared with light, and the sharp reflection of the facet stays sharp. Only a small share of the light is sent into a halo around it. That share is the **Total Integrated Scatter (TIS)**: the fraction of the light a surface scatters instead of reflecting or refracting it cleanly.

Two things about the halo are worth knowing:

- **It is bluish.** The scatter is stronger for short wavelengths (it falls with the square of the wavelength), so a poor polish gives a bluish haze around the highlight. That is real; cutters see it on a pre-polished table.
- **It follows the scratches.** The scratches are grooves in one direction, so the halo is stretched across them, perpendicular to the scratches. This is the streaked highlight cutters know from a facet that was polished in one direction.

### The Grades

A grade is a named finish. The tier table's right-click menu offers them in a **Surface Finish** submenu, and the Tier tab's inspector offers them in a combo.

| Grade | Typical lap | Roughness | What it shows |
|---|---|---|---|
| **Ideal** | none | 0 | A perfect polish: no halo. The default, and exactly what older builds rendered. |
| **Mirror** | 50k to 100k diamond or alumina | Rq 1 nm | About 0.05 % of the light in the halo at normal incidence; a faint halo, round. |
| **Fine** | 14k diamond | Rq 2.5 nm | About 0.3 %; a faint halo, slightly stretched. |
| **Commercial** | 8k diamond | Rq 6 nm | About 2 %; a visible halo, clearly stretched. |
| **Prepolish** | 3k diamond | Rq 25 nm | About 28 %; a strong bluish halo around highlights, the look of an unfinished facet. |
| **Ground (Fine)** | 1200 grit | roughness 0.30 | A frosted facet with a fine grain. |
| **Ground (Coarse)** | 600 grit | roughness 0.50 | A frosted facet with a coarse grain. |
| **Custom** | any | your numbers | **Roughness (nm)** from 0 to 100 and **Anisotropy** from 1 to 15. |

The percentages are for light arriving straight on, at a wavelength of 550 nm, from air. Inside the stone the figure is larger, because the formula grows with the refractive index of the medium the light comes from, and it falls toward grazing incidence. The lap and roughness columns are typical values, chosen to look right: they are starting points that you can retune with **Custom**, not measurements of a particular lap.

**Anisotropy** says how much rougher the surface is across the scratches than along them. 1 is round, with no preferred direction. The presets carry 1 (Mirror), 2 (Fine), 3 (Commercial) and 4 (Prepolish) at full strength. Cutters sweep the stone over the lap, so the program scales those values by a **Scratch Strength** of 0.3: Fine is stretched by 1.3, Commercial by 1.6 and Prepolish by 1.9. A **Custom** finish uses the anisotropy you type, unscaled.

### Frosting Is Ground

The old **Frosted** tick box of the Tier tab (Chapter 4, "Frosting a Tier") and the snowflake button of a facet chip still work, and a frosted tier is now a **Ground** tier of roughness 0.45. That roughness is a glass-like rough surface, not a flat grey: a ground girdle shows the soft forward glow of the light that passes through it. **Ground (Fine)** and **Ground (Coarse)** are the same finish with a finer and a coarser grain. The program restores the energy a rough surface would otherwise lose, so a ground facet does not render darker than a polished one for that reason alone.

A design saved with a frosted tier opens as before, and its pictures look different once, on purpose: the matte, flat Lambertian look of the older frosting is replaced by the ground finish described here. Frosted renders and tilt curves are therefore a little different from those of earlier builds.

## Setting a Finish

### On a Tier

Right-click a tier row in the tier table, choose **Surface Finish** and pick a grade. **Custom...** opens the tier in the inspector with its Custom values. Below the grades the same menu has **No Sweep**, **Follow the Sweep** and **Show Surface Finish** (or **Hide Surface Finish**). The menu is offered for a flat tier only, a tier built from a concave tool has no finish of its own (Chapter 16), and it is not offered while a tutorial locks the tier table.

In the inspector, the Tier tab (Advanced interface) has a **Surface Finish** section:

- the grade combo and, under it, a one-line summary of what the grade means;
- **Roughness (nm)** and **Anisotropy** sliders when the grade is **Custom**;
- **Custom Angle** and **No Sweep** for a polished or Custom finish (a ground finish is round and has no direction);
- **Finish of One Facet**, described next;
- the **Show Surface Finish** tick box.

The sliders apply when you let go of them. Each change is one undoable edit, and choosing the grade the tier already has changes nothing and records no step.

### On One Facet

**Finish of One Facet** picks a facet of the loaded tier by its index position and gives it a finish of its own: **Follow Tier** (no override) or one of the grades. A facet with an override keeps it when the gear changes, the same way a frosted facet does.

### The Direction of the Scratches

The scratches are the grooves of the last strokes on the lap. Which way they point on a facet depends on your machine, so the program does not fix one default for everyone. Three things set it, in this order of precedence:

1. **Custom Angle** on the tier. The scratches run at the angle you type, in degrees from the line parallel to the girdle, measured around the facet's normal. The machine is ignored.
2. The **Faceting Machine** preference, described next, which gives each facet its scratch direction from its own tilt and index.
3. **No Sweep** on the tier, which does not choose a direction but removes the fan (see below): the scratches take the one nominal direction of the middle of the sweep.

## The Faceting Machine

Open **Edit → Preferences...** and find the **Faceting Machine** section (Chapter 17). It is a setting of you and your machine. It is remembered for you and is never saved in a design: the same design renders with each cutter's own scratch directions, and a design you send to someone else does not carry your machine.

### Why the Direction Depends on the Machine

While you polish, the lap turns under the stone. At the spot where the stone touches the lap, the lap moves along the tangent of the circle through that spot. The scratches therefore run along that tangent, and the part of it that lies in the facet gives the scratch direction on the facet. The turning direction of the lap does not matter: a scratch is a line, not an arrow.

Two things change that tangent from machine to machine:

- how the arm that carries the stone is lined up with the lap;
- where on the lap the stone touches, because the tangent turns as the contact point moves in and out.

### A Machine With a Centred Arm

Picture the lap seen from above, a circle with the stone's dop arm lying along a radius, pointing at the lap centre. At the contact point the lap moves at right angles to that radius, which is at right angles to the arm. Whatever radius the stone is at, the angle between the arm and the tangent stays the same. The scratches run parallel to the girdle of the facet, at every radius. The fan of directions has no width. This is the plain machine, and it is the default (**Arm Points at the Lap Centre**, offset 0 degrees).

### A Mast Machine

Picture the same lap with the mast standing to one side, as on a mast machine on a side slide with a quadrant and an index gear. The dop arm still reaches out over the lap, but the line of the arm no longer passes through the lap centre. It runs at some distance from the centre, the **Mast Position**. As the stone moves in and out along that line, the radial line through the contact point turns relative to the arm, and with it the tangent, so the scratches fan out. At the radius `r` the scratches are turned from the girdle-parallel direction by

> Arm Offset + arcsin(Mast Position / r)

degrees. An example: with the mast 30 mm to the side and a stroke between 40 and 80 mm of the lap radius, the scratches fan over about 22 to 49 degrees. With the stroke narrowed to 55 to 65 mm the fan shrinks to about 27 to 33 degrees. The farther the mast stands to the side, the more the fan widens.

### The Settings

- **Arm Points at the Lap Centre** and **Mast to the Side** choose between the two machines above. **Mast to the Side** shows the **Mast Position** slider (from minus 150 to plus 150 mm; a negative value puts the mast on the other side of the centre).
- **Arm Offset** (minus 45 to plus 45 degrees) is the angle between the dop arm and the radial line through the contact point, added to everything else. 0 gives scratches parallel to the girdle.
- **Lap Radius** (50 to 200 mm).
- **Sweep Range**: **Narrow** (55 to 65 percent of the lap radius), **Typical** (40 to 80 percent), **Wide** (20 to 95 percent) or **Custom**, which shows the **Inner Radius** and **Outer Radius** sliders in millimetres.
- **Reset to the Plain Machine** puts the arm back at the lap centre with no offset, a 100 mm lap and the Typical sweep.

A small figure shows the lap from above: the gray circle is the lap, the blue band is where the stone travels, the amber line is the dop arm and the white ticks are the scratches at three radii. A sentence under it states the range of scratch angles of your settings, for example "The scratches fan out from 22 to 49 degrees from the line parallel to the girdle." (A plain machine reads "The scratches run 0 degrees from the line parallel to the girdle.")

### How to Measure Your Machine

Put a straightedge along the dop arm and look down on the lap. The **Arm Offset** is the angle between the arm and the line from the lap centre to the spot where the stone touches the lap. For a plain machine the two lines coincide and the offset is 0. For a mast machine measure the **Mast Position** as the shortest distance from the lap centre to the line along the arm, and the **Lap Radius** from the centre to the rim.

### The Sweep Is the Last Pass

The final strokes decide the scratches, because each pass erases those before it. So the **Sweep Range** is the range of your last polishing pass: how far in and out on the lap you moved the stone then. If you finish with short strokes near the middle, choose **Narrow**.

### How the Fan Looks in the Render

The renderer does not take one angle. For every light path it draws a contact radius uniformly over the sweep and turns it into the scratch angle with the formula above, so the picture shows the true mix of directions. The visible effect is a softer, fan-shaped halo instead of one sharp streak. **No Sweep** on a tier collapses the fan to the one angle of the middle of the sweep, as for a cutter who holds the stone still. **Custom Angle** fixes the angle outright.

## Show Surface Finish

**Show Surface Finish** is a view aid for the Solid viewport (not in the Diagram view or while a cut is only partly shown). It tints every facet by its grade and draws short ticks along the direction of its scratches where the polish is directional. Switch it on from the **Surface Finish** pill in the viewport's toolbar, the tier menu or the inspector tick box. It is for you only: it is not saved and does not change a render.

## Abraded Edges

Wear takes the edges of a stone first. In the Stone tab, the **Abraded Edges** group has two sliders:

- **Width**: how wide the worn band along every facet edge is (up to 0.1 in model units, where the stone is about 1 across). 0 is off: sharp, clean edges.
- **Wear**: how rough the band is. A low value is a lightly dulled edge, a high one a rough, frosted edge. Setting a width from 0 starts the wear at 0.3.

The band is a ground finish along every edge, found in the same way as **Edge Rounding**, and it combines with it. Each change is one undoable step. The worn edges show in Path-Traced pictures only.

## Where the Finish Shows and Where It Does Not

- **Path-Traced, Solid + Path-Traced, exports, render jobs, remote workers, the web viewer and the command-line tool** draw the finish. A scene with a finish goes to the graphics card when there is one, as any other scene does.
- **Ray-Traced** ignores Surface Finish and Abraded Edges, because that view has no scatter model. The Stone tab says so next to the group.
- **The figures ignore it.** The Checks numbers, the Verdict, Compare, Optimize, the sweeps, Tune and the tilt curves still treat every facet as perfectly polished. When a finish is on, the Checks tooltip, the Verdict and the Compare header add the line "Figures ignore Surface Finish" (or "Inclusions", "Play-of-Color" and so on, naming only what is on). A figure for polish haze may follow in a later version.
- The catalogue thumbnails and the tilt-curve staleness check include the finish, so changing it redraws the thumbnail of the design.

## Files

A tier's finish is stored with the tier, in these optional keys of a `[[tiers]]` entry: `surface` (the grade name), `surface_rq_nm`, `surface_lap_deg` and `surface_aniso` (only for **Custom**), `surface_no_sweep`, `ground_alpha` (a ground tier whose roughness is not the default), and `surface_facets` with the per-facet overrides by index-wheel position. The Abraded Edges settings are `edge_abrasion_width`, `edge_abrasion_alpha` and, for a file that sets one, `edge_abrasion_halo_rq_nm` (a roughness that dulls the polish next to the band; the Stone tab has no control for it) in `[render_effects]`. Each is written only when used, and none changes the file's version, so a design without a finish is byte for byte what an older build wrote and an older build opens such a design and ignores the keys (Chapter 11). Your **Faceting Machine** is in the settings file (the `faceting_machine` key), not in the design.

The `.gcs` and `.asc` formats have no notation for a finish, so nothing is exported to them and nothing is read from them.

## Limitations

- The scratch direction comes from a model of the machine, not from a measurement of your lap. Check the figure against a real facet and tune the **Arm Offset** and the sweep.
- The grades are typical values for the roughness. The correlation length of a real lap is not published, so the width of the halo is a free choice: use **Custom** to match a stone you know.
- Only the last polishing pass is modeled. Earlier, coarser scratches are not drawn.
- **Scratch Strength** is fixed at 0.3 for the grades; only **Custom** sets the stretch directly.
- A concave tool tier has no finish, and a finish on a facet next to a tool is treated like any other facet.
- Chips (small flaws knocked out of an edge) are not modeled, only the worn band.
- A ground facet scatters the light once per bounce with the energy put back; the extra direction a real rough surface gives to repeated scattering is not modeled.

## Troubleshooting

| Symptom | Cause | What to do |
|---|---|---|
| The tier menu has no **Surface Finish** | The tier is a concave tool tier, or a tutorial has the table locked. | Use a flat tier; finish the tutorial step. |
| The **Surface Finish** section is missing from the inspector | The interface is **Simple**, or no saved tier is loaded. | Switch to Advanced (Chapter 17) and load a tier. |
| **Custom Angle** and **No Sweep** are missing | The grade is **Ideal** or a **Ground** one. | Pick a polish grade or **Custom**; a ground finish is round. |
| The renders show no halo | The grade is **Ideal**, or the view is Ray-Traced. | Pick a grade; switch to Path-Traced. |
| Every grade looks the same | The roughness is small compared with the light's wavelength. | Compare **Prepolish** with **Mirror**, or try **Custom** at 50 nm. |
| The scratches point the wrong way | The Faceting Machine preference does not describe your machine. | Measure the arm offset and mast position (above), or give the tier a **Custom Angle**. |
| The halo is one sharp streak, not a fan | **Arm Points at the Lap Centre**, or **No Sweep** on the tier, or a **Narrow** sweep. | Choose **Mast to the Side** or a wider sweep if that matches your machine. |
| My stones look different from a colleague's | The machine setting is yours, not the design's. | This is intended; compare machine settings. |

## Next Steps

Chapter 4 covers the tier form and the older **Frosted** tick box, Chapter 17 the Preferences dialog, Chapter 25 the Crystal Axis, Chapter 27 the inclusions that scatter light inside the stone, and Chapter 22 the tutorials: Polish Quality and Haze, and Scratch Direction.
