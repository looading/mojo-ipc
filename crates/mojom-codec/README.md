# mojom-codec

An unofficial component of the [mojo-ipc SDK](https://github.com/looading/mojo-ipc); this is not an official Google or Chromium crate.

Re-exports the Mojom codec and derive support used by generated protocol types.

This component is part of SDK `0.2.0`.
Keep the SDK component versions aligned when upgrading.

Upstream Chromium is pinned to `bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`.

The Rust crate builds with ordinary Cargo and does not require a Chromium checkout. Running IPC requires the matching native SDK library, explicitly loaded at runtime through `mojo-ipc::Runtime::load`. This crate does not download the native runtime.

The `0.2.0` codec and derive use a coordinated hidden serialization hook.
Upgrade them together. New compact public AST variants require low-level
consumers to adapt exhaustive matches and equality/hash/order from `0.1.0`;
Mojom wire compatibility is a separate guarantee.

Licensed under BSD-3-Clause; see [LICENSE](LICENSE).
