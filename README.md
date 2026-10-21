# Mojo IPC：官方 Core 与 Rust bindings 的 Cargo 移植

这是开发中的独立 SDK 源码工程，基线为 Chromium
`bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`。底层使用真正的官方 Mojo Core，
没有用本地队列或自定义 socket RPC 替代 Mojo。
这是社区维护的 Cargo 移植，不是 Google 或 Chromium 官方发布的 Rust SDK。

**macOS x86_64 已完成官方 Mojo Core 的原生构建和真实 Rust 双进程类型化通信验证。**
父子进程与独立启动进程的连接均已通过，运行库位于本机 `dist/x86_64-apple-darwin/`。
Windows x64 已在 macOS 上完成官方 Core DLL 交叉编译，以及 cargo-xwin 的 Rust
完整编译和链接；Windows 运行验收仍待执行。Linux、macOS arm64 已通过 Rust 编译检查，
原生构建和运行仍待验收。
当前源码的 Cargo 包版本为 `0.2.2`。已接入 GitHub Actions 四平台预构建和 Release
分发流程，首次远端构建与发布仍待执行。原生库下载和使用方式见
[预构建库说明](.github/NATIVE_WORKFLOW.md)。

`0.2.2` 增加原生库自动构建和分发，Rust API、Mojom wire 格式、Chromium pin
及原生 ABI 与 `0.2.1` 保持一致。Cargo 包发布与原生库 Release 分开执行。

`0.2.1` 保留 `0.2.0` 的 Rust API、Mojom wire 格式、Chromium pin 和原生 ABI。
接收时复用提取 handles 的原生查询结果，省去重复的 `GetMessageData`；
primary 快捷派发用具体 owned job 省去临时 `Box`，既有 FIFO、锁释放及 fallback 条件保持。
同时修复合法空 payload 的原生空指针处理，覆盖零/多个 handles 和共享/可变读取。
本轮 56 次对照中，64 KiB typed p99 的 ABBA / BAAB 中位数分别下降 1.01% / 8.41%，
但 0 B、64 B、1 MiB 仍有 p99 回退，不能视为已全面解决尾延迟。
完整结果与全部不利项见 [逐 case 对照](benchmarks/P99_BAAB_OPTIMIZATION.md)。

`0.2.0` 新增紧凑数组的公开 AST 变体，低层 `MojomValue` 的穷尽匹配和相等、哈希、
排序处理需适配，不能视为 `0.1.0` 的 Rust 源码兼容更新。codec、derive、生成器和运行时
组件应一起升级到 `0.2.0` 并重新生成协议；公开命名 AST 与自定义 Context 转换的默认语义
保留。Mojom wire 格式、上游 Chromium pin 和原生 C ABI v1 保持不变，历史 `0.1.0`
registry 对照用于验证 wire 兼容。

`0.2.0` 还加入栈上消息头编码和单事件 trap arm 路径，保留公开 owned-message 和 `arm()`
接口。它们的性能与尾延迟效果须以冻结产物的独立对照结果为准。

合格的 primary-only 接口现在可在已经异步投递的 watcher task 末尾处理消息，省去
额外的 endpoint task；用户 handler 执行前会释放 router/watcher 的回调锁。绑定
sequence、同 pipe FIFO 和异步收包边界保留，不会在发送调用栈内同步执行 handler。
associated 接口及不满足条件的路径继续使用原来的排队投递，路由一旦降级就不恢复
该快捷路径。任意其他 runner task 与 handler 之间的旧相对顺序可能改变，不能再
依赖额外的第二个 task turn。新 routing 的四项原生语义测试已在 macOS x86_64
通过；正式性能对照已完成八轮，尾延迟仍存在顺序分组分歧，不能视为已解决 p99 回退。

## 工程组成

| 目录 | 用途 |
| --- | --- |
| `native/` | Chromium GN overlay、版本化 C ABI、IO/dispatch 线程、进程 bootstrap、原生 smoke |
| `crates/mojo-sys` | 官方头文件生成的 FFI 和 46 个 thunk dispatch；显式加载原生库 |
| `crates/mojo-c-api`、`mojo-system` | 官方安全 C API 包装、句柄、管道、共享缓冲区、Trap |
| `crates/mojom-codec*`、`mojom-derive` | 官方 Mojom 二进制编解码与 derive 的 Cargo 适配 |
| `crates/mojo-sequence`、`mojo-bindings` | 原生 sequence 适配、Remote/Receiver、Rust associated router |
| `crates/mojo-ipc` | 对外 Runtime、进程连接和接口类型入口 |
| `crates/mojom-build` | 已打包官方 Python parser/generator、模板和 Python 依赖 |
| `examples/protocol` | 真实 `.mojom` 协议与双进程 Rust 示例 |
| `xtask/` | 开发者 FFI 再生成与 native overlay 安装工具 |

官方 Rust 源码保留原版权信息和 BSD 许可证。Python 工具及第三方依赖保留各自许可证，
见 `crates/mojom-build/vendor/UPSTREAM.json` 与相邻 LICENSE 文件。移植改动见 `UPSTREAM.md`。

## 不依赖 Chromium checkout 的检查

需要 Rust 1.96.0（`rust-toolchain.toml` 固定）及 Python 3.10+；CI 使用 Python 3.12。
生成器的 PLY/Jinja2/MarkupSafe 已随源码打包，不需要联网 pip install。

```sh
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
python3 -B tools/mojom/test_codegen.py
```

如果默认 `python3` 太旧，用 `MOJOM_PYTHON` 指定解释器：

```sh
export MOJOM_PYTHON=/path/to/python3.12
cargo test --workspace --all-targets --locked
"$MOJOM_PYTHON" -B tools/mojom/test_codegen.py
```

该环境变量也应保留到后续 `cargo run`。Windows 默认查找 `python`；需要覆盖时使用
`$env:MOJOM_PYTHON = 'C:\path\python.exe'`。
`.github/workflows/cargo.yml` 提供三 OS 的 Rust/Mojom CI，以及 Linux 上通过 cargo-xwin
完整链接 Windows Rust 目标的 job；
`.github/workflows/native.yml` 使用 GitHub-hosted runner，从固定 Chromium 源码和工具链
开始构建四个原生目标。手动运行可下载构建包；推送匹配 Cargo 版本的 tag 后，四平台
验收全部通过才发布 GitHub Release。完整流程见 `.github/NATIVE_WORKFLOW.md`。
两个 workflow 均未在远端执行。

## 性能与稳定性基准

`benchmarks/` 提供独立的私有 Cargo benchmark 和跨平台 Python supervisor：
类型化 Echo/批量 Push、原始消息管道对照、纯 Mojom 编解码对照、持续负载、
进程创建/关闭循环，以及持有未完成请求时的异常断连检查。

```sh
export MOJO_RUNTIME_LIBRARY="$PWD/dist/x86_64-apple-darwin/libmojo_rs_native.dylib"
python3 benchmarks/run.py --stress-seconds 60 \
  --output-dir target/benchmark-runs/local-001
```

需要 Python 3.10+；默认解释器较旧时同时设置 `MOJOM_PYTHON`。
结果包含真实完成数、RTT 分位数、应用 payload 吞吐、资源采样和运行环境。
统计口径、各平台资源采集限制和长时运行参数见 `benchmarks/README.md`。

`0.2.0` primary-dispatch 轮的 macOS x86_64 正式八轮中，64 KiB typed RTT 的 run p50 中位数为
275.6115 → 246.0965 µs，run p99 中位数为 481.6085 → 408.1355 µs。
但 p99 在 ABBA 组下降 28.49%，在 BAAB 组上升 25.79%；最后一轮 after 和
全部下降项均保留。每轮前后记录的 CPU_Speed_Limit 也发生变化，端点观测不足以
确定原因。独立补充 BAAB、数值控制与本轮 after 有限稳定性运行均已分别核验，
详见 `VALIDATION.md`。

## 从 macOS/Linux 交叉构建 Windows Rust 目标

安装完整 LLVM（`clang-cl`、`lld-link` 等在 PATH 中），然后执行：

```sh
cargo install --locked cargo-xwin --version 0.23.1
rustup target add x86_64-pc-windows-msvc
export XWIN_ARCH=x86_64
cargo xwin build --workspace --all-targets --locked --target x86_64-pc-windows-msvc
```

本机 macOS x64 已实际执行通过，生成
`target/x86_64-pc-windows-msvc/debug/examples/echo.exe` 和测试可执行文件。
这比 `cargo check` 多覆盖了 Windows 链接。原生库通过 `libloading` 在运行时加载，
因此构建这些 Rust 目标无需先提供 Mojo DLL；执行类型化 IPC 时必须有匹配的 DLL。
工具用法见 [cargo-xwin](https://github.com/rust-cross/cargo-xwin)。

Mojo Core DLL 由 Chromium GN 构建。已使用微软 SDK 28000.2270、真实 CRT/ATL/PDB，
完成 macOS → Windows x64 的实际编译、链接与 PE 检查；本机产物位于
`dist/x86_64-pc-windows-msvc/mojo_rs_native.dll`。构建入口为显式
`--target-os win --build --gn-args-file <winsysroot/gn-args.gn>`，完整的 SDK 获取、
目录/环境适配、RC 和 compiler-rt 步骤见 `native/README.md`。
Windows 运行验收继续保留在现有原生 CI 中，尚未执行。

## 在业务协议 crate 中生成代码

参照 `examples/protocol`。业务协议 crate 的 `Cargo.toml` 添加：

```toml
[dependencies]
ipc = { package = "mojo-ipc", version = "=0.2.1" }

[build-dependencies]
mojom-build = "=0.2.1"
```

在仓库中开发时，也可以使用对应 `crates/` 目录的 `path` 依赖。
运行时依赖重命名为 `ipc` 后，`build.rs` 使用实际别名：

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

协议 crate 的根模块加载生成索引：

```rust
include!(concat!(env!("OUT_DIR"), "/mojom/mod.rs"));
```

所有实际 imports 会一起解析并生成到这个协议 crate，Cargo 跟踪完整依赖闭包。
普通客户端/服务端共同依赖协议 crate，无需使用 GN 编译 Rust bindings。
宿主通过 `mojo-ipc` 使用 Runtime/Remote/Receiver；底层管道和共享缓冲区 API 也可从
`ipc::system` 访问，完整 bindings 模块从 `ipc::bindings` 访问。

普通 Rust/Tokio 宿主线程绑定接口时，需要显式指定 SDK 的 dispatch runner：

```rust,ignore
let remote = pending_remote.bind_with_options(Some(runtime.dispatch_runner()), None);
let receiver = pending_receiver.bind_with_options(
    service,
    Some(runtime.dispatch_runner()),
    None,
);
```

默认 `bind()` 要求当前线程已有 Chromium sequence，普通 Rust 线程不满足此条件。
服务方法和回调在原生 dispatch 线程执行；连接、接受 invitation 和等待进程退出等阻塞
操作应放在宿主的阻塞线程，避免占用 dispatch 或异步执行器工作线程。

## 构建真正的原生库

SDK 维护者可通过 GitHub Actions 自动准备固定版本的 Chromium 源码和平台构建依赖，
验收通过后分发预构建库。包内含动态库、C 头文件、构建与验收记录及依赖许可证，
下载后校验 `SHA256SUMS`，再设置 `MOJO_RUNTIME_LIBRARY` 使用。
[构建、发布与下载步骤](.github/NATIVE_WORKFLOW.md)包含各平台目标和版本化下载地址。

本地源码构建仍需要提前准备完整 Chromium checkout 及官方平台依赖：

```sh
python3 native/scripts/build_native.py \
  --chromium-src /path/to/chromium/src \
  --smoke \
  --artifact-dir /path/to/mojo-native-artifact
```

脚本先检查 Chromium revision，再安装 GN overlay，仅构建 SDK runtime/smoke 目标。
它不会重置 checkout 或自动下载整份 Chromium。完整步骤、macOS launchd fixture 和
三平台差异见 `native/README.md`。

## 运行双进程类型化示例

原生库通过上述 smoke 后，设置实际库路径：

```sh
export MOJO_RUNTIME_LIBRARY=/path/to/libmojo_rs_native.dylib
cargo run -p mojo-example-protocol --example echo
```

Linux 使用 `.so`，Windows 使用 `mojo_rs_native.dll`；PowerShell 可用
`$env:MOJO_RUNTIME_LIBRARY = 'C:\path\mojo_rs_native.dll'`。

示例由 broker 启动同一可执行文件作为 participant，使用原生 Invitation 建连，验证：

* 生成的 Echo 请求/响应；
* 含中文、数组、枚举、可空字段的 Record；
* `pending_remote<Observer>` 端点跨进程传递与回调；
* 关闭 Echo 后子进程收到断连并退出。

该示例已在 macOS x86_64 使用实际 GN 构建的原生库运行通过。
其他平台仍需构建各自的原生库并执行验收。

## 生命周期与当前边界

* 原生库、Core、IO/dispatch 线程保持到进程退出；没有公开 shutdown/unload/restart。
* 每个进程固定一个 broker/participant 角色；broker 执行 spawn/listen，participant 执行
  accept/connect。类型化接口在建立的 pipe 上双向通信。
* `Runtime::load` 是 unsafe：调用者必须提供可信的、匹配固定 revision 的 SDK 库。
  版本字符串检查不等于验证任意动态库的可信性。
* `accept_inherited` 是 unsafe：token 必须来自真实 SDK spawn helper，并且只能消费一次。
  普通字符串不代表合法、独占的原生句柄所有权。
* `spawn_worker` 的可执行文件路径和参数要求 UTF-8，且不能含 NUL。
* 接口处理保持 sequence 串行分发；callback API 沿用上游，尚未提供 Future 客户端层。
* 断连会释放 pending success callbacks 并调用 disconnect handler，不伪造成功响应。
* Mojom 语义属性目前只允许 `EnableIf/EnableIfNot`；不支持的 Sync、MinVersion 等明确报错。
* 自定义 typemap、跨 Cargo crate schema imports、协议版本协商和 SDK peer handshake 仍待实现。
* macOS named listener 使用 Mach bootstrap namespace。本机普通 CLI 的完整 Mojo 连接已通过；
  namespace 策略限制下可采用 launchd `MachServices` 部署，该部署 fixture 尚未注册实测。
* Rust associated router 已保留，其原生跨进程 FIFO/断连行为仍需集成验收。
* macOS x86_64 已检查导出符号、系统动态依赖和动态加载前后的默认 malloc zone；
  Windows x64 已完成交叉构建与 PE 导出/静态导入检查，运行验收仍待完成。
  Linux/macOS arm64 原生目标、并发/崩溃/长时间运行测试仍待完成。

实际已执行的检查与未执行项见 `VALIDATION.md`。

## 0.2.0 尾延迟对照摘要（历史记录）

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
