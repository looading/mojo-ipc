# Service Supervisor Implementation Plan

> Execute inline; request an independent review of ownership and failure paths.

**Goal:** Move process startup/restart/stop policy into a reusable host API.

**Architecture:** Keep pure configuration/retry decisions separate from native
child/bootstrap ownership and the host-driven state machine. Reuse the existing
registry as the authority for session-specific startup readiness.

**Constraints:** Rust 1.96, standard library only, unchanged native ABI and Mojom
schemas, existing default-deny access policies, bounded startup/stop waits and
finite retries. No UI/runtime dependency or transparent business-request replay.

- [x] Add spec, policy, lifecycle types and behavior tests in `supervisor/`.
- [x] Add exact-owner readiness queries and a reusable child/bootstrap guard.
- [x] Implement host `ProcessSupervisor` add/poll/snapshot/stop APIs and opt-in
  participant helpers. Reap every old child before scheduling a replacement.
- [x] Convert the main network fixture to public supervision. Add native failure
  fixtures for unregistered, repeatedly crashing and uncooperative participants.
- [x] Run focused/native tests, request review, address findings, run the full
  workspace/generator/release/format checks and update docs/validation evidence.

## Verification receipt

Workspace/all-target check passed; 117 ordinary Rust tests passed with 16 native
opt-in tests ignored. Separately, all four service-network native tests and all
11 service-layer unit tests passed on macOS arm64. Generator: 6 passed. Release
tooling: 19 run, 1 existing GN-dependent skip. Formatting/whitespace checks passed.
Independent review found no remaining concrete correctness issues after atomic
deadline-aware readiness, duplicate-key validation and actual retry counts were
implemented. Full commands and native runtime provenance are in `VALIDATION.md`.
