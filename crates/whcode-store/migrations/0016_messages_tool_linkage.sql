-- schema v16：messages 增工具链路列（T389／FR-797／ADR-088）。
-- assistant 消息落模型声明的工具调用（VARIANT，ADR-036 JSON 语义列口径）；
-- tool 消息落关联调用 ID。写入链路 `CAST(? AS JSON)::VARIANT`，读取
-- `CAST(CAST(col AS JSON) AS VARCHAR)` 转回文本；历史数据两列为 NULL（合法空）。
-- 表挂索引先撤再建（Check 466 口径）。
DROP INDEX IF EXISTS idx_messages_session_ts_id;
ALTER TABLE messages ADD COLUMN tool_call_id TEXT;
ALTER TABLE messages ADD COLUMN tool_calls_json VARIANT;
CREATE INDEX IF NOT EXISTS idx_messages_session_ts_id ON messages(session_id, ts, id);
