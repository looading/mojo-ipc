# IPC 性能与稳定性测试

`0.2.1` 的 56 次逐 case ABBA/BAAB 对照、全部不利项与复现口径见
[P99_BAAB_OPTIMIZATION.md](P99_BAAB_OPTIMIZATION.md)。64 KiB typed 两组 p99
方向改善，但其他大小仍有回退；各轮历史结果单独保留。

这里测试完整 Rust SDK 的双进程、类型化 Mojom IPC：Rust 调用端、生成的协议代码、
序列化、官方 Mojo Core、跨进程传输、接收端调度以及响应回调都在测量路径中。
结果代表这套 SDK 和测试程序的端到端表现，不能直接当作裸 Mojo Core 的性能。

另外提供两组诊断对照：`raw_transport` 使用相同的双进程、原生 IO/dispatch、
Rust watcher 和 channel，但传输原始字节，不使用 Mojom 编解码或类型化 router；
`codec_only` 只在当前进程内编码和解码相同的 Echo 参数结构，计时期间不初始化原生库。
原始消息也包含分配、拷贝和调度成本，并不是零拷贝或裸 Core 上限。

`ipc/` 是私有 Cargo 包 `mojo-ipc-bench`。`run.py` 是只使用 Python 标准库的
外层控制程序，负责构建、运行、资源采样和保存结果。运行时仍需用户提供可信的、
与当前 SDK revision 及目标架构匹配的原生库。

首轮 macOS x86_64 的真实数值、编解码瓶颈和稳定性分析见 [RESULTS.md](RESULTS.md)，
其可移植 JSON 快照位于 `baselines/`。每次运行仍应保留自己的完整结果目录。
uint8 数组优化后的 wire 兼容性验证与前后实测见
[ARRAY_OPTIMIZATION.md](ARRAY_OPTIMIZATION.md)。
其他基础类型数组的实现、兼容范围与新旧 codec 对照方法见
[PRIMITIVE_ARRAY_OPTIMIZATION.md](PRIMITIVE_ARRAY_OPTIMIZATION.md)。
可空元素的 tag/value 快路径及独立复测方法见
[NULLABLE_ARRAY_OPTIMIZATION.md](NULLABLE_ARRAY_OPTIMIZATION.md)。

2026-10-21 的 primary routing/stack header/arm 尾延迟对照保存为
[可移植基线](baselines/macos-x64-tail-latency-20261021.json)，包含正式 48 指标、
每轮 N/spread、ABBA/BAAB、全部下降项与独立补充 BAAB。正式 64 KiB typed p99
在两个 block 的方向相反，补测改善不覆盖原回退；具体限制见根目录 `VALIDATION.md`。

## 运行

从仓库根目录执行。原生库通过 `MOJO_RUNTIME_LIBRARY` 指定；代码生成需要
Python 3.10+，默认解释器不满足时另设 `MOJOM_PYTHON`。

```sh
export MOJO_RUNTIME_LIBRARY="$PWD/dist/x86_64-apple-darwin/libmojo_rs_native.dylib"
# 如默认 python3 版本太旧：
# export MOJOM_PYTHON=/absolute/path/to/python3.12

python3 benchmarks/run.py \
  --output-dir target/benchmark-runs/local-001
```

`--output-dir` 必填，必须是空目录或尚不存在的新目录；每次运行换一个目录，避免
覆盖已有记录。默认先构建 release 版本，再执行
`target/release/mojo-ipc-bench`，Windows 上文件名为 `mojo-ipc-bench.exe`。
测试程序会启动真正的 participant 子进程，不能用只支持进程内消息的替代库运行。

Windows 原生运行时可使用以下 PowerShell 设置：

```powershell
$env:MOJO_RUNTIME_LIBRARY = 'C:\absolute\path\mojo_rs_native.dll'
python benchmarks/run.py --output-dir target/benchmark-runs/windows-001
```

已有 release 二进制时，可跳过构建；二进制放在其他目录时，同时指定路径：

```sh
python3 benchmarks/run.py \
  --skip-build \
  --binary /absolute/path/to/mojo-ipc-bench \
  --output-dir target/benchmark-runs/local-002
```

二进制和原生库都必须能在运行机器上执行。例如，在 macOS 上交叉编译出的 Windows
EXE 需要带到 Windows 执行。已编译的测试程序不需要在运行机器上安装 Chromium 源码；
Rust MSVC 可执行文件自身的 VC runtime 依赖仍需满足。

## 参数

以下参数由 supervisor 转发给 Rust harness：

| 参数 | 默认值 | 用途 |
| --- | --- | --- |
| `--iterations` | `5000` | 小消息测量阶段的请求数下限；大 payload 按预算降低，满足最短计时时长前仍继续发送。 |
| `--warmup` | `500` | 请求的预热次数；大 payload 按预算降低，实际次数写入结果。 |
| `--sample-ms` | `1000` | 每个性能 case 的最短计时时长，单位毫秒；设为 `0` 时仅按最少次数运行。 |
| `--stress-seconds` | `30` | 持续负载的目标时长，单位秒；设为 `0` 时关闭这一分支，基础正确性、churn 和 crash 检查仍会运行。 |
| `--churn` | `100` | 重复连接及 worker 生命周期的测试轮数。 |
| `--window` | `64` | 窗口负载允许同时在途的请求数。 |
| `--timeout-seconds` | `15` | harness 等待响应、阶段推进及退出的超时参数。 |
| `--legacy-sizes` | `0,64,1024,65536,1048576` | 筛选原 legacy 的 payload 字节数；不改变被选 case 的计时循环、次数预算和校验。 |
| `--legacy-phases` | `codec,raw,typed,oneway,correctness,stress,churn,crash` | 筛选原 legacy 阶段；关闭某项就不会得到该项的验收结果。 |

supervisor 另接受以下参数：

| 参数 | 用途 |
| --- | --- |
| `--output-dir DIR` | 必填，独立的空结果目录。 |
| `--skip-build` | 使用已有二进制，不运行 release 构建。 |
| `--binary PATH` | 覆盖默认 release 二进制路径。 |
| `--total-timeout-seconds N` | 外层运行超时；默认是 `600 + stress-seconds`，默认参数下为 630 秒。 |
| `--resource-interval-seconds N` | 周期性 RSS 采样间隔，默认 `1` 秒；设为 `0` 时禁用周期采样，静置检查点仍会采集。 |

例如，显式使用全部默认负载参数：

```sh
python3 benchmarks/run.py \
  --iterations 5000 --warmup 500 \
  --sample-ms 1000 --stress-seconds 30 \
  --churn 100 --window 64 --timeout-seconds 15 \
  --output-dir target/benchmark-runs/local-003
```

## 结果目录

| 文件 | 内容 |
| --- | --- |
| `metadata.json` | 运行参数、平台、命令等复现实验信息。 |
| `raw.log` | Rust harness 的原始输出和诊断记录。 |
| `result.json` | harness 返回的结构化性能与稳定性结果。 |
| `resources.json` | 周期性资源采样、静置检查点及采集限制说明。 |
| `build.log` | release 构建日志；使用 `--skip-build` 时不生成此文件。 |
| `report.html` | 成功运行后自动生成的离线中文报告，包含环境、性能对照、完整负载表及资源检查点。 |

也可对已有记录执行 `python3 benchmarks/report.py RESULT_DIR`，重新生成该目录的
`report.html`。报告仅用 Python 标准库，CSS 内联、不请求外网，并链接同目录 JSON
和原始日志。报告生成失败只输出 `report_failed`，不会把有效的 benchmark 数据判为失败。

比较两次运行，可生成独立的离线 HTML：

```sh
python3 benchmarks/compare.py \
  target/benchmark-runs/before target/benchmark-runs/after \
  --output target/benchmark-runs/comparison.html
```

比较工具核对 target、编译器、原生库 SHA-256、完整配置、机器等条件，并按
case / payload / window 匹配。条件不一致或缺少记录时会显式标出；速度比只是两次
观测的比值，不是统计置信度。报告保留两边的样本数、吞吐、稳定性及原始记录链接。

只复测 64 KiB 类型化串行 Echo 时，可使用以下选择器。它保留原始 Echo 的回调、
校验与析构顺序；`--resource-interval-seconds 0` 关闭计时期间的周期 RSS 轮询：

```sh
python3 benchmarks/run.py \
  --skip-build --binary /absolute/path/to/frozen-release-binary \
  --matrices legacy --legacy-sizes 65536 --legacy-phases typed \
  --iterations 1000 --warmup 100 --sample-ms 5000 --window 1 \
  --stress-seconds 0 --churn 0 --resource-interval-seconds 0 \
  --output-dir target/benchmark-runs/typed-64k-001
```

前后版本应使用独立 Cargo target 目录构建，并记录源码、同一份 harness、二进制和
原生库的 SHA-256。交错运行 ABBA 与 BAAB，分别比较每轮 p50/p99 的中位数并保留
单轮范围；不要把多轮分位数当成合并样本的分位数。编译、profiler、分配诊断和
稳定性压测应另行执行，避免与正式计时并行。

`mojo-ipc-bench --case 65536 --operation typed` 是独立诊断 probe，另支持 `raw`、
`codec-encode`、`codec-decode` 和 `codec-pair`。诊断 typed 会先校验并析构响应缓冲，
再向驱动线程通知完成，与原 legacy 的回调发布顺序不同。其 profiler、直方图或
before/after 对比必须独立报告，不与上述原 legacy 结果混合。

超时、异常退出或未完成的运行可能没有完整的 `result.json`。保留原始日志和
运行状态，不能把不完整记录作为一次成功测试。

## 统计口径

`nullable_arrays` 独立记录 11 种基础类型的可空元素、3 档预算和 3 种 None 分布，
共 99 codec + 99 typed，固定 2 次预热、至少 8 次且至少 200ms。预算含前置 tag
位图、值槽和内部对齐，不含数组头/尾对齐；选择不超过预算的最大元素数，报告目标
和真实大小，吞吐使用真实值。all_none/all_some/mixed 等长等 wire 大小，mixed
每三个位置第一个为 None。两种浮点包装均测试 bits，None 不触发 inner typemap。

新增 `primitive_arrays` 独立记录 bool、int8/16/32/64、uint16/32/64、float、double：
每型 64 B / 1 KiB / 64 KiB 数组 wire 元素数据，30 组双进程串行 Echo 和 30 组纯
codec。每组固定预热 2 次、至少 8 次且至少 200 ms，不使用原 uint8 的次数预算；
其配置单独保存在 `primitive_arrays.configuration`。Bool 按打包后的 ceil(N/8)
字节计算，float/double 按 IEEE 位模式校验，包含 NaN payload 和负零。
报告/对比工具逐类型展示元素数、真实样本数和 wire payload 口径；这不是所有类型
具有相同 Rust 内存占用。原 uint8 的参数和下述口径保持原样。

预热用于先走通建连后的分配、序列化和调度路径，预热请求不应混入正式延迟样本。
比较结果时应保持构建模式、负载参数、payload 大小、原生库和机器环境一致。

类型化 case 的请求下限为 `min(iterations, max(8, 1MiB / max(payload, 1)))`，
预热为 `min(warmup, max(2, 512KiB / max(payload, 1)))`。它们是次数预算，不是
总传输字节硬上限；运行还需达到 `sample-ms` 的最短时长。结果逐行记录请求值、
生效下限和实际完成数。原始字节和纯 codec 对照也按自己的字节预算限制最少次数。
1MiB 类型化路径当前很慢，因此默认可能只有少量样本；
`small_sample_tail_caution=true` 时，p99 仅是这批样本的分位数，不能据此估计生产尾延迟。

Echo 使用单请求串行和有界流水线窗口，逐条核对 ID、完整 payload 和完成数。
大消息窗口最多保留约 8MiB 应用 payload，结果中的 `window` 是实际生效值。
单向 Push 使用同接口上的 Inspect 响应作为批次屏障，核对接收数、字节数、顺序与内容；
它的吞吐包含每批的屏障成本。持续负载不保留无限延迟样本，只累计完成数和最大 RTT。
异常退出测试先确认子端持有 64 个未回复请求，再令该测试子进程退出 17，检查断连、
pending 清零、成功回调为零，以及这 64 个闭包恰好释放一次。

吞吐量中的 payload 字节数指应用数据。MiB 按 `1024 × 1024` 字节计算，不包含
Mojom 消息头、对齐、传输元数据或操作系统开销，也不等于网络或管道上的实际字节数。
应按对应结果字段区分请求和响应，不能把应用 payload 吞吐直接解释为 wire 吞吐。

增大窗口允许多个请求同时等待响应，会改变吞吐、排队和延迟。测量路径还包含
Rust channel、调度、回调和统计开销；Python 资源监测也会占用机器资源。
这些因素都属于本测试环境的一部分。窗口测试与逐次往返测试应分别比较。

周期采样记录父进程和当时 active workers 的 RSS；采样间隔默认一秒，由
`--resource-interval-seconds` 控制，与性能计时参数 `--sample-ms` 独立。
进程 RSS 的合计可能重复计算共享页，不能当作唯一物理内存占用。
文件描述符只在静置检查点采集，不做每秒扫描。

检查点采用握手：harness 确认没有在途 RPC、worker 已退出，静置至少 50 ms，
输出检查点后等待 stdin ACK。supervisor 在这个停顿内采集父进程可测的 RSS、
文件描述符和线程数，再发送 ACK 继续。macOS harness 还可在事件中提供
`mach_port_name_count` 与 `mach_port_probe_error`，supervisor 原样保存到
`resources.json` 的 events 和 checkpoints。这里的排空是测试程序可观察的 RPC 状态，
不是对 Mojo Core 内部所有队列或分配器状态的探测。

稳定性应观察多轮负载和连接创建/释放后的资源趋势，以及响应完整性、断连和正常退出。
RSS 可能因分配器缓存、页驻留和线程状态保留而不立即回落，不能用固定的 RSS 增量
阈值直接判定内存泄漏。macOS 的 FD 计数不包含 Mach ports，因此 FD 稳定也不代表
Mach port 数量已受检验。新增的 Mach 计数统计父 task IPC namespace 中的名字数量，
不统计每个名字持有的引用数；名字计数稳定也不等于无泄漏。非 macOS、探测失败或
未包含该探测的旧结果记为 `null` 或缺少字段，报告显示“未采集”，有探测错误时同时展示。
不会把旧记录补成 0 或视作已经通过 Mach 资源检查。

各平台的资源采集能力不同。不支持或采集失败的字段记为 `null`，并附带原因说明；
`null` 不表示资源数量为零。尚未采集的数据、未运行的平台和未验证的场景均不能
视为通过。此文档不附带预先编造的性能数值；实际结果以每次保存的完整记录为准。

进一步位图、schema cache、发包和任务投递优化及测量边界见 [PERFORMANCE_OPTIMIZATION.md](PERFORMANCE_OPTIMIZATION.md)。独立 CPU 热点探针见 [PROFILING.md](PROFILING.md)。
