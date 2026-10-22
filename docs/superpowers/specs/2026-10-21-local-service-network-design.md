# Local application service network

The approved first phase adds a `mojo-service` crate above the existing SDK and
a runnable manager/provider/consumer example. Existing Mojo Core, native ABI,
bindings and bootstrap roles remain unchanged.

## Architecture

One broker hosts a service manager. Applications join as participants through
existing bootstrap pipes; the host assigns each connection an identity and an
explicit, default-deny publish/connect policy. An application may both publish
and consume services. A service key consists of a logical name and interface
name, independent of a PID. Connections require an exact application protocol
version. Each registration receives a monotonically increasing generation.

The manager stores a factory endpoint for each registration. A consumer creates
a business pipe and transfers its receiver through the manager to that factory.
Business calls then use the transferred pipe without application-level message
forwarding by the manager. Generic message-pipe handles in the control protocol
allow business schemas to stay in their own crates; the provider validates and
binds its known interface. A successful connect means the endpoint was routed,
not that a business operation succeeded.

## Ownership and failure

Factory disconnect, directory disconnect or host connection removal removes the
affected registrations. Cleanup checks the registration generation so delayed
cleanup cannot remove a replacement. Closing a host connection also prevents
queued control requests from registering new services under that session.
Unregister requires ownership and the matching generation. Existing business
pipes are not revoked by unregister or control-plane disconnect.

Restart is explicit: the host waits for the failed provider, starts a new one,
and attaches it with the same logical identity/policy. The consumer observes its
old business pipe disconnect, discovers the new generation and binds again.
No pending business request is automatically replayed. Manager restart, OS peer
authentication, general supervision, deadlines, quotas, load balancing, native
multi-client listeners and cross-machine transports are outside this phase.
The callback API follows the SDK; host code bounds waits and observes disconnect.

## Verification

Pure Rust tests cover policy isolation, duplicate registration, exact versions,
ownership, closed sessions and delayed cleanup after replacement. An opt-in
native integration test runs a broker, provider and consumer as real processes.
Each child uses a separate one-shot named bootstrap listener; the broker uses
its known child PID for admission and retains a kill/wait handle for cleanup.
Participants watch a piped-stdin liveness lease before native initialization,
and exit on EOF even if the broker is killed without running destructors.
It checks authorization failures, discovery, typed calls through transferred
endpoints, provider crash, registration cleanup and a fresh connection after
restart. All subprocesses have bounded orchestration and cleanup on failure.
Workspace/all-target checks, existing tests and generator tests must still pass.
Native execution requires the trusted pinned runtime for the host architecture;
missing runtime is reported separately from ordinary test results.
