#!/usr/bin/env python3
"""Adapt real Microsoft CRT/SDK inputs to the pinned Chromium winsysroot layout.

This creates local symlinks and metadata only. It does not download packages,
modify Chromium, synthesize headers/libraries, or run a GN/native build.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import tempfile


from chromium_source import OFFICIAL_COMMIT as COMMIT, add_revision_argument
MIN_SDK = (10, 0, 28000, 0)
MACHINES = {"x86": 0x014C, "x64": 0x8664, "arm64": 0xAA64}
COMPAT_PATCH = """diff --git a/build/config/win/visual_studio_version.gni b/build/config/win/visual_studio_version.gni
--- a/build/config/win/visual_studio_version.gni
+++ b/build/config/win/visual_studio_version.gni
@@ -40,7 +40,7 @@ if (visual_studio_path == "") {
          "You must set the windows_sdk_version if you set the path")
   assert(wdk_path != "",
          "You must set the wdk_path if you set the visual studio path")
-  visual_studio_runtime_dirs = []
+  visual_studio_runtime_dirs = ""
 }
 
 visual_studio_version_logs = [ "windows_sdk_version=${windows_sdk_version}" ]
"""


def directory(value: str) -> Path:
    path = Path(value).expanduser().resolve()
    if not path.is_dir():
        raise argparse.ArgumentTypeError(f"directory does not exist: {path}")
    return path


def find_file(parent: Path, name: str) -> Path:
    """NuGet uses some .Lib spellings; do not require a case-insensitive host."""
    matches = [item for item in parent.iterdir()
               if item.name.casefold() == name.casefold() and item.is_file()]
    if len(matches) != 1:
        raise RuntimeError(f"expected one {name} in {parent}, found {len(matches)}")
    return matches[0]


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def archive_machine(path: Path, architecture: str) -> None:
    """Check actual COFF/import members, including MS import/bigobj headers."""
    expected = MACHINES[architecture]
    count = 0
    with path.open("rb") as source:
        total_size = os.fstat(source.fileno()).st_size
        if source.read(8) != b"!<arch>\n":
            raise RuntimeError(f"not a COFF archive: {path}")
        while True:
            header = source.read(60)
            if not header:
                break
            if len(header) != 60 or header[58:60] != b"`\n":
                raise RuntimeError(f"invalid archive member header: {path}")
            try:
                size = int(header[48:58])
            except ValueError as error:
                raise RuntimeError(f"invalid archive member size: {path}") from error
            if size < 0:
                raise RuntimeError(f"invalid archive member size: {path}")
            if source.tell() + size > total_size:
                raise RuntimeError(f"truncated archive member: {path}")
            name = header[:16].decode("ascii", errors="replace").strip()
            prefix = source.read(min(size, 20))
            if len(prefix) != min(size, 20):
                raise RuntimeError(f"truncated archive member: {path}")
            machine = None
            if name not in ("/", "//", "/SYM64/") and len(prefix) >= 20:
                if prefix[:4] == b"\x00\x00\xff\xff":
                    machine = int.from_bytes(prefix[6:8], "little")
                else:
                    candidate = int.from_bytes(prefix[:2], "little")
                    if candidate in MACHINES.values():
                        machine = candidate
            if machine is not None:
                if machine != expected:
                    raise RuntimeError(
                        f"{path}: machine 0x{machine:04x} is not {architecture}")
                count += 1
            source.seek(size - len(prefix) + (size % 2), os.SEEK_CUR)
    if not count:
        raise RuntimeError(f"no identifiable {architecture} COFF members: {path}")


def link(path: Path, source: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.symlink_to(source, target_is_directory=source.is_dir())


def write_json(path: Path, value: object) -> None:
    """Replace metadata only; never replace live CRT/SDK inputs."""
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=path.parent,
                                     prefix=".metadata-", delete=False) as temporary:
        temporary.write(json.dumps(value, indent=2) + "\n")
        temporary_path = Path(temporary.name)
    temporary_path.replace(path)


def check_atl(include: Path, libraries: dict[str, Path]) -> dict[str, object]:
    for name in ("atldef.h", "atlbase.h"):
        find_file(include, name)
    result = {}
    for architecture, parent in libraries.items():
        library = find_file(parent, "atls.lib")
        archive_machine(library, architecture)
        result[architecture] = {"path": str(library), "sha256": sha256(library)}
    return result


def append_atl(root: Path, include: Path, libraries: dict[str, Path], *, chromium_commit: str = COMMIT) -> None:
    """Add a delta view, leaving every existing CRT/SDK file identity intact."""
    manifest_path = root / "adapter-manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("chromium_commit") != chromium_commit or manifest.get("schema") != 1:
        raise RuntimeError(f"not a matching SDK adapter root: {root}")
    architectures = manifest["architectures"]
    if set(libraries) != set(architectures):
        raise RuntimeError(f"provide ATL libraries for these root architectures: {architectures}")
    evidence = check_atl(include, libraries)
    vc = Path("VC/Tools/MSVC") / manifest["crt_version"]
    crt = root / vc / "include"
    if not crt.is_dir():
        raise RuntimeError(f"original CRT include directory is missing: {crt}")
    baseline = {path.relative_to(crt).as_posix().casefold()
                for path in crt.rglob("*") if path.is_file()}
    headers = [path for path in include.rglob("*") if path.is_file()
               and path.relative_to(include).as_posix().casefold() not in baseline]
    names = {path.relative_to(include).as_posix().casefold() for path in headers}
    if not {"atldef.h", "atlbase.h"}.issubset(names):
        raise RuntimeError("ATL delta would not add atldef.h and atlbase.h; primary CRT already has ATL")
    destination = root / vc / "atlmfc"
    if destination.exists() or destination.is_symlink():
        raise RuntimeError(f"ATL destination already exists; refusing to replace it: {destination}")
    metadata = {"source_include": str(include), "libraries": evidence,
                "delta_headers": {path.relative_to(include).as_posix(): sha256(path) for path in headers},
                "excluded_original_crt_files": len(baseline)}
    environments = []
    for architecture in architectures:
        path = root / "Windows Kits/10/bin" / f"SetEnv.{architecture}.json"
        data = json.loads(path.read_text(encoding="utf-8"))
        for variable, extra in (("INCLUDE", vc / "atlmfc/include"),
                                ("LIB", vc / "atlmfc/lib" / architecture)):
            components = list(extra.parts)
            if components not in data["env"][variable]:
                data["env"][variable].append(components)
        environments.append((path, data))
    with tempfile.TemporaryDirectory(prefix=".atl-delta-", dir=destination.parent) as temporary:
        staging = Path(temporary) / "atlmfc"
        staging.mkdir()
        for path in headers:
            link(staging / "include" / path.relative_to(include), path)
        for architecture, parent in libraries.items():
            # Linking only ATL additions avoids aliasing a second CRT cache.
            original = root / vc / "lib" / architecture
            original_names = {path.relative_to(original).as_posix().casefold()
                              for path in original.rglob("*") if path.is_file()}
            for path in parent.rglob("*"):
                if path.is_file() and path.relative_to(parent).as_posix().casefold() not in original_names:
                    link(staging / "lib" / architecture / path.relative_to(parent), path)
        (staging / "adapter-atl-manifest.json").write_text(
            json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
        staging.rename(destination)
    for path, data in environments:
        write_json(path, data)
    manifest["atl"] = {"mode": "delta", "path": str(vc / "atlmfc"), **metadata}
    write_json(manifest_path, manifest)
    print(f"Added {len(headers)} genuine ATL headers without replacing CRT/SDK inputs: {destination}")


def atl_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--atl-include-dir", type=directory,
                        help="separate or merged genuine ATL headers; duplicate CRT files are excluded")
    for architecture in MACHINES:
        parser.add_argument(f"--atl-lib-{architecture}", type=directory)


def add_atl_main() -> int:
    parser = argparse.ArgumentParser(description="Append genuine ATL to an existing adapter root without replacing CRT/SDK files.")
    parser.add_argument("--winsysroot", required=True, type=directory)
    add_revision_argument(parser)
    atl_arguments(parser)
    args = parser.parse_args(sys.argv[2:])
    if args.atl_include_dir is None:
        parser.error("--atl-include-dir is required")
    libraries = {architecture: getattr(args, f"atl_lib_{architecture}")
                 for architecture in MACHINES if getattr(args, f"atl_lib_{architecture}") is not None}
    append_atl(args.winsysroot, args.atl_include_dir, libraries, chromium_commit=args.chromium_revision)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     epilog="Use 'add-atl --help' to extend an existing root without replacing CRT/SDK inputs.")
    parser.add_argument("--sdk-include-dir", required=True, type=directory,
                        help="real SDK version directory containing shared/um/ucrt")
    parser.add_argument("--sdk-version", default="10.0.28000.0")
    parser.add_argument("--crt-include-dir", required=True, type=directory)
    for architecture in MACHINES:
        required = architecture in ("x86", "x64")
        parser.add_argument(f"--crt-lib-{architecture}", required=required, type=directory)
        parser.add_argument(f"--sdk-um-lib-{architecture}", required=required, type=directory)
        parser.add_argument(f"--sdk-ucrt-lib-{architecture}", required=required, type=directory)
    parser.add_argument("--clang-cl", required=True, type=Path,
                        help="real host-native Chromium clang-cl executable")
    parser.add_argument("--visual-studio-version", choices=("2022", "2026"), default="2022",
                        help="actual CRT toolset generation, not an emulated VS installation")
    parser.add_argument("--output", required=True, type=Path,
                        help="new local winsysroot directory; must not exist")
    atl_arguments(parser)
    add_revision_argument(parser)
    args = parser.parse_args()
    try:
        version = tuple(int(part) for part in args.sdk_version.split("."))
    except ValueError as error:
        raise RuntimeError("--sdk-version must contain four numeric components") from error
    if len(version) != 4 or version < MIN_SDK:
        raise RuntimeError("this Chromium pin requires Windows SDK 10.0.28000.0 or newer")
    if not re.fullmatch(r"\d+\.\d+\.\d+\.\d+", args.sdk_version):
        raise RuntimeError("--sdk-version must contain four numeric components")
    sdk_header = find_file(args.sdk_include_dir / "shared", "sdkddkver.h")
    if not re.search(r"^\s*#\s*define\s+NTDDI_WIN11_BR\b",
                     sdk_header.read_text(encoding="utf-8-sig", errors="replace"), re.MULTILINE):
        raise RuntimeError(f"real SDK header lacks required NTDDI_WIN11_BR: {sdk_header}")
    find_file(args.sdk_include_dir / "um", "windows.h")
    find_file(args.sdk_include_dir / "ucrt", "stdio.h")
    crt_header = find_file(args.crt_include_dir, "crtversion.h")
    crt_text = crt_header.read_text(encoding="utf-8-sig", errors="replace")
    crt_parts = []
    for component in ("MAJOR", "MINOR", "BUILD"):
        match = re.search(r"^\s*#\s*define\s+_VC_CRT_" + component + r"_VERSION\s+(\d+)\b",
                          crt_text, re.MULTILINE)
        if not match:
            raise RuntimeError(f"missing real CRT {component} version: {crt_header}")
        crt_parts.append(match.group(1))
    crt_version = ".".join(crt_parts)
    find_file(args.crt_include_dir, "vcruntime.h")
    clang_cl = args.clang_cl.expanduser().resolve()
    if not clang_cl.is_file() or not os.access(clang_cl, os.X_OK):
        raise RuntimeError(f"not an executable host compiler: {clang_cl}")
    output = args.output.expanduser().absolute()
    if output.exists() or output.is_symlink():
        raise RuntimeError(f"output already exists; choose a new winsysroot directory: {output}")
    architectures = []
    evidence = {}
    for architecture in MACHINES:
        values = [getattr(args, f"{kind}_{architecture}")
                  for kind in ("crt_lib", "sdk_um_lib", "sdk_ucrt_lib")]
        if not any(values):
            continue
        if not all(values):
            raise RuntimeError(f"provide all three {architecture} library directories")
        architectures.append(architecture)
        for kind, parent, filename in zip(("crt", "um", "ucrt"), values,
                                          ("libcmt.lib", "kernel32.lib", "libucrt.lib")):
            library = find_file(parent, filename)
            archive_machine(library, architecture)
            evidence[f"{kind}_{architecture}"] = {
                "path": str(library), "sha256": sha256(library)}
    atl_libraries = {architecture: getattr(args, f"atl_lib_{architecture}")
                     for architecture in architectures if getattr(args, f"atl_lib_{architecture}") is not None}
    if args.atl_include_dir is not None:
        if set(atl_libraries) != set(architectures):
            raise RuntimeError(f"provide separate ATL libraries for all architectures: {architectures}")
        atl_evidence = check_atl(args.atl_include_dir, atl_libraries)
    else:
        if atl_libraries:
            raise RuntimeError("--atl-lib-* requires --atl-include-dir")
        try:
            atl_evidence = check_atl(args.crt_include_dir, {
                architecture: getattr(args, f"crt_lib_{architecture}") for architecture in architectures})
        except RuntimeError as error:
            raise RuntimeError(
                "Mojo's base dependency requires real ATL headers and atls.lib before GN; "
                "use a primary CRT containing ATL or supply --atl-include-dir and --atl-lib-*; " + str(error)) from error
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".mojo-win-toolchain-", dir=output.parent) as temporary:
        staging = Path(temporary) / "root"
        staging.mkdir()
        sdk = Path("Windows Kits/10")
        vc = Path("VC/Tools/MSVC") / crt_version
        link(staging / vc / "include", args.crt_include_dir)
        link(staging / sdk / "Include" / args.sdk_version, args.sdk_include_dir)
        for architecture in architectures:
            for kind, destination in (("crt_lib", vc / "lib" / architecture),
                                      ("sdk_um_lib", sdk / "Lib" / args.sdk_version / "um" / architecture),
                                      ("sdk_ucrt_lib", sdk / "Lib" / args.sdk_version / "ucrt" / architecture)):
                link(staging / destination, getattr(args, f"{kind}_{architecture}"))
            compiler_dir = vc / "bin/Hostx64" / architecture
            link(staging / compiler_dir / "cl.exe", clang_cl)
            include_paths = [vc / "include"] + [
                sdk / "Include" / args.sdk_version / part
                for part in ("ucrt", "shared", "um", "winrt")
                if (args.sdk_include_dir / part).is_dir()]
            library_paths = [vc / "lib" / architecture,
                             sdk / "Lib" / args.sdk_version / "ucrt" / architecture,
                             sdk / "Lib" / args.sdk_version / "um" / architecture]
            environment = {"VSINSTALLDIR": [["."]], "PATH": [list(compiler_dir.parts)],
                           "INCLUDE": [list(path.parts) for path in include_paths],
                           "LIB": [list(path.parts) for path in library_paths]}
            metadata = staging / sdk / "bin" / f"SetEnv.{architecture}.json"
            metadata.parent.mkdir(parents=True, exist_ok=True)
            metadata.write_text(json.dumps({"env": environment}, indent=2) + "\n", encoding="utf-8")
        gn_args = {"visual_studio_path": str(output),
                   "visual_studio_version": args.visual_studio_version,
                   "windows_sdk_path": str(output / sdk),
                   "windows_sdk_version": args.sdk_version,
                   # This pin requires a nonempty path; the user-mode graph
                   # never uses WDK headers. No WDK installation is claimed.
                   "wdk_path": str(output / sdk)}
        (staging / "gn-args.gn").write_text("\n".join(
            f"{key} = {json.dumps(value)}" for key, value in gn_args.items()) + "\n", encoding="utf-8")
        (staging / "chromium-compat.patch").write_text(COMPAT_PATCH, encoding="utf-8")
        manifest = {"schema": 1, "chromium_commit": args.chromium_revision,
                    "sdk_version": args.sdk_version, "crt_version": crt_version,
                    "visual_studio_version": args.visual_studio_version,
                    "architectures": architectures, "wdk_available": False,
                    "compiler_alias_target": str(clang_cl),
                    "sdk_header": {"path": str(sdk_header), "sha256": sha256(sdk_header)},
                    "crt_header": {"path": str(crt_header), "sha256": sha256(crt_header)},
                    "libraries": evidence,
                    "native_build_executed": False,
                    "notes": ["Inputs are symlinked and must remain available.",
                              "Headers/libraries are genuine inputs; no API is synthesized.",
                              "WDK is absent; only the user-mode Mojo target is supported.",
                              "Chromium compatibility patch is emitted, not applied."]}
        if args.atl_include_dir is None:
            manifest["atl"] = {"mode": "primary_crt", "libraries": atl_evidence}
        (staging / "adapter-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        if args.atl_include_dir is not None:
            append_atl(staging, args.atl_include_dir, atl_libraries)
        staging.rename(output)
    print(f"Prepared real Windows SDK {args.sdk_version} / CRT {crt_version}: {output}")
    print(f"GN arguments: {output / 'gn-args.gn'}")
    print(f"Pinned compatibility patch (not applied): {output / 'chromium-compat.patch'}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(add_atl_main() if sys.argv[1:2] == ["add-atl"] else main())
    except (RuntimeError, OSError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
