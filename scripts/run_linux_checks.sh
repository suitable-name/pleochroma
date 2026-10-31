#!/usr/bin/env bash
# Runs the general (Linux) commands of the 2026-10-26 campaign in one go and logs everything.
#
# Usage (from anywhere):
#   bash scripts/run_linux_checks.sh [--repo DIR] [--skip-gpu] [--skip-bench] [--jobs N] [--ramdisk DIR]
#
#   --repo DIR     repository root (default: the current directory)
#   --skip-gpu     skip the three GPU commands (they need an adapter)
#   --skip-bench   leave out the measurements (benches + live_render_bench), which otherwise run
#                  one after another once the parallel part has finished
#   --jobs N       how many steps of the parallel phase run at once (default 8)
#   --ramdisk DIR  target folder root of the two big workspace builds (default /mnt/ramdisk/target;
#                  when its parent does not exist they build under <repo>/target/checks instead)
#
# Prerequisite (not run here): rustup target add wasm32-unknown-unknown; trunk on PATH.
#
# Order: up to --jobs steps run at once, the release workspace test (the long one) started
# first. The UI runs and the GPU steps each form one chain
# that runs its steps one after another (they share the display, the GPU and the harness output
# folder, and the GPU diagnosis steps gate on the step before them). The measurements run last,
# alone, once everything else has finished.
#
# Target folders: cargo locks a target folder for the whole build, so steps sharing one wait for
# each other. Every step (or chain) therefore builds in a folder of its own:
#   test-workspace, clippy-workspace   <ramdisk>/<step>            (the two big workspace builds)
#   every other parallel step          <repo>/target/checks/<step>
#   the UI chain, the GPU chain        <repo>/target/checks/ui, <repo>/target/checks/gpu
#   the measurements                   <repo>/target                (alone, the usual folder)
# Each folder is a full build of its own: expect several GB per folder on the first run, and
# incremental rebuilds afterwards.
#
# Output: <repo>/owner-run-logs/<timestamp>/
#   NN-<name>.log   full output of each command
#   summary.md      exit code, verdict and duration per command, in step order (send me this folder)
# Every command runs even when an earlier one failed.

set -u

REPO="$(pwd)"
SKIP_GPU=0
WITH_BENCH=1
MAX_JOBS=8
RAMDISK=/mnt/ramdisk/target
while [ $# -gt 0 ]; do
    case "$1" in
        --repo) REPO="$2"; shift 2 ;;
        --skip-gpu) SKIP_GPU=1; shift ;;
        --skip-bench) WITH_BENCH=0; shift ;;
        # The old opt-in switch; the measurements run by default now.
        --with-bench) WITH_BENCH=1; shift ;;
        --jobs) MAX_JOBS="$2"; shift 2 ;;
        --ramdisk) RAMDISK="$2"; shift 2 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

cd "$REPO" || { echo "no repo at $REPO" >&2; exit 2; }
# rustc crashed with SIGSEGV compiling pleochroma-cut for the UI harness (2026-10-27_112759) and
# suggested a larger stack; 16 MiB, unless the caller already set one.
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"
STAMP="$(date +%Y-%m-%d_%H%M%S)"
OUT="$REPO/owner-run-logs/$STAMP"
mkdir -p "$OUT"
SUMMARY="$OUT/summary.md"
INDEX=0
TOTAL=0
# The exit code of the last step `run` ran (0 in the counting pass); `run_if_failed` gates on it.
LAST_CODE=0
# DRY=1: the first pass over all_steps only counts the steps, so every line can say [xx/yy].
DRY=0
# BACKGROUND=1: `run` starts its step in the background (at most MAX_JOBS at once) and returns.
BACKGROUND=0
# The target folder of the next `run`; empty means <repo>/target/checks/<step name>.
STEP_TARGET=""
CHECKS_TARGET="$REPO/target/checks"
if [ ! -d "$(dirname "$RAMDISK")" ]; then
    echo "no $(dirname "$RAMDISK"): the workspace builds use $CHECKS_TARGET instead" >&2
    RAMDISK="$CHECKS_TARGET"
fi
# One summary row per step lands here as <index>.row; summary.md gets them in step order at the end.
ROWS="$OUT/.rows"
mkdir -p "$ROWS"

{
    echo "# Linux run $STAMP"
    echo
    echo "Repo: $REPO  (git: $(git rev-parse --short HEAD 2>/dev/null || echo n/a))"
    echo
    echo "| Step | Name | Exit | Verdict | Minutes | Command |"
    echo "|---|---|---|---|---|---|"
} > "$SUMMARY"

# run_step INDEX NAME DISPLAY CMD... : runs CMD from the repo root, logs it, writes its summary
# row, and returns CMD's exit code.
run_step() {
    local index="$1" name="$2" display="$3"
    shift 3
    local log tag target
    log="$OUT/$(printf '%02d' "$index")-$name.log"
    target="${STEP_TARGET:-$CHECKS_TARGET/$name}"
    tag="$(printf '[%02d/%02d]' "$index" "$TOTAL")"
    echo "$tag started: $display  (target: $target)"
    local start end code verdict minutes
    start=$(date +%s)
    mkdir -p "$target"
    ( cd "$REPO" && export CARGO_TARGET_DIR="$target" && echo "CARGO_TARGET_DIR=$target" && "$@" ) > "$log" 2>&1
    code=$?
    end=$(date +%s)
    # Exit 77 means the step skipped itself (a missing prerequisite), not a failure.
    if [ "$code" -eq 0 ]; then verdict="ok"; elif [ "$code" -eq 77 ]; then verdict="skipped"; else verdict="FAILED"; fi
    minutes="$(awk "BEGIN { printf \"%.1f\", ($end - $start) / 60 }")"
    echo "$tag $verdict ($minutes min): $name"
    # The folder is made again here: a step that ends after its run folder was moved or deleted
    # (2026-10-29_163229) must not fail on the missing row folder.
    mkdir -p "$ROWS"
    printf '| %s | %s | %d | %s | %s | `%s` |\n' "$tag" "$name" "$code" "$verdict" \
        "$minutes" "$display" > "$ROWS/$(printf '%03d' "$index").row"
    return "$code"
}

# Ctrl+C (or a TERM to this script) stops the whole run: every running step and the steps not
# started yet. Without it the script went on starting the remaining steps after a stop, and each of
# them failed at once (2026-10-29_165635: 30 steps with exit 137 in 0.0 min).
on_stop() {
    trap - INT TERM
    echo "stopped: ending every running step; no further steps start" >&2
    kill 0 2>/dev/null
    exit 130
}
trap on_stop INT TERM

# Blocks until fewer than MAX_JOBS background steps are running.
wait_for_slot() {
    while [ "$(jobs -rp | wc -l)" -ge "$MAX_JOBS" ]; do
        wait -n 2>/dev/null || true
    done
}

# run NAME DISPLAY CMD... : one step. In the background phase it is started and `run` returns at
# once; otherwise it runs to the end and LAST_CODE holds its exit code.
run() {
    local name="$1" display="$2"
    shift 2
    INDEX=$((INDEX + 1))
    if [ "$DRY" -eq 1 ]; then LAST_CODE=0; return 0; fi
    if [ "$BACKGROUND" -eq 1 ]; then
        wait_for_slot
        run_step "$INDEX" "$name" "$display" "$@" &
        LAST_CODE=0
    else
        run_step "$INDEX" "$name" "$display" "$@"
        LAST_CODE=$?
    fi
}

# chain FUNCTION : runs FUNCTION's steps one after another in one background slot (so
# `run_if_failed` inside it sees the step before it). The steps keep the indices they would get
# inline: FUNCTION is counted first, and the parent moves INDEX past them.
chain() {
    local fn="$1" start="$INDEX" dry="$DRY" target="$STEP_TARGET" count
    DRY=1
    "$fn"
    count=$((INDEX - start))
    DRY="$dry"
    # FUNCTION sets its own STEP_TARGET; the counting pass must not leak it to later steps.
    STEP_TARGET="$target"
    INDEX="$start"
    if [ "$DRY" -eq 1 ]; then INDEX=$((start + count)); return 0; fi
    wait_for_slot
    ( BACKGROUND=0; "$fn" ) &
    INDEX=$((start + count))
}

# run_if_failed GATE NAME DISPLAY CMD... : `run` CMD only when GATE (an earlier step's exit code)
# is not 0; otherwise the step is recorded as skipped (exit 77), so the [xx/yy] count stays fixed.
run_if_failed() {
    local gate="$1"
    shift
    if [ "$DRY" -eq 1 ] || [ "$gate" -ne 0 ]; then
        run "$@"
    else
        local name="$1" display="$2"
        run "$name" "$display (skipped: the gating step passed)" \
            bash -c 'echo "skipped: the gating step passed on this machine"; exit 77'
    fi
}

# The GPU driver this run uses (proves which Mesa build is active), Vulkan and GL.
gpu_driver_info() {
    if command -v vulkaninfo >/dev/null 2>&1; then
        vulkaninfo --summary 2>&1 | grep -iE "deviceName|driverName|driverInfo|driverVersion|apiVersion"
    else
        echo "vulkaninfo not installed (package vulkan-tools)"
    fi
    if command -v glxinfo >/dev/null 2>&1; then
        glxinfo -B 2>&1 | grep -iE "OpenGL renderer|OpenGL core profile version|OpenGL version"
    else
        echo "glxinfo not installed (package mesa-progs / mesa-utils)"
    fi
    return 0
}

# rusqlite must NOT be in the browser build: `cargo tree -i` fails with "did not match".
check_no_rusqlite() {
    local text
    text="$(cargo tree -p pleochroma-web --target wasm32-unknown-unknown -i rusqlite 2>&1)"
    echo "$text"
    if echo "$text" | grep -q "did not match"; then
        echo "OK: rusqlite is not in the browser build"
        return 0
    fi
    echo "PROBLEM: rusqlite reaches the browser build (see the tree above)"
    return 1
}

# cargo ui-harness, then its report and any crash dumps into the log folder. The harness runs
# scenarios in worker processes and replays each worker's output only when that scenario's turn
# comes, so a crashed worker (2026-10-27_165259: "worker 8 exited with signal: 11 (SIGSEGV)")
# shows up in the log without its scenario; the scenario name and the worker's last output lines
# are the `worker-crash` finding in target/ui-harness/report.md, which the next harness run
# overwrites. The core dumps of this step (systemd-coredump) carry the native backtrace.
run_ui_harness() {
    local since code
    since="$(date '+%Y-%m-%d %H:%M:%S')"
    cargo ui-harness "$@"
    code=$?
    # The harness writes its report into the target folder of this step (CARGO_TARGET_DIR).
    local reports="${CARGO_TARGET_DIR:-target}/ui-harness"
    cp -f "$reports/report.md" "$OUT/ui-harness-report.md" 2>/dev/null \
        && echo "copied $reports/report.md to $OUT/ui-harness-report.md"
    cp -f "$reports/report.json" "$OUT/ui-harness-report.json" 2>/dev/null
    if [ -f "$OUT/ui-harness-report.md" ]; then
        echo "--- worker-crash findings:"
        grep -n "worker-crash" "$OUT/ui-harness-report.md" || echo "(none)"
    fi
    if command -v coredumpctl >/dev/null 2>&1; then
        coredumpctl --no-pager --since="$since" list > "$OUT/ui-harness-coredumps.txt" 2>&1
        coredumpctl --no-pager --since="$since" info >> "$OUT/ui-harness-coredumps.txt" 2>&1
        echo "core dumps since $since: see $OUT/ui-harness-coredumps.txt"
    else
        echo "coredumpctl not found: no backtraces of crashed workers collected"
    fi
    return "$code"
}

# The UI runs, one after another: they share the display and one target folder.
ui_steps() {
STEP_TARGET="$CHECKS_TARGET/ui"
# Passed in run 2026-10-29_203237 and untouched by later changes; uncomment for the next full run.
# run ui-static "cargo ui-static" cargo ui-static
# Targeted: only the Variants lesson, whose driver changed after 2026-10-29_203237 (the full harness passed there apart from it). Drop the filter for a full run.
run ui-harness "cargo ui-harness -- tutorial:solving-variants (+ report.md and core dumps into this folder)" run_ui_harness -- tutorial:solving-variants
# ui-screens photographs a real library: PLEOCHROMA_HARNESS_LIBRARY names its facet_diagrams.sqlite
# (copied into a throw-away profile). Without it the step skips itself (exit 77) instead of failing.
# Skipped in every run that does not set PLEOCHROMA_HARNESS_LIBRARY; uncomment (and set it) to rewrite the screenshots.
# run ui-screens "cargo ui-screens (needs PLEOCHROMA_HARNESS_LIBRARY)" \
#     bash -c 'if [ -z "${PLEOCHROMA_HARNESS_LIBRARY:-}" ]; then echo "skipped: PLEOCHROMA_HARNESS_LIBRARY is not set"; exit 77; fi; cargo ui-screens'
# Targeted: the three lessons whose debug readbacks changed after 2026-10-29_203237 (all lessons passed there). Drop the ids for a full run.
run tutorial-videos "PLEOCHROMA_VIDEOS_DEBUG=1 cargo tutorial-videos -- <3 lessons>" \
    env PLEOCHROMA_VIDEOS_DEBUG=1 cargo tutorial-videos -- solving-verdict-fixes tiers-cheater-offset viewing-optical-diagnostics
}

# Every step, in order. Called twice: once with DRY=1 to count them, once to run them.
all_steps() {
# Everything below up to the measurements runs in parallel, at most MAX_JOBS steps at once.
BACKGROUND=1
# A0 the release workspace test, started first since it is the longest step.
# ACO_DEBUG=nosched is the app's RADV workaround (the 2026-10-28 scheduler fault); the GPU tests
# inside this run would otherwise hit the stock driver's miscompile. Other drivers ignore it.
# --no-fail-fast: one failing crate must not hide the results of the others.
# Passed in run 2026-10-29_203237 and untouched by later changes; uncomment for the next full run.
# STEP_TARGET="$RAMDISK/test-workspace"
# run test-workspace "ACO_DEBUG=nosched cargo test --workspace --all-features --all-targets --no-fail-fast --release" \
    # env ACO_DEBUG=nosched cargo test --workspace --all-features --all-targets --no-fail-fast --release
# A1 lint and build checks
# STEP_TARGET="$RAMDISK/clippy-workspace"
# run clippy-workspace "cargo clippy --workspace --all-features --all-targets" \
    # cargo clippy --workspace --all-features --all-targets
# Every later parallel step: a folder of its own under target/checks.
STEP_TARGET=""
# The UI harness, screenshot and video targets need the `ui-harness` feature, which a workspace
# `--all-targets` run does not reach.
run clippy-ui "cargo ui-clippy" cargo ui-clippy
# Passed in run 2026-10-29_203237 and untouched by later changes; uncomment for the next full run.
# run clippy-wasm "cargo clippy --target wasm32-unknown-unknown -p pleochroma-web -p pleochroma-web-compute -p pleochroma-web-gpu -p pleochroma-web-core" \
    # cargo clippy --target wasm32-unknown-unknown -p pleochroma-web -p pleochroma-web-compute -p pleochroma-web-gpu -p pleochroma-web-core
# run vault-wasm-check "cargo check -p pleochroma-vault --no-default-features --target wasm32-unknown-unknown" \
    # cargo check -p pleochroma-vault --no-default-features --target wasm32-unknown-unknown
# run no-rusqlite-in-web "cargo tree -p pleochroma-web --target wasm32-unknown-unknown -i rusqlite (expect: did not match)" \
    # check_no_rusqlite

# A2 tests
# Passed in run 2026-10-29_203237 and untouched by later changes; uncomment for the next full run.
# run test-doc "cargo test --workspace --doc" \
    # cargo test --workspace --doc
# run test-vault-no-sqlite "cargo test -p pleochroma-vault --no-default-features" \
    # cargo test -p pleochroma-vault --no-default-features
# Verified in run 2026-10-29_170825 and not needed again; uncomment to run it once more.
# run rich-quartz "cargo test -p pleochroma-cut-core rich_quartz -- --nocapture" \
#     cargo test -p pleochroma-cut-core rich_quartz -- --nocapture
# Targeted runs of this campaign's new tests (already inside test-workspace; separate logs make a
# failure easy to find): browser lesson offer and wording, Next's executor, the browser lesson
# event/highlight guards, browser GPU admission (reachable variant, kernel lists, compile limits).
# Verified in run 2026-10-29_170825 and not needed again; uncomment to run it once more.
# run web-offer "cargo test -p pleochroma-editor guide::web_offer"     cargo test -p pleochroma-editor guide::web_offer
# run guide-perform "cargo test -p pleochroma-web-core guide_perform"     cargo test -p pleochroma-web-core guide_perform
# run guide-targets "cargo test -p pleochroma-web-core --test guide_targets"     cargo test -p pleochroma-web-core --test guide_targets
# run gpu-admission "cargo test -p pleochroma-web-core gpu_admission"     cargo test -p pleochroma-web-core gpu_admission
# run frosted-optimizer-ignored "cargo test -p pleochroma-cut-core --release -- --ignored an_optimizer_run_on_a_frosted_design" \
#     cargo test -p pleochroma-cut-core --release -- --ignored an_optimizer_run_on_a_frosted_design
# Color variants campaign (2026-10-27), already inside test-workspace; one separate log for its
# tests (one cargo call, so one build): the variant data and per-axis built-ins, the keyed color
# list and per-axis editor, the design snapshot and `.psdd`, patterns, the vault store, the Live
# Render color, web and mobile. A pinned face-up color that moved by more than dE2000 1.0 names
# the entry and its fit record.
# Verified in run 2026-10-29_170825 and not needed again; uncomment to run it once more.
# COLOR_FILTERS="variants built_in_hues color_entry lch_color variant_choice color_variant pattern render_body_color body_color_combo render_color materials_symmetry scene_identity design_color design_props pattern_wire"
# run color-variants-tests "cargo test -p pleochroma -p pleochroma-editor -p pleochroma-cut-core -p pleochroma-cut -p pleochroma-web-core -p pleochroma-mobile-core -p pleochroma-vault --lib -- <color filters>" \
#     bash -c "cargo test -p pleochroma -p pleochroma-editor -p pleochroma-cut-core -p pleochroma-cut -p pleochroma-web-core -p pleochroma-mobile-core -p pleochroma-vault --lib -- $COLOR_FILTERS"

# A3 UI runs (one chain)
chain ui_steps

# A4 browser build
# Passed in run 2026-10-29_203237 and untouched by later changes; uncomment for the next full run.
# run trunk-release "(cd apps/pleochroma-web && trunk build --release)" \
    # bash -c 'cd apps/pleochroma-web && trunk build --release'

# A5 GPU (one chain)
# Passed in run 2026-10-29_203237 and untouched by later changes; uncomment for the next full run.
# if [ "$SKIP_GPU" -eq 0 ]; then
    # chain gpu_steps
# fi

# The parallel phase ends here: the measurements need a quiet machine.
if [ "$DRY" -eq 0 ]; then wait; fi
BACKGROUND=0
STEP_TARGET="$REPO/target"

# A7 measurements (quiet machine only)
# Passed in run 2026-10-29_203237 and untouched by later changes; uncomment for the next full run.
# if [ "$WITH_BENCH" -eq 1 ]; then
    # run bench-retarget-refit "cargo bench -p pleochroma-editor --bench retarget_refit" \
        # cargo bench -p pleochroma-editor --bench retarget_refit
    # run bench-history-snapshot "cargo bench -p pleochroma-editor --bench history_snapshot" \
        # cargo bench -p pleochroma-editor --bench history_snapshot
    # run bench-guide-copy "cargo bench -p pleochroma --bench guide_copy" \
        # cargo bench -p pleochroma --bench guide_copy
    # run bench-diagnostic-image "cargo bench -p pleochroma --bench diagnostic_image" \
        # cargo bench -p pleochroma --bench diagnostic_image
    # run live-render-bench "cargo run --profile probe -p pleochroma-cut --features gpu,render-bench --example live_render_bench -- ... --json $OUT/perf-gpu-base.json" \
        # cargo run --profile probe -p pleochroma-cut --features gpu,render-bench --example live_render_bench -- \
            # --engines path --backend gpu --resolutions 1920x1080 --path-targets 128 --denoise off \
            # --passes 6 --repeats 3 --json "$OUT/perf-gpu-base.json"
    # The opt-in GPU timers (perf lane GT, MEASURE_GPU.md P24/H04): where the ~50-60 ms fixed cost
    # of a pass goes. Two sizes in one run, so the fixed part can be compared; read pass 2 onwards.
    # run live-render-bench-timing "PLEOCHROMA_GPU_TIMING=1 cargo run --profile probe -p pleochroma-cut --features gpu,render-bench --example live_render_bench -- ... --json $OUT/perf-gpu-timing.json" \
        # env PLEOCHROMA_GPU_TIMING=1 cargo run --profile probe -p pleochroma-cut --features gpu,render-bench --example live_render_bench -- \
            # --engines path --backend gpu --resolutions 800x600,1920x1080 --path-targets 128 --denoise off \
            # --passes 6 --repeats 3 --json "$OUT/perf-gpu-timing.json"
# fi

}

# The GPU steps, one after another: they share the adapter, and the diagnosis steps gate on
# the step before them (run_if_failed).
gpu_steps() {
    STEP_TARGET="$CHECKS_TARGET/gpu"
    run gpu-driver-info "vulkaninfo --summary / glxinfo -B (the Mesa build in use)" gpu_driver_info
    # The stock-driver run fails on the known RADV scheduler fault (2026-10-28); the run with the workaround below is the check. Uncomment to see whether a new Mesa fixed it.
    # run gpu-equivalence-harness "PLEOCHROMA_REQUIRE_GPU=1 cargo run --profile probe -p pleochroma --features gpu --example gpu_equivalence_harness" \
    #     env PLEOCHROMA_REQUIRE_GPU=1 cargo run --profile probe -p pleochroma --features gpu --example gpu_equivalence_harness
    # EQUIVALENCE_CODE=$LAST_CODE
    # The RADV scheduler fault (2026-10-28): the stock driver fails the zoned and all-features
    # GENERIC comparisons, ACO_DEBUG=nosched (the app's RADV workaround) passes them all.
    run gpu-equivalence-harness-aco-nosched "ACO_DEBUG=nosched RADV_DEBUG=nocache PLEOCHROMA_REQUIRE_GPU=1 cargo run --profile probe -p pleochroma --features gpu --example gpu_equivalence_harness" \
        env ACO_DEBUG=nosched RADV_DEBUG=nocache PLEOCHROMA_REQUIRE_GPU=1 cargo run --profile probe -p pleochroma --features gpu --example gpu_equivalence_harness
    # Already inside the release workspace test (all features, on the server's GPU); uncomment for a separate log.
    # run gpu-ew-tests "PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --features gpu --release ew_ -- --nocapture" \
    #     env PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --features gpu --release ew_ -- --nocapture
    # run gpu-hardware-tests "PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --features gpu -- gpu_hardware_tests" \
    #     env PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --features gpu -- gpu_hardware_tests
    # Zoned-opal GPU-vs-CPU gap (reviews/FABLE_zoned_opal.md section 7): the zoned GPU tests with
    # their printed ratios, the ignored six-row diagnostic, and the same zoned tests on the GL
    # backend (a different driver compile of the same WGSL: compiler vs shader text).
    # The zoned GPU tests: the stock driver fails them (known RADV fault) and the release workspace test runs them with ACO_DEBUG=nosched.
    # run zoned-gpu-tests "PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --all-features --lib gpu_backend::zoned_tests -- --nocapture" \
    #     env PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --all-features --lib gpu_backend::zoned_tests -- --nocapture
    # ZONED_CODE=$LAST_CODE
    # Verified in run 2026-10-29_170825 and not needed again; uncomment to run it once more.
    # run zoned-opal-diagnosis "PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --all-features --lib zoned_opal_gap_diagnosis -- --ignored --nocapture" \
    #     env PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --all-features --lib zoned_opal_gap_diagnosis -- --ignored --nocapture
    # Driver check (2026-10-27): Mesa 26.2.x ships an ACO miscompile (mesa/mesa#16618, fixed by
    # !45419 on 2026-10-27: the post-RA optimizer folds an exec copy into a tied-def instruction
    # and drops a nested if's condition; 26.0.8 is fine, RDNA2-4). The owner's server runs Mesa
    # 26.2.5 with !45419 patched in since 2026-10-27, so these diagnosis steps (and the bisect
    # below) run only when zoned-gpu-tests above still FAILS; otherwise they are skipped.
    # ACO_DEBUG=noopt skips both ACO optimizer passes (pre-RA `optimize` and `optimize_postRA`)
    # and ACO_DEBUG=nosched the schedulers; RADV_DEBUG=nocache keeps the on-disk shader cache
    # from answering with an earlier compile. A pass under noopt (and a fail without it) puts the
    # fault in the driver's optimizer, not in the shader text.
    # Verified in run 2026-10-29_170825 and not needed again; uncomment to run it once more.
    # run_if_failed "$ZONED_CODE" zoned-gpu-tests-aco-noopt "ACO_DEBUG=noopt RADV_DEBUG=nocache PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --all-features --lib gpu_backend::zoned_tests -- --nocapture" \
    #     env ACO_DEBUG=noopt RADV_DEBUG=nocache PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --all-features --lib gpu_backend::zoned_tests -- --nocapture
    # run_if_failed "$ZONED_CODE" zoned-opal-diagnosis-aco-noopt "ACO_DEBUG=noopt RADV_DEBUG=nocache PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --all-features --lib zoned_opal_gap_diagnosis -- --ignored --nocapture" \
    #     env ACO_DEBUG=noopt RADV_DEBUG=nocache PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --all-features --lib zoned_opal_gap_diagnosis -- --ignored --nocapture
    # Covered by the release workspace test, which runs with ACO_DEBUG=nosched.
    # run_if_failed "$ZONED_CODE" zoned-gpu-tests-aco-nosched "ACO_DEBUG=nosched RADV_DEBUG=nocache PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --all-features --lib gpu_backend::zoned_tests -- --nocapture" \
    #     env ACO_DEBUG=nosched RADV_DEBUG=nocache PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --all-features --lib gpu_backend::zoned_tests -- --nocapture
    # Verified in run 2026-10-29_170825 and not needed again; uncomment to run it once more.
    # run zoned-gpu-tests-gl "WGPU_BACKEND=gl PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --all-features --lib gpu_backend::zoned_tests -- --nocapture" \
    #     env WGPU_BACKEND=gl PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --all-features --lib gpu_backend::zoned_tests -- --nocapture
    # Bisect of the zoned-opal gap (2026-10-27): on RADV a zoned host with an inert structure
    # (coverage 0) is ~14 % bright while the plain zoned stone matches. Each key rebuilds the
    # shaders with one route switched off (crates/pleochroma/build.rs BISECT_PATCHES); read the
    # "A/plain" GPU row: 1.000 under a key means that route holds the fault. The kernel-set hash
    # moves in these builds, which is expected; the next normal build restores the shaders.
    # Verified in run 2026-10-29_170825 and not needed again; uncomment to run it once more.
    # for key in bragg_zoned ori_zoned bragg; do
    #     run_if_failed "$ZONED_CODE" "zoned-opal-bisect-$key" "PLEOCHROMA_WGSL_BISECT=$key PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --all-features --lib zoned_opal_gap_diagnosis -- --ignored --nocapture" \
    #         env PLEOCHROMA_WGSL_BISECT="$key" PLEOCHROMA_REQUIRE_GPU=1 cargo test -p pleochroma --all-features --lib zoned_opal_gap_diagnosis -- --ignored --nocapture
    # done
}

DRY=1
INDEX=0
all_steps
TOTAL=$INDEX
DRY=0
INDEX=0
all_steps
wait

# The rows in step order (the parallel steps finish in any order).
cat "$ROWS"/*.row >> "$SUMMARY" 2>/dev/null
rm -rf "$ROWS"

{
    echo
    echo "Not scripted: browser manual checks (OWNER_COMMANDS.md A4.16), browser GPU probes (A6),"
    echo "frosted noise hand test (A7.27), Windows pins (run_windows_pins.ps1), figure re-capture."
    echo "Tutorial debug files: videos/debug/ (transfer with this folder)."
} >> "$SUMMARY"

echo "Done. Summary: $SUMMARY"
