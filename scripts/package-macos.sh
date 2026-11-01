#!/usr/bin/env bash
# Package an already-built desktop executable; this script never invokes Cargo.
# --preflight checks the packaging tools before a workflow spends time compiling.
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
    echo "Packaging requires macOS (sips, iconutil, plutil, lipo and ditto)." >&2
    exit 1
fi
preflight=false
if [[ "${1:-}" == --preflight ]]; then
    preflight=true
    shift
fi
if [[ $# -ne 2 ]]; then
    echo "Usage: bash scripts/package-macos.sh [--preflight] <apple-darwin target> <version>" >&2
    exit 1
fi

target="$1"
version="$2"
case "$target" in
    aarch64-apple-darwin) arch=arm64 ;;
    x86_64-apple-darwin) arch=x86_64 ;;
    *) echo "Unsupported macOS target: $target" >&2; exit 1 ;;
esac
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.+-]+)?$ ]]; then
    echo "Expected a semantic app version, got: $version" >&2
    exit 1
fi
bundle_version="${version%%[-+]*}"
deployment_target="${MACOSX_DEPLOYMENT_TARGET:-13.0}"
if [[ ! "$deployment_target" =~ ^[0-9]+\.[0-9]+(\.[0-9]+)?$ ]]; then
    echo "Invalid MACOSX_DEPLOYMENT_TARGET: $deployment_target" >&2
    exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

binary_architectures() {
    # Apple's -verify_arch consumes every following argument as an architecture.
    # -archs is explicitly intended for scripts and has no variable argument list.
    # https://github.com/apple-oss-distributions/cctools/blob/main/man/lipo.1
    xcrun lipo -archs "$1"
}

if [[ "$preflight" == true ]]; then
    for tool in xcrun sips iconutil plutil ditto install awk; do
        command -v "$tool" >/dev/null
    done
    xcrun --find lipo
    xcrun --find otool
    test -r LICENSE
    test -r apps/pleochroma-cut/assets/icon.png
    # Exercise the same query used below on a real Mach-O without building one.
    # A system executable may be universal or arm64e, regardless of our target.
    probe_archs="$(binary_architectures /usr/bin/true)"
    test -n "$probe_archs"
    printf 'Packaging preflight passed: target=%s version=%s system-slices=%s\n' \
        "$target" "$version" "$probe_archs"
    exit 0
fi

binary="${CARGO_TARGET_DIR:-target}/$target/release/pleochroma-cut"
test -f "$binary"
# These archives are architecture-specific, so require exactly the requested slice.
actual_archs="$(binary_architectures "$binary")"
printf 'Package architecture: expected=%s actual=%s binary=%s\n' "$arch" "$actual_archs" "$binary"
if [[ "$actual_archs" != "$arch" ]]; then
    echo "Expected a single $arch executable, found: $actual_archs" >&2
    exit 1
fi

# Reject accidental Homebrew/Qt/rust-toolchain dylib dependencies rather than ship
# an app that only runs on the build runner. Apple framework/system dylibs are allowed.
xcrun otool -L "$binary" | awk '
    NR > 1 && $1 !~ /^\/System\/Library\// && $1 !~ /^\/usr\/lib\// {
        print "Unbundled dynamic dependency: " $0 > "/dev/stderr"
        invalid = 1
    }
    END { exit invalid }
'

stage_dir="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/pleochroma-macos.XXXXXX")"
trap 'rm -rf "$stage_dir"' EXIT
app="$stage_dir/Pleochroma Cut.app"
resources="$app/Contents/Resources"
mkdir -p "$app/Contents/MacOS" "$resources"
install -m 755 "$binary" "$app/Contents/MacOS/pleochroma-cut"
install -m 644 LICENSE "$resources/LICENSE.txt"

# Slint embeds its UI, images and manual at compile time. The bundle needs only
# the native application icon; derive an iconset from the committed desktop PNG.
iconset="$stage_dir/Pleochroma.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" apps/pleochroma-cut/assets/icon.png \
        --out "$iconset/icon_${size}x${size}.png" >/dev/null
    twice=$((size * 2))
    sips -z "$twice" "$twice" apps/pleochroma-cut/assets/icon.png \
        --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil --convert icns "$iconset" --output "$resources/Pleochroma.icns"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>com.indicatrix-suite.cut</string>
    <key>CFBundleName</key><string>Pleochroma Cut</string>
    <key>CFBundleDisplayName</key><string>Pleochroma Cut</string>
    <key>CFBundleExecutable</key><string>pleochroma-cut</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$bundle_version</string>
    <key>CFBundleVersion</key><string>$bundle_version</string>
    <key>CFBundleIconFile</key><string>Pleochroma.icns</string>
    <key>LSMinimumSystemVersion</key><string>$deployment_target</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist"

# Keep executable modes and the .app directory structure intact through artifact upload.
# No Developer ID signing, notarization or release publishing is performed.
mkdir -p dist
archive="$repo_root/dist/pleochroma-cut-${version}-macos-${arch}.zip"
ditto -c -k --sequesterRsrc --keepParent "$app" "$archive"
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    printf 'archive=%s\n' "$archive" >> "$GITHUB_OUTPUT"
fi
printf 'Created %s\n' "$archive"
