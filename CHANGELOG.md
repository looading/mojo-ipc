# Changelog

All version changes are maintained here, with the newest version first.
The README contains project setup and usage instructions and links to this file.

## Unreleased

- Add explicit `RuntimeConfig` and custom Chromium revision selection to native
  build, Windows toolchain and packaging commands. Preserve the official default,
  strict C ABI/layout checks, and path/role/revision identity after initialization.
  Custom native builds do not change the generated bindings' source provenance.


- Add opt-in `HealthPolicy` and session-owned `HealthProbe` registration with
  a retained `HealthBinding`. Require timely probe registration for monitored
  readiness, then publish health events and per-attempt counters from host polls.
- Bound probes to one raw request per endpoint, stamp reply receive times and
  preserve deadlines/failure history across endpoint replacement. Threshold
  failures close admission, allow shutdown grace, reap the old process and use
  the existing finite restart budget/backoff without replaying business calls.
- Add deterministic health state tests and native periodic/transient/unhealthy,
  endpoint replacement, missing-probe, delayed-poll and hung-process recovery
  checks, including service subscription changes. Append the optional control
  protocol while preserving existing ordinals and native ABI.

- Add authorized dynamic discovery with an atomic initial snapshot and bounded
  ServiceWatch long polls. Emit Added/Removed events with per-subscription
  revisions; hidden services do not affect those revisions. Cap queues at 256
  events and require explicit resubscription after overflow.
- Add ServiceSubscription borrowing Future and host timeout wait, reusing one
  pending Next across abandoned/timed-out waits. Guard replacement/disconnect
  cleanup by token and release replies/endpoints outside registry locks. Append
  Watch to the control protocol; new-feature peers require matching definitions.
- Use snapshot/change-driven discovery in the three-process consumer and add
  native subscription tests for filtering, timeout reuse, async wake/drop,
  overflow/resync, replacement, session revocation and overlapping raw Next.

- Add optional runtime-independent `CallClient`, shared deadline `CallContext`,
  `Call` Future/host wait, weak cross-thread cancellation and responder bridges.
  Resolve local results exactly once, clean local tracking before publication,
  and fail pending calls on client drop/context shutdown without request replay.
- Bound both local pending calls and original binding response slots. Timeout
  and cancellation do not retract remote execution; cancelled slots continue
  consuming capacity until reply/disconnect. Preserve normal late-response and
  returned-handle validation without modifying generated SDK methods or bindings.
- Use bounded calls in the three-process consumer and add native held-response
  tests for timeouts, cancellation, capacity recovery, disconnect/shutdown and
  late-returned handle release. Extend the private fixture protocol with CallProbe
  and a response-bearing Crash; the SDK/native ABI remains unchanged.

- Add reusable host-driven `ProcessSupervisor` with startup registration
  readiness, fixed-backoff Never/OnFailure/Always restart policies, finite
  lifetime retry budgets, lifecycle events/snapshots and graceful/forced stop.
  Own and reap direct child handles before replacement. Provide opt-in managed
  participant bootstrap and parent-death helpers without changing the native ABI.
- Make readiness atomic and session-specific with manager registration
  timestamps, preserving on-time readiness across delayed polls while rejecting
  late registration. Reject duplicate required keys and missing publish grants.
- Replace the example's manual restart branch with the supervisor; add native
  failure-path checks for crash loops, pending-retry cancellation, startup and
  control-loss timeouts, uncooperative children and normal successful exits.

- Add experimental `mojo-service` above the existing IPC SDK: host-assigned
  identity, default-deny publish/connect policies, registration/discovery,
  exact interface version checks, generation-checked cleanup and business
  endpoint transfer. Existing SDK APIs and native ABI remain unchanged.
- Add a real manager/provider/consumer fixture with authorization checks,
  provider crash/restart, explicit consumer rebinding and bounded subprocess
  cleanup. Native execution is opt-in and part of the native CI gates.

## 0.2.2 (2026-10-21)

This release adds native library build and distribution automation. The Rust
API, Mojom wire format, Chromium pin and native C ABI v1 remain unchanged from
`0.2.1`. Cargo crate publication is separate from native GitHub Releases.

- Build Linux x86_64 GNU, macOS x86_64/arm64, and Windows x86_64 MSVC libraries
  on fresh GitHub-hosted runners, provisioning the pinned Chromium checkout,
  depot_tools and required platform tools.
- Run Cargo/Mojom checks, native smoke, the independent-process named-channel
  fixture, and the Rust typed parent/child example before packaging.
- Audit native architecture, exports and runtime dependencies; distribute the
  runtime library, C header, build/acceptance manifests and dependency licenses
  in versioned archives with SHA-256 checksums.
- Publish the complete four-target set through a version-tag-triggered GitHub
  Release. Support manual acceptance builds and safe retries of draft uploads.
- Use the pinned GN/Ninja executables directly in hosted builds, avoiding
  depot_tools Python launchers that may not have a bootstrapped interpreter.
  Allow an existing SDK tag to be rebuilt with a corrected workflow while
  retaining its source commit and recording the workflow commit separately.
- Select an installed Xcode SDK containing Chromium's required DarwinFoundation
  module maps before provisioning a macOS producer, instead of relying on the
  runner image's older default SDK.
- Add the native build directory to the smoke processes' dynamic-library search
  path so Linux/macOS acceptance can load the newly built shared library.
- Recognize the macOS runtime's relative self install ID during dependency
  auditing, and use tested workflow-commit packaging tools with the immutable
  SDK checkout's version, header and license when rebuilding an existing tag.
- Verify and publish draft Releases by numeric ID, and allow publication to
  reuse the complete verified package set from a completed native workflow run.

## 0.2.1 (2026-10-21)

This patch retains Rust source compatibility with `0.2.0`, the Mojom wire
format, Chromium pin and native C ABI v1. Keep SDK component versions aligned.

- Replace the eligible primary dispatch's boxed closure with a concrete owned
  job value. Preserve lock release, weak endpoint ownership, bound sequence,
  per-pipe FIFO, permanent fallback and public raw-watcher behavior.
- Parse typed incoming headers using the bytes returned by handle extraction,
  avoiding a second native data lookup. Retain the owned raw message and
  malformed-message reporting and handle cleanup behavior.
- Fix empty native message buffers: a null pointer with zero bytes is safely
  represented as an empty Rust slice; non-empty buffers still require a valid
  pointer.
- Add six real macOS x86_64 message-read native tests, covering zero/multiple
  handles and shared/mutable reads. The four existing routing
  tests and the trap and message-parts native tests also passed separately.
  These correctness checks do not establish a measured latency improvement.
- Complete 56 case-interleaved ABBA/BAAB trials on macOS x86_64. The 64 KiB
  typed run-p99 medians decreased 1.01% / 8.41% in the respective blocks,
  while 0 B, 64 B and 1 MiB typed p99 retained regressions. Preserve all
  40 adverse scoped comparisons; do not treat this patch as a universal p99
  fix. See `benchmarks/P99_BAAB_OPTIMIZATION.md` for the full comparison.

## 0.2.0 (2026-10-21)

This minor version changes the pre-1.0 Rust source API through new public compact-array AST
variants. Adapt exhaustive matches and representation equality/hash/order, and
upgrade the SDK codec, derive, generator and runtime components together to
`0.2.0`. Regenerate protocol bindings. Public named AST conversion and custom
Context defaults are retained. The Mojom wire format, Chromium pin and native
C ABI v1 remain unchanged; registry `0.1.0` remains a historical wire-compatibility
reference.

- Coalesce eligible primary-only endpoint dispatch into the existing posted
  watcher task, after releasing router/watcher callback guards. The original
  asynchronous boundary, bound sequence, per-pipe FIFO and associated-endpoint
  posted fallback remain. Eligibility is permanently disabled after associated
  registration or any fallback. Ordering relative to unrelated runner tasks
  can change: the historical extra second task turn is no longer guaranteed.
  Four real macOS x86_64 native tests cover FIFO/disconnect, callback reentry,
  unbound associated backlog and releasing the last watcher owner. Eight
  interleaved benchmark runs observed lower overall typed RTT medians, while
  the reverse-order block retained regressions and substantial environment
  variation. This is not a guarantee of lower production p99 in every case.
- Add stack-backed V1/V2/V3 outgoing message header encoding while retaining
  public `serialize() -> Vec<u8>` and exact bytes, including unknown flag bits.
- Add a single-event trap arming path for SDK watchers while retaining public
  `arm()` return types and blocking-event subset semantics. Separate allocator
  checks confirm removal of the outgoing header's 56-byte allocation and the
  internal arm path's 32-byte event-buffer allocation on the tested host.

- Reduce complex-type codec allocations: lazy parser labels for array/union/map
  metadata, serialization-only derived struct conversion without diagnostic
  field names, direct pointer-array tables and fixed map bodies. Keep the public
  named AST conversion and custom conversion defaults. The new hidden trait
  hook and derive must be upgraded together; wire compatibility is independent
  of Cargo source compatibility.
- Batch bool bitmaps with baseline SSE2 on x86_64 and a portable fallback;
  group nullable tag/value processing while preserving exact Mojom wire bytes.
- Cache derived concrete struct schemas without a per-message global cache
  lookup, and remove temporary tuple-vector/padding allocations.
- Send header and payload directly into one fresh committed Core message;
  preserve public owned-message APIs and handle ownership. Use one task Box
  with concrete callbacks while preserving the native run/cancel ABI.

- Add compact nullable scalar arrays, preserving leading tag bitmaps, aligned
  retained value slots, bool dual bitmaps, zero-filled None encoding, dirty
  None/padding decode tolerance and floating-point bits. Custom typemaps only
  convert Some; nullable enums and handles keep their existing validation.
- Add public `NullablePrimitiveArray` and `MojomValue::NullablePrimitiveArray`;
  like the other compact forms, this requires low-level AST match adaptation.
- Extend compact array encoding/decoding to bool, signed integers, uint16/32/64,
  float and double, including native and OrderedFloat vectors/fixed arrays.
  Preserve LSB bool packing, little-endian scalar layout and floating-point
  bit patterns. Enum/handle validation retains the generic path. Adapt map
  columns and associated-interface ID arrays to compact values.
- Add the public `PrimitiveArray` enum and `MojomValue::PrimitiveArray` variant;
  low-level AST matches and equality/hash/order must account for this form.
- Encode/decode non-nullable `array<uint8>` in bulk, avoiding one intermediate
  value and debug field-name allocation per byte. Preserve the existing Mojom
  wire format, fixed-array checks, nullable/nested arrays and custom typemaps.
- Add `MojomValue::ByteArray(Vec<u8>)`. This changes the public Rust AST enum:
  exhaustive matches and AST equality/hash/order need to account for the new
  representation. Legacy `Array(UInt8...)` remains encodable and convertible.
  This is not a published 0.1.0 patch or a claim of Rust source compatibility.

### 性能验证（macOS x86_64）

`0.2.0` primary-dispatch 轮的 macOS x86_64 正式八轮中，64 KiB typed RTT 的 run p50 中位数为
275.6115 → 246.0965 µs，run p99 中位数为 481.6085 → 408.1355 µs。
但 p99 在 ABBA 组下降 28.49%，在 BAAB 组上升 25.79%；最后一轮 after 和
全部下降项均保留。每轮前后记录的 CPU_Speed_Limit 也发生变化，端点观测不足以
确定原因。独立补充 BAAB、数值控制与本轮 after 有限稳定性运行均已分别核验，
详见 `VALIDATION.md`。

### 尾延迟对照摘要

以下为 macOS x86_64 的独立 run 统计量中位数变化，After 相对 Before；延迟负值
表示更低，吞吐正值表示更高。正式八轮、各 block 与补充 BAAB 分别计算，不池化。

| 64 KiB 对照 | RTT p50 | RTT p99 | RPC/s |
| --- | --- | --- | --- |
| 正式八轮 typed | −10.71% | −15.26% | +12.62% |
| 正式 ABBA typed | −13.93% | −28.49% | +20.22% |
| 正式 BAAB typed | +22.64% | +25.79% | −13.80% |
| 独立补充 BAAB raw | −3.17% | −2.48% | +4.06% |
| 独立补充 BAAB typed | −14.80% | −26.65% | +16.81% |

补测每侧仅两轮，typed p50 的 before 为 533.300/298.479 µs、after 为
443.595/265.091 µs；p99 同样有明显跨轮漂移。固定 30 秒间隔不证明环境噪声已
消除，补测改善也不覆盖正式 BAAB 的回退。完整 48 指标、全部每轮 N/spread、
两个 block、19 项全八轮下降及其他 scope 下降记录保存在
[可移植基线](benchmarks/baselines/macos-x64-tail-latency-20261021.json)；这些观测
不是裸 Mojo Core 指标、显著性结论或其他平台的原生验收。

独立数值控制的 16 次运行仍保留 9 条 scope 下降记录，其中全八轮 3 条；15 个
codec/derive 源文件哈希相同也不能据此认定全部差异为噪声。after 稳定性运行完成
30.0015 秒、1,403,739 次请求，pending 为 0，并通过 100 次进程循环及 64 个
未完成请求的异常断连。104 个 worker 均回收，FD/Mach names 为 4/38；RSS 从
32,239,616 增至 49,942,528 字节，churn 10→100 又增长 24 KiB。有限通过不证明无泄漏。

## 0.1.0

First crates.io release of the standalone Cargo port of Chromium Mojo, pinned to
`bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`.

- Explicit loading of a matching native Mojo Core library through a versioned C ABI.
- Broker/participant initialization, child-process bootstrap, and named connections.
- Official Rust message/data pipes, shared buffers, traps, interface bindings, and
  Mojom encoding with Cargo dependency aliases.
- Bundled official Mojom parser/generator and Python dependencies; generation
  requires Python 3.10+ without a separate pip installation.
- Real macOS x86_64 two-process typed IPC validation; Windows x64 native DLL and
  Rust executable cross-compilation and linking. Windows runtime, Linux native,
  and macOS arm64 native validation remain pending.

The Cargo packages contain Rust sources and generator assets. Native libraries
are supplied separately and are not downloaded or compiled by Cargo. This is a
community-maintained SDK port, not an official Google or Chromium release.
