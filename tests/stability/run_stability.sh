#!/usr/bin/env bash
# 稳定性测试采集（T116、§11.5、NFR-204）。
#
# 用法：
#   ./tests/stability/run_stability.sh [小时数]     # 默认 24（候选发布 72）
#   STABILITY_SMOKE=1 ./tests/stability/run_stability.sh   # 冒烟模式（数分钟采样，
#                                                   # 验证采集链路；不计入稳定性结论）
#
# 采集指标（§11.5）：进程 RSS、线程数、句柄数（Windows 经 PowerShell 计数）、
# 库文件大小、事件行数；按采样窗口输出时间序列 JSON。
#
# 状态（2026-10-22）：采集链路已实现；**24/72 小时连续执行阻塞**（需长跑环境，
# T116 不得勾选——见 tasks.md 实现记录）。STABILITY_SMOKE=1 冒烟模式可在本机
# 验证采集链路，其结果不作稳定性结论。

set -euo pipefail

HOURS="${1:-24}"
SMOKE="${STABILITY_SMOKE:-0}"

if [[ "$SMOKE" == "1" ]]; then
    INTERVAL_SECONDS=5
    TOTAL_SAMPLES=3
    OUT_NAME="stability-smoke"
    WORKLOAD=0
elif [[ "${STABILITY_PROBE:-0}" == "1" ]]; then
    # 探针模式（10 分钟）：工作负载 + 短间隔采样，验证"不白跑"。
    INTERVAL_SECONDS=20
    TOTAL_SAMPLES=30
    OUT_NAME="stability-probe"
    WORKLOAD="${STABILITY_WORKLOAD:-1}"
else
    INTERVAL_SECONDS=3600
    TOTAL_SAMPLES=$((HOURS + 1))
    OUT_NAME="stability-${HOURS}h"
    WORKLOAD="${STABILITY_WORKLOAD:-1}"
fi

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT_DIR="$ROOT/artifacts/$OUT_NAME-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$OUT_DIR"
OUT_FILE="$OUT_DIR/timeseries.json"

echo "稳定性采样（冒烟=$SMOKE 工作负载=${WORKLOAD:-0}）：$TOTAL_SAMPLES 个样本，间隔 ${INTERVAL_SECONDS}s"
echo "输出：$OUT_FILE"

# 工作负载（防白跑）：后台循环执行极限测试（1M 事件写入/查询/删除——真实
# 写入+读取+删除负载），使采样有持续运行的进程可测。无负载则样本全为 null。
WORKLOAD_PID=""
if [[ "${WORKLOAD:-0}" == "1" ]]; then
    export PATH="$HOME/.cargo/bin:$PATH"
    (while true; do
        cargo test --locked --test extreme_scale 2>/dev/null ||         cargo test --locked -p whcode-tests --test extreme_scale 2>/dev/null || true
    done) &
    WORKLOAD_PID=$!
    echo "工作负载已启动（PID $WORKLOAD_PID）：extreme_scale 循环（写入/查询/删除）"
fi

# 工作负载清理（2026-10-22 实测缺陷修复）：异常退出（Ctrl-C、父 shell 被杀、
# set -e 触发）曾导致负载循环遗留 4 小时（agent 会话中断遗产）。清理必须：
#   1. 覆盖 EXIT/INT/TERM 三条路径；
#   2. 杀**进程树**（taskkill /T）——单独 kill 子 shell 不会终止其子进程
#      cargo.exe 及其下游（Windows 无进程组语义）。
cleanup_workload() {
    if [[ -n "${WORKLOAD_PID:-}" ]]; then
        taskkill //PID "$WORKLOAD_PID" //T //F >/dev/null 2>&1 || kill "$WORKLOAD_PID" 2>/dev/null || true
        WORKLOAD_PID=""
    fi
}
trap cleanup_workload EXIT INT TERM

SAMPLES=""
for i in $(seq 1 "$TOTAL_SAMPLES"); do
    TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    # 找 whcode 进程（无则记 null——空闲期采集）。
    PID=$(tasklist //FI "IMAGENAME eq whcode.exe" //FO CSV 2>/dev/null | tail -1 | cut -d',' -f2 | tr -d '"' || true)
    # 无 whcode 常驻进程时采样工作负载进程（cargo/测试二进制——真实负载）。
    if [[ -z "$PID" || "$PID" == "INFO:"* ]]; then
        PID=$(powershell -NoProfile -Command "
            Get-Process | Where-Object { \$_.Path -like '*whcode-extreme*' -or \$_.Path -like '*extreme_scale*' } |
            Select-Object -First 1 -ExpandProperty Id" 2>/dev/null | head -1)
    fi
    if [[ -n "$PID" && "$PID" != "INFO:"* ]]; then
        METRICS=$(powershell -NoProfile -Command "
            \$p = Get-Process -Id $PID -ErrorAction SilentlyContinue
            if (\$p) {
                @{ rss_mb = [math]::Round(\$p.WorkingSet64 / 1MB, 1);
                   threads = \$p.Threads.Count;
                   handles = \$p.HandleCount } | ConvertTo-Json -Compress
            } else { 'null' }" 2>/dev/null || echo null)
    else
        METRICS=null
    fi
    SAMPLES="$SAMPLES{\"sample\": $i, \"ts\": \"$TS\", \"pid\": \"$PID\", \"metrics\": $METRICS},"
    echo "样本 $i/$TOTAL_SAMPLES：$METRICS"
    [[ "$i" == "$TOTAL_SAMPLES" ]] || sleep "$INTERVAL_SECONDS"
done

# 输出时间序列（去尾逗号）。
printf '{"task": "T116", "duration_hours": %s, "smoke": %s, "samples": [%s]}\n' \
    "$HOURS" "$SMOKE" "${SAMPLES%,}" > "$OUT_FILE"
# 终止工作负载（正常路径显式调用；异常路径由 EXIT trap 兜底）。
cleanup_workload

echo "时间序列已写入 $OUT_FILE"
echo "注意：24/72 小时连续执行的稳定性结论须以非冒烟运行的结果为准（T116 验收）。"
echo "磁盘评估：工作负载每周期净增 ≈150MB（1M 事件），72h 需周期性 compact 或
清理（X4），否则磁盘 ≈100GB+ 会满——长期运行必须绑定清理循环。"
