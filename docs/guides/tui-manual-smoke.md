# TUI 人工冒烟协议（V-003）

**日期**：2026-10-21（2026-10-22 随六视图接线更新）| **适用**：Windows 11 amd64 真实控制台
| **关联**：T024、T082、V-003、ADR-016

自动冒烟已覆盖：启动首帧、键盘计数、`q` 退出、强杀后可读（`script/tui_manual_smoke.py`）；
六视图切换与固定帧取证（`script/tui_views_smoke.py`，产物 `artifacts/tui-views-smoke/`）。
本协议只补齐自动化无法可靠完成的两项：**终端 resize 重绘**与 **Ctrl-C 退出**。

## 1. 前置

- 使用 Windows Terminal 或 conhost，窗口可自由拖动缩放；
- 工作区根目录执行；
- 不通过管道重定向 stdout/stderr（TUI 需要真实控制台）。

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
cargo run -p whcode-bin --release
```

主题可用环境变量覆盖（默认 `eink-day`）：`$env:WHCODE_UI_THEME = "amoled-night"`。

## 2. 步骤与判据

| # | 操作 | 期望 | 记录 |
|---|---|---|---|
| 1 | 启动 | 首帧在一次重绘内出现，标题为「whcode —— 会话（1/6）」，无闪烁循环 | 截图/录屏 |
| 2 | 按 `1`—`6` | 标题序号与视图名（会话/任务/上下文/事件/Spec 状态/diff 预览）逐一匹配；导航键不计入页脚按键数 | 六帧标题 |
| 3 | 按 `a`、`z` 等字符键 | 页脚按键计数增加；不处理方向键与选中项 | 计数值 |
| 4 | 拖动窗口边缘改变尺寸 | 布局在 resize 后重绘（页脚尺寸更新），无花屏、无挂起 | 修改前后尺寸 |
| 5 | 按 `Ctrl-C` | 退出事件循环，退出码 0，终端状态恢复 | 退出码 |
| 6 | 再次启动，按 `q` | 正常退出，退出码 0 | 退出码 |
| 7 | 运行中最小化窗口 ≥10s 后还原（可选） | 最小化期间不重绘；还原后单次全量重绘、内容正确 | 观察记录 |

## 3. 证据格式

```text
日期：
提交：
终端：Windows Terminal / conhost，版本
窗口：起始 120x30 → 结束 90x24
步骤 4：通过/失败；失败现象：
步骤 5：退出码；终端是否恢复：
步骤 6：退出码：
备注：
```

证据写入 `artifacts/tui-manual-<日期>.txt`，并在 `checklist.md` 对应 Check 勾选前复核。
未取得记录时，V-003 保持“未通过”；不得以自动冒烟代替人工项。
桌面自动化记录见 `artifacts/tui-manual-2026-10-21.txt`；用户 2026-10-21 复核接受为 V-003 关闭依据。
六视图自动化取证见 `artifacts/tui-views-2026-10-22.txt`（同法可重跑）。

## 4. 已知限制

- conhost 对程序化 `SetConsoleScreenBufferSize` 可能返回错误 87；人工拖动窗口不受此限；
- 若 resize 后布局不重绘，记录为缺陷并附窗口尺寸与终端类型，不标记通过。
