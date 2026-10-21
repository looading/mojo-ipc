#!/usr/bin/env python3
"""Provision a pinned Chromium producer on a disposable GitHub-hosted runner."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys

from build_native import COMMIT


DEPOT_COMMIT = "3a8af1e64be01a524f7d1c46daf8c2dcac63ff85"  # Chromium DEPS
CHROMIUM_URL = "https://chromium.googlesource.com/chromium/src.git"
DEPOT_URL = "https://chromium.googlesource.com/chromium/tools/depot_tools.git"


def run(command: list[str], *, cwd: Path | None = None) -> None:
    print("+", subprocess.list2cmdline([str(x) for x in command]), flush=True)
    subprocess.run(command, cwd=cwd, check=True)


def require_hosted(root: Path) -> None:
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
        raise RuntimeError("provisioning/cleanup is restricted to disposable GitHub-hosted runners")
    temporary = Path(os.environ["RUNNER_TEMP"]).resolve()
    workspace = Path(os.environ["GITHUB_WORKSPACE"]).resolve()
    if root == temporary or temporary not in root.parents:
        raise RuntimeError("producer root must be a child of RUNNER_TEMP")
    if root == workspace or workspace in root.parents or root in workspace.parents:
        raise RuntimeError("producer root and SDK workspace must be disjoint")
    if root.exists():
        raise RuntimeError("producer root must be new; refusing to reuse or overwrite a checkout")


def cleanup_hosted() -> None:
    """Remove only unused, known preinstalls on the ephemeral producer VM."""
    system = platform.system()
    if system == "Linux":
        paths = ["/usr/local/lib/android", "/usr/share/dotnet", "/opt/ghc", "/usr/share/swift"]
    elif system == "Darwin":
        developer = Path(subprocess.check_output(["xcode-select", "-p"], text=True).strip()).resolve()
        paths = [str(p) for p in Path("/Applications").glob("Xcode*.app")
                 if not p.is_symlink() and p.resolve() not in developer.parents]
        paths.append(str(Path.home() / "Library/Android"))
    else:
        paths = [r"C:\Android", r"C:\ghcup", r"C:\hostedtoolcache\windows\CodeQL"]
    for value in paths:
        path = Path(value)
        if not path.exists():
            continue
        print(f"Reclaiming disposable runner space: {path}", flush=True)
        if system in ("Linux", "Darwin"):
            run(["sudo", "rm", "-rf", "--", str(path)])
        else:
            shutil.rmtree(path)


def gclient_config() -> str:
    solution = {
        "name": "src", "url": CHROMIUM_URL, "managed": False,
        "deps_file": "DEPS", "custom_deps": {},
        "custom_vars": {
            "checkout_configuration": "small",
            "checkout_js_coverage_modules": False,
            "checkout_openxr": False,
            "checkout_pgo_profiles": False,
            "checkout_telemetry_dependencies": False,
            "download_remoteexec_cfg": False,
            "download_reclient": False,
        },
    }
    return "solutions = " + repr([solution]) + "\n"


def checkout(url: str, revision: str, destination: Path) -> None:
    run(["git", "init", str(destination)])
    run(["git", "config", "core.longpaths", "true"], cwd=destination)
    run(["git", "remote", "add", "origin", url], cwd=destination)
    run(["git", "fetch", "--depth=1", "origin", revision], cwd=destination)
    run(["git", "checkout", "--detach", "FETCH_HEAD"], cwd=destination)
    actual = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=destination, text=True).strip()
    if actual != revision:
        raise RuntimeError(f"checkout revision mismatch: {actual} != {revision}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True, type=Path)
    args = parser.parse_args()
    root = args.root.resolve()
    require_hosted(root)
    expected = os.environ["NATIVE_EXPECTED_SYSTEM"]
    cpu = {"x86_64": "x64", "amd64": "x64", "arm64": "arm64", "aarch64": "arm64"}.get(platform.machine().lower())
    if platform.system() != expected or cpu != os.environ["NATIVE_CPU"]:
        raise RuntimeError(f"runner OS/CPU does not match target: {platform.system()}/{platform.machine()}")
    cleanup_hosted()
    root.mkdir(parents=True)
    free_gib = shutil.disk_usage(root).free / 2**30
    print(f"Free producer disk space after cleanup: {free_gib:.1f} GiB", flush=True)
    if free_gib < 25:
        raise RuntimeError("need at least 25 GiB free for a shallow Chromium producer checkout")
    depot = root / "depot_tools"
    source = root / "src"
    os.environ.update({
        "DEPOT_TOOLS_UPDATE": "0", "DEPOT_TOOLS_METRICS": "0",
        "DEPOT_TOOLS_WIN_TOOLCHAIN": "0", "GIT_TERMINAL_PROMPT": "0",
        "PATH": str(depot) + os.pathsep + os.environ["PATH"],
    })
    if platform.system() == "Windows":
        # gclient creates many dependency repositories; each needs long-path
        # support as well as the two repositories initialized below.
        run(["git", "config", "--global", "core.longpaths", "true"])
    checkout(DEPOT_URL, DEPOT_COMMIT, depot)
    checkout(CHROMIUM_URL, COMMIT, source)
    (root / ".gclient").write_text(gclient_config(), encoding="utf-8")
    gclient = ([os.environ.get("COMSPEC", "cmd.exe"), "/d", "/c", str(depot / "gclient.bat")]
               if platform.system() == "Windows" else [str(depot / "gclient")])
    run(gclient + ["sync", "--no-history", "--shallow", "--nohooks",
                   "--jobs=2", "--revision", f"src@{COMMIT}"], cwd=root)
    if platform.system() == "Linux":
        run(["sudo", sys.executable, str(source / "build/install-build-deps.py"),
             "--no-prompt", "--no-arm", "--no-chromeos-fonts", "--no-nacl"], cwd=source)
    run(gclient + ["runhooks"], cwd=root)
    actual = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True).strip()
    if actual != COMMIT:
        raise RuntimeError(f"gclient changed Chromium revision: {actual}")
    artifact = Path(os.environ["GITHUB_WORKSPACE"]) / "dist" / os.environ["NATIVE_TARGET"]
    build = source / "out/mojo-rs-native"
    values = {
        "CHROMIUM_SRC": str(source), "MOJOM_PYTHON": sys.executable,
        "MOJO_NATIVE_BUILD_DIR": str(build), "MOJO_NATIVE_ARTIFACT_DIR": str(artifact),
        "MOJO_RUNTIME_LIBRARY": str(artifact / os.environ["NATIVE_LIBRARY_NAME"]),
        "DEPOT_TOOLS_UPDATE": "0", "DEPOT_TOOLS_WIN_TOOLCHAIN": "0",
    }
    with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as stream:
        for key, value in values.items():
            if "\n" in value or "\r" in value:
                raise RuntimeError(f"unsupported newline in {key}")
            stream.write(f"{key}={value}\n")
    with open(os.environ["GITHUB_PATH"], "a", encoding="utf-8") as stream:
        stream.write(str(depot) + "\n")
        # Keep Actions' Python 3.12 ahead of depot_tools' Python shims.
        stream.write(str(Path(sys.executable).parent) + "\n")
    print(f"Pinned Chromium producer ready: {source}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
