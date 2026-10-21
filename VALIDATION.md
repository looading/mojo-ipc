# 实施与验证记录

记录更新：2026-10-21（原生基础运行验证为 2026-10-20，benchmark 为 2026-10-21）。Chromium 固定为
`bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`，SDK ABI 为 1。

## 当前结论

官方 Rust bindings 的 Cargo 移植、官方 Mojom 生成链路与原生 C ABI 已落地。
本次已下载固定版本 Chromium 源码，完成 DEPS 同步、上游 hooks 和 macOS x86_64
原生构建。真实父子进程、独立启动进程及 Rust Mojom 类型化通信均已运行通过。
Windows x64 已完成从 macOS 的官方 Core DLL 交叉编译、最终链接与 PE 检查，
Rust 目标也已由 cargo-xwin 完整编译和链接；Windows 运行验收仍待执行。
Linux、macOS arm64 当前通过 Rust 编译检查，原生构建和运行仍待验收。
当前工程是可继续集成的开发版，尚不能作为已经完成三平台验收的发行版。
Cargo SDK 的 `0.1.0` 已发布到 crates.io，原生库仍由使用者单独提供。

## 0.2.0 primary dispatch：实现与已验收边界

本轮新增 primary-only dispatch 候选：只有从未注册 associated endpoint、仍有
唯一且已绑定的 primary、没有排队 backlog/关闭状态，并且当前位于相同绑定
runner 时，才在已投递的 watcher task 末尾执行 owned handler。执行前释放
router/watcher 回调锁；每条消息处理完后再 rearm。任何 fallback、associated
注册或相关状态变化都会永久关闭资格，后续使用既有 posted scheduler。

同 pipe FIFO、绑定 sequence 与异步收包边界保留，handler 不会在发送 Write
调用栈内同步执行。任意 runner task 的旧 interleaving 则发生可见变化：旧的
`W,T,H`（watcher、其他 task、endpoint handler）可以变为 `W(H),T`，不能再
依赖额外的第二 task turn。associated endpoint 的未绑定 backlog 仍阻塞后续
同 pipe 消息，之后 bind 才按既有路径放行。

已新增 `crates/mojo-bindings/src/multiplex_router/native_dispatch_tests.rs`，
以真实 native pipe/dispatch sequence 覆盖下列场景。修订测试已在 macOS
x86_64 真实运行，保存日志为 `native-routing-tests-fixed.log`，4 passed、
0 failed、0 ignored；它们与普通 workspace、header/arm 的记录分别保留：

| 新原生测试 | 状态 |
| --- | --- |
| 256 条 primary 消息 FIFO，最后一条之后才断连 | macOS x86_64 通过 |
| handler 内注册/丢弃 associated 并发送响应，之后保持永久 fallback | macOS x86_64 通过 |
| cfg(test) per-router queue observer 证明实际 backlog，未绑定 associated 阻塞 primary，bind 后放行 | macOS x86_64 通过 |
| handler 返回前丢弃最后 watcher owner，peer 立即观察关闭 | macOS x86_64 通过 |

首次 backlog 测试的信号观察法已发现固定上游限制：
`mojo/core/ipcz_driver/mojo_trap.cc:228–233` 明确将
`TriggerWhenUnsatisfied` 视为不支持的旧功能，底层使用 `IPCZ_NO_FLAGS`；
注册可以被接受，但不会按该条件通知。该次等待失败不构成新 routing 的失败证据，
也不能使用此信号证明 backlog 已正确入队。失败日志保留；修订的测试通过
`cfg(test)` 的 per-router queue observer 观察实际队列，发布构建不含该观察器，
原生实现未作修改。初始失败和修订后通过的日志、信号限制及修正原因分别保留。

新测试捕获 native callback 中的断言错误并传回测试线程，避免跨 native ABI
展开。测试不依赖旧 second-turn 的任意 task 顺序。新 routing 的冻结产物已
完成下述正式八轮；已留存的旧 combined header/arm profile 与候选诊断不归属于
这一新 routing，也不能用来解释新路径的性能。

### 本轮正式性能记录与限制

本轮在 macOS x86_64 使用同一原生库、共同 15 文件 harness 和独立 target
构建的 before/after，执行原 legacy loop 八轮 ABBA + BAAB。严格汇总为
complete、errors 为空，共 48 个主指标。数据位于
`target/validation/tail-routing-formal/`，独立汇总位于
`target/validation/tail-routing-summaries/formal/summary.json`。

| 64 KiB typed RTT | 全八轮 before → after | ABBA 变化 | BAAB 变化 |
| --- | --- | --- | --- |
| p50 | 275.6115 → 246.0965 µs（−10.7089%） | −13.9302% | +22.6359% |
| p99 | 481.6085 → 408.1355 µs（−15.2558%） | −28.4939% | +25.7909% |

这些是各 label 四次 run 的统计量中位数，各 block 每侧仅两次；不是池化分位数、
置信区间或显著性结论。p99 的两个 block 方向相反，不能声称已解决尾延迟回退。
第 7 轮 after 的 64 KiB p50/p99 为 408.459/666.819 µs，保留参与汇总。
严格汇总中的 75 条下降记录为全八轮 19 条、ABBA 11 条、BAAB 45 条的合计，
并非 75 个独立指标；所有下降项均保留。

每轮前后的 `pmset` CPU_Speed_Limit 端点从 100 经 88、77 到 75，最后一轮
为 75 → 46。这是环境稳定性限制，不能替代连续频率/温度 trace，不能据此确定
环境是全部差异的原因，也不能排除实现相关退步。单机结果不扩大其他平台的运行
验收范围。补充结果不会池化进或替换上述八轮；数值控制与本轮 after 有限稳定性
运行也已分别核验，见下文。

### 独立补充 BAAB

正式八轮之后预先固定单组 BAAB，四轮各测 64 KiB raw/typed 各 5 秒，轮间固定
30 秒静默，不按 CPU 速度端点或结果重试、跳选。运行与六项汇总核验已完成，
strict summary errors 为空、该 scope 六项无下降记录。保存于
`target/validation/tail-routing-supplemental-baab/`，与正式八轮及先前独立 ABBA
focused 分别保留。

| 64 KiB 补充对照 | p50 变化 | p99 变化 | RPC/s 变化 |
| --- | --- | --- | --- |
| raw | −3.1749% | −2.4752% | +4.0648% |
| typed | −14.7988% | −26.6476% | +16.8052% |

每侧仍仅两轮。typed p50 before 为 533.300/298.479 µs，after 为
443.595/265.091 µs；p99 before 为 1013.573/440.751 µs，after 为
670.750/396.032 µs，全部逐轮值与 N 保留。raw 也有明显跨轮 spread。
这些改善不能覆盖正式 BAAB 的 p99 +25.7909%，不能据静默间隔认定环境噪声已
消除，也不能确定实现或环境的因果贡献。可移植 JSON 基线同时保留两组数据、
正式全部 48 指标及 75 个 scope 下降记录，未合并统计。

### 独立数值控制与 after 稳定性

数值控制对 `nullable-u64-64k` 和 `u8-1m` 各执行独立八轮 ABBA+BAAB，共 16 次
codec-only 运行、10 个主指标。严格检查 source/build/native/checksum 绑定后通过，
保留全部 9 条 scope 下降记录：全八轮 3 条、ABBA 1 条、BAAB 5 条。双方 15 个
codec/derive 源文件哈希相同；仍保留 `u8-1m` 的小幅下降及 BAAB p99 明显下降项，
不把它们删除或确定归因于噪声。这一控制不池化进 IPC 的正式/补测分位数。

after 稳定性运行完成 30.0015 秒，1,403,739 sent = completed，最终 pending 为 0；
100 次 churn 通过，异常断连时 64 个 held callback 全部释放、success 为 0、最终
pending 为 0、worker 按预期退出 17。104 个 worker 均回收（103 个退出 0、1 个
退出 17），cleanup 为空。14 个静置检查点均无 active worker，FD 为 4、Mach
namespace 名字数为 38；未采集的 thread/handle 仍为 null。

RSS 由首检查点 32,239,616 增至末检查点 49,942,528 字节；churn 10→100
仍增长 24 KiB。离散点不是进程真实峰值，稳定的 FD/Mach 名字数也不覆盖所有引用、
内核或 native 资源，不能据有限运行声称零泄漏或长期增长有界。完整记录分别位于
`target/validation/tail-routing-numeric/`、`target/validation/tail-routing-stability-after/`。

## crates.io 0.1.0 首发

2026-10-21（北京时间）完成全部 10 个 crate 的首次发布：`mojo-sys`、
`mojo-c-api`、`mojo-sequence`、`mojo-system`、`mojom-derive`、
`mojom-codec-core`、`mojom-codec`、`mojo-bindings`、`mojom-build`、`mojo-ipc`。
主入口为 <https://crates.io/crates/mojo-ipc/0.1.0>，生成器为
<https://crates.io/crates/mojom-build/0.1.0>。xtask 和仓库示例未发布。

发布前，全 workspace 的 publish dry-run 和独立包编译验证通过；Rust 10 项测试、
Mojom 生成器 6 项测试再次通过。内部依赖精确锁定 `=0.1.0`，各包携带 README
和 LICENSE，生成器携带全部 vendor 资源与第三方许可证。

发布后，通过公开 crates.io API 确认 10 个版本均未被 yank，SHA-256 与本地上传包
逐一相符。另在源码仓库外创建协议 crate，仅使用注册表依赖，真实执行生成器并在
macOS x86_64 上运行双进程 Echo。类型化请求/响应、导入 Record、接口端点传递和
关闭流程通过；Cargo metadata 确认全部 10 个 SDK 包均来自 crates.io，无本地
path 或 patch 覆盖。该验证没有扩大 Windows/Linux/macOS arm64 的原生验收范围。

发布日志、公开包校验记录和独立消费者回执位于被 Git 忽略的
`target/validation/crates-io-0.1.0/`。

## 已执行

宿主为 macOS x86_64；项目使用 Rust 1.96.0、Python 3.12。早期检查使用任务目录
`work/rust-toolchain` 下的隔离工具链，通过包装脚本配置工具链、Python 与 libclang。
2026-10-21 已按官方 rustup 流程安装到用户常规 `~/.cargo`、`~/.rustup` 并配置 PATH；
默认 stable 为 1.99.1，项目按 `rust-toolchain.toml` 自动使用 1.96.0。
本次发布预检、Rust 测试、上传及独立消费者验证均使用常规安装的 Cargo。

| 检查 | 结果 | 覆盖范围 |
| --- | --- | --- |
| `cargo check --workspace --all-targets --locked` | 通过 | 所有 crate、官方生成的协议与 Rust 双进程示例的编译检查 |
| `cargo test --workspace --locked` | 106 项通过、6 项原生 opt-in 单列 | 本轮完整 workspace 普通测试；ignored 不计入 106 项，6 项原生测试分别执行并保留独立回执 |
| native Trap、message parts 与 routing 调度 | 1 + 1 + 4 项通过 | macOS x86_64 真实 native；routing 固定后日志为 4 passed、0 failed、0 ignored |
| `python3 -B tools/mojom/test_codegen.py` | 6 项通过 | 官方 parser/generator、重命名 runtime、完整 imports 跟踪、模块名冲突、属性拒绝与目标平台条件 |
| `cargo fmt --all --check` | 通过 | Rust 格式检查 |
| `cargo xtask gen-sys` | 通过 | 从固定官方 C headers 和 SDK header 再生成类型及 46 个 thunk forwarding 函数 |
| `cargo package --list -p mojom-build --allow-dirty --locked` | 通过 | parser、generator、模板、Python 依赖与许可证均进入 Cargo 包清单；实际发布结果见首发记录 |

以下目标都执行并通过了 `cargo check --workspace --all-targets --locked --target <target>`：

* `x86_64-unknown-linux-gnu`
* `x86_64-pc-windows-msvc`
* `aarch64-apple-darwin`

加上宿主 `x86_64-apple-darwin`，已覆盖四种 Rust target 的类型与条件编译检查。
交叉检查没有链接或运行对应平台的原生库，不能据此认定 Linux/Windows 原生支持已验收。
本机检查日志保存在被 Git 忽略的 `target/validation/` 下。

## Windows Rust 交叉编译与链接

在 macOS x86_64 上实际执行：

```sh
cargo xwin build --workspace --all-targets --locked --target x86_64-pc-windows-msvc
```

结果通过，使用 Rust 1.96.0、cargo-xwin 0.23.1、Chromium 配套 LLVM 24，
下载 CRT/SDK 并完成所有目标的链接。产物包含 Echo 示例、xtask 和各 crate 的测试程序。
`file` 与 `llvm-readobj` 确认 Echo 为 PE32+、COFF x86-64、AMD64 可执行文件。
其导入表含 Windows 系统 API、UCRT 和 `VCRUNTIME140.dll`。
日志为 `target/validation/cargo-xwin-windows-build.log`，构建记录与 14 个 PE 文件的
SHA-256 位于 `target/validation/cargo-xwin-windows-build.json`；未执行这些 Windows 程序。

xwin 的本次默认 VS17 包含 SDK 26100、CRT 14.44（headers build 35220）。
该默认 SDK 不含固定 Chromium 要求的 `NTDDI_WIN11_BR`；后续原生构建改用微软
官方 NuGet SDK 28000.2270，保留真实 xwin CRT，并补齐 ATL、PDB、RC 和 compiler-rt。
`build_native.py` 已加入显式 Windows 交叉构建入口，跨 OS 的 `--smoke` 提前拒绝。

新增入口通过 Python AST/CLI help 和 5 条实际 CLI 拒绝路径检查，拒绝时不会写入
checkout overlay。5 组隔离命令计划检查属于 mock，不是 Windows GN 构建结果。
对应记录为 `target/validation/windows-target-guards.json` 和
`target/validation/windows-target-command-plan.json`。
修改后以原 macOS 参数重新执行真实 GN 构建与 native smoke，再次通过（0 项重编译）；
日志为 `target/validation/native-macos-cross-entry-regression.log`，没有重打包现有 dist 库。

`.github/workflows/cargo.yml` 新增 Ubuntu 上的 cargo-xwin 完整链接 job，
本地 YAML 解析通过；远端尚未执行。Windows 的原生运行验收 job 继续保留。

## Windows x64 原生 DLL 交叉构建

2026-10-21 在 macOS x86_64 上执行完整依赖编译并完成最终 DLL 链接。
固定 Chromium Clang/LLD 24，主机工具使用系统 Xcode/SDK；目标使用 SDK
10.0.28000.0（官方 NuGet servicing 版 10.0.28000.2270）、CRT 14.44.35220
和真实 ATL。三包 SDK 的固定 SHA-512、官方 NuGet catalog 记录和解压结果均已核对。

| 检查 | 结果 | 覆盖范围 |
| --- | --- | --- |
| `build_native.py --target-os win --target-cpu x64 --build --gn-args-file …` | 通过 | 实际 GN 生成、Windows base/Perfetto/ipcz/Mojo Core 与 SDK runtime 编译、LLD 最终链接 |
| 同一入口增量复验 | 通过 | 0 项重编译，DLL SHA-256 保持一致 |
| PE 文件头 | 通过 | AMD64、PE32+、DLL 标志，文件大小 2,537,984 字节 |
| 导出检查 | 通过 | SDK ABI 为 `MojoRsGetApiV1`；另保留上游内部 `GetHandleVerifier`，未作为 SDK ABI 暴露给 Rust |
| 静态导入检查 | 通过 | 仅 KERNEL32、ADVAPI32、dbghelp、ntdll、USER32、SHELL32、USERENV、WINMM、ole32 等系统 DLL；没有配套 Chromium 或 VC runtime DLL 导入 |
| SDK 获取与适配器 | 通过 | 缓存 SHA-512、内容原位比对、无写入复验；真实 x86/x64 COFF 检查、旧 SDK/错误架构/缺 ATL 拒绝、ATL 安全追加与预处理 |

产物位于 `dist/x86_64-pc-windows-msvc/mojo_rs_native.dll`；SHA-256：
`c0da3bd2442a87e7a3451b33064ad5ddf9d53e0094b008371219fd518b80d256`。
库旁保存原生 manifest、构建记录、PE 检查、工具链来源与 SDK/CRT/ATL/PDB 证据。
`target/validation/native-windows-x64/` 保留本次构建与工具链验证日志。

工具链采用显式 VS/SDK 配置；仅将上游 `visual_studio_runtime_dirs` 的空列表修成空字符串，
使其满足 `exec_script` 参数类型。没有伪造 SDK API 或降低 SDK 版本检查。
编译器检查和链接器 `/WX` 均保持；通过补齐与 CRT 库哈希匹配的微软 PDB 解决
LNK4099，没有屏蔽该警告。准备期间的 SDK 文件身份变化通过固定输入、重建 PCM 解决；
获取脚本已改为原位比对/复用，实际只读复验确认 inode、mtime、ctime 不变。

以上是交叉构建与静态检查结果，未在 Windows 执行 DLL、原生 smoke 或类型化 IPC。
原生 manifest 的 `native_smoke_passed` 保持 `false`，不生成 Windows 运行验收通过的记录。

## macOS x86_64 原生运行

使用上游配套 Clang 24、系统 Xcode/SDK，执行了真正的 GN 配置、编译和链接。
首次完整构建完成 2522 项任务，随后通过同一构建入口复验，增量构建无需重新编译。
GN 配置采用非 component release，关闭 allocator shim、PartitionAlloc-as-malloc、
BackupRefPtr 和 alloc token，避免动态加载 SDK 时替换宿主进程的全局分配器。

| 检查 | 结果 | 覆盖范围 |
| --- | --- | --- |
| `build_native.py --smoke` | 通过 | Core 初始化、原生线程、父子进程 Invitation、message pipe 端点和 shared buffer 传递/修改、正常退出 |
| `native/scripts/named_smoke.py` | 通过 | 独立启动的两个 CLI 进程通过 Mach bootstrap 建连，传递 pipe/shared buffer，双方退出码为 0 |
| `cargo run --locked -p mojo-example-protocol --example echo` | 通过 | 真实 Rust 双进程的中文请求/响应、imported Record、数组/枚举/可空字段、`PendingRemote<Observer>` 端点与回调、断连退出 |
| 非 Unicode 宿主参数复验 | 通过 | `inherited_token()` 使用 `args_os()`，忽略无关非 Unicode 参数；携带非法 UTF-8 参数启动实际 echo，完整类型化通信仍通过 |
| `nm` 导出检查 | 通过 | 唯一导出符号为 `_MojoRsGetApiV1` |
| `otool` 动态依赖检查 | 通过 | 除 dylib 自身 ID 外仅依赖 `/System/Library`、`/usr/lib` 下的系统库；最低 macOS 13.0，构建 SDK 26.5 |
| 实际宿主动态加载检查 | 通过 | Python 宿主用 `RTLD_LOCAL` 加载前后，`malloc_default_zone()` 地址保持一致 |

本机库和 manifest 位于 `dist/x86_64-apple-darwin/`；SHA-256：
`ab9bc6337bc313547b88f3b812f5b4ad138bd6e89a0c0bbc545c20227c867c49`。
`dist/` 与 `target/validation/` 均被 Git 忽略；这些是本机实测产物，没有发布或上传。
库旁的 `acceptance-manifest.json` 记录本地验收范围，`runtime-audit.json` 和
`build-provenance.json` 记录审计、精确构建参数和证据位置；typed echo 与 named 日志也已归档。
固定 Chromium checkout 位于本次任务工作目录的
`work/chromium-native/checkout/src/`，没有安装或修改全局工具链。

原生部分另外完成了以下静态/环境检查：

* SDK public header 的 C11 语法检查。
* `runtime_smoke.cc` 对固定官方 C headers 的 C++20 语法检查。
* 使用配套 Clang 24、C++23 和系统 SDK libc++ 对 smoke 进行严格语法检查；
  修复 5 处缓冲区诊断并补充 append buffer 容量检查后通过。
* `runtime.cc` 与 smoke 均通过实际 GN compiler flags 和 Chromium 插件检查，
  包括 unsafe-buffer 与 raw-ptr 检查，没有全局关闭这些检查。
* 原生构建/launchd 脚本的 Python AST 与 `--help` 检查，launchd plist 的 lint。
* macOS 上编译并执行 `native/tests/macos_checkin_probe.c`：两个兄弟 CLI 进程对随机
  Mach 服务名的 check-in/lookup 均成功。此项只证明当前宿主的 OS 行为，不涉及 Mojo Core。

`.github/workflows/cargo.yml` 已配置 Linux、macOS、Windows 的 Cargo/Mojom 测试；
`native.yml` 已配置四个目标的手动 self-hosted 原生验收，包含 native smoke、独立进程
named channel 和 Rust typed echo。新增 workflow 已通过本地 YAML 解析及其 7 段内嵌
Python 的语法检查，但没有推送仓库或触发远端 workflow，不能把配置文件视作 CI 执行结果。

## IPC benchmark 与稳定性基线

2026-10-21 增加私有 Cargo 包 `mojo-ipc-bench`、独立 Mojom 测试协议和跨平台
Python supervisor。测量完整类型化请求/响应、单向消息、原始消息传输和纯 codec，
覆盖 0 B、64 B、1 KiB、64 KiB、1 MiB；逐条核对 ID 与完整数据，保存原始日志、
JSON 和自动生成的离线 HTML 报告。工具和统计口径见 `benchmarks/README.md`。

在 macOS x86_64、release / opt-level 3、实际官方 Core dylib 上完成正式运行：

* 类型化串行 p50：64 B 为 130.397 µs，1 KiB 为 779.152 µs，1 MiB 为 929.777 ms。
* 原始消息对照串行 p50：64 B 为 56.762 µs，1 KiB 为 58.323 µs，1 MiB 为 3.070 ms。
* 1 MiB 纯 codec 编码 p50 为 55.142 ms，解码 p50 为 399.043 ms。
  对照表明当前字节数组的通用 Rust 编解码是主要瓶颈；本次未修改公开 SDK 实现。
* 60 秒、1 KiB、窗口 64 的持续负载完成 154,858 次往返；校验错误为 0，pending 清零。
* 100 轮 worker 创建、通信、关闭全部通过。14 个静置检查点 FD 均为 4；从持续负载
  结束到第 100 轮 RSS 增加 36 KiB，第 30 轮起保持不变。
* worker 持有 64 个未完成请求后按预期异常退出 17，全部回调闭包释放，成功回调为 0，
  pending 为 0。104 个 worker 全部按预期退出，无需强制清理。

完整本地记录为 `target/benchmark-runs/macos-x64-20261021/`，可移植快照为
`benchmarks/baselines/macos-x64-20261021.json`，结果分析为 `benchmarks/RESULTS.md`。
大 payload 的类型化样本较少，不能据此估计生产 p99；短时 RSS/FD 趋势也不能证明
长期无泄漏。该首轮未采集 Mach ports，后续 harness 已增加 macOS 自身任务的
Mach port 名字计数；这个指标也不覆盖同一个名字下的引用数。

补充的生命周期复测完成 1,000 轮重建：103 个静置检查点 FD 均为 4、Mach port
名字计数均为 38，探测无错误；第 10 轮到第 1,000 轮 RSS 增加 284 KiB。
1,003 个 worker 全部按预期退出，64 个待完成回调释放检查再次通过，无需强制清理。
该轮关闭持续负载，性能 case 使用单次零预热，仅用于 smoke；正式性能数字仍取
前一轮基线。完整记录位于 `target/benchmark-runs/macos-x64-churn1000-20261021/`，
快照为 `benchmarks/baselines/macos-x64-churn1000-20261021.json`。

新增 benchmark 通过 Linux GNU、Windows MSVC 的 Rust 编译检查；这两个平台的
benchmark 尚未实际运行。Windows DLL 的交叉构建结果不能替代 Windows 性能数据。

## uint8 数组优化复验

2026-10-21 将非 nullable uint8 数组改为紧凑字节 AST 表示和批量读写，保留原有
Mojom 格式、长度/指针/padding 校验、nullable/nested/fixed 和 typemap 转换。
公开 Rust `MojomValue` 新增 `ByteArray` variant，直接使用 AST 的调用方需适配；
这不是 Rust 源码无破坏变更，也未发布到 crates.io。详见 `UPSTREAM.md` 和 CHANGELOG。

24 项 workspace 测试通过，含新增 12 项 legacy/golden wire、复合类型和畸形数据检查。
另用真实 crates.io 0.1.0 和本地 codec 的两个独立消费者完成 17 组逐字节相同编码及
双向解码验证；回执与源代码位于 `target/validation/array-compat-crates-io/`。
Windows MSVC、Linux GNU 的 workspace/all-targets 编译检查和最新 release 构建通过。

与原基线相同机器、编译器、原生库、配置和采样间隔完成正式复测：

* 类型化 RTT p50：1 KiB 从 779.152 → 88.582 µs，64 KiB 从 57.745 → 0.304 ms，
  1 MiB 从 929.777 → 4.185 ms，分别约 8.80×、189.82×、222.18×。
* 1 MiB 纯 codec 编码从 55.142 ms → 42.404 µs，解码从 399.043 ms → 41.195 µs。
* 空消息 p50 本轮慢约 23.7%，raw 对照同步慢约 25.1%；未隐藏此项，也不据单轮
  推断原因。旧大消息样本少，前后速度比不是置信区间或生产尾延迟保证。
* 60 秒完成 1,742,784 次往返，校验错误 0、pending 清零；100 次 worker 生命周期、
  异常退出后 64 个回调释放检查均通过。104 个 worker 按预期退出，无需强制清理。
* 14 个静置检查点 FD 均为 4、Mach port 名字数均为 38，无活跃 worker 或探测错误；
  持续负载结束到第 100 轮重建父 RSS 增加 40 KiB，不能据此证明长期无泄漏。

完整结果位于 `target/benchmark-runs/macos-x64-array-optimized-20261021/`，比较报告为
`target/benchmark-runs/array-optimization-comparison.html`。分析及可移植快照位于
`benchmarks/ARRAY_OPTIMIZATION.md` 和 `benchmarks/baselines/`。本轮总耗时约 94.9 秒。

## 其他基础类型数组优化复验

其他基础类型数组优化已完成同日复验：bool、int8/16/32/64、uint16/32/64、
float/double 的非 nullable 元素数组使用紧凑表示，保留 LE、bool 位打包、NaN/负零位模式、
nullable/typemap/enum/handle 语义及 map/关联 ID 校验。42 项 workspace 测试通过，
Linux GNU 与 Windows MSVC workspace/all-targets 检查通过；未重新发布。

真实 crates.io 0.1.0 与本地实现的 78 组编码逐字节相同，双向解码通过；回执在
`target/validation/primitive-array-compat-crates-io/`。使用同一份冻结 harness，分别链接
uint8-only core 快照及新 core，顺序完成两轮正式 benchmark（其余条件一致）。
新增 10 型 × 3 大小共 30 组 RTT 与 30 组纯 codec；64KiB RTT p50 提升约 25–197×，
所有新增类型本轮 p50 均缩短。旧大包样本仅 8–24，尾分位仍属探索性。

两轮 30 秒负载分别完成 1,007,846 / 990,103 次往返，均无校验错误、pending 清零；
100 次重建和 64 回调释放通过，各 15 检查点 FD4 / Mach 名字数38，无需强制清理。
这部分持续负载是原 uint8 1KiB/window64，不代表对新增类型进行同等时长长期验收。
完整分析、构建来源和性能表见 `benchmarks/PRIMITIVE_ARRAY_OPTIMIZATION.md`。

## 尚未执行的关键验收

1. 在 Windows x86_64 上执行父子进程 smoke、独立进程 named channel 和 Rust typed echo，
   完成真实加载、运行依赖和 IPC 验收；当前 DLL 交叉构建与静态检查已通过。
2. 在 Linux x86_64 和 macOS arm64 完成原生构建、两类连接、typed echo 与依赖/打包检查；
   当前只完成这些 Rust target 的编译检查。
3. macOS 的 launchd fixture 注册/清理实测。
4. 补齐 associated interface 的跨进程顺序与断连、异常退出和长期运行验收。

首轮发行目标为 Linux x86_64 GNU、Windows x86_64 MSVC、macOS x86_64/arm64。
原生构建、两类进程连接和类型化接口必须逐目标通过，不能用单一宿主的 Cargo 检查替代。

## 接下来的功能工作

当前 callback API 沿用官方 bindings；断连时释放 pending success callbacks 并调用
disconnect handler。尚无逐请求错误结果或 Future 客户端层。
SDK peer handshake、协议兼容性协商、自定义 typemap、跨协议 Cargo crate 的 schema imports
以及预编译原生包分发仍待实现。暂不支持的 Mojom 语义属性会显式报错。

## 2026-10-21 位图与消息路径性能迭代

完成非可空 bool 的基线 SSE2/通用位图处理、可空元素分组和 bool 固定八项展开，
具体 struct schema OnceLock、直接字段 Vec 与原地 padding，发包分段写入新 Core
message，以及单层 Box<F> sequence task。native ABI 和 Mojom wire 不变；Rust AST
新 compact variant 的源码兼容说明见 CHANGELOG/UPSTREAM。

最终 `cargo test --workspace --all-targets --locked` 为 75 项通过、1 项 opt-in 原生
测试默认忽略；该原生测试另以可信 macOS 库真实运行通过，校验 empty/1MiB 多段消息、
不额外 commit 即可发送、handle 转移后可用与 unsent 销毁关闭。官方 generator 6 项
通过。最终 Windows/Linux/macOS arm64 全 workspace/all-targets 交叉检查通过，
macOS x86_64 之外的平台没有本轮原生性能结果。

最终 registry codec 对照为 nullable 287 + primitive 78 = 365 项逐字节相同、双向
解码及 semantic probes 通过。最终源码快照与哈希在
`target/validation/performance-round-linear-compat/`；workspace/cross logs 在
`target/validation/performance-round-linear/`，native/codegen补充在
`target/validation/performance-round/`。正式性能与局限见
`benchmarks/PERFORMANCE_OPTIMIZATION.md`；本轮没有发布新的 crates.io 版本。
