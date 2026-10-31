# pleochroma-ai

Standalone reinforcement-learning environments, models, training code and inference wrappers for gemstone cut optimization and material retargeting. The crate is built independently; it is not currently connected to the desktop application.

## Modules

- `env`: deterministic greedy `OptimizerEnv` and `RetargetEnv`, shared scoring/preservation logic, measured observations and finite-difference teacher labels.
- `model`: pure Rust set-attention network, composed sensitivity policy, mobility-aware action projection, random sampler and schema-versioned checkpoints.
- `training`: Burn network twin, supervised imitation trainer and PPO support.
- `gpu_optics`: native CUDA analytical objective, bounded candidate/pose batches and spectral absorption.
- `gpu_geometry`: cached meet networks and native CUDA f64 residual batches, certified by the original CPU preservation gates.
- `inference`: task-specific optimizer/retarget wrappers and geometry-aware `score_design_at_fidelity` readout.
- `bridge`: compatibility adapter for producing an editor angle plan from a certified retarget result; it refuses a reconstruction that does not match the exact solved geometry.

## Environment behavior

Actions contain one requested angle delta per design tier. Moves are projected against an immutable meet-preservation baseline, with configurable angle scopes/bounds and coordinated mast repairs. A candidate is accepted only when it remains solvable and closed, preserves configured manufacturability constraints, and scores better. A no-light result is not a construction error.

Both environments accept `ObjectiveFidelity::{Fast, Full}`, lighting, an optional `ShapeTarget`, angle-coordinate allow-lists and per-tier bounds. Their observation has `GLOBAL_DIM = 3781` and `TIER_DIM = 384` with `OBS_LAYOUT_VERSION = 7` (the first 41 global and 26 tier entries keep their layout-6 meaning). Layout 6 corrected wheel-reference orientation, negative gear counts, cheater offsets and implicit index-zero facets. Layout 7 appends a bounded, explicit design context (see `env::design_features`): every orbit position and its authored frosting, relation programs, named-meet participants and dimensional targets per tier, the original hinge and preservation policy, the rough shape and physical stone size, and up to eight concave tools. It also appends the source and target material (see `env::material_features`): optical-character flags, principal frame, principal indices at the D/F/C lines, and the three principal absorption spectra on the scorer's 380-780 nm grid. Designs beyond those capacities are refused with `EnvError::UnsupportedDesign` rather than truncated. The seven sensitivity components are windowing, extinction, brilliance loss, yield loss, lighter-tone loss, deeper-tone loss and shape distance. Yield is omitted from normalized optical coefficients when it is unavailable; shape cost remains additive. Tone is scored using the same core path as other optical terms.

`OptimizerEnvConfig::source_material_override` and `RetargetEnvConfig::source_material_override` allow training code to provide measured source material properties. In retargeting, the configured target material still controls scoring. Scoring applies the design's crystal-axis override and physical stone width to the material before every candidate evaluation (`env::material_features::material_for_design`) and uses the authored frosted facet finishes. Observations carry the three-point absorption summaries, material path scale combined with measured stone width, scattering and target lighting as before, plus the full per-axis spectra and frame of layout 7. Zoning, inclusions, scattering transport, edge rounding and polish grades are not evaluated by the analytical objective (`material_scope_notes` lists them for a corpus). The legacy scalar scattering feature is retained, but does not make the teacher simulate scattering.

## Models and training

The network predicts per-tier component slopes and adds a learned action residual to their objective-weighted policy direction. The residual starts at zero and can learn useful moves when a central derivative cancels or a joint gradient is infeasible. Immobile tiers are masked. Schema 6 (which keeps the residual head of schema 5) requires observation layout 7; both embedding widths changed, so every older checkpoint is refused and must not be used as a warm start. The embedding layers are now most of the parameters (266,688 of the Medium tier's 367,885).

The teacher keeps derivative labels separate from behavior labels. It compares verified coordinate winners with canonicalized gradient line-search candidates; if none improves, it tries at most eight additional coordinate and paired candidates on two deterministically ranked tiers. A selected action is stored as the exact finite raw action evaluated at unit scale. An unsuccessful search has no action label; it does not teach that zero movement is optimal. All candidates retain meet-preservation and scoring gates.

Imitation trains component Huber loss plus masked Huber and cosine losses on the verified squashed action, and gain regression. Verified zero coordinates are supervised; missing actions are masked. The earlier [representation review](docs/representation-review-2026-10-26.md) records the layout-6 gaps that prompted this change. Layout 7 supplies the bounded constraint programs and spectra described above. It still uses f32 inputs, has explicit capacity limits, and represents the analytical scoring model, not arbitrary spectral path-tracing physics. Input coverage does not establish learned accuracy; that requires held-out evaluation.

Training is driven by the separate `pleochroma-trainer` package. This crate contains no catalogue database integration or desktop UI hookup. The trainer configures objective, fidelity, lighting, shape target and material conditions before constructing environments.

## Inference

`RlOptimizerModel::optimize` returns cut-core's optimizer result after endpoint scoring. `RlRetargetModel::retarget_certified` returns a `CertifiedRetargetResult` containing the proposal, materialized design, exact angles and masts, and original-relative preservation facts. The compatibility `retarget` method also carries the solved angles and masts in its proposal. `score_design_at_fidelity` measures a materialized design without rebuilding or recapturing an environment.

Rollout fidelity and endpoint/report fidelity are separate settings. The package guarantees shared scoring semantics and preservation certification; it makes no empirical claim about training speed or model quality.


## Native environment acceleration

With the `cuda` feature, `EnvExecution::new_cuda(ordinal)` owns optical and projection services
for one run. `OptimizerEnv::new_with_execution` and `RetargetEnv::new_with_execution` attach
them before baseline scoring. Existing `new` constructors remain CPU reference APIs. Restored
episodes reuse their saved measurements, then attach the service for subsequent proposals.
No accelerator object is serialized. The trainer enables these services only for explicit CUDA;
CPU and WGPU keep CPU environment scoring.

The ports copy the analytical objective and preservation equations from this repository's
MIT-licensed core implementation, retaining source references in each module. They do not
substitute beauty-rendered pixels for the training objective or add effects absent from the
CPU analytical scorer. The optics support isotropic/uniaxial/biaxial paths, authored frosting,
concave tool intersections, supported lighting and principal-axis absorption. Spectral
attenuation is GPU-batched while wavelength sampling, absorption-band evaluation, illuminant
and CIE conventions remain those of the reference. Full fidelity retains its complete tilt grid.

Named, derived and dimensional meet authoring still uses the exact CPU solver to generate
candidate planes. CUDA evaluates residual batches; small QR corrections and the final original
meet/topology gate remain CPU operations. Unsupported concave projection and explicit capacity
limits use counted CPU fallbacks. CUDA failures never silently retry on CPU. Counters identify
actual execution and fallback work instead of treating the CUDA neural-backend label as proof
that all work ran on GPU.

CUDA float arithmetic is not claimed bit-identical to the CPU reference. Recovery artifacts
record environment arithmetic provenance, and ignored device regressions cover reference
optics, batched tone and end-to-end preserved projections. These must be run on the target GPU;
no empirical correctness, throughput or utilization claim follows from source inspection alone.
