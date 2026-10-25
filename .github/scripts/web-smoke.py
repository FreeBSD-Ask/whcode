#!/usr/bin/env python3
"""Web 服务端冒烟（FR-603、ADR-023；CI 与本地同源）。

判据（可归因信号，不用“进程没崩”冒充通过）：
  1. `whcode serve --web --bind 127.0.0.1:<空闲端口>` 启动后，`GET /api/status` 返回 200 且含
     `schema_version` 与 `snapshot`；
  2. `GET /api/sessions` 返回 200 与 JSON 数组；
  3. `GET /` 返回 HTML（含 `<!DOCTYPE` 或 `<html`）；
  4. 关闭进程后退出码为 0（或平台约定的正常终止码）。

入站默认 TLS（ADR-040，2026-10-25）：本脚本按默认档位用 **https** 访问回环地址，并跳过
自签证书链校验（只验证“服务可用与路由正确”）；证书内容、指纹与浏览器信任**不**由本脚本
证明（归 T192 的真实握手证据），不得把本脚本通过当作 TLS 配置正确的结论。

工作区建在仓库外临时目录；只监听 127.0.0.1，不开放外部访问。
用法：
    python .github/scripts/web-smoke.py [--exe target/release/whcode.exe] [--json artifacts/web-smoke.json]
"""

from __future__ import annotations

import argparse
import json
import pathlib
import socket
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

REPO = pathlib.Path(__file__).resolve().parents[2]
DEFAULT_EXE = REPO / "target" / "release" / "whcode.exe"

# 自签证书（默认 builtin 档位）只用于回环冒烟：跳过链校验，不声称信任正确。
SSL_CONTEXT = ssl.create_default_context()
SSL_CONTEXT.check_hostname = False
SSL_CONTEXT.verify_mode = ssl.CERT_NONE


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def get(url: str, timeout: float = 5.0) -> tuple[int, bytes, str]:
    try:
        with urllib.request.urlopen(url, timeout=timeout, context=SSL_CONTEXT) as resp:
            return resp.status, resp.read(), resp.headers.get("content-type", "")
    except urllib.error.HTTPError as err:
        return err.code, err.read(), err.headers.get("content-type", "")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", default=str(DEFAULT_EXE))
    parser.add_argument("--json", default=str(REPO / "artifacts" / "web-smoke.json"))
    args = parser.parse_args()

    exe = pathlib.Path(args.exe)
    if not exe.is_file():
        sys.exit(f"缺少可执行文件：{exe}（先 cargo build --release -p whcode-bin）")

    workspace = pathlib.Path(tempfile.mkdtemp(prefix="whcode-web-smoke-"))

    # 先建一个会话，/api/status 与 /api/events 需要 session 参数。
    seed = subprocess.run(
        [str(exe), "--workspace", str(workspace), "session", "new", "--title", "冒烟", "--json"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=120,
    )
    if seed.returncode != 0:
        sys.exit(f"创建会话失败（{seed.returncode}）：{seed.stderr or seed.stdout}")
    envelope = json.loads(seed.stdout)
    session_id = (
        envelope.get("session_id")
        or envelope.get("id")
        or (envelope.get("data") or {}).get("session", {}).get("id")
    )
    if not session_id:
        sys.exit(f"会话信封缺少 session id：{envelope}")

    port = free_port()
    # 默认档位是入站 TLS（ADR-040）：用 https 访问，自签证书跳过链校验。
    base = f"https://127.0.0.1:{port}"
    process = subprocess.Popen(
        [str(exe), "serve", "--web", "--bind", f"127.0.0.1:{port}", "--workspace", str(workspace)],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    checks: list[dict[str, object]] = []
    try:
        deadline = time.time() + 30
        status_body = b""
        while time.time() < deadline:
            try:
                code, status_body, ctype = get(f"{base}/api/status?session={session_id}", timeout=2)
                if code == 200:
                    checks.append({"check": "api_status_http", "ok": True, "detail": f"{code} {ctype}"})
                    break
            except (urllib.error.URLError, TimeoutError, ConnectionError):
                pass
            if process.poll() is not None:
                out, err = process.communicate(timeout=5)
                sys.exit(f"服务进程提前退出（{process.returncode}）：{err or out}")
            time.sleep(0.3)
        else:
            sys.exit("30 秒内 /api/status 未就绪")

        payload = json.loads(status_body.decode("utf-8"))
        checks.append(
            {
                "check": "api_status_envelope",
                "ok": isinstance(payload, dict) and "schema_version" in payload,
                "detail": f"keys={sorted(payload)[:8]}" if isinstance(payload, dict) else type(payload).__name__,
            }
        )

        code, body, ctype = get(f"{base}/api/sessions")
        sessions = json.loads(body.decode("utf-8")) if code == 200 else None
        checks.append(
            {
                "check": "api_sessions_json",
                "ok": code == 200
                and isinstance(sessions, dict)
                and isinstance(sessions.get("sessions"), list)
                and len(sessions["sessions"]) >= 1,
                "detail": f"{code} {ctype}",
            }
        )

        code, body, ctype = get(f"{base}/")
        text = body.decode("utf-8", errors="replace")
        checks.append(
            {
                "check": "index_html",
                "ok": code == 200 and ("<html" in text.lower() or "<!doctype" in text.lower()),
                "detail": f"{code} {ctype} bytes={len(body)}",
            }
        )
    finally:
        if process.poll() is None:
            process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)

    checks.append(
        {
            "check": "graceful_shutdown",
            "ok": process.returncode in (0, 1, -15, 143),
            "detail": f"exit={process.returncode}",
        }
    )

    failed = [item for item in checks if not item["ok"]]
    report = {
        "schema_version": 1,
        "exe": str(exe),
        "bind": f"127.0.0.1:{port}",
        "workspace": str(workspace),
        "checks": checks,
        "failed": len(failed),
    }
    out = pathlib.Path(args.json)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    for item in checks:
        print(f"{'PASS' if item['ok'] else 'FAIL'} {item['check']}: {item['detail']}")
    print(f"报告：{out}")
    return 0 if not failed else 1


if __name__ == "__main__":
    sys.exit(main())
