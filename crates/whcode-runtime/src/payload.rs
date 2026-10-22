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

/// 校验 payload 数据与元数据（版本/平台/哈希三重，FR-703）。
///
/// # Errors
///
/// 平台不匹配或校验和不符时返回 [`WhError`]（不静默接受）。
pub fn validate(data: &[u8], meta: &PayloadMeta, expected_platform: &str) -> Result<(), WhError> {
    if meta.platform != expected_platform {
        return Err(WhError::new(
            ErrorCode::RuntimeUnavailable,
            format!(
                "payload 平台不匹配：预期 {expected_platform}，实际 {}（FR-703）",
                meta.platform
            ),
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

    #[test]
    fn platform_mismatch_and_checksum_drift_are_rejected() {
        let data = b"payload-bytes";
        assert!(validate(data, &meta("windows-amd64"), "windows-amd64").is_ok());

        let err =
            validate(data, &meta("linux-amd64"), "windows-amd64").expect_err("平台不匹配必须拒绝");
        assert_eq!(err.code, ErrorCode::RuntimeUnavailable);

        let drifted = meta("windows-amd64");
        let err =
            validate(b"other-bytes", &drifted, "windows-amd64").expect_err("校验和漂移必须拒绝");
        assert!(err.to_string().contains("校验和"));
    }
}
