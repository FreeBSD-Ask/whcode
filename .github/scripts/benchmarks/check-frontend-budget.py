#!/usr/bin/env python3
"""极简前端基线审计（T137、FR-618、ADR-014；T200／ADR-041 扩展；T190 证书设置页）。

规则（FR-618 预算）：
  - JS ≤ 50 KB（gzip 压缩后；审计 app.js、suspend.js 与 settings.js）；
  - CSS ≤ 20 KB（gzip 压缩后）；
  - DOM 起始标签 ≤ 2 000/页（静态上界；审计 index.html 与 settings.html；
    数据行由分页与有界列表约束，ADR-041 决策 7）；
  - 数据渲染只允许 textContent／属性赋值——禁止 innerHTML/outerHTML/insertAdjacentHTML
    （XSS 防线，ADR-041 决策 6）；
  - 无 WebGL / 视频 / 自动播放；
  - 运行时路径（app.js 与 settings.js）不得创建/探测 GPU 上下文（NFR-212）；
  - 无第三方字体、无第三方 JS 依赖、无 node 构建链产物（package.json / node_modules）。

用法：python .github/scripts/benchmarks/check-frontend-budget.py   # 审计，违规即非零退出
      python .github/scripts/benchmarks/check-frontend-budget.py --selftest
"""

from __future__ import annotations

import gzip
import pathlib
import re
import sys

FRONTEND = pathlib.Path(__file__).resolve().parents[3] / "crates" / "whcode-web" / "frontend"
JS_BUDGET_BYTES = 50 * 1024
CSS_BUDGET_BYTES = 20 * 1024
DOM_BUDGET_NODES = 2000

FORBIDDEN_MARKERS = {
    ".js": [
        b"new WebSocket(",
        b"<video",
        b"autoplay",
        b"requestanimationframe(",
    ],
    ".html": [b"<video", b"autoplay"],
}

# DOM 注入 API（出现即失败）：XSS 防线要求服务端数据只经 textContent／属性赋值。
DOM_INJECTION_MARKERS = [b"innerhtml", b"outerhtml", b"insertadjacenthtml"]

START_TAG = re.compile(rb"<[a-zA-Z][a-zA-Z0-9-]*")
HTML_COMMENT = re.compile(rb"<!--.*?-->", re.S)


def gzip_size(data: bytes) -> int:
    return len(gzip.compress(data))


def dom_start_tags(html: bytes) -> int:
    """静态统计 DOM 起始标签数（`<tag`；HTML 注释与 DOCTYPE 的 `<` 不计入）。

    这是首屏静态上界：动态数据行的规模由服务端有界列表（首屏快照 ≤ 50 条）
    与分页约束，不在本脚本动态统计范围。
    """
    without_comments = HTML_COMMENT.sub(b"", html)
    return len(START_TAG.findall(without_comments))


def check_js(name: str, data: bytes) -> list[str]:
    """JS 审计：预算、禁用标记与 DOM 注入 API。"""
    failures: list[str] = []
    lowered = data.lower()
    for marker in FORBIDDEN_MARKERS.get(".js", []):
        if marker in lowered:
            failures.append(f"{name} 含禁止标记：{marker.decode()}")
    if gzip_size(data) > JS_BUDGET_BYTES:
        failures.append(f"{name} gzip {gzip_size(data)} 字节超过预算 {JS_BUDGET_BYTES}（FR-618）")
    for marker in DOM_INJECTION_MARKERS:
        if marker in lowered:
            failures.append(
                f"{name} 含 {marker}：数据渲染只允许 textContent／属性赋值（XSS 防线，ADR-041）"
            )
    return failures


def check_html(name: str, data: bytes) -> list[str]:
    """HTML 审计：禁用标记与 DOM 节点预算。"""
    failures: list[str] = []
    lowered = data.lower()
    for marker in FORBIDDEN_MARKERS.get(".html", []):
        if marker in lowered:
            failures.append(f"{name} 含禁止标记：{marker.decode()}")
    nodes = dom_start_tags(data)
    if nodes > DOM_BUDGET_NODES:
        failures.append(
            f"{name} DOM 起始标签 {nodes} 超过预算 {DOM_BUDGET_NODES}（FR-618／ADR-041）"
        )
    return failures


def check_css(name: str, data: bytes) -> list[str]:
    """CSS 审计：体积与第三方字体。"""
    failures: list[str] = []
    if gzip_size(data) > CSS_BUDGET_BYTES:
        failures.append(f"{name} gzip {gzip_size(data)} 字节超过预算 {CSS_BUDGET_BYTES}（FR-618）")
    if b"@font-face" in data.lower():
        failures.append(f"{name} 含 @font-face：禁止第三方字体（FR-618）")
    return failures


def selftest() -> int:
    """负向自测：DOM 计数与注入禁令必须能对违规夹具给出失败（门禁有效性自检）。"""
    ok = dom_start_tags(b"<div><span></span></div>") == 2
    ok = ok and dom_start_tags(b"<!-- <div> --><!DOCTYPE html>") == 0
    ok = ok and dom_start_tags(b"<li>" * (DOM_BUDGET_NODES + 1)) > DOM_BUDGET_NODES
    ok = ok and dom_start_tags(b"<div>") <= DOM_BUDGET_NODES
    ok = ok and bool(check_js("fake.js", b"el.innerHTML = x;"))
    ok = ok and bool(check_js("fake.js", b"el.insertAdjacentHTML('beforeend', x);"))
    ok = ok and not check_js("fake.js", b"el.textContent = x;")
    ok = ok and not check_html("fake.html", b"<p>ok</p>")
    if not ok:
        print("FAIL: selftest 未通过")
        return 1
    print("selftest ok：DOM 计数与注入禁令可判定违规")
    return 0


def main() -> int:
    if "--selftest" in sys.argv:
        return selftest()
    if not FRONTEND.is_dir():
        print(f"前端目录不存在：{FRONTEND}")
        return 1

    failures: list[str] = []
    sizes: dict[str, int] = {}
    for name in [
        "app.js",
        "suspend.js",
        "settings.js",
        "index.html",
        "settings.html",
        "styles.css",
    ]:
        path = FRONTEND / name
        if not path.is_file():
            failures.append(f"缺少 {name}")
            continue
        data = path.read_bytes()
        if path.suffix == ".js":
            failures += check_js(name, data)
        elif path.suffix == ".html":
            failures += check_html(name, data)
        elif path.suffix == ".css":
            failures += check_css(name, data)
        if path.suffix in (".js", ".css"):
            sizes[name] = gzip_size(data)

    # NFR-212：运行时路径不得创建/探测 GPU 上下文——app.js 与 settings.js 不得调用
    # getContext/probeGpu（suspend.js 的 probeGpu 是显式启用的能力探测，未被运行时路径
    # 调用；见 app.js 注释）。
    for runtime_js in ["app.js", "settings.js"]:
        path = FRONTEND / runtime_js
        if not path.is_file():
            continue
        text = path.read_bytes().lower()
        # 标记按“调用形态”匹配（允许注释中提及函数名，不允许实际调用）。
        for marker in [b"getcontext(", b"probegpu("]:
            if marker in text:
                failures.append(
                    f"{runtime_js} 含 {marker.decode()}：运行时路径不得创建/探测 GPU 上下文（NFR-212）"
                )

    for banned in ["package.json", "node_modules"]:
        if (FRONTEND / banned).exists():
            failures.append(f"前端目录含 {banned}：违反 ADR-014 无 node 构建链")

    if failures:
        for failure in failures:
            print(f"FAIL: {failure}")
        return 1
    pages = {
        name: dom_start_tags((FRONTEND / name).read_bytes())
        for name in ["index.html", "settings.html"]
    }
    print(
        f"前端基线通过：JS gzip app {sizes.get('app.js', 0)} B / "
        f"settings {sizes.get('settings.js', 0)} B（各 ≤ {JS_BUDGET_BYTES}）/ "
        f"CSS gzip {sizes.get('styles.css', 0)} B（≤ {CSS_BUDGET_BYTES}）/ "
        f"DOM 起始标签 index {pages['index.html']} · settings {pages['settings.html']}"
        f"（各 ≤ {DOM_BUDGET_NODES}），无违例"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
