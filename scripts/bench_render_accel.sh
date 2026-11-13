#!/usr/bin/env bash
# Does the path cache (or, optionally, adaptive sampling) save time? Builds the CLI and runs the
# render-acceleration benchmark (benchmarks/render_accel/run.sh) on the still scenes, with the
# path-cache runs under --path-cache, then writes one verdict table per optimization.
# Beam tracing is retired: the CLI is built without it and no beam row runs, unless
# --with-beam asks for them (then the CLI is built with the `beam` feature into target/beam).
# Adaptive sampling is an optional build feature (`adaptive-sampling`, off by default): no adaptive
# row runs, unless --with-adaptive asks for them (then the CLI is built with that feature into
# target/adaptive; --with-beam's CLI has it already). target/quick-release is never overwritten.
#
# Per scene the benchmark renders two references (16k to 32k samples), a fixed-sample baseline at
# the adaptive cap, and with --path-cache a path-cache run at the cap. With --with-adaptive also
# four adaptive path-traced runs (two targets per metric, after an untimed warm-up) and two adaptive
# path-cache runs.
# With --with-beam also a beam run (with the adaptive residual) on the default engines, and the
# engine matrix (GPU, CPU+GPU: a plain run and a beam run each; the CPU row only with --matrix-cpu).
# Each beam and path-cache run is capped at 10 x the baseline + 120 s.
# Every run is judged by the benchmark's quality rule against the baseline (section 7.4). The
# summary sets each adaptive run and each path-cache run against the baseline, and with --with-beam
# the fastest beam run that passes against the fastest plain run that passes. A scene whose beams
# the renderer declined ("Beam tracing off") has no beam run to count: it prints "beams declined"
# with the seconds the declined run took, and the engine matrix marks such a row "(beams declined)".
#
# Usage (from the repository root, or with --repo):
#   bash scripts/bench_render_accel.sh [--repo DIR] [--scene ID]... [--timeout DURATION]
#                                      [--no-build] [--with-beam] [--matrix-cpu] [--path-cache] [--cache-keep]
#                                      [--cache-keep-scenes LIST]
#                                      [--with-adaptive] [--vram-gb N]
#
#   --with-adaptive     also run the adaptive rows (run.sh --adaptive: four plain adaptive rows and
#                       their warm-up, the adaptive path-cache rows),
#                       with a CLI built with the `adaptive-sampling` feature into target/adaptive
#                       (the plain adaptive rows stop only background tiles)
#   --no-adaptive       accepted and ignored (no adaptive row is the default)
#   --matrix-cpu        also run the --local cpu row of the engine matrix (hours per scene; only
#                       with --with-beam)
#   --path-cache        also run the path-cache runs (one at the cap; with --with-adaptive also two
#                       adaptive)
#   --cache-keep        also run the kept-cache pair (run.sh --cache-keep: keep_first builds and
#                       keeps the cache, keep_again loads it) on the scenes of --cache-keep-scenes;
#                       implies --path-cache
#   --cache-keep-scenes LIST  scenes that get the kept-cache pair, comma separated (default
#                       s2_sapphire,s3_small; s1_glints refuses the cache before it is built)
#   --with-beam         also run the beam run and the engine matrix (a CLI built with the
#                       `beam` feature into target/beam; the renderer may decline beams: see
#                       scripts/bench_beam_decline.sh, which measures the decline)
#   --no-beam           accepted and ignored (no beam row is the default)
#   --vram-gb N         free video memory the path cache may plan with, in GB (default: an exported
#                       PLEOCHROMA_CACHE_VRAM_GB, else the free memory nvidia-smi reports for the
#                       first GPU; without either the cache can only use system RAM)
#   --repo DIR          repository root (default: the current directory)
#   --scene ID          a scene of benchmarks/render_accel/scenes.json (repeatable; default:
#                       s1_glints s2_sapphire s3_small; s5_opal is left out because beams refuse a
#                       scattering stone, s6_8k because one 8K reference takes most of a day)
#   --timeout DURATION  hard stop per scene (default 10h; `timeout` syntax). A scene that runs out
#                       has no results.json; its folder still holds every finished render and its
#                       seconds.
#   --no-build          use the CLI already built in target/quick-release (with --with-beam: in
#                       target/beam/quick-release; with --with-adaptive: in
#                       target/adaptive/quick-release)
#
# Output: <repo>/owner-run-logs/bench-render-accel-<timestamp>/
#   build.log                the CLI build
#   <scene>/                 the benchmark's folder of that scene (results.json, renders, logs)
#   <scene>.log              the benchmark's output for that scene
#   summary.md               the verdict table (the .plraw files are not needed)
#
# Needs jq and bash 4.

set -u

# Headless boxes have no session runtime dir; libwayland prints an error line per Vulkan ICD probe
# without it. A private one silences it and changes nothing else.
if [ -z "${XDG_RUNTIME_DIR:-}" ]; then
    export XDG_RUNTIME_DIR="${TMPDIR:-/tmp}/pleochroma-xdg-$(id -u)"
    mkdir -p -m 700 "$XDG_RUNTIME_DIR"
fi

REPO="$(pwd)"
SCENES=()
TIMEOUT=10h
BUILD=1
MATRIX_CPU=()
PATH_CACHE=()
ADAPTIVE=()
BEAM=()
CACHE_KEEP=0
CACHE_KEEP_SCENES="s2_sapphire,s3_small"
VRAM_GB="${PLEOCHROMA_CACHE_VRAM_GB:-}"
while [ $# -gt 0 ]; do
    case "$1" in
        --with-beam) BEAM=(--beam --engine-matrix); shift ;;
        --no-beam) shift ;;
        --cache-keep) CACHE_KEEP=1; PATH_CACHE=(--path-cache); shift ;;
        --cache-keep-scenes) CACHE_KEEP_SCENES="$2"; shift 2 ;;
        --with-adaptive) ADAPTIVE=(--adaptive); shift ;;
        --no-adaptive) shift ;;
        --matrix-cpu) MATRIX_CPU=(--matrix-cpu); shift ;;
        --path-cache) PATH_CACHE=(--path-cache); shift ;;
        --vram-gb) VRAM_GB="$2"; shift 2 ;;
        --repo) REPO="$2"; shift 2 ;;
        --scene) SCENES+=("$2"); shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        --no-build) BUILD=0; shift ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done
if [ ${#SCENES[@]} -eq 0 ]; then
    SCENES=(s1_glints s2_sapphire s3_small)
fi

cd "$REPO" || { echo "no repo at $REPO" >&2; exit 2; }
# Sources that differ from the last run's get the current time, so cargo rebuilds what changed
# even when the copied files keep older times (scripts/fresh_sources.sh).
. "$REPO/scripts/fresh_sources.sh"
fresh_sources
# gpu_name prints the GPU's name on any machine (scripts/gpu_name.sh).
. "$REPO/scripts/gpu_name.sh"
command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; exit 2; }
OUT_DIR="$REPO/owner-run-logs/bench-render-accel-$(date +%Y-%m-%d_%H%M%S)"
mkdir -p "$OUT_DIR"
if [ ${#BEAM[@]} -gt 0 ]; then
    # The beam CLI: its own target dir, so the regular build cache stays. The `beam` feature
    # implies `adaptive-sampling`, so it serves --with-adaptive too.
    CLI="$REPO/target/beam/quick-release/pleochroma-cli"
    BUILD_ARGS=(--target-dir "$REPO/target/beam"
        --features gpu,beam,pleochroma/serde,pleochroma/hdr,pleochroma-net/compression)
    BUILD_NOTE="quick-release, GPU, beam"
elif [ ${#ADAPTIVE[@]} -gt 0 ]; then
    # The adaptive CLI: its own target dir, so target/quick-release stays the default build.
    CLI="$REPO/target/adaptive/quick-release/pleochroma-cli"
    BUILD_ARGS=(--target-dir "$REPO/target/adaptive"
        --features gpu,adaptive-sampling,pleochroma/serde,pleochroma/hdr,pleochroma-net/compression)
    BUILD_NOTE="quick-release, GPU, adaptive sampling"
else
    CLI="$REPO/target/quick-release/pleochroma-cli"
    BUILD_ARGS=(--features gpu,pleochroma/serde,pleochroma/hdr,pleochroma-net/compression)
    BUILD_NOTE="quick-release, GPU"
fi

if [ "$BUILD" -eq 1 ]; then
    echo "== build the CLI ($BUILD_NOTE)"
    cargo build --profile quick-release -p pleochroma-cli --bins \
        "${BUILD_ARGS[@]}" 2>&1 | tee "$OUT_DIR/build.log"
    if [ "${PIPESTATUS[0]}" -ne 0 ]; then
        echo "the CLI build failed, see $OUT_DIR/build.log" >&2
        exit 1
    fi
fi
# Parts cache-note and m1 of run_full_round.sh run the default CLI (target/quick-release): an
# adaptive bench builds that one too, so they never run a stale binary.
if [ "$BUILD" -eq 1 ] && [ ${#ADAPTIVE[@]} -gt 0 ] && [ ${#BEAM[@]} -eq 0 ]; then
    echo "== build the default CLI (quick-release, GPU) for cache-note and m1"
    cargo build --profile quick-release -p pleochroma-cli --bins \
        --features gpu,pleochroma/serde,pleochroma/hdr,pleochroma-net/compression \
        2>&1 | tee -a "$OUT_DIR/build.log"
    if [ "${PIPESTATUS[0]}" -ne 0 ]; then
        echo "the default CLI build failed, see $OUT_DIR/build.log" >&2
        exit 1
    fi
fi
[ -x "$CLI" ] || { echo "no CLI at $CLI" >&2; exit 1; }

# The path cache cannot ask the GPU how much memory is free: it plans with what it is told.
if [ ${#PATH_CACHE[@]} -gt 0 ]; then
    if [ -z "$VRAM_GB" ] && command -v nvidia-smi >/dev/null 2>&1; then
        free_mib=$(nvidia-smi --query-gpu=memory.free --format=csv,noheader,nounits 2>/dev/null | head -n 1 | tr -dc '0-9')
        if [ -n "$free_mib" ]; then VRAM_GB=$(( free_mib / 1024 )); fi
    fi
    if [ -n "$VRAM_GB" ]; then
        export PLEOCHROMA_CACHE_VRAM_GB="$VRAM_GB"
    else
        echo "note: free video memory unknown (no --vram-gb, no nvidia-smi): the path cache can only use system RAM" >&2
    fi
fi

STATUS=()
for scene in "${SCENES[@]}"; do
    echo "== $scene"
    start=$SECONDS
    KEEP=()
    if [ "$CACHE_KEEP" -eq 1 ] && [[ ",$CACHE_KEEP_SCENES," == *",$scene,"* ]]; then
        KEEP=(--cache-keep)
    fi
    timeout "$TIMEOUT" bash "$REPO/benchmarks/render_accel/run.sh" --cli "$CLI" \
        --out-dir "$OUT_DIR/$scene" --scene "$scene" ${ADAPTIVE[@]+"${ADAPTIVE[@]}"} ${BEAM[@]+"${BEAM[@]}"} \
        ${MATRIX_CPU[@]+"${MATRIX_CPU[@]}"} ${PATH_CACHE[@]+"${PATH_CACHE[@]}"} ${KEEP[@]+"${KEEP[@]}"} \
        2>&1 | tee "$OUT_DIR/$scene.log"
    code=${PIPESTATUS[0]}
    STATUS+=("| $scene | $code | $(( (SECONDS - start + 59) / 60 )) |")
done

# The verdict per scene from its results.json: the fastest passing plain run on the default engines
# (the fixed baseline or an adaptive run) against the fastest passing beam run on the same engines,
# then the engine matrix (each beam run against the plain run on the same engines).
{
    # The title names the rows that ran.
    rows=""
    [ ${#ADAPTIVE[@]} -gt 0 ] && rows="$rows, adaptive sampling"
    [ ${#BEAM[@]} -gt 0 ] && rows="$rows, beams"
    [ ${#PATH_CACHE[@]} -gt 0 ] && rows="$rows, the path cache"
    rows="${rows:-, the fixed-sample baselines only}"
    echo "# Render-acceleration benchmark: ${rows#, }, $(date '+%Y-%m-%d %H:%M')"
    echo
    # The code that ran: the box's tree is copied over its clone, whose HEAD can stay the same
    # with other code. "-dirty" says the tree differs from HEAD; "version" is the workspace
    # version; "sources" is sources_hash (fresh_sources.sh): the hash of every file git sees
    # (owner-run-logs left out), the same on any machine.
    echo "Repo: $REPO (git: $(git describe --always --dirty 2>/dev/null || echo unknown), version: $(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1), sources: $(sources_hash)), cores: $(nproc)"
    echo "GPU: $(gpu_name)"
    if [ ${#PATH_CACHE[@]} -gt 0 ]; then
        if [ -n "${PLEOCHROMA_CACHE_VRAM_GB:-}" ]; then
            echo "Path cache video memory: $PLEOCHROMA_CACHE_VRAM_GB GB"
        else
            echo "Path cache video memory: unknown, so the cache could only use system RAM"
        fi
    fi
    echo
    echo "| scene | exit | minutes |"
    echo "| --- | --- | --- |"
    printf '%s\n' "${STATUS[@]}"
    if [ ${#ADAPTIVE[@]} -gt 0 ]; then
    echo
    echo "## Adaptive sampling against the fixed-sample baseline (plain path tracer, default engines)"
    echo
    echo "| scene | adaptive run | passes | seconds | baseline seconds | speed-up | samples saved |"
    echo "| --- | --- | --- | --- | --- | --- | --- |"
    for scene in "${SCENES[@]}"; do
        results="$OUT_DIR/$scene/results.json"
        if [ ! -f "$results" ]; then
            echo "| $scene | no results.json (stopped or failed, see $scene.log) | | | | | |"
            continue
        fi
        jq -r '
            .scenes[0] as $s
            | $s.adaptive[]
            | .figures.seconds as $sec
            | "| \($s.id) | \(.metric) \(.target) | \(.passes) | \($sec) | \($s.baseline.seconds) | "
              + (if $sec > 0 then ((($s.baseline.seconds / $sec) * 100 | round) / 100 | tostring) + "x" else "-" end)
              + " | "
              + (if ($s.baseline.samples_used // 0) > 0
                 then (((1 - (.figures.samples_used / $s.baseline.samples_used)) * 1000 | round) / 10 | tostring) + " %"
                 else "-" end)
              + " |"' "$results"
    done
    fi
    if [ ${#BEAM[@]} -gt 0 ]; then
    echo
    echo "## Default engines: fastest run that meets the quality rule"
    echo
    echo "| scene | fastest plain run | seconds | fastest beam run | seconds | beam speed-up | beam residual % | leaves |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- |"
    for scene in "${SCENES[@]}"; do
        results="$OUT_DIR/$scene/results.json"
        if [ ! -f "$results" ]; then
            echo "| $scene | no results.json (stopped or failed, see $scene.log) | | | | | | |"
            continue
        fi
        jq -r '
            .scenes[0] as $s
            | ([{name: "baseline", seconds: $s.baseline.seconds}]
               + [$s.adaptive[] | select(.passes) | {name: "adaptive \(.metric) \(.target)", seconds: .figures.seconds}]
              | min_by(.seconds)) as $plain
            | ([$s.beam[] | select(.engine == "default" and .passes and (.declined | not))] | min_by(.figures.seconds)) as $beam
            | ([$s.beam[] | select(.engine == "default" and .declined)][0]) as $declined
            | "| \($s.id) | \($plain.name) | \($plain.seconds) | "
              + (if $beam == null and $declined != null
                 then "beams declined | \($declined.figures.seconds // $declined.seconds // "-") (the price of asking) | | \($declined.stats.residual_pct // "-") | \($declined.stats.leaves // "-") |"
                 elif $beam == null
                 then "no beam run passes\(if ([$s.beam[] | select(.engine == "default" and .timed_out)] | length) > 0 then " (timed out)" else "" end) | | | \([$s.beam[] | select(.engine == "default")][0].stats.residual_pct // "-") | \([$s.beam[] | select(.engine == "default")][0].stats.leaves // "-") |"
                 else "beam \($beam.mode) | \($beam.figures.seconds) | \(if $beam.figures.seconds > 0 then (($plain.seconds / $beam.figures.seconds * 100 | round) / 100) else "-" end)x | \($beam.stats.residual_pct // "-") | \($beam.stats.leaves // "-") |"
                 end)' "$results"
    done
    echo
    echo "## Engine matrix: beam run against the plain run on the same engines (fixed samples at the cap)"
    echo
    echo "| scene | engines | beam passes | beam seconds | plain seconds | speed-up | beam residual % |"
    echo "| --- | --- | --- | --- | --- | --- | --- |"
    for scene in "${SCENES[@]}"; do
        results="$OUT_DIR/$scene/results.json"
        [ -f "$results" ] || continue
        jq -r '
            .scenes[0] as $s
            | $s.beam[] | select(.engine != "default")
            | (.figures.seconds // .seconds) as $bsec
            | "| \($s.id) | \(.engine) | \(.passes)\(if .timed_out then " (timed out)" else "" end)\(if .declined then " (beams declined)" else "" end) | \($bsec // "-") | \(.engine_baseline.seconds) | \(if ($bsec // 0) > 0 then ((.engine_baseline.seconds / $bsec * 100 | round) / 100 | tostring) + "x" else "-" end) | \(.stats.residual_pct // "-") |"' "$results"
    done
    fi
    if [ ${#PATH_CACHE[@]} -gt 0 ]; then
        echo
        echo "## Path cache against the fixed-sample baseline (default engines)"
        echo
        echo "| scene | path-cache run | passes | seconds | baseline seconds | speed-up | cache |"
        echo "| --- | --- | --- | --- | --- | --- | --- |"
        for scene in "${SCENES[@]}"; do
            results="$OUT_DIR/$scene/results.json"
            [ -f "$results" ] || continue
            jq -r '
                .scenes[0] as $s
                | $s.path_cache[]
                | (.figures.seconds // .seconds) as $sec
                | "| \($s.id) | \(.mode) | \(.passes)\(if .timed_out then " (timed out)" else "" end) | \($sec // "-") | \($s.baseline.seconds) | "
                  + (if .timed_out then "-"
                     elif ($sec // 0) > 0 then ((($s.baseline.seconds / $sec) * 100 | round) / 100 | tostring) + "x"
                     else "-" end)
                  + " | \(if .stats.used then "used" else "not used" end): \(.stats.cache_line // "no Path cache note") |"' "$results"
        done
    fi
    echo
    echo "Speed-up = baseline seconds / run seconds, so above 1 is faster. It counts as a saving only"
    echo "when the run passes (\"passes\" true: its picture is as close to the references as the"
    echo "fixed-sample baseline's)."
    if [ ${#ADAPTIVE[@]} -gt 0 ]; then
        echo "Adaptive sampling: samples saved is the share of the baseline's samples the run did not trace."
    fi
    if [ ${#BEAM[@]} -gt 0 ]; then
        echo "Beams: \"no beam run passes\" means the beams were faster or slower but not as good as the"
        echo "baseline; \"beams declined\" means the renderer refused beams for the scene (a \"Beam tracing"
        echo "off\" note) and drew a plain picture, so its seconds are the price of asking, not a beam"
        echo "result; \"timed out\" means a beam or path-cache run hit its time cap (10 x the baseline"
        echo "+ 120 s) and was stopped. Path cache: \"not used\" means the cache switched itself off (the"
        echo "note says why), so that row's time is a plain-render time, not a cache result."
    elif [ ${#PATH_CACHE[@]} -gt 0 ]; then
        echo "Path cache: \"timed out\" means a run hit its time cap (10 x the baseline + 120 s) and was"
        echo "stopped; \"not used\" means the cache switched itself off (the note says why), so that"
        echo "row's time is a plain-render time, not a cache result."
    fi
} > "$OUT_DIR/summary.md"

echo
cat "$OUT_DIR/summary.md"
echo "Folder: $OUT_DIR"
