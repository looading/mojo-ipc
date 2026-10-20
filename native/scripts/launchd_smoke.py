#!/usr/bin/env python3
"""Generate, optionally run, and unregister a macOS independent-process smoke.

Without --run this only writes a plist/paths and prints the generated commands.
With --run the temporary LaunchAgent is always removed in a finally block. It
does not install files in ~/Library/LaunchAgents or request administrator access.
"""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import platform
import plistlib
import shlex
import subprocess
import sys
import time
import uuid


def fixture(executable: Path, work: Path, label: str) -> dict:
    return {
        "Label": label,
        "ProgramArguments": [str(executable), "--named-server", label,
                             "--peer-pid-file", str(work / "client.pid")],
        "MachServices": {label: True},
        "WorkingDirectory": str(executable.parent),
        "KeepAlive": False,
        "RunAtLoad": False,
        "StandardOutPath": str(work / "server.stdout.log"),
        "StandardErrorPath": str(work / "server.stderr.log"),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--run", action="store_true")
    args = parser.parse_args()
    if platform.system() != "Darwin":
        raise RuntimeError("launchd fixture generation and smoke run only on macOS")
    executable = args.executable.expanduser().resolve()
    base = args.work_dir.expanduser().resolve()
    # Separate each invocation, including stale pid files, registrations/logs.
    label = "com.example.mojo-rust-sdk.smoke." + uuid.uuid4().hex
    work = base / label.rsplit(".", 1)[-1]
    work.mkdir(parents=True, exist_ok=False)
    plist = work / (label + ".plist")
    plist.write_bytes(plistlib.dumps(fixture(executable, work, label)))
    domain = f"gui/{os.getuid()}"
    register = ["launchctl", "bootstrap", domain, str(plist)]
    unregister = ["launchctl", "bootout", domain + "/" + label]
    client_command = [str(executable), "--named-client", label]
    print(f"Fixture: {plist}")
    print(f"Server logs: {work}")
    if not args.run:
        print(shlex.join(register))
        print("Launch the named client, record its PID in client.pid, then:")
        print(shlex.join(unregister))
        return 0
    if not executable.is_file() or not os.access(executable, os.X_OK):
        raise RuntimeError(f"not an executable native smoke binary: {executable}")
    # A GUI login bootstrap domain is part of this fixture's explicit contract.
    subprocess.run(["launchctl", "print", domain], check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    registered = False
    client = None
    try:
        subprocess.run(register, check=True)
        registered = True
        client = subprocess.Popen(client_command, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, text=True)
        temporary_pid = work / "client.pid.tmp"
        temporary_pid.write_text(str(client.pid) + "\n", encoding="ascii")
        temporary_pid.replace(work / "client.pid")
        stdout, stderr = client.communicate(timeout=25)
        if client.returncode != 0:
            raise RuntimeError(f"client exited {client.returncode}:\n{stdout}\n{stderr}")
        log = work / "server.stdout.log"
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if log.is_file() and "native named smoke passed" in log.read_text(errors="replace"):
                print("macOS launchd independent-process native smoke passed")
                return 0
            time.sleep(0.05)
        raise RuntimeError("server did not report successful pipe/shared-buffer exchange")
    finally:
        if client is not None and client.poll() is None:
            client.kill()
            client.wait()
        if registered:
            # This removes only our unique fixture label and stops its server.
            result = subprocess.run(unregister, text=True, capture_output=True)
            if result.returncode != 0:
                raise RuntimeError("fixture cleanup failed: " + result.stderr +
                                   "\nRun: " + shlex.join(unregister))


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
