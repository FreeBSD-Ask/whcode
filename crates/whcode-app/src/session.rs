//! # 会话命令（T046/T047、FR-101/FR-102、ADR-018）
//!
//! 命令编排：权限检查（T050）→ 存储原语（`whcode-store::directory`）→ 结果。
//! 删除是破坏性命令，复用 [`whcode_store::purge`] 的预览/确认/审计/活跃保护全链
//! （FR-509）；归档复用 [`whcode_store::set_archived`]。
//!
//! ## 会话切换（FR-102、C-03、V-101）
//!
//! [`switch_session`] 只返回四类有界数据：元数据一行、预聚合概览一行（最终一致）、
//! 最新 [`SWITCH_TAIL_LIMIT`] 条活跃尾部、工作区事件游标。**不**加载完整历史；
//! 尾部是否截断以 `tail_complete` 标注。切换是纯读操作，不更新 `last_active_at`
//! （活跃性由命令面在 M3+ 随命令写入维护；避免读路径产生写副作用）。
//!
//! 边界：无 SQL；错误语义统一走 [`whcode_domain::WhError`]。

use duckdb::Connection;
use whcode_domain::{ErrorCode, WhError};
use whcode_store::directory::{self, SessionRow};
use whcode_store::fork::{ForkOutcome, fork_session as fork_session_store};
use whcode_store::overview as overview_store;
use whcode_store::query::{self, Direction, EventRow};
use whcode_store::{
    PurgeCriteria, PurgePreview, PurgeProtection, PurgeResult, execute_purge, preview_purge,
    set_archived,
};

use crate::authz::{Actor, Capability, Policy, require};

/// 会话列表的默认上限（有界读取；调用方可传更小值）。
pub const LIST_SESSIONS_LIMIT: usize = 200;

/// 会话切换返回的活跃尾部条数上限（FR-102"有限活跃尾部"）。
pub const SWITCH_TAIL_LIMIT: usize = 50;

/// 侧栏搜索词上限（字符数；超限返回 `invalid_state`，不静默截断）。
pub const MAX_SESSION_SEARCH_CHARS: usize = 128;

/// 会话目录排序（FR-746）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionSort {
    /// 最近活跃优先（默认，与既有列表顺序一致）。
    #[default]
    LastActive,
    /// 创建时间新者优先。
    Created,
    /// 标题字典序（大小写不敏感），同序按 ID。
    Title,
}

/// 会话分组（侧栏分段，FR-746）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionGroup {
    /// 未归档。
    Active,
    /// 已归档。
    Archived,
}

/// 目录条目：会话行 + 所属分组。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionEntry {
    /// 会话元数据。
    pub session: SessionRow,
    /// 分组（归档分段）。
    pub group: SessionGroup,
}

/// 目录查询（GUI 与 Web 共用；FR-610 同批纪律）。
///
/// `search` 匹配标题或 ID（大小写不敏感，去首尾空白）；`include_archived`
/// 为 `false` 时归档条目被过滤，为 `true` 时归档排在活跃之后（组内再按
/// `sort` 排序）。`limit` 由调用方给出并封顶于 [`LIST_SESSIONS_LIMIT`]。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionDirectoryQuery {
    /// 搜索词（空＝不过滤）。
    pub search: String,
    /// 排序键。
    pub sort: SessionSort,
    /// 是否包含归档会话。
    pub include_archived: bool,
    /// 条数上限（0 视为 1）。
    pub limit: usize,
}

/// 生成会话目录（FR-746）：过滤 → 排序 → 分组，全部有界。
///
/// # Errors
///
/// 权限拒绝、搜索词超长（[`MAX_SESSION_SEARCH_CHARS`]）或存储失败时返回
/// [`WhError`]。
pub fn session_directory(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    query: &SessionDirectoryQuery,
) -> Result<Vec<SessionEntry>, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    let search = query.search.trim();
    if search.chars().count() > MAX_SESSION_SEARCH_CHARS {
        return Err(WhError::invalid_state(format!(
            "搜索词超过 {MAX_SESSION_SEARCH_CHARS} 字符"
        )));
    }
    let needle = search.to_lowercase();
    let limit = query.limit.clamp(1, LIST_SESSIONS_LIMIT);
    // 归档过滤在应用层完成：一次读取两类分组，避免两条 SQL 与两次权限判定。
    let rows = directory::list_sessions(conn, true, limit)?;
    let mut entries: Vec<SessionEntry> = rows
        .into_iter()
        .filter(|row| query.include_archived || !row.archived)
        .filter(|row| {
            needle.is_empty()
                || row.title.to_lowercase().contains(&needle)
                || row.id.to_lowercase().contains(&needle)
        })
        .map(|session| SessionEntry {
            group: if session.archived {
                SessionGroup::Archived
            } else {
                SessionGroup::Active
            },
            session,
        })
        .collect();
    entries.sort_by(|left, right| {
        let group_rank = |group: SessionGroup| match group {
            SessionGroup::Active => 0,
            SessionGroup::Archived => 1,
        };
        group_rank(left.group)
            .cmp(&group_rank(right.group))
            .then_with(|| match query.sort {
                SessionSort::LastActive => right
                    .session
                    .last_active_at_ms
                    .cmp(&left.session.last_active_at_ms),
                SessionSort::Created => {
                    right.session.created_at_ms.cmp(&left.session.created_at_ms)
                }
                SessionSort::Title => left
                    .session
                    .title
                    .to_lowercase()
                    .cmp(&right.session.title.to_lowercase()),
            })
            .then_with(|| left.session.id.cmp(&right.session.id))
    });
    entries.truncate(limit);
    Ok(entries)
}

/// 创建会话请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSession {
    /// 标题（非空）。
    pub title: String,
    /// 模型标识（可空；M2 不校验取值）。
    pub model: Option<String>,
}

/// 会话切换视图：元数据 + 概览 + 有界尾部 + 游标（FR-102）。
#[derive(Debug, Clone, PartialEq)]
pub struct SessionView {
    /// 会话元数据。
    pub session: SessionRow,
    /// 预聚合概览（最终一致；`refreshed_at` 指示滞后）。
    pub overview: Option<whcode_store::SessionOverview>,
    /// 活跃尾部：最新事件，按 `seq` 降序，至多 [`SWITCH_TAIL_LIMIT`] 条。
    pub tail: Vec<EventRow>,
    /// 工作区事件游标（`events.seq` 最大值；断线补拉基线，FR-202/605）。
    pub event_cursor: i64,
    /// `false` 表示会话事件数超过尾部上限，尾部被截断。
    pub tail_complete: bool,
}

fn not_found(context: &str, id: &str) -> WhError {
    WhError::new(ErrorCode::NotFound, format!("{context}不存在：{id}"))
}

/// 创建会话（FR-101）。
///
/// # Errors
///
/// 权限拒绝或存储失败时返回 [`WhError`]。
pub fn new_session(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    request: NewSession,
    now_ms: u64,
) -> Result<SessionRow, WhError> {
    require(policy, actor, Capability::SessionCreate)?;
    directory::create_session(
        conn,
        &request.title,
        request.model.as_deref(),
        "active",
        now_ms,
    )
}

/// 分叉会话（FR-769／ADR-065）：在源会话 turn 结束边界创建新会话并复制对话历史前缀。
///
/// 权限按创建会话判定（分叉产生新会话行）；`title` 缺省取"{源标题} (fork)"，空白标题
/// 一律走默认值。边界与复制语义见 [`whcode_store::fork_session`]（store 原语是唯一
/// SQL 落点，ADR-002）。
///
/// # Errors
///
/// 权限拒绝、源会话不存在（`not_found`）、边界不是 turn 结束或继承量超限
/// （`invalid_state`）、存储失败（`io_error`）时返回 [`WhError`]。
pub fn fork_session(
    conn: &mut Connection,
    policy: &dyn Policy,
    actor: &Actor,
    source_session_id: &str,
    at_seq: i64,
    title: Option<&str>,
    now_ms: u64,
) -> Result<ForkOutcome, WhError> {
    require(policy, actor, Capability::SessionCreate)?;
    let title = match title.map(str::trim).filter(|t| !t.is_empty()) {
        Some(title) => title.to_string(),
        None => {
            let source = directory::get_session(conn, source_session_id)?
                .ok_or_else(|| not_found("会话", source_session_id))?;
            format!("{} (fork)", source.title)
        }
    };
    fork_session_store(conn, source_session_id, at_seq, &title, now_ms)
}

/// 列出会话（FR-101）；默认不含归档会话。
///
/// # Errors
///
/// 权限拒绝、`limit` 越界或存储失败时返回 [`WhError`]。
pub fn list_sessions(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    include_archived: bool,
    limit: usize,
) -> Result<Vec<SessionRow>, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    directory::list_sessions(conn, include_archived, limit)
}

/// 查看会话元数据（FR-101 show）；不存在返回 `not_found`。
///
/// # Errors
///
/// 权限拒绝、会话不存在或存储失败时返回 [`WhError`]。
pub fn show_session(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
) -> Result<SessionRow, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    directory::get_session(conn, session_id)?.ok_or_else(|| not_found("会话", session_id))
}

/// 重命名会话（FR-101）；返回更新后的元数据。
///
/// # Errors
///
/// 权限拒绝、会话不存在、标题为空或存储失败时返回 [`WhError`]。
pub fn rename_session(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    new_title: &str,
) -> Result<SessionRow, WhError> {
    require(policy, actor, Capability::SessionRename)?;
    directory::rename_session(conn, session_id, new_title)?;
    directory::get_session(conn, session_id)?.ok_or_else(|| not_found("会话", session_id))
}

/// 归档/取消归档会话（FR-101 archive）。
///
/// 存储层在同一事务内写 `session_archive` 审计（ADR-038），应用层只做权限判定与透传。
///
/// # Errors
///
/// 权限拒绝、会话不存在（全有或全无）或存储失败时返回 [`WhError`]。
pub fn set_session_archived(
    conn: &mut Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_ids: &[&str],
    archived: bool,
) -> Result<usize, WhError> {
    require(policy, actor, Capability::SessionArchive)?;
    set_archived(conn, session_ids, archived, &actor.id)
}

/// 删除会话（FR-101 delete；破坏性命令）。
///
/// 复用清理全链：预览（`confirm = false` 时可直接作为 dry-run 输出）→ 显式确认 →
/// 审计 → 活跃会话保护。`protection` 由调用方传入（应用层把当前打开的会话放入
/// `protect_ids`）；目标会话受保护时按策略跳过或整体拒绝。
///
/// # Errors
///
/// 权限拒绝、确认缺失、活跃保护命中 `Abort` 或存储失败时返回 [`WhError`]。
pub fn delete_session(
    conn: &mut Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    confirm: bool,
    protection: &PurgeProtection,
    now_ms: u64,
) -> Result<PurgeResult, WhError> {
    require(policy, actor, Capability::SessionDelete)?;
    let criteria = PurgeCriteria::sessions([session_id]);
    let preview = preview_purge(conn, &criteria, protection, now_ms)?;
    execute_purge(conn, &preview, confirm, &actor.id, protection, now_ms)
}

/// 删除命令的预览（dry-run）：不写库，返回将删除的集合与行数。
///
/// # Errors
///
/// 权限拒绝或存储失败时返回 [`WhError`]。
pub fn preview_delete_session(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    protection: &PurgeProtection,
    now_ms: u64,
) -> Result<PurgePreview, WhError> {
    require(policy, actor, Capability::SessionDelete)?;
    let criteria = PurgeCriteria::sessions([session_id]);
    preview_purge(conn, &criteria, protection, now_ms)
}

/// 切换会话（FR-102）：仅加载元数据、概览、有界尾部与游标。
///
/// # Errors
///
/// 权限拒绝、会话不存在或存储失败时返回 [`WhError`]。
pub fn switch_session(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
) -> Result<SessionView, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    let session =
        directory::get_session(conn, session_id)?.ok_or_else(|| not_found("会话", session_id))?;
    let overview = overview_store::load_overview(conn, session_id)?;

    // 多取 1 条用于判定"尾部是否被截断"，随后裁回上限——有界加载，不拉全量。
    let page = query::events_page(
        conn,
        session_id,
        Direction::Backward,
        None,
        SWITCH_TAIL_LIMIT + 1,
    )?;
    let tail_complete = page.items.len() <= SWITCH_TAIL_LIMIT;
    let mut tail = page.items;
    tail.truncate(SWITCH_TAIL_LIMIT);

    let event_cursor = query::latest_seq(conn)?;
    Ok(SessionView {
        session,
        overview,
        tail,
        event_cursor,
        tail_complete,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};
    use duckdb::params;
    use whcode_store::{apply_migrations, get_session};

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn seed_events(conn: &Connection, session_id: &str, count: i64) {
        for seq in 1..=count {
            conn.execute(
                "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                     kind, payload_json, ts, persistence_level)
                 VALUES (?, ?, ?, NULL, NULL, NULL, 'StreamingChunk', CAST('{}' AS JSON)::VARIANT,
                         current_timestamp, 'redacted')",
                params![seq, format!("E-{seq}"), session_id],
            )
            .expect("插入事件");
        }
    }

    #[test]
    fn session_directory_filters_sorts_and_groups_archived() {
        let mut conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        for (title, now) in [("Alpha", 1_000u64), ("beta", 2_000), ("Gamma", 3_000)] {
            new_session(
                &conn,
                &policy,
                &actor,
                NewSession {
                    title: title.to_string(),
                    model: None,
                },
                now,
            )
            .expect("创建会话");
        }
        let all = list_sessions(&conn, &policy, &actor, false, 10).expect("列表");
        let beta = all.iter().find(|row| row.title == "beta").expect("beta");
        set_session_archived(&mut conn, &policy, &actor, &[beta.id.as_str()], true).expect("归档");

        let active = SessionDirectoryQuery {
            search: String::new(),
            sort: SessionSort::Title,
            include_archived: false,
            limit: 10,
        };
        let entries = session_directory(&conn, &policy, &actor, &active).expect("目录");
        assert_eq!(entries.len(), 2, "默认不含归档");
        assert!(
            entries
                .iter()
                .all(|entry| entry.group == SessionGroup::Active)
        );
        assert_eq!(
            entries[0].session.title, "Alpha",
            "标题字典序（大小写不敏感）"
        );
        assert_eq!(entries[1].session.title, "Gamma");

        let with_archived = SessionDirectoryQuery {
            include_archived: true,
            ..active.clone()
        };
        let entries = session_directory(&conn, &policy, &actor, &with_archived).expect("含归档");
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries.last().expect("尾项").group,
            SessionGroup::Archived,
            "归档排在活跃之后"
        );

        let searched = SessionDirectoryQuery {
            search: "gam".to_string(),
            ..with_archived
        };
        let entries = session_directory(&conn, &policy, &actor, &searched).expect("搜索");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session.title, "Gamma");

        let oversized = SessionDirectoryQuery {
            search: "x".repeat(MAX_SESSION_SEARCH_CHARS + 1),
            ..SessionDirectoryQuery::default()
        };
        let err = session_directory(&conn, &policy, &actor, &oversized).expect_err("超长搜索词");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
        assert!(!err.retryable);
    }

    #[test]
    fn session_lifecycle_commands_roundtrip() {
        let mut conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let created = new_session(
            &conn,
            &policy,
            &actor,
            NewSession {
                title: "会话一".to_string(),
                model: Some("gpt-x".to_string()),
            },
            1_000,
        )
        .expect("创建");
        assert!(created.id.starts_with("S-"));
        assert_eq!(created.status, "active");

        let listed =
            list_sessions(&conn, &policy, &actor, false, LIST_SESSIONS_LIMIT).expect("列表");
        assert_eq!(listed.len(), 1);

        let renamed =
            rename_session(&conn, &policy, &actor, &created.id, "新标题").expect("重命名");
        assert_eq!(renamed.title, "新标题");

        let archived =
            set_session_archived(&mut conn, &policy, &actor, &[created.id.as_str()], true)
                .expect("归档");
        assert_eq!(archived, 1);
        let listed = list_sessions(&conn, &policy, &actor, false, LIST_SESSIONS_LIMIT)
            .expect("不含归档列表");
        assert!(listed.is_empty(), "归档会话不出现在默认列表");

        let shown = show_session(&conn, &policy, &actor, &created.id).expect("show");
        assert!(shown.archived);
        assert_eq!(shown.title, "新标题");
    }

    #[test]
    fn missing_session_reports_not_found_on_show_rename_switch() {
        let conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let err = show_session(&conn, &policy, &actor, "S-none").expect_err("show");
        assert_eq!(err.code, ErrorCode::NotFound);
        let err = rename_session(&conn, &policy, &actor, "S-none", "x").expect_err("rename");
        assert_eq!(err.code, ErrorCode::NotFound);
        let err = switch_session(&conn, &policy, &actor, "S-none").expect_err("switch");
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[test]
    fn permission_denial_is_uniform_across_all_session_commands() {
        let mut conn = migrated();
        let policy = DefaultPolicy;
        let actor = Actor::new("受限用户", []); // 无任何能力

        for (name, result) in [
            (
                "create",
                new_session(
                    &conn,
                    &policy,
                    &actor,
                    NewSession {
                        title: "t".into(),
                        model: None,
                    },
                    1,
                )
                .map(|_| ()),
            ),
            (
                "list",
                list_sessions(&conn, &policy, &actor, false, 10).map(|_| ()),
            ),
            (
                "show",
                show_session(&conn, &policy, &actor, "S-1").map(|_| ()),
            ),
            (
                "rename",
                rename_session(&conn, &policy, &actor, "S-1", "x").map(|_| ()),
            ),
            (
                "archive",
                set_session_archived(&mut conn, &policy, &actor, &["S-1"], true).map(|_| ()),
            ),
            (
                "delete",
                delete_session(
                    &mut conn,
                    &policy,
                    &actor,
                    "S-1",
                    true,
                    &PurgeProtection::default(),
                    1,
                )
                .map(|_| ()),
            ),
            (
                "switch",
                switch_session(&conn, &policy, &actor, "S-1").map(|_| ()),
            ),
        ] {
            let err = result.expect_err("无能力调用必须拒绝");
            assert_eq!(err.code, ErrorCode::PermissionDenied, "命令 {name}");
            assert!(!err.retryable, "命令 {name} 的拒绝不可重试");
        }
        assert_eq!(
            get_session(&conn, "S-1").expect("库可读"),
            None,
            "拒绝路径不得产生副作用"
        );
    }

    #[test]
    fn switch_loads_bounded_tail_overview_and_cursor() {
        let conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let created = new_session(
            &conn,
            &policy,
            &actor,
            NewSession {
                title: "切换".into(),
                model: None,
            },
            1_000,
        )
        .expect("创建");
        seed_events(&conn, &created.id, 200);

        let view = switch_session(&conn, &policy, &actor, &created.id).expect("切换");
        assert_eq!(view.session.id, created.id);
        assert_eq!(view.tail.len(), SWITCH_TAIL_LIMIT, "尾部必须有界");
        assert!(!view.tail_complete, "200 条事件超过尾部上限");
        assert_eq!(view.tail[0].seq, 200, "尾部按 seq 降序（最新在前）");
        assert_eq!(view.event_cursor, 200);
        assert!(view.overview.is_none(), "无任务会话无概览行");
    }

    #[test]
    fn switch_tail_marks_completeness_when_within_limit() {
        let conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let created = new_session(
            &conn,
            &policy,
            &actor,
            NewSession {
                title: "小会话".into(),
                model: None,
            },
            1_000,
        )
        .expect("创建");
        seed_events(&conn, &created.id, 10);

        let view = switch_session(&conn, &policy, &actor, &created.id).expect("切换");
        assert_eq!(view.tail.len(), 10);
        assert!(view.tail_complete, "未超上限时必须标注完整");
    }

    #[test]
    fn delete_requires_confirmation_and_respects_protection() {
        let mut conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let target = new_session(
            &conn,
            &policy,
            &actor,
            NewSession {
                title: "待删".into(),
                model: None,
            },
            1_000,
        )
        .expect("创建");
        let open_session = new_session(
            &conn,
            &policy,
            &actor,
            NewSession {
                title: "打开中".into(),
                model: None,
            },
            1_000,
        )
        .expect("创建");

        // dry-run：只预览，不删除。now_ms 取远晚于创建时刻（1_000）的值，
        // 使目标不在默认 24h 活跃窗口内；打开中的会话由 protect_ids 显式保护。
        let later = 10_000_000_000u64;
        let protection = PurgeProtection {
            protect_ids: vec![open_session.id.clone()],
            ..PurgeProtection::default()
        };
        let preview =
            preview_delete_session(&conn, &policy, &actor, &target.id, &protection, later)
                .expect("预览");
        assert_eq!(preview.selected_sessions, vec![target.id.clone()]);
        assert!(
            show_session(&conn, &policy, &actor, &target.id).is_ok(),
            "预览不删除"
        );

        // 未确认：拒绝。
        let err = delete_session(
            &mut conn,
            &policy,
            &actor,
            &target.id,
            false,
            &protection,
            later,
        )
        .expect_err("未确认必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        // 确认后删除；打开中的会话不受影响。
        let result = delete_session(
            &mut conn,
            &policy,
            &actor,
            &target.id,
            true,
            &protection,
            later,
        )
        .expect("确认删除");
        assert_eq!(result.deleted_sessions, vec![target.id.clone()]);
        assert!(show_session(&conn, &policy, &actor, &target.id).is_err());
        assert!(show_session(&conn, &policy, &actor, &open_session.id).is_ok());
    }

    #[test]
    fn delete_rejects_protected_session_under_abort_policy() {
        let mut conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let target = new_session(
            &conn,
            &policy,
            &actor,
            NewSession {
                title: "打开中".into(),
                model: None,
            },
            1_000,
        )
        .expect("创建");

        let protection = PurgeProtection {
            protect_ids: vec![target.id.clone()],
            on_protected: whcode_store::OnProtected::Abort,
            ..PurgeProtection::default()
        };
        // 预览即拒绝（Abort 策略）。
        let err = preview_delete_session(&conn, &policy, &actor, &target.id, &protection, 1_000)
            .expect_err("Abort 策略必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = delete_session(
            &mut conn,
            &policy,
            &actor,
            &target.id,
            true,
            &protection,
            1_000,
        )
        .expect_err("执行同样拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(
            show_session(&conn, &policy, &actor, &target.id).is_ok(),
            "拒绝不得删除"
        );
    }

    fn seed_boundary_turn(conn: &mut Connection, session_id: &str) {
        whcode_store::append_turn(
            conn,
            session_id,
            &[
                whcode_store::TurnEventDraft {
                    kind: whcode_domain::EventKind::TurnStarted,
                    request_id: Some("req-1".to_string()),
                    tool_call_id: None,
                    payload: serde_json::json!({"kind": "TurnStarted"}),
                },
                whcode_store::TurnEventDraft {
                    kind: whcode_domain::EventKind::TurnFinished,
                    request_id: Some("req-1".to_string()),
                    tool_call_id: None,
                    payload: serde_json::json!({"kind": "TurnFinished"}),
                },
            ],
            &[whcode_store::TurnMessageDraft {
                role: "assistant",
                content: "边界轮回复".to_string(),
                token_count: Some(6),
                parent_id: None,
            }],
        )
        .expect("边界轮落表");
    }

    #[test]
    fn fork_session_creates_child_with_default_title_and_prefix() {
        let mut conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let source = new_session(
            &conn,
            &policy,
            &actor,
            NewSession {
                title: "源会话".to_string(),
                model: None,
            },
            1_000,
        )
        .expect("创建");
        seed_boundary_turn(&mut conn, &source.id);

        let outcome =
            fork_session(&mut conn, &policy, &actor, &source.id, 2, None, 2_000).expect("分叉");
        assert_eq!(outcome.source_session, source.id);
        assert_eq!(outcome.forked_at_seq, 2);
        assert_eq!(outcome.inherited_events, 2);
        assert_eq!(outcome.inherited_messages, 1);
        assert_eq!(outcome.session.title, "源会话 (fork)", "缺省标题带来源标记");

        let child_events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id = ?",
                params![outcome.session.id],
                |row| row.get(0),
            )
            .expect("子会话事件");
        let child_messages: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = ?",
                params![outcome.session.id],
                |row| row.get(0),
            )
            .expect("子会话消息");
        assert_eq!((child_events, child_messages), (2, 1));

        // 显式标题覆盖默认值；半截 turn 边界透传 store 的 invalid_state。
        let titled = fork_session(
            &mut conn,
            &policy,
            &actor,
            &source.id,
            0,
            Some("  自定  "),
            3_000,
        )
        .expect("显式标题");
        assert_eq!(titled.session.title, "自定", "标题去首尾空白");
        let mid_turn = fork_session(
            &mut conn,
            &policy,
            &actor,
            &source.id,
            1,
            Some("半截"),
            3_000,
        )
        .expect_err("turn 内边界必须拒绝");
        assert_eq!(mid_turn.code, ErrorCode::InvalidState);
        let missing =
            fork_session(&mut conn, &policy, &actor, "S-404", 0, None, 3_000).expect_err("缺失源");
        assert_eq!(missing.code, ErrorCode::NotFound);
    }

    #[test]
    fn fork_session_requires_session_create_capability() {
        let mut conn = migrated();
        let policy = DefaultPolicy;
        let actor = Actor::new("limited", [Capability::SessionRead]);
        let source = new_session(
            &conn,
            &policy,
            &actor_with_all_capabilities("admin"),
            NewSession {
                title: "源".to_string(),
                model: None,
            },
            1_000,
        )
        .expect("创建");
        seed_boundary_turn(&mut conn, &source.id);

        let err = fork_session(&mut conn, &policy, &actor, &source.id, 2, None, 2_000)
            .expect_err("无 SessionCreate 必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
    }
}
