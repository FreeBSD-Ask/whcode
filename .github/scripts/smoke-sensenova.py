#!/usr/bin/env python3
"""真 API 冒烟：sensenova 单条请求（FR-764／ADR-060 项 K）。

目的：给“真实链路可达”留下可归档证据，而不是拿本地 mock 证明端到端可用。
判定三态（写入 artifact，禁止把跳过记成通过）：

  pass     HTTP 2xx 且返回非空 `choices[0].message`——真实链路可用；
  skip     未找到凭据（`SENSENOVA_API_KEY` 环境变量与 `local/sensenova.env` 均无）——
           **不是通过**，CI 为绿只表示“无凭据，未执行”；
  fail     有凭据但请求失败、超时、非 2xx 或结构不符——必须在批次复核中解释。

凭据纪律（AGENTS.md §3.1、§5.2）：
  - 只从环境变量或 `local/sensenova.env` 读取，后者不入库；
  - 任何输出、artifact、异常文本都不得包含 key；本脚本只打印长度与布尔量；
  - 不写事件表、不写追踪正文：本脚本是开发期门禁，不进入应用运行路径。

局限：单条请求只证明“当前网络、当前凭据、当前模型 ID 可用”，不证明重试、流式、
工具调用、压缩等路径；那些仍需各自的行为测试。

复审触发：模型 ID／端点变更（AGENTS.md §3.1 更新）→ 同步本脚本默认值；
连续两个批次 `skip`（CI 无凭据）→ 该门禁的证据价值需复审，不得长期以“绿”占位。

用法：
    python .github/scripts/smoke-sensenova.py [--artifact artifacts/sensenova-smoke.json]
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import socket
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
ENV_FILE = ROOT / "local" / "sensenova.env"
DEFAULT_URL = "https://token.sensenova.cn/v1/chat/completions"
DEFAULT_MODEL = "sensenova-6.8-flash-lite"
TIMEOUT_SECONDS = 30
MAX_TOKENS = 16


def load_env_file(path: pathlib.Path) -> dict[str, str]:
    """容忍 `KEY=VALUE`、`export KEY=VALUE` 与引号；不解析注释以外语法。"""
    if not path.is_file():
        return {}
    values: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or "=" not in stripped:
            continue
        key, _, value = stripped.partition("=")
        key = key.strip()
        if key.startswith("export "):
            key = key[len("export ") :].strip()
        value = value.strip().strip("'\"")
        if key:
            values[key] = value
    return values


def resolve(env_file: dict[str, str], name: str, default: str) -> str:
    return os.environ.get(name) or env_file.get(name) or default


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--artifact", default="artifacts/sensenova-smoke.json", help="结果归档路径"
    )
    parser.add_argument(
        "--env-file",
        default=str(ENV_FILE),
        help="凭据文件（默认 local/sensenova.env；不存在即视为无凭据）",
    )
    args = parser.parse_args(argv)

    env_file = load_env_file(pathlib.Path(args.env_file))
    api_key = os.environ.get("SENSENOVA_API_KEY") or env_file.get("SENSENOVA_API_KEY", "")
    url = resolve(env_file, "SENSENOVA_CHAT_URL", DEFAULT_URL)
    model = resolve(env_file, "SENSENOVA_MODEL", DEFAULT_MODEL)

    artifact = pathlib.Path(args.artifact)
    if not artifact.is_absolute():
        artifact = ROOT / artifact
    artifact.parent.mkdir(parents=True, exist_ok=True)
    base = {
        "checked_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "endpoint_host": urllib.parse.urlparse(url).hostname or "unknown",
        "model": model,
    }

    if not api_key:
        record = {**base, "status": "skip", "reason": "未找到凭据（环境变量与 local/sensenova.env 均无）"}
        artifact.write_text(
            json.dumps(record, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
            newline="\n",
        )
        print("sensenova-smoke: SKIP（无凭据，未执行；本结果不是通过）")
        return 0

    payload = {
        "model": model,
        "messages": [{"role": "user", "content": "仅回复 OK"}],
        "max_tokens": MAX_TOKENS,
        "temperature": 0,
    }
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {api_key}",
        },
        method="POST",
    )
    started = time.monotonic()
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT_SECONDS) as response:
            status_code = response.status
            body = response.read(1 << 20)
    except urllib.error.HTTPError as err:
        elapsed_ms = int((time.monotonic() - started) * 1000)
        record = {
            **base,
            "status": "fail",
            "reason": f"HTTP {err.code}",
            "latency_ms": elapsed_ms,
        }
        artifact.write_text(
            json.dumps(record, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
            newline="\n",
        )
        print(f"sensenova-smoke: FAIL（HTTP {err.code}，{elapsed_ms} ms）")
        return 1
    except (urllib.error.URLError, socket.timeout, TimeoutError) as err:
        elapsed_ms = int((time.monotonic() - started) * 1000)
        record = {
            **base,
            "status": "fail",
            "reason": f"网络错误 {type(err).__name__}",
            "latency_ms": elapsed_ms,
        }
        artifact.write_text(
            json.dumps(record, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
            newline="\n",
        )
        print(f"sensenova-smoke: FAIL（{type(err).__name__}，{elapsed_ms} ms）")
        return 1

    elapsed_ms = int((time.monotonic() - started) * 1000)
    try:
        parsed = json.loads(body.decode("utf-8", errors="replace"))
        choices = parsed.get("choices") or []
        message = choices[0].get("message") if choices else None
        content = (message or {}).get("content") or ""
        usage = parsed.get("usage") or {}
        ok = bool(status_code == 200 and content)
    except (ValueError, AttributeError, IndexError, TypeError):
        ok = False
        usage = {}
        content = ""

    record = {
        **base,
        "status": "pass" if ok else "fail",
        "latency_ms": elapsed_ms,
        "http_status": status_code,
        "content_nonempty": bool(content),
        "usage": {
            "prompt_tokens": usage.get("prompt_tokens"),
            "completion_tokens": usage.get("completion_tokens"),
            "total_tokens": usage.get("total_tokens"),
        },
    }
    artifact.write_text(
        json.dumps(record, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )

    if not ok:
        print(f"sensenova-smoke: FAIL（HTTP {status_code}，结构不符或正文为空）")
        return 1
    print(
        "sensenova-smoke: PASS"
        f"（{elapsed_ms} ms，prompt={record['usage']['prompt_tokens']}，"
        f"completion={record['usage']['completion_tokens']}）"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
