# Chromophore physics color audit

Date: 2026-10-26. Status: **source and scientific review; no implementation changes**.

The experimental color feature has a credible spectral calculation framework, but it is not yet a generally validated predictor of gemstone color from chemical composition or treatment. Confirmed solver and persistence defects coexist with material models of very different scientific quality. Selected corundum data have primary measurement support; several other hosts combine measured spectral shapes with estimated strengths, fixed chemical populations, or appearance presets.

This report consolidates the primary review and all three Astra review lanes: catalogue science, numerical modeling, and application/rendering integration. It includes the smaller findings, limitations, corrected suspicions, and coverage gaps rather than only the highest-priority issues.

The audit was read-only. No application, benchmark, build, test, Clippy, database query, or Git command was run. Only this documentation is added by the follow-up task. Numerical examples below are algebraic deductions from source, not executed reproductions. Proposed verification is future work.

## 1. Scope and interpretation

The reviewed path is the experimental desktop color UI, recipe/catalogue data, forward absorption resolution, inverse color fitting, photographic color-model selection, fluorescence, persistence, and the consumers of those outputs on desktop, remote workers, mobile, and web. This is not a general audit of all renderer physics, AI training, or shader algorithms.

The desktop feature is named **physical-color**. The cut-core feature is named **physics-color**. The desktop gate intentionally controls authoring UI; previously saved physical materials still load and render without enabling that UI. The empty cut-core feature did not establish a separate gate in the inspected material/native paths. This distinction is intentional support behavior, not a missing-gate defect.

All 24 host sections were inspected: andalusite, benitoite, beryl, chrysoberyl, corundum, cubic zirconia, diamond, andradite garnet, grossular garnet, pyralspite garnet, GGG, silicate glass, opal, peridot, quartz, rutile, spinel, synthetic moissanite, tanzanite, topaz, tourmaline, YAG, zircon, and feldspar. Inspection does not mean every coefficient has been independently established experimentally.

Findings use three distinctions:

- **Confirmed implementation defect:** follows from the present control flow, data handling, or equations. Runtime impact may still depend on the stated input.
- **Scientific inconsistency:** the implemented mechanism, spectrum, or calibration contradicts its own data contract or the cited primary evidence.
- **Approximation or evidence gap:** potentially useful behavior whose quantitative validity is not established. It must not be promoted to a confirmed universal error.

Source links point to the reviewed checkout at C:/Users/ngussek/Dev/Rust/diagram_loader. Line numbers are navigation aids and can move. Code blocks labeled Current source are excerpts, not proposed implementations. External papers support the associated scientific statements, not execution of this application.

### 1.1 Finding register

| Group | IDs | Content |
|---|---|---|
| Implementation | I01-I12 | Locks, reachability, treatment modeling/search, input normalization, reported errors, statistical model selection, persistence, web support, UV swatches, UI confidence, worker validation |
| Catalogue and scientific evidence | S01-S20 | Material-specific inconsistencies, calibration assumptions, treatment presets, provenance, missing independent validation |
| Numerical and physical limitations | N01-N06 | Directional swatches, identifiability/search limits, spectral compression, line shapes, sampling, fluorescence approximation |
| Robustness | R01-R05 | Catalogue validation, emitter truncation, malformed persisted recipes, geometric unit conversion |
| Coverage and retained behavior | Sections 6-9 | Supported paths, rejected allegations, host coverage, recommended work order, future acceptance criteria, review crosswalk |

These 43 records consolidate overlapping observations. For example, S16 describes the treatment assumption itself, while I03 describes a separate disagreement between two implementations of that assumption.

## 2. Confirmed implementation findings

### I01 Locked concentrations can change during recipe construction

**Priority: high. Confidence: high. Scope: inverse solver, including the desktop caller.**

The internal model reserves locked concentrations and fits free constituents into the remaining capacity of a summed bound. Candidate storage retains the original free values instead of the evaluated capped values. Final construction then calls the general recipe clamp, which can proportionally scale locked and free entries together.

Current source excerpts:

~~~rust
Candidate {
    ids: model.items.iter().map(|i| i.id.clone()).collect(),
    amounts: c.to_vec(),
    // ...
}
~~~

~~~rust
for (id, v) in &self.locked {
    recipe.set_amount(id, *v);
}
for (id, &v) in ids.iter().zip(c) {
    if v > 1e-9 {
        recipe.set_amount(id, v);
    }
}
recipe.clamp_amounts(self.cat);
~~~

For andalusite with V + Cr <= 0.5, lock V at 0.3 and evaluate Cr at 0.4. The internal model uses V=0.3, Cr=0.2. Final proportional clamping can instead return V=0.214286, Cr=0.285714. Both drive the same summed spectrum, so exact color rescoring does not necessarily detect the altered lock.

**Correction:** persist the evaluated concentrations; use a projection that never modifies locks. Reject inconsistent locked inputs explicitly. Check the composition contract after final resolution.

**Future acceptance:** test returned recipes, not only internal capped vectors, for a summed limit with locked and free constituents. Existing summed-cap coverage stops at the internal vector, while the existing lock test uses a host without this summed-cap case.

Evidence: [candidate storage](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:719), [free capacity calculation](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:950), [recipe construction](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:1056), [joint clamping](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/catalogue.rs:594), [application caller](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/material/color.rs:111).

### I02 Reachability can be false despite an evaluated acceptable candidate

**Priority: high. Confidence: high.**

The solver ranks candidates by squared color error plus a constituent-count penalty. It only retains the winner, then applies the reachability threshold to that winner.

Current source:

~~~rust
cost: metric * metric + KAPPA * counted as f64,
~~~

~~~rust
reachable: metric <= ACCEPT_DE,
~~~

KAPPA and ACCEPT_DE are both 1. A one-constituent result with delta E 1.1 costs 2.21; a two-constituent result with delta E 0.5 costs 2.25. The acceptable result loses. If final rescoring preserves those errors, the returned result says unreachable despite an acceptable evaluated alternative.

**Correction:** retain the best feasible solution independently. Prefer feasibility before sparsity, or separate the simplest explanation from the closest achievable match in the result contract.

**Future acceptance:** exercise candidate selection with this cost ordering, then verify the final recipe against the actual forward model. The general search limitations in N02 are additional to this contract defect.

Evidence: [selection](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:618), [cost](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:735), [result](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:1165).

### I03 Treatment centers violate the inverse model linearity assumption

**Priority: high for treatment-enabled fitting. Confidence: high.**

The inverse model probes each present element at concentration 1 and multiplies the resulting non-intrinsic, non-pair spectrum by that element's concentration. The forward treatment resolver supports constant centers without precursors and centers whose creation saturates. These are not homogeneous linear concentration laws.

Current source:

~~~rust
probe.insert((*el).to_string(), 1.0);
let treated = apply_treatments(host, &stub, &probe, &mut warnings);
// ...
let k = effective_concentration(
    host, chromo, &probe, &treated.split, &treated.created,
);
~~~

Concrete shipped cases:

| Case | Forward behavior | Inverse approximation problem |
|---|---|---|
| Zircon reducing_anneal_900 | Constant blue-center intensity 1; ordinary peak 0.8 per mm | At U=100 ppm the attached unit spectrum predicts 80 per mm |
| Topaz irradiation_250 | Blue-center intensity 1 with no required element | The constant spectrum is attached to the Cr probe |
| Diamond NV creation | min(0.01 times N, 20) | Linear extrapolation gives 35 at N=3500 instead of the cap of 20 |

Final rescoring only refines the selected candidate locally for five iterations. It can report the selected recipe's actual color, but cannot recover discarded treatment/subset alternatives or repair their earlier ranking.

**Exposure:** the inspected desktop wrapper currently constructs NaturalOnly requests. This defect concerns public TreatmentPolicy::Allow functionality and treatment-enabled acceptance helpers, not every ordinary desktop color adjustment.

**Correction:** represent constant centers in the base spectrum and evaluate capped creation laws dynamically. Share these laws with the forward resolver rather than deriving them from a unit probe.

**Future acceptance:** compare the inverse evaluation with the unbudgeted forward resolver for every allowed treatment at zero, low, ordinary, and saturated concentrations. Include precursor-free recipes.

Evidence: [inverse model](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:824), [created-center resolution](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/resolve.rs:418), [zircon treatment](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:4624), [desktop policy](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/material/color.rs:119).

### I04 Treatment-only solutions are not searched

**Priority: medium. Confidence: high. Scope: treatment-enabled solver requests.**

The size-zero baseline always builds an untreated model:

~~~rust
let mut model = self.build_model(&[], &[]);
~~~

Treatment combinations are considered only for subsets with one or more free constituents. If all selectable entries are locked, the search cannot change color through an allowed treatment alone. A precursor-free treatment also cannot return its exact empty-entry recipe through this search.

**Correction:** enumerate allowed treatments for the empty free subset before searching added constituents.

**Future acceptance:** use a fully locked Fe-bearing quartz recipe with an irradiated target, and a zero-Cr blue-topaz treatment target. The fitted recipe must not invent an unnecessary constituent.

Evidence: [baseline](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:713), [treatment combinations](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:749).

### I05 Duplicate imported entries bypass bounds and disagree with inspection

**Priority: medium. Confidence: high. Scope: imported or manually constructed recipe payloads.**

Each entry is clamped before entries with the same identifier are added:

~~~rust
let used = e.amount.clamp(0.0, max);
// ...
*amounts.entry(e.id.clone()).or_insert(0.0) += used;
~~~

Two Cr entries of 2 wt% each can therefore resolve as 4 wt% although each passes the 2 wt% limit. Single-element channels are not repaired by the later multi-element summed-cap pass. ColorRecipe::amount returns only the first matching entry, so inspection and resolution can disagree.

ColorMode::from_json directly deserializes the entries, and native reconstruction accepts that result. Normal set_amount authoring avoids duplicates. Stored absorption does not immediately re-resolve on import, but subsequent forward resolution and live fluorescence use these entries.

**Correction:** reject duplicate identifiers or canonicalize by aggregating first and clamping once. Use the canonical representation for editing, display, persistence and both optical resolvers.

**Future acceptance:** inspect native import, re-resolution, fluorescence and reported amounts with duplicate identifiers. A clear rejection or a single consistently bounded entry is required.

Evidence: [normalization](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/resolve.rs:248), [recipe access](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/recipe.rs:121), [JSON ingestion](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/material/color.rs:289), [native ingestion](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/native/convert.rs:365).

### I06 A zero-weight second target reports a perfect match

**Priority: lower. Confidence: high.**

When a second target exists but its optimization weight is zero, metrics returns Some(0.0) for its delta E. The actual second-illuminant mismatch may be large.

~~~rust
(de, de_a.or_else(|| self.target_a.map(|_| 0.0)), combined)
~~~

**Correction:** calculate reported errors independently from weighted optimization residuals. A disabled objective may be reported as unevaluated or with its actual error, not as a fabricated zero.

**Future acceptance:** provide a deliberately mismatched second target at zero weight and check the reported unweighted error.

Evidence: [metrics](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:326).

### I07 Photographic model selection overstates statistical significance

**Priority: high for scientific interpretation. Confidence: high.**

The rough-color fitting pipeline compares the chromophore model with a regularized smooth spectral basis. It uses a 99 percent chi-square threshold with degrees of freedom derived from the difference in effective parameter counts:

~~~rust
let statistic = (candidates[a].chi2 - candidates[b].chi2) / scale2;
let dof = (candidates[b].effective_params - candidates[a].effective_params).max(1.0);
let critical = chi_square_quantile(Z_99, dof);
~~~

These models have not been established as nested models satisfying the regularity assumptions of that likelihood-ratio calibration. A positive fourteen-band smooth basis is not an exact parameter-restricted parent of arbitrary catalogue spectra and nonlinear chemical laws. Substituting effective degrees of freedom does not establish the required distribution.

AIC is calculated but does not control this selection. Leave-one-view-out validation runs after selection, on the selected model only. The UI consequently presents a statistical significance conclusion that this procedure has not calibrated. Non-nested likelihood-ratio comparisons do not generally have the ordinary chi-square limiting distribution; see [Findley and Wei's primary paper](https://www.census.gov/library/working-papers/1989/adrm/rr89-08.html).

**Correction:** compare held-out predictive performance for both fitted models, or calibrate a model-comparison statistic through an appropriate simulation/bootstrap procedure. Until calibrated, describe the existing decision as a heuristic preference rather than a 1 percent significance test. Account for regularization and correlated image residuals in any replacement inference.

**Future acceptance:** establish decision calibration under known synthetic generating models and independent views; retain an explicit insufficient-evidence outcome. Do not infer chemical correctness merely because a chromophore model is selected.

Evidence: [selection](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/rough_plan/colour_fit/solve/select.rs:68), [smooth model](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/rough_plan/colour_fit/solve/models.rs:312), [selection before cross-validation](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/rough_plan/colour_fit/solve/fitter.rs:651).

### I08 Catalogue changes alter fluorescence while absorption stays frozen

**Priority: high for reproducibility. Confidence: high.**

Saved absorption intentionally renders from resolved_bands. Saved fluorescence instead resolves from the current catalogue.

~~~rust
(true, Some(recipe)) => resolve_fluorescence(catalogue, recipe),
~~~

~~~rust
(ActiveColor::Physics, Some(recipe)) => recipe.resolved_bands.to_tensor(),
~~~

An untouched historical design can change glow strength, emission, or excitation after an update while retaining its old body absorption. A removed host can eliminate fluorescence. Excitation may no longer agree with the material's actual frozen extinction. The feature-off preservation policy has the same reproducibility limit.

**Correction:** snapshot resolved emitters and their version with absorption, or explicitly migrate both together. A user choosing to update the material should see that both effects change.

**Future acceptance:** load the same native recipe against a deliberately modified catalogue and verify stable rendering data until an explicit migration. Cover desktop restore and mobile/custom scene reconstruction.

Evidence: [fluorescence](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/material/color.rs:199), [absorption and version contract](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/material/color.rs:248), [desktop restore](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/gui/editor/native_io/open_commit/restore_material.rs:54).

### I09 Browser workers omit recipe fluorescence

**Priority: medium. Confidence: high.**

Actual browser CPU workers use ordinary scene builders without the custom recipes needed to populate fluorescence. The GPU browser path hardcodes fluorescent=false and uses the ordinary builder too. A fluorescent recipe can keep its absorption but lose its glow.

Custom-aware builder tests are not evidence that the actual browser worker uses that builder. Mobile has separate custom-aware plumbing and is not covered by this defect.

**Correction:** carry resolved fluorescence in browser scene messages and use the appropriate builder/admission rules. If unsupported, explicitly expose the missing effect instead of silently presenting a complete result.

**Future acceptance:** follow one persisted fluorescent recipe through desktop, mobile and actual web worker messages. Check resulting scene emitters and backend admission, not only an isolated builder.

Evidence: [CPU worker](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-web-core/src/worker.rs:453), [empty/default versus custom-aware builders](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-web-core/src/scene/mod.rs:666), [GPU worker](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-web-gpu/src/lib.rs:210), [admission caveat](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-web-core/src/gpu_admission.rs:87).

### I10 UV glow color treats photon density as radiant energy

**Priority: medium. Confidence: high.**

The analytic UV swatch multiplies photon emission density directly by color-matching functions:

~~~rust
let f = e.emission_density(l);
flux += f * Vec3::from(cie_1931_cmf(l));
~~~

CIE color-matching functions refer to equal radiant power. A photon spectrum requires the wavelength-dependent photon-energy conversion. Omitting it overweights longer-wavelength photons in broad or mixed emission spectra. Full-brightness normalization does not remove that chromaticity error; a single narrow line is mostly unaffected chromatically. [CIE definition](https://cie.co.at/eilv/236).

**Correction:** keep photon yield as its own quantity and integrate radiant energy for XYZ. For emission energy normalized to a monochromatic excitation lamp, include the lamp-wavelength/emission-wavelength ratio. Actual transport already includes the corresponding factor.

**Future acceptance:** compare analytic and transport spectral conventions for a broad emitter, two separated emitters, and a narrow single line. Separate normalized hue checks from absolute energy checks.

Evidence: [analytic integration](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/fluorescence.rs:203), [transport energy factor](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/fluorescence.rs:565).

### I11 Fluorescence UI conflates modeled output with measured behavior

**Priority: medium. Confidence: high for the reporting contract.**

The UI reports Fluorescence: none when no visible glow is calculated. A missing implemented emitter can lead to the same result as an experimentally nonfluorescent material. Weak/strong thresholds refer to modeled photon yield, not calibrated visual brightness or gemological grading.

Fluorescence-specific uncertainty is not carried adequately into the readout. The host banner examines chromophore-level confidence, while yield/sideband/quenching confidence is separate. Emerald includes explicitly estimated fluorescence parameters.

A current built-in example with known positive emission omitted solely because a yield is missing was not established. The confirmed problem is that the reporting contract cannot distinguish that situation from known inert behavior.

**Correction:** distinguish data unavailable, no modeled visible emission, and established nonfluorescence. State lamp wavelength, reference path, model assumptions and fluorescence-specific provenance alongside any strength estimate.

**Future acceptance:** exercise missing-data, zero-yield, estimated-yield and calibrated-data states. They must not share a misleading definitive observation.

Evidence: [UI readout](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/gui/optics/physics_state.rs:1419), [strength thresholds](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/fluorescence.rs:51), [UI labels](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/ui/components/material_editor/chromophore_picker.slint:588), [estimated emerald data](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:380).

### I12 Worker validation omits fluorescence material compatibility

**Priority: medium. Confidence: high. Scope: externally supplied scenes.**

Worker validation checks emitter fields individually. Requirements for compatible absorption units and an unzoned medium are enforced only by debug assertions inside transport. A structurally valid submitted scene can reach a debug panic or unsupported release calculation. Normal UI-created recipes avoid these combinations.

A zoned medium cannot use one homogeneous global excitation clock without a suitable spatial model. Incorrect absorption-unit pairing can also invalidate distance/rate conversion.

**Correction:** validate the full material/emitter combination at scene and worker boundaries. Reject unsupported combinations with an explicit reason rather than relying on debug assertions.

**Future acceptance:** compare debug and release admission behavior for valid per-mm scenes, incompatible units, and zoned fluorescence. Unsupported scenes must be rejected before transport.

Evidence: [worker validation](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-worker/src/validate/mod.rs:145), [transport requirements](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/raytracer/transport/inner/bounce_body.rs:109).

## 3. Catalogue science and calibration

These findings distinguish wrong or contradictory modeling from useful but uncalibrated appearance approximations. A plausible rendered color does not validate a constituent concentration, species fraction, or treatment mechanism.

### S01 Blue diamond uses the wrong boron continuum

**Scientific inconsistency; high confidence.**

The boron channel declares a 700 nm edge. Generic conversion moves every edge toward higher energy and turns it into a Gaussian. With the default 3000 inverse-cm width, this produces a peak near 633.5 nm and declining absorption toward the far red. The resulting approximate coefficients are 0.60 at 550 nm and 0.23 at 780 nm, in inverse cm per ppm.

Current data:

~~~toml
centre_nm = 700.0
shape = "edge"
peak_coeff = 1.76
~~~

Current conversion:

~~~rust
let onset_cm1 = 1e7 / self.centre_nm;
self.centre_nm = 1e7 / 0.5f64.mul_add(fwhm, onset_cm1);
self.peak_coeff = self.peak_coeff.map(|p| 2.0 * p);
~~~

Measured type-IIb absorption instead extends from the infrared and decreases from red toward blue. The current representation introduces an artificial red transmission window; changing its strength alone cannot correct that shape. [King et al., natural type-IIb diamonds](https://www.gia.edu/doc/Characterizing-Natural-Color-Type-IIB-Blue-Diamonds.pdf).

**Correction and evidence needed:** use a measured red/infrared continuum or a suitable continuum representation. Validate the visible slope and transmission over several path lengths independently of color fitting.

Evidence: [boron band](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:1833), [edge conversion](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/catalogue.rs:1164).

### S02 Corundum iron valence is tuned to hue rather than donor balance

**Scientific inconsistency; high confidence.**

The default split assigns 85 percent of Fe to Fe2+ and 15 percent to Fe3+. The source comment explicitly chooses this split to meet blue-hue requirements, and it also applies to Fe-only recipes.

~~~toml
valence_split = { Fe = { "Fe2+" = 0.85, "Fe3+" = 0.15 }, Ti = { "Ti3+" = 0.0, "Ti4+" = 1.0 } }
~~~

The primary study describes Fe alone as trivalent and Fe2+ production through donor compensation. A fixed hue-selected fraction is therefore an empirical specimen assumption, not a prediction from total Fe analysis. The catalogue comment expects the Si donor contribution to be folded into the Mg input, although the input remains named Mg; Si/Ni/H effects are not represented as a complete measured composition. [Dubinsky et al.](https://www.gia.edu/gems-gemology/spring-2020-corundum-chromophores).

**Correction:** distinguish measured elemental amounts from species populations and compensators. Implement a supported donor/site model or expose the empirical population state explicitly. Do not alter species fractions merely to offset a coefficient problem.

Evidence: [default split and input convention](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:832), [compensator definition](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:1007).

### S03 Corundum Fe3+ strength conflicts with the measured reference

**Scientific inconsistency; high confidence.**

The 450 nm linear peak is 0.047502 inverse cm per all-atom ppma, plus a positive quadratic contribution. The primary study's maximum cross-section of approximately 2.5 times 10^-20 square cm, multiplied by the catalogue's 1.175369 times 10^17 ions per cubic cm per ppma, gives approximately 0.00294 inverse cm per ppma. The current linear term alone is about 16 times larger. [Measured Fe3+ cross-sections](https://www.gia.edu/gems-gemology/spring-2020-corundum-chromophores).

This is a comparison of absorption coefficients per active Fe3+ concentration, not a claim that final pixels are 16 times too dark. The band source retains an older spectrum-plus-CIELAB fit; the small default Fe3+ fraction partially masks the discrepancy.

**Correction:** refit against composition-resolved Fe spectra with explicit concentration dependence and thermal-state limits. Resolve this together with S02 rather than calibrating one error against the other.

Evidence: [linear and quadratic band data](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:1083).

### S04 The advertised Mn Ti tourmaline channel is absent

**Implementation/data-contract inconsistency; high confidence.**

The channel is named Fe2+-Ti4+ / Mn2+-Ti4+ IVCT, but its elements and partners contain only Fe/Ti. Generic pair resolution returns zero without both partner elements. An Fe-free Mn+Ti recipe cannot use the advertised yellow channel; Mn instead activates the Mn3+ pink band because there is no corresponding Mn species split.

**Correction:** separate the mechanisms and their populations if supported by evidence, or remove the unsupported Mn-Ti claim. Verify a Mn+Ti-only recipe independently from an Fe+Ti recipe.

Evidence: [channel and partners](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:4362), [pair requirement](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/resolve.rs:502), [Mn channel](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:4291).

### S05 Benitoite requires trace Ti despite Ti being part of the host

**Implementation/data-contract inconsistency; high confidence.**

The host formula is BaTiSi3O9 and the channel description calls Ti essential. Nevertheless, its proposed Fe-Ti mechanism uses the generic two-trace-input pairing rule and requires a Ti recipe entry. Fe cannot interact with the Ti already present in the host lattice.

**Correction:** represent essential lattice occupancy separately from trace concentrations. This audit establishes the inconsistent implementation; it does not independently establish that the proposed Fe-Ti mechanism is the correct complete explanation of benitoite color.

Evidence: [host and pairing data](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:151).

### S06 Beryl manganese conflates distinct species and varieties

**Scientific/data inconsistency; high confidence.**

A fixed Mn2+:Mn3+ split of 1:1 makes the red-beryl and morganite spectra activate together for every Mn input. The morganite entry is evaluated as Mn2+ while its band source says Mn3+. Measured blue/yellow beryls can contain substantial Mn without the modeled red/pink contribution, so total Mn does not establish that universal split. [Hu and Lu](https://origin.prod.gia.edu/gems-gemology/spring-2020-color-characteristics-blue-to-yellow-beryl).

**Correction:** resolve the contradictory attribution and expose supported site/valence or specimen states. Verify the states independently rather than blending varieties through a universal total-Mn recipe.

Evidence: [split](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:252), [red beryl](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:506), [morganite](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:532).

### S07 Tanzanite treatment is not supported by its mechanistic citation

**Scientific inconsistency; high confidence.**

The treatment comment invokes Ti3+ to Ti4+ oxidation. The cited experiment instead reports increasing V oxidation and decreasing Ti oxidation, and changes around both 450-460 nm and 520 nm. The implementation removes only the gamma 441.69 nm component. It does not establish the measured treatment response across axes. The primary abstract and figure captions support the oxidation-state findings; the full paper contains the directional spectra. [Pluthametwisute et al.](https://pubmed.ncbi.nlm.nih.gov/32824379/), [full paper](https://pmc.ncbi.nlm.nih.gov/articles/PMC7465713/).

**Correction:** correct the explanation and fit paired before/after polarized measurements. Until then, call this an unvalidated appearance approximation. The current axis mapping itself was not found reversed; S07 concerns treatment physics and spectral coverage.

Evidence: [treatment](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:3931).

### S08 Measured shapes frequently retain estimated concentration scales

**Calibration limitation; confirmed from provenance.**

Emerald, alexandrite, peridot and amethyst use adopted spectra lacking the same-sample composition needed for an independent absolute concentration calibration. Their amplitudes retain earlier anchors. Tanzanite applies a factor of 3.54 to preserve an earlier anchor despite ambiguous header composition.

These inputs support selected spectral shapes, not a verified mapping from ppm or oxide wt% to absorption strength. A visually improved spectrum can still produce incorrect inferred concentrations.

**Correction:** separately label shape provenance and amplitude provenance. Require same-specimen thickness, polarization, composition and absorption calibration before presenting concentrations as predictive. Keep shape-only fits available with appropriate confidence.

Evidence: [emerald](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:281), [alexandrite](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:627), [peridot](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:3169), [amethyst](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:3440), [tanzanite](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:3965), [test explicitly excluding scale](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/primary_data_tests.rs:973).

### S09 Alexandrite alpha is not measured in the adopted dataset

**Coverage limitation; confirmed from provenance.**

The alpha spectrum remains the report-table shape. Beta and gamma use the adopted Caltech GRR874 spectra without a Cr analysis. A verified axis mapping is not a verified three-axis spectrum or concentration response.

**Correction:** retain separate confidence for each axis and for absolute scale. Acquire an independently characterized three-axis sample before claiming a validated directional color-change prediction.

Evidence: [mapping label and alpha data](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:595).

### S10 Diamond nitrogen populations are a preset rather than defect chemistry

**Scientific limitation with an incorrect species equivalence; high confidence.**

The host assigns 80 percent isolated N and 20 percent A_aggregate, then routes the aggregate portion to N3/N2 visible absorption. A/B aggregates and color-active N3 are distinct populations; A/B aggregation itself does not produce those visible bands. This prevents total-N input from representing aggregate-rich colorless material correctly. [GIA diamond study](https://www.gia.edu/gems-gemology/fall-2020-natural-color-d-to-z-diamonds-crystal-clear-perspective).

Irradiation also creates fixed H3/H4 and NV fractions without consuming or repartitioning the starting nitrogen/vacancy populations.

**Correction:** distinguish defects, aggregation state, charge state and their inventories. Alternatively expose a named empirical state preset and avoid presenting the total-N mapping as general chemistry.

Evidence: [default and treatment populations](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:1646), [Cape channel](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:1715).

### S11 Diamond zero phonon lines remain broad symmetric envelopes

**Spectral approximation with known deficiencies.**

N3, H3/H4 and NV0 retain broad symmetric absorption components centered at zero-phonon-line positions. Narrow lines and their asymmetric vibronic envelopes are different spectral features. The earlier plan required envelope maxima, and the research already identifies the resulting misplaced long-wave absorption. Adding another H3 sideband does not remove the broad symmetric component.

**Correction:** fit a narrow line and a supported asymmetric envelope separately. Validate the absorption red edge, not only one integrated swatch.

Evidence: [N3](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:1729), [H3 and H4](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:1782), [NV0](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:1889), [plan requirement](C:/Users/ngussek/Dev/Rust/diagram_loader/docs/chromophore-colour-plan.md:127), [existing research caveat](C:/Users/ngussek/Dev/Rust/diagram_loader/docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md:1746).

### S12 Boron concentration is not corrected for compensation

**Scientific limitation; high confidence.**

The diamond model uses B concentration without donor compensation and substitutes broad exclusions against several nitrogen centers. The primary type-IIb study distinguishes total B from uncompensated acceptors. Chemical coexistence is not equivalent to an impossible recipe. [King et al.](https://www.gia.edu/doc/Characterizing-Natural-Color-Type-IIB-Blue-Diamonds.pdf).

**Correction:** model electrically active uncompensated boron separately from total elemental concentration and independently tracked optical defects. Validate compensated and uncompensated samples; do not use exclusions as a quantitative compensation model.

Evidence: [boron concentration and exclusions](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:1821).

### S13 Quartz treatments are unconditional conversion presets

**Model limitation; confirmed from data.**

Irradiation converts fixed fractions of total Fe and Al into centers. The 450-degree treatment creates a fixed Fe-derived citrine fraction; the 550-degree treatment converts the modeled Fe3+ population to Fe2+ prasiolite. The comments acknowledge estimated conversion and source dependence, but site availability, charge compensation, dose, time and suitable-source conditions are absent from the operative state.

**Correction:** label these as assumed treatment states. Predictive treatment functionality would require supported precursor populations and paired measurements across the relevant conditions. A temperature label alone does not establish a kinetic or origin-sensitive model.

Evidence: [quartz treatments](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:3372).

### S14 Aquamarine spectra and population laws are insufficiently constrained

**Scientific calibration/model limitation.**

The model combines a 680 nm band and an 835 nm single-ion-labeled feature under a single random Fe2+ times Fe3+ population law, with a fixed redox split. Hu and Lu's measured beryl samples show an extraordinary feature near 600 nm and an ordinary feature near 820 nm whose strength is not simply proportional to total Fe. They do not establish the current universal pair scaling. [Hu and Lu](https://origin.prod.gia.edu/gems-gemology/spring-2020-color-characteristics-blue-to-yellow-beryl).

The ordinary/extraordinary direction is consistent in the current catalogue; the older reversed-dichroism allegation is not a current confirmed defect.

**Correction:** separate features by supported mechanism and site population, and fit both polarization spectra with same-sample chemistry.

Evidence: [aquamarine channels](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:433).

### S15 Generic pair coordination and host aliases exceed their calibration

**Documented approximation with consequential limits.**

Several hosts use a generic coordination number of six in a random pair law. The dimensional conversion is reasonable for the stated occupancy assumptions, but it does not establish site ordering, pair equilibria or clustering for each mineral.

Tourmaline aliases elbaite varieties, dravite and schorl to one host composition, density and population split, while the Fe bound explicitly excludes schorl-level Fe. An alias should not imply a quantitatively calibrated shared composition model.

**Correction:** distinguish host/site populations and limit aliases to the range actually supported. Label generic coordination models as estimates until calibrated against composition-dependent spectra.

Evidence: [pair law](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/resolve.rs:527), [tourmaline aliases and chemistry](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:4243), [Fe limit](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:4323).

### S16 Precursor free centers and colloid conversions are appearance controls

**Documented approximation; separate from inverse-model defect I03.**

Topaz and zircon treatments can create maximum center intensity without a required elemental precursor. Glass reducing strike converts all modeled Cu to a colloid-associated species while folding colorless Cu+ behavior into its coefficient. These controls do not calculate defect inventory, treatment dose, colloid size or particle number density.

**Correction:** preserve them as explicit appearance presets unless precursor and particle models are added with evidence. Avoid interpreting the resulting concentration as a measured defect or colloid population.

Evidence: [topaz](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:4150), [zircon](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:4624), [glass](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:2589).

### S17 Secondary labels sometimes repeat earlier estimates

**Evidence-quality finding, not proof that every value is false.**

The repository research records unchanged numbers carried forward from earlier recalled estimates in several material groups. Some garnet entries explicitly retain prior-estimate provenance. Relabeling such values as secondary does not create independent corroboration.

**Correction:** retain the original evidence chain and distinguish recalled estimate, literature transcription, digitized spectrum and calibrated fit. Require an identifiable source passage or dataset for each promoted coefficient.

Evidence: [corundum and synthetic research](C:/Users/ngussek/Dev/Rust/diagram_loader/docs/chromophores/corundum_chrysoberyl_spinel_garnet-synthetics.md:1042), [quartz and related research](C:/Users/ngussek/Dev/Rust/diagram_loader/docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md:1718), [retained garnet estimate](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:2341).

### S18 YAG and GGG concentration bases remain unresolved

**Potentially large calibration discrepancy; not a confirmed universal factor error.**

Current entries use all-atom ppma and often lack per-band sources. Earlier research discusses substituted-site ppm and an undefined original report basis. With 20 atoms and three Y/Gd sites per formula unit, these bases differ by 20/3 for those substitution sites; that factor does not apply indiscriminately to every dopant site.

The audit could not establish which basis the original experimental coefficient used. Therefore it would be incorrect to assert that every coefficient is wrong by 6.67.

**Correction:** recover the original experimental definition, document the concentration basis and conversion, and re-establish per-band provenance before presenting absolute concentrations.

Evidence: [GGG](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:2508), [YAG](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:4425), [earlier unit discussion](C:/Users/ngussek/Dev/Rust/diagram_loader/docs/chromophores/corundum_chrysoberyl_spinel_garnet-synthetics.md:663).

### S19 Plausibility checks depend on optional metadata

**Validation gap; high confidence.**

Cross-section/absorption consistency is checked only if both metadata fields exist. Molar absorptivity/class consistency is similarly conditional. The validator does not derive all equivalent quantities and cross-check them.

The offered topaz Cr peak of 0.08 inverse cm per site-ppm implies an epsilon of approximately 905 liters per mole per cm using the stored site density. That lies far outside the validator's own nominal spin-allowed range of 1-50, yet absent metadata avoids the check. This establishes an internal plausibility gap; it does not independently determine the correct experimental coefficient.

**Correction:** derive equivalent quantities where the concentration basis permits it. Flag inconsistencies for review without treating a generic transition-class range as an infallible physical law.

Evidence: [conditional checks](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/catalogue.rs:1226), [topaz coefficient](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:4174).

### S20 Independent measured color accuracy has not been established

**Validation limitation and stale evidence wording.**

The repository research acknowledges that the acquired data do not pair an independent measured color with its own measured spectrum. Some GIA color references are calculated from spectra; some Caltech comparisons reuse data fitted by the model. These establish useful internal or spectral-fit consistency, not general predictive accuracy on independent stones.

The secondary reference fixture still describes values as measured CIELAB even though its surrounding comments disqualify them as validation targets. The broader statement that no such sample exists anywhere should be narrowed to no such sample obtained in this research.

Fluorescence yield, sideband and quenching parameters are also broadly estimated. No general calibrated absolute fluorescence claim is supported.

**Correction:** define evidence levels separately for measured spectral shape, absolute concentration scale, independent color prediction, treatment response and fluorescence. Reserve validated claims for independent specimens and acquisition conditions.

**Future acceptance:** retain fitting samples and independent validation samples separately. Record raw measurements, units, thickness, orientation, illuminant, observer, uncertainties, processing and fit residuals. Do not use a desired output hue as independent evidence for the parameters selected to produce it.

Evidence: [primary-data scope](C:/Users/ngussek/Dev/Rust/diagram_loader/docs/chromophores/research-2026-10-primary-data.md:7), [validation admission](C:/Users/ngussek/Dev/Rust/diagram_loader/docs/chromophores/research-2026-10-validation.md:298), [reference fixture wording](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/reference_validation.rs:6), [estimated fluorescence](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:380).

## 4. Numerical and physical limitations

### N01 Unpolarized swatches are a principal axis proxy

The uniaxial swatch averages transmitted intensities as (2 To + Te)/3; the biaxial version averages three principal transmittances. This is documented in code but is not a specified directional observation or, in general, an exact orientation average at finite optical depth.

For a uniaxial crystal with To=0 and Te=1, the proxy gives 1/3. Propagation along the optic axis gives the ordinary response, while a principal perpendicular view averages the two transverse modes and gives 1/2 in this example. Averaging absorption before exponentiation and averaging transmissions afterward are different operations.

**Consequence:** a swatch can be useful for choosing an overall appearance but does not guarantee the same color at every viewing direction. Concentrations inferred from that proxy are conditional on the proxy and reference path.

**Next work:** expose the observation convention; offer directional polarized/unpolarized swatches where appropriate. Validate the principal viewing cases and intermediate orientations separately.

Evidence: [body colors](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/color/body_color.rs:154), [inverse proxy](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:335).

### N02 Inverse color is neither unique chemistry nor proof of impossibility

One Lab target supplies three observables. Multiple constituents, sites, redox states, treatments and unknown optical path can produce similar colors. Some catalogue constituents share an identical summed spectrum, so even a second illuminant cannot distinguish them individually.

The solver further restricts the search to at most three free entries, two treatments, a minimum free concentration of 0.001 times the maximum, a finite coarse grid, one LM start per candidate and a 40,000-evaluation ceiling. These are search choices, not boundaries of chemical possibility.

**Next work:** describe results as candidate recipes under stated assumptions. Separate found feasible, closest found, search capped and unsupported model states. If chemical inference is a goal, require additional independent spectral/compositional information and report non-identifiability. Fix I02 even if the broader heuristic search remains.

Evidence: [limits](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:61), [candidate search](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/solver.rs:981).

### N03 Spectral compression has no enforced perceptual bound

The body resolver reduces spectra to eight bands using a CMF-weighted squared absorption-error criterion on a 5 nm grid. It does not directly minimize full-spectrum transmission or constrain final color error. Multiplying every candidate merge cost by the same positive reference-path squared does not change which pair wins.

Merging continues to meet the band budget. A D65 error warning at one reference path is reported afterward, not enforced as a guarantee over thickness, illuminant, polarization or excitation wavelength.

Fluorescence independently compresses each emitter's excitation bands using proximity and mixture moments. Even when both start from the same coefficients, independently compressed body absorption and excitation need not remain identical. The size of that discrepancy for current recipes has not been measured.

**Next work:** define separate spectral and perceptual acceptance budgets over representative paths and illuminants. Preserve narrow/excitation-critical features and inspect body/excitation consistency. If the representation cannot meet its accuracy target, expose that limit rather than silently claiming the original model's accuracy.

Evidence: [body merge cost](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/resolve.rs:628), [excitation merging](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/fluorescence.rs:423).

### N04 Supported catalogue shapes are converted approximately

Lorentzian bands are converted to Gaussians with the same peak and FWHM. Their integrated areas differ: the Gaussian has approximately 0.677 of the Lorentzian area, and the far wings disappear. The conversion comments correctly disclose this approximation. An edge becomes a Gaussian shoulder and eventually falls at higher energy, which is not a general continuum model. [NIST line-shape reference](https://www.nist.gov/pml/atomic-spectroscopy-compendium-basic-ideas-notation-data-and-formulas/atomic-spectroscopy-6).

**Next work:** retain actual line/continuum forms where the renderer can support them, or validate the chosen approximation over the wavelengths that influence both color and excitation. S01 is a concrete current failure of the generic edge convention, not merely this general warning.

Evidence: [shape conversion](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/catalogue.rs:1143).

### N05 Fixed sampling can underresolve narrow bands

Color integration samples integer wavelengths at 1 nm intervals. The zircon line at 653.5 nm with a 20 inverse-cm FWHM is approximately 0.854 nm wide. A weak-absorption Gaussian area estimate gives roughly 0.774 peak-nm from integer samples versus 0.910 continuously, about 15 percent low for that line's contribution. This does not establish a 15 percent error in the complete color.

The 5 nm compression grid can miss narrow features more severely. The stored CMFs are a coarser table interpolated at evaluation time, as is D65; 1 nm integration does not mean all underlying data are native 1 nm measurements.

**Next work:** use adaptive or band-integrated quadrature where required, and compare convergence against an independent reference. Record the actual observer/illuminant source and interpolation. The official [CIE 1 nm observer dataset](https://cie.co.at/datatable/cie-1931-colour-matching-functions-2-degree-observer) is a reference for the observer, not permission to undersample material lines narrower than that grid.

Evidence: [integration](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/color/body_color.rs:229), [zircon line](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/data/chromophores.toml:4648).

### N06 Fluorescence has a restricted physical model

Excitation collapses principal absorption axes to a scalar average. Emission is isotropic and depolarized, while attenuation still depends on the oriented absorption tensor. Orientation can therefore change attenuation without a fully corresponding excitation response. This is a potential anisotropic energy-consistency problem, not a numerically demonstrated conservation failure. Isotropic energy tests would not establish correctness in that case.

Only one fluorescence event is allowed per path. Emitted photons can be absorbed, but subsequent fluorescent re-emission/cascades are not modeled. Excitation covers 300-780 nm and camera integration 380-780 nm. The model is Stokes-only and broadens emission lines to at least 1 nm. It does not model 254 nm excitation, anti-Stokes emission, delayed luminescence/phosphorescence, or full narrow-line spectroscopy.

**Next work:** clearly state these limits in the scientific contract. Before adding complexity, validate scalar energy balance, then anisotropic excitation/absorption consistency. Add further emission events or spectral ranges only with an appropriate estimator and calibrated data.

Evidence: [excitation averaging](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/fluorescence.rs:375), [emission and single-event transport](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/raytracer/transport/inner/bounce_body.rs:577), [line widths](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/fluorescence.rs:50), [Stokes restriction](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/fluorescence.rs:258).

## 5. Additional robustness findings

These findings are distinct from confirmed failures with ordinary built-in recipes. Their trigger conditions must remain attached to any implementation issue.

### R01 Sideband centers are not validated consistently

Catalogue validation accepts finite zero/negative sideband centers, while renderer validation requires positive centers. An accepted custom catalogue can therefore produce an invalid renderer payload. No current bundled-data trigger was established.

**Correction:** apply compatible finite/positive wavelength validation at ingestion and construction. Reject the catalogue with a specific field error.

Evidence: [catalogue validation](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/catalogue.rs:1377), [renderer validation](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/fluorescence.rs:365).

### R02 Unknown quencher names silently have no effect

A nonempty quencher name can pass catalogue validation without resolving to a known host element/unit. Resolution then uses a factor of one. This can conceal a misspelled or unsupported quenching relationship. No bundled-data instance was established.

**Correction:** resolve all quencher references during catalogue validation and reject unknown identifiers.

Evidence: [name validation](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/catalogue.rs:1392), [quenching fallback](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/fluorescence.rs:507).

### R03 Truncated emitters disappear from analytic background absorption

If more than 16 emitters are generated, the list is truncated by weight. Discarded emitters are not added to the nonemitting background used by the analytic glow denominator. Their absorption should still compete for excitation photons even if their emission is omitted.

No current catalogue trigger with more than 16 simultaneous emitting channels was established.

**Correction:** retain the omitted excitation absorption as background and report emission truncation. Verify energy accounting with a constructed expanded catalogue.

Evidence: [truncation](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/fluorescence.rs:362).

### R04 Malformed persisted recipe JSON silently becomes fantasy color

Native reconstruction falls back to the legacy/fantasy representation when recipe JSON cannot be read. The fallback is useful for display compatibility, but the user is not clearly told that the physical recipe was rejected.

**Correction:** preserve useful display fallback while surfacing the rejected physical payload and its reason. Avoid silently presenting the fallback as the original physics.

Evidence: [native recipe handling](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/native/convert.rs:365).

### R05 Physical scaling depends on measurable geometry

Chromophore absorption is tagged per millimeter, and shared setup normally converts actual width to model width. Unset real size uses 7 mm. When geometry width cannot be measured or is degenerate, the setup retains an existing scale. The normal physical-unit guarantee is conditional on successful geometry measurement.

**Correction:** expose the assumed size and invalid-measurement state where it affects color. Reject a physically quantified result or use an explicitly labeled fallback rather than implying the same calibration.

Evidence: [physical unit tag](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/materials/custom.rs:423), [geometry failure path](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/render_setup/materials.rs:211), [size conversion](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/render_setup/materials.rs:306).

## 6. Retain the parts that are supported

### 6.1 Mathematical and spectral foundations

Source derivation found the following internally coherent:

- Oxide wt% converts through density, oxide molar mass, cation count and Avogadro's constant. Site-ppm and all-atom ppma use distinct number densities.
- Dividing inverse-cm absorption coefficients by ten correctly gives inverse-mm coefficients.
- Beer-Lambert transmission uses exp(-alpha times path), appropriate for Napierian alpha. Decadic measured absorbance must be converted with ln(10) before entering this convention. [IUPAC definition](https://goldbook.iupac.org/terms/view/B00626).
- Energy-Gaussian evaluation uses wavenumber 10^7/lambda for nm wavelengths and inverse-cm widths. FWHM/2.35482 is the appropriate Gaussian sigma conversion.
- The random pair expression z times cA times cB times 10^-6 is dimensionally coherent for ppm site occupancies. This does not establish its host-specific chemistry; see S15.
- The energy-domain Gaussian mixture-moment formulas are mathematically coherent. A correct moment calculation does not itself bound perceptual compression error.
- The inspected CIELAB and CIEDE2000 formulas, including the hue branches and residual cross-term completion, agree with their intended equations. The source includes the Sharma reference cases; they were not run. [Sharma, Wu and Dalal](https://hajim.rochester.edu/ece/sites/gsharma/ciede2000/ciede2000noteCRNA.pdf).

Relevant source: [concentration conversion](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/catalogue.rs:347), [absorption conversion](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/resolve.rs:314), [CIELAB and color differences](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/color/body_color.rs:296).

D65 uses a tabulated daylight distribution rather than a 6500 K blackbody substitute. The incandescent comparison is explicitly 3200 K in the UI and solver documentation; it is not claimed there to be standard illuminant A at 2856 K. Internal A-like variable names should not be used to reinterpret that documented target. Planckian display colors receive a specified white adaptation; analytical XYZ and display RGB should remain distinguished.

### 6.2 Corrected suspicions and inactive data

The final review rejected several possible allegations:

- Fluorescence excitation does include the quadratic coefficient, using the same concentration-squared term and per-mm conversion as body absorption.
- Both forward absorption and fluorescence construct energy Gaussians. There is no separate confirmed bug in which only fluorescence silently discards those shapes; the shared approximation is N04.
- CPU fluorescence transport includes the photon-energy ratio and capped-clock rate/survival correction. I10 is the analytic swatch, not a claim that transport omits that factor.
- Current chrysoberyl and tanzanite axis-length mappings agree with locally preserved Caltech headers. Direct Caltech retrieval failed during this review, so current-server verification is not claimed.
- No basis was found to revive the previous blanket allegation that ruby's blue transmission window is necessarily wrong.
- Suspect/incomplete garnet Fe2+-Fe3+ IVCT, corundum hole-Fe, diamond GR1, some CZ dopants and the old heated-tanzanite row are hidden from offered bands. Their presence in research/data is not evidence that current recipes render those defective rows.

Evidence: [quadratic excitation](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/fluorescence.rs:396), [transport correction](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/raytracer/transport/inner/bounce_body.rs:571), [offered-band filtering](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/optics/chromophore/catalogue.rs:989).

### 6.3 Application and backend behavior

| Consumer | Source-level finding | Qualification |
|---|---|---|
| Desktop physical-color UI | Editing is feature gated | Persistence/rendering intentionally remain available without the UI feature |
| Desktop live and export | Active custom fluorescence is carried into scene state | I08 still permits catalogue updates to change it |
| Remote rendering | Scene fluorescence is transmitted | I12 requires stronger submitted-scene compatibility validation |
| Mobile live and remote | Custom-aware scene plumbing carries fluorescence | Do not confuse this with the separate browser worker omission |
| Browser CPU and GPU | Actual workers omit custom fluorescence | I09 |
| GPU absorption | Energy-Gaussian bands and physical absorption scaling are encoded | This was a host-side encoding/routing review, not an exhaustive WGSL audit |
| GPU fluorescence/UV | Admission refuses unsupported use | A refusal/fallback is different from implemented GPU glow |
| Whitted Ray-Traced | Fluorescence/UV explicitly declined | It must not be presented as fluorescence-equivalent to Path-Traced |
| Analytical batch tilt | Sends no fluorescence | Intentional documented omission |
| Optical diagnostics | Explicitly warns of excluded effects | Its output is not fluorescence-aware rendered performance |

Evidence: [desktop emitters](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/bridge/render_thread/context/mod.rs:618), [export snapshot](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/bridge/export_thread/scene_snapshot.rs:310), [mobile scene](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-mobile-core/src/render/scene.rs:130), [mobile remote](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-mobile-core/src/remote/rendering.rs:684), [Whitted admission](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/renderer/whitted/prep.rs:79), [GPU material encoding](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma/src/renderer/buffers/material.rs:445), [batch tilt](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/gui/batch/tilt/engine/sweep.rs:147), [diagnostic exclusions](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/bridge/optical_overlay/export.rs:363).

Native/library reconstruction gives current recipes precedence over N-band color and legacy RGB, detects older-editor RGB conflicts through fallback comparison, and applies default orientation after installing absorption. Feature-off desktop deliberately retains original physics after an older-editor conflict, whereas generic snapshot readers can select fantasy. That is an explicit policy difference to document, not a confirmed accidental data-loss defect.

Evidence: [snapshot policy](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/native/convert.rs:365), [material reconstruction](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/native/convert.rs:489), [library precedence](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/pleochroma-cut-core/src/material/library_row.rs:334), [feature-off desktop policy](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/pleochroma-cut/src/gui/editor/native_io/open_commit/restore_material.rs:72).

## 7. Coverage across the host catalogue

| Host or family | Evidence status and remaining scope |
|---|---|
| Andalusite | Joint V/Cr model exposes lock and identifiability issues; no complete independently calibrated coefficient set established |
| Benitoite | Host/trace Ti inconsistency; proposed mechanism still needs independent establishment |
| Beryl | Measured emerald shape support with empirical scale; Mn and aquamarine population issues; Maxixe control incomplete |
| Chrysoberyl | Beta/gamma shape support; alpha and absolute Cr scale incomplete; locally supported axis mapping |
| Corundum | Stronger selected Cr/V/Fe-Ti evidence; Fe species and Fe3+ magnitude problems; trapped-hole coverage incomplete |
| Cubic zirconia | Provisional dopant data; suspect entries hidden; no complete primary calibration established |
| Diamond | Boron continuum/compensation and nitrogen/defect population issues; GR1 hidden; broad ZPL approximations |
| Andradite garnet | Report-level absorption/end-member assumptions; complete composition-response calibration not established |
| Grossular garnet | Report-level coefficients and color windows; complete composition-response calibration not established |
| Pyralspite garnet | End-member assumptions and retained estimates; suspect Fe IVCT hidden; Fe3+ site coupling incomplete |
| GGG | Concentration basis and source ambiguity; complete primary calibration not established |
| Silicate glass | Estimated coefficients and simplified redox/colloid treatment; particle effects not inferred |
| Opal | Homogeneous absorption does not establish play-of-color or structure-dependent appearance |
| Peridot | Adopted spectral shape lacks independent absolute composition scale; trace Ni/Cr coverage incomplete |
| Quartz | Adopted amethyst shape with empirical scale; treatment fractions are presets; defect populations incomplete |
| Rutile | Existing estimates remain provisional; no full new primary calibration established |
| Spinel | Report-level parameters; Fe cluster coverage incomplete; no full new primary calibration established |
| Synthetic moissanite | Existing estimates remain provisional; no full new primary calibration established |
| Tanzanite | Shape/mapping support with retained scale; heating mechanism and response need correction |
| Topaz | Treatment centers need assumptions made explicit; Cr magnitude metadata gap; additional centers incomplete |
| Tourmaline | Missing advertised Mn-Ti branch; generic chemistry does not cover all named varieties |
| YAG | Concentration basis and per-band source ambiguity; complete primary calibration not established |
| Zircon | Precursor-free treatment and inverse mismatch; narrow-line sampling; some center controls incomplete |
| Feldspar | Lone band explicitly an unfitted estimate; structural appearance is outside homogeneous absorption |

No complete primary coefficient validation was newly established for andalusite, benitoite, the garnets, topaz, tourmaline, spinel, glass, opal, CZ, YAG/GGG, rutile, moissanite, zircon or feldspar. This is a statement about this audit's evidence, not a claim that useful literature cannot exist.

Descriptive catalogue entries are not necessarily reachable recipe controls. Incomplete or unavailable cases include Mg/Cr trapped-hole corundum orange, optional green-diamond GR1, brown/defect populations without creation controls, Maxixe, some zircon/topaz centers, spinel Fe clusters, garnet Fe3+ site coupling, and peridot trace Ni/Cr. Separately, opal play-of-color, feldspar structural effects, colloidal scattering and inclusion appearances require more than homogeneous absorption. Do not count all of these as bugs in a deliberately limited absorption feature, but do not imply that selecting the host covers them.

## 8. Recommended follow-up and acceptance

The following is a proposed work order, not implementation performed by this task.

1. **Restore solver contracts.** Address I01-I06. Preserve locks and canonical concentrations, retain feasible solutions, evaluate treatment laws consistently, search empty-subset treatments and report actual errors. These fixes should not alter experimental coefficients to make a desired swatch pass.
2. **Make saved results and consumers consistent.** Address I08-I12 and R01-R05. Snapshot the optical payload consistently, validate scene compatibility, fix photon-to-energy swatches, make browser omissions explicit, and distinguish missing data from measured inert behavior.
3. **Correct the strongest catalogue contradictions.** Resolve S01-S07 and S10-S12 with traceable primary evidence. Separate species/site state from total elemental chemistry. Preserve old resolved snapshots while introducing an explicit catalogue migration.
4. **Align claims with evidence.** Address S08-S09 and S13-S20, plus N01-N02. Classify each band and treatment by the evidence it actually has. A concentration estimate derived from a shape-only fit must not acquire a measured label through UI wording.
5. **Establish controlled numerical error.** Address N03-N06. Compare full and compressed spectra, spectral quadrature, paths, polarizations and illuminants. Establish fluorescence energy consistency under the supported anisotropic conditions before broadening the supported domain.
6. **Replace uncalibrated statistical confidence.** Address I07 with an appropriate predictive or calibrated comparison. Independently validate both candidate models and the model-selection decision.

### 8.1 Future verification matrix

No commands below were run; this is a specification of evidence to collect when implementation and execution are separately authorized.

| Area | Required evidence |
|---|---|
| Solver contracts | End-to-end returned recipes preserve locks and bounds; acceptable solutions are not discarded by sparsity; duplicate imports are handled consistently |
| Treatment equivalence | Inverse and forward spectra agree for every supported treatment at zero, low, typical and saturated inputs |
| Spectral numerics | Independent convergence for narrow lines, edges and broad wings; full versus compressed spectra at multiple path lengths and polarizations |
| Colorimetry | Independent observer/illuminant fixtures; distinguish calculated-reference agreement from measured-specimen agreement; report both errors |
| Chemistry and calibration | Same-specimen composition, thickness, polarization, source spectrum and uncertainty; validation specimens held out from coefficient fitting |
| Treatment prediction | Paired before/after spectra with documented source state, atmosphere, dose/time/temperature and applicable material range |
| Fluorescence | Photon yield and radiant energy checked separately; broad/multiple emitters; nonemitting background; anisotropic excitation; support/refusal boundaries |
| Persistence | Catalogue-update stability, explicit migration, feature-off loading, older-editor conflict policy and corrupt payload diagnostics |
| Consumer parity | Actual desktop/mobile/web/remote scene construction, not merely isolated builder tests; explicit backend refusal for unsupported effects |
| Photographic model selection | Both models evaluated on held-out views; calibrated uncertainty; insufficient-evidence and misspecified-model cases |

A runtime test passing is necessary for the software contract but cannot independently validate an empirical coefficient that supplied the expected result. Conversely, good measured spectra cannot excuse a solver that changes a locked amount. These are separate acceptance requirements.

### 8.2 Evidence levels to expose

For each host, constituent and treatment, record independently:

- Spectral-shape source: measured raw data, digitized published curve, secondary transcription or estimate.
- Absolute scale: same-specimen chemical analysis and path calibration, transferred anchor or assumed coefficient.
- Polarization/site coverage: measured axes and species versus assumed mapping or averaged response.
- Treatment support: paired measurements and applicable conditions versus an appearance preset.
- Fluorescence support: measured yield/emission/quenching, estimated parameters, known absent or unavailable data.
- Validation independence: reused fitting data versus held-out specimen/measurement.

A single confidence badge cannot communicate all six dimensions. Default UI wording should match the weakest dimension relevant to the user's requested prediction.

## 9. Completeness crosswalk

The crosswalk preserves every distinct finding and qualification from the three Astra lanes, including follow-up corrections. Overlapping observations have one primary record rather than being counted as independent failures. The three reviewers checked the saved draft against their respective findings; their reference and wording corrections were incorporated.

| Review lane and original observation | Destination |
|---|---|
| Numerical: locked amount corruption | I01 |
| Numerical follow-up: acceptable candidate rejected by sparsity | I02 |
| Numerical: constant/capped treatment laws | I03 |
| Numerical: missing treatment-only search | I04 |
| Numerical: duplicate amounts; later native/fluorescence exposure clarification | I05 |
| Numerical: zero-weight second-target error | I06 |
| Primary review: non-nested photo-model significance and selected-only cross-validation | I07 |
| Integration: frozen absorption/current-catalogue fluorescence | I08 |
| Integration: actual browser workers omit emitters | I09 |
| Integration: photon versus radiant-energy analytic XYZ | I10 |
| Integration: missing/estimated data and strength labels | I11 |
| Integration: submitted-scene material compatibility | I12 |
| Science original findings 1-20 | S01-S20, in the original finding order |
| Numerical: principal-axis swatch proxy | N01 |
| Numerical: metamerism, non-identifiability, finite search | N02 |
| Numerical/integration: body compression and separate excitation compression | N03 |
| Numerical: Lorentzian and edge approximation | N04 |
| Numerical/primary review: sub-nm lines, 1 nm integration, coarser tables | N05 |
| Integration: scalar excitation, one fluorescence event, spectral/time limits | N06 |
| Integration: sideband wavelength validation | R01 |
| Integration: unresolved quencher identifiers | R02 |
| Integration: more than 16 emitters and analytic background | R03 |
| Integration: malformed native JSON fallback | R04 |
| Integration: default 7 mm and failed width measurement | R05 |
| Numerical: correct units, Gaussian moments, CIEDE2000, pair dimensions | 6.1 |
| Follow-up: quadratic fluorescence term and shared energy-Gaussian handling are present | 6.2 |
| Science: corrected old axis/ruby allegations, hidden suspect rows | 6.2 |
| Integration: feature spelling/gating, material precedence and older-editor policy | 1 and 6.3 |
| Integration: CPU estimator correction and supported desktop/mobile/export/remote paths | 6.2 and 6.3 |
| Integration: unsupported GPU/Whitted glow; tilt/diagnostic exclusions | 6.3 |
| Science: all 24 hosts, incomplete controls, structural-color limits | 7 |
| Science: failed direct Caltech retrieval; no literature-wide negative claim | 6.2 and 7 |
| Integration: GPU host encoding inspected, no exhaustive shader audit | 6.3 |

The report does not claim experimental validation of every host, exhaustive execution of every call site, or a complete literature search proving that missing evidence does not exist. It records the confirmed source defects and the evidence actually available for scientific interpretation.

