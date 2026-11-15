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
    /// 是否默认启用（P5.6／FR-784／ADR-024 修订）：当前四项全部为 `false`——
    /// 运行时不随主二进制启用，必须显式 opt-in（FR-704）。此字段让“默认值”可见，
    /// 而不是藏在装配代码里。
    pub default_enabled: bool,
    /// 启用条件（默认关闭时的可操作说明；`default_enabled` 为真时给“已默认启用”的判据）。
    pub enable_requirement: &'static str,
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
        .map(|provider| {
            let name = provider.name();
            let (default_enabled, enable_requirement) = enable_policy(name);
            RuntimeEntry {
                name: name.to_string(),
                capability: provider.declaration().capability.as_str(),
                available: probe.then(|| provider.is_available()),
                unsupported: provider.declaration().unsupported.clone(),
                default_enabled,
                enable_requirement,
            }
        })
        .collect()
}

/// 运行时默认启用策略（P5.6／FR-784／ADR-024 修订记录 2026-11-15）。
///
/// 结论：**四项全部默认关闭**。理由不是“没做”，而是三条硬约束的交点——资源铁律
/// （空闲态不得引入常驻进程）、安全边界（执行面必须显式 opt-in，ADR-070 同口径）、
/// 自由原则（默认值必须可见且可撤销）。策略写成表而不是散落在装配判断里，是为了让
/// `doctor`／Web `/api/runtimes` 能把“为什么默认关闭、怎么打开”原文输出。
///
/// 未登记的运行时按“默认关闭＋先补 ADR”处理，不静默当作可启用。
#[must_use]
pub fn enable_policy(name: &str) -> (bool, &'static str) {
    match name {
        "js" => (
            false,
            "内置引擎不交付（ADR-024 决策 1）：执行请求返回 runtime_unavailable",
        ),
        "python" => (
            false,
            "需安装解释器并显式 opt-in（FR-704）；执行经受控 worker（超时/输出上限/凭据剥离）",
        ),
        "gitbash" => (
            false,
            "需安装 Git Bash payload 并通过来源/版本/哈希/平台四重校验（FR-703）",
        ),
        "bsdshell" => (
            false,
            "M7 延后（ADR-013）：当前不交付，SSH 远程开发目标不要求原生引擎",
        ),
        _ => (
            false,
            "未登记的运行时默认关闭：启用前先补 ADR、能力声明与门禁",
        ),
    }
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

/// 策略表必须覆盖清单里的每个运行时，且当前无一项默认启用（改默认必须走 ADR）。
#[test]
fn every_runtime_has_a_policy_and_none_is_default_enabled() {
    let entries = inventory(false);
    assert_eq!(entries.len(), 4, "清单固定四项");
    for entry in &entries {
        assert!(
            !entry.default_enabled,
            "{} 默认启用必须由 ADR 修订并同批更新本测试",
            entry.name
        );
        assert!(
            !entry.enable_requirement.is_empty(),
            "{} 的启用条件不能为空（默认值必须可解释）",
            entry.name
        );
    }
    let (js_default, _) = enable_policy("js");
    assert!(!js_default, "JS 不交付引擎（ADR-024 决策 1）");
    let (unknown_default, reason) = enable_policy("no-such-runtime");
    assert!(!unknown_default);
    assert!(
        reason.contains("ADR"),
        "未登记运行时须指向 ADR 流程：{reason}"
    );
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
