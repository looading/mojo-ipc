# 实施与验证记录

记录更新：2026-10-21（macOS 运行验证为 2026-10-20）。Chromium 固定为
`bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`，SDK ABI 为 1。

## 当前结论

官方 Rust bindings 的 Cargo 移植、官方 Mojom 生成链路与原生 C ABI 已落地。
本次已下载固定版本 Chromium 源码，完成 DEPS 同步、上游 hooks 和 macOS x86_64
原生构建。真实父子进程、独立启动进程及 Rust Mojom 类型化通信均已运行通过。
Windows x64 已完成从 macOS 的官方 Core DLL 交叉编译、最终链接与 PE 检查，
Rust 目标也已由 cargo-xwin 完整编译和链接；Windows 运行验收仍待执行。
Linux、macOS arm64 当前通过 Rust 编译检查，原生构建和运行仍待验收。
当前工程是可继续集成的开发版，尚不能作为已经完成三平台验收的发行版。

## 已执行

宿主为 macOS x86_64；使用 Rust 1.96.0、Python 3.12。Rust 工具链装在本次任务的
`work/rust-toolchain` 下，没有修改用户全局 PATH。下列 Cargo 命令通过任务包装脚本执行，
包装脚本仅设置本地工具链、Python 与 libclang 路径，不替换测试实现。

| 检查 | 结果 | 覆盖范围 |
| --- | --- | --- |
| `cargo check --workspace --all-targets --locked` | 通过 | 所有 crate、官方生成的协议与 Rust 双进程示例的编译检查 |
| `cargo test --workspace --all-targets --locked` | 10 项通过 | derive 路径适配、断连 callback 释放、sequence 查询、精确 wire 对齐与生成协议编解码 |
| `python3 -B tools/mojom/test_codegen.py` | 6 项通过 | 官方 parser/generator、重命名 runtime、完整 imports 跟踪、模块名冲突、属性拒绝与目标平台条件 |
| `cargo fmt --all --check` | 通过 | Rust 格式检查 |
| `cargo xtask gen-sys` | 通过 | 从固定官方 C headers 和 SDK header 再生成类型及 46 个 thunk forwarding 函数 |
| `cargo package --list -p mojom-build --allow-dirty --locked` | 通过 | parser、generator、模板、Python 依赖与许可证均进入 Cargo 包清单；没有发布 crate |

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
