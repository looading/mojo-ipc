# mojom-codec-core

An unofficial component of the [mojo-ipc SDK](https://github.com/looading/mojo-ipc); this is not an official Google or Chromium crate.

Implements the Mojom wire value model, binary serialization and deserialization used by generated bindings.

This component is part of SDK `0.3.0`.
Keep the SDK component versions aligned when upgrading.

Upstream Chromium is pinned to `bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`.

The Rust crate builds with ordinary Cargo and does not require a Chromium checkout. Running IPC requires the matching native SDK library, explicitly loaded at runtime through `mojo-ipc::Runtime::load`. This crate does not download the native runtime.

Version `0.2.0` represents non-nullable primitive arrays using compact
`MojomValue::ByteArray(Vec<u8>)` and `MojomValue::PrimitiveArray(PrimitiveArray)`
representations. They cover bool, signed/unsigned integers, float and double.
Nullable elements use `MojomValue::NullablePrimitiveArray(NullablePrimitiveArray)`
with tag bitmaps and retained value slots; None encoding is zero-filled and
Some floats preserve their exact bits. Native Vec<Option<T>> and fixed arrays
avoid per-Some boxes, while custom conversions still run only for Some.
The Mojom wire format and generated field types
are unchanged. Low-level AST consumers upgrading from `0.1.0` must handle the new enum variants;
`ByteArray` and the legacy `Array(UInt8...)` have different Rust equality/hash/
order even though they encode to identical bytes. The encoder and Rust array
conversions continue to accept the legacy representation. Custom typemaps,
nullable elements and other element types retain their scalar semantics.
Bool arrays use LSB bit packing, and floats preserve their IEEE bit patterns.
Enums and handles/endpoints keep their validated generic path.

The `0.2.0` transition is a Rust source API change from `0.1.0`, including exhaustive AST matches
and representation equality/hash/order. The hidden serialization-only trait
hook and derive must be kept aligned with SDK `0.3.0`; public named AST conversion
and custom Context conversion defaults retain their previous semantics. Wire
compatibility is verified independently against historical registry `0.1.0`
consumers. The native C ABI remains v1.

Licensed under BSD-3-Clause; see [LICENSE](LICENSE).
