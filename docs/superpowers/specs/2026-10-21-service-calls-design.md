# Bounded service calls with deadlines and cancellation

Continue the existing service layer with a runtime-independent `CallClient<T>`,
`Call<R>` Future, cancellation handle and blocking host-thread wait. Keep the
native ABI, generated interface methods and underlying bindings unchanged.

## API and completion

One shared, cloneable `CallContext` owns a deadline scheduler and timer thread.
It can bind multiple pending typed remotes as bounded call clients. A call takes
a timeout and a closure that sends exactly one response-bearing generated RPC,
moving the supplied `CallResponder<R>` into its response callback. Multi-value
responses map to a tuple or business value. No async runtime dependency is added.

Success, timeout, cancellation, response-callback destruction, explicit context
shutdown and client drop race to complete one shared result exactly once.
Response callback destruction reports Disconnected; client drop explicitly fails
its remaining local calls. Call drop cancels local waiting. A cloned cancellation
handle can cancel from another thread. Future polling registers a Waker, while
blocking wait is rejected on the native dispatch sequence and timer thread.
No user Waker or payload destructor runs under a scheduler/result/client lock.

Deadline records and client-local pending records are removed on completion.
Scheduler entries and response responders hold weak references, so cancellation
releases the local future's state/resources without retaining them until reply.
Explicit context shutdown resolves remaining calls and signals timer exit; the
last context owner also signals shutdown without joining from a dispatch thread.

## Cancellation and bounded transport state

Cancellation and timeout do not undo remote execution or remove a generated
binding response callback. That callback retains only the weak responder bridge
when callers follow the API contract; legitimate late replies are still decoded,
validated and disposed normally by the existing bindings. This preserves handling
of returned handles/associated interfaces and unknown-response validation.

Each client limits both locally pending calls and underlying pending response
slots. A cancelled/timed-out sent request retains transport capacity until its
reply or disconnect, and reaching capacity rejects new calls before sending.
This bounds never-replied request accumulation without changing bindings or
silently closing unrelated calls. Dropping/rebinding the client releases the old
connection. There is no automatic replay or server-side cancellation protocol.

## Verification

Pure tests cover deadline wakeup, success/cancel/disconnect/shutdown races,
result consumption, cancellation-handle ownership, cleanup of weak scheduler
records and Waker reentrancy. Native tests cover normal typed calls, late replies
after timeout/cancel with a usable connection, bounded never-replied requests,
capacity recovery, peer/client disconnect and rejection of blocking dispatch wait.
Retain the supervisor/network checks and all workspace/generator/release gates.
