-- schema v2：事件追溯索引（data-model.md §3、FR-204）。
--
-- 一次调用需按 request_id / tool_call_id 串联事件；v1 只建了 session/task 索引。
-- 本迁移只加索引，不改表结构与数据，失败时整个事务回滚且不登记版本（NFR-103、§7.4）。

CREATE INDEX IF NOT EXISTS idx_events_request ON events(request_id);
CREATE INDEX IF NOT EXISTS idx_events_tool_call ON events(tool_call_id);
