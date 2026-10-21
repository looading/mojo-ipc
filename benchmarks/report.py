#!/usr/bin/env python3
"""Render RESULT_DIR/{result,resources,metadata}.json as an offline HTML report."""

from __future__ import annotations

import argparse
from collections import Counter
import html
import json
import math
import os
from pathlib import Path
import tempfile


def escaped(value) -> str:
    return html.escape(str(value), quote=True)


def number(value):
    return value if isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) else None


def fmt(value, places: int = 2) -> str:
    value = number(value)
    return f"{value:,.{places}f}" if value is not None else "—"


def integer(value) -> str:
    value = number(value)
    return f"{value:,.0f}" if value is not None else "—"


def ms(value, places: int = 3) -> str:
    return fmt(value / 1000, places) if number(value) is not None else "—"


def mib(value) -> str:
    return fmt(value / (1024 * 1024)) if number(value) is not None else "—"


def payload(value) -> str:
    if number(value) is None:
        return "—"
    if value >= 1024 * 1024 and value % (1024 * 1024) == 0:
        return f"{integer(value / (1024 * 1024))} MiB"
    if value >= 1024 and value % 1024 == 0:
        return f"{integer(value / 1024)} KiB"
    return f"{integer(value)} B"


def badge(passed, *, disabled: bool = False) -> str:
    if disabled:
        return '<span class="badge muted">未启用</span>'
    if passed is True:
        return '<span class="badge pass">本次通过</span>'
    if passed is False:
        return '<span class="badge fail">未通过</span>'
    return '<span class="badge muted">未记录</span>'


def caution(count) -> str:
    if number(count) is not None and count < 100:
        return '<strong class="caution">探索性：N &lt; 100</strong>'
    return ""


def table(headers: list[str], rows: list[list[str]], caption: str, *, identifier: str = "") -> str:
    identity = f' id="{escaped(identifier)}"' if identifier else ""
    head = "".join(f'<th scope="col">{escaped(value)}</th>' for value in headers)
    body = "".join("<tr>" + "".join(f"<td>{value}</td>" for value in row) + "</tr>" for row in rows)
    if not rows:
        body = f'<tr><td colspan="{len(headers)}">未记录</td></tr>'
    return f'<div class="table-wrap"><table{identity}><caption>{escaped(caption)}</caption><thead><tr>{head}</tr></thead><tbody>{body}</tbody></table></div>'


def read_object(path: Path) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"expected a JSON object: {path}")
    return value


def records(value) -> list[dict]:
    return [row for row in value if isinstance(row, dict)] if isinstance(value, list) else []


def object_at(value, name: str) -> dict:
    result = value.get(name)
    return result if isinstance(result, dict) else {}


def primitive_section(primitive: dict) -> str:
    if not primitive:
        return ""
    configuration = object_at(primitive, "configuration")
    typed, codec = records(primitive.get("typed_serial")), records(primitive.get("codec_only"))
    parts = ['<section><h2>其他基础类型数组：独立配置</h2>',
             table(["配置", "记录值"], [[escaped(key), escaped(json.dumps(value, ensure_ascii=False))]
                   for key, value in configuration.items()], "primitive_arrays.configuration", identifier="primitive-configuration"),
             '<p>按数组元素在 Mojom 中占用的数据字节预算统一比较，bool 为 ceil(elements/8)，其他类型按元素位宽计算。'
             '<strong>数组 wire bytes 不含数组、struct 或 message 头、指针和 ID，不等于完整传输流量。</strong>'
             '原 uint8 负载与参数保持独立。float/double 使用生成器实际的 OrderedFloat 字段，按 IEEE-754 bits 校验，保留 signed zero 和 NaN payload。</p>']
    typed_rows = []
    for row in typed:
        latency = object_at(row, "latency")
        typed_rows.append([
            escaped(row.get("primitive_type", "未记录")), integer(row.get("elements")),
            payload(row.get("target_array_wire_payload_bytes")), integer(row.get("array_wire_payload_bytes")),
            integer(row.get("window")), integer(row.get("warmup_count")), integer(row.get("minimum_completed")),
            integer(row.get("minimum_sample_ms")), integer(row.get("completed")),
            ms(latency.get("p50_us"), 6), ms(latency.get("p99_us"), 6), integer(latency.get("samples")),
            fmt(row.get("rpc_per_second")), fmt(row.get("array_wire_payload_one_direction_mib_per_second")),
            integer(row.get("final_pending")), caution(latency.get("samples")) or "—",
        ])
    parts += ['<h2>Primitive arrays：真实双进程串行 Echo</h2>',
              table(["类型", "元素数", "目标数组 payload", "实际数组 wire bytes", "Window", "Warmup", "完成下限", "最短 ms", "完成 RPC", "RTT p50 ms", "RTT p99 ms", "延迟 N", "RPC/s", "数组单向 MiB/s", "最终 pending", "样本提示"],
                    typed_rows, f"记录 {len(typed_rows)} 条 primitive typed case", identifier="primitive-typed"),
              '<p>RTT 从类型化方法调用到回调入口，输入 clone 和逐元素 bits 校验在 RTT 外、吞吐墙钟内。'
              '这是完整 Rust SDK 路径，不是裸 Mojo Core；每个 case 的实际元素数、样本量与固定短测量预算均单独展示。</p>']
    codec_rows = []
    for row in codec:
        encode, decode = object_at(row, "encode_latency"), object_at(row, "decode_latency")
        codec_rows.append([
            escaped(row.get("primitive_type", "未记录")), integer(row.get("elements")),
            payload(row.get("target_array_wire_payload_bytes")), integer(row.get("array_wire_payload_bytes")),
            integer(row.get("encoded_body_bytes")), integer(row.get("completed")),
            ms(encode.get("p50_us"), 6), ms(encode.get("p99_us"), 6), integer(encode.get("samples")),
            ms(decode.get("p50_us"), 6), ms(decode.get("p99_us"), 6), integer(decode.get("samples")),
            fmt(row.get("codec_pairs_per_second")), fmt(row.get("array_wire_payload_mib_per_second")),
            (caution(min(value for value in (encode.get("samples"), decode.get("samples")) if number(value) is not None)) or "—")
                if any(number(value) is not None for value in (encode.get("samples"), decode.get("samples"))) else "—",
        ])
    parts += ['<h2>Primitive arrays：进程内 codec 对照</h2>',
              table(["类型", "元素数", "目标数组 payload", "数组 wire bytes", "完整编码 body bytes", "完成 pairs", "Encode p50 ms", "Encode p99 ms", "Encode N", "Decode p50 ms", "Decode p99 ms", "Decode N", "Pairs/s", "数组 MiB/s", "样本提示"],
                    codec_rows, f"记录 {len(codec_rows)} 条 primitive codec case；计时阶段未初始化 native runtime", identifier="primitive-codec"),
              '<p>编码与解码分别计时，其分位数之和不是完整 pair p50 或 IPC RTT p50。Pairs/s 是含输入 clone、编码、解码和 bits 校验的完整循环吞吐；'
              '数组 MiB/s 每个 pair 只计一次数组元素数据。N 是存储的分位数样本量，可能采用 reservoir；N &lt; 100 的结果属于探索性。</p></section>']
    return "\n".join(parts)


def nullable_section(nullable: dict) -> str:
    if not nullable:
        return ""
    configuration = object_at(nullable, "configuration")
    typed, codec = records(nullable.get("typed_serial")), records(nullable.get("codec_only"))
    parts = ['<section><h2>Nullable 元素数组：独立配置与三种 null 模式</h2>',
             table(["配置", "记录值"], [[escaped(key), escaped(json.dumps(value, ensure_ascii=False))]
                   for key, value in configuration.items()], "nullable_arrays.configuration", identifier="nullable-configuration"),
             '<p>11 种类型分别覆盖 all_none、all_some、mixed（index % 3 == 0 为 None）。同一类型与目标预算选择相同元素数，'
             'None 仍占完整值槽。预算是数组 header 后的实际数据区：bool 为两组 ceil(elements/8) bitmap；'
             'numeric 为 tag bitmap、到元素宽度的内部对齐与所有固定值槽。取不超过预算的最大元素数，因此实际字节可能小于目标。'
             '<strong>实际数组 wire bytes 包含 tag、值和内部 padding，不含 8 字节 array header、尾部 8 字节对齐、'
             'struct/message 头、指针和 ID，不等于完整传输流量。</strong>吞吐按实际数组 wire bytes 计算。'
             'float/double 的 Option&lt;OrderedFloat&gt; 同时校验 None/Some tag 与 Some 的 IEEE-754 bits，包括 signed zero 和 NaN payload。</p>']

    def layout(row):
        return [escaped(row.get("primitive_type", "未记录")), escaped(row.get("null_pattern", "未记录")),
                integer(row.get("elements")), integer(row.get("null_count")),
                payload(row.get("target_array_wire_payload_bytes")), integer(row.get("array_wire_payload_bytes")),
                integer(row.get("array_tag_bitmap_bytes")), integer(row.get("array_value_bytes")),
                integer(row.get("array_internal_padding_bytes")), integer(row.get("array_trailing_alignment_bytes"))]

    layout_headers = ["类型", "Null 模式", "元素数", "None 数", "目标预算", "实际数组 wire bytes",
                      "Tag bytes", "值区 bytes", "内部 padding", "尾部 padding"]
    typed_rows = []
    for row in typed:
        latency = object_at(row, "latency")
        typed_rows.append(layout(row) + [
            integer(row.get("warmup_count")), integer(row.get("minimum_completed")), integer(row.get("minimum_sample_ms")),
            integer(row.get("completed")), ms(latency.get("p50_us"), 6), ms(latency.get("p99_us"), 6),
            integer(latency.get("samples")), fmt(row.get("rpc_per_second")),
            fmt(row.get("array_wire_payload_one_direction_mib_per_second")), integer(row.get("final_pending")),
            caution(latency.get("samples")) or "—",
        ])
    parts += ['<h2>Nullable arrays：真实双进程串行 Echo</h2>',
              table(layout_headers + ["Warmup", "完成下限", "最短 ms", "完成 RPC", "RTT p50 ms", "RTT p99 ms", "延迟 N",
                                      "RPC/s", "实际数组单向 MiB/s", "最终 pending", "样本提示"],
                    typed_rows, f"记录 {len(typed_rows)} 条 nullable typed case，window=1", identifier="nullable-typed"),
              '<p>这是完整 Rust SDK 路径，不是裸 Core。RTT 从类型化方法调用到回调入口；输入 clone 与 tag/bits 校验在 RTT 外、'
              '吞吐墙钟内。每个 case 固定短测量预算，不能将单轮大数组或 N &lt; 100 的 p99 当作稳定尾延迟。</p>']
    codec_rows = []
    for row in codec:
        encode, decode = object_at(row, "encode_latency"), object_at(row, "decode_latency")
        counts = [value for value in (encode.get("samples"), decode.get("samples")) if number(value) is not None]
        codec_rows.append(layout(row) + [
            integer(row.get("encoded_body_bytes")), integer(row.get("completed")),
            ms(encode.get("p50_us"), 6), ms(encode.get("p99_us"), 6), integer(encode.get("samples")),
            ms(decode.get("p50_us"), 6), ms(decode.get("p99_us"), 6), integer(decode.get("samples")),
            fmt(row.get("codec_pairs_per_second")), fmt(row.get("array_wire_payload_mib_per_second")),
            (caution(min(counts)) or "—") if counts else "—",
        ])
    parts += ['<h2>Nullable arrays：进程内 codec 对照</h2>',
              table(layout_headers + ["完整编码 body bytes", "完成 pairs", "Encode p50 ms", "Encode p99 ms", "Encode N",
                                      "Decode p50 ms", "Decode p99 ms", "Decode N", "Pairs/s", "实际数组 MiB/s", "样本提示"],
                    codec_rows, f"记录 {len(codec_rows)} 条 nullable codec case；计时阶段未初始化 native runtime", identifier="nullable-codec"),
              '<p>完整编码 body 包含本 case 的 struct header、ID、array pointer、array header、数据与尾部对齐，尚不含 IPC message header。'
              '实际编码后在计时区外校验 array header、元素数和 body 长度。Pairs/s 包含输入 clone、编码、解码、tag/bits 与 wire 长度校验；'
              'MiB/s 每个 pair 只计一次实际数组数据区。编码、解码分别计时，分位数之和不是完整 pair p50 或 IPC RTT p50。'
              'N 是保存的分位数样本量，可能采用 reservoir；N &lt; 100 标为探索性。</p></section>']
    return "\n".join(parts)


def complex_section(complex_types: dict) -> str:
    if not complex_types:
        return ""
    configuration = object_at(complex_types, "configuration")
    typed, codec = records(complex_types.get("typed_serial")), records(complex_types.get("codec_only"))
    expected = len(configuration.get("cases", [])) * len(configuration.get("target_encoded_body_budgets_bytes", []))
    parts = ['<section><h2>复杂类型：独立配置与实际 body 字节</h2>',
             table(["配置", "记录值"], [[escaped(key), escaped(json.dumps(value, ensure_ascii=False))]
                   for key, value in configuration.items()], "complex_types.configuration", identifier="complex-configuration"),
             f'<p>配置预期每阶段 {integer(expected)} 条；实际 typed {integer(len(typed))}、codec {integer(len(codec))}。'
             'UTF-8 字符串、string 数组、多字段 struct、struct 数组、嵌套数组、两类 map、四 variant union 数组与 nullable 嵌套组合分别测量。'
             '每档取请求 Params body 不超过预算的最大 fixture scale；<strong>实际编码 body bytes 包含 struct/array/string/map/union 头、指针及 padding，'
             '排除外层 Mojo message 与 native framing，不等于应用有效载荷或完整传输流量。</strong>吞吐按实际 body 字节计算。</p>',
             '<p>每次返回均完整逐字段比较，并计算 typed、length-prefixed FNV-1a64 逻辑摘要；map 按键排序计算摘要。'
             'None/Some、union variant、enum discriminant 与浮点 IEEE bits 都参与校验，包括 signed zero、NaN payload。'
             'HashMap 编码顺序可以变化；逻辑摘要不是 wire 摘要，也不是密码学完整性保证。</p>']

    def layout(row):
        return [escaped(row.get("complex_case", "未记录")), escaped(row.get("size_label", "未记录")),
                integer(row.get("scale")), integer(row.get("top_level_items")),
                payload(row.get("target_encoded_body_budget_bytes")), integer(row.get("request_encoded_body_bytes")),
                integer(row.get("response_encoded_body_bytes")), '<code>' + escaped(row.get("content_checksum_fnv1a64", "未记录")) + '</code>']

    headers = ["类型", "档次", "Scale", "顶层项目数", "请求 body 预算", "请求 body bytes", "响应 body bytes", "完整内容 FNV-1a64"]
    typed_rows = []
    for row in typed:
        latency = object_at(row, "latency")
        typed_rows.append(layout(row) + [integer(row.get("warmup_rpcs")), integer(row.get("minimum_completed")),
            integer(row.get("minimum_sample_ms")), integer(row.get("completed")), ms(latency.get("p50_us"), 6),
            ms(latency.get("p99_us"), 6), integer(latency.get("samples")), fmt(row.get("rpc_per_second")),
            fmt(row.get("encoded_body_one_direction_mib_per_second")), integer(row.get("final_pending")), caution(latency.get("samples")) or "—"])
    parts += ['<h2>复杂类型：真实双进程 typed 串行 Echo</h2>',
              table(headers + ["Warmup", "完成下限", "最短 ms", "完成 RPC", "RTT p50 ms", "RTT p99 ms", "延迟 N", "RPC/s", "请求 body MiB/s", "最终 pending", "样本提示"],
                    typed_rows, f"记录 {len(typed_rows)} 条 complex typed case；window=1", identifier="complex-typed"),
              '<p>RTT 从类型化方法调用到回调入口，包含两端 codec、native 传输、调度与回调；输入 clone、回调完整内容/bits/摘要校验和最终输出 drop 在 RTT 外。'
              '吞吐墙钟包含这些工作、mpsc driving 与等待。计时不包含代码生成。默认最少 8 样本、warmup 2 仅便于快测；正式应增加样本与时长，记录值见表。</p>']
    codec_rows = []
    for row in codec:
        encode, decode = object_at(row, "encode_latency"), object_at(row, "decode_latency")
        counts = [value for value in (encode.get("samples"), decode.get("samples")) if number(value) is not None]
        codec_rows.append(layout(row) + [integer(row.get("warmup_pairs")), integer(row.get("minimum_completed")),
            integer(row.get("minimum_sample_ms")), integer(row.get("completed")), ms(encode.get("p50_us"), 6),
            ms(encode.get("p99_us"), 6), integer(encode.get("samples")), ms(decode.get("p50_us"), 6),
            ms(decode.get("p99_us"), 6), integer(decode.get("samples")), fmt(row.get("codec_pairs_per_second")),
            fmt(row.get("encoded_body_mib_per_second")), (caution(min(counts)) or "—") if counts else "—"])
    parts += ['<h2>复杂类型：进程内 codec 对照</h2>',
              table(headers + ["Warmup", "完成下限", "最短 ms", "完成 pairs", "Encode p50 ms", "Encode p99 ms", "Encode N", "Decode p50 ms", "Decode p99 ms", "Decode N", "Pairs/s", "请求 body MiB/s", "样本提示"],
                    codec_rows, f"记录 {len(codec_rows)} 条 complex codec case；计时阶段未初始化 native runtime", identifier="complex-codec"),
              '<p>Encode 排除输入 clone；decode 排除返回值完整校验与最终销毁；codec 内部的分配、临时值销毁包含在各自阶段内。'
              'Pairs/s 包含 clone、两个阶段、完整校验、drop 与统计开销，每 pair 只计一次实际请求 body。'
              '<strong>Encode/decode 分位数之和不是 pair p50 或 IPC RTT p50。</strong>N 是存储的分位数样本量，超过 100000 用 Algorithm R reservoir，不截取前缀；min/mean/max 使用全部观测。</p>']
    fixture_rows = []
    for row in codec:
        fixture_rows.append([escaped(row.get("complex_case")), escaped(row.get("size_label")),
                             escaped(json.dumps(row.get("content_summary", {}), ensure_ascii=False, sort_keys=True))])
    parts += [table(["类型", "档次", "递归内容计数"], fixture_rows, "完整 fixture 递归统计；scale 含义因类型而异", identifier="complex-content"), '</section>']
    return "\n".join(parts)


def render(result: dict, resources: dict, metadata: dict) -> str:
    performance = records(result.get("performance"))
    raw = records(result.get("raw_transport"))
    codec = records(result.get("codec_only"))
    checkpoints = records(resources.get("checkpoints"))
    samples = records(resources.get("rss_samples"))
    machine = object_at(metadata, "hardware")
    native = object_at(metadata, "native_library")
    config = object_at(result, "configuration")
    typed_serial = {row.get("payload_bytes"): row for row in performance if row.get("name") == "echo_serial"}
    raw_serial = {row.get("payload_bytes"): row for row in raw if row.get("name") == "raw_bytes_serial"}
    codec_by_size = {row.get("payload_bytes"): row for row in codec}
    sizes = sorted(size for size in set(typed_serial) | set(raw_serial) if number(size) is not None)
    run_ok = result.get("ok") is True and resources.get("ok") is True and metadata.get("ok") is True
    parts = [
        '<header><p class="eyebrow">MOJO · RUST SDK · 本机实测</p><h1>类型化 IPC 性能与稳定性报告</h1>',
        f'<p>{badge(run_ok)} <span class="subtitle">{escaped(result.get("target", "未记录 target"))} · {escaped(metadata.get("started_utc", "时间未记录"))}</span></p>',
        '<p>完整 Rust SDK 双进程端到端测试，包含生成的协议代码、编解码、Mojo 传输、调度与回调。'
        'raw 对照仍包含 Rust 安全消息 API、watcher、拷贝及 channel，<strong>不是裸 Mojo Core</strong>。</p></header>',
    ]
    largest = max(sizes) if sizes else None
    if largest is not None and largest in typed_serial and largest in raw_serial:
        typed_latency = object_at(typed_serial[largest], "latency")
        raw_latency = object_at(raw_serial[largest], "latency")
        diagnosis = (f'<p>最大 payload {payload(largest)}：类型化串行 p50 <strong>{ms(typed_latency.get("p50_us"))} ms</strong>，'
                     f'raw 串行 p50 <strong>{ms(raw_latency.get("p50_us"))} ms</strong>。')
        if largest in codec_by_size:
            encode = object_at(codec_by_size[largest], "encode_latency")
            decode = object_at(codec_by_size[largest], "decode_latency")
            diagnosis += (f'同 payload 纯 codec 编码 p50 为 {ms(encode.get("p50_us"))} ms，'
                          f'解码 p50 为 {ms(decode.get("p50_us"))} ms。')
            if number(encode.get("p50_us")) is not None and number(decode.get("p50_us")) is not None:
                diagnosis += (f'两项 p50 之和为 {ms(encode["p50_us"] + decode["p50_us"])} ms；'
                              '这是成本尺度参考，<strong>不是实测 pair p50</strong>。往返包含两端编解码。'
                              '应结合 typed/raw 与 codec 对照选择 profiling 方向；不同计时口径不能直接相减归因。')
        parts += ['<section class="finding"><h2>本轮观察</h2>', diagnosis + '</p>',
                  '<p>以下低于 100 个样本的结果显著标为探索性。尤其不能用大包的单轮 p99 推断稳定尾延迟或生产 SLA。</p></section>']

    info = [
        ["机器", escaped(machine.get("model", machine.get("machine", "未记录")))],
        ["CPU", escaped(machine.get("processor", "未记录"))],
        ["CPU / 内存", f'{integer(machine.get("physical_cpus"))} 物理核 / {integer(machine.get("logical_cpus"))} 逻辑核；'
                        f'{fmt(machine["memory_bytes"] / (1024 ** 3)) if number(machine.get("memory_bytes")) is not None else "—"} GiB'],
        ["OS / target", f'{escaped(metadata.get("os", "未记录"))}<br><code>{escaped(result.get("target", "未记录"))}</code>'],
        ["编译", f'{escaped(result.get("compiler", "未记录"))}<br>{escaped(result.get("profile", "未记录"))} / '
                  f'opt-level={escaped(result.get("opt_level", "未记录"))} / optimized={escaped(result.get("optimized", "未记录"))}'],
        ["Chromium pin", f'<code>{escaped(result.get("chromium_revision", metadata.get("chromium_commit", "未记录")))}</code>'],
        ["源码 commit", f'<code>{escaped(metadata.get("repository_commit", "未记录"))}</code>'],
        ["原生库 SHA256", f'<code>{escaped(native.get("sha256", "未记录"))}</code><br><span class="small">{escaped(native.get("path", "未记录"))}</span>'],
        ["测试二进制 SHA256", f'<code>{escaped(metadata.get("binary_sha256", "未记录"))}</code>'],
    ]
    parts += ['<section><h2>环境与实际配置</h2>', table(["项目", "记录"], info, "复现实验环境"),
              table(["配置", "请求值"], [[escaped(key), escaped(value)] for key, value in config.items()], "本轮 harness 参数"),
              '<p>大 payload 使用次数与窗口预算；下表展示实际预热、实际完成数和生效下限。'
              f'RSS 周期采样间隔为 {fmt(resources.get("rss_interval_seconds"))} 秒；0 表示禁用周期采样，检查点仍采集。</p>',
              f'<details><summary>完整编译器与执行命令</summary><pre>{escaped(metadata.get("rustc", "未记录"))}\n\n'
              f'{escaped(json.dumps(metadata.get("command"), ensure_ascii=False, indent=2))}</pre></details></section>']

    comparison = []
    for size in sizes:
        typed, baseline = typed_serial.get(size, {}), raw_serial.get(size, {})
        tl, rl = object_at(typed, "latency"), object_at(baseline, "latency")
        comparison.append([payload(size), ms(tl.get("p50_us")), ms(tl.get("p99_us")),
                           integer(tl.get("samples")) + " " + caution(tl.get("samples")),
                           ms(rl.get("p50_us")), ms(rl.get("p99_us")),
                           integer(rl.get("samples")) + " " + caution(rl.get("samples"))])
    parts += ['<section><h2>类型化串行与 raw 串行对照</h2>',
              table(["应用 payload", "Typed p50 ms", "Typed p99 ms", "Typed N", "Raw p50 ms", "Raw p99 ms", "Raw N"],
                    comparison, f"{len(comparison)} 个 payload，window=1", identifier="serial-comparison"),
              '<p>Typed 延迟从方法调用到响应回调入口；raw 从发送 API 入口到 watcher 回调入口。'
              '两者均排除部分 payload 准备和回调验证开销，吞吐墙钟则包含这些工作。'
              'N 是记录的延迟样本数；这里直接展示原始分位数，未重新估计。</p></section>']

    codec_rows = []
    for row in codec:
        encode, decode = object_at(row, "encode_latency"), object_at(row, "decode_latency")
        codec_rows.append([payload(row.get("payload_bytes")), integer(row.get("completed")),
                           ms(encode.get("p50_us"), 6), ms(encode.get("mean_us"), 6),
                           ms(decode.get("p50_us"), 6), ms(decode.get("mean_us"), 6),
                           integer(encode.get("samples")) + " / " + integer(decode.get("samples")),
                           caution(row.get("completed")) or "—"])
    parts += ['<section><h2>纯 Mojom codec 对照</h2>',
              table(["应用 payload", "完成 pairs", "Encode p50 ms", "Encode 均值 ms", "Decode p50 ms", "Decode 均值 ms", "分位样本 Encode / Decode", "解释"],
                    codec_rows, "当前进程内编码 / 解码，计时阶段不初始化 native runtime", identifier="codec"),
              '<p>这不是 IPC 测量。均值来自全部 observations；超过容量时，分位数来自配对 reservoir 样本。'
              '编码、解码分别计时，完整 pair 循环的吞吐还包含输入 clone 和输出校验。</p></section>']

    names = {"echo_serial": "类型化串行", "echo_pipeline": "类型化流水线", "oneway_batched_barrier": "类型化单向＋屏障"}
    typed_rows = []
    for index, row in enumerate(performance, 1):
        latency = object_at(row, "latency")
        count = latency.get("samples")
        throughput = row.get("rpc_per_second", row.get("messages_per_second"))
        typed_rows.append([integer(index), escaped(names.get(row.get("name"), row.get("name", "未记录"))),
                           payload(row.get("payload_bytes")), integer(row.get("window")),
                           integer(row.get("warmup_requests", row.get("warmup_messages"))),
                           integer(row.get("effective_minimum_requests")), integer(row.get("completed")),
                           ms(latency.get("p50_us")), ms(latency.get("p99_us")), integer(count),
                           fmt(throughput), fmt(row.get("payload_one_direction_mib_per_second")),
                           integer(row.get("final_pending")), caution(count if count is not None else row.get("completed")) or "—"])
    parts += ['<section><h2>完整类型化负载结果</h2>',
              table(["#", "Case", "应用 payload", "实际 window", "实际 warmup", "生效次数下限", "完成数", "p50 ms", "p99 ms", "延迟 N", "RPC 或消息 / s", "单向应用 MiB/s", "最终 pending", "样本提示"],
                    typed_rows, f"全部 {len(typed_rows)} 个 typed / pipeline / oneway case", identifier="typed-cases"),
              '<p>单向模式的速率单位是消息/s，通过批次屏障确认完成；不采集逐条 RTT，因此延迟列为 —。'
              '应用 MiB/s 只按 payload 计数，不含 Mojom 头、对齐、平台传输和其他 wire 开销。'
              '窗口会改变排队时间；此结果还含 Rust channel、dispatch、校验与监测的调度开销。</p></section>']
    primitive = object_at(result, "primitive_arrays")
    if primitive:
        parts.append(primitive_section(primitive))
    nullable = object_at(result, "nullable_arrays")
    if nullable:
        parts.append(nullable_section(nullable))

    complex_types = object_at(result, "complex_types")
    if complex_types:
        parts.append(complex_section(complex_types))

    stress, churn, crash = (object_at(result, key) for key in ("stress", "churn", "crash"))
    stress_disabled = stress.get("enabled") is False
    stress_pass = None
    if not stress_disabled and stress:
        stress_pass = (result.get("ok") is True and number(stress.get("sent")) is not None
                       and stress.get("sent") == stress.get("completed") and stress.get("final_pending") == 0)
    churn_latency = object_at(churn, "cycle_latency")
    workers = records(resources.get("workers"))
    exit_codes = Counter(str(worker.get("exit_code", "未记录")) for worker in workers)
    stability = [
        ["持续负载", badge(stress_pass, disabled=stress_disabled),
         f'{fmt(stress.get("elapsed_seconds"), 3)} 秒；sent={integer(stress.get("sent"))}，completed={integer(stress.get("completed"))}，'
         f'final_pending={integer(stress.get("final_pending"))}；{fmt(stress.get("rpc_per_second"))} RPC/s'],
        ["连接 / worker churn", badge(churn.get("passed"), disabled=churn.get("enabled") is False),
         f'完成 {integer(churn.get("cycles"))} 轮；周期 p50={ms(churn_latency.get("p50_us"))} ms，'
         f'p99={ms(churn_latency.get("p99_us"))} ms，N={integer(churn_latency.get("samples"))}'],
        ["worker 异常退出", badge(crash.get("passed"), disabled=crash.get("enabled") is False),
         f'held={integer(crash.get("held_requests"))}；released_callbacks={integer(crash.get("released_callbacks"))}；'
         f'success_callbacks={integer(crash.get("success_callbacks"))}；final_pending={integer(crash.get("final_pending"))}；'
         f'预期退出码={integer(crash.get("expected_exit_code"))}'],
        ["其他实际计数", "记录值", f'边界 echo={integer(result.get("correctness_boundary_echoes"))}；'
         f'validation_errors={integer(result.get("validation_errors"))}；worker 生命周期记录={integer(len(workers))}；'
         f'退出码分布：{escaped(json.dumps(dict(exit_codes), ensure_ascii=False))}'],
    ]
    parts += ['<section><h2>本轮正确性与稳定性检查</h2>', table(["检查", "本次状态", "真实计数"], stability, "有限时长、有限次数的验收结果"),
              '<p>持续负载状态由本次 overall ok、sent=completed 和 final_pending=0 派生；churn/crash 状态取自原始报告。'
              '通过这些检查不构成无泄漏、长期可靠性或生产负载稳定性的保证。</p></section>']

    rss_parent, rss_worker, rss_sum = [], [], []
    for sample in samples:
        parent = object_at(sample, "parent").get("rss_bytes")
        measured_workers = [row.get("rss_bytes") for row in records(sample.get("workers"))]
        if number(parent) is not None:
            rss_parent.append(parent)
        rss_worker.extend(value for value in measured_workers if number(value) is not None)
        if number(parent) is not None and all(number(value) is not None for value in measured_workers):
            rss_sum.append(parent + sum(measured_workers))
    parts += ['<section><h2>静置检查点与资源趋势</h2>',
              f'<p>周期 RSS 样本 {integer(len(samples))} 组：父进程采样最大值 <strong>{mib(max(rss_parent) if rss_parent else None)} MiB</strong>，'
              f'单个 worker 采样最大值 {mib(max(rss_worker) if rss_worker else None)} MiB，'
              f'同一采样时刻父子 RSS 合计最大值 {mib(max(rss_sum) if rss_sum else None)} MiB。'
              '<strong>采样最大值不是真实峰值</strong>；RSS 合计可能重复计算共享页。</p>']
    first_rss = next((object_at(row, "parent").get("rss_bytes") for row in checkpoints
                      if number(object_at(row, "parent").get("rss_bytes")) is not None), None)
    stages = {"after_raw_transport": "raw 完成后", "after_performance": "typed 完成后",
              "after_stress": "持续负载后", "churn": "连接循环", "after_crash": "异常退出后",
              "after_primitive_arrays": "其他基础类型数组后", "after_nullable_arrays": "Nullable 元素数组后", "after_complex_types": "复杂类型矩阵后"}
    checkpoint_rows = []
    for row in checkpoints:
        parent = object_at(row, "parent")
        rss = parent.get("rss_bytes")
        delta = (rss - first_rss) / (1024 * 1024) if number(rss) is not None and first_rss is not None else None
        mach_count = row.get("mach_port_name_count")
        mach_cell = integer(mach_count) if number(mach_count) is not None else "未采集"
        if row.get("mach_port_probe_error"):
            mach_cell += '<br><span class="small">' + escaped(row["mach_port_probe_error"]) + '</span>'
        checkpoint_rows.append([escaped(stages.get(row.get("stage"), row.get("stage", "未记录"))),
                                integer(row.get("iteration")), fmt(row.get("elapsed_seconds"), 3), mib(rss),
                                fmt(delta), integer(parent.get("fd_count")), integer(parent.get("thread_count")),
                                integer(parent.get("handle_count")), mach_cell,
                                integer(len(row["active_worker_pids"])) if isinstance(row.get("active_worker_pids"), list) else "—",
                                fmt(row.get("sampling_ms"), 3)])
    parts += [table(["阶段", "轮次", "已运行 s", "父 RSS MiB", "相对首检查点 ΔMiB", "FD", "线程", "Windows handles", "Mach port 名字数", "active workers", "采样耗时 ms"],
                    checkpoint_rows, f"全部 {len(checkpoint_rows)} 个检查点；harness 排空 RPC、worker 退出并静置至少 50 ms 后，采样完成再 ACK", identifier="checkpoints"),
              '<p>— 表示 null、缺失或未测量，<strong>不代表 0</strong>。macOS 的 FD 仅计数 lsof 数字文件描述符，'
              '<strong>不覆盖 Mach ports</strong>；本 driver 未采集 macOS/Windows 线程数。'
              'Mach port 名字数由 macOS harness 在检查点提供，统计父 task IPC namespace 的名字数量，'
              '<strong>不统计每个名字的引用数</strong>，计数稳定不等于无泄漏。旧结果缺少字段、非 macOS 或探测失败时显示“未采集”；'
              '若记录了探测错误，会一并展示，不会用 0 补齐。'
              'FD/handles 只在检查点测量。未采集到的短命 worker 不包含在周期 RSS 最大值中。'
              '分配器缓存、页驻留和线程状态都会影响 RSS，不能用固定 RSS 增量阈值判定泄漏。</p></section>']
    parts += ['<footer><h2>原始记录</h2><p><a href="result.json">result.json</a> · '
              '<a href="resources.json">resources.json</a> · <a href="metadata.json">metadata.json</a> · '
              '<a href="raw.log">raw.log</a></p><p>本报告直接展示同目录数据，不修改输入，也不重算原始分位数。'
              '仅反映本机、本构建和本轮配置；不同平台、构建、窗口与监测设置应分别复测。</p></footer>']
    css = """
    :root{color-scheme:light;--ink:#172536;--muted:#516278;--line:#d9e1e9;--accent:#075985}
    *{box-sizing:border-box}body{margin:0;background:#f2f5f8;color:var(--ink);font:15px/1.65 system-ui,-apple-system,'Segoe UI',sans-serif}
    main{max-width:1500px;margin:0 auto;padding:32px 24px}header,section,footer{background:#fff;border:1px solid var(--line);border-radius:12px;padding:24px;margin-bottom:20px}
    header{border-top:5px solid var(--accent)}h1{font-size:30px;line-height:1.3;margin:8px 0 14px}h2{font-size:21px;margin:0 0 14px}p{margin:10px 0}.eyebrow{letter-spacing:.13em;font-size:12px;color:var(--accent);font-weight:700}
    .subtitle,.small,footer{color:var(--muted)}.small{font-size:12px}.finding{border-left:5px solid #c0841a;background:#fffcf2}.badge{display:inline-block;border-radius:5px;padding:2px 9px;font-size:13px;font-weight:700;white-space:nowrap}
    .pass{background:#d9f3e7;color:#155e43}.fail{background:#fee2e2;color:#991b1b}.muted{background:#edf1f5;color:#516278}.caution{display:block;color:#9a3412;background:#fff0db;padding:2px 5px;border:1px solid #f2c78a;border-radius:4px;white-space:nowrap;font-size:12px}
    .table-wrap{overflow-x:auto;margin:14px 0}table{border-collapse:collapse;width:100%;font-size:13px;font-variant-numeric:tabular-nums}caption{text-align:left;color:var(--muted);padding-bottom:8px;font-size:13px}
    th,td{border-bottom:1px solid var(--line);padding:10px 12px;text-align:right;vertical-align:top}th{background:#edf3f8;white-space:nowrap}td:first-child,th:first-child{text-align:left}tbody tr:nth-child(even){background:#fafcfe}td{min-width:70px}
    code,pre{font-family:ui-monospace,SFMono-Regular,Consolas,monospace;font-size:12px;overflow-wrap:anywhere}pre{white-space:pre-wrap;background:#f4f7fa;padding:14px;border-radius:6px}summary{cursor:pointer;color:var(--accent)}a{color:var(--accent)}
    @media(max-width:700px){main{padding:16px 10px}header,section,footer{padding:16px}h1{font-size:25px}}@media print{body{background:white}main{max-width:none;padding:0}header,section,footer{break-inside:avoid;border-radius:0}.table-wrap{overflow:visible}table{font-size:9px}th,td{padding:5px}}
    """
    return ('<!doctype html><html lang="zh-CN"><head><meta charset="utf-8">'
            '<meta name="viewport" content="width=device-width, initial-scale=1">'
            '<title>Mojo Rust SDK 实测报告</title><style>' + css + '</style></head><body><main>'
            + "\n".join(parts) + '</main></body></html>\n')


def generate_report(directory: Path) -> Path:
    directory = directory.expanduser().resolve()
    result = read_object(directory / "result.json")
    resources = read_object(directory / "resources.json")
    metadata = read_object(directory / "metadata.json")
    content = render(result, resources, metadata)
    destination = directory / "report.html"
    handle, name = tempfile.mkstemp(prefix=".report-", suffix=".html", dir=directory)
    temporary = Path(name)
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as stream:
            stream.write(content)
        temporary.replace(destination)
    finally:
        temporary.unlink(missing_ok=True)
    return destination


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("result_dir", type=Path)
    arguments = parser.parse_args()
    try:
        print(generate_report(arguments.result_dir))
        return 0
    except (OSError, ValueError, TypeError) as error:
        parser.exit(1, f"report generation failed: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
