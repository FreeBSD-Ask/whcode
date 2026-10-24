#!/usr/bin/env python3
"""WHCode 基线基准编排（tasks.md T027；V-006、R-12、任务书 §5.2 测量纪律）。

职责：
  1. 以 release 构建调用 `whcode bench baseline`，把写入吞吐、关键事件提交延迟分位数、
     数据库文件字节数与运行期 DuckDB 引擎版本写入 `artifacts/baseline-<platform>.json`；
  2. 在同一产物中补齐测量纪律字段：OS 名称与版本、CPU 型号/核数、内存、编译类型、
     Rust 工具链（`rustc -vV` 的 host 与 release）、脚本路径与 UTC 生成时间。

纪律：
  - 只用 Python 标准库，不调用 bash/GNU 工具；Windows 侧用 `platform`、`winreg`、`ctypes`
    与 PowerShell `Get-CimInstance`（不使用 wmic 作为唯一路径，Win11 已移除）；
  - 只“记录”不“判定”：取不到的字段写明确的缺失标记，不得猜测；
  - 单机单次样本不构成预算冻结；不同平台的结果不得合并为一组平均值（R-12）；
  - 产物为 UTF-8 无 BOM、LF 换行。

用法：
    python .github/scripts/benchmarks/run-baseline.py
"""

from __future__ import annotations

import ctypes
import datetime
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
    "cargo": [pathlib.Path.home() / ".cargo" / "bin" / "cargo.exe"],
    "rustc": [pathlib.Path.home() / ".cargo" / "bin" / "rustc.exe"],
}


def tool_env() -> dict[str, str]:
    """子进程环境：禁用彩色输出，避免 ANSI 转义码污染证据。"""
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


def capture(args: list[str], timeout: int = 300) -> tuple[str, str, int]:
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


def utc_now() -> str:
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def collect_os() -> dict[str, object]:
    """OS 名称与版本（Python 标准库 `platform`）。"""
    return {
        "os_name": platform.system() or f"{NO_EVIDENCE}：platform.system 为空>",
        "os_version": platform.version() or f"{NO_EVIDENCE}：platform.version 为空>",
        "os_release": platform.release() or f"{NO_EVIDENCE}：platform.release 为空>",
        "os_platform": platform.platform() or f"{NO_EVIDENCE}：platform.platform 为空>",
        "machine": platform.machine() or f"{NO_EVIDENCE}：platform.machine 为空>",
        "os_evidence": "Python 标准库 platform（system/version/release/platform/machine）",
    }


def cim_cpu() -> dict[str, object] | None:
    """Windows CPU 信息：PowerShell `Get-CimInstance Win32_Processor`。"""
    powershell = shutil.which("powershell") or "powershell"
    script = (
        "(Get-CimInstance Win32_Processor | Select-Object -First 1 "
        "Name,NumberOfCores,NumberOfLogicalProcessors | ConvertTo-Json -Compress)"
    )
    stdout, _, code = capture(
        [powershell, "-NoProfile", "-NonInteractive", "-Command", script]
    )
    if code != 0 or not stdout.strip():
        return None
    try:
        payload = json.loads(stdout.strip())
    except json.JSONDecodeError:
        return None
    if isinstance(payload, list):
        payload = payload[0] if payload else {}
    return payload if isinstance(payload, dict) else None


def winreg_cpu_model() -> str | None:
    """Windows CPU 型号：注册表 `ProcessorNameString`（stdlib `winreg`）。"""
    try:
        import winreg
    except ImportError:
        return None
    key_path = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"
    try:
        with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, key_path) as key:
            value, _ = winreg.QueryValueEx(key, "ProcessorNameString")
    except OSError:
        return None
    text = str(value).strip()
    return text or None


def collect_cpu() -> dict[str, object]:
    logical = os.cpu_count()
    model: str | None = None
    physical: int | None = None
    evidence: list[str] = []

    if os.name == "nt":
        model = winreg_cpu_model()
        evidence.append("winreg ProcessorNameString" if model else f"{NO_EVIDENCE}：winreg 未取得型号>")
        cim = cim_cpu()
        if cim:
            if cim.get("Name"):
                model = str(cim["Name"]).strip()
            if isinstance(cim.get("NumberOfCores"), int):
                physical = cim["NumberOfCores"]
            if isinstance(cim.get("NumberOfLogicalProcessors"), int):
                logical = cim["NumberOfLogicalProcessors"]
            evidence.append("Get-CimInstance Win32_Processor")
        else:
            evidence.append(f"{NO_EVIDENCE}：Get-CimInstance Win32_Processor 未取得>")
    else:
        model = platform.processor() or None
        evidence.append("Python 标准库 platform.processor")

    if logical is None:
        evidence.append(f"{NO_EVIDENCE}：os.cpu_count 返回空>")
    return {
        "model": model or f"{NO_EVIDENCE}：CPU 型号未取得>",
        "cores_physical": physical,
        "cores_logical": logical,
        "evidence": "；".join(evidence),
    }


class _MemoryStatusEx(ctypes.Structure):
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


def collect_memory() -> dict[str, object]:
    if os.name == "nt":
        try:
            status = _MemoryStatusEx()
            status.dwLength = ctypes.sizeof(status)
            if ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status)):
                return {
                    "total_bytes": int(status.ullTotalPhys),
                    "evidence": "kernel32!GlobalMemoryStatusEx（ctypes）",
                }
            return {
                "total_bytes": None,
                "evidence": f"{NO_EVIDENCE}：GlobalMemoryStatusEx 返回失败>",
            }
        except (AttributeError, OSError) as exc:
            return {
                "total_bytes": None,
                "evidence": f"{NO_EVIDENCE}：GlobalMemoryStatusEx 不可用（{exc}）>",
            }

    try:
        pages = os.sysconf("SC_PHYS_PAGES")
        page_size = os.sysconf("SC_PAGE_SIZE")
        return {
            "total_bytes": int(pages * page_size),
            "evidence": "os.sysconf(SC_PHYS_PAGES × SC_PAGE_SIZE)",
        }
    except (AttributeError, ValueError, OSError) as exc:
        return {
            "total_bytes": None,
            "evidence": f"{NO_EVIDENCE}：os.sysconf 不可用（{exc}）>",
        }


def collect_toolchain() -> dict[str, str]:
    rustc = locate("rustc")
    if rustc is None:
        marker = f"{NO_EVIDENCE}：rustc 不在 PATH 与已知路径>"
        return {
            "rust_toolchain": marker,
            "rustc_release": marker,
            "rustc_host": marker,
        }
    stdout, stderr, code = capture([rustc, "-vV"], timeout=120)
    text = stdout.strip() or stderr.strip()
    if code != 0 or not text:
        marker = f"{NO_EVIDENCE}：rustc -vV 无有效输出（退出码 {code}）>"
        return {
            "rust_toolchain": marker,
            "rustc_release": marker,
            "rustc_host": marker,
        }
    first_line = next((line for line in text.splitlines() if line.strip()), text)
    release = next(
        (line.split(":", 1)[1].strip() for line in text.splitlines() if line.startswith("release:")),
        None,
    )
    host = next(
        (line.split(":", 1)[1].strip() for line in text.splitlines() if line.startswith("host:")),
        None,
    )
    return {
        "rust_toolchain": first_line,
        "rustc_release": release or f"{NO_EVIDENCE}：rustc -vV 缺少 release 字段>",
        "rustc_host": host or f"{NO_EVIDENCE}：rustc -vV 缺少 host 字段>",
    }


def platform_tag() -> str:
    """产物文件名中的平台标识：系统 + 体系结构（避免不同架构互相覆盖）。"""
    system = platform.system().lower() or "unknown"
    machine = platform.machine().lower() or "unknown"
    return f"{system}-{machine}"


def run_benchmark(cargo: str, out_file: pathlib.Path) -> int:
    args = [
        cargo,
        "run",
        "--release",
        "-p",
        "whcode-bin",
        "--",
        "bench",
        "baseline",
        "--out",
        str(out_file),
    ]
    print(f"执行：{' '.join(args)}")
    try:
        completed = subprocess.run(
            args,
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=3600,
        )
    except subprocess.TimeoutExpired:
        print(f"{NO_EVIDENCE}：基准命令超时（3600 秒）>", file=sys.stderr)
        return 2
    except FileNotFoundError:
        print(f"{NO_EVIDENCE}：cargo 不存在：{cargo}>", file=sys.stderr)
        return 2

    if completed.stdout:
        print(completed.stdout, end="")
    if completed.returncode != 0:
        if completed.stderr:
            print(completed.stderr, end="", file=sys.stderr)
        print(f"基准命令失败（退出码 {completed.returncode}）", file=sys.stderr)
        return completed.returncode if completed.returncode > 0 else 2
    return 0


def main() -> int:
    out_dir = REPO_ROOT / "artifacts"
    out_dir.mkdir(parents=True, exist_ok=True)
    out_file = out_dir / f"baseline-{platform_tag()}.json"
    out_file.unlink(missing_ok=True)

    cargo = locate("cargo")
    if cargo is None:
        print(f"{NO_EVIDENCE}：cargo 不在 PATH 与已知路径；无法执行基线测量>", file=sys.stderr)
        return 2

    code = run_benchmark(cargo, out_file)
    if code != 0:
        return code

    try:
        report = json.loads(out_file.read_text(encoding="utf-8"))
    except OSError as exc:
        print(f"读取基准产物失败：{exc}", file=sys.stderr)
        return 2
    except json.JSONDecodeError as exc:
        print(f"基准产物不是合法 JSON：{exc}", file=sys.stderr)
        return 2

    discipline = report.get("measurement_discipline")
    if not isinstance(discipline, dict):
        print("基准产物缺少 measurement_discipline 字段块", file=sys.stderr)
        return 2

    discipline.update(collect_os())
    discipline["cpu"] = collect_cpu()
    discipline["memory"] = collect_memory()
    discipline.update(collect_toolchain())
    # 保留夹具自报的报告生成时间，另记本次编排补齐测量条件的时间，二者不得互相冒充。
    discipline["bench_report_generated_at_utc"] = discipline.get("generated_at_utc")
    observed_profile = discipline.get("build_profile")
    discipline["build_profile"] = "release"
    if observed_profile != "release":
        discipline["build_profile_note"] = (
            f"二进制自报 build_profile={observed_profile}，与脚本调用的 --release 不一致；"
            "不得据此推断优化级别"
        )
    discipline["script_path"] = ".github/scripts/benchmarks/run-baseline.py"
    discipline["generated_at_utc"] = utc_now()
    discipline["command_line"] = (
        "cargo run --release -p whcode-bin -- bench baseline "
        f"--out artifacts/{out_file.name}"
    )
    discipline["cargo_executable"] = cargo
    discipline["platform_separation_note"] = (
        "单平台单机单次样本；不同平台结果不得合并为平均值，且不构成性能预算冻结（R-12）"
    )

    out_file.write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )

    aggregate = report.get("aggregate", {})
    latency = aggregate.get("latency_us", {})
    print("")
    print(f"产物：{out_file.relative_to(REPO_ROOT)}")
    print(f"OS：{discipline['os_name']} {discipline['os_version']}")
    print(f"CPU：{discipline['cpu']['model']}（逻辑核 {discipline['cpu']['cores_logical']}）")
    print(f"内存：{discipline['memory']['total_bytes']} 字节")
    print(f"工具链：{discipline['rustc_release']} / {discipline['rustc_host']}")
    print(f"编译类型：{discipline['build_profile']}")
    print(f"DuckDB 引擎：{report.get('duckdb_engine_version')}")
    print(
        "数据库参数：memory_limit="
        f"{discipline['database_parameters']['memory_limit']}，threads="
        f"{discipline['database_parameters']['threads']}"
    )
    print(
        f"吞吐中位数：{aggregate.get('throughput_events_per_second_median')} 条/秒"
    )
    print(
        "提交延迟（microseconds）：p50="
        f"{latency.get('p50')}，p95={latency.get('p95')}，p99={latency.get('p99')}，"
        f"样本={latency.get('sample_count')}"
    )
    db_bytes = [run.get("db_bytes") for run in report.get("runs", [])]
    print(f"数据库文件字节数（每次重复）：{db_bytes}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
