# Sourced by the entry scripts that print a `GPU:` line (never executed on its own).
#
#   . "$REPO/scripts/gpu_name.sh"
#   echo "GPU: $(gpu_name)"
#
# gpu_name prints the name of the machine's GPU on one line, the first source that answers wins:
# `nvidia-smi -L` (the first GPU), `vulkaninfo --summary` (its deviceName lines), on the Windows Git
# Bash the video controllers that PowerShell reports, `lspci` (the VGA, 3D and Display lines), else
# `unknown`. A machine without nvidia-smi (a Radeon with RADV, say) still gets its GPU name this
# way. It needs no tool in particular, prints nothing else, and
# never fails the calling script, with or without set -e and set -o pipefail.

gpu_name() {
    local name=""
    if command -v nvidia-smi > /dev/null 2>&1; then
        name="$( (nvidia-smi -L 2> /dev/null || true) | head -n 1)"
    fi
    if [ -z "$name" ] && command -v vulkaninfo > /dev/null 2>&1; then
        name="$( (vulkaninfo --summary 2> /dev/null || true) \
            | sed -n 's/^[[:space:]]*deviceName[[:space:]]*=[[:space:]]*//p' | tr -d '\r' \
            | awk 'NF { printf "%s%s", (n++ ? "; " : ""), $0 }')"
    fi
    if [ -z "$name" ]; then
        case "$(uname -s 2> /dev/null)" in
            MINGW* | MSYS* | CYGWIN*)
                if command -v powershell.exe > /dev/null 2>&1; then
                    name="$( (powershell.exe -NoProfile -Command \
                        '(Get-CimInstance Win32_VideoController).Name' 2> /dev/null || true) \
                        | tr -d '\r' | awk 'NF { printf "%s%s", (n++ ? "; " : ""), $0 }')"
                fi
                ;;
        esac
    fi
    if [ -z "$name" ] && command -v lspci > /dev/null 2>&1; then
        name="$( (lspci 2> /dev/null || true) | grep -E 'VGA compatible|3D controller|Display controller' \
            | sed 's/^[^ ]* //' | awk 'NF { printf "%s%s", (n++ ? "; " : ""), $0 }')"
    fi
    printf '%s\n' "${name:-unknown}"
    return 0
}
