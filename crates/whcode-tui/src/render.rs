//! # 渲染纪律（T083/T129、§5.3、FR-613、FR-617）
//!
//! 三条纪律的可执行实现：
//!
//! 1. **只读快照**（T083/§5.3）：渲染只消费 [`crate::views::ViewSnapshot`]（不可变
//!    数据），渲染器不持有可变业务状态；
//! 2. **脏标记重绘**（T083）：仅脏时重绘——[`Renderer::handle_event`] 按事件类型
//!    置脏，[`Renderer::render`] 重绘后清脏并返回是否实际绘制；
//! 3. **不在帧路径反序列化 JSON**（T083/§5.3）：本模块无 `serde_json` 反序列化调用
//!    （结构测试断言），数据由准备层处理为纯数据。
//!
//! ## 空闲期零重绘（T129、FR-613）
//!
//! [`Renderer::redraw_count`] 计数器：空闲期（无事件、数据未变）渲染调用**不增加**
//! 重绘计数——NFR-205 空闲 CPU 验证的可执行依据。**终端绘制由终端模拟器承担，
//! WHCode 不直接调度 GPU**（FR-613 文档要求）。
//!
//! ## 窗口最小化渲染门控（T135、FR-617）
//!
//! [`Renderer::minimized`] 置真时跳过绘制（关键事件仍按类型记账置脏），还原时
//! **单次全量重绘**（`force_full_redraw` 清脏前恰好绘制一次）。

use crate::views::ViewSnapshot;

/// 渲染器：脏标记 + 重绘计数 + 最小化门控（T083/T129/T135）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Renderer {
    dirty: bool,
    redraw_count: usize,
    minimized: bool,
    skipped_while_minimized: usize,
}

impl Default for Renderer {
    fn default() -> Self {
        Self {
            dirty: true, // 初始帧须绘制一次。
            redraw_count: 0,
            minimized: false,
            skipped_while_minimized: 0,
        }
    }
}

impl Renderer {
    /// 空渲染器（初始脏，保证首帧绘制）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 处理事件：按类型置脏（数据变更类事件置脏；纯时间/空闲事件不置脏——
    /// T129 空闲期零重绘）。关键事件在最小化期间**继续记账置脏**（FR-617）。
    pub fn handle_event(&mut self, event: RenderEvent) {
        match event {
            RenderEvent::DataChanged | RenderEvent::CriticalTransition => {
                self.dirty = true;
            }
            RenderEvent::IdleTick => {
                // 空闲事件不置脏：数据未变不重绘（T129）。
            }
            RenderEvent::Minimized => {
                self.minimized = true;
            }
            RenderEvent::Restored => {
                // 还原：单次全量重绘（FR-617）——置脏一次，跳过计数清零。
                self.minimized = false;
                self.dirty = true;
                self.skipped_while_minimized = 0;
            }
        }
    }

    /// 渲染快照：仅脏且未最小化时返回**待绘制的文本行**（实际写入终端由调用方完成）；
    /// 未脏（空闲）或最小化时返回 `None`。
    ///
    /// `max_lines` 为帧路径的行数上界（终端行数，Check 83 帧预算）：投影只格式化可见
    /// 行——帧成本由终端尺寸界定，与快照行数无关（完整行数由准备层按“有界尾部”裁剪）。
    ///
    /// - 空闲期零重绘（T129）：未脏时不增加重绘计数；
    /// - 最小化门控（T135/FR-617）：跳过绘制并计数（关键事件已按类型记账置脏，
    ///   还原后单次全量补绘）。
    #[must_use]
    pub fn render(&mut self, snapshot: &ViewSnapshot, max_lines: usize) -> Option<Vec<String>> {
        if self.minimized {
            if self.dirty {
                self.skipped_while_minimized += 1;
            }
            return None;
        }
        if !self.dirty {
            return None; // 空闲期零重绘（T129）。
        }
        self.dirty = false;
        self.redraw_count += 1;
        Some(snapshot.render_lines_limited(max_lines)) // 只读投影；不反序列化（§5.3）。
    }

    /// 强制单次全量重绘（还原路径，FR-617）：返回待绘制的文本行。
    #[must_use]
    pub fn force_full_redraw(
        &mut self,
        snapshot: &ViewSnapshot,
        max_lines: usize,
    ) -> Option<Vec<String>> {
        self.dirty = true;
        self.render(snapshot, max_lines)
    }

    /// 是否存在待绘制帧（脏）——事件循环据此决定是否进入最小化探测
    /// （T135/ADR-021：无待绘制帧时不探测，空闲成本为零）。
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// 重绘计数（T129 空闲期零重绘断言的可执行依据）。
    #[must_use]
    pub const fn redraw_count(&self) -> usize {
        self.redraw_count
    }

    /// 是否最小化（FR-617 可查询）。
    #[must_use]
    pub const fn is_minimized(&self) -> bool {
        self.minimized
    }

    /// 最小化期间跳过的绘制次数（可查询）。
    #[must_use]
    pub const fn skipped_while_minimized(&self) -> usize {
        self.skipped_while_minimized
    }
}

/// 渲染事件（数据变更置脏；空闲不置脏；最小化/还原门控）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderEvent {
    /// 数据变更（会话/任务/事件数据更新）。
    DataChanged,
    /// 关键状态转换（最小化期间仍置脏记账，FR-617）。
    CriticalTransition,
    /// 空闲心跳（不置脏——T129）。
    IdleTick,
    /// 窗口最小化。
    Minimized,
    /// 窗口还原。
    Restored,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::ViewId;

    fn snapshot() -> ViewSnapshot {
        ViewSnapshot::empty(ViewId::Sessions)
    }

    /// T083：脏标记重绘——重绘后清脏，数据未变不重绘；返回的文本行即帧内容。
    #[test]
    fn dirty_flag_redraws_only_when_dirty() {
        let mut renderer = Renderer::new();
        let expected = snapshot().render_lines();
        assert_eq!(
            renderer.render(&snapshot(), 24),
            Some(expected.clone()),
            "首帧必须绘制且给出文本行"
        );
        assert_eq!(renderer.redraw_count(), 1);

        // 空闲期：数据未变不重绘（T129 零重绘）。
        assert!(renderer.render(&snapshot(), 24).is_none(), "未脏不得重绘");
        renderer.handle_event(RenderEvent::IdleTick);
        assert!(
            renderer.render(&snapshot(), 24).is_none(),
            "空闲心跳不得触发重绘"
        );
        assert_eq!(renderer.redraw_count(), 1, "空闲期零重绘（T129）");

        // 数据变更：置脏后重绘一次。
        renderer.handle_event(RenderEvent::DataChanged);
        assert_eq!(renderer.render(&snapshot(), 24), Some(expected));
        assert_eq!(renderer.redraw_count(), 2);
        assert!(!renderer.is_dirty(), "重绘后必须清脏");
    }

    /// T129：空闲期零重绘——连续空闲心跳重绘计数不增。
    #[test]
    fn idle_period_has_zero_redraws() {
        let mut renderer = Renderer::new();
        let _ = renderer.render(&snapshot(), 24);
        let baseline = renderer.redraw_count();
        for _ in 0..100 {
            renderer.handle_event(RenderEvent::IdleTick);
            assert!(renderer.render(&snapshot(), 24).is_none());
        }
        assert_eq!(
            renderer.redraw_count(),
            baseline,
            "100 次空闲心跳零重绘（T129/FR-613）"
        );
    }

    /// T135：最小化跳过绘制 + 关键事件记账 + 还原单次全量重绘。
    #[test]
    fn minimize_gates_drawing_and_restores_with_full_redraw() {
        let mut renderer = Renderer::new();
        let _ = renderer.render(&snapshot(), 24);
        let before = renderer.redraw_count();

        renderer.handle_event(RenderEvent::Minimized);
        assert!(renderer.is_minimized());
        // 首帧后已清脏：无待绘制帧可跳过，跳过计数为 0。
        assert!(
            renderer.render(&snapshot(), 24).is_none(),
            "最小化期间跳过绘制"
        );
        assert_eq!(renderer.skipped_while_minimized(), 0);

        // 关键事件在最小化期间继续记账置脏（FR-617）：待绘制帧被跳过并计数。
        renderer.handle_event(RenderEvent::CriticalTransition);
        assert!(renderer.is_dirty(), "关键转换在最小化期间仍置脏记账");
        assert!(
            renderer.render(&snapshot(), 24).is_none(),
            "仍最小化，不绘制"
        );
        assert_eq!(renderer.skipped_while_minimized(), 1);

        // 还原：单次全量重绘。
        renderer.handle_event(RenderEvent::Restored);
        assert!(
            renderer.force_full_redraw(&snapshot(), 24).is_some(),
            "还原后单次全量重绘"
        );
        assert_eq!(renderer.redraw_count(), before + 1);
        assert_eq!(renderer.skipped_while_minimized(), 0, "还原后跳过计数清零");
    }

    /// T083：不在帧路径反序列化 JSON——结构断言：非测试源不含 serde_json。
    #[test]
    fn render_path_does_not_deserialize_json() {
        // 结构断言：剥掉文档注释与测试模块后，代码不含 serde_json 调用（§5.3）
        // （自指规避：断言字符串位于测试模块内，须剥离后再匹配）。
        let source = include_str!("render.rs");
        let code_only: String = source
            .split("#[cfg(test)]")
            .next()
            .unwrap_or(source)
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        assert!(
            !code_only.contains("serde_json"),
            "渲染帧路径不得反序列化 JSON（§5.3）"
        );
        let views_source = include_str!("views.rs");
        let views_code: String = views_source
            .split("#[cfg(test)]")
            .next()
            .unwrap_or(views_source)
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        assert!(!views_code.contains("serde_json::from_"));
    }
}
