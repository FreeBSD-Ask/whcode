//! # payload 独立分发与校验（T102、FR-703、D-07）
//!
//! payload **独立分发**（来源/版本/哈希/平台校验，D-07）——不随主二进制默认携带；
//! 校验失败返回结构化错误（FR-708）。
//!
//! 哈希为 FNV-1a 64（与 `whcode_store::schema::checksum` 同算法）：**意外损坏检测**，
//! 不作密码学用途；防篡改须外部签名（威胁模型与失效条件已写明）。

use whcode_domain::{ErrorCode, WhError};

/// payload 元数据（来源/版本/哈希/平台，FR-703）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PayloadMeta {
    /// 来源（URL 或镜像标识）。
    pub source: String,
    /// 版本。
    pub version: String,
    /// 平台（如 `windows-amd64`）。
    pub platform: String,
    /// FNV-1a 64 校验和（意外损坏检测，非密码学）。
    pub checksum: String,
}

/// FNV-1a 64 校验和（与 store schema::checksum 同算法、同标签）。
#[must_use]
pub fn fnv64(data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

/// payload 来源白名单（FR-703：来源校验）。
///
/// 条目为**精确匹配**的 URL 或镜像标识（大小写敏感；安装器负责把用户输入的源规范化为
/// 与清单一致的字符串）。空白名单表示“未配置”，此时任何来源都被拒绝——默认拒绝优先，
/// 避免把“没有配置”误当成“允许一切”。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct SourceAllowlist {
    entries: Vec<String>,
}

impl SourceAllowlist {
    /// 由来源列表构造（忽略空白项；去重保序）。
    #[must_use]
    pub fn new<I, S>(sources: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut entries: Vec<String> = Vec::new();
        for source in sources {
            let source = source.into();
            let source = source.trim();
            if !source.is_empty() && !entries.iter().any(|existing| existing == source) {
                entries.push(source.to_string());
            }
        }
        Self { entries }
    }

    /// 来源是否在白名单内。
    #[must_use]
    pub fn is_allowed(&self, source: &str) -> bool {
        self.entries.iter().any(|entry| entry == source.trim())
    }

    /// 已配置的来源数量（诊断）。
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空（未配置 = 拒绝一切）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 校验 payload 数据与元数据（来源/版本/平台/哈希四重，FR-703）。
///
/// 校验顺序固定：来源白名单 → 平台 → 版本非空 → 哈希。顺序固定便于复现与审计。
///
/// # Errors
///
/// - 来源不在白名单：`permission_denied`（不可重试，须更换来源或更新白名单）；
/// - 平台不匹配、版本为空或校验和不符：`runtime_unavailable`（不静默接受）。
pub fn validate(
    data: &[u8],
    meta: &PayloadMeta,
    expected_platform: &str,
    allowed_sources: &SourceAllowlist,
) -> Result<(), WhError> {
    if !allowed_sources.is_allowed(&meta.source) {
        return Err(WhError::new(
            ErrorCode::PermissionDenied,
            format!(
                "payload 来源不在白名单，拒绝安装：{}（FR-703；白名单条目数 {}）",
                meta.source,
                allowed_sources.len()
            ),
        ));
    }
    if meta.platform != expected_platform {
        return Err(WhError::new(
            ErrorCode::RuntimeUnavailable,
            format!(
                "payload 平台不匹配：预期 {expected_platform}，实际 {}（FR-703）",
                meta.platform
            ),
        ));
    }
    if meta.version.trim().is_empty() {
        return Err(WhError::new(
            ErrorCode::RuntimeUnavailable,
            "payload 版本为空：无法判定兼容性（FR-703）".to_string(),
        ));
    }
    let actual = fnv64(data);
    if actual != meta.checksum {
        return Err(WhError::new(
            ErrorCode::RuntimeUnavailable,
            format!(
                "payload 校验和不符：预期 {}，实际 {actual}（FR-703）",
                meta.checksum
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(platform: &str) -> PayloadMeta {
        PayloadMeta {
            source: "https://example.com/payload".to_string(),
            version: "1.0".to_string(),
            platform: platform.to_string(),
            checksum: fnv64(b"payload-bytes"),
        }
    }

    fn allowlist() -> SourceAllowlist {
        SourceAllowlist::new(["https://example.com/payload", "mirror:internal"])
    }

    /// Check 102：来源不明／哈希不符／平台不符三类输入都必须被拒绝。
    #[test]
    fn unknown_source_checksum_drift_and_platform_mismatch_are_rejected() {
        let data = b"payload-bytes";
        validate(data, &meta("windows-amd64"), "windows-amd64", &allowlist())
            .expect("四重校验通过");

        // ① 来源不明：不在白名单 → permission_denied（不可重试）。
        let mut unknown = meta("windows-amd64");
        unknown.source = "https://evil.example/payload".to_string();
        let err =
            validate(data, &unknown, "windows-amd64", &allowlist()).expect_err("来源不明必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        assert!(!err.retryable, "来源问题需改输入/白名单，不是重试");
        assert!(err.message.contains("来源不在白名单"), "{err}");

        // ② 哈希不符。
        let err = validate(
            b"other-bytes",
            &meta("windows-amd64"),
            "windows-amd64",
            &allowlist(),
        )
        .expect_err("校验和漂移必须拒绝");
        assert!(err.to_string().contains("校验和"));

        // ③ 平台不符。
        let err = validate(data, &meta("linux-amd64"), "windows-amd64", &allowlist())
            .expect_err("平台不匹配必须拒绝");
        assert_eq!(err.code, ErrorCode::RuntimeUnavailable);

        // 版本为空同样拒绝（FR-703 的版本维度）。
        let mut no_version = meta("windows-amd64");
        no_version.version = "  ".to_string();
        let err =
            validate(data, &no_version, "windows-amd64", &allowlist()).expect_err("空版本必须拒绝");
        assert!(err.message.contains("版本"), "{err}");
    }

    /// 未配置白名单 = 拒绝一切（默认拒绝优先），空白项与重复项被忽略。
    #[test]
    fn empty_allowlist_rejects_everything_and_new_normalizes_entries() {
        let data = b"payload-bytes";
        let err = validate(
            data,
            &meta("windows-amd64"),
            "windows-amd64",
            &SourceAllowlist::default(),
        )
        .expect_err("未配置白名单必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);

        let list = SourceAllowlist::new(["  https://example.com/payload  ", "", "   "]);
        assert_eq!(list.len(), 1, "空白项被忽略");
        assert!(list.is_allowed("https://example.com/payload"));
        assert!(!list.is_allowed("https://example.com/other"));
        let duplicate = SourceAllowlist::new(["a", "a", "b"]);
        assert_eq!(duplicate.len(), 2, "重复项被去除");
    }
}
