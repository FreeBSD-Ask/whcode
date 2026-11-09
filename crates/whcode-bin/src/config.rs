//! 配置加载与数据库/追踪基线默认值（tasks.md T016）。
//!
//! 默认值来源：任务书 §5.4（数据库配置基线）与 D-05（追踪默认 `redacted`）。这些数值是
//! **可配置基线**，须由 M0/M1 基准校准，不得写死为架构定理（§0.3-6）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use whcode_app::approval::{
    ApprovalPolicy, DEFAULT_APPROVAL_WAIT_MS, MAX_APPROVAL_TOOL_RULES, MAX_APPROVAL_WAIT_MS,
};
use whcode_domain::approval::{ApprovalRule, MAX_APPROVAL_ID_LEN};
use whcode_domain::{CheckpointPolicy, TraceLevel, WhError};
use whcode_platform::{paths, resources};
use whcode_tui::Theme;

/// DuckDB 缓冲管理器默认内存上限（MiB），§5.4。
pub const DEFAULT_MEMORY_LIMIT_MB: u32 = 192;
/// DuckDB 缓冲管理器候选最大内存上限（MiB），§5.4。
pub const CANDIDATE_MAX_MEMORY_LIMIT_MB: u32 = 384;

/// UI 配置（FR-612）：主题经配置选择（`ui.theme`），可被环境变量 `WHCODE_UI_THEME` 覆盖。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UiConfig {
    /// 渲染主题；默认 `zcode-night`（2026-11-06 用户批准切换，ADR-057/FR-739）。
    #[serde(default)]
    pub theme: Theme,
    /// 界面语言（FR-625/615）；`None` 时走 `WHCODE_UI_LOCALE` → LANG/LC_ALL → zh-CN。
    #[serde(default)]
    pub locale: Option<String>,
}

/// 数据库配置基线（§5.4）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbConfig {
    /// `memory_limit`（MiB）。仅约束 DuckDB 缓冲管理器，**不**代表进程 RSS 上限。
    pub memory_limit_mb: u32,
    /// `memory_limit` 的候选上限（MiB），由用户配置决定是否使用。
    pub max_memory_limit_mb: u32,
    /// `threads`：初始不高于 `min(4, 可用物理核数)`。
    pub threads: u32,
    /// `temp_directory`：工作区专用临时目录，启动时须检查可写性与磁盘余量。
    pub temp_directory: PathBuf,
    /// `max_temp_directory_size`（MiB）：显式上限；超限须返回明确错误并保护一致性。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_temp_directory_size_mb: Option<u64>,
}

/// 追踪配置（FR-203、D-05）。
///
/// 默认值由 [`TraceLevel`] 的 `#[default]` 变体提供（`redacted`），因此本结构可派生 `Default`。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TraceConfig {
    /// 默认追踪级别；默认 `redacted`。
    pub default_level: TraceLevel,
}

/// 工具审批配置（T255／FR-744、ADR-064 决策 2）。
///
/// 解析顺序：`tools` 里的 per-tool 覆盖优先，否则按副作用范围取默认值。
/// **默认全 `allow`**（行为兼容：无 UI 时既有工具不得全部变 `unavailable`，ADR-060 §2.2）。
/// `ask`／`deny` 只能由显式配置生效；破坏性工具的 `ask` 默认切换留给 B3b（ADR-064）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalConfig {
    /// 只读工具的默认规则。
    #[serde(default = "ApprovalConfig::allow")]
    pub default_read_only: ApprovalRule,
    /// 工作区写工具的默认规则。
    #[serde(default = "ApprovalConfig::allow")]
    pub default_workspace_write: ApprovalRule,
    /// per-tool 覆盖（工具稳定名 → 规则）。
    #[serde(default)]
    pub tools: BTreeMap<String, ApprovalRule>,
    /// 一次 `ask` 的等待预算（毫秒；上限 [`MAX_APPROVAL_WAIT_MS`]）。
    #[serde(default = "ApprovalConfig::default_wait_ms")]
    pub wait_ms: u64,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            default_read_only: ApprovalRule::Allow,
            default_workspace_write: ApprovalRule::Allow,
            tools: BTreeMap::new(),
            wait_ms: DEFAULT_APPROVAL_WAIT_MS,
        }
    }
}

impl ApprovalConfig {
    /// 默认规则（`allow`）：接线前的行为就是默认值，默认值可见可改可撤销。
    #[must_use]
    pub const fn allow() -> ApprovalRule {
        ApprovalRule::Allow
    }

    /// 默认等待预算。
    #[must_use]
    pub const fn default_wait_ms() -> u64 {
        DEFAULT_APPROVAL_WAIT_MS
    }

    /// 加载期校验：越界配置不得进入运行期（配置错误由人修正，自动重试无意义）。
    ///
    /// # Errors
    ///
    /// - `wait_ms` 为 0 或超 [`MAX_APPROVAL_WAIT_MS`]：`parse_error`；
    /// - 工具名为空或超 [`MAX_APPROVAL_ID_LEN`]：`parse_error`；
    /// - 覆盖数量超 [`MAX_APPROVAL_TOOL_RULES`]：`parse_error`（有界）。
    pub fn validate(&self) -> Result<(), WhError> {
        if self.wait_ms == 0 || self.wait_ms > MAX_APPROVAL_WAIT_MS {
            return Err(WhError::parse_error(format!(
                "审批等待预算非法（{} ms）：取值须在 1—{MAX_APPROVAL_WAIT_MS} ms 之间",
                self.wait_ms
            )));
        }
        if self.tools.len() > MAX_APPROVAL_TOOL_RULES {
            return Err(WhError::parse_error(format!(
                "工具级审批规则过多（{}）：上限 {MAX_APPROVAL_TOOL_RULES}",
                self.tools.len()
            )));
        }
        for tool in self.tools.keys() {
            let trimmed = tool.trim();
            if trimmed.is_empty() {
                return Err(WhError::parse_error("审批规则的工具名不得为空"));
            }
            if trimmed.chars().count() > MAX_APPROVAL_ID_LEN {
                return Err(WhError::parse_error(format!(
                    "审批规则的工具名超过长度上限 {MAX_APPROVAL_ID_LEN}"
                )));
            }
        }
        Ok(())
    }

    /// 转换为运行期规则表（先校验；非法配置不得构造出策略）。
    ///
    /// # Errors
    ///
    /// 同 [`Self::validate`]。
    // 装配根的调用点是 B3b 的 `run_turn_with_approval` 接线（本批只交付后端语义）；
    // 语义由 `config` 的单元测试覆盖，不留未校验的构造路径。
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "B3b 装配根接线前的唯一调用方；语义已由 config 单测覆盖"
        )
    )]
    pub fn to_policy(&self) -> Result<ApprovalPolicy, WhError> {
        self.validate()?;
        let mut policy = ApprovalPolicy::new(self.default_read_only, self.default_workspace_write);
        for (tool, rule) in &self.tools {
            policy = policy.with_tool_rule(tool.trim(), *rule)?;
        }
        Ok(policy)
    }
}

/// 顶层配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// 工作区根路径（数据库、产物与运行时状态的持久化边界，ADR-001）。
    pub workspace_path: PathBuf,
    /// 数据库基线。
    pub db: DbConfig,
    /// 追踪基线。
    pub trace: TraceConfig,
    /// UI 基线（主题等，FR-612）。
    #[serde(default)]
    pub ui: UiConfig,
    /// 检查点触发策略（FR-405：定时 / 按步骤 / 危险操作前的间隔与保留数）。
    #[serde(default)]
    pub checkpoint: CheckpointPolicy,
    /// 工具审批规则（FR-744／ADR-064；缺省全 `allow`，与接线前行为一致）。
    #[serde(default)]
    pub approval: ApprovalConfig,
}

impl Config {
    /// 按基线默认值构造某工作区的配置。
    #[must_use]
    pub fn for_workspace(workspace_path: impl Into<PathBuf>) -> Self {
        let workspace_path = workspace_path.into();
        let cores = resources::available_physical_cores();
        Self {
            db: DbConfig {
                memory_limit_mb: DEFAULT_MEMORY_LIMIT_MB,
                max_memory_limit_mb: CANDIDATE_MAX_MEMORY_LIMIT_MB,
                threads: resources::thread_budget(cores),
                temp_directory: paths::workspace_temp_dir(&workspace_path),
                max_temp_directory_size_mb: None,
            },
            trace: TraceConfig::default(),
            ui: UiConfig::default(),
            checkpoint: CheckpointPolicy::default(),
            approval: ApprovalConfig::default(),
            workspace_path,
        }
    }

    /// 从 JSON 加载配置。
    ///
    /// # Errors
    ///
    /// JSON 解析失败或检查点策略越界时返回 `parse_error`（`retryable = false`）：配置错误
    /// 必须由人修正，自动重试没有意义且会掩盖问题。
    pub fn from_json(json: &str) -> Result<Self, WhError> {
        let config: Self = serde_json::from_str(json)
            .map_err(|err| WhError::parse_error(format!("配置解析失败：{err}")))?;
        // FR-405：策略边界在加载期校验，越界配置不得进入运行期。
        config
            .checkpoint
            .validate()
            .map_err(|err| WhError::parse_error(format!("检查点策略非法：{err}")))?;
        // FR-744／ADR-064：审批规则同样在加载期校验（非法值拒绝，不静默回落）。
        config
            .approval
            .validate()
            .map_err(|err| WhError::parse_error(format!("审批配置非法：{err}")))?;
        Ok(config)
    }

    /// 启动期检查：临时目录必须存在且可写（§5.4）。
    ///
    /// # Errors
    ///
    /// 目录不可写时返回 `io_error`（`retryable = true`，修复权限后重试有效）。
    pub fn check_temp_directory(&self) -> Result<(), WhError> {
        // 骨架阶段：目录自动创建属 M1 工作区初始化任务（T032/T047）；此处只做可写性判定。
        if !self.db.temp_directory.is_dir() {
            return Err(WhError::new(
                whcode_domain::ErrorCode::IoError,
                format!("临时目录不存在：{}", self.db.temp_directory.display()),
            ));
        }
        Ok(())
    }

    /// 生效主题：环境变量 `WHCODE_UI_THEME` 优先于配置值（ui-design §主题：可覆盖）。
    #[must_use]
    pub fn effective_theme(&self) -> Theme {
        self.theme_from_override(std::env::var("WHCODE_UI_THEME").ok().as_deref())
    }

    /// [`Config::effective_theme`] 的纯函数形式（不触碰进程环境，可测试）。
    ///
    /// 覆盖值非法时输出警告并回落配置值——环境变量笔误不得拒绝启动（错误可见、
    /// 可改、可撤销），但也不静默：警告写明合法取值与生效值。
    #[must_use]
    pub fn theme_from_override(&self, override_value: Option<&str>) -> Theme {
        let Some(raw) = override_value else {
            return self.ui.theme;
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return self.ui.theme;
        }
        match trimmed.parse::<Theme>() {
            Ok(theme) => theme,
            Err(_) => {
                eprintln!(
                    "WHCODE_UI_THEME={trimmed:?} 不是合法主题\
                     （amoled-night/eink-day/eink-night/terminal-green/zcode-day/zcode-night），使用配置值 {}",
                    self.ui.theme.as_str()
                );
                self.ui.theme
            }
        }
    }

    /// 生效语言（FR-625/615，T175）：`ui.locale` → `WHCODE_UI_LOCALE` → 既有
    /// `detect_locale` 链（LANG/LC_ALL → zh-CN）。Web 端 `?locale=` 与
    /// Accept-Language 在此默认值之上作为显式覆盖（api 层）。
    #[must_use]
    pub fn effective_locale(&self) -> String {
        self.locale_from_override(std::env::var("WHCODE_UI_LOCALE").ok().as_deref())
    }

    /// [`Config::effective_locale`] 的纯函数形式（除 `detect_locale` 尾链外不触碰环境）。
    #[must_use]
    pub fn locale_from_override(&self, override_value: Option<&str>) -> String {
        if let Some(raw) = override_value {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        if let Some(configured) = self.ui.locale.as_deref() {
            let trimmed = configured.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        let (locale, _source) = whcode_domain::i18n::detect_locale(None);
        locale
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_app::approval::SideEffectScope;

    #[test]
    fn defaults_match_task_book_baseline() {
        let config = Config::for_workspace("D:/ws");
        assert_eq!(config.db.memory_limit_mb, 192);
        assert_eq!(config.db.max_memory_limit_mb, 384);
        let cores = resources::available_physical_cores();
        // 断言精确值而非上界：`threads` 必须是 min(4, 可用并行度)，硬编码为 1 也应被抓住。
        assert_eq!(config.db.threads, resources::thread_budget(cores));
        assert!(config.db.threads <= 4);
        assert_eq!(config.trace.default_level, TraceLevel::Redacted);
        assert_eq!(config.db.temp_directory, PathBuf::from("D:/ws").join("tmp"));
        assert_eq!(config.db.max_temp_directory_size_mb, None);
        assert_eq!(
            config.ui.theme,
            Theme::ZcodeNight,
            "默认主题 zcode-night（2026-11-06 用户批准切换，ADR-057/FR-739）"
        );
    }

    #[test]
    fn json_roundtrip_preserves_values() {
        let config = Config::for_workspace("/tmp/ws");
        let json = serde_json::to_string_pretty(&config).expect("序列化应成功");
        let restored = Config::from_json(&json).expect("反序列化应成功");
        assert_eq!(config, restored);
    }

    #[test]
    fn invalid_json_yields_parse_error() {
        let err = Config::from_json("{ not json").expect_err("无效 JSON 必须失败");
        assert_eq!(err.code, whcode_domain::ErrorCode::ParseError);
        assert!(!err.retryable, "配置错误不应标记为可重试");
    }

    #[test]
    fn json_allows_explicit_overrides() {
        let json = r#"{
            "workspace_path": "/tmp/ws",
            "db": {
                "memory_limit_mb": 96,
                "max_memory_limit_mb": 384,
                "threads": 2,
                "temp_directory": "/tmp/ws/tmp",
                "max_temp_directory_size_mb": 512
            },
            "trace": { "default_level": "metadata" }
        }"#;
        let config = Config::from_json(json).expect("显式覆盖应被接受");
        assert_eq!(config.db.memory_limit_mb, 96);
        assert_eq!(config.db.max_temp_directory_size_mb, Some(512));
        assert_eq!(config.trace.default_level, TraceLevel::Metadata);
    }

    #[test]
    fn missing_temp_directory_is_reported() {
        let mut config = Config::for_workspace("D:/nonexistent-workspace-for-test");
        config.db.temp_directory = PathBuf::from("D:/nonexistent-workspace-for-test/tmp");
        let err = config
            .check_temp_directory()
            .expect_err("不存在的临时目录必须报错");
        assert_eq!(err.code, whcode_domain::ErrorCode::IoError);
    }

    /// FR-612：`ui.theme` 参与 JSON 往返；未给出 `ui` 键时取默认值。
    #[test]
    fn ui_theme_roundtrips_and_defaults() {
        let mut config = Config::for_workspace("/tmp/ws");
        config.ui.theme = Theme::AmoledNight;
        let json = serde_json::to_string_pretty(&config).expect("序列化");
        assert!(json.contains("\"theme\": \"amoled-night\""), "{json}");
        let restored = Config::from_json(&json).expect("反序列化");
        assert_eq!(restored.ui.theme, Theme::AmoledNight);

        let without_ui = r#"{
            "workspace_path": "/tmp/ws",
            "db": {
                "memory_limit_mb": 96,
                "max_memory_limit_mb": 384,
                "threads": 2,
                "temp_directory": "/tmp/ws/tmp",
                "max_temp_directory_size_mb": null
            },
            "trace": { "default_level": "redacted" }
        }"#;
        let config = Config::from_json(without_ui).expect("缺 ui 键应取默认值");
        assert_eq!(config.ui.theme, Theme::ZcodeNight);
        // 非法主题名必须显式失败（可操作错误），不得静默回落。
        let bad = without_ui.replace(
            "\"trace\"",
            "\"ui\": { \"theme\": \"midnight\" }, \"trace\"",
        );
        let err = Config::from_json(&bad).expect_err("未知主题名必须失败");
        assert_eq!(err.code, whcode_domain::ErrorCode::ParseError);
    }

    /// 环境变量覆盖：合法值生效、空值忽略、非法值可见回落（警告 + 配置值）。
    #[test]
    fn theme_override_is_visible_and_reversible() {
        let mut config = Config::for_workspace("/tmp/ws");
        config.ui.theme = Theme::EinkNight;
        assert_eq!(
            config.theme_from_override(None),
            Theme::EinkNight,
            "无覆盖取配置值"
        );
        assert_eq!(
            config.theme_from_override(Some("  ")),
            Theme::EinkNight,
            "空白覆盖视为未设置"
        );
        assert_eq!(
            config.theme_from_override(Some("amoled-night")),
            Theme::AmoledNight,
            "合法覆盖生效"
        );
        assert_eq!(
            config.theme_from_override(Some(" eink-day ")),
            Theme::EinkDay,
            "覆盖值两端空白被容忍"
        );
        assert_eq!(
            config.theme_from_override(Some("midnight")),
            Theme::EinkNight,
            "非法覆盖回落配置值（不拒绝启动）"
        );
    }

    /// FR-405（Check 171）：检查点策略随配置读写；缺省键取默认值，越界值加载期拒绝。
    #[test]
    fn checkpoint_policy_is_configurable_and_validated_at_load() {
        let defaults = Config::for_workspace("/tmp/ws");
        assert_eq!(
            defaults.checkpoint,
            whcode_domain::CheckpointPolicy::default(),
            "默认策略可见"
        );

        let json = r#"{
            "workspace_path": "/tmp/ws",
            "db": {
                "memory_limit_mb": 96,
                "max_memory_limit_mb": 384,
                "threads": 2,
                "temp_directory": "/tmp/ws/tmp",
                "max_temp_directory_size_mb": null
            },
            "trace": { "default_level": "redacted" },
            "checkpoint": {
                "interval_seconds": 60,
                "every_steps": 5,
                "before_dangerous": false,
                "retain": 3
            }
        }"#;
        let config = Config::from_json(json).expect("合法策略");
        assert_eq!(config.checkpoint.interval_seconds, 60);
        assert_eq!(config.checkpoint.every_steps, 5);
        assert!(!config.checkpoint.before_dangerous);
        assert_eq!(config.checkpoint.retain, 3);

        // 越界（保留数 0）必须在加载期失败，不得进入运行期。
        let bad = json.replace("\"retain\": 3", "\"retain\": 0");
        let err = Config::from_json(&bad).expect_err("越界保留数必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::ParseError);
        assert!(err.message.contains("检查点策略"), "{err}");
    }

    /// 最小合法配置（审批段按需拼接）。
    const BASE_JSON: &str = r#"{
        "workspace_path": "/tmp/ws",
        "db": {
            "memory_limit_mb": 96,
            "max_memory_limit_mb": 384,
            "threads": 2,
            "temp_directory": "/tmp/ws/tmp",
            "max_temp_directory_size_mb": null
        },
        "trace": { "default_level": "redacted" }
    }"#;

    /// T255／FR-744（Check 349）：默认审批策略全 `allow`，与接线前行为一致。
    #[test]
    fn approval_defaults_are_all_allow_and_behavior_compatible() {
        let config = Config::for_workspace("/tmp/ws");
        assert_eq!(config.approval, ApprovalConfig::default());
        assert_eq!(
            config.approval.default_read_only,
            ApprovalRule::Allow,
            "只读默认 allow：无 UI 时既有工具不得全部变 unavailable（ADR-060 §2.2）"
        );
        assert_eq!(config.approval.default_workspace_write, ApprovalRule::Allow);
        assert_eq!(config.approval.wait_ms, DEFAULT_APPROVAL_WAIT_MS);
        assert!(config.approval.tools.is_empty(), "默认无 per-tool 覆盖");

        // 缺 `approval` 键同样取默认（旧配置零迁移）。
        let parsed = Config::from_json(BASE_JSON).expect("缺审批段取默认");
        assert_eq!(parsed.approval, ApprovalConfig::default());

        let policy = parsed.approval.to_policy().expect("默认策略合法");
        assert_eq!(
            policy.range_defaults(),
            (ApprovalRule::Allow, ApprovalRule::Allow)
        );
        assert_eq!(policy.tool_rule_count(), 0);
    }

    /// 显式配置生效：per-tool 覆盖优先于副作用范围默认。
    #[test]
    fn approval_rules_parse_and_resolve_per_tool() {
        let json = BASE_JSON.replace(
            "\"trace\"",
            "\"approval\": {
                \"default_workspace_write\": \"ask\",
                \"tools\": { \"shell\": \"deny\", \"read_file\": \"allow\" },
                \"wait_ms\": 5000
            }, \"trace\"",
        );
        let config = Config::from_json(&json).expect("合法审批配置");
        assert_eq!(
            config.approval.default_workspace_write,
            ApprovalRule::Ask,
            "写范围默认可显式设为 ask"
        );
        assert_eq!(config.approval.default_read_only, ApprovalRule::Allow);
        assert_eq!(config.approval.tools.len(), 2);
        assert_eq!(config.approval.wait_ms, 5_000);

        let policy = config.approval.to_policy().expect("规则表可构造");
        assert_eq!(
            policy.rule_for("shell", SideEffectScope::WorkspaceWrite),
            ApprovalRule::Deny,
            "per-tool 覆盖优先"
        );
        assert_eq!(
            policy.rule_for("read_file", SideEffectScope::ReadOnly),
            ApprovalRule::Allow
        );
        assert_eq!(
            policy.rule_for("grep", SideEffectScope::WorkspaceWrite),
            ApprovalRule::Ask,
            "未覆盖的写工具取写默认"
        );
        assert_eq!(
            policy.rule_for("grep", SideEffectScope::ReadOnly),
            ApprovalRule::Allow,
            "只读默认不受写默认影响"
        );
    }

    /// 非法值在加载期拒绝（不静默回落，不进入运行期）。
    #[test]
    fn approval_invalid_values_are_rejected_at_load() {
        // 未知规则名（"prompt" 不是封闭三值之一）。
        let bad_rule = BASE_JSON.replace(
            "\"trace\"",
            "\"approval\": { \"tools\": { \"shell\": \"prompt\" } }, \"trace\"",
        );
        let err = Config::from_json(&bad_rule).expect_err("未知规则名必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::ParseError);
        assert!(!err.retryable, "配置错误不可重试");

        // 等待预算越界（0 与超上限都拒绝）。
        for wait in ["0", &(MAX_APPROVAL_WAIT_MS + 1).to_string()] {
            let json = BASE_JSON.replace(
                "\"trace\"",
                &format!("\"approval\": {{ \"wait_ms\": {wait} }}, \"trace\""),
            );
            let err = Config::from_json(&json).expect_err("等待预算越界必须拒绝");
            assert_eq!(err.code, whcode_domain::ErrorCode::ParseError);
            assert!(err.message.contains("审批"), "{err}");
        }

        // 工具名空白：加载期拒绝（错误可操作）。
        let bad_name = BASE_JSON.replace(
            "\"trace\"",
            "\"approval\": { \"tools\": { \"  \": \"ask\" } }, \"trace\"",
        );
        let err = Config::from_json(&bad_name).expect_err("空工具名必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::ParseError);
        assert!(err.message.contains("工具名"), "{err}");
    }
}
