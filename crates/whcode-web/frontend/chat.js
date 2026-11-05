// 对话页（T251/FR-742）：原生 ESM，无构建链、无框架。
// 发送经 POST /api/llm/run（同步语义，与 CUI/TUI 一致——同等待到完成）；
// 完成后重拉 GET /api/chat/tail 重渲染。无 JS 时服务端直出历史（chat.html）。
// 注意：WS 逐块增量属 T251 剩余项（serve_loop 单线程多路复用，需并发设计档案）。
const main = document.getElementById("chat-main");
const form = document.getElementById("chat-form");
const status = document.getElementById("chat-status");
const session = new URLSearchParams(location.search).get("session") ?? "";

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

form.hidden = false;
form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const provider = document.getElementById("chat-provider").value.trim();
  const prompt = document.getElementById("chat-prompt").value.trim();
  if (!provider || !prompt || !session) return;
  status.textContent = "生成中…";
  try {
    const response = await fetch("/api/llm/run", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ session, provider, prompt }),
    });
    const payload = await response.json();
    if (!response.ok) {
      status.textContent = `失败：${payload.error?.message ?? response.status}`;
      return;
    }
    status.textContent = "完成";
    document.getElementById("chat-prompt").value = "";
    await loadTail();
  } catch (error) {
    status.textContent = `失败：${error}`;
  }
});

if (session) {
  void loadTail();
} else {
  status.textContent = "缺 session 参数（示例：/chat?session=S-1）";
}
