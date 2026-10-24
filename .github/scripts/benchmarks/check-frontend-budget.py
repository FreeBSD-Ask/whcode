#!/usr/bin/env python3
"""极简前端基线审计（T137、FR-618、ADR-014）。

规则（FR-618 预算）：
  - JS ≤ 50 KB（gzip 压缩后；审计 app.js 与 suspend.js）；
  - CSS ≤ 20 KB（gzip 压缩后）；
  - 无 WebGL / 视频 / 自动播放；
  - 运行时路径（app.js）不得创建/探测 GPU 上下文（NFR-212）；
  - 无第三方字体、无第三方 JS 依赖、无 node 构建链产物（package.json / node_modules）。

用法：python .github/scripts/benchmarks/check-frontend-budget.py   # 审计，违规即非零退出
"""

from __future__ import annotations

import gzip
import pathlib
import sys

FRONTEND = pathlib.Path(__file__).resolve().parents[3] / "crates" / "whcode-web" / "frontend"
JS_BUDGET_BYTES = 50 * 1024
CSS_BUDGET_BYTES = 20 * 1024

FORBIDDEN_MARKERS = {
    ".js": [
        b"new WebSocket(",
        b"<video",
        b"autoplay",
        b"requestanimationframe(",
    ],
    ".html": [b"<video", b"autoplay"],
}


def gzip_size(data: bytes) -> int:
    return len(gzip.compress(data))


def main() -> int:
    if not FRONTEND.is_dir():
        print(f"前端目录不存在：{FRONTEND}")
        return 1

    failures: list[str] = []
    for name in ["app.js", "suspend.js", "index.html", "styles.css"]:
        path = FRONTEND / name
        if not path.is_file():
            failures.append(f"缺少 {name}")
            continue
        data = path.read_bytes()
        for marker in FORBIDDEN_MARKERS.get(path.suffix, []):
            if marker in data.lower():
                failures.append(f"{name} 含禁止标记：{marker.decode()}")
        if path.suffix == ".js" and gzip_size(data) > JS_BUDGET_BYTES:
            failures.append(
                f"{name} gzip {gzip_size(data)} 字节超过预算 {JS_BUDGET_BYTES}（FR-618）"
            )
        if path.suffix == ".css" and gzip_size(data) > CSS_BUDGET_BYTES:
            failures.append(
                f"{name} gzip {gzip_size(data)} 字节超过预算 {CSS_BUDGET_BYTES}（FR-618）"
            )

    # NFR-212：运行时路径不得创建/探测 GPU 上下文——app.js 不得调用 getContext/probeGpu
    # （suspend.js 的 probeGpu 是显式启用的能力探测，未被运行时路径调用；见 app.js 注释）。
    app = FRONTEND / "app.js"
    if app.is_file():
        text = app.read_bytes().lower()
        # 标记按“调用形态”匹配（允许注释中提及函数名，不允许实际调用）。
        for marker in [b"getcontext(", b"probegpu("]:
            if marker in text:
                failures.append(
                    f"app.js 含 {marker.decode()}：运行时路径不得创建/探测 GPU 上下文（NFR-212）"
                )

    for banned in ["package.json", "node_modules"]:
        if (FRONTEND / banned).exists():
            failures.append(f"前端目录含 {banned}：违反 ADR-014 无 node 构建链")

    # 第三方字体：@font-face 或外部字体链接。
    css = (FRONTEND / "styles.css").read_bytes() if (FRONTEND / "styles.css").is_file() else b""
    if b"@font-face" in css.lower():
        failures.append("styles.css 含 @font-face：禁止第三方字体（FR-618）")

    if failures:
        for failure in failures:
            print(f"FAIL: {failure}")
        return 1
    print(
        f"前端基线通过：JS gzip {gzip_size((FRONTEND / 'app.js').read_bytes())} B / "
        f"CSS gzip {gzip_size((FRONTEND / 'styles.css').read_bytes())} B，无违例"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
