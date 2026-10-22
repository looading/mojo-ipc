# Bounded Service Calls Implementation Plan

> Execute inline and request an independent concurrency/lifecycle review.

**Goal:** Bound typed RPC waiting and memory with deadlines, Future completion,
explicit local cancellation and transport backpressure.

**Constraints:** Rust 1.96, standard library only; unchanged native ABI, existing
bindings and generated-method signatures. No server-side cancellation/replay.

- [x] Implement a shared weak-reference deadline scheduler and context shutdown.
- [x] Implement exactly-once call state, Future/host wait, weak responder and
  cancellation handle. Test deadline wakeup, races, reentrancy and cleanup.
- [x] Add a typed client with bounded local/transport outstanding requests and
  deterministic pending-call failure on client drop.
- [x] Add real native call fixtures for delayed/held replies, late-response
  validation, disconnects and capacity admission. Keep existing native checks.
- [x] Review implementation, fix findings, document semantics and run workspace,
  generator/release, format and native verification. Record actual results.

## Verification receipt

Workspace/all-target check passed; 128 ordinary Rust tests passed with 17 native
opt-in tests ignored. Separately, 22 service unit tests plus five real native
network tests passed on macOS arm64. Generator: 6 passed; release tooling: 19 run
with one existing real-GN skip. Formatting/whitespace checks passed. Independent
review found no blocking issue after cleanup-before-publication and timer-thread /
last-context-owner coverage were added. See `VALIDATION.md` for runtime provenance
and the exact log paths. The three-process consumer uses the public call API.
