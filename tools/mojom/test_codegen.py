#!/usr/bin/env python3
"""End-to-end tests for the real upstream parser/generator Cargo adaptation."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

VENDOR = Path(__file__).resolve().parents[2] / "crates/mojom-build/vendor"
spec = importlib.util.spec_from_file_location("cargo_mojom", VENDOR / "cargo_mojom.py")
adapter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adapter)


class CodegenTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="mojom sdk space ")
        self.root = Path(self.directory.name)
        self.schema = self.root / "schemas"
        self.schema.mkdir()
        self.out = self.root / "output with spaces"

    def tearDown(self):
        self.directory.cleanup()

    def write(self, logical, text):
        path = self.schema / logical
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def generate(self, entry="echo.mojom", **extra):
        configuration = {
            "source_root": str(self.schema),
            "sources": [entry],
            "out_dir": str(self.out),
            "runtime_path": "::renamed_ipc",
            "features": [],
        }
        configuration.update(extra)
        return adapter.run(configuration)

    def test_imported_types_and_request_response_use_renamed_runtime(self):
        self.write("common.mojom", "module test.types; struct Request { string text; };\n")
        self.write("echo.mojom", 'module test.api; import "common.mojom"; interface Echo { Ping(test.types.Request request) => (string reply); };\n')
        self.generate()
        code = (self.out / "generated/echo.mojom.rs").read_text()
        self.assertIn("pub trait Echo", code)
        self.assertIn("crate::common::Request", code)
        self.assertIn("::renamed_ipc::__private", code)
        self.assertIn('#[mojom(runtime = "::renamed_ipc")]', code)
        self.assertIn("EXPECTS_RESPONSE", code)
        self.assertIn("deserialize_exact", code)
        self.assertNotIn("chromium::import!", code)
        self.assertIn("pub mod common;", (self.out / "mod.rs").read_text())

    def test_sync_method_is_rejected_before_codegen(self):
        self.write("echo.mojom", "module test; interface Echo { [Sync] Ping() => (); };\n")
        with self.assertRaisesRegex(ValueError, r"echo\.mojom:1.*\[Sync\]"):
            self.generate()

    def test_versioned_field_is_rejected_before_codegen(self):
        self.write("echo.mojom", "module test; struct Value { [MinVersion=1] string? text; };\n")
        with self.assertRaisesRegex(ValueError, r"\[MinVersion\]"):
            self.generate()

    def test_duplicate_file_modules_are_rejected(self):
        self.write("a/types.mojom", "module a; struct A { string text; };\n")
        self.write("b/types.mojom", "module b; struct B { string text; };\n")
        self.write("echo.mojom", 'module test; import "a/types.mojom"; import "b/types.mojom"; interface Echo { Ping(); };\n')
        with self.assertRaisesRegex(ValueError, "module name collision"):
            self.generate()

    def test_transitive_import_is_tracked_and_regenerated(self):
        leaf = self.write("leaf.mojom", "module test.leaf; struct Leaf { uint32 value; };\n")
        self.write("common.mojom", 'module test.common; import "leaf.mojom"; struct Common { test.leaf.Leaf value; };\n')
        self.write("echo.mojom", 'module test; import "common.mojom"; interface Echo { Ping(test.common.Common value); };\n')
        self.generate()
        dependencies = json.loads((self.out / "dependencies.json").read_text())
        self.assertIn(str(leaf.resolve()), dependencies)
        leaf.write_text("module test.leaf; struct Leaf { uint64 value; };\n")
        self.generate()
        self.assertIn("pub value: u64", (self.out / "generated/leaf.mojom.rs").read_text())

    def test_target_features_use_official_conditional_filter(self):
        self.write("echo.mojom", "module test; interface Echo { Ping(); [EnableIf=is_linux] LinuxOnly(); };\n")
        self.generate()
        generated = self.out / "generated/echo.mojom.rs"
        self.assertNotIn("fn LinuxOnly", generated.read_text())
        self.generate(features=["is_linux"])
        self.assertIn("fn LinuxOnly", generated.read_text())


if __name__ == "__main__":
    unittest.main()
