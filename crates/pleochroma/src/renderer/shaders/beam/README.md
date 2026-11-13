Beam-tracing WGSL units (beam guide §14.7). Patched or concatenated at run time.

- `branch_sig.wgsl` is also appended to the path cache's completion unit
  (`renderer/gpu/path_cache/source.rs`), so it ships in every `gpu` build and must not move or be
  renamed.
- The other files are included only with the off-by-default `beam` feature (and `gpu`).
- Their tests compile only with `RUSTFLAGS="--cfg beam_tests"` (`scripts/run_beam_tests.sh`).
