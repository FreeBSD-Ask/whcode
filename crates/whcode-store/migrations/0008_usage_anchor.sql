-- 0008：usage 锚点（FR-758／T268／ADR-060 项 E）
--
-- 只存一条：主键固定 `scope = 'last'`，每次成功请求覆盖（有界，不随会话增长）。
-- 存的是**实测**（供应商返回的 usage 与请求字符数）——字符/token 比由此得出，
-- 供预算判断校准；不满足条件的请求不写锚点（缺失即回退 4 字符启发式）。
CREATE TABLE IF NOT EXISTS usage_anchor (
    scope             TEXT PRIMARY KEY CHECK (scope = 'last'),
    provider          TEXT NOT NULL,
    model             TEXT NOT NULL,
    prompt_tokens     BIGINT NOT NULL CHECK (prompt_tokens >= 0),
    completion_tokens BIGINT NOT NULL CHECK (completion_tokens >= 0),
    prompt_chars      BIGINT NOT NULL CHECK (prompt_chars > 0),
    ts                TIMESTAMP NOT NULL
);
