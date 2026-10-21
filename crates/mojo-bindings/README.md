# mojo-bindings

An unofficial component of the [mojo-ipc SDK](https://github.com/looading/mojo-ipc); this is not an official Google or Chromium crate.

Provides typed Remote/Receiver endpoints, request and response routing, callbacks and associated interfaces.

This component is part of SDK `0.2.1`.
Keep the SDK component versions aligned when upgrading.

Upstream Chromium is pinned to `bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`.

The Rust crate builds with ordinary Cargo and does not require a Chromium checkout. Running IPC requires the matching native SDK library, explicitly loaded at runtime through `mojo-ipc::Runtime::load`. This crate does not download the native runtime.

Version `0.2.1` uses an owned primary-dispatch job value in place of a per-message
boxed closure. Typed incoming header parsing shares the successful handle
extraction's byte view instead of issuing another native data lookup. The
public owned-message API, lock-release, weak ownership, FIFO and fallback
rules remain. These changes do not by themselves establish a p99 improvement.

Version `0.2.0` added stack-backed V1/V2/V3 outgoing header encoding while
retaining the public owned-message API and exact header bytes. Performance and
tail-latency effects are evaluated separately.

Since `0.2.0`, eligible primary-only endpoints can run their handler at the end of an
already-posted watcher task, after the router/watcher callback guards are
released, instead of posting a second endpoint task. The bound sequence,
per-pipe FIFO and asynchronous receive boundary remain; handlers are not
invoked synchronously on the sending Write call stack. Associated endpoints
and ineligible states retain posted routing, with permanent fallback once
the primary-only eligibility is lost. Relative ordering against unrelated
runner tasks can change: no extra second task turn is guaranteed. For `0.2.0`, four native
scheduling tests passed on macOS x86_64. Eight formal comparison runs completed
for that version, but the ABBA and BAAB tail-latency results disagree; this does
not establish that p99 regressions are solved or validate other platforms.

Licensed under BSD-3-Clause; see [LICENSE](LICENSE).
