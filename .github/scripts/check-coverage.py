#!/usr/bin/env python3
"""覆盖率试点门禁：`whcode-app`／`whcode-store` 行覆盖 ≥90%（FR-762／ADR-060 项 I）。

性质：可复现的测量门槛，不做趋势解读。判定口径固定为**crate 级行覆盖**
（`cargo llvm-cov` 的 `lines` 计数），不采纳 per-file 100% 口径（平台分支多的 Rust
crate 成本畸高，proposal-v3 §5 反建议已明确拒绝）。

判定规则：
  1. 对每个试点 crate，取其所有源文件的 `lines.count/covered` 汇总；
  2. 扣减 `EXEMPT_FILES` 中登记的豁免文件（每条必须带理由；豁免不是默认权利）；
  3. 行覆盖率低于 `THRESHOLD` 即失败；工具缺失或输出无法解析同样失败（**不得假绿**）。

局限（必须承认）：
  - 覆盖率不证明测试质量：空断言、只跑正向路径都能推高行覆盖。它只排除“代码从未
    被执行”这一类问题，其余仍由行为测试与 Check 取证负责。
  - 平台分支（Windows/FreeBSD 条件编译）只能在对应平台测得；在 Windows 上测得的
    数字不得外推为跨平台结论。

CI 定位：ADR-060 项 I 规定“CI 先登记非阻断”——工作流步骤带 `continue-on-error: true`，
红不阻断合并，但必须在批次复核中解释；转为阻断须另立 ADR。

复审触发：2027-05-07（D15 试点复审）；阈值或豁免清单变更须同步本文件与
`docs/guides/software-engineering-process.md` 的登记。

用法：
    python .github/scripts/check-coverage.py [--root <仓库根>] [--json <输出文件>]
"""

from __future__ import annotations

import argparse
import json
import pathlib
import shutil
import subprocess
import sys

THRESHOLD = 90.0
PILOT_CRATES = ("whcode-app", "whcode-store")

# 豁免文件 → 理由。当前为空：试点口径为 crate 级汇总，个别文件低于均值不构成豁免理由。
# 新增豁免必须写明“为何不可测”（平台专用、外部副作用、生成代码），并在复审时重新评估。
EXEMPT_FILES: dict[str, str] = {}


def run_cov(root: pathlib.Path) -> str:
    cargo = shutil.which("cargo")
    if cargo is None:
        raise SystemExit("FAIL: 未找到 cargo（请在 Rust 工具链环境中运行）")
    command = [
        cargo,
        "llvm-cov",
        "--json",
        "--summary-only",
        "--manifest-path",
        str(root / "Cargo.toml"),
    ]
    for crate in PILOT_CRATES:
        command.extend(["-p", crate])
    proc = subprocess.run(command, cwd=root, capture_output=True, text=True)
    if proc.returncode != 0:
        tail = (proc.stderr or proc.stdout).strip().splitlines()[-8:]
        raise SystemExit(
            "FAIL: cargo llvm-cov 运行失败（工具缺失或编译失败，不得记为通过）：\n"
            + "\n".join(tail)
        )
    return proc.stdout


def extract_json(stdout: str) -> dict:
    for line in stdout.splitlines():
        stripped = line.strip()
        if stripped.startswith('{"data"'):
            return json.loads(stripped)
    raise SystemExit("FAIL: 未能从 cargo llvm-cov 输出中解析 JSON（输出格式变更？）")


def crate_of(filename: str) -> str | None:
    normalized = filename.replace("\\", "/")
    for crate in PILOT_CRATES:
        if f"/crates/{crate}/" in normalized:
            return crate
    return None


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", default=str(pathlib.Path(__file__).resolve().parents[2]))
    parser.add_argument("--json", dest="json_out", default=None)
    args = parser.parse_args(argv)
    root = pathlib.Path(args.root).resolve()

    report = extract_json(run_cov(root))
    totals: dict[str, dict[str, int]] = {
        crate: {"count": 0, "covered": 0} for crate in PILOT_CRATES
    }
    exempted: list[str] = []

    for entry in report.get("data", []):
        for file_entry in entry.get("files", []):
            filename = file_entry.get("filename", "")
            crate = crate_of(filename)
            if crate is None:
                continue
            rel = filename.replace("\\", "/").split("/crates/", 1)[-1]
            if rel in EXEMPT_FILES:
                exempted.append(rel)
                continue
            lines = file_entry.get("summary", {}).get("lines", {})
            totals[crate]["count"] += int(lines.get("count", 0))
            totals[crate]["covered"] += int(lines.get("covered", 0))

    failures: list[str] = []
    print(f"覆盖率试点（阈值 {THRESHOLD:.1f}% 行覆盖，crate 级）")
    for crate in PILOT_CRATES:
        count = totals[crate]["count"]
        covered = totals[crate]["covered"]
        percent = (covered / count * 100.0) if count else 0.0
        mark = "OK" if percent >= THRESHOLD else "FAIL"
        print(f"  {mark:4} {crate}: {covered}/{count} 行 = {percent:.2f}%")
        if percent < THRESHOLD:
            failures.append(f"FAIL: {crate} 行覆盖 {percent:.2f}% < {THRESHOLD:.1f}%")
    if exempted:
        print(f"  豁免 {len(exempted)} 个文件：{', '.join(sorted(exempted))}")
    for path, reason in sorted(EXEMPT_FILES.items()):
        print(f"    豁免理由 {path}: {reason}")

    if args.json_out:
        out = pathlib.Path(args.json_out)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(
            json.dumps(
                {
                    "threshold": THRESHOLD,
                    "crates": {c: {**totals[c], "percent": (
                        totals[c]["covered"] / totals[c]["count"] * 100.0
                    ) if totals[c]["count"] else 0.0} for c in PILOT_CRATES},
                    "exempted": sorted(exempted),
                },
                ensure_ascii=False,
                indent=2,
            )
            + "\n",
            encoding="utf-8",
            newline="\n",
        )

    if failures:
        for line in failures:
            print(line)
        return 1
    print("coverage: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
