#!/usr/bin/env python3
"""从内建 properties 目录生成 gettext PO 源文件（FR-615，T276）。

编译期内建目录（`crates/whcode-domain/src/i18n/<locale>.properties`）是运行时事实源；
`po/` 下的 PO 文件是面向翻译者的**同源派生**副本：键表与译文必须逐条一致，由
`whcode-domain` 的契约测试（`po_catalogs_match_properties`）强制，漂移即 CI 失败。

用法：

    python .github/scripts/i18n-po.py            # 生成/覆盖 PO 文件
    python .github/scripts/i18n-po.py --check    # 只校验是否已同步（供本地复跑）

退出码：0 成功或已同步；1 需要重新生成（--check）；2 参数或读取错误。

为什么不让运行时直接读 PO：内建目录经 `include_str!` 嵌入，部署零文件依赖（本地优先）；
PO 只在安装了翻译文件时才覆盖内建（`I18n::load_with_po`，见 domain 文档）。
"""

from __future__ import annotations

import argparse
import pathlib
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]
I18N_DIR = REPO_ROOT / "crates" / "whcode-domain" / "src" / "i18n"
LOCALES = (
    "zh-CN",
    "zh-TW",
    "en",
    "ja",
    "ko",
    "ru",
    "fr",
    "es",
    "de",
    "ar",
    "hi",
    "ur",
    "bn",
)

HEADER = """# {locale} translation catalog for WHCode (FR-615).
# Generated from {source} by .github/scripts/i18n-po.py; do not edit by hand.
# Edit the properties file instead, then regenerate — a contract test fails on drift.
#
msgid ""
msgstr ""
"Project-Id-Version: whcode 0.1.0\\n"
"Report-Msgid-Bugs-To: https://github.com/FreeBSD-Ask/whcode/issues\\n"
"Language: {locale}\\n"
"MIME-Version: 1.0\\n"
"Content-Type: text/plain; charset=UTF-8\\n"
"Content-Transfer-Encoding: 8bit\\n"

"""


def read_properties(path: pathlib.Path) -> dict[str, str]:
    """解析 `key = value`（`#` 注释；值两侧的成对引号去除，与 Rust 侧一致）。"""
    entries: dict[str, str] = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        key, sep, value = line.partition("=")
        if not sep:
            continue
        value = value.strip()
        if len(value) >= 2 and value.startswith('"') and value.endswith('"'):
            value = value[1:-1]
        entries[key.strip()] = value
    return entries


def escape(value: str) -> str:
    out = []
    for ch in value:
        if ch == "\\":
            out.append("\\\\")
        elif ch == '"':
            out.append('\\"')
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\t":
            out.append("\\t")
        else:
            out.append(ch)
    return "".join(out)


def render(locale: str, entries: dict[str, str]) -> str:
    source = f"i18n/{locale}.properties"
    parts = [HEADER.format(locale=locale, source=source)]
    for key in sorted(entries):
        parts.append(f'msgid "{escape(key)}"\nmsgstr "{escape(entries[key])}"\n\n')
    return "".join(parts)


def main() -> int:
    parser = argparse.ArgumentParser(description="生成/校验 i18n PO 源文件")
    parser.add_argument(
        "--check",
        action="store_true",
        help="只校验 PO 与 properties 是否同步（不同步返回 1）",
    )
    args = parser.parse_args()

    pending: list[str] = []
    for locale in LOCALES:
        properties = I18N_DIR / f"{locale}.properties"
        if not properties.is_file():
            print(f"缺少内建目录：{properties}", file=sys.stderr)
            return 2
        po_path = I18N_DIR / "po" / f"{locale}.po"
        rendered = render(locale, read_properties(properties))
        if args.check:
            if not po_path.is_file() or po_path.read_text(encoding="utf-8") != rendered:
                pending.append(str(po_path.relative_to(REPO_ROOT)))
            continue
        po_path.parent.mkdir(parents=True, exist_ok=True)
        with open(po_path, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(rendered)
        print(f"已写入 {po_path.relative_to(REPO_ROOT)}")

    if args.check:
        if pending:
            print("需要重新生成（python .github/scripts/i18n-po.py）：", file=sys.stderr)
            for path in pending:
                print(f"  {path}", file=sys.stderr)
            return 1
        print("PO 源文件已与内建目录同步")
    return 0


if __name__ == "__main__":
    sys.exit(main())
