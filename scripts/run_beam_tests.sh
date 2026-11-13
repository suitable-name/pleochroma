#!/usr/bin/env bash
# Beam tracing is retired: the code compiles with the `beam` features, and its tests compile only
# with --cfg beam_tests. This script is the one place they run, in their own target dir
# (target/beam-tests), so the RUSTFLAGS rebuild leaves the regular cache alone. The first run
# rebuilds every dependency (tens of GB). The tests run on request only: run_full_round.sh runs
# them as part beam-tests (--with-beam); part beam-check runs only steps 00 and 00c.
#
# Usage (from the repository root, or with --repo):
#   bash scripts/run_beam_tests.sh [--repo DIR] [--steps LIST] [--timeout DURATION]
#                                  [--stall SECONDS] [--hang-repro N] [--with-long] [--cost]
#                                  [--with-ui] [--no-clippy]
#
#   --repo DIR          repository root (default: the current directory)
#   --steps LIST        run only the steps whose number is in the comma list (for example
#                       `01,02,02a`); default: every step
#   --timeout DURATION  hard stop per step that sets none of its own (default 45m; `timeout` syntax)
#   --stall SECONDS     a step whose log gets no new line for this long is stalled: the states of
#                       all its threads go to NN-<name>.stall.txt and the step is killed (default
#                       1200; the module test steps set 300, their test binary is built before)
#   --hang-repro N      also run step 02r N times: GPU beam tests mixed with CPU ones, every
#                       GPU test creating its own instance at once, the test gate off, to catch a
#                       stall with its thread states (default 0: not run)
#   --with-long         also run the long beam equivalence rows (step 09: every ignored
#                       renderer::beam:: row, single-threaded, quick-release; hours)
#   --cost              also run the two beam cost diagnostics (step 10; the budgets come from
#                       PLEOCHROMA_BEAM_COST_BUDGET_S and PLEOCHROMA_BEAM_BUILD_BUDGET_S when set)
#   --with-ui           also run the cut UI harness with the beam states (step 11)
#   --no-clippy         leave out the two clippy steps (00 and 00c)
#
# Steps (each runs even when an earlier one failed; every cargo command runs with
# RUSTFLAGS="... --cfg beam_tests" and CARGO_TARGET_DIR=target/beam-tests):
#   00  clippy on the whole workspace with all features: the beam build with the cfg
#   00c clippy on the cut UI test targets (ui_harness, ui_static, ui_screens, tutorial_videos)
#   01  the pleochroma test binary is built (its own step: cargo prints nothing while it compiles)
#   02  the beam CPU tests of pleochroma (modules, accel settings, path cache beam pins, key,
#       reference tracer, tray shadow, adaptive offset); no GPU is acquired
#   02a GPU only, four at a time: the GPU beam modules and the backend's beam reference
#   02r (--hang-repro N) the hang mix, N times
#   03  the ignored beam rows that took over 60 s in the workspace run (71 CPU rows)
#   03g the ignored GPU beam row
#   03b the other ignored beam rows: the leaf cache, cube reflections, two-bounce budget and
#       smoke rows
#   04  pleochroma-cut: beam viewport, export worker, remote tiles, preferences, output defaults,
#       render jobs, ETA (its test binary is built first, in the row 04-build-cut-test-binary);
#       04b its ignored rows
#   05  pleochroma-worker: beam tile requests and the render core
#   06  pleochroma-cli: the beam flags, render_job, help
#   07  pleochroma-net: the beam tile messages
#   08  pleochroma-render-jobs: the beam settings range check
#   09  (--with-long) the long beam equivalence rows
#   10  (--cost) the beam cost diagnostics
#   11  (--with-ui) the cut UI harness with the beam states
#
# A cargo test step that ran no test at all (a filter that matches nothing exits 0) is listed as
# NO TESTS RAN, and the script then exits 1 (not checked for step 11: the UI harness has its own
# report and exit code).
#
# Output: <repo>/owner-run-logs/beam-tests-<timestamp>/
#   NN-<name>.log        full output per step
#   NN-<name>.stall.txt  only for a stalled step: memory, GPU, and per thread its state, kernel wait
#                        channel and system call (plus gdb backtraces when gdb is installed)
#   summary.md           exit code and minutes per step (send me this folder)

set -u

# Headless boxes have no session runtime dir; libwayland prints an error line per Vulkan ICD probe
# without it. A private one silences it and changes nothing else.
if [ -z "${XDG_RUNTIME_DIR:-}" ]; then
    export XDG_RUNTIME_DIR="${TMPDIR:-/tmp}/pleochroma-xdg-$(id -u)"
    mkdir -p -m 700 "$XDG_RUNTIME_DIR"
fi

REPO="$(pwd)"
STEPS=""
TIMEOUT=45m
STALL=1200
HANG_REPRO=0
WITH_LONG=0
COST=0
WITH_UI=0
NO_CLIPPY=0
while [ $# -gt 0 ]; do
    case "$1" in
        --repo) REPO="$2"; shift 2 ;;
        --steps) STEPS=",$2,"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        --stall) STALL="$2"; shift 2 ;;
        --hang-repro) HANG_REPRO="$2"; shift 2 ;;
        --with-long) WITH_LONG=1; shift ;;
        --cost) COST=1; shift ;;
        --with-ui) WITH_UI=1; shift ;;
        --no-clippy) NO_CLIPPY=1; shift ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

cd "$REPO" || { echo "no repo at $REPO" >&2; exit 2; }
# Sources that differ from the last run's get the current time, so cargo rebuilds what changed
# even when the copied files keep older times (scripts/fresh_sources.sh).
. "$REPO/scripts/fresh_sources.sh"
fresh_sources
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"
OUT_DIR="$REPO/owner-run-logs/beam-tests-$(date +%Y-%m-%d_%H%M%S)"
mkdir -p "$OUT_DIR"
SUMMARY="$OUT_DIR/summary.md"
{
    echo "# Beam tests (on request), $(date '+%Y-%m-%d %H:%M')"
    echo
    # The code that ran: the box's tree is copied over its clone, so HEAD alone does not name it.
    # "-dirty" says the tree differs from HEAD; "version" is the workspace version; "sources" is
    # sources_hash (fresh_sources.sh): the hash of every file git sees (owner-run-logs left out),
    # the same on any machine.
    echo "Repo: $REPO (git: $(git describe --always --dirty 2>/dev/null || echo unknown), version: $(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1), sources: $(sources_hash)), cores: $(nproc)"
    echo "Toolchain: $(rustc -V 2>/dev/null || echo unknown), $(cargo -V 2>/dev/null || echo unknown)"
    echo
    echo "| step | exit | minutes |"
    echo "| --- | --- | --- |"
} > "$SUMMARY"

# The process group of the running step (`timeout` makes one and leads it), or empty; the step's
# name and start (SECONDS), for the row of an interrupted step.
RUNNER=""
CURRENT_STEP=""
CURRENT_STEP_START=0

# Ctrl-C, TERM or a closed terminal (HUP): the step runs in the background in its own process group
# and would survive the script, so it is killed here with everything it started, and its row says
# it was interrupted.
on_interrupt() {
    if [ -n "$RUNNER" ]; then
        kill -KILL -- "-$RUNNER" 2>/dev/null
    fi
    if [ -n "$CURRENT_STEP" ]; then
        echo "| $CURRENT_STEP | interrupted | $(awk -v s="$(( SECONDS - CURRENT_STEP_START ))" 'BEGIN { printf "%.1f", s / 60 }') |" \
            >> "$SUMMARY"
    fi
    echo "interrupted; logs: $OUT_DIR" >&2
    exit 130
}
trap on_interrupt INT TERM HUP

# runner_alive: whether the running step's `timeout` process still runs (a zombie does not).
runner_alive() {
    local line=""
    { read -r line < "/proc/$RUNNER/stat"; } 2>/dev/null || return 1
    line="${line##*) }"
    [ "${line%% *}" != Z ]
}

# group_pids PGID: the processes of process group PGID, read from /proc (no procps needed).
group_pids() {
    local pgid="$1" stat line rest pid
    for stat in /proc/[0-9]*/stat; do
        { read -r line < "$stat"; } 2>/dev/null || continue
        # After the command name's closing ')': state, parent, process group.
        rest="${line##*) }"
        # shellcheck disable=SC2086
        set -- $rest
        if [ "${3:-}" = "$pgid" ]; then
            pid="${stat#/proc/}"
            echo "${pid%/stat}"
        fi
    done
}

# dump_stall NAME FILE SECONDS: what the stalled step was doing, written to FILE. Per thread: id,
# name, state, kernel wait channel and system call number with its arguments (202 = futex: a
# user-space lock; 16 = ioctl: inside a driver), the kernel stack where readable (root), then gdb
# backtraces of the test binaries when gdb is installed.
# It must run in the script's own shell and read the per-thread files with the shell's `read`: only
# an ancestor of the stalled process may read `syscall` on a box with restricted ptrace. It reads no
# file that takes the stalled process's memory-map lock (`cmdline`, `maps`), and every external
# tool that could block on the driver has a hard bound.
dump_stall() {
    local name="$1" out="$2" quiet="$3"
    local pid task tid comm stat_line state wchan call line lines exe
    {
        echo "# Stalled step $name, $(date '+%Y-%m-%d %H:%M:%S'): no new log line for $quiet s"
        echo
        echo "## Memory"
        head -n 5 /proc/meminfo
        echo
        echo "## GPU"
        timeout -s KILL 20 nvidia-smi 2>&1 | head -n 40
        for pid in $(group_pids "$RUNNER"); do
            comm=""
            { read -r comm < "/proc/$pid/comm"; } 2>/dev/null
            echo
            echo "## Process $pid ($comm)"
            echo "tid | name | state | kernel wait channel | system call"
            for task in /proc/"$pid"/task/[0-9]*; do
                [ -d "$task" ] || continue
                tid="${task##*/}"
                comm=""
                stat_line=""
                wchan=""
                call=""
                { read -r comm < "$task/comm"; } 2>/dev/null
                { read -r stat_line < "$task/stat"; } 2>/dev/null
                { read -r wchan < "$task/wchan"; } 2>/dev/null
                { read -r call < "$task/syscall"; } 2>/dev/null
                state="${stat_line##*) }"
                state="${state%% *}"
                if [ "$tid" = "$pid" ]; then
                    comm="$comm (main thread)"
                fi
                printf '%s | %s | %s | %s | %s\n' \
                    "$tid" "$comm" "${state:-?}" "${wchan:-?}" "${call:0:48}"
                lines=0
                while [ "$lines" -lt 12 ] && read -r line; do
                    printf '    %s\n' "$line"
                    lines=$((lines + 1))
                done 2>/dev/null < "$task/stack"
            done
        done
        if command -v gdb > /dev/null 2>&1; then
            for pid in $(group_pids "$RUNNER"); do
                exe=$(timeout -s KILL 5 readlink "/proc/$pid/exe" 2>/dev/null)
                if [[ "$exe" == */target/* ]]; then
                    echo
                    echo "## gdb backtraces of process $pid"
                    timeout -s KILL 120 gdb -p "$pid" -batch -ex "set pagination off" \
                        -ex "thread apply all bt 25" 2>&1 | tail -n 3000
                fi
            done
        fi
    } > "$out" 2>&1
}

# step NAME COMMAND...: runs COMMAND with a hard stop ($TIMEOUT) and a stall detector ($STALL
# seconds without a new log line: the thread states go to NAME.stall.txt and the step is killed),
# logs it and records exit code and minutes. A step sets its own bounds with
# `TIMEOUT=10m STALL=300 step ...`. A step left out by --steps is listed as skipped.
# LAST_STATUS holds the exit code of the step that ran last.
LAST_STATUS=""
step() {
    local name="$1"; shift
    local log="$OUT_DIR/$name.log"
    if [ -n "$STEPS" ] && [[ "$STEPS" != *",${name%%-*},"* ]]; then
        echo "| $name | skipped (--steps) | 0 |" >> "$SUMMARY"
        return
    fi
    echo "== $name"
    local start=$SECONDS
    local quiet=0
    local note=""
    CURRENT_STEP="$name"
    CURRENT_STEP_START=$start
    : > "$log"
    timeout --kill-after=30s "$TIMEOUT" "$@" > >(tee -a "$log") 2>&1 &
    RUNNER=$!
    while runner_alive; do
        sleep 2
        quiet=$(( $(date +%s) - $(stat -c %Y "$log" 2>/dev/null || date +%s) ))
        if [ "$quiet" -ge "$STALL" ]; then
            dump_stall "$name" "$OUT_DIR/$name.stall.txt" "$quiet"
            echo "== $name: no output for $quiet s, killed (thread states: $name.stall.txt)" \
                | tee -a "$log"
            kill -KILL -- "-$RUNNER" 2>/dev/null
            note=" (stalled: no output for $quiet s)"
            break
        fi
    done
    # The redirect keeps bash's job notice of a killed step ("line N: PID Killed timeout ...") out
    # of the log.
    wait "$RUNNER" 2>/dev/null
    local status=$?
    LAST_STATUS=$status
    RUNNER=""
    CURRENT_STEP=""
    if [ -z "$note" ] && { [ "$status" -eq 124 ] || [ "$status" -eq 137 ]; }; then
        note=" (hard stop $TIMEOUT)"
    fi
    # Tenths of a minute: whole minutes would show a 4 s step as 1.
    local minutes
    minutes=$(awk -v s="$(( SECONDS - start ))" 'BEGIN { printf "%.1f", s / 60 }')
    echo "| $name | $status$note | $minutes |" >> "$SUMMARY"
}

# skip_step NAME REASON [HINT]: a step that cannot run on this machine. It prints the reason and
# the hint and is listed as skipped with the reason (not red); a step left out by --steps is listed
# as skipped (--steps), as `step` lists it.
skip_step() {
    local name="$1" reason="$2" hint="${3:-}"
    if [ -n "$STEPS" ] && [[ "$STEPS" != *",${name%%-*},"* ]]; then
        echo "| $name | skipped (--steps) | 0 |" >> "$SUMMARY"
        return
    fi
    echo "== $name: skipped ($reason)"
    if [ -n "$hint" ]; then
        echo "note: $hint" >&2
    fi
    echo "| $name | skipped ($reason) | 0 |" >> "$SUMMARY"
}

# The environment. An explicit RUSTFLAGS REPLACES .cargo/config.toml's target rustflags, so those
# are restated; the cfg that compiles the beam tests goes on top. The own target dir keeps the
# regular build cache. `.cargo` is not in git, so a clone has no config: the flags are set here
# too, the mold link only where clang and mold are installed (without them no build script would
# link).
unset CARGO_ENCODED_RUSTFLAGS            # it would override RUSTFLAGS
BASE_RUSTFLAGS="${RUSTFLAGS:-}"          # an owner's own RUSTFLAGS already replaced the config flags
if [ -z "$BASE_RUSTFLAGS" ]; then        # restate .cargo/config.toml, which RUSTFLAGS replaces
    case "$(uname -m)" in x86_64|amd64) BASE_RUSTFLAGS="-C target-cpu=x86-64-v3" ;; esac
    if [ "$(uname -s)" = Linux ] && [ "$(uname -m)" = x86_64 ] \
        && command -v clang > /dev/null 2>&1 && command -v mold > /dev/null 2>&1; then
        BASE_RUSTFLAGS="$BASE_RUSTFLAGS -C linker=clang -C link-arg=-fuse-ld=mold"
    fi
fi
export RUSTFLAGS="$BASE_RUSTFLAGS --cfg beam_tests"
export RUSTDOCFLAGS="--cfg beam_tests"
export CARGO_TARGET_DIR="$REPO/target/beam-tests"
LIB_FEATURES=gpu,path-cache-disk,beam,stone-shadow

# check_ran NAME: after a cargo test step. Sums the N of every "test result: ... N passed" line of
# the step's log; a step that ran, ended with exit code 0 and ran no test at all (a filter that
# matches nothing exits 0, and without --cfg beam_tests the beam tests do not exist) is listed in
# the summary and makes the script exit 1.
NO_TESTS=0
# passed_count LOG: the sum of the N of every "test result: ... N passed" line of LOG.
passed_count() {
    sed -n 's/^test result: .* \([0-9][0-9]*\) passed;.*/\1/p' "$1" | awk '{ n += $1 } END { print n + 0 }'
}

check_ran() {
    local name="$1"
    local log="$OUT_DIR/$name.log"
    [ -f "$log" ] || return 0
    [ "$LAST_STATUS" = 0 ] || return 0
    local ran
    ran=$(passed_count "$log")
    if [ "$ran" -eq 0 ]; then
        # The step's `tee` (a process substitution `step` cannot wait for) can still be writing the
        # log's last lines when the step has ended: count once more after a pause.
        sleep 2
        ran=$(passed_count "$log")
    fi
    if [ "$ran" -eq 0 ]; then
        echo "| $name | NO TESTS RAN (does the filter match? is --cfg beam_tests set?) | 0 |" >> "$SUMMARY"
        NO_TESTS=1
    fi
}

# The beam build of the whole workspace with the cfg: library, apps, and every beam test target.
if [ "$NO_CLIPPY" -eq 1 ]; then
    skip_step 00-clippy-beam "--no-clippy"
    skip_step 00c-ui-clippy-beam "--no-clippy"
else
    step 00-clippy-beam cargo clippy --workspace --exclude pleochroma-mobile-android --all-targets \
        --all-features --message-format=short
    # The short format of step 00: cargo replays the warnings step 00 cached in it anyway.
    step 00c-ui-clippy-beam cargo clippy -p pleochroma-cut --all-features --test ui_harness \
        --test ui_static --test ui_screens --test tutorial_videos --message-format=short
fi

# The lib test binary of every 02* and 03* step, built on its own: cargo prints nothing while it
# compiles the crate, which the 300 s stall bound of the test steps would take for a stall.
step 01-build-test-binary cargo test --no-run -p pleochroma --lib --features "$LIB_FEATURES"

# Module filters are substring matches of the test paths (`renderer::beam::` does not match
# `renderer::gpu::beam::`). No test of this step acquires a GPU.
TIMEOUT=30m STALL=300 step 02-pleochroma-cpu cargo test --no-fail-fast -p pleochroma --lib \
    --features "$LIB_FEATURES" -- \
    renderer::beam:: renderer::accel_beam renderer::path_cache::tests_bounce_beam \
    renderer::path_cache::tests_key optics::raytracer::transport::reference_tests \
    optics::raytracer::environment::tray_shadow renderer::adaptive::tests_offset
check_ran 02-pleochroma-cpu

# GPU tests never share a step with CPU tests: a stalled driver call blocks the whole test process.
TIMEOUT=15m STALL=300 step 02a-pleochroma-beam-gpu env PLEOCHROMA_REQUIRE_GPU=1 cargo test \
    --no-fail-fast -p pleochroma --lib --features "$LIB_FEATURES" -- \
    renderer::gpu::beam:: renderer::gpu_backend::backend::reference renderer::gpu::frame::live_source \
    --test-threads 4
check_ran 02a-pleochroma-beam-gpu

# Only with --hang-repro N: GPU beam tests mixed with CPU ones, every GPU test creating its own
# instance at once, the test gate off, N times, to reproduce a stall. A stalled round is killed
# after 4 minutes of silence and leaves its thread states in 02r-hang-repro-<n>.stall.txt.
if [ "$HANG_REPRO" -gt 0 ]; then
    for round in $(seq 1 "$HANG_REPRO"); do
        TIMEOUT=10m STALL=240 step "02r-hang-repro-$round" env PLEOCHROMA_GPU_TEST_GATE=0 cargo test \
            --no-fail-fast -p pleochroma --lib --features "$LIB_FEATURES" -- \
            renderer::beam::tests_d5_hybrid:: renderer::gpu::beam:: renderer::gpu_backend::backend::
        check_ran "02r-hang-repro-$round"
    done
fi

# The ignored rows that took over 60 s in the workspace run, each by its exact name.
SLOW_CPU=(
    renderer::beam::tests_d1_equivalence::the_image_is_bit_identical_for_one_and_eight_threads
    renderer::beam::tests_d2_equivalence::the_full_tree_is_bit_identical_for_one_and_eight_threads_and_across_runs
    renderer::beam::tests_d2_equivalence::the_tree_logs_its_counts
    renderer::beam::tests_d3_equivalence::diamond_under_the_warmest_planckian_preset_reports_the_end_clamp
    renderer::beam::tests_d3_equivalence::glass_renders_bit_for_bit_the_same_with_and_without_sub_nodes
    renderer::beam::tests_d3_equivalence::the_worst_node_step_of_a_dispersive_stone_is_measured_and_logged
    renderer::beam::tests_d3_intervals::a_grazing_entry_splits_its_interval_and_the_render_is_deterministic
    renderer::beam::tests_d3_intervals::a_non_dispersive_stone_never_splits
    renderer::beam::tests_d3_intervals::a_transmitted_leaf_whose_centroid_leaves_its_branch_is_not_kept_whole
    renderer::beam::tests_d3_intervals::kept_regions_lie_inside_the_exact_region_of_every_wavelength_of_the_bin
    renderer::beam::tests_d3_intervals::per_node_regions_cover_at_least_what_the_end_intersection_covers
    renderer::beam::tests_d4_biaxial::glass_and_diamond_renders_count_no_birefringent_events
    renderer::beam::tests_d4_uniaxial::isotropic_stones_count_no_birefringent_events
    renderer::beam::tests_d5_hybrid::a_cap_that_splits_the_batches_by_the_estimate_gives_the_same_image
    renderer::beam::tests_d5_hybrid::a_declining_engine_fails_over_to_the_cpu_with_the_same_image
    renderer::beam::tests_d5_hybrid::a_tile_ordered_render_holds_only_its_tiles_and_equals_the_frame_render_there
    renderer::beam::tests_d5_hybrid::a_tile_restricted_run_equals_the_full_run_on_its_tiles_and_is_zero_elsewhere
    renderer::beam::tests_d5_hybrid::batches_of_any_size_and_no_engine_give_the_same_image_with_fixed_samples
    renderer::beam::tests_d5_hybrid::one_and_eight_threads_are_bit_identical
    renderer::beam::tests_d5_hybrid::progress_is_monotone_phased_and_ends_at_one
    renderer::beam::tests_d5_hybrid::the_end_clamp_holds_and_the_clamp_row_is_reported_for_a_white_diamond
    renderer::beam::tests_d5_hybrid::the_estimate_counts_the_kept_indexs_memberships_counts_and_cursors
    renderer::beam::tests_d5_hybrid::the_gpu_layouts_are_built_only_for_an_engine_that_wants_them
    renderer::beam::tests_d5_hybrid::the_hooks_see_every_piece_with_its_final_pixels_and_its_leaves
    renderer::beam::tests_d5_hybrid::the_hybrid_image_matches_the_plain_reference_on_the_brilliant
    renderer::beam::tests_d5_hybrid::the_hybrid_image_matches_the_plain_reference_on_the_cube
    renderer::beam::tests_d5_hybrid::the_production_tracer_is_printed_against_the_reference
    renderer::beam::tests_d5_hybrid::two_cpu_engines_share_the_batches_in_turn_and_give_the_same_image
    renderer::beam::tests_d5_hybrid::without_a_residual_the_image_is_the_clamped_beam_part_of_the_frame
    renderer::beam::tests_d5_residual::a_cancelled_token_returns_none_at_once
    renderer::beam::tests_d5_residual::a_subset_batch_with_a_beam_offset_equals_the_restricted_reference_run
    renderer::beam::tests_d5_residual::a_token_set_during_the_run_stops_it_within_a_pass
    renderer::beam::tests_d5_residual::an_empty_batch_gives_an_empty_residual_and_a_foreign_tile_gives_none
    renderer::beam::tests_d5_residual::an_empty_kept_index_gives_the_plain_reference_adaptive_run
    renderer::beam::tests_d5_residual::one_and_eight_threads_are_bit_identical
    renderer::beam::tests_d5_residual::the_residual_with_the_kept_index_plus_the_beams_matches_the_plain_reference
    renderer::beam::tests_d6_relight::a_background_of_the_wrong_size_is_refused
    renderer::beam::tests_d6_relight::a_cancelled_relight_returns_none
    renderer::beam::tests_d6_relight::a_glare_change_relights_to_the_fresh_render
    renderer::beam::tests_d6_relight::a_head_shadow_change_relights_to_the_fresh_render
    renderer::beam::tests_d6_relight::a_new_studio_pose_relights_to_the_fresh_render
    renderer::beam::tests_d6_relight::a_new_studio_preset_relights_to_the_fresh_render
    renderer::beam::tests_d6_relight::a_second_relight_under_the_same_lighting_changes_nothing
    renderer::beam::tests_d6_relight::an_empty_cache_has_nothing_to_relight_but_the_ray_traced_call_builds
    renderer::beam::tests_d6_relight::batches_over_the_cap_are_retraced_and_give_the_same_image
    renderer::beam::tests_d6_relight::retain_counts_the_batch_honestly_and_refuses_past_the_cap
    renderer::beam::tests_d6_relight::studio_to_an_hdr_map_refines_the_leaves_and_relights_to_the_fresh_render
    renderer::beam::tests_d6_relight::studio_to_an_hdr_map_relights_a_dispersive_stone_to_the_fresh_render_bit_for_bit
    renderer::beam::tests_d6_relight::the_ray_traced_relight_is_close_to_the_whitted_image_in_level
    renderer::beam::tests_d6_relight::the_ray_traced_relight_is_deterministic_and_the_same_for_a_capped_cache
    renderer::beam::tests_d6_relight::the_ray_traced_relight_matches_beams_plus_residual_on_covered_pixels
    renderer::beam::tests_d5_hybrid::a_preloaded_render_lends_the_cached_leaves_and_equals_the_copying_render
    renderer::beam::tests_d6_relight::a_refined_leaf_that_dropped_a_piece_is_not_restored_whole
    renderer::beam::tests_d6_relight::a_residual_only_missing_tile_is_not_black_after_a_hybrid_relight
    renderer::beam::tests_d7_audit::a_cache_cut_into_other_batches_still_serves_every_tile
    renderer::beam::tests_d7_audit::a_cache_of_another_tile_edge_loads_nothing
    renderer::beam::tests_d7_audit::a_cache_with_missing_batches_is_completed_by_tracing_them
    renderer::beam::tests_d7_audit::a_cancel_at_random_instants_returns_within_the_bound_and_finishes_nothing
    renderer::beam::tests_d7_audit::a_cancelled_preloaded_render_returns_promptly_and_finishes_nothing
    renderer::beam::tests_d7_audit::a_forced_small_engine_binding_routes_the_batch_to_the_cpu_with_one_log_line
    renderer::beam::tests_d7_audit::a_forced_small_leaf_cache_keeps_less_and_never_changes_the_image
    renderer::beam::tests_d7_audit::a_forced_small_memory_cap_gives_the_same_image_and_one_log_line_per_early_hand_over
    renderer::beam::tests_d7_audit::a_preloaded_render_equals_a_fresh_render_bit_for_bit_and_traces_nothing
    renderer::beam::tests_d7_audit::progress_never_decreases_over_a_whole_render_for_any_batching_and_engine_mix
    renderer::beam::tests_d7_failures::a_cancel_at_a_random_point_returns_at_once_and_writes_nothing_twice
    renderer::beam::tests_d7_failures::a_chain_of_failing_engines_ends_on_the_pipelines_cpu_with_the_same_image
    renderer::beam::tests_d7_failures::a_decline_is_deterministic_run_to_run
    renderer::beam::tests_d7_failures::a_device_lost_between_the_stages_reruns_the_batch_on_the_cpu
    renderer::beam::tests_d7_failures::a_remote_worker_that_disconnects_fails_over_with_the_same_image
    renderer::beam::tests_d7_failures::an_engine_declining_at_batch_k_hands_the_batch_over_with_the_same_image
    renderer::beam::tests_d7_stress::an_engine_over_its_cap_declines_to_the_cpu_and_the_batches_that_fit_stay_on_it
)
TIMEOUT=2h STALL=3600 step 03-pleochroma-slow cargo test --no-fail-fast -p pleochroma --lib \
    --features "$LIB_FEATURES" -- --ignored --exact --test-threads 4 "${SLOW_CPU[@]}"
check_ran 03-pleochroma-slow
TIMEOUT=2h STALL=3600 step 03g-pleochroma-slow-gpu env PLEOCHROMA_REQUIRE_GPU=1 cargo test --no-fail-fast \
    -p pleochroma --lib --features "$LIB_FEATURES" -- --ignored --exact --test-threads 4 \
    renderer::gpu::beam::tests_reference::the_reference_megakernel_equals_production_on_a_constant_index_material
check_ran 03g-pleochroma-slow-gpu

# The other ignored beam rows: the leaf cache, the cube reflections, the two-bounce budget and the
# `_smoke` rows. The long equivalence rows are step 09 (--with-long).
TIMEOUT=2h STALL=3600 step 03b-pleochroma-slow-more cargo test --no-fail-fast -p pleochroma --lib \
    --features "$LIB_FEATURES" -- \
    renderer::beam::leaf_cache:: \
    renderer::beam::tests_d1_equivalence::cube_reflections_match_the_reference_under_ \
    renderer::beam::tests_d2_equivalence::a_two_bounce_budget_gives_the_reflection_image \
    _smoke --ignored --test-threads 4
check_ran 03b-pleochroma-slow-more

# The cut's lib test binary of 04 and 04b, built on its own (as 01 builds pleochroma's): the first
# run compiles the whole app and its GUI dependencies here, far longer than 04's 10-minute bound.
step 04-build-cut-test-binary cargo test --no-run -p pleochroma-cut --lib --features beam
TIMEOUT=10m step 04-pleochroma-cut cargo test --no-fail-fast -p pleochroma-cut --lib --features beam -- \
    bridge::render_thread::still_beams bridge::render_thread::ray_beams \
    bridge::export_thread::worker::core::beam bridge::export_thread::remote::beam_tiles \
    gui::preferences gui::render::output_defaults gui::render_jobs gui::progress_eta
check_ran 04-pleochroma-cut
TIMEOUT=1h STALL=3600 step 04b-pleochroma-cut-slow cargo test --no-fail-fast -p pleochroma-cut --lib --features beam -- \
    bridge::export_thread::worker::core::beam bridge::render_thread::still_beams \
    bridge::render_thread::ray_beams --ignored --test-threads 4
check_ran 04b-pleochroma-cut-slow

step 05-worker cargo test --no-fail-fast -p pleochroma-worker --features beam -- \
    beam_tiles render_core::beam
check_ran 05-worker

step 06-cli cargo test --no-fail-fast -p pleochroma-cli --features beam -- \
    args::tests_beam commands::render_job help::tests
check_ran 06-cli

step 07-net cargo test --no-fail-fast -p pleochroma-net --features render -- messages::beam_tiles
check_ran 07-net

step 08-render-jobs cargo test --no-fail-fast -p pleochroma-render-jobs -- \
    beam_settings_outside_their_range_are_refused
check_ran 08-render-jobs

# The long beam equivalence rows: every ignored renderer::beam:: row, one at a time, in the
# quick-release profile (opt-level 3 without LTO). Hours.
if [ "$WITH_LONG" -eq 1 ]; then
    TIMEOUT=6h STALL=21600 step 09-beam-long cargo test --no-fail-fast -p pleochroma \
        --profile quick-release --features "$LIB_FEATURES" -- \
        renderer::beam:: --ignored --nocapture --test-threads 1
    check_ran 09-beam-long
fi

# The two beam cost diagnostics (ignored rows of renderer::beam::tests_d6_cost). The budgets are
# passed through when set.
if [ "$COST" -eq 1 ]; then
    COST_ENV=()
    if [ -n "${PLEOCHROMA_BEAM_COST_BUDGET_S:-}" ]; then
        COST_ENV+=("PLEOCHROMA_BEAM_COST_BUDGET_S=$PLEOCHROMA_BEAM_COST_BUDGET_S")
    fi
    if [ -n "${PLEOCHROMA_BEAM_BUILD_BUDGET_S:-}" ]; then
        COST_ENV+=("PLEOCHROMA_BEAM_BUILD_BUDGET_S=$PLEOCHROMA_BEAM_BUILD_BUDGET_S")
    fi
    TIMEOUT=2h STALL=7200 step 10-cost env ${COST_ENV[@]+"${COST_ENV[@]}"} cargo test \
        --no-fail-fast -p pleochroma --lib --features "$LIB_FEATURES" -- \
        --ignored --exact --nocapture --test-threads 1 \
        renderer::beam::tests_d6_cost::tree_cost_by_bounces_and_settings_for_the_relight_rig \
        renderer::beam::tests_d6_cost::the_ray_traced_build_of_the_relight_rig_within_a_time_limit
    check_ran 10-cost
fi

# The cut UI harness with the beam states (it runs on command only; the beam states exist with the
# feature alone). No check_ran here: the harness has its own `main` (`harness = false`), prints no
# "test result:" line, and exits non-zero when it finds anything (its report is ui-harness/report.md
# under this script's target dir). The bound covers its first build in the harness profile.
if [ "$WITH_UI" -eq 1 ]; then
    TIMEOUT=2h step 11-ui-harness cargo test -p pleochroma-cut --features ui-harness,beam \
        --profile harness --test ui_harness
fi

echo
cat "$SUMMARY"
echo "Logs: $OUT_DIR"
if [ "$NO_TESTS" -eq 1 ]; then
    echo "a step ran no test: see NO TESTS RAN in the summary" >&2
    exit 1
fi
