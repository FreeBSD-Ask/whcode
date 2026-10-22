CREATE TABLE IF NOT EXISTS workspace_meta (
    workspace_id   TEXT PRIMARY KEY,
    schema_version BIGINT NOT NULL,
    created_at     TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    id             TEXT PRIMARY KEY,
    title          TEXT NOT NULL,
    created_at     TIMESTAMP NOT NULL,
    last_active_at TIMESTAMP NOT NULL,
    model          TEXT,
    archived       BOOLEAN NOT NULL DEFAULT FALSE,
    status         TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tasks (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL,
    title           TEXT NOT NULL,
    stage           TEXT NOT NULL,
    status          TEXT NOT NULL,
    parent_id       TEXT,
    acceptance_json TEXT NOT NULL,
    retry_count     INTEGER NOT NULL DEFAULT 0,
    updated_at      TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS stage_artifacts (
    id             TEXT PRIMARY KEY,
    session_id     TEXT NOT NULL,
    task_id        TEXT,
    stage          TEXT NOT NULL,
    artifact_type  TEXT NOT NULL,
    content        TEXT NOT NULL,
    refs_json      TEXT NOT NULL,
    parser_version TEXT NOT NULL,
    validated      BOOLEAN NOT NULL,
    version        INTEGER NOT NULL,
    created_at     TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    seq               BIGINT PRIMARY KEY,
    id                TEXT NOT NULL UNIQUE,
    session_id        TEXT,
    task_id           TEXT,
    request_id        TEXT,
    tool_call_id      TEXT,
    kind              TEXT NOT NULL,
    payload_json      TEXT NOT NULL,
    ts                TIMESTAMP NOT NULL,
    persistence_level TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL,
    role        TEXT NOT NULL,
    content_ref TEXT NOT NULL,
    token_count INTEGER,
    ts          TIMESTAMP NOT NULL,
    parent_id   TEXT
);

CREATE TABLE IF NOT EXISTS context_windows (
    id               TEXT PRIMARY KEY,
    session_id       TEXT NOT NULL,
    window_start_seq BIGINT NOT NULL,
    window_end_seq   BIGINT NOT NULL,
    total_tokens     BIGINT NOT NULL,
    summary_ref      TEXT,
    created_at       TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS session_overview (
    session_id    TEXT PRIMARY KEY,
    by_stage_json TEXT NOT NULL,
    total         BIGINT NOT NULL,
    done          BIGINT NOT NULL,
    blocked       BIGINT NOT NULL,
    refreshed_at  TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS checkpoints (
    id                 TEXT PRIMARY KEY,
    task_id            TEXT NOT NULL,
    step               INTEGER NOT NULL,
    delta_json         TEXT NOT NULL,
    message_cursor     TEXT,
    artifact_refs_json TEXT NOT NULL,
    ts                 TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS tool_executions (
    id              TEXT PRIMARY KEY,
    tool_call_id    TEXT NOT NULL,
    state           TEXT NOT NULL,
    input_hash      TEXT NOT NULL,
    started_at      TIMESTAMP,
    finished_at     TIMESTAMP,
    output_ref      TEXT,
    side_effect_ref TEXT
);

CREATE TABLE IF NOT EXISTS audit (
    id          TEXT PRIMARY KEY,
    actor       TEXT NOT NULL,
    action      TEXT NOT NULL,
    target      TEXT NOT NULL,
    detail_json TEXT NOT NULL,
    ts          TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS runtime_state (
    name         TEXT PRIMARY KEY,
    enabled      BOOLEAN NOT NULL,
    backend      TEXT NOT NULL,
    version      TEXT NOT NULL,
    installed_at TIMESTAMP,
    payload_path TEXT
);

CREATE TABLE IF NOT EXISTS runtime_executions (
    id                  TEXT PRIMARY KEY,
    task_id             TEXT NOT NULL,
    runtime             TEXT NOT NULL,
    script_hash         TEXT NOT NULL,
    input_hash          TEXT NOT NULL,
    runtime_fingerprint TEXT NOT NULL,
    state               TEXT NOT NULL,
    exit_code           INTEGER,
    duration_ms         BIGINT,
    peak_mb             BIGINT,
    stdout_ref          TEXT,
    stderr_ref          TEXT,
    ts                  TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS schema_migrations (
    version    BIGINT PRIMARY KEY,
    applied_at TIMESTAMP NOT NULL,
    checksum   TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_events_session_seq ON events(session_id, seq);
CREATE INDEX IF NOT EXISTS idx_events_task ON events(task_id);
CREATE INDEX IF NOT EXISTS idx_messages_session_ts ON messages(session_id, ts);
CREATE INDEX IF NOT EXISTS idx_tasks_session_status ON tasks(session_id, status);
CREATE INDEX IF NOT EXISTS idx_checkpoints_task ON checkpoints(task_id);
CREATE INDEX IF NOT EXISTS idx_tool_executions_call ON tool_executions(tool_call_id);
CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit(ts);
