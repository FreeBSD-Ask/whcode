//! turn 循环集成验收（T211、Check 307、FR-718—724）。
//!
//! 与 `crates/whcode-app/src/turn.rs` 的模块测试互补：本文件从 **tests/ 口径**验证
//! 落库侧可观察事实——事件 seq 单调、`turn_id` 贯穿、messages 轮次可重建会话对话
//! （plan-p0-1 §2.6 的三项集成判据）；五种终止的分支矩阵与安全/资源断言在模块测试
//! 内覆盖（`turn::tests`），此处不重复。

use whcode_app::llm::{LlmContext, ProviderFactory, ProviderRegistry};
use whcode_app::turn::{TurnRequest, run_turn};
use whcode_app::{DefaultPolicy, NewSession, actor_with_all_capabilities, new_session};
use whcode_domain::WhError;
use whcode_llm::StubProvider;
use whcode_llm::http::profile::ProviderProfile;
use whcode_llm::provider::StubStep;
use whcode_store::apply_migrations;

struct StubFactory(StubProvider);

impl ProviderFactory for StubFactory {
    fn create(&self, _profile: &ProviderProfile) -> Result<Box<dyn whcode_llm::Provider>, WhError> {
        Ok(Box::new(self.0.clone()))
    }
}

fn fixture() -> (
    duckdb::Connection,
    String,
    std::path::PathBuf,
    ProviderRegistry,
    StubFactory,
) {
    let conn = duckdb::Connection::open_in_memory().expect("内存库");
    apply_migrations(&conn).expect("迁移");
    let id = new_session(
        &conn,
        &DefaultPolicy,
        &actor_with_all_capabilities("turn-itest"),
        NewSession {
            title: "turn 集成验收".to_string(),
            model: None,
        },
        1_700_000_000_000,
    )
    .expect("新建会话")
    .id;
    let profile: ProviderProfile = serde_json::from_str(
        r#"{"name":"p","protocol":"openai","base_url":"https://api.example.com",
            "model":"test-model","auth":{"type":"none"}}"#,
    )
    .expect("剖面");
    let registry = ProviderRegistry::from_profiles(vec![profile]).expect("注册表");
    let factory = StubFactory(StubProvider::new());
    let dir = std::env::temp_dir().join(format!(
        "whcode-turn-itest-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("临时目录");
    (conn, id, dir, registry, factory)
}

static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn request(session_id: &str, prompt: &str) -> TurnRequest {
    TurnRequest {
        session_id: session_id.to_string(),
        provider: "p".to_string(),
        prompt: prompt.to_string(),
        model: None,
        max_tokens: None,
        request_id: None,
        iterations_limit: None,
        budget_tokens: None,
        workspace_root: None,
        depth: 0,
    }
}

#[test]
fn turn_persists_monotonic_events_threaded_by_turn_id_and_rebuildable_messages() {
    let (mut conn, id, dir, registry, factory) = fixture();
    let context = LlmContext {
        registry: Some(&registry),
        registry_error: None,
        factory: &factory,
        cancel_dir: &dir,
    };
    let outcome = run_turn(
        &mut conn,
        &DefaultPolicy,
        &actor_with_all_capabilities("turn-itest"),
        &context,
        &request(&id, "集成验收提示词"),
        11,
    )
    .expect("turn 完成");
    assert_eq!(outcome.termination_reason, "model_finished");

    // 事件 seq 全会话单调递增，且每条事件的 turn_id 与信封一致（贯穿）。
    let mut stmt = conn
        .prepare(
            "SELECT seq, kind, CAST(CAST(payload_json AS JSON)->>'$.turn_id' AS VARCHAR)
             FROM events WHERE session_id = ? ORDER BY seq",
        )
        .expect("prepare");
    let mut rows = stmt.query([&id]).expect("query");
    let mut prev_seq = 0i64;
    let mut kinds = Vec::new();
    while let Some(row) = rows.next().expect("行") {
        let seq: i64 = row.get(0).expect("seq");
        let kind: String = row.get(1).expect("kind");
        let turn_id: Option<String> = row.get(2).expect("turn_id");
        assert!(seq > prev_seq, "seq 必须单调：{prev_seq} -> {seq}");
        prev_seq = seq;
        if kind != "ProgressChanged" {
            assert_eq!(
                turn_id.as_deref(),
                Some(outcome.turn_id.as_str()),
                "turn_id 必须贯穿（kind={kind}）"
            );
        }
        kinds.push(kind);
    }
    drop(stmt);
    assert_eq!(kinds.first().map(String::as_str), Some("RequestStarted"));
    assert_eq!(kinds.last().map(String::as_str), Some("RuntimeTerminated"));
    for expected in ["RequestStarted", "StreamingChunk", "RequestCompleted"] {
        assert!(kinds.iter().any(|k| k == expected), "缺少 {expected}");
    }

    // messages 轮次可重建会话对话：user 提示词 → assistant 回显，角色与顺序正确。
    let mut stmt = conn
        .prepare("SELECT role, content_ref FROM messages WHERE session_id = ? ORDER BY id")
        .expect("prepare");
    let mut rows = stmt.query([&id]).expect("query");
    let mut rebuilt: Vec<(String, String)> = Vec::new();
    while let Some(row) = rows.next().expect("行") {
        rebuilt.push((row.get(0).expect("role"), row.get(1).expect("content")));
    }
    drop(stmt);
    assert_eq!(
        rebuilt,
        vec![
            ("user".to_string(), "集成验收提示词".to_string()),
            ("assistant".to_string(), "集成验收提示词".to_string()),
        ],
        "messages 可重建对话（FR-722）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tool_call_executes_and_returns_result_to_model() {
    let (mut conn, id, dir, registry, _fixture_factory) = fixture();
    // 本用例需要工具剧本：替换夹具的回显 Provider；工作区绑定真实文件。
    let factory = StubFactory(StubProvider::with_script(vec![
        StubStep {
            chunks: vec!["调用".to_string()],
            finish_reason: "tool_call".to_string(),
            tool_calls: vec![whcode_llm::provider::LlmToolCall {
                id: "call-itest".to_string(),
                name: "read_file".to_string(),
                arguments: r#"{"path":"src/lib.rs"}"#.to_string(),
            }],
        },
        StubStep::simple("结果已回填", "stop"),
    ]));
    std::fs::create_dir_all(dir.join("src")).expect("工作区子目录");
    std::fs::write(dir.join("src/lib.rs"), "fn itest() {}\n").expect("工作区文件");
    let context = LlmContext {
        registry: Some(&registry),
        registry_error: None,
        factory: &factory,
        cancel_dir: &dir,
    };
    let mut req = request(&id, "读文件");
    req.workspace_root = Some(dir.clone());
    let outcome = run_turn(
        &mut conn,
        &DefaultPolicy,
        &actor_with_all_capabilities("turn-itest"),
        &context,
        &req,
        13,
    )
    .expect("turn 完成（工具执行后继续）");
    assert_eq!(outcome.termination_reason, "model_finished");
    assert_eq!(outcome.iterations, 2, "工具执行后进入第二轮");
    let exec_state: String = conn
        .query_row(
            "SELECT state FROM tool_executions WHERE tool_call_id='call-itest'",
            [],
            |row| row.get(0),
        )
        .expect("登记行");
    assert_eq!(exec_state, "completed", "三步登记终态（T215）");
    let tool_msg: String = conn
        .query_row(
            "SELECT content_ref FROM messages WHERE session_id=? AND role='tool'",
            [&id],
            |row| row.get(0),
        )
        .expect("tool 消息");
    assert!(tool_msg.contains("fn itest()"), "工具结果以 role:tool 回填");
    let _ = std::fs::remove_dir_all(&dir);
}
