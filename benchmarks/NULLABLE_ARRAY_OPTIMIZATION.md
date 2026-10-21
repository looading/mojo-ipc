# 可空基础类型元素数组优化

2026-10-21，将紧凑数组路径扩展到 bool、int8/uint8、int16/uint16、int32/uint32、
int64/uint64、float/double 的可空元素，即 Rust `Vec<Option<T>>` / `[Option<T>; N]`。
普通浮点和生成接口使用的 OrderedFloat 均支持，固定、嵌套及外层可空数组保持兼容。

## 实现与协议布局

新增 `NullablePrimitiveArray` 保存连续的原生 Option 缓冲，避免每个元素的
MojomValue、每个 Some 的 Box、调试名字和通用布局对象。原生类型通过安全 Any
检查转移所有权；普通浮点与 OrderedFloat 显式包/拆，不对 Vec 做布局强转。

设 `B = ceil(N/8)`，以下长度含 8 字节数组头，不含最终 8 字节尾对齐：

* 可空 bool：`8 + 2B`，先 tag 位图，再 value 位图，均为低位优先。
* 其他宽度 w：`8 + align_up(B,w) + N*w`，tag 位图之后先对齐再放全部值槽。
* N=0：长度恰为 8；None 槽仍存在，编码 tag0 且值全零，不因空值比例而省略槽。

长度、乘法及对齐均 checked；固定 N、声明存储范围和尾对齐仍验证，完整范围校验
后才分配。解码保持旧版对 None 脏槽、unused 位及非零 padding 的容忍；Some 浮点
保留 NaN/sNaN payload 和正负零位模式。自定义 typemap 仍逐 Some 调用，None 不触发
内层转换，失败正常返回错误。Map 值列展开保留 Option 语义。

Enum 继续验证合法 discriminant，即使对应 tag 为 None；handle/endpoint 使用原本的
sentinel、索引和所有权规则。它们保留通用路径，没有按数字存储宽度强套快路径。
Mojom wire 与生成字段类型不变。底层公开 Rust AST 增加
`MojomValue::NullablePrimitiveArray(NullablePrimitiveArray)`，直接穷尽匹配或依赖 AST
equality/hash/order 的代码需适配；这些源码改动尚未重新发布。

## 测试与 benchmark 方法

新增 18 项 wire 兼容测试，包括 11 型 × 全空/全非空/混合 × N=0/1/7/8/9/15/16/17/
63/64/65；保留独立 legacy encoder 和人工 golden、float bits、脏 None/unused bits/
padding、固定/嵌套/外层可空/Map、Context/失败 typemap、Enum None 验证、畸形头与
所有截断前缀。原测试的 nullable AST 形态断言更新，原 golden/wire 断言未删除。
新增生成协议的边界与 tag/浮点位校验 2 项；整个 workspace **62 项测试通过**。

冻结新增 harness，before 使用本轮开始时的非可空 primitive 优化版 core/facade
完整快照，after 使用本轮实现。逐文件比较 Rust/schema/build.rs 的 SHA-256，确保
两份 harness 一致；同一编译器、原生库和配置，所有编译/消费者验证在正式测量前结束。

每个版本新增 11 型 × 三档目标数组数据区预算 64B/1KiB/64KiB × 三种空值分布
（all_none、all_some、mixed；mixed 每 3 个元素第一个为 None），共 **99 组纯 codec
和 99 组真实双进程串行 Echo**。每组预热 2 次，至少 8 次且至少持续 200ms。

数据区预算包含 tag 位图、值槽和内部对齐，不含数组头、尾对齐或 Params 头/ID/指针。
按不超过预算的最大 N 选择元素数，三种 pattern 的 N 和实际 wire 大小相同；例如
uint8 目标 64B 的实际数据区为 63B，而不是 64B，吞吐使用真实字节数。报告逐行显示
目标/实际大小、元素数、None 数、tag/value/padding 及实际样本数。

浮点回应按 tag 和 bits 校验，codec 另外检查真实 wire 长度。RTT 排除输入 clone 和
回调校验，整个循环吞吐包含它们。原 uint8 和非可空 primitive case/口径保留，
两轮均使用 30 秒 uint8 1KiB/window64 持续负载及 100 次 worker 重建。
空值分布只改变有效槽内容，不缩小实际 wire；不能把 wire 吞吐当作有效非空数据吞吐。

记录目录为 `target/benchmark-runs/macos-x64-nullable-{before,after}-20261021/`。
构建来源为 `target/validation/nullable-array-before/build-inputs.json` 和
`target/validation/nullable-array-after-build.json`。源码未提交，Git HEAD 不能单独标识
本轮二进制，另存源码与 binary 哈希。两次单轮观测不等于统计置信度；旧大包样本少时
p99 只作探索，有限时间的资源计数不能证明长期无泄漏。

## 首轮实测（macOS x86_64）

以下为 64KiB 目标数组数据区、mixed（每三个元素首个 None）的真实双进程串行 RTT
p50，单位微秒。Before 为上轮非可空 primitive 优化版，After 为首版可空紧凑路径；
后续位图/消息发送优化另见性能迭代报告，不覆盖本轮原始数据。

| 类型 | Before µs | After µs | 加速 |
| --- | ---: | ---: | ---: |
| bool | 365,657.708 | 2,308.767 | 158.4× |
| int8 | 103,729.991 | 1,133.808 | 91.5× |
| uint8 | 106,224.744 | 1,149.145 | 92.4× |
| int16 | 56,844.770 | 716.795 | 79.3× |
| uint16 | 58,406.830 | 744.746 | 78.4× |
| int32 | 29,013.506 | 410.477 | 70.7× |
| uint32 | 30,330.338 | 537.515 | 56.4× |
| int64 | 16,286.832 | 536.274 | 30.4× |
| uint64 | 17,536.379 | 398.151 | 44.0× |
| float | 29,637.966 | 487.579 | 60.8× |
| double | 16,885.732 | 421.802 | 40.0× |

完整 99 组 codec 和 99 组 typed case 均完成并通过 ID、tag、值/浮点 bits 校验。
真实 registry 0.1.0 与新路径 **287 组逐字节一致、双向解码通过**；11 类脏 None、
unused 位和 padding probes、自定义 Some/None 调用规则、Enum None 非法槽拒绝相同。
Linux/Windows workspace 全目标交叉检查通过，但未在目标 OS 运行本轮 benchmark。

两个版本各运行 30 秒持续负载和 100 次 worker 重建；分别完成 1,038,718 / 899,872
次压力请求（本次 After 约慢 13.4%，不能将所有指标描述为提升）。每轮 106 个 worker，
105 个正常退出、1 个预期崩溃退出 17，无强制清理。16 个静止 checkpoint 的 FD=4、
Mach port name=38、活动 worker=0。RSS 包含 allocator 缓存和系统波动，有限测量不能
证明长期无泄漏。新路径小/大数组收益应看逐行数据；旧版大包通常仅 8 次样本，p99
只作探索。源码与二进制 SHA、原始资源、配置和完整延迟分布均保留，未筛掉慢项。
