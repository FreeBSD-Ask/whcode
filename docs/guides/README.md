# docs/guides

使用者与贡献者指南（tasks.md T007、T119）。

## 已建立

| 文件 | 内容 |
|---|---|
| `project-status.md` | 带日期的项目状态快照（里程碑、未完成项、能力与门禁、CI 工作流）；权威账本仍是 tasks.md／checklist.md |
| `philosophy.md` | 核心哲学与工程方法论（形式审查、正负和解、三重时间、0 信任） |
| `design-principles.md`、`module-design.md` | 设计原则与 14 个 crate 的职责/依赖/失败模式 |
| `software-engineering-process.md` | G0—G5 阶段门禁、角色合并、产物映射 |
| `requirements-engineering.md` | FR 模板、需求质量判据、三方追溯抽查方法、变更控制 |
| `repository-layout.md` | 目录分类原则（由 `tests/contract/test_layout.rs` 强制） |
| `pr-checklist.md` | 每 PR 必过的 §15 七项清单（含宪法与 spec 的补充约束） |
| `cui-manual.md` | CUI 命令树、退出码表与 `--json` 契约（T119） |
| `tui.md` | TUI 启动/键位/六视图/主题与**未实现项**清单（T119，2026-10-23） |
| `web-deployment-security.md` | Web 监听边界、令牌与 Origin、端点清单、有界性、已知缺口（T119，2026-10-23） |
| `disaster-recovery.md` | 备份导出、文件级还原、compact 语义、清理保护、未接线能力（T119，2026-10-23） |
| `tui-manual-smoke.md` | TUI 人工冒烟协议（resize/Ctrl-C） |
| `ui-design.md` | 主题精确色值、墨水屏约束、i18n/RTL、前端预算 |
| `gui-suspend.md` | 窗口最小化/不可见时的挂起与恢复规则 |
| `resource-budget.md` | 内存/CPU/GPU/线程/句柄预算与回归门禁 |
| `performance-and-concurrency.md` | 并发设计档案、一手文献门槛、测量→复测流程 |
| `signal-chain-completeness.md` | 信号链完备原则（T116 复盘） |
| `test-model-sensenova.md` | 测试模型接入、图像输入与已知限制 |
| `bsd-interface.md` | BSD 平台接口预留契约（延后，不作验收结论） |
| `doctrine-application.md` | 工程准则应用记录（审计/改动/缺口） |
| `yike-philosophy.md` | 一刻哲学源流与工程翻译 |
| `knowing-before-doing.md` | 知难行易（宪法原则 X）：任务认知卡、门禁落点、反例与复审触发 |

## T119 交付记录（2026-10-23）

四份指南齐备：`cui-manual.md`（命令面与退出码）、`tui.md`（键位与未实现项）、
`web-deployment-security.md`（监听边界、令牌/Origin、端点、有界性）、
`disaster-recovery.md`（导出/还原/compact/清理保护）。

一致性口径：三份新指南逐条引用实现位置（`文件:行号`），并把**未实现/未接线**项单列
（如 TUI 无命令面板与翻页键、Web 无安全响应头/限速、无 restore 子命令），
不以设计意图替代当前行为。文档与实现出现分歧时，以实现为准并同步修订本文。

## 2026-10-25 更新（ADR-040）

`serve --web` 已默认启用入站 TLS（`--tls builtin|external|off`），工作区证书库
`<workspace>/.whcode/certs/` 与设置页 `/settings/certs` 见 `web-deployment-security.md`。
上文 T119 交付记录中的“Web 无 TLS”一项已随本次变更移除；安全响应头、限速、CSRF、
Host 头校验与令牌轮换仍是真实缺口。
