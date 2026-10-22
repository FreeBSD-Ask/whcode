# docs/guides

使用者与贡献者指南（tasks.md T007、T119）。

## 已建立

| 文件 | 内容 |
|---|---|
| `philosophy.md` | 核心哲学与工程方法论（形式审查、正负和解、三重时间、0 信任） |
| `design-principles.md`、`module-design.md` | 设计原则与 13 个 crate 的职责/依赖/失败模式 |
| `software-engineering-process.md` | G0—G5 阶段门禁、角色合并、产物映射 |
| `requirements-engineering.md` | FR 模板、需求质量判据、三方追溯抽查方法、变更控制 |
| `repository-layout.md` | 目录分类原则（由 `tests/contract/test_layout.rs` 强制） |
| `pr-checklist.md` | 每 PR 必过的 §15 七项清单（含宪法与 spec 的补充约束） |
| `cui-manual.md` | CUI 命令树、退出码表与 `--json` 契约（T119 已交付部分） |
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

## 未交付（T119 复核结论，2026-10-23）

以下三份文档**尚不存在**，T119 因此未完成（Check 119 未通过）：

- TUI 键位表（当前键位散见 `crates/whcode-tui/src` 与 `ui-design.md`，未成独立表）；
- Web 部署与安全配置指南（默认回环绑定、局域网认证/Origin/TLS 配置；现仅有 ADR-022/023）；
- 灾难恢复说明（备份、导出/导入、迁移失败与 compact 回滚流程）。

恢复条件：三份文档补齐并与实现逐条对照后，重跑 Check 119。
