# CUI 手册（T119、§12.2）

**版本**：`0.1.0` | **更新**：2026-10-22 | 契约：`contracts/cui-cli.md`

## 1. 全局约定

- 所有 JSON 输出含 `schema_version`；错误对象恒四字段（`code`/`message`/`retryable`/
  `details`）；稳定退出码：成功 `0`，错误按类别 `10`（权限）/`20`（门禁）/`30`（运行时）/
  `40`（背压）/`50`（状态）/`60`（不存在）/`70`（I/O）/`80`（解析）/`90`（超时）/`99`（内部）。
- 非交互脚本路径可用（ADR-009）：`--json` + 退出码即可编程消费。
- 权限：全部命令经应用服务统一判定，拒绝不可重试（FR-604）。

## 2. 会话命令

```bash
whcode session new --title "修复登录"          # 创建（自动分配 S- 前缀 ID）
whcode session list [--all]                    # 列表（默认不含归档）
whcode session show --session S-1              # 元数据
whcode session rename --session S-1 --title "新标题"
whcode session archive --session S-1 [--unarchive]
whcode session delete --session S-1 [--dry-run] [--yes]   # 破坏性：默认只预览
```

删除有预览/确认/审计/活跃会话保护（FR-509）四项机制；删除当前打开的会话
须先把其 ID 从保护名单移除（应用层管理）。

## 3. 任务命令

```bash
whcode task list --session S-1
whcode task show --task T-1        # 依赖/步骤/产物引用/最近事件/失败原因
whcode task retry --task T-1       # 仅 failed/aborted/interrupted 可重试
whcode task abort --task T-1       # 终态与 unknown 不可中止（FR-410）
```

## 4. SDD 门禁

```bash
whcode spec status --session S-1    # 阶段（状态机：Constitution→Specify→Clarify→
                                    # Plan→Tasks→Analyze→Implement→Checklist→Converge）
whcode spec next --session S-1
whcode spec analyze --session S-1   # 五类缺陷（遗漏/孤儿/无效引用/循环依赖/覆盖不足）
whcode spec converge --session S-1  # 验收四态 + 差异报告
whcode spec force-advance --session S-1 --to plan --reason "理由"   # reason 必填
```

高优先级澄清未解决时 Plan 被拒；Analyze 未通过时 Implement 被拒（FR-307）——
仅提示词要求不构成门禁（FR-308）。

## 5. 事件

```bash
whcode events list --session S-1 --after 100
whcode events export --session S-1 --after 0 --format jsonl   # 首行头部注明脱敏策略
```

`redacted`（默认）级别下疑似秘密字段替换为 `[REDACTED]`（FR-203/205、D-05）。

## 6. 数据库维护

```bash
whcode db stats                     # 文件大小/WAL/已用块/可用块（FR-507/510）
whcode db check                     # 必需索引检查
whcode db purge --before 1700000000000 [--dry-run] [--yes]   # 清理（预览/确认/审计/活跃保护）
whcode db checkpoint                # 显式 checkpoint
whcode db compact [--dry-run] [--yes]   # 复制—校验—切换（不承诺缩小文件，FR-510）
whcode db archive --export PATH     # 工作区导出（whcode-export v1）
whcode db explain --query "SELECT ..."   # 只读查询的查询计划
```

`db vacuum` **不存在**：磁盘压缩能力命名为 `db compact`（FR-510、R-09）。

## 7. 诊断

```bash
whcode doctor [--json]              # 环境与配置自检
```
