# 域 P：工程过程与质量门禁

> ZCode 证据路径以 `local/zcode-src`（commit 3c5b227）为根，以下省略前缀，实际绝对路径为
> `D:\WHCode\local\zcode-src\...`。

## 1. 关键路径勘误

- **CI 不是 GitHub Actions workflow**：ZCode 用 **husky pre-push + `verify:pre-push`** 脚本
  （`package.json` 的 `"verify:pre-push"`），无 `.github/workflows/` CI 流水线。
- `reviewRequired` 实际为 **19**（非任务列的 25）：`architecture-policy.yaml` 的
  `reviewRequired` 字段列出 19 条规则需人工 review。
- 豁免条目实际 **29 条**（`architecture-policy.yaml` 的 `exemptions`）。

## 2. 对照表

| 项 | ZCode | WHCode | 判定 |
|---|---|---|---|
| 架构策略 | `architecture-policy.yaml`：`maxFileLines=800`、`maxContractLines=300`、`maxPublicMethods=12`、`forbidCycles`、`forbidDeepImports`；29 条豁免 | `test_layering`（分层/平台分支/前端不触库）+ `test_layout`（目录分类） | WHCode 门禁可执行、有契约测试；ZCode 用策略文件+检查脚本 |
| 架构检查 | `scripts/architecture/architecture-check.mjs` 在 pre-push 运行 | `cargo test -p whcode-tests --test layering/layout` | 相当；WHCode 在 CI 而非仅本地 |
| 质量门禁 | husky pre-push + `verify:pre-push`（本地钩子，无 CI 强制） | CI（`.github/workflows/ci.yml`）为唯一验收入口（ADR-037） | **WHCode 先手**：CI 强制+可复现 |
| 评审规则 | `reviewRequired` 19 条（需人工 review） | G0—G5 门禁（软件工程过程规范）+ 用户批准 | WHCode 门禁体系更完整 |
| ADR | **无 ADR**（无决策记录机制） | ADR-001—045 体系 | **WHCode 先手**：决策可追溯 |
| 三方追溯 | 无 FR↔任务↔Check 追溯 | `traceability.py` + `check_counts.py` | **WHCode 先手**：需求可追溯 |
| Action 固定 SHA | 无 GitHub Actions | `action-versions.json` + `check-actions.py`（最新 release + 固定 SHA） | **WHCode 先手**：供应链可审计 |
| 提交门禁 | husky pre-push（本地可绕过） | trunk-based + 小步提交 + 用户授权 | 相当；WHCode 更强制 |

## 3. 与 WHCode 差异修正（相对原差异书）

- **修正**：ZCode CI 是 husky pre-push + `verify:pre-push`，非 GitHub Actions workflow。
- **修正**：`reviewRequired` 实际 19 条（非 25）；豁免实际 29 条。
- **补充**：ZCode **无 ADR、无三方追溯、无 action 固定 SHA**——工程过程上 WHCode 全面先手
  （决策记录、需求追溯、供应链审计均为 WHCode 独有）。

## 4. 存疑项

- `verify:pre-push` 是否包含架构检查脚本，还是仅 `tsc`/`lint`/`test`，需读 package.json 确认
  `[待验证]`。
- husky pre-push 的 `--no-verify` 绕过是否被 CI 阻止（无 CI 则无法阻止）`[待验证]`。
