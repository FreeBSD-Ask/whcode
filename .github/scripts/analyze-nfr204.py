#!/usr/bin/env python3
"""NFR-204 组合判据分析（ADR-025 修订记录，2026-10-26 用户批准）。

输入：长跑夹具产出的 timeseries.jsonl（sample 行 + summary 行）。
输出：verdict JSON（随 artifact 归档）+ 公开注解摘要；退出码：
  0 = 通过，或前置全满足但统计区间跨越 0.5（无权判定，须登记）；
  1 = 违反，或前置不满足（夹具纪律缺口），或输入缺失/不可解析。

判据（ADR-025 修订记录，冻结值 0.5 MB/h 不变）：
- 前置：release 构建、样本 ≥ 60、单 PID、event_missing 全程 0、线程/句柄
  恒定（允许预热期一次性下降）；
- 五要素：预热后（丢弃前 10%）OLS 斜率及 95% CI、按清理周期相位对齐的
  跨周期斜率及 95% CI、首末斜率（交叉核对）、线程/句柄序列、噪声带宽
  （σ、bootstrap 95% CI、最小可辨斜率 1.645×SE）；
- 三态：通过 = 各斜率 95% 上界 ≤ 0.5；违反 = 各斜率 95% 下界 > 0.5；
  其余为无权判定。估计量之间结论冲突（含与首末斜率）一律无权判定，
  不得择优引用。
"""

from __future__ import annotations

import json
import math
import os
import random
import sys

BUDGET = 0.5
BOOTSTRAP_REPS = 10_000
BOOTSTRAP_SEED = 20261026
MIN_SAMPLES = 60

# t 分布 97.5% 分位表（双侧 95%），df 之间线性插值，df≥120 用正态近似。
T_TABLE = (
    (5, 2.571), (6, 2.447), (7, 2.365), (8, 2.306), (9, 2.262), (10, 2.228),
    (12, 2.179), (14, 2.145), (16, 2.120), (18, 2.101), (20, 2.086),
    (25, 2.060), (30, 2.042), (40, 2.021), (60, 2.000), (120, 1.980),
)
T_INF = 1.959964


def t_crit_975(df: int) -> float:
    if df <= 0:
        raise ValueError(f"df={df}")
    if df >= 120:
        return T_INF
    for (d0, t0), (d1, t1) in zip(T_TABLE, T_TABLE[1:]):
        if d0 <= df <= d1:
            return t0 + (t1 - t0) * (df - d0) / (d1 - d0)
    raise ValueError(f"df={df}")


def ols_slope(points: list[tuple[float, float]]) -> tuple[float, float]:
    """返回 (斜率, SE)。"""
    n = len(points)
    mt = sum(p[0] for p in points) / n
    my = sum(p[1] for p in points) / n
    sxx = sum((t - mt) ** 2 for t, _ in points)
    sxy = sum((t - mt) * (y - my) for t, y in points)
    slope = sxy / sxx
    sse = sum((y - (slope * (t - mt) + my)) ** 2 for t, y in points)
    se = math.sqrt(sse / (n - 2) / sxx)
    return slope, se


def bootstrap_ci(points: list[tuple[float, float]]) -> list[float]:
    rng = random.Random(BOOTSTRAP_SEED)
    n = len(points)
    slopes = []
    for _ in range(BOOTSTRAP_REPS):
        sample = [points[rng.randrange(n)] for _ in range(n)]
        mt = sum(p[0] for p in sample) / n
        my = sum(p[1] for p in sample) / n
        sxx = sum((t - mt) ** 2 for t, _ in sample)
        if sxx == 0:
            continue
        sxy = sum((t - mt) * (y - my) for t, y in sample)
        slopes.append(sxy / sxx)
    slopes.sort()

    def pct(p: float) -> float:
        idx = min(int(p * len(slopes)), len(slopes) - 1)
        return slopes[idx]

    return [pct(0.025), pct(0.975)]


def constant_or_single_warmup_drop(values: list[int]) -> tuple[bool, str]:
    """全程恒定，或允许一次非增阶梯（预热期下降）。"""
    uniq = []
    for v in values:
        if not uniq or uniq[-1] != v:
            uniq.append(v)
    if len(uniq) == 1:
        return True, f"恒定 {uniq[0]}"
    if len(uniq) == 2 and uniq[0] > uniq[1]:
        return True, f"预热期一次性下降 {uniq[0]}→{uniq[1]}，其后恒定"
    return False, f"非恒定：变化序列 {uniq}"


def cycle_lag(deleted: list[int]) -> int | None:
    """由 deleted_total 的跳变推断清理周期（样本数）；不足两个跳变返回 None。"""
    jumps = [i for i in range(1, len(deleted)) if deleted[i] > deleted[i - 1]]
    if len(jumps) < 2:
        return None
    diffs = [b - a for a, b in zip(jumps, jumps[1:])]
    diffs.sort()
    return diffs[len(diffs) // 2]


def aligned_slope(samples: list[dict], lag: int, warmup: int) -> dict | None:
    """跨周期同相位配对：(i, i+lag)，i 在预热后；每对斜率取均值与 t-CI。"""
    pairs = []
    for i in range(warmup, len(samples) - lag):
        t0, y0 = samples[i]["elapsed_hours"], samples[i]["rss_mb"]
        t1, y1 = samples[i + lag]["elapsed_hours"], samples[i + lag]["rss_mb"]
        dt = t1 - t0
        if dt > 0:
            pairs.append((y1 - y0) / dt)
    if len(pairs) < 5:
        return None
    m = len(pairs)
    mean = sum(pairs) / m
    var = sum((p - mean) ** 2 for p in pairs) / (m - 1)
    se = math.sqrt(var / m)
    t = t_crit_975(m - 1)
    return {
        "pairs": m,
        "cycle_lag_samples": lag,
        "slope_mb_per_h": round(mean, 4),
        "ci95": [round(mean - t * se, 4), round(mean + t * se, 4)],
    }


def classify(ci: list[float]) -> str:
    lower, upper = ci
    if upper <= BUDGET:
        return "通过"
    if lower > BUDGET:
        return "违反"
    return "无权判定"


def main() -> int:
    if len(sys.argv) != 2:
        print("用法：analyze-nfr204.py <timeseries.jsonl>", file=sys.stderr)
        return 2
    path = sys.argv[1]
    samples: list[dict] = []
    summary: dict = {}
    try:
        with open(path, encoding="utf-8") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                row = json.loads(line)
                if row.get("kind") == "sample":
                    samples.append(row)
                elif row.get("kind") == "summary":
                    summary = row
    except OSError as err:
        print(f"::error title=NFR-204::无法读取 {path}: {err}")
        return 2
    if not samples or not summary:
        print("::error title=NFR-204::缺少 sample 或 summary 行（零不等于通过）")
        return 2

    n = len(samples)
    warmup = math.ceil(0.1 * n)
    subset = [
        (s["elapsed_hours"], s["rss_mb"])
        for s in samples[warmup:]
    ]
    slope, se = ols_slope(subset)
    tcrit = t_crit_975(len(subset) - 2)
    ols_ci = [slope - tcrit * se, slope + tcrit * se]
    boot_ci = bootstrap_ci(subset)

    pids = {s.get("pid") for s in samples}
    single_pid = len(pids) == 1 and None not in pids
    missing_ok = all(s.get("event_missing") == 0 for s in samples)
    threads_ok, threads_note = constant_or_single_warmup_drop(
        [s.get("threads", -1) for s in samples]
    )
    handles_ok, handles_note = constant_or_single_warmup_drop(
        [s.get("handles", -1) for s in samples]
    )
    build_profile = summary.get("build_profile")
    release_ok = build_profile == "release"

    lag = cycle_lag([s.get("deleted_total", 0) for s in samples])
    aligned = aligned_slope(samples, lag, warmup) if lag else None

    rss_values = [s["rss_mb"] for s in samples]
    mean_rss = sum(rss_values) / n
    sigma = math.sqrt(
        sum((v - mean_rss) ** 2 for v in rss_values) / (n - 1)
    )
    minimal_detectable = round(1.645 * se, 4)

    b_fl = summary.get("rss_slope_mb_per_h")
    if not isinstance(b_fl, (int, float)) or isinstance(b_fl, bool):
        t0, y0 = samples[0]["elapsed_hours"], samples[0]["rss_mb"]
        t1, y1 = samples[-1]["elapsed_hours"], samples[-1]["rss_mb"]
        b_fl = (y1 - y0) / (t1 - t0) if t1 > t0 else None

    prereq = {
        "release_build": release_ok,
        "samples_ge_60": n >= MIN_SAMPLES,
        "sample_count": n,
        "single_pid": single_pid,
        "pids": sorted(str(p) for p in pids),
        "event_missing_zero": missing_ok,
        "threads_constant": threads_ok,
        "handles_constant": handles_ok,
    }
    prereq_met = all(
        v for k, v in prereq.items() if isinstance(v, bool)
    )

    estimates: list[dict] = [
        {
            "name": "warmup_ols",
            "slope": round(slope, 4),
            "se": round(se, 4),
            "ci95_t": [round(v, 4) for v in ols_ci],
            "ci95_bootstrap": [round(v, 4) for v in boot_ci],
            "verdict": classify([round(v, 4) for v in ols_ci]),
        }
    ]
    if aligned:
        estimates.append({**aligned, "name": "phase_aligned", "verdict": classify(aligned["ci95"])})

    verdicts = {e["verdict"] for e in estimates}
    b_class = "无权判定"
    if isinstance(b_fl, (int, float)) and not isinstance(b_fl, bool):
        b_class = "通过" if b_fl <= BUDGET else ("违反" if b_fl > 1.0 else "无权判定")
    conflict = ("通过" in verdicts and b_class == "违反") or (
        "违反" in verdicts and b_class == "通过"
    ) or ("通过" in verdicts and "违反" in verdicts)

    if not prereq_met:
        verdict = "无权判定（前置不满足）"
        exit_code = 1
    elif conflict or verdicts == {"通过", "违反"}:
        verdict = "无权判定（估计量结论冲突，不得择优引用）"
        exit_code = 0
    elif verdicts == {"通过"} and b_class != "违反":
        verdict = "通过"
        exit_code = 0
    elif verdicts == {"违反"} and b_class != "通过":
        verdict = "违反"
        exit_code = 1
    else:
        verdict = "无权判定（区间跨越 0.5，须登记并升级 24 h 深检）"
        exit_code = 0

    result = {
        "verdict": verdict,
        "exit_code": exit_code,
        "budget_mb_per_h": BUDGET,
        "prerequisites": prereq,
        "threads_note": threads_note,
        "handles_note": handles_note,
        "first_last_slope_mb_per_h": b_fl,
        "first_last_class": b_class,
        "estimates": estimates,
        "noise": {
            "sigma_mb": round(sigma, 4),
            "minimal_detectable_slope_mb_per_h": minimal_detectable,
            "warmup_discarded": warmup,
            "bootstrap_reps": BOOTSTRAP_REPS,
            "bootstrap_seed": BOOTSTRAP_SEED,
        },
        "summary_fields": {
            "build_profile": build_profile,
            "duckdb_version": summary.get("duckdb_version"),
            "memory_limit_setting": summary.get("memory_limit_setting"),
            "threads_setting": summary.get("threads_setting"),
            "samples": summary.get("samples"),
            "event_missing_last": summary.get("event_missing_last"),
        },
        "method": "ADR-025 修订记录（2026-10-26 批准）：预热丢弃前 10% 样本；"
        "相位对齐按 deleted_total 跳变推断清理周期后同相位跨周期配对；"
        "三态结论与冲突条款见 ADR；本脚本由 CI 程序化实施，人工不得改写数值",
    }
    out_path = os.path.join(os.path.dirname(path) or ".", "nfr204-verdict.json")
    with open(out_path, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(result, fh, ensure_ascii=False, indent=2)
        fh.write("\n")

    est = estimates[0]
    notice = (
        f"verdict={verdict}; b_fl={b_fl}; "
        f"ols={est['slope']} CI95={est['ci95_t']} ({est['verdict']}); "
        f"aligned={aligned['slope_mb_per_h'] if aligned else '未取得'}; "
        f"sigma={result['noise']['sigma_mb']}; min_detect={minimal_detectable}; "
        f"prereq={'满足' if prereq_met else '不满足'}; "
        f"threads={threads_note}; handles={handles_note}; "
        f"pid 单一={single_pid}; event_missing 全程零={missing_ok}; "
        f"build_profile={build_profile}; 详情见 nfr204-verdict.json"
    )
    print(f"::notice title=NFR-204 组合判据::{notice}")
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
