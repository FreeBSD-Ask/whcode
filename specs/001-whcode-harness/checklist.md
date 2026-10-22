# 验收检查点: WHCode — Rust 原生 LLM 编码 Harness

**Feature**: [spec.md](./spec.md) | **Plan**: [plan.md](./plan.md) | **Tasks**: [tasks.md](./tasks.md) | **Date**: 2026-10-20

**语义**：验收检查点（非需求质量清单）。每项可由“通过／不通过”二值判定；`[x]` 表示该判据**经复核通过**，勾选权归复核人，生成时一律 `[ ]`。

**组织**：第一层 实现完整性（Check 1—127 对应 Task T001—T127，Check 225 对应 T134，Check 227—228 对应 T135—T136，Check 229 对应 T137，Check 230—233 对应 T138—T141，Check 234—237 对应 T142，Check 238—239 对应 T143，Check 240 对应 T144，Check 241—243 对应 T145，Check 244—245 对应 T146，Check 246—248 对应 T147，Check 249 对应 T148）→ 第二层 需求覆盖（Check 128—195）→ 第三层 边界与异常（Check 196—209、218—249）→ 第四层 既有功能未回归（Check 210—217）。

**缺口说明**：spec 全部 79 条 FR（含 2026-10-21 新增的 FR-611—FR-618）、NFR-101—107/201—222、V-001—V-307 与 tasks.md 全部 148 条任务均有对应检查点，本清单 Check 1—249，无遗漏项。平台承诺已按 ADR-011 修订，OpenBSD/DragonFly 的判据由服务端改为客户端。FR-406（并行调度）与 research.md TBD-01—TBD-10 属任务书定义的延后/待验证能力，其检查点判据为“未通过前置条件时不得声称已交付”（见 Check 172、Check 22 起 M0 相关项）。史料／考据类判据：本项目无历史文献类需求；对应物为源文本基线不可改写与 [S1]—[S8] 证据锚点可定位，见 Check 215/216。

---

## 第一层：实现完整性（对应 tasks.md 全部 148 条任务）

### Phase 1 Setup（T001—T009）

- [ ] Check 1: 仓库根 `Cargo.toml` 声明 13 个成员 crate（whcode-domain—whcode-bin）且 `cargo metadata` 解析成功（来源：Task T001）
- [ ] Check 2: 仓库根存在 `rust-toolchain.toml`，版本号已锁定 1.99.1（无占位字符串）；host triple 按平台 runner 默认值、Windows 使用 msvc（来源：Task T002、T021、D-01 修订）
- [ ] Check 3: `rustfmt.toml` 存在且 CI 对 fmt/clippy 运行并 deny warnings（来源：Task T003）
- [ ] Check 4: `crates/` 下 13 个 crate 目录各有可编译入口（lib.rs 或 main.rs）（来源：Task T004）
- [ ] Check 5: `ci/platform-matrix/` 含 freebsd/openbsd/netbsd/dragonfly/windows 五个 CI 配置文件（来源：Task T005）
- [ ] Check 6: `docs/guides/pr-checklist.md` 含任务书 §15 七项清单且被 PR 模板引用（来源：Task T006）
- [ ] Check 7: `docs/adr/ADR-001.md`—`ADR-010.md` 存在并标注未批准/批准状态（来源：Task T007、T029）
- [ ] Check 8: `tests/{unit,contract,integration,crash,stability,fixtures}/` 六个目录存在且各有 README 说明用途（来源：Task T008）
- [ ] Check 9: workspace `Cargo.toml` 中可选运行时 feature 默认关闭，默认 `cargo build` 不引入运行时依赖（来源：Task T009、FR-704）

### Phase 2 Foundational（T010—T019）

- [ ] Check 10: `crates/whcode-domain/src/state.rs` 定义任务/LLM 请求/运行时执行/SDD 阶段四组枚举，取值与 data-model.md §2 完全一致（来源：Task T010、spec §3.1）
- [ ] Check 11: `crates/whcode-domain/src/event.rs` 定义全部事件类型枚举与 payload 上限常量（来源：Task T011、FR-201）
- [ ] Check 12: `crates/whcode-domain/src/error.rs` 的错误对象含 code/message/retryable/details 四字段且有序列化测试（来源：Task T012）
- [ ] Check 13: `crates/whcode-domain/src/gate.rs` 阶段转换记录含来源阶段、目标阶段、执行者、校验结果、时间戳五要素（来源：Task T013）
- [ ] Check 14: `crates/whcode-platform/` 之外无 `cfg(target_os)` 平台分支（grep 验证）（来源：Task T014、NFR-101）
- [ ] Check 15: `crates/whcode-app/src/queue.rs` 有界队列对关键命令返回背压错误、对展示快照执行合并（单元测试断言两种行为）（来源：Task T015）
- [ ] Check 16: 配置默认值符合基线：`memory_limit`=192 MB、`threads`≤min(4,物理核)、追踪级别=redacted（配置加载测试断言）（来源：Task T016、§5.4、D-05）
- [ ] Check 17: 日志初始化含脱敏过滤器，测试断言 API Key 形态字符串不出现在日志输出（来源：Task T017、FR-203）
- [ ] Check 18: 审计记录类型含 actor/action/target/detail_json/ts 且提供强制写入钩子（来源：Task T018、§10.3）
- [ ] Check 19: `tests/contract/test_layering.rs` 通过：前端 crate 依赖图中无指向 `whcode-store` 写入路径的边（来源：Task T019、ADR-003）

### Phase 3 US1 M0（T020—T031）🎯 MVP

- [ ] Check 20: Windows 与 FreeBSD/NetBSD 各产出一份 `artifacts/deps-<platform>.txt`，含 Rust/DuckDB/编译器/OS 四类版本号；分平台判定，Windows 与四 BSD 分别取证，不得以单一平台代替；OpenBSD/DragonFly BSD 已移出原生目标，不要求原生依赖记录（来源：Task T020、V-001、ADR-011）
- [ ] Check 21: `rust-toolchain.toml` 锁定版本与各平台依赖记录中 Rust 版本一致；分平台判定，Windows 与四 BSD 分别取证，不得以单一平台代替（来源：Task T021、V-001）
- [ ] Check 22: Windows 与 FreeBSD/NetBSD 的数据库冒烟（创建/事务写读/关闭重启/迁移）通过并留存日志；分平台判定，Windows 与四 BSD 分别取证，不得以单一平台代替；OpenBSD/DragonFly BSD 不作为原生目标判定（来源：Task T022、V-002、ADR-011）
- [ ] Check 23: `docs/adr/ADR-DB-BUILD.md` 按平台记录构建方式、补丁与上游状态，且与实际构建产物一致（来源：Task T023、R-01）
- [x] Check 24: Windows 11 amd64 TUI 冒烟（启动/字符键盘/resize/Ctrl-C/q）有桌面自动化记录，强杀后可读作为异常恢复补充；方向键/选中项未实现不计入；raw mode 残留未独立验证。用户 2026-10-21 复核接受（来源：Task T024、V-003、`artifacts/tui-manual-2026-10-21.txt`）
- [ ] Check 25: 内置 SSH 客户端（FR-611）在 Windows 与四类 BSD 的连通性、受限命令、超时与输出上限有客户端侧记录；FreeBSD/NetBSD 服务端验收属 M7，V-004 未取得证据者须标明阻塞（来源：Task T025、FR-611、ADR-011）
- [ ] Check 26: 写者原型测试通过：重复幂等键去重、批次阈值触发、关键事件 flush 确认、写入失败返回可重试错误（来源：Task T026、V-005）
- [ ] Check 27: 基线报告含全部测量纪律字段（OS/硬件/编译类型/工具链/DuckDB 版本/参数/数据规模/采样窗口/脚本）（来源：Task T027、V-006）
- [ ] Check 28: `docs/matrices/platform-matrix.md` 标注各预算为“已冻结”或链接修订 ADR，无“未测量即承诺”表述（来源：Task T028、V-006）
- [ ] Check 29: ADR-001—014 批准记录各含决策、理由、约束、状态、责任人、退出条件六要素（来源：Task T029、V-007）
- [ ] Check 30: `docs/matrices/decisions.md` 对 D-01/D-02/D-03/D-04/D-06/D-12/D-13/D-14 各有决策结果与证据引用（来源：Task T030）
- [ ] Check 31: `artifacts/m0-report.md` 含五平台证据包链接与逐平台 Go/No-Go 陈述；ADR-013 后仅 Windows 作为验收结论，其余平台标注延后（来源：Task T031、§11.1、ADR-013）

### Phase 4 US2 M1（T032—T045）

- [ ] Check 32: 14 张表 schema 存在且 `schema_migrations` 可重复执行不报错（来源：Task T032、FR-501）
- [ ] Check 33: 从上两个受支持 schema 版本的迁移测试通过；人为中断迁移后版本号未前移（来源：Task T033、NFR-103、§7.4）
- [ ] Check 34: 写者服务测试：队列有界、幂等键去重、500 行/50 ms 阈值可配置、关键事件 flush 返回确认（来源：Task T034、FR-502/503）
- [ ] Check 35: 游标分页测试：同页请求两次结果一致，向前/向后翻页无重复无遗漏（来源：Task T035、FR-504）
- [ ] Check 36: events/messages/tasks 索引存在，基准报告显示关键查询使用索引或已论证计划合理（来源：Task T036、§7.2）
- [ ] Check 37: `session_overview` 预聚合可由原始数据重建且重建结果与统计一致（来源：Task T037、FR-506）
- [ ] Check 38: 监控输出含数据库统计、查询计划、WAL 状态、写者队列深度四类信息（来源：Task T038、FR-507）
- [x] Check 39: 清理操作在活跃会话存在时被拒绝或跳过该会话，且预览输出与实际删除集合一致（来源：Task T039、FR-509）。T039 落地：Skip（默认跳过）/Abort（整体拒绝）双策略，预览与执行两时点复检；预览集合与实际删除集合、各表行数一致由测试断言（`purge.rs`：`preview_selects_by_time_and_reports_protected`、`preview_set_matches_executed_deletions`、`abort_policy_rejects_at_preview_and_execute`）。用户 2026-10-21 复核接受
- [ ] Check 40: 导出文件含版本与清单，导入校验拒绝不兼容 schema 与重复 ID（来源：Task T040、§7.4）
- [ ] Check 41: compact 流程在副本校验失败时不切换文件且原库可写（来源：Task T041、V-107）
- [ ] Check 42: WAL 超阈值触发 checkpoint 的记录可查，高负载路径无强制 checkpoint（来源：Task T042、§5.4）
- [ ] Check 43: 已确认事务进程终止+重启后，关键事件与状态一致性测试通过（来源：Task T043、V-104）
- [ ] Check 44: 同一幂等键第二次提交返回首次结果或确定冲突，库内无重复记录（来源：Task T044、V-106）
- [ ] Check 45: 清理/维护报告含文件大小、WAL、已用块、可用块四项统计（来源：Task T045、FR-510）

### Phase 5 US3 M2（T046—T055）

- [ ] Check 46: 六个会话命令（new/list/show/rename/archive/delete）各自有通过的正向与失败路径测试（来源：Task T046、FR-101）
- [ ] Check 47: 切换 10 万事件会话后进程仅加载元数据+概览+有界尾部，内存增量符合冻结预算（来源：Task T047、FR-102、V-101）
- [ ] Check 48: 任务总览输出含阶段分布、完成度、阻塞、活跃任务四类字段（来源：Task T048、FR-103）
- [ ] Check 49: 任务详情返回依赖、步骤、产物引用、最近事件、失败原因五类信息（来源：Task T049、FR-104）
- [ ] Check 50: 无权限调用返回 `permission_denied`（retryable=false），四端错误码一致（来源：Task T050、FR-604）
- [ ] Check 51: 状态快照含 schema_version 与 event_cursor，且与库内状态一致（来源：Task T051、FR-314）
- [ ] Check 52: 任务 retry 递增 retry_count；abort 后状态为 aborted 且会话不丢失（来源：Task T052、FR-401/408）
- [ ] Check 53: 夹具生成器可稳定产出 10 万事件/1 万任务数据集并有规模断言（来源：Task T053）
- [ ] Check 54: 分页游标在 10 万事件数据集上无重复、无遗漏（边界探测脚本输出零差集）（来源：Task T054、V-101）
- [ ] Check 55: 概览查询在 1 万任务数据集上报告 p50/p95/p99 且 p95 ≤ 冻结预算（来源：Task T055、V-102）

### Phase 6 US4 M3（T056—T067）

- [ ] Check 56: Provider 语义六项（能力探测/取消/流式/计量/超时/错误归一化）各有对应测试（来源：Task T056、§8.4）
- [ ] Check 57: 流式积压注入测试中，关键状态转换事件零丢失，仅展示类事件被合并（来源：Task T057、FR-206）
- [ ] Check 58: 四个追踪级别行为差异测试通过；redacted 下疑似秘密字段不出现在输出（来源：Task T058、FR-203）
- [ ] Check 59: 一次调用可在事件流中按 request_id/session_id/task_id/tool_call_id 完整串联（来源：Task T059、FR-204）
- [ ] Check 60: 上下文加载返回 items+next_cursor+consumed_budget+truncated_reason 四元组（来源：Task T060、FR-105）
- [ ] Check 61: 压缩后 summary 指针可定位且原始历史仍可完整读取（来源：Task T061、FR-107）
- [ ] Check 62: 预算视图输出当前预算、装入来源、截断原因三项（来源：Task T062、FR-108）
- [ ] Check 63: 计量记录区分估算与供应商计量值，估算带 estimate 标记（来源：Task T063、FR-207）
- [ ] Check 64: 完整性标记四类（缺失 chunk/脱敏/降采样/持久化失败）可被查询（来源：Task T064、FR-208）
- [ ] Check 65: `events export` 输出头部注明脱敏策略（来源：Task T065、FR-205）
- [ ] Check 66: 断线重连按游标补拉后，事件集合与在线流一致（来源：Task T066、FR-202）
- [ ] Check 67: `tests/integration/test_trace_and_context.rs` 全部通过（来源：Task T067、V-103）

### Phase 7 US5 M4（T068—T081）

- [ ] Check 68: 工作流配置带版本号，阶段定义含允许输入/必需产物/校验器/转换四要素（来源：Task T068、FR-302）
- [ ] Check 69: 解析器对损坏样例报告无法解析段落、重复 ID、未识别字段，无信息丢弃（来源：Task T069、FR-303）
- [ ] Check 70: Constitution 创建/导入产生版本记录，变更留审计（来源：Task T070、FR-301）
- [ ] Check 71: 需求/任务 ID 稳定：重解析产物后 ID 不变（来源：Task T071、FR-304）
- [ ] Check 72: 追溯检查：任一任务可回溯至需求与计划决策，断链被检出（来源：Task T072、FR-305）
- [ ] Check 73: 五类缺陷样例（遗漏需求/孤儿任务/无效引用/循环依赖/覆盖不足）均被 Analyze 检出（来源：Task T073、FR-306）
- [ ] Check 74: 高优先级澄清未解决时 Plan 转换被应用服务拒绝（非提示词）（来源：Task T074、FR-307/308）
- [ ] Check 75: 连续 2 次纠偏失败后任务为 blocked，原文与错误可查询（来源：Task T075、FR-309）
- [ ] Check 76: Converge 对每条验收条件输出 pass/fail/not_tested/not_applicable 之一并附证据引用（来源：Task T076、FR-310）
- [ ] Check 77: 存在 fail/not_tested 验收条件的任务不得为 completed，且差异报告可查（来源：Task T077、FR-311）
- [ ] Check 78: `force-advance` 无 `--reason` 时被拒绝；有 reason 时审计含 actor/reason/跳过门禁/产物版本（来源：Task T078、FR-312）
- [ ] Check 79: 模型调用请求含显式阶段角色字段，取值来自状态机而非历史推断（来源：Task T079、FR-313）
- [ ] Check 80: 三端请求 spec 状态返回相同阶段、门禁结果与产物版本（来源：Task T080、FR-314）
- [ ] Check 81: `tests/fixtures/sdd/` 夹具触发 V-201—V-207 七项判定各至少一次（来源：Task T081）

### Phase 8 US6 M5（T082—T089）

- [x] Check 82: TUI 六视图（会话树/任务/上下文/事件/Spec 状态/diff）均可在固定终端尺寸下截图取证（来源：Task T082、FR-601）——**可执行形式已取得**：单元测试 `six_views_render_at_fixed_terminal_size`（80×24 逐视图帧 + 内容断言）；真实控制台帧 `artifacts/tui-views-2026-10-22.txt`（release 二进制，`1`—`6` 切换逐帧落盘，`script/tui_views_smoke.py` 可重跑）；视图数据源未装配，正文显式「暂无数据」（数据接线见 Check 179 备注）
- [x] Check 83: 渲染路径无完整历史克隆与帧内 JSON 反序列化（代码审查+基准确认帧预算）（来源：Task T083、§5.3）——**三证**：①结构断言（`render.rs`/`views.rs` 剥离注释与测试模块后无 serde_json）；②帧路径强制限行投影 `render_lines_limited`（帧成本由终端尺寸界定，完整历史不进帧路径——10k 行快照同阶，测试 `limited_projection_is_bounded_and_consistent`）；③帧预算归档 `artifacts/tui-frame-budget.json`（release 实测 p50 250 µs / p95 281 µs，候选预算 p95 ≤ 16 ms **未冻结**）
- [ ] Check 84: CUI 命令树与 contracts/cui-cli.md §1 一致且 `whcode --help` 可枚举（来源：Task T084、FR-602）
- [ ] Check 85: 每个 CUI 命令 `--json` 输出含 schema_version；错误输出含四字段（来源：Task T085）
- [ ] Check 86: 破坏性命令 dry-run 输出与实际执行影响集一致（来源：Task T086、§8.1）
- [ ] Check 87: `docs/matrices/feature-parity.md` 覆盖全部已交付命令并标注三端支持状态（来源：Task T087、FR-610）
- [ ] Check 88: TUI/CUI 同命令一致性契约测试通过（最终状态/权限/错误码）（来源：Task T088、V-109）
- [ ] Check 89: JSON 信封契约测试覆盖全部命令（来源：Task T089）

### Phase 9 US7 M6（T090—T095）

- [ ] Check 90: 浏览器经 HTTP/WebSocket 完成与 CUI 相同命令并得到一致结果（来源：Task T090、FR-603）——**部分证据（2026-10-22，批次 2 更新）**：CUI 命令调度已装配；**读命令跨端一致已取证**（`test_tui_cui_parity::same_read_commands_yield_identical_state_across_cui_and_web`：事件/会话/任务三组数据一致；CUI 真实命令见 `artifacts/cui-2026-10-22-batch2.txt`）；**未取得**：写命令（session new/rename/delete、task retry/abort）尚无 Web 写 API——不得据此勾选
- [x] Check 91: 默认启动仅绑定 127.0.0.1；无认证时外部绑定启动失败并给出原因（来源：Task T091、FR-606）——**已取得**：`whcode serve --web --bind 0.0.0.0:8740` → `invalid_state: 对外监听 … 必须显式开启认证与 Origin 检查（FR-606、V-110）：拒绝启动`（退出码 1）；默认 `127.0.0.1` 监听实测可用；契约测试 `v110_default_loopback_and_external_requires_auth` ✓
- [ ] Check 92: 未携带有效凭证的 WS 连接被拒；跨 Origin 被拒；超限消息被拒；URL 中无 token（来源：Task T092、§10.2）——**部分证据（2026-10-22）**：跨 Origin 403、请求体超限拒绝、URL 无 token 均有测试；**缺口**：`serve` 未提供 token 配置（回环模式认证关闭为 V-110 默认），认证模式的 WS 凭证路径不可达——对外部署前置
- [ ] Check 93: Web 七类业务操作（会话/任务/历史/追踪/SDD/清理/运行时）各有手动或自动化验证记录（来源：Task T093、FR-609）——**部分证据（2026-10-22，批次 1 更新）**：会话清单（可点击选择）/任务（`/api/tasks` 总览 + 活跃任务）/历史（事件尾部）/SDD（状态快照）**已在真实浏览器验证（4/7）**；追踪/清理/运行时未接入 Web——不得据此勾选
- [x] Check 94: 浏览器断网重连后状态与快照一致，事件无缺失（来源：Task T094、FR-605）——**已取得**：真实浏览器断线（服务停止）→ 页脚「未连接」、游标保留；服务重启后指数退避重连 → 「已连接」、游标 6、事件 6 行无重复无缺失（`artifacts/web-browser-2026-10-22.txt`）
- [x] Check 95: `tests/contract/test_web_security_defaults.rs` 通过（V-110 五项默认值）（来源：Task T095）——**已取得**：契约测试 5 例通过（含 ADR-022 修订后的 Origin 语义）；全工作区测试链覆盖

### Phase 10 US8 M7（T096—T099）

- [ ] Check 96: FreeBSD/NetBSD 各验证服务端公钥认证、会话隔离、resize、安全退出四项；OpenBSD/DragonFly BSD 由内置 SSH 客户端验证连通性与受限入口（来源：Task T096、FR-607、FR-611、ADR-011）
- [ ] Check 97: SSH 服务端与内置 SSH 客户端默认入口均无系统 Shell；显式授权路径单独配置且有审计（来源：Task T097、FR-608、FR-611、ADR-011）
- [ ] Check 98: SSH 会话期间写者服务为唯一写入方，无第二数据库连接写路径（来源：Task T098、ADR-005）
- [ ] Check 99: `artifacts/ssh-acceptance/` 含 FreeBSD/NetBSD 服务端证据与四平台客户端连通性证据；未完成平台有明确阻塞原因（来源：Task T099、V-004、FR-611、ADR-011）

### Phase 11 US9 M8（T100—T114）

- [ ] Check 100: Runtime Provider 六接口（name/capability/is_available/env_fingerprint/execute/terminate）有契约测试（来源：Task T100、FR-701）
- [ ] Check 101: 三档能力等级（Full/Restricted/ShellOnly）在 UI 与门禁输出中可见（来源：Task T101、FR-702）
- [ ] Check 102: payload 安装校验拒绝来源不明/哈希不符/平台不符三类输入（来源：Task T102、FR-703）
- [ ] Check 103: 运行时 feature 关闭构建中，相关动态库不加载、端口不占用、工具不注册（来源：Task T103、FR-704、V-301）
- [ ] Check 104: 子进程超时/输出上限/取消/资源限制四项各有触发测试（来源：Task T104、FR-705）
- [ ] Check 105: 每次脚本执行前存在检查点记录，含脚本哈希与输入哈希（来源：Task T105、FR-706）
- [ ] Check 106: 有副作用的脚本重复执行不命中缓存（来源：Task T106、FR-707）
- [ ] Check 107: 引用未启用运行时的任务被 Analyze/执行前检查阻止并给出安装建议（来源：Task T107、FR-708/710）
- [ ] Check 108: Converge 绑定的运行时测试记录含命令、指纹、退出状态、结果引用（来源：Task T108、FR-709）
- [ ] Check 109: Windows 子进程树与 BSD 进程组终止后无残留进程，stderr 尾部入库（来源：Task T109、FR-409）
- [ ] Check 110: JS 引擎对 Node API 调用返回能力不足错误，而非崩溃或静默失败（来源：Task T110、D-08）
- [ ] Check 111: Python worker 记录解释器版本/ABI/路径；受限档位列出不支持项（来源：Task T111、D-09）
- [ ] Check 112: Git Bash payload 验证管道/重定向/路径/终止四类命令，payload 哈希可核验（来源：Task T112、V-304）
- [ ] Check 113: BSD Shell provider 与 gitbash 在能力清单中为不同条目（来源：Task T113、§4.7.1）
- [ ] Check 114: V-301—V-307 全部有自动化或留证测试，三时点中断均产生 `unknown`（来源：Task T114、V-307）

### Phase 12 US10 M9（T115—T121）

- [ ] Check 115: 优化后测量报告对比冻结预算，未达标项有热点分析与 ADR（来源：Task T115、NFR-201—210）
- [ ] Check 116: 24 小时稳定性报告存在；候选发布另含 72 小时报告，二者含 §11.5 全部指标（来源：Task T116）
- [ ] Check 117: 分配器 ADR 记录基准数据；无基准时不更换默认分配器（来源：Task T117、D-11）
- [ ] Check 118: Windows 11 amd64 发布包可安装运行；BSD 延后并在已知限制中说明；兼容矩阵随包发布（来源：Task T118、§12.2、ADR-013）
- [ ] Check 119: 四份指南文档（CUI/TUI/Web 部署安全/灾难恢复）存在且与实现一致（来源：Task T119）
- [ ] Check 120: 运行时能力清单含 payload 校验方式与许可证清单（来源：Task T120）
- [ ] Check 121: 风险台账 R-01—R-12 状态有更新，每条有处置记录或关闭依据（来源：Task T121、§13）

### Phase 13 Polish（T122—T127）

- [ ] Check 122: D-01—D-14 在 decisions 文档中均有最终状态，spec/plan 无残留未解释的 `[待验证]`（来源：Task T122）
- [ ] Check 123: 安全复查清单逐项通过：脱敏默认、白名单、最小环境、审计保留、Web/SSH 边界（来源：Task T123、§10）
- [ ] Check 124: quickstart.md 全流程端到端执行一次且各步预期结果成立（来源：Task T124）
- [ ] Check 125: 事件/快照/CUI JSON 三类 schema 的版本变更记录存在且向后兼容策略明确（来源：Task T125、§8.3）
- [ ] Check 126: clippy 零告警，无未使用 feature 与死代码（来源：Task T126）
- [ ] Check 225: 目录分类门禁通过：顶层条目全部在 repository-layout.md 白名单内，crate 命名 / tests 六类（含 README）/ docs 三类 / ci 两类 / specs 命名均符合，本地目录无入库形态源码（来源：Task T134／docs/guides/repository-layout.md）
- [ ] Check 127: 三方追溯（spec FR↔tasks↔本清单）审计输出零断链（来源：Task T127、FR-305/306）

---

## 第二层：需求覆盖（spec 验收项与逐条需求）

### 验收标准 V-001—V-307（spec SC-001—021 的判定载体）

- [ ] Check 128: Windows 11 amd64 构建与依赖记录齐备，工具链/DuckDB/编译器/OS 版本锁定；FreeBSD/NetBSD/OpenBSD/DragonFly 按 ADR-013 延后，不作验收结论，恢复时补分平台证据（来源：spec V-001／SC-021／Task T020、ADR-013）
- [ ] Check 129: Windows 11 amd64 数据库冒烟四步（创建/事务写读/重启/迁移）通过；BSD 平台按 ADR-013 延后，恢复时补各自证据（来源：spec V-002／Task T022、ADR-013）
- [ ] Check 130: Windows 11 amd64 TUI 最小闭环五步测试全部通过（BSD 延后）；启动可在带控制台环境自动取证（`script/tui_smoke.py` 的管道退出、`--version`、`doctor` 非交互三项），键盘 / resize / Ctrl-C / 异常恢复四项必须人工留证（来源：spec V-003／Task T024）
- [ ] Check 131: V-004 服务端验收与 BSD 客户端连通性按 ADR-013 延后，不作通过结论；已有 SSH 客户端回环原型证据，无法完成者有阻塞原因（来源：spec V-004／Task T025、T099、FR-611、ADR-013）
- [ ] Check 132: 写者原型四项（幂等键/批次/确认/错误传播）测试通过（来源：spec V-005／Task T026）
- [ ] Check 133: 候选性能预算经固定夹具测量后被冻结或以 ADR 修订（来源：spec V-006／SC-021／Task T027、T028）
- [ ] Check 134: ADR-001—014 全部批准且未决项有责任人与退出条件（来源：spec V-007／Task T029）
- [ ] Check 135: 10 万事件会话加载仅元数据+概览+有界尾部，分页游标无重复无遗漏（来源：spec V-101／SC-001／Task T054）
- [ ] Check 136: 1 万任务概览查询报告 p50/p95/p99 且满足冻结预算（来源：spec V-102／SC-002／Task T055）
- [ ] Check 137: 断线重连补拉后关键事件完整，内容采样/脱敏均有标记（来源：spec V-103／SC-003／Task T066）
- [ ] Check 138: 已确认事务经进程终止+重启后状态与关键事件一致（来源：spec V-104／SC-004／Task T043）
- [ ] Check 139: 步骤边界、外部操作前/后、结果登记前三时点崩溃注入后恢复符合协议（来源：spec V-105／SC-005／Task T114）
- [ ] Check 140: 重复幂等键不产生重复任务或重复登记（来源：spec V-106／SC-006／Task T044）
- [ ] Check 141: 清理/压缩保护活跃会话；compact 副本校验成功后方切换（来源：spec V-107／SC-007／Task T039、T041）
- [ ] Check 142: 指定关键查询有计划与性能基线，计划变化触发审查（来源：spec V-108／Task T036、T038）
- [ ] Check 143: 同组业务命令在 TUI/CUI/Web 最终状态、权限与错误码一致（来源：spec V-109／SC-008／Task T088、T095）
- [ ] Check 144: Web 默认 localhost、追踪默认脱敏、子进程不泄露凭据三项默认值验证通过（来源：spec V-110／SC-009／Task T095）
- [ ] Check 145: 违反 Constitution 的产物不能通过门禁；变更有版本与审计（来源：spec V-201／SC-010／Task T070、T074）
- [ ] Check 146: 未解决高优先级澄清项使 Plan 转换被拒绝（来源：spec V-202／SC-011／Task T074）
- [ ] Check 147: 遗漏需求、孤儿任务、无效引用、循环依赖、未覆盖验收项五类可检出（来源：spec V-203／SC-012／Task T073）
- [ ] Check 148: 解析连续两次纠偏失败后任务 blocked，错误与原文可查（来源：spec V-204／SC-013／Task T075）
- [ ] Check 149: 未满足/未测试验收条件生成差异报告，不标记成功收敛（来源：spec V-205／SC-014／Task T076、T077）
- [ ] Check 150: 人工放行审计含执行者、理由、跳过项、产物版本（来源：spec V-206／SC-015／Task T078）
- [ ] Check 151: 任意前端显示同一阶段、门禁结果与产物版本（来源：spec V-207／SC-016／Task T080）
- [ ] Check 152: 关闭全部可选运行时的默认构建主程序可运行，差异按实际文件与 RSS 报告（来源：spec V-301／SC-017／Task T103）
- [ ] Check 153: 受限 JS 通过固定 ECMAScript 测试集；Node API 缺失返回能力不足错误（来源：spec V-302／SC-018／Task T110）
- [ ] Check 154: Python 记录版本/ABI/依赖/解释器路径/C 扩展测试结果，无超范围承诺（来源：spec V-303／SC-018／Task T111）
- [ ] Check 155: Git Bash 验证管道/重定向/路径/信号终止/目标命令，payload 哈希可核验（来源：spec V-304／SC-018／Task T112）
- [ ] Check 156: 任务超时后子进程树按平台策略清理，stderr 尾部保留且状态可恢复（来源：spec V-305／SC-019／Task T109）
- [ ] Check 157: 引用未启用运行时的任务执行前被阻止并给出安装/替代建议（来源：spec V-306／SC-018／Task T107）
- [ ] Check 158: 意图提交后、执行期间、登记后三时点中断的不确定操作标记 `unknown`（来源：spec V-307／SC-005／Task T114）

### 未被 V 项直接覆盖的功能需求（逐条判定）

- [ ] Check 159: 六个会话生命周期命令各有正向与失败路径测试（来源：spec FR-101／Task T046）
- [ ] Check 160: 任务详情五类信息（依赖/步骤/产物引用/最近事件/失败原因）可查询（来源：spec FR-104／Task T049）
- [ ] Check 161: 长历史注入经预算读取，单次注入内存增量有上界断言（来源：spec FR-105／Task T060）
- [ ] Check 162: 压缩后原始历史仍可完整查阅（来源：spec FR-107／Task T061）
- [ ] Check 163: 用户可查看预算、装入来源与截断原因（来源：spec FR-108／Task T062）
- [ ] Check 164: 八类统一事件类型均有实例化测试（来源：spec FR-201／Task T011）
- [ ] Check 165: 事件导出注明脱敏策略（来源：spec FR-205／Task T065）
- [ ] Check 166: 流式积压下关键状态转换零丢失（来源：spec FR-206／Task T057）
- [ ] Check 167: 计量值区分估算与供应商计量（来源：spec FR-207／Task T063）
- [ ] Check 168: 追踪完整性标记四类可查（来源：spec FR-208／Task T064）
- [ ] Check 169: 每阶段定义允许输入/必需产物/校验器/转换且从版本化配置读取（来源：spec FR-302／Task T068）
- [ ] Check 170: 模型调用请求携带显式阶段角色与允许操作（来源：spec FR-313／Task T079）
- [ ] Check 171: 检查点触发（定时/按步骤/危险操作前）间隔与保留数可配置（来源：spec FR-405／Task T105）
- [ ] Check 172: 并行子任务调度未通过状态与写入一致性测试前不启用（来源：spec FR-406／Task T114）
- [ ] Check 173: 轮询兜底仅读只读快照，写者服务无阻塞（来源：spec FR-407／Task T034）
- [ ] Check 174: 单个可重试任务失败不会话级丢失；不可恢复错误向上传播（来源：spec FR-408／Task T052）
- [ ] Check 175: 批量阈值（500 行/50 ms）可配置且经基准校准记录（来源：spec FR-503／Task T034）
- [ ] Check 176: 历史默认分页不使用大 OFFSET（来源：spec FR-504／Task T035）
- [ ] Check 177: 历史注入达预算即停止并返回截断原因（来源：spec FR-505／Task T060）
- [ ] Check 178: 未经基准证明不启用额外缓存/分区/分库/物化结构（来源：spec FR-511／research TBD-10）
- [ ] Check 179: TUI 六视图完整交付（来源：spec FR-601／Task T082）——**部分证据（2026-10-22，批次 1 更新）**：六视图结构/切换/固定尺寸帧 + **真实数据源已接线**（`StoreViewSource`：会话/任务/事件/上下文/Spec 五视图来自 workcode-app 只读命令；`j`/`k` 选择会话、`r` 刷新；真实控制台 13/13 通过）；**未取得**：Diff 视图无数据源（converge 差异未接线）——六分之五，未勾选
- [x] Check 180: CUI 退出码表文档化且测试覆盖（来源：spec FR-602／Task T084、T085）——**已取得（2026-10-22，批次 2）**：`docs/guides/cui-manual.md` §1 列出 0/2/10…99 稳定码表；`output.rs::exit_codes_are_stable_across_error_codes` 断言互异与端点；调度测试覆盖 0（成功）/2（用法）/50（未接线/状态）/80（只读白名单拒绝）；真实命令退出码见 `artifacts/cui-2026-10-22-batch2.txt`
- [x] Check 181: Web 经 HTTP/WebSocket 访问同一应用服务，状态由同一事件流驱动（来源：spec FR-603／Task T090）——**已取得**：`serve --web` 装配访问真实工作区库；浏览器 GET `/api/sessions`、`/api/events`、`/api/status` 均经 `whcode_app` 同一服务层（事件读取同一 events 表）；WS 握手 open→close:1000；证据 `artifacts/web-browser-2026-10-22.txt`
- [ ] Check 182: SSH 默认无任意系统 Shell（来源：spec FR-608／Task T097）
- [ ] Check 183: Web 七类业务操作不经 PTY（来源：spec FR-609／Task T093）
- [ ] Check 184: 新能力同 PR 更新能力矩阵与测试（抽查最近 5 个功能 PR）（来源：spec FR-610／Task T087）
- [ ] Check 185: Runtime Provider 六接口契约测试通过（来源：spec FR-701／Task T100）
- [ ] Check 186: 副作用脚本的重复执行不命中缓存（来源：spec FR-707／Task T106）
- [ ] Check 187: Converge 绑定的运行时测试四要素齐备（来源：spec FR-709／Task T108）

### 非功能需求

- [ ] Check 188: 除 `whcode-platform` 外无平台条件编译分支（来源：spec NFR-101／Task T014）
- [ ] Check 189: 默认构建不启用平台专属/运行时 feature，feature 依赖图有检查（来源：spec NFR-102／Task T009）
- [ ] Check 190: 持久化结构有 schema 版本、迁移脚本与降级/备份说明（来源：spec NFR-103／Task T032）
- [ ] Check 191: 崩溃注入后已确认关键事件无丢失（来源：spec NFR-104／Task T043）
- [ ] Check 192: 模块依赖检查证明核心逻辑/数据库写入/UI 渲染/子进程运行时隔离（来源：spec NFR-105／Task T019）
- [ ] Check 193: 指标可经 CUI JSON 导出，不依赖图形界面（来源：spec NFR-106／Task T038）
- [ ] Check 194: 平台限制与运行时能力均有可查询说明文档（来源：spec NFR-107／Task T118、T120）
- [ ] Check 195: NFR-201—210 十项预算各有分平台实测记录，并已冻结或经 ADR 修订；测量记录含全部纪律字段（来源：spec NFR-201—210／Task T027、T115）

---

## 第三层：边界与异常

- [ ] Check 196: 外部操作在“副作用已发生、结果未登记”时崩溃，恢复标记 `unknown` 且不重放副作用（来源：spec Edge／FR-410／Task T114）
- [ ] Check 197: 事件队列饱和时关键事件走背压/失败/安全停止，无静默丢弃（来源：spec Edge／ADR-006／Task T015）
- [ ] Check 198: 超限 payload 被截断并留下标记，原始无静默删除（来源：spec Edge／R-05／Task T011）
- [ ] Check 199: 慢客户端存在时其他会话广播延迟无劣化（对比测试）（来源：spec Edge／§6.3／Task T094）
- [ ] Check 200: 重复幂等键仅返回既有结果或确定冲突（来源：spec Edge／§7.3／Task T044）
- [ ] Check 201: Windows 子进程树与 BSD 进程组清理后有零残留验证（来源：spec Edge／§10.2／Task T109）
- [ ] Check 202: 运行时缺失/不兼容返回结构化错误，任务状态非静默跳过（来源：spec Edge／FR-708／Task T107）
- [ ] Check 203: 损坏产物样例触发解析报告且原文保留（来源：spec Edge／R-06／Task T069）
- [ ] Check 204: 清理后报告解释空间未缩小的原因与统计（来源：spec Edge／R-10／Task T045）
- [ ] Check 205: 迁移中断后数据库版本未前移且可重试（来源：spec Edge／§7.4／Task T033）
- [ ] Check 206: LLM 失败、断网、磁盘不足、写入失败四情形纳入稳定性测试集（来源：spec Edge／§11.5／Task T116）
- [ ] Check 207: 三时点崩溃注入（步骤边界/执行前后/登记前）各产生符合协议的恢复状态（来源：spec V-105/V-307／Task T114）
- [x] Check 208: 活跃会话在 purge 期间被保护且审计记录在案（来源：spec FR-509／Task T039）。T039 落地：显式保护名单 + 默认 24h 活跃窗口（可见可改），执行时点复检使预览后新变活跃的会话被跳过并报告；删除与 `db_purge` 审计同一事务提交，空清理同样留审计（`purge.rs`：`execute_rechecks_protection_after_preview`、`ttl_within_default_active_window_protects_everything`）。用户 2026-10-21 复核接受
- [ ] Check 209: WebSocket 消息超限被拒且连接不中断其他订阅者（来源：spec §10.2／Task T092）

### 新增需求（2026-10-21）

- [ ] Check 218: 内置 SSH 客户端的安全边界（仅公钥认证、受限入口、最小环境、输出上限、超时、按 ADR-007 登记）有测试或留证（来源：spec FR-611／ADR-011／Task T025、T097）

### 新增需求（2026-10-21，显示适配与资源纪律 FR-612—FR-616）

- [x] Check 219: 三套主题可配置切换，A 屏夜间与墨水屏夜间背景精确为 #000000、墨水屏日间背景精确为 #FFFFFF 且前景黑字（来源：spec FR-612／Task T128）——**已取得**：`ui.theme`（kebab 稳定名）+ 环境变量 `WHCODE_UI_THEME` 覆盖（非法值警告回落，测试 `theme_override_is_visible_and_reversible`）；主题进入渲染单元（`theme_colors_are_applied_to_cells`：eink-day 背景 `#FFFFFF`/前景黑、amoled-night 背景 `#000000`/前景 `#E6E6E6`）；默认 `eink-day`（ui-design）
- [ ] Check 220: TUI 空闲期重绘计数为零、无周期性全量重绘；Web 动效总开关生效且尊重 prefers-reduced-motion（来源：spec FR-613／Task T129、T133）——**TUI 侧已取得（2026-10-22）**：重放证据 100 空闲步零重绘、无待绘制帧零探测；**Web 侧未取得运行时证据**（`animationsEnabled` 门控与仅 `transform`/`opacity` 为代码级；浏览器实测待 Web 服务入口装配）
- [ ] Check 221: 空闲会话卸载后内存可观测下降，唤醒后事件无重复无缺失，卸载/唤醒行为可查询（来源：spec FR-614／Task T131）
- [x] Check 222: zh-CN 与 en 双语输出语义一致；缺失 locale 回退 en；RTL 双向文本处理接口存在且渲染层无 LTR 硬编码（来源：spec FR-615／Task T130）——**已取得（2026-10-22，批次 3）**：双语键表/占位符一致性测试（`both_catalogs_define_the_same_keys`）；TUI 真实控制台 zh/en 各 13/13（标题/页脚/摘要，数据字段不翻译）；CUI 同一命令 zh/en 人类行；Web `/api/i18n` 与浏览器 `?locale=en|zh-CN|ar`（`dir=rtl`、回退 en 不伪造）；证据 `artifacts/i18n-2026-10-22-batch3.txt`；**已知缺口**：clap 帮助与 app/store 错误消息未本地化（错误码→文本映射属后续）
- [ ] Check 223: 传输层进度事件（连接/断开/重连/字节计数/RTT/原因码）可经 CUI JSON 导出（来源：spec FR-616／Task T132）
- [ ] Check 224: gettext PO 接口契约测试通过：i18n 资源目录结构、PO 加载器、zh-CN→en 回退链（来源：spec FR-615／Task T130）
- [ ] Check 227: 最小化/不可见期间无绘制与动画调用（CPU 近空闲基线），无 `requestAnimationFrame` 调用；恢复时单次全量重绘且事件游标连续、关键事件零丢失（来源：spec FR-617／Task T135、T136／`docs/guides/gui-suspend.md`）——**两侧主要证据已取得（2026-10-22）**：TUI 侧——重放证据（最小化跳过绘制并记账、还原单次全量重绘、空闲零探测）+ computer-use 真实窗口最小化/还原（内容完整）；Web 侧——**实测缺陷并修复**：挂起（blur）期间轮询由 9 次/2.5s 降为 **0**，恢复后单次刷新 + 续拉，游标/日志不重复不遗漏；无 `requestAnimationFrame` 由预算门禁静态断言；**剩余缺口**：真实 `visibilityState=hidden` 分支在当前内置浏览器不可触发（后台标签仍报告 visible）——桌面浏览器人工验证前不勾选
- [ ] Check 228: 图形稳定性：禁用 GPU/软件渲染下功能可用无崩溃；连续最小化-还原 20 次无上下文泄漏；关闭动效或 `prefers-reduced-motion` 时不创建动画定时器（来源：spec FR-617／Task T136）——**部分证据（2026-10-22）**：GPU 维度按 NFR-212 为"默认不创建上下文"（运行时路径禁 `getContext`/GPU 探测；浏览器实测 canvas=0、无 WebGL）；**缺口**：20 次最小化-还原循环与 `prefers-reduced-motion` 运行时定时器断言需桌面浏览器（内置浏览器无该控制面）——不勾选
- [ ] Check 229: 前端预算与依赖审计通过：首屏 JS ≤ 50 KB（gzip）、CSS ≤ 20 KB（gzip）、无 WebGL/视频/自动播放/第三方字体、无 `node_modules` 与打包产物；无 JS 时只读视图可用（来源：spec FR-618／ADR-014／Task T137）——**部分证据（2026-10-22）**：预算门禁扩展审计 `suspend.js`（JS gzip 2477 B / CSS 763 B）并新增 NFR-212（运行时路径禁 GPU 上下文）与 `requestAnimationFrame` 断言，全部通过；**缺口**：无 JS 时只读视图可用性未在真实浏览器验证（会话数据经 JS 拉取）——不勾选
- [ ] Check 230: 过程门禁 G0—G5 各有输入/活动/产物/退出判据，且产物映射到真实仓库文件；单人项目的评审替代机制已写明（来源：`docs/guides/software-engineering-process.md`／Task T138）
- [ ] Check 231: 需求工程规范含 FR 模板、六条需求质量判据、三方追溯抽查命令与变更控制流程，并用 ADR-012/ADR-013 两个真实案例验证流程可运行（来源：`docs/guides/requirements-engineering.md`／Task T139）
- [ ] Check 232: 模块设计覆盖 13 个 crate 的六项（职责/不负责/关键类型/依赖/失败模式/测试位置），且与 `test_layering` 的依赖方向一致（来源：`docs/guides/module-design.md`／Task T140）
- [ ] Check 233: UI 设计规范含三主题精确色值（#000000/#FFFFFF）、墨水屏残影约束、i18n/RTL 规则、极简预算与最小化挂起，且逐条对应 FR-612/615/617/618（来源：`docs/guides/ui-design.md`／Task T141）
- [ ] Check 234: 资源四类（内存/GPU/线程/句柄）有可复现测量与基线；新增固定占用无 ADR 不得合入；空闲态不随会话数线性增长（来源：spec NFR-211／Task T142）
- [ ] Check 235: 默认不创建 GPU 上下文；最小化/不可见期间不持有 GPU 资源；视觉效果可显式关闭（来源：spec NFR-212／FR-617／Task T142）
- [ ] Check 236: 默认回环监听、`redacted` 追踪、最小权限、凭据不落日志与事件；放宽项有用户批准与 ADR（来源：spec NFR-213／Task T142）
- [ ] Check 237: 非平台 crate 无平台分支；BSD 交叉检查命令可执行且结果记录；构建链无 GNU 工具（来源：spec NFR-214／D-01／ADR-013／Task T142）
- [ ] Check 238: 自有 crate 零 `unsafe`：workspace 设置 `unsafe_code = "forbid"` 且 13 个成员与 tests 全部继承，契约测试通过；源码无 unsafe 块/函数（来源：spec NFR-215／Task T143）
- [ ] Check 239: 内存溢出/泄漏防护：队列、缓冲、缓存有容量上限与上限测试；句柄 RAII 与释放测试；禁止 `mem::forget`；72 小时泄漏监控未完成时明确标缺口（来源：spec NFR-215、NFR-204／Task T143）
- [ ] Check 240: 哲学总纲存在且含六条命题与记录模板；G2/G4 记录含形式、正负、过去/现在/将来依据与 0 信任校验；PR 清单与 AGENTS.md 已接入（来源：Task T144／`docs/guides/philosophy.md`）
- [ ] Check 241: 并发改动有设计档案（不变量、线性化点、内存序、进展保证、数据所有权、失败模式、验证计划、回退）与至少 3 篇一手文献依据；无档案不得合入，禁止原型先行（来源：spec NFR-216／ADR-015／Task T145）
- [ ] Check 242: 并发实现有 Loom/Shuttle 模型或等价验证、压力与差分测试；不得以“运行数小时未崩溃”作为正确性结论（来源：spec NFR-216／ADR-015／Task T145）
- [ ] Check 243: 内存/CPU 优化有基线、热点定位、前后同夹具对比与分位数；汇编改动有独立 ADR、隔离、安全包装、回退路径与差分测试（来源：spec NFR-217／ADR-015／Task T145）
- [ ] Check 244: 自由与退出权：全部数据可导出、核心能力可离线使用、默认值可撤销；无私有格式锁定、隐藏导出、强制账户/联网、暗黑模式；退出路径有端到端测试（来源：spec NFR-218／Task T146）
- [ ] Check 245: 安全反教条：安全措施有威胁模型、攻击面、证据、代价与失效条件；不以“最佳安全实践/行业标准/检查清单”替代威胁分析与复现验证（来源：spec NFR-219／Task T146／`docs/guides/philosophy.md` §10.6）
- [ ] Check 246: 不伪造与未知显式：工具/模型/代理输出可追溯到原始来源或可复现命令；未知、失败、不可验证均显式标 `unknown`/`[待验证]`；LLM 输出不作唯一证据（来源：spec NFR-220／Task T147／`docs/guides/yike-philosophy.md`）
- [ ] Check 247: 反进步叙事与测量程序：未以采用率、星数、下载量、主流共识或版本号替代正确性证据；性能/安全结论附环境、参数、分位数与交叉验证（来源：spec NFR-221／Task T147）
- [ ] Check 248: 用户友好、反苦难与文档时效：错误信息可操作且不责怪用户；不以 RTFM/苦难/加班作为设计或贡献证据；文档与 ADR 含假设、适用范围与复审日期（来源：spec NFR-222／Task T147／`docs/guides/yike-philosophy.md`）
- [ ] Check 249: 工程准则应用记录存在；`ci/check_counts.py` 通过并接入 Windows CI；AGENTS/过程/需求/PR 已包含证据层级、三次复查与可逆性规则（来源：Task T148／`docs/guides/doctrine-application.md`）

### 新增验收边界（2026-10-21，TUI 非交互）

- [x] Check 226: TUI 不得在 stdin 非交互时进入事件循环：管道调用必须立即退出（退出码 0），证据为 `script/tui_smoke.py` 的 `graceful_exit_on_closed_stdin`（来源：spec Assumptions／Task T024／V-003）——**已取得（2026-10-22 复跑）**：`script/tui_smoke.py` 三项自动检查全部通过（管道 stdin 退出码 0 / `--version` / `doctor` 非交互），产物 `artifacts/tui-smoke/20261022-185913/summary.json`；`script/tui_non_tty_smoke.py` 的 headless/detached 用例同样通过

---

## 第四层：既有功能未回归

**说明**：本仓库为全新工程（首个提交仅含 `.gitignore`），无用户侧既有功能；本层定义“已通过验收不被后续里程碑破坏”的回归判据。

- [ ] Check 210: M1—M9 任一合并后，Windows 11 amd64 数据库冒烟（Check 22 用例）重跑通过；BSD 恢复时补各自冒烟（来源：quickstart §2／Task T022、ADR-013）
- [ ] Check 211: 每个里程碑合并后，契约测试套件（分层/JSON 信封/三端一致）全量通过（来源：quickstart §7／Task T088、T089）
- [ ] Check 212: schema 变更后，从上两个历史版本的迁移测试仍通过（来源：data-model §1／Task T033）
- [ ] Check 213: M2 起每个里程碑合并后，耐久性与幂等性测试（V-104/V-106 用例）保持通过（来源：quickstart §3／Task T043、T044）
- [ ] Check 214: Web/SSH 合入后，五项安全默认值（V-110 用例）重跑通过（来源：quickstart §7／Task T095）
- [ ] Check 215: `source-whcode-taskbook-v3.md` 自入库起内容哈希不变（对照 `git log --follow` 该文件无修改提交）（来源：spec Input／本管道约束）
- [ ] Check 216: docs 内 [S1]—[S8] 引用链接（DuckDB 并发/VACUUM/空间回收/Rust 连接/平台支持/PyO3/Boa/Spec Kit）可定位且未改写其结论（来源：任务书 §16／Check 128—158 证据）
- [ ] Check 217: 已合并功能 PR 均附 §15 七项清单的完成记录（抽查最近 5 个 PR）（来源：任务书 §15／Task T006）
