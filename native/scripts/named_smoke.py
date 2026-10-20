#!/usr/bin/env python3
"""Run the native independent-process IPC smoke and preserve its logs.

Launches the server and client separately as sibling processes. This tests the
named platform channel, invitation, message-pipe transfer and shared mapping;
it does not use the SDK's parent/child spawn bootstrap.
"""

from __future__ import annotations

import argparse
from contextlib import ExitStack
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import time
import uuid


READY = "native named server ready"
PASSED = "native named smoke passed"


def log_text(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def stop(process: subprocess.Popen | None) -> None:
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, required=True,
                        help="built native runtime_smoke executable")
    parser.add_argument("--work-dir", type=Path, required=True,
                        help="parent for a unique run directory; logs are retained")
    parser.add_argument("--timeout", type=float, default=30,
                        help="total readiness/exchange deadline in seconds (default: 30)")
    args = parser.parse_args()
    if not 0 < args.timeout < float("inf"):
        raise RuntimeError("--timeout must be finite and positive")
    executable = args.executable.expanduser().resolve()
    if not executable.is_file() or not os.access(executable, os.X_OK):
        raise RuntimeError(f"not an executable native smoke binary: {executable}")
    system = platform.system()
    if system not in ("Linux", "Darwin", "Windows"):
        raise RuntimeError(f"unsupported native smoke platform: {system}")
    run_id = uuid.uuid4().hex
    work = args.work_dir.expanduser().resolve() / run_id
    work.mkdir(parents=True, exist_ok=False)
    print(f"Native named smoke logs: {work}", flush=True)
    server_stdout = work / "server.stdout.log"
    server_stderr = work / "server.stderr.log"
    client_stdout = work / "client.stdout.log"
    client_stderr = work / "client.stderr.log"
    pid_file = work / "client.pid"
    metadata = {"platform": system, "executable": str(executable),
                "timeout_seconds": args.timeout, "passed": False}
    server = None
    client = None
    try:
        with ExitStack() as resources:
            if system == "Linux":
                # sockaddr_un paths are short. Keep the socket out of a possibly
                # long CI work path, in a private directory removed on exit.
                socket_dir = resources.enter_context(
                    tempfile.TemporaryDirectory(prefix="mojo-rs-", dir="/tmp"))
                name = str(Path(socket_dir) / "ipc")
            elif system == "Darwin":
                name = "com.example.mojo-rust-sdk.smoke." + run_id
            else:
                # NamedPlatformChannel adds the Windows named-pipe prefix.
                name = "mojo-rust-sdk-smoke-" + run_id
            metadata["channel_name"] = name
            logs = [resources.enter_context(path.open("wb")) for path in
                    (server_stdout, server_stderr, client_stdout, client_stderr)]
            deadline = time.monotonic() + args.timeout
            server = subprocess.Popen(
                [str(executable), "--named-server", name,
                 "--peer-pid-file", str(pid_file)], cwd=executable.parent,
                stdin=subprocess.DEVNULL, stdout=logs[0], stderr=logs[1])
            metadata["server_pid"] = server.pid
            # Stop processes before closing logs/removing the socket directory.
            resources.callback(stop, server)
            while READY not in log_text(server_stdout):
                code = server.poll()
                if code is not None:
                    raise RuntimeError(f"server exited {code} before readiness")
                if time.monotonic() >= deadline:
                    raise RuntimeError("deadline waiting for named server readiness")
                time.sleep(0.02)
            client = subprocess.Popen(
                [str(executable), "--named-client", name], cwd=executable.parent,
                stdin=subprocess.DEVNULL, stdout=logs[2], stderr=logs[3])
            metadata["client_pid"] = client.pid
            resources.callback(stop, client)
            temporary_pid = work / "client.pid.tmp"
            temporary_pid.write_text(str(client.pid) + "\n", encoding="ascii")
            temporary_pid.replace(pid_file)
            while True:
                server_code = server.poll()
                client_code = client.poll()
                if server_code is not None and server_code != 0:
                    raise RuntimeError(f"server exited {server_code}")
                if client_code is not None and client_code != 0:
                    raise RuntimeError(f"client exited {client_code}")
                if server_code is not None and client_code is not None:
                    break
                if time.monotonic() >= deadline:
                    raise RuntimeError("deadline during pipe/shared-buffer exchange")
                time.sleep(0.02)
            if PASSED not in log_text(server_stdout):
                raise RuntimeError("server did not confirm pipe/shared-buffer exchange")
            metadata["passed"] = True
        print(f"Native independent-process pipe/shared-buffer smoke passed ({system})")
        return 0
    except BaseException as error:
        metadata["error"] = str(error)
        raise
    finally:
        # ExitStack normally performs these before closing the log streams.
        # These calls also handle failure while setting up that stack.
        try:
            stop(client)
        finally:
            stop(server)
            metadata["server_exit_code"] = server.poll() if server else None
            metadata["client_exit_code"] = client.poll() if client else None
            (work / "result.json").write_text(
                json.dumps(metadata, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
