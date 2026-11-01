#!/usr/bin/env python3
"""Resolve a macOS workflow's products, version and release before compiling."""

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib


def gh_json(*args):
    return json.loads(subprocess.check_output(["gh", *args], text=True))


def main():
    product = os.environ.get("PRODUCT", "both")
    if product not in ("cut", "worker", "both"):
        sys.exit("product must be cut, worker or both")
    products = ("cut", "worker") if product == "both" else (product,)
    package_run = os.environ.get("PACKAGE_FROM_RUN", "").strip()
    publish_run = os.environ.get("PUBLISH_FROM_RUN", "").strip()
    original_version = os.environ.get("PACKAGE_VERSION", "").strip()
    if package_run and publish_run:
        sys.exit("Choose package_from_run OR publish_from_run, not both")
    run_id = package_run or publish_run
    if run_id and not re.fullmatch(r"[1-9][0-9]*", run_id):
        sys.exit("The source run ID must be the numeric ID from /actions/runs/<ID>")
    if run_id and not original_version:
        sys.exit("Reusing a run requires package_version from its original build")
    if original_version and not run_id:
        sys.exit("package_version requires package_from_run or publish_from_run")
    package = tomllib.loads(Path("Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]
    version = original_version if run_id else package["version"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+([+-][0-9A-Za-z.+-]+)?", version):
        sys.exit(f"Invalid application version: {version!r}")
    mode = "publish" if publish_run else "package" if package_run else "build"
    repo = os.environ["GITHUB_REPOSITORY"]
    if run_id:
        run = gh_json("api", f"repos/{repo}/actions/runs/{run_id}")
        if run["status"] != "completed":
            sys.exit("Wait for the source run to finish before reusing its artifacts")
        if run.get("head_repository", {}).get("full_name", "").lower() != repo.lower():
            sys.exit("Release artifacts must come from a run of this repository, not a fork")
        print(f"Artifact source: {run['html_url']} (commit {run['head_sha']})")

    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
    is_release = os.environ["GITHUB_EVENT_NAME"] == "release"
    attach = is_release or os.environ.get("ATTACH_TO_RELEASE", "true") == "true"
    tag = event["release"]["tag_name"] if is_release else os.environ.get("RELEASE_TAG", "").strip()
    if not attach and (tag or publish_run):
        sys.exit("release_tag and publish_from_run require attach_to_release=true")
    release_url = ""
    if attach:
        # Resolve latest ONCE. A new release during the build must not redirect uploads.
        args = ["release", "view", "--repo", repo, "--json", "tagName,isDraft,isImmutable,url"]
        if tag:
            args.extend(["--", tag])
        release = gh_json(*args)
        tag = release["tagName"]
        if release["isDraft"] or release.get("isImmutable", False):
            sys.exit("The destination release must be published and accept asset uploads")
        if tag.removeprefix("v") != version:
            sys.exit(f"Release {tag} does not match binary version {version}; select the matching "
                     "release_tag or disable attach_to_release for an artifact-only build")
        release_url = release["url"]
        print(f"Release destination: {release_url}")

    common = "pleochroma/serde,pleochroma/hdr,pleochroma-net/compression,pleochroma/gpu"
    arches = (
        ("macos-15", "aarch64-apple-darwin", "arm64", ""),
        ("macos-15-intel", "x86_64-apple-darwin", "x86_64", "-C target-cpu=x86-64"),
    )
    matrix = {"include": [
        {"product": app, "runner": runner, "target": target, "arch": arch,
         "rustflags": flags,
         "features": common + (",pleochroma-worker/worker" if app == "worker" else "")
                     + f",pleochroma-{app}/gpu",
         "archive": f"pleochroma-{app}-{version}-macos-{arch}.zip"}
        for app in products for runner, target, arch, flags in arches
    ]}
    values = {"matrix": json.dumps(matrix, separators=(",", ":")), "version": version,
              "mode": mode, "source_run": run_id, "release_tag": tag if attach else ""}
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        for key, value in values.items():
            if "\n" in value or "\r" in value:
                sys.exit(f"Invalid multiline output: {key}")
            print(f"{key}={value}", file=output)
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as summary:
        print(f"Products: {', '.join(products)} · version {version} · mode {mode}\n", file=summary)
        print(f"Release: {release_url or 'artifacts only'}\n", file=summary)
        if run_id:
            print(f"Source run: {run_id}\n", file=summary)
        print("CPU build jobs: 1. Architectures/products run sequentially.\n", file=summary)
    print(f"Declared minimum Rust: {package['rust-version']}")


if __name__ == "__main__":
    main()
