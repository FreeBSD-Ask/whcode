-- schema v15：任务充分性计算报告（FR-795／ADR-085）。
--
-- 一次计算一条记录：阶段（pre／mid／end）、可判定片段的结论（satisfied／counterexample）、
-- 逐条计数与反例明细。反例明细是半结构化 JSON，按 ADR-036 用 VARIANT；写入链路为
-- `CAST(? AS JSON)::VARIANT`，读取时 `CAST(CAST(col AS JSON) AS VARCHAR)` 转回文本。
--
-- 语义校验（阶段与结论的封闭枚举、文本长度、计数一致性）留在 `whcode-app::coherence`；
-- 数据库只保证 NOT NULL 与类型（ADR-018 分层，与 0012／0014 同口径）。
CREATE SEQUENCE IF NOT EXISTS whcode_coherence_run_id_seq;

CREATE TABLE IF NOT EXISTS coherence_runs (
    id                   TEXT PRIMARY KEY,
    session_id           TEXT,
    phase                TEXT NOT NULL,
    verdict              TEXT NOT NULL,
    requirement          TEXT NOT NULL,
    total                BIGINT NOT NULL,
    counterexamples      BIGINT NOT NULL,
    unknowns             BIGINT NOT NULL,
    counterexamples_json VARIANT NOT NULL,
    created_at           TIMESTAMP NOT NULL DEFAULT current_timestamp
);

CREATE INDEX IF NOT EXISTS idx_coherence_runs_session
    ON coherence_runs(session_id, created_at, id);
CREATE INDEX IF NOT EXISTS idx_coherence_runs_phase
    ON coherence_runs(phase, created_at, id);
