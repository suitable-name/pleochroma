#!/usr/bin/env python3
"""Check that a packaged macOS archive actually works before it is uploaded or published.

`verify-macos-archive.py` checks the archive's layout without running anything. This check
unpacks the very ZIP that would be published, the way a user's Mac does (`ditto`), and then:

1. verifies its code signature strictly (`codesign --verify --deep --strict`). An unsigned
   bundle around an arm64 executable that carries only the linker's ad-hoc signature fails
   here; a downloaded copy of such a bundle is what macOS reports as "damaged and can't be
   opened";
2. starts the program with `--version` on this runner, which has the archive's architecture,
   and requires the exit status 0 and the line `pleochroma-<product> <version>`. A binary the
   kernel refuses to run (an invalid signature on Apple silicon, a missing dependency, the
   wrong architecture) fails here.

The workflow runs it after packaging and before every upload, in every mode, so a release
only ever receives an archive that passed both.
"""

import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile

ARCH = {"aarch64-apple-darwin": "arm64", "x86_64-apple-darwin": "x86_64"}

# `--version` answers before any window, GPU or network work; a minute is generous even on a
# loaded runner.
VERSION_TIMEOUT_S = 60


def run(command, **kwargs):
    """Runs `command`, prints it with its output, and returns the completed process."""
    print("+", " ".join(str(part) for part in command), flush=True)
    done = subprocess.run(command, capture_output=True, text=True, **kwargs)
    for stream in (done.stdout, done.stderr):
        if stream.strip():
            print(stream.rstrip(), flush=True)
    return done


def fail(message):
    print(f"::error::{message}", flush=True)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as output:
            print(f"Smoke check failed: {message}", file=output)
    sys.exit(1)


def main():
    product = os.environ["PRODUCT"]
    version = os.environ["APP_VERSION"]
    target = os.environ["RUST_TARGET"]
    arch = ARCH[target]
    machine = platform.machine()
    if machine != arch:
        fail(f"the {arch} archive must be checked on a {arch} runner, not on {machine}")

    archive = Path("dist", f"pleochroma-{product}-macos-{arch}.zip").resolve()
    if not archive.is_file():
        fail(f"no archive at {archive}")

    with tempfile.TemporaryDirectory(prefix="pleochroma-macos-smoke-") as folder:
        unpacked = Path(folder)
        if run(["ditto", "-x", "-k", str(archive), str(unpacked)]).returncode != 0:
            fail(f"ditto could not unpack {archive.name}")
        if product == "cut":
            signed = unpacked / "Pleochroma Cut.app"
            executable = signed / "Contents" / "MacOS" / "pleochroma-cut"
        elif product == "worker":
            signed = unpacked / f"pleochroma-worker-{version}-macos-{arch}" / "pleochroma-worker"
            executable = signed
        else:
            fail(f"unknown product {product}")
        if not executable.is_file():
            fail(f"{archive.name} has no {executable.relative_to(unpacked)}")

        # The signature as the user's Mac sees it after the download is unpacked.
        verify = run(["codesign", "--verify", "--deep", "--strict", "--verbose=2", str(signed)])
        if verify.returncode != 0:
            fail(
                f"{signed.name} in {archive.name} fails `codesign --verify --strict`; "
                "a downloaded copy would be reported as damaged"
            )
        run(["codesign", "--display", "--verbose=2", str(signed)])
        # Gatekeeper's verdict, for the log only: an app without a Developer ID and
        # notarization is always "rejected" here, which is expected.
        run(["spctl", "--assess", "--type", "execute", "--verbose=2", str(signed)])

        expected = f"pleochroma-{product} {version}"
        try:
            started = run([str(executable), "--version"], timeout=VERSION_TIMEOUT_S)
        except subprocess.TimeoutExpired:
            fail(f"`{executable.name} --version` did not finish within {VERSION_TIMEOUT_S} s")
        if started.returncode != 0:
            fail(
                f"`{executable.name} --version` exited with status {started.returncode} "
                "(a negative status is the signal that killed it)"
            )
        answer = started.stdout.strip()
        if answer != expected:
            fail(f"`{executable.name} --version` printed {answer!r}, expected {expected!r}")

    message = f"{archive.name}: signature verified, `--version` printed `{expected}` on {machine}."
    print(message)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as output:
            print(message, file=output)


if __name__ == "__main__":
    main()
