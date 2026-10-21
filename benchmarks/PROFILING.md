# 独立 codec profiling probe

`codec_probe` 是 private benchmark package 的独立 binary，复用已有官方生成的 Mojom Params；不修改主 benchmark harness、不创建 worker、不加载 Mojo native runtime，也不要求 `MOJO_RUNTIME_LIBRARY`。它用于定位 CPU 热点和候选改动快测；正式端到端 before/after 仍使用 `benchmarks/run.py`。

Probe 已真实构建，before 的 bool 与 nullable-bool 独立采样已成功，记录位于 `target/validation/performance-profiling-before/`。后续候选测量仍在进行，本文不提前给出优化结论。以下命令用于独立复现；先结束其他正式 benchmark，再单独运行，避免互相干扰。

## 构建和快测

在仓库根目录构建，可用独立 target 目录与主测试隔离：

```sh
CARGO_TARGET_DIR=target/profiling \
CARGO_PROFILE_RELEASE_DEBUG=1 \
CARGO_PROFILE_RELEASE_STRIP=none \
cargo build --release --locked -p mojo-ipc-bench --bin codec_probe

xcrun dsymutil target/profiling/release/codec_probe
```

保留 release 优化；debug info 用于符号化。不要把不同 profile、编译器或 RUSTFLAGS 的数值直接作候选性能对比。采样报告中保存 binary SHA256、源码版本及实际构建参数，复现时使用同一组条件。

最短两秒 smoke 示例：

```sh
target/profiling/release/codec_probe \
  --case nullable-u32-64k --pattern mixed --operation pair \
  --seconds 2 --iterations 8 --warmup 8 \
  --output target/profiling/nullable-u32-mixed-smoke.json
```

输出路径必须不存在；省略 `--output` 时仅输出 stdout JSONL。默认运行 `u8-1m` / `pair`，至少 15 秒、8 次完成和 8 次预热。`--seconds` 是完整选择循环的最短墙钟时长，`--iterations` 是完成下限，两者均满足才结束。

| Case | 生成的 Params | 内容 |
| --- | --- | --- |
| `empty` | `Bench_Info_Params` | 真正无字段请求，编码 body 为 8 字节；不是空数组 |
| `u8-1m` | `Bench_Echo_Params` | 1 MiB `Vec<u8>` |
| `bool-64k` | `Bench_EchoBool_Params` | 524,288 个 bool，packed 数组数据 64 KiB |
| `nullable-bool-64k` | `Bench_EchoNullableBool_Params` | 两组 bitmap 总数据预算 64 KiB |
| `nullable-u32-64k` | `Bench_EchoNullableUint32_Params` | tag bitmap、内部对齐和值槽总数据预算 64 KiB |

Nullable 两种类型都要分别运行 `--pattern all_none`、`all_some`、`mixed`；mixed 的 None 在 `index % 3 == 0`。取最大不超过预算的元素数，实际 wire bytes 单列；None 保留完整值槽。非 nullable case 拒绝 `--pattern`。

`--operation encode` 只计编码操作，`decode` 反复解码一份已验证 wire body，`pair` 每轮编码再解码刚生成的字节。每轮输出都完整校验，首轮还校验 array header、元素数及完整 body 长度；错误会非零退出。浮点尚未纳入 probe；case adapter 允许自定义 equality，后续加入 float/double 必须按 Option tag 和 IEEE-754 bits 校验，不能使用 OrderedFloat 的等价语义。

## 计时边界

JSON 的 `timing` 分列 input_clone、encode、decode、validation 和 output_drop。不适用阶段为 null。各阶段有最多 100,000 个 reservoir 样本，分位数为 nearest-rank；均值、最小和最大值来自全部 observations。Encode/Decode 分位数之和不是 pair 分位数。

- `profile_clone`：创建 Params 并 clone 输入，在 encode 计时外。Serialize 消费输入，因此此准备不可假装不存在。
- `profile_encode`：只围绕 `serialize`。内部 AST、分配、拷贝与析构属于 codec 时间。
- `profile_decode`：只围绕 `deserialize_exact`；返回对象的验证和析构在 decode 计时外。
- `profile_validate_wire` / `profile_validate`：逐轮比对 wire oracle 或解码 fixture，校验 ID、值与 None/Some tag。
- `profile_drop`：返回值/字节缓冲的释放，在 codec 计时外。

这些函数使用 `#[inline(never)]` 提供调用树边界。完整 `operations_per_second` 包括输入 clone（需要时）、校验、返回对象释放、计时和统计；pair 每轮只计一次。数组 MiB/s 按每轮一份实际数组数据区计算，不含 array/struct/message 头、指针、ID 或尾部 padding。

Checksum 的 FNV-1a64 只在测量前对 wire oracle 计算一次；逐轮校验保证输出相同，iteration accumulator 记录完成过程，不把每轮全量 hash 混进测量。两秒 smoke 和带 profiler 的结果均为诊断值；空消息尤其受计时器与 wrapper 开销影响。

## macOS sample

本机 `/usr/bin/sample` 支持 `sample PID duration_seconds interval_ms -mayDie -fullPaths -file PATH`。它会周期性暂停指定进程并采集所有线程调用栈；默认间隔 1 ms。只附加本次启动、身份已确认的 PID，避免按模糊进程名附加其他任务。

下面的 Python 示例先等 probe 完成预热，再启动 10 秒采样并释放 START gate；probe 共跑 20 秒。不要与正式 benchmark 同时执行：

```python
import json
from pathlib import Path
import subprocess

out = Path("target/profiling/u8-encode")
out.mkdir(parents=True, exist_ok=False)
with (out / "probe.jsonl").open("w") as log:
    process = subprocess.Popen([
        "target/profiling/release/codec_probe", "--case", "u8-1m",
        "--operation", "encode", "--seconds", "20", "--start-gate",
        "--output", str(out / "result.json"),
    ], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    ready = json.loads(process.stdout.readline())
    log.write(json.dumps(ready) + "\n")
    assert ready["event"] == "probe_ready" and ready["pid"] == process.pid
    sampler = subprocess.Popen([
        "/usr/bin/sample", str(process.pid), "10", "1", "-mayDie", "-fullPaths",
        "-file", str(out / "sample.txt"),
    ])
    process.stdin.write("START\n")
    process.stdin.flush()
    process.stdin.close()
    for line in process.stdout:
        log.write(line)
    assert process.wait() == 0
    assert sampler.wait() == 0
```

`sample` 看到完整循环，包括 clone、校验、统计与释放。判断 codec 热点须看 `profile_encode` / `profile_decode` 子树；不能把整个进程样本都算进 codec，也不能把等待栈数量当 CPU 成本。采样会改变运行时间，独立无 profiler 重跑才可用作候选快测。小差异建议交错多轮、同机同构建比较，再用正式端到端 benchmark 验证。

现有主 harness 可在独立 profiling run 中按 `case_started` / `case_finished` 选择 codec、raw 或 typed 阶段：codec 在 native 初始化前执行；raw 仍含安全消息 API、拷贝、watcher、dispatch 与 mpsc，不是裸 Core。原 u8 的 `--sample-ms` 可延长窗口，primitive/nullable 则固定 200 ms，逐个 case 不够长，宜用本 probe。对 transport 阶段应同时采集 harness 父进程与 `worker_started` 真实 PID，分辨主线程等待、dispatch 与 native IO；多进程 `sample` 本身也有开销。不能从 typed/raw 延迟简单相减得到 codec 成本。

针对宽数值退步还提供 `numeric_probe`，复用原 codec probe 的计时边界，增加 nullable-u64-64k 和 nullable-float-64k（浮点按 Option tag 与 IEEE bits 校验，含 NaN/sNaN 和正负零）。
