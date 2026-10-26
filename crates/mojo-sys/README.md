# mojo-sys

An unofficial component of the [mojo-ipc SDK](https://github.com/looading/mojo-ipc); this is not an official Google or Chromium crate.

Provides generated Mojo C types, the native SDK API table and runtime dispatch through dynamically loaded function pointers.

This component is part of SDK `0.3.0`.
Keep the SDK component versions aligned when upgrading.

Upstream Chromium is pinned to `bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`.

The Rust crate builds with ordinary Cargo and does not require a Chromium checkout. Running IPC requires the matching native SDK library, explicitly loaded at runtime through `mojo-ipc::Runtime::load`. This crate does not download the native runtime.

Licensed under BSD-3-Clause; see [LICENSE](LICENSE).

## Explicit Chromium source selection

`RuntimeConfig::default()` keeps the official Chromium pin above. Consumers of an audited custom build can pass an exact 40-character lowercase commit through `initialize_with_config`, or `mojo_ipc::Runtime::load_with_config`. This still enforces native C ABI v1, the exact expected revision, the API table and thunk sizes. An empty value, branch name, short commit or different revision is rejected. Reinitialization also checks the requested path, role and revision.

The caller must audit compatibility of the custom Chromium public C headers/thunks with the generated bindings in this crate. The source of those generated types remains the official pin; choosing a custom runtime does not regenerate types or establish compatibility by itself. No process environment variable silently changes the expected revision.

## C layout targets

The original 64-bit generated types remain in `src/generated/types.rs`.
Windows x86 MSVC, including the `i686-win7-windows-msvc` Rust target, selects
`types_windows_x86.rs`, generated with Clang target `i686-pc-windows-msvc` from
the same vendored official headers and native ABI1 header. It includes all
size, alignment and named-field offset assertions. `MOJO_POINTER_FIELD`
anonymous padding and the public structures' eight-byte alignment are retained;
the native API and thunk tables use their actual 32-bit pointer layout.
Other 32-bit targets are rejected instead of compiling without C layout checks.
The anonymous bitfield slots are opaque `MaybeUninit` storage, because C output
and callback objects need not initialize padding bytes. Their `Debug` formatter
does not read those bytes. The generator accepts only anonymous padding without
value getters; a future named bitfield fails generation for explicit review.

`cargo xtask gen-sys` regenerates both targets and verifies that their 46 thunk
dispatch signatures are identical. `cargo xtask gen-sys --check` verifies the
committed files against fresh Clang/bindgen output without rewriting them.
Generation requires libclang; ordinary Cargo builds continue to use committed
bindings without a Chromium checkout or native build.
