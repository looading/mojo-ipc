#!/usr/bin/env python3
"""Verify the complete native package matrix and publish it through GitHub CLI."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile

from package_native import COMMIT, ROOT, TARGETS, digest, validate_receipts, version


def validate_producer(run: dict, repository: str, run_id: str, *, completed: bool) -> tuple[str, str]:
    if (str(run.get("id")) != run_id or
            run.get("repository", {}).get("full_name") != repository or
            (run.get("head_repository") or {}).get("full_name") != repository or
            run.get("path", "").split("@", 1)[0] != ".github/workflows/native.yml" or
            run.get("event") not in ("push", "workflow_dispatch")):
        raise RuntimeError("untrusted native package producer")
    if completed and run.get("status") != "completed":
        raise RuntimeError("native package producer run has not completed")
    commit, attempt = run.get("head_sha", ""), run.get("run_attempt")
    if not re.fullmatch(r"[0-9a-f]{40}", commit) or type(attempt) is not int or attempt < 1:
        raise RuntimeError("invalid native package producer commit or attempt")
    return commit, str(attempt)


def verify_packages(directory: Path, tag: str, sdk_commit: str, *, sdk_root: Path = ROOT,
                    workflow_commit: str | None = None) -> list[Path]:
    sdk_version = version(sdk_root)
    if tag != "v" + sdk_version:
        raise RuntimeError("release tag does not match workspace.package.version")
    archives = [directory / f"mojo-native-{sdk_version}-{target}.tar.gz" for target in TARGETS]
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
        for key, value in {"sdk_version": sdk_version, "sdk_commit": sdk_commit, "target": target,
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
        acceptance = json.loads(payload["acceptance-manifest.json"])
        validate_receipts(target, json.loads(payload["native-manifest.json"]),
                          acceptance, payload[filename], sdk_commit,
                          allow_previous_attempts=True)
        if workflow_commit is not None and acceptance.get("workflow_commit") != workflow_commit:
            raise RuntimeError("acceptance belongs to another producer workflow commit")
        audit = json.loads(payload["dependency-audit.json"])
        if audit.get("passed") is not True or audit.get("target") != target:
            raise RuntimeError("package native dependency audit did not pass")
    checksum_file = directory / "SHA256SUMS"
    checksum_file.write_text("".join(sorted(checksums)), encoding="utf-8")
    return [*archives, checksum_file]


def api(endpoint: str, *, paginate: bool = False, payload: dict | None = None) -> object:
    command = ["gh", "api", endpoint]
    if paginate:
        command.extend(["--paginate", "--slurp"])
    if payload is None:
        return json.loads(subprocess.check_output(command, text=True))
    with tempfile.TemporaryDirectory() as temporary:
        path = Path(temporary) / "request.json"
        path.write_text(json.dumps(payload), encoding="utf-8")
        return json.loads(subprocess.check_output(
            [*command, "--method", "PATCH", "--input", str(path)], text=True))


def find_release(repository: str, tag: str) -> dict | None:
    # The by-tag endpoint only returns published releases. Listing with push
    # access also returns drafts; their numeric IDs remain stable on publish.
    pages = api(f"repos/{repository}/releases?per_page=100", paginate=True)
    matches = [item for page in pages for item in page if item.get("tag_name") == tag]
    if len(matches) > 1:
        raise RuntimeError("multiple matching release drafts; refusing an ambiguous upload")
    return matches[0] if matches else None


def publish(files: list[Path], tag: str, sdk_commit: str) -> None:
    if os.environ.get("GITHUB_REF") != "refs/tags/" + tag:
        raise RuntimeError("publication requires running on the matching version tag")
    repository = os.environ["GITHUB_REPOSITORY"]
    release = find_release(repository, tag)
    if release is None:
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
        release = find_release(repository, tag)
        if release is None:
            raise RuntimeError("created release draft is not visible to its publisher")
    expected = {p.name: "sha256:" + digest(p.read_bytes()) for p in files}
    if not release["draft"]:
        actual = {asset["name"]: asset.get("digest") for asset in release["assets"]}
        if actual != expected:
            raise RuntimeError("published release assets differ; refusing to overwrite a published release")
        print(f"Release {tag} already contains these exact assets")
        return
    # Only drafts can be retried with replacement. Reuse already verified
    # uploads after an interrupted publication instead of uploading them again.
    actual = {asset["name"]: asset.get("digest") for asset in release["assets"]}
    if actual != expected:
        subprocess.run(["gh", "release", "upload", tag, "--repo", repository, "--clobber", *map(str, files)], check=True)
    release_id = release["id"]
    endpoint = f"repos/{repository}/releases/{release_id}"
    uploaded = api(endpoint)
    if {a["name"]: a.get("digest") for a in uploaded["assets"]} != expected:
        raise RuntimeError("uploaded release assets do not match the verified complete package set")
    published = api(endpoint, payload={"draft": False, "prerelease": "-" in tag})
    if published.get("draft") is not False or published.get("tag_name") != tag:
        raise RuntimeError("GitHub did not confirm the expected published release")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package-dir", type=Path, required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--verify-only", action="store_true")
    parser.add_argument("--sdk-root", type=Path, default=ROOT)
    parser.add_argument("--workflow-commit", help="expected native producer workflow commit")
    args = parser.parse_args()
    commit = os.environ["GITHUB_SHA"]
    files = verify_packages(args.package_dir, args.tag, commit, sdk_root=args.sdk_root,
                            workflow_commit=args.workflow_commit)
    if not args.verify_only:
        publish(files, args.tag, commit)
    print("Verified native release assets:", ", ".join(p.name for p in files))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
