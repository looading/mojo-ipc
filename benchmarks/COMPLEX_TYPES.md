# 复杂类型 benchmark 与独立 codec probe

本矩阵使用实际生成的 Mojom 接口、真实两个 Rust 进程和同一份 fixture 做纯 codec 对照。默认仍运行此前的 `legacy,primitive,nullable` 三个矩阵；复杂类型需显式启用。计时包含生成的协议代码、codec、native 传输、调度与回调，不含构建或协议代码生成。这里没有预先写入性能结论。

## 选择矩阵与采样预算

```sh
# 正式复杂矩阵：9 类型 × 3 档 × codec/typed = 54 case。
python3 benchmarks/run.py --output-dir target/benchmark-runs/complex-A1 \
  --matrices complex --complex-codec-ms 2000 --complex-typed-ms 2000 \
  --complex-iterations 200 --complex-warmup 20

# 全部矩阵；原有 case 的参数和内容保持独立。
python3 benchmarks/run.py --output-dir target/benchmark-runs/all \
  --matrices legacy,primitive,nullable,complex

# 仅原有 byte/raw/codec/正确性/stress/churn/crash，用于复测此前回退。
python3 benchmarks/run.py --output-dir target/benchmark-runs/legacy \
  --matrices legacy

# 冻结后的二进制，复用监督器、checkpoint 采样与清理；建议先 B/A/A/B。
python3 benchmarks/run.py --skip-build --binary /absolute/path/frozen-before-binary \
  --output-dir target/benchmark-runs/complex-B1 --matrices complex \
  --complex-cases record_array,string_array,nullable_nested \
  --complex-codec-ms 2000 --complex-typed-ms 2000 \
  --complex-iterations 200 --complex-warmup 20
```

`--complex-cases` 接收逗号分隔的下面九个名称；只筛类型，三档仍分别运行。`--complex-codec-ms` 与 `--complex-typed-ms` 分别控制每个 case 最短测量时长，`0` 表示只按次数。`--complex-iterations` 是所有复杂 case 的最少完成数，没有按大包缩减。`--complex-warmup` 是每项未计入统计的预热次数。默认 `1000 ms / 8 / 2` 用于开发快测，不能因此声称正式尾延迟已充分采样。旧 `--sample-ms / --iterations / --warmup` 继续只控制原有 legacy case；primitive/nullable 保留先前固定配置。`stress/churn/crash` 只在 legacy 启用时运行，complex-only 不虚报这些检查通过。

运行 typed case 仍须设置 `MOJO_RUNTIME_LIBRARY` 为已验收的原生库。监督器的默认总超时会加入所选复杂 case 的预算；极大的最小完成数可能超过时长预算，可显式提高 `--total-timeout-seconds`。worker 的看门狗同样加入所选 typed case 最短时长，并保留 600 秒余量。

## Fixture 和字节口径

| Case | 内容 | Scale / 顶层项目 |
|---|---|---|
| `utf8_string` | 中文、emoji、NUL、非 ASCII 字符 | 重复次数 / Unicode scalar 数 |
| `record` | u64、负值 enum、i32、bool、double、UTF-8、bytes、可空 note | 可变字段规模 / 1 record |
| `record_array` | 字段非均匀的多字段 record 数组 | record 个数 |
| `nested_array` | u32 二维数组，含空与非空内层 | 外层数组元素数 |
| `string_map` | 唯一 UTF-8 key → record | map 条目数 |
| `numeric_map` | 正负 i32 key → 变长 bytes，含空值 | map 条目数 |
| `union_array` | Text / Record / Numbers / Metric 四 variant 循环 | union 个数，至少 4 |
| `nullable_nested` | outer None、Some 空字段、inner None、全 Some 四类循环，含 struct/array/map/union | 外层元素数，至少 4 |
| `string_array` | 空字符串及长短变化 UTF-8 字符串 | 字符串个数 |

small / medium / large 的目标是 **请求 generated Params body** 上限 `1024 / 16384 / 131072` 字节。预飞行实际调用官方生成类型的 serializer，通过指数搜索和二分选择不超预算的最大 scale；请求和响应 body 长度分别记录。这里包含 struct header、ID、指针、array/string/map/union header 和 padding，排除外层 Mojo message header、native framing 及 handles；不是应用 payload，也不是完整网络流量。不同类型的实际长度与元素数可以不同，结果同时写入目标预算、scale、顶层项目数、递归内容计数和实际 bytes。吞吐一律使用实际字节，不以目标代替。

所有返回内容逐字段验证，enum discriminant、union variant、None/Some tag 与 IEEE-754 bits 均参与；浮点包含 signed zero、无穷大、NaN payload 与 subnormal。另对每个完整返回值计算 length-prefixed、带类型/variant tag 的 canonical FNV-1a64 逻辑摘要。map 按 key 排序计算摘要，因此 `HashMap` 迭代或编码顺序变化不会误报。摘要不替代逐字段比较，也不是 wire hash 或密码学保证。

纯 codec 的输入 clone 在 encode 计时之外，完整返回值校验与最终 drop 在 decode 之外；codec 内部临时值的分配与销毁属于对应阶段。typed RTT 从方法调用到回调入口，输入 clone、回调完整内容校验和最终输出 drop 在 RTT 之外。两类吞吐墙钟均包含 clone、校验、drop、统计和 harness 驱动开销；typed 必须等验证和销毁后才发送下一条。codec 每 pair 计一次请求 body，typed 单向计请求 body，双向计请求加响应 body。

每项输出完成数、保存的延迟 N、p50/p99 与样本说明。延迟最多存储 100000 个 Algorithm R reservoir 样本，不取前 100000 前缀；min/mean/max 使用全部观察。N 小于 100 标探索性。Encode/decode 各自分位数之和不是 pair p50，也不是 RTT p50。应保留独立 ABBA 每轮数据，不能平均或合并各轮 p50 后称 pooled percentile 或置信区间。

## 独立 profile / focused ABBA

`complex_probe` 共享上述 fixture、scale 选择与完整内容验证，只运行进程内 codec，不初始化 native runtime，也不需要 `MOJO_RUNTIME_LIBRARY`。

```sh
cargo build --release -p mojo-ipc-bench --bin complex_probe --locked

# 快测示例；实际正式 focused 建议更长时间、多轮重复。
target/release/complex_probe --case record_array --size large \
  --operation decode --seconds 2 --iterations 200 --warmup 20 \
  --output target/probe-records-decode.json

# profile 入口：warmup 完成后输出 probe_ready 和实际 PID，等待 stdin 一行 START。
target/release/complex_probe --case string_array --size large \
  --operation decode --seconds 15 --iterations 200 --warmup 20 --start-gate
# 另一终端使用上面的 PID：
# sample <PID> 10 -file target/string-array-before.sample.txt
# 启动 sample 后，在 probe 的终端输入 START 并回车。
```

`--operation encode|decode|pair`，`--size small|medium|large`，`--seconds 0` 只按最小次数。`--output` 只能创建新文件，stdout 始终输出 JSONL。`#[inline(never)] profile_clone/profile_encode/profile_decode/profile_validate/profile_drop` 提供可识别采样边界；map canonical key 排序会出现在 validation，而不会混入 decode 时钟。decode-only 重复使用一个预飞行不可变 body；encode-only 每轮仍在编码时钟之外 decode 并验证新输出，所以 wall operations/s 不能冒充独立编码吞吐。pair 分别记录两阶段时间，完整 loop 吞吐另计。开启 `sample` 后的计时仅用于诊断，不混入无 profiler 的正式结果；它是统计采样，短函数未出现在栈上不证明耗时为零。

before/after 必须使用完全相同的 harness/schema/build 配置与 fixture。报告会核对 complex 配置、完整 case 集、实际 body 长度、scale 和 canonical 内容摘要；任意差异或单边 case 都保留并警示。报告由 `report.py` / `compare.py` 离线生成，不提供显著性或绝对最优保证。

## 四轮结果汇总

```sh
python3 benchmarks/complex_summary.py target/validation/complex-formal-abba
# 或 --output /absolute/path/summary.html；同时写同名 summary.json
```

输入目录固定包含 `0-before / 1-after / 2-after / 3-before`，每项均须有 `result/resources/metadata.json`；四轮原始 report 与日志保持独立。脚本核对配置/native/fixture，并为 27 typed、27 encode、27 decode 指标记录两轮各自数值、N、完整 body 长度、两轮 p50 spread、每 label 两个 run-level p50/p99 的中位数，以及所有 AFTER p50 更高项。缺项、重复或不一致不会被默认为可比；失败时退出非零并保留可审查页面。这个中位数不表示 pooled percentile，两个重复也不足以证明差异的成因。
