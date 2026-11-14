#!/usr/bin/env python3
"""PR 基准回归门禁（P7.2／ADR-075）：三个指标的**多轮中位＋相对回归**判定。

背景与口径：
  - P7.2 要求把“turn 首 token／会话切换／历史页”三个指标放进 PR 门禁，并多轮取中位；
  - 托管 runner 的绝对性能与 ADR-025 冻结值（本机 i7-1260P）不可比——2026-10-25 已有
    “托管 runner 上 NFR-207/209 未达冻结预算而红”的先例。因此**PR 门禁判定相对回归**：
    只与**同一 runner 类别、同一构建剖面**取得的基线比较；绝对冻结预算仍留在
    `perf-budget`（workflow_dispatch）通道，作发布门槛（ADR-025 决策 1、2 不变）；
  - 判据（两项取宽者，满足任一即通过）：`current <= baseline × (1 + tolerance)` 或
    `current <= baseline + slack_ms`。slack 是噪声底：基线很小时比率没有意义
    （例如 8 ms 抖动 2 ms 已是 25%），用绝对量兜住才不会把抖动当回归；
  - 基线未取得（runner 类别或剖面不匹配）时**显式 SKIP 并不声称通过**——这是
    “正确的空”（口径不匹配）与“错误的空”（没跑出信号）的区分，见
    docs/guides/signal-chain-completeness.md。

三个指标（均由 `cargo test -p whcode-tests --test nfr_budget <filter> -- --ignored --nocapture`
产出，夹具与 `perf-budget` 完全同一份）：

| 指标 | 夹具 | 判据量 | 说明 |
|---|---|---|---|
| turn 首 token | `p72_turn_first_token_latency` | **p50** | harness 就绪延迟（桩 Provider，不含网络与模型思考） |
| 会话切换 | `nfr207_switch_latency_within_frozen_budget` | **p50** | 10 万事件会话切换 |
| 历史页 | `nfr209_history_single_page_query_p95_within_frozen_budget` | **p50** | 单页查询（尾页形状） |

为什么判 p50 而不是 p95（2026-11-15 实测，ADR-075 §2.3）：同机同剖面双跑的抖动是
历史页 p95 28.9 → 44.6 ms（+54%，尾部离群 50—53 ms 由页缓存与磁盘主导），而 p50
24.5 → 26.5 ms（+8%）。尾部是环境噪声，中心趋势才反映“这次改动有没有让路径变慢”；
尾部（p95）的判据仍留在 ADR-025 的绝对预算通道。

用法：
    python .github/scripts/benchmarks/pr-bench-gate.py                 # PR 判定（默认基线路径）
    python .github/scripts/benchmarks/pr-bench-gate.py --rounds 3 --json artifacts/pr-bench.json
    python .github/scripts/benchmarks/pr-bench-gate.py --write-baseline artifacts/perf-baseline-pr.json
    python .github/scripts/benchmarks/pr-bench-gate.py --self-test     # 纯函数证伪探针

证据纪律：每条命令的退出码与解析到的原始行都进 JSON；缺指标、解析失败、样本为 0
一律按失败处理，不以 0 毫秒或空值冒充达标（ADR-010／信号链完备）。
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]

#: 默认基线（随仓库版本化：PR 门禁必须能在无网络、无历史 artifact 的情况下判定）。
DEFAULT_BASELINE = REPO_ROOT / "docs" / "matrices" / "perf-baseline-pr.json"

#: 构建剖面。固定为 debug：与 `perf-budget`／`measure-nfr-budgets.py` 对 NFR-207/209 的
#: 既有口径一致，基线只需与同剖面比较；绝对量不作发布门槛（那是 ADR-025 的 dispatch 通道）。
PROFILE = "debug"

#: 默认容差（相对）与噪声底（绝对毫秒）。取值依据见 ADR-075 §2：托管 runner 同提交
#: 复测的抖动经验值（历史记录里 p95 在 7—13 ms 间波动，约 ±30%），加上绝对兜底后
#: 只拦“成倍数”的回归，不拦抖动。
DEFAULT_TOLERANCE = 0.30
DEFAULT_SLACK_MS = 10.0

#: 指标定义：名称 -> (cargo test 过滤器, 输出行标签, 期望键)。
METRICS: dict[str, tuple[str, str, str]] = {
    "turn_first_token_p50_ms": ("p72", "P7.2 turn 首 token", "p50"),
    "session_switch_p50_ms": ("nfr207", "NFR-207 会话切换", "p50"),
    "history_page_p50_ms": ("nfr209", "NFR-209 历史单页查询", "p50"),
}

#: 分位数行：`<label>: n=20 p50=7.1ms p95=12.9ms p99=13.1ms min=7.0ms max=14.0ms`
PERCENTILE_LINE = re.compile(
    r"^(?P<label>.+?): n=(?P<n>\d+) p50=(?P<p50>\d+(?:\.\d+)?)ms "
    r"p95=(?P<p95>\d+(?:\.\d+)?)ms p99=(?P<p99>\d+(?:\.\d+)?)ms "
    r"min=(?P<min>\d+(?:\.\d+)?)ms max=(?P<max>\d+(?:\.\d+)?)ms$"
)


def fail(message: str) -> int:
    print(f"FAIL: {message}")
    return 1


def runner_class() -> str:
    """当前运行环境类别（基线只与同类环境比较；不同类别的数字不可比）。"""
    return platform.platform()


def cargo() -> str:
    found = shutil.which("cargo") or shutil.which("cargo.exe")
    if found is None:
        raise RuntimeError("未找到 cargo：无法运行测量夹具")
    return found


def run_fixture(filter_name: str) -> tuple[str, int]:
    """运行一个 `#[ignore]` 夹具，返回 (stdout+stderr, 退出码)。"""
    command = [
        cargo(),
        "test",
        "-p",
        "whcode-tests",
        "--test",
        "nfr_budget",
        filter_name,
        "--",
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]
    started = time.time()
    # WHCODE_PERF_ABSOLUTE_BUDGET=0：夹具只测量不判 ADR-025 绝对预算（PR 相对回归模式，
    # ADR-075）。perf-budget 通道不设该变量，绝对预算照旧断言。
    env = {**os.environ, "WHCODE_PERF_ABSOLUTE_BUDGET": "0"}
    proc = subprocess.run(
        command,
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    elapsed = time.time() - started
    output = proc.stdout + "\n" + proc.stderr
    print(f"[{filter_name}] 退出码 {proc.returncode}，耗时 {elapsed:.1f}s")
    return output, proc.returncode


def parse_metric(output: str, label: str, key: str) -> float | None:
    """从夹具输出里取指定标签行的指定分位数（毫秒）；取不到返回 None。

    注意 `--nocapture` 的 stdout 与测试 harness 的进度输出共用一行（行首是
    `test <name> ... `），所以按标签**行内截断**后再匹配，不能锚定行首（实测踩过）。
    """
    for line in output.splitlines():
        line = line.strip()
        index = line.find(label)
        if index < 0:
            continue
        match = PERCENTILE_LINE.match(line[index:])
        if match and match.group("label") == label:
            n = int(match.group("n"))
            if n <= 0:
                return None
            return float(match.group(key))
    return None


def measure_round() -> tuple[dict[str, float], dict[str, str], list[str]]:
    """跑一轮（三个夹具各一次），返回 (指标值, 原始行, 问题列表)。"""
    values: dict[str, float] = {}
    raw: dict[str, str] = {}
    problems: list[str] = []
    for name, (filter_name, label, key) in METRICS.items():
        output, code = run_fixture(filter_name)
        if code != 0:
            problems.append(f"{name}：夹具退出码 {code}（{filter_name}）")
            continue
        value = parse_metric(output, label, key)
        if value is None:
            problems.append(f"{name}：输出中找不到 `{label}` 的 {key}（不可用的空）")
            continue
        values[name] = value
        raw[name] = next(
            (line.strip() for line in output.splitlines() if label in line),
            "",
        )
    return values, raw, problems


def median_of_rounds(rounds: list[dict[str, float]]) -> dict[str, float]:
    """逐指标取跨轮中位（rounds 已按轮组织）。"""
    merged: dict[str, float] = {}
    names = sorted({name for round_values in rounds for name in round_values})
    for name in names:
        samples = [round_values[name] for round_values in rounds if name in round_values]
        if samples:
            merged[name] = statistics.median(samples)
    return merged


def limits(baseline: dict[str, float], tolerance: float, slack_ms: float) -> dict[str, float]:
    """每个指标的允许上限：baseline×(1+tolerance) 与 baseline+slack 取**宽者**（较大值）。

    取宽是对的语义：两个条件任一满足即通过，所以上限是二者的较大值。取严会把
    “基线 2 ms、slack 10 ms”这种小基线场景误判成回归（比率 400% 但绝对差仅 8 ms）。
    """
    return {
        name: max(value * (1.0 + tolerance), value + slack_ms)
        for name, value in baseline.items()
    }


def load_baseline(path: Path) -> dict:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict) or "metrics" not in payload:
        raise ValueError(f"基线文件结构不符（缺 metrics）：{path}")
    return payload


def judge(
    current: dict[str, float],
    baseline: dict,
    tolerance: float,
    slack_ms: float,
) -> tuple[list[str], list[str]]:
    """判定回归；返回 (失败条目, 说明行)。纯函数，供 --self-test 证伪。"""
    failures: list[str] = []
    notes: list[str] = []
    base_metrics = baseline.get("metrics", {})
    if not isinstance(base_metrics, dict):
        return ["基线 metrics 不是对象"], notes
    allowed = limits({k: float(v) for k, v in base_metrics.items()}, tolerance, slack_ms)
    for name, value in current.items():
        if name not in allowed:
            # 基线缺指标 ≠ 通过：无法判定就报失败，要求重抓基线（避免键名变更后静默放行）。
            notes.append(f"{name}：基线无此项 → 无法判定（需重抓基线）")
            failures.append(f"{name}（基线缺指标）")
            continue
        limit = allowed[name]
        verdict = "OK" if value <= limit else "REGRESSED"
        notes.append(
            f"{name}: current={value:.1f}ms baseline={base_metrics[name]:.1f}ms "
            f"limit={limit:.1f}ms → {verdict}"
        )
        if value > limit:
            failures.append(name)
    return failures, notes


def write_baseline(path: Path, metrics: dict[str, float], rounds: int, tolerance: float, slack_ms: float) -> None:
    payload = {
        "schema_version": 1,
        "metric_scope": "p7.2-pr-gate",
        "runner_class": runner_class(),
        "profile": PROFILE,
        "rounds": rounds,
        "tolerance": tolerance,
        "slack_ms": slack_ms,
        "measured_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "metrics": metrics,
        "notes": (
            "PR 门禁只在 runner_class 与 profile 都相同的环境下判定（ADR-075）；"
            "绝对冻结预算见 ADR-025 与 perf-budget 通道"
        ),
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(f"基线已写入 {path}")


def self_test() -> int:
    """纯函数证伪探针：判定逻辑必须能拦回归、放行抖动、拒绝不可用的空。"""
    failures: list[str] = []
    base = {"metrics": {"a": 10.0, "b": 100.0}}

    # 1) 抖动放行：10% 相对 + 绝对兜底之内 → 不红。
    fails, _ = judge({"a": 11.0, "b": 90.0}, base, 0.30, 10.0)
    if fails:
        failures.append(f"抖动被误判：{fails}")

    # 2) 成倍回归拦截：+120% 必红。
    fails, _ = judge({"a": 22.0}, base, 0.30, 10.0)
    if fails != ["a"]:
        failures.append(f"回归未拦：{fails}")

    # 3) 基线缺指标不能算通过（键名变更后必须重抓基线）。
    fails, _ = judge({"a": 10.0, "zz_missing": 1.0}, base, 0.30, 10.0)
    if fails != ["zz_missing（基线缺指标）"]:
        failures.append(f"基线缺指标未判失败：{fails}")

    # 4) 绝对兜底生效：基线 2 ms、slack 10 ms 时，10 ms 不应红（比率已 400%，绝对差仅 8 ms）。
    fails, _ = judge({"a": 10.0}, {"metrics": {"a": 2.0}}, 0.30, 10.0)
    if fails:
        failures.append(f"小基线抖动被误判：{fails}")

    # 5) 分位数行解析：正常行可取 p95；n=0 视为不可用的空。
    good = "NFR-207 会话切换: n=20 p50=7.1ms p95=12.9ms p99=13.1ms min=7.0ms max=14.0ms"
    if parse_metric(good, "NFR-207 会话切换", "p95") != 12.9:
        failures.append("分位数行解析失败")
    zero = "NFR-207 会话切换: n=0 p50=0.0ms p95=0.0ms p99=0.0ms min=0.0ms max=0.0ms"
    if parse_metric(zero, "NFR-207 会话切换", "p95") is not None:
        failures.append("n=0 未被判为不可用的空")

    # 6) 中位：偶数样本取两中值均值（3 轮中位即第 2 轮，若为偶数轮次则取均值）。
    if median_of_rounds([{"a": 1.0}, {"a": 9.0}]).get("a") != 5.0:
        failures.append("跨轮中位计算失败")

    if failures:
        for item in failures:
            print(f"FAIL: self-test: {item}")
        return 1
    print("self-test: 6 项探针全部通过（抖动放行／回归拦截／缺指标拒绝／绝对兜底／空样本拒绝／中位）")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="PR 基准回归门禁（P7.2／ADR-075）")
    parser.add_argument("--rounds", type=int, default=3, help="测量轮数（默认 3，逐轮三夹具各一次）")
    parser.add_argument("--baseline", default=str(DEFAULT_BASELINE), help="基线 JSON（默认 docs/matrices/perf-baseline-pr.json）")
    parser.add_argument("--write-baseline", default=None, help="把本轮测量写成基线文件（供人工复核后提交）")
    parser.add_argument("--json", default=None, help="把结论写成 JSON（CI 归档）")
    parser.add_argument("--tolerance", type=float, default=DEFAULT_TOLERANCE, help="相对容差（默认 0.30）")
    parser.add_argument("--slack-ms", type=float, default=DEFAULT_SLACK_MS, help="绝对噪声底（默认 10 ms）")
    parser.add_argument("--self-test", action="store_true", help="只跑纯函数证伪探针")
    args = parser.parse_args()

    if args.self_test:
        return self_test()
    if args.rounds < 1:
        return fail("--rounds 必须 ≥ 1")

    try:
        cargo()
    except RuntimeError as err:
        return fail(str(err))

    rounds: list[dict[str, float]] = []
    raw_lines: list[dict[str, str]] = []
    problems: list[str] = []
    for index in range(args.rounds):
        print(f"=== 第 {index + 1}/{args.rounds} 轮 ===")
        values, raw, round_problems = measure_round()
        problems.extend(round_problems)
        if values:
            rounds.append(values)
            raw_lines.append(raw)
    if problems:
        for item in problems:
            print(f"FAIL: {item}")
        return 1
    medians = median_of_rounds(rounds)
    missing = [name for name in METRICS if name not in medians]
    if missing:
        return fail(f"指标缺失：{missing}（不可用空值充当通过）")

    print("=== 跨轮中位 ===")
    for name, value in medians.items():
        print(f"  {name}: {value:.1f} ms")

    if args.write_baseline:
        write_baseline(Path(args.write_baseline), medians, args.rounds, args.tolerance, args.slack_ms)
        return 0

    baseline_path = Path(args.baseline)
    if not baseline_path.is_file():
        print(f"SKIP: 未找到基线 {baseline_path}；PR 门禁未判定（不声称通过）")
        return 0
    try:
        baseline = load_baseline(baseline_path)
    except (json.JSONDecodeError, ValueError) as err:
        return fail(f"基线不可解析：{err}")
    if baseline.get("profile") != PROFILE:
        print(
            f"SKIP: 基线剖面 {baseline.get('profile')} ≠ 当前 {PROFILE}；"
            "不同剖面的绝对量不可比（不声称通过）"
        )
        return 0
    if baseline.get("runner_class") != runner_class():
        print(
            f"SKIP: 基线 runner_class 与当前不同\n  基线: {baseline.get('runner_class')}\n"
            f"  当前: {runner_class()}\n  PR 门禁只在同类环境判定（ADR-075；不声称通过）"
        )
        return 0

    failures, notes = judge(medians, baseline, args.tolerance, args.slack_ms)
    print("=== 判定 ===")
    for line in notes:
        print(f"  {line}")

    report = {
        "schema_version": 1,
        "metric_scope": "p7.2-pr-gate",
        "runner_class": runner_class(),
        "profile": PROFILE,
        "rounds": args.rounds,
        "tolerance": args.tolerance,
        "slack_ms": args.slack_ms,
        "medians_ms": medians,
        "raw_lines": raw_lines,
        "baseline_metrics": baseline.get("metrics"),
        "failures": failures,
        "verdict": "pass" if not failures else "fail",
        "generated_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    if args.json:
        out = Path(args.json)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")
        print(f"报告已写入 {out}")

    if failures:
        return fail(f"基准回归：{failures}（详见上方 limit 对比）")
    print("PR 基准回归门禁：PASS（三指标均在容差内）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
