-- schema v17：FTS 索引时效元数据（ADR-099）。
--
-- DuckDB 的 fts 扩展按表建索引（`create_fts_index('messages', ...)`），无会话级增量接口。
-- 本表记录最近一次建索引时的 max(messages.ts) 与行数，用于判断是否需要重建——
-- 无新消息时零成本跳过，避免每次检索都全量重建。
-- 单行表（id 恒为 1，CHECK 约束保证）。
CREATE TABLE IF NOT EXISTS fts_index_meta (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    indexed_max_ts_us BIGINT NOT NULL,
    indexed_rows      BIGINT NOT NULL,
    updated_at     TIMESTAMP NOT NULL
);
