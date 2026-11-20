//! # 运行时能力等级展示（Check 101、FR-702、ADR-024）
//!
//! 三档能力等级 `full`／`restricted`／`shell_only` 必须在 UI 与门禁输出中**可见**，
//! 且不得把受限实现表述为完整生态（D-08／R-10）。
//!
//! T395（ADR-089 批 6）：声明清单与启用策略**下沉**到
//! [`whcode_app::runtime_mgmt`]（唯一事实源——runtime 命令面与展示共用）；
//! 本模块保留 re-export 与 doctor/TUI 的展示辅助（`summary`／探测口径）。

pub use whcode_app::runtime_mgmt::inventory;

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
    use whcode_app::runtime_mgmt::enable_policy;
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
