# Mojo IPC：Rust SDK 与本机应用服务网络

基于 Chromium 官方 Mojo Core 和 Rust bindings 的 Cargo 移植，面向 Rust 进程间通信。
SDK 提供类型化 Mojom 接口、消息管道、共享缓冲区和进程连接；实验性的 `mojo-service`
在此之上提供应用进程监管、服务发现与故障恢复。

本项目由社区维护，与 Google 或 Chromium 官方发布无关。
当前源码版本为 `0.3.0`，固定 Chromium revision 为
`bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`，原生 ABI 为 1。
未发布改动与历史版本见 [CHANGELOG.md](CHANGELOG.md)。

[快速开始](#快速开始) · [服务网络](#应用进程服务网络) ·
[业务接入](#接入业务协议) · [平台状态](#平台与验证状态) ·
[开发维护](#开发与维护) · [文档索引](#文档索引)

## 项目能力

| 层次 | 提供的能力 | 入口 |
| --- | --- | --- |
| IPC SDK | 官方 Mojo Core、消息管道、共享缓冲区、进程 bootstrap | [`mojo-ipc`](crates/mojo-ipc/README.md) |
| 类型化接口 | Mojom 代码生成、二进制编解码、Remote/Receiver、接口端点传递 | [`mojom-build`](crates/mojom-build/README.md) |
| 应用服务网络（实验性） | 服务注册与发现、访问授权、变化订阅、调用超时与取消、进程监管与健康检查 | [`mojo-service`](crates/mojo-service/README.md) |

宿主为接入进程分配身份和权限，管理器负责发现服务、转交接口端点。
连接建立后，应用通过独立的 Mojo 业务管道直接通信；管理器不转发业务消息。
应用可以同时提供和调用服务，服务名独立于 PID。

## 快速开始

### 检查源码

需要 Rust 1.96.0（由 [`rust-toolchain.toml`](rust-toolchain.toml) 固定）和 Python 3.10+。
代码生成器及 PLY/Jinja2/MarkupSafe 已随源码打包，无需安装额外 Python 包。
从仓库根目录执行：

```sh
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
python3 -B tools/mojom/test_codegen.py
```

这组检查不需要 Chromium checkout 或原生动态库。需要加载原生库的测试默认忽略，
应在准备好可信运行库后单独执行。

默认解释器版本过旧时，设置 `MOJOM_PYTHON`，并保留到后续构建和运行：

```sh
export MOJOM_PYTHON=/path/to/python3.12
"$MOJOM_PYTHON" -B tools/mojom/test_codegen.py
```

Windows 默认查找 `python`，可用 `$env:MOJOM_PYTHON = 'C:\path\python.exe'` 覆盖。

### 准备原生库并运行 IPC 示例

Cargo 包不包含或自动下载 Mojo Core 动态库。先按
[原生库构建与分发说明](.github/NATIVE_WORKFLOW.md)准备与 SDK、操作系统和 CPU 架构
匹配的可信运行库。自行构建时使用固定 revision 的 Chromium，步骤见
[`native/README.md`](native/README.md)。

```sh
export MOJO_RUNTIME_LIBRARY=/absolute/path/to/libmojo_rs_native.dylib
cargo run -p mojo-example-protocol --example echo
```

Linux 使用 `libmojo_rs_native.so`。Windows 使用 `mojo_rs_native.dll`，在 PowerShell 设置：

```powershell
$env:MOJO_RUNTIME_LIBRARY = 'C:\absolute\path\mojo_rs_native.dll'
cargo run -p mojo-example-protocol --example echo
```

示例启动 broker 和 participant 两个进程，验证 Echo 请求/响应、复杂 Record、
`pending_remote<Observer>` 跨进程传递及断连退出。

### 运行服务网络示例

保留上述 `MOJO_RUNTIME_LIBRARY`，运行管理器、提供方和消费方三个真实进程：

```sh
cargo run -p mojo-service-network --bin service-network
cargo test -p mojo-service-network --test native_network -- --ignored --nocapture
```

示例验证权限控制、类型化调用、提供方崩溃与自动重启，以及原消费方重新连接。
额外原生测试覆盖目录清理、监管故障、调用取消与迟到响应、订阅溢出、健康检查及
管理器异常退出后的子进程退出。完整场景见
[服务网络示例](examples/service-network/README.md)。

## 应用进程服务网络

`mojo-service` 用于管理同机应用进程及其服务，目前处于实验阶段。

| 能力 | API | 行为与约定 |
| --- | --- | --- |
| 接入与授权 | `ServiceManager`、`AccessPolicy` | 宿主指定身份，发布和调用权限默认拒绝 |
| 注册与连接 | `ServiceDirectory`、`bind_factory` | 按服务键和接口版本查找，转交类型化业务端点 |
| 动态发现 | `ServiceDirectory::watch`、`ServiceSubscription` | 原子快照加 Added/Removed 事件，支持 Future 和可超时等待 |
| 调用控制 | `CallContext`、`CallClient` | Future、超时、本地取消及未完成响应数量限制 |
| 进程监管 | `ProcessSupervisor` | 启动就绪检查、有限重启预算、固定退避、退出宽限期与回收 |
| 持续健康检查 | `HealthPolicy`、`bind_health` | 默认关闭，连续失败达到阈值后按既有策略回收并重启 |

宿主在可阻塞线程定期调用 `ProcessSupervisor::poll()` 驱动监管和健康检查。
提供方重启后，消费方按订阅事件发现新 generation，并显式建立新的业务连接。
已发送的业务请求不会自动重放；本地取消不撤销远端操作，底层响应槽在回复或断连前仍占用额度。
订阅队列有容量上限，溢出返回 `ResyncRequired`，需重新订阅并取得新快照。

底层 Remote/Receiver 保留 callback API；可选的 `CallClient` 为业务调用提供 Future 层。
协议、生命周期及完整用法见 [`mojo-service` 文档](crates/mojo-service/README.md)。

## 接入业务协议

将共享 `.mojom` 定义放在独立协议 crate，让客户端和服务端共同依赖它。
可直接参照 [`examples/protocol`](examples/protocol)。

从本仓库源码接入时，在协议 crate 的 `Cargo.toml` 中配置本地路径，
将 `/path/to/mojo-ipc` 替换为实际 checkout 路径：

```toml
[dependencies]
ipc = { package = "mojo-ipc", path = "/path/to/mojo-ipc/crates/mojo-ipc" }

[build-dependencies]
mojom-build = { path = "/path/to/mojo-ipc/crates/mojom-build" }
```

使用已发布的 Cargo 包时，SDK 组件与 `mojom-build` 应采用匹配的版本；
源码版本和原生 GitHub Release 的发布状态需分别确认。

在 `build.rs` 中指定 schema 和依赖别名：

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    mojom_build::Config::new()
        .source_root("schema")
        .source("echo.mojom")
        .runtime_crate("ipc")
        .generate()?;
    Ok(())
}
```

在协议 crate 的 `src/lib.rs` 加载生成索引：

```rust
include!(concat!(env!("OUT_DIR"), "/mojom/mod.rs"));
```

imports 会在同一协议 crate 内解析和生成，无需用 GN 编译 Rust bindings。
普通 Rust/Tokio 宿主线程绑定接口时，显式使用 SDK 的 dispatch runner：

```rust,ignore
let remote = pending_remote.bind_with_options(Some(runtime.dispatch_runner()), None);
let receiver = pending_receiver.bind_with_options(
    service,
    Some(runtime.dispatch_runner()),
    None,
);
```

服务方法和回调在原生 dispatch 线程串行执行。阻塞连接、等待子进程退出等操作应放到
宿主的可阻塞线程。生成器配置与限制见 [`mojom-build`](crates/mojom-build/README.md)，
接口绑定与所有权见 [`mojo-ipc`](crates/mojo-ipc/README.md)。

## 平台与验证状态

以下汇总本地已执行的验证，更新于 2026-10-22。CI 配置覆盖目标不等于已完成运行验收。

| 目标 | 已有验证 | 仍待完成 |
| --- | --- | --- |
| macOS x86_64 | 官方 Core 原生构建、父子和独立进程连接、Rust 类型化 IPC、历史性能与稳定性测试 | 本轮服务网络运行验收 |
| macOS arm64 | 使用可信本地原生库，完成 7 项服务网络原生测试 | 全量 SDK、依赖审计和发行打包验收 |
| Windows x86_64 MSVC | Core DLL 交叉构建及 PE 检查，历史 Rust workspace 编译和链接 | Windows 原生运行及服务网络验收 |
| Linux x86_64 GNU | 历史 Rust 编译检查 | 原生运行及服务网络验收 |

最新普通工作区测试为 **146 项通过**；另在 macOS arm64 执行的 **7 项服务网络原生测试通过**。
原生工作流已纳入服务网络验收门禁，本轮未执行远端 CI。各轮证据、平台限制及未执行项
统一记录在 [VALIDATION.md](VALIDATION.md)。

## 开发与维护

### 原生构建与发布

[`native.yml`](.github/workflows/native.yml) 配置了四个平台的原生构建与验收。
手动运行可生成验收构建包；推送匹配 Cargo 版本的 tag 后，全部目标通过才发布 GitHub Release。
流程与下载方式见 [NATIVE_WORKFLOW.md](.github/NATIVE_WORKFLOW.md)。

在已有 Chromium checkout 中本地构建：

```sh
python3 native/scripts/build_native.py \
  --chromium-src /path/to/chromium/src \
  --smoke \
  --artifact-dir /path/to/mojo-native-artifact
```

脚本检查固定 revision 并安装 GN overlay，不会自动下载 Chromium 或重置 checkout。
平台依赖、Windows 原生交叉构建和 macOS launchd fixture 见 [`native/README.md`](native/README.md)。

### Windows Rust 交叉编译

安装包含 `clang-cl`、`lld-link` 的完整 LLVM，然后在 macOS/Linux 上执行：

```sh
cargo install --locked cargo-xwin --version 0.23.1
rustup target add x86_64-pc-windows-msvc
export XWIN_ARCH=x86_64
cargo xwin build --workspace --all-targets --locked --target x86_64-pc-windows-msvc
```

该命令覆盖 Rust 编译和链接。运行时仍需匹配的 Mojo DLL，并在 Windows 上实际执行验收。
普通 Cargo CI 与交叉链接 job 见 [`cargo.yml`](.github/workflows/cargo.yml)。

### 性能与稳定性

设置 `MOJO_RUNTIME_LIBRARY` 后，从仓库根目录运行：

```sh
python3 benchmarks/run.py --stress-seconds 60 \
  --output-dir target/benchmark-runs/local-001
```

每次使用新的结果目录。基准覆盖类型化 Echo/批量 Push、原始管道与纯 codec 对照、
持续负载、进程生命周期及异常断连，输出 RTT 分位数、吞吐、资源采样与环境信息。
统计口径、历史结果及各平台采样限制见 [`benchmarks/README.md`](benchmarks/README.md)。

## 工程结构

| 路径 | 职责 |
| --- | --- |
| [`native/`](native) | Chromium GN overlay、版本化 C ABI、IO/dispatch 线程与原生进程 bootstrap |
| [`crates/mojo-sys`](crates/mojo-sys) | 官方头文件生成的 FFI、thunk dispatch 与显式原生库加载 |
| [`crates/mojo-c-api`](crates/mojo-c-api)、[`mojo-system`](crates/mojo-system) | 安全 C API 包装、句柄、消息管道、共享缓冲区与 Trap |
| [`crates/mojom-codec-core`](crates/mojom-codec-core)、[`mojom-codec`](crates/mojom-codec)、[`mojom-derive`](crates/mojom-derive) | 官方 Mojom 编解码与 derive 的 Cargo 适配 |
| [`crates/mojo-sequence`](crates/mojo-sequence)、[`mojo-bindings`](crates/mojo-bindings) | 原生 sequence 适配、Remote/Receiver 与 Rust associated router |
| [`crates/mojo-ipc`](crates/mojo-ipc) | 对外 Runtime、进程连接和接口类型入口 |
| [`crates/mojo-service`](crates/mojo-service) | 实验性本机服务网络与进程监管 |
| [`crates/mojom-build`](crates/mojom-build) | 官方 Python parser/generator、模板和依赖 |
| [`examples/protocol`](examples/protocol)、[`examples/service-network`](examples/service-network) | 双进程 IPC 与三进程服务网络示例 |
| [`benchmarks/`](benchmarks) | 性能、资源采样与稳定性基准 |
| [`xtask/`](xtask) | FFI 再生成与 native overlay 安装工具 |

## 当前边界

- 原生库、Mojo Core 和 IO/dispatch 线程保留到进程退出，没有公开的 shutdown、unload 或角色切换。
- 每个进程固定 broker/participant 角色；类型化接口可在建立的 pipe 上双向通信。
  `Runtime::load` 与 `accept_inherited` 的安全前提、句柄生命周期见 [`mojo-ipc`](crates/mojo-ipc/README.md)。
- 生成器仅支持 `EnableIf/EnableIfNot` 语义属性；不支持的 `Sync`、`MinVersion` 等会显式报错。
  自定义 typemap、跨 Cargo crate 的 schema imports、SDK peer handshake 和协议版本协商尚未提供。
- 服务网络目前同机运行，同一服务键只允许一个提供方；尚无管理器恢复、依赖启动编排、
  多实例负载均衡、操作系统对端认证、进程资源限制或全局配额。
- macOS launchd fixture 注册实测，以及 associated interface 的原生跨进程顺序与断连验收仍待完成。

## 文档索引

| 需求 | 文档 |
| --- | --- |
| 使用 IPC SDK 与管理连接生命周期 | [`mojo-ipc`](crates/mojo-ipc/README.md) |
| 生成业务协议代码 | [`mojom-build`](crates/mojom-build/README.md) |
| 接入本机服务网络 | [`mojo-service`](crates/mojo-service/README.md) |
| 运行和理解服务网络示例 | [示例说明](examples/service-network/README.md) |
| 构建、验收、分发原生库 | [工作流说明](.github/NATIVE_WORKFLOW.md)、[原生构建](native/README.md) |
| 运行性能与稳定性基准 | [基准说明](benchmarks/README.md) |
| 核对已验证范围与版本变更 | [验证记录](VALIDATION.md)、[更新记录](CHANGELOG.md) |
| 了解官方源码与移植改动 | [UPSTREAM.md](UPSTREAM.md) |

官方 Rust 源码保留原版权信息与 BSD 许可证。Python 工具及第三方依赖保留各自许可证，
见 [`UPSTREAM.json`](crates/mojom-build/vendor/UPSTREAM.json) 及相邻 LICENSE 文件。

### 自定义 Chromium 来源

SDK 默认使用上述官方 Chromium 提交。用户可用自己的 Git checkout，通过原生构建器的 `--chromium-src`、`--chromium-revision` 选择源码；加载时通过 `Runtime::load_with_config` 明确传入同一提交。默认 `Runtime::load` 的官方版本检查保留，ABI 和布局兼容性仍需审核。详见 [原生构建参数](native/README.md#custom-source)和 [Rust 加载配置](crates/mojo-ipc/README.md#custom-chromium-checkout)。
