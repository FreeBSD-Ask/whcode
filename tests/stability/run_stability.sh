#!/usr/bin/env bash
# 稳定性长跑入口（T116、§11.5、NFR-204）。
#
# 用法：
#   ./tests/stability/run_stability.sh [小时数]       # 默认 5.5（ADR-033；24/72 深检可选）
#   STABILITY_SMOKE=1 ./tests/stability/run_stability.sh   # 冒烟（30 秒 / 5 秒采样）
#
# 与 2026-10-24 之前版本的差异（缺陷修复）：
#   1. 采样由长驻测试进程**自行**完成并逐行追加 JSONL——不再只在 25 个样本
#      全部结束后一次性写盘，中途中断不再丢失全部数据；
#   2. 负载进程只启动**一次**并持续运行，RSS 斜率归属于同一 PID（NFR-204）；
#   3. 采样字段补齐 CPU、数据库/WAL 大小、事件缺失、failed 任务与三类延迟；
#      写者队列深度/最老队列项年龄为 null（夹具直连 Connection，缺口已登记）。
#
# 输出（不入库）：artifacts/stability-<N>h-<时间戳>/
#   timeseries.jsonl  逐样本 JSON（含 summary 行）
#   run.log           启动与进度日志
#   workspace/        长跑工作区（DuckDB 与 WAL）

set -euo pipefail

HOURS="${1:-5.5}"
SMOKE="${STABILITY_SMOKE:-0}"

if [[ "$SMOKE" == "1" ]]; then
    TOTAL_SECONDS=30
    INTERVAL_SECONDS=5
    LABEL="smoke"
else
    # 支持小数小时（默认 5.5）：shell 算术不支持浮点，统一交给 awk。
    TOTAL_SECONDS=$(awk -v hours="$HOURS" 'BEGIN { printf "%d", hours * 3600 }')
    INTERVAL_SECONDS="${STABILITY_INTERVAL_SECS:-300}"
    LABEL="${HOURS}h"
fi

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT_DIR="$ROOT/artifacts/stability-${LABEL}-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$OUT_DIR/workspace"
OUT_FILE="$OUT_DIR/timeseries.jsonl"
LOG_FILE="$OUT_DIR/run.log"

export WHCODE_STABILITY_HOURS="$HOURS"
export WHCODE_STABILITY_SECONDS="$TOTAL_SECONDS"
export WHCODE_STABILITY_INTERVAL_SECS="$INTERVAL_SECONDS"
export WHCODE_STABILITY_DIR="$OUT_DIR/workspace"
export WHCODE_STABILITY_OUT="$OUT_FILE"
export PATH="$HOME/.cargo/bin:$PATH"

echo "长跑参数：时长 ${TOTAL_SECONDS}s，采样间隔 ${INTERVAL_SECONDS}s，输出 $OUT_FILE" | tee "$LOG_FILE"
echo "判据：JSONL 逐样本落盘；event_missing 必须为 0；队列缺口以 null 标注，不以 0 冒充。" | tee -a "$LOG_FILE"

cd "$ROOT"
cargo test --locked -p whcode-tests --test stability_run -- --ignored --nocapture 2>&1 | tee -a "$LOG_FILE" &
CARGO_PID=$!
cleanup() {
    taskkill //PID "$CARGO_PID" //T //F >/dev/null 2>&1 || kill "$CARGO_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM
wait "$CARGO_PID"

if [[ ! -s "$OUT_FILE" ]]; then
    echo "失败：未产出 JSONL（$OUT_FILE）" | tee -a "$LOG_FILE"
    exit 1
fi

echo "完成：$(grep -c '"kind":"sample"' "$OUT_FILE" || true) 个样本；汇总行：" | tee -a "$LOG_FILE"
tail -1 "$OUT_FILE" | tee -a "$LOG_FILE"
