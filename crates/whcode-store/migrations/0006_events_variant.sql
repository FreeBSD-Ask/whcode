-- schema v6：events.payload_json 由 TEXT 改为 VARIANT（ADR-034、data-model.md §1）。
--
-- 事件载荷是半结构化 JSON，2.0 的 VARIANT 提供原生 shredded 存储。本迁移重建
-- events 表，并用“JSON 文本 → JSON → VARIANT”的显式链路复制既有行：直接
-- ALTER COLUMN ... TYPE VARIANT 只会把文本原样包成 VARCHAR 变体，语义错误。
-- 索引随 DROP TABLE 一并消失，复制完成后按 v1／v2 的定义重建；任一步失败时
-- 整个事务回滚且不登记版本（NFR-103、§7.4）。

CREATE TABLE events_v6 (
    seq               BIGINT PRIMARY KEY,
    id                TEXT NOT NULL UNIQUE,
    session_id        TEXT,
    task_id           TEXT,
    request_id        TEXT,
    tool_call_id      TEXT,
    kind              TEXT NOT NULL,
    payload_json      VARIANT NOT NULL,
    ts                TIMESTAMP NOT NULL,
    persistence_level TEXT NOT NULL
);

INSERT INTO events_v6
SELECT seq, id, session_id, task_id, request_id, tool_call_id, kind,
       CAST(payload_json AS JSON)::VARIANT, ts, persistence_level
FROM events;

DROP TABLE events;
ALTER TABLE events_v6 RENAME TO events;

CREATE INDEX IF NOT EXISTS idx_events_session_seq ON events(session_id, seq);
CREATE INDEX IF NOT EXISTS idx_events_task ON events(task_id);
CREATE INDEX IF NOT EXISTS idx_events_request ON events(request_id);
CREATE INDEX IF NOT EXISTS idx_events_tool_call ON events(tool_call_id);
