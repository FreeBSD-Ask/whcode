// WHCode Web 前端（T093/T094，原生 ESM，无构建链——ADR-014）。
// 断线重连与按游标补齐（T094/FR-605）：轮询 /api/events 以游标增量拉取，
// 连接失败指数退避重连，恢复后从上次游标继续（不重复、不遗漏）。

const state = {
  cursor: 0,
  session: null,
  backoffMs: 500,
  backoffMaxMs: 8000,
  connected: false,
};

import { SuspendMachine, State, probeGpu } from "./suspend.js";

const el = (id) => document.getElementById(id);

function setConnected(connected) {
  state.connected = connected;
  el("connection-state").textContent = connected ? "已连接" : "未连接";
}

// 动效总开关（T133/FR-613）：尊重 prefers-reduced-motion，默认关闭非必要动效。
const animationsEnabled =
  window.matchMedia("(prefers-reduced-motion: no-preference)").matches;

async function fetchJson(url) {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`HTTP ${response.status}`);
  }
  return response.json();
}

// 会话清单（FR-101）。
async function loadSessions() {
  const data = await fetchJson("/api/sessions");
  const list = el("session-list");
  list.textContent = "";
  for (const session of data.sessions ?? []) {
    const item = document.createElement("li");
    item.textContent = `${session.id} ${session.title} [${session.stage}]`;
    list.append(item);
  }
}

// 事件增量（T094/FR-605）：从游标补齐，恢复后不重复不遗漏。
async function pollEvents() {
  if (!state.session) {
    return;
  }
  const data = await fetchJson(
    `/api/events?session=${encodeURIComponent(state.session)}&after=${state.cursor}`,
  );
  const log = el("event-log");
  for (const event of data.events ?? []) {
    log.textContent += `${event.seq} ${event.kind}\n`;
    state.cursor = event.seq;
  }
  el("event-cursor").textContent = `游标 ${state.cursor}`;
}

// 状态快照（FR-314）。
async function loadStatus() {
  if (!state.session) {
    return;
  }
  const data = await fetchJson(
    `/api/status?session=${encodeURIComponent(state.session)}`,
  );
  el("spec-status").textContent = JSON.stringify(data, null, 2);
}

// 轮询循环：失败退避重连（T094/FR-605），恢复后重置退避。
async function pollLoop() {
  try {
    await loadSessions();
    await pollEvents();
    await loadStatus();
    if (!state.connected) {
      state.backoffMs = 500;
    }
    setConnected(true);
  } catch {
    setConnected(false);
    state.backoffMs = Math.min(state.backoffMs * 2, state.backoffMaxMs);
  }
  window.setTimeout(pollLoop, state.connected ? 1000 : state.backoffMs);
}

// 标签切换（会话/任务/历史/SDD）。
for (const button of document.querySelectorAll("#tabs button")) {
  button.addEventListener("click", () => {
    for (const other of document.querySelectorAll("#tabs button")) {
      other.setAttribute("aria-selected", "false");
    }
    button.setAttribute("aria-selected", "true");
    for (const section of document.querySelectorAll("main section")) {
      section.hidden = true;
    }
    el(`view-${button.dataset.tab}`).hidden = false;
    if (animationsEnabled) {
      // 仅 opacity（GPU 合成友好，T133）；reduced-motion 时无动效。
      el("main").style.opacity = "0.6";
      window.setTimeout(() => {
        el("main").style.opacity = "";
      }, 150);
    }
  });
}

document.addEventListener("DOMContentLoaded", () => {
  const params = new URLSearchParams(window.location.search);
  state.session = params.get("session");

  // 挂起状态机（T136/FR-617）：visibilitychange + blur/focus 判定；恢复去抖
  // ≥250 ms；恢复时取一次快照并丢弃中间帧（轮询游标即快照——不重复不遗漏）。
  const machine = new SuspendMachine(() => {
    el("connection-state").textContent = "恢复中…";
    loadStatus();
    pollEvents();
  });
  machine.attach(() => state.cursor);
  machine.onChange((newState) => {
    if (newState === State.Active && state.connected) {
      setConnected(true);
    }
  });
  window.addEventListener("unload", () => machine.detach());

  pollLoop();
});
