// 对话页（T251/FR-742）：原生 ESM，无构建链、无框架。
//
// 两条通道，职责分开：
// ① 历史：服务端直出（chat.html）＋ `GET /api/chat/tail` 重拉；
// ② 增量：turn 期间经 `/ws` 接收 `llm.chunk`，按 `session` 归属后追加到实时尾。
// 发送仍走 `POST /api/llm/run`（同步等待完成，与 CUI/TUI 同语义）——WS 只承担
// 呈现，不改变完成信封；无 JavaScript 时历史仍完整可读（chat.html 的 noscript）。
//
// 认证约束（ADR-023／ADR-058 §3.3）：浏览器无法给 WebSocket 握手附加
// `Authorization` 头，且 token 禁止经 URL 传递。认证部署下 `/ws` 会被拒绝，
// 此时静默回落为“发送后重拉历史”，不伪造实时，也不把失败当成功。
const main = document.getElementById("chat-main");
const form = document.getElementById("chat-form");
const status = document.getElementById("chat-status");
const tools = document.getElementById("tool-cards");
const approvals = document.getElementById("approval-list");
const approvalStatus = document.getElementById("approval-status");
const session = new URLSearchParams(location.search).get("session") ?? "";

// 文本层（FR-615）：与首页共用 `/api/i18n`；缺失键回退中文静态文案（不显示键名）。
const texts = {};
const t = (key, fallback) => texts[key] ?? fallback;

const loadTexts = async () => {
  try {
    const response = await fetch("/api/i18n");
    if (!response.ok) return;
    const payload = await response.json();
    Object.assign(texts, payload.entries ?? {});
    for (const node of document.querySelectorAll("[data-i18n]")) {
      const value = texts[node.dataset.i18n];
      if (value) node.textContent = value;
    }
  } catch {
    // 文本层失败不阻断对话：静态中文兜底。
  }
};

// 实时尾：JS 动态创建，避免无 JS 时出现空节点（DOM 预算口径）。
const live = document.createElement("div");
live.id = "chat-live";
live.hidden = true;
live.setAttribute("aria-live", "polite");
main.appendChild(live);

const LIVE_LIMIT = 20_000;

const resetLive = () => {
  live.textContent = "";
  live.hidden = true;
};

const appendChunk = (text) => {
  if (!text) return;
  live.hidden = false;
  // 有界：只保留尾部窗口，长回答不无限增长文本节点。
  live.textContent = (live.textContent + text).slice(-LIVE_LIMIT);
};

const renderTail = (items) => {
  main.querySelectorAll("[data-role=entry]").forEach((node) => node.remove());
  const fragment = document.createDocumentFragment();
  for (const item of items) {
    const line = document.createElement("p");
    line.dataset.role = "entry";
    const tag = document.createElement("strong");
    tag.textContent = `[${item.role}] `;
    line.appendChild(tag);
    line.appendChild(document.createTextNode(item.content));
    fragment.appendChild(line);
  }
  main.insertBefore(fragment, main.firstChild);
};

const loadTail = async () => {
  const response = await fetch(
    `/api/chat/tail?session=${encodeURIComponent(session)}`,
  );
  if (!response.ok) return;
  const payload = await response.json();
  renderTail(payload.items ?? []);
};

// ── 工具调用卡（FR-743／T257）────────────────────────────────────────────
// 事件源是 `ToolCallStarted`／`ToolCallFinished`（顶层 `kind`）；参数不进入事件，
// 卡片只显示工具名、执行 id、状态与有界输出。卡片数有上限，旧卡先移除。
const MAX_TOOL_CARDS = 20;
const cards = new Map();

const ensureToolCard = (toolCallId, toolName, execId) => {
  if (!toolCallId) return null;
  let card = cards.get(toolCallId);
  if (card) return card;
  if (cards.size >= MAX_TOOL_CARDS) {
    const oldest = cards.keys().next().value;
    cards.get(oldest)?.remove();
    cards.delete(oldest);
  }
  const article = document.createElement("article");
  article.className = "tool-card";
  const header = document.createElement("header");
  const name = document.createElement("strong");
  name.textContent = toolName ?? "?";
  const state = document.createElement("span");
  state.className = "tool-card__state";
  state.textContent = t("web.tool.running", "运行中");
  header.append(name, state);
  const meta = document.createElement("p");
  meta.className = "meta-line";
  meta.textContent = `${t("web.tool.exec", "执行")}: ${execId ?? "-"}`;
  const output = document.createElement("pre");
  output.className = "tool-card__output";
  output.hidden = true;
  article.append(header, meta, output);
  tools.appendChild(article);
  tools.parentElement.hidden = false;
  card = { article, state, output };
  cards.set(toolCallId, card);
  return card;
};

const settleToolCard = (frame) => {
  const payload = frame.payload ?? {};
  const id = frame.tool_call_id ?? payload.tool_call_id;
  const card = cards.get(id) ?? ensureToolCard(id, payload.tool_name, payload.exec_id);
  if (!card) return;
  const value = payload.output_value;
  card.state.textContent = payload.state ?? payload.reason ?? "?";
  if (payload.error_code) {
    card.state.textContent += ` · ${payload.error_code}`;
  }
  if (typeof value === "string" && value.length > 0) {
    card.output.hidden = false;
    card.output.textContent = value;
  }
};

// ── 四态审批（FR-745／T257）─────────────────────────────────────────────
// 轮询只在 turn 运行期间启用（递归 setTimeout，不用 setInterval）；请求表不触库，
// turn 期间也能取到。答案映射：批准＝本回合、仅本次＝approve、记住规则＝
// approve_remember、拒绝＝reject_remember（写 deny 规则，可在设置页撤销）。
const APPROVAL_POLL_MS = 1_000;
let running = false;
let pollTimer = null;
let lastApprovals = "";

const submitDecision = async (requestId, answer, buttons) => {
  buttons.forEach((button) => {
    button.disabled = true;
  });
  try {
    const response = await fetch("/api/approvals/decide", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ request_id: requestId, answer }),
    });
    if (!response.ok) {
      const payload = await response.json().catch(() => ({}));
      approvalStatus.textContent =
        t("web.approval.failed", "审批失败") + `: ${payload.message ?? response.status}`;
      buttons.forEach((button) => {
        button.disabled = false;
      });
      return;
    }
    approvalStatus.textContent = t("web.approval.decided", "已处理");
    await refreshApprovals();
  } catch (error) {
    approvalStatus.textContent = `${t("web.approval.failed", "审批失败")}: ${error}`;
    buttons.forEach((button) => {
      button.disabled = false;
    });
  }
};

const renderApprovals = (requests) => {
  const signature = JSON.stringify(requests.map((request) => request.id));
  if (signature === lastApprovals) return;
  lastApprovals = signature;
  approvals.replaceChildren();
  approvals.parentElement.hidden = requests.length === 0;
  for (const request of requests) {
    const article = document.createElement("article");
    article.className = "tool-card approval-card";
    const header = document.createElement("header");
    const name = document.createElement("strong");
    name.textContent = `${t("web.approval.tool", "工具")}: ${request.tool}`;
    const rule = document.createElement("span");
    rule.className = "tool-card__state";
    rule.textContent = request.rule ?? "ask";
    header.append(name, rule);
    const meta = document.createElement("p");
    meta.className = "meta-line";
    meta.textContent = `${t("web.approval.exec", "执行")}: ${request.exec_id}`;
    const reason = document.createElement("p");
    reason.className = "meta-line";
    reason.textContent =
      `${t("web.approval.reason", "原因")}: ${request.display_reason ?? "-"}`;
    const actions = document.createElement("div");
    actions.className = "approval-card__actions";
    const buttons = [];
    for (const [answer, key, fallback] of [
      ["approve_turn", "web.approval.approve_turn", "本回合批准"],
      ["approve", "web.approval.approve_once", "仅本次"],
      ["approve_remember", "web.approval.remember", "记住规则"],
      ["reject_remember", "web.approval.reject", "拒绝"],
    ]) {
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = t(key, fallback);
      button.addEventListener("click", () => {
        void submitDecision(request.id, answer, buttons);
      });
      buttons.push(button);
      actions.appendChild(button);
    }
    article.append(header, meta, reason, actions);
    approvals.appendChild(article);
  }
};

const refreshApprovals = async () => {
  try {
    const response = await fetch("/api/approvals/pending");
    if (!response.ok) return;
    const payload = await response.json();
    renderApprovals(payload.requests ?? []);
  } catch {
    // 网络抖动下保持现有卡片，不清空（不清空即不假装“没有待审批”）。
  }
};

const scheduleApprovalPoll = () => {
  if (!running) return;
  pollTimer = setTimeout(async () => {
    await refreshApprovals();
    scheduleApprovalPoll();
  }, APPROVAL_POLL_MS);
};

const startApprovalWatch = () => {
  running = true;
  if (pollTimer === null) scheduleApprovalPoll();
};

const stopApprovalWatch = () => {
  running = false;
  if (pollTimer !== null) {
    clearTimeout(pollTimer);
    pollTimer = null;
  }
  void refreshApprovals();
};

const connectStream = () => {
  if (!session) return;
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  let socket;
  try {
    socket = new WebSocket(`${scheme}://${location.host}/ws`);
  } catch {
    return; // 浏览器禁用 WS：保持同步发送路径。
  }
  socket.addEventListener("message", (event) => {
    if (typeof event.data !== "string") return;
    let frame;
    try {
      frame = JSON.parse(event.data);
    } catch {
      return;
    }
    // 跨会话帧不渲染：广播面覆盖全部订阅者（ADR-058 §3.3 的已知放宽）。
    // 事件帧（EventRecord）：`kind` 区分工具开始/结束；`session` 字段名是 `session_id`。
    if (frame.kind === "ToolCallStarted" || frame.kind === "ToolCallFinished") {
      if (frame.session_id !== session) return;
      if (frame.kind === "ToolCallStarted") {
        const payload = frame.payload ?? {};
        ensureToolCard(
          frame.tool_call_id ?? payload.tool_call_id,
          payload.tool_name,
          payload.exec_id,
        );
      } else {
        settleToolCard(frame);
      }
      void refreshApprovals();
      return;
    }
    if (frame.session !== session) return;
    if (frame.type === "llm.chunk") {
      appendChunk(frame.delta ?? "");
    } else if (frame.type === "llm.loss") {
      status.textContent = `增量丢帧 ${frame.dropped} 块（有损通道，最终以完成信封为准）`;
    }
  });
  // 拒绝（认证部署）或断开：不提示、不重试；同步发送路径照旧可用。
  socket.addEventListener("close", () => {
    socket = null;
  });
};

form.hidden = false;
form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const provider = document.getElementById("chat-provider").value.trim();
  const prompt = document.getElementById("chat-prompt").value.trim();
  if (!provider || !prompt || !session) return;
  status.textContent = "生成中…";
  resetLive();
  startApprovalWatch();
  try {
    const response = await fetch("/api/llm/run", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ session, provider, prompt }),
    });
    const payload = await response.json();
    if (!response.ok) {
      const detail = payload.message ?? payload.error?.message ?? response.status;
      status.textContent = `失败：${detail}`;
      return;
    }
    status.textContent = "完成";
    document.getElementById("chat-prompt").value = "";
    resetLive();
    await loadTail();
  } catch (error) {
    status.textContent = `失败：${error}`;
  } finally {
    stopApprovalWatch();
  }
});

if (session) {
  connectStream();
  void loadTexts();
  void loadTail();
  void refreshApprovals();
} else {
  status.textContent = "缺 session 参数（示例：/chat?session=S-1）";
}
