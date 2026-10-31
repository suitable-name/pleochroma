# 25. Crystal Axis

Everything in this chapter is part of every build of the program. It applies to every material that is not isotropic; for an isotropic material (Diamond, Spinel, Cubic Zirconia and the like) the controls described here are hidden or say that there is no crystal axis to orient.

## What You Will Do

Many gem materials look different depending on how the stone sits in its crystal. Looking along the special axis of a sapphire you see one color and one sharp image; looking across it you see another color, and a strongly birefringent stone shows its back facets doubled. The direction of that axis in the cut stone is the **Crystal Axis**. This chapter shows how to:

- read what the Crystal Axis is, and how the program measures it;
- see it, set it and turn it for one stone in the editor;
- choose a default axis for a custom material;
- tell the Rough Planner how the crystal runs in a piece of rough, and let it orient the stones to it, with rules you can override and give a reason for;
- carry the true orientation from a plan into the design, the cutting sheet and Cutting Mode, so the stone is cut from the rough the way the picture shows it.

Chapter 2 describes the Stone tab, Chapter 6 materials, Chapter 15 the Rough Planner and Chapter 20 Cutting Mode. This chapter is the one place that explains how they work together.

## The Idea

### The Axis Is a Line

The **Crystal Axis** (the c-axis of the crystal, written "c-axis" once on the cutting sheet) is a *line*, not an arrow: pointing it up or down gives the same stone. The program therefore shows it in one standard form, leaning to the upper side of the stone.

Two numbers describe it, measured with the table facing up:

- **Tilt** is the angle between the axis and the table normal, from 0 to 90 degrees. Tilt 0 runs the axis straight up through the table, so you look along it. Tilt 90 lays it flat in the plane of the table, so you look across it.
- **Azimuth** is the direction the axis leans toward, as an angle around the table normal, from 0 to 360 degrees. It is counted from the model's +X side toward +Z, which is clockwise as seen from above the table. It has no effect while the tilt is 0.

### Azimuth and the Index Gear

Azimuth 0 points to the first position of the index gear, which is index 0 when the design has no reference angle. Azimuth then grows the way the indices grow. The cutting sheet prints the azimuth of a leaning or flat axis as an index of the design's gear: the index is the azimuth divided by 360 degrees, times the number of teeth, less the design's reference angle. On a 96-tooth gear with no reference angle, azimuth 90 degrees is index 24 and a half turn is index 48. The pictures on the cutting sheet and in Cutting Mode say "Index 0 at the Right" for the same reason: they are drawn from above with index 0 on the right.

### The Second Axis for Two-Axis Crystals

A biaxial crystal (Tanzanite, Alexandrite, Andalusite, Peridot, Topaz, Chrysoberyl and some custom materials) has three principal directions, not two. The Crystal Axis fixes one of them (the one with the largest index). The **Second Axis** says how the other two are turned about it, from 0 to 180 degrees. For a trichroic stone such as Tanzanite the three directions show three different colors, so the Second Axis decides which color you see through the table. For a stone with no strong color difference between the directions it only moves the doubling a little.

Without a Second Axis the material's own completion is used. The Stone tab then says "Second axis not set; the color across the first axis is approximate." for Tanzanite, Alexandrite, Andalusite and for a two-axis custom material with direction-dependent color. A uniaxial or isotropic material has no Second Axis for its optics.

### The Turn That Orients the Silk

Inclusions with a direction (Chapter 27) need to know more than the Crystal Axis. The rutile needles of a sapphire lie in the plane across the axis, in three directions 60 degrees apart, and which of the three is the first is a turn about the axis. For this the Stone tab's **Inclusions** group offers a **Silk Turn**, from 0 to 180 degrees. For a uniaxial stone it is the same stored turn as the Second Axis, used only to orient the inclusions: the optics of a uniaxial stone never read it, so setting it changes neither the doubling nor the color. A stone that has no inclusions renders exactly as before whatever turn is stored. For a two-axis stone (Chrysoberyl, Alexandrite) the inclusions follow the real Second Axis, and the **Cat's-Eye Silk** preset needs it to be set.

### Colors Named by What You See

For the materials where the color depends strongly on the direction, the program offers presets named by the color they give face-up, not by an axis letter. The name describes the ray you look along, which is the ray you do not see. The face-up color of each preset is derived from the material's own absorption, and its swatch is shown beside the name.

| Material | Presets |
|---|---|
| Tanzanite | **Best Blue Face-Up** and **Violet Face-Up** |
| Alexandrite | **Strongest Color Change** (the direction that changes most between daylight and a warm lamp) |
| Andalusite | **Best Green Face-Up**, offered only when one direction of the stone is green (a green hue with some saturation); the built-in color of the material shows none, so it offers no color-named preset until a variant with a green direction is chosen |
| Peridot | none: its three colors are close, yellow-green to green |

A custom material has none of these. Presets of the other kind (Through the Table, In the Table Plane, Along the Width) work for every material that has a Crystal Axis.

### Doubling

A birefringent stone splits every light path in two: an ordinary ray and an extraordinary ray travel at different speeds and leave the stone at slightly different points. Looking through the table you see the back facets twice, a little apart. The program measures how far apart the two images fall, averaged over the light the stone returns, and shows the figure as **Doubling**, in arc-minutes.

Doubling is zero when you look straight along the axis (the axis through the table) and grows as the axis leans away from it, in proportion to how strongly birefringent the material is. That is why the usual rule for Zircon, Peridot, Rutile and Synthetic Moissanite is to put the table across the axis. Where a material has its own curve for the extraordinary index (Chapter 6, "Curves for the Extraordinary Ray"), the two images also separate a little by color, because the direction in which the extraordinary ray is displaced follows the index at each wavelength. The figures of the Brilliance, Fire and Scintillation readouts count both images, half of the light each. A material with no birefringence has no Doubling, and the Doubling figure is not shown for it.

Doubling is a figure to read. It never changes a score or a Verdict by itself.

### How the Axis Changes What You See

For a material whose color depends on the direction (Tourmaline, Tanzanite, Alexandrite and others), the face-up color, the tilt curve and the metrics all follow the axis. Rendering a stone with the axis through the table and then flat in the table plane gives two visibly different stones. A material that is not direction dependent changes only through doubling.

## Materials

### Built-In Materials

Each built-in material brings its own axis: the table normal for most, and an axis lying in the plane of the table for Tourmaline and Aquamarine, the way they are usually cut. The material editor shows the axis of the selected built-in material read-only. A design can always set its own axis, which wins over the material's.

### A Default Axis for a Custom Material

In the Material Editor (Chapter 6), every custom material that is not isotropic has a **Crystal Axis** row: **Not Set** keeps the built-in behavior, **Set** takes a **Tilt** and an **Azimuth**. For a two-axis material the row sets the axis of the largest index, and a **Second Axis** row with a **Turn** follows once the axis is set. A design that uses the material takes this axis unless it sets its own. The order of precedence is always: the design's own setting, then the material's default, then the built-in axis.

Saving the material stores the default with it, and a design file that carries the material in its `[material.custom]` table carries the default too. Changing the default turns every design on that material, and the catalogue thumbnails of those designs are drawn again.

## The Editor

### The Stone Tab

Open the **Stone** tab of the Parameters dock (Chapter 2). The **Crystal Axis** section has two switches:

- **As Material** keeps the material's axis (or its stored default). The stone is rendered as the material says.
- **Set for This Stone** takes the tilt, azimuth and, for two-axis materials, the Second Axis you give below. They are saved in the design.

The presets are one click each:

- **Through the Table** puts the axis straight up through the table.
- **In the Table Plane** lays it flat in the table plane.
- **Along the Width** lays it flat along the width of the stone, the narrowest direction across the girdle. The stone must be solved, because the width comes from its solved shape.
- The material's color-named presets, with a swatch of the color each gives.

The preset that equals the current axis is lit. **Tilt** and **Azimuth** are sliders, with their values in degrees, shown while **Set for This Stone** is on. For a two-axis material **Second Axis** offers **Not Set** and **Set**, and a **Turn** slider from 0 to 180 degrees when it is set. Each choice is one undoable change of the design, like every other edit.

For an isotropic material the section reads "This material is isotropic (e.g. Diamond, Spinel, Cubic Zirconia). It has no crystal axis to orient."

### Showing the Axis on the Stone

The **Crystal Axis** pill in the Solid viewport's toolbar draws the axis over the stone as a line through its centre, with a handle at the upper end. A two-axis stone also shows its Second Axis, with a handle of its own. The line is drawn in the Solid, Path-Traced, Solid + Path-Traced and Ray-Traced views and in the Diagram's profile panel. The pill is offered only for a material that has an axis.

The pill is off the first time. It is remembered for you (the `show_crystal_axis` key of the settings file), and it also switches itself on for the rest of the session the first time you set an axis, and when you open a planned stone that came with one. Switching it off yourself keeps it off.

**To turn the axis by hand,** drag its handle. The stone changes while you drag: the tilt and azimuth sliders follow the handle, and the picture re-renders. Releasing the mouse keeps the axis as one undo step. **Escape** during the drag puts the axis back the way it was. Pressing a handle of a stone that is on **As Material** turns **Set for This Stone** on, as moving a slider does. **Undo** takes the whole drag back.

### Readouts

The metrics bar over the viewport shows two more figures for a stone that has an axis. Hover the bar for their meaning.

- **Axis:** the angle between the Crystal Axis and the table normal, for example 12 degrees.
- **Doubling:** how far apart the two images of a birefringent stone fall, in arc-minutes, once the first measurement is done.

### The Verdict Note

When the material has an orientation rule (the table of rules below) the Verdict adds one note to its reasons. It says how far the axis is from the usual orientation, for example "The crystal axis leans 8° from the table normal, which follows the usual orientation for Sapphire (Table Across, within 15°)." The note never changes the Verdict, the headline or a score; it is listed after the reasons that do. A material with no rule gets no note.

### Where the Axis Is Used

The axis a stone has is used by every picture and every figure of it: Live Render, exported pictures, render jobs, a remote worker, the catalogue thumbnail, tilt curves and videos, Compare, the Color Pattern preview, and the scoring behind Optimize, the angle sweeps, Tune, Baseline, Trials and Retarget (a Retarget to another material keeps the stone's orientation). The command-line tool renders with it too. A design that sets no axis, on a material that has no stored default, renders and scores as it always did, and its thumbnails are not redrawn.

For a birefringent built-in material the face-up color, the light-return figures and the Doubling follow the axis, which they did not before. Their pictures and figures therefore changed once, on purpose, and then stay the same until you change the axis. Appendix C lists each material's optical character.

## The Rough Planner

The Rough Planner (Chapter 15) can use the crystal's direction in the rough. You tell it how the crystal runs, it orients the stones to a rule, and it hands each stone to the editor with its true orientation.

### The Axis of the Rough

In the left column, between the **ROUGH** and **SHAPE** sections, the **CRYSTAL AXIS** panel appears for a material that has an axis. It starts as **Unknown**: the planner knows nothing about the crystal and plans exactly as it always did. Press **Set** to give the direction. Every change is one undo step, like every edit of the rough.

With **Set**:

- **Tilt (deg)** is the angle between the axis and the rough's own Y axis, 0 to 90 degrees.
- **Azimuth (deg)** is its direction around the Y axis, from X toward Z, 0 to 360 degrees.
- **Second Axis (deg)** appears for a two-axis material. Leave it empty for the material's own completion.
- A line under the fields says in plain words how the axis runs in the rough.

Four ways find the axis for you:

- **Align to Face** (then **Click a Face...**): click a face of the rough in the 3D view and the axis is set along its normal. Click the pill again to cancel.
- **Align to Edge** (then **Click the First End...** and **Click the Second End...**): click two points on the rough, such as the ends of a crystal edge, and the axis is set along the line between them.
- **Fit to Crystal Length**: sets the axis along the longest direction of the rough. A crystal is usually longest along its axis, but not always, so the panel says how sure it is: "Fitted to the rough's length. Confidence 85 % (the rough is 2.4 times longer than wide)." When the longest direction hardly stands out it says so and asks you to check the axis against the crystal.
- **From Color Zones**: sets the axis along the axis of a color zone you accepted in the Rough Color wizard (Chapter 24), a cylinder or prism around the crystal. It is available only when the plan has such a zone.

In the 3D view the rough's axis is drawn as a cyan line through the rough with a handle at its tip. Drag the handle ("Drag to turn the crystal axis") to turn the axis; a drag anywhere else orbits as usual. In a result view, a short amber line through every stone shows the axis the stones were planned with. In the photo overlays of Locate Inclusion from Photos and of the Rough Color wizard, the axis is drawn as a line over each photo.

### The Orientation Rule

As soon as the axis is **Set**, the material's usual rule is switched on, and a line says so: "Stones are oriented to the crystal axis: Table Across (Sapphire default)". The planner never applies a rule while the axis is Unknown, and a material with no rule shows no line.

A rule has a **kind**, a **tolerance** and a **strength**:

- **Table Across**: the axis runs through the table (tilt 0 in the stone).
- **In the Table Plane**: the axis lies flat (tilt 90).
- **A Custom Angle**: a **Tilt (deg)** and, optionally, an **Azimuth (deg)** in the stone's own frame. Leave the azimuth empty when the direction around does not matter.
- the material's color-named rules (**Best Blue Face-Up**, **Violet Face-Up**, **Strongest Color Change**, **Best Green Face-Up**) for the materials that have them: the stone is turned so that principal direction faces up through the table. **Best Green Face-Up** appears only while a direction of the stone is green.
- **Tolerance (deg)** is how many degrees a stone may be off before it counts as off.
- **Preferred** lets a stone that is off the rule still count, but its value drops by **Weight (% per deg)** for every degree beyond the tolerance (0.5 % is the default). **Required** lets no stone be off by more than the tolerance.

These are the rules the program proposes for the built-in materials. It never applies one silently.

| Material | Default rule | Tolerance |
|---|---|---|
| Sapphire, Ruby | Table Across | 15 degrees |
| Emerald | Table Across | 20 degrees |
| Aquamarine, Benitoite | In the Table Plane | 15 degrees |
| Tourmaline | In the Table Plane | 10 degrees |
| Zircon, Synthetic Moissanite, Peridot | Table Across (their doubling) | 15 degrees |
| Rutile | Table Across | 10 degrees |
| Tanzanite | Best Blue Face-Up | 15 degrees |
| Alexandrite | Strongest Color Change | 15 degrees |
| Andalusite, the Quartz family, Topaz, Chrysoberyl (Yellow) | none | |
| a one-axis custom material with direction-dependent color | Table Across | 15 degrees |
| a two-axis custom material with direction-dependent color | none (choose a rule for it) | |
| every other custom material, every isotropic material | none | |

Under **ORIENTATION RULE** you can replace the default for the whole plan. The first choice is **Material Default**, then **Ignore the Axis** (the planner cuts for the most weight and no stone is judged against the axis) or **Set** (your own kind, tolerance, strength and weight). A **Reason** field saves why you differ from the default. A rule you change applies the next time you plan: the amber note "Rules changed since planning. The stones keep the orientation they were planned with; plan again to apply them." says so, and the same for a change of the axis itself. Each change is one undo step, together with the axis.

### What the Planner Does with the Rule

The rule enters the search like any other value: a stone that follows it counts in full, a stone beyond the tolerance counts less (Preferred) or not at all (Required). A stone in a block cut from the rough has four poses with the same piece and weight (half turns about its three box axes), and the planner picks the one nearest the rule; the pose it chose is kept with the plan.

A plan with a **Preferred** rule also gives you layouts that make the trade visible. The result list adds alternatives next to the plan's own layouts, labelled **Best Orientation (Unrefined)**, **Balanced (Unrefined)** and, when it differs, **Best Yield (Unrefined)**: the best layout for the most weight, for the most stones in line with the rule, and the knee between them. "Unrefined" says that they are the best layout of one cut order and have not gone through the extra refinement of the plan's own layouts. A plan with no rule, or with a Required rule, has no alternatives and costs nothing extra.

### Reading the Stones

Under the results, **STONES AND THE CRYSTAL AXIS** lists the stones of the selected result, locked stones first. Each row shows:

- the stone and its design, and **Locked** when it is locked;
- "Axis 12° from the table normal": the true angle in the stone's own frame, from the pose the stone really has;
- the rule column: a check mark when the stone follows the rule, "15° off" when it does not, and "overridden (reason)" when you turned it by hand;
- for a two-axis material, "Color direction 20° from the table normal": the angle between the direction the rule asks for and the table normal.

The text of the **Cut Plan** has the same lines: a block "Crystal Axis of the stones" lists them, with each stone's angle and status.

### Overriding the Planner

You can differ from the planner at three levels, and each carries an optional **Reason** that is saved with the plan and printed on the cutting sheet.

1. **The plan**: **Ignore the Axis** or your own rule under **ORIENTATION RULE**, as above.
2. **A design**: select a design row in the results. Its rule appears under the stone rows, with **Follow the Plan**, **Ignore the Axis** and **Set**, and its own Reason. A rule you change here applies the next time you plan.
3. **A stone**: each row has these controls.
   - **Pose 1, 2, 3, 4**: the stone as the planner placed it, or turned a half turn about one of its box axes. The piece and the weight stay; only the crystal axis angle changes.
   - **Re-Fit**: fits the best stone of this design into the same piece again, under the rule, without moving the saw cuts.
   - **Free Orientation** (for a single stone fitted exactly): turn the stone by hand in the 3D view. The planner sizes it to the largest stone that fits in the new position. **Done Turning** ends it.
   - **Reason**: why this stone is set up differently.

A stone you turned says "overridden (reason)" in its row. Every override is one undo step, and all of them are saved with the plan and come back when you open it again.

### Lock: Keep a Stone and Plan the Rest

**Lock** keeps one stone as it is: its piece becomes a solid inside the rough that no other stone may enter, and the next plan cuts the rest of the rough around it. A plan with a locked stone runs the way a plan for a mesh rough runs, also for a block, and its layouts list the locked stone first, outside the cut plan.

The limitation is stated in the results: **the locked stone is cut first; the slabs below are for the remaining rough.** The saw plan of the remaining rough is a plan of slabs and bars across the whole piece. It cannot in general be cut around a piece that sits in the interior, so the program lists the locked stone ahead of it and plans the rest on its own. You cut the locked stone out of the rough first, and then follow the plan for what is left. **Unlock** lets the next plan use the piece again.

### The Hand-Off to the Editor

**Open in Editor** on a design row carries the stone's TRUE orientation into the design: the rough's orientation, turned by the pose the stone actually has (the pose you chose, or the one the planner chose), never the rule's target. The editor writes it as **Set for This Stone** in the same single undo step that sets the preform and the stone's width, switches on the **Crystal Axis** display for the session, and the Stone tab shows **Set for This Stone** with the stone's tilt and azimuth. The rule the stone was planned under and its Reason are stored in the design too, so the cutting sheet can check the stone against the rule it was planned for. If the rough's axis is Unknown, or the material is isotropic, the design gets no axis and the hand-off is as before.

### Saving and Sharing

A saved plan holds the rough's axis, the rule and every override with its Reason, and each stone's pose, Lock and Reason. A plan that sets none of these is saved as before. **Export** writes them into the plan file and **Import...** brings them back (Chapter 15).

## Color Patterns That Follow the Axis

A zoned custom material (a Watermelon tourmaline, for example) has an **Axis** in its Color Pattern Editor (Chapter 6). The entry **Follows Crystal Axis** turns the pattern with the stone's Crystal Axis: the zones' long direction runs along the axis wherever it points, whether it is the material's default, the design's own setting or a preset. Change the axis of the stone and the zones turn with it in Live Render, exported pictures, thumbnails and scoring. Table Normal, Across, Across Turned and Custom keep their fixed meaning. A pattern adopted from the Rough Color wizard keeps the frame it was measured in ("Imported") and does not follow.

## The Cutting Sheet and Cutting Mode

### The Orientation Block

When a stone's axis is set (by the design or by its material's default), the cutting sheet gains an **Orientation** block (Chapter 11). It lists:

- "Crystal axis: 40.0° from the table normal, azimuth 120.0° (index 32 of 96)", with the second axis turn for a two-axis stone: "Second axis: turned 37.0° about the crystal axis";
- the sentence "Orient the rough so the crystal axis (c-axis) leans 40° from the table normal, toward azimuth 120° (index 32)." (or "runs straight through the table", or "lies flat in the table plane, along azimuth ...");
- the rule check: "Orientation rule: Table Across (Sapphire default, within 15°): followed", or "20° off";
- the **Reason** of any override.

Beside the list are two small drawings: the stone from above, with index 0 at the right, and from the side with the table up. A design with no axis set prints the same sheet it always did.

### Cutting Mode

The first step of Cutting Mode shows the same instruction (Chapter 20): the sentence, the rule line and the Reason are the first lines of the step's note, and a picture under them shows the stone **From Above (Index 0 at the Right)** and **From the Side (Table Up)**, with the axis in amber. The picture draws the Crystal Axis only, not the Second Axis. Only the first step has it, because it tells you how to hold the rough before you cut.

## Mobile and Web

**Mobile.** The Stone tab's **Crystal Axis** row switches between **As Material** and **Set for This Stone**, and offers the same presets as the desktop, with the color-named ones for the materials that have them. A two-axis material has **Second Axis** and **Second Axis Turn**. The orientation uses the same precedence (design, material default, built-in). The metrics read out **Doubling** for a birefringent stone, Retarget keeps the orientation, and Cutting Mode carries the orientation sentence and picture on its first step, the same words as the desktop.

**Web.** A design that you open in the web app brings its own axis: while the render is linked to the design, the panel shows **Crystal Axis (from the design)** and its values, and the design wins over the per-user setting. Switch off **Linked to Design** to use the **Crystal Axis Override** yourself, with its **Second Axis** and **Second Axis Turn** for a two-axis material. The web tilt sweep keeps the axis. The browser's metrics line does not show Doubling.

## Files

A design's own axis is saved in its `[render_effects]` table (`c_axis_override_enabled`, `c_axis_tilt_deg`, `c_axis_azimuth_deg`, and `c_axis_second_azimuth_deg` only when the Second Axis is set). A stone that came from a plan also carries a `[crystal_axis_plan]` table with the rule and its Reason. A custom material's default axis is stored with the material. Every one of these is written only when it is used, so a design without them is byte for byte what an older build wrote, and an older build opens such a design and ignores them (Chapter 11).

## Limitations

- The axis is a line: tilt and azimuth give its direction, not which end points up.
- **Along the Width** needs a solved design, and ignores concave tools in its outline.
- The Second Axis exists for a biaxial material only (the **Silk Turn** of a uniaxial material, Chapter 27, is the same turn used to orient inclusions and nothing else). For Topaz and Chrysoberyl (Yellow), which are biaxial with no direction-dependent color, it moves the doubling only. Peridot's color difference between directions is small.
- A block of rough has four poses per stone; **Re-Fit** and **Free Orientation** reach the others.
- The planner never orients to the crystal when the axis is **Unknown**, and a rule applies only at the next plan.
- Alternatives of a Preferred rule are not refined (**Unrefined**), and add to the planning time of a block rough.
- **Lock** plans the rest of the rough on its own and lists the locked stone first. The cut plan below it is for the remaining rough only.
- A pattern imported from Rough Color does not follow the axis, and a zone color measured in the rough stays in the stone's own frame.
- The phone and desktop pictures show the Crystal Axis, not the Second Axis. The web HUD shows no Doubling.
- Optimize, the sweeps and the tilt curves do roughly twice the work per figure for a birefringent material, because they follow both images of the light.

## Troubleshooting

| Symptom | Cause | What to do |
|---|---|---|
| The Crystal Axis section says the material is isotropic | The material has the same index and color in every direction. | Nothing to set. Pick a birefringent material if you want an axis. |
| The **Crystal Axis** pill is missing | The material is isotropic, or the view does not draw the stone. | Pick a material with an axis; use Solid, Path-Traced, Solid + Path-Traced, Ray-Traced or Diagram. |
| **Along the Width** is missing | The design is not solved. | Solve it (Chapter 5). |
| "Second axis not set; the color across the first axis is approximate." | The material has direction-dependent color and the stone has no Second Axis. | Choose a color-named preset, or set the Second Axis. |
| The Doubling figure is not shown | The material has no birefringence, or the first measurement is not done yet. | Wait for the figures, or pick a birefringent material. |
| The planner has no **CRYSTAL AXIS** panel | The material has no crystal axis. | Choose another material. |
| **From Color Zones** is dim | The plan has no accepted zone along the crystal. | Fit zones in the Rough Color wizard (Chapter 24), or set the axis by hand. |
| Rows still show the old orientation | The rules or the axis changed after planning. | Plan again. |
| The cutting sheet has no Orientation block | The design sets no axis and its material has no stored default, or the material is isotropic. | Set the axis in the Stone tab, or hand the stone over from a plan. |

## Next Steps

Chapter 6 covers materials, Chapter 29 custom materials and Color Patterns, Chapter 15 the Rough Planner, Chapter 20 Cutting Mode, Chapter 24 the Rough Color wizard, and Chapter 27 the inclusions that follow the axis, Chapter 22 the tutorials: Orient a Sapphire, Tourmaline With the Axis in the Table, Best Blue Tanzanite, Set a Stone's Crystal Axis and Override a Stone's Orientation.
