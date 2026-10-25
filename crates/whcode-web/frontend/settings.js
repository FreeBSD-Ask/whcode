// WHCode 证书设置页（T190／FR-627，原生 ESM，无构建链——ADR-014）。
//
// 职责边界：
// - 只做呈现与确认：认证（Bearer token）、Origin 与能力判定全部在服务端；
// - PEM 与私钥一律不落 localStorage、不写日志；私钥输入在提交成功后立即清空，
//   状态响应本身不含私钥（ADR-040 决策 4）；
// - 错误响应显示 `code` 与 `message`，且不清空表单（可操作、不责怪用户）。

const el = (id) => document.getElementById(id);

// 文本层（FR-615）：服务端 /api/i18n 下发 zh-CN／en 词条；缺键回落到内置默认文本。
let texts = {};

function t(key, fallback, args = {}) {
  let text = typeof texts[key] === "string" ? texts[key] : fallback;
  for (const [name, value] of Object.entries(args)) {
    text = text.split(`{${name}}`).join(String(value));
  }
  return text;
}

// 认证 token（ADR-023）：只从 URL fragment（#token=…）读取——fragment 由浏览器本地
// 保存、从不发送给服务器；仅存内存，不写 localStorage，也不写入 DOM 文本。
const authToken = (() => {
  const hash = new URLSearchParams(window.location.hash.replace(/^#/, ""));
  const token = hash.get("token");
  return token && token.trim() ? token.trim() : null;
})();

function authHeaders(extra = {}) {
  const headers = { ...extra };
  if (authToken !== null) {
    headers.Authorization = `Bearer ${authToken}`;
  }
  return headers;
}

/** 结构化 API 错误：`code` 与 `message` 来自服务端错误信封（FR-602）。 */
class ApiError extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

function apiError(data, status) {
  const code = data && typeof data.code === "string" ? data.code : `http_${status}`;
  const message =
    data && typeof data.message === "string" ? data.message : `HTTP ${status}`;
  return new ApiError(code, message);
}

async function fetchJson(url) {
  const response = await fetch(url, { headers: authHeaders() });
  const data = await response.json().catch(() => null);
  if (!response.ok) {
    throw apiError(data, response.status);
  }
  return data ?? {};
}

async function postJson(url, payload) {
  const response = await fetch(url, {
    method: "POST",
    headers: authHeaders({ "Content-Type": "application/json" }),
    body: JSON.stringify(payload),
  });
  const data = await response.json().catch(() => null);
  if (!response.ok) {
    throw apiError(data, response.status);
  }
  return data ?? {};
}

function describeError(error) {
  const code = typeof error?.code === "string" ? error.code : "network_error";
  const message =
    typeof error?.message === "string" && error.message
      ? error.message
      : t("settings.error.generic", "操作失败");
  return { code, message };
}

function setText(id, text) {
  const node = el(id);
  if (node) {
    node.textContent = text;
  }
}

function setStatus(text, isError = false) {
  const node = el("action-status");
  node.textContent = text;
  node.dataset.state = isError ? "error" : "ok";
}

/** 加载文本层：应用到 data-i18n／data-i18n-placeholder 与 <html lang/dir>。 */
async function loadLocale() {
  // 语言选择（FR-625）：显式 `?locale=` 优先，否则交给服务端链解析（含 config 默认）。
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
}

// 主题（FR-612/625）：与首页同一套机制——服务端注入 data-theme 为默认，
// `?theme=` 与选择器写 localStorage（可见、可改、可撤销）。
const THEMES = ["eink-day", "eink-night", "amoled-night", "terminal-green"];

function wireTheme() {
  const params = new URLSearchParams(window.location.search);
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
  const select = el("theme-select");
  if (storedTheme) {
    select.value = storedTheme;
  }
  select.addEventListener("change", () => {
    const value = select.value;
    if (!value) {
      localStorage.removeItem("whcode-theme");
      delete document.documentElement.dataset.theme;
      return;
    }
    localStorage.setItem("whcode-theme", value);
    applyTheme(value);
  });
}

/** 三态布尔渲染：`null`／缺失一律“未知”，不把未探测说成“否”。 */
function stateWord(value) {
  if (value === null || value === undefined) {
    return t("settings.status.unknown", "未知");
  }
  return value ? t("settings.status.yes", "是") : t("settings.status.no", "否");
}

function valueOrUnknown(value) {
  return value === null || value === undefined || value === "" ? t("settings.status.unknown", "未知") : String(value);
}

/** 单个证书文件状态行（路径、份数、SHA-256、已修改、内置版本较新）。 */
function certFileLine(file) {
  const parts = [
    t("settings.field.path", "路径：{value}", { value: file.path }),
    t("settings.field.count", "份数：{value}", { value: String(file.cert_count) }),
    t("settings.field.sha256", "SHA-256：{value}", { value: file.sha256 }),
    t("settings.field.modified", "已修改：{value}", { value: stateWord(file.modified) }),
  ];
  if (file.newer_bundled_available) {
    parts.push(
      t("settings.field.newer", "内置版本较新：{value}", {
        value: t("settings.status.yes", "是"),
      }),
    );
  }
  return parts.join(" · ");
}

/** 把 `TlsStatus`（`GET /api/certs` 与写操作的响应）渲染到页面。 */
function renderStatus(status) {
  el("restart-banner").hidden = !status.restart_required;

  const outbound = status.outbound ?? {};
  for (const radio of document.querySelectorAll('input[name="outbound"]')) {
    radio.checked = radio.value === outbound.mode;
  }
  setText(
    "outbound-builtin-status",
    outbound.builtin
      ? certFileLine(outbound.builtin)
      : t("settings.outbound.missing_builtin", "内置副本缺失：状态未报告该文件"),
  );
  setText(
    "outbound-extra-status",
    outbound.extra
      ? certFileLine(outbound.extra)
      : t("settings.outbound.empty_extra", "未配置追加根证书（正确的空：文件中没有证书）"),
  );
  setText(
    "outbound-system-status",
    t("settings.outbound.system_roots_line", "系统库根证书数：{value}", {
      value:
        outbound.system_root_count === null || outbound.system_root_count === undefined
          ? t("settings.outbound.system_na", "未涉及（当前档位不读取系统库）")
          : String(outbound.system_root_count),
    }),
  );
  setText(
    "outbound-effective-status",
    t("settings.outbound.effective_line", "生效根证书总数：{value}", {
      value: valueOrUnknown(outbound.effective_root_count),
    }),
  );

  const inbound = status.inbound ?? {};
  for (const radio of document.querySelectorAll('input[name="inbound"]')) {
    radio.checked = radio.value === inbound.mode;
  }
  setText(
    "inbound-subject",
    t("settings.inbound.subject", "主题：{value}", { value: valueOrUnknown(inbound.subject) }),
  );
  setText(
    "inbound-sans",
    t("settings.inbound.sans", "SAN：{value}", {
      value: Array.isArray(inbound.sans) && inbound.sans.length > 0
        ? inbound.sans.join("、")
        : t("settings.status.unknown", "未知"),
    }),
  );
  setText(
    "inbound-not-after",
    t("settings.inbound.not_after", "到期：{value}", {
      value: valueOrUnknown(inbound.not_after),
    }),
  );
  setText(
    "inbound-days",
    t("settings.inbound.days_remaining", "剩余天数：{value}", {
      value: valueOrUnknown(inbound.days_remaining),
    }),
  );
  setText(
    "inbound-sha",
    t("settings.inbound.sha256", "指纹 SHA-256：{value}", {
      value: valueOrUnknown(inbound.sha256),
    }),
  );
  setText(
    "inbound-material",
    t("settings.inbound.material", "证书存在：{cert} · 私钥存在：{key}", {
      cert: stateWord(inbound.cert_present),
      key: stateWord(inbound.key_present),
    }),
  );
  const noMaterial = el("inbound-no-material");
  noMaterial.hidden = Boolean(inbound.cert_present);
  noMaterial.textContent = inbound.cert_present
    ? ""
    : t(
        "settings.inbound.no_material",
        "尚无服务端证书材料（可上传证书链或重新生成自签证书）",
      );

  const notes = el("inbound-notes");
  notes.textContent = "";
  if (!Array.isArray(status.notes) || status.notes.length === 0) {
    const row = document.createElement("li");
    row.textContent = t("settings.notes.none", "无附加提示");
    notes.append(row);
  } else {
    for (const note of status.notes) {
      const row = document.createElement("li");
      row.textContent = String(note);
      notes.append(row);
    }
  }
}

/** 载入单个根证书文本（内置副本或追加证书）。textarea 内容不写 localStorage。 */
async function loadCaText(kind) {
  const data = await fetchJson(`/api/certs/ca?kind=${kind}`);
  el(`ca-${kind}-pem`).value = typeof data.pem === "string" ? data.pem : "";
}

async function loadCaTexts() {
  await Promise.all([loadCaText("builtin"), loadCaText("extra")]);
}

function selectedMode(name) {
  const checked = document.querySelector(`input[name="${name}"]:checked`);
  return checked ? checked.value : null;
}

async function saveModes() {
  const payload = {};
  const outbound = selectedMode("outbound");
  if (outbound !== null) {
    payload.outbound = outbound;
  }
  const inbound = selectedMode("inbound");
  if (inbound !== null) {
    payload.inbound = inbound;
  }
  if (inbound === "off") {
    // 降档确认（ADR-040 决策 4）：取消则不发请求。
    if (!window.confirm(t("settings.confirm.inbound_off", "将关闭入站 TLS（回到明文 HTTP），重启 serve --web 后生效。继续？"))) {
      return;
    }
    payload.confirm = true;
  }
  renderStatus(await postJson("/api/certs/modes", payload));
}

async function saveCa(kind) {
  const pem = el(`ca-${kind}-pem`).value;
  const payload = { kind, pem };
  if (kind === "builtin" && pem.trim() === "") {
    // 写空内置副本属降档：必须显式确认，服务端同样要求 confirm=true。
    if (!window.confirm(t("settings.confirm.builtin_empty", "将清空内置根证书副本（出站信任不再含内置基线）。继续？"))) {
      return;
    }
    payload.confirm = true;
  }
  renderStatus(await postJson("/api/certs/ca", payload));
  // 只回读刚保存的那一份，避免覆盖另一侧尚未保存的编辑。
  await loadCaText(kind);
}

async function resetCa(kind) {
  const key = kind === "builtin" ? "settings.confirm.reset_builtin" : "settings.confirm.reset_extra";
  const fallback =
    kind === "builtin"
      ? "将把内置根证书副本恢复为随包资产。继续？"
      : "将清空追加根证书。继续？";
  if (!window.confirm(t(key, fallback))) {
    return;
  }
  renderStatus(await postJson("/api/certs/ca/reset", { kind }));
  await loadCaText(kind);
}

async function uploadServer() {
  const certPem = el("server-cert-pem").value.trim();
  const keyPem = el("server-key-pem").value.trim();
  if (!certPem || !keyPem) {
    throw new ApiError("missing_input", t("settings.error.missing_pem", "请先填写证书链与私钥（PEM）"));
  }
  // 上传外部私钥属降档动作（ADR-040 决策 4）：确认后带 confirm=true。
  if (!window.confirm(t("settings.confirm.external_key", "将保存外部服务端证书与私钥，并把入站档位切换为 external。继续？"))) {
    return;
  }
  renderStatus(
    await postJson("/api/certs/server", { cert_pem: certPem, key_pem: keyPem, confirm: true }),
  );
  // 私钥绝不回显：提交成功后立即清空输入框，并说明“已保存、不再显示”。
  el("server-key-pem").value = "";
  el("key-saved-note").hidden = false;
}

async function regenerateServer() {
  if (!window.confirm(t("settings.confirm.regenerate", "将重新生成自签服务端证书（现有指纹失效）。继续？"))) {
    return;
  }
  renderStatus(await postJson("/api/certs/server/self-signed", {}));
}

async function downloadServerCert() {
  const data = await fetchJson("/api/certs/server");
  if (typeof data.cert_pem !== "string" || !data.cert_pem) {
    throw new ApiError("no_cert", t("settings.inbound.no_material", "尚无服务端证书材料（可上传或重新生成自签）"));
  }
  const blob = new Blob([data.cert_pem], { type: "application/x-pem-file" });
  const link = document.createElement("a");
  link.href = URL.createObjectURL(blob);
  link.download = "server-cert.pem";
  link.click();
  URL.revokeObjectURL(link.href);
}

async function verifyHost() {
  const host = el("verify-host").value.trim();
  if (!host) {
    throw new ApiError("missing_host", t("settings.error.missing_host", "请先填写主机名或 IP 字面量"));
  }
  const payload = { host };
  const portText = el("verify-port").value.trim();
  if (portText !== "") {
    const port = Number.parseInt(portText, 10);
    if (!Number.isFinite(port) || port < 1 || port > 65535) {
      throw new ApiError("bad_port", t("settings.error.bad_port", "端口必须是 1..=65535 的整数"));
    }
    payload.port = port;
  }
  const data = await postJson("/api/certs/verify", payload);
  setText(
    "verify-result",
    t(
      "settings.verify.ok",
      "校验成功：主题 {subject} · 到期 {not_after} · 指纹 {sha256} · {tls_version}",
      {
        subject: valueOrUnknown(data.subject),
        not_after: valueOrUnknown(data.not_after),
        sha256: valueOrUnknown(data.sha256),
        tls_version: valueOrUnknown(data.tls_version),
      },
    ),
  );
}

/** 写操作包装：统一错误呈现（含 code 与 message），失败不清空表单。 */
async function runAction(labelKey, fallbackLabel, action) {
  const label = t(labelKey, fallbackLabel);
  setStatus(t("settings.action.running", "正在执行：{label}", { label }), false);
  try {
    await action();
    setStatus(t("settings.action.done", "完成：{label}", { label }), false);
  } catch (error) {
    const described = describeError(error);
    setStatus(
      t("settings.action.failed", "操作失败（{label}）：{code} {message}", {
        label,
        code: described.code,
        message: described.message,
      }),
      true,
    );
  }
}

/** 校验包装：失败只写结果区，同样显示 code 与 message（不伪造成功）。 */
async function runVerify() {
  try {
    await verifyHost();
  } catch (error) {
    const described = describeError(error);
    setText(
      "verify-result",
      t("settings.verify.failed", "校验失败：{error}", {
        error: `${described.code}: ${described.message}`,
      }),
    );
  }
}

async function refresh() {
  try {
    renderStatus(await fetchJson("/api/certs"));
    await loadCaTexts();
  } catch (error) {
    const described = describeError(error);
    setStatus(
      t("settings.error.load", "无法读取证书状态：{code} {message}", {
        code: described.code,
        message: described.message,
      }),
      true,
    );
  }
}

document.addEventListener("DOMContentLoaded", () => {
  wireTheme();
  // 先取文本层再取状态：错误与状态文案按服务端解析的语言呈现；
  // 文本层失败不阻塞界面（内置默认文本可见）。
  loadLocale()
    .catch(() => {})
    .then(refresh);

  el("modes-save").addEventListener("click", () => {
    runAction("settings.modes.save", "保存档位", saveModes);
  });
  el("ca-builtin-save").addEventListener("click", () => {
    runAction("settings.outbound.save_builtin", "保存内置副本", () => saveCa("builtin"));
  });
  el("ca-builtin-reset").addEventListener("click", () => {
    runAction("settings.outbound.reset_builtin", "恢复内置副本", () => resetCa("builtin"));
  });
  el("ca-extra-save").addEventListener("click", () => {
    runAction("settings.outbound.save_extra", "保存追加证书", () => saveCa("extra"));
  });
  el("ca-extra-reset").addEventListener("click", () => {
    runAction("settings.outbound.reset_extra", "清空追加证书", () => resetCa("extra"));
  });
  el("server-upload").addEventListener("click", () => {
    runAction("settings.inbound.upload", "上传并切换为 external", uploadServer);
  });
  el("server-self-signed").addEventListener("click", () => {
    runAction("settings.inbound.regenerate", "重新生成自签证书", regenerateServer);
  });
  el("server-cert-download").addEventListener("click", (event) => {
    event.preventDefault();
    runAction("settings.inbound.download", "下载公开证书（PEM）", downloadServerCert);
  });
  el("verify-form").addEventListener("submit", (event) => {
    event.preventDefault();
    runVerify();
  });

  refresh();
});
