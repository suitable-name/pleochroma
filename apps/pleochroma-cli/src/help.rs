//! The help pages: [`text`] returns the page for a [`Topic`].
//!
//! The pages are plain strings so a test can check that every flag the parser takes is on its
//! page, and the README can quote them.

use crate::args::Topic;

const ROOT: &str = "\
pleochroma-cli: solve, check, optimize, retarget, sweep and export faceting designs without a window.

Usage:
  pleochroma-cli <command> <design>... [options]
  pleochroma-cli <command> --help

A <design> is a .psdd, .asc, .gem or .gcs file. Every command that
takes a design takes several (the shell expands globs as usual): see Several designs below.

Commands:
  info      Name, gear, symmetry, tier list and material of a design.
  solve     Solve the design, report closure and warnings, optionally save it.
  metrics   Windowing, brilliance, extinction, fire and the proportions of the stone.
  validate  Manufacturability warnings and the overall Good / Check / Problem verdict.
  optimize  Search for better facet angles and optionally save the best result.
  retarget  Adapt the angles to another material with the dialog's validity gate.
  sweep     Score one tier over a range of angles.
  snap-meets  Move tiers that reach no meet point onto the nearest point on the stone.
  export    Write .asc, .psdd, .gcs or the cutting sheet (.html or .pdf).
  census    Measure the meet-preserving gate over templates, files and a library (read-only).
  render      Render a still picture from a render job file (*.job.json).
  tilt-video  Render a tilt video from a render job file; resumes from its frames.
  image-compare  Compare two PLRAW1 renders as a viewer sees them (render-acceleration benchmark).

render and tilt-video take a job file instead of a design: pleochroma-cli render --help.

Options for every command:
  -h, --help     Show the page of a command (pleochroma-cli retarget --help).
  -V, --version  Show the version.
  --no-update-check
                 Do not ask GitHub for a newer release first. Without it, a newer release
                 is named in one info line on standard error (never on standard output);
                 the variable PLEOCHROMA_NO_UPDATE_CHECK (any value) does the same.
  --radv-workaround on|off
                 GPU builds on Linux: work around Mesa's RADV driver, which renders some
                 zoned stones (opal) and stones with oriented inclusions wrong (sets
                 ACO_DEBUG=nosched, about 10 % slower GPU). Default: on only with an AMD
                 GPU on the amdgpu kernel driver;
                 the variable PLEOCHROMA_RADV_WORKAROUND (on or off) does the same.

Material and lighting:
  --material NAME   A built-in material, or a custom one of the --db library.
  --ri N            A bare refractive index instead (a flat, non-dispersive material).
  --db FILE         A design library (facet_diagrams.sqlite), opened read-only, for its
                    custom materials.
  --lighting NAME   daylight, incandescent, ring, spotlight, iso (or grading: the grading
                    tray, the default), tent, dome (sky, no sun), sun (sky plus direct sun),
                    tray (white tray), shop, window, illuminant-a or aset.

Several designs:
  pleochroma-cli metrics *.psdd --csv
  Each design runs as the command would run on it alone, in the order given.
  - Text reports follow one another, each under a \"==> FILE <==\" line and a blank line between.
    JSON reports follow one another as complete documents. metrics --csv prints its header
    line once and a row for each design.
  - --out FILE (for sweep, --csv FILE) names one file, so it is refused with several designs.
    --out-dir DIR writes one file for each design instead, named after the design without its
    extension: DIR/<name>.<extension of that output> (.psdd for solve, optimize, retarget and
    snap-meets; .txt, .json or .csv for a report; the chosen format for export, which then needs
    --format). DIR must exist.
    Two designs that would get the same file name are refused before anything runs.
  - A design that fails prints its error under its name and the others still run. The exit code
    is the first failure's code (else 4 when a validate verdict is Problem or a snap-meets run
    left something to look at, else 0), and the files of the designs that worked are written.
    The last line says how many failed.

Exit codes:
  0  Done.
  1  The command line is wrong.
  2  The design cannot be used, or a result was refused (nothing is written then).
  3  A file could not be read or written.
  4  validate: the verdict is Problem. snap-meets: a tier still has no meet point, or the
     snap cut facets away (the snapped design is written all the same).
  5  render and tilt-video only: the render failed or was stopped.

Output is deterministic: fixed decimals, stable order, JSON with sorted keys. (render and
tilt-video print their progress and an estimate of the time left to standard error.)

Angles: the text reports show every facet angle as a positive number, and the block
(crown, pavilion, girdle) says which side of the girdle it is on. JSON keeps the stored
signed convention instead, the one the design files use: a pavilion angle is negative.
";

const INFO: &str = "\
Usage: pleochroma-cli info <design>... [--json] [--out FILE | --out-dir DIR] [--db FILE]

Prints the design's name, gear, symmetry, preform (shape, size and offset), material and the
tier list (standard code, the tier's own name, block, angle as a positive number, index
positions, what each tier meets or follows). Does not solve.

  --json       Print JSON instead of text.
  --out FILE   Write the report to FILE instead of printing it.
  --out-dir DIR
               With several designs: write each report to DIR/<name>.txt (.json with --json).
  --db FILE    A design library, for custom materials.
";

const SOLVE: &str = "\
Usage: pleochroma-cli solve <design>... [--out FILE.psdd | --out-dir DIR] [--json] [--db FILE]

Solves every tier, checks that the facets enclose a stone and lists the manufacturability
warnings. Exit code 2 when the design does not solve or does not close.

  --out FILE   Save the design as a .psdd file (also converts .asc, .gem and .gcs).
               Nothing is written when the design does not solve and close.
  --out-dir DIR
               With several designs: save each as DIR/<name>.psdd.
  --json       Print JSON instead of text.
  --db FILE    A design library, for custom materials.
";

const METRICS: &str = "\
Usage: pleochroma-cli metrics <design>... [--material NAME | --ri N] [--tilt]
                              [--json | --csv] [--lighting NAME] [--out FILE | --out-dir DIR]
                              [--db FILE]

Table-up windowing, brilliance, extinction, fire and scintillation, the proportions of the
stone and its yield. With --tilt the tilt performance is averaged over four axes and 181
angles too (about a second and a half).

Without --material or --ri the design's own material is used; a design with none is scored
with its refractive index as a flat material, and the report says so.

Two girdle figures, both as a percentage of the width:
  girdle_pct_of_width       (\"Girdle thickness\") the full height of the girdle faces, from
                            their highest corner to their lowest. On a brilliant it includes
                            the scallops the break facets cut, so it reads high; it is the
                            girdle the total depth adds up with.
  girdle_line_pct_of_width  (\"Girdle line\") the thinnest thickness at the girdle line, the
                            corners where the girdle faces meet: the girdle the editor's
                            verdict and the cutting sheet read. Empty (null in JSON, n/a in
                            text) when the stone has no girdle line. It is the LAST CSV column,
                            so scripts that read the columns by position keep working.

  --material NAME   Score in this material.
  --ri N            Score in a material of this refractive index.
  --tilt            Also average the tilt performance.
  --json | --csv    Print JSON, or a header line and one row, instead of text.
  --lighting NAME   The light the score is taken under (default grading, the grading tray).
  --out FILE        Write the report to FILE instead of printing it.
  --out-dir DIR     With several designs: write each report to DIR/<name>.txt (.json, .csv).
  --db FILE         A design library, for custom materials.
";

const VALIDATE: &str = "\
Usage: pleochroma-cli validate <design>... [--json] [--material NAME | --ri N]
                               [--lighting NAME] [--out FILE | --out-dir DIR] [--db FILE]

Lists the manufacturability warnings and the overall verdict of the desktop editor's status
strip: Good, Check or Problem, with the reasons. Exit code 4 when the verdict is Problem,
2 when the design cannot be read. With several designs the exit code is 4 when none failed
and at least one is a Problem.

  --json            Print JSON instead of text.
  --material NAME   Judge the optics in this material instead of the design's own.
  --ri N            Judge the optics in a material of this refractive index.
  --lighting NAME   The light the optics are measured under (default grading, the grading tray).
  --out FILE        Write the report to FILE instead of printing it.
  --out-dir DIR     With several designs: write each report to DIR/<name>.txt (.json).
  --db FILE         A design library, for custom materials.
";

const OPTIMIZE: &str = "\
Usage: pleochroma-cli optimize <design>... [--preset NAME] [--budget N] [--starts N] [--seed N]
                               [--vary-anchored] [--candidates N] [--lighting NAME]
                               [--json] [--out FILE.psdd | --out-dir DIR] [--db FILE]

Runs the Optimize tab's search on the design's own material and lists the ranked candidates.
The best one is saved with --out. When the search finds nothing better, nothing is written.

  --preset NAME      balanced (default), brilliance, low-windowing, low-extinction,
                     keep-weight, lighten-dark or intensify-pale. The last two also steer the
                     face-up tone (lighter, or a stronger color) of a colored material,
                     sized by the design's girdle diameter.
  --budget N         Evaluations of the coordinate stage (default 800), shared by all starts.
  --starts N         Starting arrangements to try, 1 to 32 (default 8). Each start gets at least
                     four sweeps of the free tiers; a budget too small for that runs fewer
                     starts, and --starts 1 is the plain single descent.
  --seed N           The search seed (default 0). The same inputs give the same result.
  --vary-anchored    Turn pinned (scale-reference) tiers about their girdle edges too. On by
                     default when every tier is pinned, as in the tab.
  --candidates N     How many ranked candidates to keep, 1 to 5 (default 3).
  --lighting NAME    The light the search scores and tones under (default grading, the canonical
                     preset: a run without --lighting is scored under the grading tray).
  --json             Print JSON instead of text.
  --out FILE         Save the best candidate as a .psdd file.
  --out-dir DIR      With several designs: save each best candidate as DIR/<name>.psdd.
  --db FILE          A design library, for custom materials.
";

const RETARGET: &str = "\
Usage: pleochroma-cli retarget <design>... (--material NAME | --ri N) [--mode shift|optimize]
                               [--crown-follow | --crown-fraction F | --crown-ratio]
                               [--preset NAME]
                               [--range DEG] [--budget N] [--seed N] [--no-keep-look]
                               [--girdle-allowance] [--lighting NAME]
                               [--json] [--out FILE.psdd | --out-dir DIR] [--db FILE]

Adapts the facet angles to another material with the Retarget dialog's engine and validity
gate: every facet turns about its girdle corners, and the result is checked for a closed
stone, a girdle that survives, a table that stays flat and facets that do not vanish.
A result the gate refuses is never written: the reasons are printed and the exit code is 2.
Only a Valid verdict applies, as in the dialog's Apply: a change that could not be checked (the
current design does not solve or close) is refused with the same verdict line, so there is no
option to let one through. The design's material is set to the target in the saved file.

  --material NAME      The material to retarget for.
  --ri N               A bare refractive index instead.
  --mode shift         The critical-angle shift only (default).
  --mode optimize      The shift, then a search around it for better angles.
  --crown-follow       Scale every crown angle's tangent by the pavilion's vertical stretch, so
                       the stone keeps its silhouette and table size (default: the crown
                       follows the pavilion's stretch).
  --crown-fraction F   Move the crown by this share (0 to 1) of the pavilion's shift
                       instead (0 leaves the crown where it is).
  --crown-ratio        Scale the crown angle by the ratio of the two critical angles instead.
  --preset NAME        What the search favours (optimize mode; default balanced).
  --range DEG          Degrees either side of each angle the search may move (default 6).
  --budget N           Evaluations of the search (default 300).
  --seed N             The search seed (default 0).
  --no-keep-look       Do not penalise options that drift from the design's table size and
                       crown-to-pavilion ratio (optimize mode; by default every option is
                       scored with that penalty).
  --girdle-allowance   Allow a thicker girdle, as the dialog's \"Allow a Thicker Girdle (up to
                       +10 %)\": when the new angles would pinch the girdle at its corners, thicken
                       the band by 5 % and then 10 % (never thinner) before giving up. Plan view,
                       table and culet size and the crown-to-pavilion ratio are kept exactly; the
                       result text says which step was used. On in the dialog; off here unless
                       given, so a result that is refused without it may be valid with it.
  --lighting NAME      The light everything is scored under (default grading, the grading tray).
  --json               Print JSON instead of text.
  --out FILE           Save the retargeted design as a .psdd file.
  --out-dir DIR        With several designs: save each as DIR/<name>.psdd.
  --db FILE            A design library, for custom materials.
";

const SWEEP: &str = "\
Usage: pleochroma-cli sweep <design>... --tier NAME --from A --to B --step S [--tilt]
                            [--csv FILE | --out-dir DIR] [--json] [--material NAME | --ri N]
                            [--lighting NAME] [--db FILE]

Sets one tier to every angle from A to B in steps of S, solves and scores each one, and
prints the table, from the flattest angle to the steepest. The design's own angle is always
a row. Angles are positive numbers, and the side of the girdle comes from the tier:
--from 39 --to 43 --step 0.5 sweeps a pavilion tier. A sign in front is ignored, so
--from -43 --to -39 does the same. The text table and the CSV show positive angles; the
JSON keeps the stored signed ones (a pavilion angle is negative).

  --tier NAME       The tier: its name (P1, G1/G2), or #N for the Nth row of the tier list.
  --from A --to B   The range in degrees from flat, either end first.
  --step S          The distance between angles; at most 200 angles in all.
  --tilt            Also average the tilt performance of every row (about 1.4 s a row).
  --csv FILE        Write the rows as CSV to FILE.
  --out-dir DIR     With several designs: write each one's rows to DIR/<name>.csv.
  --json            Print JSON instead of text.
  --material NAME   Score in this material instead of the design's own.
  --ri N            Score in a material of this refractive index.
  --lighting NAME   The light the score is taken under (default grading, the grading tray).
  --db FILE         A design library, for custom materials.
";

const SNAP_MEETS: &str = "\
Usage: pleochroma-cli snap-meets <design>... (--out FILE.psdd | --out-dir DIR | --dry-run)
                                 [--max-move MM | --max-move-fraction F] [--db FILE]

Moves the depth of every tier that reaches no meet point onto the nearest point already on
the stone, so the cutter has a point to cut it to. The cutting sheet's meet column names the
point a tier's facets reach at the moment the tier is cut, on the stone cut so far. A tier set
to a free depth (by Optimize, or by hand) often reaches none, and the sheet then gives only
its depth (\"Cut to depth 2.81 mm.\"). This command walks the cutting order and gives each
such tier that is set by an explicit depth the depth that puts it through the nearest point
of the stone cut so far, when that point is within the limit. The anchors (the tiers that set
the stone size, the centerpoint, the girdle width or the girdle line) never move, and no angle
or index changes. The design is then solved again and its meet points worked out once more.

It prints one line for each tier it moved, the facets the new depths cut away, then the tiers
that still have no meet point:
  P2  0.7662 -> 0.7598  (moved 0.03 mm, now meets P1, G1)
  Cut away by the snap: P3 (all 10 facets) and C3 (3 of 10 facets)
  Still no meet point: T (0.83 mm off)
Distances are in millimeters when the design has a girdle diameter, else a share of the
stone's size (1.2 % of the stone's size).

The nearest point can be another point than the designer meant, and the new depths can cut
small tiers away: moving a depth changes the design. The snapped stone is checked by the same
rule as the editor's Snap to Meets fix. The snap is refused (exit code 2, nothing is written)
when the stone would no longer solve or close, would lose its girdle or get a much thinner
one, or would put the table or the culet on the wrong side of the girdle. Facets that are cut
away do not refuse it; they are listed. Check the optics afterwards (pleochroma-cli metrics,
validate).

  --out FILE           Save the snapped design as a .psdd file (also converts .asc, .gem and
                       .gcs). Tiers that still have no meet point do not stop the save.
  --out-dir DIR        With several designs: save each as DIR/<name>.psdd.
  --dry-run            Only report what would move. Nothing is written, even with --out.
  --max-move MM        Move a tier at most this many millimeters. Needs the design's girdle
                       diameter (the stone's size); a design without one refuses it: use
                       --max-move-fraction instead.
  --max-move-fraction F
                       Move a tier at most this share of the stone's size (default 0.02, that
                       is 0.2 mm on a 10 mm stone).
  --db FILE            A design library, for custom materials.

Exit codes: 0 every tier but the anchors meets a point and nothing was cut away;
4 some tier still has no meet point, or the snap cut facets away (the snapped design is
still written); 1 the command line is wrong, or --max-move is given for a design without a
girdle diameter; 2 the design does not solve or close, before or after the snap, or the snap
would break the stone (nothing is written); 3 a file could not be read or written.
";

const EXPORT: &str = "\
Usage: pleochroma-cli export <design>... --format asc|psdd|gcs|html|pdf
                             (--out FILE | --out-dir DIR) [--db FILE] [--date TEXT]

Writes the solved design in another form. The format may be left out when FILE ends in
.asc, .psdd, .gcs, .html or .pdf. The design must solve and close; nothing is written
otherwise.

  --format asc         GemCAD cutting instructions, the file the editor's Export writes
                       (CRLF line ends).
  --format psdd        A self-contained .psdd design file (the older name pleochroma is
                       accepted too).
  --format gcs         A Gem Cut Studio file (experimental: written to the published
                       Gem Cut Studio 1.1 file description and read back by this program, but
                       not yet opened in Gem Cut Studio itself; check a design whose facet
                       positions are not mirrored about index 0 before relying on it).
  --format html        The cutting instructions as a web page in the standard template,
                       headed by the design's title, designer and notes, followed by
                       shaded renderings, the masts and the carat estimate.
  --format pdf         The same cutting instructions as a PDF: the standard template on
                       US Letter pages, the views as line drawings.
  --out FILE           Where to write it.
  --out-dir DIR        With several designs: write each as DIR/<name>.<extension of the format>.
                       --format is then required.
  --db FILE            A design library, for custom materials.
  --date TEXT          The date the html or pdf sheet prints beside the designer, for
                       example 'October 2026'. Without it no date is printed, so the sheet
                       is the same every time.
";

const CENSUS: &str = "Usage: pleochroma-cli census [DESIGN...] [--templates] [--fixtures]
                             [--db FILE (--all | --ids LIST) [--limit N] [--offset M]]
                             [--targets LIST] [--modes shift[,optimize]]
                             [--crown follow|fixed|ratio|fraction=F] [--budget N] [--timings]
                             [--out FILE] [--out-dir DIR] [--json]

Measures what the meet-preserving gate does to many designs and writes one row for each
design, target material and mode: the verdict under the Retarget bounds, the raw shape changes
under bounds no stone reaches, the travel the turn is granted, and for a refused shift whether
the refusal is a branch mismatch (the ideal masts reproduce the meets, the authored constraints
pick another vertex) or a real infeasibility. It changes no design and no library: the library
is opened read-only. A design that cannot be read or captured is a row, never an error.

  DESIGN...         Design files to measure.
  --templates       The nine built-in templates.
  --fixtures        The concave fixture and the 1.72 brilliant.
  --db FILE         A design library (facet_diagrams.sqlite), opened read-only.
  --all             Every design of the library, in ascending entry id order.
  --ids LIST        Only these entry ids (comma separated).
  --limit N         At most N library designs, after --offset (for sharding a long run).
  --offset M        Skip the first M library designs.
  --targets LIST    Materials to retarget to (default diamond,sapphire,quartz,cz,tourmaline;
                    cz is Cubic Zirconia; a number is a bare refractive index). A target within
                    0.005 of a design's own index gets no row.
  --modes LIST      shift (default), optimize or both.
  --crown RULE      follow (default), fixed, ratio or fraction=F, as retarget.
  --budget N        Evaluations of the optimize mode (default 100).
  --timings         Add the timing columns; they differ from run to run, the rest does not.
  --out FILE        Write the report (CSV, or JSON with --json) to FILE; the percentile summary
                    is printed. Without it the report is printed.
  --out-dir DIR     Write the retargeted .psdd of every Valid row to DIR/<key>__<target>__<mode>.psdd
                    (DIR must exist).
  --json            JSON instead of CSV.

The CSV starts with a comment line carrying census_version, and ends with comment lines (#)
holding the percentile tables. A whole library at five targets takes hours: start with
--targets sapphire --limit 200. Exit code 0 when the census ran.
";

const RENDER: &str = "\
Usage: pleochroma-cli render JOB.job.json [--out FILE.png] [--local cpu|gpu|cpu+gpu]
                             [--remote HOST:PORT --cert-dir FOLDER]
                             [--compute local|remote|both] [--transfer full|final]
                             [--contribute-local] [--quiet] [--material NAME]
                             [--color-pattern NAME] [--color-variant KEY] [--highlights whiten|keep-color] [--meter]
                             [--stone-shadow on|off] [--tray-reflection PERCENT]
                             [--raw-out FILE.plraw] [--progress-raw FOLDER] [--sample-offset N]
@ADAPTIVE_USAGE_RENDER@
                             [--path-cache] [--persist-cache] [--cache-dir FOLDER]
@BEAM_USAGE_RENDER@
                             [--optics-tables] [--verbose]

Renders the still picture a render job file describes. The file is written by the desktop
app's render queue (File > Render Jobs...), which also exports scripts that call this command.
It renders with the app's own engine, so the picture is the one the app would have made.

  --out FILE.png          Write the picture here instead of where the job says.
  --material NAME         Render the job's stone in another built-in material (any letter
                          case), sized for the job's stone width as the app sizes it. An
                          unknown name is refused with the list of materials.
  --color-variant KEY     Render the job's material in one of its built-in color variants,
                          at the job's stone width: the full key (ruby.thai) or the part after
                          the dot (thai). An unknown key is refused with the material's keys.
  --color-pattern NAME    Render the stone in one of its material's typical color patterns
                          (Hexagonal Banding, Watermelon, ...; any letter case, spaces or
                          dashes), built on the material's own color as the Color Pattern
                          window builds it. A Watermelon or Reverse Watermelon also turns the
                          Crystal Axis through the table, as the app does. Not with
                          --color-variant.
  --highlights RULE       How colors too bright for the screen are shown: keep-color (default,
                          they keep their hue and saturation and are dimmed; only light
                          brighter than white turns white) or whiten (they keep their
                          brightness and turn paler). With a remote worker, whiten needs
                          --transfer full.
  --meter                 Read the job's exposure as a correction on top of the lighting's
                          light meter, as the app meters its live view (white paper shows the
                          same under every lighting). For a job written before the meter or by
                          hand; a job lit by an HDR map keeps its exposure.
  --stone-shadow on|off   Override Stone Shadow. Studio lights with a backdrop only.
                          Computes a Final quality map before the first sample.
  --tray-reflection PERCENT
                          Override Tray Reflection: how mirror-like the surface under the
                          stone is, as its reflectance straight on, 0 to 100 (0 is the matte
                          tray). About 4 looks like black acrylic or glass, 100 is a mirror;
                          glancing views reflect more. Studio lights with a backdrop only.
                          Written to the job as format 4 (format 5 with color zones, 6 with
                          a turned HDR map). Not for tilt-video.
  --engine path|ray     Replace the job renderer: spectral Path Traced or deterministic Ray Traced.
                          Ray uses the job AA tap count; its tint follows the absorption spectrum.
  --local ENGINES         The engines of this computer: cpu, gpu or cpu+gpu (default cpu+gpu).
                          gpu needs a build with the gpu feature; without it everything renders
                          on the processor and a note says so.
                          A gpu build keeps the graphics card's compiled shaders in the pleochroma
                          folder of your user cache folder; this only speeds up the next start.
  --radv-workaround on|off
                          Mesa's RADV driver (AMD, Linux, gpu build) mis-schedules the GPU kernel,
                          so some zoned stones (opal) and stones with oriented inclusions
                          render wrong. on sets
                          ACO_DEBUG=nosched (about 10 % slower GPU). Default: on only with an AMD
                          GPU on the amdgpu kernel driver. PLEOCHROMA_RADV_WORKAROUND does the
                          same; an ACO_DEBUG you set yourself is left alone.
  --remote HOST:PORT      Also use a remote worker. Needs --cert-dir.
  --cert-dir FOLDER       The folder with ca.pem, client.pem and client.key. Needs --remote.
  --compute WHERE         Replace the job's choice: local, remote or both. remote and both
                          need --remote.
  --transfer WHAT         Replace the job's choice: full (raw sample data) or final (the
                          finished picture only).
  --contribute-local      With --transfer final: this computer renders a share too.
  --quiet                 No progress lines (notes and errors are still printed).
@VERBOSE@
  --raw-out FILE.plraw    Also write the finished picture as a PLRAW1 file, for image-compare:
                          per pixel the mean CIE XYZ before the tone map, and the number of
                          samples behind it. The PNG is written as usual. Not with a remote
                          worker's finished picture (--transfer final).
  --progress-raw FOLDER   Also write PLRAW1 snapshots FOLDER/spp_NNNNNN.plraw (NNNNNN is the
                          samples per pixel) at every power of two from 16 up, and the final
                          image. A snapshot is taken at the first batch boundary at or after
                          the power of two; the file says how many samples it holds. Only for
                          a render on this computer (not with --compute remote or both).
  --sample-offset N       Start at sample N instead of 0 (a whole number). The same samples per
                          pixel, drawn from other sample indices: a second, independent
                          render of the same picture. Not with --engine ray, nor with
                          --transfer final.
@ADAPTIVE_FLAGS_RENDER@
  --path-cache            Shade the stone from a path cache: the paths of light inside it are
                          worked out once per pixel and reused for every sample. It is a quick
                          preview for trying lights on a kept view: facet edges are approximate,
                          so render the final picture without it. Used only together with
                          --persist-cache (a cache that is not kept never repays its build, and
                          a note says so). Uses memory (a note gives the size and where it
                          lives); a stone it cannot serve (the stone shadow, the tray
                          reflection, opal, frosted facets, a scattering body, concave tiers)
                          renders as without it, and a note says why. Overrides the job's own
                          path cache setting. Not with --engine ray. Renders this run with the
                          branching estimator, which the path cache needs; the run says so.
  --persist-cache         Keep the path cache on disk for re-renders of the same view, light
                          changes included; implies --path-cache. A second run loads it instead
                          of building it.
  --cache-dir FOLDER      Where kept path caches live. Default: the app's render_cache folder,
                          beside its settings file. Only used when a cache is kept.
@BEAM_FLAGS_RENDER@
  --optics-tables         Read the Fresnel reflection and transmission and the absorption
                          exponential from lookup tables built for the job's material (accurate
                          to about 2e-5), on the processor and on the graphics card alike,
                          instead of computing them at every interface. The picture is the same
                          within the tables' accuracy but not bit for bit. Whether it renders
                          faster depends on the machine (it is off by default). Only for the
                          path-traced renderer; a remote worker renders without the tables. Not
                          with --engine ray.

Output: standard output is the path of the picture written, one line. Standard error carries
the progress and the estimated time left (one rewritten line on a terminal, a line per 10
percent otherwise), notes, and the line 'wrote PATH'. A name that is taken is not
overwritten: the job's name gets ' (2)' and so on.

Exit codes: 0 done; 1 the command line is wrong, or the job file holds a tilt video;
2 the job file cannot be used (not a job file, newer format, invalid values, an HDR map that
is missing or changed); 3 a file could not be read or written; 5 the render failed or was
stopped.
";

const TILT_VIDEO: &str = "\
Usage: pleochroma-cli tilt-video JOB.job.json [--out-dir FOLDER] [--restart]
                                  [--local cpu|gpu|cpu+gpu] [--remote HOST:PORT --cert-dir FOLDER]
                                  [--compute local|remote|both] [--transfer full|final]
                                  [--contribute-local] [--quiet] [--material NAME]
                                  [--color-pattern NAME] [--color-variant KEY] [--highlights whiten|keep-color] [--meter]
                                  [--raw-out FOLDER] [--sample-offset N]
@ADAPTIVE_USAGE_TILT@
                                  [--path-cache] [--persist-cache] [--cache-dir FOLDER]
@BEAM_USAGE_TILT@
                                  [--optics-tables] [--verbose]

Renders the tilt performance video a render job file describes: every frame as a picture,
then the video. The file is written by the desktop app's render queue (File > Render Jobs...),
which also exports scripts that call this command. It renders with the app's own engine.

A video resumes by default. Frames the same job already wrote to its folder are kept, and
only the missing ones are rendered, so a run that stopped part way (Ctrl+C, a power cut)
continues where it stopped. Frames are written atomically. --restart deletes this job's
frames first and starts at frame one. A folder that holds another job's frames is refused.
Frames made with the branching estimator (a run with --path-cache or --persist-cache, or an
older job file) are not continued by a run with exit collection, nor the reverse: --restart
renders them again.

  --out-dir FOLDER        The frame folder, instead of the one the job names.
  --restart               Start again from the first frame.
  --material NAME         Render the job's stone in another built-in material (any letter
                          case), sized for the job's stone width as the app sizes it. An
                          unknown name is refused with the list of materials.
  --color-variant KEY     Render the job's material in one of its built-in color variants,
                          at the job's stone width: the full key (ruby.thai) or the part after
                          the dot (thai). An unknown key is refused with the material's keys.
  --color-pattern NAME    Render the stone in one of its material's typical color patterns
                          (Hexagonal Banding, Watermelon, ...; any letter case, spaces or
                          dashes), built on the material's own color as the Color Pattern
                          window builds it. A Watermelon or Reverse Watermelon also turns the
                          Crystal Axis through the table, as the app does. Not with
                          --color-variant.
  --highlights RULE       How colors too bright for the screen are shown: keep-color (default,
                          they keep their hue and saturation and are dimmed; only light
                          brighter than white turns white) or whiten (they keep their
                          brightness and turn paler). With a remote worker, whiten needs
                          --transfer full.
  --meter                 Read the job's exposure as a correction on top of the lighting's
                          light meter, as the app meters its live view. A job lit by an HDR map
                          keeps its exposure.
                          Each of these three options makes a different video: give it its own
                          --out-dir (the folder of the plain run is refused as another job's).
  --engine path|ray      Replace the job renderer: spectral Path Traced or deterministic Ray Traced.
                          Ray uses the job AA tap count; its tint follows the absorption spectrum.
  --local ENGINES         The engines of this computer: cpu, gpu or cpu+gpu (default cpu+gpu).
                          gpu needs a build with the gpu feature; without it everything renders
                          on the processor and a note says so.
                          A gpu build keeps the graphics card's compiled shaders in the pleochroma
                          folder of your user cache folder; this only speeds up the next start.
  --radv-workaround on|off
                          Mesa's RADV driver (AMD, Linux, gpu build) mis-schedules the GPU kernel,
                          so some zoned stones (opal) and stones with oriented inclusions
                          render wrong. on sets
                          ACO_DEBUG=nosched (about 10 % slower GPU). Default: on only with an AMD
                          GPU on the amdgpu kernel driver. PLEOCHROMA_RADV_WORKAROUND does the
                          same; an ACO_DEBUG you set yourself is left alone.
  --remote HOST:PORT      Also use a remote worker. Needs --cert-dir.
  --cert-dir FOLDER       The folder with ca.pem, client.pem and client.key. Needs --remote.
  --compute WHERE         Replace the job's choice: local, remote or both. remote and both
                          need --remote. remote traces nothing on this computer (it only
                          receives and saves the frames and encodes the video); if the
                          remote is missing or drops, the run stops with an error and a
                          later run continues from the frames already written.
  --transfer WHAT         Replace the job's choice: full (raw sample data) or final (the
                          finished picture only).
  --contribute-local      With --transfer final: this computer renders a share too.
  --quiet                 No progress lines (notes and errors are still printed).
@VERBOSE@
  --raw-out FOLDER        Also write every frame this run renders as a PLRAW1 file,
                          FOLDER/frame_NNNN.plraw, numbered like the PNG frames, for
                          image-compare: per pixel the mean CIE XYZ before the tone map, and
                          the number of samples behind it. A frame that is only copied from an
                          earlier pose, or was already on disk, gets none. Not with a remote
                          worker's finished pictures (--transfer final).
  --sample-offset N       Start every frame at sample N instead of 0 (a whole number): a
                          second, independent render of the same video. Not with --engine ray,
                          nor with --transfer final. (--progress-raw is for render only.)
@ADAPTIVE_FLAGS_TILT@
  --path-cache            Shade the stone of every frame from a path cache: the paths of light
                          inside it are worked out once per pixel and reused for every sample.
                          A quick preview with approximate facet edges, used only together with
                          --persist-cache. Uses memory (a note gives the size and where it
                          lives); a stone it cannot serve renders as without it, and a note says
                          why. Overrides the job's own path cache setting. Not with --engine ray.
                          Renders this run with the branching estimator, which the path cache needs;
                          the run says so.
  --persist-cache         Keep every frame's path cache on disk for re-renders; implies
                          --path-cache. Takes disk space (the Preferences limit applies).
  --cache-dir FOLDER      Where kept path caches live. Default: the app's render_cache folder,
                          beside its settings file. Only used when a cache is kept.
@BEAM_FLAGS_TILT@
  --optics-tables         Read the Fresnel reflection and transmission and the absorption
                          exponential of every frame from lookup tables built for the job's
                          material (accurate to about 2e-5), on the processor and on the
                          graphics card alike. The frames are the same within the tables'
                          accuracy but not bit for bit, so this makes a different video: give
                          it its own --out-dir. Off by default; whether it renders faster
                          depends on the machine. A remote worker renders without the tables.
                          Not with --engine ray.

Output: standard output is the path written, one line: the MP4 or GIF, or the frame folder
when no video could be made (a note says why). Standard error carries the progress and the
estimated time left (one rewritten line on a terminal, a line per finished frame otherwise),
notes, and the line 'wrote PATH'. Unless the job keeps its frames, they are deleted once the
video exists.

Exit codes: 0 done; 1 the command line is wrong, or the job file holds a still picture;
2 the job file cannot be used (not a job file, newer format, invalid values, an HDR map that
is missing or changed, a folder of another job); 3 a file could not be read or written;
5 the render failed or was stopped.
";

const IMAGE_COMPARE: &str = "\
Usage: pleochroma-cli image-compare REF.plraw RUN.plraw [--ref2 REF2.plraw]
                                    [--tonemap-from JOB.job.json] [--error-png FILE.png]
                                    [--json] [--out FILE]

Compares two renders of the same scene as a viewer would see them, for the render-acceleration
benchmark. The files are the PLRAW1 images that render and tilt-video write with --raw-out and
--progress-raw: the mean CIE XYZ of every pixel before the tone map, and the number of samples
behind it. REF is the reference, RUN the image under test; both need the same width and height.

Both images go through the render's own tone map (the ACES curve at exposure 1, the
keep-color rule for over-bright colors; the job's exposure is already part of the XYZ values)
into the color space of the written picture. The difference of two pixels is Delta E 2000
(CIEDE2000) between their display colors.

  --ref2 FILE          A second, independent reference of the same scene (rendered with
                       --sample-offset equal to the first one's samples). Adds the noise floor.
  --tonemap-from JOB   Take the color space of the pictures from this render job
                       (sRGB, Display P3 or Rec.2020). Without it the defaults of a new job
                       are used: sRGB.
  --error-png FILE     Write a false-color Delta E map: black 0, blue 1, green 2, yellow 5,
                       red 10 or more, in between linear. An 8-bit PNG.
  --json               Print JSON (keys sorted) instead of text.
  --out FILE           Write the report to FILE instead of printing it.

Report, one \"key: value\" line each (or the JSON keys of the same names):
  noise_floor_de.mean, noise_floor_de.p99
                       Mean and 99th percentile of Delta E between REF and REF2 (only with
                       --ref2).
  changed_area_pct     Percent of pixels whose Delta E between REF and RUN is above
                       max(1.0, noise_floor_de.p99); the threshold is 1.0 without --ref2.
  changed_threshold_de The threshold just used.
  worst_tile_de        The largest mean Delta E of any 16 x 16 tile (edge tiles may be smaller).
  glint_recall         The share of REF's glint pixels that are glint pixels in RUN; null (the
                       text report says no glints) when REF has none, for example when so many
                       pixels clip to white that the percentile is the brightest value. A glint
                       is a pixel brighter than the 99.9th percentile of the
                       image's display luminance and at least 4 times the median of its 5 x 5
                       neighbourhood (the borders repeat their edge pixels). RUN uses its own
                       percentile.
  samples_used         The sum of RUN's per-pixel sample counts.
  max_spp              The largest per-pixel count of RUN.
  samples_saving       1 - samples_used / (width x height x max_spp).
  mean_de, p99_de      Mean and 99th percentile of Delta E between REF and RUN.

Percentiles are nearest-rank: the value at rank ceil(p x n) of the sorted values. Everything is
deterministic: the same files give the same bytes.

Exit codes: 0 done; 1 the command line is wrong, or the images differ in size; 2 the job file
cannot be used; 3 a file could not be read (not a PLRAW1 image, or the wrong length) or written.
";

#[cfg(feature = "beam")]
const BEAM_USAGE_RENDER: &str =
    "                             [--beam] [--beam-residual-min-spp N] [--beam-batch-tiles N]
                             [--beam-region-mode end-intersection|per-node] [--beam-nodes N]
                             [--keep-beams]
";
#[cfg(not(feature = "beam"))]
const BEAM_USAGE_RENDER: &str = "";

#[cfg(feature = "beam")]
const BEAM_USAGE_TILT: &str =
    "                                  [--beam] [--beam-residual-min-spp N] [--beam-batch-tiles N]
                                  [--beam-region-mode end-intersection|per-node] [--beam-nodes N]
                                  [--keep-beams]
";
#[cfg(not(feature = "beam"))]
const BEAM_USAGE_TILT: &str = "";

#[cfg(feature = "beam")]
const VERBOSE: &str =
    "  --verbose               Also print the renderer's debug log lines to standard error (the
                          beam tracer's per-batch lines among them). The RUST_LOG variable
                          sets the filter; default: warnings, the beam tracer's per-batch
                          lines, the GPU's decline reasons, the adaptive passes and the
                          CPU+GPU split of every batch.
";
#[cfg(all(not(feature = "beam"), feature = "adaptive-sampling"))]
const VERBOSE: &str =
    "  --verbose               Also print the renderer's debug log lines to standard error. The
                          RUST_LOG variable sets the filter; default: warnings, the GPU's
                          decline reasons, the adaptive passes and the CPU+GPU split of every
                          batch.
";
#[cfg(not(feature = "adaptive-sampling"))]
const VERBOSE: &str =
    "  --verbose               Also print the renderer's debug log lines to standard error. The
                          RUST_LOG variable sets the filter; default: warnings, the GPU's
                          decline reasons and the CPU+GPU split of every batch.
";

#[cfg(feature = "beam")]
const BEAM_FLAGS_RENDER: &str =
    "  --beam                  Trace the stone's facets as beams of light: each picture region
                          that light reaches by the same chain of facets is shaded exactly, and
                          sampling only covers what the beams leave out, so a faceted stone
                          cleans up in far fewer samples. A stone it cannot serve (the stone
                          shadow, the tray reflection, rounded edges, concave, opal, frosted
                          facets, depth of field) renders as without it, and a note says why.
                          With --path-cache, beams win for the stones they serve. Overrides the
                          job's own beam setting. Runs on this computer and on a remote worker
                          (--compute remote|both), which renders whole tiles. Not with --engine
                          ray. Renders this run with the branching estimator, which beam tracing
                          needs; the run says so.
  --beam-residual-min-spp N
                          The samples per pixel every stone pixel gets at least for what the
                          beams leave out (1 to 65536, default 64). Implies --beam.
  --beam-batch-tiles N    Tiles traced and shaded together (1 to 4096, default 256); fewer
                          tiles use less memory. Implies --beam.
  --beam-region-mode M    How a region is kept across a color interval: end-intersection
                          (default) or per-node. Implies --beam.
  --beam-nodes N          Color samples across the spectrum, a multiple of 8 from 8 to 256
                          (default 48). Implies --beam.
  --keep-beams            Keep the beams on disk for re-renders of the same view (light changes
                          included): a second run loads them instead of tracing the facets
                          again, and takes its residual from the minimum samples of
                          --beam-residual-min-spp in one pass. Implies --beam. Renders on this
                          computer only; takes disk space (the Preferences limit applies) and
                          uses --cache-dir.
";
#[cfg(not(feature = "beam"))]
const BEAM_FLAGS_RENDER: &str = "";

#[cfg(feature = "beam")]
const BEAM_FLAGS_TILT: &str =
    "  --beam                  Trace the stone's facets of every frame as beams of light: each
                          picture region that light reaches by the same chain of facets is
                          shaded exactly, and sampling only covers what the beams leave out.
                          A stone it cannot serve renders as without it, and a note says why.
                          With --path-cache, beams win for the stones they serve. Overrides
                          the job's own beam setting. Runs on this computer and on a remote
                          worker (--compute remote|both), which renders whole tiles. Not with
                          --engine ray. Renders this run with the branching estimator, which beam
                          tracing needs; the run says so.
  --beam-residual-min-spp N
                          The samples per pixel every stone pixel gets at least for what the
                          beams leave out (1 to 65536, default 64). Implies --beam.
  --beam-batch-tiles N    Tiles traced and shaded together (1 to 4096, default 256); fewer
                          tiles use less memory. Implies --beam.
  --beam-region-mode M    How a region is kept across a color interval: end-intersection
                          (default) or per-node. Implies --beam.
  --beam-nodes N          Color samples across the spectrum, a multiple of 8 from 8 to 256
                          (default 48). Implies --beam.
  --keep-beams            Keep every frame's beams on disk for re-renders of the same video,
                          but only together with --persist-cache (a frame is a new view, so the
                          beams are kept per frame, like the path cache). Implies --beam.
                          Renders on this computer only; takes disk space (the Preferences
                          limit applies).
";
#[cfg(not(feature = "beam"))]
const BEAM_FLAGS_TILT: &str = "";

#[cfg(feature = "adaptive-sampling")]
const ADAPTIVE_USAGE_RENDER: &str =
    "                             [--adaptive] [--target-noise X] [--min-spp N]
                             [--noise-metric relative|absolute] [--gpu-convergence]
";
#[cfg(not(feature = "adaptive-sampling"))]
const ADAPTIVE_USAGE_RENDER: &str = "";

#[cfg(feature = "adaptive-sampling")]
const ADAPTIVE_USAGE_TILT: &str =
    "                                  [--adaptive] [--target-noise X] [--min-spp N]
                                  [--noise-metric relative|absolute] [--gpu-convergence]
";
#[cfg(not(feature = "adaptive-sampling"))]
const ADAPTIVE_USAGE_TILT: &str = "";

#[cfg(feature = "adaptive-sampling")]
const ADAPTIVE_FLAGS_RENDER: &str =
    "  --adaptive              Stop sampling the parts of the picture that have converged: the
                          samples per pixel become a cap, and each 16 x 16 pixel tile stops once
                          it is clean enough (never before --min-spp). Fewer samples for the
                          same picture. Only for the path-traced renderer on this computer (not
                          with --engine ray, nor with a remote worker). A note says how many
                          samples were saved. With --raw-out the counts of the PLRAW1 file are
                          the real ones per pixel; --progress-raw snapshots follow the passes.
  --target-noise X        The noise at which a tile stops; implies --adaptive. With the relative
                          metric a fraction below 1 (default 0.01, one percent of the tile's
                          lightness); with the absolute one a Delta E (default 1).
  --min-spp N             No tile stops before N samples per pixel; implies --adaptive. A multiple
                          of 16 (the pass size), at most the samples per pixel. Default 64 (512
                          when the run renders with branching), or the largest multiple of 16
                          below the samples per pixel if that is less.
  --noise-metric M        How the target is measured: relative|absolute; implies --adaptive.
                          Default relative.
  --gpu-convergence       With --adaptive: keep the picture's sums on the graphics card and
                          measure the tiles there, reading back one value per tile per pass
                          instead of the whole picture. Only when every pass runs on this
                          computer's GPU (not with a remote worker, --local cpu or
                          --progress-raw); otherwise a note says it was not used and the
                          standard adaptive path renders. Does not turn --adaptive on.
";
#[cfg(not(feature = "adaptive-sampling"))]
const ADAPTIVE_FLAGS_RENDER: &str = "";

#[cfg(feature = "adaptive-sampling")]
const ADAPTIVE_FLAGS_TILT: &str =
    "  --adaptive              Stop sampling the parts of each frame that have converged: the
                          samples per pixel become a cap, and each 16 x 16 pixel tile stops once
                          it is clean enough (never before --min-spp). Every frame runs it on its
                          own. Only for the path-traced renderer on this computer (not with
                          --engine ray, nor with a remote worker). A note per frame says how many
                          samples were saved. Each setting makes a different video: give it its
                          own --out-dir. With --raw-out the counts of the PLRAW1 files are the
                          real ones per pixel.
  --target-noise X        The noise at which a tile stops; implies --adaptive. With the relative
                          metric a fraction below 1 (default 0.01, one percent of the tile's
                          lightness); with the absolute one a Delta E (default 1).
  --min-spp N             No tile stops before N samples per pixel; implies --adaptive. A multiple
                          of 16 (the pass size), at most the samples per pixel. Default 64 (512
                          when the run renders with branching), or the largest multiple of 16
                          below the samples per pixel if that is less.
  --noise-metric M        How the target is measured: relative|absolute; implies --adaptive.
                          Default relative.
  --gpu-convergence       With --adaptive: keep each frame's sums on the graphics card and
                          measure the tiles there, reading back one value per tile per pass
                          instead of the whole frame. Only when every pass runs on this
                          computer's GPU (not with a remote worker or --local cpu); otherwise a
                          note says it was not used and the standard adaptive path renders the
                          frame. Does not turn --adaptive on and does not change the frame folder.
";
#[cfg(not(feature = "adaptive-sampling"))]
const ADAPTIVE_FLAGS_TILT: &str = "";

/// The parts of the `render` and `tilt-video` pages that depend on the `beam` feature: each
/// `(marker line, text)` replaces the marker's line in the page. With the feature the texts are
/// the beam flags and the entries that name them; without it the flags do not exist, so the
/// usage lines and flag entries are empty.
const BEAM_PARTS: [(&str, &str); 5] = [
    ("@BEAM_USAGE_RENDER@\n", BEAM_USAGE_RENDER),
    ("@BEAM_USAGE_TILT@\n", BEAM_USAGE_TILT),
    ("@VERBOSE@\n", VERBOSE),
    ("@BEAM_FLAGS_RENDER@\n", BEAM_FLAGS_RENDER),
    ("@BEAM_FLAGS_TILT@\n", BEAM_FLAGS_TILT),
];

/// The parts of the `render` and `tilt-video` pages that differ with the `adaptive-sampling`
/// feature: each `(marker line, text)` replaces the marker's line in the page. With the feature
/// the texts are the adaptive sampling flags and the usage lines that name them; without it the
/// flags do not exist, so every entry is empty.
const ADAPTIVE_PARTS: [(&str, &str); 4] = [
    ("@ADAPTIVE_USAGE_RENDER@\n", ADAPTIVE_USAGE_RENDER),
    ("@ADAPTIVE_USAGE_TILT@\n", ADAPTIVE_USAGE_TILT),
    ("@ADAPTIVE_FLAGS_RENDER@\n", ADAPTIVE_FLAGS_RENDER),
    ("@ADAPTIVE_FLAGS_TILT@\n", ADAPTIVE_FLAGS_TILT),
];

/// The help page for `topic`.
#[must_use]
pub fn text(topic: Topic) -> String {
    let page = match topic {
        Topic::Root => ROOT,
        Topic::Info => INFO,
        Topic::Solve => SOLVE,
        Topic::Metrics => METRICS,
        Topic::Validate => VALIDATE,
        Topic::Optimize => OPTIMIZE,
        Topic::Retarget => RETARGET,
        Topic::Sweep => SWEEP,
        Topic::SnapMeets => SNAP_MEETS,
        Topic::Export => EXPORT,
        Topic::Census => CENSUS,
        Topic::Render => RENDER,
        Topic::TiltVideo => TILT_VIDEO,
        Topic::ImageCompare => IMAGE_COMPARE,
    };
    // The beam tracing and adaptive sampling parts of the render and tilt-video pages (see
    // `BEAM_PARTS` and `ADAPTIVE_PARTS`).
    let page = BEAM_PARTS
        .into_iter()
        .chain(ADAPTIVE_PARTS)
        .fold(page.to_string(), |page, (marker, part)| {
            page.replace(marker, part)
        });
    // The UV lamps are offered only in a `physical-color` build, as in the desktop app.
    #[cfg(feature = "physical-color")]
    return page.replace(
        "illuminant-a or aset.",
        "illuminant-a, aset,\n                    uv365 or uv395.",
    );
    #[cfg(not(feature = "physical-color"))]
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY_TOPIC: [Topic; 14] = [
        Topic::Render,
        Topic::TiltVideo,
        Topic::ImageCompare,
        Topic::Root,
        Topic::Info,
        Topic::Solve,
        Topic::Metrics,
        Topic::Validate,
        Topic::Optimize,
        Topic::Retarget,
        Topic::Sweep,
        Topic::SnapMeets,
        Topic::Export,
        Topic::Census,
    ];

    #[test]
    fn every_page_is_non_empty_and_ends_in_a_line_feed() {
        for topic in EVERY_TOPIC {
            let page = text(topic);
            assert!(page.len() > 100, "{topic:?}");
            assert!(page.ends_with('\n'), "{topic:?}");
        }
    }

    #[test]
    fn every_command_page_starts_with_its_usage_line() {
        for (topic, word) in [
            (Topic::Info, "info"),
            (Topic::Solve, "solve"),
            (Topic::Metrics, "metrics"),
            (Topic::Validate, "validate"),
            (Topic::Optimize, "optimize"),
            (Topic::Retarget, "retarget"),
            (Topic::Sweep, "sweep"),
            (Topic::SnapMeets, "snap-meets"),
            (Topic::Export, "export"),
            (Topic::Census, "census"),
            (Topic::Render, "render"),
            (Topic::TiltVideo, "tilt-video"),
            (Topic::ImageCompare, "image-compare"),
        ] {
            let page = text(topic);
            assert!(
                page.starts_with(&format!("Usage: pleochroma-cli {word} ")),
                "{word}"
            );
            assert_eq!(Topic::of(word), topic);
        }
    }

    #[test]
    fn the_root_page_lists_every_command_and_exit_code() {
        let page = text(Topic::Root);
        for word in [
            "info",
            "solve",
            "metrics",
            "validate",
            "optimize",
            "retarget",
            "sweep",
            "snap-meets",
            "export",
            "census",
            "render",
            "tilt-video",
            "image-compare",
        ] {
            assert!(page.contains(&format!("  {word} ")), "{word}");
        }
        for code in ["  0  ", "  1  ", "  2  ", "  3  ", "  4  ", "  5  "] {
            assert!(page.contains(code), "{code:?}");
        }
    }

    #[test]
    fn the_root_page_explains_several_designs_and_the_command_pages_take_them() {
        let root = text(Topic::Root);
        for phrase in [
            "Several designs:",
            "==> FILE <==",
            "--out-dir DIR",
            "refused before anything runs",
            "the others still run",
        ] {
            assert!(root.contains(phrase), "the root page lacks {phrase:?}");
        }
        for (topic, word) in [
            (Topic::Info, "info"),
            (Topic::Solve, "solve"),
            (Topic::Metrics, "metrics"),
            (Topic::Validate, "validate"),
            (Topic::Optimize, "optimize"),
            (Topic::Retarget, "retarget"),
            (Topic::Sweep, "sweep"),
            (Topic::SnapMeets, "snap-meets"),
            (Topic::Export, "export"),
        ] {
            let page = text(topic);
            assert!(
                page.starts_with(&format!("Usage: pleochroma-cli {word} <design>... ")),
                "{word}"
            );
        }
    }

    /// The metrics page explains both girdle figures and names the last CSV column.
    #[test]
    fn the_metrics_page_explains_both_girdle_figures() {
        let page = text(Topic::Metrics);
        for phrase in [
            "girdle_pct_of_width",
            "girdle_line_pct_of_width",
            "full height of the girdle faces",
            "thinnest thickness at the girdle line",
            "LAST CSV column",
        ] {
            assert!(page.contains(phrase), "the metrics page lacks {phrase:?}");
        }
    }

    /// The page quotes the default limit and the sheet's words as the code has them, and
    /// says where millimeters come from.
    #[test]
    fn the_snap_meets_page_quotes_the_default_limit_and_the_sheet_words() {
        let page = text(Topic::SnapMeets);
        let default = pleochroma_cut_core::DEFAULT_SNAP_MAX_MOVE_FRACTION;
        assert!(page.contains(&format!("(default {default},")), "{page}");
        for phrase in [
            "its depth (\"Cut to depth 2.81 mm.\")",
            "Still no meet point: T (0.83 mm off)",
            "(moved 0.03 mm, now meets P1, G1)",
            "girdle diameter",
            "nothing is written",
            "4 some tier still has no meet point, or the snap cut facets away",
            "Cut away by the snap: P3 (all 10 facets) and C3 (3 of 10 facets)",
            "would break the stone (nothing is written)",
            "Snap to Meets",
        ] {
            assert!(page.contains(phrase), "the page lacks {phrase:?}");
        }
        let root = text(Topic::Root);
        assert!(
            root.contains("snap-meets: a tier still has no meet point"),
            "{root}"
        );
    }

    #[test]
    fn the_retarget_page_says_only_valid_applies_and_the_gcs_label_says_why() {
        let retarget = text(Topic::Retarget);
        assert!(
            retarget.contains("Only a Valid verdict applies"),
            "{retarget}"
        );
        assert!(retarget.contains("same verdict line"), "{retarget}");
        let export = text(Topic::Export);
        assert!(
            export.contains("not yet opened in Gem Cut Studio"),
            "{export}"
        );
    }

    #[test]
    fn the_pages_say_angles_are_positive_and_json_keeps_the_signed_ones() {
        let root = text(Topic::Root);
        assert!(root.contains("positive number"), "{root}");
        assert!(root.contains("JSON keeps the stored"), "{root}");
        assert!(root.contains("a pavilion angle is negative"), "{root}");

        let sweep = text(Topic::Sweep);
        assert!(sweep.contains("Angles are positive numbers"), "{sweep}");
        assert!(sweep.contains("--from 39 --to 43"), "{sweep}");
        assert!(sweep.contains("A sign in front is ignored"), "{sweep}");
        assert!(
            !sweep.contains("Angles are signed like the tier"),
            "the old rule is gone: {sweep}"
        );
        assert!(
            !sweep.contains("pavilion angles are negative"),
            "the old rule is gone: {sweep}"
        );

        let info = text(Topic::Info);
        assert!(info.contains("standard code"), "{info}");
        assert!(info.contains("positive number"), "{info}");
    }

    #[test]
    fn each_render_page_names_the_engine_flags() {
        let render = text(Topic::Render);
        assert!(render.contains("--stone-shadow on|off"));
        assert!(render.contains("--tray-reflection PERCENT"));
        assert!(
            render.contains("[--tray-reflection PERCENT]"),
            "the usage block lacks the flag"
        );
        assert!(
            render.contains("[--stone-shadow on|off]"),
            "the usage block lacks the flag"
        );
        // Lines wrap, so compare the page with its white space folded.
        let flat = render.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains("before the first sample"), "{flat}");
        #[cfg(all(feature = "beam", beam_tests))]
        assert_eq!(
            flat.matches("cannot serve (the stone shadow,").count(),
            2,
            "the path-cache and beam lists both name the stone shadow: {flat}"
        );
        #[cfg(not(feature = "beam"))]
        assert_eq!(
            flat.matches("cannot serve (the stone shadow,").count(),
            1,
            "the path-cache list names the stone shadow: {flat}"
        );
        let engine_flags: &[&str] = &[
            "--engine",
            "--local",
            "--remote",
            "--cert-dir",
            "--compute",
            "--transfer",
            "--contribute-local",
            "--quiet",
            "--material",
            "--color-pattern",
            "--color-variant",
            "--highlights",
            "keep-color",
            "--meter",
            #[cfg(all(feature = "beam", beam_tests))]
            "--beam",
            #[cfg(all(feature = "beam", beam_tests))]
            "--beam-residual-min-spp",
            #[cfg(all(feature = "beam", beam_tests))]
            "--beam-batch-tiles",
            #[cfg(all(feature = "beam", beam_tests))]
            "--beam-region-mode",
            #[cfg(all(feature = "beam", beam_tests))]
            "--beam-nodes",
            "--verbose",
        ];
        for (topic, own) in [
            (Topic::Render, "--out"),
            (Topic::TiltVideo, "--out-dir"),
            (Topic::TiltVideo, "--restart"),
        ] {
            let page = text(topic);
            assert!(page.contains(own), "{topic:?} page lacks {own}");
            for name in engine_flags {
                assert!(page.contains(name), "{topic:?} page lacks {name}");
            }
            assert!(page.contains("gpu feature"), "{topic:?}");
        }
    }

    /// Every marker of `BEAM_PARTS` and `ADAPTIVE_PARTS` is replaced, and without the `beam`
    /// feature no page names a beam flag.
    #[test]
    fn the_beam_parts_leave_no_marker_and_a_default_build_no_beam_flag() {
        for topic in EVERY_TOPIC {
            let page = text(topic);
            assert!(!page.contains('@'), "{topic:?}: a marker is left: {page}");
            #[cfg(not(feature = "beam"))]
            assert!(
                !page.contains("--beam") && !page.contains("--keep-beams"),
                "{topic:?}: {page}"
            );
        }
    }

    /// A build without the `adaptive-sampling` feature has no adaptive flag, so no page names one
    /// (nor the adaptive passes of `--verbose`).
    #[cfg(not(feature = "adaptive-sampling"))]
    #[test]
    fn a_build_without_adaptive_sampling_names_no_adaptive_flag() {
        for topic in EVERY_TOPIC {
            let page = text(topic);
            for word in [
                "--adaptive",
                "--target-noise",
                "--min-spp",
                "--noise-metric",
                "--gpu-convergence",
                "adaptive passes",
            ] {
                assert!(!page.contains(word), "{topic:?} names {word}: {page}");
            }
        }
    }

    /// A build with the feature names every adaptive flag on both the `render` and the
    /// `tilt-video` page, in the usage block and in the entries.
    #[cfg(feature = "adaptive-sampling")]
    #[test]
    fn a_build_with_adaptive_sampling_names_every_adaptive_flag() {
        for topic in [Topic::Render, Topic::TiltVideo] {
            let page = text(topic);
            for flag in [
                "--adaptive",
                "--target-noise",
                "--min-spp",
                "--noise-metric",
                "--gpu-convergence",
            ] {
                assert!(page.matches(flag).count() >= 2, "{topic:?} lacks {flag}");
            }
            assert!(
                page.contains("[--adaptive] [--target-noise X]"),
                "{topic:?}"
            );
        }
    }

    /// Asserts that the page of every topic names each of its flags.
    fn assert_pages_name_their_flags(flags: &[(Topic, &[&str])]) {
        for (topic, names) in flags {
            let page = text(*topic);
            for name in *names {
                assert!(page.contains(name), "{topic:?} page lacks {name}");
            }
        }
    }

    #[test]
    fn each_page_names_the_flags_its_command_takes() {
        let flags: [(Topic, &[&str]); 5] = [
            (Topic::Info, &["--json", "--out", "--out-dir", "--db"]),
            (Topic::Solve, &["--out", "--out-dir", "--json", "--db"]),
            (
                Topic::Metrics,
                &[
                    "--material",
                    "--ri",
                    "--tilt",
                    "--json",
                    "--csv",
                    "--lighting",
                    "--out",
                    "--out-dir",
                ],
            ),
            (
                Topic::Validate,
                &[
                    "--json",
                    "--material",
                    "--ri",
                    "--lighting",
                    "--out",
                    "--out-dir",
                ],
            ),
            (
                Topic::Optimize,
                &[
                    "--preset",
                    "--budget",
                    "--starts",
                    "--seed",
                    "--vary-anchored",
                    "--candidates",
                    "--out",
                    "--out-dir",
                ],
            ),
        ];
        assert_pages_name_their_flags(&flags);
    }

    #[test]
    fn the_later_pages_name_the_flags_their_commands_take() {
        let flags: [(Topic, &[&str]); 6] = [
            (
                Topic::Retarget,
                &[
                    "--material",
                    "--ri",
                    "--mode",
                    "--crown-follow",
                    "--crown-fraction",
                    "--crown-ratio",
                    "--range",
                    "--no-keep-look",
                    "--girdle-allowance",
                    "--out",
                    "--out-dir",
                ],
            ),
            (
                Topic::Sweep,
                &[
                    "--tier",
                    "--from",
                    "--to",
                    "--step",
                    "--tilt",
                    "--csv",
                    "--out-dir",
                ],
            ),
            (
                Topic::SnapMeets,
                &[
                    "--out",
                    "--out-dir",
                    "--dry-run",
                    "--max-move",
                    "--max-move-fraction",
                    "--db",
                ],
            ),
            (Topic::Export, &["--format", "--out", "--out-dir"]),
            (
                Topic::Census,
                &[
                    "--templates",
                    "--fixtures",
                    "--db",
                    "--all",
                    "--ids",
                    "--limit",
                    "--offset",
                    "--targets",
                    "--modes",
                    "--crown",
                    "--budget",
                    "--timings",
                    "--out",
                    "--out-dir",
                    "--json",
                ],
            ),
            (
                Topic::ImageCompare,
                &["--ref2", "--tonemap-from", "--error-png", "--json", "--out"],
            ),
        ];
        assert_pages_name_their_flags(&flags);
    }
}
