//! # 重复执行缓存判定（T106、FR-707）
//!
//! 缓存**仅用于纯函数或显式启用缓存的任务**（FR-707）——默认不缓存（副作用
//! 任务重复执行不命中缓存）；缓存命中以脚本+输入哈希为键。

use std::collections::BTreeMap;

use crate::payload::fnv64;

/// 执行缓存（默认空；仅显式启用时写入，FR-707）。
#[derive(Debug, Default)]
pub struct ExecutionCache {
    enabled: bool,
    entries: BTreeMap<(String, String), String>,
}

impl ExecutionCache {
    /// 禁用缓存（默认：副作用任务不缓存）。
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            entries: BTreeMap::new(),
        }
    }

    /// 显式启用（仅纯函数任务使用）。
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            entries: BTreeMap::new(),
        }
    }

    /// 是否启用。
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 缓存键：脚本 + 输入哈希。
    fn key(script: &str, input: &str) -> (String, String) {
        (fnv64(script.as_bytes()), fnv64(input.as_bytes()))
    }

    /// 查询；未启用时恒 `None`（FR-707：默认不缓存）。
    #[must_use]
    pub fn get(&self, script: &str, input: &str) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        self.entries
            .get(&Self::key(script, input))
            .map(String::as_str)
    }

    /// 写入；未启用时忽略。
    pub fn put(&mut self, script: &str, input: &str, output: &str) {
        if self.enabled {
            self.entries
                .insert(Self::key(script, input), output.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_only_applies_when_explicitly_enabled() {
        let mut disabled = ExecutionCache::disabled();
        disabled.put("echo", "", "hi");
        assert!(
            disabled.get("echo", "").is_none(),
            "禁用缓存不命中（FR-707）"
        );

        let mut enabled = ExecutionCache::enabled();
        enabled.put("echo", "", "hi");
        assert_eq!(enabled.get("echo", ""), Some("hi"), "显式启用后命中");
        assert_ne!(enabled.get("other", ""), Some("hi"), "不同脚本不串键");
    }
}
