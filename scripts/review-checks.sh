#!/usr/bin/env bash
# Review checks: clippy, the whole test suite and the extra checks, run one after another.
#
# Usage (from anywhere inside the repository):
#   scripts/review-checks.sh            # every step
#   scripts/review-checks.sh 3 5        # only steps 3 and 5 (numbers as printed in the summary)
#   RUN_GPU=1 scripts/review-checks.sh  # also the steps that need a GPU adapter (the GPU tests, the
#                                       # GPU equivalence harness and the Ray-Traced kernel checks)
#   CENSUS_DB=/path/to/facet_diagrams.sqlite scripts/review-checks.sh
#                                       # also the meet census over the first 100 library designs
#                                       # (opened read-only); CENSUS_IDS=6,123 adds those entry ids
#                                       # as a second census
#
# Writes into target/review-checks/:
#   summary.txt     the file to paste back: per step the command, exit code, duration, and only the
#                   lines that matter (clippy warnings/errors, compile errors, failing tests with
#                   their panic messages, test result lines; for the harness its build id, adapter,
#                   every failing check with its detail lines, the pass/fail count and the verdict;
#                   for the census one short line per design and the percentile summary)
#   step-N.log      the full output of step N, for when the summary is not enough
#   gpu_harness_last_run.json  the harness's machine-readable run record (copy it to
#                   crates/pleochroma/docs/ when a physics change is committed)
#   census_lib100.csv, census_ids.csv  the census reports
#
# The workspace steps exclude the Android app, which has its own steps (desktop preview,
# no-default-features, cargo ndk), so a mobile compile error does not hide the rest of the
# workspace. cargo ndk needs the NDK variables. The AI and trainer crates are not workspace
# members and are tested through their own manifests.

set -u

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root" || exit 1

out_dir="$repo_root/target/review-checks"
mkdir -p "$out_dir"
summary="$out_dir/summary.txt"

# One short line per census row: key, title, capture verdict, status and the network columns.
# The CSV quotes fields that hold commas, so it is read with Python's csv module.
census_digest() {
  python3 - "$1" <<'PY'
import csv, sys
rows = [line for line in open(sys.argv[1], encoding="utf-8") if not line.startswith("#")]
wanted = ["key", "title", "gemcad_version", "mast_decimals", "capture", "census_status",
          "junctions", "faces", "valence", "split_ratio", "max_slack_ppm", "reasons"]
reader = csv.DictReader(rows)
print("census row: " + " | ".join(wanted))
counts = {}
for row in reader:
    capture = row.get("capture", "")
    verdict = "ok" if capture == "ok" else "refused"
    counts[verdict] = counts.get(verdict, 0) + 1
    print("census row: " + " | ".join((row.get(name) or "")[:90] for name in wanted))
print("census captures: " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())))
PY
}
export -f census_digest

steps=(
  "cargo clippy --workspace --exclude pleochroma-mobile-android --all-targets --all-features --message-format=short"
  "cargo ui-clippy --message-format=short"
  "cargo test --workspace --exclude pleochroma-mobile-android --all-targets --all-features --no-fail-fast --message-format=short --release"
  "cargo test --workspace --exclude pleochroma-mobile-android --all-features --doc --no-fail-fast --message-format=short"
  "cargo test --manifest-path crates/pleochroma-ai/Cargo.toml --all-targets --no-fail-fast --message-format=short"
  "cargo test --manifest-path apps/pleochroma-trainer/Cargo.toml --all-targets --no-fail-fast --message-format=short"
  "cargo clippy -p pleochroma-web -p pleochroma-web-gpu -p pleochroma-web-compute --target wasm32-unknown-unknown --message-format=short"
  "cargo clippy -p pleochroma-mobile-android --features desktop-preview --all-targets --message-format=short"
  "cargo check -p pleochroma-mobile-android --no-default-features --features desktop-preview --message-format=short"
  "cargo ndk -t arm64-v8a check -p pleochroma-mobile-android --all-targets"
  "cargo run --release -p pleochroma-cli -- census --db \"\$CENSUS_DB\" --all --limit 100 --targets sapphire --modes shift --out target/review-checks/census_lib100.csv; code=\$?; census_digest target/review-checks/census_lib100.csv; exit \$code"
  "cargo run --release -p pleochroma-cli -- census --db \"\$CENSUS_DB\" --ids \"\$CENSUS_IDS\" --targets sapphire --modes shift --out target/review-checks/census_ids.csv; code=\$?; census_digest target/review-checks/census_ids.csv; exit \$code"
  "PLEOCHROMA_REQUIRE_GPU=1 cargo test --no-fail-fast --message-format=short -p pleochroma --features gpu -- --test-threads=1 tool_check tool_tests event_wavefront::tool_tests event_wavefront::tests::ew_matches_megakernel_per_material_class"
  "PLEOCHROMA_REQUIRE_GPU=1 cargo run --profile probe -p pleochroma --features gpu --example gpu_equivalence_harness -- --json target/review-checks/gpu_harness_last_run.json --date $(date -u '+%Y-%m-%d')"
  "PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --features gpu --lib -- --test-threads=1 renderer::gpu::whitted"
)

selected=("$@")
wanted() {
  [ ${#selected[@]} -eq 0 ] && return 0
  local n
  for n in "${selected[@]}"; do [ "$n" = "$1" ] && return 0; done
  return 1
}

# Why step `$1` cannot run in this environment, or nothing when it can.
skip_reason() {
  local cmd="$1"
  if [ "${cmd#PLEOCHROMA_REQUIRE_GPU}" != "$cmd" ] && [ "${RUN_GPU:-0}" != "1" ]; then
    echo "needs a GPU; run with RUN_GPU=1"
  elif [ "${cmd#*CENSUS_IDS}" != "$cmd" ] && { [ -z "${CENSUS_DB:-}" ] || [ -z "${CENSUS_IDS:-}" ]; }; then
    echo "needs CENSUS_DB and CENSUS_IDS"
  elif [ "${cmd#*CENSUS_DB}" != "$cmd" ] && [ -z "${CENSUS_DB:-}" ]; then
    echo "needs the library; run with CENSUS_DB=/path/to/facet_diagrams.sqlite"
  fi
}

# The lines of a log that matter, in order. Clippy/check in short format puts one diagnostic per
# line (repeats across targets are dropped, float_cmp is counted per file at the end); cargo test prints failing tests as "---- name stdout ----" blocks followed by "failures:".
# With a second argument "harness", the GPU equivalence harness's own report is read as well: it
# prints one "[Group] label ... PASS/FAIL" line per check, indented detail lines under it, and a
# final "gpu_equivalence_harness: ..." verdict. With "census", the census digest lines and the
# census's own summary (everything after its first line that starts with "Census Summary") pass.
extract() {
  awk -v mode="${2:-}" '
    mode == "census" && /^census (row|captures): / { print; next }
    mode == "census" && /^Census Summary/ { insummary = 1 }
    mode == "census" && insummary { print; next }
    mode == "harness" && /^\[[^]]+\] .*FAIL/ { hfail++; indetail = 1; detail = 0; print; next }
    mode == "harness" && /^\[[^]]+\] .*PASS/ { hpass++; indetail = 0; next }
    mode == "harness" && indetail && /^  / { if (detail++ < 12) print; next }
    mode == "harness" { indetail = 0 }
    mode == "harness" && /^(pleochroma::BUILD_ID = |adapter: )/ { print; next }
    mode == "harness" && /^gpu_equivalence_harness: / { print substr($0, 1, 200); next }
    /^---- .* stdout ----$/ { inblock = 1; lines = 0; body = 0; print; next }
    inblock {
      if ($0 ~ /^$/) { if (body) inblock = 0; next }
      body = 1
      if (lines++ < 40) print
      next
    }
    /^[^ ].*\.rs:[0-9]+:[0-9]+: warning: strict comparison of `f(32|64)` or `f(32|64)`/ {
      if (!seen[$0]++) { split($0, part, ":"); fc[part[1]]++ }
      next
    }
    /^[^ ].*\.rs:[0-9]+:[0-9]+: (warning|error)/ { if (!seen[$0]++) print; next }
    /^(error|warning)(\[[A-Z0-9]+\])?: / && !/generated [0-9]+ warning/ { if (!seen[$0]++) print; next }
    /^test result: / { infail = 0; print; next }
    /^    [a-zA-Z_:0-9]+$/ && infail { print; next }
    /^failures:$/ { infail = 1; print; next }
    /^test .* \.\.\. FAILED$/ { print; next }
    /panicked at/ { print; next }
    /Running / { print; next }
    /Doc-tests / { print; next }
    END {
      for (f in fc) printf "%s: warning: float_cmp x%d (grouped)\n", f, fc[f]
      if (mode == "harness" && hpass + hfail > 0) printf "harness checks: %d passed, %d failed\n", hpass, hfail
    }
  ' "$1"
}

{
  echo "review-checks summary"
  echo "date: $(date -u '+%Y-%m-%d %H:%M:%S UTC')"
  echo "host: $(uname -srm)"
  echo "rustc: $(rustc --version 2>/dev/null)"
  echo "repo: $repo_root"
  echo
} > "$summary"

overall=0
for i in "${!steps[@]}"; do
  n=$((i + 1))
  wanted "$n" || continue
  cmd="${steps[$i]}"
  log="$out_dir/step-$n.log"
  why="$(skip_reason "$cmd")"
  if [ -n "$why" ]; then
    echo ">>> step $n: skipped ($why)"
    { echo "=================================================================="; echo "step $n: skipped, $why: $cmd"; echo; } >> "$summary"
    continue
  fi
  echo ">>> step $n: $cmd"
  start=$(date +%s)
  bash -c "$cmd" > "$log" 2>&1
  code=$?
  secs=$(( $(date +%s) - start ))
  [ $code -ne 0 ] && overall=1
  {
    echo "=================================================================="
    echo "step $n: $cmd"
    echo "exit $code after ${secs}s (full log: target/review-checks/step-$n.log)"
    echo "------------------------------------------------------------------"
    mode=""
    [ "${cmd#*gpu_equivalence_harness}" != "$cmd" ] && mode="harness"
    [ "${cmd#*census_digest}" != "$cmd" ] && mode="census"
    extract "$log" "$mode" > "$out_dir/step-$n.extract"
    head -n 1500 "$out_dir/step-$n.extract"
    if [ "$(wc -l < "$out_dir/step-$n.extract")" -gt 1500 ]; then
      echo "... more lines in target/review-checks/step-$n.log"
    fi
    echo
  } >> "$summary"
  echo "    exit $code after ${secs}s"
done

echo
echo "summary: $summary"
exit $overall
