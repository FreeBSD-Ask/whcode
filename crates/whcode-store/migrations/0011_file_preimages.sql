-- schema v11：文件前像（T289／FR-776／ADR-068）。
--
-- 每次成功的 write_file／edit_file 调用登记一条“改前内容”快照（只存文本、有界），
-- 供 session restore 预览与恢复。超限或非 UTF-8 的文件不存内容，只登记跳过原因（不静默）。
-- content 与 skipped_reason 恰有一个非空；由应用层保证。
CREATE TABLE IF NOT EXISTS file_preimages (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    exec_id TEXT NOT NULL,
    path TEXT NOT NULL,
    size_bytes BIGINT NOT NULL,
    content TEXT,
    skipped_reason TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT current_timestamp
);

CREATE INDEX IF NOT EXISTS idx_file_preimages_session
    ON file_preimages(session_id, created_at, id);
