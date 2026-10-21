"""Offline producer/package/release tests; fixtures are not Mojo runtime builds."""

from __future__ import annotations

import ast
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import package_native as package
import prepare_hosted as hosted
import release_native as release


SDK_COMMIT = "a" * 40
LINUX = """ELF Header:
  Type: DYN (Shared object file)
  Machine: Advanced Micro Devices X86-64
 0x0000000000000001 (NEEDED) Shared library: [libc.so.6]
 0x000000000000001d (RUNPATH) Library runpath: [$ORIGIN]
Symbol table '.dynsym':
 1: 0000000000000000 0 NOTYPE GLOBAL DEFAULT UND malloc@GLIBC_2.17 (2)
 2: 0000000000000000 0 OBJECT GLOBAL DEFAULT ABS MOJO_RS_NATIVE_1
 3: 0000000000001234 42 FUNC GLOBAL DEFAULT 11 MojoRsGetApiV1@@MOJO_RS_NATIVE_1
"""
WINDOWS = """ImageFileHeader {
  Machine: IMAGE_FILE_MACHINE_AMD64 (0x8664)
  Characteristics [ IMAGE_FILE_DLL ]
}
Export { Ordinal: 1
  Name: MojoRsGetApiV1
}
Export { Ordinal: 2
  Name: GetHandleVerifier
}
Import {
  Name: KERNEL32.dll
}
DelayImport {
  Name: ADVAPI32.dll
}
"""
MAC = """{arch}
/tmp/libmojo_rs_native.dylib:
    @rpath/libmojo_rs_native.dylib (compatibility version 0.0.0, current version 0.0.0)
    /usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1351.0.0)
0000000000001234 T _MojoRsGetApiV1
      minos 13.0
        sdk 15.5
"""


def audit_fixture(target):
    os_name, cpu, _ = package.TARGETS[target]
    raw = LINUX if os_name == "linux" else WINDOWS if os_name == "win" else MAC.format(arch="x86_64" if cpu == "x64" else "arm64")
    return package.audit_output(target, raw), raw


class HostedTests(unittest.TestCase):
    def test_provisioning_refuses_local_execution_before_mutation(self):
        with patch.dict(os.environ, {}, clear=True), self.assertRaisesRegex(RuntimeError, "GitHub-hosted"):
            hosted.require_hosted(Path("/tmp/cannot-clean-local"))

    def test_provisioning_rejects_workspace_and_existing_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            env = {"GITHUB_ACTIONS": "true", "RUNNER_ENVIRONMENT": "github-hosted",
                   "RUNNER_TEMP": str(root), "GITHUB_WORKSPACE": str(root / "workspace")}
            with patch.dict(os.environ, env):
                with self.assertRaisesRegex(RuntimeError, "disjoint"):
                    hosted.require_hosted(root / "workspace/producer")
                (root / "existing").mkdir()
                with self.assertRaisesRegex(RuntimeError, "must be new"):
                    hosted.require_hosted(root / "existing")
                hosted.require_hosted(root / "fresh")

    def test_small_config_keeps_normal_pinned_dependency_resolution(self):
        solutions = ast.literal_eval(hosted.gclient_config().split("=", 1)[1])
        self.assertEqual(solutions[0]["custom_deps"], {})
        self.assertEqual(solutions[0]["url"], hosted.CHROMIUM_URL)
        self.assertFalse(solutions[0]["managed"])
        self.assertEqual(solutions[0]["custom_vars"]["checkout_configuration"], "small")

    @unittest.skipUnless(os.environ.get("MOJO_TEST_GN"), "requires an explicit GN binary")
    def test_license_scan_preserves_standalone_graph_root(self):
        executable = os.environ["MOJO_TEST_GN"]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / ".gn").write_text('buildconfig = "//BUILDCONFIG.gn"\n')
            (root / "BUILDCONFIG.gn").write_text('set_default_toolchain("//:toolchain")\n')
            (root / "BUILD.gn").write_text('toolchain("toolchain") {}\ngroup("default") {}\n')
            overlay = root / "sdk/mojo_rs/native"
            overlay.mkdir(parents=True)
            (overlay / "BUILD.gn").write_text('group("runtime") { deps = [ "//third_party/example" ] }\n')
            third_party = root / "third_party/example"
            third_party.mkdir(parents=True)
            (third_party / "BUILD.gn").write_text('group("example") {}\n')
            out = root / "out"
            out.mkdir()
            (out / "args.gn").write_text("")
            (out / "build.ninja").write_text("")
            arguments = ["desc", f"--root={root}", str(out), package.LABEL, "deps", "--as=buildfile", "--all"]
            original = subprocess.run([executable, *arguments], capture_output=True, text=True, timeout=20)
            self.assertNotEqual(original.returncode, 0, "fixture must be unreachable from the default GN root")
            wrapped = subprocess.run([sys.executable, str(package.ROOT / "native/scripts/license_gn.py"), *arguments],
                                     env=dict(os.environ, MOJO_LICENSE_GN=executable),
                                     capture_output=True, text=True, timeout=20)
            self.assertEqual(wrapped.returncode, 0, wrapped.stdout + wrapped.stderr)
            self.assertIn("third_party/example/BUILD.gn", wrapped.stdout.replace("\\", "/"))


class AuditTests(unittest.TestCase):
    def test_all_supported_architectures_and_import_formats(self):
        for target in package.TARGETS:
            with self.subTest(target=target):
                audit, _ = audit_fixture(target)
                self.assertTrue(audit["passed"])
                self.assertEqual(audit["exports"], ["MojoRsGetApiV1"])

    def test_rejects_non_system_dependencies_and_delay_imports(self):
        for target, text in (("x86_64-unknown-linux-gnu", LINUX.replace("libc.so.6", "libbase.so")),
                             ("x86_64-apple-darwin", MAC.format(arch="x86_64").replace("/usr/lib/", "/opt/homebrew/lib/")),
                             ("x86_64-pc-windows-msvc", WINDOWS.replace("ADVAPI32.dll", "vcruntime140.dll"))):
            with self.subTest(target=target), self.assertRaisesRegex(RuntimeError, "dependencies"):
                package.audit_output(target, text)

    def test_rejects_wrong_cpu_unexpected_exports_and_absolute_rpath(self):
        for text in (LINUX.replace("X86-64", "AArch64"), LINUX.replace("MojoRsGetApiV1", "UnexpectedExport"),
                     LINUX.replace("$ORIGIN", "/home/runner/build")):
            with self.subTest(text=text), self.assertRaises(RuntimeError):
                package.audit_output("x86_64-unknown-linux-gnu", text)

    @unittest.skipUnless(sys.platform == "darwin" and shutil.which("cc"), "requires local macOS compiler")
    def test_actual_macho_inspection_and_host_allocator_probe(self):
        import platform
        target = "aarch64-apple-darwin" if platform.machine() == "arm64" else "x86_64-apple-darwin"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "fixture.c").write_text("void *MojoRsGetApiV1(void) { return 0; }\n")
            library = root / "libmojo_rs_native.dylib"
            subprocess.run(["cc", "-dynamiclib", str(root / "fixture.c"), "-o", str(library),
                            "-Wl,-install_name,@rpath/libmojo_rs_native.dylib"], check=True)
            audit, _ = package.audit_library(target, library)
            self.assertTrue(audit["default_malloc_zone_preserved"])


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.chromium = self.root / "chromium"
        self.chromium.mkdir()
        (self.chromium / "LICENSE").write_text("Chromium license fixture\n")
        self.output = self.root / "packages"
        self.environment = patch.dict(os.environ, {"GITHUB_SHA": SDK_COMMIT, "GITHUB_RUN_ID": "17", "GITHUB_RUN_ATTEMPT": "1"})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def artifact(self, target):
        artifact = self.root / target
        artifact.mkdir()
        os_name, cpu, filename = package.TARGETS[target]
        library = b"native binary test fixture; not a Mojo implementation\n"
        (artifact / filename).write_bytes(library)
        (artifact / "mojo_rs_native.h").write_bytes((package.ROOT / "native/include/mojo_rs_native.h").read_bytes())
        (artifact / "native-manifest.json").write_text(json.dumps({
            "chromium_commit": package.COMMIT, "abi_version": 1, "os": os_name, "cpu": cpu,
            "library": filename, "sha256": package.digest(library), "native_smoke_passed": True,
        }))
        receipt = {gate: True for gate in package.GATES}
        receipt.update(target=target, sdk_commit=SDK_COMMIT, github_run_id="17", github_run_attempt="1")
        (artifact / "acceptance-manifest.json").write_text(json.dumps(receipt))
        (artifact / "private-build.log").write_text("not for distribution")
        return artifact

    def build_package(self, target, artifact=None):
        artifact = artifact or self.artifact(target)

        def generate_notices(command, **kwargs):
            self.assertIn("license_file", command)
            Path(command[-1]).write_text("Third-party dependency notice fixture\n")

        with patch.object(package, "inspect", return_value=package.COMMIT), \
             patch.object(package, "audit_library", return_value=audit_fixture(target)), \
             patch.object(package.subprocess, "run", side_effect=generate_notices):
            return package.create_package(artifact, target, self.chromium, self.root / "gn-out", self.output)

    def build_matrix(self):
        for target in package.TARGETS:
            self.build_package(target)

    def test_round_trip_complete_matrix_and_archive_allowlist(self):
        self.build_matrix()
        files = release.verify_packages(self.output, "v" + package.version(), SDK_COMMIT)
        self.assertEqual(len(files), 5)
        self.assertEqual(len(files[-1].read_text().splitlines()), 4)
        with tarfile.open(files[0]) as stream:
            names = stream.getnames()
        self.assertFalse(any("private-build" in name for name in names))
        self.assertTrue(any(name.endswith("/THIRD_PARTY_NOTICES.txt") for name in names))

    def test_failed_stale_or_wrong_target_acceptance_cannot_be_packaged(self):
        for key, value in (("native_smoke_passed", False), ("sdk_commit", "b" * 40),
                           ("target", "invalid"), ("github_run_id", "old-run")):
            with self.subTest(key=key):
                target = next(iter(package.TARGETS))
                artifact = self.artifact(target)
                path = artifact / "acceptance-manifest.json"
                receipt = json.loads(path.read_text())
                receipt[key] = value
                path.write_text(json.dumps(receipt))
                with self.assertRaises(RuntimeError):
                    self.build_package(target, artifact)
                shutil.rmtree(artifact)

    def test_modified_library_or_header_is_rejected(self):
        target = next(iter(package.TARGETS))
        artifact = self.artifact(target)
        original = (artifact / package.TARGETS[target][2]).read_bytes()
        (artifact / package.TARGETS[target][2]).write_bytes(b"corrupted")
        with self.assertRaisesRegex(RuntimeError, "sha256"):
            self.build_package(target, artifact)
        (artifact / package.TARGETS[target][2]).write_bytes(original)
        (artifact / "mojo_rs_native.h").write_text("stale header")
        with self.assertRaisesRegex(RuntimeError, "header"):
            self.build_package(target, artifact)

    def test_release_rerun_accepts_prior_successful_jobs_from_same_run(self):
        self.build_matrix()
        with patch.dict(os.environ, {"GITHUB_RUN_ATTEMPT": "2"}):
            self.assertEqual(len(release.verify_packages(self.output, "v" + package.version(), SDK_COMMIT)), 5)
        with patch.dict(os.environ, {"GITHUB_RUN_ID": "other-run"}), self.assertRaisesRegex(RuntimeError, "another workflow run"):
            release.verify_packages(self.output, "v" + package.version(), SDK_COMMIT)

    def test_producer_cannot_package_an_old_attempt(self):
        target = next(iter(package.TARGETS))
        artifact = self.artifact(target)
        with patch.dict(os.environ, {"GITHUB_RUN_ATTEMPT": "2"}), self.assertRaisesRegex(RuntimeError, "workflow attempt"):
            self.build_package(target, artifact)

    def test_partial_matrix_and_wrong_tag_are_rejected(self):
        self.build_package(next(iter(package.TARGETS)))
        with self.assertRaisesRegex(RuntimeError, "complete four-target"):
            release.verify_packages(self.output, "v" + package.version(), SDK_COMMIT)
        with self.assertRaisesRegex(RuntimeError, "tag"):
            release.verify_packages(self.output, "v999.0.0", SDK_COMMIT)

    def test_corrupted_archive_is_rejected_before_publication(self):
        self.build_matrix()
        archive = next(self.output.glob("*.tar.gz"))
        archive.write_bytes(archive.read_bytes() + b"corruption")
        with self.assertRaisesRegex(RuntimeError, "checksum"):
            release.verify_packages(self.output, "v" + package.version(), SDK_COMMIT)


class PublishTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.file = Path(self.temporary.name) / "asset"
        self.file.write_bytes(b"asset data")
        self.tag = "v" + package.version()
        env = patch.dict(os.environ, {"GITHUB_REF": "refs/tags/" + self.tag, "GITHUB_REPOSITORY": "test/repo"})
        env.start()
        self.addCleanup(env.stop)

    def test_upload_failure_leaves_draft_unpublished(self):
        commands = []

        def run(command, **kwargs):
            commands.append(command)
            if command[:3] == ["gh", "release", "upload"]:
                raise subprocess.CalledProcessError(1, command)
            return subprocess.CompletedProcess(command, 0, json.dumps({"draft": True}), "")

        with patch.object(release.subprocess, "run", side_effect=run), self.assertRaises(subprocess.CalledProcessError):
            release.publish([self.file], self.tag, SDK_COMMIT)
        self.assertFalse(any(c[:3] == ["gh", "release", "edit"] for c in commands))

    def test_existing_public_release_cannot_be_overwritten(self):
        result = subprocess.CompletedProcess([], 0, json.dumps({"draft": False, "assets": []}), "")
        with patch.object(release.subprocess, "run", return_value=result) as run, self.assertRaisesRegex(RuntimeError, "refusing"):
            release.publish([self.file], self.tag, SDK_COMMIT)
        self.assertEqual(run.call_count, 1)

    def test_upload_is_verified_before_publishing(self):
        result = subprocess.CompletedProcess([], 0, json.dumps({"draft": True}), "")
        assets = {"assets": [{"name": self.file.name, "digest": "sha256:" + package.digest(self.file.read_bytes())}]}
        with patch.object(release.subprocess, "run", return_value=result) as run, \
             patch.object(release.subprocess, "check_output", return_value=json.dumps(assets)):
            release.publish([self.file], self.tag, SDK_COMMIT)
        self.assertEqual(run.call_args_list[-1].args[0][:3], ["gh", "release", "edit"])

    def test_mismatching_server_assets_leave_draft_unpublished(self):
        result = subprocess.CompletedProcess([], 0, json.dumps({"draft": True}), "")
        with patch.object(release.subprocess, "run", return_value=result) as run, \
             patch.object(release.subprocess, "check_output", return_value='{"assets": []}'), \
             self.assertRaisesRegex(RuntimeError, "uploaded release assets"):
            release.publish([self.file], self.tag, SDK_COMMIT)
        self.assertFalse(any(c.args[0][:3] == ["gh", "release", "edit"] for c in run.call_args_list))


if __name__ == "__main__":
    unittest.main()
