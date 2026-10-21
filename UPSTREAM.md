# Upstream provenance and port changes

Chromium revision: `bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`.
Official source: <https://chromium.googlesource.com/chromium/src/>.

Production Rust code is copied from `mojo/public/rust/{c_mojo_api,system,
mojom_value_parser,bindings}`. The public C headers are copied from
`mojo/public/c/system`. Copyright notices are retained. Python provenance and
the original source hashes are separately recorded in
`crates/mojom-build/vendor/UPSTREAM.json`.

Intentional Rust changes:

1. GN crate imports become Cargo dependencies. No GN-built Rust rlibs are
   distributed to consumers.
2. C++ ScopedHandle/associated-endpoint interoperability is excluded. The Rust
   associated-interface router remains present.
3. `base::SequencedTaskRunner` and trace/time CXX wrappers are replaced by a
   narrow C ABI adapter. The actual native scheduler remains Chromium's base.
4. The derive macros accept `#[mojom(runtime = "::ipc")]`, use explicit facade
   paths, and support renamed Cargo dependencies.
5. A watcher retains its supplied runner when rearming, instead of assuming
   the caller's thread has a current Chromium sequence.
6. Disconnect drains and drops pending success callbacks before invoking the
   disconnect handler. New requests after observed disconnect do not retain
   success callbacks indefinitely. The callback API still does not return a
   per-request RPC error result.
7. `deparse_structured_body` aligns each packed field before recording its
   pointer slot. The pinned upstream encoder recorded some pointer locations
   before alignment, corrupting a string/nullable string following a u32.
   Regression tests assert the exact wire slot and reject truncated data.
8. Non-nullable `uint8` arrays use a compact `MojomValue::ByteArray(Vec<u8>)`
   representation and bulk byte reads/writes. Native `Vec<u8>` and `[u8; N]`
   move their buffers using checked `Any` downcasts, without unsafe casts.
   Custom typemaps still run their per-element conversion; maps expand compact
   scalar columns before the existing length/duplicate-key validation. Fixed
   lengths, pointers, declared array storage, truncation and eight-byte padding
   remain validated. The Mojom schema and wire format are unchanged, and the
   encoder still accepts the legacy `Array(UInt8...)` representation.
9. Fixed-array typemap conversion failures propagate their original error.
   The inherited `process_results` closure previously unwrapped a short
   iterator before its conversion error could propagate, causing a panic.
10. Other non-nullable scalar arrays use `MojomValue::PrimitiveArray`: bool,
    int8/16/32/64, uint16/32/64, float and double. Bools retain LSB-first bit
    packing; integers/floats use explicit little-endian storage, preserving
    floating-point bit patterns. Raw float vectors safely wrap/unwrap
    OrderedFloat rather than casting Vec layouts. Map scalar columns and
    associated-interface uint32 ID arrays accept compact values while keeping
    their duplicate-key and invalid-ID checks. Enum predicates and
    handles/endpoints retain the generic implementation.
11. Nullable numeric/bool arrays use `NullablePrimitiveArray`, with a leading
    LSB-first tag bitmap and retained zero-filled None value slots. Wide slots
    align after tags; bool values use a separate bitmap. Decode validates the
    full body and tail before allocating, retains dirty-None/padding tolerance,
    and preserves Some floating-point bits. Native Option buffers use checked
    Any transfers; custom typemaps still convert Some and never convert None.
    Nullable enum predicates and handle/endpoint sentinel rules remain generic.

12. Bitmap operations batch full groups: x86_64 uses baseline SSE2 for
    non-nullable bools; other targets use the portable implementation. Nullable
    scalar tags are encoded in groups of eight; decoding detects globally
    all-None/all-Some bitmaps and retains linear chunks for mixed values.
    All-None encoding reuses the zero-initialized body.
    Neither Option layout nor Vec control blocks are reinterpreted; wire/None
    padding, enum/handle checks and floating-point bits remain unchanged.
13. Derived concrete structs cache their packed schema in a per-type OnceLock
    and construct field vectors directly. The fallback cache for hand-written
    impls remains. Context must not change a type's schema, as before.
14. Outgoing typed messages use a fresh committed Core message with separate
    header/payload slices, avoiding an intermediate concatenation. Checked
    lengths and handle ownership follow the pinned C API contract. The public
    owned message and into_data APIs are retained. Sequence tasks use one
    Box<F> and concrete run/cancel callbacks with the same native ABI.

The compact byte-array representation adds a variant to the public Rust
`MojomValue` enum. Other scalar arrays also add `PrimitiveArray` and a matching
public enum; nullable elements add `NullablePrimitiveArray`. Direct AST users
with exhaustive matches must handle these forms;
derived equality/hash/order also distinguish it from a legacy `Array` even
when their encoded bytes match. This is a Rust source/API change, not a wire
protocol change, and must be considered when choosing a future release version.
Generated Mojom methods and their `Vec<u8>` / `[u8; N]` field types are unchanged.

The native runtime and Cargo/Mojom build adapters are SDK-specific code, not
officially published Google/Chromium APIs. The bundled generator modifications
replace GN imports with Cargo paths; unsupported semantic attributes are
rejected rather than silently treated as supported.

The native build explicitly disables Chromium's allocator shim and
PartitionAlloc-as-malloc (plus their BRP/alloc-token settings). The Apple shim
has a constructor which replaces the process's default malloc zone; a dynamically
loaded SDK must not install that process-wide allocator policy in its Rust host.
PartitionAlloc remains available for explicit internal upstream uses. GN graph
generation selects the SDK root target and root pattern rather than editing
Chromium's root BUILD.gn. Linux's named platform channel also requires upstream
`//net`; the native dependency closure is not limited to base and mojo alone.

The Windows cross-build producer uses Microsoft's SDK 10.0.28000.2270 NuGet
packages (installed SDK version 10.0.28000.0) and genuine xwin CRT inputs. The
fetcher pins the official package SHA-512 values; the adapter preserves actual
SDK/CRT versions and checks COFF library architectures before writing a local
winsysroot and Chromium `SetEnv` metadata. These packages remain producer inputs
and are not bundled into the Rust crate sources.

For this pin, the generated `chromium-compat.patch` changes only
`build/config/win/visual_studio_version.gni`'s explicit-toolchain branch from
`visual_studio_runtime_dirs = []` to an empty string. That value is passed as a
single `exec_script` argument, which GN requires to be a string. The automatic
Chromium toolchain path and minimum SDK checks are preserved. Windows
compiler-rt and the host-native RC are downloaded at their existing DEPS hashes;
the macOS toolchain files are retained.

Upstream updates must update the revision, original-source records, generated
FFI and all native artifacts together, then repeat generated-protocol and
real native integration tests on every supported target.
