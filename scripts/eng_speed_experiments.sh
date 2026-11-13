#!/usr/bin/env bash
# Engine speed experiments (the staged kernel, the sorted wavefront and the hybrid rule). Every
# render is the benchmark scene s1_glints with --compute local; the material, the engines and the
# switches change.
#
# Estimator: every row renders with exit collection, the CLI's estimator.
#
#   S  the staged megakernel: Synthetic Moissanite, Diamond and Sapphire at 12 and 32 bounces
#      and 512 samples on the GPU alone, the megakernel (PLEOCHROMA_STAGED=0) against the staged
#      kernel with the stage ends 8, 5, "5,12" and "8,16" (PLEOCHROMA_STAGED=<list>; ends at or
#      over the bounce cap are left out), all with PLEOCHROMA_GPU_TIMING=1. Prints the rate of each
#      row from its "gpu_timing pass" lines and each staged row's factor over its megakernel row.
#   E  the sorted wavefront (PLEOCHROMA_SORTED): Synthetic Moissanite, Diamond and Sapphire at
#      12 and 32 bounces and 512 samples on the GPU alone, each as the megakernel
#      (PLEOCHROMA_STAGED=0 PLEOCHROMA_SORTED=0), the staged kernel (PLEOCHROMA_STAGED=<the
#      --staged-ends list>), the sorted wavefront with a 256 MiB and a 1024 MiB ray pool
#      (PLEOCHROMA_SORTED=1 and 1024) and, at 32 bounces, the sorted wavefront without wgpu's check
#      of every indirect dispatch (WGPU_VALIDATION_INDIRECT_CALL=0). Every row
#      writes its linear image (--raw-out); the notes give each row's pass seconds, its factor over
#      the megakernel and over the staged row, and image-compare against the megakernel row (mean
#      and p99 Delta E).
#   H  the hybrid rule: Synthetic Moissanite at 32 bounces with --local gpu and --local cpu+gpu.
#      Run it on a typical GPU machine with --parts H.
#
# Every timed render follows an untimed warm-up of the same pipeline at 16 samples, so the timed run
# loads its compiled kernels from the cache. The seconds include the CLI's start-up (about 2 s),
# the same in every row.
#
# Tools: bash, grep, sed, awk, cmp and timeout (Linux, and the Windows Git Bash); no jq.
#
# Usage (from the repository root):
#   bash scripts/eng_speed_experiments.sh [--repo DIR] [--cli PATH] [--no-build] [--spp N]
#                                         [--parts LIST] [--staged-ends LIST]
#                                         [--row-timeout DURATION]
#
#   --repo DIR              repository root (default: the current directory)
#   --cli PATH              measure this pleochroma-cli binary and build nothing
#   --no-build              measure <repo>/target/quick-release/pleochroma-cli as it is. Without
#                           this the script builds it first (the command of bench_render_accel.sh
#                           without --with-beam, a CLI without beam tracing; seconds when nothing
#                           changed), so the rows never time an old binary
#   --spp N                 samples per pixel of the long rows of part H (default 2048)
#   --parts LIST            comma list of S, E and H (default: S,E)
#   --staged-ends LIST      the stage ends of part E's staged rows (default 5,12, the code
#                           default and the best list of part S on the A100)
#   --row-timeout DURATION  hard stop per render, the warm-ups included (default 20m; `timeout`
#                           syntax)
#
# Output: <repo>/owner-run-logs/eng-speed-<timestamp>/
#   summary.md     one row per timed render, then the notes of each part
#   *.log          the CLI's standard output; *.err its log lines (hybrid decision, gpu_timing)
#   *.plraw        part E: the linear image of each timed render (--raw-out, 4 MiB each)
#   *.compare.txt  part E: image-compare's error output per compared row (why a dE cell is "-")

set -u

REPO="$(pwd)"
CLI=""
SPP=2048
PARTS=",S,E,"
STAGED_ENDS=5,12
ROW_TIMEOUT=20m
BUILD=1
while [ $# -gt 0 ]; do
    case "$1" in
        --repo) REPO="$2"; shift 2 ;;
        --cli) CLI="$2"; BUILD=0; shift 2 ;;
        --no-build) BUILD=0; shift ;;
        --spp) SPP="$2"; shift 2 ;;
        --parts) PARTS=",$2,"; shift 2 ;;
        --staged-ends) STAGED_ENDS="$2"; shift 2 ;;
        --row-timeout) ROW_TIMEOUT="$2"; shift 2 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

cd "$REPO" || { echo "no repo at $REPO" >&2; exit 2; }
# Sources that differ from the last run's get the current time, so cargo rebuilds what changed
# even when the copied files keep older times (scripts/fresh_sources.sh).
. "$REPO/scripts/fresh_sources.sh"
fresh_sources
# gpu_name prints the GPU's name on any machine (scripts/gpu_name.sh).
. "$REPO/scripts/gpu_name.sh"
# Every row sets the engine switches it measures; none comes from the caller's shell. An unset
# PLEOCHROMA_STAGED runs the staged kernel (the default), so every row that
# measures the single megakernel sets PLEOCHROMA_STAGED=0 itself. The rows of part H set nothing
# and run the default engine (the staged kernel).
unset PLEOCHROMA_STAGED PLEOCHROMA_SORTED PLEOCHROMA_GATED_LOOP PLEOCHROMA_GPU_PIPELINE \
    WGPU_VALIDATION_INDIRECT_CALL
CLI="${CLI:-$REPO/target/quick-release/pleochroma-cli}"

# Headless boxes have no session runtime dir; libwayland prints an error line per Vulkan ICD probe
# without it.
if [ -z "${XDG_RUNTIME_DIR:-}" ]; then
    export XDG_RUNTIME_DIR="${TMPDIR:-/tmp}/pleochroma-xdg-$(id -u)"
    mkdir -p -m 700 "$XDG_RUNTIME_DIR"
fi

JOB="$REPO/benchmarks/render_accel/jobs/s1_glints.job.json"
# sed reads and sets the job's keys (no jq: the Windows Git Bash has none), so each one must occur
# exactly once in the job.
for key in width height max_bounces samples_per_pixel; do
    [ "$(grep -c "\"$key\":" "$JOB")" -eq 1 ] \
        || { echo "$JOB: \"$key\" does not occur exactly once" >&2; exit 2; }
done
OUT="$REPO/owner-run-logs/eng-speed-$(date +%Y-%m-%d_%H%M%S)"
mkdir -p "$OUT"
if [ "$BUILD" -eq 1 ]; then
    echo "== build the CLI (quick-release, GPU)"
    cargo build --profile quick-release -p pleochroma-cli --bins \
        --features gpu,pleochroma/serde,pleochroma/hdr,pleochroma-net/compression 2>&1 | tee "$OUT/build.log"
    [ "${PIPESTATUS[0]}" -eq 0 ] || { echo "the CLI build failed, see $OUT/build.log" >&2; exit 1; }
fi
[ -x "$CLI" ] || { echo "no CLI binary at $CLI (build it, or pass --cli)" >&2; exit 2; }
job_width=$(sed -nE 's/^ *"width": *([0-9]+).*/\1/p' "$JOB")
job_height=$(sed -nE 's/^ *"height": *([0-9]+).*/\1/p' "$JOB")
PIXELS=$((job_width * job_height))
SUMMARY="$OUT/summary.md"
NOTES="$OUT/notes.md"
: > "$NOTES"
{
    echo "# ENG speed experiments, $(date '+%Y-%m-%d %H:%M')"
    echo
    echo "CLI: $CLI; s1_glints, $PIXELS pixels"
    echo "GPU: $(gpu_name)"
    echo
    echo "| part | material | bounces | spp | engines | switch | exit | seconds | pass seconds | M samples/s (passes) |"
    echo "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
} > "$SUMMARY"

# job_copy SPP BOUNCES: the s1 job with the given samples per pixel and bounce limit.
job_copy() {
    local copy="$OUT/job_${1}spp_${2}b.json"
    [ -f "$copy" ] || sed -E "s/(\"samples_per_pixel\": *)[0-9]+/\1$1/; s/(\"max_bounces\": *)[0-9]+/\1$2/" \
        "$JOB" > "$copy"
    echo "$copy"
}

# strip_colors FILE: the file without terminal color codes.
strip_colors() {
    sed 's/\x1b\[[0-9;]*m//g' "$1"
}

# pass_stats FILE: "SECONDS RATE" summed over the file's "gpu_timing pass" lines without the first
# one (seconds of GPU passes, million samples per second), or "- -" without such lines.
pass_stats() {
    strip_colors "$1" | grep "gpu_timing pass" | awk '
        NR > 1 {
            for (i = 1; i <= NF; i++) {
                if ($i ~ /^wall_ms=/) { split($i, w, "="); ms += w[2] }
                if ($i ~ /^tuples=/) { split($i, t, "="); tuples += t[2] }
            }
        }
        END { if (ms > 0) printf "%.1f %.2f", ms / 1000, tuples / ms / 1000; else printf "- -" }'
}

# first_pass_ms FILE: wall_ms of the file's first "gpu_timing pass" line (the start-up pass: the
# pipelines, the scene upload, the survivor buffer), or "-".
first_pass_ms() {
    local ms
    ms=$(strip_colors "$1" | grep -m 1 "gpu_timing pass" | sed -E 's/.* wall_ms=([0-9.]+).*/\1/')
    echo "${ms:--}"
}

# 1 while part E runs: every timed render also writes its linear image to <row>.plraw.
RAW=0

# render PART MATERIAL BOUNCES SPP ENGINES SWITCH [VAR=VALUE...]: a warm-up at 16 samples, then the
# timed render with the given environment variables; one summary row.
render() {
    local part="$1" material="$2" bounces="$3" spp="$4" engines="$5" switch="$6"
    shift 6
    local name="${part}_$(echo "$material" | tr ' ' '_')_${bounces}b_${spp}spp_$(echo "$engines" | tr '+' '_')_${switch}"
    echo "== $name"
    # Both renders stop after --row-timeout; --foreground keeps the CLI in this script's process
    # group, where Ctrl-C reaches it.
    env "$@" timeout --foreground "$ROW_TIMEOUT" "$CLI" render "$(job_copy 16 "$bounces")" \
        --material "$material" --compute local --local "$engines" \
        --quiet --out "$OUT/${name}_warmup.png" \
        > "$OUT/${name}_warmup.log" 2>&1
    local start end status
    local raw=()
    if [ "$RAW" -eq 1 ]; then
        raw=(--raw-out "$OUT/$name.plraw")
    fi
    start=$(date +%s.%N)
    env "$@" RUST_LOG=warn,pleochroma_cut::render_bench=info timeout --foreground "$ROW_TIMEOUT" \
        "$CLI" render "$(job_copy "$spp" "$bounces")" --material "$material" \
        --compute local --local "$engines" ${raw[@]+"${raw[@]}"} \
        --verbose --out "$OUT/$name.png" > "$OUT/$name.log" 2> "$OUT/$name.err"
    status=$?
    end=$(date +%s.%N)
    local seconds stats
    seconds=$(awk -v a="$start" -v b="$end" 'BEGIN { printf "%.1f", b - a }')
    stats=$(pass_stats "$OUT/$name.err")
    echo "| $part | $material | $bounces | $spp | $engines | $switch | $status | $seconds | ${stats% *} | ${stats#* } |" >> "$SUMMARY"
}

if [[ "$PARTS" == *",S,"* ]]; then
    for material in "Synthetic Moissanite" "Diamond" "Sapphire"; do
        for bounces in 12 32; do
            render S "$material" "$bounces" 512 gpu megakernel PLEOCHROMA_GPU_TIMING=1 PLEOCHROMA_STAGED=0
            for ends in 8 5 5,12 8,16; do
                # At 12 bounces the second end of a pair is at or over the cap: the row would
                # repeat the single-end row.
                if [ "$bounces" -eq 12 ] && [ "$ends" != "${ends%,*}" ]; then
                    continue
                fi
                render S "$material" "$bounces" 512 gpu "staged_${ends/,/_}" \
                    PLEOCHROMA_GPU_TIMING=1 PLEOCHROMA_STAGED="$ends"
            done
        done
    done
    {
        echo
        echo "## S:the staged megakernel against the megakernel"
        echo
        echo "Factor = staged M samples/s (passes) / megakernel M samples/s (passes) of the same"
        echo "material and bounce cap. A row with 0 staged chunks is invalid (the megakernel ran)."
        echo "A row with scaled chunks had survivor regions under half the chunk (rays that do not fit stay in their threads): its factor is a"
        echo "lower bound. Pick the default stage ends from the list with the best factors and no"
        echo "row under 0.97 (verify_ENG.md of round 2026-11-11). First pass ms: the"
        echo "start-up pass, left out of every rate (the megakernel row's in brackets). PNG ="
        echo "megakernel: the two 8-bit pictures are byte-equal (a staged picture may differ by rounding)."
        echo
        echo "| row | staged chunks | scaled chunks | M samples/s | factor | first pass ms | PNG = megakernel |"
        echo "| --- | --- | --- | --- | --- | --- | --- |"
        for err in "$OUT"/S_*_staged_*.err; do
            [ -f "$err" ] || continue
            base="${err%_staged_*}_megakernel.err"
            rate=$(pass_stats "$err"); rate="${rate#* }"
            ref=$(pass_stats "$base"); ref="${ref#* }"
            chunks=$(strip_colors "$err" | grep -c "gpu_timing staged_chunk")
            scaled=$(strip_colors "$err" | grep -c "gpu_timing staged_chunk.*scaled=true")
            factor=$(awk -v a="$rate" -v b="$ref" 'BEGIN { if (a + 0 > 0 && b + 0 > 0) printf "%.2f", a / b; else printf "-" }')
            same=no
            cmp -s "${err%.err}.png" "${base%.err}.png" && same=yes
            echo "| $(basename "$err" .err) | $chunks | $scaled | $rate | $factor | $(first_pass_ms "$err") ($(first_pass_ms "$base")) | $same |"
        done
    } >> "$NOTES"
fi

if [[ "$PARTS" == *",E,"* ]]; then
    RAW=1
    for material in "Synthetic Moissanite" "Diamond" "Sapphire"; do
        for bounces in 12 32; do
            render E "$material" "$bounces" 512 gpu megakernel PLEOCHROMA_GPU_TIMING=1 \
                PLEOCHROMA_STAGED=0 PLEOCHROMA_SORTED=0
            render E "$material" "$bounces" 512 gpu staged PLEOCHROMA_GPU_TIMING=1 \
                PLEOCHROMA_STAGED="$STAGED_ENDS" PLEOCHROMA_SORTED=0
            render E "$material" "$bounces" 512 gpu sorted PLEOCHROMA_GPU_TIMING=1 \
                PLEOCHROMA_STAGED=0 PLEOCHROMA_SORTED=1
            render E "$material" "$bounces" 512 gpu sorted1024 PLEOCHROMA_GPU_TIMING=1 \
                PLEOCHROMA_STAGED=0 PLEOCHROMA_SORTED=1024
            if [ "$bounces" -eq 32 ]; then
                # The same without wgpu's check of every indirect dispatch (one more tiny dispatch
                # and three transitions each, five or seven per round): what the check costs.
                render E "$material" "$bounces" 512 gpu sortednoval PLEOCHROMA_GPU_TIMING=1 \
                    PLEOCHROMA_STAGED=0 PLEOCHROMA_SORTED=1 WGPU_VALIDATION_INDIRECT_CALL=0
            fi
        done
    done
    RAW=0
    {
        echo
        echo "## E: the sorted wavefront (E3) against the megakernel and the staged kernel"
        echo
        echo "pass s = seconds of the GPU passes after the first (the first pass is left out of"
        echo "every rate, as in part S). vs mk / vs staged = the row's M samples/s (passes)"
        echo "over the megakernel row's / the staged row's of the same material and bounce cap;"
        echo "\"-\": the row or its reference has no pass lines (its exit is in the first table). Chunks: the"
        echo "row's \"gpu_timing sorted_chunk\" lines (staged rows: \"staged_chunk\"); 0 means the"
        echo "engine did not run and the row is invalid. dE: image-compare of the row's linear"
        echo "image against its megakernel row's (mean, p99); a twin stays under 0.05 mean."
        echo "Default rule (owner, 2026-11-11; e3_sorted_1111/verify.md): the sorted wavefront"
        echo "replaces the staged kernel as the default only when sorted reaches 1.10 vs staged on"
        echo "Synthetic Moissanite and Diamond at 32 bounces, no sorted row is under 0.97 vs staged"
        echo "here AND in part E on a typical GPU (the Windows desktop), every image is a twin, the"
        echo "E3 tests are green on the A100 and on the Windows desktop (Vulkan), and the owner accepts the cold"
        echo "compile of one class on Windows. Otherwise the sorted wavefront is removed the version"
        echo "after."
        echo
        echo "Staged rows: PLEOCHROMA_STAGED=$STAGED_ENDS (--staged-ends)."
        echo
        echo "| row | chunks | pass s | M samples/s | vs mk | vs staged | mean dE | p99 dE |"
        echo "| --- | --- | --- | --- | --- | --- | --- | --- |"
        for err in "$OUT"/E_*.err; do
            [ -f "$err" ] || continue
            row=$(basename "$err" .err)
            base="${err%_gpu_*}_gpu"
            bounces=$(echo "$row" | sed -E 's/.*_([0-9]+)b_512spp_.*/\1/')
            stats=$(pass_stats "$err")
            mk=$(pass_stats "${base}_megakernel.err"); mk="${mk#* }"
            st=$(pass_stats "${base}_staged.err"); st="${st#* }"
            case "$row" in
                *_megakernel) chunks="-" ;;
                *_staged) chunks=$(strip_colors "$err" | grep -c "gpu_timing staged_chunk") ;;
                *) chunks=$(strip_colors "$err" | grep -c "gpu_timing sorted_chunk") ;;
            esac
            vs_mk=$(awk -v a="${stats#* }" -v b="$mk" 'BEGIN { if (a + 0 > 0 && b + 0 > 0) printf "%.2f", a / b; else printf "-" }')
            vs_st=$(awk -v a="${stats#* }" -v b="$st" 'BEGIN { if (a + 0 > 0 && b + 0 > 0) printf "%.2f", a / b; else printf "-" }')
            image="- -"
            if [ "$err" != "${base}_megakernel.err" ] && [ -f "${base}_megakernel.plraw" ] \
                && [ -f "$OUT/$row.plraw" ]; then
                image=$("$CLI" image-compare "${base}_megakernel.plraw" "$OUT/$row.plraw" \
                    --tonemap-from "$(job_copy 512 "$bounces")" 2> "$OUT/$row.compare.txt" \
                    | awk -F': ' '$1 == "mean_de" { m = $2 } $1 == "p99_de" { p = $2 }
                        END { if (m != "" && p != "") print m, p }') || image="- -"
                [ -n "$image" ] || image="- -"
            fi
            echo "| $row | $chunks | ${stats% *} | ${stats#* } | $vs_mk | $vs_st | ${image% *} | ${image#* } |"
        done
    } >> "$NOTES"
fi

if [[ "$PARTS" == *",H,"* ]]; then
    for spp in "$SPP" 64; do
        render H "Synthetic Moissanite" 32 "$spp" gpu default
        render H "Synthetic Moissanite" 32 "$spp" cpu+gpu default
    done
    {
        echo
        echo "## H: the hybrid rule"
        echo
        echo "Pass: at each sample count the cpu+gpu seconds are at most the gpu seconds times 1.03 plus 0.5 s."
        echo "On a machine whose GPU does 70 % or more of the work the decision line below says gpu_only=true."
        echo
        echo '```'
        for err in "$OUT"/H_*cpu_gpu*.err; do
            [ -f "$err" ] || continue
            echo "$(basename "$err"):"
            strip_colors "$err" | grep "hybrid split" | sed 's/^/  /'
        done
        echo '```'
    } >> "$NOTES"
fi

cat "$NOTES" >> "$SUMMARY"
echo
cat "$SUMMARY"
echo "Logs: $OUT"
