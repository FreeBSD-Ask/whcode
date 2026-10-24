# 工程准则应用记录（WHCode 试点）

**日期**：2026-10-21 | **准则版本**：`engineering-doctrine/`（00—07 + 模板） | **周期**：应用 → 回流 → 复审

本文件记录把 `engineering-doctrine/` 应用到 WHCode 的审计、改动、缺口与回流结论。
它不是验收通过声明：未取得证据的项仍标缺口。

## 1. 审计矩阵

| 准则 | 项目现状 | 结论 |
|---|---|---|
| 00 哲学 | `philosophy.md`、`yike-philosophy.md`、AGENTS 1.2—1.4 | 已落地；本轮补证据层级 |
| 01 过程 | G0—G5、tasks/checklist、门禁脚本 | 已落地；G4 补三次复查 |
| 02 需求 | FR/NFR、追溯、变更控制 | 已落地；补证据层级与三次复查 |
| 03 设计 | design-principles、crate 分层、layering 门禁 | 已落地 |
| 04 并发/性能 | ADR-015、performance 指南、无并发实现 | 规则已落地；缺真实档案样例 |
| 05 安全/自由 | NFR-218/219、SECURITY.md、UI §11 | 部分落地；缺威胁模型逐项与退出闭环 |
| 06 UI/资源 | ui-design、ADR-014、预算 | 规则已落地；Web 未实现 |
| 07 交付 | repository-layout、PR 清单、EOL、CHANGELOG | 已落地；本轮补计数门禁 |

## 2. 本轮应用

1. **计数门禁**：新增 `.github/scripts/check_counts.py`，从 `spec.md`/`tasks.md`/`checklist.md` 计算
   FR/任务/Check 实际数量，并核对文档声明；接入 `.github/workflows/platform-matrix.yml`（platform 矩阵中的 windows 条目）。
2. **证据层级与三次复查**：写入 AGENTS §4.2、过程 G4、需求工程 §3.1、PR 清单。
3. **可逆性分级**：可逆决策快速验证；不可逆决策（数据格式、协议、权限、并发内存序）
   设计先行并留回退证据。
4. **登记**：T148、Check 249；计数同步为 148 任务 / 249 检查点。

## 3. 回流到准则

| 经验 | 回流位置 |
|---|---|
| 计数会漂移，手工同步不可靠 | `01-process.md` §8、`07-delivery.md` §3 |
| 文档会与磁盘不一致，以命令结果为准 | `01-process.md` §8 |
| “设计先行”与交付压力冲突时，不得用原型先行绕过 | `01-process.md` §8、`templates/pr-checklist.template.md` |
| AI 总结不能作为唯一证据 | `01-process.md` §8、PR 模板 |
| FR/任务/Check 需要单一计数来源 | `07-delivery.md` §3 |
| 文档需要假设、适用范围与复审日期 | `07-delivery.md` §3 |

## 4. 剩余缺口

| 缺口 | 影响 | 下一动作 |
|---|---|---|
| 退出/导出/离线无端到端测试 | NFR-218 无法判定 | M1/M2 增加导出—恢复—离线闭环 |
| 威胁模型未逐项成文 | NFR-219 只能部分判定 | Web/SSH 边界补威胁模型 |
| 并发设计档案无样例 | NFR-216 缺可复用样例 | 首个并发任务按模板产出并归档 |
| 文档复审日期未逐项登记 | NFR-222 只能部分判定 | 从 M1 起为 ADR/指南补复审字段 |
| `.github/scripts/check_counts.py` 未覆盖所有计数位置 | 仍可能局部漂移 | 按失败案例追加模式 |

## 5. 下一周期触发

- 新增里程碑/版本；
- 新增并发、网络或 UI 实现；
- 计数门禁失败或文档与磁盘不一致；
- 准则目录新增/修订条款；
- 用户要求复审。

复审时重跑本矩阵，更新缺口状态；不得把“已应用”当作“已通过”。
