#!/usr/bin/env bash
# Runs the render-acceleration benchmark (guide section 7.4, steps 1, 3 and 5; step 2 with
# --adaptive, which needs a CLI built with `--features adaptive-sampling`).
#
# For every scene of scenes.json it
#   1. renders a reference REF (--sample-offset 0) and an independent second reference REF2
#      (--sample-offset = the reference spp), both with --raw-out; REF also with --progress-raw
#      for the convergence curve (stills only);
#   2. compares REF with REF2 (the noise floor) and every progress snapshot with REF and REF2
#      (changed area against samples per pixel);
#   3. with --adaptive: renders a fixed-spp BASELINE at the adaptive cap and an adaptive run for
#      every target noise of both metrics, compares each with REF2 (threshold from REF), and
#      applies the selection rule of section 7.4: a run passes when it is as good as the baseline
#      (changed_area_pct <= baseline + 0.1, worst_tile_de <= 1.1 x baseline, glint_recall >= 0.99)
#      on every scene. The adaptive runs come right after the baseline, before the path-cache and
#      beam runs, which may each take up to ten times as long.
#   4. with --path-cache: a fixed-spp run at the adaptive cap with --path-cache, judged by the same
#      rule against the baseline (guide gates 3 and 4); with --adaptive too, one adaptive run per
#      metric (target 0.01 relative, 1 absolute) with --path-cache as well; with --cache-keep a
#      kept-cache pair (keep_first builds and keeps the cache, keep_again loads it: the re-render
#      the cache is for). The "Path cache" note of each run's log (<scene>/cached*.log) says
#      whether the cache was used or why not.
#   5. with --beam: a fixed-spp run at the adaptive cap with --beam (beam tracing plus the adaptive
#      residual, guide 16.5), judged by the same rule against the baseline, with its seconds and
#      the beam stats parsed from the "Beams: ..." note of the run's log (<scene>/beam_*.log).
#   6. with --engine-matrix (implies --beam): the beam run repeated with --compute local and
#      --local gpu and --local cpu+gpu (hybrid), each next to a fixed-spp baseline on the same
#      engines, so the table shows the beam speed-up per engine. The --local cpu row (an hour or
#      more per scene) runs only with --matrix-cpu.
#   Steps 5 and 6 (the beam rows) need a CLI built with `--features beam`; beam tracing is retired.
#   `bash scripts/bench_render_accel.sh --with-beam` builds one into target/beam.
# Every render runs the CLI's own estimator, exit collection (the apps' estimator), so REF,
# REF2 and the baselines are exit-collection yardsticks. The path-cache and beam rows render
# branching through their own flags, and their figures are judged against the baseline of
# their scene at the same cap.
# Every render's wall time in seconds is kept as "seconds" next to its figures. A beam render runs
# with --verbose (the per-batch engine lines are in <scene>/beam_*.log). Beam and path-cache
# renders run under a time cap of 10 x the baseline's seconds + 120 s; a render that hits the cap
# is recorded as passes false, timed_out true (figures null) and the run goes on. Adaptive and
# path-cache renders run with --verbose too (the adaptive pass timing and the cache's "path cache:"
# lines are in their logs); a beam row whose log says "Beam tracing off" is marked declined true.
# Everything is written to results.json in the output folder and summarised in a table.
#
# Needs bash 4 or newer and jq. The script starts no build. Reference renders at 16k to 32k
# samples take a long time: run it on a quiet machine, and use --scene to run a few at a time.
#
# Usage: run.sh [--cli PATH] [--out-dir DIR] [--scene ID]... [--render-arg ARG]... [--adaptive]
#               [--path-cache] [--cache-keep] [--beam] [--beam-arg ARG]... [--engine-matrix] [--matrix-cpu]
#               [--dry-run]
#   --cli PATH         the pleochroma-cli executable (default: pleochroma-cli on the PATH)
#   --out-dir DIR      the output folder (default: out next to this script)
#   --scene ID         run only this scene id (repeatable; default: all)
#   --render-arg ARG   one extra argument for every render / tilt-video call (repeatable),
#                      for example: --render-arg --local --render-arg gpu
#   --adaptive         also run step 2 and the adaptive path-cache rows
#                      (needs a CLI built with `--features adaptive-sampling`; this script stops
#                      with a message when the CLI has no --adaptive)
#   --path-cache       also run step 4 (needs --path-cache of render / tilt-video)
#   --cache-keep       also run the kept-cache pair of step 4 (implies --path-cache; needs
#                      --persist-cache and --cache-dir of render)
#   --beam             also run step 5 (needs a CLI built with `--features beam`; this script stops
#                      with a message when the CLI has no --beam)
#   --beam-arg ARG     one extra argument for the beam runs only (repeatable), for example:
#                      --beam-arg --beam-residual-min-spp --beam-arg 64
#   --engine-matrix    also run step 6 (implies --beam, so the beam CLI). Do not put --local or --compute in
#                      --render-arg with it: the matrix sets them itself.
#   --matrix-cpu       add the --local cpu row to the engine matrix (default: gpu and hybrid only)
#   --dry-run         print the render commands of every scene and run nothing (no CLI needed)

set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
cli="pleochroma-cli"
out_dir="$here/out"
adaptive=0
path_cache=0
cache_keep=0
beam=0
matrix=0
matrix_cpu=0
dry_run=0
wanted=()
render_args=()
beam_args=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --cli) cli="$2"; shift 2 ;;
        --out-dir) out_dir="$2"; shift 2 ;;
        --scene) wanted+=("$2"); shift 2 ;;
        --render-arg) render_args+=("$2"); shift 2 ;;
        --adaptive) adaptive=1; shift ;;
        --path-cache) path_cache=1; shift ;;
        --cache-keep) path_cache=1; cache_keep=1; shift ;;
        --beam) beam=1; shift ;;
        --beam-arg) beam_args+=("$2"); shift 2 ;;
        --engine-matrix) beam=1; matrix=1; shift ;;
        --matrix-cpu) matrix_cpu=1; shift ;;
        --dry-run) dry_run=1; shift ;;
        -h|--help) sed -n '2,68p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "unknown argument: $1 (see --help)" >&2; exit 1 ;;
    esac
done

command -v jq >/dev/null 2>&1 || { echo "jq is required" >&2; exit 1; }
if [[ $dry_run -eq 0 ]]; then
    command -v "$cli" >/dev/null 2>&1 || { echo "cannot find '$cli': give the path with --cli" >&2; exit 1; }
    if [[ $beam -eq 1 || $adaptive -eq 1 ]]; then
        # Captured once, first: under pipefail a `| grep -q` can fail on the writer's SIGPIPE.
        cli_help=$("$cli" render --help 2>&1 || true)
    fi
    if [[ $beam -eq 1 ]]; then
        [[ $cli_help == *--beam* ]] || { echo "this CLI has no beam tracer: build pleochroma-cli with --features beam (beam tracing is retired)" >&2; exit 1; }
    fi
    if [[ $adaptive -eq 1 ]]; then
        [[ $cli_help == *--adaptive* ]] || { echo "this CLI has no adaptive sampling: build pleochroma-cli with --features adaptive-sampling" >&2; exit 1; }
    fi
fi

# The selection rule of section 7.4, step 4: a jq condition on one run's figures ($f) against the
# scene's fixed-spp baseline at the same cap ($b). Both are compared with REF2, which shares no
# samples with either (both start at sample 0, like REF).
area_margin=0.1
tile_factor=1.1
min_recall=0.99
# A null glint_recall (the reference has no glints) has nothing to lose and passes.
rule="(\$f.changed_area_pct <= \$b.changed_area_pct + $area_margin) and (\$f.worst_tile_de <= \$b.worst_tile_de * $tile_factor) and (\$f.glint_recall == null or \$f.glint_recall >= $min_recall)"

# The adaptive targets of section 7.4, step 2: relative error as a fraction, absolute as Delta E.
# Two per metric: more targets gave the same picture (35 to 41 % of the samples saved on s1
# at every target).
relative_targets=(0.1 0.01)
absolute_targets=(10 1)

# The log filter of a path-cache run (with --verbose): the cache's own "path cache: ..." debug lines
# of the GPU backend and the cached pass, and the export thread's info lines.
cache_log_filter="warn,pleochroma::renderer::gpu_backend=debug,pleochroma::renderer::gpu::frame::cached_pass=debug,pleochroma_cut::bridge::export_thread=info"

run_cli() {
    echo "> ${RUST_LOG:+RUST_LOG=$RUST_LOG }$cli $*" >&2
    if [[ $dry_run -eq 1 ]]; then return 0; fi
    # TIME_CAP (seconds, set by beam_row) bounds one render: a beam run that is far slower than its
    # baseline is stopped and recorded as timed out instead of blocking the scene.
    if [[ -n ${TIME_CAP:-} ]]; then
        timeout "$TIME_CAP" "$cli" "$@" </dev/null
    else
        "$cli" "$@" </dev/null
    fi
}

# image-compare --json: prints the report.
compare_raw() { # REF RUN JOB [REF2] [ERROR_PNG]
    local ref=$1 run=$2 job=$3 ref2=${4:-} png=${5:-}
    local args=(image-compare "$ref" "$run" --tonemap-from "$job" --json)
    if [[ -n $ref2 ]]; then args+=(--ref2 "$ref2"); fi
    if [[ -n $png ]]; then args+=(--error-png "$png"); fi
    run_cli "${args[@]}"
}

# The numbers kept in results.json for one comparison (reads a report on standard input).
figures() {
    jq -c '{changed_area_pct, worst_tile_de, glint_recall, mean_de, p99_de, max_spp, samples_used, samples_saving}'
}

# The worst of several figure objects, one per line on standard input (the frames of a tilt scene).
merge_worst() {
    jq -s -c '{
        changed_area_pct: (map(.changed_area_pct) | max),
        worst_tile_de: (map(.worst_tile_de) | max),
        glint_recall: ([.[] | .glint_recall | select(. != null)] | if length == 0 then null else min end),
        mean_de: (map(.mean_de) | max),
        p99_de: (map(.p99_de) | max),
        max_spp: (map(.max_spp) | max),
        samples_used: (map(.samples_used) | add),
        samples_saving: (map(.samples_saving) | min)
    }'
}

# Renders the current scene; prints the .plraw file (a still) or folder (a tilt video) it wrote.
# PROGRESS, when set, is the folder for --progress-raw (stills only).
render_scene() { # NAME SAMPLE_OFFSET JOB [EXTRA...]
    local name=$1 offset=$2 jobpath=$3
    shift 3
    local common=(${scene_args[@]+"${scene_args[@]}"} ${render_args[@]+"${render_args[@]}"} "$@" --sample-offset "$offset")
    local started
    started=$(date +%s.%N)
    # The CLI's output is kept next to the render as NAME.log (its "note:" and progress lines are
    # on stderr, teed so they still reach the terminal): the
    # beam stats are read from it. A dry run touches no file.
    local log="$folder/$name.log"
    if [[ $dry_run -eq 1 ]]; then log=/dev/null; fi
    if [[ $kind == tilt_video ]]; then
        local raw="$folder/${name}_raw"
        if [[ $dry_run -eq 0 ]]; then mkdir -p "$raw"; fi
        run_cli tilt-video "$jobpath" --out-dir "$folder/${name}_frames" --restart --raw-out "$raw" "${common[@]}" >"$log" 2> >(tee -a "$log" >&2)
        if [[ $dry_run -eq 0 ]]; then awk -v a="$started" -v b="$(date +%s.%N)" 'BEGIN { printf "%.1f\n", b - a }' >"$folder/$name.seconds"; fi
        echo "$raw"
        return 0
    fi
    local still=(render "$jobpath" --out "$folder/$name.png" --raw-out "$folder/$name.plraw")
    if [[ -n ${PROGRESS:-} ]]; then
        if [[ $dry_run -eq 0 ]]; then mkdir -p "$PROGRESS"; fi
        still+=(--progress-raw "$PROGRESS")
    fi
    run_cli "${still[@]}" "${common[@]}" >"$log" 2> >(tee -a "$log" >&2)
    if [[ $dry_run -eq 0 ]]; then awk -v a="$started" -v b="$(date +%s.%N)" 'BEGIN { printf "%.1f\n", b - a }' >"$folder/$name.seconds"; fi
    echo "$folder/$name.plraw"
}

# The beam stats of a run, read from its log: the "Beams: N leaves, X % residual" note (null
# when the run printed none, for example because beams were refused) and every note line.
beam_stats() { # LOG
    local log=$1 line leaves=null residual=null notes
    line=$(grep -h 'Beams:' "$log" | tail -n 1 || true)
    # An old abbreviated "142k" form is read as 142000.
    if [[ $line =~ Beams:\ ([0-9]+)(k?)\ leaves,\ ([0-9.]+)\ %\ residual ]]; then
        leaves=${BASH_REMATCH[1]}
        if [[ ${BASH_REMATCH[2]} == k ]]; then leaves=$((leaves * 1000)); fi
        residual=${BASH_REMATCH[3]}
    fi
    notes=$({ grep -h '^note:' "$log" || true; } | jq -R -s -c 'split("\n") | map(select(length > 0))')
    jq -n -c --arg line "$line" --argjson leaves "$leaves" --argjson residual "$residual" --argjson notes "$notes" \
        '{beams_line: (if $line == "" then null else $line end), leaves: $leaves, residual_pct: $residual, notes: $notes}'
}

# The path-cache status of a run, read from its log: the last "Path cache" line (the cache's size
# and memory when it was used, or why it was not) and whether the cache was used.
cache_stats() { # LOG
    local log=$1 line
    # Case-sensitive and anchored: only the "note: Path cache ..." line, never the lower-case
    # "path cache: ..." debug lines of a --verbose run.
    line=$(grep -h '^note: Path cache' "$log" | tail -n 1 || true)
    jq -n -c --arg line "$line" \
        '{cache_line: (if $line == "" then null else ($line | sub("^note: "; "")) end),
          used: ($line | test("in video memory|in system RAM"))}'
}

# The time cap of a beam or path-cache render: ten times the baseline's seconds plus two minutes.
run_cap() {
    echo $(( $(jq -r '.seconds | ceil' <<<"$baseline") * 10 + 120 ))
}

# Whether a render wrote its output: a still's .plraw, or for a tilt video the raw file of every
# frame of REF. A render stopped by its time cap (or failed) has none.
wrote_output() { # RUN
    local run=$1 frame
    if [[ $kind != tilt_video ]]; then
        [[ -e $run ]]
        return
    fi
    for frame in "$ref"/*.plraw; do
        [[ -e $run/$(basename "$frame") ]] || return 1
    done
}

# One beam render of the current scene at the adaptive cap, judged against the baseline; prints
# one row of "beam" for results.json. MODE names the files (beam_MODE.*), ENGINE is the label of
# the engines (default, cpu, gpu or hybrid), the remaining arguments are the engine flags. For an
# engine matrix row ENGINE_BASELINE is the fixed-spp render on the same engines (null otherwise).
beam_row() { # MODE ENGINE ENGINE_BASELINE [ENGINE_FLAGS...]
    local mode=$1 engine=$2 engine_baseline=$3
    shift 3
    local name="beam_$mode" run figs stats cap declined
    # A beam run may take at most ten times the baseline plus two minutes; --verbose keeps the
    # per-batch engine lines in the run's log.
    cap=$(run_cap)
    run=$(TIME_CAP=$cap render_scene "$name" 0 "$run_job" "$@" --beam --verbose ${beam_args[@]+"${beam_args[@]}"} || true)
    stats=$(beam_stats "$folder/$name.log")
    # A "Beam tracing off" note means the renderer declined beams and drew a plain picture: the
    # row's seconds are then the price of asking, not a beam result.
    declined=$(jq -c '[.notes[] | test("Beam tracing off")] | any' <<<"$stats")
    if ! wrote_output "$run"; then
        # Stopped by the time cap (or failed): no output to compare, so the row fails the rule.
        echo "$name: no output after $(cat "$folder/$name.seconds" 2>/dev/null || echo '?') s (cap $cap s): timed out" >&2
        jq -n -c --arg mode "$mode" --arg engine "$engine" --argjson eb "$engine_baseline" --argjson stats "$stats" \
            --argjson declined "$declined" \
            --argjson s "$(cat "$folder/$name.seconds" 2>/dev/null || echo null)" \
            '{mode: $mode, engine: $engine, passes: false, timed_out: true, declined: $declined, seconds: $s, figures: null, engine_baseline: $eb, stats: $stats}'
        return 0
    fi
    figs=$(compare_scene "$ref2" "$run" "$job" "$ref" "$folder/${name}_error" | with_seconds "$name")
    jq -n -c --arg mode "$mode" --arg engine "$engine" --argjson f "$figs" --argjson b "$baseline" \
        --argjson eb "$engine_baseline" --argjson stats "$stats" --argjson declined "$declined" \
        "{mode: \$mode, engine: \$engine, passes: ($rule), timed_out: false, declined: \$declined, figures: \$f, engine_baseline: \$eb, stats: \$stats}"
}

# One path-cache render of the current scene at the adaptive cap, judged against the baseline;
# prints one row of "path_cache" for results.json. MODE is "fixed" or "adaptive_METRIC_TARGET",
# NAME names the files (NAME.*), the remaining arguments are extra render flags. Like a beam run
# it may take at most ten times the baseline plus two minutes; "stats" says whether the cache was
# used (a row whose cache was off is a plain render and its time is not a cache result).
cached_row() { # MODE NAME [FLAGS...]
    local mode=$1 name=$2 run figs stats cap
    shift 2
    cap=$(run_cap)
    # --verbose and the cache log filter put the cache's own "path cache: ..." lines in the log.
    run=$(TIME_CAP=$cap RUST_LOG=$cache_log_filter render_scene "$name" 0 "$run_job" --path-cache --verbose "$@" || true)
    stats=$(cache_stats "$folder/$name.log")
    if ! wrote_output "$run"; then
        echo "$name: no output after $(cat "$folder/$name.seconds" 2>/dev/null || echo '?') s (cap $cap s): timed out" >&2
        jq -n -c --arg mode "$mode" --argjson stats "$stats" \
            --argjson s "$(cat "$folder/$name.seconds" 2>/dev/null || echo null)" \
            '{mode: $mode, passes: false, timed_out: true, seconds: $s, figures: null, stats: $stats}'
        return 0
    fi
    figs=$(compare_scene "$ref2" "$run" "$job" "$ref" "$folder/${name}_error" | with_seconds "$name")
    jq -n -c --arg mode "$mode" --argjson f "$figs" --argjson b "$baseline" --argjson stats "$stats" \
        "{mode: \$mode, passes: ($rule), timed_out: false, figures: \$f, stats: \$stats}"
}

# The fixed-spp render at the cap on the engines of an engine matrix row (no beams); prints its figures.
engine_baseline() { # MODE ENGINE_FLAGS...
    local name="baseline_$1" run
    shift
    run=$(render_scene "$name" 0 "$run_job" "$@")
    compare_scene "$ref2" "$run" "$job" "$ref" "$folder/${name}_error" | with_seconds "$name"
}

# The engine flags of the matrix rows: label, then the flags (words; none contain spaces). The
# CPU row (an hour or more per scene) is added only by --matrix-cpu.
matrix_rows=("gpu --compute local --local gpu" "hybrid --compute local --local cpu+gpu")
if [[ $matrix_cpu -eq 1 ]]; then matrix_rows+=("cpu --compute local --local cpu"); fi

# Adds the wall time of render NAME to a figures object on standard input.
with_seconds() { # NAME
    jq -c --argjson s "$(cat "$folder/$1.seconds")" '. + {seconds: $s}'
}

# Compares REF with RUN for the current scene: the figures of a still, the worst frame of a tilt scene.
compare_scene() { # REF RUN JOB [REF2] [ERROR_PNG_PREFIX]
    local ref=$1 run=$2 job=$3 ref2=${4:-} prefix=${5:-}
    if [[ $kind != tilt_video ]]; then
        local png=""
        if [[ -n $prefix ]]; then png="$prefix.png"; fi
        compare_raw "$ref" "$run" "$job" "$ref2" "$png" | figures
        return 0
    fi
    local frame name second png
    local all=()
    for frame in "$ref"/*.plraw; do
        name=$(basename "$frame" .plraw)
        second=""
        if [[ -n $ref2 ]]; then second="$ref2/$name.plraw"; fi
        png=""
        if [[ -n $prefix ]]; then png="${prefix}_$name.png"; fi
        all+=("$(compare_raw "$frame" "$run/$name.plraw" "$job" "$second" "$png" | figures)")
    done
    if [[ ${#all[@]} -eq 0 ]]; then
        echo "no frames in $ref" >&2
        return 1
    fi
    printf '%s\n' "${all[@]}" | merge_worst
}

# A copy of the job with the adaptive sample cap in place of the reference count; prints its path.
adaptive_job() { # JOB REFERENCE_SPP ADAPTIVE_SPP
    if [[ $2 -eq $3 ]]; then
        echo "$1"
        return 0
    fi
    local copy="$folder/adaptive.job.json"
    sed -E "s/\"samples_per_pixel\":[[:space:]]*[0-9]+/\"samples_per_pixel\": $3/" "$1" >"$copy"
    echo "$copy"
}

# A copy of the job with SPP samples per pixel, written to <scene folder>/NAME.job.json; prints its path.
spp_job() { # JOB SPP NAME
    local copy="$folder/$3.job.json"
    if [[ $dry_run -eq 0 ]]; then
        sed -E "s/\"samples_per_pixel\":[[:space:]]*[0-9]+/\"samples_per_pixel\": $2/" "$1" >"$copy"
    fi
    echo "$copy"
}

mkdir -p "$out_dir"
records=()

while IFS= read -r -u 3 entry; do
    id=$(jq -r '.id' <<<"$entry")
    if [[ ${#wanted[@]} -gt 0 ]]; then
        match=0
        for w in "${wanted[@]}"; do
            if [[ $w == "$id" ]]; then match=1; fi
        done
        if [[ $match -eq 0 ]]; then continue; fi
    fi
    kind=$(jq -r '.kind' <<<"$entry")
    reference_spp=$(jq -r '.reference_spp' <<<"$entry")
    adaptive_spp=$(jq -r '.adaptive_max_spp' <<<"$entry")
    job="$here/$(jq -r '.job' <<<"$entry")"
    mapfile -t scene_args < <(jq -r '.render_args[]' <<<"$entry")
    folder="$out_dir/$id"
    echo
    echo "=== $id: $(jq -r '.title' <<<"$entry")"
    if [[ $dry_run -eq 1 ]]; then
        # Only the render commands, in the order a real run issues them.
        if [[ $kind == tilt_video ]]; then progress=""; else progress="$folder/progress"; fi
        if [[ $reference_spp -eq $adaptive_spp ]]; then run_job=$job; else run_job="$folder/adaptive.job.json"; fi
        PROGRESS="$progress" render_scene ref 0 "$job" >/dev/null
        PROGRESS="" render_scene ref2 "$reference_spp" "$job" >/dev/null
        render_scene baseline 0 "$run_job" >/dev/null
        if [[ $adaptive -eq 1 ]]; then
            render_scene adaptive_warmup 0 "$(spp_job "$job" 1024 adaptive_warmup)" --adaptive --target-noise 10 --noise-metric absolute >/dev/null
            for metric in relative absolute; do
                if [[ $metric == relative ]]; then targets=("${relative_targets[@]}"); else targets=("${absolute_targets[@]}"); fi
                for target in "${targets[@]}"; do
                    render_scene "adaptive_${metric}_${target}" 0 "$run_job" --adaptive --target-noise "$target" --noise-metric "$metric" --verbose >/dev/null
                done
            done
        fi
        if [[ $path_cache -eq 1 ]]; then
            RUST_LOG=$cache_log_filter render_scene cached 0 "$run_job" --path-cache --verbose >/dev/null
            if [[ $adaptive -eq 1 ]]; then
                for pair in "relative 0.01" "absolute 1"; do
                    read -r metric target <<<"$pair"
                    RUST_LOG=$cache_log_filter render_scene "cached_adaptive_${metric}_${target}" 0 "$run_job" --path-cache --verbose --adaptive --target-noise "$target" --noise-metric "$metric" >/dev/null
                done
            fi
            if [[ $cache_keep -eq 1 ]]; then
                for keep in keep_first keep_again; do
                    RUST_LOG=$cache_log_filter render_scene "cached_$keep" 0 "$run_job" --path-cache --verbose --persist-cache --cache-dir "$folder/path_cache_kept" >/dev/null
                done
            fi
        fi
        if [[ $beam -eq 1 ]]; then
            render_scene beam_fixed 0 "$run_job" --beam --verbose ${beam_args[@]+"${beam_args[@]}"} >/dev/null
        fi
        if [[ $matrix -eq 1 ]]; then
            for row in "${matrix_rows[@]}"; do
                read -r -a words <<<"$row"
                render_scene "baseline_${words[0]}" 0 "$run_job" "${words[@]:1}" >/dev/null
                render_scene "beam_${words[0]}" 0 "$run_job" "${words[@]:1}" --beam --verbose ${beam_args[@]+"${beam_args[@]}"} >/dev/null
            done
        fi
        continue
    fi
    rm -rf "$folder"
    mkdir -p "$folder"

    # Step 1: two independent references.
    if [[ $kind == tilt_video ]]; then progress=""; else progress="$folder/progress"; fi
    ref=$(PROGRESS="$progress" render_scene ref 0 "$job")
    ref2=$(PROGRESS="" render_scene ref2 "$reference_spp" "$job")

    # Step 3: the noise floor, REF against REF2.
    noise=$(compare_scene "$ref" "$ref2" "$job" "" "$folder/noise_floor")

    # Step 5: the convergence curve from the progress snapshots (stills only).
    curve='[]'
    if [[ $kind != tilt_video ]]; then
        for snapshot in "$progress"/*.plraw; do
            if [[ ! -e $snapshot ]]; then continue; fi
            vs_ref=$(compare_raw "$ref" "$snapshot" "$job" "$ref2" | figures)
            vs_ref2=$(compare_raw "$ref2" "$snapshot" "$job" "$ref" | figures)
            curve=$(jq -c --arg file "$(basename "$snapshot")" --argjson a "$vs_ref" --argjson b "$vs_ref2" \
                '. + [{file: $file, spp: $a.max_spp, vs_ref: $a, vs_ref2: $b}]' <<<"$curve")
        done
    fi

    # The fixed-spp baseline at the adaptive cap (with --adaptive, --path-cache or --beam).
    rows='[]'
    baseline='null'
    cached='[]'
    beam_rows='[]'
    if [[ $adaptive -eq 1 || $path_cache -eq 1 || $beam -eq 1 ]]; then
        run_job=$(adaptive_job "$job" "$reference_spp" "$adaptive_spp")
        base=$(render_scene baseline 0 "$run_job")
        baseline=$(compare_scene "$ref2" "$base" "$job" "$ref" "$folder/baseline_error" | with_seconds baseline)
    fi
    # Step 2 (only with --adaptive): adaptive renders for every target of both metrics. They come
    # before the path-cache and beam runs, which may each take up to ten times as long.
    if [[ $adaptive -eq 1 ]]; then
        # Not timed and not judged: the first adaptive render after a change of the shader text
        # compiles the live megakernel (about 20 s on the A100 box), which would be charged to
        # the first adaptive row. Its seconds stay in adaptive_warmup.seconds.
        render_scene adaptive_warmup 0 "$(spp_job "$job" 1024 adaptive_warmup)" --adaptive --target-noise 10 --noise-metric absolute >/dev/null
        for metric in relative absolute; do
            if [[ $metric == relative ]]; then targets=("${relative_targets[@]}"); else targets=("${absolute_targets[@]}"); fi
            for target in "${targets[@]}"; do
                name="adaptive_${metric}_${target}"
                run=$(render_scene "$name" 0 "$run_job" --adaptive --target-noise "$target" --noise-metric "$metric" --verbose)
                figs=$(compare_scene "$ref2" "$run" "$job" "$ref" "$folder/${name}_error" | with_seconds "$name")
                rows=$(jq -c --arg metric "$metric" --argjson target "$target" --argjson f "$figs" --argjson b "$baseline" \
                    ". + [{metric: \$metric, target: \$target, passes: ($rule), figures: \$f}]" <<<"$rows")
            done
        done
    fi
    # Step 4 (only with --path-cache): the same cap with the path cache, then adaptive + cache,
    # each under the time cap of a beam run.
    if [[ $path_cache -eq 1 ]]; then
        row=$(cached_row fixed cached)
        cached=$(jq -c --argjson r "$row" '. + [$r]' <<<"$cached")
        if [[ $adaptive -eq 1 ]]; then
            for pair in "relative 0.01" "absolute 1"; do
                read -r metric target <<<"$pair"
                row=$(cached_row "adaptive_${metric}_${target}" "cached_adaptive_${metric}_${target}" \
                    --adaptive --target-noise "$target" --noise-metric "$metric")
                cached=$(jq -c --argjson r "$row" '. + [$r]' <<<"$cached")
            done
        fi
        if [[ $cache_keep -eq 1 ]]; then
            # A kept cache: the render that builds and keeps it, then the re-render that loads it.
            # A folder of an earlier run would make the first render load instead of build.
            rm -rf "$folder/path_cache_kept"
            for keep in keep_first keep_again; do
                row=$(cached_row "$keep" "cached_$keep" --persist-cache --cache-dir "$folder/path_cache_kept")
                cached=$(jq -c --argjson r "$row" '. + [$r]' <<<"$cached")
            done
        fi
    fi
    # Step 5 (only with --beam): the same cap with beams (the engine choice of --render-arg), then
    # step 6 (only with --engine-matrix): beams and the plain baseline on CPU, GPU and hybrid.
    if [[ $beam -eq 1 ]]; then
        row=$(beam_row fixed default null)
        beam_rows=$(jq -c --argjson r "$row" '. + [$r]' <<<"$beam_rows")
    fi
    if [[ $matrix -eq 1 ]]; then
        for entry_row in "${matrix_rows[@]}"; do
            read -r -a words <<<"$entry_row"
            eb=$(engine_baseline "${words[0]}" "${words[@]:1}")
            row=$(beam_row "${words[0]}" "${words[0]}" "$eb" "${words[@]:1}")
            beam_rows=$(jq -c --argjson r "$row" '. + [$r]' <<<"$beam_rows")
        done
    fi

    record=$(jq -n -c --arg id "$id" --arg kind "$kind" --argjson spp "$reference_spp" \
        --argjson noise "$noise" --argjson curve "$curve" --argjson base "$baseline" --argjson rows "$rows" \
        --argjson cached "$cached" --argjson beam_rows "$beam_rows" \
        '{id: $id, kind: $kind, reference_spp: $spp, noise_floor: $noise, convergence: $curve, baseline: $base, adaptive: $rows, path_cache: $cached, beam: $beam_rows}')
    records+=("$record")
done 3< <(jq -c '.scenes[]' "$here/scenes.json")

if [[ $dry_run -eq 1 ]]; then
    echo
    echo "Dry run: nothing was rendered or written."
    exit 0
fi

if [[ ${#records[@]} -eq 0 ]]; then
    echo "no scene matches" >&2
    exit 1
fi

scenes_json=$(printf '%s\n' "${records[@]}" | jq -s -c '.')

# The selection rule (only with --adaptive): the loosest target that passes on every scene.
selection='{}'
if [[ $adaptive -eq 1 ]]; then
    selection=$(jq -c '
        . as $scenes
        | ["relative", "absolute"]
        | map(
            . as $m
            | ([$scenes[0].adaptive[] | select(.metric == $m) | .target] | unique) as $targets
            | ([$targets[] | . as $t
                | select([$scenes[] | .adaptive[] | select(.metric == $m and .target == $t) | .passes]
                         | (length == ($scenes | length)) and all)]) as $ok
            | {key: $m,
               value: (if ($ok | length) > 0
                       then {loosest_target: ($ok | max),
                             samples_saving: (reduce $scenes[] as $s ({};
                                . + {($s.id): ($s.adaptive[] | select(.metric == $m and .target == ($ok | max)) | .figures.samples_saving)}))}
                       else {loosest_target: null, note: "no target passes on every scene"} end)})
        | from_entries' <<<"$scenes_json")
fi

jq -n \
    --arg generated "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --arg cli "$cli" \
    --argjson scenes "$scenes_json" --argjson selection "$selection" \
    --argjson area "$area_margin" --argjson tile "$tile_factor" --argjson recall "$min_recall" \
    '{generated: $generated, cli: $cli,
      rule: {against: "REF2; baseline = fixed spp at adaptive_max_spp",
             changed_area_pct_at_most_baseline_plus: $area, worst_tile_de_at_most_baseline_times: $tile,
             glint_recall_at_least: $recall, glint_recall_null_passes: true},
      scenes: $scenes, selection: $selection}' >"$out_dir/results.json"

echo
echo "Noise floor (REF against REF2; the worst frame for a tilt scene)"
jq -r '.scenes[] | [.id, .reference_spp, .noise_floor.mean_de, .noise_floor.p99_de, .noise_floor.changed_area_pct, .noise_floor.worst_tile_de, (.noise_floor.glint_recall // "n/a")] | @tsv' "$out_dir/results.json" \
    | { printf 'scene\treference_spp\tmean_de\tp99_de\tchanged_area_pct\tworst_tile_de\tglint_recall\n'; cat; } \
    | { column -t -s "$(printf '\t')" 2>/dev/null || cat; }

if [[ $adaptive -eq 1 ]]; then
    echo
    echo "Adaptive runs"
    jq -r '.scenes[] | .id as $id | ([$id, "baseline", .baseline.max_spp, "-", .baseline.changed_area_pct, .baseline.worst_tile_de, (.baseline.glint_recall // "n/a"), .baseline.samples_saving, .baseline.seconds]), (.adaptive[] | [$id, .metric, .target, .passes, .figures.changed_area_pct, .figures.worst_tile_de, (.figures.glint_recall // "n/a"), .figures.samples_saving, .figures.seconds]) | @tsv' "$out_dir/results.json" \
        | { printf 'scene\tmetric\ttarget\tpasses\tchanged_area_pct\tworst_tile_de\tglint_recall\tsamples_saving\tseconds\n'; cat; } \
        | { column -t -s "$(printf '\t')" 2>/dev/null || cat; }
    echo
    echo "Selection (loosest target that passes on every scene)"
    jq -c '.selection' "$out_dir/results.json"
fi

if [[ $path_cache -eq 1 ]]; then
    echo
    echo "Path cache runs (against the fixed-spp baseline at the same cap; cache_used false = a plain render, see its note)"
    jq -r '.scenes[] | .id as $id | .baseline.seconds as $bs | (.path_cache[] | [$id, .mode, .passes, .timed_out, .figures.changed_area_pct, .figures.worst_tile_de, (.figures.glint_recall // "n/a"), (.figures.seconds // .seconds), $bs, .stats.used, .stats.cache_line]) | @tsv' "$out_dir/results.json" \
        | { printf 'scene\tmode\tpasses\ttimed_out\tchanged_area_pct\tworst_tile_de\tglint_recall\tseconds\tbaseline_seconds\tcache_used\tcache\n'; cat; } \
        | { column -t -s "$(printf '\t')" 2>/dev/null || cat; }
fi

if [[ $beam -eq 1 ]]; then
    echo
    echo "Beam runs (against the fixed-spp baseline at the same cap; the default engines of --render-arg first)"
    jq -r '.scenes[] | .id as $id | .baseline.seconds as $bs | (.beam[] | select(.engine == "default") | [$id, .mode, .passes, .timed_out, .declined, .figures.changed_area_pct, .figures.worst_tile_de, (.figures.glint_recall // "n/a"), (.figures.seconds // .seconds), $bs, .stats.leaves, .stats.residual_pct]) | @tsv' "$out_dir/results.json" \
        | { printf 'scene\tmode\tpasses\ttimed_out\tdeclined\tchanged_area_pct\tworst_tile_de\tglint_recall\tseconds\tbaseline_seconds\tleaves\tresidual_pct\n'; cat; } \
        | { column -t -s "$(printf '\t')" 2>/dev/null || cat; }
fi

if [[ $matrix -eq 1 ]]; then
    echo
    echo "Engine matrix (beam run against the fixed-spp run on the same engines; speedup = baseline seconds / beam seconds)"
    jq -r '.scenes[] | .id as $id | (.beam[] | select(.engine != "default") | (.figures.seconds // .seconds) as $bsec | [$id, .engine, .passes, .timed_out, .declined, .figures.changed_area_pct, .figures.worst_tile_de, (.figures.glint_recall // "n/a"), $bsec, .engine_baseline.seconds, (if ($bsec // 0) > 0 then (.engine_baseline.seconds / $bsec) else null end), .stats.leaves, .stats.residual_pct]) | @tsv' "$out_dir/results.json" \
        | { printf 'scene\tengine\tpasses\ttimed_out\tdeclined\tchanged_area_pct\tworst_tile_de\tglint_recall\tbeam_seconds\tbaseline_seconds\tspeedup\tleaves\tresidual_pct\n'; cat; } \
        | { column -t -s "$(printf '\t')" 2>/dev/null || cat; }
fi

echo
echo "Wrote $out_dir/results.json"
