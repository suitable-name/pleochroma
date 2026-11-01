#!/usr/bin/env bash
# Profile the real trainer CLI, including its selected device's host-side work.
# CUDA kernels and dynamically loaded driver libraries are not rewritten by PGO/BOLT.
set -euo pipefail

usage() {
    cat <<'HELP'
Usage: bash scripts/pgo-bolt-trainer.sh --db /path/facet_diagrams.sqlite [options]

Linux native builds only. The instrumented executable must run on this machine.
CUDA requires an accessible NVIDIA GPU and CUDA/NVRTC runtime during profiling.

  --device cuda|cpu|gpu      Training device (default: cuda; gpu means WGPU)
  --bolt                     Also instrument and rewrite the PGO executable with BOLT
                             (Linux x86_64 only; llvm-bolt and merge-fdata required)
  --cpu CPU                  rustc target CPU (default: native, tuned to this host)
  --jobs N                   Concurrent Cargo jobs (default: 1)
  --workers N                Profiling rollout slots (default: 4)
  --batch-size N             Profiling steps per rollout (default: 32)
  --mini-batch N             Profiling update mini-batch (default: 16)
  --iterations N             Profiling iterations per scenario (default: 2)
  --validation-designs N     Held-out profiling designs (default: 1)
  --max-tiers N              Maximum design tiers (default: 128)
  --full-fidelity             Profile Full scoring instead of Fast (much slower)
  --design-dir DIR           Also load authored/native designs from this directory
  --output-dir DIR           Parent of a fresh run directory (default: target/pgo-trainer)
  --bin-dir DIR              Final executable directory (default: project bin)
  -h, --help                 Show this help

No existing checkpoint or profile is overwritten. Each invocation creates a fresh
directory containing logs, workload checkpoints, raw profiles and artifacts:
  artifacts/pleochroma-trainer-cli-pgo
  artifacts/pleochroma-trainer-cli-pgo-bolt  (only with --bolt)

Only after all requested stages succeed, the selected result atomically replaces
bin/pleochroma-trainer-cli (or --bin-dir/pleochroma-trainer-cli).

Install matching LLVM tools with `rustup component add llvm-tools-preview`.
PGO_LLVM_BIN may name a matching LLVM tool directory. LLVM major.minor must match
rustc. BOLT tools must come from the same installation (BOLT_BIN, or PATH).
This profiles finite workloads, not a runtime deadline; geometry may take a while.
HELP
}

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
need_value() { [[ $# -ge 2 && -n "$2" ]] || die "Missing value for $1"; }
positive() { [[ "$2" =~ ^[1-9][0-9]*$ ]] || die "$1 must be a positive integer"; }
bounded() {
    positive "$1" "$2"
    [[ ${#2} -le 9 && "$2" -ge "$3" && "$2" -le "$4" ]] || die "$1 must be between $3 and $4"
}

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEVICE=cuda
DB=
DESIGN_DIR=
CPU=native
JOBS=1
WORKERS=4
BATCH_SIZE=32
MINI_BATCH=16
ITERATIONS=2
VALIDATION_DESIGNS=1
MAX_TIERS=128
OUTPUT_DIR="$ROOT/target/pgo-trainer"
BIN_DIR="$ROOT/bin"
USE_BOLT=0
FULL_FIDELITY=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --db) need_value "$@"; DB="$2"; shift 2 ;;
        --device) need_value "$@"; DEVICE="$2"; shift 2 ;;
        --cpu) need_value "$@"; CPU="$2"; shift 2 ;;
        --jobs) need_value "$@"; JOBS="$2"; shift 2 ;;
        --workers) need_value "$@"; WORKERS="$2"; shift 2 ;;
        --batch-size) need_value "$@"; BATCH_SIZE="$2"; shift 2 ;;
        --mini-batch) need_value "$@"; MINI_BATCH="$2"; shift 2 ;;
        --iterations) need_value "$@"; ITERATIONS="$2"; shift 2 ;;
        --validation-designs) need_value "$@"; VALIDATION_DESIGNS="$2"; shift 2 ;;
        --max-tiers) need_value "$@"; MAX_TIERS="$2"; shift 2 ;;
        --design-dir) need_value "$@"; DESIGN_DIR="$2"; shift 2 ;;
        --output-dir) need_value "$@"; OUTPUT_DIR="$2"; shift 2 ;;
        --bin-dir) need_value "$@"; BIN_DIR="$2"; shift 2 ;;
        --bolt) USE_BOLT=1; shift ;;
        --full-fidelity) FULL_FIDELITY=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "Unknown argument: $1" ;;
    esac
done
[[ "$(uname -s)" = Linux ]] || die 'Run this script on the Linux training host.'
case "$DEVICE" in cuda|cpu|gpu) ;; *) die '--device must be cuda, cpu or gpu' ;; esac
positive --jobs "$JOBS"
bounded --workers "$WORKERS" 1 1024
bounded --batch-size "$BATCH_SIZE" 16 4096
bounded --mini-batch "$MINI_BATCH" 8 1024
bounded --iterations "$ITERATIONS" 2 1000000
bounded --validation-designs "$VALIDATION_DESIGNS" 1 64
bounded --max-tiers "$MAX_TIERS" 1 128
[[ "$MINI_BATCH" -le "$BATCH_SIZE" ]] || die '--mini-batch must not exceed --batch-size'
[[ -n "$DB" && -r "$DB" && -f "$DB" ]] || die '--db must name a readable design database'
for tool in cargo rustc; do command -v "$tool" >/dev/null || die "Missing $tool"; done
DB="$(realpath -- "$DB")"
if [[ -n "$DESIGN_DIR" ]]; then
    [[ -d "$DESIGN_DIR" ]] || die '--design-dir must name a directory'
    DESIGN_DIR="$(realpath -- "$DESIGN_DIR")"
fi
cd "$ROOT"
HOST="$(rustc -vV | sed -n 's/^host: //p')"
case "$HOST" in
    x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) ;;
    *) die "Unsupported native target: $HOST (expected Linux GNU x86_64 or aarch64)" ;;
esac
if [[ "$USE_BOLT" = 1 && "$HOST" != x86_64-unknown-linux-gnu ]]; then
    die 'The optional BOLT path is supported here only on Linux x86_64.'
fi

llvm_version() {
    sed -nE 's/.*LLVM version:? ([0-9]+\.[0-9]+).*/\1/p' | head -n 1
}
RUST_LLVM="$(rustc -vV | llvm_version)"
[[ -n "$RUST_LLVM" ]] || die 'Cannot determine rustc LLVM version'
SYSROOT="$(rustc --print sysroot)"
find_profdata() {
    local candidate version
    local -a candidates=()
    if [[ -n "${PGO_LLVM_BIN:-}" ]]; then
        candidate="$PGO_LLVM_BIN/llvm-profdata"
        [[ -x "$candidate" ]] || die "PGO_LLVM_BIN does not contain an executable llvm-profdata: $candidate"
        version="$("$candidate" --version | llvm_version)"
        [[ "$version" = "$RUST_LLVM" ]] || die "PGO_LLVM_BIN provides LLVM $version; rustc uses $RUST_LLVM"
        printf '%s\n' "$candidate"
        return
    fi
    candidates+=("$SYSROOT/lib/rustlib/$HOST/bin/llvm-profdata")
    candidates+=("/usr/lib/llvm/${RUST_LLVM%%.*}/bin/llvm-profdata")
    candidates+=("/usr/lib/llvm-${RUST_LLVM%%.*}/bin/llvm-profdata")
    if command -v llvm-profdata >/dev/null 2>&1; then candidates+=("$(command -v llvm-profdata)"); fi
    for candidate in "${candidates[@]}"; do
        [[ -x "$candidate" ]] || continue
        version="$("$candidate" --version | llvm_version)"
        if [[ "$version" = "$RUST_LLVM" ]]; then printf '%s\n' "$candidate"; return; fi
    done
    die "No llvm-profdata matching rustc LLVM $RUST_LLVM. Install llvm-tools-preview or set PGO_LLVM_BIN."
}
PROFDATA="$(find_profdata)"
if [[ "$USE_BOLT" = 1 ]]; then
    if [[ -n "${BOLT_BIN:-}" ]]; then
        BOLT="$BOLT_BIN/llvm-bolt"
    else
        BOLT="$(command -v llvm-bolt)" || die 'Missing llvm-bolt (set BOLT_BIN or PATH)'
    fi
    [[ -x "$BOLT" ]] || die "Cannot execute $BOLT"
    BOLT="$(realpath -- "$BOLT")"
    MERGE_FDATA="$(dirname "$BOLT")/merge-fdata"
    [[ -x "$MERGE_FDATA" ]] || die "Missing matching BOLT companion $MERGE_FDATA"
fi

mkdir -p "$OUTPUT_DIR"
OUTPUT_DIR="$(cd "$OUTPUT_DIR" && pwd)"
RUN_DIR="$(mktemp -d "$OUTPUT_DIR/run-$(date -u +%Y%m%dT%H%M%SZ)-XXXXXX")"
PUBLISH_TEMP=
trap 'code=$?; if [[ -n "$PUBLISH_TEMP" ]]; then rm -f -- "$PUBLISH_TEMP"; fi; if [[ $code -ne 0 ]]; then printf "Failed (exit %s); retained diagnostics: %s\n" "$code" "$RUN_DIR" >&2; fi' EXIT
mkdir -p "$RUN_DIR/raw" "$RUN_DIR/logs" "$RUN_DIR/artifacts"
MANIFEST="$ROOT/apps/pleochroma-trainer/Cargo.toml"
TARGET_DIR="$RUN_DIR/target"
[[ -f "${MANIFEST%Cargo.toml}Cargo.lock" ]] || die 'Standalone trainer Cargo.lock is required'

# Both builds select the identical manifest, binary, features, target and target directory.
# An explicit --target keeps instrumentation out of host build scripts and proc macros.
CARGO_ARGS=(build --locked --release --jobs "$JOBS" --manifest-path "$MANIFEST"
    --no-default-features --bin pleochroma-trainer-cli --target "$HOST" --target-dir "$TARGET_DIR")
[[ "$DEVICE" != cuda ]] || CARGO_ARGS+=(--features cuda)
WORKLOAD_ARGS=(--db "$DB" --device "$DEVICE" --workers "$WORKERS"
    --batch-size "$BATCH_SIZE" --mini-batch "$MINI_BATCH" --iterations "$ITERATIONS"
    --validation-designs "$VALIDATION_DESIGNS" --max-tiers "$MAX_TIERS")
[[ -z "$DESIGN_DIR" ]] || WORKLOAD_ARGS+=(--design-dir "$DESIGN_DIR")
[[ "$FULL_FIDELITY" = 0 ]] || WORKLOAD_ARGS+=(--full-fidelity)

# The script owns codegen flags. Preserve runtime device/cache variables, but exclude
# inherited compiler flags and wrappers which can silently change the profile identity.
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS LLVM_PROFILE_FILE
export RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER= CARGO_INCREMENTAL=0
export CARGO_PROFILE_RELEASE_DEBUG=1 CARGO_PROFILE_RELEASE_STRIP=none
export CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
BASE_FLAGS=(-C "target-cpu=$CPU")
if [[ "$USE_BOLT" = 1 ]]; then
    BASE_FLAGS+=(-C force-frame-pointers=yes -C link-arg=-Wl,--emit-relocs)
fi
encode_flags() { local IFS=$'\x1f'; printf '%s' "$*"; }
{
    rustc -vV
    cargo --version
    "$PROFDATA" --version
    printf 'Device: %s\nCPU: %s\nDatabase: %s\n' "$DEVICE" "${CPU:-target default}" "$DB"
    printf 'Cargo invocation: '; printf '%q ' cargo "${CARGO_ARGS[@]}"; printf '\n'
    printf 'Workload invocation: '; printf '%q ' "$TARGET_DIR/$HOST/release/pleochroma-trainer-cli" --pgo --output-dir "$RUN_DIR/pgo-workload" "${WORKLOAD_ARGS[@]}"; printf '\n'
    if [[ "$USE_BOLT" = 1 ]]; then "$BOLT" --version; fi
    git rev-parse HEAD || true
    git status --short || true
} > "$RUN_DIR/provenance.txt"
printf 'Profiling output: %s\n' "$RUN_DIR"

printf '\n==> Build instrumented trainer\n'
# Atomic counters avoid races between the persistent CPU pool and training thread.
CARGO_ENCODED_RUSTFLAGS="$(encode_flags "${BASE_FLAGS[@]}" -C "profile-generate=$RUN_DIR/raw" -C llvm-args=-instrprof-atomic-counter-update-all=true)" \
    cargo "${CARGO_ARGS[@]}" 2>&1 | tee "$RUN_DIR/logs/build-generate.log"
BINARY="$TARGET_DIR/$HOST/release/pleochroma-trainer-cli"
printf '\n==> Collect PGO profile with actual training, validation and recovery\n'
LLVM_PROFILE_FILE="$RUN_DIR/raw/trainer-%m-%p.profraw" \
    "$BINARY" --pgo --output-dir "$RUN_DIR/pgo-workload" \
    "${WORKLOAD_ARGS[@]}" 2>&1 | tee "$RUN_DIR/logs/pgo-workload.log"
shopt -s nullglob
RAW_PROFILES=("$RUN_DIR/raw/"*.profraw)
[[ ${#RAW_PROFILES[@]} -gt 0 ]] || die 'The completed workload wrote no raw PGO profiles'
"$PROFDATA" merge -o "$RUN_DIR/merged.profdata" "${RAW_PROFILES[@]}"
[[ -s "$RUN_DIR/merged.profdata" ]] || die 'The merged PGO profile is empty'
"$PROFDATA" show --all-functions --counts "$RUN_DIR/merged.profdata" > "$RUN_DIR/profile-functions.txt"
grep -q 'pleochroma_ai' "$RUN_DIR/profile-functions.txt" || die 'The profile contains no pleochroma-ai functions'
"$PROFDATA" show "$RUN_DIR/merged.profdata" | tee "$RUN_DIR/logs/profile-summary.log"

printf '\n==> Build PGO trainer from the matching profile\n'
CARGO_ENCODED_RUSTFLAGS="$(encode_flags "${BASE_FLAGS[@]}" -C "profile-use=$RUN_DIR/merged.profdata" -C llvm-args=-pgo-warn-missing-function)" \
    cargo "${CARGO_ARGS[@]}" 2>&1 | tee "$RUN_DIR/logs/build-use.log"
if grep -Ei 'hash mismatch|profile data may be out of date|malformed instrumentation profile' "$RUN_DIR/logs/build-use.log"; then
    die 'PGO reported an incompatible profile; inspect build-use.log'
fi
PGO_BINARY="$RUN_DIR/artifacts/pleochroma-trainer-cli-pgo"
install -m 755 "$BINARY" "$PGO_BINARY"

if [[ "$USE_BOLT" = 1 ]]; then
    mkdir -p "$RUN_DIR/bolt"
    printf '\n==> Instrument PGO executable for BOLT\n'
    "$BOLT" "$PGO_BINARY" -instrument --instrumentation-file="$RUN_DIR/bolt/profile.fdata" \
        --instrumentation-file-append-pid -o "$RUN_DIR/bolt/trainer-instrumented" \
        2>&1 | tee "$RUN_DIR/logs/bolt-instrument.log"
    chmod +x "$RUN_DIR/bolt/trainer-instrumented"
    printf '\n==> Collect BOLT profile with the same workload\n'
    "$RUN_DIR/bolt/trainer-instrumented" --pgo \
        --output-dir "$RUN_DIR/bolt-workload" "${WORKLOAD_ARGS[@]}" \
        2>&1 | tee "$RUN_DIR/logs/bolt-workload.log"
    FDATA_PROFILES=("$RUN_DIR/bolt/"profile.fdata*)
    [[ ${#FDATA_PROFILES[@]} -gt 0 ]] || die 'The BOLT workload wrote no profiles'
    "$MERGE_FDATA" "${FDATA_PROFILES[@]}" > "$RUN_DIR/bolt/merged.fdata"
    [[ -s "$RUN_DIR/bolt/merged.fdata" ]] || die 'The merged BOLT profile is empty'
    printf '\n==> Rewrite the original PGO executable\n'
    "$BOLT" "$PGO_BINARY" -data="$RUN_DIR/bolt/merged.fdata" \
        -o "$RUN_DIR/artifacts/pleochroma-trainer-cli-pgo-bolt" \
        -reorder-blocks=ext-tsp -reorder-functions=hfsort+ \
        -split-functions -split-all-cold -dyno-stats \
        2>&1 | tee "$RUN_DIR/logs/bolt-optimize.log"
    chmod +x "$RUN_DIR/artifacts/pleochroma-trainer-cli-pgo-bolt"
fi
SELECTED_BINARY="$PGO_BINARY"
if [[ "$USE_BOLT" = 1 ]]; then SELECTED_BINARY="$RUN_DIR/artifacts/pleochroma-trainer-cli-pgo-bolt"; fi
mkdir -p "$BIN_DIR"
BIN_DIR="$(cd "$BIN_DIR" && pwd)"
# Stage on the destination filesystem so rename cannot expose a partial executable.
# A failed BOLT stage exits above and never changes an existing published trainer.
PUBLISH_TEMP="$(mktemp "$BIN_DIR/.pleochroma-trainer-cli.XXXXXX")"
install -m 755 "$SELECTED_BINARY" "$PUBLISH_TEMP"
mv -fT -- "$PUBLISH_TEMP" "$BIN_DIR/pleochroma-trainer-cli"
PUBLISH_TEMP=
printf '\nFinished. Executables and all diagnostics are retained in:\n  %s\n' "$RUN_DIR"
printf 'PGO: %s\n' "$PGO_BINARY"
if [[ "$USE_BOLT" = 1 ]]; then printf 'PGO+BOLT: %s/artifacts/pleochroma-trainer-cli-pgo-bolt\n' "$RUN_DIR"; fi
printf 'Ready to run: %s/pleochroma-trainer-cli\n' "$BIN_DIR"
printf 'Measure these against a normal release on representative training runs; speedup is not guaranteed.\n'
