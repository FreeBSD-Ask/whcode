//! # 前端静态资产（T093/T137 装配用、ADR-014 服务端直出）
//!
//! 资产在**编译期嵌入二进制**（`include_str!`）：运行期不依赖工作目录里的前端目录，
//! 也避免按路径读取引入穿越面。路由表是固定白名单（不做目录遍历）——未知路径一律
//! `None`，由调用方返回 404。
//!
//! 与 `ci/benchmarks/check-frontend-budget.py` 审计的是**同一批源文件**（体积预算
//! 不因“嵌入二进制”而漂移；审计在源文件上执行）。
//!
//! 安全边界：静态资产是界面外壳（无业务数据），由服务端直接返回；**业务数据全部经
//! `/api/*` 且受认证/Origin 校验**（`api::handle_request`，ADR-022）。

/// 首页（`/` 与 `/index.html`）。
pub const INDEX_HTML: &str = include_str!("../frontend/index.html");
/// 主脚本（原生 ESM，无构建链）。
pub const APP_JS: &str = include_str!("../frontend/app.js");
/// 挂起状态机（T136/FR-617）。
pub const SUSPEND_JS: &str = include_str!("../frontend/suspend.js");
/// 样式（仅 transform/opacity 动效，FR-613/T133）。
pub const STYLES_CSS: &str = include_str!("../frontend/styles.css");

/// 静态资产路由：返回 `(Content-Type, 响应体)`；未知路径返回 `None`。
///
/// 只接受 GET 语义下声明的四个路径（含首页别名）；查询串已由调用方剥离。
///
/// 首页在本函数中仍是**未注入数据**的模板；服务端直出版本由 [`render_index`] 生成
/// （无 JS 环境下的只读快照，FR-618）。
#[must_use]
pub fn static_asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    let (content_type, body): (&'static str, &'static str) = match path {
        "/" | "/index.html" => ("text/html; charset=utf-8", INDEX_HTML),
        "/app.js" => ("text/javascript; charset=utf-8", APP_JS),
        "/suspend.js" => ("text/javascript; charset=utf-8", SUSPEND_JS),
        "/styles.css" => ("text/css; charset=utf-8", STYLES_CSS),
        _ => return None,
    };
    Some((content_type, body.as_bytes()))
}

/// 首屏注入点：服务端把只读快照渲染到该注释位置（见 [`render_index`]）。
pub const INITIAL_VIEW_MARKER: &str = "<!-- WHCODE_INITIAL_VIEW -->";

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
pub fn render_index(sessions: &[InitialSession], truncated: bool, error: Option<&str>) -> String {
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
    INDEX_HTML.replace(INITIAL_VIEW_MARKER, &block)
}

/// 认证模式下的首页兜底：**不注入任何业务数据**（ADR-022：业务数据只经认证后的
/// `/api/*` 进出；浏览器导航不会携带 Bearer 头，因此这里不提供快照）。
///
/// 与服务端是否“查到了数据”无关——这是权限边界，不是查询失败，故不复用
/// [`render_index`] 的错误分支（避免把“未授权”说成“数据不可用”）。
#[must_use]
pub fn render_index_auth_required() -> String {
    let block = "<noscript>\n      \
                 <p>无 JS 只读快照仅在本机回环模式（未启用认证）提供；\
                 当前监听启用了认证，请启用 JavaScript 并经 `/api/*` 认证后查看（ADR-022）。</p>\n    \
                 </noscript>";
    INDEX_HTML.replace(INITIAL_VIEW_MARKER, block)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 路由白名单：四个资产可达；未知/穿越路径一律 `None`（无目录遍历面）。
    #[test]
    fn asset_routes_are_fixed_whitelist() {
        for (path, content_type) in [
            ("/", "text/html"),
            ("/index.html", "text/html"),
            ("/app.js", "text/javascript"),
            ("/suspend.js", "text/javascript"),
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
        let html = render_index(&sessions, false, None);
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
        let html = render_index(&[], false, Some("查询失败：<db down>"));
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
        let html = render_index(&[], false, None);
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
        let html = render_index(&sessions, true, None);
        assert!(html.contains("noscript-truncated"), "截断必须可见");
        assert!(html.contains("可能还有更多"), "必须说明还有未显示的数据");
    }

    /// 认证模式：快照一律不注入（业务数据只经 `/api/*`，ADR-022）。
    #[test]
    fn auth_mode_renderer_withholds_snapshot() {
        let html = render_index_auth_required();
        assert!(html.contains("<noscript>"), "仍需无 JS 提示");
        assert!(html.contains("仅在本机回环模式"), "必须写明提供条件");
        assert!(
            !html.contains("noscript-sessions"),
            "认证模式不得渲染任何会话列表"
        );
        assert!(!html.contains(INITIAL_VIEW_MARKER), "标记必须被替换");
    }
}
