#!/usr/bin/env python3
"""Verify the complete native package matrix and publish it through GitHub CLI."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile

from package_native import COMMIT, TARGETS, digest, validate_receipts, version


def verify_packages(directory: Path, tag: str, sdk_commit: str) -> list[Path]:
    if tag != "v" + version():
        raise RuntimeError("release tag does not match workspace.package.version")
    archives = [directory / f"mojo-native-{version()}-{target}.tar.gz" for target in TARGETS]
    if {p.name for p in directory.glob("*.tar.gz")} != {p.name for p in archives}:
        raise RuntimeError("release requires exactly the complete four-target archive set")
    checksums = []
    for target, archive in zip(TARGETS, archives):
        expected_checksum = f"{digest(archive.read_bytes())}  {archive.name}\n"
        if archive.with_name(archive.name + ".sha256").read_text(encoding="utf-8") != expected_checksum:
            raise RuntimeError(f"archive checksum mismatch: {archive.name}")
        checksums.append(expected_checksum)
        stem = archive.name.removesuffix(".tar.gz")
        with tarfile.open(archive, "r:gz") as stream:
            members = stream.getmembers()
            if any(not m.isfile() or not m.name.startswith(stem + "/") or "/" in m.name[len(stem) + 1:] for m in members):
                raise RuntimeError("unexpected archive member type or path")
            payload = {m.name[len(stem) + 1:]: stream.extractfile(m).read() for m in members}
            if len(payload) != len(members):
                raise RuntimeError("duplicate archive members")
        metadata = json.loads(payload.pop("package-manifest.json"))
        for key, value in {"sdk_version": version(), "sdk_commit": sdk_commit, "target": target,
                           "chromium_commit": COMMIT, "abi_version": 1}.items():
            if metadata.get(key) != value:
                raise RuntimeError(f"package provenance mismatch: {key}")
        if metadata.get("files") != {name: digest(data) for name, data in payload.items()}:
            raise RuntimeError("package member checksums do not match its manifest")
        filename = TARGETS[target][2]
        expected_files = {filename, "mojo_rs_native.h", "native-manifest.json", "acceptance-manifest.json",
                          "dependency-audit.json", "dependency-audit.txt", "LICENSE", "CHROMIUM_LICENSE",
                          "THIRD_PARTY_NOTICES.txt"}
        if payload.keys() != expected_files or any(not data for data in payload.values()):
            raise RuntimeError("package is missing required files or contains unexpected files")
        validate_receipts(target, json.loads(payload["native-manifest.json"]),
                          json.loads(payload["acceptance-manifest.json"]), payload[filename], sdk_commit,
                          allow_previous_attempts=True)
        audit = json.loads(payload["dependency-audit.json"])
        if audit.get("passed") is not True or audit.get("target") != target:
            raise RuntimeError("package native dependency audit did not pass")
    checksum_file = directory / "SHA256SUMS"
    checksum_file.write_text("".join(sorted(checksums)), encoding="utf-8")
    return [*archives, checksum_file]


def publish(files: list[Path], tag: str, sdk_commit: str) -> None:
    if os.environ.get("GITHUB_REF") != "refs/tags/" + tag:
        raise RuntimeError("publication requires running on the matching version tag")
    repository = os.environ["GITHUB_REPOSITORY"]
    result = subprocess.run(["gh", "api", f"repos/{repository}/releases/tags/{tag}"], text=True, capture_output=True)
    if result.returncode:
        if "HTTP 404" not in result.stderr:
            raise RuntimeError(f"could not query release: {result.stderr}")
        notes = (f"Native Mojo runtime {tag}\n\nSDK commit: {sdk_commit}\nChromium: {COMMIT}\nABI: 1\n\n"
                 "All four targets passed native smoke, independent-process named-channel, Rust typed IPC, "
                 "and native dependency/export checks. Each archive includes the C header, provenance, "
                 "acceptance receipts and dependency licenses. Verify the download against SHA256SUMS, "
                 "extract it, and point MOJO_RUNTIME_LIBRARY at the library.\n\n"
                 "Platform requirements and the audit scope are recorded in dependency-audit.json.\n")
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "notes.md"
            path.write_text(notes, encoding="utf-8")
            subprocess.run(["gh", "release", "create", tag, "--repo", repository, "--verify-tag", "--draft",
                            "--title", f"Mojo native {tag}", "--notes-file", str(path)], check=True)
        release = {"draft": True}
    else:
        release = json.loads(result.stdout)
    if not release["draft"]:
        expected = {p.name: "sha256:" + digest(p.read_bytes()) for p in files}
        actual = {asset["name"]: asset.get("digest") for asset in release["assets"]}
        if actual != expected:
            raise RuntimeError("published release assets differ; refusing to overwrite a published release")
        print(f"Release {tag} already contains these exact assets")
        return
    # Only drafts can be retried with replacement. The public release is edited
    # only after all five assets have uploaded successfully.
    subprocess.run(["gh", "release", "upload", tag, "--repo", repository, "--clobber", *map(str, files)], check=True)
    uploaded = json.loads(subprocess.check_output(
        ["gh", "api", f"repos/{repository}/releases/tags/{tag}"], text=True))
    expected = {p.name: "sha256:" + digest(p.read_bytes()) for p in files}
    if {a["name"]: a.get("digest") for a in uploaded["assets"]} != expected:
        raise RuntimeError("uploaded release assets do not match the verified complete package set")
    subprocess.run(["gh", "release", "edit", tag, "--repo", repository, "--draft=false",
                    "--prerelease=" + ("true" if "-" in version() else "false")], check=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package-dir", type=Path, required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--verify-only", action="store_true")
    args = parser.parse_args()
    commit = os.environ["GITHUB_SHA"]
    files = verify_packages(args.package_dir, args.tag, commit)
    if not args.verify_only:
        publish(files, args.tag, commit)
    print("Verified native release assets:", ", ".join(p.name for p in files))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
