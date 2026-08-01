//! HTTP method policy for the kernel REST surface.
//!
//! Business dispatch intentionally remains in `rest`: this module is the
//! edge-only gate that prevents a request with the wrong verb from reaching
//! body parsing, authentication, storage, or a stateful realtime operation.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MethodPolicy {
    allowed: &'static [&'static str],
}

impl MethodPolicy {
    pub(crate) fn allowed(self) -> &'static [&'static str] {
        self.allowed
    }

    pub(crate) fn allow_header(self) -> String {
        self.allowed.join(", ")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MethodGate {
    /// The method is valid and normal request processing should continue.
    Dispatch,
    /// A known route answered its method-discovery request at the edge.
    Options(MethodPolicy),
    /// The path is known, but this method may not dispatch it.
    MethodNotAllowed(MethodPolicy),
    /// Preserve the router's existing authentication/not-found behavior.
    UnknownRoute,
}

const GET: MethodPolicy = MethodPolicy {
    allowed: &["GET", "HEAD", "OPTIONS"],
};
const STATEFUL_GET: MethodPolicy = MethodPolicy {
    allowed: &["GET", "OPTIONS"],
};
const POST: MethodPolicy = MethodPolicy {
    allowed: &["POST", "OPTIONS"],
};
const APP_ROUTE: MethodPolicy = MethodPolicy {
    allowed: &["GET", "POST", "OPTIONS"],
};

/// Classify a request before any route implementation can run.
///
/// `HEAD` is deliberately narrower than `GET`: `/events/{sub}` drains queued
/// events and app-provided GET handlers have no read-only declaration today.
/// Running either merely to construct HEAD headers would perform hidden work.
pub(crate) fn gate(method: &str, url: &str) -> MethodGate {
    let Some(policy) = policy_for(url) else {
        return MethodGate::UnknownRoute;
    };

    if method == "OPTIONS" {
        return MethodGate::Options(policy);
    }
    if policy.allowed.contains(&method) {
        MethodGate::Dispatch
    } else {
        MethodGate::MethodNotAllowed(policy)
    }
}

fn policy_for(url: &str) -> Option<MethodPolicy> {
    let path = url.split('?').next().unwrap_or(url);
    // Keep segmentation byte-for-byte aligned with `Rest::route`: in
    // particular, an empty interior segment must not turn a malformed path
    // into a known route at the method gate.
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();

    match segs.as_slice() {
        ["health" | "metrics" | "ready"] => Some(GET),
        ["login" | "logout"] | ["revoke", _] => Some(POST),
        ["meta"] | ["meta", _] => Some(GET),
        ["doctype"]
        | ["doctype", _, "script" | "reclaim"]
        | ["notification"]
        | ["transition", _, _]
        | ["subscribe", _]
        | ["unsubscribe", _]
        | ["read", _]
        | ["write", _]
        | ["aggregate", _]
        | ["enqueue", _] => Some(POST),
        ["mail", "outbox"] | ["workflow", _, _] | ["audit", _, _] | ["lag", _] | ["app"] => {
            Some(GET)
        }
        // Polling consumes the in-process queue. It is GET for compatibility,
        // but HEAD must not drain it while throwing the representation away.
        ["events", _] => Some(STATEFUL_GET),
        ["app", "plan" | "install" | "update"] | ["app", _, "disable" | "enable" | "uninstall"] => {
            Some(POST)
        }
        // The current route-plugin world declares a path, not per-verb
        // read/write semantics. Preserve its documented GET|POST contract and
        // refuse HEAD until the manifest can declare a handler read-only.
        ["app", _, _] => Some(APP_ROUTE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{gate, MethodGate};

    fn allow(method: &str, path: &str) -> Vec<&'static str> {
        match gate(method, path) {
            MethodGate::Options(policy) | MethodGate::MethodNotAllowed(policy) => {
                policy.allowed().to_vec()
            }
            other => panic!("expected an edge response, got {other:?}"),
        }
    }

    #[test]
    fn mutating_core_routes_are_post_only() {
        for path in [
            "/login",
            "/logout",
            "/revoke/alice",
            "/doctype",
            "/doctype/Invoice/script",
            "/doctype/Invoice/reclaim",
            "/notification",
            "/transition/Invoice/one",
            "/subscribe/Invoice",
            "/unsubscribe/sub-1",
            "/read/Invoice",
            "/write/Invoice",
            "/aggregate/Invoice",
            "/enqueue/rebuild",
            "/app/plan",
            "/app/install",
            "/app/update",
            "/app/accounts/disable",
            "/app/accounts/enable",
            "/app/accounts/uninstall",
        ] {
            assert_eq!(gate("POST", path), MethodGate::Dispatch, "{path}");
            assert_eq!(allow("GET", path), ["POST", "OPTIONS"], "{path}");
        }
    }

    #[test]
    fn read_only_routes_support_get_and_head() {
        for path in [
            "/health",
            "/metrics",
            "/ready",
            "/meta",
            "/meta/Invoice",
            "/mail/outbox",
            "/workflow/Invoice/one",
            "/audit/Invoice/one",
            "/lag/balance",
            "/app",
        ] {
            assert_eq!(gate("GET", path), MethodGate::Dispatch, "{path}");
            assert_eq!(gate("HEAD", path), MethodGate::Dispatch, "{path}");
            assert_eq!(allow("POST", path), ["GET", "HEAD", "OPTIONS"], "{path}");
        }
    }

    #[test]
    fn head_never_consumes_stateful_or_unclassified_app_gets() {
        assert_eq!(gate("GET", "/events/sub-1"), MethodGate::Dispatch);
        assert_eq!(allow("HEAD", "/events/sub-1"), ["GET", "OPTIONS"]);

        assert_eq!(gate("GET", "/app/accounts/report"), MethodGate::Dispatch);
        assert_eq!(gate("POST", "/app/accounts/report"), MethodGate::Dispatch);
        assert_eq!(
            allow("HEAD", "/app/accounts/report"),
            ["GET", "POST", "OPTIONS"]
        );
    }

    #[test]
    fn options_reports_each_known_routes_allow_set() {
        assert_eq!(
            allow("OPTIONS", "/meta?ignored=true"),
            ["GET", "HEAD", "OPTIONS"]
        );
        assert_eq!(allow("OPTIONS", "/write/Invoice"), ["POST", "OPTIONS"]);
    }

    #[test]
    fn unknown_paths_keep_existing_router_behavior() {
        assert_eq!(gate("GET", "/not-a-route"), MethodGate::UnknownRoute);
        assert_eq!(gate("OPTIONS", "/not-a-route"), MethodGate::UnknownRoute);
        assert_eq!(gate("OPTIONS", "/meta//Invoice"), MethodGate::UnknownRoute);
    }
}
