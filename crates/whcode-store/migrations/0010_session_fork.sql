-- schema v10：会话分叉（T282／FR-769／ADR-065）。
--
-- parent_id／forked_at_seq／inherited_event_count 是分叉谱系与复制范围的持久事实；
-- origin_event_id 记录被复制事件在源会话中的稳定 ID，与 session_id 组成幂等键
-- （同一目标会话不得重复复制同一源事件）。DuckDB 不支持带约束的 ADD COLUMN，
-- 因此列可空、父会话存在性由应用层校验，索引在列添加后单独创建。
ALTER TABLE sessions ADD COLUMN parent_id TEXT;
ALTER TABLE sessions ADD COLUMN forked_at_seq BIGINT;
ALTER TABLE sessions ADD COLUMN inherited_event_count BIGINT;

ALTER TABLE events ADD COLUMN origin_event_id TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_events_origin
    ON events(session_id, origin_event_id);
