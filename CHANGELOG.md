# 变更记录

本文件记录可观察的变更。格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

**版本标记约定**：

- `0.1.0` 对应 M0（平台可行性与基线冻结）的工程侧基线，**不是发布版本**；
- Git 标签形如 `v0.1.0-m0`（预发布标识），只标记“该里程碑证据齐备时的代码状态”；
- 只有在该里程碑的强制验收全部通过后，才会去掉预发布标识。

## [Unreleased]

### 变更

- 版本号改为**二进制构建时间**（2026-10-23，用户要求）：`crates/whcode-bin/build.rs` 在
  构建时取系统时钟注入 `WHCODE_BUILD_STAMP`（**本地时间** `YYYY-MM-DDTHH:MM±HH:MM`、
  精确到分钟）。时区获取**不做平台判定**（NFR-101：平台条件编译集中在 `whcode-platform`，
  构建脚本同样计入契约测试），改按可用性依次探测外部时间命令（`powershell` → `date`）；
  两者均不可用时回退 UTC `...Z` 并输出 `cargo:warning`（不静默降级）；零新依赖、零 unsafe。
  `whcode --version`、`doctor` 文本与 `doctor --json` 的 `whcode_version`
  均报告该时间戳。Cargo 包版本保持 `0.1.0`（语义化版本用于依赖与打包；展示版本 = 构建时间）。
- 程序中文名调整为「旺皇 Code」（2026-10-23）：中文文本层 `app.title`、TUI 中文标题、
  CUI `--help` 简介、Web 页首标题与 README 标题同步；英文名保持
  `WHCode Coding Harness` / `whcode` 不变（`?locale=en` 与 `LANG=en` 路径不受影响）。
- 批次 4：Web 仅限局域网 + 认证边界（2026-10-22，ADR-023）：`serve --web` 增加
  `--token-file`；地址边界只允许回环或 RFC1918 私网（公网/`0.0.0.0` 拒绝）；局域网
  监听强制认证与 Origin 检查；前端从 URL fragment 读取 token 并以 Bearer 调用 API；
  响应加 `cache-control: no-store`。Origin 语义修订：缺失 Origin 两种模式均放行
  （同源 GET 真实形态；跨站由 Bearer 阻断），带 Origin 必须命中允许集。
- 批次 3：文本层端到端接线（2026-10-22，T130）：zh-CN/en 词表扩至 55 键并新增
  `resolve_fmt`/`entries`；TUI 标题/页脚/空态与视图摘要、CUI 人类可读输出、Web 标签/
  状态/摘要统一经 `whcode-domain::i18n`；Web 新增 `GET /api/i18n`（?locale= →
  Accept-Language → zh-CN，返回 rtl 与词条）与前端 `data-i18n`/`<html lang dir>`；
  `?locale=` 显式覆盖。修复主入口路由回归（`whcode --workspace DIR` 误入 clap）与
  首屏任务列表延迟。数据字段/错误码/退出码不随语言变化。
- 批次 2：CUI 命令调度装配（2026-10-22，T084/T085 接线）：`whcode <command>` 经
  `whcode-app` 执行会话/任务/事件/状态/SDD 状态与 `db` 维护命令（stats/check/purge/
  compact/archive/explain），`--json` 统一信封、错误四字段、稳定退出码（新增用法错误
  `2`）；`--workspace`/`--json` 为全局旗标；未接线命令（run/events show/spec 门禁/
  runtime/db checkpoint）显式失败。新增写入并修复：显式删除不套 TTL 活跃窗口保护
  （避免"删除刚创建的会话"被静默跳过）。同命令一致性用例（CUI vs Web 读命令）落地。
- 批次 1：前端数据接入（2026-10-22）：TUI 新增 `StoreViewSource`（装配根实现）——六视图
  经 `whcode-app` 只读命令供给（会话/任务/事件/上下文/Spec 真实数据；`j`/`k` 选择会话、
  `r` 刷新、`> ` 选中标记；数据源错误显式写入摘要行，打开失败回退空数据源并告警）。
  Web 新增 `/api/tasks`（会话/工作区总览）与任务列表、会话条目点击选择（aria-current、
  URL 同步、键盘可达）、无参数时自动选中首个会话。修复会话切换的并发事件重复（在途
  守卫 + 会话快照校验 + 游标去重）与事件游标起点负数。演示夹具扩展 3 个任务。
- Web 服务装配与端到端测试（2026-10-22，T090—T095 补装配 + T135/T136 收口）：
  `whcode serve --web [--bind HOST:PORT] [--workspace DIR] [--config PATH]`
  （contracts/cui-cli.md §1）——静态前端**编译期嵌入**（固定白名单路由、无目录遍历面）
  + `/api/*` 经同一应用服务；顺序服务循环（DuckDB 连接 `Send` 非 `Sync`，ADR-002）；
  WebSocket 握手 + 正常关闭（会话推送未实现，前端不依赖）；外部绑定拒绝（FR-606 实测）。
  ADR-022 修订 Origin 匹配（scheme+host、端口不参与；回环模式缺失 Origin 放行，
  非回环 host 仍拒绝）。新增演示工作区夹具 `tests/integration/seed_web_demo.rs`（幂等）。
  真实浏览器与桌面取证：`artifacts/web-browser-2026-10-22.txt`、
  `artifacts/tui-computeruse-2026-10-22.txt`（截图 `web-browser-2026-10-22/history-view.png`）。
- T082/T083/T129/T135 TUI 接线 + T136 收口 + T128 配置接线（2026-10-22）：六视图接入
  真实运行循环（`1`—`6` 切换、`ViewSource` 准备层、`EmptyViewSource` 空数据源显式
  「暂无数据」不伪造）；脏标记门控与空闲零重绘进入循环，重放证据 `run_scripted_steps`
  与真实循环共用 `LoopCore`（探测计数/最小化跳过/还原单次重绘均断言）；窗口最小化
  探测接线（ADR-021 修订：仅待绘制帧 + 输入静默 ≥1s 时探测、间隔 5s→60s 指数退避、
  检测到最小化回落 5s——空闲零探测）；帧路径限行投影 `render_lines_limited`（帧成本
  由终端尺寸界定，帧预算归档 `artifacts/tui-frame-budget.json`，release p95 448 µs）；
  主题经 `ui.theme` + `WHCODE_UI_THEME` 选择并进入渲染单元（FR-612）；前端审计扩展
  `suspend.js` 与 NFR-212 断言（运行时路径禁 GPU 上下文探测、无 `requestAnimationFrame`）；
  真实控制台六视图取证 `artifacts/tui-views-2026-10-22.txt`。
- T068—T081 SDD 门禁状态机（M4）：`whcode-speckit` 交付版本化工作流配置与应用层
  状态机（FR-302）、Markdown 解析器（未解析段落/重复 ID/未识别字段报告 + 纠偏重试
  ≤2 次，仍失败 blocked 并保留原文，FR-303/309/R-06）、Constitution 创建/导入/变更
  审计（FR-301）、稳定 ID 与引用校验（FR-304）、双向追溯与断链检出（FR-305）、
  Analyze 五类缺陷检出（FR-306）、Converge 四态判定与差异报告（FR-310/311）。
  `whcode-app` 交付门禁强制执行（高优先级澄清阻止 Plan、Analyze 阻止 Implement，
  FR-307/308）与 force-advance（reason 必填 + 审计含 actor/reason/跳过门禁/产物版本/
  时间戳，FR-312）与阶段角色显式注入（FR-313）；快照 `session.stage` 对四端同源
  （FR-314）。store 迁移 v5（`sessions.stage` 可空列 + 回填，DuckDB ALTER 限制已记录）。
  speckit 21 例内联、app 5 例新增、CUI spec_cmd、集成测试 V-201—V-207 七项判定
  （`tests/fixtures/sdd/` 违规产物夹具，T081）。
- T056—T067、T132 应用服务（M3）：`whcode-llm` 交付 Provider 语义与桩实现（能力探测/
  取消/流式/计量来源/超时/错误归一化，FR-201）、双队列事件路由（关键不丢弃返回
  `backpressure`、展示可合并并留可查询标记，FR-206/ADR-006）、四级追踪与双层脱敏
  （字段名 + `sk-`/`Bearer` 值模式）与完整性标记（FR-203/208/D-05）、链路关联与断链
  检出（FR-204）、计量来源两级（估算标记 `cost_is_estimate`，FR-207）、传输层进度
  事件（FR-616）。`whcode-app` 交付上下文加载器契约（`items + next_cursor +
  consumed_budget + truncated_reason` 有界迭代，FR-105）、压缩摘要指针（压缩后原文
  仍可查，FR-107）、事件导出（脱敏策略随导出注明，FR-205）、按游标补拉（FR-202/605）。
  `whcode-cui` 交付预算视图（FR-108）与事件 JSON/JSONL 视图。18 例 llm 内联、
  7 例 app 新增、3 例 CUI、1 例集成测试（脱敏/串联/截断/补拉，V-103 关联）。
- T046—T052 应用服务（M2）：`whcode-app` 交付会话命令（new/list/show/rename/archive/
  delete，FR-101）、会话切换有界加载（元数据 + 预聚合概览 + 50 条尾部 + 事件游标，
  FR-102）、任务总览（阶段分布/完成度/阻塞/活跃，FR-103）、任务详情五类信息与
  retry/abort 状态机（FR-104/401/408）、能力模型权限检查（拒绝统一
  `permission_denied` 不可重试，FR-604）、四端同源状态快照（FR-314；`stage`/
  `context`/`llm`/`runtimes` 属 M3/M4/M8，显式 `None`/空标注）。删除复用清理全链
  （预览/确认/审计/活跃保护，FR-509）。28 例内联测试。
- ADR-018：`whcode-app` 经 `whcode-store` 公共 API 访问存储、SQL 只在 store；
  AGENTS/module-design 分层口径同步修订。store 新增 `directory`（会话目录，
  schema v4 会话 ID 序列）与 `tasks`（任务投影，状态/阶段分布、有界详情读取、
  产物只出引用）存储原语；purge 判据扩展按会话 ID 清理；迁移 v0→v4/v1→v4/
  v2→v4/v3→v4 路径补齐。
- T053—T055 集成测试（`tests/`）：`gen_dataset` 夹具（10 万事件 + 1 万任务固定
  分布）；`session_switch` 验证切换有界加载与双向游标无重复无遗漏（V-101/SC-001）；
  `overview_perf` 报告 1 万任务概览 p50/p95/p99 并归档
  `artifacts/overview-perf-m2.json`（p95 7.6 ms，候选预算 50 ms，预算未冻结——
  V-102/SC-002，D-06）。
- T045/T043/T044 集成测试（`tests/integration/`）：`space_report` 覆盖清理/压缩前后
  文件大小、WAL、已用块、可用块四项统计并显式排除任何"VACUUM 释放空间"承诺；
  `durability` 以子进程 `abort`（0xC0000409 fail-fast，不运行析构）模拟进程终止，
  重开后 WAL 重放还原全部已确认事务、`seq` 唯一、按幂等键重放返回既有 seq（V-104/V-106）；
  `idempotency` 覆盖跨写者重启去重、同键不同载荷确定性返回既有结果、缓冲中同键
  不得重复入队（V-106）。
- T042 WAL 策略：`wal.rs` 提供阈值设置（白名单校验后透传 `checkpoint_threshold`/
  `wal_autocheckpoint`，`None` 保持现状）、显式/空闲窗口 checkpoint 与纯函数空闲判定；
  高负载路径无强制 checkpoint，自动阈值行为经 `monitor::wal_status` 观测。
- T041 compact：`compact.rs` 实现 D-10 流程（进程内维护锁 → CHECKPOINT 停写 → 文件复制 →
  副本校验（schema 版本/必需索引/逐表行数）→ 原子切换（备份改名 + 失败恢复）→ 复验）；
  副本校验失败或切换失败不动原库（Check 41）；`SpaceReport` 记录文件大小/WAL/已用块/
  可用块四项；不承诺缩小文件（FR-510、R-09）。BSD 文件锁语义按 ADR-013 延后取证。
- T040 导出/导入：`export.rs` 实现 `whcode-export` v1（UTF-8 JSON：清单 + 表数据 +
  分块校验和）；导出 11 张业务表（`audit`/`runtime_state`/`schema_migrations` 不出库），
  流式写出无 `OFFSET`；导入前依次校验内容哈希、格式/版本、schema 兼容（导出版本 ≤
  目标版本）、分块校验和、列一致与主键/唯一键冲突，任一失败整体拒绝（Check 40）；
  导出写 `data_export` 审计，导入不在强制审计清单（决策已记录）；时间戳经 SQL CAST
  往返无损。
- T039 清理与归档：`purge.rs` 实现会话归档（全有或全无）、按时间清理与 TTL
  （`--before`/TTL 判据，支持仅清理已归档会话）；清理以会话为根删除 `tasks`、
  `checkpoints`、`runtime_executions`、`events`、`messages`、`stage_artifacts`、
  `context_windows`、`session_overview` 与 `sessions` 行，`audit`/全局行不清理；
  FR-509 四项机制：`preview_purge` 只读冻结清理集、`execute_purge` 强制显式确认、
  删除与 `db_purge` 审计同一事务、活跃会话保护（显式名单 + 默认 24h 活跃窗口，
  预览与执行两时点各检查一次，跳过集在结果与审计中报告，`Abort` 策略可整体拒绝）；
  10 例测试覆盖预览—执行一致、确认拒绝、TTL 边界与活跃窗口交互、空清理审计。
- T038 监控：`monitor::collect` 一次输出数据库统计、14 表精确行数、WAL 状态与写者队列深度；
  `plan_query` 仅接受只读查询，支持 `EXPLAIN`/`EXPLAIN ANALYZE`；写者服务新增
  `queued_len`/`capacity` 诊断指标（`AtomicUsize`，SeqCst）。
- T037 会话概览：`session_overview` 预聚合与事务化重建（单会话/全量），标注 `session_id`
  为强一致键、计数与阶段分布为最终一致；6 例测试覆盖滞后检测、upsert、空会话与未知会话拒绝。
- T036 索引与计划：schema v3 补齐 messages `(session_id, ts, id)` 与 tasks `(session_id, stage)`
  索引，删除被覆盖的旧消息索引；新增索引注册表与完整性检查；基准实验记录 8 条关键查询的
  `EXPLAIN`/`EXPLAIN ANALYZE`——单列索引命中，复合索引在 DuckDB 1.5.6 未触发扫描，顺序扫描
  已逐条论证（`artifacts/query-plan-report.json`）。
- T035 游标分页：`events` 按 `seq`、`messages` 按 `(ts, id)` 建立稳定全序，双向排他游标；
  单页上限 1000，无 `OFFSET` 路径；5 例测试覆盖重复请求一致与双向无重复无遗漏。
- T033 跨版本迁移：schema 升级到 v2（事件 `request_id`/`tool_call_id` 追溯索引，FR-204）；
  集成测试覆盖 v0→v2、v1→v2 数据保留与中断回滚（版本号不前移、半成品索引不残留）。
- T034 写者服务：专用写线程 + 有界通道 + 同步确认（ADR-017）；`whcode-store` 34 例测试通过，
  压力证据 `artifacts/writer-service-stress.json`（4 000 事件、`seq` 唯一连续、p50 45 µs）。
- 用户复核接受桌面自动化 TUI 记录，关闭 V-003 并勾选 Check 24；方向键/raw mode 残留限制已注明。
- V-003 桌面自动化冒烟：resize、Ctrl-C、q 均通过；非人工逐键，人工复核待确认（`artifacts/tui-manual-2026-10-21.txt`）。
- T032/T033：14 表 schema 与迁移集成测试；写者服务并发设计档案 ADR-017（T034 设计先行）。
- M0 按 ADR-016 有条件收口：Windows 工程基线 Go；V-003 人工项待补，V-004 延后 M7，V-006 预算不冻结；新增 TUI 人工冒烟协议。
- 工程准则应用至本项目：新增 `ci/check_counts.py` 计数门禁、证据层级/三次复查/可逆性规则与 `doctrine-application.md`；T148、Check 249。
- 新增 `engineering-doctrine/`：可移植工程哲学、过程、需求、设计、并发、安全、UI、交付与模板。
- 完整读取 `sacred_sources.duckdb` 的一刻哲学与四版前言，新增 `docs/guides/yike-philosophy.md`；NFR-220—222、T147、Check 246—248。
- 哲学增补自由、洞穴与安全：不定义用户自由、退出权、反全知幻觉、证明优先、主体性、安全反教条；NFR-218/219、T146、Check 244—245。
- 新增 `docs/guides/performance-and-concurrency.md` 与 ADR-015：并发设计先行、一手文献门槛、汇编例外；NFR-216/217、T145、Check 241—243。
- 新增 `docs/guides/philosophy.md` 与哲学门禁：形式主义审查、正负和解、三重时间依据、
  0 信任原则。
- 需求基线新增 NFR-211—215：资源占用（内存/GPU/线程/句柄）、GPU 纪律、安全优先、
  BSD 代码兼容与禁用 GNU 工具、内存安全与零 `unsafe`。
- 新增 `AGENTS.md` 代理协作入口与过程文档集（软件工程过程、需求工程、设计原则、模块设计、
  UI 设计、测试模型）。
- 契约测试新增 workspace lints 继承校验，确保全部成员保持 `unsafe_code = "forbid"`；
  clippy 增 `mem_forget = "deny"`。
- 新增 `docs/guides/test-model-sensenova.md`，记录图像输入实测与 `thinking` 对象形态。

### 修复

- Web Origin 校验全串精确比较误伤真实浏览器（Origin 含端口；同源 GET 不带 Origin →
  一律 403）：按 ADR-022 改为 scheme + host 匹配（回环模式缺失 Origin 放行，
  非回环 host 仍拒绝；认证模式仍要求 Origin）。
- `serve --web` 静态路由未剥离查询串：`GET /?session=…` 落入 404——已修复并加回归断言。
- Web 挂起（blur/visibilitychange）期间未暂停轮询（FR-617）：真实浏览器实测挂起
  2.5 s 内仍发出 9 次 API 请求 → 修复后 0 次；恢复单次刷新后续拉，游标不重复不遗漏。
- `whcode-tui` 的 `theme` 模块此前**未在 `lib.rs` 声明**（文件在盘但未编译，测试从未
  运行）——已声明并接线（T128 收口，2026-10-22）。
- `tests/stability/run_stability.sh` 工作负载（`extreme_scale` 循环）在异常退出路径
  **不清理**：曾遗留负载循环 4 小时（干扰后续构建与测试）。修复：`EXIT`/`INT`/`TERM`
  trap + `taskkill /T` 进程树清理（Windows 无进程组语义，单杀子 shell 不终止 cargo 下游）。

### 计划

- M1 存储与事件基座：14 表 schema 与可重复迁移、单写者服务线程、游标分页、预聚合概览、
  清理/压缩、监控（tasks.md T032—T045）。

## [0.1.0] — 2026-10-21（M0 基线）

### 新增

- **存储**：DuckDB 单文件持久化（官方预编译动态库 1.5.6，SHA256 校验）；数据库冒烟四步
  （创建、事务写读、关闭重启、迁移幂等）；事件写者原型（幂等键、批次阈值、关键事件确认、
  错误传播与批次保留）。
- **写者性能**：批量落库（分块 IN 查询已存在键 + 分块多行 `VALUES` 参数化插入 + 插入前
  分配连续 `seq`，不再逐条回读）；实测吞吐中位数 **6 921 条/秒**（20 000 条 × 3 次，
  Windows 11 / i7-1260P / release / DuckDB 1.5.6 / `memory_limit=192MB`）。
- **TUI**：最小闭环（尺寸探测、事件循环、键盘与 resize、退出、终端恢复守卫）；
  非交互 stdin 时**不进入**事件循环（修复管道调用挂起的缺陷）。
- **SSH 客户端原型**：公钥认证、受限命令执行、超时与输出上限、按“意图—执行—登记”留痕。
- **CLI**：`--version`、`doctor [--config PATH] [--json] [--verbose]`、
  `bench baseline --out PATH [--events N] [--repeat R]`；`doctor --json` 增加
  `whcode_version` 与运行期 `duckdb_engine_version`。
- **工程体系**：十三 crate 分层与可执行契约测试（前端不触库、平台分支收敛、目录分类门禁）；
  五平台 CI 定义；依赖记录脚本；基线基准夹具与稳定性测试占位；LF/UTF-8 编码策略；
  SECURITY.md、CONTRIBUTING.md、PR/Issue 模板；BSD-2-Clause 许可（FreeBSD 中文社区）。

### 变更

- 平台范围收敛为 **Windows 11 amd64**（ADR-013），不考虑任何 32 位平台；
  FreeBSD/NetBSD/OpenBSD/DragonFly 延后，CI 改为手工触发。
- MSRV 与可复现工具链分离：`rust-version = "1.88"`（MSRV），`rust-toolchain.toml` 锁定
  `1.99.1`。
- 基准夹具 `memory_limit` 由未经验证的 384MB 改回 §5.4 基线 192MB（ADR-012 证伪了
  “192MB 会 OOM”的断言）。

### 修复

- 事件写入逐条回读 `seq` 造成的吞吐瓶颈（158 → 6 921 条/秒）。
- TUI 在“管道 stdin + 控制台 stdout”环境永久等待按键。
- 错误对象在无细节时省略 `details` 键，违反契约四字段要求。
- 有界队列在快照溢出时可能淘汰关键条目，违反“关键事件不得静默丢弃”。

### 已知限制

- 性能预算**未冻结**：缺 72 小时时间序列（NFR-204、§11.5）。
- V-003 交互四项（物理键盘、resize 重绘、Ctrl-C 恢复、强杀后 raw mode）待人工留证。
- SSH 服务端与运行时（JS/Python/Shell）尚未实现（M7/M8）。
- Web UI、CUI 命令树、SDD 门禁状态机尚未实现（M2—M6）。

## [0.0.0] — 2026-10-20

### 新增

- 初始工程骨架：Cargo workspace（13 crate）、领域类型、有界队列、配置与日志脱敏、
  分层与平台隔离契约测试。
