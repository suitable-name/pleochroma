# 6. Adapting a Design for Another Material

## What You Will Do

This chapter is about a question every cutter eventually asks: "I like this
cut, but I want to cut it in a different gem material -- what do I need to
change?" It covers the built-in material list, building your own custom
material, the Stone tab's Material and RI Override controls,
how a material actually reaches the optimizer/tilt curve/viewport/export,
and **Retarget**, the command bar's automated proposal for re-angling a
design for a different material.

## What Refractive Index Is, Briefly

**Refractive index (RI)** measures how strongly a material bends light. It
determines a stone's **critical angle** -- the angle, measured from a
facet's normal, beyond which light hitting that facet from inside the stone
reflects internally instead of escaping:

> critical angle = arcsin(1 / RI)

A cutting design's pavilion angles are chosen so that light entering
through the crown strikes the pavilion facets steeper than the critical
angle, reflects internally (rather than leaking out the bottom, which shows
as **windowing** -- a washed-out, see-through patch) and exits back out
through the crown towards the viewer. **Birefringence** is a related but
separate property: some materials split light into two rays with slightly
different refractive indices, part of what gives certain gems (peridot,
zircon, and others) their characteristic doubling.

A cutting design that performs well in one material's RI will not
necessarily perform well in another -- a design cut for diamond (RI about
2.417) at a given pavilion angle may window badly if cut in quartz (RI
about 1.54) at the same angle, because quartz's critical angle is larger.

## The Built-In Material List

The renderer ships 38 built-in materials (including the zoned Ametrine), used both by the Live Render
viewport and as starting templates in the Material Editor. Full dispersion
and color detail for every one is in Appendix C; the figures that matter
for cutting -- refractive index, birefringence, and whether the material
has real absorption bands modelled (which mostly affects render color, not
cutting) -- are:

| Material | n_D | Birefringence (Δn) | Optical character | Absorption modelled |
|---|---|---|---|---|
| Diamond | 2.417 | — | Isotropic | No |
| Sapphire | 1.768 | −0.0081 | Uniaxial (−) | Yes |
| Ruby | 1.768 | −0.0081 | Uniaxial (−) | Yes |
| Emerald | 1.579 | −0.0060 | Uniaxial (−) | Yes |
| Zircon | 1.925 | +0.0590 | Uniaxial (+) | Yes |
| Alexandrite | 1.743 | +0.0076 | Biaxial (+) | Yes |
| Topaz | 1.627 | +0.0080 | Biaxial (+) | Yes |
| Spinel | 1.716 | — | Isotropic | Yes |
| Quartz | 1.544 | +0.0091 | Uniaxial (+) | No |
| Tourmaline | 1.639 | −0.0210 | Uniaxial (−) | Yes |
| Tanzanite | 1.701 | +0.0130 | Biaxial (+) | Yes |
| Synthetic Moissanite | 2.647 | +0.0415 | Uniaxial (+) | No |
| Cubic Zirconia | 2.158 | — | Isotropic | No |
| Aquamarine | 1.577 | −0.0060 | Uniaxial (−) | Yes |
| Morganite | 1.577 | −0.0060 | Uniaxial (−) | Yes |
| Chrysoberyl (Yellow) | 1.746 | +0.0076 | Biaxial (+) | Yes |
| Amethyst | 1.544 | +0.0091 | Uniaxial (+) | Yes |
| Citrine | 1.544 | +0.0091 | Uniaxial (+) | Yes |
| Ametrine | 1.544 | +0.0091 | Uniaxial (+) | Yes (two color zones) |
| Pyrope Garnet | 1.714 | — | Isotropic | Yes |
| Almandine Garnet | 1.790 | — | Isotropic | Yes |
| Spessartine Garnet | 1.800 | — | Isotropic | Yes |
| Color-Change Garnet (Pyrope-Spessartine) | 1.760 | — | Isotropic | Yes |
| Grossular Garnet (Tsavorite) | 1.734 | — | Isotropic | Yes |
| Andradite Garnet (Demantoid) | 1.887 | — | Isotropic | Yes |
| Peridot | 1.654 | +0.0360 | Biaxial (+) | Yes |
| YAG | 1.833 | — | Isotropic | No |
| GGG | 1.970 | — | Isotropic | No |
| Benitoite | 1.757 | +0.0470 | Uniaxial (+) | Yes |
| Andalusite | 1.634 | −0.0100 | Biaxial (−) | Yes |
| Opal | 1.450 | — | Isotropic | No |
| Glass (N-BK7) | 1.517 | — | Isotropic | No |
| Glass (F2) | 1.620 | — | Isotropic | No |
| Rutile | 2.616 | +0.287 | Uniaxial (+) | Yes |
| Precious Opal (White) | 1.450 | — | Isotropic | No |
| Precious Opal (Black) | 1.450 | — | Isotropic | Yes (dark body) |
| Labradorite | 1.5625 | +0.0080 | Biaxial (+) | No |
| Moonstone | 1.523 | −0.0080 | Biaxial (−) | No |
| Titanite (Sphene) | 1.894 | +0.1226 | Biaxial (+) | Yes |

The last four are the stones whose look comes from the inside: Precious Opal (White), Precious Opal (Black), Labradorite and Moonstone. Their play-of-color, schiller and adularescence are not part of the material but a choice of the design (Chapters 27 and 28); choosing one of these materials switches its effect on. The older **Opal** is the plain body-color opal and stays as it was. **Titanite (Sphene)**, last in the list, has more fire than diamond (B–G dispersion 0.057 against 0.044) and a birefringence of 0.12, so the back facets show doubled through the table; it is yellow-green, with its strongly colored ray lying in the table. Its indices are measured values from 1887 at three wavelengths; its color follows a measured spectrum of a Madagascar crystal. Typing "Sphene" or "Titanite" finds it. Chrysoberyl (Yellow) shows +0.0076 where older versions showed +0.0090: it now takes the published wavelength-dependent curves of the mineral, whose total birefringence is that smaller figure (see "Curves for the Extraordinary Ray" below).

Several of these colors are not invented: Tanzanite, Aquamarine, Spessartine,
Tsavorite, Demantoid and Peridot are fitted to measured absorption spectra, and
Appendix C names the source of each.

(Sphene/titanite is deliberately not included -- see the Troubleshooting
chapter's Known Limitations.)

## The Single Material Catalogue

Every material picker in the app -- the Stone tab's Material and Preview as
combos, the New Design dialog's Starting Material combo, the Retarget target,
the Rough Planner's material and the material editor -- shares one list: all 34
built-in materials (Ametrine included)
above, in that order, followed by every custom material
you have saved, sorted alphabetically. Whichever picker you open, you see
the same materials in the same relative order (built-ins first, then your own).

## Where the Stone's Material Lives: The Stone Tab

The design's material, refractive-index override and body color are set in one place,
the **Stone** tab of the Parameters dock (Chapter 2), reachable from the Live Render
view and the Edit view alike (the View menu, the Parameters rail at the side of the
viewport, or Ctrl+Alt+1). Each change is an edit of the design and one undo step. The Edit view's
Design Settings panel keeps the geometry and the metadata (gear, symmetry, mirror,
title, header lines, footnotes, reference angle, printed proportions) and shows the
material as a compact read-only chip -- material, color swatch and refractive index --
with an **Edit in Stone Tab** link.

To see the stone in another material without changing it, use **Preview As...** in the
Stone tab (next section). The old "Linked to design" toggle is gone: nothing can leave
the picture on a different material from the design's without a visible amber badge.

## The Crystal Axis of a Material

A material that is not isotropic has a crystal axis, the direction that decides its color and
its doubling in a stone cut from it. Each built-in material brings its own. A custom material
uses the table normal unless you give it a default in the Material Editor's **Crystal Axis**
row (see the field table in Chapter 29). A design's own setting, made in the **Crystal Axis** section
of the Stone tab, wins over both. Chapter 25 explains the axis, the Second Axis of two-axis
materials, and the presets named by the color they give face-up (**Best Blue Face-Up**,
**Violet Face-Up**, **Strongest Color Change**, and **Best Green Face-Up** when one
direction of the stone is green).

## Custom Materials

Building a material of your own, in the Material Editor, has a chapter of its own: Chapter 29 (Custom Materials and Color Patterns). It covers the Material Editor's fields, the dispersion coefficients, the curves for the ordinary and the extraordinary ray, the body color, and Color Patterns. Everything below works the same for a built-in and a custom material.

## RI Override vs. Material

The Stone tab (Parameters dock; the Design Settings panel above the tier list shows
the same material as a read-only chip with an **Edit in Stone Tab** link) holds:

- **Material** -- the combo described above: `(none)`, every built-in, your
  own custom materials, and a final `Custom RI...` entry. Picking a name
  sets the design's material by name; picking `Custom RI...` (or `(none)`)
  clears the name so a typed RI override is the only thing describing this
  design's optics -- useful for a species with no built-in preset and no
  custom material of your own yet.
- **RI Override** -- blank uses the selected material's own refractive
  index; typing a number (greater than 1.0) overrides it.
- **Effective RI** / **Critical Angle** -- read-only. Effective RI resolves
  in this order: your typed override, else the selected material's own real
  RI (built-in **or custom catalogue material**), else the schedule's
  legacy recorded value. Critical Angle is `arcsin(1 / effective RI)` in
  degrees.
- **Body Color** -- the design's own color. The box lists, in this order:
  *Material Default* (the material's own color), the **color variants** the
  program ships for the material (Rubellite, Verdelite and Indicolite for a
  tourmaline, Pink and Yellow for a sapphire), **your own saved variants** for this material, the nine generic
  presets (Clear, Blue, Red and so on) **only when the material has one color in
  every direction** (Diamond, Spinel, the garnets, glass, a custom material that
  is not birefringent), and a final **Custom...** entry. A swatch beside the box
  shows the face-up color of the entry, and the line under it names the color
  of each direction for a pleochroic entry ("Along the Axis: Blue / Across the
  Axis: Green"). The variants and what they mean are the subject of "Color
  Variants and Pleochroism" below. Choosing **Custom...** opens the color
  editor under the box with three sliders: **Tone** (how light or dark, 0 black
  to 100 white), **Saturation** (how rich, 0 grey) and **Hue** (the basic color
  in degrees around the color wheel). For a stone whose crystal shows a
  different color in different directions, the editor has one such color for each
  direction (see below). **Use Picker...** offers the
  hue/saturation/brightness picker instead; the sliders then show the picked
  color's tone, saturation and hue. Press **Apply Color** to use it (one step
  you can undo); **Cancel** changes nothing. The girdle diameter also sizes the
  face-up color the Optimize tab predicts (Chapter 8).
  The color of a gem depends on its size: the same absorption looks deeper in a
  larger stone. The editor therefore shows the color you chose as four swatches,
  at 3 mm, 5 mm, 10 mm and **This stone** (the design's own size: its girdle
  width times 1.5, or 5 mm while no girdle diameter is set). The design keeps
  the color as an absorption spectrum per millimetre, so a larger stone in the
  same design is rendered deeper and a smaller one lighter.
  Under the swatches a badge says **Reachable** with the color difference
  (Delta E) from what you asked for, or, in amber, that the color is **not fully
  reachable** with the seven-band color model and how far the closest reachable
  color is. Very
  rich or very light-and-rich colors are not all reachable; lower the
  saturation or change the tone. The presets above are unchanged and still apply
  at the size of the design.
  The same entry shows when the design holds a color that is not one of the
  presets (one read from a file or made in the color editor). A Custom color
  made for one material stays while you only apply other changes to the same
  material; choosing another material gives the stone that material's own
  color, because a color made for the directions of one crystal does not fit
  another. Choosing a preset, a variant or *Material Default* replaces it.
  Older versions of the program show a close single-color version of a color
  made here, without its directions. The web app has the same editor, solves in its background worker
  and renders the full color in its 3D view.
  The same editor opens from two more places: **Pick...** in the Gem Material
  Editor (Fantasy color; Save keeps the color in the material) and **Custom
  Color...** in the color control of **Preview Color** (while previewing) and of a
  library row. Those two change only the view; the Stone tab's Body Color is
  the one that changes the design (one undo step).

### Color Variants and Pleochroism

A **color variant** is a named color of one material: *Pink* for a
sapphire, *Rubellite* for a tourmaline. It is a complete absorption spectrum
for every direction through the crystal, not a single tint, so a stone that
shows a different color in different directions (a pleochroic stone: sapphire,
tourmaline, alexandrite, tanzanite and most other colored crystals that are not cubic)
keeps that behavior when you give it another color. Everything else about the
stone stays as it is: refractive index, dispersion, birefringence, crystal axis,
inclusions. Only the color changes, and only in this design: a variant never
becomes the material's default and never changes another design.

**Why one row per direction.** A one-axis crystal (uniaxial: sapphire, ruby,
tourmaline, quartz, beryl) has two colors, **Along the Axis** (light vibrating
across the crystal axis, the color you see looking along it) and **Across the
Axis** (light vibrating along the axis). A two-axis crystal (biaxial: tanzanite,
alexandrite, topaz, andalusite, chrysoberyl) has three, **Alpha**, **Beta** and
**Gamma**. The Custom editor therefore shows one color row for each direction
of the stone's material, with a swatch for each and a face-up swatch for what
the stone shows from the table. A pleochroic stone never becomes one flat color
through this editor or the list: a material that has one color in every direction
(cubic crystals, glass) gets one row, and the others get the rows their crystal
needs. The direction rows are seeded from the stone's current color, so a small
change of hue keeps the stone's dichroism. Which direction you see face-up
depends on the stone's Crystal Axis (Chapter 25).

**The list.** The Stone tab's **Body Color** box and the Live Render **Color**
list (Chapter 9) offer the same entries, in this order:

1. **Material Default**, the material's own color.
2. The built-in variants of the material.
3. The variants you saved for this material (below), each under the name you gave it.
   They sit in the same flat list, after the built-in ones; the list has no group headings.
4. The nine generic presets, only for a one-color material.
5. **Custom...**, always last.

A design that has no material name (its material is only a guess from the
refractive index) lists no variants, because a variant belongs to a named material;
choosing the row the design already sits on changes nothing. A pleochroic material
never offers the generic presets, because a preset is one
color in every direction and would flatten the stone. For a pleochroic entry the
line under the box (and the tooltip of the row in the Live Render list) names the
color of each direction. A stone with an old single color on such a material is
described under "Old Designs" below.

**Where the built-in variants come from.** Many are fitted to measured
absorption spectra of the variety, mostly taken with polarized light, for example
those of the Caltech Mineral Spectroscopy Server and the GIA studies of the causes
of color; the others are built from published band positions. A variant is marked
**Estimated** when its spectrum is not a measured one of that variety or when its
strength was chosen for a typical face-up color rather than measured, which is true
of almost all of them: the tooltip of the entry begins with **Estimated** and says
which part is estimated, the Color Variants section of the Gem Material Editor shows
an **Estimated** tag, and the zone variant list adds "(Estimated)". An estimated
variant is a reasonable starting point, not a measured stone. Each variant keeps its sources, which are listed with the research and the
fit records in `docs/color_variants/` of the source tree.

The same beryl color (Heliodor, Red Beryl, Goshenite) is offered for Aquamarine,
Emerald and Morganite. It is defined relative to the crystal axis, and each
material's own default Crystal Axis (Chapter 25) decides which direction you see
face-up, so the same variant looks different on the three. The face-up swatch of a
variant is drawn the same way: the color of the stone seen through the table with
the material's default Crystal Axis, not an average of the directions, while the
tooltip names the color of each direction.

**Save as Variant.** In the Custom editor, **Save as Variant...** asks for a name
(a suggestion in Title Case is filled in), saves the finished colors for this
material in your library, and applies them to the design in one undo step. The
variant then appears in the color list for every design of that material,
and only for that material (it has the directions of that crystal).

**Variant Changed: Update.** A design stores its own copy of the variant it
uses, so it looks the same without your library, on another computer and in the
browser, and it does not change when you edit or delete the variant later. When
the library's variant of the same name has changed since the design took it, the
Stone tab says **Variant Changed** and offers **Update**, which copies the
current numbers into the design as one undo step. If the variant was deleted, the
design keeps its copy.

**Color Variants (Material Editor).** The Gem Material Editor has a **Color
Variants** section (Chapter 29) to look at, rename, duplicate and delete
variants.

**Old Designs.** A design saved before variants, with one of the nine presets or
an old single color, still renders exactly as saved. On a material that has one
color in every direction nothing is wrong. On a pleochroic material the color list shows
**Legacy: <name> (No Pleochroism)** and the Stone tab shows **Legacy Color: No
Pleochroism** with two buttons: **Convert...** opens the Custom editor with that
color on every direction, to adjust each direction (**Apply Color** stores it as a Custom Color for every
direction even if you move no slider), and **Nearest Variant**
replaces the color with the built-in or saved variant (or the material's own
color) that is closest to the old color face-up. Nothing changes until you press one
(one undo step), so old work is never altered silently.

**Zones.** A zone of a Color Pattern (Chapter 29) takes a variant of the stone's
material as its color too, and a stone with zones takes no body color: its
color is its zones.

**The Live Render toolbar.** The Color control next to Render Material offers
the same list for the open design and shows the variant's own name and swatch
(Chapter 9).

**Simple and Advanced.** The Simple interface (Chapter 17) leaves out the Stone tab's
RI Override (which shows its own "Some advanced settings are in use" note when it holds
a value) and the Design Settings panel's Symmetry Order, Mirror and Apply Symmetry controls, the Extra Header
Lines and Footnotes boxes, Gear Ref. Angle, and the printed-proportions row.
Hidden controls still apply: when one of them holds something other than its
default (an RI override, extra header lines, footnotes, a printed proportion, a
reference angle other than zero, Mirror on), the panel shows the line "Some
advanced settings are in use. Switch to Advanced to see them." The **?** in the
panel's header opens this section.

**Export agrees with the on-screen figure.** Export Edited .asc, Save,
and the cutting-sheet export all resolve the design's Material the
same catalogue-aware way the Effective RI chip does -- so a custom
material's own refractive index reaches the exported `I` line and matches
what you see on screen, with no separate "which RI did the export actually
use" question.

**Limitation.** One small estimate does not resolve custom catalogue materials:
the live margin bar that appears under an angle field while you type in it. It
is measured against the built-in materials only. For a design whose Material is a custom catalogue material with no
explicit RI Override, that bar can differ a little from the MARGIN badge until
you commit the angle. The Solid-view windowing-risk hatch, the MARGIN badge,
Live Render, the exports and the cutting sheet all use the custom material's
own index. Setting an explicit RI Override matching the custom material's
value keeps even the typing estimate in agreement.

## Inferred Material: A Guess, Never a Fact

An imported `.asc` file carries only an `I` line -- a bare refractive index,
never a species name. So when a design's Material is unset (`(none)`), the
app looks for the nearest built-in preset within 0.02 of the design's own
effective RI and shows it as a labelled **guess**, never as though it were a
recorded fact:

> **Sapphire? (from RI 1.76)** — with a **Set Material** button next to it.

You'll see this badge in three places, always the same wording: the Stone
tab (under the Material combo), the inspector's Yield tab
(next to the Eff. RI line), and the catalogue detail card for a design with
no material name of its own. The printed cutting sheet's header shows the
same guess text in its "Material" row for an unnamed design, so a printed
sheet never states a species as fact that the app itself is only guessing
at.

Hovering the badge lists any OTHER built-in presets within the same 0.02
tolerance, so you can see what else was close before trusting the nearest
one. Clicking **Set Material** writes the shown name into the design's
Material as an ordinary, undoable edit (Ctrl+Z reverts it like any other
change) -- once a name is set, the guess badge disappears everywhere at
once (there is nothing left to guess), and the name reaches the saved
`.psdd` file and Export Edited .asc the normal way. If nothing built in
is within tolerance, no badge is shown at all -- the app never forces a
guess onto a material it cannot place.

## The Tier List's MARGIN Column

Each pavilion tier (a tier whose C/P/G column reads P) shows how many degrees
its authored angle sits above the design's own effective critical angle,
color-coded:

- **Green** (Safe) -- 2 degrees or more of margin.
- **Amber** (Marginal) -- less than 2 degrees, but still past the critical
  angle. A small edit, a manufacturing tolerance, or a different material
  could tip this into windowing.
- **Red** (Windows) -- already below the critical angle for the design's
  current effective RI. Light leaks straight through this facet.

A **crown** tier (C in the C/P/G column) shows a margin too, suffixed **"(est.)"**
-- an ESTIMATE of whether light entering through that crown facet (rather
than straight down through the table) would still reflect off the
design's own main pavilion facet on the same side of the stone, derived from Snell's law at the crown
facet plus the plain pavilion margin above. It follows exactly one ray in
one cross-section and ignores every other path light actually takes
through a real stone, so treat it as a rough guide, not a certainty --
never the same weight as a pavilion row's own plain margin. A girdle tier
(angle exactly zero), or a crown tier in a design with no pavilion tier at
all to estimate against, shows a plain dash.

The same color-coded bar appears live in the inspector's Tier tab, next
to the Angle field itself, updating as you type -- so a pavilion angle you
are about to save shows red, with its margin and a one-line reason,
*before* you leave the field or click Save Tier.

## Preview As...: Looking at Another Material

The picture always shows the design's own material. When you want to see the same
stone in another material (a sapphire as a ruby, say) without changing the design, use
**Preview As...** in the Stone tab. It replaces the older "Linked to design" toggle and
works differently in three ways:

- **It is explicit.** A preview starts only when you pick a material in **Preview As...**.
  Choosing a material in the Stone tab's Material combo, the Design Settings panel or
  anywhere else edits the design (one undo step) and never leaves a hidden preview behind.
- **It is visible.** While it is on, an amber badge, **Previewing <material>: not the
  design's material**, shows in the Stone tab, in the Live Render and Edit toolbars, in
  the Design Settings panel and in the Export and Tilt Video dialogs (which also state
  which material they will use). The picture, exports and tilt videos use the preview
  material; the tier list and the optimizer keep using the design's own material.
- **It ends with one button.** **Back to Design** returns to the design's material and
  clears any preview color. **Save & Apply** in the material editor and a color
  taken from the Rough Planner also end it, because they change the design's material.
  While a preview is on, edits to the design (including its material) still go to the
  design; the preview material stays as it is until you press Back to Design.

A **library row** you are only browsing has no design material to preview against. The
Stone tab shows **Library design: shown as <material>** with a view-only **Material** and
**Color**; the choice is remembered as the app's preference for library rows and is
never saved with the catalogue entry. The row starts out in the material its design file
names, when it names one (with the file's refractive index and body color), else in the
material its previews were drawn in, else in the one its refractive index points to.

## Which RI Each Part of the App Actually Uses

| Consumer | RI source |
|---|---|
| Design Settings' Effective RI / Critical Angle chips | Catalogue-aware (built-in, custom, or override) |
| Export Edited .asc / Save / cutting sheet | Catalogue-aware (see above) |
| Tier list's MARGIN badges | Catalogue-aware |
| Optimize | Catalogue-aware |
| Tilt curve | Catalogue-aware |
| Live Render viewport | Catalogue-aware |
| Retarget dialog | Catalogue-aware |
| Solid-view facet-map hatch overlay | Catalogue-aware (built-in, custom, or override) |

## Loading a Design: The Material Suggestion

Loading a catalogue design never changes the schedule's own recorded RI on
its own. If the schedule's RI is within 0.01 of a built-in preset's own n_D,
the status strip's Details (Chapter 5) offers **"Set material to X (RI ...)?"**
-- accepting it sets the design's material to that preset (for the
optimizer/tilt-curve/viewport to use its real dispersion), but if that
preset's own n_D would otherwise move the *exported* RI by more than 0.01,
the app pins an explicit RI override to the schedule's own original value
first, so accepting the suggestion never silently changes what a
subsequent export writes. Dismissing it ("No Thanks") leaves the design
exactly as loaded, with no material name set at all.

## Retarget: An Automated Proposal

Click **Retarget...** on the command bar (Chapter 3) to open
a dialog that proposes new pavilion and crown angles for a different
material, for you to review before anything is applied.

- **Target Material** -- a combo of built-in and custom materials, plus a
  **Custom n_D** field for a typed refractive index. It starts on the
  design's own current material. A readout below shows exactly what the
  picker resolves to: the material's name, its n_D, and its critical
  angle.
- **Stage** -- a two-button choice, **1. Shift** and **2. Optimize**; the next
  section explains the two in plain words.
  - **1. Shift** (the default) -- a deterministic move that keeps every
    pavilion tier's own margin over the critical angle fixed at what it
    already was. Always available, and instant.
  - **2. Optimize** -- starts from the Shift result and searches every crown and
    pavilion angle, inside a range you choose, for the best score in the
    target material. It runs only when you press **Search**, in the
    background, and offers up to three valid options to pick from.
- **Crown Handling** (Advanced) -- how the crown tiers follow the
  pavilion. There are three rules. **Crown Follows the Pavilion** is the
  default: a steeper or shallower pavilion makes the stone deeper or
  shallower, and every crown angle is scaled by the same vertical stretch,
  so the stone keeps its silhouette (the crown height against the pavilion
  depth) and its table size. Switch it off to use a **Fraction of Shift**
  slider instead (the crown moves by that share of the pavilion's change in
  critical angle; 0% leaves the crown untouched). **Scale Crown by Ratio
  Instead** scales every crown tier's own angle by the ratio of the two
  materials' critical angles; it wins over the other two rules.
- **The proposal table** -- one row per crown and pavilion tier: its
  block, name, old angle, new angle, margin, and risk badge, so you can see
  exactly what would change and whether it still reads Safe before
  committing to anything. What the table does and does not show is
  explained in the next sections.

### Step 1: Shift, Then Step 2: Optimize

Retarget starts at **1. Shift**. Pick the target material and inspect the shifted
proposal. **2. Optimize** searches from that shift without applying it to the design.
Its comparison keeps the shifted stone on the left even before an optimized
option is selected; the right asks you to select a candidate. An invalid Shift is
labeled with its verdict, or an explicit unavailable message if it has no geometry.
Optimize can still try to repair it. The original design stays unchanged until Apply.

**Shift** is a formula. Every pavilion facet moves by the change in the
critical angle between the two materials, and the crown follows the pavilion's
stretch unless you choose another rule in Crown Handling. It is instant, it always gives the same answer, and it shows
you what "keep the same margin" means for this stone. Its limit is that it
looks at one facet at a time: for a large change of material, or a stone with
little room to spare, the result can be **Not valid** (the girdle disappears,
a facet is lost). The dialog then adds one line to the verdict: **Shift alone
is not valid here. Use Optimize.**

**Optimize** starts from the Shift result and then lets every crown and
pavilion angle move on its own, looking for the best score in the target
material. It can do two things Shift cannot: find a valid stone when the
formula's result is not valid, and give you a better stone than the formula
does, because the angles Shift leaves to a rule (the crown most of all) can be
adjusted too.

*What Optimize may change.* Every crown and pavilion angle, inside the **Range**
you pick (3, 6, 10 or 15 degrees either side of its Shift angle; 6 is the
default). An angle never crosses the horizontal and never goes steeper than
89 degrees. The facet heights that go with the new angles change too, because
each facet turns about the edge it shares with the girdle, exactly as Shift
does. *What it never changes:* the angle of the table or the culet, the
girdle, and any tier whose angle follows a relation (see "Tiers that follow a
relation" below). It also keeps at least half the girdle thickness the design
has now. The table and culet heights are refitted on every option, as Shift
does, so they keep their size. A pavilion angle never drops below the target's
critical angle plus the margin the design has now (at most 2 degrees of it), so
the search cannot buy brilliance with a window.

*Keep the Design's Look.* In Advanced mode the Optimize settings have a switch,
on by default. While it is on, every option (and the Shift start it is compared
with) is scored with a penalty for drifting from the table size and the
crown-to-pavilion ratio the design has now, so the search prefers options that
look like your stone; the verdict quotes the stone's depth and the
crown-to-pavilion ratio next to the old ones. Switch it off and the options are
ranked by the optical score and the yield alone.

*Allow a thicker girdle.* In Advanced mode the dialog has a switch, **Allow a
Thicker Girdle (up to +10 %)**, on by default. When the new angles would pinch
the girdle at its corners, Retarget thickens the girdle band by 5 % and then by
10 % (never thinner) before giving up, so a change that was **Not valid** only
because of the girdle can become valid. It is only tried after a girdle
failure; a change that is valid as it is stays exactly as it was. The plan view,
the table and culet size and the crown-to-pavilion ratio are kept, and only the
total depth grows by the girdle change. The result text says which step was
used. Switch it off to keep the girdle band exactly as it is.

*The scoring light.* The Optimize search and the dialog's optical comparison
score the stone under the **Grading tray** (the evenly lit hemisphere with a head
shadow), whatever lighting the viewport shows, so the options are ranked by the
standard light-return figures.

*The settings.* **Objective** says what the search favours (Balanced weighs
windowing, extinction and brilliance equally; Brilliance, Low windowing, Low
extinction and Keep weight lean one way, and the line under the box says
which). **Range** is described above. **Effort** is how many trial stones the
search may score: Quick 100, Normal 300, Thorough 800. A trial stone is a
**step**.

*How long it takes.* Before you press Search the dialog times one step on your
own design and tells you, for example **Up to 335 steps, about 12 s at 40 ms a
step, plus a few seconds to score the results.** A small design finishes in
seconds; a large one at Thorough effort can take minutes. While it runs, a line
under the Search button names the stage (trying angles, fine-tuning, scoring
the results), the step count and, once there is enough to go on, the most time
that is left. **Cancel** stops the search without closing the dialog. The
search is seeded, not random: the same design, settings and target give the
same options every time.

*The options.* When the search ends, up to three options are listed, best
score first (higher is better, 100 is best). Each shows its score, windowing, brilliance,
extinction, the share of the rough the finished stone gives up (**Yield
Loss**), and **Valid**. Only valid options are listed: every result of the
search goes through the same checks as a Shift result (see "The verdict"
below) against the design as it is now, and a result that fails is dropped. A
note under the summary says how many were dropped and why. If the Shift result
itself was not valid, the search starts from the largest part of the Shift
change that is valid and the first note says so. If the search finds nothing
better than where it started, the note says that too, and what to try (a wider
range, another objective, more steps).

*Picking one.* Click an option and the table, the verdict, the optical
comparison and the comparison pane all switch to it, so you can look before you
decide. Nothing is applied by searching or by picking. **Apply stays disabled
until an option is picked**, and then commits that option as one undo step.

### What Retarget Changes, and What It Never Touches

- **The table and the culet never change.** A flat facet has no slope, so
  there is nothing for a critical angle to say about it; tilting one is
  what used to push the table through zero and onto the pavilion side.
  Both are still listed, greyed out and marked **Not changed**, so you can
  see that they stay put. Their height may still be adjusted (see below),
  but never their angle.
- **The girdle is not listed at all.** Its facets are vertical and
  structural, not optical.
- **A tier that follows a relation is never changed on its own.** If a tier's
  angle is set by a relation such as `P1 - 2` (Chapter 4, "Relations Between
  Tiers"), its row reads **Follows** and the relation where the margin goes.
  It moves exactly as far as the tiers it reads, in both modes: Shift works
  out the new angle from the relation after the others have moved, and
  Optimize leaves it out of the search and works it out again for every
  option. If the relation cannot hold after the change (the result would not
  be a facet angle), the change is **Not valid** and the reason says so.
  Applying keeps the relation true in the same undo step.
- **A shifted angle never crosses the horizontal.** A new angle always stays
  on the side of the old one (crown stays crown, pavilion stays pavilion) and is
  held between 1 and 89.5 degrees. A row that would have gone beyond a limit
  is held at the limit, shows **(held)** after its new angle, and a note
  under the table says which row and why.

### How the Facets Move: They Turn About the Girdle

Changing a facet's angle alone is not enough: every facet is also cut to a
height (its mast), and leaving the height alone while the angle changes
swings the facet around a point deep inside the stone. A steeper crown or
pavilion then pushes out past the girdle, or eats it. That is how a retarget
could make the girdle disappear.

Retarget therefore turns each facet about the edge it shares with the
girdle, so the girdle keeps its outline and its thickness (unless the thicker-girdle
allowance had to thicken it, see above). Where one facet
sits next to another that also moves (a main next to a break facet), it
follows its neighbour's new position. The table and the culet then get a new
height so they keep their size even though the facets around them tilted.
The line under the verdict says how the heights were chosen. Only the
heights change this way; every angle is exactly the one in the table you
reviewed.

### The Verdict: Valid or Not Valid

Under the table the dialog judges the retargeted stone and says so in one
line, for example **Valid: girdle 2.1 % (was 2.3 %), thinnest point 0.40 %
(was 0.50 %), table 56 % (was 56 %)**. The girdle figure is its overall
thickness; the thinnest point is the narrowest the girdle band gets, which on a
faceted girdle is at its corners. The thinnest-point part appears only when it
differs from the overall figure.
It shows **Checking...** while it works (a moment, in the background; the
dialog stays usable) and re-checks every time you change the target material
or the crown handling.

A change is **Not valid** when the stone would no longer be a sound design.
The reasons are listed in plain English, and **Apply is disabled** until you
change the settings so it is valid again:

- the facets no longer close into a stone;
- the girdle disappears, or becomes less than half as thick as it was, either
  overall or at its thinnest point (a stone whose girdle closes to a knife edge
  at its corners is refused even if the overall thickness is unchanged), after
  the thicker-girdle allowance above has had its two tries;
- the table would sit below the top of the girdle, or the culet above the
  bottom of it;
- an original corner of the stone would come apart, an original facet or edge
  would vanish, or the outline would change by more than the facets' own turn
  about the girdle accounts for (each facet may move only as far as its turn
  implies, and the allowance is measured on the angles you asked for, not on
  anything the check repaired afterwards);
- a tier loses all or some of its facets;
- facets become too small to cut;
- a tier that follows a relation cannot follow it any more.

At the Shift stage the verdict is worked out for the formula's result. At the
Optimize stage it is the verdict of the option you picked, and the options are
always valid. The comparison pane beside the table keeps working on a change that
is not valid, so you can look at what goes wrong; it only waits while the check
is running. If the current
design itself does not solve, the check cannot compare anything: the dialog
says **Could not check this change**, and Apply stays dimmed: there is no
checked stone to apply. Fix the design first, then retarget it.

### The Risk Column and Its Margin

The margin is measured in degrees on the safe side of the target material's
critical angle, and it means different things per block (hover the **Margin**
header for a reminder):

- **Pavilion rows** show the plain margin for light entering through the
  table: **Safe** at 2 degrees or more, **Marginal** from 0 to 2,
  **Windows** below 0 (light leaks out of the back).
- **Crown rows** show an estimate, suffixed **est.**, of the same margin for
  light entering through that crown facet, read against the main pavilion
  angle of the *retargeted* design (the same estimate the tier list shows,
  see "The Tier List's MARGIN Column" above). A crown facet is never judged
  with the pavilion formula, which is what used to flag every crown row as
  Windows. The estimate follows a single ray through that crown facet and onto
  the main pavilion facet on the same side of the stone, and nothing else:
  light that wanders to other facets is not counted. Read it as a guide, not
  a measurement. The dialog repeats this in a note whenever crown estimates
  are shown.
- **The table and the culet** show **Not changed** and no margin. A row with
  nothing to measure against (a crown row in a design with no pavilion) shows
  a dash.

### The Optical Comparison

Below the verdict a small table compares three stones on the same three
numbers (windowing, brilliance and extinction, each a percentage of the light,
measured with the table up under the Grading tray): the **current**
design in its current material, the **current** design unchanged in the
**target** material, and the **retargeted** design in the target material. It
shows what the material change costs the stone as it is and what the
retarget wins back. It is a fast estimate for comparing these three columns
with each other, not a replacement for a full tilt sweep (Chapter 8). A column
that cannot be measured reads **n/a** (for example the first one when the
design names no material).

### Errors That Stop a Proposal, and Apply

A design that does not close cannot be retargeted at all: the table is replaced
by **"Design does not solve: ..."**, and a search says it needs a design that
solves first. Optimize works on imported designs whose tiers are all pinned
scale references (Chapter 8): it moves them about the girdle edge, so there
is nothing to **Adopt** first.

Click **Apply** to commit the whole proposal as **one** undoable edit --
the new angles, the adjusted facet heights and the material change
together, so a single Undo reverts all of it at once (the history entry
reads "Retarget for" and the material's name). At the Optimize stage that is the
option you picked. Applying re-solves the design; check the status strip
afterward the same as any other edit. The comparison is inline: it sits beside
the proposal in the dialog, and **Apply** is the only button that commits it.
The Return key obeys the same rules as the button: a change that is still being
checked, or is not valid, or an Optimize stage with no option picked, is refused
with a message. Hover the dimmed **Apply** to read why.

Chapter 14 covers two related tools that live in the same part of the
command bar: **Set Baseline** / **Compare to Baseline**, for diffing
your current angles against an earlier point in the same session, and the
tilt-curve persistence rule for a design that is not yet in your catalogue.

## Doing It by Hand

Retarget's Shift stage already automates the mechanical part of this, but
understanding the reasoning is worth knowing, and Design Settings' MARGIN
column is what you would watch either way:

1. Load the design and note its current pavilion main angle(s) and its
   effective RI (the Design Settings panel's own readout).
2. Work out the critical angle for the target material (critical angle =
   arcsin(1 / RI), measured from the facet's normal). A higher RI gives a
   smaller critical angle; a lower RI gives a larger one.
3. Set the Stone tab's Material combo to the target material
   (or type a Custom RI). Watch the tier list's MARGIN column: any pavilion
   tier that turns red windows at the new RI.
4. On the inspector's Tier tab (Chapter 4), select each red or amber
   pavilion tier in turn and edit its **Angle (deg)** field so the MARGIN
   column reads green again, then **Save Tier**.
5. Click **Solve** (Chapter 5) and check the status strip still reads
   "Closed solid."
6. The Live Render viewport already shows your
   target material (unless you are in a Preview As... session; press Back to Design) -- check the **windowing** and **extinction** readouts
   (Chapter 2) at a range of tilt angles using the tilt-performance graph.
   Iterate on the crown and pavilion angles until performance looks right.
7. Optionally, once at least one tier is a free (meet-based) tier rather
   than a pinned scale reference (see Adopt, Chapter 8), run **Optimize** to
   fine-tune angles for windowing/extinction/tilt-brilliance under the
   design's own material -- Optimize resolves the SAME material (built-ins,
   custom catalogue materials, and any RI override) the viewport and tilt
   curve use, so there is no separate "which material did Optimize actually
   score against" question.

## Authored Versus Effective Refractive Index

The app keeps two distinct notions of "the design's RI": the AUTHORED
value (the legacy `.asc` `I` line as last imported, before any material
selection or override) and the EFFECTIVE value (override, else a resolved
material's own n_D, else the authored value as a last resort —
`Design::effective_refractive_index`). Every scoring, rendering and export
path uses the effective value. The `.psdd` design file keeps
the authored value separate, so it survives a Save / reopen round trip
on its own, with no `.asc` beside it (see Chapter 11).

## Next Steps

Continue to Chapter 7 for a full worked example of building a design from
nothing, using the New Design dialog and putting the tier form, constraints,
and Solve together in practice.
