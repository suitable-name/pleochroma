#!/usr/bin/env bash
# The owner's whole test round in one command: the GPU checks and measurements (exit collection,
# path cache, GPU lifecycle, sorted wavefront) and the beam gating checks, one part after another,
# with one summary at the end.
#
# Parts (each runs even when an earlier one failed):
#   retest        scripts/retest.sh (its header lists the steps and what was left out)
#   bench         scripts/bench_render_accel.sh on the scenes s1_glints, s2_sapphire and
#                 s3_small (with --bench-typical: s2_sapphire and s3_small only, on the default
#                 engines). It builds the quick-release CLI, then runs the render_accel
#                 benchmark: references and the fixed baseline, all with exit collection. No cache
#                 or beam rows, and no adaptive rows unless --with-adaptive.
#   cache-note    three renders that ask for the path cache without keeping it: each must print the
#                 one-shot note and take the plain render's time; plus one job
#                 that asks for beams: the default build renders it without them and prints the
#                 note; and one that asks for adaptive sampling: the default build renders it
#                 without and prints "Adaptive sampling off: this build has no adaptive sampling."
#   speed         scripts/eng_speed_experiments.sh with its default parts S and E (--speed-parts
#                 names others). S: the megakernel against the staged kernel (PLEOCHROMA_STAGED) on
#                 the GPU. E: the sorted wavefront (PLEOCHROMA_SORTED) against both, each image
#                 compared with its megakernel row's. Part H (hybrid rule) runs only on request.
#                 It builds the CLI itself (seconds when part bench has built it).
#   m1            only with --with-m1: plain against kept-cache renders at 64, 256 and 1024 samples
#                 on s2_sapphire and s3_small against part bench's references; it first builds
#                 the two kept caches (about 5 minutes)
#   ui            the cut's UI in the default build (beam controls hidden): clippy of the UI harness
#                 target, the static .slint scan and the whole UI harness (about 1-1.5 h)
#   beam-check    scripts/run_beam_tests.sh --steps 00,00c: clippy of the beam build with
#                 --cfg beam_tests (workspace and the cut's UI targets); builds target/beam-tests
#                 (tens of GB, about 25-40 min)
#   on-request    only with --with-on-request: scripts/run_tests_on_request.sh, the tests over 60 s
#   beam-tests    only with --with-beam: scripts/run_beam_tests.sh (the beam build with --cfg
#                 beam_tests in target/beam-tests; its own header lists the steps)
#
# Beam tracing is retired: the default parts build and run no beam code except retest's step 01d
# and part beam-check, which only compile and lint the beam build. The beam tests run only with
# --with-beam.
#
# Exit codes: retest.sh exits 1 only on NO TESTS RAN (and run_beam_tests.sh likewise), and clippy
# exits 0 on warnings. A red clippy or test step, or a clippy step with warnings, therefore shows as
# part exit 0: the row "retest red steps" (and "beam-check red steps") after the part row names the
# steps whose exit is not 0, and the warning rows of the clippy steps (retest's after each clippy
# step, beam-check's after its part row) count their warnings; the step tables appended to the
# round summary below have the rest. A part that was not asked for is listed as skipped (opt-in) or
# skipped (--parts), and a part that Ctrl-C, TERM or a closed terminal stopped as interrupted.
#
# Usage (from the repository root, on the GPU machine); example:
#   bash scripts/run_full_round.sh --gate-repro 20 --exit-repro 10 --with-m1
#
#   bash scripts/run_full_round.sh [--repo DIR] [--parts LIST] [--steps LIST] [--gate-repro N]
#                                  [--hang-repro N] [--exit-repro N] [--bench-typical]
#                                  [--speed-parts LIST] [--with-adaptive] [--with-m1]
#                                  [--with-on-request] [--with-beam] [--beam-hang-repro N]
#                                  [--with-long] [--no-beam]
#
#   --repo DIR          repository root (default: the current directory)
#   --parts LIST        comma list of parts to run (default: retest,bench,cache-note,speed,ui,
#                       beam-check); it replaces the defaults, including ui and beam-check, and the
#                       --with-* flags given before it. cache-note uses the CLI that part
#                       bench builds (without bench: target/quick-release/pleochroma-cli as it is);
#                       m1 needs part bench in the same round (its CLI and its references)
#   --steps LIST        part retest runs only these steps (retest.sh --steps)
#   --gate-repro N      part retest also runs the stall reproduction N times with the lifecycle
#                       turn on
#   --hang-repro N      part retest also runs it N times with the turn off (the control)
#   --exit-repro N      part retest also runs the thread-exit stall reproduction (step 02t: late
#                       exits, hybrid-frame helpers, warm-up helpers) N times with the turn on
#   --bench-typical     part bench runs only the scenes s2_sapphire and s3_small, on the default
#                       engines, for a machine whose default engines split a frame between the CPU
#                       and the GPU. Part m1 still works (it needs exactly these two scenes); part
#                       cache-note uses the CLI the bench built
#   --with-adaptive     adaptive sampling is an optional build feature: part retest also runs its
#                       steps 04a, 05c and 06e (retest.sh --with-adaptive), and part bench builds
#                       the adaptive CLI and runs the adaptive rows
#   --speed-parts LIST  part speed runs only these parts of eng_speed_experiments.sh (its --parts:
#                       a comma list of S, E and H; default S,E), for example `S` on a box
#                       that cannot run the sorted wavefront rows
#   --with-m1           also run part m1 (plain against kept-cache renders, about 5 minutes;
#                       needs part bench)
#   --with-on-request   also run part on-request (hours)
#   --with-beam         also run the part beam-tests
#   --beam-hang-repro N part beam-tests also runs the step 02r hang mix N times
#   --with-long         part beam-tests also runs the long beam rows (hours); only with --with-beam
#   --no-beam           accepted and ignored (the default parts have no beam rows)
#
# Needs jq for the parts bench, cache-note and m1 (their job edits and figures). The check runs
# once the arguments are parsed, so --parts and --with-m1 count: when one of these parts is
# selected and jq is missing, the script names the parts and stops before any part runs, exit 2,
# instead of after the retest, which takes hours. Install jq (for example `sudo apt install jq`),
# or leave the parts out with --parts.
#
# Output: <repo>/owner-run-logs/round-<timestamp>/
#   NN-<part>.log   the full output of each part
#   summary.md      exit code, minutes and log folder per part, then each part's own summary.
#                   Send me this folder and the folders it names; the .plraw files are not needed.

set -u

REPO="$(pwd)"
PARTS=",retest,bench,cache-note,speed,ui,beam-check,"
RETEST_ARGS=()
BEAM_ARGS=()
SPEED_ARGS=()
BENCH_ARGS=()
BENCH_TYPICAL=0
while [ $# -gt 0 ]; do
    case "$1" in
        --repo) REPO="$2"; shift 2 ;;
        --parts) PARTS=",$2,"; shift 2 ;;
        --steps) RETEST_ARGS+=(--steps "$2"); shift 2 ;;
        --gate-repro) RETEST_ARGS+=(--gate-repro "$2"); shift 2 ;;
        --hang-repro) RETEST_ARGS+=(--hang-repro "$2"); shift 2 ;;
        --exit-repro) RETEST_ARGS+=(--exit-repro "$2"); shift 2 ;;
        --bench-typical) BENCH_TYPICAL=1; shift ;;
        --with-adaptive) RETEST_ARGS+=(--with-adaptive); BENCH_ARGS+=(--with-adaptive); shift ;;
        --speed-parts) SPEED_ARGS+=(--parts "$2"); shift 2 ;;
        --with-m1) PARTS="${PARTS}m1,"; shift ;;
        --with-on-request) PARTS="${PARTS}on-request,"; shift ;;
        --with-beam) PARTS="${PARTS}beam-tests,"; shift ;;
        --beam-hang-repro) BEAM_ARGS+=(--hang-repro "$2"); shift 2 ;;
        --with-long) BEAM_ARGS+=(--with-long); shift ;;
        --no-beam) shift ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

cd "$REPO" || { echo "no repo at $REPO" >&2; exit 2; }
# Parts bench, cache-note and m1 need jq. Stop before the first part (the retest alone takes
# hours) rather than let them exit 2 one after the other.
if ! command -v jq >/dev/null 2>&1; then
    NEED_JQ=""
    for name in bench cache-note m1; do
        if [[ "$PARTS" == *",$name,"* ]]; then NEED_JQ="${NEED_JQ:+$NEED_JQ, }$name"; fi
    done
    if [ -n "$NEED_JQ" ]; then
        echo "jq is required by the selected part(s): $NEED_JQ, and it is not installed." >&2
        echo "Install jq (for example: sudo apt install jq), or leave these parts out with --parts." >&2
        exit 2
    fi
fi
# Sources that differ from the last run's get the current time, so cargo rebuilds what changed
# even when the copied files keep older times (scripts/fresh_sources.sh).
. "$REPO/scripts/fresh_sources.sh"
fresh_sources
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"
# The engine switches never come from the caller's shell: the tests compare against the production
# engine (the staged megakernel), and the renders of parts bench, cache-note and m1 measure the
# default engine (part speed sets its switches per row).
unset PLEOCHROMA_GATED_LOOP PLEOCHROMA_STAGED PLEOCHROMA_SORTED PLEOCHROMA_GPU_PIPELINE \
    WGPU_VALIDATION_INDIRECT_CALL
# Headless boxes have no session runtime dir; libwayland prints an error line per Vulkan ICD probe
# without it.
if [ -z "${XDG_RUNTIME_DIR:-}" ]; then
    export XDG_RUNTIME_DIR="${TMPDIR:-/tmp}/pleochroma-xdg-$(id -u)"
    mkdir -p -m 700 "$XDG_RUNTIME_DIR"
fi

LOGS="$REPO/owner-run-logs"
# A round that stopped without its trap (SIGKILL, or a stopped or replaced container) keeps its
# start mark and has no row for the part it was in: its summary gets a line for that, and the
# next round lists it. Two rounds at once are unsupported (they share the GPU and target/).
STOPPED_ROUNDS=""
for mark in "$LOGS"/round-*/.started; do
    [ -e "$mark" ] || continue
    echo "| (stopped) | no row for the part it was in: the round ended without its trap | | |" \
        >> "${mark%/.started}/summary.md"
    STOPPED_ROUNDS="$STOPPED_ROUNDS ${mark%/.started}"
    rm -f "$mark"
done
OUT="$LOGS/round-$(date +%Y-%m-%d_%H%M%S)"
mkdir -p "$OUT"
START_MARK="$OUT/.started"
touch "$START_MARK"
SUMMARY="$OUT/summary.md"
{
    echo "# Full test round, $(date '+%Y-%m-%d %H:%M')"
    echo
    # The code that ran: the box's tree is copied over its clone, so HEAD alone does not name it.
    # "-dirty" says the tree differs from HEAD; "version" is the workspace version; "sources" is
    # sources_hash (fresh_sources.sh): the hash of every file git sees (owner-run-logs left out),
    # the same on any machine.
    echo "Repo: $REPO (git: $(git describe --always --dirty 2>/dev/null || echo unknown), version: $(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1), sources: $(sources_hash)), cores: $(nproc)"
    echo "Toolchain: $(rustc -V 2>/dev/null || echo unknown), $(cargo -V 2>/dev/null || echo unknown)"
    echo
    # A Cargo.lock that does not match the manifests is rewritten by the round's first cargo call,
    # and only that step's log shows it, so the summary says so here.
    if ! cargo metadata --locked --format-version 1 > /dev/null 2>&1; then
        echo "Cargo.lock did not match the manifests (or cargo metadata failed): this round's first"
        echo "cargo call rewrites it; send Cargo.lock with the logs."
        echo
    fi
    if [ -n "$STOPPED_ROUNDS" ]; then
        echo "Earlier rounds that stopped without their trap (see their summary.md):$STOPPED_ROUNDS"
        echo
    fi
    echo "| part | exit | minutes | logs |"
    echo "| --- | --- | --- | --- |"
} > "$SUMMARY"
FOLDERS=()

# newest_folder PREFIX: the newest owner-run-logs/PREFIX-* folder made in this round, or nothing.
newest_folder() {
    local dir
    dir=$(ls -td "$LOGS/$1"-*/ 2>/dev/null | head -n 1)
    if [ -n "$dir" ] && [ "$dir" -nt "$START_MARK" ]; then
        echo "${dir%/}"
    fi
}

# warning_count LOG: the warnings in a cargo clippy log: every line that starts one (`warning: ...`
# in the human format, `file:line:column: warning: ...` in the short one), without cargo's
# per-crate totals (`warning: `crate` (lib) generated N warnings`) and its "build failed" line.
warning_count() {
    grep -E '^warning|: warning:' "$1" \
        | grep -cvE '^warning: (`[^`]+` \(.+\) generated [0-9]+ warnings?|build failed)'
}

# red_steps FOLDER: the rows of FOLDER/summary.md (a step table of retest.sh or run_beam_tests.sh)
# whose exit is not 0, as "step exit" joined by "; ", or "none". Skipped rows and warning
# counts are not red.
red_steps() {
    awk -F ' [|] ' '
        /^[|] [0-9]/ {
            name = substr($1, 3)
            code = $2
            if (code != "0" && code !~ /^skipped/ && code !~ /warning/) {
                list = list (list == "" ? "" : "; ") name " " code
            }
        }
        END { print (list == "" ? "none" : list) }' "$1/summary.md"
}

# Ctrl-C, TERM or a closed terminal (HUP): the part that was running gets an "interrupted" row
# (retest.sh and run_beam_tests.sh also kill their running step and give it a row) and the start
# mark goes. bash runs the trap once the part's pipeline has ended, which Ctrl-C and a closed
# terminal do at once; a TERM sent to this script alone waits for the part (send it to the process
# group). SIGKILL and a restarted container run no trap: the next round reports such a round.
RUNNING_PART=""
RUNNING_PREFIX=""
RUNNING_START=0
on_round_interrupt() {
    trap - INT TERM HUP
    if [ -n "$RUNNING_PART" ]; then
        echo "| $RUNNING_PART | interrupted | $(( (SECONDS - RUNNING_START + 59) / 60 ))" \
            "| $(newest_folder "$RUNNING_PREFIX") |" >> "$SUMMARY"
    fi
    rm -f "$START_MARK"
    echo "interrupted; logs: $OUT" >&2
    exit 130
}
trap on_round_interrupt INT TERM HUP

# part NN NAME PREFIX COMMAND...: runs a part unless --parts leaves it out, logs it, and records its
# exit code, minutes and log folder (the newest PREFIX-* folder).
part() {
    local number="$1" name="$2" prefix="$3"
    shift 3
    if [[ "$PARTS" != *",$name,"* ]]; then
        local why="--parts"
        case "$name" in
            m1) why="opt-in: --with-m1" ;;
            on-request) why="opt-in: --with-on-request" ;;
            beam-tests) why="opt-in: --with-beam" ;;
        esac
        echo "| $name | skipped ($why) | 0 | |" >> "$SUMMARY"
        return
    fi
    echo "== part $name"
    local start=$SECONDS
    RUNNING_PART="$name"
    RUNNING_PREFIX="$prefix"
    RUNNING_START=$start
    "$@" 2>&1 | tee "$OUT/$number-$name.log"
    local status=${PIPESTATUS[0]}
    RUNNING_PART=""
    local minutes=$(( (SECONDS - start + 59) / 60 ))
    local folder
    folder=$(newest_folder "$prefix")
    echo "| $name | $status | $minutes | ${folder:-none} |" >> "$SUMMARY"
    if [ -n "$folder" ]; then
        FOLDERS+=("$name:$folder")
    fi
}

# cache_note_part: a render that asks for the path cache without keeping it must print the
# one-shot note and take the plain render's time. Three renders at 8192 samples on the default
# engines, then two jobs at 64 samples that ask for beams and for adaptive sampling, with the CLI
# part bench built. Its folder is owner-run-logs/cache-note-<timestamp>/ (summary.md and one log
# per scene). Every render stops after 30 minutes, so a stalled GPU cannot hold the round;
# --foreground keeps the render in this shell's process group, where Ctrl-C reaches it.
cache_note_part() {
    local cli="$REPO/target/quick-release/pleochroma-cli"
    local jobs="$REPO/benchmarks/render_accel/jobs"
    local out scene job start seconds note status=0
    local extra=()
    out="$LOGS/cache-note-$(date +%Y-%m-%d_%H%M%S)"
    [ -x "$cli" ] || { echo "no CLI at $cli (part bench builds it)" >&2; return 2; }
    command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; return 2; }
    mkdir -p "$out"
    {
        echo "# Path cache asked for without keeping it, $(date '+%Y-%m-%d %H:%M')"
        echo
        echo "Expected: the note on every scene; seconds as the fixed baseline of part bench."
        echo
        echo "| scene | seconds | note printed |"
        echo "| --- | --- | --- |"
    } > "$out/summary.md"
    for scene in s1_glints s2_sapphire s3_small; do
        job="$out/$scene.job.json"
        jq '.kind.samples_per_pixel = 8192' "$jobs/$scene.job.json" > "$job"
        extra=()
        if [ "$scene" = s1_glints ]; then
            extra=(--material "Synthetic Moissanite")
        fi
        start=$SECONDS
        timeout --foreground --kill-after=30s 30m "$cli" render "$job" ${extra[@]+"${extra[@]}"} \
            --out "$out/$scene.png" --path-cache --verbose > "$out/$scene.log" 2>&1 || status=1
        seconds=$((SECONDS - start))
        if grep -q "Path cache: not used, it only pays" "$out/$scene.log"; then
            note=yes
        else
            note=NO
            status=1
        fi
        echo "| $scene | $seconds | $note |" >> "$out/summary.md"
    done
    # The default build has no beam tracer: a job that asks for beams renders without them and says
    # so (export worker `queue_beams_unavailable`). This row checks it with the CLI part bench
    # builds (quick-release, GPU, no beam); retest 04 (the export worker) and 06d (the CLI's
    # render_job tests) check the same in test builds.
    job="$out/s3_small_beam.job.json"
    jq '.kind.samples_per_pixel = 64 | .kind.accel.beam = {}' "$jobs/s3_small.job.json" > "$job"
    start=$SECONDS
    timeout --foreground --kill-after=30s 30m "$cli" render "$job" --out "$out/s3_small_beam.png" \
        --verbose > "$out/s3_small_beam.log" 2>&1 || status=1
    seconds=$((SECONDS - start))
    if grep -q "Beam tracing off: this build has no beam tracer" "$out/s3_small_beam.log"; then
        note=yes
    else
        note=NO
        status=1
    fi
    echo "| s3_small, job asks for beams (64 spp) | $seconds | $note |" >> "$out/summary.md"
    # The same for adaptive sampling (an optional feature, off in the default build): the job asks
    # for it (AdaptiveSettings has no serde defaults, so every field is spelled), the default build
    # renders it at its own samples without adaptive sampling and says so.
    job="$out/s3_small_adaptive.job.json"
    jq '.kind.samples_per_pixel = 64 | .kind.accel.adaptive = {"target_noise": 0.01, "metric": "RelativeLstar", "min_spp": 64, "max_spp": 64, "pass_spp": 16, "tile": 16}' \
        "$jobs/s3_small.job.json" > "$job"
    start=$SECONDS
    timeout --foreground --kill-after=30s 30m "$cli" render "$job" --out "$out/s3_small_adaptive.png" \
        --verbose > "$out/s3_small_adaptive.log" 2>&1 || status=1
    seconds=$((SECONDS - start))
    if grep -q "Adaptive sampling off: this build has no adaptive sampling." "$out/s3_small_adaptive.log"; then
        note=yes
    else
        note=NO
        status=1
    fi
    echo "| s3_small, job asks for adaptive sampling (64 spp) | $seconds | $note |" >> "$out/summary.md"
    cat "$out/summary.md"
    return "$status"
}

# m1_part (--with-m1): is the kept path cache worth keeping as a preview? For
# s2_sapphire and s3_small, a plain and a kept render at 64, 256 and 1024 samples, each compared
# with the references part bench made in this round (REF2 as the reference and REF as the second
# one, as
# benchmarks/render_accel/run.sh compares). Per scene one untimed render at the cap first builds and
# keeps the cache (as run.sh's keep_first); the kept renders load it and must print "loaded from
# disk". The part sets no PLEOCHROMA_CACHE_VRAM_GB, so the cache sits where the app puts it by
# default (system RAM); the loaded column names the place. Needs part bench of this round: its CLI
# and, per scene, <bench folder>/<scene>/<scene>/ with adaptive.job.json, ref.plraw and ref2.plraw.
# Its folder is owner-run-logs/m1-<timestamp>/ (summary.md, one log per render); the kept caches
# are built in target/m1-path-cache/ and removed at the end.
# Each render stops after 20 minutes (the keep render after 30) and each comparison after 10;
# --foreground keeps them in this shell's process group, where Ctrl-C reaches them.
m1_part() {
    local cli="$REPO/target/quick-release/pleochroma-cli"
    local caches="$REPO/target/m1-path-cache"
    local base out scene d f missing cache n kind name start seconds loaded figures status=0
    local extra=()
    out="$LOGS/m1-$(date +%Y-%m-%d_%H%M%S)"
    [ -x "$cli" ] || { echo "no CLI at $cli (part bench builds it)" >&2; return 2; }
    command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; return 2; }
    mkdir -p "$out"
    base=$(newest_folder bench-render-accel)
    {
        echo "# M1: kept path cache against plain renders, $(date '+%Y-%m-%d %H:%M')"
        echo
        echo "References: ${base:-none (part bench did not run in this round)}"
        echo
        echo "Reading: the kept cache is worth keeping as a preview if a kept row has a lower mean_de"
        echo "than the plain row that took the same or more seconds. A kept row with loaded = NO"
        echo "rebuilt its cache: its seconds do not count (run the part once more)."
        echo
        echo "The plain rows run exit collection, the CLI's estimator (the references' estimator too)."
        echo "The kept rows run branching through --path-cache, which needs it."
        echo
        echo "| scene | render | spp | seconds | loaded | mean_de | changed_area_pct | worst_tile_de | p99_de |"
        echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    } > "$out/summary.md"
    for scene in s2_sapphire s3_small; do
        d="$base/$scene/$scene"
        missing=""
        for f in adaptive.job.json ref.plraw ref2.plraw; do
            if [ -z "$base" ] || [ ! -f "$d/$f" ]; then missing="$missing $f"; fi
        done
        if [ -n "$missing" ]; then
            echo "| $scene | inputs missing in ${d}:$missing | | | | | | | |" >> "$out/summary.md"
            status=1
            continue
        fi
        # The kept cache: one untimed render at the cap builds and keeps it (run.sh's keep_first).
        # A cache left by an earlier run would make it load instead of build.
        cache="$caches/$scene"
        rm -rf "$cache"
        timeout --foreground --kill-after=30s 30m "$cli" render "$d/adaptive.job.json" --out "$out/${scene}_keep_first.png" --path-cache \
            --persist-cache --cache-dir "$cache" --verbose < /dev/null > "$out/${scene}_keep_first.log" 2>&1 \
            || status=1
        if ! grep -q '^note: Path cache: .*kept for re-renders' "$out/${scene}_keep_first.log"; then
            echo "| $scene | no kept cache, see ${scene}_keep_first.log | | | | | | | |" >> "$out/summary.md"
            status=1
            continue
        fi
        for n in 64 256 1024; do
            jq ".kind.samples_per_pixel = $n" "$d/adaptive.job.json" > "$out/${scene}_$n.job.json"
            for kind in plain kept; do
                name="${scene}_${kind}_$n"
                extra=()
                if [ "$kind" = kept ]; then
                    extra=(--path-cache --persist-cache --cache-dir "$cache" --verbose)
                fi
                # Tenths of a second: a render at 64 samples takes a few seconds, and a count in
                # whole seconds would be off by up to one.
                start=$(date +%s.%N)
                timeout --foreground --kill-after=30s 20m "$cli" render "$out/${scene}_$n.job.json" --out "$out/$name.png" \
                    --raw-out "$out/$name.plraw" ${extra[@]+"${extra[@]}"} \
                    < /dev/null > "$out/$name.log" 2>&1 || status=1
                seconds=$(awk -v a="$start" -v b="$(date +%s.%N)" 'BEGIN { printf "%.1f", b - a }')
                loaded=-
                if [ "$kind" = kept ]; then
                    if grep -q "loaded from disk" "$out/$name.log"; then loaded=yes; else loaded=NO; fi
                    if grep -q '^note: Path cache: .*in video memory' "$out/$name.log"; then
                        loaded="$loaded (video memory)"
                    elif grep -q '^note: Path cache: .*in system RAM' "$out/$name.log"; then
                        loaded="$loaded (system RAM)"
                    fi
                fi
                figures=$(timeout --foreground --kill-after=30s 10m "$cli" image-compare "$d/ref2.plraw" "$out/$name.plraw" \
                    --tonemap-from "$d/adaptive.job.json" --json --ref2 "$d/ref.plraw" \
                    < /dev/null 2>> "$out/$name.log" \
                    | jq -r '[.mean_de, .changed_area_pct, .worst_tile_de, .p99_de] | map(tostring) | join(" | ")')
                [ -n "$figures" ] || status=1
                echo "| $scene | $kind | $n | $seconds | $loaded | ${figures:-compare failed} |" >> "$out/summary.md"
            done
        done
    done
    rm -rf "$caches"
    cat "$out/summary.md"
    return "$status"
}

# ui_part: the cut's UI in the default build (beam controls hidden): clippy of the UI harness
# target with the default features (the beam states are linted only by part beam-check, with
# --all-features), the static .slint scan, then the whole UI harness (a filtered run skips its
# coverage gate), bounded at 3 h (uiprobe has no per-scenario timeout). The scan and the harness
# run the commands of the cargo aliases `ui-static` and `ui-harness` (.cargo/config.toml), written
# out here: `.cargo` is not in git, so a clone has no aliases. Its folder is
# owner-run-logs/ui-<timestamp>/ (summary.md, the three logs, the report).
ui_part() {
    local out status=0 clippy_status static_status harness_status harness_note=""
    local clippy_count static_result harness_result
    local reports="${CARGO_TARGET_DIR:-$REPO/target}/ui-harness"
    out="$LOGS/ui-$(date +%Y-%m-%d_%H%M%S)"
    mkdir -p "$out"
    cargo clippy -p pleochroma-cut --features ui-harness --test ui_harness 2>&1 \
        | tee "$out/ui-clippy.log"
    clippy_status=${PIPESTATUS[0]}
    [ "$clippy_status" -eq 0 ] || status=1
    # cargo ui-static
    cargo test -p pleochroma-cut --test ui_static 2>&1 | tee "$out/ui-static.log"
    static_status=${PIPESTATUS[0]}
    [ "$static_status" -eq 0 ] || status=1
    # A report an earlier run left must not pass for this run's when the harness writes none.
    rm -f "$reports/report.md" "$reports/report.json"
    # cargo ui-harness
    # The harness runs in `timeout`'s own process group, so the hard stop reaches the test binary
    # and uiprobe's workers under cargo, but a Ctrl-C at the terminal does not: this function (a
    # subshell of the part's pipeline) kills that group itself, as retest.sh's `step` does.
    timeout --kill-after=60s 3h cargo test -p pleochroma-cut --features ui-harness \
        --profile harness --test ui_harness > >(tee "$out/ui-harness.log") 2>&1 &
    harness_pid=$!
    trap 'kill -KILL -- "-$harness_pid" 2>/dev/null; exit 130' INT TERM HUP
    wait "$harness_pid" 2>/dev/null
    harness_status=$?
    trap - INT TERM HUP
    # The log's `tee` (a process substitution) can still be writing its last lines.
    sleep 2
    [ "$harness_status" -eq 0 ] || status=1
    if [ "$harness_status" -eq 124 ] || [ "$harness_status" -eq 137 ]; then
        harness_note=" (hard stop 3h)"
    fi
    cp -f "$reports/report.md" "$reports/report.json" "$out/" 2>/dev/null
    [ -f "$out/report.md" ] || harness_note="$harness_note, no report written"
    clippy_count=$(warning_count "$out/ui-clippy.log")
    static_result=$(grep -E '^test result:' "$out/ui-static.log" | tail -n 1)
    harness_result=$(grep -E '^uiprobe: (PASS|FAIL|ABORT)' "$out/ui-harness.log" | tail -n 1)
    {
        echo "# UI checks, default build, $(date '+%Y-%m-%d %H:%M')"
        echo
        echo "| check | exit | result |"
        echo "| --- | --- | --- |"
        echo "| clippy of the ui_harness target | $clippy_status | $clippy_count warning(s) |"
        echo "| static .slint scan (ui_static) | $static_status | $static_result |"
        echo "| UI harness$harness_note | $harness_status | $harness_result |"
        echo
        grep -E '^uiprobe: coverage' "$out/ui-harness.log"
    } > "$out/summary.md"
    cat "$out/summary.md"
    return "$status"
}

# No --timeout for the retest: its own default (45m) and the per-step bounds apply, and a stalled
# step is ended by its stall detector.
part 1 retest retest bash scripts/retest.sh ${RETEST_ARGS[@]+"${RETEST_ARGS[@]}"}
# retest.sh exits 0 with red steps (its exit code reports only NO TESTS RAN): this row names them.
if [[ "$PARTS" == *",retest,"* ]]; then
    retest_folder=$(newest_folder retest)
    if [ -n "$retest_folder" ] && [ -f "$retest_folder/summary.md" ]; then
        echo "| retest red steps | $(red_steps "$retest_folder") | | |" >> "$SUMMARY"
    fi
fi
# --bench-typical: the two scenes for a machine whose default engines split a frame between the CPU
# and the GPU; the full set is all three scenes. Both m1 scenes stay in the typical set.
if [ "$BENCH_TYPICAL" -eq 1 ]; then
    part 2 bench bench-render-accel bash scripts/bench_render_accel.sh \
        --scene s2_sapphire --scene s3_small ${BENCH_ARGS[@]+"${BENCH_ARGS[@]}"}
else
    part 2 bench bench-render-accel bash scripts/bench_render_accel.sh \
        ${BENCH_ARGS[@]+"${BENCH_ARGS[@]}"}
fi
part 3 cache-note cache-note cache_note_part
part 4 speed eng-speed bash scripts/eng_speed_experiments.sh ${SPEED_ARGS[@]+"${SPEED_ARGS[@]}"}
part 5 m1 m1 m1_part
part 6 ui ui ui_part
# The cold clippy of target/beam-tests can take longer than run_beam_tests.sh's default hard stop of
# 45 minutes per step, so the part passes 90m; its stall detector still ends a step that prints
# nothing for 20 minutes.
part 7 beam-check beam-tests bash scripts/run_beam_tests.sh --steps 00,00c --timeout 90m
# Clippy exits 0 on warnings (the workspace lints are `warn`): the red steps of part beam-check,
# then the warnings of its two clippy logs, counted (both print them in the short format,
# `file:line:column: warning:`); 0 means clean.
if [[ "$PARTS" == *",beam-check,"* ]]; then
    beam_check_folder=$(newest_folder beam-tests)
    if [ -n "$beam_check_folder" ] && [ -f "$beam_check_folder/summary.md" ]; then
        echo "| beam-check red steps | $(red_steps "$beam_check_folder") | | |" >> "$SUMMARY"
    fi
    for log in 00-clippy-beam 00c-ui-clippy-beam; do
        [ -n "$beam_check_folder" ] && [ -f "$beam_check_folder/$log.log" ] || continue
        echo "| beam-check $log | $(warning_count "$beam_check_folder/$log.log") warning(s) | | |" \
            >> "$SUMMARY"
    done
fi
part 8 on-request on-request bash scripts/run_tests_on_request.sh
part 9 beam-tests beam-tests bash scripts/run_beam_tests.sh ${BEAM_ARGS[@]+"${BEAM_ARGS[@]}"}

# Each part's own summary, after the table.
for entry in ${FOLDERS[@]+"${FOLDERS[@]}"}; do
    name="${entry%%:*}"
    folder="${entry#*:}"
    if [ -f "$folder/summary.md" ]; then
        {
            echo
            echo "## $name ($folder)"
            echo
            cat "$folder/summary.md"
        } >> "$SUMMARY"
    fi
done
rm -f "$START_MARK"

echo
cat "$SUMMARY"
echo "Logs: $OUT"
