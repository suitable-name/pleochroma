# Sourced by the entry scripts that run cargo on the Linux box (never executed on its own).
#
#   . "$REPO/scripts/fresh_sources.sh"   # after the cd to the repository root
#   fresh_sources                        # before the first cargo command
#   $(sources_hash)                      # the 12-hex "sources" hash of the Repo: identity line
#
# Why: cargo decides what to rebuild by comparing modification times. The owner copies the tree
# onto the box's clone and keeps the files' own times, so a changed file can look older than the
# box's last build of its crate. Then the crate stays stale, silently: the build-script binary can
# stay older than a copied build.rs, so the generated shader units (for example
# sorted_wavefront.generated.wgsl) are never written and the build fails. Any crate whose changed
# files all look older than its last build stays stale the same way.
#
# What: fresh_sources compares the hash of every source file's content with the hash stored at the
# last run. When they differ (or nothing is stored) it gives every source file the current time,
# so cargo rebuilds what changed, and stores the new hash. It needs no knowledge of how the tree
# was copied. It does nothing, and prints nothing, off Linux, on a Windows checkout seen through
# WSL (/mnt/..., whose edits carry fresh times), without git, or outside a git work tree. It never
# fails the calling script, with or without set -e and set -o pipefail. An entry script started by
# another one (the parts of run_full_round.sh) only stores the new hash and touches nothing: what
# changed during the run already carries fresh times.

# Prints the 12-hex hash of the content of every file git sees (owner-run-logs left out), so the
# same files give the same hash on any machine. Run from the repository root.
sources_hash() {
    git ls-files -z --cached --others --exclude-standard -- . ':(exclude)owner-run-logs' 2>/dev/null | xargs -0 sha1sum 2>/dev/null | sha1sum | cut -c1-12
}

# Touches every source file when the sources differ from the last run's. Run from the repository
# root, before the first cargo command.
fresh_sources() {
    local repo stamp now_hash old_hash
    if [ "$(uname -s 2>/dev/null)" != "Linux" ]; then
        return 0
    fi
    repo="$(pwd)"
    case "$repo" in
        /mnt/*) return 0 ;;
    esac
    if ! command -v git > /dev/null 2>&1; then
        return 0
    fi
    if ! git rev-parse --is-inside-work-tree > /dev/null 2>&1; then
        return 0
    fi
    stamp="${CARGO_TARGET_DIR:-$repo/target}/.pleochroma-sources-stamp"
    # Without pipefail, as the Repo: lines run it: under the pgo scripts' set -o pipefail one file
    # that sha1sum cannot read (a tracked file deleted from the tree) would fail the pipeline and
    # turn the guard off without a word.
    now_hash="$(set +o pipefail; sources_hash)" || return 0
    old_hash="$(cat "$stamp" 2> /dev/null)" || old_hash=""
    # The entry scripts a run starts (the parts of run_full_round.sh) inherit the mark: every file
    # that changes during the run (a Cargo.lock that cargo rewrote) already has a fresh time, so
    # they only store the new hash and the builds of the earlier parts stay valid.
    local nested="${PLEOCHROMA_SOURCES_CHECKED:-}"
    export PLEOCHROMA_SOURCES_CHECKED=1
    if [ "$now_hash" = "$old_hash" ]; then
        return 0
    fi
    if [ -z "$nested" ]; then
        echo "fresh_sources: the sources differ from the last run's; every source file gets the current time so cargo rebuilds what changed."
        git ls-files -z --cached --others --exclude-standard -- . ':(exclude)owner-run-logs' 2> /dev/null | xargs -0 -r touch -c -- 2> /dev/null || true
    fi
    mkdir -p "$(dirname "$stamp")" 2> /dev/null || return 0
    printf '%s\n' "$now_hash" > "$stamp" 2> /dev/null || true
    return 0
}
