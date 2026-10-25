#!/usr/bin/env python3
"""发布包 tag／版本／SHA256 校验（build-release.yml 的发布门禁；ADR-039／Task T183）。

为什么单独一层：构建、打包、上传、发布是不同进程，任何一步都可能让
“文件名里的版本”“tag 版本”“实际字节”三者脱钩。Release 一旦公开就会被镜像与缓存，
故上传前用一次独立复算把三者对齐：

  1. 目录内 `whcode-*-windows-amd64.zip` 必须唯一——多个说明打包污染，缺失说明
     打包未执行；且必须非空且是有效 zip 归档（0 字节或非 zip 一律失败）；
  2. 文件名中的 `<version>` 与 tag（去掉 `v` 前缀）一致，或 tag 形如
     `<version>-<预发布标识>`（如 `v0.1.0-rc.1`）；
  3. `.sha256` 记录的哈希与重新计算的 zip 哈希一致；记录必须恰为 sha256sum 的两个
     字段（64 位十六进制与文件名，文件名允许前缀 `*`）且文件名等于当前包；
  4. 传入 `--record` 时，与 `package-windows.py` 写出的安装验证记录首部行
     `package = <zip>（<字节> 字节，sha256=<64hex>）` 交叉校验包名、字节数与
     sha256——任一不符即失败，发布作业据此中止。

任一条不满足即退出 1，并打印 `tag = ...`／`package_version = ...` 等可归因字段；
不以 0 或空值冒充通过（docs/guides/signal-chain-completeness.md）。

只用 Python 标准库；写出文件为 UTF-8 无 BOM、LF，内容为 sha256sum 兼容格式。

用法：
    python .github/scripts/check-release.py --dir target/dist --write
    python .github/scripts/check-release.py --dir dist --tag v0.1.0
    python .github/scripts/check-release.py --dir dist --tag v0.1.0 \
        --record artifacts/install-verify-windows-amd64.txt
"""

from __future__ import annotations

import argparse
import hashlib
import pathlib
import re
import sys
import zipfile

#: 发布包命名由 package-windows.py 的 `whcode-{version}-windows-amd64.zip` 决定。
ZIP_PATTERN = re.compile(r"^whcode-(?P<version>.+)-windows-amd64\.zip$")
HEX_PATTERN = re.compile(r"^[0-9a-f]{64}$")
#: 安装验证记录首部行：package-windows.py 的 write_record() 决定此格式。
RECORD_PATTERN = re.compile(
    r"^package\s*=\s*(?P<name>[^（）\s]+)\s*（(?P<bytes>\d+)\s*字节，sha256=(?P<digest>[0-9a-fA-F]{64})）$"
)


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def find_zip(directory: pathlib.Path) -> tuple[pathlib.Path | None, str | None]:
    """在目录中定位唯一发布包；返回 (zip, 问题描述)。"""
    if not directory.is_dir():
        return None, f"{directory} 不存在（打包产物目录缺失）"
    candidates = sorted(path for path in directory.glob("whcode-*-windows-amd64.zip") if path.is_file())
    if not candidates:
        return None, f"{directory} 下未找到 whcode-*-windows-amd64.zip（打包是否已执行？）"
    if len(candidates) > 1:
        names = "、".join(path.name for path in candidates)
        return None, f"{directory} 下有多个发布包，必须唯一：{names}"
    return candidates[0], None


def checksum_path(zip_path: pathlib.Path) -> pathlib.Path:
    return zip_path.with_name(zip_path.name + ".sha256")


def validate_zip(zip_path: pathlib.Path) -> list[str]:
    """发布包必须非空且是有效 zip：0 字节与损坏文件都不得冒充发布产物。"""
    if zip_path.stat().st_size == 0:
        return [f"{zip_path.name}：发布包 zip 为空（0 字节），拒绝作为发布产物"]
    if not zipfile.is_zipfile(zip_path):
        return [f"{zip_path.name}：发布包 zip 不是有效归档（zipfile.is_zipfile 为假）"]
    return []


def record_problems(record_path: pathlib.Path, zip_path: pathlib.Path, digest: str) -> list[str]:
    """把安装验证记录首部行与当前包逐项对齐；记录缺失或无法解析也算失败。"""
    if not record_path.is_file():
        return [f"缺少安装验证记录 {record_path}（--record 指定的文件不存在）"]
    try:
        text = record_path.read_text(encoding="utf-8")
    except OSError as error:
        return [f"无法读取安装验证记录 {record_path}：{error}"]

    line = next((item.strip() for item in text.splitlines() if item.strip().startswith("package =")), None)
    if line is None:
        return [f"{record_path.name} 缺少 package = … 首部行，无法交叉校验包名、字节数与 sha256"]
    match = RECORD_PATTERN.match(line)
    if match is None:
        return [f"{record_path.name} 的 package 首部行无法解析：{line}"]

    problems: list[str] = []
    recorded_name = match.group("name")
    if recorded_name != zip_path.name:
        problems.append(f"安装验证记录包名 {recorded_name} 与当前包 {zip_path.name} 不一致")
    recorded_bytes = int(match.group("bytes"))
    actual_bytes = zip_path.stat().st_size
    if recorded_bytes != actual_bytes:
        problems.append(f"安装验证记录字节数 {recorded_bytes} 与实测 {actual_bytes} 不一致")
    recorded_digest = match.group("digest").lower()
    if recorded_digest != digest:
        problems.append(f"安装验证记录 sha256 {recorded_digest} 与实测 {digest} 不一致")
    return problems


def report(problems: list[str], tag: str, package_version: str, digest: str) -> int:
    for problem in problems:
        print(f"FAIL: {problem}")
    print(f"tag = {tag or '未取得'}")
    print(f"package_version = {package_version or '未取得'}")
    print(f"sha256 = {digest or '未取得'}")
    return 1 if problems else 0


def write_mode(directory: pathlib.Path) -> int:
    zip_path, problem = find_zip(directory)
    if zip_path is None:
        return report([str(problem)], "", "", "")

    problems = validate_zip(zip_path)
    if problems:
        return report(problems, "", "", "")

    digest = sha256(zip_path)
    target = checksum_path(zip_path)
    target.write_text(f"{digest}  {zip_path.name}\n", encoding="utf-8", newline="\n")
    print(f"package = {zip_path.name}")
    print(f"sha256 = {digest}")
    print(f"已写入：{target}")
    return 0


def tag_mode(directory: pathlib.Path, tag: str, record: pathlib.Path | None) -> int:
    zip_path, problem = find_zip(directory)
    if zip_path is None:
        return report([str(problem)], tag, "", "")

    match = ZIP_PATTERN.match(zip_path.name)
    version = match.group("version") if match else ""
    problems: list[str] = validate_zip(zip_path)
    if problems:
        # 无效发布包没有可信的实测哈希，宁可打印“未取得”，也不把空文件的哈希当结论。
        return report(problems, tag, version, "")
    digest = sha256(zip_path)

    # tag 的前缀 `v` 只用于人类可读；版本比对一律用去前缀后的字符串。
    tag_version = tag[1:] if tag.startswith("v") else tag
    if version == "":
        problems.append(f"无法从 {zip_path.name} 解析版本号")
    elif not (tag_version == version or tag_version.startswith(f"{version}-")):
        problems.append(f"tag 版本 {tag_version} 与包版本 {version} 不一致（预发布须为 {version}-…）")

    recorded_file = checksum_path(zip_path)
    if not recorded_file.is_file():
        problems.append(f"缺少 {recorded_file.name}（先用 --write 生成）")
    else:
        parts = recorded_file.read_text(encoding="utf-8").split()
        if len(parts) != 2:
            problems.append(
                f"{recorded_file.name} 不是 sha256sum 格式（应恰为 64 位十六进制与文件名两个字段，"
                f"实际 {len(parts)} 个）"
            )
        else:
            recorded, recorded_name = parts
            if not HEX_PATTERN.match(recorded):
                problems.append(f"{recorded_file.name} 不是 sha256sum 格式（首字段非 64 位十六进制）")
            elif recorded != digest:
                problems.append(f"SHA256 不一致：记录 {recorded}，实测 {digest}")
            if recorded_name.lstrip("*") != zip_path.name:
                problems.append(f"{recorded_file.name} 记录的文件名 {recorded_name} 不是 {zip_path.name}")

    if record is not None:
        problems.extend(record_problems(record, zip_path, digest))

    code = report(problems, tag, version, digest)
    if code == 0:
        suffix = f"，安装验证记录 {record.name} 与包一致" if record is not None else ""
        print(f"发布校验通过：tag={tag} 与包版本 {version} 一致，SHA256 与记录一致{suffix}")
    return code


def main() -> int:
    # Windows 控制台代码页非 UTF-8 时中文输出会抛 UnicodeEncodeError，与其余门禁脚本同处理。
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    parser = argparse.ArgumentParser(description="发布包 tag／版本／SHA256 校验")
    parser.add_argument("--dir", required=True, help="发布包所在目录（如 target/dist／dist）")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--write", action="store_true", help="写入 <zip>.sha256")
    mode.add_argument("--tag", default=None, help="校验 tag 与包版本、SHA256 一致")
    parser.add_argument(
        "--record",
        default=None,
        metavar="PATH",
        help="安装验证记录（与 --tag 同用：交叉校验包名、字节数与 sha256；--write 模式不需要）",
    )
    args = parser.parse_args()

    directory = pathlib.Path(args.dir)
    if args.write:
        return write_mode(directory)
    return tag_mode(directory, args.tag, pathlib.Path(args.record) if args.record else None)


if __name__ == "__main__":
    sys.exit(main())
