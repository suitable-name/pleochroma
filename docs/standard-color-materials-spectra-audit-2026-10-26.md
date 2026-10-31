# Standard color and gemstone material audit

Date: 2026-10-26. Status: **source and scientific review, including the supplied Caltech spectra; no implementation changes**.

The standard renderer has a substantial spectral and anisotropic optical framework, but its materials are a mixture of measured spectral fits, representative host properties, and qualitative appearance presets. It should not yet be described as a generally validated predictor of a particular gemstone's color. Several confirmed data-flow and numerical defects can also change the material between editing, preview, persistence, and rendering.

The highest-priority corrections are physical-size consistency, coherent refractive-index overrides, preservation of the actual edited spectrum, browser material identity, and explicit handling of unsupported effects. Material-specific work includes Peridot's misplaced principal index, Benitoite's unsupported spectral attribution, and inaccurate claims about Ametrine and Tanzanite calibration.

This report consolidates all findings from the Astra reviews of the color model, application integration, and material spectra, together with the primary review and subsequent user-supplied evidence. Overlapping findings are combined; smaller findings, limitations, supported behavior, and rejected suspicions remain included. It is the companion to the [experimental chromophore audit](C:/Users/ngussek/Dev/Rust/diagram_loader/docs/chromophore-physics-color-audit-2026-10-26.md), not a replacement for it.

## 1 Scope and evidence

The scope is the current standard color path: built-in materials, ordinary body-color presets and custom colors, spectra and dispersion, anisotropy, sizing, zoning, material effects, color-related metrics, persistence, and desktop/browser/mobile consumers. Experimental concentration and treatment authoring is excluded except where its code is actually shared by standard functionality.

Two exceptions to an assumed separation matter:

- The ordinary seven-band Custom color solver lives inside `optics/chromophore/solver.rs`. Its directory does not make that functionality experimental.
- The standard Ametrine preset resolves quartz recipes through the chromophore catalogue. Its sector assumptions therefore belong in this report.

The review used source inspection, primary-source reading, user-provided spectra and images, and small data-parsing/arithmetic checks. No builds, tests, Clippy, benchmarks, application sessions, database reads, or Git commands were run for this review. Numerical examples below are algebraic deductions or explicitly identified interpolation results, not application measurements. Only this report is added.

Finding classifications:

| Classification | Meaning |
|---|---|
| Confirmed defect | The current equations, data flow, or preserved-data contract establish a mismatch. Runtime impact is limited to the stated trigger. |
| Scientific inconsistency | The implementation or its explanation conflicts with its own mathematical contract or cited primary evidence. |
| Approximation | An intentional simplification whose accuracy has a bounded or unestablished domain. |
| Evidence gap | The available sources do not establish a particular calibration or scientific claim. This does not prove the numerical result is wrong. |

P1 means a broad or persistent material-identity/physical-consistency problem; P2 means a narrower correctness problem; P3 means a documentation, provenance, or limited-scope issue. These are review priorities, not measured incident rates.

Source links refer to this checkout. Line numbers are navigation aids. Source excerpts are current code, sometimes shortened with surrounding context omitted. External sources support the specific scientific statements next to their links; they do not establish that this application has passed validation.

## 2 How the standard color path works

The [38-entry built-in catalogue](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/mod.rs:301) supplies host dispersion, optical character, principal absorption bands, and optional material effects. Most presets use legacy model-unit absorption; Ametrine uses per-millimetre zones.

Ordinary named body colors replace the host's absorption with a three-band isotropic approximation. The Custom LCh picker instead solves seven nonnegative energy-domain bands for a desired color at a reference path. The standard call chain is [desktop color editor](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/gui/editor/render_color.rs:195) → [shared LCh model](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-editor/src/lch_color.rs:56) → [seven-band solver](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:1603).

Material selection applies overrides in this order: per-stone pattern, inherited material zoning, then plain color; otherwise nonempty band rows take precedence over the RGB fallback. Physical sizing and crystal orientation are resolved before rendering. Path-traced anisotropic transport follows individual optical modes. The faster Ray-Traced view deliberately uses a reduced model. Color metrics and swatches are separate consumers and must be checked independently of the main tracer.

The distinction between an optical direction and a crystallographic axis is essential:

- Uniaxial absorption uses ordinary polarization perpendicular to crystallographic c and extraordinary principal polarization parallel to c.
- Biaxial slots are `o_ray = alpha`, `beta_ray = beta`, `e_ray = gamma`.
- For a biaxial material, the application's `c_axis` field represents the gamma optical principal direction; it need not be the mineral's crystallographic c direction.
- A propagation direction is not the electric-field polarization direction. Looking along c does not imply E parallel to c.

## 3 Confirmed application and integration findings

### I01 Standard absorption depends on arbitrary model scale

**P1, confirmed defect, high confidence.** Shared setup uses the following expressions in [materials.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/render_setup/materials.rs:339):

```rust
// ModelUnit
width_mm / PHYSICS_DEFAULT_STONE_WIDTH_MM / MODEL_UNIT_FACE_UP_PATH
// PerMm
(width_mm / model_width) as f32
```

Only the PerMm branch divides by the actual geometric width. `needs_model_width` also excludes ModelUnit materials from width measurement. The tracer multiplies actual model-space ray length by this scale. Therefore multiplying all geometric coordinates by q, while keeping the same physical stone width, multiplies legacy optical depth by q.

The application does not supply a compensating universal normalization: [solved-plane export](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/design/export/planes.rs:45), [editor solve policy](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-editor/src/solve_policy.rs:271), and [ASC geometry](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/geometry/cuts/asc_schedule.rs:149) retain authored mast scale. The [design contract](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/design/mod.rs:116) describes that scale as dimensionless. The setup comment claiming independence from file normalization is consequently incorrect.

**Correction:** include actual model width in a documented legacy calibration. Preserve the intended reference appearance; do not relabel existing fitted optical densities as measured mm^-1 coefficients without conversion.

**Future acceptance:** equivalent solids at model scales 0.5, 1, and 2, with equal physical width, must give the same corresponding absorption optical depths. Doubling physical width should double optical depth. Cover built-in colors, RGB colors, band colors, and each client using shared setup.

### I02 Browser custom-material snapshots can be saved under the wrong name

**P1, confirmed defect, high confidence.** [Browser state](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-web/src/app/state.rs:129) returns the single retained custom snapshot for any non-built-in material:

```rust
if builtin {
    return None;
}
self.custom_material.as_ref()
```

The catalogue can hold multiple custom names, while [custom creation](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-web/src/editor/settings/custom.rs:68) overwrites that one snapshot. Creating A, creating B, then selecting or undoing back to A can serialize A's name with B's spectrum and optical properties. The live catalogue may mask the problem until reload.

**Correction:** associate complete snapshots with stable material identities and use the selected identity for save, Undo, and reload.

**Future acceptance:** two custom materials with distinct absorption, index, and dispersion must remain distinct after switching, Undo, native save/reload, and custom → built-in → custom transitions.

### I03 Browser worker transport omits material-owned inclusions and structure

**P2, confirmed defect, high confidence.** [GemMaterial](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/mod.rs:293) marks resolved inclusions with `serde(skip)`. Native load restores material-owned families and Bragg structure through [axis_curves.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/native/axis_curves.rs:37), but browser scenes serialize actual materials through [worker protocol](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-web-core/src/protocol.rs:582).

The separate browser [effects record](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-web-core/src/scene/effects.rs:123) carries design effect choices, not a complete copy of those restored material-owned families. Such a material can therefore load successfully yet lose its needles, plates, or structure when sent to a rendering worker.

**Correction:** serialize validated resolved material physics explicitly, or reconstruct it inside workers from the full custom snapshot. The desktop [MaterialInclusions payload](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-net/src/scene/material_inclusions.rs:21) demonstrates an existing transport pattern.

**Future acceptance:** compare worker inputs before/after a scene round trip for arbitrary material-owned families and structure, on CPU and GPU routes and export. This finding does not assert that all browser zoning is lost: per-stone patterns are explicitly transported, and material-owned zoning needs separate reachability evidence.

### I04 Keep discards the saved seven-band color

**P2, confirmed defect, high confidence.** The custom-material dialog promises to preserve a saved custom color through its Keep swatch. [The callback](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/gui/optics/dialog_color.rs:41) clears the full spectrum even when Keep supplies no replacement triple:

```rust
slot.bands = None;
if triple.is_some() {
    slot.triple = triple;
}
```

[Save](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/gui/optics/custom_materials/save.rs:337) can preserve bands only if they remain in dialog state. Keep → Save, or preset → Keep → Save, can replace a seven-band color with its RGB fallback.

**Correction:** retain the complete original color and restore both bands and fallback when Keep is selected. **Future acceptance:** spectrum, unit, and size response survive both sequences exactly; the existing full-band persistence path remains intact.

### I05 View-only Custom applies a different spectrum from its preview

**P2, confirmed defect, high confidence.** The solver produces seven-band swatches and reachability, but [TARGET_VIEW](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/gui/editor/render_color.rs:303) applies only the fallback triple:

```rust
TARGET_VIEW => ui
    .global::<ViewportModel>()
    .invoke_render_color_custom_solved(r, g, b),
```

[LCh solving](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-editor/src/lch_color.rs:59) derives that triple through a second legacy fit. Its error is not the seven-band error reported by the picker; the intermediate byte-sRGB conversion also clips/quantizes the target. The legacy fit assumes path 1, while application scales it with width/7. A component fitted to T=0.5 can become T=0.25 at twice that reference scale.

**Correction:** persist the full spectrum in view-only state. If legacy triples remain necessary, evaluate and report the final legacy material at the actual view size.

**Future acceptance:** design, material-dialog, and view-only applications of the same target agree on their effective spectra, or accurately disclose any approximation. Cover nonreference sizes and targets with appreciable legacy-fit residuals.

### I06 The view-only picker can read an unrelated design width

**P2, confirmed defect, high confidence.** [render_color.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/gui/editor/render_color.rs:236) uses render-context width only for dialog and pattern-zone targets. TARGET_VIEW instead reads the open editor design's girdle diameter. A library preview can therefore solve color using a different stone's size.

**Correction:** obtain TARGET_VIEW width from the active render context. **Future acceptance:** deliberately different editor/library widths must produce picker labels and solver inputs belonging to the viewed stone. This is independent of I05's spectrum replacement.

### I07 Browser and mobile color edits can remain hidden behind an existing pattern

**P2, confirmed defect, high confidence.** [Browser LCh](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-web/src/editor/settings/lch.rs:181), browser presets, and [mobile presets](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-mobile-core/src/design_props/materials.rs:313) can commit a new plain color while retaining `color_pattern_override`. [Material selection](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/material/selection.rs:193) applies the pattern first and returns. The accepted edit is then visually ineffective.

Mobile's availability check examines the named material's zoning rather than the applied per-stone pattern. Desktop's corresponding action explicitly clears the pattern, so client behavior differs.

**Correction:** use one explicit replacement policy: clear an existing pattern on an intentional plain-color replacement, or disable/explain plain editing while it remains authoritative. **Future acceptance:** visible result, stored precedence, Undo, and reload agree across clients for a patterned stone.

### I08 Zoning suppresses structural color without an accurate capability message

**P2, confirmed defect/unsupported combination, high confidence.** The [CPU branch](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/raytracer/scattering/oriented.rs:465) selects zoned scattering before considering Bragg structure. The [GPU branch](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/renderer/shaders/transport_bounce/08b_extend_scatter.wgsl:490) uses the same alternative routing.

The custom editor can configure both. The [manual](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/docs/manual/28-play-of-color-and-schiller.md:103) describes the combination as unvalidated, although structure is omitted on this path.

**Correction:** implement combined transport or explicitly reject/disable it. **Future acceptance:** adding zones either preserves structural scattering or gives an actionable unsupported-state response on both backends. Merely preserving the structure in saved state is insufficient.

### I09 Tone optimization ignores shaped zones without an applicable warning

**P2, confirmed defect/unsupported computation, high confidence.** [Tone evaluation](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/color/metrics/tone.rs:314) reads `material.absorption`, which a zoned material fills with its base tensor. It does not accumulate separate shaped-zone path lengths. Changing a shaped zone can change the render while leaving Tone unchanged.

The [Optimize UI](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/gui/editor/optimize_panel/mod.rs:346) still enables Tone and [prepares patterned materials](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/gui/editor/optimize_solve.rs:324). [Effects warnings](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/gui/render/physics_effects_ui.rs:263) cover inclusions, surface finish, and play-of-color, but not this zoning omission.

**Correction:** integrate per-zone/per-mode optical depth in the metric path, or suppress/label Tone for zones. **Future acceptance:** changing an optically important shaped zone while holding the base fixed must change Tone appropriately or mark it unavailable. Compare a zone covering the whole stone with its homogeneous equivalent. Main path-traced zoning is not implicated by this finding.

### I10 Optimize describes an obsolete unset-size rule

**P3, confirmed documentation mismatch.** [optimize_view.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-editor/src/optimize_view.rs:394) describes one model unit as one color-strength unit when size is unset. Current setup instead uses a 7 mm default and the relevant calibration/actual-width conversion.

**Correction:** derive the explanation from shared sizing semantics. **Future acceptance:** the message matches actual resolved scaling for unset and explicit width, for both RGB and band colors.

## 4 Shared optical and numerical findings

### M01 RI overrides change only one principal curve

**P1, confirmed defect, high confidence.** [selection.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/material/selection.rs:185) replaces only the base dispersion:

```rust
let a = n_d as f32;
gem.dispersion = DispersionModel::Cauchy { a, b: 0.0, c: 0.0 };
```

Independent extraordinary and biaxial alpha/gamma curves survive and are still used by [optics.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/optics.rs:24). For example, setting quartz's ordinary index to 1.80 while keeping its extraordinary curve near 1.55 creates a large negative split instead of its small positive split. Metadata can cease to describe the resulting optics.

**Correction:** define whether the override flattens or shifts dispersion, and apply that contract to all principal curves and derived metadata. **Future acceptance:** inspect all principal indices across the visible band for uniaxial and biaxial overrides, including sign, intended splitting, and CPU/GPU parity. Preserve unmodified material behavior.

### M02 Sorting principal indices can detach values from their crystal directions

**P2, confirmed conditional defect.** [optics.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/optics.rs:33) sorts the three evaluated values, then attaches them to the unchanged frame:

```rust
let mut sorted = [alpha_curve.evaluate(lambda_nm), n_beta,
                  gamma_curve.evaluate(lambda_nm)];
sorted.sort_by(f32::total_cmp);
```

Crossing custom curves or M01's override can move an index from one physical direction to another while leaving absorption axes unchanged. For authored values (1.61, 2.00, 1.63), the 2.00 value moves from beta to gamma. Reviewed built-ins are not established to cross in their visible domain; this is not a claim that every built-in is affected.

**Correction:** preserve the axis/value pairing, consistently permute all tensors if relabeling is necessary, or reject unsupported crossings. **Future acceptance:** the world-space dielectric tensor remains the authored tensor through ordering changes.

### M03 Oriented swatches average absorption before exponentiation

**P2, confirmed mathematics defect with an active consumer.** [principal_axes.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/color/principal_axes.rs:107) averages the two mode projection weights. [The swatch](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/color/principal_axes.rs:163) combines absorption coefficients using those weights, and `body_color` exponentiates the result.

For two equally incident modes, these are different:

```text
implemented: T = exp[-L(alpha_1 + alpha_2)/2]
required:    T = (exp[-L alpha_1] + exp[-L alpha_2])/2
```

Optical depths 0 and 4 give 0.1353 versus 0.5092. The error darkens unequal-mode transmission and can alter hue. [Crystal-axis preset scoring](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/crystal_axis/presets.rs:229) calls this helper under two illuminants, so the older comment saying it is unwired is stale.

**Correction:** retain each mode's weights, exponentiate its own optical depth, average transmitted intensities, then integrate color. **Future acceptance:** verify the analytic example, equal-absorption and zero-thickness limits, and oriented two-mode slabs. The main anisotropic tracer and its assigned-mode metric rays already separate modes; this defect must not be generalized to them.

### M04 Biaxial swatch weights use displacement vectors as electric-field vectors

**P2, confirmed helper-contract defect; normal UI impact is limited.** The biaxial branch in [principal_axes.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/color/principal_axes.rs:113) uses `eigen_polarizations`, whose [contract](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/birefringence/biaxial.rs:187) returns D vectors. Absorption weights require E vectors. The [existing conversion](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/birefringence/biaxial.rs:367) divides local components by the corresponding squared indices.

**Correction:** convert D to E before taking squared projections. **Future acceptance:** oblique directions agree with assigned-mode E-field calculations; principal-axis and isotropic limits remain unchanged. D and E are parallel on principal directions, so this particular error cancels for the current principal-direction preset ranking, unlike M03.

### M05 Reopening the picker reconstructs a different stored spectrum

**P2, confirmed defect for valid historical or nonbasis rows.** [lch_color.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-editor/src/lch_color.rs:161) extracts peaks by center alone:

```rust
if let Some(row) = rows.iter().find(|row| (row[0] - centre).abs() < 0.5) {
    *slot = row[2];
}
```

Widths and domains are discarded, off-basis bands are omitted, and duplicate centers are not summed. The current energy-domain basis can replace a historical wavelength-domain spectrum when opening and applying without an intentional target change. A wavelength Gaussian at 620 nm with sigma 45 nm has normalized value 0.6065 at both 575 and 665 nm; its locally equivalent energy Gaussian does not preserve those two values.

**Correction:** seed Lab from the actual decoded spectrum and retain that spectrum unless an edit requires refitting. **Future acceptance:** no-change editing preserves historical wavelength rows, arbitrary centers/widths, duplicate centers, and current basis rows, or clearly reports a defined refit.

### M06 Band domain is encoded by a noninvertible width heuristic

**P2, confirmed general codec defect; normal seven-band output is currently safe.** [Pattern conversion](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/material/pattern/convert.rs:28) and [custom materials](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/custom.rs:371) infer energy-domain bands from `width > 500`. Serialization stores only center, width, and peak.

An energy sigma of 400 cm^-1 is read back as 400 nm; a wavelength sigma above 500 nm is read as energy-domain. Current seven-band energy widths exceed the threshold, which explains why their ordinary round trips are unaffected.

**Correction:** persist explicit shape and units in a versioned representation, with a provenance-aware legacy migration. **Future acceptance:** round-trip both shapes below, at, and above the threshold, preserving evaluated spectra and existing files.

### M07 Nonzoned GPU materials truncate excess bands with only a log warning

**P2, confirmed conditional CPU/GPU mismatch.** [GPU decline checking](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/renderer/buffers/zoning.rs:306) exits for nonzoned materials. [Packing](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/renderer/buffers/material.rs:535) warns and truncates to `MAX_ABSORPTION_BANDS`, while CPU absorption sums all bands.

Imported/custom lists can exceed eight entries, changing color by backend and row ordering. The current seven-band picker is within capacity and is not the affected case.

**Correction:** decline unsupported materials, expand the representation, or use an explicit bounded-error refit. **Future acceptance:** a strong ninth absorber and row permutations either retain equivalent spectra or trigger deterministic CPU routing.

### M08 A resolved scale of one is mistaken for an unsized material

**P3, confirmed helper edge case.** [Zone swatch sizing](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/zoned.rs:109) tests `scale.to_bits() != 1.0_f32.to_bits()` to distinguish sizing state. A valid resolved scale of one can therefore fall back to physical width divided by an assumed model width of two. With physical/model widths both seven, the correct scale one becomes 3.5 for the swatch.

**Correction:** represent sizing state explicitly or pass the resolved scale. **Future acceptance:** continuity immediately below, at, and above one. The inspected current zone-editor call creates an explicitly unsized material, so a common user-facing occurrence of this edge is not established.

### M09 Invalid band centers pass the documented validation contract

**P3, confirmed input-validation gap.** [Custom bands](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/custom.rs:369) and [zoning validation](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/zoning/mod.rs:363) check finiteness and positive width but do not require a positive wavelength center. Zoning accepts nonnegative amplitude; retained custom rows require strictly positive amplitude.

**Correction:** reject nonpositive centers and invalid domain parameters before evaluation. **Future acceptance:** zero, negative, NaN, and infinite centers plus invalid widths/amplitudes are handled explicitly without changing valid picker outputs.

### M10 HDR colorimetry and calibration metadata are not applied

**P2, conditional input-contract defect and calibration limitation.** [HDR loading](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/renderer/env_map_hdr.rs:70) decodes RGBE into raw RGB floats and passes them to `EnvironmentMap::from_rgb`. The [spectral reconstruction](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/renderer/env_map_spectrum.rs:14) treats these as linear sRGB. No application step applies source primaries, exposure calibration, or color correction.

The pinned `image` 0.25.10 decoder retains exposure/color-correction metadata but its pixel reader converts RGBE directly. Its exposure documentation distinguishes stored values from calibrated radiance. Thus a file with nondefault calibration/primaries is not established to be interpreted as intended. The exact exposure policy must be explicit: restoring physical radiance and preserving a deliberately exposed artistic panorama are different contracts.

**Correction:** define accepted HDR color space/calibration, transform supported source metadata consistently, and reject or disclose unsupported variants. Preserve the same decoding policy on local and remote workers.

**Future acceptance:** small HDR inputs with known primaries and calibration exercise the agreed contract and produce identical local/remote map values. This is separate from the unavoidable limitation of reconstructing a spectrum from RGB, discussed below.

## 5 Material-specific scientific findings

### S01 Peridot puts the alpha index in the beta slot

**P2, confirmed material-property defect.** The [Peridot entry](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/peridot_through_benitoite.rs:51) sets its base Cauchy curve to approximately 1.654 at the D line. The same entry identifies representative alpha/beta/gamma values of 1.654/1.669/1.690, then supplies beta-minus-alpha 0.015 and total spread 0.036. Shared biaxial setup interprets the base curve as beta.

The resulting indices are approximately **1.639005 / 1.654005 / 1.675005**, all 0.015 below the entry's intended representative triplet. This affects refraction, Fresnel response, critical angles, and optic-axis geometry. The [existing regression target](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/tests/dispersion.rs:206) repeats 1.654 and therefore protects the mistaken convention.

The [Sardinia primary study](https://www.gia.edu/gems-gemology/summer-2009-peridot-sardinia-italy-adamo) reports distinct alpha, beta, and gamma ranges, corroborating the need to distinguish minimum and middle indices. The user's [Mogok study](https://www.gia.edu/dam/migrated-assets/docs/doc1/winter-2021-gemological-characterization-peridot-pyaung-gaung-mogok-myanmar.pdf) does not provide a replacement three-principal-index calibration for GRR418.

**Correction:** use a coherent principal-index reference and the declared beta convention. Verify all three indices, not merely the base scalar. Do not swap absorption spectra to compensate: the GRR418 absorption slot mapping appears correct.

### S02 Benitoite's spectral attribution is stronger than its cited evidence

**P2, scientific inconsistency.** The [preset](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/peridot_through_benitoite.rs:208) implements a 635 nm wavelength Gaussian with sigma 60 nm and ordinary/extraordinary peaks 0.1/2.0, attributed to Fe2+-Ti4+ intervalence transfer.

The original [Laurs, Rohtert and Gray study, printed page 184 and Figure 28](https://www.gia.edu/dam/migrated-assets/gems-and-gemology/pdf/FA97.pdf) reports a broad absorption peak around 700 nm, extending strongly into the near infrared, and does not establish the asserted chemical cause. It supports much stronger E-parallel-c absorption. Thus the direction of dichroism is supported; the center and chemical certainty are not a transcription of that measurement.

**Correction:** fit an appropriate measured polarized spectrum and distinguish appearance fitting from an established mechanism. Moving 635 to 700 nm alone, without refitting width and strength, is not a sufficient correction. A newer primary attribution could supersede the older uncertainty, but must be supplied explicitly.

### S03 The red Spinel preset lacks a measured shortwave absorption system

**P2/P3, spectral incompleteness with unmeasured rendered impact.** [Spinel](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/spinel_through_tourmaline.rs:37) contains only a 540 nm, sigma 55 nm, peak 2.4 band. Its explanation contrasts this with a two-band corundum structure.

Primary [GIA spinel spectroscopy](https://www.gia.edu/gems-gemology/fall-2021-purple-gem-spinel-vietnam-afghanistan) identifies Cr-related shortwave absorption around 390–400 nm as well as the 530–550 nm system. The one-band model omits the measured shortwave absorption system; transmission and color differences have not been quantified at matched concentration and path.

**Correction:** fit a suitably identified Cr-spinel specimen including its shortwave absorption, or label the existing entry as a qualitative red preset. Do not treat one selected specimen as universal for every spinel color and composition.

### S04 Ametrine's equal-iron natural-history claim is incorrect

**P3 scientific inconsistency, with broader calibration implications.** [ametrine.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/ametrine.rs:25) gives both sectors 28.7 ppma total iron and explains natural sector differences as centers rather than different iron amounts. The modeled citrine half uses an irradiation/anneal sequence with fixed conversion fractions.

The original [Anahi study, printed page 22](https://www.gia.edu/dam/migrated-assets/docs/doc1/the-anahi-ametrine-mine-bolivia.pdf) instead reports less iron and water in the amethyst sectors than in the citrine sectors. Its natural-sector observations do not establish the code's universal equal-iron premise or its treatment yields.

**Correction:** describe equal iron and the 10%/50% conversion choices as modeling assumptions, or replace them with sector-specific spectroscopy/chemistry. A straight bicolor boundary is a useful chosen cut configuration, not the full growth-sector geometry of every crystal. Sharing the catalogue with experimental authoring is not itself a defect.

### S05 Tanzanite is a hybrid representative tensor with unresolved absolute calibration

**P3 provenance and calibration gap; no definite multiplicative error established.** The [Tanzanite entry](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/tanzanite_through_cubic_zirconia.rs:108) fits GRR2292 data and multiplies the alpha ray by 2.119 using ratios from GRR1265 and GRR3045. It also describes approximately 0.6 wt% V2O3 as the source crystal's own composition.

The [retained alpha-file metadata](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/reference_spectra/caltech_zoisite_grr2292_blue_b_alpha_5p142mm.csv:1) states that composition is unpublished and that this ray comes from another thickness. [Collection notes](C:/Users/ngussek/Dev/Rust/diagram_loader/docs/chromophores/primary-2026-10/data/tanzanite/NOTES.md:28) record an unresolved distinction between source-index normalization to 3 mm and individual file thicknesses. The newly supplied alpha file confirms its header and data; it does not resolve that distinction.

The result can be a useful heated-like appearance model. It is not an unmodified complete measured tensor from a single fully characterized sample. Transferring a ratio from another specimen is an additional calibration assumption. A natural blue zone may resemble a heated appearance, but similarity does not establish the original sample's treatment history.

**Correction:** preserve the specimen-specific raw spectra, state every baseline and scaling operation, remove unsupported composition certainty, and resolve original path normalization before claiming absolute absorption. Keep GRR1265a and GRR2292 as distinct references. Section 8 records exactly what the supplied files add.

### S06 Tanzanite's index description overstates coherence with its reference

**P3, internal documentation inconsistency.** The [index discussion](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/tanzanite_through_cubic_zirconia.rs:56) calls 1.700858 within 1.691–1.70; it is slightly outside that interval. With the chosen spreads, the model yields approximately **1.69836 / 1.70086 / 1.71136**, rather than the Hurlbut specimen's **1.6915 / 1.6935 / 1.7020** quoted later in the file.

The code intentionally borrows only beta's fractional position in the triplet. Unlike S01, this does not prove a misplaced principal slot or an impossible material; it shows a representative combination presented too strongly as specimen-derived.

**Correction:** identify independent provenance for absolute beta, total spread, and beta fraction, or use one measured triplet. The [Hurlbut primary URL](https://msaweb.org/AmMin/AM54/AM54_702.pdf) was unavailable on the final retrieval attempt; the quoted specimen values here are also retained in the source and research notes, not newly verified from that failed request.

### S07 Yellow Chrysoberyl's n-squared offset does not preserve dispersion exactly

**P3, mathematical documentation error with a small numerical effect.** [aquamarine_through_citrine.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/aquamarine_through_citrine.rs:153) adds a constant to n^2, then says index dispersion and spreads are preserved. Taking a square root changes differences between indices. The comments themselves quote slightly different spreads, 0.007567 and 0.007581.

**Correction:** describe a common offset in n^2 and report the resulting dispersion/spread. This is a plausible representative adjustment, not an exact invariance.

### S08 Tourmaline's orientation explanation confuses geometry and polarization

**P3, documentation inconsistency.** [spinel_through_tourmaline.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/spinel_through_tourmaline.rs:190) places c in the table plane, then describes the table as perpendicular to c. It also associates looking along c with the extraordinary/dark polarization even though the preset has stronger ordinary absorption.

**Correction:** distinguish table-plane orientation, propagation direction, and polarization. The implemented `Vec3::X` orientation is not shown to be reversed; do not change it just to match the erroneous explanation.

### S09 Measured-shape fits still contain selected calibration assumptions

**Evidence classification, not nine newly proven bugs.** The following nine presets fit energy-domain Gaussians to measurement-derived input. All nine still store model-unit absorption. A successful fit to its own input is not independent validation of a specimen or finished stone.

| Preset | Data and implemented calibration | Remaining qualification |
|---|---|---|
| Sapphire | GIA Fe-Ti pair cross sections, three bands per polarization, 10 ppma cm | Representative pair density; other natural-sapphire absorbers omitted |
| Ruby | GIA Cr3+ sample 1110 cross sections, three bands per polarization, 800 ppma cm | Chosen strength; calculated reference colors are not independent measured finished-stone colors |
| Emerald | GRR3570 polarized digest, scaled to 0.3 cm | No matched Cr/V analysis; digest resolution and baseline interpretation |
| Aquamarine | GRR1488 polarized data, baselines 0.683/0.866 removed, scaled to 8.8 mm | Chemistry unknown; ordinary-ray baseline taken from visible minimum |
| Tanzanite | GRR2292 data, alpha multiplied by 2.119 | Cross-specimen adjustment, thickness ambiguity, unsupported composition claim |
| Peridot | GRR418 polarized data, scaled to 8.8 mm | Unknown sample chemistry and reflection treatment; NIR fit parameters hit bounds |
| Spessartine | GRR43, baseline 0.030 removed, scaled to 8.8 mm | Selected Fe-poor specimen; no independent spectral holdout |
| Tsavorite | GRR229, baseline 0.042 removed, half-strength | Scaling the entire mixed absorber spectrum is not selective variation of V alone |
| Demantoid | GIA2952, baseline 0.816 removed, scaled to 8.8 mm | Unknown chemistry, unseparated pedestal, shortwave saturation |

Sources in code: [corundum and emerald](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/diamond_through_emerald.rs:109), [aquamarine](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/aquamarine_through_citrine.rs:60), [spessartine](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/garnets_pyrope_through_spessartine.rs:126), [tsavorite and demantoid](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/garnets_grossular_and_andradite.rs:31).

The [GIA corundum study](https://www.gia.edu/gems-gemology/spring-2020-corundum-chromophores) combines chemistry and spectroscopy to derive cross sections, providing stronger concentration-based evidence than an unanalysed specimen spectrum. It also describes multiple contributors to natural corundum color. A preset based on one selected contributor remains a chosen material model.

**Correction:** attach calibration records to every fitted preset: exact sample, orientation, temperature, thickness, ordinate convention, baseline/reflection treatment, fit domain, residuals, and deliberate strength adjustments. Never infer missing chemical analysis from a typical locality composition.

### S10 Published dispersion validity is not represented by the material model

**P3 scientific applicability gap.** [dispersion.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/dispersion.rs:133) derives a numerical wavelength floor from model type and poles. It does not store the source experiment's validity interval.

Examples identified in the reviewed source:

- Rutile's DeVore equation is used below the approximately 430 nm lower measurement range.
- Moissanite's cited lower range is 435.8 nm; the renderer uses shorter wavelengths.
- YAG is used below its cited 400 nm boundary; Diamond reaches 780 nm beyond its cited 760 nm upper boundary.
- Excitation-related paths can use Spinel below 350 nm and Cubic Zirconia below 361 nm. This does not imply standard fluorescence is activated.
- Estimated Cauchy curves are extrapolated toward the visible edges and held flat below 380 nm.

These observations establish extrapolation, not that every extrapolated value is materially wrong. The [DeVore paper](https://opg.optica.org/josa/abstract.cfm?uri=josa-41-6-416) is the primary equation source; its [equation/range transcription](https://refractiveindex.info/?shelf=main&book=TiO2&page=Devore-o) is a useful secondary locator.

Additional points retained from the review:

1. The common 0.579 conversion from gemological B-G to F-C dispersion is a borrowed average, not a universal physical constant. Under the selected two-term Cauchy equation and stated Fraunhofer lines, the ratio is approximately 0.58464. This is small relative to some declared estimate uncertainties.
2. Failure to find an equation in a database does not establish that no primary equation exists. Comments should say no primary fit was sourced here.
3. Distant-pole replacements of polynomial terms in Alexandrite/Moissanite are approximations despite wording suggesting exactness; documented visible errors are small.
4. The cited [Wood-Nassau GGG paper](https://opg.optica.org/ao/abstract.cfm?URI=ao-29-25-3704) already includes a Sellmeier fit. The comment about waiting for one is stale. Publisher HTML tables were inconsistent in the review, so replacement coefficients must be checked against the original PDF before use.

**Correction:** retain scientific validity ranges, source conditions, and explicit extrapolation policy independently of numerical pole safeguards.

## 6 Inventory of all 38 presets

This is coverage of the inspected catalogue, not certification of each material. A published equation denotes a source-backed host equation; it does not validate the entire colored material. Except Ametrine, these entries use model-unit absorption.

| Material | Host optical model | Absorption or appearance provenance |
|---|---|---|
| Diamond | Cubic isotropic; Peter equation | Empty absorption |
| Sapphire | Trigonal uniaxial negative; Malitson-Dodge ordinary/extraordinary | GIA fitted polarized energy bands |
| Ruby | Same corundum host | GIA fitted polarized energy bands |
| Emerald | Hexagonal uniaxial negative; estimated Cauchy, -0.006 offset | GRR3570 polarized fit |
| Zircon | Tetragonal uniaxial positive; high-zircon Cauchy estimate, +0.059 | Five tuned U-related bands; isotropic absorption |
| Alexandrite | Orthorhombic biaxial positive; three Walling-derived curves | Two tuned wavelength bands per axis, guided partly by 77 K spectra |
| Topaz | Orthorhombic biaxial positive; estimated Cauchy and spreads | One tuned 620 nm blue-topaz defect band |
| Spinel | Cubic isotropic; Tropf equation | One 540 nm red-preset band; S03 |
| Quartz | Trigonal uniaxial positive; Ghosh ordinary/extraordinary | Empty absorption |
| Tourmaline | Trigonal uniaxial negative; estimated Cauchy, -0.021 | Tuned 720/430 nm bands, 3:1 ordinary/extraordinary |
| Tanzanite | Orthorhombic biaxial positive; estimated Cauchy and spreads | GRR2292 hybrid polarized fit; S05/S06 |
| Synthetic Moissanite | Hexagonal uniaxial positive; Wang 6H ordinary/extraordinary approximations | Empty absorption |
| Cubic Zirconia | Cubic isotropic; Wood-Nassau 12 mol% yttria equation | Empty absorption |
| Aquamarine | Hexagonal uniaxial negative; beryl estimate, -0.006 | GRR1488 polarized fit |
| Morganite | Same beryl host estimate | One tuned 540 nm isotropic band |
| Chrysoberyl (Yellow) | Orthorhombic biaxial positive; adjusted Walling trio | One tuned 440 nm isotropic band |
| Amethyst | Quartz Ghosh ordinary/extraordinary | One tuned 545 nm isotropic band |
| Citrine | Quartz Ghosh ordinary/extraordinary | One tuned 400 nm edge surrogate |
| Ametrine | Quartz optics inherited from Citrine | Two catalogue-derived zones, per mm |
| Pyrope Garnet | Cubic isotropic; estimated Cauchy | Tuned 505/570 nm bands |
| Almandine Garnet | Cubic isotropic; estimated Cauchy | Tuned 505/520/573 nm bands |
| Spessartine Garnet | Cubic isotropic; estimated Cauchy | GRR43 fitted spectrum |
| Color-Change Garnet (Pyrope-Spessartine) | Cubic isotropic; estimated Cauchy | Five tuned bands including 573 nm |
| Grossular Garnet (Tsavorite) | Cubic isotropic; estimated Cauchy | GRR229 fit at half strength |
| Andradite Garnet (Demantoid) | Cubic isotropic; estimated Cauchy | GIA2952 fit |
| Peridot | Orthorhombic biaxial positive; estimated Cauchy with beta-slot defect | GRR418 polarized fit |
| YAG | Cubic isotropic; Zelmon equation | Empty absorption |
| GGG | Cubic isotropic; estimated Cauchy | Empty absorption |
| Benitoite | Hexagonal uniaxial positive; separate ordinary/extraordinary Cauchy estimates | Tuned 635 nm polarized band; S02 |
| Andalusite | Orthorhombic biaxial negative; estimated Cauchy and spreads | Three qualitative directional bands |
| Opal | Amorphous, represented by Cubic/isotropic placeholder; estimated Cauchy | Empty body absorption |
| Glass (N-BK7) | Amorphous with Cubic/isotropic placeholder; Schott equation | Empty absorption |
| Glass (F2) | Same placeholder; Schott equation | Empty absorption |
| Rutile | Tetragonal uniaxial positive; DeVore ordinary/extraordinary | Tuned 350 nm edge surrogate, equal absorption in both slots |
| Precious Opal (White) | Approximate opal host | Empty body; structure configured separately |
| Precious Opal (Black) | Approximate opal host | Three broad aesthetic dark-body bands; structure separate |
| Labradorite | Triclinic biaxial positive; estimated indices, borrowed Moonstone dispersion | Empty body; structure separate |
| Moonstone | Monoclinic biaxial negative; estimated indices/dispersion | Empty body; adularescence separate |

### 6.1 Directional properties and material identities

Only Alexandrite and Yellow Chrysoberyl have separately dispersing biaxial principal curves. Other biaxial entries use wavelength-independent index spreads. Nine uniaxial presets have independent extraordinary curves: Sapphire, Ruby, Quartz, Amethyst, Citrine, Ametrine, Moissanite, Benitoite, and Rutile. Remaining uniaxials use constant index offsets.

All built-ins leave `alpha_axis=None`; setup completes a deterministic frame around gamma until an explicit second axis is supplied. That is a default orientation, not a measured crystallographic mounting. Alexandrite's stated source convention maps optical alpha to crystal c, beta to a, gamma to b. [Farrell and Newnham](https://rruff.info/doclib/am/vol50/AM50_1972.pdf) provides directional spectroscopy, including low-temperature measurements; combining those positions with tuned room-appearance strengths is a representative construction.

Morganite, Amethyst, Citrine, Zircon, and Topaz use isotropic absorption despite anisotropic hosts. That omits possible dichroism; it does not mean refraction is isotropic. Yellow Chrysoberyl's weak-pleochroism approximation has support for the cited specimen. Andalusite's bands are expressly qualitative and its beta fraction is adjusted to enforce the chosen negative sign rather than copied from one measured triplet.

Names also hide selected varieties and conditions: red Spinel, Fe-bearing Tourmaline, blue Topaz, high Zircon, heated-like Tanzanite, and 6H Moissanite. These should not imply complete coverage of every variety, defect population, polytype, or treatment of the named mineral.

### 6.2 Remaining material approximations

- Morganite's single 540 nm band and simplified Mn-site explanation do not cover the range of reported manganese states and substitution mechanisms. A [GIA specimen study](https://www.gia.edu/gems-gemology/fall-2024-gemnews-helvine-inclusions-in-morganite) reports a feature nearer 567 nm; [original treatment research](https://www.igc-gemmology.org/igc-2023/irradiation-treatment-and-gamma-spectroscopy-of-morganite/) describes additional complexity. This is a qualitative-preset limitation, not proof that one center is wrong for every specimen.
- Zircon's nominally sharp bands have sigma 6–8 nm, or FWHM approximately 14–19 nm. The model is not a high-resolution identification spectrum.
- Rutile's Gaussian shortwave absorption is an edge surrogate, not a measured semiconductor-edge model.
- Labradorite's dispersion is borrowed from Moonstone; the source labels this an estimate. Opal and glasses use a Cubic enum placeholder for isotropy, not a claim of cubic crystallinity.
- Host equations generally omit sample-specific temperature, composition, strain, and treatment variation unless explicitly encoded. A representative species curve cannot establish those conditions for a user's rough.

### 6.3 Density, haze, inclusion, surface, and fluorescence properties

All 38 constructors initialize scattering strength, anisotropy parameter, and edge-rounding radius to zero, with no material-owned inclusions. Ametrine alone supplies built-in zoning. These are clean-host defaults; they do not describe the inclusion population of every natural specimen.

[Recommended haze](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/custom.rs:179) is an appearance control. Examples include Emerald (0.6, 0.35), Ruby (0.3, 0.5), Sapphire (0.25, 0.5), Tourmaline (0.25, 0.4), and Quartz/Peridot (0.15, 0.3), expressed as strength and g. These per-model-unit settings are not measured inclusion concentrations or standardized clarity grades. Their physical-size interpretation requires a separate calibration.

Precious Opal, Labradorite, and Moonstone host entries require separate structural/inclusion choices to show play-of-color or adularescence. Needles, plates, and random families exist; a faceted convex model without a cabochon dome is not a complete model of a smooth star or cat's eye. Surface finish and machine geometry likewise influence appearance and should be held consistent in comparisons.

Standard built-ins do not automatically activate fluorescence. The desktop [emitter route](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/bridge/render_thread/context/mod.rs:645) returns none for ordinary built-in/fantasy selections; recipe emitters are a separate path. Standard Ruby is therefore not a full fluorescent-ruby simulation simply because a fluorescence subsystem exists. Zoned fluorescence is also a restricted combination: the [transport assertion](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/raytracer/transport/inner/bounce_body.rs:117) requires no zoning when fluorescence is active. Normal standard-UI crash reachability was not established; this is a capability boundary requiring preflight rejection if exposed.

The [specific-gravity table](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/material/specific_gravity.rs:38) covers only 14 of the 38 optical presets: Diamond, Sapphire, Ruby, Emerald, Zircon, Alexandrite, Topaz, Spinel, Quartz, Tourmaline, Tanzanite, Synthetic Moissanite, Cubic Zirconia, and Rutile. Unsupported names return None rather than a guessed value. This affects mass/carat/yield coverage, not the absorption equation directly. Even supported values are representative ranges, sometimes composition-dependent.

## 7 Numerical and scientific interpretation limits

These records retain findings that should not be mislabeled as universal renderer failures.

### L01 Seven-band color matching is not identification of gemstone chemistry

Three color coordinates do not uniquely determine seven absorption amplitudes. The [standard inverse fit](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:1447) selects a metamer using its basis and prior. A D65 match does not establish a measured spectrum or matching behavior under incandescent light or a narrowband LED. Ordinary recoloring deliberately installs an isotropic absorption tensor while retaining host optics; it is an artistic variation, not a chemical reconstruction of another variety.

**Required interpretation:** describe ordinary Custom as spectral color matching with a chosen basis. Use independent spectra and multiple illuminants for material-prediction claims.

### L02 Reachability is bounded-search success rather than proof of impossibility

The [solver](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:1414) bounds the optical-density search to 0.001–8, uses a 3^7 coarse search, three starts and limited local refinement, with adjacent-amplitude regularization. After fitting, optical densities below 0.002 are zeroed and color error is recomputed; returned amplitudes can therefore be zero. A returned within-threshold fit demonstrates success. Failure to find one does not prove that no acceptable seven-band solution exists.

The regularizer uses `0.01 * (od[i] - od[i + 1])`; this favors similar adjacent coefficients, not directly minimum spectral curvature. A claim of globally closest or physically smoothest requires a stronger definition and evidence.

**Future acceptance:** compare difficult targets with independently constructed reachable spectra and a stronger bounded optimizer; separately report best found error, stopping reason, and representation limits.

### L03 The claimed size-dependent reachability is not implemented

[The solution](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:1654) divides fitted optical density by the requested path to obtain amplitudes per millimetre. With no independent coefficient/concentration bounds, the same target has the same fitted optical density and color error at every positive path. [The message](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-editor/src/lch_color.rs:118) saying it is not fully reachable at this size therefore attributes the limitation to an unmodeled constraint.

**Correction:** describe basis/fit limitations accurately, or introduce justified material-coefficient bounds if genuine size-dependent reachability is intended. Verify inverse amplitude scaling and unchanged fit error across positive reference paths.

### L04 Reference swatches use path proxies and different defaults

[The picker](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-editor/src/lch_color.rs:126) uses 1.5 times physical diameter, or a 5 mm path when unset. Render setup uses a 7 mm default stone and a 2.52-model-unit reference calibration. These are proxies for selected cuts, not the actual distribution of optical paths in every design. The source's historical calibration claims were not rerun here.

Even with I01 and I06 corrected, a single slab swatch need not match the average color of a faceted stone with multiple reflections. Clearly identify swatch path, actual stone size, illumination, and whether the color is a slab prediction or a rendered observation.

### L05 Orientation-free swatches are representative averages

[body_color.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/color/body_color.rs:154) averages three principal transmissions equally for biaxial materials and uses `(2 To + Te) / 3` for uniaxial materials. These useful representative mixtures are neither a general fixed-view transmission nor necessarily the exact finite-depth average over all viewing directions. They are separate from M03's incorrect averaging order in the oriented helper.

### L06 Polarization approximations have narrower domains than full anisotropic optics

The main [absorption path](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/raytracer/absorption.rs:227) computes assigned-mode E geometry at a hero wavelength and reuses it across spectral channels. Strongly dispersive birefringence requires comparison against wavelength-specific geometry before quantifying that approximation's error.

The model assumes coincident refractive and absorbing principal frames. More general anisotropic materials can require separate tensors/frames; the review did not establish that every built-in violates this assumption.

For a refractively isotropic but genuinely dichroic custom medium, [scalar attenuation](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/raytracer/absorption.rs:183) averages two absorption forms rather than using a complete diattenuating Mueller operator. Ordinary isotropic built-ins have equal absorption slots, so that special case is not a general defect in them.

### L07 RGB environments cannot recover measured illumination spectra

[HDR spectral reconstruction](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/renderer/env_map_spectrum.rs:43) maps RGB to fixed smooth spectral bases. Even exact XYZ reconstruction of the source RGB does not recover the actual spectral power distribution. Spectrally different lights can share RGB while producing different gemstone colors and dispersion effects. Increasing HDR brightness scales the chosen spectrum rather than recovering narrow lines.

This is a documented approximation, separate from M10's metadata handling. A more sophisticated upsampler can choose a better metamer but still cannot uniquely recover an unknown measured spectrum from three channels. Quantitative color-change work needs measured or explicitly defined illumination spectra.

### L08 Colorimetric and displayed comparisons answer different questions

The audit distinguishes raw spectral/XYZ predictions, Lab relative to the illuminant white, adapted display Lab, and clipped/quantized sRGB. A visual color patch or screenshot is not interchangeable with any of the first three. Tone mapping, white balance, exposure, display gamut, and the surrounding image influence perceived appearance.

Use the official [CIE 1931 two-degree observer data](https://cie.co.at/datatable/cie-1931-colour-matching-functions-2-degree-observer) and [D65 data](https://cie.co.at/datatable/cie-standard-illuminant-d65) as independent numerical references when validating integration. Their 1 nm data can support convergence comparisons; matching the application's own reduced table is a regression check, not independent validation.

The [color-change garnet test source](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/tests/color_change_garnet.rs:78) compares D65 with a 3200 K Planckian illuminant through display-derived Lab. That is useful for a qualitative trend but is not a measurement under standard illuminant A, which is approximately 2856 K, nor a reproduction of one measured specimen.

## 8 Updated Caltech evidence supplied during this review

### 8.1 Zoisite GRR1265a

All three supplied files state the same specimen identifier, Tanzania, room temperature, and a **3.00 mm normalized path**. Each contains 1,180 numeric wavelength/absorbance pairs spanning **400.916–1103.831 nm**, after two header lines.

| Optical direction | Source polarization and color label | Supplied file |
|---|---|---|
| Alpha | E parallel b, red-violet | [alpha data](<C:/Users/ngussek/.codex/attachments/8faca024-c40d-459b-be1a-050621eb7cd9/Pasted text.txt>) |
| Beta | E parallel c, blue | [beta data](<C:/Users/ngussek/.codex/attachments/1bf7aec5-e345-497c-b73a-65b25d77fc16/Pasted text.txt>) |
| Gamma | E parallel a, yellow-green | [gamma data](<C:/Users/ngussek/.codex/attachments/d9c1aeab-c522-4871-aea1-451c7fbf8d11/Pasted text.txt>) |

The header records `V2O3=0.35; FeO=0.027`. It does not specify concentration units, analytical method, or uncertainty. Preserve those values as reported; do not silently convert them to wt%, ppm, or an atomic concentration. Preserve the specimen's **a suffix**; the older repository digest shortens it to GRR1265.

The supplied plot labels absorbance, and the photographs show the stated polarized color differences. Under conventional decadic absorbance:

```text
mu_a(lambda) in mm^-1 = ln(10) * A(lambda) / 3.00
T(lambda, L_mm)       = exp[-mu_a(lambda) * L_mm]
                     = 10^[-A(lambda) * L_mm / 3.00]
```

The 3 mm value is already normalized. Dividing again by an assumed original thickness would double-correct it. Original slab thickness and reflection/baseline correction remain unstated. A common sample identifier does not prove one physical slab was used in all orientations.

This reference is stronger than relying solely on the retained 5 nm digest, but it is **not GRR2292**, the main input to the current preset. It does not establish GRR2292's composition, path normalization, or justification for transferring an alpha/gamma ratio. Room temperature does not state heat-treatment history; the older digest's categorical unheated label exceeds these headers.

The 380–400.916 nm interval remains missing. Values near the shortwave edge vary sharply; that may reflect the absorption edge, noise, or measurement limits and should not be declared an error without instrument context. The photographs have no stated illumination, camera, exposure, or white-balance calibration, so they support qualitative directional appearance rather than numerical colorimetry.

### 8.2 Forsterite GRR418

The supplied San Carlos, Arizona, room-temperature spectra are already the input reference for the Peridot preset. Original repository files are retained as [ol418.a](C:/Users/ngussek/Dev/Rust/diagram_loader/docs/chromophores/primary-2026-10/data/peridot/ol418.a), [ol418.b](C:/Users/ngussek/Dev/Rust/diagram_loader/docs/chromophores/primary-2026-10/data/peridot/ol418.b), and [ol418.g](C:/Users/ngussek/Dev/Rust/diagram_loader/docs/chromophores/primary-2026-10/data/peridot/ol418.g). Header metadata and six checked sample values match the user-provided text. This was a spot-check, not a full equivalence comparison of the pasted data.

Each direction has its own thickness. The graph's raw absorbance heights must not be treated as intrinsic absorption coefficients without that correction.

| Direction | Header thickness | Interpolated coefficient at 560 nm | At 700 nm | At 1050 nm |
|---|---:|---:|---:|---:|
| Alpha | 1.141 mm | 0.023051 mm^-1 | 0.145777 mm^-1 | 1.684801 mm^-1 |
| Beta | 1.1381 mm | 0.096543 mm^-1 | 0.252921 mm^-1 | 1.620840 mm^-1 |
| Gamma | 0.660 mm | 0.044081 mm^-1 | 0.160548 mm^-1 | 5.400072 mm^-1 |

These are calculations from linear interpolation of the retained irregular-grid measurements followed by `ln(10) * A / thickness_mm`. They are not executed renderer results, fit residuals, or claimed reflection-corrected bulk coefficients. The supplied plot explicitly labels absorbance, improving the ordinate evidence beyond the bare headers. The exact base-of-log convention is taken as conventional decadic absorbance.

The gamma section is thinner, so its thickness-normalized apparent NIR attenuation is much stronger relative to alpha/beta than the raw graph heights alone suggest. The visible interval is covered by all three datasets; unlike GRR1265a, there is no missing 380–401 nm interval.

No absorption-axis swap was identified: alpha maps to `o_ray`, beta to `beta_ray`, gamma to `e_ray`. This does not cure the independent refractive-index slot error S01.

The fitted 1000 nm Gaussian peak parameters 35.2/35.2/12.7 are not measured NIR peak heights. The fit covers 380–780 nm; [its comments](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/peridot_through_benitoite.rs:76) say NIR parameters reached bounds and only their visible tails constrained them. Their ordering alone is not proof of an amplitude or axis bug.

GRR418's analyzed composition/Fa content, reflection correction, and independently measured color remain unavailable in the retained metadata. The user's [2021 Mogok paper, Figure 13](https://www.gia.edu/dam/migrated-assets/docs/doc1/winter-2021-gemological-characterization-peridot-pyaung-gaung-mogok-myanmar.pdf) provides nonpolarized spectroscopy of other samples, useful context but not independent three-axis validation or chemical analysis of GRR418.

### 8.3 Zoisite GRR2292 alpha file and ten omitted rows

The [newly supplied file](<C:/Users/ngussek/.codex/attachments/a3dcd73a-0dca-4461-8e75-3b51aa601035/Pasted text.txt>) has these headers:

```text
Zoisite, grr2292, Arusha,
5.142 mm, E\\b =short UV-Vis yellow
```

The repository's [blue-labelled alpha CSV](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/reference_spectra/caltech_zoisite_grr2292_blue_b_alpha_5p142mm.csv:1) already records the same yellow-labelled header and says the same file was linked for both blue and yellow zones. This resolves availability of the actual alpha-file text, not proof that it is an independently measured blue-only alpha spectrum. The original linked-page context should still be checked before claiming distinct zone measurements.

**D01 — Confirmed reference-data omission, P3.** A numeric comparison of the entire supplied file with the retained CSV established:

- Supplied raw file: 1,373 numeric rows, range 360.16–1124.22 nm.
- Retained CSV: 1,363 numeric rows.
- All 1,363 shared wavelength/absorbance pairs match exactly after numeric parsing.
- The supplied raw file contains no duplicate wavelengths.
- Ten valid rows are absent from the CSV.

| Omitted wavelength in nm | Absorbance |
|---:|---:|
| 367.00 | 0.5146 |
| 378.00 | 0.3849 |
| 654.00 | 0.1390 |
| 738.00 | 0.0663 |
| 788.00 | 0.0540 |
| 860.00 | 0.0359 |
| 961.00 | 0.0254 |
| 1040.00 | 0.0189 |
| 1078.00 | 0.0181 |
| 1091.00 | 0.0182 |

Every omitted wavelength is written with a trailing decimal point in the supplied text, such as `367., .5146`. **The user's clarification is correct: `367.` means `367.00`, and is a valid numeric token.** The comparison parsed it that way. None of these rows should be rejected or treated as missing measurements.

The common notation strongly suggests a parser/import filter issue, but the exact historical import script has not been located, so that cause remains an inference. The CSV note about dropping duplicate wavelengths does not explain these omissions. Their actual effect on the fitted preset has not been measured and should not be exaggerated.

**Correction:** preserve the original text, accept valid trailing-decimal tokens, restore the ten rows in a separately authorized data correction, and document the conversion. Future import checks should compare row counts, duplicates, ranges, and all numeric pairs with the raw file.

The file does not state composition, treatment, reflection correction, or whether its numerical values were already normalized to the source index's 3 mm convention despite the 5.142 mm header. It therefore does not by itself settle S05's absolute calibration. In particular, do not automatically divide by 5.142 mm and also apply an assumed 3 mm normalization.

### 8.4 Evidence status after the new attachments

| Earlier uncertainty | Updated status |
|---|---|
| Can the numerical GRR1265 reference be inspected? | Yes: three user-supplied raw files, explicitly GRR1265a |
| Is GRR1265a's axis/path metadata known? | Yes: three explicit polarization labels and 3.00 mm normalized paths |
| Is GRR1265a proved unheated? | No: room temperature does not establish treatment history |
| Does GRR418 have usable polarized data? | Yes: all three supplied; matching retained reference already used by the fit |
| Is GRR418 newly independent validation? | No: it is the preset's input dataset |
| Is the Peridot ordinate merely guessed from bare headers? | The supplied graph now explicitly identifies absorbance; reflection correction remains unspecified |
| Is the GRR2292 alpha file available? | Yes: supplied raw text; retained CSV matches shared values but omits ten rows |
| Is the yellow header inconsistent with the retained blue-labelled filename? | The retained metadata already records this situation; source linkage is shared according to those notes |
| Is the GRR2292 normalization ambiguity resolved? | No |
| Are specimen chemistry and measured finished-stone color now established for all references? | No |

## 9 Supported behavior and rejected suspicions

These conclusions prevent the report from overstating the defects.

1. **Absorption mathematics:** independent Gaussian absorbers add optical depth. Overlapping bands therefore compound attenuation correctly. The wavelength Gaussian, energy Gaussian, and conversion `FWHM = 2 sqrt(2 ln 2) sigma` are algebraically correct in [absorption.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/absorption.rs:94). The field name `width_nm` is misleading for energy bands but the evaluator intentionally uses cm^-1; M06 is the serialization ambiguity.
2. **Color optimization equations:** the seven-band forward model evaluates exponential transmission of summed optical densities. [The CIEDE2000 residual construction](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/color/body_color.rs:479) correctly completes its cross term. These are positive source findings, not proof of global optimality or validation of every material.
3. **Main anisotropic transport:** assigned-mode absorption uses E, including biaxial D-to-E conversion. [Anisotropic metric rays](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/color/metrics/ray_trace.rs:444) also retain mode-specific weights. M03/M04 are helper findings, not a blanket indictment of the path tracer.
4. **Zoned main transport:** [zoned.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/raytracer/zoned.rs:280) converts lengths consistently and sums per-zone optical depths. [Soft-zone kernels](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/zoning/kernels.rs:859) blend coefficient contributions, not sRGB colors. I09 concerns the separate Tone path.
5. **Refraction conventions:** the uniaxial extraordinary-index endpoints and undirected optic-axis handling are consistent in [uniaxial.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/birefringence/uniaxial.rs:38). Dispersion evaluation converts nm to micrometres before the equations. No systematic nm/micrometre or squared-pole transcription mistake was found.
6. **Native persistence exists:** [native conversion](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/native/convert.rs:143) writes and restores bands, triple, scale, and pattern. Desktop color edits use undoable material edits, and full-band custom-material saving exists. The claim that all bands are lost on save is false; I04 is a specific destructive UI route.
7. **Preview material is intentionally separate:** [Preview As](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/gui/render/material.rs:199) clears stale view tint and can return to the design material. Keeping a custom color across a named-material change can be deliberate [desktop combo policy](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/gui/editor/body_color_combo.rs:28), not an unconditional leak of previous state.
8. **Patterns have explicit precedence:** per-stone patterns are transported in browser scenes and rebuilt from authored geometry at current size. It is incorrect to say all browser zoning disappears. Fixed geometric patterns are not required to turn with the crystal; following patterns have explicit [desktop](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/gui/color_pattern/follow.rs:39) and browser routes.
9. **Remote material effects have a richer route:** desktop remote rendering explicitly carries material-owned inclusion families/structure. I03 is the browser worker boundary, not every remote path. Browser design effects can explicitly force CPU through [gpu_slot.rs](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-web-core/src/host/gpu_slot.rs:191); that disclosed routing is not silent material loss.
10. **Orientation guards exist:** isotropic hosts and non-biaxial second-axis choices have deliberate guards. Uniaxial second-axis settings can orient silk without changing uniaxial refractive optics. Optical orientation and pattern geometry are separate controls.
11. **Rendering modes disclose limitations:** [Ray-Traced UI](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/ui/components/parameters_panel/render_output_tab.slint:140) explains averaged tint and omitted polarization, double refraction, haze, zoning, edge rounding, and beyond-cap paths. Those disclosed approximations differ from silent I08/I09 behavior. Solid rendering is not a spectral-color reference.
12. **Other effects are partly disclosed:** [inclusion documentation](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/docs/manual/27-inclusions-and-silk.md:104) explains metric omissions and aesthetic density controls. Surface/machine parameters travel through dedicated records; different desktop machine preferences versus browser/mobile defaults can produce different scratch directions without data loss.
13. **Capacity distinctions matter:** current seven-band output fits within eight GPU bands and its ordinary width/domain round trip works. M06/M07 concern broader supported historical/custom input.
14. **No unproven universal claims:** the absence of fields from one auxiliary browser custom-material structure alone does not prove immediate loss of every native material property. No active ordinary-UI crash from zoned fluorescence was demonstrated. No measured final-color error was inferred solely from a photograph or from a different sample's spectrum.

## 10 Existing validation and what remains missing

The inspected tests provide useful regression protection, but their existence is not evidence that they were run in this review.

| Existing source | What it checks | What it does not establish |
|---|---|---|
| [Built-in hue checks](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/tests/built_in_hues.rs:1) | Hue classes, chroma/lightness bounds, selected ray ordering and path trends | Measured-spectrum residuals or independent specimen colors; they evaluate the preset's own bands |
| [Dispersion checks](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/tests/dispersion.rs:7) | Equation/transcription targets, finite values, selected nD and dispersion | All targets being independent measurements; validity outside source ranges |
| [Biaxial checks](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/tests/biaxial.rs:47) | Ordered indices, base equals beta, total spread, sign relationships | A matched measured triplet; a coherently shifted Peridot triplet can pass |
| [Color-change garnet](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/tests/color_change_garnet.rs:14) | Chosen transmission windows and redward trend under two illuminants | Magnitude for a measured specimen, standard-A reproduction, or full rendering accuracy |
| Reference CSVs and research notes | Traceable input examples | Automatic application validation merely by being present in the repository |

No reviewed standard-material check combines an independently characterized specimen's chemistry, principal spectra, orientation, geometry, physical size, illumination, and measured final color. Terms such as measured swatch or published target should distinguish actual measurement, a computation from measured spectra, and a chosen regression constant.

Future validation should be layered:

1. **Data integrity:** retain raw text, exact identifiers and headers; validate every parsed row and transformation. Include `367.`-style tokens and leading-decimal absorbances.
2. **Units and equations:** compare Beer-Lambert attenuation, band shapes, principal-index construction, physical-size invariance, and mode averaging with independent analytic cases.
3. **Spectral fit quality:** evaluate per-axis residuals against retained measurements over the actual fit interval and separately outside it. Report absolute and relative error, treatment of saturation and baseline, and the largest deviations. Do not use a fit's own input as independent validation.
4. **Colorimetry:** converge wavelength sampling against independent CIE data; distinguish raw XYZ/Lab from adapted/clipped display values. Compare multiple thicknesses, illuminants, and polarizations.
5. **Integration:** exercise preview/apply/reopen/Keep/save/reload/Undo across desktop, browser, and mobile, retaining complete spectral and material identity.
6. **Backend consistency:** compare CPU/GPU encoded material inputs and spectra, including capacity boundaries, zones, and supported combined effects. Explicit routing is an acceptable outcome for unsupported combinations.
7. **Independent physical observation:** use controlled, documented samples and lighting for quantitative claims about finished gems. Photographic resemblance and appealing hue are useful product checks but cannot replace this layer.

## 11 Recommended work order

This is a report and proposed correction sequence, not authorization or a record of implementation.

| Order | Work | Main records |
|---|---|---|
| 1 | Fix material identity and physical consistency: model-scale invariance, coherent RI overrides, axis/value pairing, browser snapshot association | I01, I02, M01, M02, S01 |
| 2 | Preserve the actual edited spectrum through Keep, view-only application, picker seeding, and serialization | I04-I06, M05, M06 |
| 3 | Make unsupported backend/effect states explicit; correct browser worker payloads and plain-color/pattern replacement policy | I03, I07-I09, M07 |
| 4 | Correct oriented swatch mathematics and edge-case input contracts | M03, M04, M08, M09 |
| 5 | Define HDR input calibration and correct misleading size/reachability explanations | M10, I10, L02-L04 |
| 6 | Correct scientific claims and establish source-specific material calibrations | S02-S10, D01, sections 6 and 8 |
| 7 | Add the layered independent validation above, preserving documented approximations as explicit choices | Section 10 |

Do not refit material strengths before correcting geometry-scale semantics: a compensating refit could conceal I01 and then become wrong after it is fixed. Likewise, do not change Peridot absorption axes to compensate for the index-slot error, or treat removing a difficult spectral feature as a scientific correction.

## 12 Source access and provenance ledger

The corrected primary links supplied by the user are recorded here to replace earlier failed or stale paths. A tool-access failure is not proof that a server is offline or a paper/data file does not exist.

| Source | Access/evidence in this review | Appropriate use |
|---|---|---|
| [Caltech visible index](http://minerals.gps.caltech.edu/Files/Visible/Index.html) | Online access failed in this environment; HTTP was normalized to HTTPS by one tool and a direct attempt had a name-resolution failure | Source locator; do not report a confirmed 404 or successful live retrieval |
| [Caltech zoisite index](http://minerals.gps.caltech.edu/Files/Visible/zoisite/Index.html) | Same access limitation; subsequent user attachments provide specific raw data | GRR1265a and GRR2292 evidence is now inspectable through supplied files, not a fresh verification of every linked spectrum |
| [Caltech olivine index](http://minerals.gps.caltech.edu/Files/Visible/olivine/Index.html) | Same access limitation; GRR418 files retained locally and supplied by user | Polarized reference data with per-ray thicknesses |
| [Mogok Peridot 2021](https://www.gia.edu/dam/migrated-assets/docs/doc1/winter-2021-gemological-characterization-peridot-pyaung-gaung-mogok-myanmar.pdf) | Corrected PDF opened successfully | Different-specimen context; Figure 13 is nonpolarized, not three-axis GRR418 validation |
| [Anahi Ametrine 1994](https://www.gia.edu/dam/migrated-assets/docs/doc1/the-anahi-ametrine-mine-bolivia.pdf) | Corrected PDF opened successfully | Natural-sector comparison underlying S04 |
| [Benitoite 1997](https://www.gia.edu/dam/migrated-assets/gems-and-gemology/pdf/FA97.pdf) | Corrected PDF opened successfully | Printed p.184/Figure 28 underlying S02 |
| [Hurlbut Zoisite 1969](https://msaweb.org/AmMin/AM54/AM54_702.pdf) | Final request unavailable; values retained in existing research notes/source | Label the distinction between retained transcription and current direct access |
| [CIE observer](https://cie.co.at/datatable/cie-1931-colour-matching-functions-2-degree-observer) and [D65](https://cie.co.at/datatable/cie-standard-illuminant-d65) | Official dataset pages opened successfully | Independent colorimetry references |
| [PBRT Transmittance](https://www.pbr-book.org/4ed/Volume_Scattering/Transmittance) | Substantial search-result text retrieved in the model review; not treated as a successfully opened full-page source | Background on Beer-Lambert attenuation and averaging; the analytic examples in M03 are independently calculable |

Additional primary studies are linked with their specific findings in sections 5 and 6. Where a coefficient's original PDF was not checked, its source citation is not a claim of independent verification. Historical research notes may retain statements from an earlier collection date; the explicit updates in section 8 supersede those statements for the supplied files.

The attachment links above are local evidence paths, not repository-contained raw archives. The report records the metadata, comparison results, and omitted rows so its conclusions remain understandable without those paths. No raw data file, source coefficient, or old research note was changed as part of writing this report.

## 13 Coverage crosswalk

The following crosswalk makes the consolidation auditable. It includes every finding category returned by the three review lanes, including overlap and qualified observations.

| Review lane and observation | Report location |
|---|---|
| Integration: geometry/model-scale absorption | I01; shared-model overlap consolidated |
| Integration: browser custom snapshot identity | I02 |
| Integration: browser material-owned inclusion/structure transport | I03 |
| Integration: Keep loses bands | I04 |
| Integration and model: seven-band preview becomes legacy triple | I05 |
| Integration: view-only width from editor | I06 |
| Integration: web/mobile color and pattern precedence | I07 |
| Integration: zoned branch suppresses Bragg structure | I08 |
| Integration and model: Tone ignores shaped zones and lacks warning | I09 |
| Integration: stale unset-size explanation | I10 |
| Model: RI override and principal-curve identity | M01, M02 |
| Model: mode averaging and D-versus-E helper weights | M03, M04 |
| Model: stored-band picker reconstruction | M05 |
| Model: shape/width codec | M06 |
| Model: nonzoned GPU band capacity | M07 |
| Model: scale-one sentinel and invalid centers | M08, M09 |
| Primary review: HDR metadata and spectral reconstruction | M10, L07 |
| Model: identifiability, bounded search, regularization, size reachability | L01-L03 |
| Model: path proxies and orientation-free swatches | L04, L05 |
| Model: hero-wavelength geometry, coincident frames, isotropic-host dichroism | L06 |
| Model/integration: recoloring is intentionally isotropic; standard/experimental boundary | Sections 2, 6.3, L01 |
| Model/integration: zoned fluorescence restriction and no proved normal-UI crash | Section 6.3; section 9 |
| Spectra: Peridot, Benitoite, Spinel, Ametrine | S01-S04 |
| Spectra: Tanzanite calibration and index descriptions | S05, S06 |
| Spectra: Yellow Chrysoberyl and Tourmaline explanations | S07, S08 |
| Spectra: nine measurement-derived fits, baselines and sample strength assumptions | S09 |
| Spectra: validity intervals, conversion ratio, absent-fit claims, distant poles and GGG | S10 |
| Spectra: all 38 entries, polarization/default-frame coverage, chosen varieties | Sections 6, 6.1 |
| Spectra: Morganite, Zircon widths, Rutile edge, feldspar estimates and sample variation | Section 6.2 |
| Spectra/integration: haze, structure, surface, machine, fluorescence and density coverage | Section 6.3; section 9 |
| Supplied evidence: GRR1265a and GRR418 | Sections 8.1, 8.2 |
| Supplied evidence: GRR2292 alpha/yellow header and ten valid omitted rows | Section 8.3, D01 |
| Primary sources: corrected URLs and failed-access interpretation | Sections 8.4, 12 |
| All lanes: existing regression checks versus independent validation | Section 10 |
| All lanes: positive findings, documented approximations and dismissed allegations | Section 9 and individual qualifications |

The standard path is suitable as a substantial visualization and design-analysis framework with explicit approximations. Quantitative claims about a particular stone's spectrum, orientation-dependent color, chemistry, or treatment require the consistency corrections and independent evidence described above.
