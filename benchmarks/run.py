#!/usr/bin/env python3
"""Build and supervise the complete Rust SDK typed-IPC benchmark (stdlib only)."""

from __future__ import annotations

import argparse
import ctypes
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import queue
import shutil
import signal
import subprocess
import sys
import threading
import time


REPO = Path(__file__).resolve().parents[1]
PIN = "bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd"
PARAMETERS = {
    "iterations": 5000, "warmup": 500, "sample_ms": 1000,
    "stress_seconds": 30, "churn": 100, "window": 64, "timeout_seconds": 15,
}
ENV_KEYS = (
    "MOJO_RUNTIME_LIBRARY", "MOJOM_PYTHON", "RUSTFLAGS", "RUSTUP_TOOLCHAIN",
    "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET", "CARGO_PROFILE_RELEASE_LTO",
    "CARGO_PROFILE_RELEASE_CODEGEN_UNITS", "CARGO_PROFILE_RELEASE_OPT_LEVEL",
)


def utc() -> str:
    return datetime.now(timezone.utc).isoformat()


def emit(event: str, **values) -> None:
    print(json.dumps({"event": event, **values}, ensure_ascii=False), flush=True)


def command_text(command: list[str], timeout: float = 3, cwd: Path | None = None) -> str | None:
    try:
        result = subprocess.run(command, capture_output=True, text=True,
                                timeout=timeout, check=False, cwd=cwd)
        return result.stdout.strip() if result.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def sha256(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


class WindowsProcess:
    """Read-only WinAPI measurements; no dependency on PowerShell or psutil."""

    def __init__(self):
        from ctypes import wintypes as w
        self.w = w
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.psapi = ctypes.WinDLL("psapi", use_last_error=True)
        self.kernel.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
        self.kernel.OpenProcess.restype = w.HANDLE
        self.kernel.CloseHandle.argtypes = [w.HANDLE]
        self.kernel.CloseHandle.restype = w.BOOL
        self.kernel.GetProcessTimes.argtypes = [w.HANDLE] + [ctypes.POINTER(w.FILETIME)] * 4
        self.kernel.GetProcessTimes.restype = w.BOOL
        self.kernel.GetProcessHandleCount.argtypes = [w.HANDLE, ctypes.POINTER(w.DWORD)]
        self.kernel.GetProcessHandleCount.restype = w.BOOL

        class Memory(ctypes.Structure):
            _fields_ = [("cb", w.DWORD), ("PageFaultCount", w.DWORD)] + [
                (name, ctypes.c_size_t) for name in (
                    "PeakWorkingSetSize", "WorkingSetSize", "QuotaPeakPagedPoolUsage",
                    "QuotaPagedPoolUsage", "QuotaPeakNonPagedPoolUsage",
                    "QuotaNonPagedPoolUsage", "PagefileUsage", "PeakPagefileUsage", "PrivateUsage")]
        self.Memory = Memory
        self.psapi.GetProcessMemoryInfo.argtypes = [w.HANDLE, ctypes.POINTER(Memory), w.DWORD]
        self.psapi.GetProcessMemoryInfo.restype = w.BOOL

    def inspect(self, pid: int, heavy: bool = False) -> dict:
        result = {"rss_bytes": None, "private_bytes": None, "handle_count": None, "identity": None}
        handle = self.kernel.OpenProcess(0x0400 | 0x0010, False, pid)
        if not handle:
            handle = self.kernel.OpenProcess(0x1000, False, pid)
        if not handle:
            result["error"] = f"OpenProcess failed: {ctypes.get_last_error()}"
            return result
        try:
            times = [self.w.FILETIME() for _ in range(4)]
            if self.kernel.GetProcessTimes(handle, *(ctypes.byref(value) for value in times)):
                result["identity"] = (times[0].dwHighDateTime << 32) | times[0].dwLowDateTime
            memory = self.Memory()
            memory.cb = ctypes.sizeof(memory)
            if self.psapi.GetProcessMemoryInfo(handle, ctypes.byref(memory), memory.cb):
                result.update(rss_bytes=memory.WorkingSetSize, private_bytes=memory.PrivateUsage)
            else:
                result["error"] = f"GetProcessMemoryInfo failed: {ctypes.get_last_error()}"
            if heavy:
                count = self.w.DWORD()
                if self.kernel.GetProcessHandleCount(handle, ctypes.byref(count)):
                    result["handle_count"] = count.value
        finally:
            self.kernel.CloseHandle(handle)
        return result


WINDOWS = WindowsProcess() if os.name == "nt" else None


def process_identity(pid: int):
    try:
        if WINDOWS:
            return WINDOWS.inspect(pid)["identity"]
        if sys.platform.startswith("linux"):
            # The process name may contain spaces and parentheses.
            return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
        started = command_text(["ps", "-o", "lstart=", "-p", str(pid)])
        return {"started": started, "process_group": os.getpgid(pid)} if started else None
    except (OSError, ValueError, IndexError):
        return None


def process_sample(pid: int, heavy: bool = False) -> dict:
    value = {"pid": pid, "rss_bytes": None, "fd_count": None,
             "thread_count": None, "handle_count": None}
    errors = []
    try:
        if WINDOWS:
            measured = WINDOWS.inspect(pid, heavy)
            value.update({key: measured[key] for key in ("rss_bytes", "private_bytes", "handle_count")})
            if measured.get("error"):
                errors.append(measured["error"])
        elif sys.platform.startswith("linux"):
            fields = {}
            for line in Path(f"/proc/{pid}/status").read_text().splitlines():
                if ":" in line:
                    key, content = line.split(":", 1)
                    fields[key] = content.strip()
            if "VmRSS" in fields:
                value["rss_bytes"] = int(fields["VmRSS"].split()[0]) * 1024
            if heavy:
                value["thread_count"] = int(fields["Threads"]) if "Threads" in fields else None
                with os.scandir(f"/proc/{pid}/fd") as entries:
                    value["fd_count"] = sum(1 for _ in entries)
        elif sys.platform == "darwin":
            rss = command_text(["ps", "-o", "rss=", "-p", str(pid)])
            value["rss_bytes"] = int(rss) * 1024 if rss else None
            if heavy:
                lsof = shutil.which("lsof") or "/usr/sbin/lsof"
                descriptors = command_text([lsof, "-a", "-p", str(pid), "-Ff"])
                if descriptors is not None:
                    value["fd_count"] = len({line[1:] for line in descriptors.splitlines()
                                             if line.startswith("f") and line[1:].isdigit()})
                else:
                    errors.append("lsof unavailable, timed out, or process exited")
        else:
            errors.append("resource measurement is not implemented on this OS")
    except Exception as error:
        errors.append(str(error))
    if errors:
        value["errors"] = errors
    return value


def hardware() -> dict:
    result = {"machine": platform.machine(), "processor": platform.processor(),
              "logical_cpus": os.cpu_count(), "physical_cpus": None, "memory_bytes": None}
    try:
        result["memory_bytes"] = os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE")
    except (AttributeError, OSError, ValueError):
        pass
    if sys.platform == "darwin":
        for target, name in (("processor", "machdep.cpu.brand_string"), ("model", "hw.model"),
                             ("memory_bytes", "hw.memsize"), ("physical_cpus", "hw.physicalcpu")):
            value = command_text(["sysctl", "-n", name])
            if value is not None:
                result[target] = int(value) if target.endswith(("bytes", "cpus")) else value
    elif sys.platform.startswith("linux"):
        try:
            cpuinfo = Path("/proc/cpuinfo").read_text()
            for line in cpuinfo.splitlines():
                if line.startswith("model name"):
                    result["processor"] = line.split(":", 1)[1].strip()
                    break
            cores = set()
            for block in cpuinfo.split("\n\n"):
                fields = dict(line.split(":", 1) for line in block.splitlines() if ":" in line)
                fields = {key.strip(): value.strip() for key, value in fields.items()}
                if "physical id" in fields and "core id" in fields:
                    cores.add((fields["physical id"], fields["core id"]))
            result["physical_cpus"] = len(cores) or None
        except OSError:
            pass
    elif WINDOWS:
        class MemoryStatus(ctypes.Structure):
            _fields_ = [("length", ctypes.c_ulong), ("load", ctypes.c_ulong)] + [
                (name, ctypes.c_ulonglong) for name in (
                    "total_physical", "available_physical", "total_pagefile", "available_pagefile",
                    "total_virtual", "available_virtual", "available_extended_virtual")]
        status = MemoryStatus()
        status.length = ctypes.sizeof(status)
        if WINDOWS.kernel.GlobalMemoryStatusEx(ctypes.byref(status)):
            result["memory_bytes"] = status.total_physical
    return result


def popen_options() -> dict:
    if os.name == "nt":
        return {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP}
    return {"start_new_session": True}


def terminate_owned(process: subprocess.Popen, workers: dict) -> list:
    """Only this Popen's private process group/tree and identity-checked workers."""
    actions = []
    if os.name == "nt":
        if process.poll() is None:
            result = command_text(["taskkill", "/F", "/T", "/PID", str(process.pid)], timeout=5)
            actions.append({"kind": "taskkill_parent_tree", "pid": process.pid, "output": result})
    else:
        try:
            os.killpg(process.pid, signal.SIGTERM)
            actions.append({"kind": "SIGTERM_process_group", "pgid": process.pid})
        except ProcessLookupError:
            pass
        except OSError as error:
            actions.append({"kind": "group_signal_error", "error": str(error)})
    # Wait for the direct process, then also stop group members that ignored TERM.
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        if os.name == "nt":
            process.kill()
        else:
            try:
                os.killpg(process.pid, signal.SIGKILL)
                actions.append({"kind": "SIGKILL_process_group", "pgid": process.pid})
            except ProcessLookupError:
                pass
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            actions.append({"kind": "parent_still_running", "pid": process.pid})
    if os.name != "nt":
        try:
            os.killpg(process.pid, signal.SIGKILL)
            actions.append({"kind": "SIGKILL_remaining_process_group", "pgid": process.pid})
        except ProcessLookupError:
            pass
        except OSError as error:
            actions.append({"kind": "group_signal_error", "error": str(error)})
    for pid, worker in workers.items():
        identity = worker.get("identity")
        if identity is None or process_identity(pid) != identity:
            actions.append({"kind": "skip_worker_not_identifiable_or_gone", "pid": pid})
            continue
        try:
            if os.name == "nt":
                command_text(["taskkill", "/F", "/T", "/PID", str(pid)], timeout=5)
            else:
                os.kill(pid, signal.SIGKILL)
            actions.append({"kind": "stop_recorded_worker", "pid": pid})
        except ProcessLookupError:
            pass
        except OSError as error:
            actions.append({"kind": "worker_signal_error", "pid": pid, "error": str(error)})
    return actions


def run_core(command: list[str], output: Path, deadline: float, resources: dict,
             native: Path, resource_interval: float) -> int:
    environment = os.environ.copy()
    environment["MOJO_BENCH_SUPERVISED"] = "1"
    environment["MOJO_RUNTIME_LIBRARY"] = str(native)
    process = subprocess.Popen(command, cwd=REPO, env=environment, stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                               text=True, encoding="utf-8", errors="replace", bufsize=1,
                               **popen_options())
    resources["parent_pid"] = process.pid
    active = {}
    messages = queue.Queue(maxsize=4096)
    started = time.monotonic()

    def read_lines():
        try:
            for line in process.stdout:
                messages.put(line)
        finally:
            messages.put(None)

    reader = threading.Thread(target=read_lines, name="benchmark-log-reader", daemon=True)
    reader.start()
    next_sample, eof = (started if resource_interval else math.inf), False
    try:
        with (output / "raw.log").open("w", encoding="utf-8") as raw:
            while not (eof and process.poll() is not None):
                now = time.monotonic()
                if now >= deadline:
                    raise TimeoutError("supervisor total timeout exceeded")
                if now >= next_sample:
                    tick = time.monotonic()
                    resources["rss_samples"].append({
                        "elapsed_seconds": tick - started,
                        "parent": process_sample(process.pid),
                        "workers": [process_sample(pid) for pid in list(active)],
                        "sampling_ms": (time.monotonic() - tick) * 1000,
                    })
                    next_sample = time.monotonic() + resource_interval
                try:
                    line = messages.get(timeout=min(0.25, max(0.001, deadline - now)))
                except queue.Empty:
                    continue
                if line is None:
                    eof = True
                    continue
                raw.write(line)
                print(line, end="", flush=True)
                try:
                    event = json.loads(line)
                except (ValueError, TypeError):
                    continue
                if not isinstance(event, dict):
                    continue
                kind = event.get("event", event.get("type"))
                if not isinstance(kind, str):
                    continue
                record = {"elapsed_seconds": time.monotonic() - started, **event}
                resources["events"].append(record)
                if kind == "worker_started":
                    pid = event.get("pid")
                    if isinstance(pid, int) and not isinstance(pid, bool) and pid > 0 and pid not in (os.getpid(), process.pid):
                        worker = {"pid": pid, "identity": process_identity(pid),
                                  "started_seconds": record["elapsed_seconds"]}
                        active[pid] = worker
                        resources["workers"].append(worker)
                elif kind == "worker_exited":
                    worker = active.pop(event.get("pid"), None)
                    if worker is not None:
                        worker.update(exited_seconds=record["elapsed_seconds"], exit_code=event.get("exit_code"))
                elif kind == "checkpoint":
                    tick = time.monotonic()
                    try:
                        try:
                            measured = process_sample(process.pid, heavy=True)
                        except Exception as error:
                            measured = {"pid": process.pid, "rss_bytes": None, "fd_count": None,
                                        "thread_count": None, "handle_count": None,
                                        "errors": ["checkpoint measurement failed: " + str(error)]}
                        resources["checkpoints"].append({
                            # Retain native probe fields, including Mach port
                            # namespace counts/errors, without inventing values.
                            **record, "parent": measured,
                            "active_worker_pids": list(active),
                            "sampling_ms": (time.monotonic() - tick) * 1000,
                        })
                    finally:
                        # Even an unavailable metric must never leave the core waiting.
                        process.stdin.write("ACK\n")
                        process.stdin.flush()
                elif kind == "finished":
                    resources["finished_event_seen"] = True
            code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
            if active:
                raise RuntimeError(f"core exited with workers still active: {list(active)}")
            return code
    finally:
        resources["core_elapsed_seconds"] = time.monotonic() - started
        if process.poll() is None or process.poll() != 0 or active or sys.exc_info()[0] is not None:
            resources["cleanup"].extend(terminate_owned(process, active))
        resources["runtime_exit_code"] = process.poll()
        if process.stdin:
            try:
                process.stdin.close()
            except OSError:
                pass
        reader.join(timeout=1)
        # Preserve diagnostics emitted during termination without blocking on EOF.
        with (output / "raw.log").open("a", encoding="utf-8") as raw:
            while True:
                try:
                    trailing = messages.get_nowait()
                except queue.Empty:
                    break
                if trailing is not None:
                    raw.write(trailing)
                    print(trailing, end="", flush=True)
        if process.stdout and not reader.is_alive():
            process.stdout.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True, help="new or empty results directory")
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--binary", type=Path, help="override target/release/mojo-ipc-bench[.exe]")
    parser.add_argument("--total-timeout-seconds", type=float,
                        help="build + benchmark deadline (default: 600 + stress seconds)")
    parser.add_argument("--resource-interval-seconds", type=float, default=1,
                        help="periodic RSS interval (default: 1; 0 disables it, checkpoints still sample)")
    for name, default in PARAMETERS.items():
        parser.add_argument("--" + name.replace("_", "-"), type=int, default=default)
    parser.add_argument("--matrices", default="legacy,primitive,nullable",
                        help="comma-separated legacy,primitive,nullable,complex; default preserves the existing matrix")
    parser.add_argument("--legacy-sizes", default="0,64,1024,65536,1048576")
    parser.add_argument("--legacy-phases", default="codec,raw,typed,oneway,correctness,stress,churn,crash")
    parser.add_argument("--complex-cases", default="utf8_string,record,record_array,nested_array,string_map,numeric_map,union_array,nullable_nested,string_array")
    parser.add_argument("--complex-codec-ms", type=int, default=1000)
    parser.add_argument("--complex-typed-ms", type=int, default=1000)
    parser.add_argument("--complex-iterations", type=int, default=8,
                        help="minimum samples per complex row; use >=200 for formal runs")
    parser.add_argument("--complex-warmup", type=int, default=2)
    args = parser.parse_args()
    for name in PARAMETERS:
        if getattr(args, name) < (0 if name in ("warmup", "sample_ms", "stress_seconds", "churn") else 1):
            parser.error("invalid --" + name.replace("_", "-"))
    for field in ("complex_codec_ms", "complex_typed_ms", "complex_warmup"):
        if getattr(args, field) < 0:
            parser.error("invalid --" + field.replace("_", "-"))
    if args.complex_iterations <= 0:
        parser.error("--complex-iterations must be positive")
    allowed_matrices = {"legacy", "primitive", "nullable", "complex"}
    allowed_complex = {"utf8_string", "record", "record_array", "nested_array", "string_map", "numeric_map", "union_array", "nullable_nested", "string_array"}
    for field, allowed in (("matrices", allowed_matrices), ("complex_cases", allowed_complex)):
        selection = getattr(args, field).split(",")
        if len(selection) != len(set(selection)) or any(item not in allowed for item in selection):
            parser.error("invalid or duplicate --" + field.replace("_", "-"))
    complex_seconds = (len(args.complex_cases.split(",")) * 3 * (args.complex_codec_ms + args.complex_typed_ms) / 1000
                       if "complex" in args.matrices.split(",") else 0)
    timeout = args.total_timeout_seconds if args.total_timeout_seconds is not None else 600 + args.stress_seconds + complex_seconds
    if not math.isfinite(timeout) or timeout <= 0:
        parser.error("--total-timeout-seconds must be finite and positive")
    if not math.isfinite(args.resource_interval_seconds) or args.resource_interval_seconds < 0:
        parser.error("--resource-interval-seconds must be finite and nonnegative")
    output = args.output_dir.expanduser().resolve()
    if output.exists() and (not output.is_dir() or any(output.iterdir())):
        parser.error("--output-dir must be new or empty; previous results are never reused")
    output.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    deadline = started + timeout
    binary = (args.binary or REPO / "target/release" / ("mojo-ipc-bench.exe" if os.name == "nt" else "mojo-ipc-bench")).expanduser().resolve()
    command = [str(binary), "--output", str(output / "result.json")]
    for name in PARAMETERS:
        command.extend(["--" + name.replace("_", "-"), str(getattr(args, name))])
    for name in ("matrices", "legacy_sizes", "legacy_phases", "complex_cases", "complex_codec_ms", "complex_typed_ms", "complex_iterations", "complex_warmup"):
        command.extend(["--" + name.replace("_", "-"), str(getattr(args, name))])
    metadata = {"schema_version": 1, "started_utc": utc(), "benchmark": "complete Rust SDK typed IPC",
                "chromium_commit": PIN, "python": sys.version, "os": platform.platform(),
                "hardware": hardware(), "environment": {key: os.environ[key] for key in ENV_KEYS if key in os.environ},
                "supervised_environment": {"MOJO_BENCH_SUPERVISED": "1"}, "command": command,
                "total_timeout_seconds": timeout, "skip_build": args.skip_build,
                "repository_commit": command_text(["git", "rev-parse", "HEAD"], cwd=REPO),
                "rustc": command_text(["rustc", "-Vv"], cwd=REPO),
                "cargo": command_text(["cargo", "--version"], cwd=REPO)}
    resources = {"schema_version": 1, "rss_interval_seconds": args.resource_interval_seconds,
                 "measurement_notes": {
                     "rss": "Instantaneous resident working set; allocator caching and OS accounting can change it without a leak. Not a peak measurement.",
                     "fd": "Checkpoint only. Linux /proc/PID/fd; macOS lsof numeric file descriptors excludes Mach ports; Windows null.",
                     "threads": "Checkpoint only. Linux /proc/PID/status; macOS/Windows null (not measured by this driver).",
                     "handles": "Checkpoint only. Windows GetProcessHandleCount; Unix null.",
                     "mach_ports": "Checkpoint event from the native macOS harness: mach_port_name_count counts names in the parent task IPC namespace, not references per name. Null means not collected/unsupported/probe failure; mach_port_probe_error retains the probe error. A stable count is not a no-leak guarantee.",
                     "missing": "Unavailable measurements are null; per-sample errors are recorded when a probe fails.",
                     "overhead": "Monitoring and checkpoint synchronization add scheduling/probe overhead; checkpoint probes occur after the core quiesces."},
                 "events": [], "workers": [], "rss_samples": [], "checkpoints": [], "cleanup": [],
                 "finished_event_seen": False, "ok": False}
    code = 1
    emit("supervisor_started", output_dir=str(output), total_timeout_seconds=timeout)
    try:
        library = os.environ.get("MOJO_RUNTIME_LIBRARY")
        if not library:
            raise RuntimeError("set MOJO_RUNTIME_LIBRARY to the matching trusted native SDK library")
        native = Path(library).expanduser().resolve(strict=True)
        if not native.is_file():
            raise RuntimeError(f"native library is not a file: {native}")
        metadata["native_library"] = {"path": str(native), "sha256": sha256(native), "size": native.stat().st_size}
        metadata["supervised_environment"]["MOJO_RUNTIME_LIBRARY"] = str(native)
        if not args.skip_build:
            build = ["cargo", "build", "--release", "-p", "mojo-ipc-bench", "--locked"]
            metadata["build_command"] = build
            emit("build_started", command=build, log=str(output / "build.log"))
            with (output / "build.log").open("w", encoding="utf-8") as log:
                process = subprocess.Popen(build, cwd=REPO, stdout=log, stderr=subprocess.STDOUT, **popen_options())
                try:
                    build_code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
                    if build_code:
                        raise RuntimeError(f"cargo build exited {build_code}; see build.log")
                finally:
                    if process.poll() is None:
                        resources["cleanup"].extend(terminate_owned(process, {}))
        if not binary.is_file():
            raise RuntimeError(f"benchmark binary does not exist: {binary}; check --binary/CARGO_TARGET_DIR")
        metadata["binary_sha256"] = sha256(binary)
        result_code = run_core(command, output, deadline, resources, native, args.resource_interval_seconds)
        if result_code != 0:
            raise RuntimeError(f"benchmark core exited {result_code}")
        result = json.loads((output / "result.json").read_text(encoding="utf-8"))
        if not isinstance(result, dict) or result.get("ok") is not True:
            raise RuntimeError("result.json does not contain ok=true")
        resources["ok"] = True
        code = 0
    except KeyboardInterrupt:
        resources["error"] = "interrupted by user"
        code = 130
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired, TimeoutError) as error:
        resources["error"] = str(error)
    finally:
        metadata["completed_utc"] = utc()
        metadata["elapsed_seconds"] = time.monotonic() - started
        metadata["ok"] = resources["ok"]
        for filename, value in (("metadata.json", metadata), ("resources.json", resources)):
            (output / filename).write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        if resources["ok"]:
            try:
                report = subprocess.run(
                    [sys.executable, str(Path(__file__).with_name("report.py")), str(output)],
                    capture_output=True, text=True, timeout=30, check=True)
                emit("report_generated", path=str(output / "report.html"))
            except Exception as error:
                detail = error.stderr.strip() if isinstance(error, subprocess.CalledProcessError) and error.stderr else str(error)
                emit("report_failed", error=detail)
        emit("supervisor_finished", ok=resources["ok"], exit_code=code,
             error=resources.get("error"), output_dir=str(output))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
