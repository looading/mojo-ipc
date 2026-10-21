# Native prebuilt library releases

## Approved direction

Build the existing four native targets in GitHub Actions, run native acceptance,
and distribute versioned runtime archives with headers and SHA-256 checksums
through GitHub Releases. The repository and this direction were confirmed in
the conversation on 2026-10-21.

## Build environment

Reuse the existing `native.yml` matrix and its self-hosted runners: Linux x64,
macOS x64, macOS arm64, and Windows x64. Each runner requires a complete Chromium
checkout at `bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`, its platform SDKs,
depot_tools, and native inspection tools. Runner provisioning remains separate
from the SDK release workflow. An unavailable runner will leave a job queued.

The alternatives are to provision Chromium on GitHub-hosted runners for every
release, which requires a separate solution for checkout size and platform
toolchains, or publish Actions artifacts alone, which would retain the existing
temporary-download workflow. Reusing the native producers preserves their
current environment contracts and adds persistent versioned downloads.

## Workflow and release contract

- A pushed `v<workspace.package.version>` tag triggers the four-platform build.
  Reject a mismatched version before scheduling expensive native work.
- Preserve manual dispatch for acceptance and downloadable packages without
  publishing a release from a branch.
- Keep the existing Cargo, Mojom generator, native smoke, independent-process
  named-channel, and Rust typed parent/child acceptance gates.
- Audit the freshly built library's target architecture, public ABI entry point,
  and native runtime dependencies. Reject unexpected non-system libraries and
  record the inspection output. Collect Chromium and required third-party
  license notices with the pinned checkout's licensing tooling.
- Package only the runtime library, C header, provenance/acceptance/audit
  manifests, and license notices. Exclude build trees and diagnostic files that
  may be left over from previous self-hosted runs.
- Name each archive `mojo-native-<version>-<rust-target>.tar.gz`. Include the SDK
  version and commit, Chromium pin, ABI version, target, library hash, and
  completed acceptance gates in the package metadata.
- Upload packages as Actions artifacts. A separate GitHub-hosted publishing job
  requires all four producers to succeed, validates archive checksums and the
  expected complete target set, and uploads the four archives plus a combined
  `SHA256SUMS` file to the matching GitHub Release.
- Give release write permission only to the publishing job. Never replace an
  existing public asset with different bytes. Failed or partial builds must not
  create a complete-looking public release.

## Consumer contract

Document versioned asset URLs, checksum verification, archive extraction, and
setting `MOJO_RUNTIME_LIBRARY` to the extracted `.so`, `.dylib`, or `.dll`.
Rust crate publishing and automatic runtime downloading in Cargo remain outside
this change. Platform requirements recorded by each producer remain visible in
its metadata; a successful build does not imply compatibility with older OSes.

## Verification

Validate the workflow with actionlint. Exercise packaging with temporary native
artifact fixtures, including valid archives, version/target/checksum mismatches,
missing or failed acceptance, and unexpected dependencies. Check the archive
member list and recompute the distributed archive checksums. Run the real native
inspection path locally where the host and a native library are available.

The local checkout has no native artifacts, complete Chromium checkout, or
GitHub CLI. Local checks therefore cannot establish that the remote native
matrix or Release upload has run. Report that boundary explicitly; actual
release acceptance comes from the first successful workflow run.
