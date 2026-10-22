# Reusable local service process supervision

Continue the existing local service network with a host-driven `ProcessSupervisor`
in `mojo-service`. Keep the native ABI, wire schemas and business endpoint model
unchanged. The host calls `poll()` on a blocking-capable thread; the library does
not choose an async/UI runtime or run application callbacks on a new executor.

## API and ownership

`ProcessSpec` supplies a trusted executable, arguments, host identity, access
policy, required service keys/versions, startup deadline, shutdown grace and
restart policy. `add` validates and queues it. `poll` launches due processes,
checks exits and startup registration readiness, and emits lifecycle events.
Snapshots expose state, current PID, attempt/restart counts and the last error.

The supervisor owns real `Child` handles and manager admission leases. Each
launch gets a unique one-shot named listener; the actual spawned PID is supplied
to native accept. Child environment supplies the bootstrap name and attempt.
`watch_supervisor` installs an explicit stdin-EOF parent watcher before native
initialization; `connect_to_supervisor` joins after the child loads its runtime.
Managed-child stdin is reserved for this liveness lease. Both helpers are opt-in
and intended only for executables launched by this supervisor.

## Recovery and shutdown

Restart policies are Never, OnFailure and Always, with a finite lifetime restart
budget and fixed backoff. A failed spawn/bootstrap and startup readiness timeout
count as failures. Retry timing starts after the old child is reaped and its
lease removed. Readiness requires registrations owned by this exact session,
including the exact business interface versions; another process cannot satisfy
it. Readiness is a startup check, not a heartbeat or continuous health probe.
All requirements are checked under one registry lock with manager-side
registration timestamps. Delayed polling accepts an on-time registration while
rejecting late registration. Duplicate required keys are invalid. With no
required services, Ready denotes host admission only, not a peer handshake.

Control loss first allows the child its shutdown grace to exit normally. A
process that remains alive after that interval is killed/reaped and treated as
failed. Explicit stop disables pending retries, removes its lease, waits for
graceful exit and then kills/reaps if needed. Supervisor Drop performs immediate
cleanup. Business requests are never replayed and consumers still bind a fresh
generation after provider replacement. Process-tree supervision of grandchildren
and revocation of already transferred business endpoints remain outside scope.

## Verification

Unit tests exercise policy/budget/backoff decisions, spec validation and exact
session readiness. Replace the example's manual provider restart with the public
supervisor and keep its original consumer process across a crash. Native checks
exercise startup timeout, restart exhaustion, normal exit without restart,
explicit stop during backoff and forced shutdown of an uncooperative child.
Retain the original control/parent-death tests and run workspace, generator,
release-tool and native suites. Request independent review of state and cleanup.
