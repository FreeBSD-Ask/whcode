-- schema v7：其余 6 个 JSON 语义列由 TEXT 改为 VARIANT（ADR-036、data-model.md §1）。
--
-- events.payload_json 已在 v6 转换（ADR-034）。本迁移把 tasks.acceptance_json、
-- stage_artifacts.refs_json、session_overview.by_stage_json、checkpoints.delta_json、
-- checkpoints.artifact_refs_json、audit.detail_json 一并转为原生 VARIANT。
--
-- 与 v6 相同：ALTER COLUMN ... TYPE VARIANT 不会解析 JSON 文本（只会包成 VARCHAR 变体），
-- 因此逐表重建并用“JSON 文本 → JSON → VARIANT”复制。非法 JSON 由 CAST 报错中止；
-- 顶层 JSON null 会得到 VARIANT NULL 并被 NOT NULL 拒绝——两种失败都回滚整个事务，
-- 不登记版本（NFR-103、§7.4）。
--
-- 索引随 DROP TABLE 消失，复制后按现状重建：tasks 两条、checkpoints 一条、audit 一条；
-- stage_artifacts 与 session_overview 无二级索引，仅主键。
-- stage_artifacts.content 与 m0_events.payload_json 保持 TEXT（任意文本，ADR-036 边界）。

CREATE TABLE tasks_v7 (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL,
    title           TEXT NOT NULL,
    stage           TEXT NOT NULL,
    status          TEXT NOT NULL,
    parent_id       TEXT,
    acceptance_json VARIANT NOT NULL,
    retry_count     INTEGER NOT NULL DEFAULT 0,
    updated_at      TIMESTAMP NOT NULL
);

INSERT INTO tasks_v7
SELECT id, session_id, title, stage, status, parent_id,
       CAST(acceptance_json AS JSON)::VARIANT, retry_count, updated_at
FROM tasks;

DROP TABLE tasks;
ALTER TABLE tasks_v7 RENAME TO tasks;
CREATE INDEX IF NOT EXISTS idx_tasks_session_status ON tasks(session_id, status);
CREATE INDEX IF NOT EXISTS idx_tasks_session_stage ON tasks(session_id, stage);

CREATE TABLE stage_artifacts_v7 (
    id             TEXT PRIMARY KEY,
    session_id     TEXT NOT NULL,
    task_id        TEXT,
    stage          TEXT NOT NULL,
    artifact_type  TEXT NOT NULL,
    content        TEXT NOT NULL,
    refs_json      VARIANT NOT NULL,
    parser_version TEXT NOT NULL,
    validated      BOOLEAN NOT NULL,
    version        INTEGER NOT NULL,
    created_at     TIMESTAMP NOT NULL
);

INSERT INTO stage_artifacts_v7
SELECT id, session_id, task_id, stage, artifact_type, content,
       CAST(refs_json AS JSON)::VARIANT, parser_version, validated, version, created_at
FROM stage_artifacts;

DROP TABLE stage_artifacts;
ALTER TABLE stage_artifacts_v7 RENAME TO stage_artifacts;

CREATE TABLE session_overview_v7 (
    session_id    TEXT PRIMARY KEY,
    by_stage_json VARIANT NOT NULL,
    total         BIGINT NOT NULL,
    done          BIGINT NOT NULL,
    blocked       BIGINT NOT NULL,
    refreshed_at  TIMESTAMP NOT NULL
);

INSERT INTO session_overview_v7
SELECT session_id, CAST(by_stage_json AS JSON)::VARIANT, total, done, blocked, refreshed_at
FROM session_overview;

DROP TABLE session_overview;
ALTER TABLE session_overview_v7 RENAME TO session_overview;

CREATE TABLE checkpoints_v7 (
    id                 TEXT PRIMARY KEY,
    task_id            TEXT NOT NULL,
    step               INTEGER NOT NULL,
    delta_json         VARIANT NOT NULL,
    message_cursor     TEXT,
    artifact_refs_json VARIANT NOT NULL,
    ts                 TIMESTAMP NOT NULL
);

INSERT INTO checkpoints_v7
SELECT id, task_id, step, CAST(delta_json AS JSON)::VARIANT, message_cursor,
       CAST(artifact_refs_json AS JSON)::VARIANT, ts
FROM checkpoints;

DROP TABLE checkpoints;
ALTER TABLE checkpoints_v7 RENAME TO checkpoints;
CREATE INDEX IF NOT EXISTS idx_checkpoints_task ON checkpoints(task_id);

CREATE TABLE audit_v7 (
    id          TEXT PRIMARY KEY,
    actor       TEXT NOT NULL,
    action      TEXT NOT NULL,
    target      TEXT NOT NULL,
    detail_json VARIANT NOT NULL,
    ts          TIMESTAMP NOT NULL
);

INSERT INTO audit_v7
SELECT id, actor, action, target, CAST(detail_json AS JSON)::VARIANT, ts
FROM audit;

DROP TABLE audit;
ALTER TABLE audit_v7 RENAME TO audit;
CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit(ts);
