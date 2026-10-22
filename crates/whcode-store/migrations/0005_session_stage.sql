-- DuckDB 1.5.6 不支持带约束的 ADD COLUMN（"Adding columns with constraints
-- not yet supported"）：先加可空列再回填默认值；NULL 视为 'specify'（应用层兜底）。
ALTER TABLE sessions ADD COLUMN stage TEXT;
UPDATE sessions SET stage = 'specify' WHERE stage IS NULL;
