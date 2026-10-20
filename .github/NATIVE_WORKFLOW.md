# Manual native acceptance workflow

`workflows/native.yml` is a manually dispatched configuration. It has not been
executed here and is not evidence that any native target has passed. It neither
creates runners nor uses GitHub-hosted runner labels. A dispatch schedules four
existing self-hosted runners:

| Rust target | Required runner labels |
| --- | --- |
| `x86_64-unknown-linux-gnu` | `self-hosted`, `Linux`, `X64` |
| `x86_64-apple-darwin` | `self-hosted`, `macOS`, `X64` |
| `aarch64-apple-darwin` | `self-hosted`, `macOS`, `ARM64` |
| `x86_64-pc-windows-msvc` | `self-hosted`, `Windows`, `X64` |

No remote repository or runner registration is assumed. An unavailable matching
runner leaves its job queued; the workflow does not provision a replacement.
Use trusted, appropriately permissioned runners that already support checkout,
setup-python, and composite Bash actions (including Git Bash on Windows).

The separate `cargo.yml` workflow also has a `windows-cross` job on Ubuntu with
cargo-xwin 0.23.1. It compiles and links all Rust Windows targets without a
Windows runner. It does not build the Chromium GN DLL or run Windows executables.
The native producer supports a separate macOS/Linux-to-Windows build-only path;
its SDK/toolchain setup is documented in `native/README.md`. The real Windows
runtime gates below remain required, regardless of the build host used.

Before dispatch, configure each runner service's environment:

* `CHROMIUM_SRC` is an absolute path to a complete Chromium `src` checkout at
  `bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`. Keep it outside the Actions SDK
  workspace, with neither directory nested in the other. A guard checks this
  before `actions/checkout` can clean or replace workspace contents.
* The checkout's DEPS, downloaded Clang, platform SDKs and normal Chromium build
  dependencies have already been installed for that host architecture.
* `git`, `gclient`, `gn`, and `autoninja` or `ninja` are available through `PATH`,
  using the checkout's configured `depot_tools` environment. Preserve any local
  platform toolchain settings required by the normal Chromium build.
* The runner can install Python 3.12 and Rust 1.96.0 through the setup actions and
  has sufficient disk space, memory, tool-cache permissions, and network access
  for those tools and locked Cargo dependencies.

After the workspace guard, the prerequisite step checks the host OS/CPU, source
markers, downloaded compiler, tools, and exact Chromium HEAD before Rust
installation or native compilation. It fails with explicit missing-environment
diagnostics. It does not clone Chromium,
run `gclient sync`, reset the checkout, or supply missing platform SDKs. The native
builder installs its marked overlay into `CHROMIUM_SRC/sdk/mojo_rs/native` and
uses a target-specific output directory; existing checkout ownership is subject
to that builder's checks.

Each scheduled job runs Cargo checks/tests, the real official Mojom boundary
tests, native `build_native.py --smoke`, the two independently launched processes
in `named_smoke.py`, and then the Rust parent/child typed IPC example with the
freshly copied `.so`, `.dylib`, or `.dll`. A failed gate prevents the tested-native
artifact upload. Successful runs upload `dist/<target>/` containing the native
library, C header, `native-manifest.json`, named-fixture logs, `typed-echo.log`, and
a separate `acceptance-manifest.json` recording that workflow run and its
completed gates. Available fixture logs from a failed job are uploaded separately
as a diagnostics artifact; they do not constitute an acceptance receipt.

The workflow validates its native manifest and checksum before starting Rust.
Its green result covers the encoded smoke, local named-channel fixture, and
typed parent/child scenarios. It does not certify arbitrary launchd/sandbox/user
namespace deployments, long-running stress, protocol compatibility across SDK
versions, or the final OS runtime-dependency audit.
