#!/usr/bin/env python3
"""Check a packaged or recovered macOS archive without executing its binary."""

import os
from pathlib import Path
import plistlib
import shutil
import stat
import subprocess
import tempfile
import zipfile


def main():
    product = os.environ["PRODUCT"]
    version = os.environ["APP_VERSION"]
    target = os.environ["RUST_TARGET"]
    arch = {"aarch64-apple-darwin": "arm64", "x86_64-apple-darwin": "x86_64"}[target]
    filename = f"pleochroma-{product}-{version}-macos-{arch}.zip"
    archive_path = Path("dist", filename).resolve()
    if product == "cut":
        executable = "Pleochroma Cut.app/Contents/MacOS/pleochroma-cut"
    elif product == "worker":
        executable = f"pleochroma-worker-{version}-macos-{arch}/pleochroma-worker"
    else:
        raise ValueError(f"Unknown product: {product}")

    with zipfile.ZipFile(archive_path) as archive:
        if archive.testzip() is not None:
            raise ValueError("Corrupt macOS archive")
        member = archive.getinfo(executable)
        mode = member.external_attr >> 16
        if not stat.S_ISREG(mode) or mode & 0o111 == 0:
            raise ValueError("Archive must contain a regular, executable Mach-O file")
        if product == "cut":
            plist = plistlib.loads(archive.read("Pleochroma Cut.app/Contents/Info.plist"))
            bundle_version = version.split("-", 1)[0].split("+", 1)[0]
            if plist.get("CFBundleShortVersionString") != bundle_version:
                raise ValueError("Archive's embedded version does not match package_version")
        # Read only the named binary into a fresh file. Do not extract archive paths or links.
        with tempfile.TemporaryDirectory(prefix="pleochroma-macos-verify-") as folder:
            binary = Path(folder, "executable")
            with archive.open(member) as source, binary.open("wb") as output:
                shutil.copyfileobj(source, output)
            actual = subprocess.check_output(["xcrun", "lipo", "-archs", str(binary)], text=True).strip()
            if actual != arch:
                raise ValueError(f"Expected {arch}, got {actual}")
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        print(f"archive={archive_path}", file=output)
    print(f"Verified {filename}: {arch}")


if __name__ == "__main__":
    main()
