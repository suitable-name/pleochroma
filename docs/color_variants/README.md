# Color variants: research, fit records and data (index)

Written 2026-10-27 with the color variants campaign. A **color variant** is a named, complete per-axis
absorption tensor for one material (Sapphire / Pink, Tourmaline / Rubellite). Picking one in a
design copies its tensor into that design; the material keeps its optics, and a pleochroic stone keeps
one set of bands per direction. Many built-in variants are fitted to measured, mostly polarized
spectra of the variety; the others are built from published band positions. Almost every variant's
strength is chosen for a typical face-up color rather than measured, so almost every entry says
`estimated = true` (see "The `estimated` flag" below) and the app shows "Estimated" with the first
source line in the tooltip. This folder holds the evidence behind them.

## What each file is

Research (R files, one per material group; read before the fit record of the same group):

| File | Scope |
|---|---|
| `R1_corundum_spinel_chrysoberyl.md` | Sapphire, Ruby, Spinel, Chrysoberyl (Yellow), Alexandrite. The GIA 2020 corundum cross sections, recipes, derived face-up colors, drops and additions. |
| `R2_beryl_topaz_tourmaline_andalusite.md` | Emerald, Aquamarine, Morganite, the beryl family, Topaz, Tourmaline, Andalusite. Section 0.2 lists where measured spectra contradict the earlier repo assumptions (corrected in `docs/chromophores/beryl_topaz_tourmaline_andalusite.md`, 2026-10-27). |
| `R3_garnet_peridot_zircon_benitoite_tanzanite.md` | Garnet species, Peridot, Zircon, Benitoite, Tanzanite and green zoisite. |
| `R4_quartz_diamond_opal_synthetics_glass.md` | Quartz family, Opal, Rutile, Cubic Zirconia, YAG, Synthetic Moissanite, Glass; its section 4 holds the diamond band models. |
| `R5_diamond.md` | Diamond: measured band positions, widths and cross sections (no downloadable numeric spectrum of a colored diamond exists). Read with R4 section 4. |

Each R file has a summary table, one section per variant (cause, axes, spectra, band table, target
color, strength anchor, confidence), the sources it used and the sources it could not read.

Fit records (`fit_records/`, one per data file; the entry's `fit_record` key points at its section by
anchor): sources (URLs, CSV names), the fit (bands, residual), the scaling and why, the target face-up
color and the reached one (L*C*h, dE2000) and the per-direction colors against the documented
pleochroism.

| File | Data file it documents |
|---|---|
| `fit_records/corundum_spinel_chrysoberyl.md` | `corundum_spinel_chrysoberyl.toml` |
| `fit_records/beryl_topaz_tourmaline_andalusite.md` | `beryl_topaz_tourmaline_andalusite.toml` |
| `fit_records/garnet_peridot_zircon_benitoite_tanzanite.md` | `garnet_peridot_zircon_benitoite_tanzanite.toml` |
| `fit_records/quartz_diamond_opal_synthetics_glass.md` | `quartz_diamond_opal_synthetics_glass.toml` |
| `fit_records/diamond.md` | `diamond.toml` |

## The data files

All under `crates/pleochroma/data/`:

* `color_variants.toml`: the schema only (documented at the top); it holds no `[[variant]]` entry.
  Loaded first.
* `color_variants/<group>.toml`: one file per material group, `[[variant]]` entries with the keys
  `material`, `key` (`<material slug>.<name>`, slug = lowercase name with every non-alphanumeric
  character replaced by `_`), `label` (Title Case), `unit`, `reference_path_mm`, `sources`,
  `fit_record`, `estimated` and the per-ray band rows with an explicit band shape (`wavelength` or
  `energy`; never inferred from the width). The files are embedded in the program and parsed once by
  `variants::parse_all`; a bad file is caught by the unit tests, and every entry is validated against
  its host at load (finite, nonnegative, at most 8 bands per ray, rank equal to the host's, no
  half-empty tensor).
* The reserved name `default` is the Material Default, which is built from the material's own tensor
  and is never in a file.

New variants use `per_mm` (an absorption per millimetre of path, so the stone's width scales the
color physically) with `reference_path_mm = 7.0`, the stone width the numbers are calibrated for.

A colorless entry is an all-empty tensor (`pleochroic = false`, no band in any direction); it is
valid on any host, since an empty tensor has no pleochroism to lose.

### The `estimated` flag

`estimated = true` when the spectral shape is not a measured spectrum of that variety, or when the
strength is not anchored to a measurement (a strength chosen for a typical face-up color counts as
estimated). `estimated = false` only for a measured spectrum of that very variety at a measured or
published strength. The first `sources` line is one short plain sentence naming the estimated part;
the app shows it as the tooltip "Estimated: <first line>". Citations, URLs and repository paths
start on the second line. Under this rule 106 of the 111 shipped entries are estimated.

## The CSV policy

`crates/pleochroma/data/reference_spectra/*.csv` are **reference data, not shipped**: they stay in the
repository as the evidence behind the fits, and no library code `include_str!`s or `include_bytes!`s
them. The variant files in `reference_spectra/` are named
`variant_<material>_<variant>_<polarization>[_<path>].csv`; their headers give the source (mostly the
Caltech Mineral Spectroscopy Server of G. R. Rossman, downloaded 2026-10-27, CC BY-NC 4.0; the GIA 2020
corundum sheet states no license), the sample, the path length and the polarization. Values are the
published ones, sorted by wavelength, with duplicates dropped; where a file was cropped, the header
says so. Files whose headers say "user-delivered", "secondary", "unverified" or "synthesised" are not
measurements and must not be used to fit a variant. Whether the license terms allow redistribution
with the app is the owner's decision.

## The face-up path rule

A swatch of a variant is the face-up color of a stone 7 mm wide, and the path through such a stone
depends on the unit of the tensor. `variants::face_up_swatch_path(unit, stone_width_mm)` gives it:

* `per_mm`: the face-up path in millimetres, `MODEL_UNIT_FACE_UP_PATH x width / REFERENCE_MODEL_WIDTH`
  (`2.83 x width / 2.0`), which is **9.905 mm at a 7 mm stone**.
* `model_unit` (the Material Default of a built-in fitted per model unit): one model unit at 7 mm,
  scaled by `width / 7`.

So one model unit at 7 mm and 9.905 mm per-mm describe the same face-up path, which is why a per-mm
variant and a per-model-unit default can be compared at 7 mm. The number 7 is the stone width, never
the path: pass 7 as the width, not as the path of the color computation. The fit records and the tests state the path they used (9.905 mm for a per-mm variant at
7 mm). A variant's `reference_path_mm` is the stone width its numbers are calibrated for, not this path;
it has this one meaning for built-in, saved and design-stored variants alike (the `.psdd`
`[material.color_variant]` table and the vault record keep the same number), and every swatch is drawn at
`face_up_swatch_path(unit, reference_path_mm)`.

The swatch itself is the oriented face-up color: `variants::face_up_swatch` takes the face-up D65 color of
the tensor over that path through the material's default principal frame (its own Crystal Axis defaults,
the frame the render uses for a stone with no axis override), so a pleochroic variant shows the ray the
table shows, not an average of the directions; a non-pleochroic tensor gives its single color. The
tooltips keep the color of each direction.

## What the app does with them

The design color list shows Material Default, the built-in variants of the material, your saved
variants, the nine generic presets (single-color stones only) and Custom. See Chapter 6 of the manual
(`apps/pleochroma-cut/docs/manual/06-materials-and-refractive-index.md`, "Color Variants") for the user
side, and `docs/chromophores/` for the chromophore catalogue the research started from.
