# mojo-system

An unofficial component of the [mojo-ipc SDK](https://github.com/looading/mojo-ipc); this is not an official Google or Chromium crate.

Provides owned message pipes, data pipes, shared buffers, handle signals and traps.

This component is part of SDK `0.2.0`.
Keep the SDK component versions aligned when upgrading.

Upstream Chromium is pinned to `bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`.

The Rust crate builds with ordinary Cargo and does not require a Chromium checkout. Running IPC requires the matching native SDK library, explicitly loaded at runtime through `mojo-ipc::Runtime::load`. This crate does not download the native runtime.

Version `0.2.0` adds an internal single-event trap arming path for SDK
watchers. Public `arm()` retains its result type and blocking-event subset
semantics; the helper adds no trigger ordering guarantee. Its performance
effects require separate measurements.

Licensed under BSD-3-Clause; see [LICENSE](LICENSE).
