# 安全加固复查记录（T123、任务书 §10；2026-10-24）

**范围**：脱敏默认、权限白名单、最小环境、审计保留策略、Web/SSH 边界（任务书 §10.1—10.3、R-11）。
**方法**：逐域回到一手证据（`文件:行号` + 自动化测试名），不以外部分析/惯例替代；每域给出
现状判定、威胁模型对应项、失效条件与剩余缺口。本记录是 Check 123 的判据载体。

## 1. 默认脱敏（§10.1）——**通过**

| 证据 | 位置 | 测试 |
|---|---|---|
| 追踪默认 `redacted`（配置默认值） | `crates/whcode-bin/src/config.rs`（`TraceLevel::Redacted` 默认，Check 16） | `config` 默认值用例 |
| 疑似秘密字段脱敏 + 完整性标记 | `crates/whcode-llm/src/trace.rs`（四档行为差异） | `redacted_masks_suspicious_fields_and_marks_integrity`（trace.rs:185）、四档差异用例（Check 58） |
| 日志路径脱敏（`sk-`/`Bearer`/`ghp_` + 敏感键名） | `crates/whcode-bin/src/logging.rs` | `log_path_never_emits_raw_secret`（logging.rs:229）等 7 例（Check 17） |
| 导出头部注明脱敏策略；事件导出自带 `trace_level` 与 `redacted` 标记 | `crates/whcode-app/src/events.rs` | `events_jsonl_first_line_is_header_with_policy`、`export_annotates_trace_policy_and_redacts_at_redacted_level`（Check 65） |

失效条件：`full` 追踪档是显式覆盖（UI 警告属 NFR-220 文案），脱敏标记 `IntegrityFlags` 可查询；
若新增凭据形态（新前缀/新通道），须同步 `CREDENTIAL_MARKERS` 与 `trace::sanitize`。

## 2. 权限白名单（§10.2 工具调用按白名单执行）——**通过**

| 证据 | 位置 | 测试 |
|---|---|---|
| 能力枚举 + `Policy` trait + `require` 统一判定 | `crates/whcode-app/src/authz.rs:84`（trait）、`:118`（require） | `denial_is_permission_denied_and_non_retryable`（Check 50） |
| 四端同源错误码（permission_denied 不可重试） | 契约 `tests/contract/test_tui_cui_parity.rs` | `permission_semantics_identical_for_both_frontends`、`error_codes_are_frontend_agnostic` |
| Web 写路由经应用层判权（弱 Actor → 403） | `crates/whcode-web/src/api.rs`（写命令统一入口两道检查：auth + origin） | `write_routes_reject_bad_bodies_and_missing_capability` |
| 运行时能力诚实门控（未启用不注册、不可用结构化拒绝） | `crates/whcode-runtime/src/errors.rs` | `unavailable_runtimes_block_execution_before_start`（Check 107） |

失效条件：新增能力必须同时更新 `authz` 白名单与契约测试；绕过应用层直连 store 的路径由
分层契约（`frontends_do_not_depend_on_store`）持续拦截。

## 3. 最小环境（§10.2 子进程默认继承最小化集合）——**通过（本轮补齐）**

| 证据 | 位置 | 测试 |
|---|---|---|
| 最小环境白名单（8 项：PATH/PATHEXT/SystemRoot/SystemDrive/COMSPEC/TEMP/TMP/NUMBER_OF_PROCESSORS），`env_clear` 禁用完整继承后按白名单回填 | `crates/whcode-runtime/src/worker.rs:172`（`MINIMAL_ENV_ALLOWLIST`）、`:188`（`apply_minimal_env`） | `minimal_env_converges_to_allowlist_plus_explicit`（worker.rs:456） |
| 真实子进程验证：非白名单父变量（USERPROFILE）不继承、显式变量保留、`WHCODE_` 前缀永不透传 | `execute_with_cancel` 固定次序：`apply_minimal_env` → `strip_credentials` | `child_sees_only_allowlist_and_explicit_vars`（worker.rs:487） |
| 凭据剥离（第二层，先于最小环境之前已交付） | `worker.rs` `strip_credentials` | `strip_credentials_removes_credentials_from_child`（Check 144，含未剥离阳性对照） |

威胁模型对应：R-11（Web 或工具执行泄漏密钥）——子进程不再拿到父环境的凭据、代理与配置变量。
失效条件：白名单调整必须在 ADR 记录原因，不得改回完整继承；显式 `Command::env` 的变量仍经
凭据特征过滤。**剩余缺口（显式标记）**：OS 级 CPU/内存配额未实现（T104 同口径，“并发子进程
上限”不承诺配额）；工具调用的文件/网络白名单策略属工具注册表面（M8 装配），当前运行时
provider 仅 ShellOnly/Restricted 档位并已列明不支持项。

## 4. 审计保留策略（§10.3）——**通过（默认策略有测试；显式保留策略显式标记未实现）**

| 证据 | 位置 | 测试 |
|---|---|---|
| 审计不随会话 TTL 清除：清理只删会话树，既有审计行原样保留（+ 清理自身 `db_purge` 审计同事务写入） | `crates/whcode-store/src/purge.rs:840` | `audit_records_survive_purge_and_export_excludes_them` |
| 导出/导离面不携带审计（`EXCLUDED_TABLES = [audit, runtime_state, schema_migrations]`） | `crates/whcode-store/src/export.rs:70` | 同上用例的导出断言 |
| 必审计动作落点：越过门禁/修改 Constitution（`gates.rs` force_advance 审计）、清理（`purge.rs:691`）、压缩（`compact.rs`）、数据导出（`export.rs:449`） | 各写路径 | `force_advance_requires_reason_and_audits_full_elements`（Check 78）、`purge` 审计断言（Check 208） |

默认策略（显式声明，非隐含）：**审计行永不随会话 TTL 自动清除**（任务书 §10.3 的默认要求）。
**未实现（显式标记）**：管理员显式设置审计保留期限的配置面（任务书允许“除非管理员明确设置”）——
当前无任何代码路径删除审计行；需要时必须经 ADR（删除是不可逆动作，须写明保留期默认值与
退出条件）。复审触发：审计表增长纳入 `db stats` 观测，若成为空间主项再立保留策略 ADR。

## 5. Web/SSH 边界（§10.2）——**通过（Web 有实测；SSH 服务端属 M7 延后，边界声明如实）**

Web（全部有自动化测试 + 实测）：
- 默认回环监听；外部绑定未开认证/Origin 拒绝启动（`bind.rs`；Check 91，`v110_default_loopback_and_external_requires_auth`）。
- token 不经 URL query（`security.rs::token_in_query`）；Bearer + 恒定时间比较（`check_auth`）。
- Origin 校验（scheme+host、端口不参与、缺失 Origin 的放行口径 = ADR-022/023 修订）。
- WS：认证/Origin/消息大小上限（HTTP 与 WS 帧双层 1 MiB）；广播面新增边界——订阅者 ≤32
  （超限 close 1013）、每客户端出站 ≤64 帧、慢客户端断开不停别人、>1 MiB 帧 close 1009 只断
  发送者（`broadcast.rs` + `serve.rs`，Check 199/209 socket 级测试）。
- 局域网边界与威胁模型：ADR-023（默认拒绝对外监听；放宽须显式认证 + ADR）。

SSH（如实声明，不声称已验收）：
- 客户端原型安全边界已测：仅公钥认证、受限命令、输出上限、超时、意图—执行—登记（`proto.rs` 9 例，Check 218）。
- **服务端未实现**（M7/T096—T099，ADR-013 延后）；ADR-005 阻断项在案：主机密钥校验补齐前，
  客户端原型不得表述为可连接受信网络之外的目标；默认无任意系统 Shell 的判据属服务端（Check 97/182 维持未勾选，不因本复查虚勾）。

## 结论

五域逐项通过（SSH 服务端以“未实现且如实登记”的形态通过复查——复查判定的是**边界声明与
证据一致**，不是功能已交付）。本轮补齐两处缺口：最小环境白名单（代码 + 2 测试）、审计保留
默认策略测试；登记 2 项显式缺口：OS 级配额（T104 口径）、审计显式保留策略配置面。
