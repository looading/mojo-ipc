# Mojom tools

`crates/mojom-build` contains the actual bundled official Chromium toolchain so
that its Cargo package is self-contained. This directory provides a development
CLI and boundary tests for the same adapter used by Cargo build scripts.

```sh
python3.12 -B tools/mojom/test_codegen.py
python3.12 -B tools/mojom/mojom.py --manifest /absolute/path/config.json
```

A manifest is JSON with `source_root`, `sources` (relative logical `.mojom`
paths), `out_dir`, `runtime_path` (for example `::ipc`), and optional `features`
and `include_roots`. Paths to physical roots and output directories should be
absolute. The output `mod.rs` belongs at the generated protocol crate's root.

Use Python 3.10 or newer. PLY, Jinja2, and MarkupSafe are bundled, and no pip
installation or network access is needed to generate code.
