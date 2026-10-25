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
  // 语言选择（FR-625）：显式 `?locale=` 优先；否则交给服务端链解析
  // （Accept-Language → ui.locale → WHCODE_UI_LOCALE → LANG/LC_ALL → zh-CN）——
  // 不在客户端抢用 navigator.language，避免绕过服务端默认链。
  const explicit = new URL(window.location.href).searchParams.get("locale");
  const query = explicit ? `?locale=${encodeURIComponent(explicit)}` : "";
  const data = await fetchJson(`/api/i18n${query}`);
  texts = data.entries || {};
  document.documentElement.lang = data.locale || explicit || "zh-CN";
  document.documentElement.dir = data.rtl ? "rtl" : "ltr";
  for (const node of document.querySelectorAll("[data-i18n]")) {
    const key = node.dataset.i18n;
    if (typeof texts[key] === "string") {
      node.textContent = texts[key];
    }
  }
  for (const node of document.querySelectorAll("[data-i18n-placeholder]")) {
    const key = node.dataset.i18nPlaceholder;
    if (typeof texts[key] === "string") {
      node.placeholder = texts[key];
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

// 认证 token（ADR-023）：仅从 URL fragment（#token=…）读取——fragment 由浏览器本地
// 保存、**从不发送给服务器**（§10.2 禁止 query token）；内存保存，不写 localStorage。
const authToken = (() => {
  const hash = new URLSearchParams(window.location.hash.replace(/^#/, ""));
  const token = hash.get("token");
  return token && token.trim() ? token.trim() : null;
})();

async function fetchJson(url) {
  const headers = {};
  if (authToken !== null) {
    headers["Authorization"] = `Bearer ${authToken}`;
  }
  const response = await fetch(url, { headers });
  if (!response.ok) {
    throw new Error(`HTTP ${response.status}`);
  }
  return response.json();
}

// 写命令（FR-609）：POST + JSON 体；错误信封与 CUI 同源（code/message/retryable/details）。
// 破坏性命令**默认只预览**，确认按钮在预览成功后才启用（与 CUI --dry-run/--yes 同语义）。
async function postJson(url, payload) {
  const headers = { "Content-Type": "application/json" };
  if (authToken !== null) {
    headers["Authorization"] = `Bearer ${authToken}`;
  }
  const response = await fetch(url, {
    method: "POST",
    headers,
    body: JSON.stringify(payload),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    const reason = data.message ? `${data.code}: ${data.message}` : `HTTP ${response.status}`;
    throw new Error(reason);
  }
  return data;
}

function setActionStatus(text, isError = false) {
  const node = el("action-status");
  node.textContent = text;
  node.dataset.state = isError ? "error" : "ok";
}

/** 包装写命令：统一错误呈现（可操作、不责怪用户），成功后刷新视图。 */
async function runAction(label, action) {
  try {
    const result = await action();
    setActionStatus(`${label}：${result ?? "ok"}`);
    return true;
  } catch (error) {
    setActionStatus(t("web.action.failed", "操作失败", { error: error.message, label }), true);
    return false;
  }
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
    // 写命令（FR-609）：重试/中止与 CUI `task retry|abort` 同一应用服务。
    for (const [labelKey, fallback, handler] of [
      ["web.task.retry", "重试", retryTask],
      ["web.task.abort", "中止", abortTask],
    ]) {
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = t(labelKey, fallback);
      button.dataset.task = task.id;
      button.addEventListener("click", () => {
        runAction(t(labelKey, fallback), () => handler(task.id));
      });
      item.append(" ", button);
    }
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

// ---- 写命令（FR-609）：会话、任务、清理与运行时 ----
// 每条命令与服务端 `POST /api/*` 一一对应，语义与 CUI 相同（FR-604）：
// 破坏性操作先预览，确认按钮在预览成功后才启用；删除当前会话后清空选择。

let archiveArmed = null; // 已预览的归档目标（防止对未预览的会话直接归档）
let deleteArmed = null; // 已预览的删除目标（防止对未预览的会话直接删除）
let purgeArmed = null; // 已预览的清理判据（毫秒时间点）

async function createSession(title) {
  const data = await postJson("/api/sessions/new", { title });
  lastSessionsSignature = ""; // 强制重建列表
  await loadSessions();
  selectSession(data.session.id);
  return data.session.id;
}

async function renameCurrentSession(title) {
  if (!state.session) {
    throw new Error(t("web.session.none", "未选择会话"));
  }
  const data = await postJson("/api/sessions/rename", {
    session: state.session,
    title,
  });
  lastSessionsSignature = "";
  await loadSessions();
  return data.session.title;
}

async function previewArchiveCurrentSession() {
  if (!state.session) {
    throw new Error(t("web.session.none", "未选择会话"));
  }
  const target = state.session;
  await postJson("/api/sessions/archive", {
    session: target,
    archived: true,
  });
  archiveArmed = target;
  el("session-archive-confirm").disabled = false;
  return t("web.session.archive_preview_result", "将归档会话 {session}——确认后执行", {
    session: target,
  });
}

async function confirmArchiveCurrentSession() {
  if (archiveArmed === null) {
    throw new Error(t("web.session.archive_preview_required", "须先预览再确认归档"));
  }
  const target = archiveArmed;
  await postJson("/api/sessions/archive", {
    session: target,
    archived: true,
    confirm: true,
  });
  archiveArmed = null;
  el("session-archive-confirm").disabled = true;
  lastSessionsSignature = "";
  await loadSessions();
  return t("web.session.archived", "已归档会话 {session}", { session: target });
}

async function previewDeleteCurrentSession() {
  if (!state.session) {
    throw new Error(t("web.session.none", "未选择会话"));
  }
  const data = await postJson("/api/sessions/delete", { session: state.session });
  deleteArmed = state.session;
  el("session-delete-confirm").disabled = false;
  return t("web.session.delete_preview_result", "将删除：事件 {events} · 任务 {tasks}", {
    events: data.counts?.events ?? 0,
    tasks: data.counts?.tasks ?? 0,
  });
}

async function confirmDeleteCurrentSession() {
  if (deleteArmed === null) {
    throw new Error(t("web.session.preview_required", "须先预览再确认删除"));
  }
  const target = deleteArmed;
  const data = await postJson("/api/sessions/delete", { session: target, confirm: true });
  deleteArmed = null;
  el("session-delete-confirm").disabled = true;
  if (state.session === target) {
    state.session = null;
    state.cursor = 0;
    el("event-log").textContent = "";
    el("task-list").textContent = "";
    el("spec-status").textContent = "";
    const url = new URL(window.location.href);
    url.searchParams.delete("session");
    window.history.replaceState(null, "", url);
  }
  lastSessionsSignature = "";
  await loadSessions();
  return t("web.session.deleted", "已删除 {n} 个会话", {
    n: data.deleted_sessions?.length ?? 0,
  });
}

async function retryTask(taskId) {
  const data = await postJson("/api/tasks/retry", { task: taskId });
  await loadTasks();
  return `${data.task.id} → ${data.task.status}`;
}

async function abortTask(taskId) {
  const data = await postJson("/api/tasks/abort", { task: taskId });
  await loadTasks();
  return `${data.task.id} → ${data.task.status}`;
}

function purgeCutoffMs() {
  const days = Number.parseInt(el("purge-days").value, 10);
  if (!Number.isFinite(days) || days < 1) {
    throw new Error(t("web.cleanup.bad_days", "保留天数必须 ≥ 1"));
  }
  return Date.now() - days * 86_400_000;
}

async function previewPurge() {
  const cutoff = purgeCutoffMs();
  const data = await postJson("/api/purge", { before_ms: cutoff });
  purgeArmed = cutoff;
  el("purge-confirm").disabled = false;
  el("purge-result").textContent = JSON.stringify(data, null, 2);
  return t("web.cleanup.preview_result", "将清理 {sessions} 个会话", {
    sessions: data.selected_sessions?.length ?? 0,
  });
}

async function confirmPurge() {
  if (purgeArmed === null) {
    throw new Error(t("web.cleanup.preview_required", "须先预览再确认清理"));
  }
  const data = await postJson("/api/purge", {
    before_ms: purgeArmed,
    confirm: true,
  });
  purgeArmed = null;
  el("purge-confirm").disabled = true;
  el("purge-result").textContent = JSON.stringify(data, null, 2);
  lastSessionsSignature = "";
  await loadSessions();
  return t("web.cleanup.done", "已清理 {sessions} 个会话", {
    sessions: data.deleted_sessions?.length ?? 0,
  });
}

/** 运行时清单（FR-702/609）：只读声明，`available = null` 表示本路径未探测。 */
async function loadRuntimes() {
  const data = await fetchJson("/api/runtimes");
  const list = el("runtime-list");
  list.textContent = "";
  if (!data.wired) {
    const item = document.createElement("li");
    item.textContent = t("web.runtimes.unwired", "运行时清单未接线");
    list.append(item);
    return;
  }
  for (const runtime of data.runtimes ?? []) {
    const item = document.createElement("li");
    const stateText =
      runtime.available === null
        ? t("web.runtimes.unknown", "未探测")
        : runtime.available
          ? t("web.runtimes.available", "可用")
          : t("web.runtimes.unavailable", "不可用");
    item.textContent = `${runtime.name} [${runtime.capability}] ${stateText}`;
    list.append(item);
  }
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

// 上下文读取面（FR-622）：/api/context 与 TUI/GUI 同源（预算/来源/截断原因）。
async function loadContext() {
  const list = el("context-list");
  const summary = el("context-summary");
  if (!state.session) {
    summary.textContent = t("web.context.no_session", "未选择会话");
    list.textContent = "";
    return;
  }
  const body = await fetchJson(`/api/context?session=${encodeURIComponent(state.session)}`);
  summary.textContent = t("web.context.summary", "预算 {consumed}/{budget} · 来源 {sources} · 截断 {truncated}", {
    consumed: body.consumed_budget ?? 0,
    budget: body.budget_tokens ?? 0,
    sources: (body.sources ?? []).join("、") || "-",
    truncated: body.truncated_reason ?? "-",
  });
  const items = body.items ?? [];
  if (items.length === 0) {
    list.textContent = "";
    const empty = document.createElement("li");
    empty.textContent = t("web.context.empty", "（正确的空：预算内无可装入条目）");
    list.append(empty);
    return;
  }
  list.textContent = "";
  for (const item of items) {
    const row = document.createElement("li");
    row.textContent = `${item.role} · ${item.content_ref} · ${item.token_count}t`;
    list.append(row);
  }
}

// diff 读取面（FR-622）：/api/diff 与 TUI/GUI 同一 converge 路径（有界 ≤200 行）。
async function loadDiff() {
  const list = el("diff-list");
  const summary = el("diff-summary");
  if (!state.session) {
    summary.textContent = t("web.diff.no_session", "未选择会话");
    list.textContent = "";
    return;
  }
  const body = await fetchJson(`/api/diff?session=${encodeURIComponent(state.session)}`);
  summary.textContent = t("web.diff.summary", "任务 {total} · 通过 {pass} / 失败 {fail} / 未测 {untested}{truncated}", {
    total: body.task_count ?? 0,
    pass: body.pass_count ?? 0,
    fail: body.fail_count ?? 0,
    untested: body.not_tested_count ?? 0,
    truncated: body.truncated ? t("web.diff.truncated", "（超限截断）") : "",
  });
  const differences = body.differences ?? [];
  list.textContent = "";
  if (differences.length === 0) {
    const empty = document.createElement("li");
    empty.textContent = t("web.diff.empty", "（无差异：全部验收条件通过）");
    list.append(empty);
    return;
  }
  for (const line of differences) {
    const row = document.createElement("li");
    row.textContent = `${line.marker}${line.text}`;
    list.append(row);
  }
}

// 事件导出（FR-205）：/api/events/export 复用与 CUI 同一脱敏口径，下载为文件。
async function exportEvents(format) {
  if (!state.session) {
    setActionStatus(t("web.context.no_session", "未选择会话"), true);
    return;
  }
  const url = `/api/events/export?session=${encodeURIComponent(state.session)}&format=${format}`;
  const headers = {};
  if (authToken !== null) {
    headers["Authorization"] = `Bearer ${authToken}`;
  }
  const response = await fetch(url, { headers });
  if (!response.ok) {
    throw new Error(`HTTP ${response.status}`);
  }
  const text = await response.text();
  const blob = new Blob([text], {
    type: format === "jsonl" ? "application/x-ndjson" : "application/json",
  });
  const link = document.createElement("a");
  link.href = URL.createObjectURL(blob);
  link.download = `events-${state.session}.${format}`;
  link.click();
  URL.revokeObjectURL(link.href);
  setActionStatus(t("web.export.done", "导出完成"), false);
}

// 标签切换（T197/T198）：ARIA tablist 标准模式——roving tabindex + 方向键/Home/End
// 循环切换；面板用 role="tabpanel" 与 aria-labelledby 关联。不新增定时器。
function selectTab(name, options = {}) {
  const tabs = Array.from(document.querySelectorAll("#tabs button"));
  const target = tabs.find((tab) => tab.dataset.tab === name);
  if (!target) {
    return;
  }
  for (const tab of tabs) {
    const selected = tab === target;
    tab.setAttribute("aria-selected", selected ? "true" : "false");
    tab.tabIndex = selected ? 0 : -1;
  }
  for (const section of document.querySelectorAll("main section")) {
    section.hidden = section.id !== `view-${name}`;
  }
  if (options.focus) {
    target.focus();
  }
  // 读取面按需加载（FR-622）：切到上下文/diff 时拉取当前会话数据。
  if (name === "context") {
    loadContext().catch(() => {});
  } else if (name === "diff") {
    loadDiff().catch(() => {});
  }
  if (animationsEnabled) {
    // 仅 opacity（GPU 合成友好，T133）；reduced-motion 时无动效。
    el("main").style.opacity = "0.6";
    window.setTimeout(() => {
      el("main").style.opacity = "";
    }, 150);
  }
}

/** 接线 tablist：点击选中；方向键循环（RTL 下方向反转）；Home/End 首尾。 */
function wireTabs() {
  const tabs = Array.from(document.querySelectorAll("#tabs button"));
  const rtl = document.documentElement.dir === "rtl";
  for (const tab of tabs) {
    tab.addEventListener("click", () => selectTab(tab.dataset.tab));
    tab.addEventListener("keydown", (event) => {
      const index = tabs.indexOf(tab);
      const step = rtl ? -1 : 1;
      let next = null;
      if (event.key === "ArrowRight" || event.key === "ArrowDown") {
        next = tabs[(index + step + tabs.length) % tabs.length];
      } else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
        next = tabs[(index - step + tabs.length) % tabs.length];
      } else if (event.key === "Home") {
        next = tabs[0];
      } else if (event.key === "End") {
        next = tabs[tabs.length - 1];
      }
      if (next !== null) {
        event.preventDefault();
        selectTab(next.dataset.tab, { focus: true });
      }
    });
  }
}

document.addEventListener("DOMContentLoaded", () => {
  const params = new URLSearchParams(window.location.search);
  state.session = params.get("session");
  // 标签导航（T198）：先接线，保证键盘可用性与首屏状态一致。
  wireTabs();

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

  // 主题运行时切换（T175/FR-625）：服务端注入 data-theme 为默认；`?theme=` 与
  // 选择器写 localStorage（可见、可改、可撤销——选“默认”即移除恢复服务端默认）。
  const THEMES = ["eink-day", "eink-night", "amoled-night", "terminal-green"];
  const urlTheme = params.get("theme");
  if (urlTheme && THEMES.includes(urlTheme)) {
    localStorage.setItem("whcode-theme", urlTheme);
  }
  const applyTheme = (theme) => {
    if (THEMES.includes(theme)) {
      document.documentElement.dataset.theme = theme;
    }
  };
  const storedTheme = localStorage.getItem("whcode-theme");
  if (storedTheme) {
    applyTheme(storedTheme);
  }
  const themeSelect = el("theme-select");
  if (storedTheme) {
    themeSelect.value = storedTheme;
  }
  themeSelect.addEventListener("change", () => {
    const value = themeSelect.value;
    if (!value) {
      localStorage.removeItem("whcode-theme");
      document.documentElement.dataset.theme = "";
      delete document.documentElement.dataset.theme;
      return;
    }
    localStorage.setItem("whcode-theme", value);
    applyTheme(value);
  });
  // 文本层失败不阻塞界面（内置默认文本可见）。
  loadLocale().catch(() => {});
  // 带 ?session= 直接进入时首屏即拉取任务（否则要等第 15 个轮询周期）。
  if (state.session) {
    loadTasks().catch(() => {});
  }

  // 写命令接线（FR-609）：所有按钮走同一 POST 路径；破坏性命令须先预览。
  el("session-create").addEventListener("submit", (event) => {
    event.preventDefault();
    const title = el("session-title").value.trim();
    if (!title) {
      setActionStatus(t("web.session.title_required", "请输入会话标题"), true);
      return;
    }
    runAction(t("web.session.create", "新建"), () => createSession(title)).then((ok) => {
      if (ok) el("session-title").value = "";
    });
  });
  el("session-rename").addEventListener("click", () => {
    const title = el("session-rename-title").value.trim();
    if (!title) {
      setActionStatus(t("web.session.title_required", "请输入会话标题"), true);
      return;
    }
    runAction(t("web.session.rename", "重命名"), () => renameCurrentSession(title));
  });
  el("session-archive").addEventListener("click", () => {
    runAction(t("web.session.archive_preview", "归档预览"), previewArchiveCurrentSession);
  });
  el("session-archive-confirm").addEventListener("click", () => {
    runAction(t("web.session.archive_confirm", "确认归档"), confirmArchiveCurrentSession);
  });
  el("session-delete-preview").addEventListener("click", () => {
    runAction(t("web.session.delete_preview", "删除预览"), previewDeleteCurrentSession);
  });
  el("session-delete-confirm").addEventListener("click", () => {
    runAction(t("web.session.delete_confirm", "确认删除"), confirmDeleteCurrentSession);
  });
  el("purge-preview").addEventListener("click", () => {
    runAction(t("web.cleanup.preview", "预览"), previewPurge);
  });
  el("purge-confirm").addEventListener("click", () => {
    runAction(t("web.cleanup.confirm", "确认清理"), confirmPurge);
  });
  el("export-json").addEventListener("click", () => {
    runAction(t("web.export.json", "导出 JSON"), () => exportEvents("json"));
  });
  el("export-jsonl").addEventListener("click", () => {
    runAction(t("web.export.jsonl", "导出 JSONL"), () => exportEvents("jsonl"));
  });
  loadRuntimes().catch(() => {});
  pollLoop();
});
