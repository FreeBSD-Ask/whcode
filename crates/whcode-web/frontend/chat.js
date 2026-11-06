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
const session = new URLSearchParams(location.search).get("session") ?? "";

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
  }
});

if (session) {
  connectStream();
  void loadTail();
} else {
  status.textContent = "缺 session 参数（示例：/chat?session=S-1）";
}
