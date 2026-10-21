# 0.2.1：逐 case 的 ABBA / BAAB 尾延迟对照

本轮 macOS x86_64 实测中，64 KiB typed 的 p99 在 overall、ABBA、BAAB 三个口径中分别下降
2.22%、1.01%、8.41%。**其余消息大小仍有 p99 回退**：0B 的 overall / BAAB 为
+4.73% / +9.44%，64B 为 +21.57% / +9.11%，1 MiB 为 +5.22% / +21.34%。
这组结果不能表述为整体 p99 或反向顺序回退已经解决。

完整结果见 [可移植 JSON](baselines/macos-x64-p99-baab-20261021.json)。25 个唯一主指标的
75 个重叠 scope 全部保留，其中 40 条为不利方向：overall 13、ABBA 17、BAAB 10，
涉及 21 个唯一指标。这里没有显著性阈值、异常值剔除或按结果重试；40 条也不是 40 个独立指标。

## 实现及兼容边界

本轮主要缩短既有接收路径：

- primary endpoint 的 deferred dispatch 使用具体 job 值，省去该 job 的一次 `Box` 分配。
  job 仍在 router / watcher 回调锁释放后调用；既有 pipe FIFO、异步调用边界和 associated
  endpoint 的 posted fallback 保留。
- `MojomMessage` 在同一次 `read_data_with` 回调中检查 header、复制所需 payload 并接管 handles，
  避免随后为读取相同数据再次进入 C API。Core 的借用 buffer 不跨回调保留，接收端所需的
  owned payload 拷贝仍存在。
- 修复真正空 Core 消息的 Rust slice 构造：native 可以合法返回 null buffer 和零长度，
  此时使用普通空 slice，避免 `from_raw_parts(null, 0)`。有 handles 的空 payload 也有原生回归测试。

这些是 0.2.1 的源码兼容改动，Mojom wire 格式和 native ABI / 二进制保持不变。
typed 的 0B 应用 payload 仍包含协议数据，不能代替真正零字节 Core 消息的验收。
早期候选暴露的空 buffer 测试失败记录单独保留；本页结果来自修复后重新冻结、独立构建的 r2。

## 固定测量方法

使用相同的 15 文件 benchmark harness 和原 legacy 请求 / 回调循环。
before 的生产源码来自 0.2.0 commit `0b0e29b9656abefad7d920aaee9c6ef82122b6bb`，
只归一化 Cargo / lock 版本元数据；before / after 均以 0.2.1 元数据独立 release 构建。
分别绑定 194 / 195 个 SDK 文件、共同 harness、Rust 1.96.0、锁文件、二进制与 native SHA。

共 7 个 case，每个 8 次，ABBA 与 BAAB 各 4 次；每个 block 每侧只有 2 次。
其中 A 为 before，B 为 after；不同 case 交替哪种 block 先执行，实际顺序保存在 JSON 中。

|实际 case 顺序|payload|block 先后|每次最短时长|
|---|---:|---|---:|
|typed-64k|65,536 B|ABBA → BAAB|5 s|
|raw-64k|65,536 B|BAAB → ABBA|5 s|
|typed-0|0 B|ABBA → BAAB|5 s|
|codec-64k|65,536 B|BAAB → ABBA|2 s|
|typed-1m|1,048,576 B|ABBA → BAAB|5 s|
|typed-64|64 B|BAAB → ABBA|5 s|
|codec-1m|1,048,576 B|ABBA → BAAB|2 s|

初始固定静默 20 秒，每次 trial 前固定静默 5 秒；不依据 CPU 状态或测得速度延长、重试或跳选。
每次新启动进程，只运行一个 phase / payload，window=1，请求下限参数为 1,000、预热参数为 100，
保留原 legacy 大消息次数预算。实际完成 N、存储样本 N、预热、生效下限和 elapsed 均逐 run 保存。
周期 RSS 采样关闭；退出 / 回收和原检查点仍在计时段外执行。

每个 run 逐条核对请求 ID 与完整 payload，记录 sent / completed / pending 和正常退出。
legacy loop 没有输出独立 runtime fixture checksum；JSON 不虚构该字段。
codec 分位数保留原 bounded reservoir 和实际观察 N，不把 encode / decode 分位数相加。

## 64 KiB typed：三种统计口径

先计算各 run 的分位数 / rate，再取同 label 的 run 指标中位数；没有池化原始请求样本。
overall 每侧 4 次，ABBA / BAAB 各每侧 2 次。

|指标|scope|before|after|变化|
|---|---|---:|---:|---:|
|p50 RTT (µs)|overall|285.5455|284.636|-0.3185%|
|p50 RTT (µs)|ABBA|283.4295|280.9485|-0.8754%|
|p50 RTT (µs)|BAAB|292.406|287.8755|-1.5494%|
|p99 RTT (µs)|overall|446.1605|436.273|-2.2161%|
|p99 RTT (µs)|ABBA|432.085|427.7315|-1.0076%|
|p99 RTT (µs)|BAAB|492.2185|450.8275|-8.4091%|
|RPC/s|overall|3,198.2708|3,219.5830|+0.6664%|
|RPC/s|ABBA|3,230.3342|3,246.2159|+0.4916%|
|RPC/s|BAAB|3,076.7916|3,172.7709|+3.1195%|

逐 run 的跨度同样重要：before 的 p99 为 421.541–534.745 µs，after 为
422.861–461.711 µs。p50 分别为 281.651–298.929 µs 和 279.137–289.239 µs。
单次最大 RTT 仍出现 before 55.261 ms、after 41.074 ms；分位数改善不表示消除了长暂停。

|run|block|label|完成 N|p50 (µs)|p99 (µs)|RPC/s|最大 RTT (µs)|
|---:|---|---|---:|---:|---:|---:|---:|
|0|ABBA|before|16,279|281.651|421.541|3,255.654|700.091|
|1|ABBA|after|16,245|279.137|422.861|3,248.988|41,074.229|
|2|ABBA|after|16,218|282.760|432.602|3,243.444|799.594|
|3|ABBA|before|16,026|285.208|442.629|3,205.015|2,843.736|
|4|BAAB|after|15,750|289.239|461.711|3,149.820|2,088.677|
|5|BAAB|before|14,811|298.929|534.745|2,962.056|55,261.441|
|6|BAAB|before|15,958|285.883|449.692|3,191.527|794.214|
|7|BAAB|after|15,979|286.512|439.944|3,195.722|801.202|

## 保留所有大小的 p99 与控制项回退

|typed payload|overall p99变化|ABBA变化|BAAB变化|
|---|---:|---:|---:|
|0 B|+4.7338%|-8.6503%|+9.4389%|
|64 B|+21.5718%|+21.8589%|+9.1120%|
|64 KiB|-2.2161%|-1.0076%|-8.4091%|
|1 MiB|+5.2168%|+2.2753%|+21.3396%|

下表是不利 scope 记录数，scope 之间重叠。包括未改变的 codec 控制项在内，方向不利的记录全部保留。

|case|overall|ABBA|BAAB|
|---|---:|---:|---:|
|codec-1m|0|5|0|
|codec-64k|5|4|1|
|raw-64k|2|3|1|
|typed-0|3|1|3|
|typed-1m|2|2|2|
|typed-64|1|2|3|
|typed-64k|0|0|0|
|合计|13|17|10|

112 个 CPU speed / scheduler limit 端点均为 100%，available CPUs 均为 12。
原始 aggregate load-average 也保留。端点不是连续热状态记录：这些读数与固定静默都不能证明
噪声已消除、两侧热状态相等，或将变化确定归因于实现 / 环境。每个 block 每侧两次，
不足以支持统计显著性或稳定生产 tail 保证。

typed 包含协议、codec、router、native、调度和宿主控制循环；raw 仍包含 Rust safe message / watcher /
native / control loop，不能当作裸 Mojo Core。旧正式八轮、旧补测和诊断结果不被本轮替换或池化。

## 独立有限稳定性

性能序列结束后，使用同一冻结 after 二进制另行运行完整性检查、30 秒压力、100 次进程 churn 与崩溃回收。
该运行不与上述分位数池化；它的微量 smoke 样本也不是新的性能结论。

- 30.00134561 秒内完成 1,444,961 次 1 KiB echo，sent=completed，pending=0。
- 100 次 churn 完成；崩溃 case 持有的 64 个回调全部释放，成功回调及 pending 均为 0，预期退出码为 17。
- 104 个 worker 全部回收（103×exit0，1×exit17），cleanup 为空；128 次边界大小 echo 校验通过。
- 14 个静止检查点的活动 worker 数均为 0，FD=4、Mach port name=38。
  RSS 从 31,137,792 增至 49,344,512 字节；churn10→100 又增 12,288 字节。
  thread / handle 未测量，保留为 null。

这是有限运行验收；资源快照不是峰值，稳定 FD / Mach name 数不证明没有泄漏或长期内存增长有界。
原始 SHA、配置、全部检查点和缺失值说明保存在 JSON 的 `independent_finite_stability` 中。

可移植投影保留全部 56 次 raw case 数值、25×3 统计、每 run N / min / max、原始文件 SHA 和
源码 / 二进制 / native 绑定，去除了绝对机器路径与 PID。发布、远程推送及独立 registry consumer
是否成功，应查看其单独回执；本页不预先声明发布结果。
