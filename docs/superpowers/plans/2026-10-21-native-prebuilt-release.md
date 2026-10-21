# Native prebuilt release implementation plan

> **For agentic workers:** Execute this plan task-by-task in the current session.

**Goal:** Build and distribute the four native Mojo runtime libraries from clean
GitHub-hosted runners.

**Architecture:** Extend the existing native acceptance workflow with producer
provisioning and versioned archives. A separate hosted publishing job waits for
the complete matrix, verifies packages, and publishes a GitHub Release.

**Tech Stack:** GitHub Actions, Python 3.12 standard library, Chromium GN/Ninja,
Rust 1.96.0, native platform inspection tools, GitHub CLI on the publishing VM.

## Global constraints

- Chromium pin: `bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`; native ABI 1.
- Targets: Linux x64 GNU, macOS x64/arm64, Windows x64 MSVC.
- Use GitHub-hosted runners; no preconfigured checkout, runner, or secret needed.
- Tag must equal `v<workspace.package.version>` before native jobs begin.
- Preserve real Cargo/Mojom/native/named-channel/typed IPC acceptance.
- Publish only after every target passes; package notices and SHA-256 checksums.
- Validate configuration and offline failure cases; remote execution is a
  separate evidence boundary and must be reported honestly.

## Provision the hosted producers

**Files:** `native/scripts/prepare_hosted.py`,
`native/scripts/build_native.py`, `native/scripts/test_release.py`.

**Interfaces:** `prepare_hosted.py --root <runner-temp>/mojo-chromium` prepares
`src/` and `depot_tools/`, then exports paths through `GITHUB_ENV`/`GITHUB_PATH`.
`build_native.py --jobs 2` bounds Ninja parallelism.

- [x] Add a hosted-only cleanup guard, architecture validation, pinned shallow
  depot_tools/Chromium checkout, `.gclient` small configuration, platform
  dependency setup, and revision validation. Use the pinned Windows SDK
  installer with a checked SHA-256 and verify installed headers/debugging tools.
- [x] Add positive integer `--jobs` to the existing builder, forwarding it to
  Ninja without changing the default behavior of local builds.
- [x] Exercise generated `.gclient` configuration, refusal to clean non-hosted environments, and preservation of the standalone
  graph root with temporary directories and a real GN fixture. Do not download Chromium in local tests.

Run:

```sh
python3 -B -m unittest discover -s native/scripts -p 'test_release.py' -v
python3 native/scripts/prepare_hosted.py --help
python3 native/scripts/build_native.py --help
```

Expected: offline checks pass and both CLIs describe the supported arguments.

## Audit and package accepted libraries

**Files:** `native/scripts/package_native.py`,
`native/scripts/release_native.py`, `native/scripts/test_release.py`.

**Interfaces:** `package_native.py --artifact-dir <dir> --chromium-src <src>
--out <GN-out> --target <triple> --output-dir <packages>` audits a fresh library,
collects notices, and writes one archive and its checksum. `release_native.py
--package-dir <packages> --tag <v-version>` verifies the complete set and invokes
GitHub CLI for publication. Metadata binds acceptance to the SDK commit and run.

- [x] Reject failed acceptance, mismatched target/ABI/pin/hash/commit, unexpected
  exports and non-system dependencies. Record native inspection and compatibility
  details. Generate third-party notices with the pinned Chromium license tool.
- [x] Archive an explicit member allowlist under a versioned top-level directory;
  write per-archive SHA-256. Include a package manifest, native/acceptance metadata,
  C header, SDK license, Chromium license and generated dependency notices.
- [x] Verify exactly four archives and their receipts/checksums before creating
  a release. Upload all assets to a draft before publishing. Reject an existing
  published release with conflicting assets; permit safe retry of a draft.
- [x] Cover all platforms with inspection-output fixtures and exercise actual
  tar creation, member checks, corruption, stale/failed acceptance and partial
  matrix rejection. Mock only native tools and GitHub operations.

Run:

```sh
python3 -B -m unittest discover -s native/scripts -p 'test_release.py' -v
```

Expected: valid packages round-trip and invalid evidence never reaches upload.

## Connect Actions and document consumption

**Files:** `.github/workflows/native.yml`, `.github/workflows/cargo.yml`,
`.github/NATIVE_WORKFLOW.md`, `README.md`, `native/README.md`.

- [x] Add version preflight, four hosted labels, provisioning, bounded native
  build, audit/package artifact upload, and a least-permission publishing job.
  Keep manual dispatch available; branch dispatch does not publish a release.
- [x] Run the offline release checks in ordinary Cargo CI, so validation does
  not depend on an expensive Chromium build. Add failure diagnostics and a job
  summary with downloads/target/provenance information.
- [x] Document cold-run behavior, SDK setup, version tags, package layout,
  manual dispatch, downloads, checksum verification, and runtime configuration.
  Replace obsolete claims that only self-hosted/manual acceptance is supported.
- [x] Run actionlint, Python checks, `git diff --check`, and review the final
  diff against the approved hosted-runner requirement.

Run:

```sh
/tmp/mojo-native-release-tools/actionlint .github/workflows/native.yml .github/workflows/cargo.yml
python3 -B -m unittest discover -s native/scripts -p 'test_release.py' -v
git diff --check
```

Expected: no workflow/static errors and passing offline release checks. Request
an independent code review before claiming completion; fix material findings.

## Validation record

- 19 release-tooling tests passed with `MOJO_TEST_GN` set to an official GN binary.
  This includes a real unreachable-overlay regression and an actual compiled
  Mach-O inspection/allocator-probe fixture. Neither fixture substitutes for Mojo.
- actionlint 1.7.12 passed for both workflows.
- All six Python scripts parsed with Python 3.12 grammar; CLI help checks passed.
- `git diff --check` passed.
- Independent review identified and verified fixes for lost GN roots in license
  scanning, release retry provenance/artifact replacement, and Linux GLib linkage.
- Remote hosted Chromium compilation, Mojo runtime acceptance and Release upload
  were not run from this environment. Their result must come from Actions.
