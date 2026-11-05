#!/usr/bin/env python3
"""Compatibility entry point for the trainer's native --pgo workload.

New callers should invoke their trainer executable with --pgo directly. This
wrapper preserves the former --binary interface; all workload arguments and
environment variables are forwarded unchanged to the native implementation.
"""

import argparse
import os
from pathlib import Path
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False,
                                     add_help=False)
    parser.add_argument("--binary", type=Path)
    args, workload_args = parser.parse_known_args()
    if args.binary is None:
        if "--help" in workload_args or "-h" in workload_args:
            print(__doc__)
            print("Usage: profile-workload.py --binary PATH [native --pgo options]")
            return
        parser.error("--binary is required")
    try:
        binary = args.binary.resolve(strict=True)
        if not binary.is_file():
            parser.error("--binary must name a regular executable file")
        # Replace the wrapper so exit status, signals and profile flushing belong
        # to the real CLI; Python contains no duplicate training/recovery logic.
        os.execv(str(binary), [str(binary), "--pgo", *workload_args])
    except OSError as error:
        print(f"error: cannot execute trainer: {error}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
