#!/usr/bin/env bash
# Retest: the stall reproduction in three arms (02s, 02r and 02t), the changed twins (the
# companion direction match, the exit collection of the event wavefront and the staged kernel),
# the sorted wavefront (opt-in; steps 02 and 02d), the preview-only path cache, the GPU lifecycle
# turn, the beam gating (the default build has no beam tracer) and the handshake of version 0.11.0.
# Tests that passed in an earlier run and whose code has not changed are left out (list below);
# the tests that take over 60 s run on request instead
# (scripts/run_tests_on_request.sh), except the exit-splitting test (02x) and the GPU twin tests of
# 02g (the legacy wavefront's pipeline reuse, the event wavefront's exit collection and the two
# zoned-opal renders), which the changes above reach.
# Beam tracing is retired: no step tests it, and only step 01d (clippy with all
# features) compiles it; its tests run only with scripts/run_beam_tests.sh.
# Adaptive sampling is an optional build feature (`adaptive-sampling`, off by default): the regular
# steps build without it, so they test the default build's adaptive-off note and its refusal of live
# passes; step 01f lints the feature build every round, and --with-adaptive adds the steps 04a,
# 05c and 06e, which test it.
# A cargo test step that ran no test at all (a filter that matches nothing exits 0) is listed as
# NO TESTS RAN, and the script then exits 1. It is not checked after the clippy steps, the
# test-binary builds and the 02h harness.
# A clippy step exits 0 on warnings (the workspace lints are `warn`): the row after it in the
# summary counts its warnings (0 means clean; cargo's per-crate totals are not counted).
# Ctrl-C, TERM or a closed terminal kill the running step and list it as interrupted.
#
# Usage (from the repository root, or with --repo):
#   bash scripts/retest.sh [--repo DIR] [--steps LIST] [--timeout DURATION]
#                          [--stall SECONDS] [--gate-repro N] [--hang-repro N]
#                          [--exit-repro N] [--with-adaptive] [--with-long]
#
#   --repo DIR          repository root (default: the current directory)
#   --steps LIST        run only the steps whose number is in the comma list (for example
#                       `01,02,02b,02d`); default: every step. The test binary of step 02 is
#                       built whenever a listed step uses it (02b, 02c, 02x, 02d, 02g, 02i)
#   --timeout DURATION  hard stop per step that sets none of its own (default 45m; `timeout` syntax)
#   --stall SECONDS     a step whose log gets no new line for this long is stalled: the states of
#                       all its threads go to NN-<name>.stall.txt and the step is killed (default
#                       1200; the module test steps set 300, their test binary is built before)
#   --gate-repro N      also run step 02s N times: the stall reproduction (24 device lifecycles
#                       and 12 CPU tests on 12 threads) with the lifecycle turn ON (default 0)
#   --hang-repro N      also run step 02r N times: the same with the turn off
#                       (PLEOCHROMA_GPU_TEST_GATE=0), the control (default 0)
#   --exit-repro N      also run step 02t N times: the stall reproduction for the thread exits the
#                       lifecycle turn does not order (late exits of finished test threads,
#                       hybrid-frame helper threads, warm-up helper threads), 24 tests on 12
#                       threads, with the turn on (default 0)
#   --with-adaptive     also run the tests of the `adaptive-sampling` feature: steps 04a (pleochroma-cut),
#                       05c (pleochroma-worker) and 06e (pleochroma-cli), each built with it
#   --with-long         accepted and ignored: the long beam rows moved to
#                       scripts/run_beam_tests.sh --with-long
#
# Steps (each runs even when an earlier one failed):
#   01  clippy on the whole workspace, the default feature set (no beam tracer)
#   01b clippy on pleochroma alone with exactly the web GPU app's features (`gpu,hdr`)
#   01c clippy on the web apps for wasm32 (the megakernel's browser host, `gpu/frame/async_impl.rs`,
#       compiles nowhere else). Needs the wasm32-unknown-unknown standard library in the rustc
#       sysroot; without it the step prints a note and is listed as skipped, not red
#   01d clippy on the whole workspace with all features, the beam code
#       included, without --cfg beam_tests (the test builds of the beam gating)
#   01f clippy on the `adaptive-sampling` feature builds: pleochroma-cut, pleochroma-cli and
#       pleochroma-worker (`worker,adaptive-sampling`), three runs, every round (the feature is
#       off in every other step)
#   01e clippy on pleochroma-worker with `worker` and without `beam` (the
#       default build's refusal of beam tiles)
#   02  the pleochroma test binary is built (its own step: cargo prints nothing while it compiles
#       the crate), then the CPU tests: the path cache (optical-depth records, lattice pre-pass,
#       build rule), the buffer layouts, every shader unit (naga; the sorted wavefront's two units
#       in `renderer::shader_validation_sorted_tests`), the CPU tracer, the Ray-Traced CPU
#       tracer (`renderer::whitted::`, its stone-shadow leaf included), the GPU test
#       watchdog's and the lifecycle turn's own tests (`renderer::gpu::test_support`,
#       `renderer::gpu::lifecycle`; the filter also matches the repro module, whose tests are
#       ignored and do not run here), the GPU context's own tests and the adaptive renders, the
#       HDR map's yaw (`renderer::env_map::env_map_rotation::`, two metrics tests). No test
#       of this step acquires a GPU. GPU tests never share a step with CPU tests: a stalled driver
#       call blocks the whole test process
#   02b GPU step for the path cache's GPU units (source tests of phase B and the completion) and the
#       cached GPU pass with its completion twins, printed, one test at a time
#   02c the path cache's completion proof with its printed table (the cached mean per tile, the
#       paired decomposition), one test at a time
#   02x the CPU exit-splitting test (ignored as over 60 s; the companion direction match
#       reaches it), alone
#   02d GPU only: the megakernel frame renderer and its variants (the staged kernel and the
#       sorted wavefront with their twins, `renderer::gpu::frame::sorted_tests`), the event
#       wavefront with its twin tests, the transport, estimator and RNG checks, the Whitted pass,
#       the live pass, the zoned, tool, resident and merged renders, the GPU tool checks
#   02e the CPU tracer's integration goldens (`raytracer_tests`, the Zircon test's paired means
#       over 65,536 seeds included) and `white_point`
#   02g the event-wavefront twin tests and the exit-collection twin once more with the tier of
#       every scene printed (ignored tests included), the legacy wavefront's pipeline reuse, the
#       event wavefront's exit collection against the megakernel and the two zoned-opal renders
#       (the last four ignored as over 60 s)
#   02i the per-scene twin diagnostic of each material class (megakernel against event wavefront,
#       each against itself, megakernel against its GENERIC build); asserts nothing
#   02h the GPU equivalence harness (probe profile, built first): the chunk, pipeline, wavefront and
#       specialisation checks and the exit-splitting checks; writes gpu_harness.json
#   02s (--gate-repro N) the stall reproduction with the lifecycle turn on, N times
#   02r (--hang-repro N) the same with the turn off (PLEOCHROMA_GPU_TEST_GATE=0), N times
#   02t (--exit-repro N) the thread-exit reproduction (renderer::gpu::lifecycle_exit_repro_tests)
#                        with the turn on, N times
#   04  pleochroma-cut: the scene a stone that is not closed sends (no Stone Shadow, no Tray
#       Reflection: scene_snapshot, the remote dispatch's scene_state, the live dispatch), the
#       export worker (lanes, prepared rays, the beam-off note and the adaptive-off note), the
#       hybrid split, the path-cache run, the viewport cache, the render context, the render jobs,
#       the Help manual, Preferences, start-up settings, the settings file (with its beam keys),
#       output defaults, ETA, export queue, saved lighting, the tilt profile cache, the optical
#       overlay snapshot, the metrics worker, the RADV workaround; 04d the two ignored hybrid
#       end-to-end tests with the app's `gpu` feature
#   04p pleochroma-cut, the Color Pattern Editor and the Rough Planner:
#       the pattern window's presenter and pictures (gui::color_pattern), the planner's presenter,
#       steps, legend, scan report and view (gui::rough_plan), the scan import report (mesh_io),
#       and the theme-token guard test (tests/theme_tokens.rs)
#   04q pleochroma-editor: the tutorial lessons whose texts follow the pattern
#       window (guide::tutorials)
#   05a pleochroma-net: the handshake (BUILD_ID of version 0.11.0)
#   05b pleochroma-worker: the serve, coordinator, validate and render_core modules (beam gating
#       with the default build's refusal of beam tiles, render round trip, cached live pass), the
#       HDR map's orientation in the asset path, the RADV workaround flag
#   05d pleochroma-net: the wire scene and the messages (protocol 42: HDR orientation, estimator)
#   05e pleochroma-net: the `scene_roundtrip` integration test (the scene's byte layout)
#   05f pleochroma-render-jobs: the job file formats 5 and 6 (codec, validation, runner)
#   06d pleochroma-cli: render_job (with a job file that asks for beams), args, help, image_compare,
#       the RADV workaround flag
#   06f pleochroma-web-core: the HDR map's turn (worker protocol 32, scene spec, settings, solve)
#   06g pleochroma-mobile-core: the HDR map's turn (lighting settings, remote scene, analysis)
#
# Left out (they passed in an earlier run and their code has not changed): the modules
# `renderer::adaptive::` except `tests_render`, `optics::raytracer::environment::tray_shadow::` as a
# filter of its own, `optics::birefringence::`, `color::metrics::attribution`;
# `--test lighting_gallery`; the beam rows (scripts/run_beam_tests.sh; the beam-only HDR turn
# tests included); the extended
# exit-collection table (02f); the rest of pleochroma-net, web-core and mobile-core; cut-core
# girdle and the editor's PDF tests (06a, 06b, 06c).
#
# Output: <repo>/owner-run-logs/retest-<timestamp>/
#   NN-<name>.log        full output per step
#   NN-<name>.stall.txt  only for a stalled step: memory, GPU, and per thread its state, CPU time,
#                        kernel wait channel and system call; for a futex wait the owner of the
#                        lock or the thread it joins; the code addresses on the stack of each
#                        blocked thread and of a joined thread, with their function names; the
#                        threads still using CPU; the memory map (plus gdb backtraces where ptrace
#                        is allowed)
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
GATE_REPRO=0
HANG_REPRO=0
EXIT_REPRO=0
WITH_ADAPTIVE=0
WITH_LONG=0
while [ $# -gt 0 ]; do
    case "$1" in
        --repo) REPO="$2"; shift 2 ;;
        --steps) STEPS=",$2,"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        --stall) STALL="$2"; shift 2 ;;
        --gate-repro) GATE_REPRO="$2"; shift 2 ;;
        --hang-repro) HANG_REPRO="$2"; shift 2 ;;
        --exit-repro) EXIT_REPRO="$2"; shift 2 ;;
        --with-adaptive) WITH_ADAPTIVE=1; shift ;;
        --with-long) WITH_LONG=1; shift ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

cd "$REPO" || { echo "no repo at $REPO" >&2; exit 2; }
# Sources that differ from the last run's get the current time, so cargo rebuilds what changed
# even when the copied files keep older times (scripts/fresh_sources.sh).
. "$REPO/scripts/fresh_sources.sh"
fresh_sources
export RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"
# The in-process GPU test watchdog reports an overdue phase and does not abort: the stall detector
# of `step` then writes every thread's state before it kills the step.
export PLEOCHROMA_GPU_TEST_WATCHDOG="${PLEOCHROMA_GPU_TEST_WATCHDOG:-report}"
# The engine switches never come from the caller's shell: every GPU twin runs the production
# engine (the staged megakernel for a plain class, step 02d); the staged twins compare it with the
# single megakernel.
unset PLEOCHROMA_GATED_LOOP PLEOCHROMA_STAGED PLEOCHROMA_SORTED PLEOCHROMA_GPU_PIPELINE
OUT_DIR="$REPO/owner-run-logs/retest-$(date +%Y-%m-%d_%H%M%S)"
mkdir -p "$OUT_DIR"
SUMMARY="$OUT_DIR/summary.md"
{
    echo "# Retest of round 2026-11-11, $(date '+%Y-%m-%d %H:%M')"
    echo
    # The code that ran: the box's tree is copied over its clone, whose HEAD can stay the same
    # with other code. "-dirty" says the tree differs from HEAD; "version" is the workspace
    # version; "sources" is sources_hash (fresh_sources.sh): the hash of every file git sees
    # (owner-run-logs left out), the same on any machine.
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

# Filled by `load_maps` and `stack_scan` for the stall dump.
MAP_START=()
MAP_END=()
MAP_BASE=()
MAP_PATH=()
MAP_LOW=0
MAP_HIGH=0
SCAN=()
declare -A STACK_HITS=()

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

# peek PID ADDRESS: sets PEEK to the unsigned 32-bit value at ADDRESS in the memory of process PID,
# or to the empty string. The memory file is opened by this shell itself (an ancestor of the
# stalled process, which a box with restricted ptrace requires; a `$(...)` would open it in a
# child) and opened anew for every read, because `dd` seeks relative to the descriptor's position.
peek() {
    PEEK=""
    { exec 9< "/proc/$1/mem"; } 2>/dev/null || return 0
    read -r PEEK < <(timeout -s KILL 5 dd bs=1 skip="$(( $2 ))" count=4 status=none <&9 2>/dev/null \
        | od -An -tu4) 2>/dev/null
    exec 9<&-
    return 0
}

# futex_note PID CALL: for a thread inside the futex system call (202), a line saying what it waits
# for: a priority-inheritance mutex (its lock word holds the owner's thread id), a thread it joins
# (`pthread_join` waits on the thread id in the joined thread's descriptor, a shared wait), or a
# lock word (if that is a glibc mutex, the word 8 bytes further is its owner's thread id).
futex_note() {
    local pid="$1" number address op value kind
    # shellcheck disable=SC2086
    set -- $2
    number="${1:-}"
    address="${2:-0}"
    op="${3:-0}"
    value="${4:-0}"
    [ "$number" = 202 ] || return 0
    kind=$(( op & 0x7f ))
    case "$kind" in
        6|13)
            peek "$pid" "$address"
            if [ -n "$PEEK" ]; then
                echo "    waits for the priority-inheritance mutex at $address, lock word $PEEK: owned by thread $(( PEEK & 0x3fffffff ))"
            else
                echo "    waits for the priority-inheritance mutex at $address (lock word unreadable)"
            fi
            ;;
        0|9)
            if [ $(( op & 0x80 )) -eq 0 ] && [ -d "/proc/$pid/task/$(( value ))" ]; then
                echo "    joins thread $(( value )) (pthread_join)"
            elif [ "$kind" -eq 0 ]; then
                peek "$pid" "$(( address + 8 ))"
                echo "    waits on the lock word at $address (expected value $(( value ))); if it is a glibc mutex, its owner is thread ${PEEK:-unreadable}"
            fi
            ;;
    esac
    return 0
}

# load_maps PID: the executable file mappings of process PID, one entry each in MAP_START and
# MAP_END (decimal addresses), MAP_PATH (the file) and MAP_BASE (where the file's offset 0 is
# loaded: an address minus the base is the address `addr2line` and `nm` know); MAP_LOW and MAP_HIGH
# bound them all.
load_maps() {
    local range perms offset device inode path
    local -A base=()
    MAP_START=()
    MAP_END=()
    MAP_BASE=()
    MAP_PATH=()
    MAP_LOW=0
    MAP_HIGH=0
    while read -r range perms offset device inode path; do
        [[ "$path" == /* ]] || continue
        if [ -z "${base[$path]:-}" ]; then
            base[$path]=$(( 0x${range%-*} - 0x$offset ))
        fi
        [[ "$perms" == *x* ]] || continue
        MAP_START+=("$(( 0x${range%-*} ))")
        MAP_END+=("$(( 0x${range#*-} ))")
        MAP_BASE+=("${base[$path]}")
        MAP_PATH+=("$path")
        if [ "$MAP_LOW" -eq 0 ]; then
            MAP_LOW=$(( 0x${range%-*} ))
        fi
        MAP_HIGH=$(( 0x${range#*-} ))
    done < <(timeout -s KILL 5 cat "/proc/$1/maps" 2>/dev/null)
}

# stack_scan PID ADDRESS WORDS: reads WORDS 8-byte words of process PID from ADDRESS up and sets
# SCAN to those that point into an executable file mapping (`load_maps`), in order, as
# file+0xoffset. On a stack these are return addresses (stale ones included). Every hit is also
# added to STACK_HITS for `addr2line`. The memory file is opened by this shell, as in `peek`.
stack_scan() {
    local pid="$1" word index
    SCAN=()
    [ "${#MAP_START[@]}" -gt 0 ] || return 0
    { exec 9< "/proc/$pid/mem"; } 2>/dev/null || return 0
    while read -r word; do
        (( word >= MAP_LOW && word < MAP_HIGH )) || continue
        for index in "${!MAP_START[@]}"; do
            if (( word >= MAP_START[index] && word < MAP_END[index] )); then
                printf -v word '0x%x' "$(( word - MAP_BASE[index] ))"
                SCAN+=("${MAP_PATH[index]##*/}+$word")
                STACK_HITS["${MAP_PATH[index]}"]+=" $word"
                break
            fi
        done
    done < <(timeout -s KILL 10 dd bs=8 skip="$(( $2 / 8 ))" count="$3" status=none <&9 2>/dev/null \
        | od -An -v -td8 -w8)
    exec 9<&-
    return 0
}

# stack_note PID CALL: for a thread blocked in a system call (its line in `syscall` ends with the
# stack pointer and the program counter), the first 40 code addresses on its stack, innermost
# first. For a thread that joins another (see `futex_note`), also the last 48 code addresses in
# the top 64 KiB of the joined thread's stack, outermost last: the thread descriptor the join
# waits on sits at the top of that stack, and a running thread shows no stack pointer of its own.
stack_note() {
    local pid="$1" address op value hit first
    # shellcheck disable=SC2086
    set -- $2
    [[ "${8:-}" == 0x* ]] || return 0
    address="${2:-0}"
    op="${3:-0}"
    value="${4:-0}"
    stack_scan "$pid" "$8" 2048
    if [ "${#SCAN[@]}" -gt 0 ]; then
        for hit in "${SCAN[@]:0:40}"; do
            echo "      on its stack: $hit"
        done
    fi
    if [ "$1" = 202 ] && [ $(( op & 0x80 )) -eq 0 ] && [ -d "/proc/$pid/task/$(( value ))" ]; then
        stack_scan "$pid" "$(( address - 65536 ))" 8192
        if [ "${#SCAN[@]}" -gt 0 ]; then
            first=$(( ${#SCAN[@]} > 48 ? ${#SCAN[@]} - 48 : 0 ))
            for hit in "${SCAN[@]:first}"; do
                echo "      on the stack of the joined thread: $hit"
            done
        fi
    fi
    return 0
}

# dump_stall NAME FILE SECONDS: what the stalled step was doing, written to FILE. Per thread: id,
# name, state, CPU time in clock ticks (user+system), kernel wait channel and the whole system
# call line (202 = futex: a user-space lock; 16 = ioctl: inside a driver), a note on what a futex
# wait waits for (`futex_note`), the code addresses on its stack (`stack_note`, test binaries
# only), the kernel stack where readable (root); then the threads that used CPU time in the next
# two seconds, user and system apart (a thread spinning in user space or in the kernel), the
# function names at the stack addresses (`addr2line`), the memory map of the test binaries (to
# place the lock addresses in a library), and gdb backtraces where ptrace is allowed.
# It must run in the script's own shell and read the per-thread files with the shell's `read`: only
# an ancestor of the stalled process may read `syscall` and `mem` on a box with restricted ptrace.
# Every external tool that could block on the stalled process or the driver has a hard bound.
dump_stall() {
    local name="$1" out="$2" quiet="$3"
    local pid task tid comm stat_line state wchan call line lines exe path user system
    local -A user_before=() system_before=()
    STACK_HITS=()
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
            exe=$(timeout -s KILL 5 readlink "/proc/$pid/exe" 2>/dev/null)
            if [[ "$exe" == */target/* ]]; then
                load_maps "$pid"
            else
                MAP_START=()
            fi
            echo
            echo "## Process $pid ($comm)"
            echo "tid | name | state | CPU ticks user+system | kernel wait channel | system call (number, six arguments, stack pointer, program counter)"
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
                # After the command name's closing ')': state (1), ..., user time (12), system
                # time (13), both in clock ticks.
                # shellcheck disable=SC2086
                set -- ${stat_line##*) }
                state="${1:-?}"
                user_before[$tid]="${12:-0}"
                system_before[$tid]="${13:-0}"
                if [ "$tid" = "$pid" ]; then
                    comm="$comm (main thread)"
                fi
                printf '%s | %s | %s | %s | %s | %s\n' \
                    "$tid" "$comm" "$state" "${12:-0}+${13:-0}" "${wchan:-?}" "$call"
                futex_note "$pid" "$call"
                stack_note "$pid" "$call"
                lines=0
                while [ "$lines" -lt 12 ] && read -r line; do
                    printf '    %s\n' "$line"
                    lines=$((lines + 1))
                done 2>/dev/null < "$task/stack"
            done
        done
        sleep 2
        echo
        echo "## Threads that used CPU time in the 2 s after the first reading (clock ticks, usually 10 ms)"
        for pid in $(group_pids "$RUNNER"); do
            for task in /proc/"$pid"/task/[0-9]*; do
                [ -d "$task" ] || continue
                tid="${task##*/}"
                stat_line=""
                { read -r stat_line < "$task/stat"; } 2>/dev/null
                # shellcheck disable=SC2086
                set -- ${stat_line##*) }
                user=$(( ${12:-0} - ${user_before[$tid]:-${12:-0}} ))
                system=$(( ${13:-0} - ${system_before[$tid]:-${13:-0}} ))
                if [ $(( user + system )) -gt 0 ]; then
                    echo "$tid (process $pid): $user user, $system system"
                fi
            done
        done
        if command -v addr2line > /dev/null 2>&1; then
            for path in "${!STACK_HITS[@]}"; do
                echo
                echo "## Functions at the stack addresses in $path"
                # shellcheck disable=SC2086
                timeout -s KILL 300 addr2line -a -f -C -p -e "$path" ${STACK_HITS[$path]} 2>&1 \
                    | sort -u | head -n 600
            done
        fi
        for pid in $(group_pids "$RUNNER"); do
            exe=$(timeout -s KILL 5 readlink "/proc/$pid/exe" 2>/dev/null)
            if [[ "$exe" == */target/* ]]; then
                echo
                echo "## Memory map of process $pid (to place the lock addresses above in a library)"
                timeout -s KILL 5 cat "/proc/$pid/maps" 2>&1 | head -n 3000
            fi
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

# LAST_STATUS holds the exit code of the step that ran last.
LAST_STATUS=""

# step NAME COMMAND...: runs COMMAND with a hard stop ($TIMEOUT) and a stall detector ($STALL
# seconds without a new log line: the thread states go to NAME.stall.txt and the step is killed),
# logs it and records exit code and minutes. A step sets its own bounds with
# `TIMEOUT=10m STALL=300 step ...`. A step left out by --steps is listed as skipped.
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
    # Tenths of a minute, so that a round of a few seconds is not shown as zero.
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

# passed_count LOG: the sum of the N of every "test result: ... N passed" line of LOG.
passed_count() {
    sed -n 's/^test result: .* \([0-9][0-9]*\) passed;.*/\1/p' "$1" | awk '{ n += $1 } END { print n + 0 }'
}

# check_ran NAME: after a step that runs tests (not after the clippy steps, the
# `cargo test --no-run` builds or the 02h harness). Sums the N of every "test result: ... N passed"
# line of the step's log; a step that ran, ended with exit code 0 and ran no test at all (a filter
# that matches nothing exits 0) is listed in the summary and makes the script exit 1.
NO_TESTS=0
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
        echo "| $name | NO TESTS RAN (does the filter match?) | 0 |" >> "$SUMMARY"
        NO_TESTS=1
    fi
}

# warning_count LOG: the warnings in a cargo clippy log: every line that starts one (`warning: ...`
# in the human format, `file:line:column: warning: ...` in the short one), without cargo's
# per-crate totals (`warning: `crate` (lib) generated N warnings`) and its "build failed" line.
warning_count() {
    grep -E '^warning|: warning:' "$1" \
        | grep -cvE '^warning: (`[^`]+` \(.+\) generated [0-9]+ warnings?|build failed)'
}

# warnings_of NAME: after a clippy step. Clippy exits 0 on warnings (the workspace lints are
# `warn`), so the step's own row cannot show them: this row counts the step's warnings
# (`warning_count`); 0 means clean. After a red step (a build error) the count is incomplete.
warnings_of() {
    local log="$OUT_DIR/$1.log"
    [ -f "$log" ] || return 0
    local count
    count=$(warning_count "$log")
    if [ "$count" -eq 0 ]; then
        # As in check_ran: the step's `tee` can still be writing the log's last lines.
        sleep 2
        count=$(warning_count "$log")
    fi
    if [ "$LAST_STATUS" = 0 ]; then
        echo "| $1 | $count warning(s) | 0 |" >> "$SUMMARY"
    else
        echo "| $1 | $count warning(s) before clippy failed | 0 |" >> "$SUMMARY"
    fi
    # A build artifact that rustc cannot load (E0786 "found invalid metadata files", E0514
    # "compiled by an incompatible version of rustc") is no code error: a second row names it and
    # the repair (for example a stale libderive_more_impl-*.so).
    local unreadable
    unreadable=$(sed -n 's/^ *= note: invalid metadata version found: //p' "$log" | sort -u | tr '\n' ' ')
    if [ -n "$unreadable" ] || grep -qE '^error\[E0514\]' "$log"; then
        echo "| $1 | unreadable build artifact ${unreadable:-(E0514, see the log)}: cargo clean -p PACKAGE (or rm -f the file), then --steps ${1%%-*} | 0 |" >> "$SUMMARY"
    fi
}

LIB_FEATURES=gpu,path-cache-disk

# The default build: no beam tracer is compiled in any step below, except 01d.
step 01-clippy cargo clippy --workspace --all-targets
warnings_of 01-clippy
# The pleochroma crate alone with exactly the web GPU app's features (gpu,hdr), which the workspace
# run cannot see.
step 01b-clippy-gpu-without-beam cargo clippy -p pleochroma --features gpu,hdr --all-targets
warnings_of 01b-clippy-gpu-without-beam
# The megakernel's browser host (`gpu/frame/async_impl.rs`) compiles for wasm32 only. Needs the
# wasm32-unknown-unknown standard library; without it the step is skipped with a note. The check
# looks in the active compiler's sysroot, not at `rustup`, so a self-built or system toolchain
# with the wasm target counts too.
wasm32_sysroot=$(rustc --print sysroot 2>/dev/null || true)
if [ -n "$wasm32_sysroot" ] && [ -d "$wasm32_sysroot/lib/rustlib/wasm32-unknown-unknown/lib" ]; then
    step 01c-clippy-web-gpu cargo clippy -p pleochroma-web -p pleochroma-web-gpu \
        -p pleochroma-web-compute --target wasm32-unknown-unknown
    warnings_of 01c-clippy-web-gpu
else
    skip_step 01c-clippy-web-gpu \
        "wasm32-unknown-unknown standard library not in ${wasm32_sysroot:-the rustc sysroot}" \
        "install it with: rustup target add wasm32-unknown-unknown (or the system Rust's wasm target)"
fi
# Every crate with all features, the beam code included,
# but without --cfg beam_tests. Test builds that have the beam code and no beam tests compile in no
# other step (orphaned test helpers; the cut's cfg_attr(not(all(test, beam_tests)),
# expect(dead_code))
# at worker/core/beam.rs:562), nor do mobile-core's `device-gpu` path or the worker's and CLI's
# `gpu` features. A hard stop of 90 minutes; the stall detector still
# ends a step that prints nothing for 20 minutes.
TIMEOUT=90m step 01d-clippy-all-features cargo clippy --workspace \
    --exclude pleochroma-mobile-android --all-targets --all-features
warnings_of 01d-clippy-all-features
# The worker's `worker` feature without `beam`. No workspace member enables
# it, so 01 lints the worker without serve/connection/requests.rs, and 01d brings `worker` only
# together with `beam`: the default build's refusal of beam tiles is linted nowhere else.
step 01e-clippy-worker cargo clippy -p pleochroma-worker --features worker --all-targets
warnings_of 01e-clippy-worker
# Adaptive sampling is a non-default feature of the cut, the CLI and the worker: no other step
# compiles it, so each crate is linted with it here, every round.
step 01f-clippy-adaptive-cut cargo clippy -p pleochroma-cut --all-targets --features adaptive-sampling
warnings_of 01f-clippy-adaptive-cut
step 01f-clippy-adaptive-cli cargo clippy -p pleochroma-cli --all-targets --features adaptive-sampling
warnings_of 01f-clippy-adaptive-cli
step 01f-clippy-adaptive-worker cargo clippy -p pleochroma-worker --all-targets \
    --features worker,adaptive-sampling
warnings_of 01f-clippy-adaptive-worker

# The lib test binary of every 02* step but 02e (the `raytracer_tests` and `white_point` targets)
# and 02h (the probe-profile harness), built on its own: cargo prints nothing while it compiles the
# crate, which the 300 s stall bound of the test steps would take for a stall. It is built whenever
# a step that uses it runs, also when --steps leaves out 02 itself.
if [ -z "$STEPS" ] || [[ "$STEPS" =~ ,02[bcxdgi]?, ]]; then
    STEPS="" step 02-build-test-binary cargo test --no-run -p pleochroma --lib \
        --features "$LIB_FEATURES"
else
    step 02-build-test-binary cargo test --no-run -p pleochroma --lib --features "$LIB_FEATURES"
fi

# Module filters are substring matches of the test paths (`renderer::path_cache::` does not match
# `renderer::gpu::path_cache::`). No test of this step acquires a GPU.
TIMEOUT=15m STALL=300 step 02-pleochroma-cpu cargo test --no-fail-fast -p pleochroma --lib \
    --features "$LIB_FEATURES" -- \
    renderer::path_cache:: renderer::buffers:: \
    renderer::shader_validation_tests renderer::shader_validation_sorted_tests \
    renderer::gpu::test_support renderer::gpu::lifecycle \
    renderer::gpu::context:: renderer::adaptive::tests_render optics::raytracer:: \
    renderer::whitted:: renderer::env_map::env_map_rotation:: \
    color::metrics::cache::tests::metrics_cache_key_follows_the_hdr \
    color::metrics::lighting::tests::a_turned_hdr_map
check_ran 02-pleochroma-cpu
# The GPU units of the path cache, and the cached GPU pass with its completion twins (the only
# tests that run gpu/path_cache/pass.rs on a GPU), printed, one test at a time.
TIMEOUT=20m STALL=300 step 02b-path-cache-gpu env PLEOCHROMA_REQUIRE_GPU=1 cargo test \
    --no-fail-fast -p pleochroma --lib --features "$LIB_FEATURES" -- \
    renderer::gpu::path_cache:: renderer::gpu_backend::cache_tests \
    renderer::gpu_backend::cached_tests:: --nocapture --test-threads 1
check_ran 02b-path-cache-gpu
# The completion proof with its printed table (the cached means and the paired decomposition).
TIMEOUT=20m STALL=300 step 02c-path-cache-proofs cargo test --no-fail-fast -p pleochroma --lib \
    --features "$LIB_FEATURES" -- \
    renderer::path_cache::tests_shade_residual::the_completion_is_unbiased \
    --nocapture --test-threads 1
check_ran 02c-path-cache-proofs
# The CPU exit-splitting test (ignored: over 60 s). The companion direction match reaches
# it: Rutile, Zircon and Moissanite, a bias that scales with the tolerance, narrow_compat. A single
# test prints nothing while it runs, hence the stall bound of 600 s.
TIMEOUT=20m STALL=600 step 02x-exit-splitting cargo test --no-fail-fast -p pleochroma --lib \
    --features "$LIB_FEATURES" -- --ignored --exact --nocapture \
    optics::raytracer::transport::tests::exit_splitting_tests::exit_splitting_keeps_the_mean_within_a_percent_and_does_not_add_variance
check_ran 02x-exit-splitting
# PLEOCHROMA_GATED_LOOP, PLEOCHROMA_STAGED and PLEOCHROMA_SORTED are unset at the top of this
# script, so every GPU twin runs the production engine (the staged megakernel); the twins of the
# staged kernel and of the sorted wavefront switch their engine per renderer. The tests of
# `gpu_backend/whitted_native_tests.rs` run as `renderer::gpu_backend::whitted::native::tests`. The
# zoned, tool, resident and merged renders and the GPU tool checks are the renderer's other users
# (the lifecycle turn's teardown inside the turn, `prepare_turn` and the batch stepper, the shared
# bounce text, the companion direction match as GPU-vs-CPU twins).
TIMEOUT=20m STALL=300 step 02d-megakernel env PLEOCHROMA_REQUIRE_GPU=1 cargo test --no-fail-fast \
    -p pleochroma --lib --features "$LIB_FEATURES" -- \
    renderer::gpu::frame:: renderer::gpu::event_wavefront:: renderer::gpu::transport_check \
    renderer::gpu::estimator_check renderer::gpu::rng_check renderer::gpu::whitted:: \
    renderer::gpu_backend::live_tests renderer::gpu_backend::tests \
    renderer::gpu_backend::estimator_tests renderer::gpu_backend::whitted:: \
    renderer::gpu_backend::zoned_tests renderer::gpu_backend::tool_tests \
    renderer::gpu_backend::resident_tests renderer::gpu::merge_tests::gpu_hardware_tests \
    renderer::gpu::tool_check::tests::gpu_
check_ran 02d-megakernel
step 02e-cpu-goldens cargo test --no-fail-fast -p pleochroma --features "$LIB_FEATURES" \
    --test raytracer_tests --test white_point
check_ran 02e-cpu-goldens
# The twin tests, printed: every scene's tier (Exact, Golden, Rounding), and the exit-collection
# twin of the megakernel with its figures. `ew_matches_megakernel_per_material_class` is ignored
# (over 60 s), so 02d skips it and it runs only here. The legacy wavefront compiles the
# shared arm text; its pipeline-reuse test is ignored (over 60 s) and
# runs only here. So do the exit-collection twin and the two zoned-opal renders, ignored since they
# ran over 60 s in 02d.
TIMEOUT=15m STALL=300 step 02g-ew-twins env PLEOCHROMA_REQUIRE_GPU=1 cargo test --no-fail-fast \
    -p pleochroma --lib --features "$LIB_FEATURES" -- \
    ew_matches_megakernel_per_material_class ew_traces_every_tool_primitive_like_the_megakernel \
    the_megakernel_collects_the_cpu_leaves legacy_wavefront_reuses_only_matching_dispersion_pipelines \
    ew_exit_collection_follows_the_megakernel a_zoned_opal_renders_on_the_gpu_like_on_the_cpu \
    a_zoned_opal_without_reflections_renders_on_the_gpu_like_on_the_cpu \
    --include-ignored --nocapture --test-threads 1
check_ran 02g-ew-twins
# The per-scene diagnostic per material class. It asserts nothing; its lines decide whether the
# rounding tier is compiler rounding (verify TWINS 1.2, plan section 5.4).
for class in 1 2 3; do
    TIMEOUT=15m STALL=600 step "02i-ew-diagnostic-class$class" env PLEOCHROMA_REQUIRE_GPU=1 \
        PLEOCHROMA_EW_DIAG_CLASS="$class" cargo test --no-fail-fast -p pleochroma --lib \
        --features "$LIB_FEATURES" -- \
        ew_per_scene_diagnostic --ignored --nocapture --test-threads 1
    check_ran "02i-ew-diagnostic-class$class"
done

# The GPU equivalence harness, the only caller of the chunk, pipeline, wavefront and specialisation
# checks (equivalence.rs) and of the exit-splitting checks p6 (in 08_exit_splitting.wgsl;
# p6_exit_splitting.rs and its 1 mrad case). Built first in the probe profile (cargo prints nothing
# while it compiles). Exit codes: 0 every check passed, 1 a check failed, 2 no GPU adapter or an
# argument error (the log says which), 3 the JSON could not be written. No check_ran: it is not a
# test binary. A red check in a group no change touched is older than this run: report it.
step 02h-build-gpu-harness cargo build --profile probe -p pleochroma --features gpu \
    --example gpu_equivalence_harness
step 02h-gpu-harness cargo run --profile probe -p pleochroma --features gpu \
    --example gpu_equivalence_harness -- --json "$OUT_DIR/gpu_harness.json" \
    --date "$(date -u +%Y-%m-%d)"

# The stall (several threads creating and dropping GPU devices while finished test
# threads exit), in three arms. Arms 02s and 02r run the 36 tests of
# renderer::gpu::lifecycle_repro_tests (24 device lifecycles, 12 short CPU tests) on 12 threads;
# arm 02t runs the 24 tests of renderer::gpu::lifecycle_exit_repro_tests (thread exits the
# lifecycle turn does not order: late exits, hybrid-frame helpers, warm-up helpers) on 12 threads.
# A round takes seconds; a round that stalls is killed after 150 s of silence and leaves its
# thread states in <step>.stall.txt. Each arm first makes sure the test binary is built: cargo
# prints nothing while it compiles, which the 150 s would take for a stall.
#   02s: the lifecycle turn on (creation and teardown one thread at a time), the production path
#   02r: the turn off (PLEOCHROMA_GPU_TEST_GATE=0), the control
#   02t: the turn on, the thread exits the turn does not order (the teardown watchdog of the
#        turn reports a device teardown that takes too long)
REPRO_FILTER=renderer::gpu::lifecycle_repro_tests::
REPRO_EXIT_FILTER=renderer::gpu::lifecycle_exit_repro_tests::

# repro_build ARM: the test binary of the arm's rounds (seconds when it is built already).
repro_build() {
    step "$1-build-test-binary" cargo test --no-run -p pleochroma --lib --features "$LIB_FEATURES"
}

# repro_round NAME VARIABLE=VALUE...: one round of the mix with the variables set.
repro_round() {
    local name="$1"; shift
    # The hard stop leaves room for the stall dump (about 70 s) after the 150 s of silence.
    TIMEOUT=10m STALL=150 step "$name" env PLEOCHROMA_REQUIRE_GPU=1 "$@" cargo test \
        --no-fail-fast -p pleochroma --lib --features "$LIB_FEATURES" -- "$REPRO_FILTER" \
        --include-ignored --test-threads 12
    check_ran "$name"
}

if [ "$GATE_REPRO" -gt 0 ]; then
    repro_build 02s
    for round in $(seq 1 "$GATE_REPRO"); do
        repro_round "02s-gate-repro-$round" PLEOCHROMA_GPU_TEST_GATE=1
    done
fi
if [ "$HANG_REPRO" -gt 0 ]; then
    repro_build 02r
    for round in $(seq 1 "$HANG_REPRO"); do
        repro_round "02r-hang-repro-$round" PLEOCHROMA_GPU_TEST_GATE=0
    done
fi
if [ "$EXIT_REPRO" -gt 0 ]; then
    repro_build 02t
    for round in $(seq 1 "$EXIT_REPRO"); do
        TIMEOUT=10m STALL=150 step "02t-exit-repro-$round" env PLEOCHROMA_REQUIRE_GPU=1 \
            PLEOCHROMA_GPU_TEST_GATE=1 PLEOCHROMA_GPU_TEST_WATCHDOG=report cargo test \
            --no-fail-fast -p pleochroma --lib --features "$LIB_FEATURES" -- \
            "$REPRO_EXIT_FILTER" --include-ignored --test-threads 12
        check_ran "02t-exit-repro-$round"
    done
fi

step 04-pleochroma-cut cargo test --no-fail-fast -p pleochroma-cut --lib -- \
    bridge::export_thread::cache_run bridge::export_thread::batch \
    bridge::export_thread::hybrid_export_tests bridge::export_thread::worker::tests \
    bridge::export_thread::worker::types bridge::export_thread::worker::final_picture \
    bridge::export_thread::worker::core:: \
    bridge::render_thread::gpu_backend::dispatch bridge::render_thread::viewport_cache \
    bridge::render_thread::context:: bridge::render_thread::remote_lane \
    gui::render_jobs gui::progress_eta gui::preferences gui::startup_settings \
    gui::render::output_defaults gui::render::render_export::queue gui::help:: settings::model:: \
    bridge::export_thread::scene_snapshot bridge::export_thread::remote::dispatch::scene_state \
    gui::remote::orchestrator::tick::dispatch \
    bridge::optical_overlay::snapshot bridge::render_thread::metrics_worker \
    gui::render::lighting_presets gui::tilt::tilt_profile:: gui::radv_workaround
check_ran 04-pleochroma-cut

# The Color Pattern Editor and the Rough Planner: their presenters, the
# slice/thumbnail/LCh pictures, the planner's steps and scan import report, and the theme guard.
step 04p-cut-pattern-planner cargo test --no-fail-fast -p pleochroma-cut --lib -- \
    gui::color_pattern gui::rough_plan mesh_io
check_ran 04p-cut-pattern-planner
step 04p-theme-tokens cargo test --no-fail-fast -p pleochroma-cut --test theme_tokens
check_ran 04p-theme-tokens
step 04q-editor-tutorials cargo test --no-fail-fast -p pleochroma-editor --lib -- guide::tutorials
check_ran 04q-editor-tutorials

# The hybrid CPU+GPU rule end to end. Built with the app's `gpu` feature (off
# by default), or the stub backend would decline and nothing hybrid would run: calibration (it
# settles on the GPU alone on a fast GPU) and a carried split of 0.5 (concurrent CPU+GPU batches).
step 04d-pleochroma-cut-hybrid-export env PLEOCHROMA_REQUIRE_GPU=1 cargo test --no-fail-fast \
    -p pleochroma-cut --lib --features gpu -- --include-ignored --exact \
    bridge::export_thread::hybrid_export_tests::export_completes_via_hybrid_or_cpu_path \
    bridge::export_thread::hybrid_export_tests::a_carried_split_runs_concurrent_batches_and_settles_on_a_valid_share
check_ran 04d-pleochroma-cut-hybrid-export

# The handshake with the BUILD_ID of version 0.11.0 (two of its tests need `render`).
step 05a-net-version cargo test --no-fail-fast -p pleochroma-net --features render --lib -- \
    handshake::
check_ran 05a-net-version

# The wire scene of protocol 42 (the HDR map's orientation, the estimator as the last byte, the JSON
# keys that are left out when unturned or Branching) and the messages that carry it, then its byte
# layout round trip (an integration test that needs `render`).
step 05d-net-scene cargo test --no-fail-fast -p pleochroma-net --features render --lib -- \
    scene:: messages::
check_ran 05d-net-scene
step 05e-net-scene-roundtrip cargo test --no-fail-fast -p pleochroma-net --features render \
    --test scene_roundtrip
check_ran 05e-net-scene-roundtrip

# The job files of formats 5 (color zones) and 6 (a turned HDR map): codec, validation, runner
# tests.
step 05f-render-jobs cargo test --no-fail-fast -p pleochroma-render-jobs --lib
check_ran 05f-render-jobs

# The modules of the worker's beam gating (serve, coordinator, validate, render_core) in the
# default `worker` build, which does not pull in pleochroma/beam.
step 05b-pleochroma-worker cargo test --no-fail-fast -p pleochroma-worker --features worker -- \
    serve:: coordinator:: validate:: render_core:: assets::tests::environment_source \
    radv_workaround::
check_ran 05b-pleochroma-worker

# `render_job`, `args` and `help` hold the estimator rule (the cache and beam flags make a run
# branching), the tokens, and the job file
# that asks for adaptive sampling (the default build renders it without and prints the note);
# `image_compare` the null glint recall; `radv_workaround` the --radv-workaround flag's parser and
# the root help's naming of it.
step 06d-cli cargo test --no-fail-fast -p pleochroma-cli -- render_job args image_compare help \
    radv_workaround
check_ran 06d-cli

# The HDR map's turn in the browser and mobile cores (native lib tests): the web worker protocol
# (version 32 and its byte pins), the scene spec, the render settings, the optical solve and the
# worker's HDR path; the mobile lighting settings, the remote scene and the analysis key.
step 06f-web-core cargo test --no-fail-fast -p pleochroma-web-core --lib -- \
    protocol:: scene::hdr_turn_tests scene::tests::an_hdr_scene settings::tests_hdr \
    solve::optical::hdr_turn_tests worker::hdr_tests
check_ran 06f-web-core
step 06g-mobile-core cargo test --no-fail-fast -p pleochroma-mobile-core --lib -- \
    design_props::tests_hdr_turn remote::rendering::tests analysis::tests hdr::tests \
    viewport::render_view::tests::dragging_with_an_hdr_map
check_ran 06g-mobile-core

# The tests of the `adaptive-sampling` feature (--with-adaptive): the cut's adaptive export, its
# path-cache run and live passes, the worker's live-pass serving, coordinator, cache and validation,
# and the CLI's adaptive flags and job handling. The default build's refusals run in 04, 05b, 06d.
if [ "$WITH_ADAPTIVE" -eq 1 ]; then
    step 04a-pleochroma-cut-adaptive cargo test --no-fail-fast -p pleochroma-cut --lib \
        --features adaptive-sampling -- \
        bridge::export_thread::worker::core::adaptive bridge::export_thread::cache_run \
        bridge::export_thread::remote::live_pass gui::render::output_defaults gui::render_jobs \
        bridge::render_thread::adaptive_view
    check_ran 04a-pleochroma-cut-adaptive
    step 05c-pleochroma-worker-adaptive cargo test --no-fail-fast -p pleochroma-worker \
        --features worker,adaptive-sampling -- \
        serve::tests::live_pass coordinator::tests::live_pass render_core::live validate::
    check_ran 05c-pleochroma-worker-adaptive
    step 06e-cli-adaptive cargo test --no-fail-fast -p pleochroma-cli \
        --features adaptive-sampling -- render_job args help
    check_ran 06e-cli-adaptive
fi

if [ "$WITH_LONG" -eq 1 ]; then
    echo "note: --with-long moved to scripts/run_beam_tests.sh --with-long"
fi

echo
cat "$SUMMARY"
echo "Logs: $OUT_DIR"
if [ "$NO_TESTS" -eq 1 ]; then
    echo "a step ran no test: see NO TESTS RAN in the summary" >&2
    exit 1
fi
