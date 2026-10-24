#!/bin/sh
# WHCode 依赖记录（tasks.md T020/T021；V-001）。
#
# 用途：在目标平台**本地**执行，产出含 Rust / DuckDB / 编译器 / OS 四类版本的依赖记录。
# 覆盖平台：FreeBSD、OpenBSD、NetBSD、DragonFly（POSIX sh）。
# Windows 请用 .github/scripts/platform-matrix/record_deps.py——本项目不使用任何 GNU 工具，
# 故 Windows 侧不依赖 bash（见 docs/matrices/decisions.md D-01 修订记录）。
#
# 纪律：
#   - 每平台分别产出，不得合并为跨平台平均值（ADR-010、§5.2 测量纪律）；
#   - 本脚本只"记录"、不"判定"：取值 `<未取得证据>` 表示该项无实测值，
#     调用方**不得**把它解释为通过；
#   - 交叉编译产物不得作为本脚本结论的替代（G-01）。
#
# 用法：sh .github/scripts/platform-matrix/record-deps.sh [输出目录，默认 artifacts]

set -u

OUT_DIR=${1:-artifacts}
mkdir -p "$OUT_DIR"

uname_s=$(uname -s 2>/dev/null || echo unknown)
platform=$(printf '%s' "$uname_s" | tr 'A-Z' 'a-z')
OUT_FILE="$OUT_DIR/deps-$platform.txt"

# 取命令输出的第一行；命令不存在或失败时返回占位符。
first_line() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "<未取得证据：$1 不存在>"
        return
    fi
    out=$("$@" 2>&1 | head -n 1) || out=""
    if [ -z "$out" ]; then
        echo "<未取得证据：$1 无输出>"
    else
        echo "$out"
    fi
}

sha256_of() {
    target=$1
    if [ ! -f "$target" ]; then
        echo "<未取得证据：文件不存在>"
        return
    fi
    if command -v sha256 >/dev/null 2>&1; then
        sha256 -q "$target" 2>/dev/null || echo "<未取得证据：sha256 失败>"
    elif command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$target" 2>/dev/null | awk '{print $1}'
    else
        echo "<未取得证据：无 sha256 工具>"
    fi
}

host_triple=$(rustc -vV 2>/dev/null | awk '/^host:/ {print $2}')
[ -n "${host_triple:-}" ] || host_triple="<未取得证据：rustc 不可用>"

pinned_channel=$(
    awk -F'"' '/^channel/ {print $2}' rust-toolchain.toml 2>/dev/null | head -n 1
)
[ -n "${pinned_channel:-}" ] || pinned_channel="<未取得证据>"

# DuckDB：从实际解包目录读取版本，避免与 pin 值脱节。
duckdb_dir=$(ls -d vendor/duckdb/*/ 2>/dev/null | grep -v '\.cache' | head -n 1)
duckdb_dir=${duckdb_dir%/}
duckdb_version="<未取得证据：未拉取预编译库>"
duckdb_sha="<未取得证据>"
if [ -n "${duckdb_dir:-}" ]; then
    duckdb_version=$(basename "$duckdb_dir")
    lib=$(ls "$duckdb_dir"/libduckdb.so "$duckdb_dir"/libduckdb.dylib "$duckdb_dir"/duckdb.dll 2>/dev/null | head -n 1)
    [ -n "${lib:-}" ] && duckdb_sha=$(sha256_of "$lib")
fi

# 运行期引擎版本：优先复用已构建的二进制，其次现场构建（失败只记录，不中止）。
runtime_engine="<未取得证据：未运行 doctor>"
if command -v cargo >/dev/null 2>&1; then
    doctor_out=$(cargo run -q -p whcode-bin -- doctor 2>&1 || true)
    line=$(printf '%s\n' "$doctor_out" | grep -m 1 '^duckdb_engine_version:')
    [ -n "${line:-}" ] && runtime_engine=${line#duckdb_engine_version: }
fi

{
    echo "# WHCode 依赖记录（V-001）"
    echo "format_version = 1"
    echo "generated_at_utc = $(date -u '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null || echo '<未取得证据>')"
    echo "script = .github/scripts/platform-matrix/record-deps.sh"
    echo
    echo "[platform]"
    echo "os_name = $uname_s"
    echo "os_release = $(uname -r 2>/dev/null || echo '<未取得证据>')"
    echo "os_version = $(uname -v 2>/dev/null || echo '<未取得证据>')"
    echo "machine = $(uname -m 2>/dev/null || echo '<未取得证据>')"
    echo "host_triple = $host_triple"
    echo "detailed = $(uname -a 2>/dev/null || echo '<未取得证据>')"
    if command -v freebsd-version >/dev/null 2>&1; then
        echo "freebsd_version = $(freebsd-version -ku 2>&1 | tr '\n' ' ')"
    fi
    echo
    echo "[toolchain]"
    echo "pinned_channel = $pinned_channel"
    echo "rustc = $(rustc -vV 2>/dev/null | tr '\n' ' ' || echo '<未取得证据>')"
    echo "cargo = $(first_line cargo -V)"
    echo "rustup = $(first_line rustup show)"
    echo
    echo "[compiler]"
    echo "cc = $(first_line cc --version)"
    echo "cxx = $(first_line c++ --version)"
    echo "clang = $(first_line clang --version)"
    echo
    echo "[duckdb]"
    echo "package = duckdb crate（官方预编译动态库，MIT）"
    echo "prebuilt_version = $duckdb_version"
    echo "engine_version_runtime = $runtime_engine"
    echo "payload_dir = ${duckdb_dir:-<未取得证据>}"
    echo "payload_sha256 = $duckdb_sha"
    echo "provenance = ${duckdb_dir:-vendor/duckdb/<version>}/PROVENANCE.txt"
    echo
    echo "[environment]"
    echo "duckdb_lib_dir = ${DUCKDB_LIB_DIR:-<未设置>}"
    echo "cc_env = ${CC:-<未设置>}"
    echo
    echo "[dependency_tree]"
    if command -v cargo >/dev/null 2>&1; then
        # 禁用彩色输出，避免 ANSI 转义码污染证据文件。
        CARGO_TERM_COLOR=never NO_COLOR=1 cargo tree --locked --workspace --color never 2>&1 \
            || echo "<未取得证据：cargo tree 失败>"
    else
        echo "<未取得证据：cargo 不存在>"
    fi
} > "$OUT_FILE"

echo "已写出 $OUT_FILE"
