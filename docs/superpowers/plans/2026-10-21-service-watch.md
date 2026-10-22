# Dynamic Service Discovery Implementation Plan

> Execute inline; request independent lifecycle/concurrency review.

**Goal:** Replace repeated discovery queries with atomic snapshot plus bounded
authorized service-change subscriptions.

**Constraints:** Rust 1.96, std only, native ABI unchanged, append-only control
methods, one watch and one pending Next per admitted session, at most 256 events,
explicit overflow/resubscribe, no callback/endpoint destruction under locks.

- [x] Add watch schema and bounded server buffer with deferred delivery tests.
- [x] Integrate atomic registry snapshot/install, filtered events and token-guarded
  cleanup into all register/remove/disconnect/lease paths.
- [x] Add ServiceSubscription borrowing Future and timeout-reusing host wait.
  Test single-flight ownership, Waker cleanup and terminal status behavior.
- [x] Add native discovery tests and migrate consumer startup/reconnect to events.
- [x] Complete review, workspace/native/generator/release checks, docs and results.

## Verification receipt

Workspace/all-target check passed; 136 ordinary Rust tests passed, with 18 native
opt-in tests ignored. Separately, 30 service unit tests and all six network native
tests passed on macOS arm64. Generator: 6 passed. Release tooling: 19 run, one
existing real-GN skip. Formatting/whitespace checks passed. Independent review
found no correctness blocker across registry/buffer/client ownership, consumer
event-driven reconnection and the native watch fixture. See `VALIDATION.md` for
runtime provenance and exact log paths.
