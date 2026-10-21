# macOS x86_64 首次实测

这里保留优化前基线。后续 uint8 数组优化、旧版双向 wire 验证和前后对照见
[ARRAY_OPTIMIZATION.md](ARRAY_OPTIMIZATION.md)。

2026-10-21，在 Intel i7-9750H（6 核 / 12 线程）、32 GiB 内存、macOS 26.7.1
上运行。Rust 1.96.0，release / opt-level 3，SDK 0.1.0，Chromium revision
`bcbc09a9f8e4c3fd3ad2ed9cbad41e63c35ccfdd`。原生库 SHA-256 为
`ab9bc6337bc313547b88f3b812f5b4ad138bd6e89a0c0bbc545c20227c867c49`。

这一轮测得小消息能够持续正确通信，但 `array<uint8>` 的 Rust Mojom 编解码成本很高，
大消息类型化接口目前不适合追求高吞吐的场景。以下数值是本机这一轮结果，不能代表
其他机器或操作系统，也不是裸 Mojo Core 的性能上限。

## 串行往返

每次只等待一个 Echo，单位为微秒。延迟从调用方法前计到回调入口，吞吐另按包含
数据准备和校验的完整 wall time 计算。原始消息对照也经过相同的双进程、原生调度、
Rust watcher、分配、拷贝及 channel；它省去类型化 router 和 Mojom codec。

| 应用 payload | 类型化 N | 类型化 p50 | 类型化 p99 | 原始消息 N | 原始消息 p50 | 原始消息 p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 B | 10,800 | 83.816 | 176.618 | 14,472 | 57.579 | 128.517 |
| 64 B | 7,129 | 130.397 | 229.348 | 14,712 | 56.762 | 129.147 |
| 1 KiB | 1,246 | 779.152 | 1,037.163 | 14,311 | 58.323 | 136.861 |
| 64 KiB | 18 | 57,744.971 | 60,284.452 | 3,068 | 235.421 | 347.691 |
| 1 MiB | 8 | 929,776.517 | 975,608.038 | 291 | 3,069.960 | 4,612.434 |

64 KiB 和 1 MiB 的类型化样本不足 100，尤其不能据此推断生产 p99。
原始 1 MiB 串行吞吐约 290.2 MiB/s；类型化约 1.07 MiB/s，均按单方向应用数据计算。
完整记录还包含窗口 8 / 64 的流水线和有 Inspect 屏障的单向 Push。

## 编解码诊断

同进程、相同 Echo 参数结构的独立编码/解码时间如下。输入 clone 在 encode 计时外，
结果校验在 decode 计时外；codec 不初始化原生库。

| 应用 payload | 编码 p50（µs） | 解码 p50（µs） |
| --- | ---: | ---: |
| 0 B | 1.139 | 0.842 |
| 64 B | 3.775 | 19.903 |
| 1 KiB | 38.415 | 297.261 |
| 64 KiB | 3,187.234 | 24,561.843 |
| 1 MiB | 55,141.512 | 399,042.875 |

1 MiB 编码和解码的两个 p50 之和约 454 ms；往返双方各做一次，量级接近测得的
930 ms 类型化 RTT。这是定位瓶颈的对照，不是将分位数之和当作整体分位数。

代码路径也支持这一判断：`crates/mojom-codec-core/src/parsing_trait.rs` 中通用
`Vec<T>` 逐元素转为 `MojomValue`；`parse_values.rs` 的数组解析为每个字节构造
`Array_Element_{idx}` 字符串，再经过通用 struct 解析和字段名复制。
1 MiB 字节数组因此包含大量中间对象及分配。后续应优先实现字节数组的批量编码、
解码路径，并继续用本 benchmark 检查 wire 兼容性、正确性和速度变化。
本次仅增加测量工具，未修改已发布 SDK 的编解码实现。

## 稳定性

60 秒持续负载使用 1 KiB Echo、窗口 64：发送和完成均为 **154,858**，约
2,580 RPC/s，数据校验错误为 0，结束时 pending 为 0。

100 轮独立的 worker 启动、8 次 Echo、关闭和等待退出全部通过；生命周期 p50
为 18.76 ms，p99 为 21.99 ms。另一个测试让 worker 持有 64 个未完成请求后以
退出码 17 异常退出：64 个回调闭包全部释放、成功回调为 0、pending 回到 0。
共 104 个 worker，103 个正常退出，另 1 个按预期退出 17，无需强制清理。

14 个静置检查点均无活跃 worker，父进程 FD 始终为 4。从持续负载结束到第 100 轮
重建，父进程 RSS 增加 36 KiB，第 30 轮起保持不变。RSS 有分配器缓存及页驻留因素，
这个短期趋势不能证明长期无泄漏。该轮未采集 Mach port 名字计数。

随后补上 Mach port 名字采样，另跑一轮以生命周期为重点的 **1,000 轮重建**：

* 1,000 轮全部通过，生命周期 p50 为 17.48 ms，p99 为 23.85 ms。
* 103 个静置检查点无活跃 worker，FD 均为 **4**，Mach port 名字数均为 **38**，
  Mach 查询及两块返回数组的释放均未报告错误。名字计数不等于各名字的引用数。
* 从第 10 轮到第 1,000 轮，父进程 RSS 增加 284 KiB。
* 共 1,003 个 worker，1,002 个正常退出、1 个按预期退出 17；64 个待完成回调释放
  检查再次通过，未发生强制清理。

这轮总耗时约 39.1 秒，关闭持续负载，并将性能 case 设为 1 次、零预热、零最短时长，
用于扩大重建与资源检查的覆盖。它的性能数字仅是 smoke 数据，不能替代上面的正式
性能基线；1,000 次重建也不等于经过数小时或数天的稳定性验收。

## 复现与记录

```sh
export MOJO_RUNTIME_LIBRARY="$PWD/dist/x86_64-apple-darwin/libmojo_rs_native.dylib"
# MOJOM_PYTHON 需指向 Python 3.10+（默认 python3 满足时可省略）。
python3 benchmarks/run.py --stress-seconds 60 \
  --output-dir target/benchmark-runs/local-baseline-001
```

其他参数采用默认值：iterations 5000、warmup 500、sample-ms 1000、churn 100、
window 64。各 payload 的有效次数预算和实际样本数见 JSON。完整方法见 README。

本地完整日志在 `target/benchmark-runs/macos-x64-20261021/`，包含原始输出、JSON 和
离线 HTML 报告；可移植的测量快照保存在 `baselines/macos-x64-20261021.json`。
此轮总耗时约 124.4 秒，运行时未同时执行编译等重负载任务。
仓库当时有未提交的 benchmark 改动，因此记录中的 repository commit 不能单独重建
该轮测试程序；二进制 SHA-256 和工具链也保存在快照中。

生命周期复测使用另一结果目录并保留独立快照：

```sh
python3 benchmarks/run.py --iterations 1 --warmup 0 --sample-ms 0 \
  --stress-seconds 0 --churn 1000 --window 1 \
  --output-dir target/benchmark-runs/local-churn1000-001
```

本机记录为 `target/benchmark-runs/macos-x64-churn1000-20261021/`，可移植快照为
`baselines/macos-x64-churn1000-20261021.json`。

新增 benchmark 的 workspace 测试 12 项通过，Linux GNU 和 Windows MSVC 的
Rust 编译检查通过；这两个平台尚未实际运行 benchmark。
