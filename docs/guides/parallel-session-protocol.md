# 双会话并行协作协议（共享索引）

**状态**：已批准（2026-10-29，建议书 proposal-v2 D7）| **实证**：2026-10-29 三次险情
（见 [`research/synthesis/proposal-v2.md`](../../research/synthesis/proposal-v2.md) §5）

本仓库存在**多会话共享同一工作区与 git 索引**的运行形态（`git status` 可见彼此未提交
改动、`git log` 交错）。本协议把已验证有效的做法成文化，凡并行会话一律遵守。

## 1. ADR 编号认领

- 开新 ADR **前**先 `ls docs/adr/` 核对最大编号；起草与提交**同批完成**（缩短占用窗口，
  不做占位空提交）。
- 提交前发现他人新 ADR 落地，**顺延自己的编号**并更新文中自引（2026-10-29 实例：
  工具系统 ADR 因发布线占用 047 而顺延为 048）。

## 2. 任务认领

- 批次开工前在**认知卡**（宪法原则 X，ADR-049）写明本批任务号与文件落点；
- 两个会话不认领同一批任务；跨线文件（如 `CHANGELOG.md`、`spec.md` 计数行）提交前
  先 `git log --oneline -5 -- <file>` 核对有无他人并行改动。

## 3. 提交纪律

- **pathspec 提交**：共享索引下禁止 `git add -A`/`git add .`——只暂存本批文件，
  提交前 `git status --short` 复核暂存清单里没有他人的在途改动；
- 提交信息按 Conventional Commits（AGENTS.md §9）；本机门禁全绿再提交，
  推送后跑 `check-code-scanning.py` 复核（AGENTS §5.2）；
- 不改动他人未提交的工作区文件（发现他人在途改动时绕行或等待，不覆盖）。

## 4. 产物卫生

- 构建缓存、探测脚本、对照产物一律进 `local/` 或 `CARGO_TARGET_DIR`——**仓库根只允许
  布局白名单目录**（2026-10-29 实例：杂散 `target-verify/` 目录阻断 `test_layout` 门禁）；
- 临时脚本用后由 `clean.py --scope script` 回收，长期验证工具在脚本内 `KEEP_ALWAYS`
  显式保留。

## 5. 修订记录

- **2026-10-29（首版）**：proposal-v2 D7 批准后入库；四条规则均有当日实证。
