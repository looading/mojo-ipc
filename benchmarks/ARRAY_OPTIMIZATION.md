# uint8 数组优化前后实测

2026-10-21，在同一台 macOS x86_64 机器上，以相同 Rust 1.96.0 release / opt-level 3、
原生库和负载参数复测。编译、独立消费者验证均在正式 benchmark 开始前结束。
完整对照工具 `compare.py` 检查 target、编译器、原生库 SHA-256、完整配置、机器及
资源采样间隔；这两次记录的上述条件均一致，但这不保证瞬时系统负载和调度噪声一致。

## 改动

`Vec<u8>` / `[u8; N]` 使用紧凑的 `MojomValue::ByteArray(Vec<u8>)`，避免每个字节的
中间 enum、数组布局条目和调试字段名分配。用安全的 `Any::downcast_mut` 搭配
`mem::take` 转移缓冲，不增加 unsafe 转换。

非 nullable uint8 元素批量读写；固定长度、指针、声明存储范围、截断及 8 字节对齐
校验仍保留。数组头允许 `num_bytes >= 8 + num_elements`，与原通用解析器一致。
外层 nullable 和嵌套数组使用相同 wire；`array<uint8?>` 及其他元素类型保留原路径。
自定义 typemap 仍逐元素转换；uint8 map 的列展开后继续检查长度和重复键。
同时修复固定数组自定义转换失败时提前 unwrap 的旧问题，使其返回转换错误。

Mojom schema、生成接口字段类型和二进制格式均未改变。公开 Rust AST enum 增加
variant 是另一个层面的 API 变化：直接穷尽匹配 `MojomValue` 的调用方需适配，
新旧 AST 表示的 equality/hash/order 也不同。旧 `Array(UInt8...)` 仍可编码和转换。
这些是尚未发布的源码改动，未覆盖 crates.io 的 0.1.0。

## 协议兼容性验证

workspace 共 **24 项测试**通过，其中新增 12 项检查 legacy/golden wire、0/1/7/8/9/
64/1MiB 边界、嵌套/nullable/fixed 数组、u16/bool/nullable-u8、非恒等与失败的
typemap、map 重复键/长度、截断、畸形头部、指针和 padding。

另外构建两个独立 release 消费者，一个使用真实 crates.io `mojom-codec-core 0.1.0`，
另一个使用本地新代码，通过 wire 文件交叉读写。**17/17 组编码逐字节相同，17/17
旧版解新版、17/17 新版解旧版均通过**。这些用例包含 1MiB、可空、嵌套、固定数组
及 uint8 map，旧依赖没有 path/patch 覆盖。

回执、消费者源和独立 Cargo.lock 保存在 `target/validation/array-compat-crates-io/`；
注册表旧包 SHA-256 为
`cf6ddeca2793e76cca7feaac8e70fdedba5f7a8b183d2e86c402926b5203ca8d`。
Windows MSVC 和 Linux GNU 的 workspace/all-targets Rust 编译检查通过；
这两个平台尚未实际运行 benchmark。

## 类型化串行 RTT

下表为 p50，倍率是旧 p50 / 新 p50，直接展示两次观测，不合并样本或估计置信区间。

| 应用 payload | 优化前 | 优化后 | 倍率 | 样本 N（前 → 后） |
| --- | ---: | ---: | ---: | ---: |
| 0 B | 83.816 µs | 103.720 µs | 0.81× | 10,800 → 8,693 |
| 64 B | 130.397 µs | 101.576 µs | 1.28× | 7,129 → 8,929 |
| 1 KiB | 779.152 µs | 88.582 µs | 8.80× | 1,246 → 9,979 |
| 64 KiB | 57.745 ms | 0.304 ms | 189.82× | 18 → 2,939 |
| 1 MiB | 929.777 ms | 4.185 ms | 222.18× | 8 → 220 |

0 B 本轮慢了约 23.7%；raw 对照也由 57.579 µs 变为 72.041 µs，慢约 25.1%。
不能据单轮结果断言空消息退步的原因，也不能只展示加速项。64KiB/1MiB 的旧样本
较少，尤其旧 p99 不能代表稳定尾延迟。

1MiB raw 对照 p50 为 3.070 → 3.316 ms；类型化路径优化后接近这个量级。
类型化串行单方向应用吞吐由约 1.07 → 219.48 MiB/s，倍率约 206.04×；
吞吐包含准备、校验和排空，不能用它的倍率代替延迟倍率，也不是 wire 流量。

## 纯 codec

1MiB 独立编码 p50：55,141.512 → **42.404 µs**，约 1,300×。
独立解码 p50：399,042.875 → **41.195 µs**，约 9,687×。
旧纯 codec 样本 8，新完成 4,931 次；两个分位数之和不是 pair p50 或 RTT p50。
完整 pair 吞吐另含输入 clone 与结果校验。

## 稳定性与资源

60 秒、1KiB、窗口 64：完成 **1,742,784 次**往返（原基线 154,858 次），约
29,045 RPC/s，吞吐约为原来的 11.26×；校验错误 0，pending 回到 0。
100 次 worker 创建/通信/退出和异常退出后 64 个待完成回调释放检查均通过。
104 个 worker 中 103 个正常退出、1 个按预期退出 17，无需强制清理。

14 个静置检查点均无活跃 worker，FD 均为 **4**、Mach port 名字数均为 **38**，
探测无错误。从持续负载结束到第 100 轮重建，父进程 RSS 增加 40 KiB。
本轮性能阶段结束时父 RSS 为 96.66 MiB，原基线同阶段为 206.71 MiB；这是静置
采样值，受缓存/页驻留影响，不是内存分配总量或真实峰值。
这些有限时长/次数的检查不能证明长期无泄漏。

## 记录与复现

原始基线：`target/benchmark-runs/macos-x64-20261021/`。
优化后：`target/benchmark-runs/macos-x64-array-optimized-20261021/`。
两边均采用 iterations 5000、warmup 500、sample-ms 1000、stress-seconds 60、
churn 100、window 64，默认超时及每秒 RSS 采样；每个 payload 的有效次数预算不变。
优化后总耗时约 94.9 秒。

```sh
export MOJO_RUNTIME_LIBRARY="$PWD/dist/x86_64-apple-darwin/libmojo_rs_native.dylib"
# 必要时指定 MOJOM_PYTHON 为 Python 3.10+。
python3 benchmarks/run.py --stress-seconds 60 \
  --output-dir target/benchmark-runs/array-optimized-local-001
python3 benchmarks/compare.py \
  target/benchmark-runs/macos-x64-20261021 \
  target/benchmark-runs/array-optimized-local-001 \
  --output target/benchmark-runs/array-comparison-local-001.html
```

可移植快照为 `baselines/macos-x64-array-optimized-20261021.json`，包含精简资源证据、
新 codec 源码哈希和真实注册表兼容回执。源码仍未提交，记录的 repository commit
不能单独复现该测试二进制；二进制哈希与具体源码哈希另行保存。
