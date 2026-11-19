# Sourced by the test scripts (never executed on its own):
#
#   . "$REPO/scripts/radv_workaround.sh"
#   radv_test_workaround on "$REPO"     # `off` for --stock-driver
#   echo "GPU driver: $RADV_STATUS"
#
# The RADV scheduler workaround for test runs, as the app, the CLI and the worker apply it at start
# (`apply` in crates/pleochroma/src/renderer/gpu_driver.rs). `cargo test` never runs their `main`,
# so without this the GPU tests on Mesa's RADV driver use the stock shader compiler, whose
# instruction scheduler mis-schedules the large kernels: the sorted wavefront's drain kernel, for
# one, then differs from the megakernel.
#
# On Linux with an AMD GPU on the `amdgpu` kernel driver (the rule of `gpu_driver::amd_gpu_present`:
# a /sys/class/drm/card<N> with vendor 0x1002 and that driver) it exports ACO_DEBUG with the value
# of `RADV_ACO_DEBUG`, read from gpu_driver.rs, the one place it is written down. It also points
# Mesa's shader cache at a folder of its own (MESA_SHADER_CACHE_DIR), or switches the cache off
# when the folder cannot be made: Mesa's cache does not tell the two compilers apart, so a shader
# compiled without the workaround must never be served to a run with it. Another vendor's driver
# ignores both variables.
#
# An ACO_DEBUG the caller set is left alone. `off` keeps the stock compiler, to check whether a
# newer Mesa fixed its scheduler: it sets PLEOCHROMA_RADV_WORKAROUND=off, so a CLI or worker the
# script starts does not turn the workaround on in its own `main` either. RADV_STATUS gets one line
# saying what happened, which is also printed. The function never fails the calling script, with
# or without set -e and set -u.

radv_test_workaround() {
    local wanted="${1:-on}"
    local repo="${2:-.}"
    RADV_STATUS=""
    if [ -n "${ACO_DEBUG:-}" ]; then
        RADV_STATUS="RADV workaround: ACO_DEBUG=$ACO_DEBUG was set by the caller and is left alone"
    elif [ "$wanted" = "off" ]; then
        export PLEOCHROMA_RADV_WORKAROUND=off
        RADV_STATUS="RADV workaround: off (--stock-driver), the stock shader compiler, also for the CLI and the worker"
    elif [ "$(uname -s 2> /dev/null)" != "Linux" ]; then
        RADV_STATUS="RADV workaround: not applicable on this platform"
    else
        local amd=0 card vendor driver
        for card in /sys/class/drm/card*; do
            case "${card##*/}" in *-*) continue ;; esac
            [ -r "$card/device/vendor" ] || continue
            vendor="$(tr -d '[:space:]' < "$card/device/vendor" 2> /dev/null || true)"
            driver="$(readlink "$card/device/driver" 2> /dev/null || true)"
            if [ "$vendor" = "0x1002" ] && [ "${driver##*/}" = "amdgpu" ]; then
                amd=1
                break
            fi
        done
        if [ "$amd" = 0 ]; then
            RADV_STATUS="RADV workaround: not needed (no AMD GPU on the amdgpu driver)"
        else
            local value
            value="$(sed -n 's/^pub const RADV_ACO_DEBUG: &str = "\([^"]*\)";.*$/\1/p' \
                "$repo/crates/pleochroma/src/renderer/gpu_driver.rs" 2> /dev/null | head -n 1 || true)"
            if [ -z "$value" ]; then
                RADV_STATUS="RADV workaround: NOT applied, RADV_ACO_DEBUG was not found in crates/pleochroma/src/renderer/gpu_driver.rs"
            else
                export ACO_DEBUG="$value"
                local cache="${XDG_CACHE_HOME:-${HOME:-/tmp}/.cache}/pleochroma-tests/mesa-shader-cache-$value"
                if mkdir -p "$cache" 2> /dev/null; then
                    export MESA_SHADER_CACHE_DIR="$cache"
                    RADV_STATUS="RADV workaround on: ACO_DEBUG=$value, MESA_SHADER_CACHE_DIR=$cache"
                else
                    export MESA_SHADER_CACHE_DISABLE=true
                    RADV_STATUS="RADV workaround on: ACO_DEBUG=$value, Mesa shader cache disabled (no cache folder)"
                fi
            fi
        fi
    fi
    printf '%s\n' "$RADV_STATUS"
    return 0
}
