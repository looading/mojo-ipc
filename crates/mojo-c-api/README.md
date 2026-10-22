# mojo-c-api

An unofficial component of the [mojo-ipc SDK](https://github.com/looading/mojo-ipc); this is not an official Google or Chromium crate.

Wraps the Mojo C system API with Rust result types and owned handle operations.

This component is part of SDK `0.3.0`.
Keep the SDK component versions aligned when upgrading.

Upstream Chromium is pinned to `bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`.

The Rust crate builds with ordinary Cargo and does not require a Chromium checkout. Running IPC requires the matching native SDK library, explicitly loaded at runtime through `mojo-ipc::Runtime::load`. This crate does not download the native runtime.

Version `0.2.1` safely represents an empty native message buffer as an empty
Rust slice when the runtime returns a null pointer with length zero. Non-empty
buffers still require a valid pointer. Handle transfer and cleanup semantics
are retained.

Licensed under BSD-3-Clause; see [LICENSE](LICENSE).
