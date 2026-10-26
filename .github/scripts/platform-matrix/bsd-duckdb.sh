#!/bin/sh
# 在 BSD（FreeBSD／NetBSD）上从源码构建 DuckDB 2.0.0，并跑 whcode-store 冒烟
# （Check 22 的 BSD 侧）。
#
# 背景：pkg／pkgsrc 提供的 DuckDB 为 1.5.x，而项目冻结引擎是 2.0.0（ADR-034）。包版与
# `duckdb` crate 1.20000.0 的主版本不匹配，`smoke_reports_engine_version_from_runtime`
# 会以“主版本应为 2”失败。本脚本按官方源码构建 2.0.0，安装到隔离前缀，导出
# DUCKDB_LIB_DIR／DUCKDB_INCLUDE_DIR／LD_LIBRARY_PATH／WHCODE_DUCKDB_VERSION，再跑
# `cargo test --locked -p whcode-store`，把版本、摘要、构建耗时、测试退出码与 GO／NO-GO
# 写进 artifacts/store-acceptance/<platform>-store.txt。
#
# 上游事实（2026-10-25 核对，见 artifacts/platform-deps-2026-10-25/freebsd-duckdb-notes.md）：
#   - 标签 v2.0.0，发布提交 d54cbbe3df17e8e5ce68312f483b4155836112aa；
#   - GitHub 自动源码归档 https://github.com/duckdb/duckdb/archive/v2.0.0.tar.gz
#     （实测 Content-Length 106 477 075 字节；GitHub **不**为该归档发布官方 SHA-256）；
#   - 官方把 FreeBSD 列入“best effort support”，不提供预编译库与扩展。
# 因此本脚本：源码身份以“标签 + 固定提交 + `git ls-remote` 复核（可用时）”锁定，
# 归档 SHA-256 为**本地计算并记录**（不是上游摘要）；若调用方通过
# WHCODE_DUCKDB_TARBALL_SHA256 提供可信摘要，则按该值强校验。
#
# 构建选项依据：
#   - BUILD_SHELL=OFF：2.0 的 Makefile 将 DISABLE_SHELL 映射为 -DBUILD_SHELL=0，
#     tools/CMakeLists.txt 以 BUILD_SHELL 决定是否构建 CLI；
#   - BUILD_UNITTESTS=OFF：FreeBSD ports 的 databases/duckdb 使用 CMAKE_OFF=BUILD_UNITTESTS；
#     2.0 另以 ENABLE_UNITTEST_CPP_TESTS 门控 C++ 测试子目录，脚本同时传 OFF；
#   - SKIP_EXTENSIONS=jemalloc：FreeBSD ports 同款规避（上游 issue 14363 相关），
#     避免 jemalloc 扩展在 BSD 上的构建风险；
#   - OVERRIDE_GIT_DESCRIBE=v2.0.0-0-gd54cbbe3df：源码归档没有 .git，不覆盖版本时
#     DuckDB 会退回占位版本，导致主版本断言失败（官方构建文档与 FreeBSD ports 均如此用）。
#
# 判据（信号链完备：交付＝可归因信号存在，而非进程不崩溃）：
#   - 退出码 0 **且** 输出出现 `smoke_reports_engine_version_from_runtime ... ok`
#     → GO；
#   - 退出码 0 但验收用例未执行 → NO-GO（零不等于通过）；
#   - 下载／身份校验／构建／安装失败 → NO-GO，原因写明，不伪造摘要或版本。
#
# 纪律：本脚本只记录、不判定交付；NO-GO 不得改写为 GO。脚本始终以 0 退出，结论写在
# 证据文件里，因此**不得**以退出码代替阅读证据文件。BSD 结论按 ADR-013 延后，不进入
# 验收结论。
#
# 用法：sh .github/scripts/platform-matrix/bsd-duckdb.sh [输出目录，默认 artifacts/store-acceptance]
# 调用方若要在后续步骤复用导出的环境，执行（文件名按 uname -s 小写取平台名）：
#   . artifacts/store-acceptance/freebsd-duckdb.env
#   . artifacts/store-acceptance/netbsd-duckdb.env
#
# 可覆盖环境变量：
#   WHCODE_DUCKDB_PREFIX    安装前缀（默认 /usr/local/whcode-duckdb-2.0.0；隔离于系统包前缀）
#   WHCODE_DUCKDB_ARCHIVE   源码归档路径（默认 /tmp/duckdb-2.0.0.tar.gz）
#   WHCODE_DUCKDB_SRC_DIR   解包目录（默认 /tmp/duckdb-2.0.0-src）
#   WHCODE_DUCKDB_BUILD_DIR 构建目录（默认 /tmp/duckdb-2.0.0-build）
#   WHCODE_DUCKDB_JOBS      并行编译任务数（默认 ncpu；内存 < 8 GiB 时上限 2）
#   WHCODE_DUCKDB_TARBALL_SHA256 可信的源码归档 SHA-256（未提供时只记录本地计算值）

set -u

OUT_DIR=${1:-artifacts/store-acceptance}
mkdir -p "$OUT_DIR"
platform=$(uname -s 2>/dev/null | tr 'A-Z' 'a-z')
[ -n "${platform:-}" ] || platform=unknown
OUT_FILE="$OUT_DIR/$platform-store.txt"
ENV_FILE="$OUT_DIR/$platform-duckdb.env"
BUILD_LOG="/tmp/whcode-duckdb-build.$$.log"
TEST_LOG="/tmp/whcode-store-test.$$.log"
PROBE_SRC="/tmp/whcode-duckdb-probe.$$.c"
PROBE_BIN="/tmp/whcode-duckdb-probe.$$"
trap 'rm -f "$BUILD_LOG" "$TEST_LOG" "$PROBE_SRC" "$PROBE_BIN"' EXIT HUP INT TERM
: > "$BUILD_LOG"
: > "$TEST_LOG"

DUCKDB_VERSION=2.0.0
DUCKDB_TAG=v2.0.0
DUCKDB_COMMIT=d54cbbe3df17e8e5ce68312f483b4155836112aa
DUCKDB_DESCRIBE=v2.0.0-0-gd54cbbe3df
DUCKDB_REPO=https://github.com/duckdb/duckdb
DUCKDB_URL=https://github.com/duckdb/duckdb/archive/v2.0.0.tar.gz
DUCKDB_ARCHIVE_SIZE=106477075

PREFIX=${WHCODE_DUCKDB_PREFIX:-/usr/local/whcode-duckdb-2.0.0}
ARCHIVE=${WHCODE_DUCKDB_ARCHIVE:-/tmp/duckdb-2.0.0.tar.gz}
SRC_DIR=${WHCODE_DUCKDB_SRC_DIR:-/tmp/duckdb-2.0.0-src}
BUILD_DIR=${WHCODE_DUCKDB_BUILD_DIR:-/tmp/duckdb-2.0.0-build}

# rustup 安装的 cargo 不总在 root 的 PATH 里；CI 已显式导出，这里只做兜底。
if [ -d "${HOME:-/root}/.cargo/bin" ]; then
    PATH="${HOME:-/root}/.cargo/bin:$PATH"
    export PATH
fi

verdict="NO-GO"
reason="未执行"
build_status="未执行"
build_seconds="未取得"
install_seconds="未取得"
test_seconds="未取得"
archive_sha="<未取得>"
archive_sha_match="<未取得>"
archive_size="<未取得>"
tag_verification="<未取得>"
engine_version="<未取得>"
probe_rc="未执行"
engine_matches="<未取得>"
cargo_rc="未执行"
prefix_ready=0
jobs="<未取得>"
ncpu="<未取得>"
physmem="<未取得>"

u_s=$(uname -s 2>/dev/null || echo unknown)
u_r=$(uname -r 2>/dev/null || echo '<未取得证据>')
u_m=$(uname -m 2>/dev/null || echo '<未取得证据>')

sha256_of() {
    target=$1
    if [ ! -f "$target" ]; then
        echo "<未取得证据：文件不存在>"
        return
    fi
    # 工具名在各 BSD 上不一致：FreeBSD 有 sha256／sha256sum，NetBSD 主要是 sha256
    # 与 cksum -a；按可用性递减探测，全部缺失才标未取得。
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$target" 2>/dev/null | awk '{print $1}'
    elif command -v sha256 >/dev/null 2>&1; then
        sha256 "$target" 2>/dev/null | awk '{print $1}'
    elif command -v cksum >/dev/null 2>&1; then
        cksum -a sha256 "$target" 2>/dev/null | awk '{print $1}'
    else
        echo "<未取得证据：无 sha256 工具>"
    fi
}

bytes_of() {
    wc -c < "$1" 2>/dev/null | tr -d ' \n'
}

# 源码身份复核：标签的 peeled commit 必须等于固定提交；网络或 git 不可用时只记录
# “未取得”，不冒充已校验。返回 0 表示可继续，返回 1 表示身份冲突（必须 NO-GO）。
verify_tag_commit() {
    if ! command -v git >/dev/null 2>&1; then
        tag_verification="<未取得：git 不可用>"
        return 0
    fi
    if command -v timeout >/dev/null 2>&1; then
        out=$(GIT_TERMINAL_PROMPT=0 timeout 60 git ls-remote --tags "$DUCKDB_REPO" \
            "refs/tags/$DUCKDB_TAG" "refs/tags/$DUCKDB_TAG^{}" 2>/dev/null)
    else
        out=$(GIT_TERMINAL_PROMPT=0 git ls-remote --tags "$DUCKDB_REPO" \
            "refs/tags/$DUCKDB_TAG" "refs/tags/$DUCKDB_TAG^{}" 2>/dev/null)
    fi
    if [ -z "${out:-}" ]; then
        tag_verification="<未取得：git ls-remote 无输出或网络失败>"
        return 0
    fi
    peeled=$(printf '%s\n' "$out" | awk '$2 == "refs/tags/'"$DUCKDB_TAG"'^{}" {print $1}')
    direct=$(printf '%s\n' "$out" | awk '$2 == "refs/tags/'"$DUCKDB_TAG"'" {print $1}')
    got=${peeled:-$direct}
    if [ -z "${got:-}" ]; then
        tag_verification="<未取得：远端无 $DUCKDB_TAG>"
        return 0
    fi
    if [ "$got" = "$DUCKDB_COMMIT" ]; then
        tag_verification="ok:$got"
        return 0
    fi
    tag_verification="mismatch:远端 $got ≠ 固定 $DUCKDB_COMMIT"
    return 1
}

download_archive() {
    attempt=1
    while [ "$attempt" -le 3 ]; do
        rm -f "$ARCHIVE"
        if command -v fetch >/dev/null 2>&1; then
            fetch -q -o "$ARCHIVE" "$DUCKDB_URL" 2>>"$BUILD_LOG"
        elif command -v curl >/dev/null 2>&1; then
            curl -fsSL --retry 2 --connect-timeout 30 -o "$ARCHIVE" "$DUCKDB_URL" \
                2>>"$BUILD_LOG"
        else
            echo "无 fetch/curl，无法下载源码归档" >>"$BUILD_LOG"
            return 1
        fi
        # gzip -t 同时覆盖“下载被截断”这一最常见失败，而不是只看文件存在。
        if [ -s "$ARCHIVE" ] && gzip -t "$ARCHIVE" 2>/dev/null; then
            return 0
        fi
        echo "第 $attempt 次下载未通过 gzip 完整性检查，重试" >>"$BUILD_LOG"
        attempt=$((attempt + 1))
    done
    return 1
}

fetch_and_verify() {
    if ! verify_tag_commit; then
        verdict="NO-GO"
        reason="源码身份冲突：$tag_verification"
        return 1
    fi

    if [ -f "$ARCHIVE" ] && gzip -t "$ARCHIVE" 2>/dev/null; then
        echo "复用已存在的源码归档 $ARCHIVE" >>"$BUILD_LOG"
    elif ! download_archive; then
        verdict="NO-GO"
        reason="源码归档下载失败（见 build_output_tail）"
        return 1
    fi

    archive_size=$(bytes_of "$ARCHIVE")
    archive_sha=$(sha256_of "$ARCHIVE")
    expected=${WHCODE_DUCKDB_TARBALL_SHA256:-}
    if [ -n "$expected" ]; then
        if [ "$archive_sha" != "$expected" ]; then
            verdict="NO-GO"
            reason="调用方给出的 SHA-256 不匹配（期望 $expected，实际 $archive_sha）"
            return 1
        fi
        archive_sha_match="一致（调用方提供的可信摘要）"
    else
        archive_sha_match="<未提供：上游未发布该归档摘要，仅记录本地计算值>"
    fi

    rm -rf "$SRC_DIR"
    mkdir -p "$SRC_DIR"
    if ! tar -xzf "$ARCHIVE" -C "$SRC_DIR" --strip-components=1 2>>"$BUILD_LOG"; then
        verdict="NO-GO"
        reason="源码解包失败"
        return 1
    fi
    if [ ! -f "$SRC_DIR/CMakeLists.txt" ]; then
        verdict="NO-GO"
        reason="解包后缺少 CMakeLists.txt，归档结构不符合预期"
        return 1
    fi
    return 0
}

collect_host_info() {
    # 键名跨 BSD 不一致：FreeBSD 用 hw.ncpu／hw.physmem，NetBSD 常见 hw.ncpuonline／
    # hw.physmem64；逐个尝试，全部失败才标未取得（不得把缺失当成 2 核的静默默认）。
    ncpu=$(sysctl -n hw.ncpu 2>/dev/null || true)
    if [ -z "${ncpu:-}" ]; then
        ncpu=$(sysctl -n hw.ncpuonline 2>/dev/null || echo '<未取得>')
    fi
    case "$ncpu" in
        ''|*[!0-9]*) ncpu="<未取得>" ;;
    esac
    physmem=$(sysctl -n hw.physmem 2>/dev/null || true)
    if [ -z "${physmem:-}" ]; then
        physmem=$(sysctl -n hw.physmem64 2>/dev/null || echo '<未取得证据>')
    fi
}

pick_jobs() {
    case "$ncpu" in
        ''|*[!0-9]*) ncpu=2 ;;
    esac
    jobs=${WHCODE_DUCKDB_JOBS:-$ncpu}
    case "$jobs" in
        ''|*[!0-9]*) jobs=$ncpu ;;
    esac
    # DuckDB 的 unity build 单翻译单元峰值内存较高；6 GiB 级 VM 上并行度过高容易 OOM。
    case "$physmem" in
        ''|*[!0-9]*) ;;
        *)
            if [ "$physmem" -lt 8589934592 ] && [ "$jobs" -gt 2 ]; then
                jobs=2
            fi
            ;;
    esac
    [ "$jobs" -ge 1 ] || jobs=1
}

build_and_install() {
    pick_jobs
    rm -rf "$BUILD_DIR"

    echo "=== cmake 配置（DuckDB $DUCKDB_VERSION，$u_s best-effort）===" >>"$BUILD_LOG"
    # OVERRIDE_GIT_DESCRIBE 同时以环境变量与 -D 变量传入：官方文档写环境变量，
    # FreeBSD ports 用 -D 形式，两者都覆盖可避免不同版本入口差异。
    if ! OVERRIDE_GIT_DESCRIBE="$DUCKDB_DESCRIBE" cmake -G Ninja -S "$SRC_DIR" -B "$BUILD_DIR" \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX="$PREFIX" \
        -DBUILD_UNITTESTS=OFF \
        -DBUILD_SHELL=OFF \
        -DENABLE_UNITTEST_CPP_TESTS=OFF \
        -DSKIP_EXTENSIONS=jemalloc \
        -DOVERRIDE_GIT_DESCRIBE="$DUCKDB_DESCRIBE" \
        -DDUCKDB_MAJOR_VERSION=2 -DDUCKDB_MINOR_VERSION=0 -DDUCKDB_PATCH_VERSION=0 \
        >>"$BUILD_LOG" 2>&1; then
        verdict="NO-GO"
        reason="CMake 配置失败（见 build_output_tail）"
        build_status="配置失败"
        return 1
    fi

    start=$(date +%s)
    echo "=== 编译（target=duckdb jobs=$jobs）===" >>"$BUILD_LOG"
    # 只构建共享库目标：Rust `duckdb` crate 与 C 探针都只需要 libduckdb.so + duckdb.h。
    # 静态库（数倍体积）与 CMake 安装规则不参与冒烟，VM 磁盘有限时构建静态库会把
    # 安装阶段推到 ENOSPC（run 40267545791 的 FreeBSD 安装失败即此路径）。
    if ! cmake --build "$BUILD_DIR" --target duckdb --parallel "$jobs" \
        >>"$BUILD_LOG" 2>&1; then
        end=$(date +%s)
        build_seconds=$((end - start))
        verdict="NO-GO"
        reason="DuckDB 编译失败（见 build_output_tail）"
        build_status="编译失败"
        return 1
    fi
    end=$(date +%s)
    build_seconds=$((end - start))

    start=$(date +%s)
    echo "=== 安装共享库与头文件到 $PREFIX ===" >>"$BUILD_LOG"
    rm -rf "$PREFIX"
    mkdir -p "$PREFIX/lib" "$PREFIX/include"
    # 保留 libduckdb.so* 全部文件（含 SONAME 目标与符号链接），否则运行时按
    # SONAME 找不到库；只复制冒烟所需的最小集合，不跑 `cmake --install`。
    if ! cp -f "$BUILD_DIR"/src/libduckdb.so* "$PREFIX/lib/" >>"$BUILD_LOG" 2>&1 \
        || ! cp -f "$SRC_DIR/src/include/duckdb.h" "$PREFIX/include/" >>"$BUILD_LOG" 2>&1; then
        end=$(date +%s)
        install_seconds=$((end - start))
        verdict="NO-GO"
        reason="DuckDB 共享库/头文件复制失败（见 build_output_tail）"
        build_status="安装失败"
        return 1
    fi
    if [ -d "$SRC_DIR/src/include" ]; then
        cp -f "$SRC_DIR/src/include/duckdb.hpp" "$PREFIX/include/" >>"$BUILD_LOG" 2>&1 || true
    fi
    if [ ! -f "$PREFIX/include/duckdb.h" ] || ! ls "$PREFIX"/lib/libduckdb.so* >/dev/null 2>&1; then
        end=$(date +%s)
        install_seconds=$((end - start))
        verdict="NO-GO"
        reason="复制后缺少 libduckdb.so 或 duckdb.h（见 build_output_tail）"
        build_status="安装失败"
        return 1
    fi
    end=$(date +%s)
    install_seconds=$((end - start))

    {
        echo "version=$DUCKDB_VERSION"
        echo "tag=$DUCKDB_TAG"
        echo "commit=$DUCKDB_COMMIT"
        echo "describe=$DUCKDB_DESCRIBE"
        echo "archive_sha256=$archive_sha"
        echo "os_name=$u_s"
        echo "machine=$u_m"
    } > "$PREFIX/.whcode-duckdb-stamp"

    # 构建树与解包源码只用于本次安装；删除以腾出 VM 磁盘给随后的 cargo 测试。
    # 归档保留在 /tmp，便于同一 job 内重试时不重复下载。
    echo "=== 清理构建树与解包源码 ===" >>"$BUILD_LOG"
    rm -rf "$BUILD_DIR" "$SRC_DIR"
    build_status="已构建"
    return 0
}

prefix_is_ready() {
    [ -f "$PREFIX/.whcode-duckdb-stamp" ] || return 1
    [ -f "$PREFIX/include/duckdb.h" ] || return 1
    ls "$PREFIX"/lib/libduckdb.so* >/dev/null 2>&1 || return 1
    grep -q "^version=$DUCKDB_VERSION$" "$PREFIX/.whcode-duckdb-stamp" || return 1
    grep -q "^commit=$DUCKDB_COMMIT$" "$PREFIX/.whcode-duckdb-stamp" || return 1
    grep -q "^os_name=$u_s$" "$PREFIX/.whcode-duckdb-stamp" || return 1
    grep -q "^machine=$u_m$" "$PREFIX/.whcode-duckdb-stamp" || return 1
    return 0
}

export_duckdb_env() {
    DUCKDB_LIB_DIR="$PREFIX/lib"
    DUCKDB_INCLUDE_DIR="$PREFIX/include"
    LD_LIBRARY_PATH="$PREFIX/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    WHCODE_DUCKDB_VERSION="$DUCKDB_VERSION"
    export DUCKDB_LIB_DIR DUCKDB_INCLUDE_DIR LD_LIBRARY_PATH WHCODE_DUCKDB_VERSION

    {
        echo "# 由 .github/scripts/platform-matrix/bsd-duckdb.sh 生成；供后续步骤 source。"
        echo "DUCKDB_LIB_DIR='$DUCKDB_LIB_DIR'"
        echo "DUCKDB_INCLUDE_DIR='$DUCKDB_INCLUDE_DIR'"
        echo "LD_LIBRARY_PATH='$LD_LIBRARY_PATH'"
        echo "WHCODE_DUCKDB_VERSION='$WHCODE_DUCKDB_VERSION'"
        echo "WHCODE_DUCKDB_READY=1"
        echo "export DUCKDB_LIB_DIR DUCKDB_INCLUDE_DIR LD_LIBRARY_PATH WHCODE_DUCKDB_VERSION WHCODE_DUCKDB_READY"
    } > "$ENV_FILE"
}

probe_engine_version() {
    cat > "$PROBE_SRC" <<'PROBE'
#include <stdio.h>
#include <duckdb.h>
int main(void) {
    printf("%s\n", duckdb_library_version());
    return 0;
}
PROBE
    if ! cc -std=c11 -I"$PREFIX/include" "$PROBE_SRC" -L"$PREFIX/lib" -lduckdb \
        -Wl,-rpath,"$PREFIX/lib" -o "$PROBE_BIN" >>"$BUILD_LOG" 2>&1; then
        engine_version="<未取得：C 探针编译失败>"
        return 0
    fi
    engine_version=$("$PROBE_BIN" 2>>"$BUILD_LOG")
    probe_rc=$?
    [ -n "${engine_version:-}" ] || engine_version="<未取得：探针无输出>"
    case "$engine_version" in
        v2.*|2.*) engine_matches="1（v2.x，与冻结主版本一致）" ;;
        "<未取得"*) engine_matches="<未取得>" ;;
        *) engine_matches="0（不是 v2.x）" ;;
    esac
    return 0
}

run_store_tests() {
    if ! command -v cargo >/dev/null 2>&1; then
        verdict="NO-GO"
        reason="cargo 不可用，未执行 cargo test；请先按 rust-toolchain.toml 安装锁定工具链"
        return 1
    fi

    start=$(date +%s)
    CARGO_TERM_COLOR=never NO_COLOR=1 cargo test --locked -p whcode-store \
        > "$TEST_LOG" 2>&1
    cargo_rc=$?
    end=$(date +%s)
    test_seconds=$((end - start))

    if [ "$cargo_rc" -eq 0 ]; then
        if grep -Eq 'smoke_reports_engine_version_from_runtime \.\.\. ok' "$TEST_LOG" \
            && grep -Eq '^test result: ok\.' "$TEST_LOG"; then
            verdict="GO"
            reason="cargo test 通过，且验收用例 smoke_reports_engine_version_from_runtime 已执行"
        else
            verdict="NO-GO"
            reason="退出码 0 但缺少验收用例或 test result ok 行：不得算通过"
        fi
    else
        verdict="NO-GO"
        reason="cargo test 失败（退出码 $cargo_rc；详见 cargo_output）"
    fi
    return 0
}

# ---------- 主流程 ----------
collect_host_info
if prefix_is_ready; then
    prefix_ready=1
    build_status="复用已安装前缀"
    # 复用前缀时不重复下载与复核；归档摘要取自构建时写入的 stamp，并明确标注来源，
    # 避免把旧值冒充“本次重新校验”。
    archive_sha=$(awk -F= '/^archive_sha256=/ {print $2}' "$PREFIX/.whcode-duckdb-stamp" 2>/dev/null)
    [ -n "${archive_sha:-}" ] || archive_sha="<未取得：stamp 无摘要>"
    archive_sha_match="<未取得：复用前缀，摘要取自构建时 stamp>"
    tag_verification="<未取得：复用前缀，未重新复核标签提交>"
    jobs="未执行（复用前缀）"
    echo "复用已安装前缀：$PREFIX" >>"$BUILD_LOG"
elif fetch_and_verify; then
    if build_and_install; then
        prefix_ready=1
    fi
fi

if [ "$prefix_ready" -eq 1 ]; then
    export_duckdb_env
    probe_engine_version
    run_store_tests
else
    # 没有可用库时仍写一份“未执行”的环境文件，避免后续 source 一个不存在的路径。
    {
        echo "# 由 .github/scripts/platform-matrix/bsd-duckdb.sh 生成（构建未成功）。"
        echo "WHCODE_DUCKDB_READY=0"
        echo "export WHCODE_DUCKDB_READY"
    } > "$ENV_FILE"
fi

pinned_channel=$(awk -F'"' '/^channel/ {print $2}' rust-toolchain.toml 2>/dev/null | head -n 1)
[ -n "${pinned_channel:-}" ] || pinned_channel="<未取得证据>"
rustc_version=$(rustc -vV 2>/dev/null | tr '\n' ' ')
[ -n "${rustc_version:-}" ] || rustc_version="<未取得证据：rustc 不可用>"
cargo_version=$(cargo -V 2>/dev/null | head -n 1)
[ -n "${cargo_version:-}" ] || cargo_version="<未取得证据：cargo 不可用>"
cc_version=$(cc --version 2>/dev/null | head -n 1)
[ -n "${cc_version:-}" ] || cc_version="<未取得证据：cc 不可用>"
cxx_version=$(c++ --version 2>/dev/null | head -n 1)
[ -n "${cxx_version:-}" ] || cxx_version="<未取得证据：c++ 不可用>"
cmake_version=$(cmake --version 2>/dev/null | head -n 1)
[ -n "${cmake_version:-}" ] || cmake_version="<未取得证据：cmake 不可用>"
ninja_version=$(ninja --version 2>/dev/null | head -n 1)
[ -n "${ninja_version:-}" ] || ninja_version="<未取得证据：ninja 不可用>"
python_version=$(python3 --version 2>/dev/null | head -n 1)
[ -n "${python_version:-}" ] || python_version="<未取得证据：python3 不可用>"

if [ "$prefix_ready" -eq 1 ]; then
    build_gate="GO（libduckdb $DUCKDB_VERSION 已安装并可链接）"
else
    build_gate="NO-GO（$reason）"
fi
result_count=$(grep -c '^test result:' "$TEST_LOG" 2>/dev/null)
[ -n "${result_count:-}" ] || result_count=0
if [ "$result_count" -eq 0 ]; then
    result_lines="<未取得证据：无 test result 行>"
else
    result_lines=$(grep '^test result:' "$TEST_LOG" | sed 's/^/  /')
fi

{
    echo "# WHCode DuckDB 2.0.0 源码构建与 store 冒烟：$u_s 平台证据（Check 22）"
    echo "format_version = 1"
    echo "generated_at_utc = $(date -u '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null || echo '<未取得证据>')"
    echo "script = .github/scripts/platform-matrix/bsd-duckdb.sh"
    echo "source = Check 22、ADR-034（冻结引擎 2.0.0）、ADR-DB-BUILD、R-01"
    echo "scope = $u_s 为 best-effort 平台证据；按 ADR-013 延后，不进入验收结论"
    echo
    echo "[platform]"
    echo "os_name = $u_s"
    echo "os_release = $u_r"
    echo "machine = $u_m"
    echo "detailed = $(uname -a 2>/dev/null || echo '<未取得证据>')"
    if command -v freebsd-version >/dev/null 2>&1; then
        echo "freebsd_version = $(freebsd-version -ku 2>&1 | tr '\n' ' ')"
    fi
    echo "ncpu = $ncpu"
    echo "physmem_bytes = $physmem"
    echo
    echo "[toolchain]"
    echo "pinned_channel = $pinned_channel"
    echo "rustc = $rustc_version"
    echo "cargo = $cargo_version"
    echo "cc = $cc_version"
    echo "cxx = $cxx_version"
    echo "cmake = $cmake_version"
    echo "ninja = $ninja_version"
    echo "python3 = $python_version"
    echo
    echo "[duckdb_source]"
    echo "version = $DUCKDB_VERSION"
    echo "tag = $DUCKDB_TAG"
    echo "release_commit = $DUCKDB_COMMIT"
    echo "override_git_describe = $DUCKDB_DESCRIBE"
    echo "archive_url = $DUCKDB_URL"
    echo "archive_path = $ARCHIVE"
    echo "archive_size_bytes = $archive_size"
    echo "archive_size_research_reference_2026_10_25 = $DUCKDB_ARCHIVE_SIZE"
    echo "archive_sha256_local = $archive_sha"
    echo "archive_sha256_expected = $archive_sha_match"
    echo "archive_digest_upstream = <未提供：GitHub 自动源码归档无官方 SHA-256 与摘要清单>"
    echo "tag_commit_verification = $tag_verification"
    echo
    echo "[duckdb_build]"
    echo "status = $build_status"
    echo "prefix = $PREFIX"
    echo "jobs = $jobs"
    echo "cmake_options = -G Ninja -DCMAKE_BUILD_TYPE=Release -DBUILD_UNITTESTS=OFF -DBUILD_SHELL=OFF -DENABLE_UNITTEST_CPP_TESTS=OFF -DSKIP_EXTENSIONS=jemalloc -DOVERRIDE_GIT_DESCRIBE=$DUCKDB_DESCRIBE -DDUCKDB_MAJOR_VERSION=2 -DDUCKDB_MINOR_VERSION=0 -DDUCKDB_PATCH_VERSION=0"
    echo "build_seconds = $build_seconds"
    echo "install_seconds = $install_seconds"
    if [ -d "$PREFIX" ]; then
        echo "installed_libs = $(ls "$PREFIX"/lib/libduckdb.so* 2>/dev/null | tr '\n' ' ')"
    fi
    echo
    echo "[duckdb_environment]"
    echo "duckdb_lib_dir = ${DUCKDB_LIB_DIR:-<未设置>}"
    echo "duckdb_include_dir = ${DUCKDB_INCLUDE_DIR:-<未设置>}"
    echo "ld_library_path = ${LD_LIBRARY_PATH:-<未设置>}"
    echo "whcode_duckdb_version = ${WHCODE_DUCKDB_VERSION:-<未设置>}"
    echo "env_file = $ENV_FILE"
    echo
    echo "[engine_probe]"
    echo "engine_version = $engine_version"
    echo "engine_version_matches_frozen_major = $engine_matches"
    echo "probe_exit_code = $probe_rc"
    echo
    echo "[command]"
    echo "cargo_test = cargo test --locked -p whcode-store"
    echo "cargo_exit_code = $cargo_rc"
    echo "test_seconds = $test_seconds"
    echo "test_result_lines ="
    echo "$result_lines"
    echo "acceptance_test = smoke_reports_engine_version_from_runtime"
    echo
    echo "[verdict]"
    echo "store_gate = $verdict（$reason）"
    echo "build_gate = $build_gate"
    echo
    echo "[build_output_tail]"
    if [ -s "$BUILD_LOG" ]; then
        tail -n 80 "$BUILD_LOG"
    else
        echo "<未取得证据：构建未执行，无输出>"
    fi
    echo
    echo "[cargo_output]"
    if [ -s "$TEST_LOG" ]; then
        cat "$TEST_LOG"
    else
        echo "<未取得证据：cargo test 未执行，无输出>"
    fi
} > "$OUT_FILE"

echo "STORE_GATE_RESULT=$verdict（$reason）"
echo "DUCKDB_ENV_FILE=$ENV_FILE"
echo "证据：$OUT_FILE"

exit 0
