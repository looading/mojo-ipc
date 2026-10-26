"""Offline configuration tests; no fixture is a real native build."""
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

import build_native as build
import package_native as package
SDK_COMMIT = "a" * 40
import prepare_windows_toolchain as windows

CUSTOM = "6d2bf4f33d77670349c45ffa1464bef101c0eda5"

class ChromiumConfigurationTests(unittest.TestCase):
    def test_custom_overlay_keeps_checkout_pin_and_native_getter_in_sync(self):
        with tempfile.TemporaryDirectory() as temporary:
            checkout = Path(temporary)
            (checkout / ".gn").write_text("")
            args = ["build_native.py", "--chromium-src", str(checkout), "--chromium-revision", CUSTOM]
            with patch.object(sys, "argv", args), patch.object(build, "checked", return_value=CUSTOM), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(build.main(), 0)
            overlay = checkout / "sdk/mojo_rs/native"
            self.assertEqual((overlay / build.MARKER).read_text().strip(), CUSTOM)
            self.assertIn(f'mojo_rs_chromium_commit = "{CUSTOM}"', (overlay / "BUILD.gn").read_text())

    def test_windows7_profile_is_explicit_and_recorded_without_claiming_kernel_acceptance(self):
        with tempfile.TemporaryDirectory() as temporary:
            checkout = Path(temporary) / "src"
            checkout.mkdir()
            (checkout / ".gn").write_text("")
            out = checkout / "out/win7"
            out.mkdir(parents=True)
            (out / "mojo_rs_native.dll").write_bytes(b"fixture, not a runtime")
            artifact = Path(temporary) / "artifact"
            argv = ["build_native.py", "--chromium-src", str(checkout), "--chromium-revision", CUSTOM,
                    "--out", str(out), "--build", "--target-os", "win", "--target-cpu", "x64",
                    "--windows7", "--artifact-dir", str(artifact)]
            with patch.object(sys, "argv", argv), patch.object(build, "checked", return_value=CUSTOM), patch.object(build.platform, "system", return_value="Darwin"), patch.object(build, "tool", side_effect=lambda name:name), patch.object(build.shutil, "which", return_value=None), patch.object(build.subprocess, "run") as run, contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(build.main(), 0)
            self.assertIn("mojo_rs_windows7_compat=true", run.call_args_list[0].args[0][-4])
            manifest = json.loads((artifact / "native-manifest.json").read_text())
            self.assertEqual(manifest["windows_api_profile"], "win7-sp1")
            self.assertFalse(manifest["win7_runtime_verified"])
            self.assertFalse(manifest["native_smoke_passed"])
            self.assertTrue((checkout / "sdk/mojo_rs/native/windows7_compat.cc").is_file())

    def test_windows7_profile_rejects_non_windows_targets_before_mutating(self):
        with patch.object(sys, "argv", ["build_native.py", "--chromium-src", "/not-used", "--target-os", "mac", "--target-cpu", "arm64", "--windows7"]), contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, "Windows x64"):
                build.main()

    def test_default_pin_rejects_custom_checkout_before_mutation(self):
        with tempfile.TemporaryDirectory() as temporary:
            checkout = Path(temporary)
            (checkout / ".gn").write_text("")
            with patch.object(sys, "argv", ["build_native.py", "--chromium-src", str(checkout)]), patch.object(build, "checked", return_value=CUSTOM):
                with self.assertRaisesRegex(RuntimeError, "revision mismatch"):
                    build.main()
            self.assertFalse((checkout / "sdk").exists())

    def test_custom_build_embeds_the_same_revision_and_manifest(self):
        with tempfile.TemporaryDirectory() as temporary:
            checkout = Path(temporary) / "src"
            checkout.mkdir()
            (checkout / ".gn").write_text("")
            out = checkout / "out/test"
            out.mkdir(parents=True)
            (out / "libmojo_rs_native.dylib").write_bytes(b"test fixture, not a runtime")
            artifact = Path(temporary) / "artifact"
            args = ["build_native.py", "--chromium-src", str(checkout), "--chromium-revision", CUSTOM,
                    "--out", str(out), "--build", "--target-os", "mac", "--target-cpu", "arm64", "--artifact-dir", str(artifact)]
            with patch.object(sys, "argv", args), patch.object(build, "checked", return_value=CUSTOM), patch.object(build.platform, "system", return_value="Darwin"), patch.object(build, "tool", side_effect=lambda name:name), patch.object(build.shutil, "which", return_value=None), patch.object(build.subprocess, "run") as run, contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(build.main(), 0)
            gn = run.call_args_list[0].args[0]
            self.assertIn(f'mojo_rs_chromium_commit="{CUSTOM}"', next(x for x in gn if x.startswith("--args=")))
            self.assertNotIn("use_alloc_token", next(x for x in gn if x.startswith("--args=")))
            self.assertEqual(json.loads((artifact / "native-manifest.json").read_text())["chromium_commit"], CUSTOM)

    def test_custom_packaging_still_requires_explicit_matching_revision(self):
        payload = b"fixture, not a real runtime"
        native = {"chromium_commit": CUSTOM, "abi_version": 1, "os": "mac", "cpu": "arm64",
                  "library": "libmojo_rs_native.dylib", "native_smoke_passed": True, "sha256": package.digest(payload)}
        acceptance = dict.fromkeys(package.GATES, True)
        acceptance.update(target="aarch64-apple-darwin", sdk_commit=SDK_COMMIT)
        with patch.dict(package.os.environ, {}, clear=True):
            with self.assertRaisesRegex(RuntimeError, "chromium_commit"):
                package.validate_receipts("aarch64-apple-darwin", native, acceptance, payload, SDK_COMMIT)
            package.validate_receipts("aarch64-apple-darwin", native, acceptance, payload, SDK_COMMIT, chromium_commit=CUSTOM)
            with self.assertRaisesRegex(RuntimeError, "chromium_commit"):
                package.validate_receipts("aarch64-apple-darwin", native, acceptance, payload, SDK_COMMIT, chromium_commit="0" * 40)

    def test_windows_adapter_rejects_a_different_revision_before_editing(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = root / "adapter-manifest.json"
            manifest.write_text(json.dumps({"schema":1, "chromium_commit":CUSTOM}))
            before = manifest.read_bytes()
            with self.assertRaisesRegex(RuntimeError, "matching SDK adapter"):
                windows.append_atl(root, root, {})
            self.assertEqual(manifest.read_bytes(), before)

    def test_invalid_revision_and_conflicting_gn_override_are_rejected(self):
        for revision in ["main", "", "ABCDEF0123456789ABCDEF0123456789ABCDEF01"]:
            with patch.object(sys, "argv", ["build_native.py", "--chromium-src", "/not-used", "--chromium-revision", revision]), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    build.main()
        with patch.object(sys, "argv", ["build_native.py", "--chromium-src", "/not-used", "--build", "--gn-args", 'mojo_rs_chromium_commit="bad"']), contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaisesRegex(RuntimeError, "--chromium-revision"):
                build.main()

if __name__ == "__main__":
    unittest.main()
