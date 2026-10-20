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
