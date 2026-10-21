# mojom-build

Cargo adapter for Chromium's official Mojom parser and Rust generator, pinned to
`bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`. The official Python source, Rust
templates, PLY, Jinja2, MarkupSafe, and their licenses are included in the package.
Generation needs Python 3.10 or newer; it does not install Python packages or
download Chromium during a build. Set `MOJOM_PYTHON` to select an interpreter.

For SDK `0.2.1`, use matching `0.2.1` runtime, codec
and derive crates. This patch retains Rust source compatibility with `0.2.0`.
The compact public AST variants introduced in `0.2.0` require low-level
consumers upgrading from `0.1.0` to adapt and regenerate bindings; unchanged Mojom
wire bytes do not establish Rust source API compatibility.

Use a dedicated protocol crate. Its `build.rs` can contain:

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    mojom_build::Config::new()
        .source_root("schema")
        .source("echo.mojom")
        .runtime_crate("ipc")
        .generate()?;
    Ok(())
}
```

`source` paths are relative to `source_root`. All imports are discovered using
the official parser and generated into the same protocol crate. An additional
`include_root` supplies imported schemas; it does not introduce an external
generated Cargo crate. Distinct schema files must have distinct file stems.

The protocol crate's root `src/lib.rs` loads the generated module index:

```rust
include!(concat!(env!("OUT_DIR"), "/mojom/mod.rs"));
```

`runtime_crate("ipc")` corresponds to a dependency named `ipc`, for example:

```toml
[dependencies]
ipc = { package = "mojo-ipc", version = "=0.2.1" }

[build-dependencies]
mojom-build = "=0.2.1"
```

For local development inside the source repository's `examples/protocol`, use
these path dependencies instead. Adjust the paths for another directory layout:

```toml
[dependencies]
ipc = { package = "mojo-ipc", path = "../../crates/mojo-ipc" }

[build-dependencies]
mojom-build = { path = "../../crates/mojom-build" }
```

The adapter stages the complete import closure and invokes upstream template
precompilation, parser, and Rust generator. Cargo tracks imported schemas,
schema directories, and bundled tool inputs. Conditional features are selected
from Cargo's target configuration, with `enable_feature` for application flags.

V1 rejects semantic attributes other than `EnableIf` and `EnableIfNot`, including
`Sync`, `MinVersion`, `Native`, and Chromium runtime-feature annotations. Rejection
is deliberate: parsing an attribute does not establish the corresponding runtime
behavior. Custom typemap configuration and cross-Cargo-crate schema imports are
not yet exposed by this builder. Generated callback APIs, wire layouts, value
validation, and interface routing are the upstream implementations.

See `examples/protocol` for a real generated protocol, including imported types,
request/response, one-way methods, and typed pending-interface transfer.
