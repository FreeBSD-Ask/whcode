#!/usr/bin/env python3
"""CI action 版本门禁（铁律：最新版本 + 固定 commit SHA）。

背景（2026-10-24 用户指令，两次细化）：
1. 工作流里的 action 必须始终为**最新版本**；
2. action 必须使用**固定 commit SHA** 引用，禁止 tag／分支等可移动引用。

规则与执行分离：

- `.github/scripts/action-versions.json` 是唯一事实源：每个 action 记录最新版本 tag、对应 commit SHA
  与复核日期；
- 离线模式（CI 每次运行）：解析 `.github/workflows/*.yml` 的 `uses:`，要求
  `owner/repo@<40 位十六进制 SHA> # <版本>`，且 SHA 与注释版本都与清单一致；
- 联网模式（`--online`，由每周审计工作流执行）：查询 GitHub API 的最新 release／tag，
  解析其 commit SHA，与清单比对；上游更新而清单未更新即失败；
- `--update` 仅由维护者本地运行：联网刷新清单（不自动改写工作流，避免静默变更）。

禁止 `@main`／`@master`／`@v7` 等可移动引用；禁止跳过本门禁。例外必须走 ADR。
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github" / "workflows"
MANIFEST = ROOT / ".github" / "scripts" / "action-versions.json"
API = "https://api.github.com/repos/{repo}/{endpoint}"
USE_PATTERN = re.compile(
    r"uses:\s*([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)@(\S+)(?:\s*#\s*(\S+))?"
)
SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
FLOATING_REFS = {"main", "master", "HEAD"}


def workflow_refs() -> dict[str, list[tuple[str, str | None, str]]]:
    """返回 action -> [(ref, 注释版本, 工作流文件名)]。"""
    refs: dict[str, list[tuple[str, str | None, str]]] = {}
    for path in sorted(WORKFLOWS.glob("*.yml")):
        for line in path.read_text(encoding="utf-8").splitlines():
            match = USE_PATTERN.search(line)
            if not match:
                continue
            repo, ref, version = match.group(1), match.group(2), match.group(3)
            refs.setdefault(repo, []).append((ref, version, path.name))
    return refs


def request_json(repo: str, endpoint: str):
    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": "whcode-action-version-gate",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    url = API.format(repo=repo, endpoint=endpoint)
    request = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.loads(response.read().decode("utf-8"))


def version_key(tag: str) -> tuple[int, ...]:
    numbers = re.findall(r"\d+", tag)
    return tuple(int(part) for part in numbers) if numbers else (0,)


def latest_tag(repo: str) -> str:
    try:
        release = request_json(repo, "releases/latest")
        return str(release["tag_name"])
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise
    tags = request_json(repo, "tags?per_page=100")
    if not tags:
        raise RuntimeError(f"{repo} 没有 release 也没有 tag")
    return max((str(tag["name"]) for tag in tags), key=version_key)


def tag_sha(repo: str, tag: str) -> str:
    commit = request_json(repo, f"commits/{tag}")
    return str(commit["sha"])


def load_manifest() -> dict:
    return json.loads(MANIFEST.read_text(encoding="utf-8"))


def save_manifest(entries: dict[str, dict[str, str]]) -> None:
    payload = {
        "checked_at": dt.datetime.now(dt.UTC).strftime("%Y-%m-%d"),
        "source": "GitHub API releases/latest；无 release 时取最高语义化 tag；SHA 由 commits/{tag} 解析",
        "actions": dict(sorted(entries.items())),
    }
    MANIFEST.write_text(
        json.dumps(payload, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )


def update() -> int:
    refs = workflow_refs()
    entries: dict[str, dict[str, str]] = {}
    for repo in sorted(refs):
        tag = latest_tag(repo)
        entries[repo] = {"version": tag, "sha": tag_sha(repo, tag)}
    save_manifest(entries)
    print("已刷新 .github/scripts/action-versions.json：")
    for repo, item in sorted(entries.items()):
        print(f"  {repo} {item['version']} {item['sha']}")
    print("请同步修改工作流中的 uses 引用（SHA + 版本注释），然后运行离线门禁。")
    return 0


def check(online: bool) -> int:
    manifest = load_manifest()
    pinned: dict[str, dict[str, str]] = manifest.get("actions", {})
    refs = workflow_refs()
    problems: list[str] = []

    if not refs:
        problems.append(".github/workflows/ 下没有解析到任何 uses 引用")

    for repo, usages in sorted(refs.items()):
        expected = pinned.get(repo)
        if expected is None:
            problems.append(f"{repo} 未登记在 .github/scripts/action-versions.json")
            continue
        for ref, comment, workflow in usages:
            location = f"{workflow}: {repo}@{ref}"
            if ref in FLOATING_REFS or not SHA_PATTERN.match(ref):
                problems.append(f"{location} 不是固定 commit SHA（禁止 tag／分支）")
                continue
            if ref != expected.get("sha"):
                problems.append(
                    f"{location} 与清单 SHA 不一致（清单 {expected.get('version')} "
                    f"{expected.get('sha')}）"
                )
            if comment is None:
                problems.append(f"{location} 缺少版本注释（应写 `# {expected.get('version')}`）")
            elif comment != expected.get("version"):
                problems.append(
                    f"{location} 注释版本 {comment} 与清单 {expected.get('version')} 不一致"
                )

    if online:
        for repo, expected in sorted(pinned.items()):
            try:
                upstream_version = latest_tag(repo)
                upstream_sha = tag_sha(repo, upstream_version)
            except Exception as error:  # noqa: BLE001 - 联网失败必须显式失败
                problems.append(f"{repo} 联网复核失败：{error}")
                continue
            if upstream_version != expected.get("version") or upstream_sha != expected.get("sha"):
                problems.append(
                    f"{repo} 上游最新为 {upstream_version} {upstream_sha}，清单为 "
                    f"{expected.get('version')} {expected.get('sha')}；请运行 --update 并同步工作流"
                )

    if problems:
        for problem in problems:
            print(f"FAIL: {problem}")
        return 1
    mode = "离线 SHA 比对" if not online else "联网最新版本 + 离线 SHA 比对"
    print(f"CI action 版本门禁通过（{mode}；清单复核日期 {manifest.get('checked_at')}）")
    return 0


def main() -> int:
    # Windows runner 的控制台代码页不是 UTF-8（cp1252），中文输出会抛 UnicodeEncodeError。
    # 门禁本身不依赖输出编码，故强制 UTF-8 且替换无法编码的字符，避免“检查通过但打印失败”。
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    parser = argparse.ArgumentParser(description="CI action 版本门禁")
    parser.add_argument("--online", action="store_true", help="联网复核上游最新版本与 SHA")
    parser.add_argument("--update", action="store_true", help="联网刷新清单（维护者本地用）")
    args = parser.parse_args()
    if args.update:
        return update()
    return check(args.online)


if __name__ == "__main__":
    sys.exit(main())
