# Local Cargo adaptation

All upstream files were fetched from Chromium commit
`bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`. `UPSTREAM.json` records the SHA-256
of each unmodified source at acquisition, including third-party license files.
Chromium's license is in `LICENSE`; each vendored Python dependency retains its
own license in `third_party/<package>`.

Local changes:

* `cargo_mojom.py` is the SDK-owned wrapper, using the official AST parser for
  import discovery and capability validation. It drives the official two-stage
  parser/generator and produces a Cargo module index.
* `mojom_rust_generator.py` accepts `--rust_runtime_path`, passes that path to
  templates, and splits `--rust_dep_info` at the first equals sign.
* `rust_templates/module.tmpl` imports the runtime through its Cargo dependency
  alias instead of `chromium::import!`.
* The struct, enum, and union templates annotate derives with
  `#[mojom(runtime = "::dependency_alias")]`.
* Template macro imports use Jinja's `with context` so nested struct generation
  receives the selected runtime path.

The wire-format parser, field packing, method ordinals, request/response message
logic, and serialization templates otherwise retain their upstream behavior.
