# Appendix A: Glossary

Terms used throughout this manual, gathered in one place. Cutting-instructions
terms are introduced fully in Chapter 3; optical terms in Chapters 2 and 6.

**A note on preferred terms.** This manual and the app say **cutting
instructions**, not "cutting schedule" (an older, dated phrase that means
different things in different Commonwealth countries), and **faceting
diagram**, not "facet diagram" (a pre-1960s term for the plain top/bottom
views of a stone). The catalogue database file keeps its historical name,
`facet_diagrams.sqlite`, purely for compatibility with existing installs —
that file name is not a statement about which term is preferred.

**A note on interchangeable terms.** The app's own UI is not perfectly
consistent about a few of these. "Mast," "depth," and "scale value" all
refer to the same underlying number in different places: **mast** is what
a cutting machine's gauge reads and what the Tier tab's Solved
section shows; **scale value** is what you type into an **Exact Scale Value**
tier's own field; and **depth** shows up in prose (including this manual)
as a plain-language stand-in for either. They are the same number, not
three different measurements. Similarly, "block" and "tier group" both
sometimes describe the crown/pavilion/girdle split — this manual always
uses **block** for that.

**Abbe number** — A single figure for how strongly a material spreads
colors: V_d = (n_d − 1) / (n_F − n_C), using the refractive index at the
sodium D line and at the blue F and red C lines. A lower number means more
dispersion and more fire (diamond is about 55, a crown glass about 64). It
is built on the F–C interval (see **B–G dispersion**). The Material Editor shows it
live when you type a refractive-index curve as coefficients (Chapter 29), and the
Material Reference shows it on each material's details page (Chapter 6).

**Abraded edges** — A narrow worn band of ground surface along every facet edge, as on a stone that has been worn or knocked about. Set with Width and Wear in the Stone tab (Chapter 26).

**AA tap** — One fixed sub-pixel ray position used by Ray-Traced to smooth facet
and silhouette boundaries. Off uses one central tap; 4×, 8× and 16× use fixed prefixes
of the same deterministic sequence. AA does not add missing optical physics.

**Ray-Traced** — The deterministic lightweight renderer: Fresnel-weighted RGB
reflection/refraction chains, spectral absorption tint in 10 nm bands, optional three-index Fire,
and approximate frost. It finishes after its fixed taps without denoising and omits
polarization, birefringent doubling, haze, zoning, edge rounding, oriented inclusions, surface finish and play-of-color.
Path-Traced remains the spectral engine; metrics keep a separate analytic tracer.

**Adularescence** — The soft, floating sheen of a moonstone. The program draws it with a broad family of plates inside the stone, the Adularescence inclusion preset (Chapter 27).

**Along and Across** — The two colors of a pattern zone (or of a stone) in a
one-axis crystal: the **Along** color, seen looking along the crystal axis, and the
**Across** color, which is blended in when looking across it. A two-axis crystal has
three, **Alpha, Beta and Gamma**. A zone shows one color tile for each in the Color
Pattern Editor and needs a birefringent material (Chapters 6 and 29).

**Anchor** — A tier whose scale is stated directly (Meets: **Exact Scale
Value**) rather than derived from meeting other facets. Every block (crown,
pavilion, girdle) needs at least one anchor, or its overall size has
nothing to be measured against.

**Asterism** — A star of light on a gem, the six rays of a star sapphire. It comes from parallel needles that scatter light onto a cone; a true star needs a curved dome, so a faceted stone shows only fragments of one (Chapter 27).

**B–G dispersion** — The figure gemological references print for dispersion: the
refractive index at 430.8 nm (the Fraunhofer G line, violet) minus the index at
686.7 nm (the B line, red). Diamond's is 0.044. The Stone tab's Dispersion chip, the
Material Reference and the command line show it. It is larger than the **F–C**
figure (486.1 to 656.3 nm), which is the one the Material Editor takes and the
**Abbe number** is built on: for a diamond 0.026, in general about 0.58 times the
B–G figure. Both are computed from the same index curve of the material
(Chapter 6).

**Backdrop** — The lit tray under the stone: a matte surface of a chosen reflectance
(**White** 90 %, **Grey** 18 %, **Black** 2 %) lying under the stone out to the horizon
and lit by the lighting rig, or **As Lit**, the lighting's own ground (nothing below the
girdle for the Grading tray). It is seen around the stone and through its windows
alike; an HDR environment map ignores it. It never changes the metrics (Chapter 2).

**Baseline** — The copy of the design that every later change is read against. It
is set when a design opens, after an Optimize or Retarget is applied, and by **Set
Baseline**; the strip under the Inspector's tabs shows each figure's change since it.
It lives only in the session (Chapters 5 and 14).

**Block** — One of the three vertical sections of a stone: crown,
pavilion, or girdle. A tier's block is determined by its angle, not by
what you intended it to be (Chapter 3).

**Birefringence** — The property of some materials that splits light
entering them into two rays travelling at slightly different refractive
indices. Reported in this manual as Δn, the numeric difference between
those two indices at the sodium D line. Zero for an isotropic material.

**Brilliance** — The percentage of light returned to the eye by a rendered
stone; one of the app's optical performance readouts.

**Cauchy equation** — A short formula for a refractive-index curve:
n = A + B/λ² + C/λ^4, with the wavelength λ in micrometres. It describes a
transparent material well across the visible range, and it is what the
Material Editor builds from a refractive index and a dispersion slider
value. You can also type its A, B and C yourself (Chapter 29).

**Centerpoint** — The point on the stone's axis where the facets of a tier meet, at the
culet or at the top of a crown that closes to a point. The cutting sheet says "Cut to
centerpoint." for the tier that makes it, and "Meet P1 at centerpoint" for a tier whose
facets meet a corner on the axis (Chapter 11).

**Chatoyancy** — The narrow band of light across a cat's-eye. It comes from one set of parallel needles that scatter light onto a cone; like asterism it needs a curved dome to show as a clean line (Chapter 27).

**Cheater offset** — A small azimuth (index-wheel) adjustment recorded
against a tier, used when a facet needs to sit slightly off its
mathematically exact index position to actually meet its neighbours
correctly on the real stone. Shown in the cutting sheet (Chapter 11)
alongside the tier's ordinary index.

**Chromophore** — The trace element or defect in a gem material responsible
for its color (for example, Cr3+ in ruby, Fe2+ in aquamarine). The Material
Editor's physics color mode builds a color from chromophores; that editor is
shown only in builds with the `physical-color` feature (Chapter 6), but a
material's stored chromophore recipe is kept and rendered in every build.

**Cleavage** — The tendency of a crystal to split along flat planes when struck, set
by its atomic structure. A gem is described by how many directions it cleaves in and
how easily (perfect, good, distinct, imperfect, poor), with the plane's Miller
indices, for example "perfect basal {001}" for topaz. A stone with a perfect cleavage
can split in cutting, polishing and mounting. Glass and opal have none. Shown as the
cleavage line of the Stone tab's identity card (amber when it is a cutting risk) and in the
Material Reference (Chapter 6).

**Code** — A tier's label on the cutting sheet, in cutting mode and in the tier
table, where it starts each row: P1, P2, ... for pavilion tiers, G1, ... for girdle tiers,
C1, C2, ... for crown tiers, T for the table and Culet for the culet. Each letter
is numbered on its own, in the order the tiers are cut, and a concave tier carries
on the count of its side (Chapters 3 and 11).

**Color Pattern** — Zones of different colors inside a custom material, such as a
pink core in a green rim, made in the Color Pattern Editor and shown in Path-Traced
views (Chapter 6).

**Color Variant** — A named color of one material, such as Pink for a
sapphire or Rubellite for a tourmaline: a complete absorption spectrum for every direction through the crystal, so
a pleochroic stone keeps its different colors in different directions when you pick
it. Some shipped variants are fitted to measured spectra, and the others are
estimates from published band positions (they say "Estimated"); you can save your own from the Custom editor with
**Save as Variant...** (Chapters 6 and 29).

**Compare (to Baseline)** — The dialog that diffs the current design's
tiers and masts against the Baseline (Chapter
14). Not a comparison against the catalogue's printed proportions — see
Deep Solve for that.

**Contrast** — A number from 0 to 100 that says how finely the viewer's own shadow is
broken up across the face-up stone: 100 times the mean difference of the Blue share
between neighboring spots of the measuring grid. It is Pleochroma's own measure, not an
AGS or GIA figure; a high value means finely broken-up shadow, not a better grade. Its
relation to the "pattern" wording of Moses et al. 2004 (Gems & Gemology 40(3)) is in
Chapter 2, "How the Face-Up Figures Are Measured".

**Critical angle** — The angle, measured from a facet's normal, beyond
which light hitting that facet from inside the stone reflects internally
rather than escaping. Set by the material's refractive index: a higher RI
gives a smaller critical angle.

**Crown** — The block of a stone above the girdle, culminating in the
table.

**Crystal Axis (c-axis)** — The direction of a crystal's special optical axis in a cut
stone, as a line: its **tilt** is the angle to the table normal (0 to 90 degrees) and its
**azimuth** the direction it leans toward, counted clockwise seen from above from the
first position of the index gear. It decides a birefringent stone's face-up color and
doubling. The design's own setting, a custom material's default and the material's own
axis apply in that order (Chapter 25).

**Culet** — The lowest point of the pavilion on many cuts, where the
pavilion facets meet at (or near) a point.

**Deep Solve** — A slow, read-only diagnostic that checks a solved design's
geometry against the catalogue's own printed proportions. Never modifies
the design (Chapter 8).

**Detach / Reattach** — The item in a tier row's "..." menu that exempts a tier's index positions
from automatic orbit-consistency handling, for a deliberately asymmetric
design (Chapter 4).

**Dispersion** — How much a material's refractive index varies across the
visible spectrum, which is what produces "fire" (spectral flare). The app shows it as
**B–G dispersion**, the figure gem references print (diamond 0.044); the
Material Editor takes it as **F–C** (Δn(F–C), diamond 0.026), and the
**Abbe number** V_d (lower means more dispersive) is built on F–C. A custom material
can carry its full curve as Sellmeier or Cauchy coefficients (Chapter 29).

**Doubling** — How far apart, in arc-minutes, the two images of a birefringent stone fall
when you look through the table: the ordinary and the extraordinary ray leave the stone
at slightly different points. Zero along the crystal axis. Shown beside **Axis** in the
viewport's metrics bar (Chapter 25).

**Extinction** — The percentage of a stone's face reading as dark, dead
shadow rather than bright; one of the app's optical performance readouts. It
counts the light that is **Lost** inside, and the light that leaves upward but is
unlit or hidden by the head shadow (Chapter 2, "How the Face-Up Figures Are
Measured").

**Estimate (RI range)** — The refractive-index range the app traces for a design as cut.
It is always labelled an estimate, on screen and on a cutting sheet, because it is the
app's own trace and not a designer's statement (Chapter 6).

**Face-up tone** — The lightness `L*` and color strength `C*` of the light a
stone returns face-up, worked out from how far that light travelled through the
body color, under the lighting rig of the run. Shown in the Optimize tab's
Tone column and swatches.

**Faceting machine** — In the program, your machine's description in the Preferences: whether the dop arm points at the lap centre or the mast stands to the side, the arm offset, the lap radius and the sweep range. It decides the direction of the polishing scratches (Chapters 17 and 26).

**Facet** — One flat cut face of a stone. A single tier normally describes
a whole symmetric family of identical facets (an orbit), not just one.

**Fire** — A unitless index of spectral flare (the rainbow-colored flashes
caused by dispersion); one of the app's optical performance readouts.

**Fracture** — How a crystal breaks where it does not cleave: conchoidal (shell-like
curves, as in glass), uneven, subconchoidal or splintery. Given with the gem data
(Chapter 6).

**Fraunhofer lines** — The dark lines in the sun's spectrum that serve as fixed
wavelengths for measuring refractive index and dispersion. The ones used here are
G (430.8 nm, violet), F (486.1 nm, blue-green), D (589.3 nm, yellow, the sodium line
of n_D), C (656.3 nm, red) and B (686.7 nm, red). B–G dispersion uses the outer
pair and F–C the middle pair. The Material Reference marks all five on its
refractive-index chart (Chapter 6).

**Free tier** — A tier whose Meets constraint is **Unspecified Vertex** or
**Named Facet(s)** — its depth is derived by the solver from where its
plane meets its neighbours, rather than stated directly.

**Gear** — The cutting machine's dividing head: the number of possible
index positions around the stone (96, 80, 77, 72, 64, 120, or a custom
count). Changing a design's gear remaps every tier's indices (Chapter 6).

**Girdle** — The narrow band at a stone's widest point, separating crown
from pavilion.

**Ground finish** — A facet that scatters light like frost instead of reflecting it as a mirror: a rough surface. The finer grain is Ground (Fine), the coarser Ground (Coarse); a frosted facet is a ground one (Chapter 26).

**Head shadow** — How much of the sky your own head hides when you look
straight down at a stone: the dark table reflections a face-up stone really shows.
A 0–30° setting in the Lighting & Environment tab's Observer section (default 16°).
It applies to the lit models and is saved with every saved lighting and with a
design's own lighting (Chapter 2).

**Inclusion haze** — A milky veil inside the stone, from clean to milky, drawn by
scattering light from tiny inclusions. One of the four per-design material effects
of the Stone tab (Chapter 2).

**Inclusions (silk)** — Needles or plates inside a stone that all point one way and scatter light according to the direction it travels: the silk of a sapphire, the tubes of a tourmaline. A per-design choice in the Stone tab (Chapter 27).

**Index** — The position on the cutting machine's dividing head (its
"gear") where a facet is cut.

**Leak** — The part of the face where light goes out through the pavilion instead of
returning: the same rays Windowing counts, and the part of the **Dark** zone that
corresponds to ASET leakage (see *Zones (ASET-style)*).

**Level girdle** — A girdle band of the same thickness all the way around. On the
cutting sheet, "Cut to level girdle." is a crown or pavilion tier whose facets all end
on the girdle line the earlier tiers set (Chapter 11); a tier that cuts into the
girdle makes it not level (Chapter 5).

**Lighting rig** — One of the ways of lighting the stone in the live view and in
renders: the lit models (light tent, grading tray and others), the studio rigs and
the daylight rigs.
Picked in the Lighting & Environment tab (Chapter 2). A rig is
only the lights; **Saved lighting** stores a rig together with everything else about
the look.

**Lost** — The part of the face where a ray finds no way out within 10 bounces, or
leaves the stone sideways. With **Leak** it forms the **Dark** share; Lost is not an
ASET zone, and it counts toward Extinction.

**Material effects** — The settings of the Stone tab that belong to the
individual stone and are saved in its design: Inclusion Haze, Crystal Axis,
Frosted Girdle and Edge Rounding, Inclusions, Abraded Edges and
Play-of-Color. A new design starts with all of them off (Chapter 2). The
Surface Finish of a tier is saved with the tier (Chapter 26).

**Mast** — The solved distance of a facet's plane from the centre, along
its own normal — the number a cutting machine's mast gauge would read.
Always a solver output, never something you type in directly.

**Meet point** — The point where three or more facet planes intersect. A
facet's depth is usually determined by where its plane meets its
neighbours, rather than set as a direct number. On the cutting sheet a
meet point is a corner that is already on the stone when the tier is cut;
a tier that reaches none prints only its depth and is cut to it by
measurement (Chapter 11). When the named facets meet in more than one
corner, the solver takes the shallowest, unless you have picked one (see
*Picked corner*, Chapter 4).

**Mohs hardness** — The scale of scratch resistance from 1 (talc) to 10 (diamond):
a material scratches any material below it. A species is given as a range, for
example 7.5–8 for emerald. It says nothing about how well a stone resists a blow;
that is **toughness**. A glass has no Mohs figure of its own and shows about 5.5
with its Knoop hardness (Chapter 6).

**Noise threshold** — How far a figure has to move before the program calls it a
real change. Brilliance, Windowing and Extinction: 2 points; Scintillation: 3;
Fire: 5 % (at least half an index point); tilt averages: 1 point. A change under it
is "about the same" and shows grey in the Baseline strip, the Trials table and the
Angle Sweep readout (Chapter 14).

**Optical character** — Whether a material's index behaviour is isotropic
(one index in every direction), uniaxial (positive or negative, one
special axis), or biaxial (positive or negative, two special axes). See
Appendix C.

**Optimize** — The coordinate search that adjusts a design's free tiers'
angles to improve windowing, extinction, and tilt brilliance (Chapter 8).

**Orbit** — The complete set of index positions a symmetric tier occupies.

**Orientation rule** — How the Rough Planner is asked to orient stones to the crystal
axis of the rough: **Table Across**, **In the Table Plane**, a custom angle, or a
color-named rule, with a tolerance in degrees. **Preferred** lets a stone off the rule
still count at a reduced value; **Required** does not. A rule can be overridden for the
plan, a design or a stone, with a Reason (Chapter 25).

**Output defaults** — The settings the Export and Tilt Video dialogs open with: the
ones of your last run (size, samples, bounces, color space, compute, transfer). Shown
in the Render & Output tab and remembered whenever a run starts (Chapter 9).

**Parameters dock** — The right-hand panel with three tabs: Stone, Lighting &
Environment and Render & Output. Open from the toolbar buttons, the View menu, the
command palette or Ctrl+Alt+1/2/3, in the Live Render view and in the Edit view
(Chapter 2).

**Parting** — A break along a plane, like cleavage, but only where the crystal is
twinned or stressed, so a given stone may or may not show it (the Gem Data lists
basal and rhombohedral parting for sapphire and ruby). Shown in the Material Reference when the material has one
(Chapter 6).

**Pavilion** — The block of a stone below the girdle, usually culminating
in a culet or point.

**Picked corner** — The corner of the stone you chose for a tier in the Solid
view with **Pick in View**, **Pick Meet** or **M**. It is stored as the three
facets that meet there, so it follows them when their angles change, and the
tier is cut until its plane passes through it. Without one, a tier takes the
shallowest corner where its named facets meet (Chapter 4; Chapter 13,
"Picking a Meet Point").

**Pin** — The item in a tier row's "..." menu that freezes a tier's current solved mast as a new
**Exact Scale Value** anchor, converting a free tier back into a pinned
one — the reverse of Adopt (Chapter 3, Chapter 8).

**Pinned tier** — A tier whose Meets constraint is **Exact Scale Value** —
its size is a stated number, not derived. Every tier loaded from a real
`.asc` file starts pinned.

**Planned piece** — The sawn piece of rough a planned stone comes from, clipped by
the rough's outline, as the editor's preform. See Chapter 15, From a Result to the
Bench.

**Play-of-color** — The flashes of spectral color of a precious opal, which change as the stone or the light moves. They come from orderly patches of tiny silica spheres that reflect one narrow band of wavelengths (Chapter 28).

**Pleochroism** — A material showing different colors (dichroism for two,
trichroism for three) depending on the direction light travels through it,
relative to its crystal axes. The stone's Crystal Axis, and for a two-axis stone its
Second Axis, decide which color you see through the table (Chapter 25). A color
variant or a Custom Color has one color for each direction (Along and Across for a
one-axis stone, Alpha, Beta and Gamma for a two-axis one), so changing the color of a
pleochroic stone keeps its pleochroism (Chapter 6).

**Preform** — The rough shape a design is cut from before faceting (for
example, a cylinder), or the planned piece of rough the Rough Planner hands over
with **Open in Editor**.

**Preview As...** — The Stone tab's view-only way to look at the stone in another
material. The design keeps its own material; an amber badge says "Previewing" on every
surface the picture reaches, and **Back to Design** returns. It never starts by itself
(Chapter 6).

**Refractive index (RI)** — A measure of how strongly a material bends
light, expressed as n at a reference wavelength (this manual, like the
app's render materials, uses the sodium D line, 589.3nm, written n_D).

**Relation** — A rule that makes one tier's angle follow other tiers' angles
instead of being a number of its own: `P2 = P1 - 2` keeps P2 two degrees
shallower than P1, whatever P1 becomes. You type it into the Tier form's
Angle field after an `=` (`=P1-2`); the tier table marks such a tier with a
link icon. Saved with the design (Chapter 4; Chapter 11 for the file
version). An exported `.asc` carries the plain angle.

**Render job** — A saved set of render settings and a frozen copy of the stone,
waiting to be rendered. See Chapter 23.

**Render queue** — The list of render jobs in the Render Jobs window. It renders
one job at a time, from the top down, once you press **Start Queue**. See
Chapter 23.

**RI range** — The span of refractive indices a design is said to work in. A designer's
own statement (typed, or from a `.gcs` or `.psdd` file) and the app's traced estimate are
kept apart; the cutting sheet's "RI range" row prints whichever the design's RI range
field asks for (Chapters 6 and 11).

**Retarget** — The command bar's automated proposal for adapting a
design's pavilion and crown angles to a different material, reviewed in a
dialog before anything is applied (Chapter 6).

**Snapshot** — Former name of the **Baseline**. The command bar offers **Set
Baseline** and **Compare to Baseline** (Chapter 14).

**Saved lighting** — A named, complete lighting look: the lighting rig, light
direction, exposure, surface glare, backdrop (a lit tray), head shadow, the HDR environment map if
any and optionally the camera pose. Eight come with the app; your own are added to the
list and can be marked "usable for" export (Chapters 2 and 9).

**Schiller** — The single sheet of flashing color on a labradorite face, seen from one direction. It comes from fine lamellae in the crystal and is fixed to the crystal, so it follows the Crystal Axis (Chapter 28).

**Scintillation** — The percentage measure of sparkle as light and stone
move relative to each other; one of the app's optical performance
readouts.

**Second Axis** — For a biaxial material, the turn, from 0 to 180 degrees, of its two
other principal directions about the crystal axis. It decides which of a trichroic
stone's three colors faces up through the table. **Not Set** keeps the material's own
(Chapter 25).

**Sellmeier equation** — The standard formula for the refractive-index curve
of glass and crystals: n² = 1 + the sum of B·λ²/(λ² − C), one term per
resonance (usually one or three), with the wavelength λ in micrometres. Each
B is a strength and each C is a resonance wavelength squared, in µm². The
built-in Diamond, Sapphire and glass entries use it, and the Material
Editor accepts one-term and three-term fits (Chapter 29).

**Silk Turn** — A turn, from 0 to 180 degrees, of a stone's inclusions about its Crystal Axis. For a uniaxial material it is the stored Second Axis used only to orient the silk (Chapters 25 and 27).

**Solved / stale** — A design is "solved" once Solve has successfully run
against its current tier list; it becomes "stale" the moment any further
edit changes that tier list, until Solve runs again (Chapter 5).

**Stone Pose** — The position the stone is stood in on the tray for a picture, set by Tilt (0° to 180°), Turn and Spin (0° to 360°) in the Lighting tab. The stone rests on the tray at its lowest point; the lights, the tray and the camera stay put. It changes the Live Render picture, exports and render jobs, not the design, the metrics or the Edit tab (Chapter 2).

**Stone Shadow** — The optional shadow and colored caustic light cast by the stone onto a backdrop touching its culet. Studio lights only; off by default.

**Stone width** — The width of the stone across the girdle in millimetres: the
design's girdle diameter, set in the Stone tab (the same value as the Preform tab's
Girdle Diameter). It sets how deep a colored stone's color looks; when it is not set
the colors are shown for 7 mm (Chapters 2 and 6).

**Surface finish** — How well a tier or a single facet is polished, or how coarsely it is ground: Ideal, Mirror, Fine, Commercial, Prepolish, Ground (Fine), Ground (Coarse) or Custom. A real polish scatters a little light into a halo stretched across its scratches; a ground finish scatters like frost (Chapter 26).

**Sweep range** — The inner and outer radius of the lap between which the stone travels in the last polishing pass. It sets how widely the polishing scratches fan out (Chapter 26).

**Table** — The large, flat facet at the top of the crown.

**TIS (total integrated scatter)** — The fraction of the light that a rough surface scatters into a halo instead of reflecting or refracting it cleanly. It grows with the square of the roughness and falls with the square of the wavelength, so a poor polish hazes blue (Chapter 26).

**Yield** — What fraction of the rough preform's volume ends up in the
finished stone, and the resulting estimated carat weight — the inspector's
Preform tab computes both once a girdle diameter is set (Chapter 4). The
carat estimate uses the specific gravity of the design's material, set in
the Design Settings panel (Chapter 6), unless a Specific Gravity Override
is typed.

**Tier** — One row of the cutting instructions: one facet, or one symmetric
family of identical facets cut at the same angle and depth.

**Tilt average** — The mean of brilliance, windowing or extinction over the tilts of the
stone from table up, on the four tilt axes. By default it covers all tilts to ±90°,
4 axes × 181 tilt angles = 724 entries. The Tilt Average Range preference can narrow
what the Compare window and the Baseline figures show, for example to ±30° with
4 × 61 = 244 entries (Chapter 17). Optimize, Angle Sweep, Retarget and Tune always
rate all tilts to ±90°.

**Tilt curve / tilt axis** — The app's sweep of a design's optical
performance as it tips away from face-up viewing, along a chosen compass
direction around the stone (Chapter 2).

**Toughness** — A stone's resistance to breaking, chipping or cracking, graded by
GIA from very poor to exceptional. It is separate from hardness: a diamond is the
hardest gem, but its toughness is only good to exceptional because it cleaves. A material no gemological source grades reads "Not graded"
(Chapter 6).

**Tray Reflection** — The optional glossy surface under the stone: a smooth, endless mirror at the culet whose head-on reflectance is the slider value (about 4 % is black acrylic or glass, 100 % a mirror) and which reflects more at glancing angles. The stone appears mirrored in it. Studio lights with a backdrop only; off by default.

**Trial** — A numbered record of the design at a moment you kept it, with its
figures, listed in the History tab's Trials view. Trials last for the session only;
**Save as Variant...** keeps one for good (Chapter 18).

**Tune** — The Optimize tab's hand-driven mode (**Search | Tune**): the tiers one by
one in a tuning order, with a small sweep of the selected tier, nudge buttons and
Accept, Revert This Tier and Next Tier (Chapter 8).

**Two-Tier Sweep** — The Angle Sweep's **Two Tiers** mode: two tiers' angles swept
together over a grid and drawn as a heat map, up to 900 angle pairs (Chapter 21).

**Variant** — A named copy of the open design that you keep so you can come
back to it or compare it with another version. Variants live in your
library, not in the design file. Saved and opened from the inspector's
History tab (Chapter 18).

**Windowing** — The percentage of a stone's face reading as see-through
rather than reflective, caused by light leaking straight through the
pavilion instead of totally internally reflecting; one of the app's
optical performance readouts. The same share is the **Leak** part of the
Dark zone.

**Zone (pattern zone)** — One region of a Color Pattern with its own color: the
**base zone** that fills the stone wherever no other zone is, and up to four more
zones, each with a shape. The Color Pattern Editor lists them as layers, top layer
first; an upper layer covers the ones below it (Chapter 29).

**Zones (ASET-style)** — The block of the FACE-UP card that splits the face into
**Red** (light leaving 45° to 75° above the girdle plane), **Green** (0° to 45°),
**Blue** (75° to 90°, where your head is) and **Dark** (**Leak** plus **Lost**), as
percentages. They are the zones of the lighting Contrast view (ASET-style), with this
program's own blended edges instead of the published sharp limits; **Lost** is not an
ASET zone. They do not depend on the lighting. ASET is a registered trademark of the
Gemological Institute of America, Inc. (GIA); Pleochroma is not affiliated with or
endorsed by GIA or the American Gem Society (Chapter 2, "How the Face-Up Figures Are
Measured").

## Next Steps

Appendix B lists the app's keyboard shortcuts; Appendix C tables every
built-in render material's optical properties.
