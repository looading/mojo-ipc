#!/usr/bin/env python3
"""Run the same bundled official Mojom adapter used by Cargo build scripts."""

from pathlib import Path
import runpy

adapter = Path(__file__).resolve().parents[2] / "crates/mojom-build/vendor/cargo_mojom.py"
runpy.run_path(str(adapter), run_name="__main__")
