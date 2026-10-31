# Tutorial videos: debug mode

Debug mode shows what a tutorial run did, step by step, without watching a video. Use it when a
video failed (the finding is in `target/tutorial-videos/report.md`).

## Run

```text
PLEOCHROMA_VIDEOS_DEBUG=1 cargo tutorial-videos -- <part of the tutorial id>
```

PowerShell: `$env:PLEOCHROMA_VIDEOS_DEBUG = "1"; cargo tutorial-videos -- welcome`.

- Arguments after `--` that do not start with `-` pick tutorials by id substring (`welcome`
  runs `welcome-tour`). Pick one video; the debug folder of every tutorial that runs is rewritten.
- `UIPROBE_JOBS=serial` runs the tutorials one after another (easier to read in the console).
- `PLEOCHROMA_VIDEOS_OUT` moves the output folder; `PLEOCHROMA_HARNESS_LIBRARY` picks the library
  as in a video run. Debug runs need no ffmpeg.

No video is recorded: no recorder starts, nothing is captured per frame, ffmpeg is never run. A
plain input driver (`debug_driver.rs`) gives the app the same pointer and key events and the same
mock time as the recorder (holds are pumped in 33 ms steps, a glide ends in one hover move, a click
is a rest, a press and release, then the ripple's time); the screenshots in the debug folder are
the only pictures taken. Only the reading holds are shortened (150 ms per caption cue); every
other hold and every wait is as in a video run, so a step behaves as it does when filmed. Captions,
chapters and key badges do not exist in a debug run.

## What is in the folder

`<out>/debug/<tutorial-id>/` (default `<out>` is `videos/`), cleared at the start of the run:

- `steps.md` and `steps.json`: the same log. Header (video, window size and scale, library kind
  without a path, start time, total time, result, the finding text), then one section per step,
  plus a "Setup before the first step" section and a tail after the last step.
- `NN-before-<window>.png`, `NN-after-<window>.png`: step `NN` before its route acted and after it
  finished, for `main` and every open secondary window: `help`, `rough-planner`, `rough-color`
  (the Rough Color wizard), `compare`, `locate`, `rig`, `color-pattern`. At most 1920 px wide
  (logical size).
- `NN-during-<window>.png` (and `NN-during2-...`, up to `during4`): what a dialog, popup, overlay
  or secondary window looked like right before the step's route closed it, so a dialog that is
  opened and closed inside one step is still on a picture (see "During pictures"). `00-during-...`
  belongs to the setup section.
- `failure-<window>.png`: the screens when the run failed.

A step section lists: time, the guide state before and after (open, step n/total, done, title,
waiting text, highlight target), the image names, and the events in order:

- `route:` the route taken (an authored script, a `perform` recipe, a driven event, a command).
- `C1 click ...`, `H1 move ...`, `W1 wheel ...`: pointer actions with the window, the point (logical
  pixels of that window), the control the route looked for (`target ... at [x, y, w x h]`) and the
  smallest elements under the point (`under the point`). `keys ...` and `typed ...` are key
  chords and typed text.
- `lookup ...`: a control looked up on screen, with the bounds found or `NOT FOUND`.
- `readback ...`: what the route expected and what the app showed, `ok` or `MISMATCH`.
- `result:` `ok` or `FAILED: <error>`.

The markers on the images match the labels: a red ring is a click (`C1`), a blue cross the end of
a cursor move (`H1`), an orange square a wheel turn (`W1`), a green dot (`S`, "before" images only)
where the cursor stood when the step began.

## During pictures

The before and after pictures miss anything a step opens and closes again. `debug_overlays.rs`
lists what counts as "up": the main window's dialogs and overlays (Command Palette, Shortcuts,
Preferences, Cutting Mode, Glossary, Render Jobs, Angle Sweep, Edit as Text, New Design, Gear
Remap, Anchor Explainer, Retarget, Material Editor, Performance Graph, Metadata Editor, the two
batch dialogs) and every secondary window (Help, Rough Planner, Rough Color wizard, Compare,
Locate, Camera Rig, Color Pattern). The tutorial browser and the verdict reasons popup have no
entry (the first is part of every setup, the second has no open flag).

While anything is up, debug mode photographs every open window before each click, wheel turn, key
chord and typed text, and after each hold of 100 ms or more, and keeps only the latest set in
memory. The set is written as `NN-during-<window>.png` when what is up changes (a dialog was
closed) or the step ends. So the picture is the last look before the route closed it, and it
carries the pointer marks the step made so far (the mark of the click that closed it is drawn at
the place the click went to). A thing still up at the end of the step is on the `after` pictures
and gets no `during` set; one that stays up across steps is not photographed again by the next
step. The step's `notes` line says what was up (`during screenshots: taken while up: Preferences`).
A step keeps at most four sets; the rest are counted in a note. The stretch between two steps
takes no `during` pictures (it has no step number to name them with).

Video mode is unchanged: none of this runs without `PLEOCHROMA_VIDEOS_DEBUG`.

## Privacy

The privacy scan of the failure screenshots runs before every screenshot of every open window (the
during pictures too). When
it trips, the images are skipped and the log says `screenshot skipped: private text visible`. Every
logged text (titles, labels, errors, typed text) is checked too; a text with a user name or a
profile path is replaced by `[private text masked]`.

## Checklist for a failed video

1. Read the header of `steps.md`: result, the finding, the failure screenshots.
2. Find the step marked `FAILED` (the last section with events). Read its guide state before and
   after: did the step open, was `done` ever true, what does `waiting` ask for?
3. Read its `route:` and the events in order. The last `lookup ... NOT FOUND` or `MISMATCH` readback
   before the `result:` is usually the cause.
4. Open `NN-before-main.png` and `NN-after-main.png` (and the secondary windows' images). Compare
   each marker with the bounds in the log: is a click ring on the control named by `target`, or on
   something else (`under the point`)? Is the control covered, clipped, disabled or off screen?
5. Compare the readback `expected` and `observed`; check the guide `waiting` text against what the
   screenshot shows.
6. If the failure is before step 1, read the setup section and `failure-main.png`.
7. If the step is fine in the log but the video failed, the cause is outside the steps: read the
   finding text and the last events of the tail section.
