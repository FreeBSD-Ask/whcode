#!/usr/bin/env python3
"""EOL 与编码门禁：全库文本文件必须 LF、UTF-8 无 BOM（CI 与本地同源）。

规则（用户要求 + repository-layout.md）：
  - 换行符一律 LF（.gitattributes 与 .editorconfig 双重强制）；
  - UTF-8 且禁止 BOM。

行为：
  1. 枚举 git 跟踪的全部文件；**二进制文件跳过**（前 8000 字节含 NUL 即判为二进制，
     例如证据截图 PNG——二进制的 CR/LF 字节不是换行符，不参与审计）；
  2. 检测文本文件的 CRLF 与 UTF-8 BOM，输出违规清单；
  3. `--fix` 时就地归一化（CRLF→LF、去 BOM），不动内容其余字节。

退出码：无违规或 `--fix` 成功为 0；存在违规且未修复为 1（CI 门禁语义）。

用法：
    python .github/scripts/check-eol.py           # 只审计（CI 用）
    python .github/scripts/check-eol.py --fix     # 审计并修复
"""

from __future__ import annotations

import pathlib
import subprocess
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]
BOM = b"\xef\xbb\xbf"


def tracked_files() -> list[pathlib.Path]:
    result = subprocess.run(
        ["git", "ls-files"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=60,
    )
    if result.returncode != 0:
        sys.exit(f"git ls-files 失败：{result.stderr}")
    return [REPO_ROOT / line.strip() for line in result.stdout.splitlines() if line.strip()]


def is_binary(data: bytes) -> bool:
    """二进制判定：前 8000 字节含 NUL（截图 PNG 等）；二进制不参与换行符审计。"""
    return b"\x00" in data[:8000]


def main() -> int:
    fix = "--fix" in sys.argv[1:]
    crlf_files: list[str] = []
    bom_files: list[str] = []
    fixed = 0
    skipped_binary = 0
    total = 0

    for path in tracked_files():
        total += 1
        if not path.is_file():
            continue
        data = path.read_bytes()
        if is_binary(data):
            skipped_binary += 1
            continue
        has_crlf = b"\r\n" in data
        has_bom = data.startswith(BOM)
        if has_crlf:
            crlf_files.append(str(path.relative_to(REPO_ROOT)))
        if has_bom:
            bom_files.append(str(path.relative_to(REPO_ROOT)))
        if fix and (has_crlf or has_bom):
            if has_bom:
                data = data[len(BOM):]
            if has_crlf:
                data = data.replace(b"\r\n", b"\n")
            path.write_bytes(data)
            fixed += 1

    print(f"跟踪文件总数：{total}（跳过二进制 {skipped_binary}）")
    print(f"CRLF 违规：{len(crlf_files)}")
    for name in crlf_files:
        print(f"  {name}")
    print(f"BOM 违规：{len(bom_files)}")
    for name in bom_files:
        print(f"  {name}")
    if fix:
        print(f"已修复：{fixed}")
        return 0 if not (crlf_files or bom_files) or fixed == len(set(crlf_files + bom_files)) else 1
    return 0 if not (crlf_files or bom_files) else 1


if __name__ == "__main__":
    sys.exit(main())
