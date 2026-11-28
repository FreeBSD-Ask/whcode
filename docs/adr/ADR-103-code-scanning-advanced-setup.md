# ADR-103：Code Scanning 切换 advanced setup 与威胁模型收窄

- **状态**：已批准（2026-11-28，用户指令“切换到 advanced setup”；威胁模型口径经当场
  提问确认“默认 remote＋仍修完 path”）
- **关联**：AGENTS §5.2（Code Scanning 零告警铁律）、§5.1（CI action 版本铁律）；
  ADR-037（CI 测试验收入口唯一）；ADR-085（融洽性计算）
- **上游依据**：GitHub 文档 [Customizing your workflow configuration](https://docs.github.com/en/code-security/reference/code-scanning/workflow-configuration-options)
  与 [CodeQL 威胁模型说明](https://codeql.github.com/docs/codeql-language-guides/customizing-library-models-for-rust)：
  advanced setup（仓库自管工作流）默认只启用 `remote` 威胁模型；default setup 的
  `threat_model` 可配置为 `remote_and_local`，本次切换前实测其值为 `remote_and_local`、
  `query_suite` 为 `extended`。

## 问题

2026-11-28 CodeQL 分析一次产生 683 条 open 告警（此前为 0）。全部 source 属本地威胁
模型：`std::env::temp_dir` 603 条、`current_dir` 42 条、`env::var(_os)` 11 条、
`entry.path()` 3 条、文件读取 13 条、`current_exe` 3 条、stdin 1 条等。default setup 由 GitHub 托管：查询集与威胁模型不可由仓库固定，
规则/模型随上游推送变化，版本不可审计、不可复现（与 §5.1「最新版本＋固定 SHA」的
CI 纪律不对齐），一次上游更新即可让零告警基线翻红且无法回退。

## 决策

1. **切换 advanced setup**：新增 `.github/workflows/codeql.yml`（`github/codeql-action`
   v4.39.5，commit SHA 固定并登记 `action-versions.json`），语言 `rust`＋`javascript`＋
   `actions`，`build-mode: none`，`queries: security-extended`（对齐原 default setup 的
   `query_suite=extended`）。经 REST API（`PATCH /code-scanning/default-setup`，
   `state: not-configured`）关闭 default setup，当前状态已复核为 `not-configured`。
2. **威胁模型取 advanced setup 默认 `remote`**（用户批准）：`remote_and_local` 收窄为
   `remote`，本地源类别（环境变量、临时目录、本地文件读取）不再作为污染源。**此变化
   登记为分析范围收窄，不得表述为“已修复”**；`threat-model` 若改回 `remote,local`
   必须用户批准并更新本 ADR。
3. **本地源类别的代码级收口继续推进**（与告警压力解耦，用户指令“仍修完 path”）：
   `..` 字符串清洗覆盖全部 `temp_dir`/`current_dir` 派生路径构造；生产 env 流非法即
   拒绝/回退静态值；`entry.path()` 遍历项清洗；`current_exe`→`Command` canonicalize＋
   允许列表；日志外泄面 bool 化或换行剥离；反序列化生产入口补字节上限（CWE-502 DoS
   防御）。这些修复有独立的安全价值（遍历/符号链接/日志伪造/内存 DoS），不因告警
   消失而回退。
4. **门禁脚本修复**：`check-actions.py` 的 `uses:` 正则支持三层路径
   （`owner/repo/sub@SHA`——此前 `github/codeql-action/init@…` 整体漏扫）；`latest_tag`
   在 `releases/latest` 返回非语义化 tag（如 `codeql-bundle-v2.27.6`）时回退到 tags 中
   最高语义化版本。

## 正负和解

- **正面**：CodeQL 版本、查询集、触发条件、威胁模型全部收进仓库工作流，符合 §5.1 可审
  计要求；上游模型更新不再自动改变仓库告警基线；本地复核脚本与 CI 同源。
- **负面**：本地源类别告警在 remote 模型下不再产生——**零告警不再等价于“本地源类别
  无缺陷”**。承担者：后续新增本地路径/环境变量处理时，评审需按 AGENTS §5.2 的代码级
  收口要求执行（`..` 清洗、canonicalize＋前缀检查等），不能只依赖告警不报。
- **和解理由**：仓库是本地优先单用户工具（Windows amd64 验收），本地源类别的威胁
  模型以“用户自己的环境”为前提；上游不可控的模型变更代价高于保留 `remote_and_local`
  的边际收益。真实缺陷以代码级收口处置，不靠分析配置兜底。

## 已知限制与失效条件

- ①advanced setup 的 threat model 配置依赖 codeql-action 的输入名（`threat-model`），
  上游改名/移除 → 按工作流首跑失败复审；
- ②`build-mode: none` 对 Rust 的提取覆盖依赖 CodeQL 版本行为，若上游变更（如要求
  autobuild）→ 工作流失败即复审；
- ③首次工作流运行的 CI 结论待回填（Check 505 未勾选前不得声称零告警已恢复）；
- ④本地源类别的代码级收口为“当前和解”，非永恒基线（AGENTS §1.2 第 5 条）。

## 复审触发（绑定命令）

1. `python .github/scripts/check-actions.py` 或 `check-workflows.py` 失败 ⇒ 复审工作流与清单。
2. `.github/workflows/codeql.yml` 的 `run` 在六个月内无成功记录（GitHub API 查
   `repos/<o>/<r>/actions/workflows/codeql.yml/runs?status=success`）⇒ 复审可用性。
3. `python .github/scripts/check-code-scanning.py` 报 open 告警 ⇒ 按 §5.2 修复，不得回头
   放宽威胁模型。
4. 用户或上游变更 `threat_model`／`query_suite` 语义 ⇒ 复审本 ADR 的对应条款。

## 影响

- 新增：`.github/workflows/codeql.yml`；`action-versions.json` 登记 `github/codeql-action`；
  AGENTS §5.2 增补分析配置条款；CHANGELOG 记录。
- 不变：AGENTS §5.2 的“零 open 告警”要求与“禁止 dismiss”纪律；`check-code-scanning.py`
  仍以 GitHub API 为唯一事实源。
