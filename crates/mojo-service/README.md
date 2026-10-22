# mojo-service

An experimental local application service layer over this unofficial Mojo SDK.
Applications can publish and consume typed interfaces without exposing their PIDs
as service identities. The native runtime and all SDK components must match.

The broker hosts `ServiceManager`. For each trusted bootstrap pipe, call
`attach(identity, policy, pipe)` and retain its `ManagerConnection`. Policies
deny by default and grant complete `ServiceKey` values separately for publish
and connect. The host assigns identities from its own launch/admission decision;
directory messages cannot grant permissions. This crate does not authenticate
OS peers or sandbox applications.

Participants bind `ServiceDirectory` to their bootstrap pipe. Providers use
`bind_factory`, retain the factory binding, and `register` its pending remote
with a service key and exact protocol version. The factory binds each incoming
message-pipe handle to the corresponding business `PendingReceiver<T>` and keeps
the resulting receiver alive. Consumers can list services, watch changes and
`connect::<dyn Interface>` to obtain a typed pending remote and generation.
The caller is responsible for matching the Rust interface type to its service
key; generic handle transfer deliberately avoids importing business schemas into
the control-protocol crate.

Each registration has a unique generation. Duplicates are rejected. Explicit
unregister requires ownership and the matching generation. Factory disconnect,
directory disconnect or host lease removal clears owned registrations without
allowing delayed cleanup to remove a replacement. List/Connect observe the
registry when processed; Watch provides a snapshot plus changes as below.
Connect success acknowledges endpoint routing, not readiness or business
success. Already transferred business pipes have independent lifetimes;
unregister and policy lease removal do not revoke them.

Callbacks run on the SDK dispatch runner. Disconnect drops pending callbacks
following the underlying SDK semantics; observe disconnect and bound host waits.
The directory's existing callback API is unchanged; the optional `CallClient`
below supplies Future/deadline completion for typed business calls. On provider
restart, observe the old business pipe closing and explicitly discover/connect
again. Exact application versions are checked; this is not SDK ABI negotiation.

Run the manager/provider/consumer example with a trusted native runtime:

```sh
MOJO_RUNTIME_LIBRARY=/absolute/path/to/libmojo_rs_native.dylib \
  cargo run -p mojo-service-network --bin service-network
```

## Dynamic service discovery

`ServiceDirectory::watch(capacity, callback)` returns a `ServiceSubscription`
whose `initial_services()` is an authorized snapshot. The snapshot and server
watch queue are installed under one registry lock; a registration/removal is
covered by either that snapshot or later events. The snapshot is an immutable
starting point, not a live cache. Apply batches to your own view and use Connect
to resolve the actual current provider; a provider can disappear after any event.

On a blocking host thread after binding the directory:

```rust,ignore
use std::{sync::mpsc, time::Duration};

let (tx, rx) = mpsc::channel();
directory.watch(64, move |result| { let _ = tx.send(result); });
let mut subscription = rx.recv_timeout(Duration::from_secs(5))??;
for service in subscription.initial_services() {
    println!("{} generation={}", service.name, service.generation);
}
let batch = subscription.recv_timeout(Duration::from_secs(5))?;
// Async alternative: let batch = subscription.next().await?;
for change in batch.changes {
    println!("{:?} {} revision={}", change.kind, change.service.name, change.revision);
}
```

Events are Added/Removed with full ServiceInfo and a subscription-local revision
starting at one after the snapshot. Only keys granted by the directory session's
connect policy are included. Hidden mutations neither enqueue events nor advance
the local revision. Service generations retain their manager-wide meaning;
subscription revisions reset when resubscribing.

Capacity must be 1..=MAX_WATCH_EVENTS (256). A directory session has one active
watch; a new watch closes the old one using a replacement token guard. Each watch
holds at most that many queued changes and one pending Next response. It sends
updates only in response to the client's long-poll request. The helper keeps at
most one RPC and one reply batch, including across repeated wait timeouts.

`recv_timeout` returns TimedOut without abandoning its in-flight Next or losing a
later batch. `next()` is a borrowing Future; dropping the Future releases its
Waker and leaves that same RPC/batch owned by the subscription for the next wait.
Drop the subscription to close the pipe. Blocking waits on the native dispatch
sequence return WouldBlock. Future polling remains usable there.

Queue overflow returns terminal ResyncRequired and discards the partial history.
Call directory.watch again and rebuild your view from its fresh snapshot before
applying new events. Directory/watch disconnect, replacement and host lease
removal end old subscriptions as Disconnected. Busy indicates overlapping raw
Next requests; the public helper prevents those and treats Busy as terminal.
Subscription failures do not replay calls or revoke existing business endpoints.

The control schema appends Watch and adds ServiceWatch; existing control method
ordinals and the native ABI are unchanged. New watch peers require matching
control definitions. Old-peer negotiation or automatic polling fallback is not
provided. The three-process example uses snapshot/events for initial discovery
and recovery; the single List call there only checks permission filtering.

## Bounded business calls

Create one shared `CallContext` and bind each discovered typed pending remote to
a `CallClient` with an explicit outstanding-response limit. The context creates
one deadline thread shared across its clients; no Tokio or other executor is
required. `Call<R>` implements `Future<Output = Result<R, CallError>>` and can be
awaited on any executor. A blocking host thread can instead call `wait()`.

For a generated Echo interface whose response has several values:

```rust,ignore
use mojo_service::{CallClient, CallContext};
use std::time::Duration;

let calls = CallContext::new()?;
let mut client = CallClient::bind(&runtime, &calls, connection.remote, 64)?;
let call = client.call(Duration::from_secs(2), |remote, reply| {
    remote.Ping("hello".into(), move |text, instance, pid| {
        reply.respond((text, instance, pid));
    });
})?;
let cancel = call.cancellation_handle();
// In async code: let reply = call.await?;
let reply = call.wait()?;
```

The send closure executes immediately and must not block. It must send exactly
one response-bearing generated RPC and move the supplied responder into that
callback. For a single response value, `reply.into_callback()` is a convenient
adapter. Existing generated method signatures and the original bindings are
unchanged. Bind/call admission can return InvalidOptions, ContextClosed,
Disconnected or AtCapacity before sending; a zero timeout returns TimedOut.

Success, timeout, cancellation, callback destruction, client drop and context
shutdown complete a call exactly once. `call.cancel()` or a cloned cancellation
handle returns true only when it wins; dropping the call cancels local waiting.
The handles/response bridges are weak and do not retain result values. Local
deadline/client records are removed before the completed result is published.
`CallResponder` destruction and client drop report Disconnected; context shutdown
reports ContextClosed. Success arriving after the deadline cannot win even if
the deadline worker was delayed. Blocking waits on the native dispatch sequence
or deadline worker return WouldBlock; use Future polling there instead.

**Timeout/cancellation ends local waiting, without undoing remote execution.**
Sent requests keep their original binding response slots until a reply or
disconnect. Consequently, they still consume capacity after local cancellation.
Late replies are decoded/validated normally, including returned handles and
associated interfaces, then ignored by an already completed/dropped local call.
This preserves unknown-response validation and prevents repeated cancellation
from allowing an unresponsive service to accumulate unlimited callbacks.
`outstanding_requests()` reports binding slots; `pending_calls()` reports local
waiting results. After AtCapacity, wait for replies or drop/rebind the client.
The limit assumes the one-response-bearing-RPC send-closure contract above.

Calls are never automatically replayed. Business operations that may have run
before timeout require application-defined reconciliation/idempotency. Dropping
a client closes its connection and promptly fails its local waiters. Clients
retain context clones, so dropping only the original context handle does not
stop their shared timer. `CallContext::shutdown()` explicitly resolves pending
local calls and signals worker exit, without joining from a callback thread;
last-owner drop also signals exit. Future Wakers may run on the response,
deadline, cancellation or shutdown thread; user code runs outside internal locks.

## Process supervision

`ProcessSupervisor` owns child handles and admission leases. `add` validates and
queues a `ProcessSpec`; the host drives `poll` to start children, check startup
registrations, observe exits and perform policy-driven restarts. Call poll and
stop from a blocking-capable host thread, outside the Mojo dispatch sequence
and async/UI worker threads. One poll can block for each due child's bootstrap
timeout; stop can block for its configured shutdown grace.

After loading a broker runtime, configure a process like this:

```rust,ignore
use mojo_service::{AccessPolicy, ProcessSpec, ProcessSupervisor, RestartPolicy,
                   ServiceKey, ServiceManager};
use std::time::Duration;

let manager = ServiceManager::new(&runtime);
let mut supervisor = ProcessSupervisor::new(&runtime, &manager);
let key = ServiceKey::new("files", "demo.Files");
let process = supervisor.add(
    ProcessSpec::new("files-worker", "/path/to/files-worker",
        AccessPolicy::new().allow_publish(key.clone()))
        .require(key, 1)
        .startup_timeout(Duration::from_secs(10))
        .shutdown_grace(Duration::from_secs(2))
        .restart_policy(RestartPolicy::OnFailure {
            max_restarts: 3,
            backoff: Duration::from_millis(250),
        }),
)?;
// Drive from the host's supervision loop; inspect emitted events/snapshots.
let events = supervisor.poll()?;
let state = supervisor.snapshot(process);
// At host shutdown, close admission and allow graceful exit, then kill/reap.
supervisor.stop_all()?;
```

Restart policies are Never (default), OnFailure and Always. The restart budget
applies to the entire spec lifetime and does not reset on Ready. Backoff is
fixed and starts after reaping the old child and removing its admission lease.
`ProcessSnapshot.restarts` counts actual launched retries; a cancelled pending
retry does not count. Spawn/bootstrap failure, readiness timeout and a process
that remains alive after control loss are failures. Normal successful exit does
not trigger OnFailure. Explicit stop disables queued retries, including during
backoff. Cleanup errors retain the child handle and suppress replacements.

Readiness requires every configured key/version to be simultaneously registered
by this exact admitted session before its startup deadline. Registration times
are recorded in the manager, so a delayed host poll can recognize timely
readiness while rejecting actually late registration. Duplicate required keys
and requirements without publish permission are rejected. **With no required
services or health checks, Ready means host admission only; it does not establish
a peer handshake or business readiness.** Service requirements are startup checks;
optional continuous health monitoring is configured separately below.

Managed participants call `watch_supervisor()` once before loading their native
runtime, then `connect_to_supervisor(&runtime)` afterwards. The helper consumes
stdin to watch the parent-liveness lease and exits on EOF/error. The supervisor
sets `MOJO_SERVICE_SERVER` and `MOJO_SERVICE_ATTEMPT`; applications may use the
attempt number for diagnostics. This explicit helper is intended for managed
executables whose stdin is reserved for supervision. Business methods continue
to run on the SDK dispatch runner. Closing admission permits normal disconnect
handling during shutdown grace; after grace the host kills/reaps the direct
child. Supervisor Drop cleans up immediately; use stop_all for graceful exit.

The control layer still accepts externally established trusted bootstrap pipes.
Multi-client named listeners, quotas, manager recovery,
protocol negotiation, supervision of grandchildren and cross-machine transport
are not provided. Consumers still explicitly reconnect to a new generation;
business requests are never automatically replayed.

## Continuous process health

Health checking is disabled by default. Opt in on a `ProcessSpec`:

```rust,ignore
use mojo_service::HealthPolicy;
let spec = spec.health_check(HealthPolicy {
    interval: Duration::from_secs(1),
    timeout: Duration::from_millis(250),
    failure_threshold: 3,
});
```

The participant creates and retains an explicit endpoint and registers it on its
own directory session, before publishing startup services:

```rust,ignore
let (probe, health_binding) = mojo_service::bind_health(&runtime, || true)?;
// Retain health_binding until shutdown. Bound the host wait for this callback.
directory.set_health_probe(probe, |result| { /* handle registration result */ });
```

A constant true only checks that the participant's SDK dispatch sequence can
respond. Supply a short nonblocking application check for business health.
An enabled process must register its probe before the startup deadline to reach
Ready. Atomic replacement of an existing endpoint preserves its continuous
registration start time; removal/disconnect followed by a new install does not. Ready precedes the first successful check; `snapshot.health` initially
reports Unknown. Healthy is emitted on the first success and on recovery from
Unhealthy. HealthCheckFailed reports consecutive failures; successful checks
reset that counter. Health counters describe the current launch attempt.
`last_error` retains historical errors even after recovery.

Call `ProcessSupervisor::poll` regularly from a blocking-capable host thread.
The monitor has no timer thread: detection and scheduling progress only on polls.
A callback records its receive time, so a timely reply remains successful when
the host polls late. Each check gets one fixed timeout window. Replacing a probe
within that window preserves its deadline and consecutive failure count, and
ignores old replies. Only that admitted session can replace its own probe.

At most one raw Check request is outstanding per endpoint. After timeout its
binding response slot remains occupied until reply/disconnect; later scheduled
samples fail with “previous health probe still outstanding” rather than enqueue
more requests. Missing/disconnected endpoints and false responses also count as
failed samples. Below the threshold, the process stays Ready and registered with
Unhealthy status. At the threshold, the host closes admission, allows shutdown
grace, then kills/reaps if needed. This consumes the same lifetime restart budget
and fixed backoff as other failures, even if graceful shutdown exits with code 0.
Subscriptions observe old registrations removed and a new generation added after
restart. Explicit stop cancels monitoring and retries; old callbacks cannot
update a new attempt. Business calls are never replayed.

HealthProbe and SetHealthProbe append to the experimental control definitions;
participants using them require matching definitions. Native ABI is unchanged.
