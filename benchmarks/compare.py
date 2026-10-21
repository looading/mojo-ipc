#!/usr/bin/env python3
"""Compare two recorded runs: BEFORE_DIR AFTER_DIR --output comparison.html."""

from __future__ import annotations

import argparse
from collections import Counter
import html
import json
import math
import os
from pathlib import Path
import tempfile
from urllib.parse import quote


def esc(value) -> str:
    return html.escape(str(value), quote=True)


def numeric(value):
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        try:
            return value if math.isfinite(value) else None
        except OverflowError:
            pass
    return None


def fmt(value, digits: int = 3) -> str:
    return f"{value:,.{digits}f}" if numeric(value) is not None else "未采集"


def count(value) -> str:
    return f"{value:,.0f}" if numeric(value) is not None else "未采集"


def ms(value) -> str:
    return fmt(value / 1000, 6) if numeric(value) is not None else "未采集"


def mib(value) -> str:
    return fmt(value / (1024 * 1024)) if numeric(value) is not None else "未采集"


def ratio(numerator, denominator) -> str:
    if numeric(numerator) is None or numeric(denominator) is None or denominator <= 0:
        return "—"
    return f"{numerator / denominator:,.3f}×"


def payload(value) -> str:
    if numeric(value) is None:
        return "未记录"
    if value >= 1024 * 1024 and value % (1024 * 1024) == 0:
        return f"{count(value / (1024 * 1024))} MiB"
    if value >= 1024 and value % 1024 == 0:
        return f"{count(value / 1024)} KiB"
    return f"{count(value)} B"


def obj(value, key: str) -> dict:
    child = value.get(key)
    return child if isinstance(child, dict) else {}


def rows(value) -> list[dict]:
    return [row for row in value if isinstance(row, dict)] if isinstance(value, list) else []


def present(value) -> bool:
    if value is None or value == "":
        return False
    if isinstance(value, dict):
        return bool(value) and all(present(child) for child in value.values())
    return True


def plain(value) -> str:
    if not present(value):
        return "未记录"
    if isinstance(value, (dict, list)):
        return esc(json.dumps(value, ensure_ascii=False, sort_keys=True))
    return esc(value)


def table(headers: list[str], contents: list[list[str]], identifier: str) -> str:
    heading = "".join(f'<th scope="col">{esc(value)}</th>' for value in headers)
    body = "".join("<tr>" + "".join(f"<td>{value}</td>" for value in row) + "</tr>" for row in contents)
    if not contents:
        body = f'<tr><td colspan="{len(headers)}">两边均未记录</td></tr>'
    return f'<div class="scroll"><table id="{esc(identifier)}"><thead><tr>{heading}</tr></thead><tbody>{body}</tbody></table></div>'


def load(directory: Path) -> dict:
    run = {"directory": directory.expanduser().resolve()}
    for name in ("result", "resources", "metadata"):
        value = json.loads((run["directory"] / f"{name}.json").read_text(encoding="utf-8"))
        if not isinstance(value, dict):
            raise ValueError(f"{run['directory']}/{name}.json must contain a JSON object")
        run[name] = value
    return run


def checks(before: dict, after: dict) -> list[tuple]:
    result = []
    fields = (
        ("target", lambda run: run["result"].get("target")),
        ("compiler", lambda run: run["result"].get("compiler")),
        ("native SHA256", lambda run: obj(run["metadata"], "native_library").get("sha256")),
        ("configuration（完整）", lambda run: obj(run["result"], "configuration")),
        ("Chromium pin", lambda run: run["result"].get("chromium_revision")),
        ("profile / opt-level / optimized", lambda run: {key: run["result"].get(key) for key in ("profile", "opt_level", "optimized")}),
        ("OS", lambda run: run["metadata"].get("os")),
        ("硬件", lambda run: run["metadata"].get("hardware")),
        ("周期 RSS 间隔（秒）", lambda run: run["resources"].get("rss_interval_seconds")),
        ("RUSTFLAGS", lambda run: obj(run["metadata"], "environment").get("RUSTFLAGS", "<未设置>")),
    )
    if "primitive_arrays" in before["result"] or "primitive_arrays" in after["result"]:
        fields += (("primitive_arrays.configuration（完整）",
                    lambda run: obj(obj(run["result"], "primitive_arrays"), "configuration")),)
    if "nullable_arrays" in before["result"] or "nullable_arrays" in after["result"]:
        fields += (("nullable_arrays.configuration（完整）",
                    lambda run: obj(obj(run["result"], "nullable_arrays"), "configuration")),)
    if "complex_types" in before["result"] or "complex_types" in after["result"]:
        fields += (("complex_types.configuration（完整）",
                    lambda run: obj(obj(run["result"], "complex_types"), "configuration")),
                   ("complex fixtures / 全部内容摘要 / 实际 body", complex_fixture_signature))
    for name, getter in fields:
        left, right = getter(before), getter(after)
        result.append((name, left, right, present(left) and present(right) and left == right))
    return result


def index(records: list[dict], phase: str) -> dict:
    result = {}
    for record in records:
        key = (record.get("name"), record.get("payload_bytes"), record.get("window"))
        if not isinstance(key[0], str) or numeric(key[1]) is None:
            raise ValueError(f"{phase}: case requires a name and numeric payload_bytes")
        if numeric(key[2]) is None and key[2] is not None:
            raise ValueError(f"{phase}: window must be numeric or absent")
        if key in result:
            raise ValueError(f"{phase}: duplicate case key {key!r}")
        result[key] = record
    return result


def sorted_keys(before: dict, after: dict) -> list:
    return sorted(set(before) | set(after), key=lambda key: (key[1], str(key[0]), -1 if key[2] is None else key[2]))


def match_state(left: dict, right: dict) -> str:
    if not left:
        return '<strong class="warning">仅 AFTER</strong>'
    if not right:
        return '<strong class="warning">仅 BEFORE</strong>'
    return "已匹配"


def small_sample(left, right) -> str:
    sides = [label for label, value in (("BEFORE", left), ("AFTER", right))
             if numeric(value) is not None and value < 100]
    return '<strong class="warning">探索性：' + esc(" / ".join(sides)) + ' N &lt; 100</strong>' if sides else "—"


def latency_table(before: list[dict], after: list[dict], identifier: str,
                  latency_key: str = "latency", codec: bool = False) -> str:
    left_index, right_index = index(before, identifier + " BEFORE"), index(after, identifier + " AFTER")
    contents = []
    for name, size, window in sorted_keys(left_index, right_index):
        key = (name, size, window)
        left, right = left_index.get(key, {}), right_index.get(key, {})
        ll, rl = obj(left, latency_key), obj(right, latency_key)
        throughput_key = "codec_pairs_per_second" if codec else (
            "messages_per_second" if name == "oneway_batched_barrier" else "rpc_per_second")
        left_rate, right_rate = left.get(throughput_key), right.get(throughput_key)
        unit = "pairs/s" if codec else ("消息/s" if throughput_key == "messages_per_second" else "RPC/s")
        sample_left, sample_right = ll.get("samples"), rl.get("samples")
        contents.append([
            esc(name), payload(size), count(window) if window is not None else "—", match_state(left, right),
            ms(ll.get("p50_us")), ms(rl.get("p50_us")), ratio(ll.get("p50_us"), rl.get("p50_us")),
            ms(ll.get("p99_us")), ms(rl.get("p99_us")), ratio(ll.get("p99_us"), rl.get("p99_us")),
            count(sample_left), count(sample_right), fmt(left_rate), fmt(right_rate), ratio(right_rate, left_rate), unit,
            count(left.get("completed")) + " / " + count(right.get("completed")),
            small_sample(sample_left if sample_left is not None else left.get("completed"),
                         sample_right if sample_right is not None else right.get("completed")),
        ])
    return table(["Case", "应用 payload", "Window", "匹配", "B p50 ms", "A p50 ms", "p50 B/A", "B p99 ms", "A p99 ms", "p99 B/A", "B 延迟 N", "A 延迟 N", "B 吞吐", "A 吞吐", "吞吐 A/B", "单位", "完成数 B / A", "样本提示"], contents, identifier)


def primitive_index(records: list[dict], phase: str) -> dict:
    result = {}
    for row in records:
        key = (row.get("primitive_type"), row.get("name"),
               row.get("array_wire_payload_bytes", row.get("payload_bytes")), row.get("window"))
        if not isinstance(key[0], str) or not key[0] or not isinstance(key[1], str) or numeric(key[2]) is None:
            raise ValueError(f"{phase}: primitive case requires primitive_type, name and numeric array wire bytes")
        if numeric(key[3]) is None and key[3] is not None:
            raise ValueError(f"{phase}: primitive window must be numeric or absent")
        if key in result:
            raise ValueError(f"{phase}: duplicate primitive case {key!r}")
        result[key] = row
    return result


def primitive_latency_table(before: list[dict], after: list[dict], identifier: str,
                            latency_key: str = "latency", codec: bool = False) -> str:
    left_index = primitive_index(before, identifier + " BEFORE")
    right_index = primitive_index(after, identifier + " AFTER")
    type_order = {name: index for index, name in enumerate(("bool", "int8", "int16", "uint16", "int32", "uint32", "int64", "uint64", "float", "double"))}
    keys = sorted(set(left_index) | set(right_index), key=lambda key: (
        type_order.get(key[0], 100), key[0], key[2], key[1], -1 if key[3] is None else key[3]))
    contents = []
    for key in keys:
        kind, name, wire_bytes, window = key
        left, right = left_index.get(key, {}), right_index.get(key, {})
        ll, rl = obj(left, latency_key), obj(right, latency_key)
        rate_key = "codec_pairs_per_second" if codec else "rpc_per_second"
        status = match_state(left, right)
        if left and right and any(left.get(field) != right.get(field) for field in (
                "elements", "element_bits", "target_array_wire_payload_bytes", "minimum_sample_ms", "minimum_completed", "warmup_count")):
            status += '<br><strong class="warning">case 参数差异</strong>'
        contents.append([
            esc(kind), esc(name), count(wire_bytes), count(left.get("elements")) + " / " + count(right.get("elements")),
            count(window) if window is not None else "—", status,
            ms(ll.get("p50_us")), ms(rl.get("p50_us")), ratio(ll.get("p50_us"), rl.get("p50_us")),
            ms(ll.get("p99_us")), ms(rl.get("p99_us")), ratio(ll.get("p99_us"), rl.get("p99_us")),
            count(ll.get("samples")), count(rl.get("samples")), fmt(left.get(rate_key)), fmt(right.get(rate_key)),
            ratio(right.get(rate_key), left.get(rate_key)), "pairs/s" if codec else "RPC/s",
            count(left.get("completed")) + " / " + count(right.get("completed")),
            small_sample(ll.get("samples"), rl.get("samples")),
        ])
    return table(["类型", "Case", "数组 wire bytes", "元素数 B / A", "Window", "匹配", "B p50 ms", "A p50 ms", "p50 B/A", "B p99 ms", "A p99 ms", "p99 B/A", "B 延迟 N", "A 延迟 N", "B 吞吐", "A 吞吐", "吞吐 A/B", "单位", "完成数 B / A", "样本提示"], contents, identifier)


def primitive_section(before: dict, after: dict) -> str:
    if "primitive_arrays" not in before["result"] and "primitive_arrays" not in after["result"]:
        return ""
    left, right = obj(before["result"], "primitive_arrays"), obj(after["result"], "primitive_arrays")
    lc, rc = obj(left, "configuration"), obj(right, "configuration")
    contents = [[esc(key), plain(lc.get(key)), plain(rc.get(key))] for key in sorted(set(lc) | set(rc))]
    parts = ['<section><h2>其他基础类型数组：配置与 wire 预算</h2>',
             table(["Primitive 配置", "BEFORE", "AFTER"], contents, "primitive-configuration"),
             '<p>匹配 key 包含 primitive_type、case 名、数组 wire bytes 和 window，不会将不同类型的同名 case 覆盖。'
             'Wire bytes 只计数组元素数据：bool 为 ceil(elements/8)，其他类型按位宽；不含数组、struct/message 头、指针和 ID。'
             '同 wire 字节数的不同类型元素数不同。float/double 使用 OrderedFloat 字段并校验 IEEE-754 bits，包括 signed zero 与 NaN payload。</p>',
             '<h2>Primitive typed：双进程串行 RTT</h2>',
             primitive_latency_table(rows(left.get("typed_serial")), rows(right.get("typed_serial")), "primitive-typed"),
             '<h2>Primitive codec：编码</h2>',
             primitive_latency_table(rows(left.get("codec_only")), rows(right.get("codec_only")), "primitive-codec-encode", "encode_latency", True),
             '<h2>Primitive codec：解码</h2>',
             primitive_latency_table(rows(left.get("codec_only")), rows(right.get("codec_only")), "primitive-codec-decode", "decode_latency", True),
             '<p>Typed 是完整 Rust SDK 路径，不是裸 Core。Codec 两张表的吞吐均为同一完整 pair 循环的 pairs/s，包含输入 clone 和 bits 校验，'
             '不是独立编码或解码吞吐。编码 / 解码分位数之和不是总 p50、pair p50 或 RTT 分位数；N 可能来自 reservoir。'
             '任一边 N &lt; 100 均显著标为探索性，不据此宣称稳定尾延迟。只有一边有 primitive 记录时，配置检查标未记录并保留单边结果。</p></section>']
    return "\n".join(parts)


def nullable_index(records: list[dict], phase: str) -> dict:
    result = {}
    for row in records:
        key = (row.get("primitive_type"), row.get("null_pattern"), row.get("target_array_wire_payload_bytes"),
               row.get("elements"), row.get("name"), row.get("window"))
        if any(not isinstance(key[index], str) or not key[index] for index in (0, 1, 4)):
            raise ValueError(f"{phase}: nullable case requires primitive_type, null_pattern and name")
        if any(numeric(key[index]) is None or key[index] < 0 for index in (2, 3)):
            raise ValueError(f"{phase}: nullable target wire budget and elements must be nonnegative numbers")
        if numeric(key[5]) is None and key[5] is not None:
            raise ValueError(f"{phase}: nullable window must be numeric or absent")
        if key in result:
            raise ValueError(f"{phase}: duplicate nullable case {key!r}")
        result[key] = row
    return result


def nullable_latency_table(before: list[dict], after: list[dict], identifier: str,
                           latency_key: str = "latency", codec: bool = False) -> str:
    left_index, right_index = nullable_index(before, identifier + " BEFORE"), nullable_index(after, identifier + " AFTER")
    type_order = {name: index for index, name in enumerate(("bool", "int8", "uint8", "int16", "uint16", "int32", "uint32", "int64", "uint64", "float", "double"))}
    pattern_order = {name: index for index, name in enumerate(("all_none", "all_some", "mixed"))}
    keys = sorted(set(left_index) | set(right_index), key=lambda key: (
        type_order.get(key[0], 100), key[0], key[2], pattern_order.get(key[1], 100), key[1], key[3], key[4],
        -1 if key[5] is None else key[5]))
    contents = []
    for key in keys:
        kind, pattern, budget, elements, name, window = key
        left, right = left_index.get(key, {}), right_index.get(key, {})
        ll, rl = obj(left, latency_key), obj(right, latency_key)
        rate_key = "codec_pairs_per_second" if codec else "rpc_per_second"
        mib_key = "array_wire_payload_mib_per_second" if codec else "array_wire_payload_one_direction_mib_per_second"
        status = match_state(left, right)
        if left and right and any(left.get(field) != right.get(field) for field in (
                "array_wire_payload_bytes", "null_count", "element_bits", "array_header_bytes", "array_tag_bitmap_bytes",
                "array_value_bytes", "array_internal_padding_bytes", "array_trailing_alignment_bytes",
                "expected_encoded_body_bytes", "minimum_sample_ms", "minimum_completed", "warmup_count")):
            status += '<br><strong class="warning">case 参数 / wire 布局差异</strong>'
        contents.append([
            esc(kind), esc(pattern), esc(name), payload(budget), count(elements),
            count(left.get("array_wire_payload_bytes")) + " / " + count(right.get("array_wire_payload_bytes")),
            count(left.get("null_count")) + " / " + count(right.get("null_count")),
            count(window) if window is not None else "—", status,
            ms(ll.get("p50_us")), ms(rl.get("p50_us")), ratio(ll.get("p50_us"), rl.get("p50_us")),
            ms(ll.get("p99_us")), ms(rl.get("p99_us")), ratio(ll.get("p99_us"), rl.get("p99_us")),
            count(ll.get("samples")), count(rl.get("samples")), fmt(left.get(rate_key)), fmt(right.get(rate_key)),
            ratio(right.get(rate_key), left.get(rate_key)), "pairs/s" if codec else "RPC/s",
            fmt(left.get(mib_key)), fmt(right.get(mib_key)),
            count(left.get("completed")) + " / " + count(right.get("completed")),
            small_sample(ll.get("samples"), rl.get("samples")),
        ])
    return table(["类型", "Null 模式", "Case", "目标预算", "元素数", "实际 wire bytes B / A", "None 数 B / A", "Window", "匹配",
                  "B p50 ms", "A p50 ms", "p50 B/A", "B p99 ms", "A p99 ms", "p99 B/A", "B 延迟 N", "A 延迟 N",
                  "B 吞吐", "A 吞吐", "吞吐 A/B", "单位", "B 实际数组 MiB/s", "A 实际数组 MiB/s", "完成数 B / A", "样本提示"],
                 contents, identifier)


def nullable_section(before: dict, after: dict) -> str:
    if "nullable_arrays" not in before["result"] and "nullable_arrays" not in after["result"]:
        return ""
    left, right = obj(before["result"], "nullable_arrays"), obj(after["result"], "nullable_arrays")
    lc, rc = obj(left, "configuration"), obj(right, "configuration")
    contents = [[esc(key), plain(lc.get(key)), plain(rc.get(key))] for key in sorted(set(lc) | set(rc))]
    parts = ['<section><h2>Nullable 元素数组：配置、wire 预算与三种 null 模式</h2>',
             table(["Nullable 配置", "BEFORE", "AFTER"], contents, "nullable-configuration"),
             '<p>匹配 key 包含类型、null 模式、目标 wire 预算、元素数、case 名与 window；不同模式或元素数不会相互覆盖。'
             'All_none、all_some 与 mixed（index % 3 == 0 为 None）分别展示。None 仍占固定值槽。预算选最大可容纳元素数：'
             'bool 是两个 ceil(elements/8) bitmap；numeric 是 tag bitmap、内部对齐与所有值槽。'
             '<strong>实际 wire bytes 可小于目标预算，包含 tag、值和内部 padding，排除 array header、尾部对齐及 struct/message 头等开销；'
             '不是完整传输流量。</strong>MiB/s 使用各自实际数组字节数，typed 计单向，codec 每 pair 计一次。'
             'float/double 按 Option tag 与 Some IEEE-754 bits 校验，包括 signed zero 与 NaN payload。</p>',
             '<h2>Nullable typed：双进程串行 RTT</h2>',
             nullable_latency_table(rows(left.get("typed_serial")), rows(right.get("typed_serial")), "nullable-typed"),
             '<h2>Nullable codec：编码</h2>',
             nullable_latency_table(rows(left.get("codec_only")), rows(right.get("codec_only")), "nullable-codec-encode", "encode_latency", True),
             '<h2>Nullable codec：解码</h2>',
             nullable_latency_table(rows(left.get("codec_only")), rows(right.get("codec_only")), "nullable-codec-decode", "decode_latency", True),
             '<p>Typed 包含完整 Rust SDK 路径，不是裸 Core。Codec 两张表的 pairs/s 与 MiB/s 都来自同一完整 pair 循环，'
             '包含输入 clone、tag/bits 与 wire 长度校验，不是独立 encode 或 decode 吞吐。'
             '编码与解码分位数之和不是完整 pair p50、总 p50 或 IPC RTT 分位数；N 可能来自 reservoir。'
             '任一边 N &lt; 100 标为探索性，不能据此宣称稳定尾延迟。单边记录保留并标明未匹配，配置缺失不视为一致。</p></section>']
    return "\n".join(parts)


def complex_index(records: list[dict], phase: str) -> dict:
    result = {}
    for row in records:
        key = (row.get("complex_case"), row.get("size_label"), row.get("target_encoded_body_budget_bytes"),
               row.get("scale"), row.get("name"), row.get("window"))
        if any(not isinstance(key[index], str) or not key[index] for index in (0, 1, 4)):
            raise ValueError(f"{phase}: complex case requires complex_case, size_label and name")
        if any(numeric(key[index]) is None or key[index] < 0 for index in (2, 3)):
            raise ValueError(f"{phase}: complex body budget and scale must be nonnegative numbers")
        if numeric(key[5]) is None and key[5] is not None:
            raise ValueError(f"{phase}: complex window must be numeric or absent")
        if key in result:
            raise ValueError(f"{phase}: duplicate complex key {key!r}")
        result[key] = row
    return result


def complex_fixture_signature(run: dict):
    section = obj(run["result"], "complex_types")
    configuration = obj(section, "configuration")
    expected = len(configuration.get("cases", [])) * len(configuration.get("target_encoded_body_budgets_bytes", []))
    if not expected:
        return None
    signatures = {}
    fields = ("request_encoded_body_bytes", "response_encoded_body_bytes", "top_level_items",
              "content_checksum_fnv1a64", "content_summary", "content_validated")
    for phase in ("codec_only", "typed_serial"):
        indexed = complex_index(rows(section.get(phase)), phase)
        if len(indexed) != expected or any(row.get("content_validated") is not True for row in indexed.values()):
            return None
        signatures[phase] = {json.dumps(key, ensure_ascii=False): {field: row.get(field) for field in fields}
                             for key, row in sorted(indexed.items())}
    return signatures


def complex_latency_table(before: list[dict], after: list[dict], identifier: str,
                          latency_key: str = "latency", codec: bool = False) -> str:
    left_index, right_index = complex_index(before, identifier + " BEFORE"), complex_index(after, identifier + " AFTER")
    keys = sorted(set(left_index) | set(right_index), key=lambda key: (key[0], key[2], key[3], key[4], str(key[5])))
    contents = []
    for key in keys:
        kind, size, budget, scale, name, window = key
        left, right = left_index.get(key, {}), right_index.get(key, {})
        ll, rl = obj(left, latency_key), obj(right, latency_key)
        rate = "codec_pairs_per_second" if codec else "rpc_per_second"
        mib_key = "encoded_body_mib_per_second" if codec else "encoded_body_one_direction_mib_per_second"
        status = match_state(left, right)
        if left and right:
            if any(left.get(field) != right.get(field) or not present(left.get(field)) or not present(right.get(field))
                   for field in ("request_encoded_body_bytes", "response_encoded_body_bytes", "content_checksum_fnv1a64", "content_summary", "top_level_items")):
                status += '<br><strong class="alert">fixture / 内容 / 实际 body 不一致或缺失</strong>'
            if left.get("content_validated") is not True or right.get("content_validated") is not True:
                status += '<br><strong class="alert">完整内容校验未确认</strong>'
            if any(left.get(field) != right.get(field) for field in ("minimum_completed", "minimum_sample_ms", "warmup_pairs" if codec else "warmup_rpcs")):
                status += '<br><strong class="warning">case 测量配置不同</strong>'
        contents.append([esc(kind), esc(size), payload(budget), count(scale),
            count(left.get("top_level_items")) + " / " + count(right.get("top_level_items")),
            count(left.get("request_encoded_body_bytes")) + " / " + count(right.get("request_encoded_body_bytes")),
            count(left.get("response_encoded_body_bytes")) + " / " + count(right.get("response_encoded_body_bytes")), status,
            ms(ll.get("p50_us")), ms(rl.get("p50_us")), ratio(ll.get("p50_us"), rl.get("p50_us")),
            ms(ll.get("p99_us")), ms(rl.get("p99_us")), ratio(ll.get("p99_us"), rl.get("p99_us")),
            count(ll.get("samples")), count(rl.get("samples")), fmt(left.get(rate)), fmt(right.get(rate)), ratio(right.get(rate), left.get(rate)),
            "pairs/s" if codec else "RPC/s", fmt(left.get(mib_key)), fmt(right.get(mib_key)),
            count(left.get("completed")) + " / " + count(right.get("completed")), small_sample(ll.get("samples"), rl.get("samples"))])
    return table(["类型", "档次", "请求 body 预算", "Scale", "顶层项目 B / A", "请求 body B / A", "响应 body B / A", "匹配",
                  "B p50 ms", "A p50 ms", "p50 B/A", "B p99 ms", "A p99 ms", "p99 B/A", "B 延迟 N", "A 延迟 N",
                  "B 吞吐", "A 吞吐", "吞吐 A/B", "单位", "B body MiB/s", "A body MiB/s", "完成数 B / A", "样本提示"], contents, identifier)


def complex_section(before: dict, after: dict) -> str:
    if "complex_types" not in before["result"] and "complex_types" not in after["result"]:
        return ""
    left, right = obj(before["result"], "complex_types"), obj(after["result"], "complex_types")
    lc, rc = obj(left, "configuration"), obj(right, "configuration")
    parts = ['<section><h2>复杂类型：配置与完整 fixture 核对</h2>',
             table(["配置", "BEFORE", "AFTER"], [[esc(key), plain(lc.get(key)), plain(rc.get(key))] for key in sorted(set(lc) | set(rc))], "complex-configuration"),
             '<p>匹配 key 包含类型、规模档次、请求 body 预算、实际 fixture scale、case 名和 window。全局条件额外核对完整 case 集、实际请求/响应 body、顶层项目数、递归内容计数与 canonical 内容摘要。'
             '请求 body 包含生成 Params 的 header、指针、容器头和 padding；排除外层 Mojo message/native framing，不是完整 wire 流量。'
             '实际字节可能低于目标预算，MiB/s 使用各自实际 body bytes，typed 计请求单向，codec 每 pair 计一次。</p>',
             '<h2>复杂 typed：双进程串行 RTT</h2>',
             complex_latency_table(rows(left.get("typed_serial")), rows(right.get("typed_serial")), "complex-typed"),
             '<h2>复杂 codec：编码</h2>',
             complex_latency_table(rows(left.get("codec_only")), rows(right.get("codec_only")), "complex-codec-encode", "encode_latency", True),
             '<h2>复杂 codec：解码</h2>',
             complex_latency_table(rows(left.get("codec_only")), rows(right.get("codec_only")), "complex-codec-decode", "decode_latency", True),
             '<p>Typed 包含生成的协议代码、codec、native 传输、调度与回调，计时不包含代码生成。RTT 排除输入 clone、回调完整验证和输出销毁；这些仍在吞吐墙钟内。'
             '完整值逐字段比较并检查 enum、union、None/Some 与浮点 IEEE bits；逻辑摘要按 map 键排序，不要求 HashMap 编码顺序固定。'
             'Codec encode/decode 分别计时，其分位数之和不是 pair 或 RTT p50。两张 codec 表共享同一 pairs/s，包含 clone、校验、drop 与统计开销，不是独立阶段吞吐。'
             'N 可能来自 Algorithm R reservoir（非前缀截断）；任一边 N &lt;100 标探索性。倍率不证明统计显著性；独立 ABBA 重复应保留每轮结果与顺序，不能合并各轮 p50 冒充 pooled percentile。</p></section>']
    return "\n".join(parts)


def checkpoint_summary(run: dict, field: str, *, mach: bool = False) -> str:
    checkpoints = rows(run["resources"].get("checkpoints"))
    values = [row.get(field) if mach else obj(row, "parent").get(field) for row in checkpoints]
    available = [value for value in values if numeric(value) is not None]
    if not available:
        return "未采集"
    formatter = mib if field == "rss_bytes" else count
    return f'{formatter(available[0])} → {formatter(available[-1])}；范围 {formatter(min(available))}–{formatter(max(available))}；有值 {len(available)}/{len(checkpoints)} 点'


def resource_summary(run: dict) -> dict:
    result, resources = run["result"], run["resources"]
    stress, churn, crash = (obj(result, name) for name in ("stress", "churn", "crash"))
    samples = rows(resources.get("rss_samples"))
    parent_rss = [obj(sample, "parent").get("rss_bytes") for sample in samples]
    parent_rss = [value for value in parent_rss if numeric(value) is not None]
    checkpoints = rows(resources.get("checkpoints"))
    exits = Counter(str(worker.get("exit_code", "未记录")) for worker in rows(resources.get("workers")))
    return {
        "result / resources / metadata ok": esc(" / ".join(str(run[name].get("ok", "未记录")) for name in ("result", "resources", "metadata"))),
        "validation_errors / 边界 echoes": count(result.get("validation_errors")) + " / " + count(result.get("correctness_boundary_echoes")),
        "stress elapsed / completed / sent": fmt(stress.get("elapsed_seconds")) + " s / " + count(stress.get("completed")) + " / " + count(stress.get("sent")),
        "stress final_pending": count(stress.get("final_pending")),
        "churn passed / cycles": esc(churn.get("passed", "未记录")) + " / " + count(churn.get("cycles")),
        "crash passed / expected_exit": esc(crash.get("passed", "未记录")) + " / " + count(crash.get("expected_exit_code")),
        "crash held / released / success / pending": " / ".join(count(crash.get(key)) for key in ("held_requests", "released_callbacks", "success_callbacks", "final_pending")),
        "记录的 worker 退出码分布": esc(json.dumps(dict(exits), ensure_ascii=False, sort_keys=True)),
        "周期样本数 / 父 RSS 采样最大值 MiB": count(len(samples)) + " / " + mib(max(parent_rss) if parent_rss else None),
        "检查点首阶段 → 末阶段": esc(str(checkpoints[0].get("stage")) + " → " + str(checkpoints[-1].get("stage"))) if checkpoints else "未采集",
        "检查点父 RSS MiB（首 → 末 / 范围）": checkpoint_summary(run, "rss_bytes"),
        "检查点 FD（首 → 末 / 范围）": checkpoint_summary(run, "fd_count"),
        "检查点线程（首 → 末 / 范围）": checkpoint_summary(run, "thread_count"),
        "检查点 Windows handles（首 → 末 / 范围）": checkpoint_summary(run, "handle_count"),
        "检查点 Mach port 名字数（首 → 末 / 范围）": checkpoint_summary(run, "mach_port_name_count", mach=True),
        "Mach 探测错误数": count(sum(bool(row.get("mach_port_probe_error")) for row in checkpoints))
            if any("mach_port_name_count" in row or "mach_port_probe_error" in row for row in checkpoints) else "未采集",
        "清理动作数": count(len(rows(resources.get("cleanup")))),
    }


def relative_link(path: Path, output: Path, label: str) -> str:
    try:
        relative = os.path.relpath(path, output.parent).replace(os.sep, "/")
    except ValueError as error:
        raise ValueError("cannot make relative links across drives; put --output on the same drive as both runs") from error
    return f'<a href="{esc(quote(relative, safe="/."))}">{esc(label)}</a>'


def render(before: dict, after: dict, output: Path) -> str:
    conditions = checks(before, after)
    consistent = all(condition[3] for condition in conditions)
    before_ok = all(before[name].get("ok") is True for name in ("result", "resources", "metadata"))
    after_ok = all(after[name].get("ok") is True for name in ("result", "resources", "metadata"))
    banner = ('主要记录条件一致；倍率仍仅代表两次单轮观测' if consistent and before_ok and after_ok
              else '存在条件差异、缺失或未通过的运行；不能把倍率直接归因于优化')
    parts = [
        '<header><p class="eyebrow">MOJO RUST SDK · BEFORE / AFTER</p><h1>优化前后对比</h1>',
        f'<p class="{"good" if consistent and before_ok and after_ok else "alert"}">{banner}</p>',
        '<p>B = BEFORE，A = AFTER。延迟速度比 B/A、吞吐速度比 A/B，均为大于 1 表示 AFTER 更快。'
        '没有有效分母或数据缺失时显示 —，不补 0。比值不是统计显著性结论，也未提供置信区间。</p></header>',
        '<section><h2>条件核对</h2>',
        table(["检查", "BEFORE", "AFTER", "状态"], [[esc(name), plain(left), plain(right),
              '<span class="good">一致</span>' if same else '<strong class="alert">差异 / 未记录</strong>']
              for name, left, right, same in conditions], "conditions"),
    ]
    config_before, config_after = obj(before["result"], "configuration"), obj(after["result"], "configuration")
    parts += [table(["配置项", "BEFORE", "AFTER"], [[esc(key), plain(config_before.get(key)), plain(config_after.get(key))]
                    for key in sorted(set(config_before) | set(config_after))], "configuration"),
              '<p>编译器、target、native SHA256 和完整 configuration 是必要核对项；同时展示 OS、硬件、编译配置和监测差异。'
              '即使这些记录一致，系统负载、温度、CPU 调频等未控制因素仍可影响结果。'
              '源码 HEAD 与测试二进制允许因优化改变，见文末记录；HEAD 本身不证明工作区无未提交改动。</p></section>']
    perf_before, perf_after = rows(before["result"].get("performance")), rows(after["result"].get("performance"))
    serial_before = [row for row in perf_before if row.get("name") == "echo_serial"]
    serial_after = [row for row in perf_after if row.get("name") == "echo_serial"]
    parts += ['<section><h2>类型化串行：各 payload</h2>', latency_table(serial_before, serial_after, "typed-serial"),
              '<p>完整 Rust SDK 双进程路径，包含 Mojom codec、router、Mojo、dispatch 与回调。'
              '延迟从方法调用到回调入口，不是裸 Core 指标。</p></section>',
              '<section><h2>Raw 串行对照：各 payload</h2>',
              latency_table(rows(before["result"].get("raw_transport")), rows(after["result"].get("raw_transport")), "raw-serial"),
              '<p>Raw 仍包含 Rust 安全消息 API、MessagePipeWatcher、原生 IO/dispatch、拷贝和 channel，'
              '<strong>不是裸 Mojo Core，也不是零拷贝上限</strong>。其计时边界与 typed 不完全相同，不做简单扣除归因。</p></section>']
    codec_before, codec_after = rows(before["result"].get("codec_only")), rows(after["result"].get("codec_only"))
    parts += ['<section><h2>纯 codec：编码</h2>', latency_table(codec_before, codec_after, "codec-encode", "encode_latency", True),
              '<h2>纯 codec：解码</h2>', latency_table(codec_before, codec_after, "codec-decode", "decode_latency", True),
              '<p>Encode/decode 分别计时。<strong>编码与解码的分位数之和不是总 p50、pair p50 或 IPC RTT 分位数</strong>。'
              '两张表的吞吐均来自同一个完整 pair 循环，单位 pairs/s，包含输入 clone 和输出校验；不是单独编码/解码的吞吐。'
              'N 为分位数储存样本数，可能采用 reservoir；不等同于全部完成 observations。</p></section>']
    other_before = [row for row in perf_before if row.get("name") != "echo_serial"]
    other_after = [row for row in perf_after if row.get("name") != "echo_serial"]
    parts += ['<section><h2>Pipeline 与 oneway：按相同 case / payload / window 匹配</h2>',
              latency_table(other_before, other_after, "pipeline-oneway"),
              '<p>仅在一边出现的窗口或 case 会保留并标明，不拿不同窗口强行对照。'
              'Oneway 使用批次屏障确认完成，不记录逐条 RTT，其延迟列为未采集；探索性提示按完成消息数计算。'
              '所有 payload 速率均是应用数据口径，MiB = 1024² 字节，不等于 wire 流量。</p></section>']
    primitive = primitive_section(before, after)
    if primitive:
        parts.append(primitive)
    nullable = nullable_section(before, after)
    if nullable:
        parts.append(nullable)
    complex_types = complex_section(before, after)
    if complex_types:
        parts.append(complex_types)
    left, right = resource_summary(before), resource_summary(after)
    parts += ['<section><h2>稳定性与资源：本轮实际记录</h2>',
              table(["记录", "BEFORE", "AFTER"], [[esc(key), left[key], right[key]] for key in left], "stability-resources"),
              '<p>资源的首末阶段和有效采样点数应一起看，不能把不同阶段的 RSS 直接相减解释成泄漏。'
              '<strong>周期采样最大值不是真实峰值</strong>；缺失或 null 表示未采集，不代表 0。'
              'RSS 受分配器缓存和页驻留影响，不使用固定 RSS 阈值判断泄漏。macOS 的 FD 不包括 Mach ports；'
              'Mach 名字数统计 task IPC namespace 名字，不是每名字的引用数，稳定也不保证无泄漏。'
              'stress/churn/crash 通过只代表记录的有限时长与次数，不是长期稳定性保证。</p></section>',
              '<section><h2>样本量与解释限制</h2><p><strong class="warning">大包任一边 N &lt; 100 时，结果为探索性。</strong>'
              '尤其 p99 常接近少量样本的最大值，不应作为稳定尾延迟或 SLA；小样本速度比也会显著波动。'
              '请求次数预算、预热和实际完成数见原始报告。本文直接读取记录值，不合并样本、不重算分位数，也不把相关性当作因果证明。</p></section>']
    provenance = []
    for label, run in (("BEFORE", before), ("AFTER", after)):
        directory = run["directory"]
        links = " · ".join(relative_link(directory / name, output, name) for name in
                           ("report.html", "result.json", "resources.json", "metadata.json", "raw.log"))
        provenance.append([label, relative_link(directory, output, str(directory)) + "<br>" + links,
                           plain(run["metadata"].get("started_utc")), plain(run["metadata"].get("repository_commit")),
                           plain(run["metadata"].get("binary_sha256"))])
    parts += ['<footer><h2>原始目录与可追溯记录</h2>',
              table(["运行", "目录 / 原始记录（相对链接）", "开始时间", "仓库 HEAD", "测试二进制 SHA256"], provenance, "provenance"),
              '<p>离线 HTML，无外部 CSS、JavaScript 或网络请求。生成对比只写指定 HTML，不修改任一运行的 JSON 或日志。</p></footer>']
    css = """
    :root{color-scheme:light}*{box-sizing:border-box}body{margin:0;background:#f2f5f8;color:#172536;font:15px/1.65 system-ui,-apple-system,'Segoe UI',sans-serif}
    main{max-width:1540px;margin:auto;padding:28px 20px}header,section,footer{background:white;border:1px solid #d9e1e9;border-radius:12px;padding:22px;margin:0 0 20px}
    h1{font-size:30px;margin:6px 0 14px}h2{font-size:20px;margin:6px 0 14px}.eyebrow{font-size:12px;letter-spacing:.12em;color:#075985;font-weight:700}.good{color:#166047;font-weight:700}
    .alert{color:#9a3412;font-weight:700}.warning{color:#9a3412;background:#fff1da;padding:3px 5px;border-radius:4px;display:inline-block;font-size:12px}
    .scroll{overflow-x:auto;margin:12px 0}table{width:100%;border-collapse:collapse;font-size:12px;font-variant-numeric:tabular-nums}th,td{border-bottom:1px solid #d9e1e9;padding:10px;text-align:right;vertical-align:top;min-width:76px}th{background:#edf3f8;white-space:nowrap}th:first-child,td:first-child{text-align:left}tbody tr:nth-child(even){background:#fafcfe}td{overflow-wrap:anywhere}a{color:#075985}p{margin:12px 0}footer{color:#516278}
    @media(max-width:700px){main{padding:12px 8px}header,section,footer{padding:15px}h1{font-size:25px}}@media print{body{background:white}main{padding:0;max-width:none}.scroll{overflow:visible}table{font-size:8px}th,td{padding:4px}section{break-inside:avoid}}
    """
    return ('<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">'
            '<title>Mojo Rust SDK 优化前后对比</title><style>' + css + '</style></head><body><main>'
            + "\n".join(parts) + '</main></body></html>\n')


def compare(before_dir: Path, after_dir: Path, output: Path) -> Path:
    output = output.expanduser().resolve()
    if output.suffix.lower() not in (".html", ".htm"):
        raise ValueError("--output must be an HTML file; input JSON/log files are never overwritten")
    content = render(load(before_dir), load(after_dir), output)
    output.parent.mkdir(parents=True, exist_ok=True)
    handle, name = tempfile.mkstemp(prefix=".compare-", suffix=".html", dir=output.parent)
    temporary = Path(name)
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as stream:
            stream.write(content)
        temporary.replace(output)
    finally:
        temporary.unlink(missing_ok=True)
    return output


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before_dir", type=Path)
    parser.add_argument("after_dir", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    try:
        print(compare(arguments.before_dir, arguments.after_dir, arguments.output))
        return 0
    except (OSError, ValueError, TypeError) as error:
        parser.exit(1, f"comparison failed: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
