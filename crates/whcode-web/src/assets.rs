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
}
