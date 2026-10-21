# 继续优化 Rust 编解码与消息路径

在可空数组紧凑 AST 首轮之后，继续以实测定位热点。Mojom wire、native SDK ABI、
接口签名与 handle/关联接口验证保持；本轮不重建原生库，也不发布 crates.io 版本。
不能用有限机器和样本证明绝对性能极限。

## 保留的候选实现

1. 非可空 bool 位图批量处理：x86_64 使用基线 SSE2，每次处理 16 个 bool；
   不要求 AVX 或额外 CPU feature。其他架构走通用实现。解码直接完整初始化预留
   MaybeUninit 槽，之后才发布 Vec 长度。只有 0/1 写成 bool，不重解释 Vec 控制块。
2. 可空标量按八元素聚合编码 tag；解码先扫描有效 bitmap，区分全 None / 全 Some，
   混合分布保留连续 chunks_exact 路径，None 不触发内层转换。全 None 编码直接复用
   已清零的缓冲。可空 bool 的 tag/value 在寄存器内合并；解码固定展开八个 Option 后整组追加。
   保留尾部无效位、脏 None 槽、对齐、IEEE bits 和小端序规则，不假定 Option 布局。
3. 派生具体 struct 用各自 OnceLock 缓存 schema，避免每条消息读全局锁和 HashMap。
   手写 impl 保留原 fallback。Context 不得改变类型 schema 的约束与原实现相同。
   字段直接构建两个 Vec，去掉临时 tuple Vec/unzip；padding 直接 resize，避免临时 Vec。
4. 发包把 header 和 payload 分别复制进一次申请、一次 COMMIT 的新 Core message，
   去掉拼接 payload 的中转 Vec 与单独 commit。公开 owned MojomMessage/into_data 保留。
   这仍有必要的复制，不是零拷贝。长度 checked，handles 仅在 native 成功后转移。
5. sequence task 使用一个 Box<F> 和具体 run/cancel 回调，保持 C ABI、恰一次消费和
   panic 时 abort 规则，避免原先额外的动态闭包 Box。

## 测量方法与证据

主 benchmark 的 Rust、schema 和 build.rs 与首轮冻结 harness 相同。private package
仅增加独立 codec_probe binary，并添加 default-run 保持原 cargo run 的默认入口。
原 nullable 优化 binary 冻结为本轮 Before，最终源码另构建 After，记录源文件、binary、
编译器和原生库 SHA。所有正式测量与编译、consumer 验证、采样分离。

先独立 sample 确认 bool 的主要 codec 时间在位图循环，再用无 profiler 的 focused
ABBA 测试。首候选 nullable mixed bool 解码曾由约 337us 退步到 438us；保留此诊断
数据，调整为固定八项展开后重新测量。数值按八元素解码也曾让 uint64/float mixed
变慢，因此恢复精确连续 chunk 迭代，仅对全空/全非空分布进行批量分流。最终 focused 与正式 IPC 结果单独保存。
纯 codec 时间排除输入 clone、回调校验与输出释放；完整循环吞吐包含这些开销。
纯 codec 的吞吐不能当作跨进程吞吐，percentile 也不能简单相加。

主 formal run 保留原 u8、10 类非可空与 11 类可空三种分布矩阵，包含 30 秒
u8/1KiB/window64 压力、100 次进程重建和预期崩溃。Before采用冻结原二进制重新测量，最终After在其后运行；
报告保留 raw 对照、退步项、样本数、p50/p99 与资源记录，有限压力不能证明永不泄漏。

75 项 workspace 测试通过，包括 65536 个 16-bit 位图模式、全部 tag/value-byte
组合、随机长度/尾部/dirty None、legacy/golden wire 和并发 cache。额外真实 Core
分段发包测试通过，覆盖 empty/1MiB、handle 转移可用和 unsent 销毁。
真实 registry 0.1.0 与最终 codec 的 nullable 287 + primitive 78 = 365 项逐字节一致、
双向解码及 semantic probes 通过。官方 generator 6 项测试通过。
Windows/Linux/macOS arm64 全 workspace/all-targets 仅交叉编译检查；本轮真实 IPC
性能与 native 集成只在 macOS x86_64 执行。底层 AST 新增 variant 的源码适配要求
见 NULLABLE_ARRAY_OPTIMIZATION.md；本轮未发布版本。

结果目录：target/benchmark-runs/macos-x64-performance-before-20261021/ 与
target/benchmark-runs/macos-x64-performance-linear-after-20261021/。
最终检查/构建来源：target/validation/performance-round-linear/；旧版对照：
target/validation/performance-round-linear-compat/；focused 与 sample 见 PROFILING.md。

## 最终 focused ABBA（纯 codec，µs）

每格为两轮各自p50的中位数，不是合并分位数或置信区间；完整循环还包括clone/校验。

| Case | Encode Before | Encode After | Decode Before | Decode After |
| --- | ---: | ---: | ---: | ---: |
| bool-64k | 200.336 | 21.755 | 430.762 | 63.358 |
| empty | 0.114 | 0.106 | 0.093 | 0.088 |
| nullable-bool-64k-all_none | 113.867 | 91.893 | 268.885 | 44.430 |
| nullable-bool-64k-all_some | 686.246 | 101.180 | 357.783 | 258.778 |
| nullable-bool-64k-mixed | 483.696 | 98.763 | 303.571 | 251.635 |
| nullable-u32-64k-all_none | 12.993 | 10.518 | 38.930 | 5.426 |
| nullable-u32-64k-all_some | 50.362 | 37.548 | 34.330 | 4.309 |
| nullable-u32-64k-mixed | 32.088 | 34.226 | 29.752 | 34.374 |
| u8-1m | 34.097 | 39.919 | 32.962 | 38.704 |

宽数值另由独立numeric_probe按0.5秒、before/after/after/before测量每个case，float含NaN/sNaN/±0并按bits校验。混合分布的精确连续chunk路径恢复uint64/float旧版表现；均匀分布用位图分流。全空encode快捷路径也经三型、三分布对照。

原始所有JSON、checksum、source/binary SHA均保留。u8 bulk codec本来已是整段复制，本次未修改其循环；小幅波动不描述为显著收益。

## 最终双进程 IPC（macOS x86_64）

同配置、同frozen主harness的before与linear-after，30秒压力、100次重建。
下表是串行RTT p50；64KiB指数组wire数据区预算，不能把不同类型元素数视为相同。

| Case | Before µs | After µs | 加速 |
| --- | ---: | ---: | ---: |
| bool nonnullable 64KiB | 1,639.250 | 414.490 | 3.95× |
| bool all_none 64KiB | 1,126.376 | 766.721 | 1.47× |
| bool all_some 64KiB | 2,439.238 | 905.804 | 2.69× |
| bool mixed 64KiB | 2,006.025 | 877.756 | 2.29× |
| uint8 0B | 86.226 | 85.406 | 1.01× |
| uint8 65536B | 292.913 | 293.202 | 1.00× |
| uint8 1048576B | 3,930.095 | 3,947.569 | 1.00× |

30秒压力完成 1,009,974 → 1,064,360 请求（+5.38%），两边验证/pending检查通过。
每轮106个worker：105个exit0、1个预期exit17；16静止checkpoint均FD4/Mach38/active0，无强制清理。
完整矩阵保留所有p50/p99/吞吐退步及样本数。普通byte和短消息没有统一显著提升，不能把bool收益推广到全部IPC。新旧主harness单轮结果与有限时间压力不证明绝对最优或长期无泄漏。

## 关闭stress/churn的独立控制

另用相同before/after二进制运行 `--stress-seconds 0 --churn 0 --sample-ms 300
--iterations 8 --warmup 2`，数组case仍为每组200ms。这一组独立比较，不与正式30秒
压力那组混算。129个typed数组p50在正式组69项更低、60项更高；独立控制91项更低、
38项更高，回退的场景也有变化。它说明部分排序不稳定，不能据此证明已消除回归，
更不能未经测量就把它们统称为热降频或噪声。原始结果和全部退步均保留，收益主要
集中于bool和均匀可空数据；短case需要更长、重复或交错测试才能判断小幅差异。

控制目录为 `target/benchmark-runs/macos-x64-performance-control-{before,after}-20261021/`。
