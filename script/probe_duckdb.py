#!/usr/bin/env python3
"""临时探测脚本（非交付物）：确认 DuckDB 官方预编译包可获取并核对归档结构。

背景：crates.io 的 duckdb crate 1.10506.1 对应 DuckDB 1.5.6，
libduckdb-sys 的下载 URL 形如
  https://github.com/duckdb/duckdb/releases/download/v<ver>/libduckdb-<platform>.zip

本脚本只做三件事：
  1. 解析出 DuckDB 版本号；
  2. 尝试下载 libduckdb-windows-amd64.zip（失败时打印 HTTP 状态）；
  3. 列出归档成员并计算 SHA256，供后续 manifest 固定。

产物留在 script/ 下，不清理（用户规则）。
"""

from __future__ import annotations

import hashlib
import pathlib
import sys
import urllib.error
import urllib.request
import zipfile

CRATE_VERSION = "1.10506.1"
PLATFORMS = {
    "windows-amd64": "libduckdb-windows-amd64.zip",
    "linux-amd64": "libduckdb-linux-amd64.zip",
    "osx-universal": "libduckdb-osx-universal.zip",
}


def duckdb_version(crate_version: str) -> str:
    """复刻 libduckdb-sys 的版本映射：1.MAJOR_MINOR_PATCH.x -> X.Y.Z。"""
    encoded = int(crate_version.split(".")[1])
    return f"{encoded // 10_000}.{(encoded // 100) % 100}.{encoded % 100}"


def main() -> int:
    version = duckdb_version(CRATE_VERSION)
    out_dir = pathlib.Path(__file__).resolve().parent
    print(f"duckdb crate {CRATE_VERSION} -> DuckDB {version}")

    for platform, archive in PLATFORMS.items():
        url = (
            f"https://github.com/duckdb/duckdb/releases/download/v{version}/{archive}"
        )
        print(f"\n=== {platform} ===\n{url}")
        try:
            with urllib.request.urlopen(url, timeout=60) as resp:
                status = resp.status
                length = resp.headers.get("Content-Length")
        except urllib.error.HTTPError as err:
            print(f"HTTP {err.code} {err.reason}")
            continue
        except Exception as err:  # noqa: BLE001
            print(f"请求失败：{err}")
            continue

        print(f"HTTP {status}；Content-Length={length}")
        if platform != "windows-amd64":
            continue

        target = out_dir / archive
        if not target.exists():
            with urllib.request.urlopen(url, timeout=300) as resp:
                target.write_bytes(resp.read())
        digest = hashlib.sha256(target.read_bytes()).hexdigest()
        print(f"下载到 {target}；SHA256={digest}")
        with zipfile.ZipFile(target) as zf:
            for info in zf.infolist():
                print(f"  {info.filename}  ({info.file_size} B)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
