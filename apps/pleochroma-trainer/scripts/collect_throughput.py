#!/usr/bin/env python3
"""Run isolated trainer diagnostics and collect host/GPU telemetry using only the stdlib.

This script never builds software or changes a training checkpoint. Stop any other
training process on the selected GPU before running it. Child processes run in
sequence; the timeout applies to the whole child, including CUDA initialization.
"""

import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import subprocess
import sys
import threading
import time
import zipfile


def positive_int(text):
    value = int(text)
    if value < 1:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return value


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", required=True, type=Path,
                        help="built pleochroma-throughput-probe executable")
    parser.add_argument("--optics", required=True, type=Path,
                        help="built optical_throughput example executable")
    parser.add_argument("--database", type=Path,
                        default=Path(os.environ.get("DIAGRAM_DB_PATH", "/notebooks/facet_diagrams.sqlite")))
    parser.add_argument("--output", required=True, type=Path,
                        help="new output directory; existing directories are refused")
    parser.add_argument("--entries", default="69,215,283",
                        help="loaded corpus indices for the targeted projection probe")
    parser.add_argument("--workers", type=positive_int)
    parser.add_argument("--iterations", type=positive_int, default=2)
    parser.add_argument("--warmup", type=positive_int, default=1)
    parser.add_argument("--include-cpu", action="store_true",
                        help="also run the complete CPU training pipeline (potentially slow)")
    parser.add_argument("--timeout-seconds", type=positive_int, default=1800,
                        help="hard limit per child, including initialization (default: 1800)")
    parser.add_argument("--sample-seconds", type=positive_int, default=1)
    parser.add_argument("--skip-nvidia-smi", action="store_true")
    return parser.parse_args()


def now():
    return dt.datetime.now(dt.timezone.utc).isoformat()


def write_json(path, value):
    with path.open("x", encoding="utf-8") as output:
        json.dump(value, output, indent=2, allow_nan=False)
        output.write("\n")


def executable_info(path):
    digest = hashlib.sha256()
    with path.open("rb") as binary:
        for chunk in iter(lambda: binary.read(1024 * 1024), b""):
            digest.update(chunk)
    return {"path": str(path), "bytes": path.stat().st_size, "sha256": digest.hexdigest()}


def text_file(path):
    try:
        return Path(path).read_text(encoding="utf-8").strip()
    except (OSError, UnicodeError):
        return None


def host_metadata(args):
    cpu_model = None
    for line in (text_file("/proc/cpuinfo") or "").splitlines():
        if line.startswith("model name"):
            cpu_model = line.partition(":")[2].strip()
            break
    return {
        "schema": 1,
        "started_at": now(),
        "system": platform.system(),
        "release": platform.release(),
        "machine": platform.machine(),
        "python": platform.python_version(),
        "cpu_model": cpu_model,
        "logical_cpus": os.cpu_count(),
        "cpu_affinity": sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
        "cgroup_cpu_max": text_file("/sys/fs/cgroup/cpu.max"),
        "environment": {key: os.environ.get(key) for key in
                        ("CUDA_VISIBLE_DEVICES", "RAYON_NUM_THREADS", "OMP_NUM_THREADS")},
        "executables": {"probe": executable_info(args.probe), "optics": executable_info(args.optics)},
        "database": {"path": str(args.database), "bytes": args.database.stat().st_size},
        "configuration": {key: str(value) if isinstance(value, Path) else value
                          for key, value in vars(args).items()},
        "interpretation": [
            "GPU telemetry is device-wide, not attributable exclusively to the benchmark.",
            "CPU percent is measured for the child process; 100 percent means one logical CPU.",
            "Production service timers overlap and are not GPU kernel timings.",
            "Completed warmup iterations still teach the model; cases record their own config.",
            "Targeted projection cases begin at imported geometry, not a saved episode's later state.",
        ],
    }


def process_sample(pid):
    """Linux aggregate process CPU ticks and RSS; absent on other platforms."""
    content = text_file(f"/proc/{pid}/stat")
    status = text_file(f"/proc/{pid}/status")
    if content is None:
        return None
    try:
        # comm can contain spaces and parentheses; everything after the last ')' is numeric/state.
        fields = content[content.rindex(")") + 2:].split()
        ticks = int(fields[11]) + int(fields[12])  # Original fields 14 and 15.
        clock_ticks = os.sysconf("SC_CLK_TCK")
        selected = {}
        for line in (status or "").splitlines():
            name, _, value = line.partition(":")
            if name in ("VmRSS", "VmHWM", "VmSize", "Threads"):
                selected[name] = value.strip()
        return {"cpu_seconds": ticks / clock_ticks, "status": selected}
    except (ValueError, IndexError, OSError):
        return None


def gpu_sample(nvidia_smi):
    if nvidia_smi is None:
        return None
    fields = ("timestamp,index,name,uuid,driver_version,utilization.gpu,utilization.memory,"
              "memory.used,memory.total,power.draw,temperature.gpu,clocks.sm,pstate")
    try:
        result = subprocess.run(
            [nvidia_smi, f"--query-gpu={fields}", "--format=csv,noheader,nounits"],
            capture_output=True, text=True, timeout=4, check=False,
        )
        return {"fields": fields.split(","), "csv": result.stdout.strip(),
                "exit_code": result.returncode, "error": result.stderr.strip() or None}
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"error": str(error)}


def monitor(process, destination, stop, origin, interval, nvidia_smi):
    previous = None
    with destination.open("x", encoding="utf-8") as output:
        while not stop.is_set():
            observed = time.monotonic()
            cpu = process_sample(process.pid)
            if cpu is not None:
                cpu["percent_one_core"] = None
                if previous is not None and observed > previous[0]:
                    cpu["percent_one_core"] = max(0.0,
                        100 * (cpu["cpu_seconds"] - previous[1]) / (observed - previous[0]))
                previous = (observed, cpu["cpu_seconds"])
            row = {"observed_at": now(), "elapsed_seconds": observed - origin,
                   "pid": process.pid, "process": cpu, "gpu": gpu_sample(nvidia_smi)}
            output.write(json.dumps(row, allow_nan=False) + "\n")
            output.flush()
            stop.wait(max(0.0, interval - (time.monotonic() - observed)))


def stop_child(process):
    """Only stop the child/group that this script created."""
    if process.poll() is not None:
        return
    try:
        if os.name == "posix":
            os.killpg(process.pid, signal.SIGTERM)
        else:
            process.terminate()
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        try:
            if os.name == "posix":
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
        except ProcessLookupError:
            pass
        process.wait()


def run_case(name, command, root, timeout, interval, nvidia_smi):
    case_dir = root / name
    case_dir.mkdir()
    write_json(case_dir / "command.json", {"argv": command, "started_at": now()})
    print(f"Starting {name}; log: {case_dir / 'stderr.log'}", flush=True)
    origin = time.monotonic()
    stop = threading.Event()
    telemetry_errors = []
    result = {"case": name, "command": command, "started_at": now()}
    with (case_dir / "stdout.log").open("x", encoding="utf-8") as stdout, \
            (case_dir / "stderr.log").open("x", encoding="utf-8") as stderr:
        process = subprocess.Popen(command, stdout=stdout, stderr=stderr,
                                   start_new_session=os.name == "posix")

        def sample():
            try:
                monitor(process, case_dir / "hardware.jsonl", stop, origin, interval, nvidia_smi)
            except Exception as error:  # Telemetry failure must not strand the benchmark child.
                telemetry_errors.append(str(error))

        observer = threading.Thread(target=sample, name="throughput-telemetry")
        interrupted = False
        try:
            observer.start()
            deadline = origin + timeout
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise subprocess.TimeoutExpired(command, timeout)
                try:
                    result["exit_code"] = process.wait(timeout=min(30, remaining))
                    break
                except subprocess.TimeoutExpired:
                    if time.monotonic() >= deadline:
                        raise
                    print(f"{name}: still running after {time.monotonic() - origin:.0f}s; "
                          "live details are in the case report and hardware.jsonl", flush=True)
            result["status"] = "completed" if result["exit_code"] == 0 else "failed"
        except subprocess.TimeoutExpired:
            result["status"] = "timeout"
            stop_child(process)
            result["exit_code"] = process.returncode
        except KeyboardInterrupt:
            interrupted = True
            result["status"] = "interrupted"
            stop_child(process)
            result["exit_code"] = process.returncode
        finally:
            # Covers unexpected exceptions too, including failure to start telemetry.
            stop_child(process)
            stop.set()
            if observer.ident is not None:
                observer.join()  # nvidia-smi itself has a four-second timeout.
    result.update(elapsed_seconds=time.monotonic() - origin, ended_at=now(),
                  telemetry_errors=telemetry_errors)
    write_json(case_dir / "result.json", result)
    print(f"{name}: {result['status']} after {result['elapsed_seconds']:.1f}s", flush=True)
    if interrupted:
        raise KeyboardInterrupt
    return result


def cases(args):
    base = [str(args.probe), "--database", str(args.database),
            "--batch-size", "256", "--mini-batch", "128", "--updates-per-round", "16",
            "--label-every", "2", "--seed", "42"]
    if args.workers is not None:
        base.extend(["--workers", str(args.workers)])
    for device in (["cuda", "cpu"] if args.include_cpu else ["cuda"]):
        name = f"pipeline-{device}"
        yield name, base + ["--scenario", "pipeline", "--device", device,
                           "--warmup", str(args.warmup), "--iterations", str(args.iterations),
                           "--sync-every-iteration", "--output", str(args.output / name / "report")]
    name = "projection-cuda"
    yield name, base + ["--scenario", "projection", "--device", "cuda",
                       "--entries", args.entries, "--repeats", "2", "--max-step-deg", "0.5",
                       "--output", str(args.output / name / "report")]
    name = "optics-cuda"
    yield name, [str(args.optics), "--output", str(args.output / name / "optics.jsonl"),
                 "--requests", "4096", "--repeats", "2", "--in-flight", "1,8,32,128",
                 "--max-seconds", str(args.timeout_seconds), "--heartbeat-seconds", "2"]


def archive(root):
    destination = root.with_name(root.name + ".zip")
    with zipfile.ZipFile(destination, "x", compression=zipfile.ZIP_DEFLATED) as output:
        for path in sorted(root.rglob("*")):
            if path.is_file():
                output.write(path, Path(root.name) / path.relative_to(root))
    return destination


def main():
    args = arguments()
    for name in ("probe", "optics", "database"):
        path = getattr(args, name).expanduser().resolve(strict=True)
        if not path.is_file():
            raise ValueError(f"--{name} must name a regular file")
        setattr(args, name, path)
    args.output = args.output.expanduser().resolve()
    if args.output.exists() or args.output.with_name(args.output.name + ".zip").exists():
        raise ValueError("choose a new --output directory (and matching .zip name)")
    args.output.mkdir(parents=True)
    write_json(args.output / "host.json", host_metadata(args))
    nvidia_smi = None if args.skip_nvidia_smi else shutil.which("nvidia-smi")
    results = []
    failure = None
    try:
        for name, command in cases(args):
            results.append(run_case(name, command, args.output, args.timeout_seconds,
                                    args.sample_seconds, nvidia_smi))
    except KeyboardInterrupt:
        failure = "interrupted by user; partial files retained"
    except Exception as error:
        failure = str(error)
    write_json(args.output / "collection.json", {
        "schema": 1, "completed_at": now(), "failure": failure,
        "nvidia_smi_available": nvidia_smi is not None, "cases": results,
    })
    package = archive(args.output)
    print(f"Send this diagnostic bundle: {package}", flush=True)
    if failure:
        print(failure, file=sys.stderr)
    return 0 if failure is None and all(row["status"] == "completed" for row in results) else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(2)
