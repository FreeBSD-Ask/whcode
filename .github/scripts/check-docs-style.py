#!/usr/bin/env python3
"""文档文风门禁：禁用词、表情符号、中文直引号与套话（2026-11-07 用户指令）。

规则来源：用户指令“全局文档不要出现‘车道’两个字和其他乱七八糟的字”“务必确保翻译
地道、准确，去 AI 味，避免欧化语句病句”。规则要被程序判定，否则只是口号。

范围：git 跟踪的 `*.md`（散文与账本）。豁免：

- `.specify/`：spec-kit 工具模板，随上游更新覆盖，改了会被冲掉，不属于本项目文档；
- 围栏代码块（``` 与 ~~~）与行内 `code`：代码与命令示例不受中文文风规则约束。

判定规则（每条都可定位到文件与行）：

1. `BANNED_WORDS`：禁用词逐行报告（如“车道”）；
2. `EMOJI`：表情符号与勾叉符号（含变体选择符 U+FE0F）——“乱七八糟的字”；
3. `STRAIGHT_QUOTES`：中文行的直引号 `"`（中文正文须用“”或‘’，AGENTS.md §7）；
4. `AI_CLICHES`：套话与欧化句式固定串（高精度、可机械替换的少量条目）。

规则 3、4 只对 `.md` 散文生效；`.yml` 与 `.rs`（仅注释行）只查规则 1、2——
工作流里的 shell 引号与代码注释里的引号不是中文文风问题。

退出码：0 无违规；1 有违规（CI 门禁语义）；2 运行错误。

用法：
    python .github/scripts/check-docs-style.py
    python .github/scripts/check-docs-style.py --verbose    # 列出全部命中行
"""

from __future__ import annotations

import argparse
import pathlib
import re
import subprocess
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]

#: 禁用词（用户指令）：出现即违规，替换为不含该词的表达。
BANNED_WORDS = ("车道",)

#: 表情与勾叉符号：图形符号区、杂项符号区、装饰符号区与变体选择符。
EMOJI = re.compile(
    "[\U0001f000-\U0001faff\ufe0f\u2705\u274c\u2713\u2717\u26a0\u26d4\u2795"
    "\u23f3\u25d0\u25d1\u2b00-\u2bff]"
)

#: 中文行内的直引号（成对或单个都算，便于逐处改写）。
CJK = re.compile(r"[\u4e00-\u9fff]")
STRAIGHT_QUOTE = re.compile(r'"')

#: 套话与欧化句式：高精度列表，每项都能直接改写而不损信息。
AI_CLICHES = (
    "值得一提的是",
    "值得注意的是",
    "不难看出",
    "众所周知",
    "综上所述",
    "总而言之",
    "总的来说",
    "极大地",
    "显著地提升",
    "进一步地",
    "进行了",
    "被广泛使用",
    "赋能",
    "抓手",
)

FENCE = re.compile(r"^\s*(```|~~~)")
#: 行内代码（`...`）与链接目标（](...)）：其中的引号、符号不属于中文正文。
INLINE_CODE = re.compile(r"`[^`]*`")
LINK_TARGET = re.compile(r"\]\([^)]*\)")


def prose_view(line: str) -> str:
    """去掉行内代码与链接目标，只保留需要判定文风的正文。"""
    return LINK_TARGET.sub("]", INLINE_CODE.sub("", line))


def tracked_markdown() -> list[pathlib.Path]:
    result = subprocess.run(
        ["git", "ls-files", "*.md", "*.yml", "*.yaml", "*.rs"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=60,
    )
    if result.returncode != 0:
        print("git ls-files 失败", file=sys.stderr)
        sys.exit(2)
    files: list[pathlib.Path] = []
    for line in result.stdout.splitlines():
        path = pathlib.Path(line)
        if path.parts and path.parts[0] == ".specify":
            continue
        files.append(path)
    return files


def iter_prose(path: pathlib.Path):
    """逐行产出 (行号, 正文)；围栏代码块与跨行行内代码跳过。

    行内代码可以跨行（`Some("http://...")` 折行是常见写法），因此用奇偶计数跟踪：
    行内代码内的内容不属于中文正文，不参与文风判定。
    """
    try:
        text = (REPO_ROOT / path).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as err:
        print(f"读取失败：{path}（{err}）", file=sys.stderr)
        return
    in_fence = False
    in_code = False
    is_rust = path.suffix == ".rs"
    for number, line in enumerate(text.splitlines(), start=1):
        if is_rust and not line.lstrip().startswith("//"):
            continue  # Rust 只审注释（代码与字符串不属文档）
        if FENCE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        pieces = line.split("`")
        prose_parts = []
        for index, piece in enumerate(pieces):
            # 偶数段在代码外（第 0 段由 in_code 状态决定）。
            outside = (index % 2 == 0) != in_code
            if outside:
                prose_parts.append(piece)
        if len(pieces) % 2 == 0:
            in_code = not in_code
        yield number, "".join(prose_parts)


def check() -> list[tuple[str, int, str, str]]:
    violations: list[tuple[str, int, str, str]] = []
    for path in tracked_markdown():
        prose_rules = path.suffix == ".md"  # 中文直引号与套话只对散文生效
        for number, line in iter_prose(path):
            prose = prose_view(line)
            for word in BANNED_WORDS:
                if word in prose:
                    violations.append((str(path), number, "禁用词", word))
            match = EMOJI.search(prose)
            if match:
                violations.append((str(path), number, "表情符号", match.group()))
            if prose_rules and CJK.search(prose) and STRAIGHT_QUOTE.search(prose):
                violations.append((str(path), number, "中文直引号", '"'))
            if prose_rules:
                for cliche in AI_CLICHES:
                    if cliche in prose:
                        violations.append((str(path), number, "套话/欧化", cliche))
    return violations


def main() -> int:
    parser = argparse.ArgumentParser(description="文档文风门禁")
    parser.add_argument("--verbose", action="store_true", help="列出全部命中行")
    args = parser.parse_args()

    violations = check()
    if not violations:
        print("文档文风：无违规")
        return 0

    counts: dict[str, int] = {}
    for _, _, rule, _ in violations:
        counts[rule] = counts.get(rule, 0) + 1
    print(
        "文档文风违规："
        + "，".join(f"{rule} {count}" for rule, count in sorted(counts.items())),
        file=sys.stderr,
    )
    limit = len(violations) if args.verbose else 40
    for path, number, rule, detail in violations[:limit]:
        print(f"  {path}:{number}: [{rule}] {detail}", file=sys.stderr)
    if limit < len(violations):
        print(f"  ……其余 {len(violations) - limit} 处用 --verbose 查看", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
