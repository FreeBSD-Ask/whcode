//! # 运行时能力等级展示（Check 101、FR-702、ADR-024）
//!
//! 三档能力等级 `full`／`restricted`／`shell_only` 必须在 UI 与门禁输出中**可见**，
//! 且不得把受限实现表述为完整生态（D-08／R-10）。本模块只做“读取声明 + 可选探针”：
//!
//! - `doctor`（门禁输出）：`probe = true`，同时报告是否可用；
//! - TUI 摘要行与 Web `/api/runtimes`：`probe = false`，只展示能力等级与不支持项，
//!   **不启动子进程**（资源优先：界面刷新路径不得引入进程探测）。
//!
//! 边界：不注册工具、不装配运行时、不改 feature 门控（FR-704：默认全部关闭）。

use serde::Serialize;
use whcode_runtime::RuntimeProvider;
use whcode_runtime::providers::{BsdShellProvider, GitBashProvider, JsProvider, PythonProvider};

/// 单个运行时的展示条目（数据字段不翻译：`capability` 用稳定名）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeEntry {
    /// 运行时名（`js`／`python`／`gitbash`／`bsdshell`）。
    pub name: String,
    /// 能力等级稳定名（`full`／`restricted`／`shell_only`）。
    pub capability: &'static str,
    /// 是否可用；`None` 表示本次未探测（界面路径不启动子进程）。
    pub available: Option<bool>,
    /// 未覆盖能力（D-08：缺失项必须显式列出）。
    pub unsupported: Vec<String>,
}

/// 读取四个运行时的声明（顺序固定：js、python、gitbash、bsdshell）。
#[must_use]
pub fn inventory(probe: bool) -> Vec<RuntimeEntry> {
    let providers: [&dyn RuntimeProvider; 4] = [
        &JsProvider::default(),
        &PythonProvider::default(),
        &GitBashProvider::default(),
        &BsdShellProvider::default(),
    ];
    providers
        .iter()
        .map(|provider| RuntimeEntry {
            name: provider.name().to_string(),
            capability: provider.declaration().capability.as_str(),
            available: probe.then(|| provider.is_available()),
            unsupported: provider.declaration().unsupported.clone(),
        })
        .collect()
}

/// 紧凑摘要（TUI 摘要行可读）：`js=restricted(不可用) · python=restricted(未探测)`。
#[must_use]
pub fn summary(probe: bool) -> String {
    inventory(probe)
        .iter()
        .map(|entry| match entry.available {
            Some(true) => format!("{}={}(可用)", entry.name, entry.capability),
            Some(false) => format!("{}={}(不可用)", entry.name, entry.capability),
            None => format!("{}={}(未探测)", entry.name, entry.capability),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Check 101：三档能力等级都有来源，且四个运行时的名字互不混同（D-08/R-10）。
    #[test]
    fn inventory_exposes_capability_levels_without_merging_names() {
        let entries = inventory(false);
        assert_eq!(entries.len(), 4);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["js", "python", "gitbash", "bsdshell"]);
        for entry in &entries {
            assert!(
                ["full", "restricted", "shell_only"].contains(&entry.capability),
                "{} 的能力等级必须是三档之一：{}",
                entry.name,
                entry.capability
            );
            assert_eq!(entry.available, None, "界面路径不得探测子进程");
        }
        // JS 不交付引擎（ADR-024）：受限且列明不支持项。
        let js = &entries[0];
        assert_eq!(js.capability, "restricted");
        assert!(!js.unsupported.is_empty(), "受限运行时必须列明不支持项");
    }

    /// Check 101：摘要包含全部名称与能力等级；探针结果的三种状态都可读。
    #[test]
    fn summary_covers_all_runtimes_and_availability_states() {
        let text = summary(false);
        for name in ["js", "python", "gitbash", "bsdshell"] {
            assert!(text.contains(name), "摘要缺 {name}：{text}");
        }
        assert!(text.contains("restricted"), "{text}");
        assert!(text.contains("shell_only"), "{text}");
        assert!(text.contains("未探测"), "{text}");

        let probed = summary(true);
        assert!(
            probed.contains("js=restricted(不可用)"),
            "ADR-024：JS 无引擎必须报告不可用：{probed}"
        );
        assert!(
            !probed.contains("未探测"),
            "探测路径必须给出可用性：{probed}"
        );
    }
}
