-- v9：工具级审批规则（T256／T257／FR-745；本地规则存储，可在设置页撤销）。
--
-- 只存 allow / deny：ask 是“每次都问”的缺省行为，不需要持久化；
-- 一条工具一行（主键），规则变更用 UPSERT，撤销即删除。
CREATE TABLE IF NOT EXISTS approval_rules (
    tool          TEXT PRIMARY KEY,
    rule          TEXT NOT NULL CHECK (rule IN ('allow', 'deny')),
    created_at_ms BIGINT NOT NULL,
    updated_at_ms BIGINT NOT NULL
);
