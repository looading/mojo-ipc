# mojom-derive

An unofficial component of the [mojo-ipc SDK](https://github.com/looading/mojo-ipc); this is not an official Google or Chromium crate.

Provides derive macros that connect Rust data types to the pinned Mojom codec.

This component is part of SDK `0.2.0`.
Keep the SDK component versions aligned when upgrading.

Upstream Chromium is pinned to `bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`.

The Rust crate builds with ordinary Cargo and does not require a Chromium checkout. Running IPC requires the matching native SDK library, explicitly loaded at runtime through `mojo-ipc::Runtime::load`. This crate does not download the native runtime.

Version `0.2.0` emits a serialization-only hook understood by the matching
codec. Upgrade core, facade, derive and generated bindings together. The public
named AST conversion and custom Context conversion defaults are preserved.

Licensed under BSD-3-Clause; see [LICENSE](LICENSE).
