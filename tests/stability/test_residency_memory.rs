//! # 卸载内存可观测下降（Check 221、FR-614）
//!
//! 两段证据：
//! 1. **占用统计**（确定性）：常驻缓冲有界，卸载后 `accounted_bytes()` 归零，唤醒不预分配；
//! 2. **进程工作集**（实测）：装载约 30 MiB 的会话事件缓冲后卸载全部会话，
//!    经 `whcode_platform::current_rss_bytes` 读取工作集，断言其可观测下降。
//!
//! 平台边界：非 Windows 的 RSS 读取返回结构化错误（M7 延后），此时用例显式打印
//! “未执行 RSS 断言”并只跑占用统计，不产生伪装通过。

use std::time::Duration;

use whcode_app::residency::{ResidencyManager, ResidencyState};
use whcode_platform::current_rss_bytes;
use whcode_store::query::EventRow;

/// 会话数（每会话一条大载荷事件，合计约 30 MiB）。
const SESSIONS: usize = 128;
/// 单条事件载荷字节数（低于单会话 256 KiB 上限）。
const PAYLOAD_BYTES: usize = 240 * 1024;
/// 工作集下降的最低要求（MiB）：远小于理论 30 MiB，容忍分配器与后台活动。
const MIN_RSS_DROP_MIB: u64 = 8;

fn event(seq: i64, session_id: &str, payload_bytes: usize) -> EventRow {
    EventRow {
        seq,
        id: format!("E-{seq}"),
        session_id: Some(session_id.to_string()),
        task_id: None,
        request_id: None,
        tool_call_id: None,
        kind: "StreamingChunk".to_string(),
        payload_json: "x".repeat(payload_bytes),
        ts_us: seq * 1_000,
        persistence_level: "redacted".to_string(),
    }
}

/// Check 221：空闲会话卸载后内存可观测下降；唤醒后从空缓冲开始（不重复不缺失的
/// 补拉语义由 `whcode-app::residency` 的存储级用例覆盖）。
#[test]
fn unloading_idle_sessions_observably_drops_memory() {
    let mut manager = ResidencyManager::new();
    for index in 0..SESSIONS {
        let session = format!("S-{index}");
        manager.wake(&session);
        manager
            .buffer_event(event(index as i64 + 1, &session, PAYLOAD_BYTES))
            .expect("缓冲未超限");
    }
    let accounted = manager.accounted_bytes();
    assert!(
        accounted >= SESSIONS * PAYLOAD_BYTES / 2,
        "占用统计应反映装载量（accounted={accounted}）"
    );

    match current_rss_bytes() {
        Ok(before) => {
            for index in 0..SESSIONS {
                let session = format!("S-{index}");
                assert_eq!(manager.state(&session), ResidencyState::Resident);
                manager.unload(&session, index as i64 + 1, 1_000);
            }
            assert_eq!(manager.accounted_bytes(), 0, "卸载后占用必须归零");
            // 等一小段时间让工作集回落稳定（释放是同步的，测量有采样延迟）。
            std::thread::sleep(Duration::from_millis(200));
            let after = current_rss_bytes().expect("工作集复测");
            let dropped = before.saturating_sub(after);
            eprintln!(
                "卸载内存观测：accounted={accounted} 字节，RSS before={before} 字节，after={after} 字节，dropped={dropped} 字节"
            );
            assert!(
                dropped >= MIN_RSS_DROP_MIB * 1024 * 1024,
                "卸载应使工作集可观测下降：before={before} after={after} dropped={dropped} accounted={accounted}"
            );

            // 唤醒：占用仍为 0（不预分配大缓冲），状态可查询。
            for index in 0..SESSIONS {
                let session = format!("S-{index}");
                manager.wake(&session);
                assert_eq!(manager.state(&session), ResidencyState::Resident);
            }
            assert_eq!(manager.accounted_bytes(), 0, "唤醒不得预分配缓冲");
        }
        Err(err) => {
            eprintln!("未执行 RSS 断言（平台未实现工作集读取，M7 延后）：{err}");
            for index in 0..SESSIONS {
                let session = format!("S-{index}");
                manager.unload(&session, index as i64 + 1, 1_000);
            }
            assert_eq!(manager.accounted_bytes(), 0, "卸载后占用必须归零");
        }
    }
}
