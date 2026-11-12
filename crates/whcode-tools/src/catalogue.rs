//! # 工具目录生成与新鲜度校验（P7.4／FR-768／ADR-045 修订）
//!
//! `docs/guides/tool-catalogue.md` 是**受校验产物**：由本模块从
//! [`ToolRegistry::provider_manifest`]（模型可见契约）与工具元数据渲染，
//! 契约测试与其逐字节比对——任一工具改名、改参数、改副作用或改预算都会让目录漂移即红。
//!
//! 重新生成：
//!
//! ```text
//! WHCODE_UPDATE_TOOL_CATALOGUE=1 cargo test -p whcode-tools catalogue
//! ```
//!
//! 边界（为什么不是“文档更漂亮一点”）：外部评审与用户要能回答“这个模型到底能调用什么、
//! 副作用多大、有没有预算与输出契约”，而 `provider_manifest()` 只在运行时存在、不落文档；
//! 本模块把**同一份事实**投影成可 diff 的文本，并给出指纹以便复核“文档与代码同源”。
//!
//! 局限（必须承认）：目录只覆盖**内置工具**（`default_registry()`）；MCP 服务器提供的
//! 工具在运行时装配、按配置变化，不进本目录（进 `tool_manifest` 运行时事件）。

use std::path::Path;

use crate::registry::{SideEffectScope, ToolDescriptor, ToolRegistry};

/// 目录文件路径（相对仓库根）。
pub const CATALOGUE_PATH: &str = "docs/guides/tool-catalogue.md";

/// 重新生成目录的环境变量（只在测试里读；运行期不依赖）。
pub const UPDATE_ENV: &str = "WHCODE_UPDATE_TOOL_CATALOGUE";

/// provider 清单的稳定指纹（FNV-1a 64；检测“文档与代码同源”，不作密码学用途）。
///
/// 与 `whcode-store::schema::checksum` 的算法同源但**不跨 crate 复用**：分层规则不允许
/// `whcode-tools` 依赖 `whcode-store`（ADR-003），故此处保留 6 行独立实现并同口径命名。
#[must_use]
pub fn manifest_fingerprint(registry: &ToolRegistry) -> String {
    let manifest = registry.provider_manifest();
    let canonical = serde_json::to_string(&manifest).unwrap_or_default();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in canonical.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

/// 渲染目录全文（确定性：工具按稳定名升序，字段顺序固定）。
#[must_use]
pub fn render_catalogue(registry: &ToolRegistry) -> String {
    let mut out = String::new();
    out.push_str("# 工具目录（受校验产物）\n\n");
    out.push_str(
        "> 本文件由 `whcode-tools` 的 `catalogue::render_catalogue()` 生成，**不得手工编辑**。\n\
         > 重新生成：`WHCODE_UPDATE_TOOL_CATALOGUE=1 cargo test -p whcode-tools catalogue`。\n\
         > 门禁：`cargo test -p whcode-tools catalogue`（渲染结果与文件逐字节比对）。\n\n",
    );
    out.push_str(&format!(
        "manifest-fingerprint: {}\n\n",
        manifest_fingerprint(registry)
    ));
    out.push_str(&format!(
        "内置工具数：{}（MCP 服务器工具在运行时装配，不计入本目录）。\n",
        registry.len()
    ));
    for descriptor in registry.descriptors() {
        out.push_str(&render_tool(&descriptor));
    }
    out
}

fn render_tool(descriptor: &ToolDescriptor) -> String {
    let mut out = String::new();
    out.push_str(&format!("\n## {}\n\n", descriptor.name));
    out.push_str(&format!("- 说明：{}\n", descriptor.description));
    out.push_str(&format!(
        "- 副作用：{}\n",
        match descriptor.side_effect_scope {
            SideEffectScope::ReadOnly => "只读（不修改工作区）",
            SideEffectScope::WorkspaceWrite => "工作区写（修改文件内容）",
            SideEffectScope::SessionState => "会话状态写（只改会话派生状态，不触碰工作区）",
        }
    ));
    out.push_str(&format!(
        "- 执行预算：{}\n",
        match descriptor.timeout_ms {
            Some(ms) => format!("声明 {ms} ms（到期返回 `tool_timeout`，不强杀进程）"),
            None => "未声明（不设上限）".to_string(),
        }
    ));
    out.push_str(&format!(
        "- 输出契约：{}\n",
        match descriptor.output_schema {
            Some(_) => "已声明 `output_schema`（校验失败即结构化错误）",
            None => "未声明（纯文本输出，仍受统一截断上限）",
        }
    ));
    if descriptor.parameters.is_empty() {
        out.push_str("- 参数：无\n");
        return out;
    }
    out.push_str("- 参数：\n\n| 参数 | 类型 | 必填 | 说明 |\n|---|---|---|---|\n");
    for parameter in &descriptor.parameters {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            parameter.name,
            parameter.kind.as_str(),
            if parameter.required { "是" } else { "否" },
            parameter.description
        ));
    }
    out
}

/// 仓库根（测试与门禁共用；`docs/` 与 crate 目录的相对位置固定）。
#[must_use]
pub fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::default_registry;

    fn registry() -> ToolRegistry {
        default_registry().expect("默认注册表")
    }

    #[test]
    fn catalogue_matches_committed_doc() {
        let rendered = render_catalogue(&registry());
        let path = repo_root().join(CATALOGUE_PATH);
        if std::env::var(UPDATE_ENV).as_deref() == Ok("1") {
            std::fs::write(&path, &rendered).expect("写入工具目录");
            return;
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!(
                "读取 {CATALOGUE_PATH} 失败（{err}）：先运行 \
                 WHCODE_UPDATE_TOOL_CATALOGUE=1 cargo test -p whcode-tools catalogue"
            )
        });
        assert_eq!(
            committed, rendered,
            "工具目录与代码不一致：运行 WHCODE_UPDATE_TOOL_CATALOGUE=1 \
             cargo test -p whcode-tools catalogue 重新生成并复核差异"
        );
    }

    #[test]
    fn fingerprint_distinguishes_manifests_and_is_stable() {
        let empty = ToolRegistry::new();
        let registry = registry();
        assert!(manifest_fingerprint(&registry).starts_with("fnv1a64:"));
        assert_eq!(
            manifest_fingerprint(&registry),
            manifest_fingerprint(&registry)
        );
        assert_ne!(
            manifest_fingerprint(&empty),
            manifest_fingerprint(&registry)
        );
    }

    #[test]
    fn rendering_is_deterministic_and_covers_every_tool_once() {
        let registry = registry();
        let first = render_catalogue(&registry);
        assert_eq!(first, render_catalogue(&registry), "渲染必须确定性");
        assert_eq!(
            first.matches("\n## ").count(),
            registry.len(),
            "每个工具恰好一节"
        );
        for descriptor in registry.descriptors() {
            assert!(
                first.contains(&format!("\n## {}\n", descriptor.name)),
                "目录缺少工具：{}",
                descriptor.name
            );
        }
    }
}
