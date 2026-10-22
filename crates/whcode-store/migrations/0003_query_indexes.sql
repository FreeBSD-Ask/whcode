-- schema v3：补齐 messages/tasks 查询索引（data-model.md §3、FR-504/506）。
--
-- messages 的游标全序是 (session_id, ts, id)。v1 的 (session_id, ts) 是该索引的前缀，
-- 保留两份会重复维护、增加写入成本，故在 v3 中删除旧索引再建三列索引。
-- tasks 概览按会话 + 阶段过滤/分组（T037），v1 只有 (session_id, status)。
--
-- 只动索引，不改表结构与数据；失败时整个事务回滚且不登记版本（NFR-103、§7.4）。

DROP INDEX IF EXISTS idx_messages_session_ts;
CREATE INDEX IF NOT EXISTS idx_messages_session_ts_id ON messages(session_id, ts, id);

CREATE INDEX IF NOT EXISTS idx_tasks_session_stage ON tasks(session_id, stage);
