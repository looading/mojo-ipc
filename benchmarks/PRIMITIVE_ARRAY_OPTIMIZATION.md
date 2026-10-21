# 其他基础类型数组优化

本文记录非可空元素优化阶段。后续可空元素优化与独立复测见
[NULLABLE_ARRAY_OPTIMIZATION.md](NULLABLE_ARRAY_OPTIMIZATION.md)。

2026-10-21，扩展上一轮 uint8 快路径，覆盖 bool、int8/int16/int32/int64、
uint16/uint32/uint64、float、double 的非 nullable 元素数组。向量和固定数组均可使用，
浮点同时支持普通 f32/f64 与生成代码使用的 OrderedFloat。

## 实现和兼容边界

新增 `PrimitiveArray` 紧凑值表示，避免每个元素的 `MojomValue`、调试字段名及通用布局
条目。原生同类型缓冲通过安全 Any 检查转移；普通浮点和 OrderedFloat 显式包/拆，
不假设两种 Vec 可直接强制转换。

整数和浮点使用明确的小端批量读写；浮点保留 NaN payload、sNaN 和正负零的位模式。
Bool 仍使用 Mojom 的低位优先 bit packing，编码清零未使用高位，解码保持旧版对
未使用高位及非零 padding 的容忍。固定 N、checked storage size、声明存储范围、
指针和尾部 8 字节对齐继续校验，分配在完整范围验证之后进行。

外层可空和嵌套数组使用相同 wire。可空元素保留原有 tag bits 通用路径；enum 仍检查
合法 discriminant，handle/endpoint 不按相同存储宽度套用数值快路径。自定义 typemap
仍执行逐元素转换并传播错误。Map 展开紧凑列后保留长度/重复键校验，关联接口的
uint32 ID 数组继续拒绝 0 和 0xffffffff。

Mojom schema 与生成接口的字段类型不变。公开 Rust AST 新增
`MojomValue::PrimitiveArray(PrimitiveArray)`，直接穷尽匹配或依赖 AST equality/hash/order
的调用方需适配。这些改动尚未发布，不是已发布 0.1.0 的无破坏补丁。

## 验证

Workspace **42 项测试**通过，包括新 primitive wire 测试 16 项、生成协议及按位
校验测试 2 项；先前 12 项 byte-array 兼容测试也继续通过。覆盖所有整数、bool
跨字节边界、float/double 特殊位模式、固定/嵌套/可空数组、typemap、map、畸形头部、
截断、padding 和关联接口 ID。Linux GNU、Windows MSVC workspace/all-targets Rust
编译检查通过；性能和原生运行仍只在 macOS x86_64 执行。

实际构建两个独立 release 消费者：旧方使用真实 crates.io 0.1.0，新方使用本地代码。
**78/78 组编码逐字节相同，旧解新及新解旧均通过**；另外的非零 bool unused bits、
padding 容忍和非法接口 ID 拒绝检查也一致。浮点比较使用位模式，不用 NaN/负零
可能等价的 OrderedFloat equality 代替。源、锁文件、wire 和回执保存在
`target/validation/primitive-array-compat-crates-io/`。

## 本轮 benchmark 方法

先冻结新增 benchmark 源，构建两份相同 harness：before 使用本轮开始时的
uint8-only codec 快照，after 使用本轮全部基础类型实现。逐文件 SHA-256 检查确认
两份 Rust/schema/build.rs 完全相同；before facade 只移除了旧 core 不存在、且
benchmark 不使用的 PrimitiveArray 重导出。两边编译器、构建模式和原生库相同。

新增 10 种类型，每种测 64 B、1 KiB、64 KiB 的**数组 wire 元素数据**：共 30 组
真实双进程串行 Echo 与 30 组纯 codec。Bool 字节数为 ceil(elements/8)，例如
64 KiB 含 524,288 个 bool；这不是同数量的 Rust 元素或相同 Rust 内存占用。
数组/struct/message 头和指针不计入这个 payload 字节数，完整编码体长度另行记录。

每组预热 2 次，至少测 8 次且至少持续 200 ms，记录实际 N。旧大数组样本少时
p99 只能视为探索性。浮点 payload 包含特殊位模式，发送方按完整 ID 和位模式
校验回应。RTT 从方法调用到回调入口，输入 clone 和结果校验排除在 RTT 外，
但包含在整个循环吞吐中。纯 codec 同样排除准备/校验的分段延迟，pair 吞吐包括它们。

原 uint8/raw/codec case 及其参数保留，两轮均显式采用 30 秒持续负载、100 轮重建、
窗口 64 和默认 RSS 采样间隔，串行执行。所有编译和消费者验证在测量前完成。
不同轮次的瞬时系统噪声仍可能不同，倍率是两次观测的比值，不是置信区间。

记录目录：

* `target/benchmark-runs/macos-x64-primitives-before-20261021/`
* `target/benchmark-runs/macos-x64-primitives-after-20261021/`

构建源和二进制哈希保存在 `target/validation/primitive-array-before/build-inputs.json`
及 `target/validation/primitive-array-after-build.json`。源码未提交，repository commit
不能单独标识本轮源码。使用 `compare.py` 可生成包含原 uint8 及新增 primitive 全表的
离线 HTML。所有性能值以这两目录的真实 JSON 为准。

## 实测结果

同一台 Intel i7-9750H、32 GiB、macOS x86_64，Rust 1.96.0 release / opt-level 3，
原生库 SHA-256 保持
`ab9bc6337bc313547b88f3b812f5b4ad138bd6e89a0c0bbc545c20227c867c49`。
11 项记录条件均一致。以下为 **64 KiB 数组 wire 元素数据**的串行 RTT p50，
单位毫秒；旧 / 新的 p50 比值作为倍率。

| 类型 | 元素数 | 优化前 ms | 优化后 ms | 倍率 | 样本 N（前 → 后） |
| --- | ---: | ---: | ---: | ---: | ---: |
| bool | 524,288 | 277.857 | 1.709 | 162.6× | 8 → 111 |
| int8 | 65,536 | 61.694 | 0.313 | 197.2× | 8 → 562 |
| int16 | 32,768 | 30.366 | 0.319 | 95.3× | 8 → 537 |
| uint16 | 32,768 | 31.312 | 0.301 | 103.9× | 8 → 572 |
| int32 | 16,384 | 17.334 | 0.276 | 62.8× | 12 → 660 |
| uint32 | 16,384 | 17.664 | 0.267 | 66.2× | 12 → 687 |
| int64 | 8,192 | 9.245 | 0.319 | 29.0× | 22 → 544 |
| uint64 | 8,192 | 8.469 | 0.336 | 25.2× | 24 → 518 |
| float | 16,384 | 17.561 | 0.336 | 52.3× | 12 → 485 |
| double | 8,192 | 8.237 | 0.282 | 29.2× | 24 → 622 |

新增类型三个大小的 30 组 RTT p50 本轮均缩短；表中的旧大数组 N 仅 8–24，不能把
旧 p99 当作稳定尾延迟或把倍率当作置信区间。原 uint8 1MiB RTT p50 为
4.002 → 3.973 ms，基本持平，符合本轮仅扩展其他类型的目标。

64 KiB 纯 codec p50 对照（单位微秒）：

| 类型 | 编码前 | 编码后 | 解码前 | 解码后 |
| --- | ---: | ---: | ---: | ---: |
| bool | 12,037.923 | 148.358 | 116,609.219 | 386.099 |
| int8 | 2,923.060 | 4.225 | 24,282.557 | 2.118 |
| int16 | 1,608.818 | 4.186 | 12,510.488 | 2.551 |
| uint16 | 1,547.356 | 4.457 | 12,128.784 | 2.695 |
| int32 | 718.795 | 4.179 | 5,940.710 | 2.609 |
| uint32 | 725.223 | 4.085 | 6,115.941 | 2.503 |
| int64 | 381.211 | 4.053 | 3,236.163 | 2.479 |
| uint64 | 373.567 | 4.158 | 3,339.223 | 2.547 |
| float | 706.736 | 4.414 | 5,905.249 | 2.653 |
| double | 378.817 | 4.079 | 3,413.619 | 2.497 |

Bool 必须做位打包/展开，且其 Rust 元素数远多于同 wire 大小的数值数组，
不能用相同字节数下的时间差推断每元素成本。编码和解码分位数之和也不是 pair p50。

两轮稳定性检查均通过：30 秒持续负载完成 1,007,846 → 990,103 次往返，约
33,593 → 33,001 RPC/s；这段是原 uint8 1KiB/window64 负载，约 -1.8% 的单轮波动
不作为新增类型提速的证据。两边均无数据校验错误，pending 清零，100 次 worker
重建及异常退出后 64 个回调释放通过。各 105 个 worker，104 个正常退出、1 个
按预期退出 17，无需强制清理。

各 15 个静置检查点均无活跃 worker，FD 均为 4、Mach port 名字数均为 38，
probe 无错误。从持续负载结束到第 100 次重建，父 RSS 分别增加 16 KiB / 44 KiB；
有限时长的资源趋势不能证明长期无泄漏。总耗时分别约 82.1 / 77.6 秒。

可移植的 JSON 快照位于 `baselines/macos-x64-primitives-{before,after}-20261021.json`；
完整比较报告为 `target/benchmark-runs/primitive-array-comparison.html`。
