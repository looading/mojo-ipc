# Changelog

## 0.2.0 (2026-10-21)

This minor version changes the pre-1.0 Rust source API through new public compact-array AST
variants. Adapt exhaustive matches and representation equality/hash/order, and
upgrade the SDK codec, derive, generator and runtime components together to
`0.2.0`. Regenerate protocol bindings. Public named AST conversion and custom
Context defaults are retained. The Mojom wire format, Chromium pin and native
C ABI v1 remain unchanged; registry `0.1.0` remains a historical wire-compatibility
reference.

- Coalesce eligible primary-only endpoint dispatch into the existing posted
  watcher task, after releasing router/watcher callback guards. The original
  asynchronous boundary, bound sequence, per-pipe FIFO and associated-endpoint
  posted fallback remain. Eligibility is permanently disabled after associated
  registration or any fallback. Ordering relative to unrelated runner tasks
  can change: the historical extra second task turn is no longer guaranteed.
  Four real macOS x86_64 native tests cover FIFO/disconnect, callback reentry,
  unbound associated backlog and releasing the last watcher owner. Eight
  interleaved benchmark runs observed lower overall typed RTT medians, while
  the reverse-order block retained regressions and substantial environment
  variation. This is not a guarantee of lower production p99 in every case.
- Add stack-backed V1/V2/V3 outgoing message header encoding while retaining
  public `serialize() -> Vec<u8>` and exact bytes, including unknown flag bits.
- Add a single-event trap arming path for SDK watchers while retaining public
  `arm()` return types and blocking-event subset semantics. Separate allocator
  checks confirm removal of the outgoing header's 56-byte allocation and the
  internal arm path's 32-byte event-buffer allocation on the tested host.

- Reduce complex-type codec allocations: lazy parser labels for array/union/map
  metadata, serialization-only derived struct conversion without diagnostic
  field names, direct pointer-array tables and fixed map bodies. Keep the public
  named AST conversion and custom conversion defaults. The new hidden trait
  hook and derive must be upgraded together; wire compatibility is independent
  of Cargo source compatibility.
- Batch bool bitmaps with baseline SSE2 on x86_64 and a portable fallback;
  group nullable tag/value processing while preserving exact Mojom wire bytes.
- Cache derived concrete struct schemas without a per-message global cache
  lookup, and remove temporary tuple-vector/padding allocations.
- Send header and payload directly into one fresh committed Core message;
  preserve public owned-message APIs and handle ownership. Use one task Box
  with concrete callbacks while preserving the native run/cancel ABI.

- Add compact nullable scalar arrays, preserving leading tag bitmaps, aligned
  retained value slots, bool dual bitmaps, zero-filled None encoding, dirty
  None/padding decode tolerance and floating-point bits. Custom typemaps only
  convert Some; nullable enums and handles keep their existing validation.
- Add public `NullablePrimitiveArray` and `MojomValue::NullablePrimitiveArray`;
  like the other compact forms, this requires low-level AST match adaptation.
- Extend compact array encoding/decoding to bool, signed integers, uint16/32/64,
  float and double, including native and OrderedFloat vectors/fixed arrays.
  Preserve LSB bool packing, little-endian scalar layout and floating-point
  bit patterns. Enum/handle validation retains the generic path. Adapt map
  columns and associated-interface ID arrays to compact values.
- Add the public `PrimitiveArray` enum and `MojomValue::PrimitiveArray` variant;
  low-level AST matches and equality/hash/order must account for this form.
- Encode/decode non-nullable `array<uint8>` in bulk, avoiding one intermediate
  value and debug field-name allocation per byte. Preserve the existing Mojom
  wire format, fixed-array checks, nullable/nested arrays and custom typemaps.
- Add `MojomValue::ByteArray(Vec<u8>)`. This changes the public Rust AST enum:
  exhaustive matches and AST equality/hash/order need to account for the new
  representation. Legacy `Array(UInt8...)` remains encodable and convertible.
  This is not a published 0.1.0 patch or a claim of Rust source compatibility.

## 0.1.0

First crates.io release of the standalone Cargo port of Chromium Mojo, pinned to
`bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`.

- Explicit loading of a matching native Mojo Core library through a versioned C ABI.
- Broker/participant initialization, child-process bootstrap, and named connections.
- Official Rust message/data pipes, shared buffers, traps, interface bindings, and
  Mojom encoding with Cargo dependency aliases.
- Bundled official Mojom parser/generator and Python dependencies; generation
  requires Python 3.10+ without a separate pip installation.
- Real macOS x86_64 two-process typed IPC validation; Windows x64 native DLL and
  Rust executable cross-compilation and linking. Windows runtime, Linux native,
  and macOS arm64 native validation remain pending.

The Cargo packages contain Rust sources and generator assets. Native libraries
are supplied separately and are not downloaded or compiled by Cargo. This is a
community-maintained SDK port, not an official Google or Chromium release.
