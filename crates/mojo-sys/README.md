# mojo-sys

An unofficial component of the [mojo-ipc SDK](https://github.com/looading/mojo-ipc); this is not an official Google or Chromium crate.

Provides generated Mojo C types, the native SDK API table and runtime dispatch through dynamically loaded function pointers.

This component is part of SDK `0.2.2`.
Keep the SDK component versions aligned when upgrading.

Upstream Chromium is pinned to `bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`.

The Rust crate builds with ordinary Cargo and does not require a Chromium checkout. Running IPC requires the matching native SDK library, explicitly loaded at runtime through `mojo-ipc::Runtime::load`. This crate does not download the native runtime.

Licensed under BSD-3-Clause; see [LICENSE](LICENSE).

## Explicit Chromium source selection

`RuntimeConfig::default()` keeps the official Chromium pin above. Consumers of an audited custom build can pass an exact 40-character lowercase commit through `initialize_with_config`, or `mojo_ipc::Runtime::load_with_config`. This still enforces native C ABI v1, the exact expected revision, the API table and thunk sizes. An empty value, branch name, short commit or different revision is rejected. Reinitialization also checks the requested path, role and revision.

The caller must audit compatibility of the custom Chromium public C headers/thunks with the generated bindings in this crate. The source of those generated types remains the official pin; choosing a custom runtime does not regenerate types or establish compatibility by itself. No process environment variable silently changes the expected revision.
