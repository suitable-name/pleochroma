#!/usr/bin/env bash
# Beam tracing is retired: this script runs on request only and builds its own CLI with
# the `beam` feature (target/beam); no regular script calls it.
#
# What does asking for beams cost when the renderer declines them? Renders each scene at its fixed
# sample cap six times with the beam quick-release CLI: on the default engines, --local gpu and
# --local cpu+gpu, each once plain and once with --beam, all with --verbose. No reference renders,
# no adaptive runs, no picture comparison: only the seconds and the log lines count. A plain row
# renders with exit collection and a beam row with branching (the --beam flags make the run
# branching), so the time rule below compares two estimators: it judges the decline roughly,
# not to the second.
#
# A beam row passes when
#   - its log has the "beam render abandoned" line and, before it, no pipeline or engine line
#     ("beam pipeline started", "beam batch traced", "beam batch finished", "GPU host-convergence
#     residual"): the probe decided before anything else ran;
#   - the probe took at most 4.5 s (the `seconds=` field of that line);
#   - the row took at most the plain row's time plus the probe plus 2 s plus 2 % of the plain row
#     (the run-to-run noise of two renders).
# A row whose beams were not declined is listed as "beams kept" and not judged. The CPU+GPU rows
# also list the split the calibration gave (`hybrid calibration ... split=`) and the split the first
# batch on both engines measured (`hybrid batch ... measured=`), plain against beam: when a beam row
# is slow, these say whether the CPU got too many samples at the start.
#
# Usage (from the repository root, or with --repo):
#   bash scripts/bench_beam_decline.sh [--repo DIR] [--scene ID]... [--no-build]
#
#   --repo DIR   repository root (default: the current directory)
#   --scene ID   a still scene of benchmarks/render_accel/scenes.json (repeatable; default:
#                s1_glints s2_sapphire s3_small)
#   --no-build   use the beam CLI already built in target/beam/quick-release
#
# Output: <repo>/owner-run-logs/beam-decline-<timestamp>/
#   build.log            the CLI build
#   <scene>/<row>.log    the CLI's output of each row (plain_default, beam_default, plain_gpu,
#                        beam_gpu, plain_hybrid, beam_hybrid), <row>.ms its wall time
#   summary.md           the table (send me this folder; the .png files are not needed)
#
# Exit code: 0 when every judged row passes, 1 otherwise. Needs jq, GNU date and bash 4.

set -u

# Headless boxes have no session runtime dir; libwayland prints an error line per Vulkan ICD probe
# without it. A private one silences it and changes nothing else.
if [ -z "${XDG_RUNTIME_DIR:-}" ]; then
    export XDG_RUNTIME_DIR="${TMPDIR:-/tmp}/pleochroma-xdg-$(id -u)"
    mkdir -p -m 700 "$XDG_RUNTIME_DIR"
fi

REPO="$(pwd)"
SCENES=()
BUILD=1
while [ $# -gt 0 ]; do
    case "$1" in
        --repo) REPO="$2"; shift 2 ;;
        --scene) SCENES+=("$2"); shift 2 ;;
        --no-build) BUILD=0; shift ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done
if [ ${#SCENES[@]} -eq 0 ]; then
    SCENES=(s1_glints s2_sapphire s3_small)
fi
# The split columns come from the lines the CLI's --verbose filter prints; a RUST_LOG exported on
# the box would replace that filter and leave them empty.
unset RUST_LOG

cd "$REPO" || { echo "no repo at $REPO" >&2; exit 2; }
# Sources that differ from the last run's get the current time, so cargo rebuilds what changed
# even when the copied files keep older times (scripts/fresh_sources.sh).
. "$REPO/scripts/fresh_sources.sh"
fresh_sources
# gpu_name prints the GPU's name on any machine (scripts/gpu_name.sh).
. "$REPO/scripts/gpu_name.sh"
command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; exit 2; }
BENCH="$REPO/benchmarks/render_accel"
OUT_DIR="$REPO/owner-run-logs/beam-decline-$(date +%Y-%m-%d_%H%M%S)"
mkdir -p "$OUT_DIR"
CLI="$REPO/target/beam/quick-release/pleochroma-cli"

if [ "$BUILD" -eq 1 ]; then
    echo "== build the CLI (quick-release, GPU, beam)"
    cargo build --profile quick-release -p pleochroma-cli --bins --target-dir "$REPO/target/beam" \
        --features gpu,beam,pleochroma/serde,pleochroma/hdr,pleochroma-net/compression 2>&1 | tee "$OUT_DIR/build.log"
    if [ "${PIPESTATUS[0]}" -ne 0 ]; then
        echo "the CLI build failed, see $OUT_DIR/build.log" >&2
        exit 1
    fi
fi
[ -x "$CLI" ] || { echo "no CLI at $CLI (build it without --no-build; it lives in target/beam/quick-release)" >&2; exit 1; }

# The longest the probe may take, the fixed noise allowance (both in milliseconds) and the share of
# the plain row's time allowed on top (one part in NOISE_SHARE).
PROBE_LIMIT_MS=4500
NOISE_MS=2000
NOISE_SHARE=50

# plain_text FILE: the file without the log's color codes.
plain_text() {
    sed 's/\x1b\[[0-9;]*m//g' "$1"
}

# field LOG MESSAGE KEY: the value of KEY on the first log line holding MESSAGE, or nothing.
field() {
    plain_text "$1" | grep -F -- "$2" | head -n 1 | sed -n "s/.*[[:space:]]$3=\([^[:space:]]*\).*/\1/p"
}

# seconds_of MILLISECONDS: seconds with one decimal; "-" for an empty value.
seconds_of() {
    if [ -z "$1" ]; then
        echo "-"
    else
        awk -v ms="$1" 'BEGIN { printf "%.1f", ms / 1000 }'
    fi
}

# render NAME FLAGS...: one row of the current scene; its log and wall time go to the scene folder.
# A --beam row renders branching through its own flag; a plain row runs the CLI's estimator.
render() {
    local name="$1"
    shift
    local start end status
    echo "> $name: render $RUN_JOB ${SCENE_ARGS[*]+"${SCENE_ARGS[*]}"} $* --verbose --sample-offset 0"
    start=$(date +%s%N)
    "$CLI" render "$RUN_JOB" --out "$FOLDER/$name.png" ${SCENE_ARGS[@]+"${SCENE_ARGS[@]}"} \
        "$@" \
        --verbose --sample-offset 0 >"$FOLDER/$name.log" 2>&1 </dev/null
    status=$?
    end=$(date +%s%N)
    if [ "$status" -eq 0 ]; then
        echo $(( (end - start) / 1000000 )) >"$FOLDER/$name.ms"
    else
        echo "$name: the CLI ended with exit code $status, see $FOLDER/$name.log" >&2
    fi
    return "$status"
}

ROWS=()
FAILED=0
for scene in "${SCENES[@]}"; do
    echo "== $scene"
    entry=$(jq -c --arg id "$scene" '.scenes[] | select(.id == $id and .kind == "still")' "$BENCH/scenes.json")
    if [ -z "$entry" ]; then
        echo "no still scene $scene in $BENCH/scenes.json" >&2
        ROWS+=("| $scene | | | | | | | | | | FAIL: unknown scene |")
        FAILED=1
        continue
    fi
    FOLDER="$OUT_DIR/$scene"
    mkdir -p "$FOLDER"
    spp=$(jq -r '.adaptive_max_spp' <<<"$entry")
    mapfile -t SCENE_ARGS < <(jq -r '.render_args[]' <<<"$entry")
    # The scene's job at the fixed sample cap, as the benchmark's baseline renders it.
    RUN_JOB="$FOLDER/fixed.job.json"
    sed -E "s/\"samples_per_pixel\":[[:space:]]*[0-9]+/\"samples_per_pixel\": $spp/" \
        "$BENCH/$(jq -r '.job' <<<"$entry")" >"$RUN_JOB"
    for engines in default gpu hybrid; do
        case "$engines" in
            default) flags=() ;;
            gpu) flags=(--compute local --local gpu) ;;
            hybrid) flags=(--compute local --local cpu+gpu) ;;
        esac
        render "plain_$engines" ${flags[@]+"${flags[@]}"}
        render "beam_$engines" ${flags[@]+"${flags[@]}"} --beam
        plain_ms=$(cat "$FOLDER/plain_$engines.ms" 2>/dev/null || true)
        beam_ms=$(cat "$FOLDER/beam_$engines.ms" 2>/dev/null || true)
        beam_log="$FOLDER/beam_$engines.log"
        plain_log="$FOLDER/plain_$engines.log"
        if [ -z "$plain_ms" ] || [ -z "$beam_ms" ]; then
            ROWS+=("| $scene | $engines | $(seconds_of "$plain_ms") | $(seconds_of "$beam_ms") | | | | | | | FAIL: a render failed |")
            FAILED=1
            continue
        fi
        over_ms=$(( beam_ms - plain_ms ))
        splits="$(field "$plain_log" 'hybrid calibration' split) / $(field "$beam_log" 'hybrid calibration' split)"
        measured="$(field "$plain_log" 'hybrid batch' measured) / $(field "$beam_log" 'hybrid batch' measured)"
        if ! plain_text "$beam_log" | grep -q -F 'beam render abandoned'; then
            ROWS+=("| $scene | $engines | $(seconds_of "$plain_ms") | $(seconds_of "$beam_ms") | $(seconds_of "$over_ms") | | | | $splits | $measured | beams kept (not judged) |")
            continue
        fi
        # The pipeline and engine lines above the decline line: there must be none.
        early=$(plain_text "$beam_log" | sed '/beam render abandoned/,$d' \
            | grep -c -E 'beam pipeline started|beam batch traced|beam batch finished|GPU host-convergence residual')
        probe_s=$(field "$beam_log" 'beam render abandoned' seconds)
        probe_ms=$(awk -v s="$probe_s" 'BEGIN { if (s == "") print ""; else printf "%d", s * 1000 }')
        allowed_ms=$(( ${probe_ms:-0} + NOISE_MS + plain_ms / NOISE_SHARE ))
        verdict=PASS
        if [ "$early" -ne 0 ]; then
            verdict="FAIL: $early pipeline or engine lines before the decline"
        elif [ -z "$probe_ms" ]; then
            verdict="FAIL: the decline line has no seconds"
        elif [ "$probe_ms" -gt "$PROBE_LIMIT_MS" ]; then
            verdict="FAIL: the probe took over $(seconds_of "$PROBE_LIMIT_MS") s"
        elif [ "$over_ms" -gt "$allowed_ms" ]; then
            verdict="FAIL: slower than the plain row by more than the probe and the noise"
        fi
        if [ "$verdict" != PASS ]; then FAILED=1; fi
        ROWS+=("| $scene | $engines | $(seconds_of "$plain_ms") | $(seconds_of "$beam_ms") | $(seconds_of "$over_ms") | $(seconds_of "$probe_ms") | $(seconds_of "$allowed_ms") | $early | $splits | $measured | $verdict |")
    done
done

{
    echo "# The price of a declined beam render, $(date '+%Y-%m-%d %H:%M')"
    echo
    # The code that ran: the box's tree is copied over its clone, whose HEAD can stay the same
    # with other code. "-dirty" says the tree differs from HEAD; "version" is the workspace
    # version; "sources" is sources_hash (fresh_sources.sh): the hash of every file git sees
    # (owner-run-logs left out), the same on any machine.
    echo "Repo: $REPO (git: $(git describe --always --dirty 2>/dev/null || echo unknown), version: $(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1), sources: $(sources_hash)), cores: $(nproc)"
    echo "GPU: $(gpu_name)"
    echo
    echo "| scene | engines | plain s | beam s | over s | probe s | allowed over s | lines before the decline | calibrated split plain / beam | first measured split plain / beam | verdict |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    printf '%s\n' "${ROWS[@]}"
    echo
    echo "over = beam seconds minus plain seconds on the same engines. allowed = the probe's own"
    echo "seconds + 2 s + 2 % of the plain row. The split columns are filled on the rows that split"
    echo "the samples between CPU and GPU (default and hybrid on a GPU machine): the GPU's share the"
    echo "1-sample calibration gave, and the share the first batch that ran on both engines measured."
} > "$OUT_DIR/summary.md"

echo
cat "$OUT_DIR/summary.md"
echo "Folder: $OUT_DIR"
exit "$FAILED"
