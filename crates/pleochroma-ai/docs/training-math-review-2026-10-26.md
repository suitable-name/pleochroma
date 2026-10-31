# Training mathematics review — 2026-10-26

**Historical version.** The later [representation and action-capacity review](representation-review-2026-10-26.md) supersedes this document's model parameter count, composed-policy-only behavior, teacher search, and imitation action-loss descriptions. Current code uses a residual action head, checkpoint schema 5, observation layout 6, retained verified teacher actions and Huber/cosine supervision after `tanh`. The projected-secant and objective analysis below remains relevant where the later report does not replace it.

This is a source and algebra review of the CUDA, retarget, medium-model imitation route. The reported configuration is 4,096 rollout steps, labels on every second sample, and 64 optimizer updates of at most 512 rows each. No build, test, benchmark, training run, database query, or running-process inspection was performed. Current source cannot establish which revision an already running executable contains, actual device utilization, label support, acceptance rates, generalization, or convergence.

## Executed route represented by the configuration

`apps/pleochroma-trainer/src/runner.rs::build_trainer` maps `DeviceChoice::Cuda` to `BurnTrainer::new_cuda`. `training/burn_ppo_backend.rs` constructs `Device::cuda(0)` with no fallback, and `BurnTrainer::new` enables autodiff. The trainer name and module name are shared with PPO; `runner_update.rs::run_update` selects `train_imitation_epochs` for an imitation job. That function differentiates the imitation loss and applies Adam after global gradient clipping. It does not evaluate a PPO surrogate, advantage, entropy bonus, or critic loss.

Rollouts use the Burn model without gradient tracking through `forward_host`; validation and exported inference use the pure Rust network. Teacher probes and optical/geometry evaluations run outside autodiff on the host. Training does not differentiate through the ray tracer, gates, or meet-preservation solver. Source parity is checked below; numerical agreement of a particular CUDA execution was not measured.

`ModelTier::Medium` is width 64, four attention heads of width 16, and three pre-layer-normalized transformer blocks. Each block has four 64×64 attention projections and a 64→128→64 GELU feed-forward branch, with one residual addition per branch. Tier/global input widths are 26/41. Embeddings use the same approximate GELU in both implementations; a final layer norm precedes the heads. Sensitivities have seven outputs per real tier. Value and gain each read the context token plus the mean of real tier tokens. Three state-independent raw log standard deviations serve crown, pavilion, and other tiers.

The exact parameter count is

```text
P = (26+1)d + (41+1)d + L(8d²+11d) + 2d + 9(d+1) + 3
  = L(8d²+11d) + 80d + 12
  = 105,548, for d=64 and L=3.
```

The display value 105,550 is the documented rounded approximation. Imitation has no loss path through the critic's 65 parameters or the three log-std parameters. Its rollout noise instead uses `explore_std`, normally 0.3 in raw action units. The gain head is supervised, but its output does not choose the deterministic action.

With at least 512 replay rows and usable labels in every mini-batch, 64 updates consume 32,768 row draws per round. Sampling is without replacement inside each mini-batch and independent across mini-batches, so these are not 32,768 distinct new labels. A 4,096-step rollout requests 2,048 labels at period two; failed or unavailable measurements need not provide usable component labels. Updates with no target are skipped. `epochs_per_round` is inactive when `updates_per_round` is positive.

## Objective and action algebra

The seven lower-is-better component values are windowing, extinction, `100-brilliance`, `100-yield`, lighter-tone loss, deeper-tone loss, and `100*shape_distance`. All component derivatives are percentage points per signed degree. Positive pavilion movement means becoming less negative. Angle inputs normalized by 90 do not require multiplying these supervised output labels by 90: the network predicts a physical-unit quantity rather than differentiating its own normalized input.

For nonnegative weights and unchanged measurement availability, the six optical/yield/tone coefficients are their weights divided by their sum when that sum exceeds `1e-6`; an unavailable yield receives coefficient zero and leaves that denominator. Only the selected tone goal has nonzero weight. The shape coefficient stays additive, equal to `shape_target.weight`. A sum at or below `1e-6` falls back to the three optical terms equally. These conventions match `evaluate_design` and cut-core's `score_with_tone`/`ShapeTarget::penalty` in real arithmetic. Directly differencing an f32 score and composing differences of f32 components can differ by rounding. A change in metric availability can change the scorer's normalization, so fixed-coefficient linearity then does not apply.

Let `s[k,c]` be predicted component slopes, `w[c]` their coefficients, and `m[k]` indicate a real mobile tier. The composed raw action is

```text
g[k] = sum_c w[c] s[k,c]
D = max(0.05, max_{k:m[k]=1} |g[k]|)
mu[k] = -B m[k] g[k] / D,             B = 1.83178 ≈ atanh(0.95)
delta[k] = step_limit * tanh(mu[k]).
```

Both model implementations and the teacher use the same floor and sign. Above the floor the steepest active tier requests approximately 95% of the step limit; below it action size decreases continuously toward zero. Immobile tiers cannot set `D` in the model or the imitation direction target: both apply observation mobility before normalization. The public teacher oracle instead normalizes every finite supplied slope; it expects the normal generated probe labels, which mark pinned or unprobeable coordinates absent. It does not independently mask arbitrary external slope vectors. The signed-angle/user-bound projection then clamps policy actions, and the environment applies meet-preservation projection and tries scales in order, accepting the first strictly improving feasible candidate. Default scales are 1, 1/2, and 1/4, not an optimization over all possible step lengths.

Away from max ties and the floor boundary, for active coordinates below the floor,

```text
d mu[k] / d g[l] = -B delta[k,l] / 0.05.
```

Above the floor with unique maximum coordinate `r`,

```text
d mu[k] / d g[l]
  = -B delta[k,l]/D + B g[k] sign(g[r]) delta[r,l]/D².
d g[k] / d s[k,c] = w[c].
```

Thus the composition avoids division by a vanishing predicted slope. Its scale direction is null above the floor: scaling every `g` by a positive constant preserves `mu`. Huber regression supplies slope-scale supervision that directional cosine cannot supply. Max/floor boundaries are nondifferentiable and use the backend's selected subgradient. The later action derivative is `step_limit * (1-tanh(mu)²)`, but imitation does not backpropagate through that squash or through geometric projection.

## Actual loss and masks

`training/imitation_loss.rs::imitation_terms` uses

```text
H(e) = e²/2                         if |e| <= 1
       |e| - 1/2                    otherwise
L = L_sens + cosine_coeff L_dir + gain_coeff L_gain
  = L_sens + 0.5 L_dir + 0.1 L_gain  at default coefficients.
```

`L_sens` sums `H(predicted-target)` over finite component labels on real, free, mobile tiers and divides by their count, clamped to at least one. It is an average over labelled entries, so a row with more supported components/tiers contributes more entries. Its derivative is `mask * clamp(error,-1,1) / count`. Raw sensitivity MSE is reported separately; it is not the optimized sensitivity term. `L_gain` applies the same Huber formula to finite scalar oracle gains, averaging over supported rows. Its derivative has the same form, multiplied by `gain_coeff`.

For each row, `x = direction_mask * mu` and `y = direction_mask * teacher_direction`. With `a = sqrt(x·x + 1e-12)`, `b = sqrt(y·y)`, and `q = a*b + 1e-8`, the reported cosine is `c = (x·y)/q`. A direction contributes only when its mask is nonempty and `y·y > 1e-12`. `L_dir` averages `1-c` over those rows. No supported direction means zero direction loss, not one. For a supported row,

```text
d c / d x = y/q - (x·y) b x / (a q²).
```

The direction-loss gradient is its negative divided by the supported-row count and multiplied by the mask and `cosine_coeff`. The epsilon keeps the derivative finite at `x=0`; it does not make that derivative small. Near zero prediction its norm can approach 10^6 before the composition/trunk chain rule. The global gradient norm is checked for finiteness and clipped to the configured cap (normally one) before Adam. This bounds the supplied gradient norm, not the eventual Adam parameter-step norm or the loss on the next batch.

`imitation.rs::label_rows` replaces absent/nonfinite targets by numeric zero before tensor arithmetic and sets their masks to zero. NaNs are therefore not neutralized by multiplying NaN by zero. A missing gain is masked; a measured zero gain remains a real regression target. Target directions with zero norm are omitted. Label counts are clamped above at 10^9, far above these configured batches.

Padding is excluded from attention keys, value/gain pooling, policy normalization, and all targets. A context key always remains present, and attention uses a finite -10^9 key bias. Immobile real tiers remain attention/pooling context but receive no action or label weight. Burn's padded forward and the pure Rust unpadded forward implement the same operations; floating-point kernel/reduction ordering prevents a claim of bitwise parity.

## Teacher derivative and fixes

For the candidate map at the current state, each probe changes one requested signed angle, applies the same coupled preservation projection and gates as a real step, and measures the resulting components. The denominator is requested input displacement after safe/user-bound clipping, not one selected output-coordinate displacement: projection may move several coordinates.

For feasible positive/negative probe distances `h+` and `h-`, the implemented estimate is

```text
F'(0) ≈ h-/(h++h-) * (F(+h+)-F(0))/h+
      + h+/(h++h-) * (F(0)-F(-h-))/h-.
```

These weights cancel the quadratic term, so this is the quadratic-exact nonuniform three-point derivative. Equal distances reduce to `(F(+h)-F(-h))/(2h)`. If one side is infeasible, its available counterpart supplies a one-sided quotient. A feasible projected identity has known response zero and now uses the existing current evaluation without spending another score evaluation.

The review identified and corrected these source-level consistency problems:

- A zero-width inclusive user angle bound was marked mobile, and policy projection ignored its equality interval. It could influence action normalization despite the environment preventing movement. Observation now marks it immobile, and policy projection preserves equal bounds even if a hand-built token incorrectly claims mobility. No input dimension, feature position, checkpoint schema, or parameter shape changes.
- Finite negative objective weights passed environment validation, while coefficient construction clipped them to zero and the shared scorer used them unchanged. Environment validation now rejects those inputs. Mixed sampling already generates nonnegative weights, so this does not establish a cause of failures in the reported run.
- Probes previously used `abs(probe_delta)` as the projection radius for every coupled coordinate. A clipped/custom small probe therefore used a different feasible projection domain than the real step. Probes now retain the environment's full step radius while preserving the actual requested probe displacement in the quotient.
- A feasible projected no-op was previously treated like a missing/infeasible probe side. It now provides a zero response, distinguishing blocked geometry from an unavailable measurement.

The present source also uses material critical angle as context rather than forcing pavilion coordinates toward an RI-dependent threshold. That is consistent across optimizer/retarget policy projection and the teacher's shared safe signed-angle interval. This review does not establish whether an existing executable includes that earlier change.

The teacher's probe set and observation mobility are not identical in every representable input. Probes start from cut-core's movable set and use f64 bounds, whereas observation mobility excludes current angle magnitudes below 1 degree or at least 89.5 degrees and represents bounds in f32. A very shallow authored movable tier, or a positive-width f64 interval that rounds to equal f32 endpoints, can therefore be masked from model actions while still receiving a feasible teacher probe. Exact fixed bounds produce no positive or negative probe displacement and are consistent with the new immobility rule. These remaining edge cases limit claims of universal oracle/model mask equivalence; this review does not establish their presence in the reported training data.

## What the mathematics cannot establish

This estimator is a finite-difference model of a projected, gated, partly quantized, nonconvex objective. It is not a proof that an accepted improvement exists, that a predicted direction will be feasible, or that every correct prediction lowers the score. For example, `F(-h)=F(+h)=9` and `F(0)=10` produce a zero central derivative and zero deterministic oracle direction even though both sampled sides improve. A locally flat ray-count plateau also gives zero slope until a boundary is crossed. Mixed signs, projection coupling, finite step size, and the three available search scales can reject an otherwise useful local derivative.

Finite support and masked loss protect unavailable targets; they do not identify whether the observed training set provides adequate supervision for every component or material. Huber reduces outlier influence; cosine encourages agreement; neither supplies a convergence or generalization theorem for this neural network. High no-op/rejection rates require runtime breakdowns and fixed held-out evaluations to attribute. The newly added regression tests and existing parity/derivative tests were inspected or authored as source only and were not executed under the no-build/no-test constraint. Changes on disk do not alter an already running training executable.
