//! 客户端主机密钥信任文件（known_hosts 风格，TOFU——T025 阻断项结清、ADR-005/030）。
//!
//! 文件约定：`<workspace>/ssh/known_hosts`，每行一条
//!
//! ```text
//! [host]:port SHA256:指纹
//! ```
//!
//! - 端口一律写成 `[host]:port` 形式（含 22）：避免 `host` 本身含冒号（IPv6 字面量）时的
//!   二义性；解析按“第一个空格切分”，左右两侧各自非空即合法。
//! - **TOFU 语义**（Trust On First Use）：信任文件没有该主机记录 → [`HostKeyVerdict::FirstSeen`]，
//!   由调用方记录指纹并**显式打印**供人工核对后放行；有记录且指纹一致 → [`HostKeyVerdict::Trusted`]；
//!   有记录但指纹不一致 → [`HostKeyVerdict::Mismatch`]（结构化拒绝，携带行号与期望指纹）。
//!   中间人替换主机密钥无法伪造 SHA256 指纹，不匹配必须失败而不是提示后放行。
//! - 读取失败（文件存在但不可读）按错误处理（fail closed），**不得**当作“首次使用”重新记录。
//! - 本文件只存指纹不存公钥本体；SHA256 指纹对公钥是抗碰撞摘要，比对指纹与比对公钥
//!   在“是否同一把密钥”的判定上等价，且便于人工核对与审计。

use std::path::{Path, PathBuf};

use russh::keys::PublicKey;
use whcode_domain::{ErrorCode, WhError};

use crate::hostkey::{self, fingerprint};

/// 信任文件名（`<workspace>/ssh/known_hosts`）。
pub const KNOWN_HOSTS_FILE_NAME: &str = "known_hosts";

/// 主机密钥校验结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyVerdict {
    /// 信任文件有该主机且指纹一致：放行。
    Trusted,
    /// 信任文件没有该主机记录：首次使用——由调用方记录指纹并显式打印供人工核对。
    FirstSeen,
    /// 该主机已有记录但指纹不一致：中间人风险，必须拒绝（携带首次不一致的行号与期望指纹）。
    Mismatch {
        /// 信任文件中不一致记录的行号（1 起始）。
        line: usize,
        /// 记录中的期望指纹。
        expected_fingerprint: String,
    },
}

/// known_hosts 风格信任文件（`[host]:port SHA256:指纹`）。
#[derive(Debug, Clone)]
pub struct KnownHosts {
    path: PathBuf,
}

impl KnownHosts {
    /// `<workspace>/ssh/known_hosts`。
    #[must_use]
    pub fn in_workspace(workspace: &Path) -> Self {
        Self::at(hostkey::ssh_dir(workspace).join(KNOWN_HOSTS_FILE_NAME))
    }

    /// 指定路径的信任文件。
    #[must_use]
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// 信任文件路径。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 校验远端主机密钥。
    ///
    /// 匹配规则：按文件顺序找**第一条** `[host]:port` 记录——指纹一致 → [`HostKeyVerdict::Trusted`]，
    /// 不一致 → [`HostKeyVerdict::Mismatch`]；没有任何该主机记录 → [`HostKeyVerdict::FirstSeen`]。
    ///
    /// # Errors
    ///
    /// 信任文件存在但不可读时返回 [`WhError`]（fail closed，调用方必须拒绝连接）。
    pub fn verify(
        &self,
        host: &str,
        port: u16,
        key: &PublicKey,
    ) -> Result<HostKeyVerdict, WhError> {
        let actual = fingerprint(key);
        let entry = entry_host(host, port);
        if !self.path.exists() {
            return Ok(HostKeyVerdict::FirstSeen);
        }
        let text = std::fs::read_to_string(&self.path).map_err(|err| {
            WhError::new(
                ErrorCode::IoError,
                format!(
                    "主机密钥信任文件不可读（{}）：{err}。校验失败按拒绝处理，不重新记录",
                    self.path.display()
                ),
            )
        })?;
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((recorded_host, recorded_fp)) = line.split_once(' ') else {
                continue;
            };
            if recorded_host.trim() != entry {
                continue;
            }
            let recorded_fp = recorded_fp.trim();
            return if recorded_fp == actual {
                Ok(HostKeyVerdict::Trusted)
            } else {
                Ok(HostKeyVerdict::Mismatch {
                    line: index + 1,
                    expected_fingerprint: recorded_fp.to_string(),
                })
            };
        }
        Ok(HostKeyVerdict::FirstSeen)
    }

    /// 记录一条主机指纹（TOFU 首次使用）。该主机已有记录时**幂等跳过**：改绑指纹必须
    /// 人工编辑文件，不允许代码静默覆盖（否则 TOFU 退化为每次都信）。
    ///
    /// # Errors
    ///
    /// 目录创建或文件写入失败时返回 [`WhError`]。
    pub fn record(&self, host: &str, port: u16, key: &PublicKey) -> Result<(), WhError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                WhError::new(
                    ErrorCode::IoError,
                    format!("创建信任文件目录失败（{}）：{err}", parent.display()),
                )
            })?;
        }
        let entry = entry_host(host, port);
        if self.path.exists() {
            let existing = std::fs::read_to_string(&self.path).map_err(|err| {
                WhError::new(
                    ErrorCode::IoError,
                    format!("读取信任文件失败（{}）：{err}", self.path.display()),
                )
            })?;
            let already = existing.lines().any(|line| {
                line.split_once(' ')
                    .is_some_and(|(recorded, _)| recorded.trim() == entry)
            });
            if already {
                return Ok(());
            }
        }
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|err| {
                WhError::new(
                    ErrorCode::IoError,
                    format!("打开信任文件失败（{}）：{err}", self.path.display()),
                )
            })?;
        writeln!(file, "{entry} {}", fingerprint(key)).map_err(|err| {
            WhError::new(
                ErrorCode::IoError,
                format!("写入信任文件失败（{}）：{err}", self.path.display()),
            )
        })
    }
}

/// trust 文件中的主机标识（统一 `[host]:port`，含 22 端口，规避 IPv6 冒号二义）。
fn entry_host(host: &str, port: u16) -> String {
    format!("[{host}]:{port}")
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU64, Ordering};

    use russh::keys::{Algorithm, PrivateKey};

    static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

    struct TempWorkspace {
        path: PathBuf,
    }

    impl TempWorkspace {
        fn new() -> Self {
            let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("whcode-ssh-trust-{}-{seq}", std::process::id()));
            std::fs::create_dir_all(&path).expect("创建临时目录");
            Self { path }
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn ed25519_key() -> PrivateKey {
        PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("生成密钥")
    }

    #[test]
    fn three_branches_first_seen_trusted_and_mismatch() {
        let ws = TempWorkspace::new();
        let hosts = KnownHosts::in_workspace(&ws.path);
        let server_key = ed25519_key();

        // 分支一：没有记录 → FirstSeen。
        let verdict = hosts
            .verify("127.0.0.1", 2200, server_key.public_key())
            .expect("校验");
        assert_eq!(verdict, HostKeyVerdict::FirstSeen);

        // TOFU 记录后：同一把密钥 → Trusted；文件含指纹行。
        hosts
            .record("127.0.0.1", 2200, server_key.public_key())
            .expect("记录指纹");
        let verdict = hosts
            .verify("127.0.0.1", 2200, server_key.public_key())
            .expect("校验");
        assert_eq!(verdict, HostKeyVerdict::Trusted);
        let text = std::fs::read_to_string(hosts.path()).expect("读取信任文件");
        assert!(
            text.contains("[127.0.0.1]:2200 SHA256:"),
            "记录形如 [host]:port SHA256:…：{text}"
        );

        // 分支三：同主机换成另一把密钥 → Mismatch（携带行号与期望指纹）。
        let attacker = ed25519_key();
        let verdict = hosts
            .verify("127.0.0.1", 2200, attacker.public_key())
            .expect("校验（不匹配是结论不是错误）");
        match verdict {
            HostKeyVerdict::Mismatch {
                line,
                expected_fingerprint,
            } => {
                assert_eq!(line, 1);
                assert_eq!(
                    expected_fingerprint,
                    fingerprint(server_key.public_key()),
                    "期望指纹 = 首次记录的指纹"
                );
            }
            other => panic!("必须判定为不匹配：{other:?}"),
        }
    }

    #[test]
    fn different_host_and_port_are_independent_entries() {
        let ws = TempWorkspace::new();
        let hosts = KnownHosts::in_workspace(&ws.path);
        let key = ed25519_key();
        hosts.record("host-a", 22, key.public_key()).expect("记录");
        // 同一把密钥、不同主机/端口 → 仍是 FirstSeen（记录按主机区分）。
        assert_eq!(
            hosts.verify("host-b", 22, key.public_key()).expect("校验"),
            HostKeyVerdict::FirstSeen
        );
        assert_eq!(
            hosts
                .verify("host-a", 2222, key.public_key())
                .expect("校验"),
            HostKeyVerdict::FirstSeen
        );
        assert_eq!(
            hosts.verify("host-a", 22, key.public_key()).expect("校验"),
            HostKeyVerdict::Trusted
        );
    }

    #[test]
    fn unreadable_trust_file_fails_closed_instead_of_first_seen() {
        let ws = TempWorkspace::new();
        // 把 trust 路径指向一个“目录”：目录存在（exists() == true）但读取必然失败。
        let dir_as_file = ws.path.join("known_hosts_dir");
        std::fs::create_dir_all(&dir_as_file).expect("建目录");
        let hosts = KnownHosts::at(dir_as_file);
        let err = hosts
            .verify("h", 1, ed25519_key().public_key())
            .expect_err("不可读必须 fail closed");
        assert_eq!(err.code, ErrorCode::IoError);
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let ws = TempWorkspace::new();
        let path = ws.path.join("known_hosts");
        let key = ed25519_key();
        std::fs::write(
            &path,
            format!(
                "# 注释\n\n   \n[host-a]:22 {} \n",
                fingerprint(key.public_key())
            ),
        )
        .expect("写入");
        let hosts = KnownHosts::at(path);
        assert_eq!(
            hosts.verify("host-a", 22, key.public_key()).expect("校验"),
            HostKeyVerdict::Trusted,
            "注释与空行不参与匹配，行尾空格容忍"
        );
    }
}
