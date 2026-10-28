#!/usr/bin/env python3
"""Code Scanning 零 open 告警门禁（用户指令 2026-10-28：必须时刻保持 0 问题）。

以 GitHub REST API 为唯一事实源：统计 security/code-scanning（CodeQL 默认分析）
在默认分支上的 open 告警数。结论只能来自 API 返回值：

- 存在 open 告警：逐条打印（编号/规则/严重级/位置），退出码 1；
- 查询被阻塞（无凭据、无网络、仓库不存在）：退出码 2，显式写“阻塞”，
  不得以本地静态推断冒充通过；
- 0 条 open：退出码 0。

凭据来源（按序）：`GITHUB_TOKEN` 环境变量 → `gh auth token`（gh CLI 已登录时）。
仓库来源（按序）：`--repo` → `GITHUB_REPOSITORY` → `git remote get-url origin`。

本脚本不修改任何告警（不 dismiss、不关闭）：告警的消除只能由修复代码后
CodeQL 重新分析产生（ADR-037：本地运行只作复现与排障，不冒充验收）。
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path

API = "https://api.github.com"


def _utf8() -> None:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")


def resolve_repo(explicit: str | None) -> str:
    if explicit:
        return explicit
    env = os.environ.get("GITHUB_REPOSITORY")
    if env:
        return env
    url = subprocess.run(
        ["git", "remote", "get-url", "origin"],
        capture_output=True,
        text=True,
        check=False,
    ).stdout.strip()
    match = re.search(r"(?:github\.com[/:])([\w.-]+/[\w.-]+?)(?:\.git)?$", url)
    if not match:
        raise SystemExit("BLOCKED: 无法确定仓库（--repo / GITHUB_REPOSITORY / git remote 均未给出）")
    return match.group(1)


def resolve_token() -> str:
    token = os.environ.get("GITHUB_TOKEN", "").strip()
    if token:
        return token
    gh = subprocess.run(
        ["gh", "auth", "token"], capture_output=True, text=True, check=False
    ).stdout.strip()
    if gh:
        return gh
    raise SystemExit(
        "BLOCKED: 无可用凭据（GITHUB_TOKEN 或 gh auth token）。"
        "零告警结论必须来自 API，无法取得证据时按阻塞处理，不得跳过。"
    )


def api_get(url: str, token: str) -> tuple[int, object, str]:
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "whcode-check-code-scanning",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return response.status, json.loads(response.read().decode("utf-8")), response.headers.get("Link", "")
    except urllib.error.HTTPError as err:
        return err.code, {}, ""
    except (urllib.error.URLError, TimeoutError, OSError) as err:
        raise SystemExit(f"BLOCKED: GitHub API 不可达：{err}")


def fetch_open_alerts(repo: str, token: str) -> list[dict]:
    alerts: list[dict] = []
    url = f"{API}/repos/{repo}/code-scanning/alerts?state=open&per_page=100"
    while url:
        status, payload, link = api_get(url, token)
        if status == 404:
            raise SystemExit(
                "BLOCKED: 仓库无 code scanning 权限或从未生成分析（404）；"
                "零告警结论无法核验，按阻塞处理"
            )
        if status == 403:
            raise SystemExit("BLOCKED: API 拒绝（403，可能是限流或令牌权限不足）")
        if status != 200:
            raise SystemExit(f"BLOCKED: GitHub API 返回 {status}")
        alerts.extend(payload)
        match = re.search(r'<([^>]+)>; rel="next"', link)
        url = match.group(1) if match else ""
    return alerts


def main() -> int:
    _utf8()
    args = sys.argv[1:]
    repo_arg = None
    if "--repo" in args:
        repo_arg = args[args.index("--repo") + 1]
    repo = resolve_repo(repo_arg)
    token = resolve_token()
    alerts = fetch_open_alerts(repo, token)
    if alerts:
        print(f"FAIL: {repo} 存在 {len(alerts)} 条 open 的 code scanning 告警：")
        for alert in sorted(alerts, key=lambda item: item["number"]):
            instance = alert.get("most_recent_instance", {})
            location = instance.get("location", {})
            path = location.get("path", "?")
            line = location.get("start_line", "?")
            print(
                f"  #{alert['number']}\t{alert.get('rule', {}).get('id', '?')}\t"
                f"{alert.get('rule', {}).get('security_severity_level', '?')}\t"
                f"{path}:{line}"
            )
        print("修复方式：按位置修复代码并推送，由 CodeQL 重新分析消除；禁止 dismiss 代替修复。")
        return 1
    print(f"GO: {repo} 的 code scanning 当前为 0 条 open 告警（事实源：GitHub API）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
