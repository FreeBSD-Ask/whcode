#!/bin/sh
# WHCode 内置 SSH：BSD 客户端＋服务端回环验收证据（Check 25／96／99；FR-611、ADR-011、
# V-004；Task T025／T096—T098）。
#
# 用途：在 BSD（当前口径为 FreeBSD VM）内跑 `cargo test --locked -p whcode-ssh`，把版本、
# 命令、退出码与 GO／NO-GO 写进 artifacts/ssh-acceptance/<platform>-client.txt
# （FreeBSD 上即 artifacts/ssh-acceptance/freebsd-client.txt）。
#
# DuckDB 前置：`whcode-ssh` 依赖 `duckdb`（仅用 `Connection` 类型），测试二进制需链接目标平台
# 的 libduckdb。脚本按 DUCKDB_LIB_DIR → /usr/local/lib → /usr/pkg/lib 探测，命中时导出
# DUCKDB_LIB_DIR／DUCKDB_INCLUDE_DIR／LD_LIBRARY_PATH；未命中不伪造，直接按链接失败口径登记。
#
# 判据（信号链完备：交付＝可归因信号存在，而非进程不崩溃）：
#   - 退出码 0 **且** 输出同时出现 `bsd_client_acceptance_loopback ... ok` 与
#     `bsd_server_acceptance_loopback ... ok` → GO；
#   - 退出码 0 但缺任一用例 → NO-GO（用例未执行，零不等于通过）；
#   - 链接失败且与 libduckdb 相关 → `NO-GO（DuckDB 2.0 原生库不可用）`；
#   - 其他非零退出 → `NO-GO（cargo test 失败）`，附退出码。
#
# 边界：该用例是**回环**测试（进程内测试桩），证明客户端路径在本平台可运行，不构成任何
# 外部节点的连通性证据；V-004 仍属 M7，BSD 结论按 ADR-013 延后、不进入验收结论。
#
# V-004 升级口径（ADR-013 修订 2026-11-03，SSH 远程开发目标）：另跑 real_sshd_acceptance_loopback
# 对**系统自带 OpenSSH sshd**（真实进程、独立密钥对）做同机回环互操作，单独记 real_sshd_gate
# （GO／SKIPPED／NO-GO）——本轮只记录不升门（不并入 ssh_loopback_gate 硬判定），稳定后再评估；
# 跨机外部节点证据仍属 M7。
#
# 纪律：本脚本只记录、不判定交付；NO-GO 不得改写为 GO。脚本始终以 0 退出（与
# record-deps.sh 一致：结论写在证据文件与日志里），因此**不得**以退出码代替阅读证据文件。
#
# 用法：sh .github/scripts/platform-matrix/bsd-ssh-client.sh [输出目录，默认 artifacts/ssh-acceptance]

set -u

OUT_DIR=${1:-artifacts/ssh-acceptance}
mkdir -p "$OUT_DIR"

uname_s=$(uname -s 2>/dev/null || echo unknown)
platform=$(printf '%s' "$uname_s" | tr 'A-Z' 'a-z')
OUT_FILE="$OUT_DIR/$platform-client.txt"
RAW_LOG="$OUT_DIR/.$platform-client.raw.$$"
RAW_LOG2="$OUT_DIR/.$platform-client.realsshd.$$"

# 原始日志只作中转，写进证据文件后删除（临时文件不留在仓库里）。
trap 'rm -f "$RAW_LOG" "$RAW_LOG2"' EXIT HUP INT TERM
: > "$RAW_LOG"
: > "$RAW_LOG2"

# libduckdb 探测：显式环境变量优先，其次 FreeBSD／NetBSD 系统包目录。
duckdb_lib=""
for candidate_dir in "${DUCKDB_LIB_DIR:-}" /usr/local/lib /usr/pkg/lib; do
    [ -n "$candidate_dir" ] || continue
    if [ -f "$candidate_dir/libduckdb.so" ]; then
        duckdb_lib="$candidate_dir/libduckdb.so"
        export DUCKDB_LIB_DIR="$candidate_dir"
        break
    fi
done

if [ -n "${DUCKDB_LIB_DIR:-}" ] && [ -z "${DUCKDB_INCLUDE_DIR:-}" ]; then
    for candidate_inc in /usr/local/include /usr/pkg/include; do
        if [ -f "$candidate_inc/duckdb.h" ] || [ -f "$candidate_inc/duckdb.hpp" ]; then
            export DUCKDB_INCLUDE_DIR="$candidate_inc"
            break
        fi
    done
fi

# 运行期装载路径：仅当确实找到库时导出，避免设置一个指向不存在目录的假前置。
if [ -n "${DUCKDB_LIB_DIR:-}" ]; then
    LD_LIBRARY_PATH="$DUCKDB_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export LD_LIBRARY_PATH
fi

cargo_ok=0
command -v cargo >/dev/null 2>&1 && cargo_ok=1

# 版本字段先取变量再兜底：`命令 || echo` 在管道里会被 `tr`／`head` 的 0 退出码吃掉，
# 导致缺工具时留空值（空值无法与“真的没版本”区分）。
pinned_channel=$(awk -F'"' '/^channel/ {print $2}' rust-toolchain.toml 2>/dev/null | head -n 1)
[ -n "${pinned_channel:-}" ] || pinned_channel="<未取得证据>"
rustc_version=$(rustc -vV 2>/dev/null | tr '\n' ' ')
[ -n "${rustc_version:-}" ] || rustc_version="<未取得证据：rustc 不可用>"
cargo_version=$(cargo -V 2>/dev/null | head -n 1)
[ -n "${cargo_version:-}" ] || cargo_version="<未取得证据：cargo 不可用>"

verdict="NO-GO"
reason="cargo 不可用，未执行 cargo test；请先按 rust-toolchain.toml 安装锁定工具链"
rc="未执行"

if [ "$cargo_ok" -eq 1 ]; then
    # 禁用彩色输出，避免 ANSI 转义码污染证据文件。
    CARGO_TERM_COLOR=never NO_COLOR=1 cargo test --locked -p whcode-ssh > "$RAW_LOG" 2>&1
    rc=$?
    if [ "$rc" -eq 0 ]; then
        client_ok=0
        server_ok=0
        grep -Eq 'bsd_client_acceptance_loopback \.\.\. ok' "$RAW_LOG" && client_ok=1
        grep -Eq 'bsd_server_acceptance_loopback \.\.\. ok' "$RAW_LOG" && server_ok=1
        if [ "$client_ok" -eq 1 ] && [ "$server_ok" -eq 1 ]; then
            verdict="GO"
            reason="cargo test 通过，客户端与服务端回环验收用例均已执行"
        else
            verdict="NO-GO"
            reason="退出码 0 但缺少客户端或服务端验收用例（client=$client_ok server=$server_ok）：未执行不得算通过"
        fi
    elif grep -Eqi 'duckdb' "$RAW_LOG" \
        && grep -Eq 'cannot find|undefined reference|library not found|ld: error|linker' "$RAW_LOG"; then
        verdict="NO-GO"
        reason="DuckDB 2.0 原生库不可用"
    else
        verdict="NO-GO"
        reason="cargo test 失败（退出码 $rc；详见 cargo_output）"
    fi
fi

# —— 真实 sshd 互操作（V-004 CI 可达升级；ADR-013 修订 2026-11-03）——
# 前置：回环验收 GO（客户端可编译可运行才谈互操作）。单独记 real_sshd_gate，只记录不升门。
real_verdict="未执行"
real_reason="回环验收未通过或 cargo 不可用，未运行真实 sshd 互操作"
if [ "$cargo_ok" -eq 1 ] && [ "$verdict" = "GO" ]; then
    CARGO_TERM_COLOR=never NO_COLOR=1 cargo test --locked -p whcode-ssh real_sshd -- --ignored --nocapture > "$RAW_LOG2" 2>&1
    real_rc=$?
    if grep -Eq 'real_sshd_acceptance_loopback \.\.\. ok' "$RAW_LOG2" \
        && grep -Eq 'REAL_SSHD_INTEROP: GO' "$RAW_LOG2"; then
        real_verdict="GO"
        real_reason="真实 sshd 互操作通过：公钥认证/通道执行/退出码映射/stderr 捕获/TOFU/显式授权回传/超时终止"
    elif grep -Eq 'REAL_SSHD_SKIPPED' "$RAW_LOG2"; then
        real_verdict="SKIPPED"
        real_reason=$(grep -m1 'REAL_SSHD_SKIPPED' "$RAW_LOG2" | sed 's/^REAL_SSHD_SKIPPED: //')
    else
        real_verdict="NO-GO"
        real_reason="真实 sshd 互操作失败（退出码 $real_rc；详见 real_sshd_output）"
    fi
fi

{
    echo "# WHCode 内置 SSH：$uname_s 客户端＋服务端回环验收（Check 25／96／99）"
    echo "format_version = 1"
    echo "generated_at_utc = $(date -u '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null || echo '<未取得证据>')"
    echo "script = .github/scripts/platform-matrix/bsd-ssh-client.sh"
    echo "source = Task T025、FR-611、ADR-011、V-004"
    echo
    echo "[platform]"
    echo "os_name = $uname_s"
    echo "os_release = $(uname -r 2>/dev/null || echo '<未取得证据>')"
    echo "machine = $(uname -m 2>/dev/null || echo '<未取得证据>')"
    echo "detailed = $(uname -a 2>/dev/null || echo '<未取得证据>')"
    if command -v freebsd-version >/dev/null 2>&1; then
        echo "freebsd_version = $(freebsd-version -ku 2>&1 | tr '\n' ' ')"
    fi
    echo
    echo "[toolchain]"
    echo "pinned_channel = $pinned_channel"
    echo "rustc = $rustc_version"
    echo "cargo = $cargo_version"
    echo
    echo "[environment]"
    echo "duckdb_native_library = ${duckdb_lib:-<未找到>}"
    echo "duckdb_lib_dir = ${DUCKDB_LIB_DIR:-<未设置>}"
    echo "duckdb_include_dir = ${DUCKDB_INCLUDE_DIR:-<未设置>}"
    echo "ld_library_path = ${LD_LIBRARY_PATH:-<未设置>}"
    echo
    echo "[command]"
    echo "cargo_test = cargo test --locked -p whcode-ssh"
    echo "cargo_exit_code = $rc"
    echo
    echo "[verdict]"
    echo "ssh_loopback_gate = $verdict（$reason）"
    echo "client_test = bsd_client_acceptance_loopback"
    echo "server_test = bsd_server_acceptance_loopback"
    echo "covers = 客户端公钥认证/受限命令/输出上限/超时断连；服务端认证/会话隔离/resize/安全退出"
    echo "scope = 回环（进程内测试桩）；不构成任何外部节点连通性证据"
    echo "v_004 = 未取得（M7；ADR-013 延后，BSD 结论不进入验收）"
    echo
    echo "[real_sshd]"
    echo "real_sshd_gate = $real_verdict（$real_reason）"
    echo "server = 系统自带 OpenSSH sshd（真实进程、独立密钥对、最小配置、真实 TCP 回环）"
    echo "covers = 公钥认证/通道执行/退出码映射/stderr 捕获/TOFU/显式授权内容回传/超时终止"
    echo "scope = 同机回环互操作；不是跨机外部节点证据（V-004 跨机口径仍属 M7）"
    echo "gate_policy = 只记录不升门（不并入 ssh_loopback_gate 硬判定；稳定后经评估再升）"
    echo
    echo "[real_sshd_output]"
    if [ -s "$RAW_LOG2" ]; then
        cat "$RAW_LOG2"
    else
        echo "<未取得证据：真实 sshd 互操作未执行>"
    fi
    echo
    echo "[cargo_output]"
    if [ -s "$RAW_LOG" ]; then
        cat "$RAW_LOG"
    else
        echo "<未取得证据：cargo test 未执行，无输出>"
    fi
} > "$OUT_FILE"

echo "SSH_CLIENT_GATE_RESULT=$verdict（$reason）"
echo "REAL_SSHD_GATE_RESULT=$real_verdict（$real_reason）"
echo "证据：$OUT_FILE"

exit 0
