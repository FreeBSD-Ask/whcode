#!/bin/sh
# WHCode BSD 量化测量记录（ADR-025 性能证据车道扩展；Check 195／NFR-201—210 平台口径）。
#
# 用途：在 BSD VM 内运行**记录型**测量——bench baseline（写入基线）、概览查询 p50/p95/p99
# （test_overview_perf）、会话切换响应（test_nfr_budget nfr207）——写入
# artifacts/bsd-perf/<platform>-perf.txt 与同目录 JSON／日志，供平台证据归档。
#
# 口径（不得误读）：
#   - **只记录，不判定**：冻结预算（ADR-025）的范围是 Windows 11 amd64（唯一验收目标）；
#     BSD 数值是 VM 共享环境（4 核、邻居负载不可控）的受控测量信号，任何一项超预算
#     都不构成违约，也不据此改预算；
#   - 构建剖面与 VM 环境如实写入证据（debug 构建、ncpu、内存），信噪口径见 [scope]；
#   - DuckDB 原生库缺失（如 NetBSD 上游阻塞）时显式 SKIPPED，不伪造测量；
#   - 本脚本始终以 0 退出（与 bsd-ssh-client.sh 同纪律：结论写在证据文件里），
#     **不得**以退出码代替阅读证据文件。
#
# 用法：sh .github/scripts/platform-matrix/bsd-perf.sh [输出目录，默认 artifacts/bsd-perf]

set -u

OUT_DIR=${1:-artifacts/bsd-perf}
mkdir -p "$OUT_DIR"

uname_s=$(uname -s 2>/dev/null || echo unknown)
platform=$(printf '%s' "$uname_s" | tr 'A-Z' 'a-z')
OUT_FILE="$OUT_DIR/$platform-perf.txt"
RAW_LOG="$OUT_DIR/.$platform-perf.raw.$$"
BASELINE_JSON="$OUT_DIR/$platform-baseline.json"

trap 'rm -f "$RAW_LOG"' EXIT HUP INT TERM
: > "$RAW_LOG"

# libduckdb 探测：显式环境变量优先，其次 job 已产出的 env（freebsd-duckdb.env）与系统包目录。
duckdb_lib=""
if [ -z "${DUCKDB_LIB_DIR:-}" ] && [ -f "artifacts/store-acceptance/$platform-duckdb.env" ]; then
    # shellcheck disable=SC1090,SC1091
    . "artifacts/store-acceptance/$platform-duckdb.env"
fi
for candidate_dir in "${DUCKDB_LIB_DIR:-}" /usr/local/lib /usr/pkg/lib; do
    [ -n "$candidate_dir" ] || continue
    if [ -f "$candidate_dir/libduckdb.so" ]; then
        duckdb_lib="$candidate_dir/libduckdb.so"
        export DUCKDB_LIB_DIR="$candidate_dir"
        break
    fi
done
if [ -n "${DUCKDB_LIB_DIR:-}" ]; then
    LD_LIBRARY_PATH="$DUCKDB_LIB_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export LD_LIBRARY_PATH
fi

cargo_ok=0
command -v cargo >/dev/null 2>&1 && cargo_ok=1

verdict="SKIPPED"
reason="前置缺失：未满足运行条件（见 evidence 字段），未执行任何测量"

if [ "$cargo_ok" -eq 1 ] && [ -n "$duckdb_lib" ]; then
    verdict="RECORDED"
    reason="三项记录型测量已执行（bench baseline／概览 p95／NFR-207）；只记录不判定"

    # 1. 写入基线（whcode bench baseline：固定数据集与采样方法，T027／V-006）。
    CARGO_TERM_COLOR=never NO_COLOR=1 cargo run --locked -p whcode-bin -- \
        bench baseline --out "$BASELINE_JSON" > "$RAW_LOG" 2>&1
    baseline_rc=$?
    if [ "$baseline_rc" -ne 0 ]; then
        verdict="PARTIAL"
        reason="bench baseline 失败（退出码 $baseline_rc），后续测量照跑；见 [logs]"
    fi

    # 2. 概览查询分位数（1 万任务，p50/p95/p99 微秒；测试自身会写 overview-perf-m2.json）。
    CARGO_TERM_COLOR=never NO_COLOR=1 cargo test --locked -p whcode-tests --test overview_perf -- \
        --ignored --nocapture >> "$RAW_LOG" 2>&1
    overview_rc=$?

    # 3. NFR-207 会话切换响应（10 万事件，p50/p95/p99 毫秒）。
    CARGO_TERM_COLOR=never NO_COLOR=1 cargo test --locked -p whcode-tests --test nfr_budget nfr207 -- \
        --ignored --nocapture >> "$RAW_LOG" 2>&1
    nfr207_rc=$?

    if [ "$verdict" = "RECORDED" ] && { [ "$overview_rc" -ne 0 ] || [ "$nfr207_rc" -ne 0 ]; }; then
        verdict="PARTIAL"
        reason="部分测量失败（overview=$overview_rc nfr207=$nfr207_rc）；失败项按无证据登记"
    fi
elif [ "$cargo_ok" -ne 1 ]; then
    reason="前置缺失：cargo 不可用"
else
    reason="前置缺失：libduckdb 原生库不可用（如 NetBSD 上游 DuckDB 2.0.0 平台缺口），不伪造测量"
fi

pinned_channel=$(awk -F'"' '/^channel/ {print $2}' rust-toolchain.toml 2>/dev/null | head -n 1)
[ -n "${pinned_channel:-}" ] || pinned_channel="<未取得证据>"
ncpu=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo "<未取得证据>")
physmem=$(sysctl -n hw.physmem 2>/dev/null || echo "<未取得证据>")

{
    echo "# WHCode BSD 量化测量记录（$uname_s；ADR-025 证据车道扩展）"
    echo "format_version = 1"
    echo "generated_at_utc = $(date -u '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null || echo '<未取得证据>')"
    echo "script = .github/scripts/platform-matrix/bsd-perf.sh"
    echo "source = Check 195／NFR-201—210、ADR-025、ADR-013 修订 2026-11-03"
    echo
    echo "[platform]"
    echo "os_name = $uname_s"
    echo "os_release = $(uname -r 2>/dev/null || echo '<未取得证据>')"
    echo "machine = $(uname -m 2>/dev/null || echo '<未取得证据>')"
    echo "detailed = $(uname -a 2>/dev/null || echo '<未取得证据>')"
    echo "ncpu = $ncpu"
    echo "physmem_bytes = $physmem"
    echo
    echo "[toolchain]"
    echo "pinned_channel = $pinned_channel"
    echo "rustc = $(rustc -vV 2>/dev/null | tr '\n' ' ' || echo '<未取得证据>')"
    echo "build_profile = debug（与所在 job 的测试构建一致；非 release 口径）"
    echo
    echo "[environment]"
    echo "duckdb_native_library = ${duckdb_lib:-<未找到>}"
    echo "duckdb_lib_dir = ${DUCKDB_LIB_DIR:-<未设置>}"
    echo
    echo "[verdict]"
    echo "perf_record_gate = $verdict（$reason）"
    echo "budget_scope = 冻结预算（ADR-025）范围＝Windows 11 amd64；BSD 数值不参与预算判定"
    echo "scope = VM 共享环境受控测量信号（邻居负载不可控、debug 构建）；信噪口径以本节为准"
    echo
    echo "[artifacts]"
    echo "baseline_json = $([ -s "$BASELINE_JSON" ] && echo "$BASELINE_JSON" || echo '<未产出>')"
    echo "overview_report = artifacts/overview-perf-m2.json（overview_perf 测试自写；成功时）"
    echo
    echo "[logs]"
    if [ -s "$RAW_LOG" ]; then
        cat "$RAW_LOG"
    else
        echo "<未取得证据：无测量输出>"
    fi
} > "$OUT_FILE"

echo "BSD_PERF_RECORD_RESULT=$verdict（$reason）"
echo "证据：$OUT_FILE"

exit 0
