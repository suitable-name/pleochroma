#!/usr/bin/env bash
# Package an already-built Cut or Worker executable; this never invokes Cargo.
# --preflight checks the packaging tools before a workflow spends time compiling.
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
    echo "Packaging requires macOS (xcrun, lipo, otool and ditto)." >&2
    exit 1
fi
preflight=false
if [[ "${1:-}" == --preflight ]]; then
    preflight=true
    shift
fi
if [[ $# -lt 2 || $# -gt 3 ]]; then
    echo "Usage: bash scripts/package-macos.sh [--preflight] <apple-darwin target> <version> [cut|worker]" >&2
    exit 1
fi

target="$1"
version="$2"
product="${3:-cut}"
case "$product" in
    cut|worker) binary_name="pleochroma-$product" ;;
    *) echo "Unsupported macOS product: $product (expected cut or worker)" >&2; exit 1 ;;
esac
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
    for tool in xcrun ditto install awk mktemp mkdir rm cat; do
        command -v "$tool" >/dev/null
    done
    if [[ "$product" == cut ]]; then
        for tool in sips iconutil plutil; do
            command -v "$tool" >/dev/null
        done
        test -r apps/pleochroma-cut/assets/icon.png
    fi
    xcrun --find lipo
    xcrun --find otool
    test -r LICENSE
    # Exercise the same query used below on a real Mach-O without building one.
    # A system executable may be universal or arm64e, regardless of our target.
    probe_archs="$(binary_architectures /usr/bin/true)"
    test -n "$probe_archs"
    printf 'Packaging preflight passed: product=%s target=%s version=%s system-slices=%s\n' \
        "$product" "$target" "$version" "$probe_archs"
    exit 0
fi

binary="${CARGO_TARGET_DIR:-target}/$target/release/$binary_name"
test -f "$binary"
# These archives are architecture-specific, so require exactly the requested slice.
actual_archs="$(binary_architectures "$binary")"
printf 'Package architecture: expected=%s actual=%s binary=%s\n' "$arch" "$actual_archs" "$binary"
if [[ "$actual_archs" != "$arch" ]]; then
    echo "Expected a single $arch executable, found: $actual_archs" >&2
    exit 1
fi

# Reject accidental Homebrew/Qt/rust-toolchain dylib dependencies rather than ship
# a binary that only runs on the build runner. Apple framework/system dylibs are allowed.
xcrun otool -L "$binary" | awk '
    NR > 1 && $1 !~ /^\/System\/Library\// && $1 !~ /^\/usr\/lib\// {
        print "Unbundled dynamic dependency: " $0 > "/dev/stderr"
        invalid = 1
    }
    END { exit invalid }
'

stage_dir="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/pleochroma-$product-macos.XXXXXX")"
trap 'rm -rf "$stage_dir"' EXIT
if [[ "$product" == worker ]]; then
    package="$stage_dir/${binary_name}-${version}-macos-${arch}"
    mkdir -p "$package"
    install -m 755 "$binary" "$package/$binary_name"
    install -m 644 LICENSE "$package/LICENSE.txt"
    # Worker is a terminal application. Its operator-provided inputs and runtime
    # HDR cache are not installation assets, so no .app or resource bundle is needed.
    cat > "$package/README.txt" <<README
Pleochroma Worker $version for macOS $arch
Requires macOS $deployment_target or newer.

Open Terminal in this directory and run:
  ./pleochroma-worker --help
  ./pleochroma-worker serve --help
  ./pleochroma-worker cert --help

Render-capable builds also provide:
  ./pleochroma-worker join --help
  ./pleochroma-worker render --help

Join an existing coordinator using an enrolled worker certificate directory:
  ./pleochroma-worker join coordinator.example:7880 --cert-dir ./worker-cert
For token enrollment, see join --help and cert --help.

Render a supplied scene to a PNG:
  ./pleochroma-worker render --scene scene.json --out render.png --width 800 --height 600 --samples 16

serve is the coordinator; add --render to use this machine for rendering too.
Networking uses mutual TLS by default. Supply your own certificates, and use
serve --db with your own database for catalogue queries. Certificates, databases,
scene files and HDR environment maps are not included in this archive.
HDR maps are transferred and cached at runtime; PLEOCHROMA_ASSET_CACHE_DIR can
select a writable cache directory. Run from a writable working directory or
configure explicit paths for your data and certificate directories.

This executable is unsigned and has not been notarized.
README
else
    app="$stage_dir/Pleochroma Cut.app"
    package="$app"
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
fi

# Keep executable modes and the package directory intact through artifact upload.
# No Developer ID signing, notarization or release publishing is performed.
mkdir -p dist
# Unversioned, so releases/latest/download/<name> links stay valid; the version is
# inside (Info.plist, the worker's folder name and README).
archive="$repo_root/dist/${binary_name}-macos-${arch}.zip"
ditto -c -k --sequesterRsrc --keepParent "$package" "$archive"
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    printf 'archive=%s\n' "$archive" >> "$GITHUB_OUTPUT"
fi
printf 'Created %s\n' "$archive"
