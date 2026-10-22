//! 有界队列与满载语义（任务书 §5.3、§7.3-2、ADR-006）。
//!
//! 规则：所有队列设上限并定义满载语义。**关键命令不能因满载而静默丢失**（必须返回背压
//! 错误，由调用方决定重试或安全停止）；低优先级展示快照可以合并或丢弃。
//!
//! 关键约束（ADR-006、§4.2 分级）：展示快照**只允许**淘汰其他展示快照，**不得**淘汰已入队
//! 的关键条目——否则“可丢弃”的低优先级数据会把“不得丢失”的高优先级数据挤出去。

use std::collections::VecDeque;

use whcode_domain::WhError;

/// 条目等级：与 `whcode_domain::EventCriticality` 的三级语义对应（快照合并为一档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryClass {
    /// 关键条目：不得因饱和被丢弃。
    Critical,
    /// 展示快照：可合并或丢弃。
    Snapshot,
}

/// 有界队列。
///
/// 队列只区分两类入队语义：
/// - [`BoundedQueue::push_critical`]：关键条目，满载即返回背压错误；
/// - [`BoundedQueue::push_snapshot`]：展示快照，满载时**仅淘汰最旧的展示快照**；若队列中
///   全部是关键条目，则拒绝新快照（计入 [`BoundedQueue::dropped_snapshots`]）。
#[derive(Debug)]
pub struct BoundedQueue<T> {
    capacity: usize,
    items: VecDeque<(T, EntryClass)>,
    dropped_snapshots: u64,
}

impl<T> BoundedQueue<T> {
    /// 按容量创建队列。容量必须大于 0——容量为 0 的队列无法满足"关键命令不丢失"。
    ///
    /// # Panics
    ///
    /// 当 `capacity == 0` 时 panic（配置错误应在启动期暴露，而非运行期静默丢事件）。
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "有界队列容量必须大于 0（§5.3：队列上限必须显式定义）"
        );
        Self {
            capacity,
            items: VecDeque::with_capacity(capacity),
            dropped_snapshots: 0,
        }
    }

    /// 队列容量。
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 当前深度。
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 是否已满。
    pub fn is_full(&self) -> bool {
        self.items.len() >= self.capacity
    }

    /// 当前已入队的关键条目数（诊断用）。
    pub fn critical_len(&self) -> usize {
        self.items
            .iter()
            .filter(|(_, class)| *class == EntryClass::Critical)
            .count()
    }

    /// 关键条目入队。满载时返回 `backpressure`（`retryable = true`），**绝不丢弃**。
    ///
    /// # Errors
    ///
    /// 队列已满时返回 [`WhError`]，错误码为 `backpressure`。
    pub fn push_critical(&mut self, item: T) -> Result<(), WhError> {
        if self.is_full() {
            return Err(WhError::backpressure(format!(
                "队列饱和（容量 {}）：关键条目未被接收；调用方须背压、重试或安全停止",
                self.capacity
            )));
        }
        self.items.push_back((item, EntryClass::Critical));
        Ok(())
    }

    /// 展示快照入队。满载时淘汰**最旧的展示快照**以接纳新值；不得淘汰关键条目。
    ///
    /// 返回 `true` 表示新快照已接收；`false` 表示队列已满且没有可淘汰的展示快照，新快照
    /// 被丢弃（仍计入 [`BoundedQueue::dropped_snapshots`]）。
    pub fn push_snapshot(&mut self, item: T) -> bool {
        if self.is_full() {
            let oldest_snapshot = self
                .items
                .iter()
                .position(|(_, class)| *class == EntryClass::Snapshot);
            match oldest_snapshot {
                Some(index) => {
                    self.items.remove(index);
                    self.dropped_snapshots += 1;
                }
                None => {
                    // 队列全是关键条目：拒绝新快照，保护关键条目（§4.2 分级）。
                    self.dropped_snapshots += 1;
                    return false;
                }
            }
        }
        self.items.push_back((item, EntryClass::Snapshot));
        true
    }

    /// 因队列饱和而被丢弃的展示快照计数（用于监控与报告，§11.5 指标）。
    pub fn dropped_snapshots(&self) -> u64 {
        self.dropped_snapshots
    }

    /// 取出队首条目（FIFO）。
    pub fn pop(&mut self) -> Option<T> {
        self.items.pop_front().map(|(item, _)| item)
    }

    /// 取出全部条目（保持顺序）。
    pub fn drain_all(&mut self) -> Vec<T> {
        self.items.drain(..).map(|(item, _)| item).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::ErrorCode;

    #[test]
    fn critical_push_returns_backpressure_when_full() {
        let mut queue = BoundedQueue::new(2);
        assert!(queue.push_critical("e1").is_ok());
        assert!(queue.push_critical("e2").is_ok());
        let err = queue.push_critical("e3").expect_err("满载必须返回背压错误");
        assert_eq!(err.code, ErrorCode::Backpressure);
        assert!(err.retryable, "背压错误必须标记为可重试");
        assert_eq!(queue.len(), 2, "关键条目不得因满载被丢弃");
        assert_eq!(queue.critical_len(), 2);
    }

    #[test]
    fn snapshot_push_drops_oldest_snapshot_and_counts() {
        let mut queue = BoundedQueue::new(2);
        assert!(queue.push_snapshot("frame-1"));
        assert!(queue.push_snapshot("frame-2"));
        assert!(queue.push_snapshot("frame-3"));
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.dropped_snapshots(), 1);
        assert_eq!(queue.pop(), Some("frame-2"), "最旧快照应被丢弃");
        assert_eq!(queue.pop(), Some("frame-3"));
        assert!(queue.is_empty());
    }

    #[test]
    fn snapshot_must_not_evict_critical_entries() {
        // ADR-006/§4.2：展示快照可丢弃，但不得把关键条目挤出去。
        let mut queue = BoundedQueue::new(2);
        queue.push_critical("critical-1").expect("关键条目入队");
        queue.push_critical("critical-2").expect("关键条目入队");
        assert!(
            !queue.push_snapshot("frame-1"),
            "无快照可淘汰时应拒绝新快照"
        );
        assert_eq!(queue.critical_len(), 2, "关键条目必须全部保留");
        assert_eq!(queue.dropped_snapshots(), 1);
        assert_eq!(queue.drain_all(), vec!["critical-1", "critical-2"]);
    }

    #[test]
    fn snapshot_prefers_evicting_snapshot_over_critical() {
        let mut queue = BoundedQueue::new(3);
        queue.push_critical("critical-1").expect("关键条目入队");
        queue.push_snapshot("frame-1");
        queue.push_snapshot("frame-2");
        assert!(queue.push_snapshot("frame-3"), "存在旧快照时可接纳新快照");
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.critical_len(), 1, "关键条目不得被淘汰");
        assert_eq!(
            queue.drain_all(),
            vec!["critical-1", "frame-2", "frame-3"],
            "被淘汰的应是最旧快照 frame-1"
        );
    }

    #[test]
    fn queue_is_fifo_and_drainable() {
        let mut queue = BoundedQueue::new(4);
        for item in 1..=3 {
            queue.push_critical(item).expect("未满应可入队");
        }
        assert_eq!(queue.drain_all(), vec![1, 2, 3]);
        assert!(queue.is_empty());
        assert_eq!(queue.dropped_snapshots(), 0);
    }

    #[test]
    #[should_panic(expected = "容量必须大于 0")]
    fn zero_capacity_is_rejected() {
        let _ = BoundedQueue::<u8>::new(0);
    }
}
