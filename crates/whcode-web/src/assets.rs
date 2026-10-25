//! # 前端静态资产（T093/T137 装配用、ADR-014 服务端直出）
//!
//! 资产在**编译期嵌入二进制**（`include_str!`）：运行期不依赖工作目录里的前端目录，
//! 也避免按路径读取引入穿越面。路由表是固定白名单（不做目录遍历）——未知路径一律
//! `None`，由调用方返回 404。
//!
//! 与 `.github/scripts/benchmarks/check-frontend-budget.py` 审计的是**同一批源文件**（体积预算
//! 不因“嵌入二进制”而漂移；审计在源文件上执行）。
//!
//! 安全边界：静态资产是界面外壳（无业务数据），由服务端直接返回；**业务数据全部经
//! `/api/*` 且受认证/Origin 校验**（`api::handle_request`，ADR-022）。

use std::path::{Path, PathBuf};

use whcode_domain::TlsStatus;
use whcode_domain::tls::{CertFileStatus, SERVER_CERT_FILE, SERVER_KEY_FILE};

/// 首页（`/` 与 `/index.html`）。
pub const INDEX_HTML: &str = include_str!("../frontend/index.html");
/// 主脚本（原生 ESM，无构建链）。
pub const APP_JS: &str = include_str!("../frontend/app.js");
/// 挂起状态机（T136/FR-617）。
pub const SUSPEND_JS: &str = include_str!("../frontend/suspend.js");
/// 样式（仅 transform/opacity 动效，FR-613/T133）。
pub const STYLES_CSS: &str = include_str!("../frontend/styles.css");
/// 证书设置页（T190/FR-627；服务端直出，见 [`render_settings_page`]）。
pub const SETTINGS_HTML: &str = include_str!("../frontend/settings.html");
/// 证书设置页脚本（原生 ESM，无构建链）。
pub const SETTINGS_JS: &str = include_str!("../frontend/settings.js");

/// 静态资产路由：返回 `(Content-Type, 响应体)`；未知路径返回 `None`。
///
/// 只接受 GET 语义下声明的六个路径（含首页别名）；查询串已由调用方剥离。
///
/// 首页在本函数中仍是**未注入数据**的模板；服务端直出版本由 [`render_index`] 生成
/// （无 JS 环境下的只读快照，FR-618）。设置页同理：本函数返回未注入状态的模板，
/// 注入版本由 [`render_settings_page`] 生成（装配根的 `respond` 先于本路由处理
/// `/settings/certs`）。
#[must_use]
pub fn static_asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    let (content_type, body): (&'static str, &'static str) = match path {
        "/" | "/index.html" => ("text/html; charset=utf-8", INDEX_HTML),
        "/app.js" => ("text/javascript; charset=utf-8", APP_JS),
        "/suspend.js" => ("text/javascript; charset=utf-8", SUSPEND_JS),
        "/settings.js" => ("text/javascript; charset=utf-8", SETTINGS_JS),
        "/settings/certs" => ("text/html; charset=utf-8", SETTINGS_HTML),
        "/styles.css" => ("text/css; charset=utf-8", STYLES_CSS),
        _ => return None,
    };
    Some((content_type, body.as_bytes()))
}

/// 首屏注入点：服务端把只读快照渲染到该注释位置（见 [`render_index`]）。
pub const INITIAL_VIEW_MARKER: &str = "<!-- WHCODE_INITIAL_VIEW -->";

/// 合法主题四套（FR-612/713、T175）：服务端注入的默认主题必须取自本表。
pub const THEMES: [&str; 4] = ["eink-day", "eink-night", "amoled-night", "terminal-green"];

const HTML_OPEN: &str = "<html lang=\"zh-CN\">";

/// 把服务端默认主题注入 `<html data-theme="…">`（T175）：非法值回落 `eink-day`
/// （配置笔误不得让首页 500）；浏览器端 `?theme=` 与选择器可在其上运行时覆盖。
#[must_use]
pub fn with_default_theme(html: String, default_theme: &str) -> String {
    let theme = if THEMES.contains(&default_theme) {
        default_theme
    } else {
        "eink-day"
    };
    html.replacen(
        HTML_OPEN,
        &format!("<html lang=\"zh-CN\" data-theme=\"{theme}\">"),
        1,
    )
}

/// 首屏只读快照的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitialSession {
    /// 会话 ID。
    pub id: String,
    /// 标题原文（渲染时统一转义）。
    pub title: String,
    /// 状态（稳定名）。
    pub status: String,
}

/// HTML 文本转义（`& < > " '`）：任何来自数据库的文本都必须先转义再拼进 HTML。
#[must_use]
pub fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// 服务端直出首页：把只读会话快照渲染进 `<noscript>` 块。
///
/// 有 JS 时浏览器不显示 `<noscript>`，交互仍走 `/api/*` 与原生 ESM；无 JS 时至少能读到
/// 会话清单（`FR-618` 的“无 JS 时只读视图可用”判据）。查询失败必须渲染成**可见的错误**，
/// 不得以“空列表”冒充“没有会话”（信号链完备原则）；`truncated` 为真时必须写明“列表被
/// 截断”，不得让读者把“只显示 N 条”误当成“总共只有 N 条”。
#[must_use]
pub fn render_index(
    sessions: &[InitialSession],
    truncated: bool,
    error: Option<&str>,
    default_theme: &str,
) -> String {
    let mut block = String::from("<noscript>\n      <h2>只读快照 / Read-only snapshot</h2>\n");
    if let Some(message) = error {
        block.push_str(&format!(
            "      <p class=\"noscript-error\">数据不可用 / Data unavailable：{}</p>\n",
            escape_html(message)
        ));
    } else {
        block.push_str(&format!(
            "      <p>共 {} 个会话。启用 JavaScript 获得完整交互。/ \
             {} session(s) listed; enable JavaScript for full interaction.</p>\n",
            sessions.len(),
            sessions.len()
        ));
        if truncated {
            block.push_str(
                "      <p class=\"noscript-truncated\">列表已截断：仅显示前若干条，可能还有更多。\
                 / List truncated: earlier sessions only, more may exist.</p>\n",
            );
        }
        if sessions.is_empty() {
            block.push_str("      <p>当前没有会话 / No sessions.</p>\n");
        } else {
            block.push_str("      <ul class=\"noscript-sessions\">\n");
            for row in sessions {
                block.push_str(&format!(
                    "        <li><code>{}</code> <span>{}</span> <em>{}</em></li>\n",
                    escape_html(&row.id),
                    escape_html(&row.title),
                    escape_html(&row.status)
                ));
            }
            block.push_str("      </ul>\n");
        }
    }
    block.push_str("    </noscript>");
    with_default_theme(
        INDEX_HTML.replace(INITIAL_VIEW_MARKER, &block),
        default_theme,
    )
}

/// 认证模式下的首页兜底：**不注入任何业务数据**（ADR-022：业务数据只经认证后的
/// `/api/*` 进出；浏览器导航不会携带 Bearer 头，因此这里不提供快照）。
///
/// 与服务端是否“查到了数据”无关——这是权限边界，不是查询失败，故不复用
/// [`render_index`] 的错误分支（避免把“未授权”说成“数据不可用”）。
#[must_use]
pub fn render_index_auth_required(default_theme: &str) -> String {
    let block = "<noscript>\n      \
                 <p>无 JS 只读快照仅在本机回环模式（未启用认证）提供；\
                 当前监听启用了认证，请启用 JavaScript 并经 `/api/*` 认证后查看（ADR-022）。</p>\n    \
                 </noscript>";
    with_default_theme(
        INDEX_HTML.replace(INITIAL_VIEW_MARKER, block),
        default_theme,
    )
}

/// 设置页注入点：服务端把只读状态摘要渲染到该注释位置（见 [`render_settings_page`]）。
pub const SETTINGS_STATUS_MARKER: &str = "<!-- WHCODE_SETTINGS_STATUS -->";

/// 语言是否按简体中文渲染服务端直出文本（`zh*`；其余按英语）。
fn prefers_chinese(locale: &str) -> bool {
    locale.trim().to_ascii_lowercase().starts_with("zh")
}

/// 布尔词（服务端直出摘要用；`null` 不进入本函数——缺失一律走“未知”）。
fn bool_word(value: bool, zh: bool) -> &'static str {
    match (value, zh) {
        (true, true) => "是",
        (true, false) => "yes",
        (false, true) => "否",
        (false, false) => "no",
    }
}

/// 可选文本：缺失渲染为“未知”，不把未探测说成“没有”。
fn optional_text(value: Option<&str>, zh: bool) -> String {
    match value {
        Some(text) => escape_html(text),
        None => {
            if zh {
                "未知".to_string()
            } else {
                "unknown".to_string()
            }
        }
    }
}

/// 单个证书文件的状态行（路径、份数、SHA-256、已修改、内置版本较新）。
fn cert_file_line(file: &CertFileStatus, zh: bool) -> String {
    let path = escape_html(&file.path);
    let sha = escape_html(&file.sha256);
    let modified = bool_word(file.modified, zh);
    let mut line = if zh {
        format!(
            "路径：{path}；份数：{}；SHA-256：{sha}；已修改：{modified}",
            file.cert_count
        )
    } else {
        format!(
            "path: {path}; count: {}; SHA-256: {sha}; modified: {modified}",
            file.cert_count
        )
    };
    if file.newer_bundled_available {
        line.push_str(if zh {
            "；内置版本较新：是"
        } else {
            "; newer bundled version available: yes"
        });
    }
    line
}

/// 证书库目录：由状态里的证书文件路径反推（入站文件名是固定常量）。
fn cert_dir(status: &TlsStatus) -> Option<PathBuf> {
    status
        .outbound
        .builtin
        .as_ref()
        .or(status.outbound.extra.as_ref())
        .and_then(|file| Path::new(&file.path).parent().map(Path::to_path_buf))
}

/// 只读状态摘要的正文行（每行已转义，供 `<pre>` 直出）。
fn settings_status_lines(status: &TlsStatus, zh: bool) -> Vec<String> {
    let unknown = if zh {
        "未知".to_string()
    } else {
        "unknown".to_string()
    };
    let mut lines = Vec::new();

    lines.push(if zh {
        format!("出站档位：{}", status.outbound.mode.as_str())
    } else {
        format!("outbound mode: {}", status.outbound.mode.as_str())
    });
    lines.push(match &status.outbound.builtin {
        Some(file) => cert_file_line(file, zh),
        None => {
            if zh {
                "内置副本缺失：状态未报告该文件".to_string()
            } else {
                "bundled copy missing: status did not report the file".to_string()
            }
        }
    });
    lines.push(match &status.outbound.extra {
        Some(file) => cert_file_line(file, zh),
        None => {
            if zh {
                "未配置追加根证书（正确的空：文件中没有证书）".to_string()
            } else {
                "no extra CA configured (correctly empty: no certificates in the file)".to_string()
            }
        }
    });
    lines.push(match status.outbound.system_root_count {
        Some(count) => {
            if zh {
                format!("系统库根证书数：{count}")
            } else {
                format!("system root certificates: {count}")
            }
        }
        None => if zh {
            "系统库根证书数：未涉及（当前档位不读取系统库）".to_string()
        } else {
            "system root certificates: not applicable (current mode does not read the system store)"
                .to_string()
        },
    });
    lines.push(match status.outbound.effective_root_count {
        Some(count) => {
            if zh {
                format!("生效根证书总数：{count}")
            } else {
                format!("effective root certificates: {count}")
            }
        }
        None => {
            if zh {
                "生效根证书总数：未知".to_string()
            } else {
                "effective root certificates: unknown".to_string()
            }
        }
    });

    lines.push(if zh {
        format!("入站档位：{}", status.inbound.mode.as_str())
    } else {
        format!("inbound mode: {}", status.inbound.mode.as_str())
    });
    let subject = optional_text(status.inbound.subject.as_deref(), zh);
    let sans = if status.inbound.sans.is_empty() {
        unknown.clone()
    } else {
        escape_html(&status.inbound.sans.join("、"))
    };
    lines.push(if zh {
        format!("服务端证书主题：{subject}；SAN：{sans}")
    } else {
        format!("server certificate subject: {subject}; SAN: {sans}")
    });
    let not_after = optional_text(status.inbound.not_after.as_deref(), zh);
    let days = status
        .inbound
        .days_remaining
        .map_or_else(|| unknown.clone(), |days| days.to_string());
    lines.push(if zh {
        format!("有效期至：{not_after}（剩余 {days} 天）")
    } else {
        format!("valid until: {not_after} (days remaining: {days})")
    });
    let sha = optional_text(status.inbound.sha256.as_deref(), zh);
    lines.push(if zh {
        format!("服务端证书指纹：{sha}")
    } else {
        format!("server certificate fingerprint: {sha}")
    });
    let cert_present = bool_word(status.inbound.cert_present, zh);
    let key_present = bool_word(status.inbound.key_present, zh);
    lines.push(if zh {
        format!("证书存在：{cert_present}；私钥存在：{key_present}")
    } else {
        format!("certificate present: {cert_present}; private key present: {key_present}")
    });

    let (cert_path, key_path) = match cert_dir(status) {
        Some(dir) => (
            dir.join(SERVER_CERT_FILE).display().to_string(),
            dir.join(SERVER_KEY_FILE).display().to_string(),
        ),
        None => (SERVER_CERT_FILE.to_string(), SERVER_KEY_FILE.to_string()),
    };
    lines.push(if zh {
        format!(
            "文件路径：证书 {}；私钥 {}（私钥只进不出，页面不显示内容）",
            escape_html(&cert_path),
            escape_html(&key_path)
        )
    } else {
        format!(
            "file paths: certificate {}; private key {} (write-only, never displayed)",
            escape_html(&cert_path),
            escape_html(&key_path)
        )
    });

    lines.push(if status.restart_required {
        if zh {
            "重启后生效：是（入站证书或档位已变更）".to_string()
        } else {
            "takes effect after restart: yes (inbound certificate or mode changed)".to_string()
        }
    } else if zh {
        "重启后生效：否（当前运行状态与保存的档位一致）".to_string()
    } else {
        "takes effect after restart: no (running state matches saved mode)".to_string()
    });

    lines.push(if status.notes.is_empty() {
        if zh {
            "提示：无附加提示".to_string()
        } else {
            "notes: no additional notes".to_string()
        }
    } else {
        let separator = if zh { "；" } else { "; " };
        let notes = status
            .notes
            .iter()
            .map(|note| escape_html(note))
            .collect::<Vec<_>>()
            .join(separator);
        if zh {
            format!("提示：{notes}")
        } else {
            format!("notes: {notes}")
        }
    });

    lines
}

/// 不在 `<noscript>` 可见范围内的错误摘要（查询失败与未接线共用的头部）。
fn settings_noscript_open(zh: bool) -> String {
    if zh {
        "<noscript>\n      <h2>只读状态摘要（未启用 JavaScript）</h2>\n".to_string()
    } else {
        "<noscript>\n      <h2>Read-only status (JavaScript disabled)</h2>\n".to_string()
    }
}

/// 服务端直出设置页：把只读状态摘要渲染进 `<noscript>`（T190/FR-627、FR-618）。
///
/// `status = None` 表示证书库**未初始化／未接线**：必须显式说明，不得渲染成
/// “没有证书”（信号链完备原则）；所有动态文本（主题、SAN、指纹、路径、提示）
/// 一律经 [`escape_html`]。
#[must_use]
pub fn render_settings_page(status: Option<&TlsStatus>, locale: &str) -> String {
    let zh = prefers_chinese(locale);
    let mut block = settings_noscript_open(zh);
    match status {
        Some(status) => {
            block.push_str(if zh {
                "      <p>启用 JavaScript 可在本页编辑证书库；以下为服务端直出摘要。</p>\n      <pre>\n"
            } else {
                "      <p>Enable JavaScript to edit the certificate store on this page; the summary below is rendered by the server.</p>\n      <pre>\n"
            });
            for line in settings_status_lines(status, zh) {
                block.push_str("        ");
                block.push_str(&line);
                block.push('\n');
            }
            block.push_str("      </pre>\n");
        }
        None => {
            block.push_str(if zh {
                "      <p class=\"noscript-error\">证书库未初始化或未接线：本页读不到设置（不是“没有证书”）。\
                 请以 serve --web 启动并确认工作区可写（ADR-040 决策 4）。</p>\n"
            } else {
                "      <p class=\"noscript-error\">Certificate store is not initialized or not wired: \
                 settings cannot be read here (this is not “no certificates”). Start serve --web with a \
                 writable workspace (ADR-040 decision 4).</p>\n"
            });
        }
    }
    block.push_str("    </noscript>");
    SETTINGS_HTML.replace(SETTINGS_STATUS_MARKER, &block)
}

/// 服务端直出设置页（状态读取失败）：渲染可见错误摘要，不返回空白页。
///
/// 与 [`render_settings_page`] 的 `None` 分支区分：未接线是权限/装配问题，读取失败是
/// 查询错误（例如文件损坏）；两者都不得渲染成“没有证书”。
#[must_use]
pub fn render_settings_page_error(message: &str, locale: &str) -> String {
    let zh = prefers_chinese(locale);
    let escaped = escape_html(message);
    let mut block = settings_noscript_open(zh);
    block.push_str(&if zh {
        format!("      <p class=\"noscript-error\">证书状态读取失败：{escaped}</p>\n")
    } else {
        format!(
            "      <p class=\"noscript-error\">failed to read certificate status: {escaped}</p>\n"
        )
    });
    block.push_str("    </noscript>");
    SETTINGS_HTML.replace(SETTINGS_STATUS_MARKER, &block)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 路由白名单：六个资产可达；未知/穿越路径一律 `None`（无目录遍历面）。
    #[test]
    fn asset_routes_are_fixed_whitelist() {
        for (path, content_type) in [
            ("/", "text/html"),
            ("/index.html", "text/html"),
            ("/app.js", "text/javascript"),
            ("/suspend.js", "text/javascript"),
            ("/settings.js", "text/javascript"),
            ("/settings/certs", "text/html"),
            ("/styles.css", "text/css"),
        ] {
            let (actual_type, body) =
                static_asset(path).unwrap_or_else(|| panic!("{path} 必须可达"));
            assert!(
                actual_type.starts_with(content_type),
                "{path}: {actual_type}"
            );
            assert!(!body.is_empty(), "{path} 不得为空资产");
        }
        for path in [
            "",
            "/../Cargo.toml",
            "/frontend/app.js",
            "/favicon.ico",
            "/APP.JS",
            "/settings",
            "/settings/certs/",
        ] {
            assert!(static_asset(path).is_none(), "{path:?} 必须 404");
        }
    }

    /// 首页是界面外壳：四个标签与两份脚本的引用齐备（T093 结构断言）。
    #[test]
    fn index_references_tabs_and_modules() {
        for tab in ["sessions", "tasks", "history", "spec"] {
            assert!(
                INDEX_HTML.contains(&format!("data-tab=\"{tab}\"")),
                "首页缺少标签 {tab}"
            );
        }
        for id in ["connection-state", "event-cursor", "main"] {
            assert!(
                INDEX_HTML.contains(&format!("id=\"{id}\"")),
                "缺少元素 {id}"
            );
        }
        assert!(INDEX_HTML.contains("app.js"), "首页必须引用主脚本（ESM）");
        assert!(
            APP_JS.contains("./suspend.js"),
            "挂起状态机必须被主脚本引用"
        );
    }

    /// 注入点必须唯一：替换只发生一次，否则模板与渲染结果的对应关系不可判定。
    #[test]
    fn initial_view_marker_exists_exactly_once() {
        assert_eq!(INDEX_HTML.matches(INITIAL_VIEW_MARKER).count(), 1);
    }

    /// T197/T198：应用外壳结构与 ARIA 关联——七个 tab 与七个面板双向可解析，
    /// 初始只选中一个；头部与状态栏类名就位。
    #[test]
    fn index_declares_app_shell_and_tab_aria_relationships() {
        assert!(
            INDEX_HTML.contains("role=\"tablist\""),
            "头部必须声明 tablist"
        );
        assert!(INDEX_HTML.contains("class=\"app-bar\""), "头部外壳缺失");
        assert!(
            INDEX_HTML.contains("class=\"status-bar\""),
            "底部状态栏缺失"
        );
        for name in [
            "sessions",
            "tasks",
            "history",
            "context",
            "diff",
            "spec",
            "maintenance",
        ] {
            assert!(
                INDEX_HTML.contains(&format!("id=\"tab-{name}\"")),
                "缺少 tab 标识 {name}"
            );
            assert!(
                INDEX_HTML.contains(&format!("aria-controls=\"view-{name}\"")),
                "tab {name} 缺少 aria-controls"
            );
            assert!(
                INDEX_HTML.contains(&format!("id=\"view-{name}\"")),
                "缺少面板 view-{name}"
            );
            assert!(
                INDEX_HTML.contains(&format!("aria-labelledby=\"tab-{name}\"")),
                "面板 view-{name} 缺少 aria-labelledby"
            );
        }
        assert_eq!(INDEX_HTML.matches("role=\"tab\"").count(), 7);
        assert_eq!(INDEX_HTML.matches("role=\"tabpanel\"").count(), 7);
        assert_eq!(
            INDEX_HTML.matches("aria-selected=\"true\"").count(),
            1,
            "初始状态只能选中一个 tab"
        );
    }

    /// 无 JS 兜底（Check 229）：只读快照以服务端 HTML 形式存在，且标题被转义。
    #[test]
    fn render_index_embeds_readonly_snapshot_with_escaping() {
        let sessions = vec![
            InitialSession {
                id: "S-1".to_string(),
                title: "会话 A".to_string(),
                status: "active".to_string(),
            },
            InitialSession {
                id: "S-2".to_string(),
                title: "<script>alert('x')</script>".to_string(),
                status: "archived".to_string(),
            },
        ];
        let html = render_index(&sessions, false, None, "eink-day");
        assert!(html.contains("<noscript>"), "必须有 noscript 兜底");
        assert!(html.contains("只读快照"), "兜底内容必须可读");
        assert!(html.contains("S-1") && html.contains("会话 A"));
        assert!(
            html.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"),
            "标题必须经过 HTML 转义"
        );
        assert!(
            !html.contains("<script>alert('x')</script>"),
            "不得把数据库文本原样拼进 HTML（XSS）"
        );
        assert!(!html.contains(INITIAL_VIEW_MARKER), "标记必须被替换");
    }

    /// 查询失败必须渲染成可见错误，不得以空列表冒充“没有会话”。
    #[test]
    fn render_index_reports_query_error_instead_of_empty_success() {
        let html = render_index(&[], false, Some("查询失败：<db down>"), "eink-day");
        assert!(html.contains("noscript-error"));
        assert!(html.contains("&lt;db down&gt;"), "错误文本同样要转义");
        assert!(
            !html.contains("当前没有会话"),
            "错误状态不得渲染成“没有会话”"
        );
    }

    /// 正确的空：没有会话时明确写“当前没有会话”。
    #[test]
    fn render_index_marks_genuine_empty_state() {
        let html = render_index(&[], false, None, "eink-day");
        assert!(html.contains("当前没有会话 / No sessions."));
        assert!(!html.contains("noscript-error"));
        assert!(!html.contains("noscript-truncated"));
    }

    /// 截断必须显式：不得让“只显示 N 条”冒充“总共只有 N 条”（信号链完备）。
    #[test]
    fn render_index_flags_truncation_instead_of_silent_cut() {
        let sessions = vec![InitialSession {
            id: "S-1".to_string(),
            title: "会话".to_string(),
            status: "active".to_string(),
        }];
        let html = render_index(&sessions, true, None, "eink-day");
        assert!(html.contains("noscript-truncated"), "截断必须可见");
        assert!(html.contains("可能还有更多"), "必须说明还有未显示的数据");
    }

    /// 认证模式：快照一律不注入（业务数据只经 `/api/*`，ADR-022）。
    #[test]
    fn auth_mode_renderer_withholds_snapshot() {
        let html = render_index_auth_required("eink-day");
        assert!(html.contains("<noscript>"), "仍需无 JS 提示");
        assert!(html.contains("仅在本机回环模式"), "必须写明提供条件");
        assert!(
            !html.contains("noscript-sessions"),
            "认证模式不得渲染任何会话列表"
        );
        assert!(!html.contains(INITIAL_VIEW_MARKER), "标记必须被替换");
    }

    /// 服务端默认主题注入（T175）：合法值原样、非法值回落 eink-day；四套主题的
    /// 精确色值由 styles.css 的 data-theme 块承载（静态审计，不依赖浏览器）。
    #[test]
    fn default_theme_is_injected_and_validated() {
        for (input, expected) in [
            ("eink-day", "eink-day"),
            ("terminal-green", "terminal-green"),
            ("neon-pink", "eink-day"),
        ] {
            let html = with_default_theme(INDEX_HTML.to_string(), input);
            assert!(
                html.contains(&format!("<html lang=\"zh-CN\" data-theme=\"{expected}\">")),
                "{input} 应注入 data-theme={expected}"
            );
        }
        for theme in THEMES {
            assert!(
                STYLES_CSS.contains(&format!("[data-theme=\"{theme}\"]")),
                "styles.css 缺主题块 {theme}"
            );
        }
        for hex in ["#ffffff", "#000000", "#e6e6e6", "#00ff00"] {
            assert!(STYLES_CSS.contains(hex), "styles.css 缺色值 {hex}");
        }
    }

    // ── 证书设置页（T190/FR-627） ─────────────────────────────────────────

    use whcode_domain::tls::{InboundMode, OutboundStatus, ServerCertStatus, TrustMode};

    /// 一整套 `TlsStatus` 夹具（字段值与生产状态同形）。
    fn sample_status(subject: &str, note: &str) -> TlsStatus {
        TlsStatus {
            schema_version: whcode_domain::SCHEMA_VERSION,
            outbound: OutboundStatus {
                mode: TrustMode::Builtin,
                source: "builtin".to_string(),
                builtin: Some(CertFileStatus {
                    path: "/w/.whcode/certs/ca-builtin.pem".to_string(),
                    cert_count: 140,
                    sha256: "a".repeat(64),
                    modified: false,
                    bundled_version: Some("2026-10-25".to_string()),
                    bundled_sha256: Some("b".repeat(64)),
                    newer_bundled_available: true,
                }),
                extra: None,
                system_root_count: None,
                effective_root_count: Some(140),
            },
            inbound: ServerCertStatus {
                mode: InboundMode::Builtin,
                cert_present: true,
                key_present: true,
                subject: Some(subject.to_string()),
                sans: vec!["localhost".to_string(), "127.0.0.1".to_string()],
                not_before: Some("2026-10-25T00:00:00Z".to_string()),
                not_after: Some("2027-11-26T00:00:00Z".to_string()),
                sha256: Some("c".repeat(64)),
                days_remaining: Some(397),
            },
            restart_required: true,
            notes: vec![note.to_string()],
        }
    }

    /// 首页必须提供设置页入口（T190：`data-i18n` 文案，无硬编码大段文本）。
    #[test]
    fn index_links_to_certificate_settings_page() {
        assert!(
            INDEX_HTML.contains("href=\"/settings/certs\""),
            "首页缺少 /settings/certs 入口"
        );
        assert!(
            INDEX_HTML.contains("data-i18n=\"settings.link\""),
            "入口文案必须走文本层"
        );
    }

    /// 设置页模板：注入点唯一、绝对资产路径（页面位于 /settings/certs）、
    /// 表单要素与文本层键齐备。
    #[test]
    fn settings_template_declares_form_and_marker() {
        assert_eq!(
            SETTINGS_HTML.matches(SETTINGS_STATUS_MARKER).count(),
            1,
            "注入点必须唯一"
        );
        assert!(
            SETTINGS_HTML.contains("src=\"/settings.js\""),
            "设置页必须引用自身脚本（绝对路径，页面不在站点根）"
        );
        assert!(
            SETTINGS_HTML.contains("href=\"/styles.css\""),
            "设置页样式必须用绝对路径（/settings/certs 下的相对路径会指向 /settings/styles.css）"
        );
        for id in [
            "restart-banner",
            "ca-builtin-pem",
            "ca-extra-pem",
            "ca-builtin-save",
            "ca-extra-reset",
            "modes-save",
            "server-cert-pem",
            "server-key-pem",
            "server-cert-download",
            "server-self-signed",
            "verify-host",
            "verify-port",
        ] {
            assert!(
                SETTINGS_HTML.contains(&format!("id=\"{id}\"")),
                "设置页缺少元素 {id}"
            );
        }
        for key in [
            "settings.title",
            "settings.back",
            "settings.restart_banner",
            "settings.outbound.builtin",
            "settings.inbound.off",
            "settings.modes.save",
            "settings.verify.button",
            "web.theme.label",
        ] {
            assert!(
                SETTINGS_HTML.contains(&format!("data-i18n=\"{key}\"")),
                "设置页缺少文本层键 {key}"
            );
        }
    }

    /// 服务端直出设置页：动态文本（主题、SAN、指纹、路径、提示）必须转义，
    /// `<noscript>` 摘要含状态与文件路径（无 JS 时可见，FR-618／Check 286）。
    #[test]
    fn render_settings_page_escapes_dynamic_status_text() {
        let status = sample_status(
            "<script>alert('x')</script>",
            "备注 \"<img src=x onerror=1>\"",
        );
        let html = render_settings_page(Some(&status), "zh-CN");
        assert!(html.contains("<noscript>"), "必须有无 JS 兜底");
        assert!(html.contains("<pre>"), "摘要以纯文本块呈现");
        assert!(
            html.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"),
            "主题必须经 HTML 转义"
        );
        assert!(
            !html.contains("<script>alert('x')</script>"),
            "不得把状态文本原样拼进 HTML（XSS）"
        );
        assert!(
            html.contains("&quot;&lt;img src=x onerror=1&gt;&quot;"),
            "提示文本同样要转义"
        );
        assert!(!html.contains(SETTINGS_STATUS_MARKER), "标记必须被替换");
        assert!(html.contains("份数：140"), "份数必须可见");
        assert!(html.contains("ca-builtin.pem"), "必须给出证书库文件路径");
        assert!(
            html.contains("server-cert.pem") && html.contains("server-key.pem"),
            "必须给出入站证书与私钥路径（私钥路径非秘密，内容不显示）"
        );
        assert!(html.contains("重启后生效：是"), "重启标记必须如实呈现");
        assert!(
            !html.contains("PRIVATE KEY"),
            "页面不得出现私钥内容：{html}"
        );
    }

    /// 未接线（`status = None`）：显式说明读不到设置，不得渲染成“没有证书”。
    #[test]
    fn render_settings_page_reports_unwired_without_fake_state() {
        let html = render_settings_page(None, "zh-CN");
        assert!(html.contains("noscript-error"), "必须渲染可见错误");
        assert!(html.contains("未初始化或未接线"), "必须说明未接线");
        assert!(!html.contains("份数："), "未接线不得渲染份数（不伪造状态）");
        // 语言链：非 zh 请求按英语直出（服务端资源，不依赖 JS 文本层）。
        let english = render_settings_page(None, "en");
        assert!(
            english.contains("not initialized or not wired"),
            "英语 locale 必须给英语摘要"
        );
        assert!(!english.contains(SETTINGS_STATUS_MARKER));
    }

    /// 读取失败：错误摘要可见且转义，与“未接线”区分。
    #[test]
    fn render_settings_page_error_is_visible_and_escaped() {
        let html = render_settings_page_error("settings.json <损坏>", "zh-CN");
        assert!(html.contains("noscript-error"));
        assert!(html.contains("证书状态读取失败"));
        assert!(html.contains("&lt;损坏&gt;"), "错误文本必须转义");
        assert!(!html.contains(SETTINGS_STATUS_MARKER));
        let english = render_settings_page_error("broken", "en");
        assert!(english.contains("failed to read certificate status"));
    }
}
