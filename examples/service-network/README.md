# Local service network example

This fixture runs three real processes at a time: a broker/service manager, a
provider application and a consumer application. Both applications load the
SDK as participants; serving an interface does not require the broker role.
The business Echo schema is independent of the service control schema.
The consumer uses `CallClient` with a shared `CallContext`, bounded response
capacity and host-thread waits for its typed business calls.
It discovers providers through an atomic snapshot and service-change subscription;
startup and recovery wait for relevant Added events rather than polling List.

```sh
export MOJO_RUNTIME_LIBRARY=/absolute/path/to/trusted/libmojo_rs_native.dylib
cargo run -p mojo-service-network --bin service-network
```

Use the matching `.so` or DLL on Linux/Windows. The runtime must match the pinned
Chromium revision, ABI and executable architecture. Code generation needs
Python 3.10+ as for the rest of the workspace.

The host launches applications using one named listener per child and attaches
the resulting trusted pipes with fixed identities and default-deny policies.
The provider publishes public/private Echo factories. The consumer can discover
and connect only to the public service; publish and unregister attempts are
denied. Each fresh business receiver is transferred through the manager to the
provider, then typed Echo calls use that business pipe.

After the first call, the consumer deliberately crashes the provider (exit 17).
The private fixture's response-bearing Crash call resolves as Disconnected,
without fabricating a response or replaying the operation.
The host's public `ProcessSupervisor` reaps it and automatically starts a
replacement under an OnFailure policy (one retry, 50 ms backoff), with the same
logical identity. The provider must register its exact public interface before
the startup deadline to be Ready. The broker also enables optional continuous
health checks; the provider registers a responsive HealthProbe before publishing.
The original consumer observes disconnect,
receives a new generation through its subscription and resolves a fresh business
endpoint before calling the replacement. Rebinding remains explicit; business
requests are not replayed. Its one List request checks permission filtering.

Every host wait is bounded. Child handles are killed/reaped on error; a piped
stdin acts as a parent-liveness lease, so participants also exit when the broker
is terminated without running its destructors. Per-connection Echo receivers
are self-owned and release themselves on consumer disconnect.

Run all native checks explicitly:

```sh
cargo test -p mojo-service-network --test native_network -- --ignored --nocapture
```

These tests cover the three-process network, factory/directory disconnect,
explicit unregister, stale generations, host connection removal and abrupt
broker exit. They are ignored in ordinary Cargo tests because loading the native
runtime is an explicit operator trust decision. Native CI runs them on each
target before setting the `service_network_passed` acceptance gate.

The additional supervisor check exercises a missing executable, finite crash
retries, clean exit without retry, cancellation while queued/during backoff,
startup registration timeout, a participant that never joins, control loss and
forced shutdown. It also distinguishes timely registration observed by a slow
host poll from registration that actually missed its startup deadline.
The parked parent-death fixture and other uncooperative test participants have
independent safety deadlines in addition to host-owned process cleanup.

The call check holds genuine generated response callbacks to test timeout,
cross-thread cancellation, dropped calls, AtCapacity admission and recovery.
Flushing those responses proves late replies leave the connection usable and
returned message-pipe handles are released. It also checks peer/client drop,
explicit context shutdown, and blocking-wait rejection on the native sequence.

The watch check covers late-subscriber snapshots, hidden-service filtering,
subscription-local revisions, one in-flight Next across repeated timeouts,
borrowing Future wakeup/drop, stale replacement cleanup, overflow/resubscription,
directory/lease removal and raw overlapping Next rejection. A slow consumer gets
ResyncRequired and must rebuild its state from a fresh snapshot. The main network
example uses capacity 64 for its bounded restart scenario and propagates that
error if exceeded.

The health check uses real participants to cover periodic success, a transient
false response followed by recovery, replacement endpoint cleanup, missing-probe
startup timeout and threshold failure exhausting the existing restart budget.
Live replacement returns false once and then recovers to prove the new endpoint
is in use. Late replacement publishes an acknowledged marker before the host's
first readiness poll, verifying continuous startup registration evidence.
It deliberately hangs the participant's dispatch sequence while its process
stays alive: one timed-out Check remains bounded, the next sample reports its
occupied slot, and the host reaps/restarts the process. The original subscription
sees Added/Removed/Added with a new generation. A delayed host poll also verifies
that a reply received before its deadline remains successful.
