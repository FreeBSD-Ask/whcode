-- schema v14：学习捕获台账（P2.4／FR-794／ADR-084）。
--
-- 三件事：① findings 增加复发归因与计数控（pattern／recurrence_count）与 dismissed 原因码
-- （dismiss_reason，ADR-080 修订：负面控制——"为过关而过关"必须有封闭原因）；
-- ② 新表 interventions（干预台账）：四要件与 findings 同构（pattern／action／expected_effect／
-- verification 必填），outcome 由 store 在 conclude 时按 before/after 复发计数**自动判定**，
-- 不接受人工指定（防自证，ADR-084 决策 4）；
-- ③ 语义校验（非空、长度、封闭枚举）留在 `whcode-app`，数据库只保证 NOT NULL 与类型
-- （ADR-018 分层，与 0012 同口径）。
--
-- DuckDB 限制：表上挂着索引时 `ALTER TABLE ... ADD/DROP COLUMN` 会报
-- “Cannot alter entry ... because there are entries that depend on it”——先撤
-- idx_findings_status，加列后按原定义重建（0012 的定义原样搬回）。
DROP INDEX IF EXISTS idx_findings_status;

ALTER TABLE findings ADD COLUMN pattern TEXT;
ALTER TABLE findings ADD COLUMN recurrence_count BIGINT NOT NULL DEFAULT 0;
ALTER TABLE findings ADD COLUMN dismiss_reason TEXT;

CREATE INDEX IF NOT EXISTS idx_findings_status ON findings(status, created_at, id);

CREATE SEQUENCE IF NOT EXISTS whcode_intervention_id_seq;

CREATE TABLE IF NOT EXISTS interventions (
    id                  TEXT PRIMARY KEY,
    session_id          TEXT,
    pattern             TEXT NOT NULL,
    action              TEXT NOT NULL,
    expected_effect     TEXT NOT NULL,
    verification        TEXT NOT NULL,
    status              TEXT NOT NULL,
    outcome             TEXT,
    target_finding_id   TEXT,
    recurrences_before  BIGINT NOT NULL DEFAULT 0,
    recurrences_after   BIGINT,
    version             BIGINT NOT NULL,
    created_at          TIMESTAMP NOT NULL DEFAULT current_timestamp,
    updated_at          TIMESTAMP NOT NULL DEFAULT current_timestamp,
    concluded_at        TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_interventions_status ON interventions(status, created_at, id);
CREATE INDEX IF NOT EXISTS idx_interventions_pattern ON interventions(pattern, status, id);
