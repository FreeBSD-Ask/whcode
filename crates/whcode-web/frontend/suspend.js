// Web 前端挂起状态机（T136、FR-617）。
//
// 状态：Active / Suspended / Recovering。判定：`visibilitychange` + `blur/focus`；
// 恢复去抖 ≥250 ms（FR-617）；恢复时取快照并**丢弃中间帧**（仅保留最后一次
// 待处理快照）；WebGL/Canvas 上下文释放与重建：WHCode 前端**不使用** WebGL/Canvas
// （ADR-014 极简、FR-618 禁止），GPU 特性探测失败时自动降级为纯文本渲染（无降级
// 损失）；`prefers-reduced-motion` 与动效总开关（T133 已实现，此处暴露查询接口）。

export const MIN_RESUME_DEBOUNCE_MS = 250;

/** 挂起状态。 */
export const State = Object.freeze({
  Active: "active",
  Suspended: "suspended",
  Recovering: "recovering",
});

/**
 * 挂起状态机（FR-617）。
 *
 * 事件驱动：`visibilitychange`（hidden → Suspended；visible → 恢复去抖）与
 * `blur`/`focus`（同一判定，处理虚拟键盘等场景）。恢复去抖 ≥250 ms：去抖窗口内
 * 产生的帧全部丢弃，仅保留最后一次待处理快照；窗口到期后取一次快照单次全量恢复。
 */
export class SuspendMachine {
  #state = State.Active;
  #debounceTimer = null;
  #pendingSnapshot = null;
  #listeners = new Set();
  #onResume; // (snapshot) => void：恢复时取一次快照（调用方注入）。

  /**
   * @param {(snapshot: unknown) => void} onResume 恢复去抖到期后的单次全量恢复回调。
   */
  constructor(onResume) {
    this.#onResume = onResume;
  }

  /** 当前状态（可查询，FR-617）。 */
  get state() {
    return this.#state;
  }

  /** 注册状态变更监听。 */
  onChange(listener) {
    this.#listeners.add(listener);
  }

  #notify() {
    for (const listener of this.#listeners) {
      listener(this.#state);
    }
  }

  /** 页面隐藏/失焦：进入 Suspended，待处理帧保留最后一份。 */
  suspend(pendingSnapshot = null) {
    if (this.#state === State.Suspended) {
      return;
    }
    if (this.#debounceTimer !== null) {
      window.clearTimeout(this.#debounceTimer);
      this.#debounceTimer = null;
    }
    this.#state = State.Suspended;
    // 丢弃中间帧：仅保留最后一次待处理快照（FR-617）。
    this.#pendingSnapshot = pendingSnapshot;
    this.#notify();
  }

  /** 页面可见/聚焦：恢复去抖 ≥250 ms（FR-617），到期后单次全量恢复。 */
  resume(pendingSnapshot = null) {
    if (pendingSnapshot !== null) {
      this.#pendingSnapshot = pendingSnapshot;
    }
    if (this.#state === State.Recovering || this.#debounceTimer !== null) {
      return; // 去抖窗口内重复恢复请求：仅刷新待处理快照。
    }
    this.#state = State.Recovering;
    this.#notify();
    this.#debounceTimer = window.setTimeout(() => {
      this.#debounceTimer = null;
      const snapshot = this.#pendingSnapshot;
      this.#pendingSnapshot = null;
      this.#state = State.Active;
      this.#notify();
      // 单次全量恢复：取一次快照，丢弃中间帧（FR-617）。
      this.#onResume(snapshot);
    }, MIN_RESUME_DEBOUNCE_MS);
  }

  /** 绑定页面事件（visibilitychange + blur/focus）。 */
  attach(getPendingSnapshot = () => null) {
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) {
        this.suspend(getPendingSnapshot());
      } else {
        this.resume(getPendingSnapshot());
      }
    });
    window.addEventListener("blur", () => this.suspend(getPendingSnapshot()));
    window.addEventListener("focus", () => {
      // 可见性优先：`document.hidden` 为真时不得因 focus 恢复。窗口最小化期间
      // 可能残留焦点事件（浏览器/自动化驱动均观测到），此前会误把挂起机拉回
      // Active，导致 hidden 期间继续轮询（FR-617 实测缺陷，2026-10-24）。
      if (!document.hidden) {
        this.resume(getPendingSnapshot());
      }
    });
  }

  /** 解绑（测试与卸载）。 */
  detach() {
    if (this.#debounceTimer !== null) {
      window.clearTimeout(this.#debounceTimer);
      this.#debounceTimer = null;
    }
    this.#pendingSnapshot = null;
  }
}

/** GPU 特性探测（T136/FR-617）：WHCode 不使用 WebGL/Canvas（ADR-014/FR-618）——
 * 探测返回 `null` 表示无 GPU 依赖，无需降级；若未来引入 Canvas，探测失败时
 * 必须降级为纯文本渲染。 */
export function probeGpu() {
  try {
    const canvas = document.createElement("canvas");
    const hasWebGl = Boolean(canvas.getContext("webgl"));
    canvas.width = 0;
    canvas.height = 0;
    return hasWebGl ? "webgl-available" : "no-webgl";
  } catch {
    return "probe-failed";
  }
}

/** 动效总开关查询（T133/FR-613 已实现，此处为挂起机恢复动画的统一入口）。 */
export function animationsEnabled() {
  return window.matchMedia("(prefers-reduced-motion: no-preference)").matches;
}
