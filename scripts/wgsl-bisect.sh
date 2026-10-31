#!/usr/bin/env bash
# Bisects the GPU-only uniaxial fault (2026-10-22: Zircon/Tourmaline/Quartz/Ruby... come back
# ~1e38 on AMD RADV since the render-physics waves) by rebuilding the shaders with one revert at
# a time (`PLEOCHROMA_WGSL_BISECT`, see `BISECT_PATCHES` in crates/pleochroma/build.rs) and
# running the GPU equivalence harness for each.
#
# Usage (from anywhere inside the repository, on the machine with the GPU):
#   scripts/wgsl-bisect.sh                  # baseline, every key alone, then all keys together
#   scripts/wgsl-bisect.sh finish begin     # only these keys (each alone), plus baseline and all
#
# Writes target/wgsl-bisect/summary.txt: per variant the build warning that lists the reverted
# sites, the uniaxial / event-wavefront check lines and the harness verdict. Full logs next to it.

set -u
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root" || exit 1
out_dir="$repo_root/target/wgsl-bisect"
mkdir -p "$out_dir"
summary="$out_dir/summary.txt"

keys=("$@")
if [ ${#keys[@]} -eq 0 ]; then
  keys=(ori_zoned bragg oriented)
fi
variants=("" "${keys[@]}" "all")

{
  echo "wgsl-bisect summary"
  echo "date: $(date -u '+%Y-%m-%d %H:%M:%S UTC')"
  echo "host: $(uname -srm)"
  echo
} > "$summary"

for variant in "${variants[@]}"; do
  name="${variant:-baseline}"
  log="$out_dir/$name.log"
  echo ">>> $name"
  PLEOCHROMA_WGSL_BISECT="$variant" PLEOCHROMA_REQUIRE_GPU=1 \
    cargo run --profile probe -p pleochroma --features gpu --example gpu_equivalence_harness \
    > "$log" 2>&1
  code=$?
  {
    echo "=================================================================="
    echo "variant: $name (exit $code, full log: target/wgsl-bisect/$name.log)"
    grep -E "WGSL bisect:" "$log" | sed 's/^warning: [^:]*: //' | sort -u
    grep -E "^\[(Tier 3|Specialisation|Event Wavefront)\].*(Zircon|Tourmaline|Quartz|Rutile|Moissanite|Ruby|Star Silk|zoned dichroic|class 2)" "$log"
    grep -E -A 4 "^\[Furnace, NEE scattering\]" "$log"
    grep -E "^gpu_equivalence_harness: (ALL|FAILED)|^error" "$log" | cut -c1-200
    echo
  } >> "$summary"
done

echo
echo "summary: $summary"
