#!/usr/bin/env python3
"""WHCode 依赖记录（tasks.md T020/T021；V-001）——Windows / 通用 Python 实现。

与 `record-deps.sh` 产出**同一格式**的依赖记录文件。存在两个实现的原因：本项目
**不使用任何 GNU 工具**，Windows 侧不依赖 bash/GNU coreutils（见
`docs/matrices/decisions.md` D-01 修订记录）。

纪律：
  - 每平台分别产出，不得合并为跨平台平均值（ADR-010、§5.2 测量纪律）；
  - 本脚本只"记录"、不"判定"：取值 `<未取得证据…>` 表示该项无实测值，不得解释为通过；
  - 交叉编译产物不得作为本脚本结论的替代（G-01）。

用法：
    python .github/scripts/platform-matrix/record_deps.py [输出目录，默认 artifacts]
"""

from __future__ import annotations

import datetime
import hashlib
import json
import os
import pathlib
import platform
import shutil
import subprocess
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parents[3]
NO_EVIDENCE = "<未取得证据"

#: 常见但不在 PATH 上的工具位置（本项目要求：不依赖 PATH 已配置完整）。
KNOWN_TOOL_PATHS = {
    "clang": [
        pathlib.Path("C:/Program Files/LLVM/bin/clang.exe"),
    ],
    "cargo": [pathlib.Path.home() / ".cargo" / "bin" / "cargo.exe"],
    "rustc": [pathlib.Path.home() / ".cargo" / "bin" / "rustc.exe"],
}


def tool_env() -> dict[str, str]:
    """子进程环境：强制禁用彩色输出，避免 ANSI 转义码污染证据文件。"""
    env = dict(os.environ)
    env["CARGO_TERM_COLOR"] = "never"
    env["NO_COLOR"] = "1"
    env["CLICOLOR"] = "0"
    return env


def locate(name: str) -> str | None:
    hit = shutil.which(name)
    if hit:
        return hit
    for candidate in KNOWN_TOOL_PATHS.get(name, []):
        if candidate.is_file():
            return str(candidate)
    return None


def capture(args: list[str], timeout: int = 1800) -> tuple[str, str, int]:
    """执行命令，返回 (stdout, stderr, 退出码)；命令缺失或超时时返回带标记的说明。"""
    try:
        completed = subprocess.run(
            args,
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            env=tool_env(),
        )
    except FileNotFoundError:
        return "", f"{NO_EVIDENCE}：{args[0]} 不存在>", -1
    except subprocess.TimeoutExpired:
        return "", f"{NO_EVIDENCE}：{args[0]} 超时>", -1
    return completed.stdout or "", completed.stderr or "", completed.returncode


def run(args: list[str], timeout: int = 1800) -> str:
    """执行命令并合并输出为单行块（用于"记录"用途）。"""
    stdout, stderr, code = capture(args, timeout)
    merged = "\n".join(part for part in (stdout.strip(), stderr.strip()) if part)
    if not merged:
        return f"{NO_EVIDENCE}：{args[0]} 无输出（退出码 {code}）>"
    return merged


def first_line(text: str) -> str:
    lines = [line for line in text.splitlines() if line.strip()]
    return lines[0] if lines else f"{NO_EVIDENCE}：空输出>"


def sha256_of(path: pathlib.Path) -> str:
    if not path.is_file():
        return f"{NO_EVIDENCE}：文件不存在>"
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def cpu_model() -> str:
    """CPU 型号。取不到时如实标注缺失——不得猜测。"""
    text = platform.processor().strip()
    return text if text else f"{NO_EVIDENCE}：平台未提供 CPU 型号>"


def logical_cores() -> str:
    """逻辑核数（明确标注口径，避免被当成物理核数写进性能结论）。"""
    count = os.cpu_count()
    if count is None:
        return f"{NO_EVIDENCE}：无法取得>"
    return f"{count}（逻辑核，非物理核）"


def total_memory_bytes() -> str:
    """物理内存总量（Windows 用 ctypes 调 GlobalMemoryStatusEx；其他平台标注缺失）。"""
    if platform.system() != "Windows":
        return f"{NO_EVIDENCE}：本脚本只实现了 Windows 探测>"
    try:
        import ctypes

        class MemoryStatus(ctypes.Structure):
            _fields_ = [
                ("dwLength", ctypes.c_ulong),
                ("dwMemoryLoad", ctypes.c_ulong),
                ("ullTotalPhys", ctypes.c_ulonglong),
                ("ullAvailPhys", ctypes.c_ulonglong),
                ("ullTotalPageFile", ctypes.c_ulonglong),
                ("ullAvailPageFile", ctypes.c_ulonglong),
                ("ullTotalVirtual", ctypes.c_ulonglong),
                ("ullAvailVirtual", ctypes.c_ulonglong),
                ("ullAvailExtendedVirtual", ctypes.c_ulonglong),
            ]

        status = MemoryStatus()
        status.dwLength = ctypes.sizeof(MemoryStatus)
        if ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status)) == 0:
            return f"{NO_EVIDENCE}：GlobalMemoryStatusEx 失败>"
        return str(status.ullTotalPhys)
    except Exception as err:  # noqa: BLE001 —— 记录用途，任何异常都降级为缺失标记
        return f"{NO_EVIDENCE}：{err}>"


def pinned_channel() -> str:
    toolchain = REPO_ROOT / "rust-toolchain.toml"
    if not toolchain.is_file():
        return f"{NO_EVIDENCE}：rust-toolchain.toml 缺失>"
    for line in toolchain.read_text(encoding="utf-8").splitlines():
        if line.strip().startswith("channel"):
            parts = line.split('"')
            if len(parts) >= 2:
                return parts[1]
    return f"{NO_EVIDENCE}：未找到 channel>"


def cargo_config_env() -> str:
    """记录 .cargo/config.toml 注入的构建期环境（DUCKDB_LIB_DIR）。"""
    config = REPO_ROOT / ".cargo" / "config.toml"
    if not config.is_file():
        return f"{NO_EVIDENCE}：.cargo/config.toml 缺失>"
    for line in config.read_text(encoding="utf-8").splitlines():
        if line.strip().startswith("DUCKDB_LIB_DIR"):
            return line.strip()
    return f"{NO_EVIDENCE}：未找到 DUCKDB_LIB_DIR>"


def find_msvc() -> tuple[str, str]:
    """返回 (MSVC 工具集目录, 版本说明)。"""
    program_files_x86 = pathlib.Path(
        os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)")
    )
    vswhere = program_files_x86 / "Microsoft Visual Studio" / "Installer" / "vswhere.exe"
    installer_version = (
        run([str(vswhere), "-latest", "-property", "installationVersion"])
        if vswhere.is_file()
        else f"{NO_EVIDENCE}：vswhere 缺失>"
    )
    roots = sorted(
        program_files_x86.glob("Microsoft Visual Studio/*/*/VC/Tools/MSVC/*"),
        key=lambda p: p.name,
        reverse=True,
    )
    found = [p for p in roots if p.is_dir()]
    if not found:
        return f"{NO_EVIDENCE}：未安装 MSVC 生成工具>", installer_version
    newest = found[0]
    return str(newest), f"{newest.name}（安装器 {installer_version}）"


def find_windows_sdk() -> str:
    include_root = (
        pathlib.Path(os.environ.get("ProgramFiles(x86)", "C:/Program Files (x86)"))
        / "Windows Kits"
        / "10"
        / "Include"
    )
    versions = sorted((p.name for p in include_root.glob("*") if p.is_dir()), reverse=True)
    return ", ".join(versions) if versions else f"{NO_EVIDENCE}：未安装 Windows SDK>"


def host_triple() -> str:
    """宿主 Windows 架构 -> Rust 三元组；其他系统返回空串（BSD 无 vendor 载荷）。"""
    if platform.system() != "Windows":
        return ""
    machine = platform.machine().upper()
    if machine == "AMD64":
        return "x86_64-pc-windows-msvc"
    if machine == "ARM64":
        return "aarch64-pc-windows-msvc"
    return ""


def find_duckdb() -> tuple[str, str, pathlib.Path | None]:
    root = REPO_ROOT / "vendor" / "duckdb"
    version_dirs = (
        [p for p in root.glob("*") if p.is_dir() and not p.name.startswith(".")]
        if root.is_dir()
        else []
    )
    if not version_dirs:
        return f"{NO_EVIDENCE}：未拉取预编译库>", f"{NO_EVIDENCE}：未拉取预编译库>", None
    newest = sorted(version_dirs, key=lambda p: p.name, reverse=True)[0]
    # ADR-043：vendor/duckdb/<版本>/<三元组>/ 按目标三元组分目录；优先记录当前宿主
    # 架构的载荷（本脚本只记录宿主证据，交叉构建载荷由其所属平台记录）。
    triples = sorted(p for p in newest.iterdir() if p.is_dir())
    if not triples:
        return newest.name, f"{NO_EVIDENCE}：目录中未找到三元组载荷>", newest
    host = host_triple()
    ordered = [p for p in triples if p.name == host] + [p for p in triples if p.name != host]
    for candidate in ordered:
        for name in ("duckdb.dll", "libduckdb.so", "libduckdb.dylib"):
            payload = candidate / name
            if payload.is_file():
                return newest.name, sha256_of(payload), candidate
    return newest.name, f"{NO_EVIDENCE}：目录中未找到动态库>", ordered[0]


def runtime_engine_version(cargo: str | None) -> str:
    """运行期引擎版本：只解析 stdout，避免 cargo 的 stderr 提示污染 JSON。"""
    if cargo is None:
        return f"{NO_EVIDENCE}：cargo 不存在>"
    stdout, stderr, code = capture(
        [cargo, "run", "-q", "-p", "whcode-bin", "--", "doctor", "--json"]
    )
    try:
        payload = json.loads(stdout.strip())
    except json.JSONDecodeError:
        return f"{NO_EVIDENCE}：doctor --json 无法解析（退出码 {code}；stderr {first_line(stderr)}）>"
    version = payload.get("duckdb_engine_version")
    return version if isinstance(version, str) else f"{NO_EVIDENCE}：doctor 未报告引擎版本>"


def main() -> int:
    out_dir = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else pathlib.Path("artifacts")
    out_dir = out_dir if out_dir.is_absolute() else REPO_ROOT / out_dir
    out_dir.mkdir(parents=True, exist_ok=True)

    tag = platform.system().lower()
    out_file = out_dir / f"deps-{tag}.txt"

    cargo = locate("cargo")
    rustc_path = locate("rustc")
    rustc_line = run([rustc_path, "-vV"]) if rustc_path else f"{NO_EVIDENCE}：rustc 不可用>"
    host = f"{NO_EVIDENCE}：rustc 不可用>"
    for line in rustc_line.splitlines():
        if line.startswith("host:"):
            host = line.split(":", 1)[1].strip()

    msvc_dir, msvc_version = find_msvc()
    duckdb_version, duckdb_sha, duckdb_dir = find_duckdb()
    clang = locate("clang") or f"{NO_EVIDENCE}：clang 不在 PATH 与已知路径>"
    # 只探测一次：doctor 需要启动进程，重复调用会显著拉长记录时间。
    runtime_engine = runtime_engine_version(cargo)
    now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

    lines: list[str] = [
        "# WHCode 依赖记录（V-001）",
        "format_version = 1",
        f"generated_at_utc = {now}",
        "script = .github/scripts/platform-matrix/record_deps.py",
        "",
        "[platform]",
        f"os_name = {platform.system()}",
        f"os_release = {platform.release()}",
        f"os_version = {platform.version()}",
        f"machine = {platform.machine()}",
        f"host_triple = {host}",
        f"detailed = {platform.platform()}",
        "",
        "[toolchain]",
        f"pinned_channel = {pinned_channel()}",
        f"rustc = {first_line(rustc_line)}",
        f"rustc_full = {' '.join(rustc_line.split())}",
        f"cargo = {first_line(run([cargo, '-V'])) if cargo else f'{NO_EVIDENCE}：cargo 不存在>'}",
        "",
        "[compiler]",
        f"msvc_tools = {msvc_dir}",
        f"msvc_version = {msvc_version}",
        f"windows_sdk = {find_windows_sdk()}",
        f"clang = {clang}",
        "",
        "[duckdb]",
        "package = duckdb crate（官方预编译动态库，MIT；见 docs/adr/ADR-DB-BUILD.md）",
        f"prebuilt_version = {duckdb_version}",
        f"engine_version_runtime = {runtime_engine}",
        f"payload_dir = {duckdb_dir if duckdb_dir else f'{NO_EVIDENCE}>'}",
        f"payload_sha256 = {duckdb_sha}",
        f"provenance = {(duckdb_dir / 'PROVENANCE.txt') if duckdb_dir else 'vendor/duckdb/<version>/PROVENANCE.txt'}",
        "",
        "[measurement_discipline]",
        # V-006/R-12：性能结论必须能复现。这些字段描述“怎样测量”，不是测量结果本身；
        # 没有它们的性能数字不得作为发布承诺。
        f"os = {platform.platform()}",
        f"cpu = {cpu_model()}",
        f"logical_cores = {logical_cores()}",
        f"memory_bytes = {total_memory_bytes()}",
        f"build_profile = {'release（由调用方保证）' if os.environ.get('WHCODE_BUILD_PROFILE') == 'release' else '未声明（脚本不判断构建类型）'}",
        f"rustc = {first_line(rustc_line)}",
        f"duckdb_engine = {runtime_engine}",
        "db_memory_limit_mb = 192（基线默认值，见 crates/whcode-bin/src/config.rs）",
        "db_threads = min(4, 可用并行度)（基线默认值）",
        f"data_scale = {os.environ.get('WHCODE_DATA_SCALE', '<未声明：由基准脚本写入>')}",
        f"sampling_window = {os.environ.get('WHCODE_SAMPLING_WINDOW', '<未声明：由基准脚本写入>')}",
        "script = .github/scripts/platform-matrix/record_deps.py",
        "",
        "[environment]",
        f"ambient_DUCKDB_LIB_DIR = {os.environ.get('DUCKDB_LIB_DIR', '<未设置>')}",
        f"cargo_config = {cargo_config_env()}",
        f"path_has_cargo = {'是' if shutil.which('cargo') else '否（脚本按 rustup 默认目录定位）'}",
        "",
        "[dependency_tree]",
    ]

    if cargo is None:
        lines.append(f"{NO_EVIDENCE}：cargo 不存在>")
    else:
        lines.append(run([cargo, "tree", "--locked", "--workspace", "--color", "never"]))

    out_file.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(f"已写出 {out_file}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
