#!/usr/bin/env python3
"""Download and verify the Windows SDK headers and x64/x86 libraries from NuGet.

Usage: python3 native/scripts/fetch_windows_sdk.py --output /absolute/cache/path

The default version matches the pinned Chromium checkout. Only Microsoft's SDK
CPP, CPP.x64 and CPP.x86 packages are fetched; MSVC headers/libraries are separate.
Archives are cached in OUTPUT/nuget, extracted to OUTPUT/sdk-nuget/PACKAGE, and
recorded in OUTPUT/sdk-download-manifest.json. NuGet's registration/catalog API
provides SHA512 hashes (the flat-container .nupkg.sha512 URL is not available).
Existing extractions are compared byte for byte and reused without changing any
files or inodes. A different extraction requires a new --output directory.
--verify-only checks existing archives/extractions without writing any output.

The architecture packages use c/{um,ucrt}/ARCH rather than a standard installed
SDK Lib/VERSION/{um,ucrt}/ARCH directory. This script preserves the package layout.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import sys
import tempfile
import urllib.parse
import urllib.request
import zipfile


DEFAULT_VERSION = "10.0.28000.2270"
PACKAGES = (
    "microsoft.windows.sdk.cpp",
    "microsoft.windows.sdk.cpp.x64",
    "microsoft.windows.sdk.cpp.x86",
)
# Catalog SHA512 values, verified against all three downloaded 28000.2270 files.
# Other versions still require the official registration -> catalog SHA512 chain.
PINNED_SHA512 = {
    "microsoft.windows.sdk.cpp":
        "dQjr0W5oTPTsOCS8YCYQivFJG3Ypc+gRcnAfrTa6jIwK3nDFSLwNy0m9TUqWuzUMdQZQdD5RXHCJ9fcUxaYi8A==",
    "microsoft.windows.sdk.cpp.x64":
        "WhB8d1r7+9OX8tXCKkwXubz4539F0cqxWPyYXILgqQnjAxCmZVaAaxE/9Ka44ELsziRi5BaSbx4KLxyZmOe0zA==",
    "microsoft.windows.sdk.cpp.x86":
        "QBQdjfL+NxQI4K6smE+VenNjUo7uCv8077oKVCCrNTFrWGJwa6NHdjUu5wu7JcecPJVjs2KNefYgdrnK66cTwQ==",
}
EXTRACTION_MARKER = ".mojo-windows-sdk-package.json"


def digest_file(path: Path, algorithm: str) -> bytes:
    digest = hashlib.new(algorithm)
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.digest()


def get_json(url: str) -> dict:
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme != "https" or parsed.hostname != "api.nuget.org":
        raise RuntimeError(f"unexpected NuGet metadata URL: {url}")
    request = urllib.request.Request(url, headers={"Accept-Encoding": "gzip"})
    with urllib.request.urlopen(request, timeout=60) as response:
        data = response.read()
    # NuGet catalog responses may be gzip encoded independently of their suffix.
    if data.startswith(b"\x1f\x8b"):
        data = gzip.decompress(data)
    return json.loads(data)


def catalog_metadata(package: str, version: str) -> dict:
    registration_url = (
        f"https://api.nuget.org/v3/registration5-gz-semver2/{package}/{version}.json"
    )
    registration = get_json(registration_url)
    catalog_url = registration.get("catalogEntry")
    if not isinstance(catalog_url, str):
        raise RuntimeError(f"missing catalog URL for {package} {version}")
    catalog = get_json(catalog_url)
    if (str(catalog.get("id", "")).lower() != package
            or str(catalog.get("version", "")).lower() != version.lower()):
        raise RuntimeError(f"NuGet catalog identity mismatch: {package} {version}")
    if catalog.get("packageHashAlgorithm") != "SHA512":
        raise RuntimeError(f"NuGet catalog does not provide SHA512: {package}")
    encoded_hash = catalog.get("packageHash", "")
    if not isinstance(encoded_hash, str):
        raise RuntimeError(f"invalid NuGet SHA512 encoding for {package}")
    try:
        expected = base64.b64decode(encoded_hash, validate=True)
    except (ValueError, binascii.Error) as error:
        raise RuntimeError(f"invalid NuGet SHA512 for {package}") from error
    if len(expected) != hashlib.sha512().digest_size:
        raise RuntimeError(f"invalid NuGet SHA512 length for {package}")
    pin = PINNED_SHA512.get(package) if version == DEFAULT_VERSION else None
    if pin is not None and encoded_hash != pin:
        raise RuntimeError(f"NuGet catalog SHA512 differs from the source pin: {package}")
    return {
        "registration": registration_url,
        "catalog": catalog_url,
        "sha512": encoded_hash,
        "expected": expected,
        "expected_size": catalog.get("packageSize"),
    }


def fetch_archive(package: str, version: str, cache: Path, metadata: dict,
                  *, verify_only: bool = False) -> tuple:
    filename = f"{package}.{version}.nupkg"
    url = f"https://api.nuget.org/v3-flatcontainer/{package}/{version}/{filename}"
    destination = cache / filename
    if destination.is_symlink():
        raise RuntimeError(f"refusing a symlink as an archive cache entry: {destination}")
    if destination.is_file():
        if digest_file(destination, "sha512") == metadata["expected"]:
            print(f"Verified cached {filename}", flush=True)
            return destination, url
        if verify_only:
            raise RuntimeError(f"cached SHA512 mismatch in --verify-only mode: {destination}")
        print(f"Cached SHA512 mismatch; downloading {filename} again", flush=True)
    elif destination.exists():
        raise RuntimeError(f"archive cache entry is not a file: {destination}")
    if verify_only:
        raise RuntimeError(f"missing cached archive in --verify-only mode: {destination}")

    handle, temporary_name = tempfile.mkstemp(prefix=filename + ".", suffix=".partial", dir=cache)
    temporary = Path(temporary_name)
    try:
        print(f"Downloading {filename}", flush=True)
        with os.fdopen(handle, "wb") as stream:
            with urllib.request.urlopen(url, timeout=120) as response:
                shutil.copyfileobj(response, stream, length=1024 * 1024)
        if digest_file(temporary, "sha512") != metadata["expected"]:
            raise RuntimeError(f"downloaded SHA512 mismatch: {filename}")
        if (isinstance(metadata["expected_size"], int)
                and temporary.stat().st_size != metadata["expected_size"]):
            raise RuntimeError(f"downloaded size differs from NuGet catalog: {filename}")
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)
    return destination, url


def safe_members(archive: zipfile.ZipFile) -> list:
    """Reject traversal, links and names with ambiguous Windows semantics first."""
    members = []
    seen = set()
    for item in archive.infolist():
        name = item.filename
        raw_parts = name.rstrip("/").split("/")
        if (not name or "\\" in name or any(char in name for char in ':<>"|?*\x00')
                or name != item.orig_filename or name.startswith("/")
                or any(part in ("", ".", "..") or part.endswith((".", " "))
                       for part in raw_parts)):
            raise RuntimeError(f"unsafe ZIP path: {name!r}")
        devices = {"con", "prn", "aux", "nul", "conin$", "conout$"}
        devices.update(f"{prefix}{number}" for prefix in ("com", "lpt") for number in range(1, 10))
        if any(part.split(".", 1)[0].casefold() in devices for part in raw_parts):
            raise RuntimeError(f"Windows device ZIP path: {name!r}")
        # Packages are intended for case-insensitive Windows file systems too.
        normalized = "/".join(raw_parts).casefold()
        if normalized in seen:
            raise RuntimeError(f"duplicate ZIP path: {name!r}")
        seen.add(normalized)
        mode = item.external_attr >> 16
        if stat.S_IFMT(mode) not in (0, stat.S_IFREG, stat.S_IFDIR):
            raise RuntimeError(f"non-regular ZIP entry: {name!r}")
        if item.flag_bits & 1:
            raise RuntimeError(f"encrypted ZIP entry: {name!r}")
        members.append((item, PurePosixPath(*raw_parts)))
    return members


def verify_extraction(archive: zipfile.ZipFile, members: list, destination: Path,
                      package: str, metadata: dict) -> None:
    """Read existing build inputs in place; never repair or replace their paths."""
    def different(reason: str) -> None:
        raise RuntimeError(
            f"existing SDK extraction differs ({reason}): {destination}; "
            "choose a new --output directory; existing files were not changed")

    if destination.is_symlink() or not destination.is_dir():
        different("not a regular directory")
    expected_files, expected_dirs = set(), set()
    for item, relative in members:
        (expected_dirs if item.is_dir() else expected_files).add(relative.as_posix())
        expected_dirs.update(parent.as_posix() for parent in relative.parents if parent != PurePosixPath("."))
    actual_files, actual_dirs = set(), set()
    for root, directories, files in os.walk(destination, followlinks=False):
        root = Path(root)
        for name in directories + files:
            path = root / name
            mode = path.lstat().st_mode
            relative = path.relative_to(destination).as_posix()
            if stat.S_ISDIR(mode):
                actual_dirs.add(relative)
            elif stat.S_ISREG(mode):
                actual_files.add(relative)
            else:
                different(f"link or special file: {relative}")
    # The original one-off downloader has no marker. Our marker is optional and
    # must describe the same archive when present; it is never rewritten here.
    if EXTRACTION_MARKER in actual_files:
        try:
            marker = json.loads((destination / EXTRACTION_MARKER).read_text(encoding="utf-8"))
        except (OSError, ValueError):
            different("invalid extraction marker")
        if marker != {"package": package, "sha512": metadata["sha512"], "file_count": len(members)}:
            different("extraction marker does not match the archive")
        actual_files.remove(EXTRACTION_MARKER)
    if actual_files != expected_files or actual_dirs != expected_dirs:
        different("missing or extra archive members")
    for item, relative in members:
        if item.is_dir():
            continue
        path = destination.joinpath(*relative.parts)
        if path.stat().st_size != item.file_size:
            different(f"file size: {relative}")
        with archive.open(item) as source, path.open("rb") as existing:
            while True:
                expected = source.read(1024 * 1024)
                actual = existing.read(1024 * 1024)
                if expected != actual:
                    different(f"file contents: {relative}")
                if not expected:
                    break


def extract_archive(archive_path: Path, output: Path, package: str, metadata: dict,
                    *, verify_only: bool = False) -> list:
    destination = output / package
    with zipfile.ZipFile(archive_path) as archive:
        members = safe_members(archive)
        if destination.exists() or destination.is_symlink():
            verify_extraction(archive, members, destination, package, metadata)
            print(f"Verified existing extraction in place: {destination}", flush=True)
            return [item.filename for item, _ in members]
    if verify_only:
        raise RuntimeError(f"missing extraction in --verify-only mode: {destination}")
    staging = Path(tempfile.mkdtemp(prefix="." + package + ".extract-", dir=output))
    try:
        with zipfile.ZipFile(archive_path) as archive:
            members = safe_members(archive)
            for item, relative in members:
                target = staging.joinpath(*relative.parts)
                if item.is_dir():
                    target.mkdir(parents=True, exist_ok=True)
                    continue
                target.parent.mkdir(parents=True, exist_ok=True)
                with archive.open(item) as source, target.open("xb") as stream:
                    shutil.copyfileobj(source, stream, length=1024 * 1024)
            names = [item.filename for item, _ in members]
        (staging / EXTRACTION_MARKER).write_text(json.dumps({
            "package": package, "sha512": metadata["sha512"], "file_count": len(names),
        }, indent=2) + "\n", encoding="utf-8")
        if destination.exists() or destination.is_symlink():
            raise RuntimeError(f"extraction appeared concurrently; refusing to replace: {destination}")
        staging.rename(destination)
        print(f"Extracted {package}: {destination}", flush=True)
        return names
    finally:
        if staging.exists():
            shutil.rmtree(staging)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True,
                        help="cache/extraction root (reuses OUTPUT/nuget archives)")
    parser.add_argument("--version", default=DEFAULT_VERSION,
                        help=f"exact Windows SDK package version (default: {DEFAULT_VERSION})")
    parser.add_argument("--verify-only", action="store_true",
                        help="check existing caches/extractions; never create, download or write output")
    args = parser.parse_args()
    if not re.fullmatch(r"10\.0\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?", args.version):
        parser.error("--version must be an exact 10.0.BUILD.REVISION NuGet version")
    root = args.output.expanduser().resolve()
    cache, output = root / "nuget", root / "sdk-nuget"
    for directory in (cache, output):
        if directory.is_symlink():
            raise RuntimeError(f"refusing a cache/extraction directory symlink: {directory}")
        if args.verify_only:
            if not directory.is_dir():
                raise RuntimeError(f"missing directory in --verify-only mode: {directory}")
        else:
            directory.mkdir(parents=True, exist_ok=True)
    receipts = []
    for package in PACKAGES:
        metadata = catalog_metadata(package, args.version)
        archive, url = fetch_archive(package, args.version, cache, metadata, verify_only=args.verify_only)
        names = extract_archive(archive, output, package, metadata, verify_only=args.verify_only)
        receipts.append({
            "package": package,
            "version": args.version,
            "source": url,
            "registration": metadata["registration"],
            "catalog": metadata["catalog"],
            "size": archive.stat().st_size,
            "sha256": digest_file(archive, "sha256").hex(),
            "sha512": metadata["sha512"],
            "source_hash_pinned": package in PINNED_SHA512 and args.version == DEFAULT_VERSION,
            "archive": str(archive),
            "output": str(output / package),
            "file_count": len(names),
            "representative_files": [name for name in names if name.lower().endswith(
                ("sdkddkver.h", "kernel32.lib", "ucrt.lib", "windows.h", "midl.exe", "rc.exe"))],
        })
    if args.verify_only:
        print("Verified all cached SDK packages and extractions; no output was written", flush=True)
        return 0
    manifest = root / "sdk-download-manifest.json"
    handle, temporary_name = tempfile.mkstemp(prefix=".sdk-download-manifest-", dir=root)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as stream:
            json.dump(receipts, stream, indent=2)
            stream.write("\n")
        temporary.replace(manifest)
    finally:
        temporary.unlink(missing_ok=True)
    print(f"SDK manifest: {manifest}", flush=True)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError, zipfile.BadZipFile) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
