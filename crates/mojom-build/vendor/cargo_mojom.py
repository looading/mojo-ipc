#!/usr/bin/env python3
"""Cargo adapter around the pinned official Chromium parser and Rust generator.

The IDL lexer, parser, type translation, wire metadata, and Rust serialization
templates are upstream implementations. This adapter resolves imports, rejects
unsupported SDK attributes, stages inputs, and supplies Cargo metadata.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys

REVISION = "bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd"
VENDOR = Path(__file__).resolve().parent
MOJOM = VENDOR / "mojo/public/tools/mojom"
BINDINGS = VENDOR / "mojo/public/tools/bindings"
sys.path.insert(0, str(MOJOM))


def write_if_changed(path, content):
    path.parent.mkdir(parents=True, exist_ok=True)
    data = content.encode("utf-8") if isinstance(content, str) else content
    if not path.exists() or path.read_bytes() != data:
        temporary = path.with_name(path.name + ".tmp")
        temporary.write_bytes(data)
        temporary.replace(path)


def logical_path(value):
    value = str(value).replace("\\", "/")
    path = PurePosixPath(value)
    if path.is_absolute() or ".." in path.parts or ":" in value or not path.parts:
        raise ValueError(f"invalid logical Mojom path: {value!r}")
    if path.suffix != ".mojom":
        raise ValueError(f"schema must have a .mojom extension: {value!r}")
    return str(path)


def validate_attributes(node):
    """Validate actual upstream AST nodes, without a second IDL parser."""
    from mojom.parse import ast

    if isinstance(node, ast.Attribute):
        # These are processed by the official conditional-features pass. Other
        # attributes require explicit runtime semantics and are not V1 claims.
        if node.key.name not in {"EnableIf", "EnableIfNot"}:
            raise ValueError(
                f"{node.filename}:{node.start.line}: unsupported Rust SDK "
                f"attribute [{node.key.name}]; it is not silently ignored"
            )
    if isinstance(node, ast.NodeBase):
        for value in vars(node).values():
            if isinstance(value, ast.NodeBase):
                validate_attributes(value)
            elif isinstance(value, (list, tuple)):
                for child in value:
                    if isinstance(child, ast.NodeBase):
                        validate_attributes(child)


def collect_sources(configuration):
    from mojom.parse import conditional_features, parser

    roots = [Path(configuration["source_root"]).resolve()]
    roots.extend(Path(root).resolve() for root in configuration.get("include_roots", []))
    sources = {}
    features = configuration.get("features", [])

    def resolve(logical):
        matches = [root / logical for root in roots if (root / logical).is_file()]
        if not matches:
            raise ValueError(f"import {logical!r} was not found in the declared schema roots")
        unique = {str(path.resolve()): path.resolve() for path in matches}
        if len(unique) != 1:
            raise ValueError(f"ambiguous import {logical!r}: {', '.join(unique)}")
        return next(iter(unique.values()))

    def visit(logical):
        logical = logical_path(logical)
        if logical in sources:
            return
        physical = resolve(logical)
        sources[logical] = physical
        tree = parser.Parse(physical.read_text(encoding="utf-8"), logical)
        conditional_features.RemoveDisabledDefinitions(tree, features)
        validate_attributes(tree)
        for imported in tree.import_list:
            visit(imported.import_filename)

    for entry in configuration["sources"]:
        entry_path = Path(entry)
        if entry_path.is_absolute():
            entry = entry_path.resolve().relative_to(roots[0]).as_posix()
        visit(entry)
    return dict(sorted(sources.items()))


def rust_module_names(sources):
    names = {}
    for logical in sources:
        # Match upstream _GetQualifiedName's current file-to-module mapping.
        name = PurePosixPath(logical).name.split(".")[0]
        if not re.fullmatch(r"[A-Za-z_][A-Za-z_0-9]*", name):
            raise ValueError(f"{logical}: filename is not a Rust module identifier")
        if name in {"self", "Self", "super", "crate", "mod", "type", "use", "fn", "async", "await", "dyn", "match", "pub", "struct", "enum", "impl", "trait", "const", "static", "move", "ref", "mut", "where", "extern", "return", "loop", "for", "while", "if", "else", "in", "let", "as", "break", "continue", "false", "true", "unsafe"}:
            raise ValueError(f"{logical}: reserved Rust module name {name!r}")
        if name in names:
            raise ValueError(f"module name collision: {names[name]!r} and {logical!r} both map to {name!r}")
        names[name] = logical
    return names


def run(configuration):
    if sys.version_info < (3, 10):
        raise ValueError("the pinned official Rust generator requires Python 3.10+; set MOJOM_PYTHON")
    if configuration.get("chromium_revision", REVISION) != REVISION:
        raise ValueError("Mojom tool/runtime Chromium revision mismatch")
    runtime = configuration.get("runtime_path", "::mojo_ipc")
    if not re.fullmatch(r"::[A-Za-z_][A-Za-z_0-9]*", runtime):
        raise ValueError("runtime_path must be an absolute Rust crate path, such as ::ipc")
    if not configuration.get("sources"):
        raise ValueError("at least one entry schema is required")
    sources = collect_sources(configuration)
    names = rust_module_names(sources)
    output = Path(configuration["out_dir"]).resolve()
    stage, generated, bytecode = output / "schema", output / "generated", output / "bytecode"
    for directory in [stage, generated, bytecode]:
        directory.mkdir(parents=True, exist_ok=True)
    staged = []
    for logical, physical in sources.items():
        destination = stage / logical
        write_if_changed(destination, physical.read_bytes())
        staged.append(str(destination))
    dep_info = output / "rust-dep-info.json"
    write_if_changed(dep_info, json.dumps([{
        "target_name": "cargo:protocol",
        "mojom_sources": list(sources),
    }], indent=2) + "\n")
    metadata = output / "parser-metadata.json"
    write_if_changed(metadata, json.dumps({"sources": staged, "deps": []}, indent=2) + "\n")
    environment = os.environ.copy()
    environment["PYTHONNOUSERSITE"] = "1"
    environment["PYTHONDONTWRITEBYTECODE"] = "1"

    def invoke(*arguments):
        subprocess.run(
            [sys.executable, "-B", *map(str, arguments)],
            check=True, env=environment, cwd=output,
        )

    # Recompile when upstream filters/templates or our Cargo adaptation changes.
    template_inputs = sorted(
        path for path in VENDOR.rglob("*")
        if path.is_file() and path.suffix in {".py", ".tmpl"}
    )
    digest = hashlib.sha256()
    digest.update(repr(sys.version_info[:3]).encode("ascii"))
    for path in template_inputs:
        digest.update(path.relative_to(VENDOR).as_posix().encode("utf-8") + b"\0")
        digest.update(path.read_bytes())
    fingerprint = digest.hexdigest()
    stamp = bytecode / "fingerprint"
    if not (bytecode / "rust_templates.zip").exists() or not stamp.exists() or stamp.read_text() != fingerprint:
        invoke(BINDINGS / "mojom_bindings_generator.py", "-o", bytecode, "precompile", "-g", "rust")
        write_if_changed(stamp, fingerprint)
    parser_arguments = [
        MOJOM / "mojom_parser.py", "--input-root", stage,
        "--output-root", generated, "--check-imports", metadata,
        "--mojoms", *staged,
    ]
    for feature in configuration.get("features", []):
        parser_arguments.extend(["--enable-feature", feature])
    invoke(*parser_arguments)
    invoke(
        BINDINGS / "mojom_bindings_generator.py", "-o", generated,
        "generate", "-d", stage, "-g", "rust", "--bytecode_path", bytecode,
        # argparse treats an unknown --generator_argument containing spaces as
        # a positional input. A relative metadata path avoids that upstream CLI
        # edge case while every source path remains a separate argv element.
        f"--rust_dep_info={dep_info.name}", f"--rust_runtime_path={runtime}", *staged,
    )
    index = "// Generated module index. Include at the protocol crate root.\n"
    for name, logical in sorted(names.items()):
        # Rust uses a raw string because Windows paths may contain backslashes.
        path = str(generated / f"{logical}.rs")
        if '"#' in path:
            raise ValueError("output path contains an unsupported raw-string delimiter")
        index += f'#[path = r#"{path}"#]\npub mod {name};\n'
    write_if_changed(output / "mod.rs", index)
    dependencies = set(map(str, sources.values()))
    dependencies.update(str(path) for path in VENDOR.rglob("*") if path.is_file() and path.suffix in {".py", ".tmpl", ".json"})
    write_if_changed(output / "dependencies.json", json.dumps(sorted(dependencies), indent=2) + "\n")
    print(f"generated {len(sources)} official Mojom Rust module(s), Chromium {REVISION}")
    return output / "mod.rs"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        run(json.loads(arguments.manifest.read_text(encoding="utf-8")))
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"Mojom build error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
