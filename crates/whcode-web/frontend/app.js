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

// T136/FR-617：只接线挂起状态机；`probeGpu` 不被运行时路径调用——
// WHCode 前端不使用 Canvas/WebGL（ADR-014/FR-618），运行时调用探测即创建 GPU 上下文，
// 违反 NFR-212（默认不创建）。未来引入 Canvas 时按 gui-suspend §3.1 先探测后降级。
import { SuspendMachine, State } from "./suspend.js";

const el = (id) => document.getElementById(id);

// 文本层（FR-615，批次 3）：服务端 /api/i18n 下发 zh-CN/en 词条。
// 词条缺失时回落内置默认文本（可见降级，不显示键名）。
let texts = {};

function t(key, fallback, args = {}) {
  let text = typeof texts[key] === "string" ? texts[key] : fallback;
  for (const [name, value] of Object.entries(args)) {
    text = text.split(`{${name}}`).join(String(value));
  }
  return text;
}

/** 加载文本层并应用到静态标签（data-i18n）与 <html lang/dir>。 */
async function loadLocale() {
  // 语言选择：显式 `?locale=` 优先（可见、可改、可撤销），否则浏览器语言。
  const locale =
    new URL(window.location.href).searchParams.get("locale") ||
    navigator.language ||
    "zh-CN";
  const data = await fetchJson(`/api/i18n?locale=${encodeURIComponent(locale)}`);
  texts = data.entries || {};
  document.documentElement.lang = data.locale || locale;
  document.documentElement.dir = data.rtl ? "rtl" : "ltr";
  for (const node of document.querySelectorAll("[data-i18n]")) {
    const key = node.dataset.i18n;
    if (typeof texts[key] === "string") {
      node.textContent = texts[key];
    }
  }
  setConnected(state.connected);
  updateCursorLabel();
}

function updateCursorLabel() {
  el("event-cursor").textContent = t("web.cursor", "游标 {n}", { n: state.cursor });
}

function setConnected(connected) {
  state.connected = connected;
  el("connection-state").textContent = connected
    ? t("web.conn.connected", "已连接")
    : t("web.conn.disconnected", "未连接");
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

// 会话清单（FR-101）：条目可点击选择（批次 1）；渲染按签名去重（避免每秒重建 DOM）。
let lastSessionsSignature = "";

async function loadSessions() {
  const data = await fetchJson("/api/sessions");
  const sessions = data.sessions ?? [];
  const signature = JSON.stringify(
    sessions.map((s) => [s.id, s.title, s.stage, s.status, s.archived]),
  );
  if (signature === lastSessionsSignature) {
    updateSessionHighlight();
  } else {
    lastSessionsSignature = signature;
    const list = el("session-list");
    list.textContent = "";
    for (const session of sessions) {
      const item = document.createElement("li");
      item.textContent = `${session.id} ${session.title} [${session.stage}]`;
      item.dataset.session = session.id;
      item.tabIndex = 0;
      item.setAttribute("role", "button");
      item.addEventListener("click", () => selectSession(session.id));
      item.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          selectSession(session.id);
        }
      });
      list.append(item);
    }
    updateSessionHighlight();
  }
  if (state.session === null && sessions.length > 0) {
    selectSession(sessions[0].id); // 无 ?session= 参数时自动选中首个会话
  }
}

/** 切换当前会话：重置游标与事件日志，立即拉取任务/状态/事件，并同步 URL。 */
function selectSession(sessionId) {
  if (state.session === sessionId) {
    updateSessionHighlight();
    return;
  }
  state.session = sessionId;
  state.cursor = 0;
  el("event-log").textContent = "";
  updateCursorLabel();
  const url = new URL(window.location.href);
  url.searchParams.set("session", sessionId);
  window.history.replaceState(null, "", url);
  updateSessionHighlight();
  loadTasks().catch(() => {});
  loadStatus().catch(() => {});
  pollEvents().catch(() => {});
}

/** 选中态（aria-current；样式不依赖颜色）。 */
function updateSessionHighlight() {
  for (const item of document.querySelectorAll("#session-list li")) {
    item.setAttribute(
      "aria-current",
      item.dataset.session === state.session ? "true" : "false",
    );
  }
}

// 任务视图（FR-103，批次 1）：会话范围的任务总览 + 活跃任务（有界）。
async function loadTasks() {
  const session = state.session;
  if (!session) {
    return;
  }
  const data = await fetchJson(`/api/tasks?session=${encodeURIComponent(session)}`);
  if (session !== state.session) {
    return; // 会话已切换：丢弃过期响应
  }
  const list = el("task-list");
  list.textContent = "";
  const summary = document.createElement("li");
  summary.textContent = t(
    "web.tasks_summary",
    "总计 {total} · 完成 {done} · 阻塞 {blocked}",
    { total: data.total, done: data.done, blocked: data.blocked },
  );
  list.append(summary);
  for (const task of data.active ?? []) {
    const item = document.createElement("li");
    item.textContent = `${task.id} ${task.title} [${task.status}] retry=${task.retry_count}`;
    list.append(item);
  }
}

// 事件增量（T094/FR-605）：从游标补齐，恢复后不重复不遗漏。
// 并发守卫：会话切换时 selectSession 与轮询循环可能同时从游标 0 拉取——
// 在途请求不重入；响应到达时会话已切换则丢弃；seq <= cursor 一律跳过（去重）。
let eventsInFlight = false;

async function pollEvents() {
  const session = state.session;
  if (!session || eventsInFlight) {
    return;
  }
  eventsInFlight = true;
  try {
    const data = await fetchJson(
      `/api/events?session=${encodeURIComponent(session)}&after=${state.cursor}`,
    );
    if (session !== state.session) {
      return; // 会话已切换：丢弃过期响应
    }
    const log = el("event-log");
    for (const event of data.events ?? []) {
      if (event.seq <= state.cursor) {
        continue; // 游标去重
      }
      log.textContent += `${event.seq} ${event.kind}\n`;
      state.cursor = event.seq;
    }
    updateCursorLabel();
  } finally {
    eventsInFlight = false;
  }
}

// 状态快照（FR-314）。
async function loadStatus() {
  const session = state.session;
  if (!session) {
    return;
  }
  const data = await fetchJson(`/api/status?session=${encodeURIComponent(session)}`);
  if (session !== state.session) {
    return; // 会话已切换：丢弃过期响应
  }
  el("spec-status").textContent = JSON.stringify(data, null, 2);
}

// 轮询循环：失败退避重连（T094/FR-605），恢复后重置退避。
// FR-617：挂起期间**暂停主动轮询**——取消定时器、游标保留，恢复后从游标续拉
// （不重复、不遗漏）。实测缺陷修复（2026-10-22）：此前挂起期间定时器照常触发。
let pollTimer = null;
let suspendMachine = null;
let pollTicks = 0;

function schedulePoll(delayMs) {
  if (pollTimer !== null) {
    window.clearTimeout(pollTimer);
    pollTimer = null;
  }
  if (suspendMachine !== null && suspendMachine.state !== State.Active) {
    return; // 挂起/恢复中不排轮询；恢复由 onChange(Active) 重启（FR-617）。
  }
  pollTimer = window.setTimeout(pollLoop, delayMs);
}

async function pollLoop() {
  pollTimer = null;
  if (suspendMachine !== null && suspendMachine.state !== State.Active) {
    return; // 挂起竞态：即使定时器已触发也不再发起请求。
  }
  try {
    await loadSessions();
    await pollEvents();
    await loadStatus();
    pollTicks += 1;
    if (pollTicks % 15 === 0) {
      await loadTasks(); // 任务变化低频：每 15 个轮询（≈15s）刷新一次
    }
    if (!state.connected) {
      state.backoffMs = 500;
    }
    setConnected(true);
  } catch {
    setConnected(false);
    state.backoffMs = Math.min(state.backoffMs * 2, state.backoffMaxMs);
  }
  schedulePoll(state.connected ? 1000 : state.backoffMs);
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
    // 恢复（去抖到期、单次）：从游标续拉；后续轮询由 onChange(Active) 重启。
    loadTasks().catch(() => {});
    loadStatus();
    pollEvents();
  });
  suspendMachine = machine;
  machine.attach(() => state.cursor);
  machine.onChange((newState) => {
    if (newState === State.Active) {
      if (state.connected) {
        setConnected(true);
      }
      schedulePoll(250); // 恢复后尽快续拉（FR-617：单次全量刷新后继续正常节奏）。
    } else {
      // 挂起/恢复中：暂停主动轮询（FR-617）。
      if (pollTimer !== null) {
        window.clearTimeout(pollTimer);
        pollTimer = null;
      }
      if (newState === State.Recovering) {
        el("connection-state").textContent = t("web.recovering", "恢复中…");
      }
    }
  });
  window.addEventListener("unload", () => machine.detach());

  // 文本层失败不阻塞界面（内置默认文本可见）。
  loadLocale().catch(() => {});
  // 带 ?session= 直接进入时首屏即拉取任务（否则要等第 15 个轮询周期）。
  if (state.session) {
    loadTasks().catch(() => {});
  }
  pollLoop();
});
