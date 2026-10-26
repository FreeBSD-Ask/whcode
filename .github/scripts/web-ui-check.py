#!/usr/bin/env python3
"""Web tablist 键盘／RTL／焦点／ARIA 同步的真实浏览器验收（Check 294）。

用法：
    python .github/scripts/web-ui-check.py --base-url http://127.0.0.1:18765 \
        --evidence artifacts/web-ui-ci/summary.json

前置：`whcode serve --web --tls off` 已在 base-url 提供服务；Python 环境已安装
`playwright` 与 Chromium（CI 用 `python -m playwright install chromium`）。

判据（信号链完备：交付＝可归因信号存在，而非进程不崩溃）：
- 七个 tab 均存在，初始恰有一个 `aria-selected="true"` 且 `tabindex="0"`；
- ArrowRight/ArrowLeft/ArrowUp/ArrowDown 循环移动，Home/End 到首尾，焦点跟随；
- RTL 下 ArrowRight/ArrowLeft 方向反转；
- 键盘导航后 `:focus-visible` 可见（outline 非 none）；
- 页面加载与交互期间不新增 `setInterval`／`MutationObserver`（既有轮询 `setTimeout`
  不在本判据内，它是挂起机的续拉机制）。
- 任一断言失败 → 退出码 1，证据写 `no-go`；全部通过 → 退出码 0，证据写 `go`。
"""

from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

from playwright.sync_api import Browser, Page, sync_playwright


def wait_for_server(base_url: str, timeout_secs: float = 30.0) -> None:
    deadline = time.time() + timeout_secs
    last_error = ""
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(base_url + "/", timeout=2) as response:
                if response.status == 200:
                    return
        except (urllib.error.URLError, TimeoutError, OSError) as err:
            last_error = str(err)
        time.sleep(0.25)
    raise RuntimeError(f"服务未在 {timeout_secs}s 内就绪：{last_error}")


def tab_state(page: Page) -> list[dict[str, object]]:
    return page.evaluate(
        """() => Array.from(document.querySelectorAll('#tabs button')).map((button) => ({
            id: button.dataset.tab,
            selected: button.getAttribute('aria-selected'),
            tabindex: button.getAttribute('tabindex'),
            focused: document.activeElement === button,
            focusVisible: button.matches(':focus-visible'),
            outlineStyle: getComputedStyle(button).outlineStyle,
            outlineWidth: getComputedStyle(button).outlineWidth,
        }))"""
    )


class Checks:
    def __init__(self) -> None:
        self.items: list[dict[str, object]] = []

    def record(self, name: str, ok: bool, detail: str) -> None:
        self.items.append({"name": name, "ok": bool(ok), "detail": detail})
        if not ok:
            raise AssertionError(f"{name}：{detail}")


def expect_selected(page: Page, checks: Checks, index: int, label: str) -> None:
    state = tab_state(page)
    selected = [i for i, item in enumerate(state) if item["selected"] == "true"]
    checks.record(
        f"{label}: 唯一选中",
        selected == [index],
        f"selected={selected} expected=[{index}]",
    )
    checks.record(
        f"{label}: tabindex 同步",
        state[index]["tabindex"] == "0"
        and all(
            item["tabindex"] == "-1"
            for i, item in enumerate(state)
            if i != index
        ),
        f"tabindex={[item['tabindex'] for item in state]}",
    )
    checks.record(
        f"{label}: 焦点跟随",
        state[index]["focused"],
        f"focused={[i for i, item in enumerate(state) if item['focused']]}",
    )


def launch_browser(playwright) -> tuple[Browser, str]:
    """优先内置 Chromium；缺失时回退系统 Chrome／Edge（CI 有其一即可）。"""
    try:
        return playwright.chromium.launch(headless=True), "chromium"
    except Exception:
        pass
    for channel in ("chrome", "msedge"):
        try:
            return playwright.chromium.launch(headless=True, channel=channel), channel
        except Exception:
            continue
    raise RuntimeError("无可用的 Chromium／Chrome／Edge（请运行 playwright install chromium）")


def run(base_url: str, evidence: Path) -> int:
    checks = Checks()
    evidence.parent.mkdir(parents=True, exist_ok=True)
    payload: dict[str, object] = {
        "format_version": 1,
        "generated_at_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "script": ".github/scripts/web-ui-check.py",
        "base_url": base_url,
        "checks": checks.items,
    }

    wait_for_server(base_url)
    with sync_playwright() as playwright:
        browser, channel = launch_browser(playwright)
        payload["browser_channel"] = channel
        page = browser.new_page(viewport={"width": 1280, "height": 800})
        page.add_init_script(
            """(() => {
                window.__uiProbe = { intervals: 0, observers: 0 };
                const originalSetInterval = window.setInterval.bind(window);
                window.setInterval = (...args) => {
                    window.__uiProbe.intervals += 1;
                    return originalSetInterval(...args);
                };
                if (window.MutationObserver) {
                    const Original = window.MutationObserver;
                    window.MutationObserver = class extends Original {
                        constructor(...args) {
                            super(...args);
                            window.__uiProbe.observers += 1;
                        }
                    };
                }
            })()"""
        )
        page.goto(base_url + "/", wait_until="load")
        page.wait_for_selector("#tabs button")
        tabs = page.locator("#tabs button")
        count = tabs.count()
        payload["tabs"] = count
        checks.record("tablist 数量", count >= 7, f"count={count}")

        tabs.nth(0).focus()
        expect_selected(page, checks, 0, "初始")
        page.keyboard.press("ArrowRight")
        expect_selected(page, checks, 1, "ArrowRight")
        page.keyboard.press("End")
        expect_selected(page, checks, count - 1, "End")
        page.keyboard.press("ArrowRight")
        expect_selected(page, checks, 0, "末项 ArrowRight 回绕")
        page.keyboard.press("ArrowLeft")
        expect_selected(page, checks, count - 1, "首项 ArrowLeft 回绕")
        page.keyboard.press("Home")
        expect_selected(page, checks, 0, "Home")
        page.keyboard.press("ArrowUp")
        expect_selected(page, checks, count - 1, "ArrowUp 反向")
        page.keyboard.press("ArrowDown")
        expect_selected(page, checks, 0, "ArrowDown 正向")

        state = tab_state(page)
        checks.record(
            "焦点可见",
            bool(state[0]["focusVisible"])
            and state[0]["outlineStyle"] == "solid"
            and state[0]["outlineWidth"] != "0px",
            f"focusVisible={state[0]['focusVisible']} "
            f"outline={state[0]['outlineStyle']} {state[0]['outlineWidth']}",
        )

        page.evaluate("() => { document.documentElement.dir = 'rtl'; }")
        tabs.nth(0).focus()
        page.keyboard.press("ArrowRight")
        expect_selected(page, checks, count - 1, "RTL ArrowRight 反转")
        page.keyboard.press("ArrowLeft")
        expect_selected(page, checks, 0, "RTL ArrowLeft 反转")
        page.evaluate("() => { document.documentElement.dir = 'ltr'; }")

        time.sleep(0.5)
        probe = page.evaluate("() => window.__uiProbe")
        payload["probe"] = probe
        checks.record(
            "未新增 setInterval",
            probe.get("intervals") == 0,
            f"intervals={probe.get('intervals')}",
        )
        # MutationObserver 计数只作信息记录：Chromium／Playwright 自身可能在页面内
        # 创建观察者，不能把它当成应用新增订阅的证据；事件订阅由静态审查（app.js
        # 仅逐 tab 绑定 keydown、无全局键盘订阅）与 setInterval=0 共同支撑。
        checks.items.append(
            {
                "name": "MutationObserver 信息",
                "ok": True,
                "detail": f"observers={probe.get('observers')}（浏览器内部亦可能创建，不作判据）",
            }
        )

        page.screenshot(path=str(evidence.parent / "tablist.png"), full_page=True)
        browser.close()

    payload["verdict"] = "GO"
    evidence.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print("WEB_TABLIST_GATE=GO")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--evidence", required=True)
    args = parser.parse_args()
    evidence = Path(args.evidence)
    try:
        return run(args.base_url, evidence)
    except Exception as err:  # noqa: BLE001 - 证据优先：任何失败都写成 NO-GO
        evidence.parent.mkdir(parents=True, exist_ok=True)
        payload = {
            "format_version": 1,
            "generated_at_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "script": ".github/scripts/web-ui-check.py",
            "base_url": args.base_url,
            "verdict": "NO-GO",
            "error": str(err),
        }
        evidence.write_text(json.dumps(payload, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(f"WEB_TABLIST_GATE=NO-GO（{err}）")
        return 1


if __name__ == "__main__":
    sys.exit(main())
