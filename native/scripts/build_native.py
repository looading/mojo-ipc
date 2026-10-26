#!/usr/bin/env python3
"""Install/build this SDK's GN overlay in an existing pinned Chromium checkout.

This script does not clone Chromium, update DEPS, install toolchains, or change
the checkout's git revision. Run the normal Chromium checkout/setup first.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys


from chromium_source import OFFICIAL_COMMIT as COMMIT, add_revision_argument
import windows7_profile
LABEL = "//sdk/mojo_rs/native:runtime"
MARKER = ".mojo-rust-sdk-overlay"


def checked(command: list[str], *, cwd: Path) -> str:
    result = subprocess.run(command, cwd=cwd, text=True, capture_output=True)
    if result.returncode:
        sys.stderr.write(result.stdout)
        sys.stderr.write(result.stderr)
        raise RuntimeError(f"command failed ({result.returncode}): {command!r}")
    return result.stdout.strip()


def tool(name: str) -> str:
    found = shutil.which(name)
    if not found:
        raise RuntimeError(f"{name!r} is not on PATH; use Chromium's depot_tools")
    return found


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--chromium-src", type=Path, required=True)
    add_revision_argument(parser)
    parser.add_argument("--out", type=Path, help="GN output, relative to checkout")
    parser.add_argument("--build", action="store_true")
    parser.add_argument("--smoke", action="store_true", help="build and run native smoke")
    parser.add_argument("--gn-args", default="", help="additional explicit GN arguments")
    parser.add_argument("--gn-args-file", type=Path,
                        help="additional GN arguments from a saved file; --gn-args follows it")
    parser.add_argument("--target-os", choices=("linux", "mac", "win"),
                        help="target OS (default: host OS; Linux/macOS can build Windows artifacts)")
    parser.add_argument("--target-cpu", choices=("x64", "x86", "arm64"),
                        help="target CPU (default: host CPU, or x64 for cross-OS Windows builds)")
    parser.add_argument("--artifact-dir", type=Path)
    parser.add_argument("--jobs", type=int, help="maximum local Ninja parallelism")
    parser.add_argument("--windows7", action="store_true", help="explicit Windows 7 SP1 x64/x86 API profile (requires separate OS acceptance)")
    args = parser.parse_args()
    if args.jobs is not None and args.jobs < 1:
        parser.error("--jobs must be positive")
    extra_gn_args = ((args.gn_args_file.expanduser().read_text(encoding="utf-8") + "\n")
                     if args.gn_args_file else "") + args.gn_args

    if re.search(r"\bmojo_rs_chromium_commit\s*=", extra_gn_args):
        raise RuntimeError("use --chromium-revision instead of overriding it in --gn-args")

    system = platform.system()
    host_os = {"Darwin": "mac", "Linux": "linux", "Windows": "win"}.get(system)
    os_name = args.target_os or host_os
    cross_os = host_os != os_name
    cpu = args.target_cpu or ("x64" if cross_os and os_name == "win" else {
        "arm64": "arm64", "aarch64": "arm64", "AMD64": "x64", "x86_64": "x64"
    }.get(platform.machine()))
    if cpu == "x86" and (os_name != "win" or not args.windows7):
        raise RuntimeError("x86 requires explicit Windows7 profile")
    if args.windows7 and (os_name != "win" or cpu not in ("x64", "x86")):
        raise RuntimeError("Win7 profile requires Windows x64/x86")
    if re.search(r"\bmojo_rs_windows7_compat\s*=", extra_gn_args):
        raise RuntimeError("use --windows7 instead of overriding the native profile in --gn-args")
    # Validate explicit target requests before touching the checkout/overlay.
    # An install-only invocation without a target remains platform independent.
    if args.build or args.smoke or args.target_os:
        if not cpu or not host_os:
            raise RuntimeError(f"unsupported build host: {system}/{platform.machine()}")
        if cross_os and not (host_os in ("linux", "mac") and os_name == "win"):
            raise RuntimeError(
                f"unsupported cross-OS build: {host_os} -> {os_name}; "
                "only Linux/macOS -> Windows build-only is supported")
        if cross_os and args.smoke:
            raise RuntimeError(
                "--smoke cannot run a Windows target on a Linux/macOS host; "
                "use --build and run native acceptance on a Windows runner")
        if os_name != "mac" and cpu != "x64" and not (os_name == "win" and args.windows7 and cpu == "x86"):
            raise RuntimeError("release v1 targets Linux/Windows x64, explicit Win7 x86, and macOS x64/arm64")
        if re.search(r"\b(target_cpu|target_os)\s*=", extra_gn_args):
            raise RuntimeError("use --target-os and --target-cpu instead of overriding them in --gn-args")
    if args.smoke and host_os == "mac":
        # platform.machine() also reports x86_64 under Rosetta. Ask the OS to
        # execute a tiny system binary in the requested architecture instead
        # of guessing physical hardware or whether Rosetta is installed.
        architecture = {"x64": "x86_64", "arm64": "arm64"}[cpu]
        probe = subprocess.run(["/usr/bin/arch", "-" + architecture,
                                "/usr/bin/true"], capture_output=True,
                               text=True, timeout=10)
        if probe.returncode != 0:
            raise RuntimeError(
                f"this Mac cannot execute the {architecture} smoke target: "
                f"{probe.stderr.strip()}; use --build for a build-only cross-architecture artifact")

    checkout = args.chromium_src.expanduser().resolve()
    source = Path(__file__).resolve().parents[1]
    if not (checkout / ".gn").is_file():
        raise RuntimeError(f"not a Chromium src checkout: {checkout}")
    actual = checked(["git", "rev-parse", "HEAD"], cwd=checkout)
    if actual != args.chromium_revision:
        raise RuntimeError(f"Chromium revision mismatch: wanted {args.chromium_revision}, found {actual}")
    overlay = checkout / "sdk/mojo_rs/native"
    if overlay.exists() and not (overlay / MARKER).is_file():
        raise RuntimeError(f"refusing to overwrite an unrelated directory: {overlay}")
    overlay.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(source, overlay, dirs_exist_ok=True,
                    ignore=shutil.ignore_patterns("__pycache__", "*.pyc", ".DS_Store",
                                                 "work", ".research"))
    # 安装后手动执行 GN 时，默认版本也必须与当前 checkout 相同。
    gn_file = overlay / "BUILD.gn"
    gn_file.write_text(gn_file.read_text(encoding="utf-8").replace(COMMIT, args.chromium_revision), encoding="utf-8")
    (overlay / "windows7_profile_config.h").write_text(
        "// Generated native API profile.\n#define MOJO_RS_WINDOWS7_ENTROPY_PROFILE " + ("1" if args.windows7 else "0") + "\n", encoding="utf-8")
    if args.windows7:
        profile_gni = overlay / "windows7_compat.gni"
        profile_gni.write_text(profile_gni.read_text().replace("mojo_rs_windows7_compat = false", "mojo_rs_windows7_compat = true"))
    source_patches = windows7_profile.apply(checkout, allow_mac_x86=host_os == "mac" and cpu == "x86") if args.windows7 else []
    (overlay / MARKER).write_text(args.chromium_revision + "\n", encoding="utf-8")
    print(f"Installed overlay: {overlay}")
    if not args.build and not args.smoke:
        return 0

    out = args.out or Path(f"out/mojo-rs-{os_name}-{cpu}")
    out = out if out.is_absolute() else checkout / out
    # These are a starting configuration, not a claim that all dependencies
    # have been built on the current machine. Target-specific overrides and
    # the actual final link/dependency closure must be validated in CI.
    target_os_arg = f'target_os="{os_name}" ' if cross_os else ""
    # 新 Chromium 已移除此 GN 参数；默认官方 pin 保留原配置。
    token_arg = "use_alloc_token=false " if args.chromium_revision == COMMIT else ""
    gn_args = (f'is_debug=false is_component_build=false symbol_level=1 '
               f'use_allocator_shim=false use_partition_alloc_as_malloc=false '
               f'enable_backup_ref_ptr_support=false ' + token_arg +
               f'mojo_rs_chromium_commit="{args.chromium_revision}" '
               f'target_cpu="{cpu}" ' + target_os_arg + ('mojo_rs_windows7_compat=true ' if args.windows7 else '') + extra_gn_args).strip()
    gn = tool("gn")
    ninja = shutil.which("autoninja") or tool("ninja")
    # This directory is an overlay, so Chromium's normal //:default does not
    # depend on it. Select our target as GN's graph root, otherwise GN never
    # loads this BUILD.gn and Ninja cannot resolve the requested target.
    root_target = LABEL + "_smoke" if args.smoke else LABEL
    subprocess.run([gn, "gen", str(out), "--args=" + gn_args,
                    "--root-target=" + root_target,
                    "--root-pattern=" + root_target,
                    "--fail-on-unused-args"], cwd=checkout, check=True)
    parallelism = ["-j", str(args.jobs)] if args.jobs is not None else []
    subprocess.run([ninja, "-C", str(out), *parallelism, root_target.removeprefix("//")],
                   cwd=checkout, check=True)
    smoke_passed = False
    if args.smoke:
        executable = out / ("runtime_smoke.exe" if os_name == "win" else "runtime_smoke")
        subprocess.run([str(executable)], cwd=out, check=True, timeout=30)
        smoke_passed = True

    filename = {"linux": "libmojo_rs_native.so", "mac": "libmojo_rs_native.dylib",
                "win": "mojo_rs_native.dll"}[os_name]
    library = out / filename
    if not library.is_file():
        raise RuntimeError(f"GN did not produce the expected library: {library}")
    if args.artifact_dir:
        dest = args.artifact_dir.expanduser().resolve()
        dest.mkdir(parents=True, exist_ok=True)
        shutil.copy2(library, dest / filename)
        shutil.copy2(source / "include/mojo_rs_native.h", dest / "mojo_rs_native.h")
        manifest = {
            "chromium_commit": args.chromium_revision,
            "abi_version": 1,
            "os": os_name,
            "cpu": cpu,
            "gn_args": gn_args,
            "gn_root_target": root_target,
            "library": filename,
            "sha256": hashlib.sha256(library.read_bytes()).hexdigest(),
            "native_smoke_passed": smoke_passed,
            "runtime_dependency_audit": "required before redistribution",
        }
        if args.windows7:
            manifest.update(windows_api_profile="win7-sp1", windows_entropy_backend="bcrypt", chromium_source_patches=source_patches, minimum_windows_build=7601,
                            win7_runtime_verified=False)
        (dest / "native-manifest.json").write_text(
            json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        print(f"Native artifact: {dest / filename}")
    else:
        print(f"Native artifact: {library}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
