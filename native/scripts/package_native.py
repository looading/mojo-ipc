#!/usr/bin/env python3
"""Audit and archive an accepted native runtime, including dependency notices."""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tomllib

from build_native import COMMIT, LABEL
from chromium_source import add_revision_argument, revision


ROOT = Path(__file__).resolve().parents[2]
TARGETS = {
    "x86_64-unknown-linux-gnu": ("linux", "x64", "libmojo_rs_native.so"),
    "x86_64-apple-darwin": ("mac", "x64", "libmojo_rs_native.dylib"),
    "aarch64-apple-darwin": ("mac", "arm64", "libmojo_rs_native.dylib"),
    "x86_64-pc-windows-msvc": ("win", "x64", "mojo_rs_native.dll"),
}
GATES = (
    "cargo_check_passed", "cargo_tests_passed", "official_mojom_boundary_tests_passed",
    "native_smoke_passed", "independent_named_process_fixture_passed", "typed_parent_child_example_passed",
    "service_network_passed",
)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def version(sdk_root: Path = ROOT) -> str:
    value = tomllib.loads((sdk_root / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", value):
        raise RuntimeError(f"unsupported workspace version: {value!r}")
    return value


def inspect(command: list[str]) -> str:
    return subprocess.check_output(command, text=True, encoding="utf-8", errors="strict", stderr=subprocess.STDOUT)


def audit_output(target: str, output: str) -> dict:
    """Fail closed on unexpected architecture, public exports, or dependencies."""
    os_name, cpu, library = TARGETS[target]
    if os_name == "linux":
        if not re.search(r"Machine:\s+Advanced Micro Devices X86-64", output) or not re.search(r"Type:\s+DYN", output):
            raise RuntimeError("expected an x86_64 ELF shared library")
        dependencies = re.findall(r"Shared library: \[([^\]]+)\]", output)
        allowed = {"libc.so.6", "libm.so.6", "libdl.so.2", "librt.so.1", "libpthread.so.0",
                   "libgcc_s.so.1", "libstdc++.so.6", "ld-linux-x86-64.so.2"}
        if not dependencies or set(dependencies) - allowed:
            raise RuntimeError(f"unexpected Linux dependencies: {dependencies}")
        for path in re.findall(r"Library (?:runpath|rpath): \[([^\]]*)\]", output):
            if any(part not in ("", "$ORIGIN") for part in path.split(":")):
                raise RuntimeError(f"non-portable runtime search path: {path}")
        exports = set()
        for line in output.splitlines():
            fields = line.split()
            if len(fields) >= 8 and fields[4] in ("GLOBAL", "WEAK") and fields[5] == "DEFAULT" and fields[6] != "UND":
                exports.add(fields[7].split("@")[0])
        exports.discard("MOJO_RS_NATIVE_1")
        requirements = sorted(set(re.findall(r"\b(?:GLIBC|GLIBCXX|CXXABI)_[0-9.]+", output)))
    elif os_name == "mac":
        architecture = "x86_64" if cpu == "x64" else "arm64"
        if output.splitlines()[0].strip() != architecture:
            raise RuntimeError(f"expected a single-architecture {architecture} Mach-O library")
        dependencies = re.findall(r"^\s+(\S+) \(compatibility version", output, re.M)
        # otool -L includes LC_ID_DYLIB before the actual imports. Chromium's
        # noncomponent build uses ./, while component-style builds use @rpath/.
        self_ids = {"@rpath/" + library, "./" + library}
        if not dependencies or dependencies[0] not in self_ids:
            raise RuntimeError(f"unexpected macOS install ID: {dependencies[:1]}")
        dependencies = dependencies[1:]
        if not dependencies or any(not d.startswith(("/usr/lib/", "/System/Library/")) for d in dependencies):
            raise RuntimeError(f"unexpected macOS dependencies: {dependencies}")
        exports = set(re.findall(r"^[0-9a-fA-F]+\s+[A-Z]\s+_(\S+)$", output, re.M))
        requirements = re.findall(r"\b(?:minos|version|sdk)\s+[0-9.]+", output)
    else:
        if "IMAGE_FILE_MACHINE_AMD64" not in output or "IMAGE_FILE_DLL" not in output:
            raise RuntimeError("expected an AMD64 PE DLL")
        exports = set(re.findall(r"\bExport\s*\{[^}]*?\bName:\s+(\S+)", output, re.S))
        dependencies = re.findall(r"\b(?:Import|DelayImport)\s*\{[^}]*?\bName:\s+(\S+)", output, re.S)
        allowed = {name + ".dll" for name in (
            "kernel32", "kernelbase", "ntdll", "advapi32", "user32", "gdi32", "ole32", "oleaut32",
            "rpcrt4", "shell32", "shlwapi", "ws2_32", "bcrypt", "crypt32", "secur32", "psapi",
            "version", "winmm", "powrprof", "setupapi", "dbghelp", "userenv", "combase", "msvcrt",
            "cfgmgr32", "propsys", "iphlpapi", "netapi32", "synchronization",
        )}
        if not dependencies or any(d.lower() not in allowed and not d.lower().startswith(("api-ms-win-", "ext-ms-win-")) for d in dependencies):
            raise RuntimeError(f"unexpected Windows dependencies: {dependencies}")
        exports.discard("GetHandleVerifier")  # Upstream private export, never an SDK ABI.
        requirements = ["Windows x64; system DLLs listed in dependencies"]
    if exports != {"MojoRsGetApiV1"}:
        raise RuntimeError(f"unexpected SDK exports: {sorted(exports)}")
    return {"target": target, "passed": True, "exports": sorted(exports),
            "dependencies": dependencies, "platform_requirements": requirements,
            "scope": "architecture, exported symbols, direct and delay-load imports; not OS certification"}


def audit_library(target: str, library: Path) -> tuple[dict, str]:
    os_name = TARGETS[target][0]
    if os_name == "linux":
        output = inspect(["readelf", "-h", "-d", "--dyn-syms", "--wide", "--version-info", str(library)])
    elif os_name == "mac":
        output = "\n".join(inspect(command + [str(library)]) for command in (
            ["lipo", "-archs"], ["otool", "-L"], ["nm", "-gU"], ["otool", "-l"],
        ))
    else:
        tool = shutil.which("llvm-readobj") or str(Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / "LLVM/bin/llvm-readobj.exe")
        output = inspect([tool, "--file-headers", "--coff-exports", "--coff-imports", str(library)])
    audit = audit_output(target, output)
    if os_name == "mac":
        system = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
        system.malloc_default_zone.restype = ctypes.c_void_p
        before = system.malloc_default_zone()
        loaded = ctypes.CDLL(str(library.resolve()))
        if not loaded or system.malloc_default_zone() != before:
            raise RuntimeError("loading the SDK changed the host's default malloc zone")
        audit["default_malloc_zone_preserved"] = True
    return audit, output


def validate_receipts(target: str, native: dict, acceptance: dict, library: bytes, sdk_commit: str,
                      *, allow_previous_attempts: bool = False, chromium_commit: str = COMMIT) -> None:
    revision(chromium_commit)
    os_name, cpu, filename = TARGETS[target]
    for key, value in {"chromium_commit": chromium_commit, "abi_version": 1, "os": os_name,
                       "cpu": cpu, "library": filename, "native_smoke_passed": True,
                       "sha256": digest(library)}.items():
        if native.get(key) != value:
            raise RuntimeError(f"native manifest mismatch: {key}")
    if acceptance.get("target") != target or acceptance.get("sdk_commit") != sdk_commit:
        raise RuntimeError("acceptance belongs to another target or SDK commit")
    if any(acceptance.get(gate) is not True for gate in GATES):
        raise RuntimeError("all native acceptance gates must pass before packaging")
    if os.environ.get("GITHUB_RUN_ID") and acceptance.get("github_run_id") != os.environ["GITHUB_RUN_ID"]:
        raise RuntimeError("acceptance belongs to another workflow run")
    if os.environ.get("GITHUB_RUN_ATTEMPT"):
        try:
            previous = int(acceptance["github_run_attempt"])
            current = int(os.environ["GITHUB_RUN_ATTEMPT"])
        except (ValueError, TypeError, KeyError):
            raise RuntimeError("invalid acceptance workflow attempt") from None
        if not (1 <= previous <= current) or (not allow_previous_attempts and previous != current):
            raise RuntimeError("acceptance belongs to another workflow attempt")


def create_package(artifact: Path, target: str, chromium: Path, out: Path, destination: Path,
                   *, sdk_root: Path = ROOT, chromium_commit: str = COMMIT) -> Path:
    sdk_root = sdk_root.resolve()
    revision(chromium_commit)
    sdk_commit = os.environ.get("GITHUB_SHA") or inspect(["git", "-C", str(sdk_root), "rev-parse", "HEAD"]).strip()
    if inspect(["git", "-C", str(chromium), "rev-parse", "HEAD"]).strip() != chromium_commit:
        raise RuntimeError("license checkout does not match Chromium pin")
    filename = TARGETS[target][2]
    required = [filename, "mojo_rs_native.h", "native-manifest.json", "acceptance-manifest.json"]
    for name in required:
        path = artifact / name
        if path.is_symlink() or not path.is_file():
            raise RuntimeError(f"missing regular artifact file: {name}")
    native = json.loads((artifact / "native-manifest.json").read_bytes())
    acceptance = json.loads((artifact / "acceptance-manifest.json").read_bytes())
    validate_receipts(target, native, acceptance, (artifact / filename).read_bytes(), sdk_commit, chromium_commit=chromium_commit)
    if (artifact / "mojo_rs_native.h").read_bytes() != (sdk_root / "native/include/mojo_rs_native.h").read_bytes():
        raise RuntimeError("artifact C header does not match this SDK checkout")
    audit, raw = audit_library(target, artifact / filename)
    notices = artifact / "THIRD_PARTY_NOTICES.txt"
    gn_platform = {"linux": ("linux64", "gn"), "mac": ("mac", "gn"), "win": ("win", "gn.exe")}[TARGETS[target][0]]
    license_env = dict(os.environ, LICENSES_GN_PATH=str(ROOT / "native/scripts/license_gn.py"),
                       MOJO_LICENSE_GN=str(chromium / "buildtools" / gn_platform[0] / gn_platform[1]))
    subprocess.run([sys.executable, str(chromium / "tools/licenses/licenses.py"), "license_file",
                    "--format=txt", "--gn-out-dir", str(out), "--gn-target", LABEL,
                    "--target-os", TARGETS[target][0], str(notices)], cwd=chromium, env=license_env, check=True)
    if not notices.is_file() or notices.stat().st_size == 0:
        raise RuntimeError("Chromium dependency license notices are missing")
    native["runtime_dependency_audit"] = "dependency-audit.json"
    acceptance["runtime_dependency_audit"] = "dependency-audit.json"
    payload = {name: (artifact / name).read_bytes() for name in (filename, "mojo_rs_native.h")}
    for name, value in (("native-manifest.json", native), ("acceptance-manifest.json", acceptance),
                        ("dependency-audit.json", audit)):
        payload[name] = (json.dumps(value, indent=2) + "\n").encode()
    payload.update({"dependency-audit.txt": raw.encode(), "LICENSE": (sdk_root / "LICENSE").read_bytes(),
                    "CHROMIUM_LICENSE": (chromium / "LICENSE").read_bytes(),
                    "THIRD_PARTY_NOTICES.txt": notices.read_bytes()})
    metadata = {"sdk_version": version(sdk_root), "sdk_commit": sdk_commit, "target": target,
                "chromium_commit": chromium_commit, "abi_version": 1,
                "files": {name: digest(data) for name, data in payload.items()}}
    payload["package-manifest.json"] = (json.dumps(metadata, indent=2) + "\n").encode()
    stem = f"mojo-native-{version(sdk_root)}-{target}"
    destination.mkdir(parents=True, exist_ok=True)
    archive = destination / (stem + ".tar.gz")
    with tarfile.open(archive, "w:gz") as stream:
        for name, data in sorted(payload.items()):
            entry = tarfile.TarInfo(stem + "/" + name)
            entry.size, entry.mode = len(data), 0o644
            stream.addfile(entry, io.BytesIO(data))
    (destination / (archive.name + ".sha256")).write_text(f"{digest(archive.read_bytes())}  {archive.name}\n", encoding="utf-8")
    return archive


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--chromium-src", type=Path, required=True)
    add_revision_argument(parser)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--sdk-root", type=Path, default=ROOT,
                        help="validated SDK source checkout (defaults to this script's checkout)")
    args = parser.parse_args()
    print(create_package(args.artifact_dir.resolve(), args.target, args.chromium_src.resolve(),
                         args.out.resolve(), args.output_dir.resolve(), sdk_root=args.sdk_root,
                         chromium_commit=args.chromium_revision))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
