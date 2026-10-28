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

# DuckDB 版本来源优先级（每平台只取一处，来源写进记录文件，避免把包版本误当运行期证据）：
#   1. vendor/duckdb/<version>/<triple>/：官方预编译库解包目录（按目标三元组分目录，
#      ADR-043；优先当前宿主三元组）；
#   2. WHCODE_DUCKDB_VERSION：调用方已用包管理器校验过版本时显式传入，免重复探测；
#   3. 系统包：FreeBSD `pkg info duckdb`、NetBSD `pkg_info duckdb`。CI 的 BSD 矩阵走
#      包管理器安装的 libduckdb，没有 vendor 目录；缺这条兜底时四类版本中的 DuckDB
#      一项就永远是占位符（run 40159516841 的 deps-freebsd.txt 即此情形）。
duckdb_dir=""
for candidate in vendor/duckdb/*/*/; do
    [ -d "$candidate" ] || continue
    case $candidate in *'.cache'*) continue ;; esac
    candidate=${candidate%/}
    if [ "$(basename "$candidate")" = "${host_triple:-}" ]; then
        duckdb_dir=$candidate
        break
    fi
done
if [ -z "$duckdb_dir" ]; then
    duckdb_dir=$(ls -d vendor/duckdb/*/*/ 2>/dev/null | grep -v '\.cache' | head -n 1)
    duckdb_dir=${duckdb_dir%/}
fi
duckdb_version="<未取得证据：无 vendor 目录与 duckdb 系统包>"
duckdb_source="<未取得证据>"
duckdb_payload="<未取得证据>"
duckdb_sha="<未取得证据>"
duckdb_lib=""
duckdb_known=0

if [ -n "$duckdb_dir" ]; then
    duckdb_version=$(basename "$(dirname "$duckdb_dir")")
    duckdb_source="vendor:$duckdb_dir"
    duckdb_payload=$duckdb_dir
    duckdb_lib=$(ls "$duckdb_dir"/libduckdb.so "$duckdb_dir"/libduckdb.dylib \
        "$duckdb_dir"/duckdb.dll 2>/dev/null | head -n 1)
    duckdb_known=1
elif [ -n "${WHCODE_DUCKDB_VERSION:-}" ]; then
    duckdb_version=$WHCODE_DUCKDB_VERSION
    duckdb_source="env:WHCODE_DUCKDB_VERSION"
    duckdb_known=1
else
    # 包名形如 `duckdb-1.5.5`。不能只取首行首字段：NetBSD 的 pkg_info 首行是
    # `Information for duckdb-1.5.5:`，FreeBSD 的 pkg 首行才是 `duckdb-1.5.5 ...`。
    # 因此在整段输出里按正则抽第一个 `duckdb-<数字...>`，并以 pkgin list 兜底
    # （run 40183376819/40185854369 两次因首字段口径落空）。
    extract_duckdb() {
        sed -n 's/.*\(duckdb-[0-9][^[:space:]:]*\).*/\1/p' | head -n 1
    }
    for pkg_tool in pkg pkg_info; do
        command -v "$pkg_tool" >/dev/null 2>&1 || continue
        if [ "$pkg_tool" = "pkg" ]; then
            pkg_out=$("$pkg_tool" info duckdb 2>/dev/null) || pkg_out=""
        else
            pkg_out=$("$pkg_tool" duckdb 2>/dev/null) || pkg_out=""
        fi
        pkg_name=$(printf '%s\n' "$pkg_out" | extract_duckdb)
        [ -n "${pkg_name:-}" ] || continue
        case $pkg_name in
            duckdb-[0-9]*)
                duckdb_version=${pkg_name#duckdb-}
                duckdb_source="pkg:$pkg_tool"
                duckdb_known=1
                ;;
            *)
                continue
                ;;
        esac
        break
    done
    if [ "$duckdb_known" -eq 0 ] && command -v pkgin >/dev/null 2>&1; then
        pkg_name=$(pkgin list 2>/dev/null | extract_duckdb)
        if [ -n "${pkg_name:-}" ]; then
            duckdb_version=${pkg_name#duckdb-}
            duckdb_source="pkg:pkgin"
            duckdb_known=1
        fi
    fi
fi

# 载荷与校验和：vendor 取解包目录，系统包取实际链接的 libduckdb。
if [ "$duckdb_known" -eq 1 ] && [ -z "${duckdb_lib:-}" ]; then
    if [ -n "${DUCKDB_LIB_DIR:-}" ] && [ -f "$DUCKDB_LIB_DIR/libduckdb.so" ]; then
        duckdb_lib=$DUCKDB_LIB_DIR/libduckdb.so
    else
        duckdb_lib=$(ls /usr/local/lib/libduckdb.so /usr/pkg/lib/libduckdb.so 2>/dev/null | head -n 1)
    fi
fi
if [ -z "${duckdb_dir:-}" ] && [ -n "${duckdb_lib:-}" ]; then
    duckdb_payload=$(dirname "$duckdb_lib")
fi
[ -n "${duckdb_lib:-}" ] && duckdb_sha=$(sha256_of "$duckdb_lib")

# 运行期引擎版本：优先 doctor 自报。调用方可用 WHCODE_SKIP_RUNTIME_PROBE=1 显式跳过
# （仅当本平台无法构建 whcode-bin 时，例如 BSD 上 whcode-bin 依赖 GUI）；跳过或失败时
# 退回已核对的库版本，并在 engine_version_source 标注退回依据，不冒充运行期证据。
runtime_engine="<未取得证据：未运行 doctor>"
engine_source="<未取得证据>"
doctor_ok=0
if [ "${WHCODE_SKIP_RUNTIME_PROBE:-0}" != "1" ] && command -v cargo >/dev/null 2>&1; then
    doctor_out=$(cargo run -q -p whcode-bin -- doctor 2>&1 || true)
    line=$(printf '%s\n' "$doctor_out" | grep -m 1 '^duckdb_engine_version:')
    if [ -n "${line:-}" ]; then
        runtime_engine=${line#duckdb_engine_version: }
        engine_source="whcode doctor"
        doctor_ok=1
    fi
fi
if [ "$doctor_ok" -eq 0 ] && [ "$duckdb_known" -eq 1 ]; then
    runtime_engine=$duckdb_version
    engine_source="$duckdb_source（doctor 未运行或不可用，退回已核对的库版本）"
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
    echo "package = duckdb crate（default-features = false，链接外部 libduckdb）"
    echo "prebuilt_version = $duckdb_version"
    echo "version_source = $duckdb_source"
    echo "engine_version_runtime = $runtime_engine"
    echo "engine_version_source = $engine_source"
    echo "payload_dir = $duckdb_payload"
    echo "payload_sha256 = $duckdb_sha"
    if [ -n "${duckdb_dir:-}" ]; then
        echo "provenance = $duckdb_dir/PROVENANCE.txt"
    elif [ "$duckdb_known" -eq 1 ]; then
        echo "provenance = <未取得证据：系统包无 PROVENANCE.txt（来源见 version_source）>"
    else
        echo "provenance = <未取得证据：无 vendor 目录与 duckdb 系统包>"
    fi
    echo
    echo "[environment]"
    echo "duckdb_lib_dir = ${DUCKDB_LIB_DIR:-<未设置>}"
    echo "duckdb_include_dir = ${DUCKDB_INCLUDE_DIR:-<未设置>}"
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
