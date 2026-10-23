//! # 内置字体的本机安装、检测与卸载（FR-717、ADR-028 修订 1）
//!
//! “本机资源命令”：不触库、不涉会话授权——安装只影响当前用户的字体目录与 HKCU
//! 注册表，显式命令名即意图，`--dry-run` 可预览（contracts/cui-cli.md §1）。不新增
//! Capability 的理由：动作不触及会话/任务数据，且必须能在无工作区时执行。
//!
//! 分层：本模块只做编排与纯数据（来源解析、报告构造、终端配置解析）；注册表副作用经
//! [`FontRegistry`] 抽象注入，装配根（`whcode-bin`）接 `whcode-platform` 的 reg.exe
//! 实现（同 [`crate::view_source`] 的注入模式）——应用层零平台分支（NFR-101）。
//!
//! 字体来源（ADR-028 修订 1）：`--from` 显式目录 > 可执行文件旁 `assets/fonts/source`
//! > 当前目录 `assets/fonts/source`；发布包不带完整字体，缺失时报可操作错误。

use std::path::{Path, PathBuf};

use serde::Serialize;
use whcode_domain::{ErrorCode, WhError};

/// 内置字体清单条目（族名与 GUI `font_for` 的四分支一致，FR-716）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinFont {
    /// 字体族名（GUI 加载与终端配置使用）。
    pub family: &'static str,
    /// 完整字体文件名（来源目录与用户字体目录同名存放）。
    pub file_name: &'static str,
    /// HKCU 注册值名（Windows 惯例：显示名 + (TrueType)/(OpenType)）。
    pub reg_value: &'static str,
    /// 仓库布局子目录（`assets/fonts/source/<slug>/<file>`）。
    pub slug: &'static str,
}

/// 四份内置字体（ADR-028；均 SIL OFL 1.1，可再分发）。
pub const BUILTIN_FONTS: [BuiltinFont; 4] = [
    BuiltinFont {
        family: "Inter",
        file_name: "Inter-Regular.ttf",
        reg_value: "Inter Regular (TrueType)",
        slug: "inter",
    },
    BuiltinFont {
        family: "Noto Sans CJK SC",
        file_name: "NotoSansCJKsc-Regular.otf",
        reg_value: "Noto Sans CJK SC Regular (OpenType)",
        slug: "noto-sans-sc",
    },
    BuiltinFont {
        family: "JetBrains Mono",
        file_name: "JetBrainsMono-Regular.ttf",
        reg_value: "JetBrains Mono Regular (TrueType)",
        slug: "jetbrains-mono",
    },
    BuiltinFont {
        family: "Sarasa Mono SC",
        file_name: "SarasaMonoSC-Regular.ttf",
        reg_value: "Sarasa Mono SC Regular (TrueType)",
        slug: "sarasa-mono-sc",
    },
];

/// 注册表副作用抽象（HKCU Fonts 键；装配根接 reg.exe 实现，测试接内存表）。
pub trait FontRegistry {
    /// 查询值是否存在（存在时返回注册的数据字符串）。
    fn query_value(&self, value_name: &str) -> Result<Option<String>, WhError>;
    /// 写入 REG_SZ 值（指向字体文件的完整路径）。
    fn set_value(&self, value_name: &str, data: &str) -> Result<(), WhError>;
    /// 删除值；返回是否实际删除（本就不存在返回 `false`）。
    fn delete_value(&self, value_name: &str) -> Result<bool, WhError>;
}

/// 运行环境注入（装配根构造；测试注入内存表与临时目录）。
pub struct FontEnv<'a> {
    /// 注册表操作。
    pub registry: &'a dyn FontRegistry,
    /// 用户级字体目录（Windows：`%LOCALAPPDATA%\Microsoft\Windows\Fonts`）。
    pub user_fonts_dir: PathBuf,
    /// Windows Terminal settings.json 候选路径（按序探测；找不到不视为错误）。
    pub terminal_settings: Vec<PathBuf>,
}

/// 单字体安装状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FontStatus {
    /// 字体族名。
    pub family: String,
    /// 完整字体文件名。
    pub file_name: String,
    /// 用户字体目录中存在文件。
    pub file_installed: bool,
    /// HKCU 注册值存在。
    pub registered: bool,
    /// 文件与注册齐备（可被终端选用）。
    pub complete: bool,
}

/// 终端声明检测结果（只读；找不到配置不视为错误，FR-717）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TerminalStatus {
    /// 命中的 settings.json 路径；未检测到为 `None`。
    pub settings_file: Option<String>,
    /// 终端声明的 font.face 集合（去重保序）。
    pub declared_faces: Vec<String>,
    /// 其中属于内置四字体的族名。
    pub builtin_in_use: Vec<String>,
    /// 读取/解析失败说明；正常为 `None`。
    pub note: Option<String>,
}

/// `font status` 报告。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatusReport {
    /// 用户级字体目录。
    pub fonts_dir: String,
    /// 四份字体的安装状态（顺序与 [`BUILTIN_FONTS`] 一致）。
    pub fonts: Vec<FontStatus>,
    /// 终端声明检测。
    pub terminal: TerminalStatus,
}

/// 单字体安装结果（`dry_run` 时为计划动作）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledFont {
    /// 字体族名。
    pub family: String,
    /// 来源文件路径。
    pub source: String,
    /// 目标文件路径（用户字体目录）。
    pub target: String,
    /// 是否已（或将）写入注册。
    pub registered: bool,
}

/// `font install` 报告。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InstallReport {
    /// 是否为 dry-run 预览。
    pub dry_run: bool,
    /// 命中的来源目录。
    pub source_dir: String,
    /// 用户级字体目录。
    pub fonts_dir: String,
    /// 逐字体结果（顺序与 [`BUILTIN_FONTS`] 一致）。
    pub installed: Vec<InstalledFont>,
    /// 终端配置说明与片段（用户自行确认粘贴；程序不改写用户配置，FR-717）。
    pub terminal_hint: Vec<String>,
}

/// 单字体卸载结果（`dry_run` 时为将执行的动作与当前状态）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemovedFont {
    /// 字体族名。
    pub family: String,
    /// 是否已（或将）注销注册。
    pub unregistered: bool,
    /// 是否已（或将）删除文件。
    pub file_deleted: bool,
    /// 文件删除失败原因（如被终端占用）；成功或无需删除为 `None`。
    pub file_error: Option<String>,
}

/// `font uninstall` 报告。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UninstallReport {
    /// 是否为 dry-run 预览。
    pub dry_run: bool,
    /// 逐字体结果（顺序与 [`BUILTIN_FONTS`] 一致）。
    pub removed: Vec<RemovedFont>,
}

/// 终端配置建议片段（等宽字体建议 Sarasa Mono SC；粘入 profiles.defaults）。
pub const TERMINAL_FONT_SNIPPET: &str = r#""font": { "face": "Sarasa Mono SC" }"#;

/// 来源候选目录（ADR-028 修订 1）：`--from` 优先，其后可执行文件旁与当前目录的
/// 仓库布局 `assets/fonts/source`。
#[must_use]
pub fn source_candidates(from: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = from {
        dirs.push(dir.to_path_buf());
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        dirs.push(parent.join("assets").join("fonts").join("source"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("assets").join("fonts").join("source"));
    }
    dirs
}

/// 在目录内定位一份字体，支持三种布局：平铺 `<dir>/<file>`、家族目录
/// `<dir>/<slug>/<file>`、仓库布局 `<dir>/source/<slug>/<file>`。
fn locate_in_dir(dir: &Path, font: &BuiltinFont) -> Option<PathBuf> {
    [
        dir.join(font.file_name),
        dir.join(font.slug).join(font.file_name),
        dir.join("source").join(font.slug).join(font.file_name),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

/// 解析四份字体的来源路径（顺序与 [`BUILTIN_FONTS`] 一致）。
///
/// 逐候选目录尝试；第一个能凑齐四份的目录胜出。
///
/// # Errors
///
/// 候选目录都不含完整四份字体时返回 [`ErrorCode::NotFound`]，消息列出探测目录、
/// `--from` 用法与 `SOURCES.md` 指引（可操作，不静默失败）。
pub fn resolve_sources(candidates: &[PathBuf]) -> Result<(PathBuf, Vec<PathBuf>), WhError> {
    for dir in candidates {
        let found: Vec<PathBuf> = BUILTIN_FONTS
            .iter()
            .filter_map(|font| locate_in_dir(dir, font))
            .collect();
        if found.len() == BUILTIN_FONTS.len() {
            return Ok((dir.clone(), found));
        }
    }
    Err(WhError::new(
        ErrorCode::NotFound,
        format!(
            "未找到四份内置字体完整文件；已探测目录：{}。用 --from <目录> 显式指定来源\
             （支持平铺或仓库 source/<family>/ 布局），或按 assets/fonts/SOURCES.md \
             记录的上游地址下载",
            candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("；"),
        ),
    ))
}

/// 检测四份字体的安装状态与终端声明（只读，无副作用）。
#[must_use]
pub fn status(env: &FontEnv) -> StatusReport {
    let fonts = BUILTIN_FONTS
        .iter()
        .map(|font| {
            let file_installed = env.user_fonts_dir.join(font.file_name).is_file();
            let registered = env
                .registry
                .query_value(font.reg_value)
                .ok()
                .flatten()
                .is_some();
            FontStatus {
                family: font.family.to_string(),
                file_name: font.file_name.to_string(),
                file_installed,
                registered,
                complete: file_installed && registered,
            }
        })
        .collect();
    StatusReport {
        fonts_dir: env.user_fonts_dir.display().to_string(),
        fonts,
        terminal: terminal_status(env),
    }
}

/// 终端声明检测：按序探测候选 settings.json，命中即停（Store 版优先于预览版）。
fn terminal_status(env: &FontEnv) -> TerminalStatus {
    for path in &env.terminal_settings {
        if !path.is_file() {
            continue;
        }
        let display = path.display().to_string();
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => {
                return TerminalStatus {
                    settings_file: Some(display),
                    declared_faces: Vec::new(),
                    builtin_in_use: Vec::new(),
                    note: Some(format!("读取失败：{err}")),
                };
            }
        };
        return match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) => {
                let mut declared: Vec<String> = Vec::new();
                collect_faces(&value, &mut declared);
                let builtin_in_use = BUILTIN_FONTS
                    .iter()
                    .map(|font| font.family.to_string())
                    .filter(|family| declared.contains(family))
                    .collect();
                TerminalStatus {
                    settings_file: Some(display),
                    declared_faces: declared,
                    builtin_in_use,
                    note: None,
                }
            }
            Err(err) => TerminalStatus {
                settings_file: Some(display),
                declared_faces: Vec::new(),
                builtin_in_use: Vec::new(),
                note: Some(format!("JSON 解析失败：{err}")),
            },
        };
    }
    TerminalStatus {
        settings_file: None,
        declared_faces: Vec::new(),
        builtin_in_use: Vec::new(),
        note: None,
    }
}

/// 收集 JSON 树里所有 `"font"` 对象（或对象数组）声明的 `face` 字符串，兼容
/// `profiles.defaults.font`、`profiles.list[].font` 与旧版 `profiles[]` 数组形状。
fn collect_faces(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(font) = map.get("font") {
                extract_faces(font, out);
            }
            for child in map.values() {
                collect_faces(child, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_faces(item, out);
            }
        }
        _ => {}
    }
}

/// 从 `"font"` 的值里取 `face`（对象）或逐项取（数组）。
fn extract_faces(font: &serde_json::Value, out: &mut Vec<String>) {
    match font {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(face)) = map.get("face")
                && !out.contains(face)
            {
                out.push(face.clone());
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                extract_faces(item, out);
            }
        }
        _ => {}
    }
}

/// 安装四份内置字体到用户字体目录并写 HKCU 注册（显式、可逆；`uninstall` 撤销）。
///
/// 复制为覆盖语义（重跑幂等）；注册表写入失败立即返回错误，已复制的文件不影响
/// 一致性——重跑 `install` 或 `uninstall` 均可收敛。
///
/// # Errors
///
/// 来源缺失（[`ErrorCode::NotFound`]）；目录创建、文件复制或注册表写入失败
/// （[`ErrorCode::IoError`]）。
pub fn install(
    env: &FontEnv,
    from: Option<&Path>,
    dry_run: bool,
) -> Result<InstallReport, WhError> {
    let candidates = source_candidates(from);
    let (source_dir, sources) = resolve_sources(&candidates)?;
    let installed: Vec<InstalledFont> = BUILTIN_FONTS
        .iter()
        .zip(&sources)
        .map(|(font, source)| InstalledFont {
            family: font.family.to_string(),
            source: source.display().to_string(),
            target: env
                .user_fonts_dir
                .join(font.file_name)
                .display()
                .to_string(),
            registered: false,
        })
        .collect();
    let report = |installed: Vec<InstalledFont>| InstallReport {
        dry_run,
        source_dir: source_dir.display().to_string(),
        fonts_dir: env.user_fonts_dir.display().to_string(),
        installed,
        terminal_hint: hint_lines(),
    };
    if dry_run {
        return Ok(report(installed));
    }
    std::fs::create_dir_all(&env.user_fonts_dir).map_err(|err| {
        WhError::new(
            ErrorCode::IoError,
            format!("创建字体目录 {} 失败：{err}", env.user_fonts_dir.display()),
        )
    })?;
    let mut done = Vec::with_capacity(installed.len());
    for ((font, source), planned) in BUILTIN_FONTS.iter().zip(&sources).zip(&installed) {
        let target = env.user_fonts_dir.join(font.file_name);
        std::fs::copy(source, &target).map_err(|err| {
            WhError::new(
                ErrorCode::IoError,
                format!(
                    "复制 {} → {} 失败：{err}",
                    source.display(),
                    target.display()
                ),
            )
        })?;
        env.registry
            .set_value(font.reg_value, &target.display().to_string())?;
        done.push(InstalledFont {
            registered: true,
            ..planned.clone()
        });
    }
    Ok(report(done))
}

/// 终端配置说明（固定两条：片段 + 重启提示；不自动改写用户配置，FR-717）。
#[must_use]
pub fn hint_lines() -> Vec<String> {
    vec![
        format!(
            "终端配置片段（粘入 Windows Terminal settings.json 的 profiles.defaults，\
             自行确认；程序不代改用户配置）：{TERMINAL_FONT_SNIPPET}"
        ),
        "终端字形由终端模拟器控制，程序不能强制更换（FR-717）；若未立即生效，请重启终端应用。"
            .to_string(),
    ]
}

/// 卸载内置字体：注销 HKCU 注册并删除用户字体目录中的文件（尽力而为，逐项报告）。
///
/// # Errors
///
/// 注册表操作失败（[`ErrorCode::IoError`]）；文件删除失败不中断其余字体，
/// 原因记录在 [`RemovedFont::file_error`]（常见为字体正被占用，重启应用后重试）。
pub fn uninstall(env: &FontEnv, dry_run: bool) -> Result<UninstallReport, WhError> {
    let mut removed = Vec::new();
    for font in BUILTIN_FONTS {
        let target = env.user_fonts_dir.join(font.file_name);
        // 用 symlink_metadata 判定“路径被占”：同名目录等异常占用也尝试清理并如实
        // 报告，而不是静默跳过（删除失败是可操作信息，非致命）。
        let target_present = target.symlink_metadata().is_ok();
        let registered = env.registry.query_value(font.reg_value)?.is_some();
        if dry_run {
            removed.push(RemovedFont {
                family: font.family.to_string(),
                unregistered: registered,
                file_deleted: target_present,
                file_error: None,
            });
            continue;
        }
        let unregistered = env.registry.delete_value(font.reg_value)?;
        let mut file_deleted = false;
        let mut file_error = None;
        if target_present {
            match std::fs::remove_file(&target) {
                Ok(()) => file_deleted = true,
                Err(err) => {
                    file_error = Some(format!(
                        "删除 {} 失败：{err}（字体可能正被占用，重启应用后重试）",
                        target.display()
                    ));
                }
            }
        }
        removed.push(RemovedFont {
            family: font.family.to_string(),
            unregistered,
            file_deleted,
            file_error,
        });
    }
    Ok(UninstallReport { dry_run, removed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemRegistry(Mutex<BTreeMap<String, String>>);

    impl FontRegistry for MemRegistry {
        fn query_value(&self, value_name: &str) -> Result<Option<String>, WhError> {
            Ok(self.0.lock().expect("注册表锁").get(value_name).cloned())
        }
        fn set_value(&self, value_name: &str, data: &str) -> Result<(), WhError> {
            self.0
                .lock()
                .expect("注册表锁")
                .insert(value_name.to_string(), data.to_string());
            Ok(())
        }
        fn delete_value(&self, value_name: &str) -> Result<bool, WhError> {
            Ok(self
                .0
                .lock()
                .expect("注册表锁")
                .remove(value_name)
                .is_some())
        }
    }

    fn temp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("whcode-fonts-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 造一个含四份假字体的来源目录（布局由 `layout` 决定：flat=平铺、repo=仓库）。
    fn fixture_source(tag: &str, layout: &str) -> PathBuf {
        let root = temp_root(tag);
        for font in BUILTIN_FONTS {
            let path = match layout {
                "flat" => root.join(font.file_name),
                _ => root.join("source").join(font.slug).join(font.file_name),
            };
            std::fs::create_dir_all(path.parent().expect("有父目录")).expect("建目录");
            std::fs::write(&path, b"fake font bytes").expect("写字体文件");
        }
        root
    }

    fn env_for<'a>(registry: &'a MemRegistry, root: &Path) -> FontEnv<'a> {
        FontEnv {
            registry,
            user_fonts_dir: root.join("fonts"),
            terminal_settings: Vec::new(),
        }
    }

    #[test]
    fn builtin_families_match_gui_font_for() {
        // 与 whcode-gui font_for 的四分支族名逐字一致（FR-716）；GUI 是前端，
        // 应用层不引其依赖，这里以字面量锁定契约。
        let families: Vec<&str> = BUILTIN_FONTS.iter().map(|font| font.family).collect();
        assert_eq!(
            families,
            [
                "Inter",
                "Noto Sans CJK SC",
                "JetBrains Mono",
                "Sarasa Mono SC"
            ]
        );
        // 注册值名覆盖四份且各不相同（HKCU 键内唯一）。
        let mut values: Vec<&str> = BUILTIN_FONTS.iter().map(|font| font.reg_value).collect();
        values.sort_unstable();
        values.dedup();
        assert_eq!(values.len(), 4);
    }

    #[test]
    fn resolve_sources_supports_flat_and_repo_layouts() {
        let flat = fixture_source("resolve-flat", "flat");
        let repo = fixture_source("resolve-repo", "repo");
        let (_, flat_paths) = resolve_sources(std::slice::from_ref(&flat)).expect("平铺布局");
        assert_eq!(flat_paths.len(), 4);
        assert!(flat_paths[0].starts_with(&flat));
        let (_, repo_paths) = resolve_sources(std::slice::from_ref(&repo)).expect("仓库布局");
        assert!(repo_paths[0].starts_with(repo.join("source").join("inter")));
        let _ = std::fs::remove_dir_all(&flat);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn resolve_sources_missing_is_actionable() {
        let empty = temp_root("resolve-empty");
        let err = resolve_sources(std::slice::from_ref(&empty)).expect_err("必须失败");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(
            err.message.contains("--from"),
            "指引 --from：{}",
            err.message
        );
        assert!(
            err.message.contains("SOURCES.md"),
            "指引上游记录：{}",
            err.message
        );
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn install_dry_run_changes_nothing() {
        let source = fixture_source("install-dry", "flat");
        let root = temp_root("install-dry-root");
        let registry = MemRegistry::default();
        let env = env_for(&registry, &root);
        let report = install(&env, Some(&source), true).expect("dry-run");
        assert!(report.dry_run);
        assert_eq!(report.installed.len(), 4);
        assert!(!report.installed[0].registered, "dry-run 不写注册");
        assert!(!env.user_fonts_dir.exists(), "dry-run 不建目录");
        assert!(registry.0.lock().expect("锁").is_empty());
        assert_eq!(report.terminal_hint.len(), 2);
        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_then_status_reports_complete() {
        let source = fixture_source("install-full", "repo");
        let root = temp_root("install-full-root");
        let registry = MemRegistry::default();
        let env = env_for(&registry, &root);
        let report = install(&env, Some(&source), false).expect("安装");
        assert!(!report.dry_run);
        assert!(report.installed.iter().all(|item| item.registered));
        let status = status(&env);
        assert!(status.fonts.iter().all(|font| font.complete));
        assert_eq!(status.terminal.settings_file, None, "无终端配置不视为错误");
        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn uninstall_removes_install() {
        let source = fixture_source("uninstall", "flat");
        let root = temp_root("uninstall-root");
        let registry = MemRegistry::default();
        let env = env_for(&registry, &root);
        install(&env, Some(&source), false).expect("先安装");
        let report = uninstall(&env, false).expect("卸载");
        assert!(report.removed.iter().all(|item| item.unregistered));
        assert!(report.removed.iter().all(|item| item.file_deleted));
        assert!(status(&env).fonts.iter().all(|font| !font.complete));
        let _ = std::fs::remove_dir_all(&source);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn uninstall_without_install_is_noop() {
        let root = temp_root("uninstall-noop");
        let registry = MemRegistry::default();
        let env = env_for(&registry, &root);
        let report = uninstall(&env, false).expect("未安装时卸载为空操作");
        assert!(report.removed.iter().all(|item| !item.unregistered));
        assert!(report.removed.iter().all(|item| !item.file_deleted));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn uninstall_reports_file_error_without_stopping() {
        // 用同名目录占住目标文件路径，模拟文件被占用导致的删除失败（跨平台稳定）。
        let root = temp_root("uninstall-busy");
        let registry = MemRegistry::default();
        let env = env_for(&registry, &root);
        let blocked = env.user_fonts_dir.join(BUILTIN_FONTS[0].file_name);
        std::fs::create_dir_all(&blocked).expect("造同名目录");
        let report = uninstall(&env, false).expect("删除失败不中断");
        assert!(report.removed[0].file_error.is_some());
        assert!(!report.removed[0].file_deleted);
        assert!(report.removed[1].file_deleted || !report.removed[1].unregistered);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn terminal_faces_parsed_from_both_settings_shapes() {
        let root = temp_root("terminal-shapes");
        let defaults_shape = root.join("defaults.json");
        std::fs::write(
            &defaults_shape,
            r#"{"profiles": {"defaults": {"font": {"face": "Sarasa Mono SC"}}, "list": [{"name": "pwsh", "font": {"face": "Cascadia Code"}}]}}"#,
        )
        .expect("写 settings");
        let legacy_shape = root.join("legacy.json");
        std::fs::write(
            &legacy_shape,
            r#"{"profiles": [{"name": "cmd", "font": {"face": "JetBrains Mono"}}]}"#,
        )
        .expect("写 settings");
        for (path, expect_declared, expect_builtin) in [
            (
                &defaults_shape,
                vec!["Sarasa Mono SC", "Cascadia Code"],
                vec!["Sarasa Mono SC"],
            ),
            (
                &legacy_shape,
                vec!["JetBrains Mono"],
                vec!["JetBrains Mono"],
            ),
        ] {
            let registry = MemRegistry::default();
            let env = FontEnv {
                registry: &registry,
                user_fonts_dir: root.join("fonts"),
                terminal_settings: vec![path.clone()],
            };
            let report = status(&env);
            assert_eq!(
                report.terminal.settings_file.as_deref(),
                Some(path.to_string_lossy().as_ref())
            );
            assert_eq!(report.terminal.declared_faces, expect_declared);
            assert_eq!(report.terminal.builtin_in_use, expect_builtin);
            assert_eq!(report.terminal.note, None);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn terminal_unparseable_settings_reports_note() {
        let root = temp_root("terminal-broken");
        let broken = root.join("broken.json");
        std::fs::write(&broken, "{not json").expect("写坏文件");
        let registry = MemRegistry::default();
        let env = FontEnv {
            registry: &registry,
            user_fonts_dir: root.join("fonts"),
            terminal_settings: vec![broken.clone()],
        };
        let report = status(&env);
        assert_eq!(
            report.terminal.settings_file.as_deref(),
            Some(broken.to_string_lossy().as_ref())
        );
        assert!(report.terminal.note.is_some(), "解析失败要有说明");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hint_lines_forbid_silent_rewrite() {
        let hints = hint_lines();
        assert!(hints[0].contains(TERMINAL_FONT_SNIPPET));
        assert!(hints[0].contains("不代改"), "显式声明不改写用户配置");
        assert!(hints[1].contains("终端模拟器"), "终端字体不可由程序强制");
    }
}
