# Native builds and prebuilt releases

`workflows/native.yml` builds the official Mojo Core on fresh GitHub-hosted VMs.
It provisions Chromium and its dependencies; no self-hosted runner,
`CHROMIUM_SRC` setting, prewarmed cache, or personal access token is required.

| Rust target | GitHub-hosted runner | Library |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | `ubuntu-24.04` | `libmojo_rs_native.so` |
| `x86_64-apple-darwin` | `macos-15-intel` | `libmojo_rs_native.dylib` |
| `aarch64-apple-darwin` | `macos-15` | `libmojo_rs_native.dylib` |
| `x86_64-pc-windows-msvc` | `windows-2025-vs2026` | `mojo_rs_native.dll` |

## Run a build

After this workflow is available on the repository's default branch, select
**Actions → Native Mojo build and release → Run workflow**. Leave `publish`
unchecked for an acceptance build. Each successful matrix job uploads an
Actions artifact named `mojo-native-package-<target>`, retained for 14 days.

With GitHub CLI:

```sh
gh workflow run native.yml --ref main
```

## Publish a version

Push a new `v<workspace.package.version>` tag containing this workflow and the
release source. Version preflight runs before provisioning any native producer.
All four targets must pass before the publishing job starts. Publishing requires
an explicitly selected matching version tag. The workflow uses its built-in `GITHUB_TOKEN`; only
the publishing job receives `contents: write`.

For example, after preparing the SDK source and its internal Cargo versions for
the next release at `0.2.2`:

```sh
git tag v0.2.2
git push origin v0.2.2
```

Existing `v0.2.0` and `v0.2.1` tags predate this workflow. Use a new version/tag
containing the release changes rather than moving an existing tag. Crate
publication is separate from native Release publication.

A manual dispatch on a matching version tag can also publish:

```sh
gh workflow run native.yml --ref v0.2.2 -f publish=true
```

To rebuild an existing SDK tag with a corrected workflow from `main`, select
`main` under **Run workflow**, set `release_tag` to `v0.2.2` and enable `publish`:

```sh
gh workflow run native.yml --ref main -f release_tag=v0.2.2 -f publish=true
```

Preflight checks the SDK tag's Cargo version and resolves its source commit.
Every producer checks out that commit. Acceptance records preserve both the SDK
commit and the separate workflow commit/ref. Release tooling is checked out
separately at the workflow commit and tested during preflight. The packager's
`--sdk-root` keeps its version, C header and license tied to the validated SDK
checkout, allowing packaging fixes without modifying the SDK tag. Before publication the tag is
resolved again; a moved tag stops publication. A branch dispatch without an
explicit matching tag cannot publish. This mode does not move existing tags.

The publishing job verifies the complete target set, every archive checksum,
all packaged member hashes, and acceptance provenance. It uploads four archives
and `SHA256SUMS` to a draft Release, checks the server's asset hashes, then
publishes. An interrupted upload leaves a draft that can be retried. Existing
published assets are never replaced with different bytes. A prerelease Cargo
version produces a prerelease on GitHub.

## Cold producer setup

`native/scripts/prepare_hosted.py` runs only when `GITHUB_ACTIONS=true` and
`RUNNER_ENVIRONMENT=github-hosted`. Its new producer directory is outside the SDK
checkout, under `RUNNER_TEMP`. It removes known unused preinstalled tools on the
disposable VM; macOS cleanup preserves the selected Xcode. It never cleans a
developer's checkout or an existing producer directory.

It checks out Chromium at
`bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd` and depot_tools at
`3a8af1e64be01a524f7d1c46daf8c2dcac63ff85` (the revision in that Chromium DEPS).
`gclient sync --no-history --shallow` uses Chromium's `small` checkout
configuration, followed by the normal hooks. Optional coverage, telemetry,
OpenXR, PGO and remote execution downloads are disabled. Required source and
prebuilt dependencies still resolve from the pinned DEPS; no hand-selected
replacement for the native dependency graph is used.

Linux installs Chromium build dependencies. macOS selects an installed Xcode
whose SDK contains `DarwinBasic`, `DarwinFoundation1/2/3` and `libunwind` module
maps, sets `DEVELOPER_DIR`/`SDKROOT`, and uses `use_system_xcode=true`. This check
runs before Chromium downloads; the image's default macOS 15.5 SDK does not
contain the Foundation module maps required by this Chromium pin. Windows uses Visual Studio 2026 and installs Microsoft's
SDK `10.0.28000.2270` including debugging tools if its headers are absent. The
installer is pinned by SHA-256; the workflow verifies the installed SDK files.
The SDK is obtained from the
[official Windows SDK downloads](https://learn.microsoft.com/en-us/windows/apps/windows-sdk/downloads).

Native compilation uses `--jobs 2`, disables debug symbols, and uses local Ninja
without remote execution. The workflow copies the pinned GN and Ninja binaries
to an ephemeral tool directory at the front of `PATH`. For compatibility with
older SDK builders, the Ninja executable has an `autoninja` alias. This avoids
depot_tools' uninitialized Python launchers while preserving depot_tools for
other build actions. Before native smoke execution, the workflow adds the build
directory to `LD_LIBRARY_PATH` on Linux or `DYLD_LIBRARY_PATH` on macOS, preserving
existing entries. The smoke executables can then load the adjacent shared library
without depending on a component-build RPATH. Linux also uses `use_glib=false` to avoid requiring
GLib/GObject in consumer hosts. The cold source/toolchain checkout is still large.
Provisioning reports available disk space and requires at least 25 GiB free
after cleanup; every native job has the GitHub-hosted six-hour timeout. There is
no dependency on an Actions cache containing the Chromium checkout.

## Acceptance and package contents

Each target runs Cargo check/tests, the real official Mojom generator boundary
tests, native smoke, the independent-process named-channel fixture, and the Rust
typed parent/child example with the freshly built library. Failed checks prevent
package upload. Available failure fixture logs are uploaded separately.

`package_native.py` validates the pin, native ABI, target, SDK commit, workflow
run, library hash and every acceptance gate. It inspects architecture, exported
symbols and direct/delay-load dependencies using `readelf`, macOS tools, or
`llvm-readobj`. Unexpected exports or non-system dependencies stop packaging.
The supported public export is `MojoRsGetApiV1`; Windows also permits Chromium's
private `GetHandleVerifier`. macOS additionally probes whether loading the
library preserves the host's default malloc zone.

The pinned Chromium licensing tool collects the runtime target's shipped
dependency notices. Its `LICENSES_GN_PATH` hook preserves the overlay's GN graph
root through a small wrapper, tested with the producer's pinned GN. Missing
notices fail the job. The archive
`mojo-native-<version>-<target>.tar.gz` contains a matching top-level directory with:

- The runtime `.so`, `.dylib`, or `.dll`, and `mojo_rs_native.h`.
- `package-manifest.json`, `native-manifest.json` and `acceptance-manifest.json`.
- `dependency-audit.json` and the native tools' `dependency-audit.txt` output.
- `LICENSE`, `CHROMIUM_LICENSE` and `THIRD_PARTY_NOTICES.txt`.

Build trees, native test executables and private fixture logs are excluded from
consumer archives. The Actions artifact also includes an archive `.sha256` file;
the Release combines those hashes into `SHA256SUMS`.

## Download and use

After a successful Release, asset URLs have the form:

```text
https://github.com/looading/mojo-ipc/releases/download/v<version>/mojo-native-<version>-<target>.tar.gz
https://github.com/looading/mojo-ipc/releases/download/v<version>/SHA256SUMS
```

For example, for a released macOS arm64 version:

```sh
version=0.2.2
target=aarch64-apple-darwin
asset="mojo-native-$version-$target.tar.gz"
base="https://github.com/looading/mojo-ipc/releases/download/v$version"
curl -fL "$base/$asset" -o "$asset"
curl -fL "$base/SHA256SUMS" -o SHA256SUMS
awk -v file="$asset" '$2 == file' SHA256SUMS | shasum -a 256 -c -
tar -xzf "$asset"
export MOJO_RUNTIME_LIBRARY="$PWD/mojo-native-$version-$target/libmojo_rs_native.dylib"
cargo run -p mojo-example-protocol --example echo
```

On Linux, use the Linux target and `.so`; checksum verification can use
`sha256sum --check --ignore-missing SHA256SUMS`. On Windows, use the Windows
target, check with `Get-FileHash -Algorithm SHA256`, extract with `tar -xzf`, and
set `$env:MOJO_RUNTIME_LIBRARY` to the extracted `mojo_rs_native.dll`.

Use the matching SDK crate version and retain the included license notices when
redistributing. Native downloads are explicit; Cargo does not automatically
download or load them. Dependency audits record OS/library requirements, not a
promise of compatibility with every older OS. A green workflow also does not
certify arbitrary sandbox/bootstrap namespaces, long-running stress or protocol
compatibility across SDK versions.

## Local verification boundary

Ordinary `cargo.yml` CI and the native preflight run offline release-tooling
tests without downloading Chromium:

```sh
python3 -B -m unittest discover -s native/scripts -p test_release.py -v
```

The workflow has not been executed remotely as part of this change. Offline
tests, actionlint, and a small native inspection fixture validate the release
tooling; actual Mojo build and runtime results require the first hosted matrix
run. Existing historical native validation is documented in `VALIDATION.md`.
