# Local Service Network Implementation Plan

> Execute inline with `executing-plans`; request an independent review before completion.

**Goal:** Register, discover and connect application services through Mojo, including explicit recovery after provider restart.

**Architecture:** Add a standalone control-protocol crate with a default-deny host policy, an owned registry and callback clients. Exercise it with a broker and two participant applications using individual one-shot named bootstrap listeners and host-owned subprocess handles.

**Tech stack:** Rust 1.96, existing Mojo bindings and Mojom generator, standard-library synchronization and subprocess orchestration.

## Constraints

- Keep the existing SDK/native ABI and runtime lifecycle unchanged.
- Use exact business interface versions and generation-checked cleanup.
- Host assigns identity/policy; peers cannot self-authorize.
- Keep user callbacks and endpoint destruction outside registry locks.
- Use existing publish/version conventions; do not publish or push as part of this task.

## Deliverables

- [x] Add `crates/mojo-service` manifest, build script and `schema/control.mojom`.
  Define `ServiceFactory.Bind`, directory register/list/connect/unregister, statuses,
  interface versions and registration generations. Generate with existing `mojom-build`.
- [x] Implement `ServiceKey`, `AccessPolicy`, `Registry<F>`, `ServiceManager`,
  owned host connections, `ServiceDirectory` and `bind_factory`. Add behavior tests
  for denied operations, duplicate ownership, version mismatch, closed sessions
  and replacement surviving stale disconnect/unregister cleanup.
- [x] Add `examples/service-network` using a typed Echo business schema. Run
  manager/provider/consumer roles, transfer endpoints, crash and restart provider,
  reconnect the existing consumer, and bound/clean up child processes. Add an
  explicitly ignored native test invoking the executable.
- [x] Document the API, run command and phase boundaries; update the root README,
  changelog and native CI smoke entry point.
- [x] Run `cargo fmt --all -- --check`, `cargo check --workspace --all-targets --locked`,
  `cargo test --workspace --all-targets --locked`, and
  `python3 -B tools/mojom/test_codegen.py`. Run the ignored native test with a
  matching trusted runtime if available. Review the final diff and resolve
  correctness findings before reporting actual verification results.

## Verification receipt

Workspace tests: 112 passed, 15 opt-in native tests ignored. All three new native
tests were separately run and passed on macOS arm64. Generator tests: 6 passed.
Release tooling: 19 run, 1 existing GN-dependent skip. Independent review findings
were addressed: stale discovery during reconnect, callback runner consistency,
broker-death child cleanup and the regression test's own process ownership.
The parked test participant has an independent safety deadline so a future
liveness-watcher regression cannot leave it alive indefinitely. See `VALIDATION.md`.
