//! # CUI clap 命令树（T084、ADR-019、contracts/cui-cli.md §1）
//!
//! clap 只做**参数解析**（ADR-019）：命令语义（权限、dry-run、审计）全部在
//! `whcode-app`（分层不变，ADR-003/005），本模块把解析结果映射为 app 命令参数。
//!
//! 命令树以 `contracts/cui-cli.md` §1 为接口草案：`session`/`run`/`status`/`task`/
//! `spec`/`events`/`db`/`runtime`/`serve`/`doctor`。M5 范围：**解析层**——语义执行
//! 的接线（M2—M4 的 app 命令）属二进制装配（`whcode-bin`），本模块保证命令树
//! 结构、参数命名与破坏性命令的 dry-run 旗标（§8.1）。
//!
//! 统一语义（契约 §2）：`db vacuum` 不存在（磁盘压缩命名 `db compact`）；`db explain`
//! 只接受只读查询；`spec force-advance` 必须要求 `--reason`（clap `required`）。

use clap::{Parser, Subcommand};

/// whcode 命令行入口（contracts/cui-cli.md §1 命令树）。
#[derive(Debug, Clone, PartialEq, Parser)]
#[command(name = "whcode", version, about = "旺皇大模型执行框架")]
pub struct Cli {
    /// 输出 JSON（schema_version 信封，FR-602）；可放在子命令前后任意位置。
    #[arg(long, global = true)]
    pub json: bool,

    /// 工作区目录（默认配置工作区/cwd；装配根用于定位 `<workspace>/whcode.duckdb`）。
    #[arg(long, global = true)]
    pub workspace: Option<String>,

    /// 子命令。
    #[command(subcommand)]
    pub command: Command,
}

/// 子命令树（§1 接口草案；语义执行属 app 层）。
#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum Command {
    /// 会话生命周期（FR-101）。
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// 发起模型调用（FR-201；桩 Provider）。
    Run {
        /// 会话 ID。
        #[arg(long)]
        session: String,
        /// 模型标识。
        #[arg(long, default_value = "stub")]
        model: String,
        /// 追踪级别（off/metadata/redacted/full）。
        #[arg(long, default_value = "redacted")]
        trace_level: String,
    },
    /// 状态快照（FR-314）。
    Status {
        /// 会话 ID。
        #[arg(long)]
        session: String,
    },
    /// 任务查询与控制（FR-401/408）。
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    /// 上下文预算视图（FR-105/108；contracts/cui-cli.md §1）。
    Context {
        #[command(subcommand)]
        command: ContextCommand,
    },
    /// SDD 阶段与门禁（FR-307—312）。
    Spec {
        #[command(subcommand)]
        command: SpecCommand,
    },
    /// 事件查询与导出（FR-202/205）。
    Events {
        #[command(subcommand)]
        command: EventsCommand,
    },
    /// 数据库维护与清理（FR-507—510）。
    Db {
        #[command(subcommand)]
        command: DbCommand,
    },
    /// 运行时管理（FR-701—710；M8）。
    Runtime {
        #[command(subcommand)]
        command: RuntimeCommand,
    },
    /// 内置字体安装与终端声明检测（FR-717；本机资源命令，不触库、无工作区）。
    Font {
        #[command(subcommand)]
        command: FontCommand,
    },
    /// 传输入口（M6/M7）。
    Serve {
        /// Web 入口（M6）。
        #[arg(long)]
        web: bool,
        /// SSH 入口（M7，延后）。
        #[arg(long)]
        ssh: bool,
        /// 绑定地址（默认仅回环，FR-606）。
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: String,
    },
    /// 环境与配置自检（M0）。
    Doctor {
        /// 输出 JSON。
        #[arg(long)]
        json: bool,
    },
}

/// 会话子命令（FR-101）。
#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum SessionCommand {
    /// 创建会话。
    New {
        /// 标题。
        #[arg(long)]
        title: String,
        /// 模型（可空）。
        #[arg(long)]
        model: Option<String>,
    },
    /// 列出会话。
    List {
        /// 含归档会话。
        #[arg(long)]
        all: bool,
    },
    /// 查看会话。
    Show {
        /// 会话 ID。
        #[arg(long)]
        session: String,
    },
    /// 重命名会话。
    Rename {
        /// 会话 ID。
        #[arg(long)]
        session: String,
        /// 新标题。
        #[arg(long)]
        title: String,
    },
    /// 归档/取消归档（契约 §2-1 破坏族：默认 dry-run，`--yes` 执行）。
    Archive {
        /// 会话 ID。
        #[arg(long)]
        session: String,
        /// 取消归档。
        #[arg(long)]
        unarchive: bool,
        /// 只预览不执行（默认）。
        #[arg(long)]
        dry_run: bool,
        /// 显式确认执行（与 --dry-run 互斥语义，§8.1）。
        #[arg(long)]
        yes: bool,
    },
    /// 删除会话（破坏性；默认 dry-run，FR-509）。
    Delete {
        /// 会话 ID。
        #[arg(long)]
        session: String,
        /// 只预览不执行（默认）。
        #[arg(long)]
        dry_run: bool,
        /// 显式确认执行（与 --dry-run 互斥语义，§8.1）。
        #[arg(long)]
        yes: bool,
    },
}

/// 任务子命令（FR-401/408）。
#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum TaskCommand {
    /// 任务列表。
    List {
        /// 会话 ID。
        #[arg(long)]
        session: String,
    },
    /// 任务详情（FR-104 五类信息）。
    Show {
        /// 任务 ID。
        #[arg(long)]
        task: String,
    },
    /// 重试任务（FR-401）。
    Retry {
        /// 任务 ID。
        #[arg(long)]
        task: String,
    },
    /// 中止任务（FR-408）。
    Abort {
        /// 任务 ID。
        #[arg(long)]
        task: String,
    },
}

/// 上下文子命令（FR-105/108）。
#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum ContextCommand {
    /// 预算视图：当前预算、装入来源、截断原因与游标（经 `whcode_app::load_context`）。
    Budget {
        /// 会话 ID。
        #[arg(long)]
        session: String,
        /// 预算 token 数；缺省用展示默认口径（4096，与 TUI 上下文视图一致）。
        #[arg(long)]
        budget: Option<usize>,
    },
}

/// SDD 子命令（FR-307—312）。
#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum SpecCommand {
    /// 当前阶段状态（FR-314）。
    Status {
        /// 会话 ID。
        #[arg(long)]
        session: String,
    },
    /// 下一阶段。
    Next {
        /// 会话 ID。
        #[arg(long)]
        session: String,
    },
    /// 校验产物。
    Validate {
        /// 会话 ID。
        #[arg(long)]
        session: String,
    },
    /// Analyze 检查（FR-306）。
    Analyze {
        /// 会话 ID。
        #[arg(long)]
        session: String,
    },
    /// Converge 核对（FR-310/311）。
    Converge {
        /// 会话 ID。
        #[arg(long)]
        session: String,
    },
    /// 人工越过门禁（FR-312；reason 必填，契约 §2-3）。
    ForceAdvance {
        /// 会话 ID。
        #[arg(long)]
        session: String,
        /// 目标阶段。
        #[arg(long)]
        to: String,
        /// 强制放行理由（必填，FR-312）。
        #[arg(long)]
        reason: String,
    },
}

/// 事件子命令（FR-202/205）。
#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum EventsCommand {
    /// 事件列表（按游标增量）。
    List {
        /// 会话 ID。
        #[arg(long)]
        session: String,
        /// 起始游标（排他）。
        #[arg(long, default_value = "0")]
        after: i64,
    },
    /// 单事件详情。
    Show {
        /// 事件 ID。
        #[arg(long)]
        event: String,
    },
    /// 导出（脱敏策略随导出注明，FR-205）。
    Export {
        /// 会话 ID。
        #[arg(long)]
        session: String,
        /// 起始游标。
        #[arg(long, default_value = "0")]
        after: i64,
        /// 输出格式（json/jsonl）。
        #[arg(long, default_value = "json")]
        format: String,
    },
}

/// 数据库子命令（FR-507—510）。
#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum DbCommand {
    /// 数据库统计（FR-507）。
    Stats,
    /// 必需索引检查。
    Check,
    /// 按时间清理（FR-508/509；默认 dry-run）。
    Purge {
        /// 截止时间（Unix 毫秒）。
        #[arg(long)]
        before: i64,
        /// 只预览不执行（默认）。
        #[arg(long)]
        dry_run: bool,
        /// 显式确认执行。
        #[arg(long)]
        yes: bool,
    },
    /// 显式 checkpoint（FR-510；可顺带应用 WAL 策略覆盖，disaster-recovery §5）。
    Checkpoint {
        /// 覆盖 WAL 自动 checkpoint 阈值（如 16MB；缺省保持现状）。
        #[arg(long)]
        threshold: Option<String>,
        /// 覆盖 wal_autocheckpoint（如 1MB；缺省保持现状）。
        #[arg(long)]
        autocheckpoint: Option<String>,
    },
    /// 从导出文件恢复到空工作区（`db archive` 的逆操作；显式确认，无预览）。
    Restore {
        /// 导出文件路径（whcode-export 格式）。
        #[arg(long)]
        from: String,
        /// 显式确认执行（恢复是写操作；目标库必须为空）。
        #[arg(long)]
        yes: bool,
    },
    /// 磁盘压缩（复制—校验—切换，FR-510；默认 dry-run）。
    Compact {
        /// 只预览不执行（默认）。
        #[arg(long)]
        dry_run: bool,
        /// 显式确认执行。
        #[arg(long)]
        yes: bool,
    },
    /// 导出工作区。
    Archive {
        /// 输出路径。
        #[arg(long)]
        export: String,
    },
    /// 查询计划分析（只读查询，契约 §2-2）。
    Explain {
        /// 只读 SQL（SELECT/WITH 开头）。
        #[arg(long)]
        query: String,
    },
}

/// 运行时子命令（FR-701—710；语义属 M8）。
#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum RuntimeCommand {
    /// 运行时清单。
    List,
    /// 运行时状态。
    Status {
        /// 运行时名。
        #[arg(long)]
        name: String,
    },
    /// 启用运行时。
    Enable {
        /// 运行时名。
        #[arg(long)]
        name: String,
    },
    /// 禁用运行时。
    Disable {
        /// 运行时名。
        #[arg(long)]
        name: String,
    },
    /// 安装 payload。
    Install {
        /// 运行时名。
        #[arg(long)]
        name: String,
    },
}

/// `font` 子命令（FR-717、ADR-028）：用户级字体安装显式且可逆，`uninstall` 撤销；
/// 程序不改写用户终端配置，`install` 只输出配置片段供确认。
#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum FontCommand {
    /// 安装四份内置字体到用户字体目录并写 HKCU 注册（复制为覆盖语义，可重跑）。
    Install {
        /// 字体来源目录（平铺或仓库 source/<family>/ 布局）；缺省按序探测
        /// 可执行文件旁与当前目录的 `assets/fonts/source`（ADR-028 修订 1）。
        #[arg(long)]
        from: Option<String>,
        /// 只报告将执行的动作，不改系统。
        #[arg(long)]
        dry_run: bool,
    },
    /// 检测字体安装状态与终端声明（只读）。
    Status,
    /// 卸载内置字体：注销注册并删除用户字体目录中的文件（尽力而为，逐项报告）。
    Uninstall {
        /// 只报告将执行的动作，不改系统。
        #[arg(long)]
        dry_run: bool,
    },
}

/// 解析命令行参数（T084）。
///
/// # Errors
///
/// 参数非法（clap 校验失败）时返回 clap 错误的字符串形态（统一错误信封由
/// `whcode-bin` 包装，T085）。
pub fn parse<I, T>(args: I) -> Result<Cli, String>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    Cli::try_parse_from(args).map_err(|err| err.to_string())
}

/// 解析结果（装配根用：区分帮助输出与用法错误，退出码语义见 contracts/cui-cli.md §3）。
#[derive(Debug, Clone, PartialEq)]
pub enum ParseOutcome {
    /// 解析成功。
    Command(Cli),
    /// `--help` / `--version`：原样打印后以成功退出。
    Display(String),
    /// 用法错误：打印后以 `EXIT_USAGE`（2）退出。
    Usage(String),
}

/// 解析命令行（保留 clap 的错误类别：帮助/版本 vs 用法错误）。
#[must_use]
pub fn parse_outcome<I, T>(args: I) -> ParseOutcome
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    use clap::error::ErrorKind;
    match Cli::try_parse_from(args) {
        Ok(cli) => ParseOutcome::Command(cli),
        Err(err) => match err.kind() {
            ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                ParseOutcome::Display(err.to_string())
            }
            _ => ParseOutcome::Usage(err.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tree_matches_contract_shape() {
        // contracts/cui-cli.md §1 命令树：解析成功即结构合法。
        let cli = parse(["whcode", "session", "new", "--title", "测试"]).expect("session new");
        assert!(matches!(
            cli.command,
            Command::Session {
                command: SessionCommand::New { .. }
            }
        ));

        let cli = parse(["whcode", "task", "retry", "--task", "T-1"]).expect("task retry");
        assert!(matches!(
            cli.command,
            Command::Task {
                command: TaskCommand::Retry { .. }
            }
        ));

        let cli = parse([
            "whcode",
            "events",
            "export",
            "--session",
            "S-1",
            "--format",
            "jsonl",
        ])
        .expect("events export");
        assert!(matches!(
            cli.command,
            Command::Events {
                command: EventsCommand::Export { .. }
            }
        ));

        let cli = parse(["whcode", "db", "purge", "--before", "1000"]).expect("db purge");
        assert!(matches!(
            cli.command,
            Command::Db {
                command: DbCommand::Purge { .. }
            }
        ));

        let cli = parse(["whcode", "doctor", "--json"]).expect("doctor");
        assert!(matches!(cli.command, Command::Doctor { json: true }));

        // context budget（FR-105/108）：budget 可选，缺省用展示默认口径。
        let cli = parse(["whcode", "context", "budget", "--session", "S-1"]).expect("budget");
        assert!(matches!(
            cli.command,
            Command::Context {
                command: ContextCommand::Budget { budget: None, .. }
            }
        ));
        let cli = parse([
            "whcode",
            "context",
            "budget",
            "--session",
            "S-1",
            "--budget",
            "2048",
        ])
        .expect("budget 显式值");
        assert!(matches!(
            cli.command,
            Command::Context {
                command: ContextCommand::Budget {
                    budget: Some(2048),
                    ..
                }
            }
        ));
    }

    #[test]
    fn force_advance_requires_reason_per_contract() {
        // 契约 §2-3：force-advance 必须要求 --reason（缺 reason 解析失败）。
        let result = parse([
            "whcode",
            "spec",
            "force-advance",
            "--session",
            "S-1",
            "--to",
            "plan",
        ]);
        assert!(result.is_err(), "缺 --reason 必须解析失败（FR-312）");
        let cli = parse([
            "whcode",
            "spec",
            "force-advance",
            "--session",
            "S-1",
            "--to",
            "plan",
            "--reason",
            "阻塞解除",
        ])
        .expect("带 reason 合法");
        assert!(matches!(
            cli.command,
            Command::Spec {
                command: SpecCommand::ForceAdvance { .. }
            }
        ));
    }

    #[test]
    fn destructive_commands_carry_dry_run_flags() {
        // §8.1：破坏性命令支持 dry-run/确认（无隐式执行）。
        let cli = parse(["whcode", "session", "delete", "--session", "S-1"]).expect("默认");
        let Command::Session {
            command: SessionCommand::Delete { dry_run, yes, .. },
        } = cli.command
        else {
            panic!("应为 delete");
        };
        assert!(!dry_run && !yes, "缺省为 dry-run 语义（无隐式执行）");

        let cli =
            parse(["whcode", "session", "delete", "--session", "S-1", "--yes"]).expect("确认");
        let Command::Session {
            command: SessionCommand::Delete { yes, .. },
        } = cli.command
        else {
            panic!("应为 delete");
        };
        assert!(yes);
    }

    #[test]
    fn unknown_command_and_missing_args_are_rejected() {
        assert!(parse(["whcode", "bogus"]).is_err(), "未知子命令必须拒绝");
        assert!(
            parse(["whcode", "session", "new"]).is_err(),
            "缺参数必须拒绝"
        );
        assert!(parse(["whcode"]).is_err(), "缺子命令必须拒绝");
    }
}
