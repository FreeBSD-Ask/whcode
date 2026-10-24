#!/usr/bin/env python3
"""工作流语义门禁：拦截 GitHub Actions 明确不接受的上下文用法。

不能替代 GitHub 的服务端校验，但覆盖已发生的真实事故：
- `jobs.<job_id>.if` **不支持** `matrix` 上下文（matrix 在 job 级 if 求值时尚未展开），
  2026-10-24 的 `platform-matrix.yml` 就因此被判 “Invalid workflow file”。
- 同时检查 `jobs.<job_id>.runs-on` 之外没有把 `matrix` 写进 job 级 `if` 的其他形式。

只做行级结构检查（不依赖 PyYAML，Windows runner 的 Python 无第三方包）。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github" / "workflows"


def main() -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    problems: list[str] = []
    for path in sorted(WORKFLOWS.glob("*.yml")):
        lines = path.read_text(encoding="utf-8").splitlines()
        if not any(line.startswith("permissions:") for line in lines):
            problems.append(
                f"{path.relative_to(ROOT)}: 缺少顶层 permissions（CodeQL "
                "actions/missing-workflow-permissions）；至少声明 contents: read"
            )
        in_jobs = False
        job: str | None = None
        for number, line in enumerate(lines, 1):
            stripped = line.strip()
            if not in_jobs:
                if stripped == "jobs:":
                    in_jobs = True
                continue
            indent = len(line) - len(line.lstrip())
            if indent == 2 and re.match(r"^[A-Za-z0-9_-]+:\s*$", stripped):
                job = stripped[:-1]
                continue
            if indent == 4 and stripped.startswith("if:") and "matrix." in stripped:
                problems.append(
                    f"{path.relative_to(ROOT)}:{number}: jobs.{job}.if 使用 matrix 上下文；"
                    "GitHub 不支持（matrix 仅在步骤级可用）"
                )
    if problems:
        for problem in problems:
            print(f"FAIL: {problem}")
        return 1
    print("工作流语义门禁通过（job 级 if 未使用 matrix；均声明顶层 permissions）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
