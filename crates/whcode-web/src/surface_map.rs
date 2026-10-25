//! # Web 路由 → SurfaceId 清单（Task 170.4；FR-619—FR-621、ADR-032）
//!
//! 把 `(Method, 路由)` 映射到 [`whcode_app::surface::SURFACES`] 的稳定 `SurfaceId`：
//!
//! - **现有路由**（`crate::api::handle_request` 的 match 臂）逐一入清单——反向无孤儿
//!   （FR-621：先加路由后补契约即门禁失败）；
//! - **保留守卫路径**：登记为预留／排除但尚未接线的路径（`spec/*`、runtime 写、
//!   `events show`）也入清单——Task 172 对其统一返回 501 `not_supported`；
//!   `llm/*` 已随 T167 接线（Web 登记为已实现），不再走守卫；
//!   未登记路径返回 `None`（调用方仍为 404，不得把已登记能力伪装成 404）；
//! - **本变更新增路由**（`/api/context`、`/api/diff`、`/api/events/export`）先入清单，
//!   由 Task 174 接线（登记表对应端当前为预留）。
//!
//! 纯静态映射：不做 IO、不做认证（安全检查在 `crate::security`）。

/// 把 HTTP 方法与路径映射到登记表 `SurfaceId`；未登记路径返回 `None`。
///
/// `method` 取 `GET`／`POST`（[`crate::api::Method`] 的稳定形态）；`path` 可含 query，
/// 匹配前剥除。守卫路径对任意方法返回同一 `SurfaceId`（不可用语义由 Task 172 统一）。
#[must_use]
pub fn surface_of(method: &str, path: &str) -> Option<&'static str> {
    let path = path.split('?').next().unwrap_or(path);
    match (method, path) {
        // ── 现有读路由（M6 口径） ─────────────────────────────────────
        ("GET", "/api/status") => Some("spec.status"),
        ("GET", "/api/sessions") => Some("session.list"),
        ("GET", "/api/events") => Some("events.list"),
        ("GET", "/api/tasks") => Some("task.overview"),
        ("GET", "/api/runtimes") => Some("runtime.list"),
        ("GET", "/api/i18n") => Some("locale"),
        ("GET", "/ws") => Some("view.changes"),
        // ── 现有写路由（FR-609；默认预览、显式确认） ───────────────────
        ("POST", "/api/sessions/new") => Some("session.new"),
        ("POST", "/api/sessions/rename") => Some("session.rename"),
        ("POST", "/api/sessions/archive") => Some("session.archive"),
        ("POST", "/api/sessions/delete") => Some("session.delete"),
        ("POST", "/api/tasks/retry") => Some("task.retry"),
        ("POST", "/api/tasks/abort") => Some("task.abort"),
        ("POST", "/api/purge") => Some("db.purge"),
        // ── 本变更新增读路由（Task 174 接线；当前登记为预留） ──────────
        ("GET", "/api/context") => Some("context.budget"),
        ("GET", "/api/diff") => Some("view.diff"),
        ("GET", "/api/events/export") => Some("events.export"),
        // ── 证书库设置（T190、ADR-040 决策 4）：读面与连通性校验 ───────
        ("GET", "/api/certs") => Some("settings.cert.read"),
        ("GET", "/api/certs/ca") => Some("settings.cert.read"),
        ("GET", "/api/certs/server") => Some("settings.cert.read"),
        ("POST", "/api/certs/verify") => Some("settings.cert.read"),
        // ── 证书库设置：写面（档位、根证书、服务端证书） ───────────────
        ("POST", "/api/certs/modes") => Some("settings.cert.write"),
        ("POST", "/api/certs/ca") => Some("settings.cert.write"),
        ("POST", "/api/certs/ca/reset") => Some("settings.cert.write"),
        ("POST", "/api/certs/server") => Some("settings.cert.write"),
        ("POST", "/api/certs/server/self-signed") => Some("settings.cert.write"),
        // ── LLM 调用（T167 接线；Web 端为已实现） ─────────────────────
        ("POST", "/api/llm/run") => Some("llm.run"),
        ("POST", "/api/llm/cancel") => Some("llm.cancel"),
        // ── 保留守卫路径（预留／排除；Task 172 统一 501 not_supported） ─
        (_, "/api/events/show") => Some("events.show"),
        (_, "/api/spec/next") => Some("spec.next"),
        (_, "/api/spec/validate") => Some("spec.validate"),
        (_, "/api/spec/analyze") => Some("spec.analyze"),
        (_, "/api/spec/converge") => Some("spec.converge"),
        (_, "/api/spec/force-advance") => Some("spec.force-advance"),
        (_, "/api/runtimes/status") => Some("runtime.status"),
        (_, "/api/runtimes/enable") => Some("runtime.enable"),
        (_, "/api/runtimes/disable") => Some("runtime.disable"),
        (_, "/api/runtimes/install") => Some("runtime.install"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_app::surface::{FrontendId, Support, find};

    /// 现有 api.rs 路由逐条与清单一致，且对应面的 Web 状态为已实现。
    #[test]
    fn existing_routes_map_to_implemented_surfaces() {
        let routes = [
            ("GET", "/api/status", "spec.status"),
            ("GET", "/api/sessions", "session.list"),
            ("GET", "/api/events", "events.list"),
            ("GET", "/api/tasks", "task.overview"),
            ("GET", "/api/runtimes", "runtime.list"),
            ("GET", "/api/i18n", "locale"),
            ("GET", "/ws", "view.changes"),
            ("POST", "/api/sessions/new", "session.new"),
            ("POST", "/api/sessions/rename", "session.rename"),
            ("POST", "/api/sessions/archive", "session.archive"),
            ("POST", "/api/sessions/delete", "session.delete"),
            ("POST", "/api/tasks/retry", "task.retry"),
            ("POST", "/api/tasks/abort", "task.abort"),
            ("POST", "/api/purge", "db.purge"),
        ];
        for (method, path, id) in routes {
            assert_eq!(
                surface_of(method, path),
                Some(id),
                "{method} {path} 必须映射 {id}"
            );
            let entry = find(id).unwrap_or_else(|| panic!("{id} 未登记"));
            assert!(
                matches!(entry.support(FrontendId::Web), Support::Implemented { .. }),
                "{id} 的 Web 状态必须为已实现（现有路由不得指向未交付面）"
            );
        }
    }

    /// query 参数不参与匹配；带 query 的路径与裸路径同映射。
    #[test]
    fn query_strings_are_stripped_before_matching() {
        assert_eq!(
            surface_of("GET", "/api/status?session=S-1"),
            Some("spec.status")
        );
        assert_eq!(
            surface_of("GET", "/api/events?after=0"),
            Some("events.list")
        );
        assert_eq!(surface_of("GET", "/api/i18n?locale=en"), Some("locale"));
    }

    /// 本变更新增路由先入清单；Task 174 接线后 Web 端翻转为已实现（与登记表一致）。
    #[test]
    fn delivered_routes_map_to_implemented_surfaces() {
        let routes = [
            ("GET", "/api/context", "context.budget"),
            ("GET", "/api/diff", "view.diff"),
            ("GET", "/api/events/export", "events.export"),
            // 证书库设置（T190）：读面与写面均已在 Web 实现（ADR-040 决策 4）。
            ("GET", "/api/certs", "settings.cert.read"),
            ("GET", "/api/certs/ca", "settings.cert.read"),
            ("GET", "/api/certs/server", "settings.cert.read"),
            ("POST", "/api/certs/verify", "settings.cert.read"),
            ("POST", "/api/certs/modes", "settings.cert.write"),
            ("POST", "/api/certs/ca", "settings.cert.write"),
            ("POST", "/api/certs/ca/reset", "settings.cert.write"),
            ("POST", "/api/certs/server", "settings.cert.write"),
            (
                "POST",
                "/api/certs/server/self-signed",
                "settings.cert.write",
            ),
            // LLM 调用（T167）：CUI／Web 已接线；TUI 仍预留、GUI 只读排除。
            ("POST", "/api/llm/run", "llm.run"),
            ("POST", "/api/llm/cancel", "llm.cancel"),
        ];
        for (method, path, id) in routes {
            assert_eq!(surface_of(method, path), Some(id), "{method} {path}");
            let entry = find(id).expect("已登记");
            match entry.support(FrontendId::Web) {
                Support::Implemented { .. } => {}
                other => panic!("已交付路由 {id} 的 Web 状态必须为已实现，实际 {other:?}"),
            }
        }
    }

    /// 保留守卫路径返回正确 id，且对应面在 Web 端为预留或排除（不得已实现）。
    #[test]
    fn guarded_paths_map_to_registered_unavailable_surfaces() {
        let guards = [
            ("GET", "/api/events/show", "events.show"),
            ("POST", "/api/spec/next", "spec.next"),
            ("POST", "/api/spec/validate", "spec.validate"),
            ("POST", "/api/spec/analyze", "spec.analyze"),
            ("POST", "/api/spec/converge", "spec.converge"),
            ("POST", "/api/spec/force-advance", "spec.force-advance"),
            ("GET", "/api/runtimes/status", "runtime.status"),
            ("POST", "/api/runtimes/enable", "runtime.enable"),
            ("POST", "/api/runtimes/disable", "runtime.disable"),
            ("POST", "/api/runtimes/install", "runtime.install"),
        ];
        for (method, path, id) in guards {
            assert_eq!(surface_of(method, path), Some(id), "{method} {path}");
            let entry = find(id).expect("已登记");
            match entry.support(FrontendId::Web) {
                Support::Reserved { .. } | Support::Excluded { .. } => {}
                other => panic!("守卫路径 {id} 的 Web 状态必须为预留或排除，实际 {other:?}"),
            }
        }
    }

    /// 未知路径不入清单（调用方保持 404，FR-620）。
    #[test]
    fn unknown_paths_are_not_registered() {
        assert_eq!(surface_of("GET", "/api/unknown"), None);
        assert_eq!(surface_of("GET", "/"), None);
        assert_eq!(surface_of("GET", "/api/spec/bogus"), None);
        assert_eq!(surface_of("DELETE", "/api/sessions"), None);
        assert_eq!(surface_of("PUT", "/api/purge"), None);
    }
}
