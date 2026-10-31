# AI representation and action-capacity review

Source review dated 2026-10-26. Scope: `crates/pleochroma-ai` and `apps/pleochroma-trainer`. Other geometry/material sources were read to establish the contract, not edited. No database, training run, build, test, benchmark or Clippy execution was used. This document cannot establish empirical accuracy or GPU numerical parity.

This report supersedes the parameter-count, composed-policy-only, imitation-action-loss and teacher-search descriptions in `training-math-review-2026-10-26.md`. That earlier report remains useful historical analysis of the component objective, projected secants and numerical caveats. Its claims that imitation does not differentiate through `tanh`, and that a zero central derivative necessarily produces no teacher action, no longer describe the updated code.

## Conclusion

The model can represent a useful learned proposal policy for the present, restricted training distribution. It is not a complete representation of every design, material or optimization operation supported by the application. A larger transformer would not restore information that the observations omit. A measured held-out comparison against the existing optimizer is still required before calling a trained checkpoint sufficient.

Two concrete defects were addressed during this task: incorrect azimuth features and an action parameterization that could not express useful verified moves when its true local sensitivities were zero. The changes do not expand the dataset to native designs, arbitrary rough or all catalog materials. Those remain explicit coverage gaps.

## Task 3 (2026-10-26 22:19) outcome

A third pass, "fix the remaining input-feature and training-data gaps", was interrupted without a summary. It was audited and completed on 2026-10-27 by source reading only: no build, test, Clippy, benchmark or training run has executed. Everything below is therefore written, not measured. Line numbers are as of the audit.

The sections further down describe the state after task 2 (observation layout 6, schema 5, 26/41 features, a 13-entry material spectrum, a fixed block rough). Where they disagree with this section, this section wins. Current versions: observation layout 7 (`env/observation.rs:54`), checkpoint schema 6 (`model/checkpoint.rs:45`), `TIER_DIM = 384`, `GLOBAL_DIM = 3781`. Schema 5 and earlier checkpoints are refused by schema number before their network is parsed; a layout 6 file is refused by layout number. There is no migration (both embedding widths changed).

### Now covered, with the code that does it

| Former gap | What exists now | Where |
| --- | --- | --- |
| Orbit summarized by four harmonics | Every orbit position as `azimuth / 2*pi` (reference teeth, cheater degrees, signed gear, implicit index 0) plus the authored binary frosting of each position; at most 128 positions per tier, never truncated | `env/design_features_program.rs:10`, `:41`; `env/design_features.rs:276` |
| Relation present flag only | Prefix program of the angle relation: opcode, constant (signed log compression) or driver position, at most 32 nodes | `env/design_features_program.rs:55` |
| Named meet is a category | Resolved participant tier positions, at most 16, plus a fully-resolved flag | `env/design_features.rs:291` |
| Target present flag only | Target kind (depth, girdle thickness, table width) and value | `env/design_features.rs:303` |
| Preservation context hidden | Original solved mast and hinge (point, azimuth, side, wall-edge) per tier; policy limits and flags, baseline identification radius and slack, and the baseline's junction/edge/face/reach-fallback counts | `env/design_features.rs:310`, `env/design_features_rough.rs:60` |
| Rough fixed as a block, physical size absent | Rough kind, dimensions, y offset, custom half-spaces (at most 32), physical stone width, girdle frosting; current width ratio per step | `env/design_features_rough.rs:60`, `env/moves.rs:181` |
| Concave tools absent | Up to eight tools: kind, angle, azimuth, displacement, diameter, tool angle, motion and every placement | `env/design_features_rough.rs:109`, `:129` |
| Absorption reduced to three averaged samples | Source and target material each carry five optical-character flags, routing flags, the principal frame, principal indices at the D/F/C lines, scale and unit, and the three principal absorption spectra at every integer nanometre from 380 to 780 | `env/material_features.rs:81` |
| Crystal-axis orientation and stone size ignored by the scorer | Every candidate is scored with the design's axis override and physical width applied to the material and with authored frosted finishes, using the production helpers | `env/common.rs:243`, `:248`; `env/material_features.rs:231` |
| Environments built from unrepresentable designs | `EnvCore::new` and the dataset loader refuse designs beyond the stated capacities with an explicit reason | `env/common.rs:573`, `env/design_features.rs:170` |
| Only ASC designs, fixed block, 13-entry spectrum | Native `.psdd`/`.pleochroma` records keep authored rough, size, programs, tools, axis effects and material (embedded custom materials restored, unknown custom names refused); ASC records get an inferred enclosing rough and one drawn physical width, labelled as augmentation; `--design-dir` loads a directory | `trainer/src/dataset_inputs.rs:24`, `:149`; `cli_main.rs:190` |
| Spectrum limited to ten presets | Targets and ASC sources are drawn from every built-in material (including Ametrine, Peridot, Tanzanite) with a sampled crystal frame; native sources stay authored | `trainer/src/dataset_spectrum.rs:25`; `runner_env.rs:180`, `:209` |
| Split leakage across rotated, reordered, scaled families | Connected components over shared IDs and a normalized geometric-family key that ignores order, equivalent gears, scale, rotation and reflection | `trainer/src/dataset_family.rs:10`; `dataset.rs:348` |
| Validation cohort was the first N entries | Fixed cohort stratified by format, tier band, concavity and relation programs, always drawn from the validation split | `trainer/src/dataset_coverage.rs:99` |
| Shape and lighting context coarse | Shape targets are continuous in table (35 to 75 %) and crown/pavilion ratio (0.15 to 0.8) with a sampled weight; lighting stays the seven presets | `runner_env.rs:301` |
| No corpus accounting | Per-run coverage sidecar (`<stats-stem>.coverage.json`) with ASC/native counts, families, tier bands, native materials and the effects the analytical objective does not evaluate | `trainer/src/dataset_coverage.rs:33`; `cli_main.rs:213` |
| Replay memory unbounded in bytes | Aggregate capped at 512 MiB and each update snapshot at 64 MiB, in addition to the row cap | `runner_config.rs:17`, `runner_imitation.rs:153`, `runner_update.rs:55` |

Audit repairs made in this pass: four tests still asserted the pre-task-3 widths or versions (`observation_tests.rs`, `actor_critic_tests.rs`, `tests/ai_env_training_tests.rs`: 3780/256, 41 and layout 6; they now state 3781/384 and layout 7) and two GUI tests still assumed the 13-entry spectrum (`ui_state.rs`, `gui/models.rs`). The new dataset modules got `allow(dead_code)` where only the CLI binary calls them, and the new `--design-dir`, `--include-library`, `--max-tiers` flags and the long CUDA command have parser tests.

### Still not covered

- Fire, Scintillation and any render-only quantity: the seven supervised components are unchanged, and the teacher is the analytical objective.
- Spatial zoning, inclusions, structural color, scattering transport, edge rounding and abrasion, polish grades and scratch directions are not evaluated by the objective and not observed (`material_scope_notes` lists them per corpus). Authored binary frosting is the only surface effect that is both scored and observed. A per-stone `crystal_axis_plan` is not observed except through the axis override applied to the material.
- The original meet graph, per-tier inward reach and the junction list are not listed. The original plane arrangement is present (angle, mast and every orbit azimuth per tier), so the graph is determined by the inputs but not given to the policy, and only counts of its elements are.
- Capacity ceilings (orbit 128, relation 32 nodes, named meets 16, rough planes 32, concave tools 8) refuse designs instead of encoding them. Some real 360/720-position gears will exceed 128 placements and be skipped.
- Lighting is the seven presets; arbitrary environment maps and manual lights are not sampled. Shape targets are table and crown/pavilion ratio only.
- The action space is still one angle delta per tier; no tool, flat-depth or topology actions.
- Native coverage depends on what is supplied: the library normally holds ASC attachments, so without `--design-dir` (or `.psdd` attachments) the corpus has no relations, dimensional targets, concave tools or authored materials. The run prints a warning and the sidecar states the counts.
- Efficiency: a replay row is about 113 KB at 64 tiers (212 KB at 128), almost all of it static design and material context repeated per row. The 512 MiB cap keeps about 4,700 rows at 64 tiers regardless of `--label-capacity`, and the 64 MiB update snapshot limits one update's mini-batches to about 590 distinct rows. Storing the static suffixes once per episode would remove this, and was judged too large to do safely without a compiler.
- Nothing here establishes learnability: a 3781-wide global token dominated by spectra and a 384-wide tier token with sparse orbit slots are large for a 64-wide model, and only a measured ablation (for example against the layout 6 prefix alone) can say whether they help.

## What the real corpus and environment provide

`apps/pleochroma-trainer/src/dataset.rs` loads ASC attachments, parses their cutting schedules, and calls `Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), ...)`. Empty schedules and schedules above the configured tier cap are skipped. The maximum supported padding cap is 64. This review did not open the library; counts in earlier user logs are not a fresh corpus audit.

The constructor in `crates/pleochroma-cut-core/src/design/construct.rs` turns imported tiers into fixed `ScaleReference` constraints. Imported meet descriptions remain metadata, not an active native constraint program. Imported designs have no native tier relations, dimensional targets, concave-tool definitions, physical girdle width, per-stone crystal-axis plan, or authored render effects. Consequently:

| Capability | Present in the normal ASC training path? | Consequence |
| --- | --- | --- |
| Arbitrary planar cutting schedules within the tier cap | Yes, subject to load and solve validation | Useful geometric diversity, without a source-only proof of coverage |
| Existing geometric meet connectivity | Captured from solved imported geometry | Preservation remains an exact environment check |
| Native named-meet programs, relation types/parents and dimensional targets | No | Current flags cannot teach behavior absent from the data |
| Concave tool shapes and tool parameters | No | A special fixture is not evidence of real-corpus generalization |
| Arbitrary rough/preform dimensions and physical stone sizes | No; a fixed block is supplied | Yield/size conditioning is not equivalent to training across real rough |
| All supported native-file operations and structures | No | The model should not be advertised as a replacement for every optimizer function |

`runner_env.rs` replaces the design's source material with a seeded sampled preset. Retarget same-material episodes reuse that sampled source; cross-material episodes sample a separate target. This is useful augmentation, but is different from training against every design's original authored material.

`dataset_spectrum.rs` defines 13 target entries. Ten use real material presets: Quartz, Topaz, Tourmaline, Spinel, Sapphire, Zircon, Cubic Zirconia, Diamond, Synthetic Moissanite and Rutile. Fluorite, Beryl and Garnet are neutral constant-index fallbacks. The ten preset entries are the sampled source pool. This does not cover every named material available to the application, including ametrine, peridot and tanzanite. Lighting and some shape targets are sampled, while crystal-axis orientation is not independently randomized by this path.

The full source-based corpus and task coverage audit also identifies these limits:

- The split groups schedules with the same entry identifier or normalized geometry into connected components and assigns each whole group to one split (`dataset.rs:139`). This protects recognized duplicates. It is not a geometric-family split or a proof that rotated, reordered, scaled or other near-duplicate families cannot cross splits. No actual leakage was established without inspecting the corpus.
- A configured validation subset takes the first requested entries in its split. Repeating the objective sweep changes objective weights while reusing the same seeded material/context per design. That supports controlled comparisons but is not an exhaustive design-by-material-by-orientation evaluation.
- Shape sampling offers no target, table 52%, crown/pavilion ratio 0.42, or both table 58% and ratio 0.50, each with weight one (`runner_env.rs::sampled_context`). It does not continuously cover all valid shape targets or user weights.
- Lighting samples seven presets. Arbitrary environment maps and manually arranged illumination are not represented by this sampler.
- Random objective weights set tone weight to zero, while the named preset branch supplies tone objectives. Consequently the random mixture is not general continuous tone-weight coverage. The seven supervised outputs contain no dedicated Fire or Scintillation objective, even though the application's metrics can display those effects.
- `ObjectiveFidelity::Fast` uses face-up optical values, including face-up brilliance in the field named `tilt_brilliance_pct`; `Full` averages the tilt sweep (`pleochroma-cut-core/src/optimize/objective.rs:494`). Training with `--train-fidelity fast` and validating with full endpoint scores does not teach full-fidelity tilt derivatives. The fidelity feature identifies this distinction but cannot supply training examples for a fidelity never taught. Full validation can reveal the gap; it does not close it.

These are distribution and task-definition limits, separate from model expressivity. A valid ASC design with more than 64 tiers is outside the configured tensor cap regardless of network width; a smaller design with an unsupported material or native constraint program can still be outside the training distribution. The implementation does not claim otherwise.

## Observation content and its limits

`env/observation.rs`, `env/observe.rs`, and `env/features.rs` define 26 features per tier and 41 global features. Tier features include current/base angles, count and low-order moments of the orbit, block, mobility/bounds, normalized mast, current live area/centroid, and coarse constraint/relation/target flags. Global features include source/target indices, target F-C dispersion, current and baseline scores, current component values, shape target, fidelity, step/probe sizes, three averaged absorption values, material path scale, scattering strength, lighting identifier and objective coefficients.

### Geometry orientation defect, corrected in layout 6

The actual geometry computes:

```text
phi = 2*pi*(index + gear_reference_angle)/abs(gear_teeth)
      + cheater_degrees*pi/180
```

`StandardGemCuts::index_to_azimuth` in `pleochroma/src/geometry/cuts/round_brilliant.rs` and the cheater conversion in `pleochroma-cut-core/src/design/export/planes.rs` establish this formula. `gear_reference_angle` is a wheel-index offset in this calculation, despite misleading degree terminology in some metadata documentation.

Previously the orbit features interpreted this offset as physical degrees. The first-facet feature ignored it and the cheater entirely, and returned a constant direction for negative gear values even though geometry accepts their absolute tooth count. Empty index lists represent one facet at index zero, not zero facets. These are real input/geometry disagreements, rather than statistical model shortcomings. The corrected feature semantics require observation layout 6 even though the dimensions remain 26/41.

### Remaining geometry information loss

Low-order harmonic moments, first-facet direction, area and centroid are summaries, not a complete list of plane normals, offsets and adjacency. Many distinct sets have equal low-order moments. Later measured metrics add information but are also aggregate statistics; no injective encoding of arbitrary facet arrangements follows from them.

The preservation projection depends on the immutable original meet baseline and original per-tier hinges in `env/moves.rs`. The observation has base angles and some baseline scalar metrics, but not the original connectivity graph or hinge point/azimuth/side. Therefore it is not a demonstrated Markov-sufficient representation of that projection. The exact geometry gate still prevents unsafe application; policy error can appear as rejected or unchanged proposals.

For native designs, a relation-present flag does not identify its driver, relation type or coefficient. A target-present flag does not encode target type/value, and a generic named-meet category does not list its participants. These states can respond differently to an angle proposal even when the coarse constraint descriptors coincide. Normal ASC imports do not exercise these richer semantics, so passing ASC validation would not close this gap.

Concave tool type, radius, axis, position and affected surfaces are not encoded. The action vector also changes tier angles rather than creating facets or adjusting tool geometry, independent flat depths, meet programs or other arbitrary design parameters.

### Remaining material information loss

`absorption_summary` averages ordinary, extraordinary and optional beta-axis absorption at only 450, 550 and 650 nm. This discards directional absorption and most spectral shape. Two material tensors can share those samples while differing between wavelengths or by principal-axis orientation. The observations also omit optical character, ordinary/extraordinary/biaxial index splitting and crystal-axis orientation.

These omissions matter to the actual teacher, rather than merely to the path tracer. The optimizer objective calls `color::metrics::PreparedEval`, which constructs `Modes::for_material` for anisotropic propagation and `PrincipalFrame::for_pleochroic` for directional tone. Tone integrates absorption spectra along principal-axis paths. Equal three-point averages are not enough to reproduce those calculations for arbitrary user materials. Current component values do not specify their derivatives under all future angle changes.

The restricted preset distribution partially hides this problem: a unique refractive index can act as an indirect identifier for a fixed preset with fixed orientation. It does not establish transfer to custom spectra, changed crystal-axis orientation or materials outside that finite pool. Scattering, zones and other render effects must not be claimed as learned full-render objectives merely because some related scalar is present; the training score is the analytical objective actually invoked, not the complete path tracer.

## Action expressivity and the implemented residual head

The earlier policy used only component slopes `s[k,c]` and objective coefficients `w[c]`:

```text
g[k] = sum_c w[c] * s[k,c]
d[k] = -MEAN_BOUND * g[k] / max(max_mobile(abs(g)), SLOPE_NORM_FLOOR)
mean[k] = d[k]
```

If every correctly predicted local derivative is zero, the mean is necessarily zero. A verified finite move can nevertheless improve a discrete ray-count objective, cross a plateau, or require moving two coupled tiers together. Teaching nonzero slopes merely to force that action would corrupt the derivative targets.

The network now has a separate `action_head: Linear(d_model, 1)` on each final-normalized tier token:

```text
r[k] = action_head(hidden[k])
mean[k] = mobile[k] * (d[k] + r[k])
step[k] = max_step * tanh(mean[k])
```

The residual weights and bias initialize to zero in both pure Rust and Burn. Initial policies therefore retain their earlier sensitivity behavior. The residual can learn finite verified teacher actions without changing the meaning of the seven sensitivity outputs. The mean is allowed to be unbounded; `tanh` and the environment's feasibility/preservation checks still constrain the applied move. This does not prove that training discovers a useful residual.

Burn also multiplies by the real-tier mask, including when padding contains nonzero mobility data. The residual reads the same normalized hidden tokens on both implementations. Import/export, structural and finite-weight validation, and parameter counts include its weights and bias. Critic, gain and log-standard-deviation definitions are unchanged. A scalar residual per token adds only `d_model + 1` parameters.

For the Medium model, width 64, four heads and three blocks, the parameter count was 105,613 after this head, compared with 105,548 before it (task 3 widened both embeddings to 384 and 3781 inputs, which makes it 367,885). Its depth/width is not mathematically proven sufficient for every 64-tier design. Transformer attention permits inter-tier interaction; it does not guarantee accurate learned multi-bounce physics. The tensor architecture is equivariant to token permutation, but the authored-order feature means the full input construction is not an order-free geometric representation.

The current exact parameter-count formula is:

```text
P = (26+1)d + (41+1)d + L*(8*d*d + 11*d)
    + 2*d + 10*(d+1) + 3
  = L*(8*d*d + 11*d) + 81*d + 13.
```

The ten head outputs are seven component sensitivities, one residual action, one gain and one critic value. The three remaining parameters are block-specific raw log standard deviations. In imitation, the critic and log-standard-deviation heads do not receive their own supervised loss; rollout exploration uses the configured imitation noise. Their presence does not add a PPO objective to imitation.

## Updated teacher: preserve probes that already found an improvement

The exact earlier blind spot is a symmetric improving neighborhood. With `F(0)=10` and `F(-h)=F(+h)=9`, the correct central derivative is zero. Discarding the two observed improvements and generating actions only from that derivative loses an available verified move. Likewise, simultaneous gradient coordinates can be infeasible even when a single measured coordinate was feasible and helpful.

The updated path keeps two distinct outputs per tier: the actual component/score secants and the best improving canonical coordinate action measured while obtaining them. `env/teacher.rs::canonical_probe` first chooses a finite f32 raw action, reconstructs its executed `tanh(raw)*max_step` displacement, and uses that actual displacement as the finite-difference denominator. This avoids claiming that a target was tested at one displacement and executing a rounded different displacement. The full coupled-preservation radius is still the environment step radius.

The finalizer in `env/teacher_search.rs`:

1. Retains all useful coordinate winners from the probes at the unchanged current state.
2. Generates each distinct canonical finite raw gradient-scale vector, and evaluates it at execution scale one through the normal candidate gates.
3. Compares the measured gains, including the coordinate winners. Strictly greater gain replaces the winner; ties retain deterministic input order.
4. Only when there is still no winner, attempts a bounded coordinate and paired escape search on at most two tiers, selected by absolute measured slope with stable tier-index ties. Up to six coordinate attempts use both signs at half the nominal radius and at the maximum representable radius, using a quarter nominal radius instead when the nominal probe already used that maximum. Two slots are reserved for paired actions at `min(h, 0.95*step_limit)`: one uses each tier's downhill sign (positive when its slope is zero), switching to the inward sign when that canonical request is blocked at a bound; the second reverses only the second tier's sign. Thus an interior zero-slope case tries `(+,+)` and `(+,-)`, not every quadrant. Duplicates and already tested probe actions are skipped. There are at most eight additional candidate attempts in total, counting invalid projections against that attempt cap.
5. Emits `action_target=Some(raw_vector)` only for a measured feasible improvement. An unsuccessful bounded search emits no action label. `oracle_gain=Some(0)` then means that this search found no improvement, not that the state is a local or global optimum. `None` gain denotes a search that was not requested or could not be formed.

The teacher tests finite actions with all existing closure, manufacturability, girdle and meet-preservation checks. It does not weaken any preservation requirement to obtain positive training examples. The canonical raw component is bounded by `MEAN_BOUND`, so its requested per-coordinate magnitude is at most about 95% of the environment step limit. Actual coupled projection may move additional coordinates; the label is the requested raw vector that produced that result, not a fabricated derivative of those repaired coordinates.

Observation assembly and the teacher now share `EnvCore::policy_coordinate` for mobility and the f32 action interval. Fixed or f32-collapsed bounds, current angles outside the model's safe interval and tiers outside the core free set are masked consistently in coordinate probes, gradient actions and rescue candidates. This closes the older review's teacher/model mask disagreement; it does not add new degrees of freedom to the action space.

The final replay-precision check also preserves already feasible policy deltas bit for bit. Previously, `project_actions` always computed `(current + step) - current`, which can change the low bits even when no bound was hit. It now keeps `step` unchanged in the valid interval and only subtracts the current angle when clipping is actually necessary. Together with canonical teacher clipping against both native and observation bounds, this prevents an unnecessary extra rounding operation from changing a verified raw action at the policy projection layer. The physical geometry application can still round coordinates and must still pass its normal gates.

Canonicalization also guards inverse-tanh rounding: if reconstructing a finite raw action slightly exceeds the chosen displacement, the raw f32 value is moved toward zero using `next_up`/`next_down`, for at most eight adjustments. Failure to obtain a safe magnitude returns zero, never an outward-clipped unverified target. This is a bounded conservative fallback, not a proof that every extremely small representable move survives canonicalization.

This repair is deliberately bounded. It does not enumerate every pair, every sign quadrant or arbitrary multi-tier directions, search every tier at every radius, create new topology, prove global optimality or make the ray-count objective smooth. Probe provenance is a same-state API contract: numerical validation can reject malformed rows, but cannot independently prove that an externally fabricated probe came from the stated environment. The trainer's scheduled probe/finalizer path holds the same environment state until assembly.

## Updated imitation loss and the model/teacher interface

`TeacherLabel.action_target` is carried as a complete finite raw action row through rollout, the FIFO label aggregate and the bounded replay snapshot into `ImitationBatch.action_targets`. An entirely NaN row is the explicit absence representation. Partial rows are rejected, because masking arbitrary coordinates of a verified joint action can change its effect. Finite zeros on other mobile coordinates remain supervised, so learning a single-coordinate winner does not permit unrelated coordinates to drift.

The new loss is:

```text
H(e) = e*e/2           when |e| <= 1,
       |e| - 1/2      otherwise.

a_hat[k] = tanh(mean[k])
a_teacher[k] = tanh(raw_teacher[k])
L = L_sensitivity
    + cosine_coeff * L_direction
    + action_coeff * L_action
    + gain_coeff * L_gain

defaults: cosine_coeff=0.5, action_coeff=1.0, gain_coeff=0.1.
```

`L_sensitivity` remains masked Huber on the seven true component derivatives. `L_action` is masked Huber between the two squashed action-fraction vectors. `L_direction` compares those same vectors by masked cosine. `L_gain` is masked Huber on the verified score decrease. Score slopes are retained as diagnostic data; an unverified derivative direction is no longer silently substituted for a verified action target.

For one supervised action coordinate, with `N` labelled action entries, the magnitude-regression derivative before weighting/clipping is:

```text
d L_action / d mean[k]
  = mask[k] / max(N,1)
    * clamp(tanh(mean[k]) - tanh(raw_teacher[k]), -1, 1)
    * (1 - tanh(mean[k])^2).
```

Thus this imitation route now explicitly differentiates through the policy squash, but still does not differentiate through geometry projection, tracing, teacher search or environment acceptance. Raw mean values far into saturation can have small gradients. Squashed action targets avoid an ill-posed demand to regress an infinite inverse-tanh endpoint.

The cosine uses `sqrt(x*x + 1e-12)` for the predicted norm and adds `1e-8` to the norm product. A zero target norm is omitted from the cosine divisor; a verified zero coordinate remains part of magnitude regression. An absent action row contributes to neither action term. Missing component/gain values become numeric zero with zero masks before device arithmetic. Gain can be trained on a measured zero even without an action winner. Padding and immobility are excluded consistently from policy actions and label support.

Training telemetry now reports `action_samples`, counting selected samples with real/free/mobile action-label support once, alongside `action_loss`. An action loss of zero can therefore be distinguished from having no verified action labels. Verified zero coordinates count as support; missing action rows do not.

The trainer also excludes score-only diagnostic labels from replay because the revised loss does not consume them. Evaluation cases that actually run the teacher reference record its final score, candidate evaluations, step count and elapsed seconds separately. Candidate counts include probes, search and unit-scale replay; they exclude reset and endpoint rescoring, which are included in the elapsed time. Cached mean-reference results do not fabricate per-case counts.

The loss still supplies no convergence theorem. Cosine gradients close to a zero predicted vector can be large despite finite epsilons; the existing finite-gradient guard and global gradient clipping remain relevant. Competing tasks share the trunk, and even a separate residual head does not mathematically guarantee that training leaves every sensitivity estimate unchanged. The required validation is measured component error, action success, preservation and score improvement on held-out states.

Source review of the cross-file contract found that both pure and Burn means include the residual, packing applies `tanh` once to raw teacher targets, loss applies `tanh` once to predicted means, and environment execution also interprets a raw action through `tanh`. Evaluation now replays the verified teacher action rather than regenerating an unrelated slope-only vector. No duplicate squash is applied to the same target or prediction in one training path.

## Checkpoint compatibility and source coverage

(Superseded by task 3: schema 6 and observation layout 7 are now required, see above.) Checkpoint schema 5 required the residual head; observation layout 6 required corrected geometric feature semantics. Old checkpoints are rejected explicitly rather than being read with changed meaning. Restart training for the new representation. There is no automatic migration claiming equivalent predictions. Bare-network JSON also requires the new head.

Source regression coverage was added for nonzero actions with zero slopes, immobile and padded-tier masking, learned-residual pure/Burn forward parity, both-way weight transfer, mean gradients reaching the residual head, malformed/missing head rejection, previous-schema rejection and parameter counts. Existing tests of zero-initialized sensitivity composition remain relevant. These tests were written but not run.

## Source index

Paths below are relative to the repository root; line numbers identify the inspected implementation and may move with later edits.

| Source | What was verified |
| --- | --- |
| `apps/pleochroma-trainer/src/dataset.rs:122` | ASC constructor and fixed preform |
| `apps/pleochroma-trainer/src/dataset_spectrum.rs:39` | Finite material catalog and fallback entries |
| `apps/pleochroma-trainer/src/runner_env.rs:201` | Sampled source-material pool |
| `apps/pleochroma-trainer/src/runner_env.rs:217` | Actual environment material overrides |
| `crates/pleochroma-ai/src/env/observation.rs:54` | Observation layout version 7 (6 before task 3) |
| `crates/pleochroma-ai/src/env/observation.rs:205` | Per-tier feature contract |
| `crates/pleochroma-ai/src/env/observation.rs:336` | Global feature contract |
| `crates/pleochroma-ai/src/env/observe.rs:67` | Shared observation/teacher mobility and action interval |
| `crates/pleochroma-ai/src/env/observe.rs:81` | Observation assembly and coarse constraint flags |
| `crates/pleochroma-ai/src/env/features.rs:6` | Three-point averaged absorption descriptor |
| `crates/pleochroma-ai/src/env/features.rs:48` | Corrected shared azimuth conversion |
| `crates/pleochroma-ai/src/env/moves.rs:48` | Hidden immutable preservation context and hinges |
| `crates/pleochroma-ai/src/env/moves.rs:165` | Coupled action projection |
| `crates/pleochroma/src/geometry/cuts/round_brilliant.rs:64` | Authoritative index/reference azimuth formula, read-only dependency |
| `crates/pleochroma/src/color/metrics/prepared.rs:71` | Material modes and principal paths in the actual analytical scorer |
| `crates/pleochroma/src/color/metrics/birefringent.rs:108` | Anisotropic material mode construction |
| `crates/pleochroma/src/color/principal_axes.rs:95` | Directional absorption frame construction |
| `crates/pleochroma-ai/src/model/actor_critic.rs:355` | Pure forward pass with masked residual |
| `crates/pleochroma-ai/src/training/burn_model.rs:490` | Burn forward pass and padding masks |
| `crates/pleochroma-ai/src/training/burn_model_export.rs:109` | Packed export includes residual weights |
| `crates/pleochroma-ai/src/model/checkpoint.rs:45` | Required schema 6 (5 before task 3) |
| `crates/pleochroma-ai/src/env/teacher.rs:103` | Separate verified action target and true sensitivities |
| `crates/pleochroma-ai/src/env/teacher.rs:421` | Canonical finite raw probe displacement |
| `crates/pleochroma-ai/src/env/teacher_search.rs:71` | Measured candidate selection and bounded rescue |
| `crates/pleochroma-ai/src/training/imitation.rs:139` | Complete action-row batch contract |
| `crates/pleochroma-ai/src/training/imitation_labels.rs:25` | Independent derivative and verified-action packing |
| `crates/pleochroma-ai/src/training/imitation_loss.rs:77` | Squashed action Huber/cosine and component/gain losses |
| `apps/pleochroma-trainer/src/runner_imitation.rs:35` | Label transfer into replay |
| `apps/pleochroma-trainer/src/runner_update.rs:69` | Bounded owned snapshot includes action targets |
| `apps/pleochroma-trainer/src/runner_eval.rs:182` | Validation uses the teacher's verified raw action |

## What would establish broader sufficiency

1. Evaluate trained checkpoints on held-out geometric families and tier-count bands against both no-op and existing deterministic optimization, with preserved meets and rejected/no-change rates reported separately from score.
2. Include native constraint/relations/target programs in training before claiming those capabilities. Encode their actual dependencies and immutable preservation context, not just presence flags.
3. For arbitrary materials/orientations, add explicit anisotropy/axis descriptors and sampled per-principal-axis spectra, then augment and hold out those conditions. Increasing model width alone cannot replace missing inputs.
4. If concave and independent depth/tool optimization are required, extend both observations and action space and build matching corpus coverage. The current angle policy cannot implement actions absent from its output space.
5. Compare multiple model sizes and ablations using identical feasible teacher budgets and validation conditions. Capacity and data sufficiency are empirical conclusions; this source review does not promise a globally optimal or universally accurate model.
