# Native Mojo runtime

This is a GN overlay for Chromium commit
`bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`. It wraps the actual upstream
Mojo Core; it does not substitute an in-process queue or a custom RPC transport.
Rust loads `MojoRsGetApiV1`, the SDK's versioned C ABI entry point, and calls the C API table in
`include/mojo_rs_native.h`.

The shared library is deliberately used as the native boundary. Chromium's
private C++, base, allocator and standard-library objects stay inside it. Only
plain C values, caller-owned buffers and opaque retained pointers cross the
boundary. The API exposes the official `MojoSystemThunks2` after initialization;
its declarations come from the same pinned public C headers as the runtime.

The producer configuration disables Chromium's process-wide allocator shim and
PartitionAlloc-as-malloc, plus the dependent BackupRefPtr and alloc-token modes.
It keeps PartitionAlloc available for explicit internal uses. This matters when
loading the SDK into an arbitrary host: the pinned macOS shim installs a default
malloc zone from a library constructor, even with a restricted export list.
The GN target rejects an allocator-shim configuration. Each release must still
audit exports, dependencies and host allocator behavior after dynamic loading.

## Build in an existing Chromium checkout

For clean GitHub-hosted builds and versioned prebuilt downloads, see
[the native release workflow](../.github/NATIVE_WORKFLOW.md). That workflow
provisions the producer environment, runs acceptance, audits dependencies and
exports, collects license notices, and publishes the complete platform matrix.
The local builder below continues to use an explicitly provided checkout.

Install and initialize Chromium's normal development dependencies and
`depot_tools` separately. This script never clones Chromium, resets its revision,
runs `gclient sync`, or downloads toolchains on its own.

```sh
# Install the overlay only; do not configure or build.
python3 native/scripts/build_native.py --chromium-src /path/to/chromium/src
# Configure GN and build the shared library.
python3 native/scripts/build_native.py --chromium-src /path/to/chromium/src --build
# Configure, build the executable/library, then run the native integration smoke.
python3 native/scripts/build_native.py --chromium-src /path/to/chromium/src --smoke
```

The installer checks `git rev-parse HEAD` against the pinned commit, copies this
directory to `//sdk/mojo_rs/native`, and stops there unless `--build` or `--smoke`
was requested. `--build` additionally runs GN with a non-component release
configuration and builds `//sdk/mojo_rs/native:runtime`. `--smoke` includes the
native build and runs `:runtime_smoke`. An existing unrelated overlay directory
is not overwritten.

The script passes GN's global `--root-target=//sdk/mojo_rs/native:runtime`
option (or `:runtime_smoke`) to load the overlay, plus a matching `--root-pattern`
to retain only that target and its transitive dependencies. Both switches exist
in the GN revision pinned by this checkout. It does not add the SDK to Chromium's
normal root `BUILD.gn` or generate the browser's default target graph. The
selected root is recorded in an exported artifact's manifest.

`--gn-args` supplies explicit additional GN configuration; `--gn-args-file`
loads saved arguments before those command-line additions. `--out` chooses the
output directory. `--jobs N` limits local Ninja parallelism; when omitted the
existing Ninja/autoninja default is retained. `--target-os linux|mac|win` defaults to the host OS;
`--target-cpu x64` or `--target-cpu arm64` selects the CPU. macOS supports both,
while release v1 Linux/Windows target x64. CPU defaults to the host CPU, or x64
for a cross-OS Windows build. Set these through the dedicated flags; assignments
to `target_os`/`target_cpu` in `--gn-args` are rejected. `--artifact-dir`
copies the library, C header and a checksum
manifest after a successful build. These options do not replace the required
platform toolchain setup or the final runtime dependency audit.

On macOS, `--smoke` first asks `/usr/bin/arch` to execute a small system binary
in the selected target architecture. This checks execution capability, including
Rosetta, before the expensive build. Use `--build` for an architecture the
current Mac cannot execute; its smoke must run on a capable runner.

### Preparing the producer checkout

Use the pinned Chromium revision and its matching DEPS. Before `gclient sync`,
the solution's `.gclient` `custom_vars` can use this development configuration:

```python
"custom_vars": {
    "checkout_configuration": "small",
    "checkout_js_coverage_modules": False,
    "checkout_openxr": False,
    "checkout_pgo_profiles": False,
    "checkout_telemetry_dependencies": False,
    "download_remoteexec_cfg": False,
    "download_reclient": False,
},
```

`small` omits optional development/test assets; the other entries avoid coverage,
VR, PGO, telemetry and remote-build downloads unused by this target. Several are
already false by default. Android and Fuchsia are also disabled by default;
only add another target OS when provisioning its cross-build dependencies.
Keep the normal `non_git_source`
setting: it controls required prebuilt packages, including the pinned toolchains.

Run normal dependency synchronization and hooks before invoking this overlay.
They provide `build/config/gclient_args.gni`, hermetic Python, GN, Clang, Rust,
libc++ and other build inputs. A source-only checkout or only fetching `base/`
and `mojo/` is insufficient. For example, Linux's named-channel target also
depends on `//net`; the build graph, not a hand-picked directory list, determines
the required third-party sources. This pin defaults to C++23 and a matching
Chromium Clang toolchain; the earlier system-compiler syntax checks do not
replace a build with those tools.

For a local, non-official macOS build using installed Xcode, pass
`--gn-args='use_system_xcode=true'`. This pin accepts a macOS SDK of at least 15
for that configuration; its default deployment target is macOS 13.0. The
separate official-build SDK selection is 27.0 and is not required by this
overlay's ordinary release configuration. Local and CI machines still need
the platform development dependencies specified by Chromium. Keep remote
execution disabled unless that builder has an explicitly configured service.

Supported release targets are intended to be Linux x86_64 GNU, macOS arm64 and
x86_64, and Windows x86_64 MSVC. Every target requires its own native runtime
acceptance; source checks or cross-compilation do not establish that IPC works
on that OS.

### Windows builds from Linux or macOS

The pinned Chromium toolchain supports Linux/macOS hosts targeting Windows.
After separately provisioning the complete Windows toolchain and target DEPS,
the overlay has this build-only entry point:

```sh
python3 native/scripts/build_native.py \
  --chromium-src /path/to/chromium/src \
  --target-os win --target-cpu x64 --build \
  --artifact-dir dist/x86_64-pc-windows-msvc
```

The default GN output is `out/mojo-rs-win-x64`; packaging expects
`mojo_rs_native.dll` and records `"os": "win"` in its manifest. A cross-OS
`--smoke` request fails before the installer touches the checkout, because the
Windows executable must be exercised on a Windows runner. Other cross-OS
combinations are rejected. macOS cross-architecture builds retain their existing
execution-capability check.

`cargo-xwin` can provision Microsoft headers and libraries for Cargo, but it does
not configure Chromium's separate GN toolchain. The independent
`scripts/prepare_windows_toolchain.py` adapter takes genuine CRT/SDK directories
and creates Chromium's winsysroot layout and environment metadata. It checks the
actual SDK's `NTDDI_WIN11_BR`, derives the CRT version from `crtversion.h`, and
checks COFF machine types in the real `libcmt`, `kernel32` and `libucrt` archives
for both x86 and x64. Mojo's `base` dependency also requires real ATL: before
creating an output, the adapter checks `atldef.h`, `atlbase.h` and each
architecture's `atls.lib`, including its COFF machine type. It does not create
or change SDK headers or libraries.

For the Microsoft `Microsoft.Windows.SDK.CPP`, `.x64` and `.x86` NuGet packages,
the following uses their extracted original directories alongside an xwin CRT
cache provisioned with ATL and genuine libraries for both architectures:

```sh
python3 native/scripts/fetch_windows_sdk.py --output /path/to/windows-cross
XWIN_ARCH=x86,x86_64 XWIN_INCLUDE_DEBUG_SYMBOLS=true \
  XWIN_CACHE_DIR=/path/to/windows-cross/xwin-atl-cache \
  cargo xwin cache xwin --xwin-include-atl
SDK_PACKAGES=/path/to/windows-cross/sdk-nuget
CRT_INPUT=/path/to/windows-cross/xwin-atl-cache/xwin/crt
WIN_SYSROOT=/path/to/windows-cross/winsysroot
CHROMIUM_SRC=/path/to/chromium/src
python3 native/scripts/prepare_windows_toolchain.py \
  --sdk-include-dir "$SDK_PACKAGES/microsoft.windows.sdk.cpp/c/Include/10.0.28000.0" \
  --sdk-um-lib-x64 "$SDK_PACKAGES/microsoft.windows.sdk.cpp.x64/c/um/x64" \
  --sdk-ucrt-lib-x64 "$SDK_PACKAGES/microsoft.windows.sdk.cpp.x64/c/ucrt/x64" \
  --sdk-um-lib-x86 "$SDK_PACKAGES/microsoft.windows.sdk.cpp.x86/c/um/x86" \
  --sdk-ucrt-lib-x86 "$SDK_PACKAGES/microsoft.windows.sdk.cpp.x86/c/ucrt/x86" \
  --crt-include-dir "$CRT_INPUT/include" \
  --crt-lib-x64 "$CRT_INPUT/lib/x86_64" --crt-lib-x86 "$CRT_INPUT/lib/x86" \
  --clang-cl "$CHROMIUM_SRC/third_party/llvm-build/Release+Asserts/bin/clang-cl" \
  --output "$WIN_SYSROOT"
```

The fetcher pins servicing version 10.0.28000.2270 and verifies the three official
NuGet packages against fixed SHA-512 hashes and their registration/catalog
metadata. It downloads only the SDK inputs. The cargo-xwin cache command above
uses version 0.23.1 and explicitly includes ATL; its default SDK cache is unused
by this native producer. Provision Chromium Windows compiler-rt and host RC
separately as described below.

Keep `XWIN_INCLUDE_DEBUG_SYMBOLS=true`: the default GN symbol level links PDB
debug information, and Chromium treats missing CRT type-server PDB warnings
(LNK4099) as errors. The matching PDBs must remain next to their CRT libraries
or beside the DLL output; LLVM checks that they match the library objects.
The local cross-build uses genuine matching CRT PDBs and retains linker warning
checks.

If the primary CRT lacks ATL, add `--atl-include-dir`, `--atl-lib-x86` and
`--atl-lib-x64` to the prepare command, pointing at a separate genuine ATL
cache. Both separate headers and xwin's merged CRT/ATL layout are supported.
For a root prepared before ATL was available, stop its build and append ATL
without recreating the root:

```sh
ATL_INPUT=/path/to/windows-cross/xwin-atl-cache/xwin/crt
python3 native/scripts/prepare_windows_toolchain.py add-atl \
  --winsysroot "$WIN_SYSROOT" --atl-include-dir "$ATL_INPUT/include" \
  --atl-lib-x86 "$ATL_INPUT/lib/x86" --atl-lib-x64 "$ATL_INPUT/lib/x86_64"
```

This creates `VC/Tools/MSVC/<version>/atlmfc/include` and `atlmfc/lib/<arch>`
symlink views and appends their SetEnv entries. The views exclude files already
present in the original CRT, so a merged ATL cache cannot introduce a second
copy of CRT headers into Clang's module file identities. The pinned compiler and
linker automatically search these `atlmfc` directories through `/winsysroot`.
The existing CRT/SDK directories are untouched; an existing `atlmfc` directory
is never overwritten. Keep the additional ATL inputs unchanged for the build.
Local checks confirmed that the append operation preserved existing CRT, SDK
and PCM file identities, and the pinned Clang successfully preprocessed ATL and
`io.h` using the ATL delta plus the original CRT paths.

The adapter output must be new. It contains symlinks to the inputs, so keep those inputs
available and unchanged throughout GN compilation; replacing a header directory
after Clang has generated modules can invalidate file identities in its PCM cache.
No binaries or local paths belong in the SDK source tree. A generated
`adapter-manifest.json` records versions, selected source hashes and architecture
checks. `gn-args.gn` uses the real toolset generation (2022 for CRT 14.44) rather
than claiming the toolchain is Chromium's VS 2026 package. The optional
`--visual-studio-version` changes that generation for another genuine toolset.
An Apple Silicon producer also needs all three optional `*-arm64` library inputs,
because pinned GN evaluates the host arm64 toolchain metadata even when the
Windows target is x64. On Linux, Microsoft header lookup also needs a
case-insensitive filesystem/view or a correctly case-aliased input tree, as in
Chromium's documented packaged-toolchain setup. The adapter preserves the input
tree's names.

The layout uses paths such as
`VC/Tools/MSVC/<version>/include`, `VC/Tools/MSVC/<version>/lib/x64`, and
`Windows Kits/10/Include/<sdk-version>/{shared,um,ucrt}` with their corresponding
`Lib/<sdk-version>/{um,ucrt}/x64` directories. In the packaged-toolchain mode,
`Windows Kits/10/bin/SetEnv.x86.json` and `SetEnv.x64.json` describe those paths;
GN evaluates both environment descriptions even for an x64 target. The setup
also locates `cl.exe` in the described PATH, although target compilation uses
Chromium's host-native `clang-cl` and `lld-link`. The adapter supplies a symbolic
link from `cl.exe` to the real host compiler for this discovery step. Setting
`DEPOT_TOOLS_WIN_TOOLCHAIN=0` alone is insufficient
on a Mac/Linux host: that path tries to run Visual Studio's `vcvarsall.bat`.

The pinned explicit-path branch has a GN argument-type defect: it sets runtime
directories to an empty list, then passes that value as one `exec_script`
argument, which requires a string. The adapter emits `chromium-compat.patch`,
changing that value to an empty string only in the explicit Windows toolchain
branch. It does not apply the patch or write `build/win_toolchain.json`. Apply
the patch once to the matching pinned checkout, then build:

```sh
git -C "$CHROMIUM_SRC" apply --check "$WIN_SYSROOT/chromium-compat.patch"
git -C "$CHROMIUM_SRC" apply "$WIN_SYSROOT/chromium-compat.patch"
DEPOT_TOOLS_WIN_TOOLCHAIN=1 python3 native/scripts/build_native.py \
  --chromium-src "$CHROMIUM_SRC" --target-os win --target-cpu x64 --build \
  --gn-args-file "$WIN_SYSROOT/gn-args.gn" \
  --artifact-dir dist/x86_64-pc-windows-msvc
```

On a macOS producer using installed Xcode, also pass
`--gn-args='use_system_xcode=true'` for the host-side build tools.

`wdk_path` points at the SDK root to satisfy this pin's explicit-path contract.
The selected user-mode Mojo graph does not consume WDK headers, and the manifest
explicitly records that no WDK is provided. This configuration does not support
kernel-mode targets. Keep `is_component_build=false` and the release static CRT;
an independently configured Chromium `build/win_toolchain.json` can cause the
upstream runtime-DLL-copy hook to select a different installation.

This pin requires Windows SDK **10.0.28000.0 or newer** (`NTDDI_WIN11_BR`), and
its packaged toolchain selects 10.0.28000.0 with Visual Studio 2026. Do not assume
an xwin default SDK is new enough or that its catalog offers this version.
The matching Chromium Windows compiler-rt package and host-native resource
compiler are separate requirements. Follow the pinned
[Windows cross-build setup](https://chromium.googlesource.com/chromium/src/+/bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd/docs/win_cross.md)
to add `target_os = ['win']` to the producer's `.gclient`, then sync/run the
corresponding hooks. DEPS selects the Windows Clang runtime libraries and
`rc_mac`/`rc_linux`; `rc.py` uses those host-native resource compilers. Existing
Mac-only dependencies do not contain all of these inputs. This installer neither
downloads them nor changes `.gclient` or the process environment.

The adapter was executed locally with genuine SDK 10.0.28000.0 headers/libraries
(NuGet servicing version 10.0.28000.2270) and CRT 14.44.35220. Both architectures
passed the archive checks and the pinned official `setup_toolchain.py`. The
compatibility patch passed `git apply --check`; a separate minimal invocation of
the pinned GN reproduced the original list/string error. On 2026-10-21 the
actual macOS x64-to-Windows x64 GN build and final DLL link passed, followed by a
zero-step incremental build. This used the real SDK, CRT, ATL and matching CRT
PDBs, without suppressing missing-PDB linker warnings or disabling compiler
checks. The 2,537,984-byte DLL and manifests are in
`dist/x86_64-pc-windows-msvc/`. PE inspection confirmed AMD64/DLL headers, the
SDK getter and upstream private verifier exports, and only Windows system DLLs
in the static import table. Windows execution and IPC acceptance remain pending.
For the precise contracts, see the
pinned [toolchain setup](https://chromium.googlesource.com/chromium/src/+/bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd/build/toolchain/win/setup_toolchain.py),
[SDK selection](https://chromium.googlesource.com/chromium/src/+/bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd/build/vs_toolchain.py),
and [minimum SDK check](https://chromium.googlesource.com/chromium/src/+/bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd/base/win/windows_version.cc).

| Platform | Output | Spawn bootstrap | Independent process bootstrap |
| --- | --- | --- | --- |
| Linux | `libmojo_rs_native.so` | `base::LaunchProcess` and explicit fd mapping | Named Unix socket channel |
| macOS | `libmojo_rs_native.dylib` | Chromium's Mach-port rendezvous through native launch helper | Named Mach bootstrap channel; launchd `MachServices` deployment is also supported |
| Windows | `mojo_rs_native.dll` | Native process launch with explicit HANDLE inheritance | Named pipe; supply the real client PID so Mojo can duplicate handles |

On macOS the upstream `NamedPlatformChannel` checks in a Mach bootstrap service
and clients look it up in their bootstrap namespace. A local OS-level probe
(`tests/macos_checkin_probe.c`) confirmed that an ordinary unsandboxed CLI server
can check in a fresh name and another process can look it up on the development
machine. This is an OS contract probe, not evidence that the full Mojo native
smoke test has run. Namespace/OS policy can reject dynamic check-in; a service
may instead be deployed as a launchd user agent with its name in `MachServices`.
Sandboxed applications and connections across user/bootstrap namespaces require
a separate deployment policy. The parent/child path does not need this named
service deployment.

### Independent-process smoke on each platform

After building `runtime_smoke`, run the named-channel test separately:

```sh
python3 native/scripts/named_smoke.py \
  --executable /absolute/path/chromium/src/out/mojo-rs-mac-x64/runtime_smoke \
  --work-dir /absolute/path/native-named-smoke-logs
```

Use the corresponding Linux path or Windows `runtime_smoke.exe` on those
runners. This wrapper launches the server and client as two sibling processes,
waits for server readiness, then atomically supplies the client's actual PID.
It requires both exit codes to be zero and the server's confirmed message-pipe
and shared-buffer exchange. `--timeout` controls one total deadline (30 seconds
by default); cleanup terminates remaining fixture processes. A unique run
directory retains four stdout/stderr logs and `result.json`, including PIDs,
exit codes and the result. Linux uses a private short `/tmp` socket path; macOS
uses a unique Mach service name; Windows uses a unique named-pipe server name.
The macOS CLI test depends on the bootstrap namespace allowing dynamic check-in.
Use the separate launchd fixture when the deployment policy requires it.

### macOS independent-process fixture

`tests/launchd/com.example.mojo-rust-sdk.smoke.plist` is the minimal template.
Its absolute executable/work paths must be replaced before use. The automated
fixture generator performs those replacements, gives each run a unique label,
and uses the same name for the launchd job and `MachServices` entry:

```sh
python3 native/scripts/launchd_smoke.py \
  --executable /absolute/path/chromium/src/out/mojo-rs-mac-arm64/runtime_smoke \
  --work-dir /absolute/path/chromium/src/out/mojo-rs-mac-arm64/launchd-smoke
```

This default operation only writes a plist and prints the commands. Add `--run`
to register the job in `gui/<current uid>`, launch an independent client, run the
pipe/shared-buffer exchange, and unregister the unique job in a `finally` block.
It needs a logged-in GUI bootstrap domain and no root privileges. The script
does not copy anything into `~/Library/LaunchAgents`. Logs remain under the
chosen work directory. A cleanup failure fails the test and prints the exact
`launchctl bootout` command needed to remove that fixture.

The PID file in this test is a separate fixture control channel: it supplies the
client PID before the broker sends its invitation, so macOS handle transfer has
the correct target process. It is not a substitute for Mojo message transport or
an authentication protocol. Applications must likewise supply the real peer PID
through their launch/deployment protocol.

This deployment contract follows the upstream Mac channel's
`bootstrap_check_in`/`bootstrap_look_up` implementation and macOS
`launchd.plist(5)`'s `MachServices` check-in contract. See also Apple's
[launchd guide](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/CreatingLaunchdJobs.html).

## Lifecycle and ownership

* Initialize exactly one broker or participant per process. Repeating an
  identical initialization succeeds; changing roles fails. Multiple Rust
  `Runtime` handles refer to this one native runtime.
* Initialization creates a dedicated IO thread and a separate dispatch thread.
  It does not install a host main-thread loop or require Tokio.
* Native Core, base state and task threads live until process exit. There is no
  unload, restart or Core shutdown operation in ABI v1. Rust must keep the loaded
  library alive for the lifetime of any handle, task runner or callback.
* `runner_post` transfers the context when both callbacks are valid. Exactly one
  callback consumes it: `run` if executed, `cancel` if rejected/discarded. A failed
  post can synchronously invoke `cancel`; the caller must not free it again.
* Every runner getter returns one retained reference. Release each such reference
  once. The current-runner query outside a native base sequence returns null.
* `spawn_child` owns all OS handle-passing details. Its returned bootstrap pipe
  is an owned Mojo handle; the child obtains its endpoint token from
  `--mojo-rust-sdk-bootstrap=TOKEN`. If process launch succeeds but invitation
  submission fails, the helper terminates that newly launched child.
* A successful invitation submission does not prove the child accepted it.
  The typed layer must perform its SDK/protocol handshake and apply a deadline.
* Accept/connect/wait operations are synchronous. Calling them on native IO or
  dispatch workers is rejected. Async hosts must put them on a blocking worker.
* A named listener accepts one connection. Release it after use; repeated
  independent clients require a new listener. Concurrent release/use of the same
  opaque C object is not supported.
* `child_release` only closes the native process handle. It does not terminate
  a running child. A `child_wait` timeout leaves the child alive.

## Validation

`runtime_smoke` exercises API layout/version checks, initialization role checks,
native sequence identity, accepted and rejected task ownership, and real native
parent/child communication. The parent transfers a message-pipe endpoint and a
shared-buffer handle; the child replies over the transferred pipe and changes
the shared mapping. The parent verifies the changes and waits for child exit.
The client waits for the broker's acknowledgement before closing; the broker
waits for that close before exiting. This explicitly observes completion across
the transport instead of relying on process teardown to flush queued writes.

The pinned macOS x64 build was verified on 2026-10-20 with Chromium Clang 24 and
the installed Xcode macOS SDK 26.5: normal `gclient runhooks`, GN generation, the
shared-library build, `runtime_smoke` and a repeated incremental build all
succeeded. `named_smoke.py` also passed with two separately launched sibling
processes, transferring both a message-pipe endpoint and shared memory through
the dynamic Mach bootstrap channel. Both processes exited successfully.

For that macOS x64 artifact, `nm` showed only `MojoRsGetApiV1` exported, `otool`
showed only system dependencies and a macOS 13.0 deployment target, and loading
the library into a Python host preserved the default malloc-zone address.
These are local macOS x64 runtime results. Windows x64 has separately passed a
real GN cross-build, final link and PE/static-import inspection on this Mac;
its DLL and IPC tests have not run on Windows. Linux and macOS arm64 still
require native builds and runtime checks; the launchd fixture has not been run.

For another target, build and run the test inside a complete pinned Chromium
checkout before claiming native validation. Before releasing each artifact, inspect its exported
symbols and OS dependencies (`nm`/`otool`/`dumpbin` as appropriate), and exercise
the independent-launch listener on that platform. Do not infer those results
from Cargo-only tests.

The overlay requires `is_component_build=false`. `MojoRsGetApiV1` is the only
supported SDK ABI export. The Windows DLL also retains Chromium's private
`GetHandleVerifier` export for its upstream handle-verifier implementation;
that function is not part of the SDK contract. The export audit permits this
known Windows symbol and rejects unexpected exports. The upstream `Mojo*` and
ordinary C++ symbols remain private. Static archive
distribution is outside v1: Chromium's normal thin archives and individual
component outputs are not standalone native SDK artifacts.
