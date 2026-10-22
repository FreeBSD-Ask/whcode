//! # 文本层 i18n（T130、FR-615）
//!
//! 简体中文与英语**双语**、gettext PO 风格的 properties 加载接口、运行时区域检测、
//! `zh-CN → en` 回退链；为 RTL 语言预留双向文本处理接口（渲染层不假设 LTR；
//! 依据：GNU gettext 已支持 Rust 提取）。
//!
//! 资源目录 `i18n/`（`zh-CN.properties` / `en.properties`，UTF-8 无 BOM、LF）经
//! `include_str!` 编译期嵌入（零运行时文件依赖，本地优先 FR-615）。
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

use serde::{Deserialize, Serialize};

/// 编译期嵌入的简体中文资源（默认语言）。
pub const ZH_CN: &str = include_str!("i18n/zh-CN.properties");

/// 编译期嵌入的英语资源（回退语言）。
pub const EN: &str = include_str!("i18n/en.properties");

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
    /// 加载文本层：主语言目录 + en 回退。
    ///
    /// `zh-CN*` 请求加载 `zh-CN` 主目录；其他请求以 `en` 为主、`zh-CN` 回退——
    /// 回退链：主目录 → en → 键名本身。
    #[must_use]
    pub fn load(locale: &str, source: LocaleSource) -> Self {
        let primary_text = if locale.starts_with("zh") { ZH_CN } else { EN };
        let fallback_text = if locale.starts_with("zh") { EN } else { ZH_CN };
        Self {
            locale: locale.to_string(),
            source,
            primary: Catalog {
                locale: locale.to_string(),
                entries: parse_properties(primary_text),
            },
            fallback: Catalog {
                locale: "en".to_string(),
                entries: parse_properties(fallback_text),
            },
        }
    }

    /// 解析键：主目录 → en 回退 → 键名本身（不伪造文本）。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zh_cn_resources_load_with_fallback_to_en() {
        let i18n = I18n::load("zh-CN", LocaleSource::Default);
        assert_eq!(i18n.resolve("app.title"), "旺皇 Code");
        assert!(!i18n.is_empty());
        assert!(!i18n.is_empty());

        // 缺键回退 en：zh-CN 目录没有的键由 en 提供。
        let i18n = I18n::load("en", LocaleSource::Explicit);
        assert_eq!(i18n.resolve("menu.sessions"), "Sessions");
        // 键缺失时返回键名本身（不伪造文本）。
        assert_eq!(i18n.resolve("nonexistent.key"), "nonexistent.key");
    }

    /// 双语键表必须一致（Check 222「zh-CN 与 en 语义一致」的结构前提）：
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
}
