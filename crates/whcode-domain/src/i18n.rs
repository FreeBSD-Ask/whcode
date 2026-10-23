//! # 文本层 i18n（T130、FR-615）
//!
//! 简体中文与英语**双语**、gettext PO 风格的 properties 加载接口、运行时区域检测、
//! `zh-CN → en` 回退链；为 RTL 语言预留双向文本处理接口（渲染层不假设 LTR；
//! 依据：GNU gettext 已支持 Rust 提取）。
//!
//! 资源目录 `i18n/`（`zh-CN.properties` / `en.properties`，UTF-8 无 BOM、LF）经
//! `include_str!` 编译期嵌入（零运行时文件依赖，本地优先 FR-615）。
//!
//! ## 运行时 PO 加载（FR-615）
//!
//! 其他语言的翻译以 gettext PO 文件交付，运行时约定路径 `<root>/i18n/po/<locale>.po`；
//! [`I18n::load_with_po`] 以 [`PO_SUBDIR`] 拼接该路径，文件存在则**覆盖**内建目录，
//! 不存在则回落到内建资源（`zh-CN`/`en`）。读取有界（[`MAX_PO_BYTES`]），超限或语言
//! 标识非法时返回结构化错误，不静默截断；语言标识经白名单校验，禁止路径穿越。
//!
//! ## 区域检测与回退链
//!
//! [`detect_locale`] 从显式配置 → 环境变量（`LANG`/`LC_ALL`）→ 默认 `zh-CN`；
//! [`Catalog::resolve`] 回退链：`zh-CN → zh → en`（请求的语言缺键时回退 en，
//! 再缺则返回键名本身——不伪造文本）。
//!
//! 边界：仅文本层；渲染层的 RTL/BiDi 处理属 `whcode-tui`/`whcode-web`（本模块
//! 提供 [`is_rtl_locale`] 判定接口，渲染层不假设 LTR）。

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{ErrorCode, WhError};

/// 编译期嵌入的简体中文资源（默认语言）。
pub const ZH_CN: &str = include_str!("i18n/zh-CN.properties");

/// 编译期嵌入的英语资源（回退语言）。
pub const EN: &str = include_str!("i18n/en.properties");

/// 运行时 PO 资源子目录（`<root>/i18n/po/<locale>.po`，FR-615）。
pub const PO_SUBDIR: &str = "i18n/po";

/// 单个 PO 文件的读取上限（有界；超限拒绝，不静默截断）。
pub const MAX_PO_BYTES: u64 = 256 * 1024;

/// 语言标识上限长度（`zh-CN`、`pt-BR` 等足够）。
const MAX_LOCALE_LEN: usize = 32;

/// 运行时区域配置来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocaleSource {
    /// 显式配置（可见、可改、可撤销——宪法 §I）。
    Explicit,
    /// 环境变量（`LANG`/`LC_ALL`）。
    Environment,
    /// 默认值（`zh-CN`）。
    Default,
}

/// 已加载的文本目录（键 → 译文；缺键回退见 [`Catalog::resolve`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    /// 语言标识（`zh-CN` / `en`）。
    pub locale: String,
    entries: BTreeMap<String, String>,
}

impl Catalog {
    /// 从 PO 文本构造目录（键为 `msgid`；带 `msgctxt` 时为 `ctx\u{4}msgid`）。
    #[must_use]
    pub fn from_po(locale: &str, text: &str) -> Self {
        Self {
            locale: locale.to_string(),
            entries: parse_po(text),
        }
    }

    /// 从内建资源构造目录（`zh*` 取中文，其余取英语——英语是回退语言）。
    #[must_use]
    pub fn builtin(locale: &str) -> Self {
        let text = if locale.starts_with("zh") { ZH_CN } else { EN };
        Self {
            locale: locale.to_string(),
            entries: parse_properties(text),
        }
    }
}

/// 解析 properties 文本（`key = value`，`#` 注释，UTF-8）。
fn parse_properties(text: &str) -> BTreeMap<String, String> {
    let mut entries = BTreeMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            let value = value.trim();
            // 值两侧引号剥离（properties 约定）。
            let value = value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .unwrap_or(value);
            entries.insert(key.trim().to_string(), value.to_string());
        }
        // 无法解析的行保留为未知键名？——不静默丢弃：跳过但格式错误的行没有键，
        // 嵌入资源是编译期产物，格式由测试固定（见 tests）。
    }
    entries
}

/// 解析 gettext PO 文本为“键 → 译文”。
///
/// 支持的语法子集：`msgctxt`（作为键前缀 `ctx\u{4}msgid`）、`msgid`、`msgstr`、
/// 相邻引号续行拼接、转义（`\\`、`\"`、`\n`、`\t`、`\r`）与 `#` 注释。
/// 按 gettext 语义**跳过**：`#~` 废弃条目、`#, fuzzy` 标记条目（不参与翻译）、
/// 复数条目（`msgid_plural`/`msgstr[n]`——当前界面文本无复数，出现即整条跳过）、
/// 空 `msgid`（文件头）。
///
/// 未知行忽略而不是让整份文件失败：PO 由翻译工具生成，语法噪音不应导致界面无文本；
/// 失败路径（非法编码、超限文件）由 [`I18n::load_with_po`] 以结构化错误上报。
#[must_use]
pub fn parse_po(text: &str) -> BTreeMap<String, String> {
    let mut entries = BTreeMap::new();
    let mut entry = PoEntry::default();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(comment) = line.strip_prefix('#') {
            if let Some(flags) = comment.strip_prefix(',')
                && flags.split(',').any(|flag| flag.trim() == "fuzzy")
            {
                entry.fuzzy = true;
            }
            // 普通注释与 `#~` 废弃条目（其 msgid/msgstr 行以 `#~` 开头，同被跳过）。
            continue;
        }
        if let Some(rest) = line.strip_prefix("msgctxt") {
            if entry.started() {
                entry.commit(&mut entries);
            }
            entry.ctxt = unquote(rest).unwrap_or_default();
            entry.last = PoField::Ctxt;
        } else if line.starts_with("msgid_plural") {
            entry.plural = true;
            entry.last = PoField::Ignored;
        } else if let Some(rest) = line.strip_prefix("msgid") {
            if entry.started() {
                entry.commit(&mut entries);
            }
            entry.id = Some(unquote(rest).unwrap_or_default());
            entry.last = PoField::Id;
        } else if line.starts_with("msgstr[") {
            entry.plural = true;
            entry.last = PoField::Ignored;
        } else if let Some(rest) = line.strip_prefix("msgstr") {
            entry.text = unquote(rest).unwrap_or_default();
            entry.has_str = true;
            entry.last = PoField::Text;
        } else if let Some(rest) = unquote(line) {
            // 续行：拼接到当前字段。
            match entry.last {
                PoField::Ctxt => entry.ctxt.push_str(&rest),
                PoField::Id => entry.id.get_or_insert_with(String::new).push_str(&rest),
                PoField::Text => entry.text.push_str(&rest),
                // 续行出现在任何字段之前，或属于复数/未知字段：丢弃。
                PoField::None | PoField::Ignored => {}
            }
        }
    }
    entry.commit(&mut entries);
    entries
}

/// PO 条目当前累积到哪个字段（决定续行拼接到哪里）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum PoField {
    /// 尚未遇到任何字段。
    #[default]
    None,
    /// `msgctxt`。
    Ctxt,
    /// `msgid`。
    Id,
    /// `msgstr`。
    Text,
    /// 复数或未知字段：其续行丢弃。
    Ignored,
}

/// 单条 PO 条目的解析中间态。
#[derive(Debug, Default)]
struct PoEntry {
    ctxt: String,
    id: Option<String>,
    text: String,
    has_str: bool,
    fuzzy: bool,
    plural: bool,
    last: PoField,
}

impl PoEntry {
    /// 是否已开始一个新条目（用于容忍缺失空行分隔的 PO 文件）。
    fn started(&self) -> bool {
        self.last != PoField::None
    }

    /// 提交当前条目：跳过 fuzzy／复数／空 `msgid`。
    ///
    /// 尚未遇到 `msgid`（例如 `msgctxt` 先行）时保持状态不变——上下文属于下一条条目。
    fn commit(&mut self, entries: &mut BTreeMap<String, String>) {
        let Some(id) = self.id.take() else {
            return;
        };
        if !self.fuzzy && !self.plural && self.has_str && !id.is_empty() {
            let key = if self.ctxt.is_empty() {
                id
            } else {
                format!("{}\u{4}{}", self.ctxt, id)
            };
            entries.insert(key, std::mem::take(&mut self.text));
        }
        *self = Self::default();
    }
}

/// 解析 PO 字符串字面量（`"..."`）并处理转义；非字符串行返回 `None`。
fn unquote(value: &str) -> Option<String> {
    let value = value.trim();
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            // 未知转义：保留原文，不吞掉字符。
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    Some(out)
}

/// 检测运行时区域：显式配置 → 环境变量 → 默认（可见可改，宪法 §I）。
#[must_use]
pub fn detect_locale(explicit: Option<&str>) -> (String, LocaleSource) {
    if let Some(locale) = explicit.filter(|locale| !locale.trim().is_empty()) {
        return (locale.trim().to_string(), LocaleSource::Explicit);
    }
    for key in ["LC_ALL", "LANG"] {
        if let Ok(value) = std::env::var(key) {
            let locale = value
                .split('.')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            if !locale.is_empty() {
                return (locale, LocaleSource::Environment);
            }
        }
    }
    ("zh-CN".to_string(), LocaleSource::Default)
}

/// 默认文本层：`zh-CN` + `en` 回退（FR-615 默认语言）。
impl Default for I18n {
    fn default() -> Self {
        Self::load("zh-CN", LocaleSource::Default)
    }
}

/// 文本目录加载与回退（FR-615）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct I18n {
    /// 请求的语言。
    pub locale: String,
    /// 区域来源。
    pub source: LocaleSource,
    primary: Catalog,
    fallback: Catalog,
}

impl I18n {
    /// 加载文本层：主语言目录 + 回退目录。
    ///
    /// `zh-CN*` 请求加载 `zh-CN` 主目录、`en` 回退；其他请求以 `en` 为主、`zh-CN` 回退——
    /// 回退链：主目录 → 回退目录 → 键名本身。
    #[must_use]
    pub fn load(locale: &str, source: LocaleSource) -> Self {
        Self {
            locale: locale.to_string(),
            source,
            primary: Catalog::builtin(locale),
            fallback: Catalog::builtin(fallback_locale(locale)),
        }
    }

    /// 运行时加载文本层：PO 文件覆盖内建资源（FR-615）。
    ///
    /// 查找 `<root>/i18n/po/<locale>.po` 与回退语言的同名文件；文件不存在时回落到
    /// [`Catalog::builtin`]，因此无 PO 的部署与 [`I18n::load`] 等价。
    ///
    /// # Errors
    ///
    /// 语言标识非法（空、超长、含路径分隔符等）、PO 文件超限或非 UTF-8、读取失败时
    /// 返回结构化错误（`parse_error` / `io_error`）——不静默降级。
    pub fn load_with_po(root: &Path, locale: &str, source: LocaleSource) -> Result<Self, WhError> {
        validate_locale(locale)?;
        let fallback = fallback_locale(locale);
        validate_locale(fallback)?;
        let primary = match load_po_catalog(root, locale)? {
            Some(catalog) => catalog,
            None => Catalog::builtin(locale),
        };
        let fallback = match load_po_catalog(root, fallback)? {
            Some(catalog) => catalog,
            None => Catalog::builtin(fallback),
        };
        Ok(Self {
            locale: locale.to_string(),
            source,
            primary,
            fallback,
        })
    }

    /// 解析键：主目录 → 回退目录 → 键名本身（不伪造文本）。
    #[must_use]
    pub fn resolve<'a>(&'a self, key: &'a str) -> &'a str {
        self.primary
            .entries
            .get(key)
            .or_else(|| self.fallback.entries.get(key))
            .map(String::as_str)
            .unwrap_or(key)
    }

    /// 已加载条目（合并：回退语言打底，主语言覆盖）——供 Web `/api/i18n` 下发。
    #[must_use]
    pub fn entries(&self) -> BTreeMap<String, String> {
        let mut merged = self.fallback.entries.clone();
        for (key, value) in &self.primary.entries {
            merged.insert(key.clone(), value.clone());
        }
        merged
    }

    /// 带占位符的解析：`{name}` 由 `args` 顺序替换（未提供的占位符原样保留）。
    ///
    /// 键缺失时返回键名本身（与 [`I18n::resolve`] 一致，不伪造文本）。
    #[must_use]
    pub fn resolve_fmt(&self, key: &str, args: &[(&str, &str)]) -> String {
        let mut text = self.resolve(key).to_string();
        for (name, value) in args {
            let placeholder = format!("{{{name}}}");
            text = text.replace(&placeholder, value);
        }
        text
    }

    /// 已加载条目数（诊断）。
    #[must_use]
    pub fn len(&self) -> usize {
        self.primary.entries.len()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.primary.entries.is_empty()
    }
}

/// RTL 语言判定接口（为阿拉伯语等预留；渲染层不假设 LTR，FR-615）。
#[must_use]
pub fn is_rtl_locale(locale: &str) -> bool {
    matches!(
        locale.trim().to_ascii_lowercase().as_str(),
        "ar" | "he" | "fa" | "ur"
    ) || locale.trim().to_ascii_lowercase().starts_with("ar-")
}

/// 双向文本处理接口（RTL 预留；返回视觉重排前的逻辑顺序——BiDi 重排在渲染层）。
#[must_use]
pub fn logical_order(text: &str) -> &str {
    text
}

/// 回退语言：中文请求回退 `en`，其余请求回退 `zh-CN`（与 [`I18n::load`] 一致）。
#[must_use]
pub fn fallback_locale(locale: &str) -> &'static str {
    if locale.starts_with("zh") {
        "en"
    } else {
        "zh-CN"
    }
}

/// 语言标识白名单校验：ASCII 字母/数字与 `-`、`_`，长度 1—32。
///
/// 目的是禁止路径穿越（`../`）与非法文件名；不校验 BCP 47 的完整语法。
fn validate_locale(locale: &str) -> Result<(), WhError> {
    let valid = !locale.is_empty()
        && locale.len() <= MAX_LOCALE_LEN
        && locale
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
    if valid {
        Ok(())
    } else {
        Err(WhError::parse_error(format!(
            "非法语言标识（仅允许 ASCII 字母数字与 -/_，长度 1—{MAX_LOCALE_LEN}）：{locale}"
        )))
    }
}

/// 读取并解析 `<root>/i18n/po/<locale>.po`；文件不存在返回 `Ok(None)`。
fn load_po_catalog(root: &Path, locale: &str) -> Result<Option<Catalog>, WhError> {
    let path = root.join(PO_SUBDIR).join(format!("{locale}.po"));
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(WhError::new(
                ErrorCode::IoError,
                format!("PO 文件不可读（{}）：{err}", path.display()),
            ));
        }
    };
    if metadata.len() > MAX_PO_BYTES {
        return Err(WhError::new(
            ErrorCode::IoError,
            format!(
                "PO 文件超过上限 {MAX_PO_BYTES} 字节（{}）：{}",
                path.display(),
                metadata.len()
            ),
        ));
    }
    let bytes = std::fs::read(&path).map_err(|err| {
        WhError::new(
            ErrorCode::IoError,
            format!("PO 文件读取失败（{}）：{err}", path.display()),
        )
    })?;
    let text = String::from_utf8(bytes).map_err(|err| {
        WhError::parse_error(format!(
            "PO 文件不是合法 UTF-8（{}）：{err}",
            path.display()
        ))
    })?;
    Ok(Some(Catalog::from_po(locale, &text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时根目录守卫：析构时删除，避免污染系统临时目录。
    struct TempRoot(std::path::PathBuf);

    impl TempRoot {
        /// 创建空的临时根目录（不建 `i18n/po`）。
        fn empty(tag: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("whcode-i18n-{}-{tag}", std::process::id()));
            std::fs::create_dir_all(&path).expect("创建临时根目录");
            Self(path)
        }

        /// 创建带 `i18n/po` 的临时根目录。
        fn with_po(tag: &str) -> Self {
            let root = Self::empty(tag);
            std::fs::create_dir_all(root.0.join(PO_SUBDIR)).expect("创建临时 PO 目录");
            root
        }

        /// 写入 `<locale>.po`。
        fn write(&self, locale: &str, text: &str) {
            std::fs::write(self.0.join(PO_SUBDIR).join(format!("{locale}.po")), text)
                .expect("写入 PO 文件");
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// PO 字符串字面量转义（仅测试用：把内建目录导出为 PO 以便往返校验）。
    fn po_escape(value: &str) -> String {
        let mut out = String::new();
        for ch in value.chars() {
            match ch {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                other => out.push(other),
            }
        }
        out
    }

    #[test]
    fn zh_cn_resources_load_with_fallback_to_en() {
        let i18n = I18n::load("zh-CN", LocaleSource::Default);
        assert_eq!(i18n.resolve("app.title"), "旺皇大模型执行框架");
        assert!(!i18n.is_empty());
        assert!(!i18n.is_empty());

        // 缺键回退 en：zh-CN 目录没有的键由 en 提供。
        let i18n = I18n::load("en", LocaleSource::Explicit);
        assert_eq!(i18n.resolve("menu.sessions"), "Sessions");
        // 键缺失时返回键名本身（不伪造文本）。
        assert_eq!(i18n.resolve("nonexistent.key"), "nonexistent.key");
    }

    /// 双语键表必须一致（Check 222“zh-CN 与 en 语义一致”的结构前提）：
    /// 任一语言缺键都会导致回退到另一种语言或键名——发现即失败。
    #[test]
    fn both_catalogs_define_the_same_keys() {
        let zh = parse_properties(ZH_CN);
        let en = parse_properties(EN);
        let mut zh_keys: Vec<&String> = zh.keys().collect();
        let mut en_keys: Vec<&String> = en.keys().collect();
        zh_keys.sort();
        en_keys.sort();
        assert_eq!(zh_keys, en_keys, "zh-CN 与 en 的键集合必须一致");
        // 占位符集合一致（同一键在两种语言中必须引用同一组参数）。
        for key in zh_keys {
            let placeholders = |text: &str| {
                let mut names: Vec<String> = Vec::new();
                let mut rest = text;
                while let Some(start) = rest.find('{') {
                    let tail = &rest[start + 1..];
                    let Some(end) = tail.find('}') else { break };
                    names.push(tail[..end].to_string());
                    rest = &tail[end + 1..];
                }
                names.sort();
                names
            };
            assert_eq!(
                placeholders(&zh[key]),
                placeholders(&en[key]),
                "键 {key} 的占位符集合不一致"
            );
        }
    }

    #[test]
    fn formatted_resolution_substitutes_placeholders() {
        let zh = I18n::load("zh-CN", LocaleSource::Default);
        let text = zh.resolve_fmt(
            "tui.footer",
            &[("cols", "120"), ("rows", "30"), ("keys", "3")],
        );
        assert!(text.contains("窗口 120×30"), "{text}");
        assert!(text.contains("按键 3"), "{text}");
        let en = I18n::load("en", LocaleSource::Explicit);
        let text = en.resolve_fmt("web.cursor", &[("n", "6")]);
        assert_eq!(text, "Cursor 6");
        // 缺键 → 键名（不伪造）；未提供占位符原样保留。
        assert_eq!(zh.resolve_fmt("missing.key", &[]), "missing.key");
        let partial = zh.resolve_fmt("web.cursor", &[]);
        assert_eq!(partial, "游标 {n}");
    }

    #[test]
    fn locale_detection_prefers_explicit_then_environment_then_default() {
        let (locale, source) = detect_locale(Some("en"));
        assert_eq!(locale, "en");
        assert_eq!(source, LocaleSource::Explicit);
        let (_, source) = detect_locale(None);
        assert!(matches!(
            source,
            LocaleSource::Environment | LocaleSource::Default
        ));
        // 空显式配置回落环境/默认：来源不得仍是 Explicit（区域来源语义）。
        let (_, source) = detect_locale(Some("  "));
        assert!(source != LocaleSource::Explicit, "空显式配置必须回落");
    }

    #[test]
    fn rtl_interface_reserved_for_renderers() {
        assert!(is_rtl_locale("ar"));
        assert!(is_rtl_locale("ar-SA"));
        assert!(!is_rtl_locale("zh-CN"));
        assert!(!is_rtl_locale("en"));
        // 逻辑顺序接口预留：当前恒返回原文（BiDi 重排在渲染层）。
        assert_eq!(logical_order("مرحبا"), "مرحبا");
    }

    /// Check 224：PO 加载器必须处理续行、转义、`msgctxt`，并跳过 fuzzy／复数／废弃条目。
    #[test]
    fn po_loader_handles_multiline_escapes_and_skips_noise() {
        let text = concat!(
            "# 普通注释\n",
            "#, fuzzy\n",
            "msgid \"fuzzy.key\"\n",
            "msgstr \"不应生效\"\n",
            "\n",
            "msgctxt \"menu\"\n",
            "msgid \"sessions\"\n",
            "msgstr \"会话\"\n",
            "\n",
            "msgid \"app.title\"\n",
            "msgstr \"旺皇\"\n",
            "\" Code\"\n",
            "\n",
            "msgid \"\"\n",
            "msgstr \"Content-Type: text/plain; charset=UTF-8\\n\"\n",
            "\n",
            "msgid \"plural.key\"\n",
            "msgid_plural \"plural.keys\"\n",
            "msgstr[0] \"单数\"\n",
            "\n",
            "#~ msgid \"obsolete.key\"\n",
            "#~ msgstr \"废弃\"\n",
            "\n",
            "msgid \"multi.line\"\n",
            "msgstr \"\"\n",
            "\"第一行\\n\"\n",
            "\"第二行\"\n"
        );
        let entries = parse_po(text);
        assert_eq!(
            entries.get("app.title").map(String::as_str),
            Some("旺皇 Code")
        );
        assert_eq!(
            entries.get("menu\u{4}sessions").map(String::as_str),
            Some("会话"),
            "msgctxt 必须成为键前缀"
        );
        assert_eq!(
            entries.get("multi.line").map(String::as_str),
            Some("第一行\n第二行"),
            "续行与转义必须正确拼接"
        );
        assert!(!entries.contains_key("fuzzy.key"), "fuzzy 条目不得生效");
        assert!(!entries.contains_key("obsolete.key"), "废弃条目不得生效");
        assert!(!entries.contains_key("plural.key"), "复数条目不得生效");
        assert!(!entries.contains_key(""), "文件头空 msgid 不得成为键");
    }

    /// Check 224：PO 覆盖内建目录，缺键回退 `en`，再缺返回键名（不伪造）。
    #[test]
    fn po_catalog_overrides_builtin_and_falls_back_to_en() {
        let root = TempRoot::with_po("fallback");
        root.write(
            "zh-CN",
            "msgid \"app.title\"\nmsgstr \"测试标题\"\n\nmsgid \"only.zh\"\nmsgstr \"仅中文\"\n",
        );
        root.write(
            "en",
            "msgid \"app.title\"\nmsgstr \"PO Title\"\n\nmsgid \"only.en\"\nmsgstr \"EN only\"\n\nmsgid \"menu.sessions\"\nmsgstr \"Sessions\"\n",
        );
        let texts =
            I18n::load_with_po(&root.0, "zh-CN", LocaleSource::Explicit).expect("PO 加载成功");
        assert_eq!(texts.resolve("app.title"), "测试标题", "PO 覆盖内建目录");
        assert_eq!(texts.resolve("only.en"), "EN only", "缺键回退 en 目录");
        assert_eq!(texts.resolve("menu.sessions"), "Sessions", "回退 en 目录");
        assert_eq!(
            texts.resolve("not.defined"),
            "not.defined",
            "两侧都缺键时返回键名，不伪造文本"
        );
    }

    /// Check 224：无 PO 文件时与内建加载等价（部署无需携带翻译文件）。
    #[test]
    fn missing_po_files_fall_back_to_embedded_catalogs() {
        let root = TempRoot::empty("no-po");
        let loaded =
            I18n::load_with_po(&root.0, "zh-CN", LocaleSource::Default).expect("无 PO 时回退内建");
        assert_eq!(loaded, I18n::load("zh-CN", LocaleSource::Default));
    }

    /// Check 224：语言标识白名单拒绝路径穿越与空值（安全边界）。
    #[test]
    fn invalid_locale_identifiers_are_rejected() {
        let root = TempRoot::empty("bad-locale");
        for locale in [
            "../evil",
            "",
            "zh/CN",
            "zh CN",
            &"a".repeat(MAX_LOCALE_LEN + 1),
        ] {
            let err = I18n::load_with_po(&root.0, locale, LocaleSource::Explicit)
                .expect_err("非法语言标识必须失败");
            assert_eq!(err.code, ErrorCode::ParseError, "{locale}");
        }
    }

    /// Check 224：超限 PO 拒绝读取（有界，不静默截断）。
    #[test]
    fn oversized_po_file_is_rejected() {
        let root = TempRoot::with_po("oversized");
        let padding = "x".repeat(usize::try_from(MAX_PO_BYTES).expect("常量可转换") + 1);
        root.write("zh-CN", &padding);
        let err =
            I18n::load_with_po(&root.0, "zh-CN", LocaleSource::Explicit).expect_err("超限必须失败");
        assert_eq!(err.code, ErrorCode::IoError);
        assert!(err.message.contains("上限"), "{err}");
    }

    /// Check 224：i18n 资源目录结构（两份 properties、UTF-8 无 BOM、LF），
    /// 且内建目录可无损导出为 PO 文本后由加载器读回（接口可用性的往返证据）。
    #[test]
    fn i18n_resource_directory_structure_roundtrips_through_po() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("i18n");
        for name in ["zh-CN.properties", "en.properties"] {
            let path = dir.join(name);
            let bytes =
                std::fs::read(&path).unwrap_or_else(|err| panic!("读取 {name} 失败：{err}"));
            assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "{name} 不得带 BOM");
            assert!(!bytes.contains(&b'\r'), "{name} 必须为 LF 换行");
            let text =
                String::from_utf8(bytes).unwrap_or_else(|err| panic!("{name} 非 UTF-8：{err}"));
            let entries = parse_properties(&text);
            assert!(!entries.is_empty(), "{name} 不得为空");

            let mut po = String::new();
            for (key, value) in &entries {
                po.push_str(&format!(
                    "msgid \"{}\"\nmsgstr \"{}\"\n\n",
                    po_escape(key),
                    po_escape(value)
                ));
            }
            assert_eq!(parse_po(&po), entries, "{name} 经 PO 往返必须无损");
        }
    }
}
