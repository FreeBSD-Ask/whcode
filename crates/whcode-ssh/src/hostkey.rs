//! 服务端主机密钥与授权公钥清单（T096、ADR-030）。
//!
//! 约定（ADR-030）：
//! - 主机密钥：`<workspace>/ssh/host_key`，首次启动生成 Ed25519 并以 OpenSSH 格式落盘；
//!   之后每次启动加载同一把，指纹不变。**密钥内容不进日志**——只输出 `SHA256:…` 指纹
//!   （指纹是单向摘要，可入日志供人工核对）。
//! - 授权公钥：`<workspace>/ssh/authorized_keys`，OpenSSH 公钥行格式（`#` 注释与空行
//!   忽略）。文件缺失按错误处理（fail closed）：没有显式授权清单就没有任何可登录身份，
//!   不得静默回退为“无授权”或“全授权”。
//! - 仅接受 Ed25519：加载主机密钥与解析授权公钥时都做算法校验，其他算法一律拒绝
//!   （FR-611 安全边界：仅公钥认证；算法面收敛到一条）。

use std::path::{Path, PathBuf};

use russh::keys::{Algorithm, HashAlg, PrivateKey, PublicKey, load_secret_key};
use whcode_domain::{ErrorCode, WhError};

/// 工作区内 SSH 状态目录名。
pub const SSH_DIR_NAME: &str = "ssh";
/// 主机密钥文件名。
pub const HOST_KEY_FILE_NAME: &str = "host_key";
/// 授权公钥清单文件名。
pub const AUTHORIZED_KEYS_FILE_NAME: &str = "authorized_keys";

/// `<workspace>/ssh` 目录。
#[must_use]
pub fn ssh_dir(workspace: &Path) -> PathBuf {
    workspace.join(SSH_DIR_NAME)
}

/// 主机密钥路径（`<workspace>/ssh/host_key`）。
#[must_use]
pub fn host_key_path(workspace: &Path) -> PathBuf {
    ssh_dir(workspace).join(HOST_KEY_FILE_NAME)
}

/// 授权公钥清单路径（`<workspace>/ssh/authorized_keys`）。
#[must_use]
pub fn authorized_keys_path(workspace: &Path) -> PathBuf {
    ssh_dir(workspace).join(AUTHORIZED_KEYS_FILE_NAME)
}

/// 公钥的 `SHA256:…` 指纹（日志与审计唯一允许的密钥标识形态）。
#[must_use]
pub fn fingerprint(public_key: &PublicKey) -> String {
    public_key.fingerprint(HashAlg::Sha256).to_string()
}

/// 已加载的服务端主机密钥。
#[derive(Debug)]
pub struct HostKey {
    path: PathBuf,
    fingerprint: String,
    key: PrivateKey,
}

impl HostKey {
    /// 主机密钥文件路径。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `SHA256:…` 指纹（可入日志与 `doctor` 输出；不含密钥材料）。
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// 对应公钥（写 known_hosts / 提示用）。
    #[must_use]
    pub fn public_key(&self) -> &PublicKey {
        self.key.public_key()
    }

    /// 私钥本体（只交给 russh `Config::keys`；不得序列化进日志或事件）。
    #[must_use]
    pub fn private_key(&self) -> &PrivateKey {
        &self.key
    }
}

/// 加载或首次生成主机密钥（Ed25519，OpenSSH 格式落盘）。
///
/// # Errors
///
/// 目录创建、密钥生成、读写或算法不符（非 Ed25519）时返回 [`WhError`]。
pub fn load_or_create_host_key(workspace: &Path) -> Result<HostKey, WhError> {
    let path = host_key_path(workspace);
    if path.exists() {
        let key = load_secret_key(&path, None).map_err(|err| {
            WhError::new(
                ErrorCode::InvalidState,
                format!("主机密钥加载失败（{}）：{err}", path.display()),
            )
        })?;
        return finish_host_key(path, key);
    }

    let dir = ssh_dir(workspace);
    std::fs::create_dir_all(&dir).map_err(|err| {
        WhError::new(
            ErrorCode::IoError,
            format!("创建 SSH 状态目录失败（{}）：{err}", dir.display()),
        )
    })?;
    // CSPRNG：rand 0.10（ThreadRng，ChaCha12，操作系统熵播种）——版本与 russh 的发布依赖一致。
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
        .map_err(|err| WhError::new(ErrorCode::Internal, format!("主机密钥生成失败：{err}")))?;
    let encoded = key
        .to_openssh(russh::keys::ssh_key::LineEnding::LF)
        .map_err(|err| WhError::new(ErrorCode::Internal, format!("主机密钥编码失败：{err}")))?;
    // 先写临时文件再改名：避免半写文件被下次启动当成有效密钥加载。
    let tmp = dir.join(format!("{HOST_KEY_FILE_NAME}.tmp"));
    std::fs::write(&tmp, encoded.as_bytes()).map_err(|err| {
        WhError::new(
            ErrorCode::IoError,
            format!("写入主机密钥失败（{}）：{err}", tmp.display()),
        )
    })?;
    std::fs::rename(&tmp, &path).map_err(|err| {
        let _ = std::fs::remove_file(&tmp);
        WhError::new(
            ErrorCode::IoError,
            format!("落盘主机密钥失败（{}）：{err}", path.display()),
        )
    })?;
    finish_host_key(path, key)
}

/// 校验算法并组装 [`HostKey`]。
fn finish_host_key(path: PathBuf, key: PrivateKey) -> Result<HostKey, WhError> {
    if key.algorithm() != Algorithm::Ed25519 {
        return Err(WhError::invalid_state(format!(
            "主机密钥算法不是 Ed25519（{}）：拒绝启动（仅 Ed25519，FR-611）",
            path.display()
        )));
    }
    let fingerprint = fingerprint(key.public_key());
    Ok(HostKey {
        path,
        fingerprint,
        key,
    })
}

/// 加载授权公钥清单（仅 Ed25519；缺失即失败——fail closed，无任何隐式授权）。
///
/// # Errors
///
/// 文件缺失、不可读或包含无法解析/非 Ed25519 的公钥行时返回 [`WhError`]。
pub fn load_authorized_keys(workspace: &Path) -> Result<Vec<PublicKey>, WhError> {
    let path = authorized_keys_path(workspace);
    let text = std::fs::read_to_string(&path).map_err(|err| {
        WhError::new(
            ErrorCode::InvalidState,
            format!(
                "授权公钥清单不可用（{}）：{err}。请为允许登录的客户端生成 Ed25519 公钥并\
                 写入该文件（每行一把公钥），否则没有任何身份可以登录",
                path.display()
            ),
        )
    })?;
    let mut keys = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let key = PublicKey::from_openssh(line).map_err(|err| {
            WhError::new(
                ErrorCode::ParseError,
                format!("授权公钥第 {} 行解析失败：{err}", index + 1),
            )
        })?;
        if key.algorithm() != Algorithm::Ed25519 {
            return Err(WhError::invalid_state(format!(
                "授权公钥第 {} 行不是 Ed25519：拒绝启动（仅 Ed25519，FR-611）",
                index + 1
            )));
        }
        keys.push(key);
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU64, Ordering};

    static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

    /// 测试用临时工作区目录：析构时尽力删除。
    struct TempWorkspace {
        path: PathBuf,
    }

    impl TempWorkspace {
        fn new() -> Self {
            let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("whcode-ssh-hostkey-{}-{seq}", std::process::id()));
            std::fs::create_dir_all(&path).expect("创建临时目录");
            Self { path }
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn host_key_is_created_once_and_reloads_with_same_fingerprint() {
        let ws = TempWorkspace::new();
        let first = load_or_create_host_key(&ws.path).expect("首次生成主机密钥");
        assert!(first.path().exists(), "主机密钥应落盘");
        assert!(
            first.fingerprint().starts_with("SHA256:"),
            "指纹形如 SHA256:…"
        );
        assert!(
            !first.fingerprint().is_empty(),
            "指纹非空（内容不校验具体值）"
        );
        // 落盘文件是 OpenSSH 私钥格式。
        let raw = std::fs::read_to_string(first.path()).expect("读取落盘文件");
        assert!(raw.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));

        let second = load_or_create_host_key(&ws.path).expect("二次加载主机密钥");
        assert_eq!(
            first.fingerprint(),
            second.fingerprint(),
            "同一密钥文件加载出的指纹必须一致"
        );
        assert_eq!(first.path(), second.path());
        // 临时文件不残留。
        assert!(!ssh_dir(&ws.path).join("host_key.tmp").exists());
    }

    #[test]
    fn host_key_rejects_non_ed25519_key_file() {
        let ws = TempWorkspace::new();
        let dir = ssh_dir(&ws.path);
        std::fs::create_dir_all(&dir).expect("创建 ssh 目录");
        // 生成一把 ECDSA P-256 密钥并冒充主机密钥：算法校验必须拒绝。
        let other = PrivateKey::random(
            &mut rand::rng(),
            Algorithm::Ecdsa {
                curve: russh::keys::EcdsaCurve::NistP256,
            },
        )
        .expect("生成 P-256 密钥");
        let encoded = other
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .expect("编码");
        std::fs::write(host_key_path(&ws.path), encoded.as_bytes()).expect("写入冒充密钥");
        let err = load_or_create_host_key(&ws.path).expect_err("非 Ed25519 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("Ed25519"), "错误写明算法约束：{err}");
    }

    #[test]
    fn authorized_keys_missing_is_an_error_and_empty_file_yields_no_keys() {
        let ws = TempWorkspace::new();
        let err = load_authorized_keys(&ws.path).expect_err("清单缺失必须失败（fail closed）");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("authorized_keys"), "错误可操作：{err}");

        let path = authorized_keys_path(&ws.path);
        std::fs::create_dir_all(ssh_dir(&ws.path)).expect("创建 ssh 目录");
        std::fs::write(&path, "# 仅注释\n\n").expect("写入注释清单");
        let keys = load_authorized_keys(&ws.path).expect("空清单（仅注释）可加载");
        assert!(keys.is_empty(), "空清单 = 没有任何可登录身份");
    }

    #[test]
    fn authorized_keys_parses_ed25519_and_rejects_other_or_broken_lines() {
        let ws = TempWorkspace::new();
        let client = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("生成密钥");
        let client_pub = client.public_key().to_openssh().expect("编码公钥");

        let other = PrivateKey::random(
            &mut rand::rng(),
            Algorithm::Ecdsa {
                curve: russh::keys::EcdsaCurve::NistP256,
            },
        )
        .expect("生成 P-256 密钥");
        let other_pub = other.public_key().to_openssh().expect("编码公钥");

        let path = authorized_keys_path(&ws.path);
        std::fs::create_dir_all(ssh_dir(&ws.path)).expect("创建 ssh 目录");
        std::fs::write(&path, format!("{client_pub}\n{other_pub}\n")).expect("写入清单");
        let err = load_authorized_keys(&ws.path).expect_err("非 Ed25519 公钥必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        std::fs::write(&path, format!("{client_pub}\nnot-a-key\n")).expect("写入坏行");
        let err = load_authorized_keys(&ws.path).expect_err("无法解析的行必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(err.message.contains("2"), "错误写明行号：{err}");

        std::fs::write(&path, &client_pub).expect("写入合法清单");
        let keys = load_authorized_keys(&ws.path).expect("合法清单可加载");
        assert_eq!(keys.len(), 1);
        assert_eq!(fingerprint(&keys[0]), fingerprint(client.public_key()));
    }
}
