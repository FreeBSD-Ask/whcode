//! # usage 锚点（FR-758／T268／ADR-060 项 E）
//!
//! 预算判断现行用 4 字符/token 启发式（FR-207 口径）；供应商其实在每次成功响应里返回真实
//! usage。本模块把**最近一次**实测保存为单行锚点（`scope = 'last'`，每次覆盖），供预算判断
//! 校准字符/token 比；锚点缺失（从未有成功请求、供应商未返回 usage、或换了供应商/模型）
//! 时调用方回退启发式，行为与引入本模块前一致。
//!
//! 边界：
//!
//! - **只存一条**：主键固定且带 `CHECK (scope = 'last')`，表体积不随会话增长（NFR-215 有界）；
//! - **只存实测**：调用方只在供应商返回 usage 时写入；本地估算（`Usage::estimate`）不得
//!   写入，否则锚点会把启发式自己当成观测值（0 信任：不信任自证）；
//! - **不跨供应商混用**：锚点带 `provider`/`model`，调用方按身份匹配后才使用（ADR-060 §2.4
//!   “不做多供应商矩阵”——单行＋身份校验即已闭合，不引入 per-model 表）。

use duckdb::{Connection, params};
use whcode_domain::{ErrorCode, WhError};

/// 锚点唯一键（只存一条）。
pub const ANCHOR_SCOPE: &str = "last";

/// 供应商标识长度上限（字符；超限拒绝而非截断——身份截断会与别的供应商错配）。
pub const MAX_IDENTIFIER_CHARS: usize = 128;

fn exec_err(context: &str, err: impl std::fmt::Display) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}：{err}"))
}

fn invalid(context: &str) -> WhError {
    WhError::new(ErrorCode::InvalidState, context.to_string())
}

/// 最近一次成功请求的实测 usage（单行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageAnchor {
    /// 供应商名（剖面的 `name`）。
    pub provider: String,
    /// 模型 ID。
    pub model: String,
    /// 供应商返回的提示 token 数。
    pub prompt_tokens: u64,
    /// 供应商返回的生成 token 数。
    pub completion_tokens: u64,
    /// 该次请求发送的字符数（消息正文合计；比例分母）。
    pub prompt_chars: u64,
    /// 写入时刻（Unix 毫秒）。
    pub ts_ms: i64,
}

impl UsageAnchor {
    /// 实测字符/token 比；`prompt_tokens` 或 `prompt_chars` 为 0 时无法导出比例 → `None`。
    #[must_use]
    pub fn chars_per_token(&self) -> Option<f64> {
        if self.prompt_tokens == 0 || self.prompt_chars == 0 {
            return None;
        }
        Some(self.prompt_chars as f64 / self.prompt_tokens as f64)
    }

    fn validate(&self) -> Result<(), WhError> {
        if self.provider.trim().is_empty() || self.model.trim().is_empty() {
            return Err(invalid("usage 锚点的 provider/model 不得为空"));
        }
        for (label, value) in [("provider", &self.provider), ("model", &self.model)] {
            if value.chars().count() > MAX_IDENTIFIER_CHARS {
                return Err(invalid(&format!(
                    "usage 锚点的 {label} 超过 {MAX_IDENTIFIER_CHARS} 字符上限"
                )));
            }
        }
        if self.prompt_chars == 0 {
            return Err(invalid(
                "usage 锚点的 prompt_chars 必须大于 0（否则无法导出比例）",
            ));
        }
        if self.ts_ms < 0 {
            return Err(invalid("usage 锚点的 ts_ms 不得为负"));
        }
        Ok(())
    }
}

/// 写入/覆盖锚点（单条：同 `scope` 覆盖，不追加）。
///
/// # Errors
///
/// 字段非法（空身份、零字符数、负时间）→ `invalid_state`；写库失败 → `io_error`。
pub fn save_anchor(conn: &Connection, anchor: &UsageAnchor) -> Result<(), WhError> {
    anchor.validate()?;
    conn.execute(
        "INSERT INTO usage_anchor
             (scope, provider, model, prompt_tokens, completion_tokens, prompt_chars, ts)
         VALUES (?, ?, ?, ?, ?, ?, epoch_ms(?))
         ON CONFLICT (scope) DO UPDATE SET
             provider = excluded.provider,
             model = excluded.model,
             prompt_tokens = excluded.prompt_tokens,
             completion_tokens = excluded.completion_tokens,
             prompt_chars = excluded.prompt_chars,
             ts = excluded.ts",
        params![
            ANCHOR_SCOPE,
            anchor.provider,
            anchor.model,
            i64::try_from(anchor.prompt_tokens).map_err(|_| invalid("prompt_tokens 超出范围"))?,
            i64::try_from(anchor.completion_tokens)
                .map_err(|_| invalid("completion_tokens 超出范围"))?,
            i64::try_from(anchor.prompt_chars).map_err(|_| invalid("prompt_chars 超出范围"))?,
            anchor.ts_ms,
        ],
    )
    .map_err(|err| exec_err("写入 usage 锚点", err))?;
    Ok(())
}

/// 读取锚点；从未写入时返回 `None`（调用方回退启发式，不得伪造比例）。
///
/// # Errors
///
/// 读库失败 → `io_error`。
pub fn load_anchor(conn: &Connection) -> Result<Option<UsageAnchor>, WhError> {
    let mut stmt = conn
        .prepare(
            "SELECT provider, model, prompt_tokens, completion_tokens, prompt_chars,
                    epoch_ms(ts)
             FROM usage_anchor WHERE scope = ? LIMIT 1",
        )
        .map_err(|err| exec_err("准备读取 usage 锚点", err))?;
    let mut rows = stmt
        .query([ANCHOR_SCOPE])
        .map_err(|err| exec_err("读取 usage 锚点", err))?;
    let Some(row) = rows
        .next()
        .map_err(|err| exec_err("读取 usage 锚点行", err))?
    else {
        return Ok(None);
    };
    let prompt_tokens: i64 = row
        .get(2)
        .map_err(|err| exec_err("读取 prompt_tokens", err))?;
    let completion_tokens: i64 = row
        .get(3)
        .map_err(|err| exec_err("读取 completion_tokens", err))?;
    let prompt_chars: i64 = row
        .get(4)
        .map_err(|err| exec_err("读取 prompt_chars", err))?;
    Ok(Some(UsageAnchor {
        provider: row.get(0).map_err(|err| exec_err("读取 provider", err))?,
        model: row.get(1).map_err(|err| exec_err("读取 model", err))?,
        prompt_tokens: u64::try_from(prompt_tokens)
            .map_err(|_| invalid("锚点 prompt_tokens 为负（库被外部改写）"))?,
        completion_tokens: u64::try_from(completion_tokens)
            .map_err(|_| invalid("锚点 completion_tokens 为负（库被外部改写）"))?,
        prompt_chars: u64::try_from(prompt_chars)
            .map_err(|_| invalid("锚点 prompt_chars 为负（库被外部改写）"))?,
        ts_ms: row.get(5).map_err(|err| exec_err("读取锚点时间", err))?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        crate::apply_migrations(&conn).expect("迁移");
        conn
    }

    fn anchor(provider: &str, model: &str, tokens: u64, chars: u64) -> UsageAnchor {
        UsageAnchor {
            provider: provider.to_string(),
            model: model.to_string(),
            prompt_tokens: tokens,
            completion_tokens: 7,
            prompt_chars: chars,
            ts_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn save_load_roundtrip_derives_ratio() {
        let conn = conn();
        assert_eq!(load_anchor(&conn).expect("读取"), None, "初始无锚点");

        let saved = anchor("sensenova", "6.8-flash-lite", 200, 800);
        save_anchor(&conn, &saved).expect("写入");
        let loaded = load_anchor(&conn).expect("读取").expect("应有锚点");
        assert_eq!(loaded, saved);
        assert_eq!(
            loaded.chars_per_token(),
            Some(4.0),
            "800 字符 / 200 token = 4.0"
        );
        let ratio = anchor("p", "m", 100, 250).chars_per_token().expect("比例");
        assert!((ratio - 2.5).abs() < 1e-9, "250/100 = 2.5，得 {ratio}");
    }

    #[test]
    fn saving_keeps_exactly_one_row() {
        let conn = conn();
        for round in 0..5u64 {
            save_anchor(&conn, &anchor("p", "m", 100 + round, 400 + round * 4)).expect("写入");
        }
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM usage_anchor", [], |row| row.get(0))
            .expect("计数");
        assert_eq!(rows, 1, "只存一条（有界，不随请求增长）");
        let loaded = load_anchor(&conn).expect("读取").expect("应有锚点");
        assert_eq!(loaded.prompt_tokens, 104, "后写覆盖前写");
    }

    #[test]
    fn zero_measurement_cannot_derive_ratio_and_invalid_fields_are_rejected() {
        assert_eq!(
            anchor("p", "m", 0, 400).chars_per_token(),
            None,
            "token 数为 0 无法导出比例（不得伪造）"
        );
        let conn = conn();
        let err = save_anchor(&conn, &anchor("p", "m", 100, 0)).expect_err("零字符数拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = save_anchor(&conn, &anchor("", "m", 100, 400)).expect_err("空身份拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = save_anchor(
            &conn,
            &anchor("p", &"m".repeat(MAX_IDENTIFIER_CHARS + 1), 1, 4),
        )
        .expect_err("超长身份拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert_eq!(load_anchor(&conn).expect("读取"), None, "拒绝的写入不落库");
    }
}
