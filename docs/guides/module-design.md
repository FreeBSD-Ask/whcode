# 模块功能设计（crate 级）

**日期**： 2026-10-21 | **依据**： IEEE 1016（设计描述）、SWEBOK（架构与设计） | **配套**： `plan.md`、`data-model.md`、`contracts/`

每个模块写明：**职责 / 不负责什么 / 关键类型 / 依赖 / 失败模式 / 测试位置**。
新增模块必须补齐这六项，否则过不了 G2（设计基线）。

## 1. 依赖方向（单向，`test_layering` 强制）

```text
whcode-domain        （叶子：无内部依赖）
   ↑
whcode-platform      （唯一允许 cfg(target_os) 的 crate）
   ↑
whcode-app           （命令、权限、快照；不依赖前端；SQL 只在 store——ADR-018）
   ↑
whcode-store / whcode-llm / whcode-speckit / whcode-tools / whcode-runtime
   ↑
whcode-tui / whcode-cui / whcode-web / whcode-ssh   （只经 app 访问状态，禁止触库）
   ↑
whcode-bin           （装配根：配置、日志、feature）
```

## 2. 逐 crate 设计

### whcode-domain

- **职责**：状态枚举（任务/LLM 请求/运行时执行/SDD 阶段）、事件类型与可靠性分级、结构化错误与
  信封、门禁结果与阶段转换、审计记录、追踪级别、时间戳 ISO-8601 换算。
- **不负责**：I/O、数据库、UI、网络、平台分支。
- **关键类型**：`TaskStatus`、`EventKind`、`EventRecord`、`WhError`、`GateResult`、`AuditRecord`、`TraceLevel`。
- **依赖**：仅 `serde`/`serde_json`/`thiserror`。
- **失败模式**：枚举取值与 data-model 不一致（由测试断言）；时间戳解析失败返回 `None`（禁转 0）。
- **测试**：`#[cfg(test)]` 内联（状态/事件/错误/门禁/审计/追踪共 24 例）。

### whcode-platform

- **职责**：进程终止策略（Windows 子进程树 / BSD 进程组）、路径约定、资源探测、终端尺寸与
  stdin 交互性判定、用户级字体注册材料（reg.exe 子进程读写 HKCU Fonts 键、用户字体目录
  与 Windows Terminal settings.json 候选路径探测，FR-717）。
- **不负责**：业务语义、重试策略；字体安装的编排与报告（属 whcode-app::fonts，
  注册表 trait 由装配根注入本模块实现）。
- **关键类型**：`TerminationPolicy`、`ResourceLimits`；`fonts::reg_query_value`/
  `reg_set_value`/`reg_delete_value`、`user_fonts_dir`、`windows_terminal_settings_paths`。
- **失败模式**：探测不到终端返回 `None`（调用方不得当作固定尺寸）；核数探测为 0 时 panic（启动期错误）；
  非 Windows 平台字体注册显式返回 `RuntimeUnavailable`（不伪装成功）；WT settings
  全部未命中由调用方表述为“未检测到”（不视为错误）。
- **测试**：内联 12 例（含终端探测稳定性、线程预算边界、字体键名与路径布局）。

### whcode-app

- **职责**：命令编排与权限（会话/任务/总览/快照）、有界队列语义（关键背压 / 快照可淘汰）。
  存储访问一律经 whcode-store 公共 API（ADR-018）；SQL 与连接管理不进本 crate。
- **不负责**：SQL、渲染、协议编解码、SDD 门禁判定（M4 起接入）。
- **关键类型**：`Capability`/`Actor`/`Policy`/`require`、`NewSession`/`SessionView`、
  `switch_session`、`TaskOverview`/`task_overview`、`TaskDetail`/`retry_task`/`abort_task`、
  `StateSnapshot`/`build_snapshot`、`ContextWindow`/`load_context`/`compress_context`、
  `EventsExport`/`events_export`、`ReplayPage`/`replay_since`、
  `TransitionContext`/`request_transition`/`force_advance`/`ForceAdvance`、`StageRole`、
  `BoundedQueue`、字体本机命令的 `fonts::BUILTIN_FONTS`/`FontRegistry`/`FontEnv`/
  `install`/`status`/`uninstall`（FR-717：本机资源命令，不触库、不经会话授权；
  注册表副作用经 trait 注入，应用层零平台分支）。
- **失败模式**：权限拒绝统一 `permission_denied`（不可重试，FR-604）；切换只加载元数据 +
  预聚合概览 + 50 条尾部 + 游标（有界，V-101）；删除复用 purge 全链（预览/确认/审计/
  活跃保护，FR-509）；retry 仅 failed/aborted/interrupted 可用、abort 拒绝终态与
  `unknown`（FR-410）；快照缺失字段显式 `None`/空（M3/M4/M8 未实现，不伪造）；
  队列饱和返回 `backpressure`（可重试）。
- **测试**：内联 44+ 例（会话生命周期与权限、切换有界、删除确认与保护、总览双范围、
  任务详情五类信息、retry/abort 状态机、快照与库内一致、上下文契约四元组与截断原因、
  压缩指针不删原文、事件导出脱敏与增量、补拉游标与完成性、SDD 门禁（V-202/V-203）
  与 force-advance 审计（V-206）、阶段角色确定性、字体来源三级布局解析、
  安装/卸载循环（dry-run 无副作用、文件占用不中断、终端配置两种形状解析））。
- **M2 决策记录**：命令事件发射延后至 M3（事件流交付时接线）；会话 `stage` 字段
  M4 前为 `None`；`context`/`llm`/`runtimes` 快照字段 M3/M8 前为占位。

### whcode-store

- **职责**：唯一 DuckDB 链接点；版本化 schema 与迁移（v4 增会话 ID 序列）；单写者服务；
  稳定全序游标分页；索引注册表与完整性检查；会话概览预聚合与重建；数据库监控与查询
  计划分析；会话归档、按时间清理与 TTL 及按 ID 删除（预览/确认/审计/活跃保护）；
  工作区导出/导入；compact（复制—校验—原子切换）与空间统计；WAL 阈值策略与
  checkpoint 触发；会话目录与任务投影存储原语（ADR-018）；数据库冒烟。
- **不负责**：业务规则、权限判定（属 app）。
- **关键类型**：`WriterService`、`EventWriter`、`WriterConfig`、`WriteOutcome`、`Direction`、
  `EventCursor`/`MessageCursor`、`EventPage`/`MessagePage`、`REQUIRED_INDEXES`、
  `verify_indexes`、`SessionOverview`、`rebuild_overview`、`MonitorReport`、`plan_query`、
  `WriterStats`、`SmokeReport`、`PurgeCriteria`（before/ttl/sessions）、`PurgeProtection`、
  `PurgePreview`、`PurgeResult`、`preview_purge`/`execute_purge`/`set_archived`、`ExportFile`、
  `ExportReport`/`ImportReport`、`export_workspace`/`import_workspace`、`SpaceReport`、
  `CompactReport`、`compact_database`、`WalPolicy`、`apply_wal_policy`、`run_checkpoint`、
  `SessionRow`/`create_session`/`list_sessions`、`TaskRow`/`get_task`/`stage_distribution`、
  `latest_seq`。
- **失败模式**：提交失败保留待写批次（可重试）；超限载荷拒绝而非截断；空幂等键拒绝；
  迁移失败回滚且不登记版本；分页 `limit` 越界拒绝，不使用 `OFFSET`；索引缺失报
  `invalid_state` 并列出清单；概览重建对未知会话返回 `not_found`，失败不留半更新行；
  查询计划分析拒绝写语句（`permission_denied`）；写者队列深度仅供诊断，panic 后可能滞后；
  清理未显式确认即拒绝（`invalid_state`），执行时点复检活跃保护并报告跳过集，
  删除与 `db_purge` 审计同一事务（审计失败整体回滚），`audit` 行永不随会话 TTL 清除；
  归档对缺失会话返回 `not_found` 且不做部分更新；导出/导入拒绝未迁移库、格式或
  schema 不兼容、分块校验和漂移与主键/唯一键冲突（不做部分导入），导出只含业务表
  （`audit`/`runtime_state`/`schema_migrations` 不出库）；compact 副本校验失败不切换
  且原库可写，切换失败恢复备份；compact 与 `CHECKPOINT` 不承诺缩小文件，空间统计
  如实记录四项；WAL 设置值经白名单校验（拒绝引号/分号），高负载路径无强制 checkpoint。
- **测试**：内联 90 例（冒烟四步、迁移 v0→v4/v1→v4/v2→v4/v3→v4 与两类中断回滚、写者服务不变量、
  游标双向翻页与破平局、索引完整性、概览重建与滞后检测、监控四类信息与只读计划分析、
  清理预览—执行一致、确认拒绝、Abort 策略、TTL 边界与活跃窗口交互、归档全有或全无、
  导出导入往返（含时间戳无损）、格式/schema/校验和/冲突拒绝、compact 切换与校验拒绝、
  WAL 策略生效与非法值拒绝）；集成测试 `tests/integration/`：进程终止重启一致性
  （`durability`，子进程 abort 模式）、重复幂等键跨重启（`idempotency`）、清理/压缩
  四项空间统计与 VACUUM 不承诺（`space_report`）。

### whcode-llm

- **职责**（M3，已交付 T056—T064/T132）：Provider 抽象（能力探测/取消/流式/计量来源/
  超时/错误归一化 + 桩实现）、双队列事件路由（关键不丢弃/展示可合并并留可查询标记）、
  chunk 流编排（进度事件不含正文）、四级追踪与双层脱敏（字段名 + 值模式）与完整性标记、
  链路关联 ID 与断链检出、计量来源两级（供应商/本地估算，估算标记）、传输层进度事件
  （连接/断开/重连/字节计数/RTT/断连原因码）。
- **不负责**：持久化（事件由调用方落库）、UI 状态、真实 HTTP Provider。
- **关键类型**：`Provider`/`StubProvider`/`CancelToken`/`ChunkStream`、`EventRouter`/
  `pump_stream`/`PumpOutcome`、`IntegrityFlags`/`sanitize`、`Correlation`/`link`/
  `chain_breaks`、`Usage`/`estimate_tokens`、`TransportTracker`。
- **失败模式**：供应商错误归一化为 `WhError`（原文进 details）；关键队列饱和返回
  `backpressure`（可重试，不静默丢弃）；展示饱和丢弃最旧并累计合并标记；取消在
  下个 chunk 前生效（桩的既定说明）；估算计量必须标记 `cost_is_estimate`（FR-207）；
  断链（相关事件缺 request_id）可检出（FR-204）。
- **测试**：内联 18 例（桩流式/取消/超时归一化、关键背压与展示合并标记、
  编排不携带正文、四级脱敏与完整性标记、关联与断链、传输事件有界）。

### whcode-speckit

- **职责**（M4，已交付 T068—T077）：版本化工作流配置与阶段定义（应用层状态机，
  FR-302）；Markdown 解析器（未解析段落/重复 ID/未识别字段报告 + 纠偏重试 ≤2 次后
  blocked 并保留原文，FR-303/309）；Constitution 创建/导入/变更审计（FR-301）；
  稳定 ID 与引用校验（FR-304）；双向追溯与断链检出（FR-305）；Analyze 五类缺陷检出
  （FR-306）；Converge 验收条件四态判定与差异报告（FR-310/311）。
- **不负责**：阶段转换执行（属 app/gates）、权限判定、运行时执行（M8）。
- **关键类型**：`SddStage`/`WorkflowConfig`、`ParsedTask`/`ParseReport`/
  `ParseBlocked`/`parse_with_retry`、`Constitution`、`IdRegistry`、`Traceability`、
  `AnalyzeReport`/`analyze`、`ConvergeReport`/`converge`。
- **失败模式**：解析失败保留原文并标记 blocked（不静默丢弃）；未知段落行进
  `unparsed_sections`（原文保留）；空产物视为解析失败；重复 ID 与未知字段显式报告；
  循环依赖 DFS 检出；`not_applicable`/`pass` 无证据即差异（FR-310）。
- **测试**：内联 21 例（工作流转换与四要素、三段解析与标注、异常报告、纠偏与阻塞、
  ID 归一化与引用、双向追溯、五类检出、四态判定与差异）。

### whcode-tools

- **职责**（M8 前最小）：工具注册、权限策略、执行意图与结果登记（ADR-007）。
- **失败模式**：不确定副作用标 `unknown`。

### whcode-runtime

- **职责**（M8）：Runtime Provider、payload 校验、子进程 worker、能力等级。
- **失败模式**：能力不足返回结构化错误（`runtime_unavailable`），不静默跳过。

### whcode-tui

- **职责**：六视图闭环（会话/任务/上下文/事件/Spec/diff，FR-601）+ 渲染纪律（只读快照、
  脏标记重绘、空闲零重绘、帧行数上界，§5.3/T083/T129）；事件循环接线最小化探测
  （T135/ADR-021）与主题精确色值（FR-612）；进入/恢复守卫。
- **不负责**：业务命令（经 app）；不在帧路径反序列化 JSON；**视图数据准备**——数据由
  装配根经 `ViewSource` 注入（当前为 `StoreViewSource`：`whcode-bin` 打开工作区库并经
  `whcode-app` 只读命令生成快照；打开失败时回退 `EmptyViewSource` 并显式告警）。
- **关键类型**：`UiEvent`、`TuiState`、`ViewId`/`ViewSnapshot`、`ViewSource`、
  `Renderer`、`RunOptions`、`ScriptStep`/`ScriptReport`、`TerminalSession`/`TerminalGuard`、
  `Theme`。
- **失败模式**：stdin 非交互时**不得**进入事件循环（Check 226）；Drop 中恢复失败不 panic；
  最小化探测失败按“未知”处理（不伪装最小化状态，见 ADR-021 修订）。
- **测试**：内联 41 例（含六视图固定尺寸帧、帧预算归档、脚本化门控/零重绘证据——
  `run_scripted_steps` 与真实循环共用 `LoopCore`）+ 真实控制台取证
  `script/tui_views_smoke.py`（六视图帧落盘 artifacts/）。

### whcode-cui

- **职责**（M3 已交付 T062/T065；M5 继续）：预算视图（当前预算/装入来源/截断原因，
  FR-108）；事件 JSON/JSONL 查询与导出（脱敏策略随导出注明，FR-205）。
  均为应用服务的展示包装（前端禁止触库，ADR-003/005）。
- **职责**（M4 已交付 T078；M5 继续）：`force-advance` 视图（reason 必填 + 审计，
  FR-312）。M5：clap 命令树、`--json` 信封、稳定退出码、dry-run。
- **失败模式**：错误输出四字段齐全；`redacted` 下秘密不出现在导出（V-110）；
  破坏性命令必须预览/确认。

### whcode-web

- **职责**（M6）：HTTP/WebSocket 适配器、默认回环绑定、认证与 Origin 校验（ADR-022）；
  极简前端（ADR-014）；`assets` 静态资产在**编译期嵌入**二进制（固定白名单路由，
  无目录遍历面）。
- **进程入口**：`whcode-bin` 的 `serve --web` 装配（contracts/cui-cli.md §1）：
  工作区库 → 回环监听 → 顺序服务循环（单线程；DuckDB `Connection` 为 `Send` 非
  `Sync`，ADR-002）；WS 为 RFC 6455 握手 + 正常关闭（会话级推送未实现，前端不依赖）。
- **失败模式**：无认证时对外监听拒绝启动（V-110）；非回环 Origin 拒绝（ADR-022）；
  请求体有界（`MAX_MESSAGE_BYTES`）。

### whcode-ssh

- **职责**：内置 SSH 客户端（known_hosts 风格 TOFU 信任校验，ADR-005/030；默认命令策略只放行
  首 token 为 `whcode`／`whcode.exe` 的远端命令，任意系统 Shell 需显式 `allow_shell` 并留审计）；
  M7 服务端（公钥认证、会话隔离、受限入口、单写者执行器，ADR-030）。四平台现场验收按 ADR-013 延后。
- **依赖**：`whcode-app`（应用服务与权限判定）、`whcode-domain`、`russh 0.64`、`tokio`；
  `duckdb` 仅用于**透传**装配根注入的 `Connection` 类型（同 `whcode-web` 先例），
  本 crate 不自行打开数据库（`Connection::open` 只出现在测试）。
- **关键类型**：`SshTarget`、`ExecReport`、`ExecRegistry`（客户端）；
  `SshServer`、`SshPolicy`、`CommandExecutor`、`KnownHosts`、`HostKey`（服务端）。
- **失败模式**：无退出状态判 `unknown`；输出尾部有界并标记截断；认证失败结构化拒绝并审计；
  命令队列满背压（容量 64）、执行超时 30 s；主机密钥不匹配拒绝且不改写信任文件；
  客户端默认拒绝非 `whcode` 命令（`PermissionDenied`，需显式 `allow_shell` 并留审计）。

### whcode-gui

- **职责**：原生桌面 GUI（只读 v1，FR-711—FR-715、ADR-027）；iced + `tiny-skia` 软件光栅，
  默认不创建 GPU 上下文。
- **依赖**：仅 `whcode-app`（只读快照 `view_source`）与 `whcode-domain`（i18n）；不触库。
- **关键类型**：`RunOptions`、`GuiTheme`（四套精确色值，`t` 键循环切换、不重启；`terminal-green` 为纯黑底 OLED 兼容 + 荧光绿 `#00FF00`）、
  `State`（快照 + 选择位置）。
- **失败模式**：窗口/渲染后端初始化失败返回 `iced::Error`；字体来自仓库内置的
  OFL 子集（FR-716／ADR-028），四类文本按字符显式分段：比例文本拉丁 Inter／CJK
  Noto Sans CJK SC，等宽文本拉丁 JetBrains Mono／CJK Sarasa Mono SC；子集缺字由
  系统字体栈回退，界面降级但不 panic。
- **资源预算**：空闲 1200×800 下 RSS ≤ 96 MB、私有 ≤ 80 MB、线程 ≤ 24、句柄 ≤ 256、
  CPU ≤ 0.2% 单核（实测见 `artifacts/gui-run-2026-10-23/summary.json`）。

### whcode-bin

- **职责**：装配根（配置、日志、`doctor`、`bench`、TUI 启动、feature 组合）。
- **失败模式**：配置解析失败返回 `parse_error`（不可重试）；退出码稳定（0/1）。

## 3. 跨模块约束

| 约束 | 说明 | 强制方式 |
|---|---|---|
| 单写者 | 所有写入经 `whcode-store`；前端不触库 | `test_layering` |
| 同事务 | 任务状态、关键事件、检查点引用同事务提交（FR-403，M1 起） | 集成测试 |
| 错误模型 | 一律 `WhError`（四字段），不得自造错误结构 | 契约测试 |
| 状态命名 | 复用 domain 枚举，不得字符串字面量 | 代码评审 + 测试 |
| 平台分支 | 仅 `whcode-platform` | `test_layering` |

## 4. 新增模块的准入清单（G2）

- [ ] 写出六项（职责/不负责/关键类型/依赖/失败模式/测试位置）
- [ ] 依赖方向符合上图；平台分支不越界
- [ ] 错误码归属明确（复用 `ErrorCode` 或经 ADR 新增）
- [ ] 至少一个正向与一个失败路径测试
- [ ] 若引入新依赖：许可为 BSD-2 兼容、非 GPL 传染、可交叉检查
