//! 日志初始化与脱敏（tasks.md T017；FR-203、§10.1、D-05）。
//!
//! 规则：API Key、访问令牌与运行时凭据**不得**以明文写入日志、数据库或测试快照；默认追踪
//! 级别对疑似秘密字段脱敏。本模块提供日志路径上的强制脱敏（必要条件，不替代"不写入秘密"
//! 的编码纪律）与最小级别门限。

/// 脱敏占位符。
pub const REDACTED: &str = "[REDACTED]";
/// 令牌前缀（出现在自由文本中即按令牌处理）。
const TOKEN_PREFIXES: [&str; 3] = ["Bearer ", "sk-", "ghp_"];

/// 敏感键名（ASCII 小写比较；命中后按键值形态脱敏）。
const SENSITIVE_KEYS: [&str; 6] = [
    "api_key",
    "apikey",
    "token",
    "secret",
    "password",
    "authorization",
];

/// 日志级别门限。严重程度：`Error < Warn < Info < Debug`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// 错误。
    Error,
    /// 警告。
    Warn,
    /// 一般信息。
    Info,
    /// 调试细节。
    Debug,
}

fn level_label(level: Level) -> &'static str {
    match level {
        Level::Error => "ERROR",
        Level::Warn => "WARN",
        Level::Info => "INFO",
        Level::Debug => "DEBUG",
    }
}

/// 对疑似秘密字段脱敏。
///
/// 处理两类形态：
/// 1. 自由文本中的令牌（`Bearer …`、`sk-…`、`ghp_…`）——值部分替换为 [`REDACTED`]；
/// 2. 键值形态（`key=value`、`"key":"value"`）中命中 [`SENSITIVE_KEYS`] 的值。
///
/// 值运行终止于空白、`,`、`}`、`;`、`&`、`"`、`'`。函数幂等：已脱敏文本再次输入不变。
#[must_use]
pub fn redact(input: &str) -> String {
    let once = redact_bearer_tokens(input);
    let mut out = once;
    for key in SENSITIVE_KEYS {
        out = redact_key_values(&out, key);
    }
    out
}

/// 第一遍：令牌前缀后的连续非分隔符运行替换为 [`REDACTED`]。
fn redact_bearer_tokens(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    'outer: while !rest.is_empty() {
        let mut hit: Option<(usize, &'static str)> = None;
        for prefix in TOKEN_PREFIXES {
            if let Some(idx) = rest.find(prefix) {
                hit = match hit {
                    Some((best_idx, _)) if best_idx <= idx => hit,
                    _ => Some((idx, prefix)),
                };
            }
        }
        let Some((idx, prefix)) = hit else {
            break 'outer;
        };
        out.push_str(&rest[..idx]);
        out.push_str(prefix);
        let after = &rest[idx + prefix.len()..];
        let end = after
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | '}' | '&'))
            .unwrap_or(after.len());
        if end > 0 {
            out.push_str(REDACTED);
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// 第二遍：敏感键名的键值形态脱敏。
fn redact_key_values(input: &str, key: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;
    while let Some(rel) = lower[cursor..].find(key) {
        let at = cursor + rel;
        let key_end = at + key.len();
        out.push_str(&input[cursor..key_end]);
        let mut i = key_end;
        while let Some(c) = input[i..].chars().next() {
            if c == '"' || c == '\'' || c.is_whitespace() {
                i += c.len_utf8();
            } else {
                break;
            }
        }
        match input[i..].chars().next() {
            Some('=') | Some(':') => {
                i += 1;
                while let Some(c) = input[i..].chars().next() {
                    if c.is_whitespace() {
                        i += c.len_utf8();
                    } else {
                        break;
                    }
                }
                let mut vstart = i;
                if let Some(c @ ('"' | '\'')) = input[vstart..].chars().next() {
                    vstart += c.len_utf8();
                }
                let vlen = input[vstart..]
                    .find(|c: char| {
                        c.is_whitespace() || matches!(c, ',' | '}' | ';' | '&' | '"' | '\'')
                    })
                    .unwrap_or(input[vstart..].len());
                out.push_str(&input[key_end..vstart]);
                if vlen > 0 {
                    out.push_str(REDACTED);
                }
                cursor = vstart + vlen;
            }
            _ => {
                // 键名出现但不是键值形态（如 "token count"）：原样保留，继续扫描。
                out.push_str(&input[key_end..i]);
                cursor = i;
            }
        }
    }
    out.push_str(&input[cursor..]);
    out
}

/// 生成一行日志（级别门限 + 脱敏）。低于门限返回 `None`。
#[must_use]
pub fn format_log(threshold: Level, level: Level, message: &str) -> Option<String> {
    if level <= threshold {
        Some(format!("[{}] {}", level_label(level), redact(message)))
    } else {
        None
    }
}

/// 最小日志器：输出到 stderr，一律先经 [`redact`]。
#[derive(Debug, Clone, Copy)]
pub struct Logger {
    threshold: Level,
}

impl Logger {
    /// 按门限创建。
    pub const fn new(threshold: Level) -> Self {
        Self { threshold }
    }

    /// 记录一条日志（先脱敏）。
    pub fn log(&self, level: Level, message: &str) {
        if let Some(line) = format_log(self.threshold, level, message) {
            eprintln!("{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_tokens_are_redacted() {
        let out = redact("Authorization: Bearer abc123def456 tail");
        assert!(!out.contains("abc123def456"), "令牌值必须被脱敏：{out}");
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn sk_prefixed_keys_are_redacted() {
        let out = redact("key sk-abc123secret tail");
        assert!(!out.contains("abc123secret"), "{out}");
        assert!(out.starts_with("key sk-"));
    }

    #[test]
    fn json_key_values_are_redacted() {
        let out = redact(r#"{"token":"abc123","model":"stub"}"#);
        assert_eq!(out, r#"{"token":"[REDACTED]","model":"stub"}"#);
    }

    #[test]
    fn api_key_assignment_is_redacted() {
        let out = redact("api_key=sk-xyz123,");
        assert!(!out.contains("xyz123"), "{out}");
        assert!(out.starts_with("api_key="));
        assert!(out.ends_with(','));
    }

    #[test]
    fn ordinary_words_are_not_touched() {
        let text = "the token count is 5 and secrets are safe";
        assert_eq!(redact(text), text);
    }

    #[test]
    fn redaction_is_idempotent() {
        let once = redact("token=abc123");
        let twice = redact(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn level_threshold_gates_output() {
        assert!(format_log(Level::Info, Level::Debug, "x").is_none());
        assert!(format_log(Level::Info, Level::Info, "x").is_some());
        assert!(format_log(Level::Info, Level::Error, "x").is_some());
    }

    #[test]
    fn log_path_never_emits_raw_secret() {
        let line = format_log(Level::Debug, Level::Debug, "api_key=sk-supersecret123 done")
            .expect("级别应放行");
        assert!(
            !line.contains("supersecret123"),
            "日志路径不得输出原始秘密：{line}"
        );
    }
}
