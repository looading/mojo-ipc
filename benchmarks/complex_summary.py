#!/usr/bin/env python3
"""Summarize four separate complex benchmark runs in B/A/A/B order (stdlib only)."""
from __future__ import annotations

import argparse
import html
import json
import math
import os
from pathlib import Path
import statistics

import compare

RUNS = (("0-before", "before"), ("1-after", "after"), ("2-after", "after"), ("3-before", "before"))


def number(value):
    return value if isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) else None


def median(values):
    clean = [value for value in values if number(value) is not None]
    return statistics.median(clean) if clean else None


def divide(a, b):
    return a / b if number(a) is not None and number(b) is not None and b > 0 else None


def spread(values):
    clean = [value for value in values if number(value) is not None]
    return (max(clean) / min(clean) - 1) * 100 if len(clean) == 2 and min(clean) > 0 else None


def esc(value):
    return html.escape(str(value), quote=True)


def fmt(value, digits=3):
    return f"{value:,.{digits}f}" if number(value) is not None else "—"


def table(headers, records, identifier):
    return (f'<div class="scroll"><table id="{esc(identifier)}"><thead><tr>'
            + "".join(f"<th>{esc(header)}</th>" for header in headers) + "</tr></thead><tbody>"
            + "".join("<tr>" + "".join(f"<td>{cell}</td>" for cell in row) + "</tr>" for row in records)
            + "</tbody></table></div>")


def summarize(root: Path) -> dict:
    loaded = {name: compare.load(root / name) for name, _ in RUNS}
    reference = loaded[RUNS[0][0]]
    checks = []
    for name, _ in RUNS:
        run = loaded[name]
        ok = all(run[key].get("ok") is True for key in ("result", "resources", "metadata"))
        checks.append({"run": name, "check": "result/resources/metadata ok", "passed": ok})
        for label, before, after, passed in compare.checks(reference, run):
            checks.append({"run": name, "check": label, "passed": passed, "reference": before, "actual": after})
    indexed = {}
    keys = set()
    for name, _ in RUNS:
        section = compare.obj(loaded[name]["result"], "complex_types")
        for phase in ("codec_only", "typed_serial"):
            index = compare.complex_index(compare.rows(section.get(phase)), name + "/" + phase)
            indexed[name, phase] = index
            keys.update((phase, key) for key in index)
    complex_config = compare.obj(compare.obj(reference["result"], "complex_types"), "configuration")
    expected = len(complex_config.get("cases", [])) * len(complex_config.get("target_encoded_body_budgets_bytes", []))
    matrices = {name: {phase: len(indexed[name, phase]) for phase in ("codec_only", "typed_serial")} for name, _ in RUNS}
    checks.append({"run": "all", "check": "all configured cases in codec and typed", "passed": bool(expected) and all(count == expected for counts in matrices.values() for count in counts.values()), "actual": matrices, "expected_per_phase": expected})
    records = []
    for phase, key in sorted(keys, key=lambda item: (item[0], item[1][0], item[1][2], item[1][3], item[1][4], str(item[1][5]))):
        for metric in (("encode_latency", "decode_latency") if phase == "codec_only" else ("latency",)):
            observations = []
            for name, label in RUNS:
                row = indexed[name, phase].get(key, {})
                latency = compare.obj(row, metric)
                observations.append({"run": name, "label": label, "p50_us": latency.get("p50_us"), "p99_us": latency.get("p99_us"),
                    "samples": latency.get("samples"), "observations": latency.get("observations", row.get("completed")),
                    "completed": row.get("completed"), "elapsed_seconds": row.get("elapsed_seconds"),
                    "rate": row.get("codec_pairs_per_second" if phase == "codec_only" else "rpc_per_second"),
                    "request_body_bytes": row.get("request_encoded_body_bytes"), "response_body_bytes": row.get("response_encoded_body_bytes"),
                    "checksum": row.get("content_checksum_fnv1a64"), "content_validated": row.get("content_validated")})
            by_label = {}
            for label in ("before", "after"):
                values = [row for row in observations if row["label"] == label]
                p50 = [row["p50_us"] for row in values]
                by_label[label] = {"p50_us_median_of_two_runs": median(p50), "p99_us_median_of_two_runs": median([row["p99_us"] for row in values]),
                    "rate_median_of_two_runs": median([row["rate"] for row in values]), "p50_us_each_run": p50,
                    "p50_spread_percent": spread(p50), "sample_counts_each_run": [row["samples"] for row in values]}
            before, after = by_label["before"], by_label["after"]
            complete = all(number(row["p50_us"]) is not None and number(row["p99_us"]) is not None and row["content_validated"] is True for row in observations)
            same_fixture = len({(row["request_body_bytes"], row["response_body_bytes"], row["checksum"]) for row in observations}) == 1 and all(row["checksum"] for row in observations)
            p50_ratio = divide(before["p50_us_median_of_two_runs"], after["p50_us_median_of_two_runs"])
            change = divide(after["p50_us_median_of_two_runs"], before["p50_us_median_of_two_runs"])
            records.append({"phase": phase, "metric": metric, "complex_case": key[0], "size_label": key[1], "target_encoded_body_budget_bytes": key[2],
                "scale": key[3], "name": key[4], "window": key[5], "complete": complete, "same_fixture": bool(same_fixture),
                "before": before, "after": after, "p50_speed_ratio_before_over_after": p50_ratio,
                "p50_change_percent_after_minus_before": (change - 1) * 100 if change is not None else None,
                "p99_speed_ratio_before_over_after": divide(before["p99_us_median_of_two_runs"], after["p99_us_median_of_two_runs"]),
                "rate_ratio_after_over_before": divide(after["rate_median_of_two_runs"], before["rate_median_of_two_runs"]),
                "small_sample_tail_caution": any(number(row["samples"]) is not None and row["samples"] < 100 for row in observations), "runs": observations})
    checks.append({"run": "all", "check": "all latency observations and fixtures present/equal", "passed": bool(records) and all(row["complete"] and row["same_fixture"] for row in records)})
    regressions = [row for row in records if number(row["p50_change_percent_after_minus_before"]) is not None and row["p50_change_percent_after_minus_before"] > 0]
    regressions.sort(key=lambda row: row["p50_change_percent_after_minus_before"], reverse=True)
    metric_counts = {}
    for phase, metric in (("typed_serial", "latency"), ("codec_only", "encode_latency"), ("codec_only", "decode_latency")):
        selected = [row for row in records if row["phase"] == phase and row["metric"] == metric]
        values = [row["p50_change_percent_after_minus_before"] for row in selected]
        metric_counts[phase + "/" + metric] = {"total": len(selected), "after_p50_lower": sum(number(value) is not None and value < 0 for value in values),
            "after_p50_higher": sum(number(value) is not None and value > 0 for value in values), "equal": sum(value == 0 for value in values),
            "missing": sum(number(value) is None for value in values)}
    return {"schema_version": 1, "kind": "complex B/A/A/B observational summary", "ok": all(row["passed"] for row in checks),
        "aggregation": "For each label, median of exactly two independent run-level p50/p99 values. Not a pooled percentile, confidence interval or significance test.",
        "spread": "100*(max/min-1) within the two run-level p50 values for each label; no outlier removal.",
        "run_order": [{"directory": name, "label": label, "started_utc": loaded[name]["metadata"].get("started_utc"),
                       "binary_sha256": loaded[name]["metadata"].get("binary_sha256")} for name, label in RUNS],
        "matrices": matrices, "metric_counts": metric_counts, "checks": checks, "record_count": len(records), "regression_count": len(regressions),
        "regressions": [{key: row[key] for key in ("phase", "metric", "complex_case", "size_label", "p50_change_percent_after_minus_before", "p50_speed_ratio_before_over_after")} for row in regressions], "records": records}


def render(summary: dict, root: Path, output: Path) -> str:
    def link(path, label):
        return '<a href="' + esc(Path(os.path.relpath(path, output.parent)).as_posix()) + '">' + esc(label) + '</a>'
    parts = ['<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>复杂类型 ABBA 汇总</title>',
             '<style>body{font:15px/1.6 system-ui,sans-serif;margin:36px;color:#202734;background:#f5f7fa}section,header{background:white;padding:24px;margin:20px 0;border-radius:12px}.scroll{overflow:auto}table{border-collapse:collapse;white-space:nowrap;font-variant-numeric:tabular-nums;font-size:13px}td,th{border:1px solid #dce2ea;padding:7px 10px;text-align:left}th{background:#eaf0f8}.warning{color:#9a350b;font-weight:700}code{font-size:12px}a{color:#17489b}</style><header><h1>复杂类型：B/A/A/B 独立重复汇总</h1>',
             '<p>' + ('已记录条件与 fixture 一致，四轮均通过。' if summary["ok"] else '<strong class="warning">存在条件、fixture、样本或运行状态问题；请先核查。</strong>') + '</p>',
             '<p>每个 label 两轮各自的 p50/p99 取中位数（两值时即算术中点），<strong>不是合并原始样本后的 pooled percentile，不是置信区间或显著性结论</strong>。'
             'B/A 延迟速度比、A/B 吞吐速度比大于 1 表示 AFTER 更快。两轮 spread = 100×(max/min−1)，保留全部观测，不剔除回退。</p></header>']
    links = []
    for name, label in RUNS:
        links.append([esc(name), esc(label)] + [link(root / name / file, file) for file in ("report.html", "result.json", "resources.json", "metadata.json", "raw.log")])
    parts += ['<section><h2>四轮顺序与原始记录</h2>', table(["目录 / 执行顺序", "Label", "报告", "结果", "资源", "环境", "日志"], links, "runs"), link(output.with_suffix('.json'), 'summary JSON'), '</section>',
              '<section><h2>完整条件检查</h2>', table(["运行", "检查", "状态", "参考", "实际"], [[esc(row["run"]), esc(row["check"]), '一致 / 通过' if row["passed"] else '<strong class="warning">差异 / 缺失 / 未通过</strong>',
                esc(json.dumps(row.get("reference", row.get("expected_per_phase")), ensure_ascii=False, sort_keys=True)), esc(json.dumps(row.get("actual"), ensure_ascii=False, sort_keys=True))] for row in summary["checks"]], "checks"), '</section>']
    def metric_rows(rows):
        result=[]
        for row in rows:
            b,a=row["before"],row["after"]
            status=('已匹配' if row["complete"] and row["same_fixture"] else '<strong class="warning">缺失 / fixture 差异</strong>')
            if row["small_sample_tail_caution"]: status+='<br><strong class="warning">探索性 N&lt;100</strong>'
            result.append([esc(row["complex_case"]),esc(row["size_label"]),esc(row["metric"]),str(row["scale"]),str(row["target_encoded_body_budget_bytes"]),
                ' / '.join(str(item["request_body_bytes"]) for item in row["runs"] if item["label"]=="before"),
                ' / '.join(str(item["request_body_bytes"]) for item in row["runs"] if item["label"]=="after"),
                ' / '.join(str(item["response_body_bytes"]) for item in row["runs"] if item["label"]=="before"),
                ' / '.join(str(item["response_body_bytes"]) for item in row["runs"] if item["label"]=="after"),
                ' / '.join(fmt(v) for v in b["p50_us_each_run"]),' / '.join(fmt(v) for v in a["p50_us_each_run"]),
                fmt(b["p50_us_median_of_two_runs"]),fmt(a["p50_us_median_of_two_runs"]),fmt(row["p50_speed_ratio_before_over_after"],4),fmt(row["p50_change_percent_after_minus_before"]),
                fmt(b["p50_spread_percent"]),fmt(a["p50_spread_percent"]),fmt(b["p99_us_median_of_two_runs"]),fmt(a["p99_us_median_of_two_runs"]),
                esc(' / '.join(str(n) for n in b["sample_counts_each_run"])),esc(' / '.join(str(n) for n in a["sample_counts_each_run"])),
                fmt(b["rate_median_of_two_runs"]),fmt(a["rate_median_of_two_runs"]),fmt(row["rate_ratio_after_over_before"],4),status])
        return result
    headers=["类型","档次","阶段","Scale","body 预算","B 请求 body 两轮 bytes","A 请求 body 两轮 bytes","B 响应 body 两轮 bytes","A 响应 body 两轮 bytes","B 两轮 p50 μs","A 两轮 p50 μs","B p50 中位 μs","A p50 中位 μs","p50 B/A","p50 A变化 %","B spread %","A spread %","B p99 中位 μs","A p99 中位 μs","B 两轮 N","A 两轮 N","B 吞吐中位","A 吞吐中位","吞吐 A/B","检查"]
    for phase,metric,title in (("typed_serial","latency","双进程 typed RTT"),("codec_only","encode_latency","进程内 codec encode"),("codec_only","decode_latency","进程内 codec decode")):
        selected=[row for row in summary["records"] if row["phase"]==phase and row["metric"]==metric]
        parts+=['<section><h2>'+title+'</h2><p>共 '+str(len(selected))+' 项；typed 吞吐单位 RPC/s，codec 是完整 pair loop 的 pairs/s，含 clone、验证与 drop，不是独立阶段速率。</p>',table(headers,metric_rows(selected),metric),'</section>']
    regressions=[row for row in summary["records"] if number(row["p50_change_percent_after_minus_before"]) is not None and row["p50_change_percent_after_minus_before"]>0]
    regressions.sort(key=lambda row:row["p50_change_percent_after_minus_before"],reverse=True)
    parts+=['<section><h2>全部 p50 更高项：'+str(len(regressions))+' / '+str(len(summary["records"]))+'</h2>',
            '<p>按 AFTER 相对 BEFORE 的 p50 增幅排序，包含所有正增幅，未用阈值隐藏小回退。观测回退与两轮 spread 同列展示，重复次序仍不足以证明因果、热降频或纯噪声。</p>',
            table(headers,metric_rows(regressions),'all-regressions'),'</section>',
            '<section><h2>解释边界</h2><p>请求/响应 Params body 不是完整 native wire；fixture、完整字段/bits/逻辑摘要和实际长度见各轮原始结果。'
            'Typed 包含生成的协议代码、codec、native 传输、调度与回调；不含代码生成。Codec 分位数之和不是 pair 或 RTT 分位数。'
            '保留全部四轮与回退；未宣称绝对最优、统计显著性、长期稳定性或其他平台同等表现。</p></section></html>']
    return '\n'.join(parts)


def main() -> int:
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('abba_dir',type=Path,help='contains 0-before,1-after,2-after,3-before')
    parser.add_argument('--output',type=Path,help='HTML path (default ABBA_DIR/summary.html); JSON uses same stem')
    args=parser.parse_args()
    root=args.abba_dir.expanduser().resolve();output=(args.output or root/'summary.html').expanduser().resolve()
    try:
        summary=summarize(root);output.parent.mkdir(parents=True,exist_ok=True)
        output.with_suffix('.json').write_text(json.dumps(summary,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
        output.write_text(render(summary,root,output),encoding='utf-8')
    except (OSError,ValueError,KeyError,TypeError) as error:
        parser.exit(1,f'complex summary failed: {error}\n')
    print(output)
    return 0 if summary['ok'] else 2


if __name__=='__main__':
    raise SystemExit(main())
