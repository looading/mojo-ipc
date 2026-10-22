# Bounded dynamic service discovery

Continue the service network with an atomic initial snapshot and a bounded
long-poll subscription. Preserve native ABI and existing control method ordinals;
append a Watch method and add a dedicated ServiceWatch interface. Peers using
the new feature require matching control definitions; negotiation is not added.

## Control plane

Directory.Watch(capacity, pending_receiver<ServiceWatch>) returns a filtered
snapshot. Install its queue and take the snapshot under one registry lock, so
every visible change is represented either by the snapshot or later events.
One active watch per admitted directory session; replacement closes the old
watch, guarded by an independent token so stale disconnect cannot remove its
replacement. Directory/watch disconnect and host lease drop clean up state.

Events are Added/Removed with full ServiceInfo and a subscription-local monotonic
revision. Initial revision is zero. Hidden service mutations neither enqueue
events nor advance that subscription's revision. The original access policy
filters snapshot and events. Revisions reset on resubscribe; service generations
remain manager-wide identities for actual provider instances.

Each watch holds at most 256 queued events (caller chooses 1..256) and one Next
reply callback. Next drains one bounded batch or waits until a change occurs.
A second raw Next while one is pending returns Busy without replacing it.
Overflow clears the queue and produces ResyncRequired; no partial history is
silently presented as complete. The client must resubscribe for a fresh snapshot.

Queue mutations happen under registry ordering. Delivery is posted via a weak
buffer reference to the SDK dispatch runner, never inline under the registry
lock. At most one delivery job is queued per watch. Endpoint/callback destruction
and close replies happen after registry/buffer locks are released.

## Client

ServiceDirectory.watch asynchronously returns a ServiceSubscription with its
initial snapshot. next() is a borrowing Future; recv_timeout() is a host blocking
wait rejected on the native dispatch sequence. Both share one outstanding Next
RPC and one buffered reply. Dropping the borrowing Future or timing out ends only
that wait, retaining the in-flight RPC for the next attempt; no extra wire query
or lost batch is introduced. A dropped Future releases its registered Waker.
Dropping the subscription closes its watch pipe. Overflow/disconnect is terminal
for that subscription. Existing directory/business call APIs remain unchanged.

## Verification

Pure tests cover filtered revisions, ordered bounded batches, overflow, pending
Next limits, atomic snapshot ordering, stale replacement cleanup, and client
Future/Waker ownership. Native tests cover late subscribe, add/remove/restart,
permission filtering, slow-consumer resync, replacement, wait timeout reuse,
async wakeup and disconnect/lease cleanup. Convert the three-process consumer
from repeated List queries to snapshot/change-driven connection and reconnection.
Run all existing workspace/generator/release/native checks and independent review.
