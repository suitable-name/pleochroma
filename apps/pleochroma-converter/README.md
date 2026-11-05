# Pleochroma library converter

An unpublished, headless binary that exports the library's recorded designs to
Pleochroma native `.psdd` files. It reads SQLite without changing it and requires
a new or empty output folder. No renderer, GPU or training backend is started.

## Build and convert

From the repository root:

```sh
cargo build --release --locked -p pleochroma-converter
./target/release/pleochroma-converter \
  --db /notebooks/facet_diagrams.sqlite \
  --output /notebooks/pleochroma-native-designs
```

On Windows, use `target\release\pleochroma-converter.exe` and Windows paths.
`--db` defaults to `DIAGRAM_DB_PATH`, then `facet_diagrams.sqlite` in the current
directory. `--output` is required. Add `--no-adopt-meets` for conversion that
leaves all imported pins intact.

## What is converted

- Every database entry is examined, including ignored entries.
- Each entry exports at most one native file, even if it has multiple attachments
  or detail records. Complete designs are preferred to drafts. Within either
  group, selection prefers PSDD, ASC, GEM, then GCS; attachment ID breaks ties.
  A failed source is reported and the next candidate is tried. A draft is saved
  only if no candidate converts to a complete design.
- Names such as `entry-42-round-brilliant.psdd` identify the database entry rather
  than the attachment. Separate entries with the same title remain separate designs.
  Titles cannot introduce directories or unsafe filenames.
- ASC imports keep their recorded angles, indices, mast depths, notes, gear and
  symmetry. GEM/GCS use the shared importers, including GCS frosting and converter
  warnings. The exact source attachment is embedded in the output.
- Existing native files retain their authored properties, custom materials,
  attachments, history and unknown fields. Unchanged native documents are copied
  exactly; documents with adopted meets are reserialized.
- Available catalogue title, designer, source and shape information is kept in
  newly converted files. Existing native metadata remains authoritative.
  If an entry has multiple detail records, metadata comes from the lowest detail
  ID; source selection considers attachments from every detail record.
- Alternative attachments are recorded as skipped in the report and remain in
  the unchanged database. Their authoring is not merged into the chosen design.

A closed ASC cut gets an inferred enclosing block with 5% clearance so a guessed
rough cannot clip the stone. This is **not a measurement of the original rough**.
No physical size, custom spectrum, linked-tier relation or concave tool is invented.
The refractive index recorded by the source is retained.

## Source-supported enrichment

By default the converter attempts to adopt explicit positive meet instructions
from the source. Blank, unknown, conditional or negated notes remain pinned;
the importer's generic meet classification alone is not evidence of intent.
A candidate must solve and preserve the original cut's
geometry and meet network within the strict conversion tolerance. The original
baseline is kept throughout; successful adoptions do not reset it. An instruction
that fails these checks stays pinned, and the report records the reason. This is
constraint adoption, not optical optimization or a redesign.

Only source-supported enrichment is performed. Equal facet angles alone do not
establish that tiers were authored as linked followers. An angle/index table with
no source file does not contain mast depths. Such entries are reported as missing
sources instead of being reconstructed with invented depths.

## Results and failures

The output folder contains:

```text
pleochroma-native-designs/
  entry-42-round-brilliant.psdd
  ...
  report.json
  drafts/                          # created only when needed
    entry-56-incomplete.psdd
```

`report.json` (schema 2) accounts for every examined entry and design attachment, with its
source IDs/name, output path, status, adopted meet count and warnings/errors.
Only the chosen attachment has an output path. `summary.sources` counts all
eligible attachments, `summary.skipped` counts unselected alternatives, and
`summary.failed` counts failed conversion/read/write attempts. Unneeded
lower-priority sources are skipped without conversion; they are not certified as
valid or claimed to be identical to the selected source.
Parse failures do not stop other conversions. Missing sources and drafts are
explicitly counted. Complete files are published without overwriting existing
files; an output failure stops the run. A partial report is attempted on output
failure, although an unavailable/full output device may also prevent that write.

Exit codes:

- `0`: one complete file per entry, no failed attempts or missing sources.
- `2`: the pass finished with failed sources, missing sources or drafts; inspect
  the report. Successful files remain usable, including a fallback chosen after
  another source failed. Skipped alternatives alone do not make a pass partial.
- `1`: invalid arguments, setup/database-listing failure, or output failure.

Use a different empty destination for another pass. The converter never deletes
or overwrites an earlier export.

## Training use

Keep the original database in the training corpus and supplement it with these
files:

```sh
# Add these options to the normal fresh-training command:
--design-dir /notebooks/pleochroma-native-designs --include-library
```

The trainer's directory input is flat, so `drafts/` and `report.json` are not
training examples. PSDD alone does not create material/feature coverage: the
trainer treats native materials as authored, whereas ASC inputs allow source
material and size sampling. Replacing the original ASC corpus would therefore
change the training distribution. Real authored dimensional targets, relations,
custom spectra and concave designs still need suitable original examples.

## Validation

Focused source tests cover one-file-per-entry selection, fallback handling,
database coverage, output isolation, exact source preservation, native round-trips
and verified adoption. They can be run with:

```sh
cargo test --locked -p pleochroma-converter
cargo clippy --locked -p pleochroma-converter --all-targets -- -D warnings
```
