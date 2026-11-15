-- schema v12：评审发现台账（P2.2／FR-786／ADR-080）。
--
-- finding 四要件（缺口／影响／最小修复／验证路线）为必填文本，非空与长度上限由应用层校验
-- （数据库只保证 NOT NULL，语义校验留在 `whcode-app::findings`，ADR-018 分层）。
--
-- version 用于“修复乐观锁”（ADR-080 决策 3）：更新一律带 `WHERE version = ?`，
-- 并发修改时后到者得到版本冲突而不是静默覆盖。status 取值 open／fixing／fixed／dismissed，
-- severity 取值 low／medium／high（应用层枚举，避免库里出现拼写漂移）。
CREATE SEQUENCE IF NOT EXISTS whcode_finding_id_seq;

CREATE TABLE IF NOT EXISTS findings (
    id           TEXT PRIMARY KEY,
    session_id   TEXT,
    title        TEXT NOT NULL,
    gap          TEXT NOT NULL,
    impact       TEXT NOT NULL,
    owner_fix    TEXT NOT NULL,
    verification TEXT NOT NULL,
    status       TEXT NOT NULL,
    severity     TEXT NOT NULL,
    version      BIGINT NOT NULL,
    created_at   TIMESTAMP NOT NULL DEFAULT current_timestamp,
    updated_at   TIMESTAMP NOT NULL DEFAULT current_timestamp,
    fixed_at     TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_findings_status ON findings(status, created_at, id);
