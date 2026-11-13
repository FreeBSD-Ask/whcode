//! # 文件前像应用面（T289／FR-776／ADR-068）
//!
//! 抓取时机与成功条件（决策 5）：turn 在执行 `write_file`／`edit_file` **之前**读取目标
//! 原文（存在、常规文件、≤ [`whcode_store::preimage::MAX_PREIMAGE_BYTES`]），仅在该调用
//! **成功**后登记；失败／被拒不登记。超限与非 UTF-8 显式登记跳过原因。
//!
//! 恢复是破坏族（决策 7）：预览（dry-run）列出将恢复的路径与字节数；执行前写审计，
//! 审计失败即不执行；只恢复工作区内路径。
//!
//! 边界：SQL 只在 `whcode-store`；文件系统在本模块；权限判定走 `authz`。

use duckdb::Connection;
use whcode_domain::{AuditAction, AuditRecord, ErrorCode, WhError};
use whcode_store::preimage::PreimageRow;

use crate::authz::{Actor, Capability, Policy, require};

/// 抓取结果（决策 5 的显式分类；`absent`＝新建文件，恢复时删除该文件）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreimageCapture {
    /// 新建文件：改前不存在。
    Absent,
    /// 有内容（`size` 为原始大小）。
    Content { size: i64, text: String },
    /// 显式跳过（`too_large`／`not_utf8`；只登记原因，不存内容）。
    Skipped { size: i64, reason: &'static str },
}

/// 抓取前像：目标存在且是常规文件、内容 ≤ 上限且为 UTF-8 时返回 `Content`；
/// 不存在（新建文件）返回 `Absent`；超限或非 UTF-8 返回 `Skipped`；读取失败返回错误。
///
/// # Errors
///
/// 路径解析失败或元数据不可读时返回可操作错误。
pub fn capture(workspace: &std::path::Path, raw_path: &str) -> Result<PreimageCapture, WhError> {
    let resolved = whcode_tools::resolve_in_workspace(workspace, raw_path)?;
    let Ok(metadata) = resolved.metadata() else {
        // 不存在＝新建；没有“改前内容”可登记。
        return Ok(PreimageCapture::Absent);
    };
    if !metadata.is_file() {
        return Err(WhError::invalid_state(format!(
            "前像目标 {raw_path} 不是常规文件；写工具只处理文件"
        )));
    }
    let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
    if size as usize > whcode_store::preimage::MAX_PREIMAGE_BYTES {
        return Ok(PreimageCapture::Skipped {
            size,
            reason: "too_large",
        });
    }
    match std::fs::read(&resolved) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Ok(PreimageCapture::Content { size, text }),
            Err(_) => Ok(PreimageCapture::Skipped {
                size,
                reason: "not_utf8",
            }),
        },
        Err(err) => Err(WhError::new(
            ErrorCode::IoError,
            format!("读取前像 {raw_path} 失败：{err}"),
        )),
    }
}

/// 在工具调用成功后登记前像（`path` 已是工作区内相对路径）。
///
/// # Errors
///
/// 会话不存在或写入失败（登记失败按 `PersistenceError` 口径由调用方落事件）。
pub fn record_after_success(
    conn: &Connection,
    session_id: &str,
    exec_id: &str,
    path: &str,
    captured: PreimageCapture,
) -> Result<(), WhError> {
    match captured {
        PreimageCapture::Absent => {
            whcode_store::preimage::record(conn, session_id, exec_id, path, 0, None, Some("absent"))
        }
        PreimageCapture::Content { size, text } => {
            whcode_store::preimage::record(conn, session_id, exec_id, path, size, Some(&text), None)
        }
        PreimageCapture::Skipped { size, reason } => whcode_store::preimage::record(
            conn,
            session_id,
            exec_id,
            path,
            size,
            None,
            Some(reason),
        ),
    }
    .map(|_| ())
}

/// 前像清单的一次读取上限（与 store 的每会话上限一致）。
pub const MAX_PREIMAGE_LIST: usize = whcode_store::preimage::MAX_PREIMAGES_PER_SESSION;

/// 列出会话前像（新→旧，有界）。
///
/// # Errors
///
/// 权限拒绝（需读能力）、会话不存在、`limit` 越界或查询失败。
pub fn list(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    limit: usize,
) -> Result<Vec<PreimageRow>, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    if whcode_store::get_session(conn, session_id)?.is_none() {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在：{session_id}；用 session list 查看现存会话"),
        ));
    }
    whcode_store::preimage::list(conn, session_id, limit)
}

/// 恢复预览（dry-run）：给定 `exec_id` 的全部前像与将执行的动作，不写工作区。
///
/// # Errors
///
/// 权限拒绝、会话不存在、无该执行的前像（`not_found`）或查询失败。
pub fn preview(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    exec_id: &str,
) -> Result<Vec<PreimageRow>, WhError> {
    require(policy, actor, Capability::SessionRestore)?;
    if whcode_store::get_session(conn, session_id)?.is_none() {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在：{session_id}；用 session list 查看现存会话"),
        ));
    }
    let rows = whcode_store::preimage::by_exec(conn, session_id, exec_id)?;
    if rows.is_empty() {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("执行 {exec_id} 没有可恢复的前像；用 GET /api/preimages 查看可用执行"),
        ));
    }
    Ok(rows)
}

/// 执行恢复（破坏族）：把 `exec_id` 的每个有内容前像写回工作区；`absent` 前像删除
/// 该文件（恢复到“当时不存在”）；跳过行（`too_large`／`not_utf8`）不动作并保留在结果里。
/// 审计失败即不执行任何写入。
///
/// # Errors
///
/// 权限拒绝、会话不存在、无前像（`not_found`）、审计失败或任一文件写入失败
/// （失败路径中止后续写入，已写入的文件保留——逐文件结果在返回值中）。
pub fn restore(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    workspace: &std::path::Path,
    exec_id: &str,
) -> Result<Vec<(String, String)>, WhError> {
    require(policy, actor, Capability::SessionRestore)?;
    if whcode_store::get_session(conn, session_id)?.is_none() {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在：{session_id}；用 session list 查看现存会话"),
        ));
    }
    let rows = preview(conn, policy, actor, session_id, exec_id)?;
    // 审计先行：失败即不执行（沿用“审计失败阻断”口径）。ID 含毫秒时间戳（与
    // GateOverride 同惯例）：恢复不是幂等操作，同一 exec 可再次恢复，固定 ID 会撞主键。
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        });
    let paths: Vec<String> = rows.iter().map(|row| row.path.clone()).collect();
    whcode_store::insert_record(
        conn,
        &AuditRecord {
            id: format!("A-preimage-restore-{session_id}-{exec_id}-{now_ms}"),
            actor: actor.id.clone(),
            action: AuditAction::PreimageRestore,
            target: format!("session:{session_id} exec:{exec_id}"),
            detail: serde_json::json!({ "paths": paths, "count": paths.len() }),
            // 数据库时间由 `current_timestamp` 统一（`insert_record` 不使用该字段）。
            ts_ms: now_ms,
        },
    )?;
    let mut results: Vec<(String, String)> = Vec::with_capacity(rows.len());
    for row in rows {
        let resolved = whcode_tools::resolve_in_workspace(workspace, &row.path)?;
        let outcome = match (&row.content, row.skipped_reason.as_deref()) {
            (Some(text), _) => match std::fs::write(&resolved, text) {
                Ok(()) => "restored".to_string(),
                Err(err) => {
                    results.push((row.path.clone(), format!("failed:{err}")));
                    return Err(WhError::new(
                        ErrorCode::IoError,
                        format!("恢复 {} 失败：{err}；已完成的恢复保留", row.path),
                    ));
                }
            },
            (None, Some("absent")) => match std::fs::remove_file(&resolved) {
                Ok(()) => "removed".to_string(),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    "already_absent".to_string()
                }
                Err(err) => {
                    return Err(WhError::new(
                        ErrorCode::IoError,
                        format!("删除 {} 失败：{err}", row.path),
                    ));
                }
            },
            (None, Some(reason)) => format!("skipped:{reason}"),
            (None, None) => "skipped:no_content".to_string(),
        };
        results.push((row.path, outcome));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};
    use whcode_store::{apply_migrations, create_session_with_id};

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn workspace(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "whcode-preimage-{tag}-{}-{seq}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("工作区");
        dir
    }

    #[test]
    fn capture_classifies_existing_new_oversize_and_non_utf8() {
        let dir = workspace("capture");
        std::fs::write(dir.join("a.txt"), "原文\n").expect("文件");
        std::fs::write(
            dir.join("big.txt"),
            "x".repeat(whcode_store::preimage::MAX_PREIMAGE_BYTES + 1),
        )
        .expect("大文件");
        std::fs::write(dir.join("bin.dat"), [0xff, 0xfe, 0x00]).expect("二进制");

        let captured = capture(&dir, "a.txt").expect("存在文件");
        assert_eq!(
            captured,
            PreimageCapture::Content {
                size: 7,
                text: "原文\n".to_string()
            }
        );

        assert_eq!(
            capture(&dir, "new.txt").expect("新建"),
            PreimageCapture::Absent
        );
        assert_eq!(
            capture(&dir, "big.txt").expect("超限"),
            PreimageCapture::Skipped {
                size: whcode_store::preimage::MAX_PREIMAGE_BYTES as i64 + 1,
                reason: "too_large"
            }
        );
        assert_eq!(
            capture(&dir, "bin.dat").expect("非 UTF-8"),
            PreimageCapture::Skipped {
                size: 3,
                reason: "not_utf8"
            }
        );

        let err = capture(&dir, "../escape").expect_err("逃逸");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_after_success_stores_and_prunes_to_bounds() {
        let conn = migrated();
        create_session_with_id(&conn, "S-P", "前像", "active", 1_000).expect("创建");
        for index in 0..whcode_store::preimage::MAX_PREIMAGES_PER_SESSION as i64 + 5 {
            record_after_success(
                &conn,
                "S-P",
                &format!("TX-{index}"),
                &format!("out/f{index}.txt"),
                PreimageCapture::Content {
                    size: 10,
                    text: "v".repeat(10),
                },
            )
            .expect("登记");
        }
        let rows = whcode_store::preimage::list(
            &conn,
            "S-P",
            whcode_store::preimage::MAX_PREIMAGES_PER_SESSION,
        )
        .expect("清单");
        assert_eq!(
            rows.len(),
            whcode_store::preimage::MAX_PREIMAGES_PER_SESSION,
            "超出上限的最旧前像被裁剪"
        );
        // 裁剪保留最新的：最老 5 条（TX-0..TX-4）应已删除。
        let oldest = whcode_store::preimage::by_exec(&conn, "S-P", "TX-0").expect("按执行读");
        assert!(oldest.is_empty(), "最老前像已被裁剪");
        let newest = whcode_store::preimage::by_exec(&conn, "S-P", "TX-54").expect("最新");
        assert_eq!(newest.len(), 1);
    }

    #[test]
    fn restore_writes_back_and_removes_absent_with_audit() {
        let conn = migrated();
        create_session_with_id(&conn, "S-R", "恢复", "active", 1_000).expect("创建");
        let dir = workspace("restore");
        std::fs::write(dir.join("doc.md"), "改后\n").expect("当前内容");
        record_after_success(
            &conn,
            "S-R",
            "TX-1",
            "doc.md",
            PreimageCapture::Content {
                size: 6,
                text: "改前\n".to_string(),
            },
        )
        .expect("登记");
        record_after_success(&conn, "S-R", "TX-2", "created.log", PreimageCapture::Absent)
            .expect("新建登记");
        // 模拟“当时新建、之后仍存在”：恢复应删除它。
        std::fs::write(
            dir.join("created.log"),
            "后建
",
        )
        .expect("新建文件");

        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        // 预览不写工作区。
        let rows = preview(&conn, &policy, &actor, "S-R", "TX-1").expect("预览");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            std::fs::read_to_string(dir.join("doc.md")).expect("未动"),
            "改后\n"
        );

        let results = restore(&conn, &policy, &actor, "S-R", &dir, "TX-1").expect("恢复");
        assert_eq!(
            results,
            vec![("doc.md".to_string(), "restored".to_string())]
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("doc.md")).expect("已恢复"),
            "改前\n"
        );

        let results = restore(&conn, &policy, &actor, "S-R", &dir, "TX-2").expect("删除新建");
        assert_eq!(
            results,
            vec![("created.log".to_string(), "removed".to_string())]
        );
        assert!(!dir.join("created.log").exists());

        let audits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM audit WHERE action = 'preimage_restore'",
                [],
                |row| row.get(0),
            )
            .expect("审计计数");
        assert_eq!(audits, 2, "每次恢复一条审计");

        let weak = Actor::new("weak", []);
        let err = restore(&conn, &policy, &weak, "S-R", &dir, "TX-1").expect_err("无能力");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        let err = preview(&conn, &policy, &actor, "S-R", "TX-none").expect_err("无前像");
        assert_eq!(err.code, ErrorCode::NotFound);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同一执行可再次恢复：恢复不是幂等操作，第二次调用不得因审计主键冲突被拒
    /// （ID 含毫秒时间戳，见 `restore`）。
    #[test]
    fn restore_same_exec_twice_writes_two_audits() {
        let conn = migrated();
        create_session_with_id(&conn, "S-R2", "恢复", "active", 1_000).expect("创建");
        let dir = workspace("restore-twice");
        std::fs::write(dir.join("doc.md"), "第二改\n").expect("当前内容");
        record_after_success(
            &conn,
            "S-R2",
            "TX-9",
            "doc.md",
            PreimageCapture::Content {
                size: 6,
                text: "改前\n".to_string(),
            },
        )
        .expect("登记");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        restore(&conn, &policy, &actor, "S-R2", &dir, "TX-9").expect("首次恢复");
        // 用户又改坏文件，再恢复一次。
        std::fs::write(dir.join("doc.md"), "又改坏\n").expect("再改");
        restore(&conn, &policy, &actor, "S-R2", &dir, "TX-9").expect("二次恢复");
        assert_eq!(
            std::fs::read_to_string(dir.join("doc.md")).expect("内容"),
            "改前\n"
        );
        let audits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM audit WHERE action = 'preimage_restore'",
                [],
                |row| row.get(0),
            )
            .expect("审计计数");
        assert_eq!(audits, 2, "两次恢复各一条审计，互不覆盖");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
