# GUI 挂起与图形稳定性设计（最小化不绘制）

**日期**： 2026-10-21 | **对应需求**： FR-617 | **适用端**： Web UI（M6）、TUI（M5）、未来任何 GUI

目标：窗口最小化/不可见时**不做任何 GUI 工作**（不绘制、不动画、不轮询、不占 CPU），
恢复时**一次性重建画面**；同时避免“类 Linux 桌面栈”常见的图形不稳定（驱动差异导致的
花屏、上下文丢失崩溃、合成层滥用导致的内存暴涨、事件抖动导致的重建风暴）。

## 1. 权威依据

| 依据 | 要点 |
|---|---|
| MDN Page Visibility API | 后台标签页中浏览器**停止调用 `requestAnimationFrame`**，`setTimeout` 被节流；应用应监听 `visibilitychange` 主动挂起，而不是依赖节流 |
| Chrome for Developers（后台标签页） | 明确建议用 Page Visibility API 挂起“所有不必要的视觉更新” |
| WebKit“How Web Content Can Affect Power Usage” | 后台应做到**零 CPU**；同时监听 `blur/focus` 以覆盖“可见但未聚焦”的情形 |
| CSS 渲染流水线（Style→Layout→Paint→Composite） | 只有 `transform`/`opacity` 触发纯合成；`will-change` 会创建合成层并占用显存，**按需使用、用完撤销** |

## 2. 状态机（唯一渲染循环）

```text
Active ──(不可见/最小化/blur)──▶ Suspended
  ▲                                │
  └──(恢复可见 + 去抖 ≥250ms)── Recovering ──(一次全量重绘)──▶ Active
```

- **Active**：正常渲染循环（事件驱动 + 脏标记，不做无差别逐帧重绘）。
- **Suspended**：**取消** `requestAnimationFrame`、所有 `setTimeout/setInterval` 动画定时器、
  前端主动轮询与流式渲染合批；只保留必要的**事件订阅**（由应用服务推送持久化事件），
  且订阅回调只入队、不渲染。
- **Recovering**：恢复时**先取快照**（`state-snapshot` 契约），再丢弃挂起期间累积的中间帧，
  **只做一次全量重绘**；禁止“补间追赶”（会一次性触发大量布局/合成，正是卡顿与花屏的来源）。

**不可见判定**：`document.visibilityState === "hidden"` **或** `window` 失去焦点且最小化
（`outerWidth/outerHeight` 退化或平台 API 报告最小化）。以 `visibilitychange` 为主，
`blur/focus` 为辅；两者任一触发挂起，**全部恢复条件成立**才回到 Active。

**去抖**：`visibilitychange` 在部分平台会连续触发（拖动窗口、切换虚拟桌面）。恢复必须去抖
（≥250 ms，取最后一次），避免反复重建渲染上下文。

## 3. 稳定性约束（针对“类 Linux 图形不稳”）

1. **不假设 GPU 可用**：渲染只用标准 DOM/CSS；任何 canvas/WebGL 路径必须先 feature-detect，
   失败即降级为 2D 或纯 DOM，**不得**因缺少特性而崩溃或空白。
2. **不依赖 vsync 语义**：不使用“每帧必达”的假设；渲染由事件与脏标记驱动。
3. **上下文生命周期显式化**：若使用 WebGL/Canvas2D 上下文，挂起时**主动释放**，恢复时重建；
   监听 `webglcontextlost` 并降级，避免驱动重置导致的白屏或崩溃。
4. **合成层受控**：动画只改 `transform` / `opacity`；`will-change` 仅在动画开始前设置、
   结束后移除；禁止给长列表或全页面加 `will-change`（显存与内存陷阱）。
5. **动效默认最少**：尊重 `prefers-reduced-motion`；提供“动效总开关”，关闭后不创建任何
   动画定时器（与 FR-613 一致）。
6. **不引入平台专属图形栈**：不捆绑驱动、不要求特定合成器；Web 端只用浏览器标准能力，
   桌面端（未来若引入）通过平台层抽象并与 `whcode-platform` 隔离（NFR-101）。
7. **资源上限**：挂起状态下不得持有大缓冲（解码帧、离屏画布）；释放后 RSS 应可观测下降。

## 4. TUI 的对应规则

终端没有“最小化事件”的统一抽象，但 Windows 控制台最小化时：
- crossterm 不再稳定投递 resize 事件；
- 向最小化窗口绘制可能阻塞或产生乱序转义序列。

因此 TUI 需要：
1. 在 `whcode-platform` 提供 `window_is_minimized()`（Windows：`GetConsoleWindow` +
   `IsIconic`；其他平台返回 `false` 或对应能力）；
2. 渲染门控：最小化期间**跳过绘制**，但继续处理**关键事件**（状态转换必须记账，不能丢）；
3. 恢复（窗口还原）时执行**一次全量重绘**，丢弃积压的展示快照（与 ADR-006 的“快照可合并”一致）。

## 5. 验收判据

- 最小化 60 秒：CPU 采样接近 0（不高于空闲基线），无 `requestAnimationFrame` 调用；
- 恢复：单次全量重绘，无花屏、无重复内容；事件游标连续，关键事件零丢失；
- 连续最小化/还原 20 次：无渲染上下文泄漏（RSS 增量有界）、无崩溃；
- 关闭动效开关或 `prefers-reduced-motion: reduce`：不创建动画定时器；
- 禁用 GPU（浏览器软件渲染）时功能可用、无崩溃。
