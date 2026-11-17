#!/bin/sh
# FreeBSD 发布包打包与安装验证（ADR-083）。在 FreeBSD VM 的仓库根目录内运行。
#
# 前置：
#   - rustup 1.100.0 已装（pkg 的 rust 版本不确定，无法满足可复现要求）；
#   - bsd-duckdb.sh 已执行且其 env 已 source：DUCKDB_LIB_DIR／DUCKDB_INCLUDE_DIR
#     指向源码构建的 libduckdb.so 与 duckdb.h；
#   - release 构建已完成：CARGO_TARGET_DIR（缺省 target）/release/whcode，
#     且带 RUSTFLAGS='-C link-arg=-Wl,-rpath,$ORIGIN'（包内同目录带 libduckdb.so*）。
#
# 产出（全部落在仓库 artifacts/ 下，随 vmactions copyback 交宿主上传）：
#   artifacts/release-freebsd/whcode-<v>-freebsd-amd64.tar.gz 与 .sha256；
#   artifacts/release-freebsd/install-verify-freebsd-amd64.txt（安装验证记录，
#     首部行与 package-windows.py 同 schema，供 check-release.py 交叉校验）；
#   artifacts/tui-smoke-freebsd-release.json 与 web-smoke-freebsd-release.json。
#
# 安装验证在仓库外的 /tmp 随机目录解包运行：./whcode --version、./whcode doctor、
# ./whcode doctor --json 三条命令各有可归因信号才判 GO；数据落 /tmp 不污染仓库。
# 任一环节失败：写 artifacts/freebsd-release-failed.txt 并退出非零（由宿主步骤保全
# 证据后判红；不以 0 条输出冒充通过，docs/guides/signal-chain-completeness.md）。

set -eu

root_ok() {
    [ -f Cargo.toml ] && [ -f LICENSE ] && [ -f CHANGELOG.md ]
}
root_ok || {
    echo "FAIL: 必须在仓库根目录运行（Cargo.toml／LICENSE／CHANGELOG.md 缺失）" >&2
    exit 1
}

[ -n "${DUCKDB_LIB_DIR:-}" ] || {
    echo "FAIL: DUCKDB_LIB_DIR 未设置（先执行 bsd-duckdb.sh 并 source 其 env）" >&2
    exit 1
}

# 安装验证只依赖包内 RPATH=$ORIGIN：显式清掉构建环境残留的 LD_LIBRARY_PATH，
# 否则运行期能经环境变量找到 libduckdb.so，会把「断链包」误判成可安装（冒充证据）。
unset LD_LIBRARY_PATH

# 版本号唯一来源：根 Cargo.toml 的 [workspace.package]（与 package-windows.py 同口径）。
version=$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f && $1=="version"{sub(/^version[[:space:]]*=[[:space:]]*"/,"");sub(/"[[:space:]]*$/,"");print;exit}' Cargo.toml)
[ -n "$version" ] || {
    echo "FAIL: 未能从根 Cargo.toml 解析 [workspace.package] version" >&2
    exit 1
}

package_name="whcode-${version}-freebsd-amd64"
target_dir="${CARGO_TARGET_DIR:-target}"
bin_path="$target_dir/release/whcode"
out_dir="artifacts/release-freebsd"
epoch="${SOURCE_DATE_EPOCH:-$(date -u +%s)}"
generated_utc=$(date -u -r "$epoch" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u +%FT%TZ)

mkdir -p "$out_dir"
stage_root=$(mktemp -d /tmp/whcode-pkg-stage.XXXXXX)
stage="$stage_root/$package_name"
mkdir -p "$stage/assets/fonts"

verify_dir=$(mktemp -d /tmp/whcode-verify.XXXXXX)

cleanup() {
    rm -rf "$stage_root" "$verify_dir"
}
trap cleanup EXIT INT TERM

# ---------- 组包（白名单：不在清单里的文件一律不进包） ----------
[ -f "$bin_path" ] || {
    echo "FAIL: 未找到 $bin_path（先 cargo build --release --locked -p whcode-bin）" >&2
    exit 1
}
cp -p "$bin_path" "$stage/whcode"
chmod 755 "$stage/whcode"

# libduckdb.so* 含 SONAME 目标与符号链接；运行时按 SONAME 找库，缺链接即坏包。
# 符号链接用 cp -P 原样保留（默认 cp 会跟随命令行参数里的链接，破坏 SONAME 链）。
lib_count=0
for lib_file in "$DUCKDB_LIB_DIR"/libduckdb.so*; do
    [ -f "$lib_file" ] || [ -L "$lib_file" ] || continue
    if [ -L "$lib_file" ]; then
        cp -pP "$lib_file" "$stage/"
    else
        cp -p "$lib_file" "$stage/"
    fi
    lib_count=$((lib_count + 1))
done
[ "$lib_count" -ge 1 ] || {
    echo "FAIL: $DUCKDB_LIB_DIR 下未找到 libduckdb.so*（DuckDB 源码构建产物缺失）" >&2
    exit 1
}

cp -p README.md LICENSE CHANGELOG.md "$stage/"
cp -p docs/matrices/compatibility.md "$stage/compatibility.md"
cp -p docs/matrices/platform-matrix.md "$stage/platform-matrix.md"
cp -p assets/fonts/SOURCES.md "$stage/assets/fonts/SOURCES.md"
cp -p assets/fonts/source/inter/LICENSE.txt "$stage/assets/fonts/Inter-LICENSE.txt"
cp -p assets/fonts/source/noto-sans-sc/LICENSE "$stage/assets/fonts/NotoSansCJKsc-LICENSE.txt"
cp -p assets/fonts/source/jetbrains-mono/OFL.txt "$stage/assets/fonts/JetBrainsMono-OFL.txt"
cp -p assets/fonts/source/sarasa-mono-sc/LICENSE "$stage/assets/fonts/SarasaMonoSC-LICENSE.txt"

# ---------- PACKAGE-INFO.txt（与 Windows 包同布局，文案按 FreeBSD 口径） ----------
info="$stage/PACKAGE-INFO.txt"
{
    echo "WHCode FreeBSD amd64 发布包"
    echo "package = $package_name"
    echo "version = $version"
    echo "target_triple = x86_64-unknown-freebsd"
    echo "source_date_epoch = $epoch"
    echo "generated_utc = $generated_utc"
    echo "platform_scope = FreeBSD 为构建交付目标（ADR-083），不作验收结论；"
    echo "  安装验证与 TUI/Web 冒烟随本包发布取得；性能分位数未取得（perf 通道仍仅 Windows）。"
    echo ""
    echo "清单（文件 / 字节 / SHA256 / 来源）："
} >"$info"

add_manifest_line() {
    packed="$1"
    origin="$2"
    file="$stage/$packed"
    [ -f "$file" ] || {
        echo "FAIL: 缺失随包文件：$packed（来自 $origin）" >&2
        exit 1
    }
    bytes=$(wc -c <"$file" | tr -d ' ')
    digest=$(sha256 -q "$file")
    printf '  %s  %s  %s  <- %s\n' "$packed" "$bytes" "$digest" "$origin" >>"$info"
}

add_manifest_line "whcode" "$bin_path"
for lib_file in "$DUCKDB_LIB_DIR"/libduckdb.so*; do
    [ -f "$lib_file" ] || [ -L "$lib_file" ] || continue
    bytes=$(wc -c <"$lib_file" | tr -d ' ')
    digest=$(sha256 -q "$lib_file")
    printf '  %s  %s  %s  <- %s\n' "$(basename "$lib_file")" "$bytes" "$digest" "$DUCKDB_LIB_DIR/" >>"$info"
done
add_manifest_line "README.md" "README.md"
add_manifest_line "LICENSE" "LICENSE"
add_manifest_line "CHANGELOG.md" "CHANGELOG.md"
add_manifest_line "compatibility.md" "docs/matrices/compatibility.md"
add_manifest_line "platform-matrix.md" "docs/matrices/platform-matrix.md"
add_manifest_line "assets/fonts/SOURCES.md" "assets/fonts/SOURCES.md"
add_manifest_line "assets/fonts/Inter-LICENSE.txt" "assets/fonts/source/inter/LICENSE.txt"
add_manifest_line "assets/fonts/NotoSansCJKsc-LICENSE.txt" "assets/fonts/source/noto-sans-sc/LICENSE"
add_manifest_line "assets/fonts/JetBrainsMono-OFL.txt" "assets/fonts/source/jetbrains-mono/OFL.txt"
add_manifest_line "assets/fonts/SarasaMonoSC-LICENSE.txt" "assets/fonts/source/sarasa-mono-sc/LICENSE"

info_bytes=$(wc -c <"$info" | tr -d ' ')
info_digest=$(sha256 -q "$info")
printf '  %s  %s  %s  <- %s\n' "PACKAGE-INFO.txt" "$info_bytes" "$info_digest" "本脚本生成" >>"$info"

{
    echo ""
    echo "安装：tar zxvf $package_name.tar.gz && cd $package_name && ./whcode doctor"
    echo "  （libduckdb.so 与可执行文件同目录，RPATH=\$ORIGIN 已设，不依赖系统路径。）"
    echo "自检：./whcode doctor        （人类可读）"
    echo "      ./whcode doctor --json （脚本可读，含 duckdb_engine_version 与三档运行时等级）"
    echo "数据：默认在当前目录创建 whcode.duckdb；用 --workspace DIR 指定工作区。"
    echo "字体：可执行文件内嵌 SIL OFL 1.1 字体子集；来源与许可见 assets/fonts/。"
    echo "退出权：数据经 ./whcode db archive --export <PATH> 带走；删除解包目录即完全卸载（无系统级注册）。"
} >>"$info"

# ---------- tar.gz 与 .sha256（sha256sum 兼容格式；bsdtar 默认 restricted pax，POSIX 一致） ----------
tar -czf "$out_dir/$package_name.tar.gz" -C "$stage_root" "$package_name"
tar_bytes=$(wc -c <"$out_dir/$package_name.tar.gz" | tr -d ' ')
tar_digest=$(sha256 -q "$out_dir/$package_name.tar.gz")
printf '%s  %s\n' "$tar_digest" "$package_name.tar.gz" >"$out_dir/$package_name.tar.gz.sha256"

# ---------- 安装验证（仓库外解包；RPATH=$ORIGIN 是否成立在这里见分晓） ----------
unpack="$verify_dir/$package_name"
tar -xzf "$out_dir/$package_name.tar.gz" -C "$verify_dir"
[ -x "$unpack/whcode" ] || {
    echo "FAIL: 解包后缺可执行文件 $package_name/whcode" >&2
    exit 1
}

verify_record="$out_dir/install-verify-freebsd-amd64.txt"
failed=0

run_cmd() {
    # run_cmd <记录追加文件> <显示名> <命令…>：记录退出码与输出摘要；缺输出记“未取得”。
    _file="$1"
    _name="$2"
    shift 2
    _out_file=$(mktemp /tmp/whcode-cmd.XXXXXX)
    if "$@" >"$_out_file" 2>&1; then
        _rc=0
    else
        _rc=$?
    fi
    _first=$(head -n 1 "$_out_file" 2>/dev/null)
    [ -n "$_first" ] || _first="未取得（无输出）"
    {
        echo "  [$_name] 退出码=$_rc"
        echo "      输出首行: $_first"
    } >>"$_file"
    rm -f "$_out_file"
    [ "$_rc" -eq 0 ] || failed=1
}

{
    echo "WHCode FreeBSD amd64 发布包"
    # 包名写归档文件全名（含 .tar.gz）：check-release.py 的 --record 交叉校验按
    # `recorded_name == 实际包文件名` 比对，与 Windows 侧 `zip_path.name` 同口径。
    echo "package = $package_name.tar.gz（$tar_bytes 字节，sha256=$tar_digest）"
    echo "version = $version"
    echo "target_triple = x86_64-unknown-freebsd"
    echo "binary_version = $("$unpack/whcode" --version 2>&1 | head -n 1)"
    echo "rustc = $(rustc --version 2>/dev/null || echo '未取得（rustc 不在 PATH）')"
    echo "python = $(python3 -V 2>/dev/null || echo '未取得')"
    echo "os = $(uname -a)"
    echo "source_date_epoch = $epoch"
    echo "generated_utc = $generated_utc"
    echo "platform_scope = FreeBSD 为构建交付目标（ADR-083），不作验收结论。"
    echo ""
    echo "安装验证（仓库外 $verify_dir 解包运行；libduckdb.so 随包，RPATH=\$ORIGIN）："
} >"$verify_record"

run_cmd "$verify_record" "1 ./whcode --version" "$unpack/whcode" --version
run_cmd "$verify_record" "2 ./whcode doctor" "$unpack/whcode" doctor
run_cmd "$verify_record" "3 ./whcode doctor --json" "$unpack/whcode" doctor --json

# doctor --json 的可归因信号：duckdb 引擎版本与运行时等级字段必须在场。
doctor_keys=$("$unpack/whcode" doctor --json 2>/dev/null | tr ',' '\n' | grep -E 'duckdb_engine_version|runtime' || true)
{
    echo "  [doctor --json 关键字段] $doctor_keys"
} >>"$verify_record"

if [ "$failed" -ne 0 ]; then
    echo "FAIL: 安装验证三条命令存在非零退出（详见 $verify_record）" >&2
    echo "freebsd_release_failed=install-verify" >artifacts/freebsd-release-failed.txt
    exit 1
fi

# ---------- TUI／Web 冒烟（对解包后的 release 二进制；功能等价证据） ----------
python3 .github/scripts/tui-smoke.py --exe "$unpack/whcode" --json artifacts/tui-smoke-freebsd-release.json
python3 .github/scripts/web-smoke.py --exe "$unpack/whcode" --json artifacts/web-smoke-freebsd-release.json

echo "FREEBSD_RELEASE=GO（tar.gz＋sha256＋安装验证三条 GO＋TUI/Web 冒烟；ADR-083）"
