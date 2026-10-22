# mojo-ipc

An independent, unofficial Rust SDK for Chromium's Mojo IPC. This project is not
published or supported by the Chromium project or Google. It uses the official
Mojo Core and Rust bindings, pinned to Chromium commit
[`bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`](https://chromium.googlesource.com/chromium/src/+/bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd).

The crate provides typed Mojom clients and receivers, message pipes, shared
buffers, and process bootstrap helpers. This is an early SDK for communication
between your own Rust processes. Protocol version negotiation and interoperability
with arbitrary Chromium versions are not supported.

## Custom Chromium checkout

The default `Runtime::load` keeps the official Chromium pin. For a trusted custom native build, use an explicit configuration in **each process**:

```rust,no_run
use mojo_ipc::{Role, Runtime, RuntimeConfig};
let config = RuntimeConfig {
    chromium_commit: "6d2bf4f33d77670349c45ffa1464bef101c0eda5",
};
// SAFETY: the caller audited the custom image, C ABI and generated C/thunk layouts.
let runtime = unsafe { Runtime::load_with_config("/trusted/libmojo_rs_native.dylib", Role::Broker, config)? };
# Ok::<(), mojo_ipc::InitError>(())
```

The supplied revision is checked exactly; it does not bypass source or ABI checks. A process cannot change the runtime path, broker/participant role or revision after successful initialization. Bindings and the Mojom generator retain their own official source provenance. See [native producer configuration](../../native/README.md#custom-source) for checkout/build commands and compatibility requirements.

## Upgrading to 0.3.0

Use matching `=0.3.0` SDK components and `mojom-build`, and regenerate shared
protocol bindings together. The optional `mojo-service = "=0.3.0"` crate adds
local service discovery, bounded business calls and process supervision above
the existing typed IPC API. Keep its control definitions matched between peers.
The default Chromium pin and native C ABI v1 remain unchanged; this does not
assert Rust source compatibility between `0.3.0` and `0.2.x`.

## Historical 0.2.1 patch

SDK `0.2.1` retained Rust source compatibility with `0.2.0` when using matched
components. That patch left the Mojom wire format, generated field types,
Chromium pin and native C ABI v1 unchanged.

Eligible primary dispatch now carries an owned job value instead of a boxed
closure. Incoming typed header parsing uses the bytes returned during handle
extraction, avoiding a second native data lookup while retaining the raw message
for bad-message reporting. Existing ownership, lock-release, sequence, FIFO and
fallback rules remain. Empty native payloads also safely handle a null byte
pointer with length zero; a null pointer for non-empty data is still invalid.
Six message-read native tests passed on macOS x86_64. These mechanical changes
and correctness checks do not establish a measured p99 improvement.

## Historical 0.2.0 migration

For version `0.2.0`, upgrade the SDK crates and
`mojom-build` together and regenerate protocol bindings. New compact-array AST
variants require low-level exhaustive matches and equality/hash/order handling
to be adapted from `0.1.0`; this is not a Rust source-compatible patch. The
serialization-only codec/derive hook is an internal coordinated interface.
Public named AST conversion and custom Context defaults are retained. The Mojom
wire format, Chromium pin and native C ABI v1 remain unchanged; historical
registry `0.1.0` consumers are used for wire compatibility checks.

Version `0.2.0` added a stack encoding path for outgoing headers, and SDK watchers use a
single-event trap arming helper. Public owned-message and `arm()` interfaces
are preserved. Their timing and tail-latency effects require separate measured
comparisons.

Since `0.2.0`, eligible primary-only interfaces may dispatch at the end of their existing
posted watcher task. This preserves the bound sequence, per-pipe FIFO and
asynchronous receive boundary, but removes the guarantee of an extra second
task turn relative to unrelated runner tasks. Handlers do not run synchronously
on the sending Write call stack. Associated interfaces and ineligible states
retain posted routing; once primary-only eligibility is lost it is not restored.
For `0.2.0`, four native routing tests passed on macOS x86_64. Eight formal comparison runs
completed for that version, but the ABBA and BAAB tail-latency results disagree;
this does not establish that p99 regressions are solved or validate other
platforms.

## Generate a protocol crate

Put shared `.mojom` definitions in a dedicated protocol crate used by both your
client and server. Add these dependencies to its `Cargo.toml`:

```toml
[dependencies]
ipc = { package = "mojo-ipc", version = "=0.3.0" }

[build-dependencies]
mojom-build = "=0.3.0"
```

Generation requires **Python 3.10 or newer**. The parser, generator, templates,
and Python dependencies are bundled; no Chromium checkout or `pip install` is
needed for this step. Set `MOJOM_PYTHON` if the default `python3` (`python` on
Windows) is unsuitable.

For schemas under `schema/`, use this `build.rs`:

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    mojom_build::Config::new()
        .source_root("schema")
        .source("echo.mojom")
        .runtime_crate("ipc")
        .generate()?;
    Ok(())
}
```

The runtime name must match the Cargo dependency alias: `ipc` above. Load the
generated modules at the protocol crate's root, in `src/lib.rs`:

```rust
include!(concat!(env!("OUT_DIR"), "/mojom/mod.rs"));
```

Imports are generated into the same protocol crate. The source repository's
`examples/protocol` contains a complete schema and executable client/server
example, including requests, responses, imported structs, and interface endpoint
transfer. Its example is run from the repository root with
`cargo run -p mojo-example-protocol --example echo` after supplying a native
runtime as described below.

## Supply the native runtime

**The Cargo package does not contain or download the native Mojo Core library.**
Obtain a trusted SDK native library built for this SDK revision and your target
OS and CPU, or build it using the source repository's `native/` GN overlay. Native
production requires the pinned Chromium checkout and platform build dependencies.
Compiling ordinary Rust consumers does not require that checkout.

Load the library explicitly with `unsafe Runtime::load(path, role)`. Use an
absolute trusted path to `libmojo_rs_native.dylib` on macOS,
`libmojo_rs_native.so` on Linux, or `mojo_rs_native.dll` on Windows. Revision and
ABI checks do not make an arbitrary native library safe to load.

The bundled Echo example reads its path from `MOJO_RUNTIME_LIBRARY`:

```sh
export MOJO_RUNTIME_LIBRARY=/absolute/path/to/libmojo_rs_native.dylib
cargo run -p mojo-example-protocol --example echo
```

On Windows, use PowerShell's
`$env:MOJO_RUNTIME_LIBRARY = 'C:\absolute\path\mojo_rs_native.dll'` and the same
Cargo command. The example launches its own participant process. Native libraries
must match the executable's architecture. The current native Windows DLL uses a
static CRT, while a Rust executable built with the default MSVC configuration can
still require the Microsoft VC runtime and UCRT.

## Bind and keep the connection alive

On an ordinary Rust or Tokio thread, pass `Some(runtime.dispatch_runner())` to
`PendingRemote::bind_with_options` or `PendingReceiver::bind_with_options`.
The default `bind()` requires a current Chromium sequence and will panic on an
ordinary Rust thread. Service methods and response/disconnect callbacks run on
the SDK dispatch thread. Keep the returned `Remote` and `Receiver` alive while
using the connection; dropping them closes their endpoints.

Each process initializes one role. A broker uses `spawn_worker` or `listen_once`;
a participant uses `accept_inherited` or `connect`. `accept_inherited` is unsafe:
its token must be the real, unconsumed endpoint transferred by the SDK spawn
helper, and it must be accepted exactly once. `spawn_worker` requires UTF-8
program paths and arguments without interior NUL bytes. Put blocking connection
and child-wait operations on a host blocking thread.

The native image, Mojo Core, and IO/dispatch threads remain alive until process
exit. Dropping `Runtime` does not unload them. There is no public shutdown,
restart, or role-switch operation. Response callbacks follow the upstream
success-only API; disconnect releases pending callbacks and invokes the optional
disconnect handler. These underlying bindings do not provide a Future-based
client API; `mojo-service::CallClient` is a separate optional layer.

## Validation and current scope

The [original v0.3.0 producer](https://github.com/looading/mojo-ipc/actions/runs/39783898906)
at SDK commit `c22e29715d1db80e20297ff9476e4fae7a1a08bb` built all four targets.
Each native job passed native smoke, independently launched named-channel IPC,
Rust typed Echo, the service-network fixture and dependency/export audits.
The [publication recovery](https://github.com/looading/mojo-ipc/actions/runs/39842433958)
reuses those original artifacts; it does not rebuild the native libraries.

| Target | Original native acceptance |
| --- | --- |
| macOS x86_64 | [Native build and acceptance passed](https://github.com/looading/mojo-ipc/actions/runs/39783898906/job/119841954516). |
| Windows x86_64 MSVC | [Native build and acceptance passed](https://github.com/looading/mojo-ipc/actions/runs/39783898906/job/119841954529). |
| Linux x86_64 GNU | [Native build and acceptance passed](https://github.com/looading/mojo-ipc/actions/runs/39783898906/job/119841954285). |
| macOS arm64 | [Native build and acceptance passed](https://github.com/looading/mojo-ipc/actions/runs/39783898906/job/119841954526). |

Local Rust builds use Rust 1.96.0. A lower MSRV has not been established.
The generator currently rejects semantic attributes other than `EnableIf` and
`EnableIfNot`, including `Sync` and `MinVersion`. Custom typemaps, schema imports
across generated Cargo crates, and protocol version negotiation are not exposed.
The source repository's `VALIDATION.md` records the checks and remaining work.

Official source files retain their upstream copyright and BSD license notices.
