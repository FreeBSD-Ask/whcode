-- schema v13：webhook 幂等投递（P6.4／FR-788／ADR-079）。
--
-- 每行是一条“端点 × 事件”的投递事实：`idempotency_key` 为 `<endpoint>:<event_id>` 主键，
-- 认领用 `INSERT ... ON CONFLICT DO NOTHING`，重复认领影响 0 行即不再投递。
--
-- status 取值 claimed／delivered／failed（应用层枚举，库里出现未知值读取即报错）；
-- attempts 为已尝试次数，达到 max_attempts 即终态；last_status_code 与 last_error 只记
-- 传输层结果（不含请求头与凭据）。
--
-- 本表不随工作区导出（见 whcode-store::export::EXCLUDED_TABLES 与 ADR-079 决策 8）。
CREATE TABLE IF NOT EXISTS webhook_deliveries (
    idempotency_key  TEXT PRIMARY KEY,
    endpoint         TEXT NOT NULL,
    event_id         TEXT NOT NULL,
    event_seq        BIGINT NOT NULL,
    status           TEXT NOT NULL,
    attempts         BIGINT NOT NULL,
    last_status_code BIGINT,
    last_error       TEXT,
    created_at       TIMESTAMP NOT NULL DEFAULT current_timestamp,
    updated_at       TIMESTAMP NOT NULL DEFAULT current_timestamp
);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_status
    ON webhook_deliveries(status, updated_at, idempotency_key);
