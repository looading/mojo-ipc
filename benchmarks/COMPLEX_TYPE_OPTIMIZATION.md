# 复杂类型 codec 优化

本轮针对 Rust codec 在 struct、字符串数组、嵌套数组、map、union 和可空组合中构造临时 AST 与解析 metadata 的成本。对照起点是此前已经完成 primitive/nullable array 优化的 SDK 冻结源码，**不是原始发布版，也不是 Chromium C++ 或 Mojo Core 的性能对照**。本轮不修改 Mojom packing 算法、公开 AST 的类型定义或 native 传输。

矩阵、fixture、计时边界、独立 probe 和 ABBA 命令见 [COMPLEX_TYPES.md](COMPLEX_TYPES.md)。最终 SDK 已完成独立正式四轮对照，结果见文末；候选诊断和分配数据不能替代最终延迟或 IPC 吞吐结果。

## 改动与保留的行为

### 编码时省去诊断字段名

`MojomParse` 新增带默认实现的隐藏方法 `into_mojom_value_for_wire`，由 `serialize` 使用。它默认调用原有 `into_mojom_value`；现有手写实现和 custom typemap 不需要增加方法，原转换及其 `Context` 副作用继续执行。

新版 derive 在这个仅用于编码的路径中构造 `Struct(Vec::new(), fields)`，避免为每个 struct 的每个字段复制诊断名称，并递归使用字段对应的 `WireAs` 和同一个 `Context`。Vec、固定数组、Option、Box、HashMap 和派生 union 继续递归传递该路径；原有 native primitive 快捷路径保留。

公开的 `into_mojom_value` 行为保持：派生 struct 仍产生原来的字段名和字段值。解码得到的真实 struct AST 也保留原有字段名。自定义转换没有被 Any/native 快捷路径替代，None 不调用内部转换，decode 的 `try_from_mojom_value` 仍执行原有校验。现有 struct wire schema cache 继续使用，本轮没有靠删除 schema 或类型检查取得减少分配的效果。

这个隐藏方法仍是 Rust trait 的一部分。**新版 `mojom-derive` 和 `mojom-codec-core` 需要协同升级**，并通过 SDK 的依赖版本一起分发；新 derive 生成的 override 不能搭配没有该方法的旧 core。旧 derive 搭配新版 core 会使用默认方法，保留原编码行为，但不能获得该字段名优化。它不是稳定的应用调用入口，也不改变 Mojom wire 协议。

### 解码字段名按需生成

解析内部以 borrowed 名称或 `ArrayTag(index)`、`ArrayElement(index)`、`Union(tag)` 保存 label。只有真正返回 struct 的 names，或构造 `WrongPointer` 错误时，才生成 String。array、union 和 map 内部的临时 struct 不再构造随后会被丢弃的 names vector。

原错误标签继续保留，包括 `Array_Element_1`、负值 `Union_Field_-7`、`map_keys`、`map_values`。包含指针的 union 调整到父 struct ordinal 后，仍保留其原来的 `Union_Field_tag` label；这也保留了解码 AST 中已有的对应名称行为，没有改成另一个更“整齐”的字段名。

### 指针数组使用一次表和顺序解析

对 struct/string/array/map 等以指针表示的数组元素：

- 编码先建立完整的 `8 + 8N` 字节 header/pointer table，再按原 wire 顺序逐项写入非空嵌套对象并填相对偏移。None 槽保持零，避免为每项分配或保存大块 deferred metadata。
- 解码第一遍仍顺序读取并校验所有指针，只保存每项 `usize` 预期偏移；随后按原顺序验证 declared body size、消费 padding，再顺序解析嵌套对象。结果用 `Vec::with_capacity` 加 push 构造，不先填 N 个 `Invalid`。
- None 不消费嵌套对象或额外的对齐字节；非空对象的偏移、8 字节对齐、nullable 包装和数组最后的对齐保持原规则。真实子 struct 仍通过同一解析 helper 收集其字段名。

这条路径只处理 pointer 元素。内联 union、enum、handle 和其他元素仍按对应协议路径处理，不将其当作普通指针或 primitive 批量数据。

### 固定 map body 与错误文字

map 的编码直接写入原有的 24 字节 struct body 和两个非空 array pointer，随后仍按 keys、values 的顺序编码数组。values pointer 相对于其 `start + 16` 槽位计算。键值配对、确定的 AST 排序、数组元素转换以及解码的长度/重复键校验保持。

union 的无效 tag 说明改成按错误路径生成，避免成功编码时也执行 `format!`。nullable bitmap 分类在发现第一个完整 mixed byte 后即可停止继续判定全 None/全 Some；其标签、值槽和 wire bytes 不变。

### 长字符串的 SIMD UTF-8 校验

最终 SDK 采用精确依赖 `simdutf8 = "=0.1.5"`，使用默认 `std` feature 和 safe `basic::from_utf8(&[u8])` API。少于 64 字节的字符串保留原有 `String::from_utf8(raw.to_vec())` 路径；较长输入先验证 borrowed bytes，成功后对 `&str` 执行 `to_owned()`。这仍只产生一次 owned 字符串拷贝，不是零拷贝，也没有新增 SDK 级 unsafe。

SIMD 验证失败时仍调用 `String::from_utf8(raw.to_vec())`，并返回标准库的实际结果。这样保留原来的 owned `FromUtf8Error`、完整非法字节、`valid_up_to` / `error_len` 和全局 parser offset；即使 SIMD 出现 false negative，也不会假定标准库必定失败而 panic。header size、读原始字节、UTF-8 校验、最后 padding 的先后顺序不变。对应测试覆盖 63/64/65 字节边界、Unicode/NUL/ASCII control、overlong/surrogate/truncated/continuation 错误、合法前后缀、嵌套 offset 和错误优先级。

发布版 0.1.5 的主要依据是 [Cargo manifest](https://docs.rs/crate/simdutf8/0.1.5/source/Cargo.toml)、[safe basic API 源码](https://docs.rs/crate/simdutf8/0.1.5/source/src/basic.rs) 和 [发布 VCS metadata](https://docs.rs/crate/simdutf8/0.1.5/source/.cargo_vcs_info.json)。对应 commit 为 `87ee8d9d20b849eae1974a82b248ee6fe7491bcc`；其 [x86 dispatch](https://github.com/rusticstuff/simdutf8/blob/87ee8d9d20b849eae1974a82b248ee6fe7491bcc/src/implementation/x86/mod.rs) 在 `std` 下选择 AVX2/SSE4.2，缺少相应 CPU 能力时回退标准实现，内部也有 64 字节门槛。[平台 fallback](https://github.com/rusticstuff/simdutf8/blob/87ee8d9d20b849eae1974a82b248ee6fe7491bcc/src/implementation/mod.rs) 和 [aarch64 NEON 选择](https://github.com/rusticstuff/simdutf8/blob/87ee8d9d20b849eae1974a82b248ee6fe7491bcc/src/implementation/aarch64/mod.rs) 不要求 SDK 调用 unsafe implementation API 或设置 `target-cpu=native`。不要将 GitHub main 后续增加的 AVX-512 能力归到本次发布依赖。

## 候选阶段的独立诊断

引入 UTF-8 候选前的 initial round1 共 81 项指标，以及此处的 focused 数据，都保留为**候选/独立 diagnostic**；它们不是最终 SDK 四轮数据，不能与最终结果拼接、合并分位数或用作最终端到端结论。

相同 large fixture 的独立 codec focused ABBA 得到下列 decode 数值。每个数值是对应版本两轮各自 p50 的中位数，单位 µs；它不是 pooled p50、置信区间或 typed RTT：

| Case | 初始复杂类型候选，无 SIMD UTF-8 | 加入 SIMD UTF-8 的候选 |
|---|---:|---:|
| `utf8_string` | 101.1735 | 11.8435 |
| `record` | 72.818 | 11.6225 |
| `string_array` | 240.8515 | 190.463 |

这些诊断用于选择 UTF-8 候选。最终版本仍须使用同一冻结 harness、配置、native 和 fixture 单独完成正式 ABBA，并保留所有回退项。

## 一项明确的畸形输入修正

指针数组的 `slot_offset + pointer_value` 现在使用 `checked_add`。极端偏移原先可能在 debug 下溢出 panic、在 release 下整数回绕；现在返回该 pointer **槽位起点**的 `InvalidPointer`。例如第一个槽位在 offset 8，相关错误也报告 offset 8，而不是读完后的 offset 16。

这是有意的异常行为修正。有效 wire 兼容；不能因此声称所有畸形输入的错误行为与旧版逐字完全相同。局部测试同时固定正常 pointer、size、padding 的错误顺序和偏移，并覆盖 literal nullable pointer table、全 None、空数组、额外 padding、nested struct/map、union ordinal 与 nullable enum 校验。

## 初始复杂类型候选的分配诊断

这组 after 尚未包含 SIMD UTF-8。独立 consumer 在 `x86_64-apple-darwin`、`rustc 1.96.0`、release 下执行，不初始化 native runtime。before/after 使用同一份生成器、schema 和复杂 fixture；large 的请求 Params body 预算为 131072 字节，实际 bytes、scale、内容摘要均已核对相同。

每类先预热 10 个完整 encode/decode pair，然后各统计 **一次 encode 和一次 decode**。下表是 `alloc + alloc_zeroed + realloc` 的调用数，不包括 dealloc：

| Case | 实际请求 body bytes | Encode：before → after | Decode：before → after |
|---|---:|---:|---:|
| `utf8_string` | 131072 | 9 → 6 | 6 → 6 |
| `record` | 131072 | 24 → 12 | 20 → 20 |
| `record_array` | 131032 | 11678 → 2543 | 16064 → 14028 |
| `nested_array` | 130944 | 31 → 11 | 4332 → 1301 |
| `string_map` | 131072 | 9457 → 2121 | 15424 → 12151 |
| `numeric_map` | 131064 | 412 → 396 | 12487 → 4280 |
| `union_array` | 131024 | 11649 → 5353 | 20158 → 11924 |
| `nullable_nested` | 131048 | 16762 → 7488 | 29211 → 19587 |
| `string_array` | 131056 | 31 → 11 | 5483 → 1568 |

计数器只在单次 codec 调用区间开启。输入 clone、Params 构造、最终内容/位模式/摘要校验和输出 drop 在区间外；codec 内部的临时分配和销毁在区间内。全局 allocator 原样转发 `std::alloc::System`，用不分配内存的原子计数分别记录 alloc、alloc_zeroed、realloc、dealloc 的次数与请求/layout 字节。

**累计申请字节不是 RSS、存活内存、峰值内存或实际拷贝量。** realloc 可能原地完成；System/native 内部直接调用 malloc 的行为不在这个 Rust allocator 计数范围内。单次预热后的计数用于解释减少了哪些工作，不能据此推算 latency、吞吐提升或泄漏结论。计数 instrumentation 本身有成本，未参与正式 timing，也未加载 Mojo Core。

本地原始证据保存在：

- `target/validation/complex-allocations/before-allocations.json`
- `target/validation/complex-allocations/after-allocations.json`
- `target/validation/complex-allocations/execution-receipt.json`

回执包含两边实际 release binary SHA、解析后的 Cargo lock SHA、命令日志以及全部计数和申请字节。正式 ABBA 延迟、typed RTT、资源稳定性和可能回退项应从四轮无 instrumentation 的结果单独补充，不能以本表替代。

### 最终 SDK 的分配复核

加入 SIMD UTF-8 后，用新的独立 consumer 再次对照 `complex-before` 与冻结的 `complex-final`：两边 release 构建和九类 large 计数均 exit 0，fixture 的实际 bytes、scale 和内容摘要一致。**九类 encode/decode 的全部调用数与累计请求/layout 字节和上表对应的初始分配诊断完全相同**；UTF-8 校验的改变没有在这组已预热的单次调用中增加分配。这个结论只涉及计数，不表示延迟保持或改善了相同幅度。

运行明确使用配置的 Python 3.12，并将 `MOJOM_PYTHON` 指向同一解释器。两边 registry 依赖的唯一差异是最终 SDK 加入精确版本 `simdutf8 0.1.5`。冻结 SDK 文件、共享 14-file harness 和此前 allocation 原始数据/失败日志均已核验保留。

最终计数的独立原始证据位于 `target/validation/complex-final-allocations/`：`before-allocations.json`、`after-allocations.json`、`execution-receipt.json` 和 `verification-receipt.json`。旧 `complex-allocations/` 回执仍保留为初始候选诊断，没有被最终结果覆盖。最终四轮 timing 需使用 `complex-final` 单独记录。


## 最终正式测量与控制

正式结果位于 `target/validation/complex-final-abba/summary.json` / `summary.html`。相同 14-file harness、native DLL/dylib、fixture、Rust 1.96.0 release 配置执行 B/A/A/B，覆盖 9 类复杂值 × 3 种 body 预算。每项每轮至少 2 秒和 200 次观测，预热 20 次；大 case 为满足最小样本数可能运行更久。每个对照数值是两轮各自 p50 的中位数，不是合并样本的 p50，也不是置信区间。codec 计时边界外的 clone、最终验证和输出 drop 仍包含在完整循环吞吐中。

最终 after 主 binary SHA256 为 `b0ab20fc5ee9f612a82c065d8159e623890578bedcc57c3132850554fc24825b`，与实际编译的 UTF-8 candidate binary 字节相同。最终源码、候选编译来源、独立兼容 consumer 和实际工作区的关联见 `complex-final-checks/source-audit.json`。

large 预算为 131072 字节，各 case 实际 body bytes 见原始结果；下表单位为 µs：

| Case | Decode p50：before → after | Decode 倍率 | Typed RTT p50：before → after | RTT 倍率 |
|---|---:|---:|---:|---:|
| `utf8_string` | 156.883 → 15.9235 | 9.85× | 954.118 → 586.4195 | 1.63× |
| `record` | 114.1495 → 15.664 | 7.29× | 871.474 → 603.267 | 1.44× |
| `nested_array` | 837.377 → 207.0035 | 4.05× | 3235.507 → 1646.171 | 1.97× |
| `string_array` | 1194.467 → 252.7205 | 4.73× | 4252.324 → 1894.7965 | 2.24× |
| `nullable_nested` | 5512.1345 → 3461.1295 | 1.59× | 21144.906 → 14422.86 | 1.47× |

81 个 p50 对照（27 encode、27 decode、27 typed RTT）均更低；54 个去重后的完整循环吞吐对照均更高。**这不表示所有指标都改善**：`string_map/small` typed p99 的两轮中位数由 371.1555 升至 375.864 µs（+1.27%）。报告保留逐轮 p50/p99、样本数、elapsed 和 spread，p99 中位数也不是 pooled p99。

基础类型独立控制保存在 `complex-numeric-control/`，8 类各执行 ABBA + BAAB，64 次运行，不和复杂矩阵合并。mixed `uint64?` decode p50 为 17.003 → 16.9825 µs，但 p99 为 39.693 → 43.9915 µs（约 +10.83%）；1 MiB `uint8` encode/decode p50 分别约 +0.74%/+0.83%，完整 pair 吞吐约 -4.83%。这些观测保留为退步，没有用阈值过滤或一概归因于噪声。旧 byte/raw 控制为独立 ABBA，见 `complex-legacy-control/`：五档 typed RTT p50 均增加约 14–30%，64B p50 173.5605 → 219.579 µs（+26.51%），p99 303.246 → 647.916 µs（+113.66%）。raw 也有退步，但不能从 typed 中直接扣除 raw 变化；这些原始结果完整保留。

为复核旧路径，随后用同一份冻结二进制、harness、native 与全部参数独立执行反向 BAAB，见 `complex-legacy-recheck-baab/`，没有重编译或混合两组分位数。新组 typed p50：0B 83.099 → 82.955 µs（-0.17%）、64B 84.4925 → 83.693 µs（-0.95%）、1KiB +0.045%、1MiB +0.104%。先前小消息大幅退步未按原幅度重现，**仍不能宣布旧路径没有退步**：64KiB p50 296.1735 → 349.3455 µs（+17.95%），after 两轮为 272.199 / 426.492 µs；64B p99 +3.39%、64KiB p99 +17.92%、1MiB p99 +10.34%，部分 oneway 吞吐仍更低。新组每轮前后的 8 次环境抽查 SpeedLimit 均为 100，仍不是连续遥测。两组绝对延迟与相对趋势差异明显，不能证明变化都来自 codec 或都来自环境，也没有用新组覆盖旧组。全部 p50、p99、吞吐退步及配置核验由 `complex-control-summaries/` 的 `legacy` 与 `legacy-baab` 分别列出。

**环境限制：**本机 Intel macOS 运行期间的单点 `pmset -g therm` 记录 `CPU_Speed_Limit=57`，同次 Scheduler=100、AvailableCPUs=12。它不是每轮频率或温度 trace，不能证明具体 case 的退步原因。两轮 p50 存在明显 spread；倍率是该环境中的观测结果，不是跨平台承诺或“已达到理论极限”的证据。Windows/Linux x86_64、macOS arm64 仅完成编译检查，性能与运行稳定性仍需对应机器实测。

最终 after 独立稳定性测试完成 30.0023 秒、736774 次 sent/completed、window 64、final pending 0；100 次进程重建通过；故意 exit 17 的进程释放全部 64 个失败回调且 pending 0。共 104 个 worker：103 个正常 exit 0，1 个符合预期的 exit 17，没有强制回收。14 个静止检查点 FD 始终为 4、Mach port name count 始终为 38。RSS 在启动和性能阶段增长，随后约 48 MiB；短时观测和稳定的资源计数不能证明长期无泄漏。原始资源序列和所有缺失指标见 `complex-stability-after/`。

最终工作区测试 101 项通过（另 1 项 native 测试需 opt-in，已单独通过）；官方 generator 6 项检查按相同输入保留复用回执。三份独立 consumer 验证 77 个有效 fixture、47 个手工畸形 fixture、6 个 literal golden、6 个交叉解码方向。每份还执行 14936 个有界截断/低位变异；before/current 的接受状态、错误类别、Debug 和 offset 无差异。与 registry 0.1.0 比有 161 个拒绝到拒绝的诊断差异，来自较早的 compact array 路径；本轮极端 pointer overflow 的有意修正单独列于上文，不包含在该有界 sweep 中。不存在“所有畸形输入行为完全相同”或 exhaustive fuzzing 的结论。

这些优化仍在本地工作区，尚未发布新版 crate。
