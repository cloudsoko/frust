//! Module 6: the REST surface. Metadata-generated endpoints that speak the
//! filter contract and nothing else — Desk and external clients are
//! the same door the broker already serves. This is the third consumer of
//! the ONE permission compiler (criterion 2).
//!
//! Auth: session tokens. `POST /login {user, pass}`
//! proves credentials against the record access, derives the ROLE FROM THE
//! DATABASE (never the client), and mints an opaque token; every other data
//! route requires `Authorization: Bearer <token>`. Both permission halves â€”
//! rows (the DB's PERMISSIONS under the user's own session) and the field
//! envelope (role) â€” derive from the session principal. The X-Frust-* header
//! trio is DELETED; a role claim from the wire has nowhere to land.
//!
//! Endpoints:
//!   GET  /health
//!   POST /login              body: { user, pass } -> { token, user, role }
//!   POST /logout             (bearer)
//!   POST /read/{doctype}     body: { filter?, fields?, limit?, start?, order? }
//!   POST /write/{doctype}    body: { doc: {..}, record? }
//!   POST /aggregate/{doctype} body: { filter?, group_by?, metrics: [...] }
//!   POST /enqueue/{kind}     body: { payload: {..} }
//!   GET  /meta               all doctype metadata (any session)
//!   GET  /meta/{doctype}     one doctype's metadata
//!   POST /doctype            body: the DocType metadata (manager; syncs schema)
//!   GET  /audit/{doctype}/{key}  changefeed entries touching one record (manager)
//!   GET  /lag/{rollup}       Tier-2 staleness: cursor + pending feed entries

use std::sync::Arc;

use crate::broker::{Broker, Caller, HookChain};
use crate::contract::*;
use crate::router::{TenantContext, TenantRouter};
use crate::surql;
use crate::sync::MetadataSync;

/// Kernel session store. PERMISSIONS NONE â€” only the kernel reads it.
/// **Defined at BOOT, in `meta_ddl` — not lazily on the auth path.**
///
/// It used to be created by the login batch itself, on the reasoning that a
/// session table is ephemeral state rather than meta-schema. That reasoning was
/// fine; the *placement* was not. The login statement was
/// `DEFINE TABLE IF NOT EXISTS …; CREATE …;`, SurrealDB runs unbraced statements
/// in SEPARATE transactions, and `sql_with_auth_inner` retries the **whole
/// batch** on a conflict — so when two first-logins raced on the DDL, the CREATE
/// that had already committed ran a second time and the user got a duplicate
/// session row. Measured: `frust_conflict_retries_total 1` on every run of
/// `kernel_hygiene`, which is exactly where its flake came from.
///
/// Now it is an ordinary meta table like every other one
/// (binary-authoritative, applied under the boot lock), and login is a single
/// statement with no DDL in it.
pub(crate) const SESSION_TABLE: &str = "_frust_session";

/// The session-lookup cache — the third and last DB round trip on the
/// per-request path.
///
/// **This one sits in a permission path, so its invalidation is stricter than
/// the metadata cache's.** Two mechanisms, deliberately belt-and-braces:
///
/// 1. **Logout is immediate.** `POST /logout` bumps this tenant's session
///    generation, which invalidates every cached entry **of that tenant** at
///    once. Coarse, and that is the right trade: correctness beats a hit rate,
///    and the re-read costs one query. Invalidation is scoped to this
///    tenant — coarse within a tenant, never across
///    them.)
/// 2. **A short TTL backstops everything else** — session expiry, and
///    out-of-band revocation (an operator deleting the row directly). Without
///    a TTL, a row deleted outside the kernel would keep working until
///    restart, which would be exactly the silent-wrong failure this cache must
///    not introduce.
///
/// **The stated trade:** out-of-band revocation takes effect within
/// `SESSION_TTL`, not instantly. Logout — the path users and the Desk actually
/// take — is instant. A longer TTL would buy throughput at the cost of a wider
/// revocation window; 5 s keeps the window smaller than a human notices while
/// still removing the round trip from the hot path.
const SESSION_TTL: std::time::Duration = std::time::Duration::from_secs(5);

/// **Keyed by `(tenant, token)`, not by token.**
///
/// Two reasons, and the second is the one that matters. A token is only
/// meaningful inside its own tenant, so tenant-keying is what lets one
/// tenant's logout drop *its* entries and nobody else's. And it means the
/// cache never relies on 64 random characters not colliding across tenants —
/// a property that is overwhelmingly likely and that an auth path should not
/// be resting on.
type SessionCache = std::sync::Mutex<
    std::collections::HashMap<(String, String), (u64, std::time::Instant, Caller, String)>,
>;

fn session_cache() -> &'static SessionCache {
    static C: std::sync::OnceLock<SessionCache> = std::sync::OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

pub struct Rest {
    /// **N tenants, one process.** Each request finds its own
    /// tenant's `Broker` here; there is no ambient "the broker" any more.
    pub router: Arc<TenantRouter>,
    pub addr: String,
    /// Enables the live-subscription routes. None = 404.
    pub realtime: Option<Arc<crate::realtime::Realtime>>,
}

impl Rest {
    /// The single-tenant shape every existing caller has: one broker, one
    /// sync engine, one tenant. Builds a one-entry router so the *request
    /// path is identical* — a single-tenant kernel is not a special case, it
    /// is a router with one route.
    pub fn single(
        broker: Arc<Broker>,
        addr: String,
        sync: Option<Arc<MetadataSync>>,
        realtime: Option<Arc<crate::realtime::Realtime>>,
    ) -> Self {
        Self {
            router: Arc::new(TenantRouter::single(TenantContext { broker, sync })),
            addr,
            realtime,
        }
    }
}

impl Rest {
    /// Blocking serve loop: a bounded pool of request workers plus ONE
    /// maintenance ticker.
    ///
    /// The previous serve loop took one request, ran it to completion
    /// (auth → hooks → write), and only then accepted the next — so throughput
    /// was flat at ~15 req/s from 1 to 500 concurrent clients and the kernel
    /// pegged 0.85 of a single core. `tiny_http::Server` supports
    /// `recv()` from many threads, so the fix is a pool of consumers on the
    /// same listener — not an async rewrite.
    ///
    /// **The tick is deliberately NOT in the pool.** `on_tick` drains rollups,
    /// and `RollupWorker::drain` is a cursor read-modify-write: two concurrent
    /// drains would read the same changefeed range and apply the same deltas
    /// twice. It runs on exactly one dedicated thread, which is also the only
    /// piece of shared state this change could have broken. (Everything else
    /// the pool now touches in parallel — the session cache, the throttle token
    /// buckets, the route-host cache, the telemetry ring and registry — is
    /// already `Mutex`/atomic-protected, the job claim is atomic DB-side, and
    /// the telemetry trace context is a thread-local, which is
    /// exactly right per worker.)
    ///
    /// Fixed-size pool, sized to cores and capped: memory must stay in the
    /// tens-of-MB class (P-1.4), so thread-per-request is not an option.
    /// `on_tick` is `Send` but NOT `Sync`: it is MOVED onto the one
    /// maintenance thread and never shared, which is what lets it own
    /// non-`Sync` state (the rollup workers' `RefCell` caches) safely.
    pub fn serve(&self, on_tick: impl Fn() + Send) -> anyhow::Result<()> {
        let server = std::sync::Arc::new(
            tiny_http::Server::http(&self.addr)
                .map_err(|e| anyhow::anyhow!("bind {}: {e}", self.addr))?,
        );
        let workers = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(4)
            .clamp(2, 16);
        crate::telemetry::emit(
            crate::telemetry::Level::Info,
            "rest_listening",
            &[
                ("addr", serde_json::json!(self.addr)),
                ("workers", serde_json::json!(workers)),
            ],
        );

        std::thread::scope(|scope| {
            // the single maintenance thread: jobs, rollups, live-sub reaping.
            // A fixed cadence rather than "when idle" — the old design only
            // ticked when no request arrived within 200 ms, so under sustained
            // load background jobs never ran at all.
            scope.spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(200));
                on_tick();
            });

            for _ in 0..workers {
                let server = std::sync::Arc::clone(&server);
                scope.spawn(move || {
                    while let Ok(req) = server.recv() {
                        self.handle(req);
                    }
                });
            }
        });
        Ok(())
    }

    fn handle(&self, mut req: tiny_http::Request) {
        let url = req.url().to_string();
        let method = req.method().as_str().to_string();
        // the trace is born at the edge â€” or adopted from the caller so a
        // client-side flow (Desk action -> follow-on request) stays one trace
        let trace = crate::telemetry::TraceId::parse(&header(&req, "X-Trace-Id"))
            .unwrap_or_default();

        // bearer token before consuming the body reader
        let auth = header(&req, "Authorization");
        let token = auth.strip_prefix("Bearer ").map(str::to_string);
        let host = header(&req, "Host");

        // The trace's tenant label comes from the TOKEN's tenant,
        // not from an ambient broker — with N tenants in one process, "which
        // tenant was this request" is only answerable per request.
        let tenant = token
            .as_deref()
            .and_then(|t| self.router.route_token(t))
            .map_or("-".to_string(), |(ctx, _)| ctx.tenant_id().to_string());
        let _trace_ctx = crate::telemetry::enter(trace, &tenant);
        let span = crate::telemetry::Span::begin("rest_request")
            .field("path", url.split('?').next().unwrap_or(&url).to_string())
            .field("method", method.clone())
            .field("tenant", tenant);

        // Method policy is an edge concern: reject a wrong verb before body
        // parsing, authentication, database work, or a stateful GET can run.
        match crate::http_contract::gate(&method, &url) {
            crate::http_contract::MethodGate::Dispatch
            | crate::http_contract::MethodGate::UnknownRoute => {}
            crate::http_contract::MethodGate::Options(policy) => {
                let response = tiny_http::Response::empty(204)
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Allow"[..],
                            policy.allow_header().as_bytes(),
                        )
                        .unwrap(),
                    )
                    .with_header(no_store_header());
                let _ = req.respond(response);
                span.field("status", 204).ok();
                return;
            }
            crate::http_contract::MethodGate::MethodNotAllowed(policy) => {
                let payload = serde_json::json!({
                    "error": {
                        "kind": "method-not-allowed",
                        "method": method,
                        "path": url.split('?').next().unwrap_or(&url),
                        "allowed_methods": policy.allowed(),
                    }
                });
                let response = tiny_http::Response::from_string(payload.to_string())
                    .with_status_code(405)
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/json"[..],
                        )
                        .unwrap(),
                    )
                    .with_header(
                        tiny_http::Header::from_bytes(
                            &b"Allow"[..],
                            policy.allow_header().as_bytes(),
                        )
                        .unwrap(),
                    )
                    .with_header(no_store_header());
                let _ = req.respond(response);
                let e = BrokerError::InvalidValue {
                    detail: format!(
                        "FRUST:E_METHOD_NOT_ALLOWED: {method} is not allowed for {url}"
                    ),
                };
                span.field("status", 405).err(&e);
                return;
            }
        }

        // **A body that is not UTF-8 says so.** `read_to_string` fails on
        // invalid UTF-8 and leaves the buffer EMPTY, and the discarded error
        // used to turn that into "missing field `manifest_version`" — an
        // operator reading that goes looking at their manifest's shape, which
        // is not the problem. Name the actual fault instead.
        let mut body = String::new();
        if std::io::Read::read_to_string(req.as_reader(), &mut body).is_err() {
            let e = BrokerError::InvalidValue {
                detail: "request body is not valid UTF-8".into(),
            };
            let response = tiny_http::Response::from_string(
                serde_json::json!({ "error": e }).to_string(),
            )
            .with_status_code(400)
            .with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .unwrap(),
            )
            .with_header(no_store_header());
            let _ = req.respond(response);
            span.field("status", 400).err(&e);
            return;
        }

        // metrics: Prometheus text, no auth, no shell (REQ-6.4.2)
        if url.split('?').next() == Some("/metrics") {
            let text = crate::telemetry::render_prometheus();
            let response = tiny_http::Response::from_string(text)
                .with_status_code(200)
                .with_header(
                    tiny_http::Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"text/plain; version=0.0.4"[..],
                    )
                    .unwrap(),
                )
                .with_header(no_store_header());
            let _ = req.respond(response);
            span.field("status", 200).ok();
            return;
        }

        let result = self.route(&url, &body, token.as_deref(), &host);
        let (code, payload, err) = match result {
            Ok(v) => (200, v, None),
            Err(e) => (status_for(&e), serde_json::json!({ "error": e }), Some(e)),
        };
        let data = payload.to_string();
        let response = tiny_http::Response::from_string(data)
            .with_status_code(code)
            .with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
            )
            .with_header(no_store_header());
        let _ = req.respond(response);

        let span = span.field("status", code);
        match err {
            None => span.ok(),
            Some(e) => span.err(&e),
        }
    }

    fn route(
        &self,
        url: &str,
        body: &str,
        token: Option<&str>,
        host: &str,
    ) -> Result<serde_json::Value, BrokerError> {
        let path = url.split('?').next().unwrap_or(url);
        let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
        let json: serde_json::Value = if body.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(body).map_err(|e| BrokerError::InvalidValue { detail: format!("bad json body: {e}") })?
        };

        // `/health` is the only route with no tenant: it answers for the
        // process, not for anyone's data.
        match segs.as_slice() {
            ["health"] => return Ok(serde_json::json!({ "ok": true })),
            // liveness above answers for the PROCESS; readiness
            // answers for what actually booted. No auth and no tenant, same as
            // health — a readiness probe that needs a credential is a readiness
            // probe nobody wires up.
            ["ready"] => return Ok(crate::boot::readiness()),
            ["login"] => {
                let ctx = self.login_context(&json, host)?;
                return self.login(ctx, &json);
            }
            _ => {}
        }

        // **THE ROUTING STEP.** Split the token, find its tenant, and do it
        // BEFORE any database call — the caller cannot be resolved until the
        // kernel knows which database to look in.
        let (ctx, secret) = self
            .router
            .route_token(token.ok_or_else(unauthenticated)?)
            .ok_or_else(unauthenticated)?;
        let caller = &self.session_caller(ctx, &secret)?;
        self.dispatch(ctx, &segs, &json, caller, Some(&secret))
    }

    /// Which tenant is `/login` authenticating against?
    ///
    /// `/login` carries no token, so it must be told. Order: an explicit
    /// `tenant` field, then the request's subdomain, then — **only when there
    /// is exactly one tenant registered** — that one. The last step is a
    /// fallback that cannot be wrong: with two tenants it returns `None` and
    /// login refuses rather than picking one.
    ///
    /// A hint that IS present and does not resolve is refused outright. Never
    /// falling through to "the only tenant" on a bad hint is what stops a
    /// wrong subdomain from quietly logging someone into somewhere else.
    fn login_context(
        &self,
        json: &serde_json::Value,
        host: &str,
    ) -> Result<&TenantContext, BrokerError> {
        let hint = json
            .get("tenant")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| crate::router::subdomain_of(host));

        match hint {
            Some(name) => self.router.get(name).ok_or_else(unauthenticated),
            None => self.router.sole().ok_or_else(|| BrokerError::InvalidValue {
                detail: "login needs a tenant: this process serves several, so send a `tenant` \
                         field or log in on the tenant's own subdomain"
                    .into(),
            }),
        }
    }

    fn route_with_caller(
        &self,
        url: &str,
        json: &serde_json::Value,
        caller: &Caller,
    ) -> Result<serde_json::Value, BrokerError> {
        let path = url.split('?').next().unwrap_or(url);
        let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
        let ctx = self.router.sole().ok_or_else(|| BrokerError::InvalidValue {
            detail: "route_with_caller needs a single-tenant router".into(),
        })?;
        self.dispatch(ctx, &segs, json, caller, None)
    }

    fn dispatch(
        &self,
        ctx: &TenantContext,
        segs: &[&str],
        json: &serde_json::Value,
        caller: &Caller,
        token: Option<&str>,
    ) -> Result<serde_json::Value, BrokerError> {
        let json = json.clone();
        match segs {
            ["logout"] => {
                let t = surql::escape_str(token.unwrap_or_default());
                ctx.broker.db.sql_root(&format!("DELETE {SESSION_TABLE} WHERE token = '{t}';"))?;
                // a revoked token must not survive in cache for even
                // one request. Coarse on purpose — correctness over hit rate.
                crate::tenant_gen::invalidate_sessions(ctx.broker.db.tenant_id());
                Ok(serde_json::json!({ "ok": true }))
            }

            // Admin force-revoke. Manager-tier, and it revokes
            // EVERY session the user holds — revoking one while an attacker
            // holds three accomplishes nothing, so "all of them" is the only
            // safe default for a compromised account.
            //
            // Same two-step as logout, which is why there is no new machinery
            // to prove: delete the rows, then bump the generation so the
            // session cache drops immediately instead of serving the
            // revoked token for up to SESSION_TTL. The TTL stays as the
            // backstop for a direct-DB deletion that bypasses even this.
            ["revoke", user] => {
                require_manager(caller)?;
                ident(user)?;
                let u = surql::escape_str(user);
                let rows = ctx.broker.db.sql_root(&format!(
                    "DELETE {SESSION_TABLE} WHERE user = '{u}' RETURN BEFORE;"
                ))?;
                let revoked = rows.as_array().map_or(0, |a| a.len());
                crate::tenant_gen::invalidate_sessions(ctx.broker.db.tenant_id());
                crate::telemetry::emit(
                    crate::telemetry::Level::Info,
                    "session_revoked",
                    &[("user", serde_json::json!(user)), ("sessions", serde_json::json!(revoked))],
                );
                Ok(serde_json::json!({ "ok": true, "user": user, "revoked": revoked }))
            }

            ["meta"] => {
                let rows = ctx.broker.db.sql_root("SELECT * FROM doctype ORDER BY name;")?;
                Ok(serde_json::json!({ "doctypes": rows }))
            }

            ["meta", doctype] => {
                ident(doctype)?;
                let rows = ctx.broker.db.sql_root(&format!(
                    "SELECT * FROM doctype WHERE name = '{doctype}' LIMIT 1;"
                ))?;
                let rec = rows.as_array().and_then(|a| a.first()).cloned().ok_or_else(|| {
                    BrokerError::UnknownDoctype { name: (*doctype).to_string() }
                })?;
                Ok(serde_json::json!({ "doctype": rec }))
            }

            // metadata create + live schema sync â€” criterion 6's "no restart"
            ["doctype"] => {
                require_manager(caller)?;
                let Some(sync) = &ctx.sync else {
                    return Err(BrokerError::InvalidValue { detail: "doctype endpoint disabled (no sync engine)".into() });
                };
                let meta = json.get("meta").unwrap_or(&json);
                // validate the shape BEFORE storing: bad metadata must never land
                let dt: crate::sync::DocTypeDef = serde_json::from_value(meta.clone())
                    .map_err(|e| BrokerError::InvalidValue { detail: format!("bad doctype metadata: {e}") })?;
                ident(&dt.name)?;
                let content = surql::render_value(&infer_value(meta))?;
                ctx.broker.db.sql_root(&format!("CREATE doctype:{} CONTENT {content};", dt.name))?;
                crate::broker::invalidate_meta(ctx.broker.db.tenant_id()); // new doctype visible immediately
                let applied = sync.sync(&ctx.broker.db)?;
                Ok(serde_json::json!({
                    "created": dt.name,
                    "applied": applied.applied,
                    // if this sync left orphans, say so here too — the
                    // operator making the change is the right person to know.
                    "orphan_columns": applied.orphans,
                }))
            }

            // attach/replace/clear a DocType's Tier-2 client
            // script. Manager surface, like all metadata writes. No schema
            // sync: a script is DATA on the metadata record, not shape â€”
            // "scripts are live-mutable" is exactly this endpoint. The next
            // form load serves the new script; nothing restarts.
            ["doctype", doctype, "script"] => {
                require_manager(caller)?;
                ident(doctype)?;
                let exists = ctx.broker.db.sql_root(&format!(
                    "SELECT name FROM doctype WHERE name = '{doctype}' LIMIT 1;"
                ))?;
                if exists.as_array().map_or(true, |a| a.is_empty()) {
                    return Err(BrokerError::UnknownDoctype { name: (*doctype).to_string() });
                }
                let script = json.get("script").and_then(|s| s.as_str()).unwrap_or("");
                if script.trim().is_empty() {
                    // clearing restores the scriptless 2-request posture
                    ctx.broker.db.sql_root(&format!(
                        "UPDATE doctype SET client_script = NONE WHERE name = '{doctype}';"
                    ))?;
                    crate::broker::invalidate_meta(ctx.broker.db.tenant_id());
                    return Ok(serde_json::json!({ "doctype": doctype, "script": "cleared" }));
                }
                let content = surql::render_value(&Value::Text(script.to_string()))?;
                ctx.broker.db.sql_root(&format!(
                    "UPDATE doctype SET client_script = {content} WHERE name = '{doctype}';"
                ))?;
                // This route mutates script state on the
                // `doctype` record and did NOT bump — harmless today, because
                // nothing caches `client_script`, and a landmine tomorrow: the
                // script cache rides the SAME generation, so the
                // day `client_script` joins the cached metadata this omission
                // becomes a stale-script bug with no obvious cause. One line now
                // beats an archaeology session later.
                crate::broker::invalidate_meta(ctx.broker.db.tenant_id());
                Ok(serde_json::json!({ "doctype": doctype, "script": "set", "bytes": script.len() }))
            }

            // ── notification rules, manager surface ──
            //
            // The endpoint that makes "email is metadata, not code" true: a
            // notification is a record, so this route plus a generation bump is
            // the whole delivery mechanism — no recompile, no restart
            // (criterion 1). Deliberately validated BEFORE storing, like
            // `/doctype`: a rule with a typo'd event would otherwise store
            // cleanly and then never fire, which is indistinguishable from a
            // broken mail transport and sends an operator to the wrong place.
            ["notification"] => {
                require_manager(caller)?;
                let meta = json.get("notification").unwrap_or(&json);
                let rule: crate::mail::NotificationDef = serde_json::from_value(meta.clone())
                    .map_err(|e| BrokerError::InvalidValue {
                        detail: format!("bad notification metadata: {e}"),
                    })?;
                let problems = rule.validate();
                if !problems.is_empty() {
                    return Err(BrokerError::InvalidValue {
                        detail: format!("bad notification: {}", problems.join("; ")),
                    });
                }
                let content = surql::render_value(&infer_value(meta))?;
                ctx.broker.db.sql_root(&format!(
                    "CREATE {}:{} CONTENT {content};",
                    crate::mail::NOTIFICATION_TABLE, rule.name
                ))?;
                // the same generation the DocType cache rides on — the next
                // write to that doctype re-reads the rules and this one fires
                crate::broker::invalidate_meta(ctx.broker.db.tenant_id());
                Ok(serde_json::json!({ "created": rule.name, "doctype": rule.doctype, "event": rule.event }))
            }

            // **Reclaiming an orphan is an EXPLICIT act.**
            //
            // An orphan column belongs to no manifest, so no app update can
            // reclaim it — there is nothing to re-publish. It gets its own
            // named act instead, with the REQ-6.6.2 acknowledgment shape: the
            // refusal names the column AND how many rows still hold data in it,
            // because "drop a column" and "drop 4,000 values" are the same
            // action described at two very different levels of honesty.
            //
            // Deliberately NOT a boot flag. Drift is now an orphan, so boot
            // never faces an unacknowledged destructive plan — and an
            // `allow_destructive` startup switch would be the footgun
            // shape: a flag whose casual use is silent data loss.
            ["doctype", doctype, "reclaim"] => {
                require_manager(caller)?;
                ident(doctype)?;
                let column = json
                    .get("column")
                    .and_then(|c| c.as_str())
                    .ok_or_else(|| BrokerError::InvalidValue {
                        detail: "reclaim needs a 'column'".into(),
                    })?;
                ident(column)?;
                let acknowledge =
                    json.get("acknowledge").and_then(serde_json::Value::as_bool).unwrap_or(false);

                // refuse to reclaim something the metadata still declares —
                // that is not an orphan, it is a field, and dropping it belongs
                // to its app's update
                let declared = ctx.broker.db.sql_root(&format!(
                    "SELECT fields FROM doctype WHERE name = '{doctype}' LIMIT 1;"
                ))?;
                let is_declared = declared
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|r| r["fields"].as_array())
                    .is_some_and(|f| f.iter().any(|x| x["fieldname"] == column));
                if is_declared {
                    return Err(BrokerError::InvalidValue {
                        detail: format!(
                            "'{column}' is a DECLARED field of '{doctype}', not an orphan — \
                             remove it through the owning app's update, which is where its \
                             acknowledgement belongs"
                        ),
                    });
                }

                let with_data = ctx
                    .broker
                    .db
                    .sql_root(&format!(
                        "SELECT count() FROM {doctype} WHERE {column} != NONE GROUP ALL;"
                    ))
                    .ok()
                    .and_then(|v| {
                        v.as_array().and_then(|a| a.first()).and_then(|r| r["count"].as_u64())
                    })
                    .unwrap_or(0);

                if !acknowledge {
                    return Err(BrokerError::InvalidValue {
                        detail: format!(
                            "refused: reclaiming orphan column '{doctype}.{column}' DROPS IT AND \
                             ITS DATA — {with_data} row(s) still hold a value. Re-send with \
                             \"acknowledge\": true if that is what you mean."
                        ),
                    });
                }
                // through the migrator, not by hand: it drops the column AND
                // records the snapshot, so the next boot agrees the orphan is
                // gone instead of reporting a phantom
                let sync = ctx.sync.as_ref().ok_or_else(|| BrokerError::Db {
                    detail: "no schema sync engine — cannot reclaim".into(),
                })?;
                sync.reclaim(&ctx.broker.db, doctype, column)?;
                ctx.broker
                    .db
                    .sql_root(&format!("UPDATE {doctype} SET {column} = NONE;"))?;
                crate::broker::invalidate_meta(ctx.broker.db.tenant_id());
                // /metrics is the enumeration seam, so it has to stop naming a
                // column that no longer exists
                let remaining = sync.sync(&ctx.broker.db)?;
                crate::boot::publish_orphans(
                    ctx.broker.db.tenant_id(),
                    &remaining.orphans,
                    Some(&format!("{doctype}.{column}")),
                );
                crate::telemetry::emit(
                    crate::telemetry::Level::Info,
                    "orphan_reclaimed",
                    &[
                        ("doctype", serde_json::json!(doctype)),
                        ("column", serde_json::json!(column)),
                        ("rows_with_data", serde_json::json!(with_data)),
                    ],
                );
                Ok(serde_json::json!({
                    "doctype": doctype, "column": column,
                    "reclaimed": true, "rows_cleared": with_data
                }))
            }

            // The outbox, read-only, manager-only: "did it send?" answered
            // without a shell on the database. Delivery state is data.
            ["mail", "outbox"] => {
                require_manager(caller)?;
                let rows = ctx.broker.db.sql_root(&format!(
                    "SELECT notification, record, to, subject, status, attempts, last_error, \
                     enqueued_at, sent_at FROM {} ORDER BY enqueued_at DESC LIMIT 100;",
                    crate::mail::OUTBOX_TABLE
                ))?;
                Ok(serde_json::json!({ "outbox": rows }))
            }

            // ── workflow (REQ-4.1.2) ──
            //
            // Any authenticated caller: the workflow itself decides what this
            // role may do, which is the whole point of role-gated transitions.
            // `/workflow/…` answers what you MAY do; `/transition/…` does it.
            // Both read the same metadata, so the buttons a user is offered
            // and the actions the kernel will allow cannot drift apart.
            ["workflow", doctype, key] => {
                ident(doctype)?;
                ident(key)?;
                ctx.broker.workflow_actions(caller, doctype, key)
            }

            ["transition", doctype, key] => {
                ident(doctype)?;
                ident(key)?;
                let action = json
                    .get("action")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| BrokerError::InvalidValue {
                        detail: "transition needs an 'action'".into(),
                    })?;
                ctx.broker.db_transition(caller, &HookChain::default(), doctype, key, action)
            }

            // ── the app lifecycle, manager surface ──
            //
            // Registry writes live HERE rather than in `app.rs` by ruling:
            // `app.rs` stays query-free so `surql_monopoly` covers it, and
            // `rest.rs` is already the metadata-write surface. Every action
            // below is a write to a CHANGEFEED table, so the audit trail is a
            // property of the storage, not of remembering to log.
            ["app"] => {
                require_manager(caller)?;
                let rows = ctx.broker.db.sql_root(&format!(
                    "SELECT name, version, enabled, installed_at, updated_at FROM {} ORDER BY name;",
                    crate::meta::APP_TABLE
                ))?;
                Ok(serde_json::json!({ "apps": rows }))
            }

            // REQ-6.6.1 as UX: the preview an operator sees BEFORE installing.
            ["app", "plan"] => {
                require_manager(caller)?;
                let manifest = self.manifest_from(&json)?;
                let plan = manifest.plan(ctx.broker.db.target(), &ctx.broker.db)?;
                Ok(app_plan_json(&plan))
            }

            ["app", "install"] => {
                require_manager(caller)?;
                let manifest = self.manifest_from(&json)?;
                let acknowledge =
                    json.get("acknowledge").and_then(serde_json::Value::as_bool).unwrap_or(false);
                self.install_app(ctx, &manifest, acknowledge, false)
            }

            ["app", "update"] => {
                require_manager(caller)?;
                let manifest = self.manifest_from(&json)?;
                let acknowledge =
                    json.get("acknowledge").and_then(serde_json::Value::as_bool).unwrap_or(false);
                self.install_app(ctx, &manifest, acknowledge, true)
            }

            ["app", name, "disable"] => {
                require_manager(caller)?;
                ident(name)?;
                self.set_app_enabled(ctx, name, false)
            }

            ["app", name, "enable"] => {
                require_manager(caller)?;
                ident(name)?;
                self.set_app_enabled(ctx, name, true)
            }

            ["app", name, "uninstall"] => {
                require_manager(caller)?;
                ident(name)?;
                self.uninstall_app(ctx, name)
            }

            // A plugin-declared route, served.
            //
            // Reached by ANY authenticated caller â€” the route is the app's
            // surface, not the manager's â€” and everything below it runs under
            // that caller's own session. Placed after the lifecycle verbs, and
            // `enable`/`disable` are refused as route names at validation so
            // the ordering can never silently shadow a real route.
            ["app", app, path] => self.serve_app_route(ctx, caller, app, path, &json),

            // the audit trail exposes full document history â€” manager surface
            ["audit", doctype, key] => {
                require_manager(caller)?;
                ident(doctype)?;
                let record = format!("{doctype}:{key}");
                let feed = ctx.broker.db.sql_root(&format!(
                    "SHOW CHANGES FOR TABLE {doctype} SINCE 1 LIMIT 1000;"
                ))?;
                let all = feed.as_array().cloned().unwrap_or_default();
                let mine: Vec<&serde_json::Value> =
                    all.iter().filter(|e| e.to_string().contains(&record)).collect();
                Ok(serde_json::json!({ "record": record, "total": all.len(), "entries": mine }))
            }

            // Tier-2 staleness as data: cursor + pending count
            ["lag", rollup] => {
                ident(rollup)?;
                let doctypes = crate::sync::load_doctypes(&ctx.broker.db)?;
                let source = doctypes
                    .iter()
                    .find(|dt| dt.aggregates.iter().any(|a| a.kind == "worker" && a.rollup == *rollup))
                    .map(|dt| dt.name.clone())
                    .ok_or_else(|| BrokerError::UnknownDoctype { name: format!("worker rollup {rollup}") })?;
                let cur = ctx.broker.db.sql_root(&format!(
                    "SELECT vs, updated_at FROM {}:{rollup};",
                    crate::aggregates::CURSOR_TABLE
                ))?;
                let cur = cur.as_array().and_then(|a| a.first()).cloned().unwrap_or(serde_json::Value::Null);
                let vs = cur.get("vs").and_then(|v| v.as_u64()).unwrap_or(0);
                let feed = ctx.broker.db.sql_root(&format!(
                    "SHOW CHANGES FOR TABLE {source} SINCE {} LIMIT 1000;",
                    vs + 1
                ))?;
                let pending = feed.as_array().map(std::vec::Vec::len).unwrap_or(0);
                Ok(serde_json::json!({
                    "rollup": rollup, "source": source, "cursor": cur, "pending": pending
                }))
            }

            // ── kernel-owned live subscriptions ──
            ["subscribe", doctype] => {
                ident(doctype)?;
                let Some(rt) = &self.realtime else {
                    return Err(BrokerError::InvalidValue { detail: "realtime disabled".into() });
                };
                // the subscription runs under the SUBSCRIBER'S OWN jwt
                let jwt = self.session_jwt(ctx, token)?;
                let sub = rt.subscribe(&caller.user, &jwt, ctx.broker.db.target(), doctype)?;
                Ok(serde_json::json!({ "sub": sub, "budget": crate::realtime::LIVE_SUB_BUDGET_PER_TABLE }))
            }

            ["events", sub] => {
                let Some(rt) = &self.realtime else {
                    return Err(BrokerError::InvalidValue { detail: "realtime disabled".into() });
                };
                let (events, alive) = rt.drain(sub, &caller.user)?;
                Ok(serde_json::json!({ "events": events, "alive": alive }))
            }

            ["unsubscribe", sub] => {
                let Some(rt) = &self.realtime else {
                    return Err(BrokerError::InvalidValue { detail: "realtime disabled".into() });
                };
                rt.unsubscribe(sub, &caller.user)?;
                Ok(serde_json::json!({ "ok": true }))
            }

            ["read", doctype] => {
                let filter = parse_filter(json.get("filter"))?;
                let fields = parse_fields(json.get("fields"))?;
                let opts = parse_opts(&json)?;
                let rows = ctx.broker.db_read(caller, doctype, filter.as_ref(), &fields, &opts)?;
                Ok(serde_json::json!({ "rows": rows }))
            }

            ["write", doctype] => {
                // **An unknown key is refused, not discarded.**
                // `op` was accepted and silently dropped, so a client that sent
                // `{"op":"create", "record": …}` believing it forced a create
                // got an update and no complaint. On the WRITE path that is the
                // silent-wrong class: the caller's stated intent and what
                // happened disagreed, and nothing said so.
                known_keys(&json, &["doc", "record"])?;
                let doc = parse_doc(json.get("doc"))?;
                let record = json.get("record").and_then(|r| r.as_str());
                let op = if record.is_some() { WriteOp::Update } else { WriteOp::Create };
                let out = ctx.broker.db_write(caller, &HookChain::default(), op, doctype, record, &doc)?;
                // **G3, additive.** `created` has always carried the ROW, so
                // turning it into a boolean would break every reader; instead
                // `action` says which happened and `record` gives the id
                // without digging. `created` is unchanged and deprecated in the
                // docs — the additive shape the evolution policy requires.
                let id = out.get("id").cloned().unwrap_or(serde_json::Value::Null);
                Ok(serde_json::json!({
                    "created": out,
                    "action": if op == WriteOp::Create { "created" } else { "updated" },
                    "record": id,
                }))
            }

            ["aggregate", doctype] => {
                let filter = parse_filter(json.get("filter"))?;
                let group_by = parse_paths(json.get("group_by"))?;
                let metrics = parse_metrics(json.get("metrics"))?;
                let rows = ctx.broker.db_aggregate(caller, doctype, filter.as_ref(), &group_by, &metrics)?;
                Ok(serde_json::json!({ "rows": rows }))
            }

            ["enqueue", kind] => {
                let payload = parse_doc(json.get("payload"))?;
                let id = ctx.broker.enqueue(caller, kind, &payload)?;
                Ok(serde_json::json!({ "job": id }))
            }

            _ => Err(BrokerError::InvalidValue { detail: format!("no route: /{}", segs.join("/")) }),
        }
    }

    /// POST /login: prove credentials against the record access (the DB is
    /// the authority), derive the role from the app_user RECORD, mint an
    /// opaque token, persist the session (kernel restarts re-hydrate the
    /// JWT cache from it).
    fn login(
        &self,
        ctx: &TenantContext,
        json: &serde_json::Value,
    ) -> Result<serde_json::Value, BrokerError> {
        let user = json.get("user").and_then(|v| v.as_str()).unwrap_or("");
        let pass = json.get("pass").and_then(|v| v.as_str()).unwrap_or("");
        if user.is_empty() {
            return Err(BrokerError::InvalidValue { detail: "login needs user + pass".into() });
        }
        let db = &ctx.broker.db;
        let jwt = db.signin(user, pass)?; // bad creds -> PermissionDenied

        let esc = surql::escape_str(user);
        let rec = db.sql_root(&format!("SELECT role, status FROM app_user WHERE name = '{esc}' LIMIT 1;"))?;
        let rec = rec.as_array().and_then(|a| a.first()).cloned().unwrap_or(serde_json::Value::Null);
        if rec.get("status").and_then(|s| s.as_str()).is_some_and(|s| s != "active") {
            return Err(BrokerError::PermissionDenied { detail: "account disabled".into() });
        }
        let role = rec.get("role").and_then(|r| r.as_str()).unwrap_or("guest").to_string();

        let secret = db.sql_root("RETURN rand::string(64);")?;
        let secret = secret.as_str().unwrap_or_default().to_string();
        if secret.is_empty() || !secret.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(BrokerError::Db { detail: "token mint failed".into() });
        }
        // The ROW stores only the secret: it already lives in this tenant's
        // own database, so the prefix inside it would be redundant — and a
        // lookup that had to match the prefix too would be checking the
        // client's claim against itself.
        // ONE statement, no DDL. The table comes from `meta_ddl` at
        // boot; mixing a `DEFINE` into this batch is what made a conflict retry
        // duplicate the row.
        db.sql_root(&format!(
            "CREATE {SESSION_TABLE} SET token = '{secret}', user = '{esc}', role = '{}', \
             jwt = '{}', created_at = time::now();",
            surql::escape_str(&role),
            surql::escape_str(&jwt)
        ))?;

        // **The wire token carries its tenant.** The prefix is the
        // canonical `TenantId` this kernel resolved and authenticated against
        // — asserted by the kernel after validation, never the client's
        // string handed back. A client presenting some other tenant's prefix
        // gets a lookup in THAT tenant's database, where its secret is not,
        // so spoofing fails closed with no cross-tenant read.
        let token = TenantRouter::mint_token(ctx.tenant_id(), &secret);
        Ok(serde_json::json!({ "token": token, "user": user, "role": role, "tenant": ctx.tenant_id() }))
    }

    /// The session's stored SurrealDB JWT â€” the credential a live
    /// subscription authenticates with (the subscriber's own authority).
    fn session_jwt(&self, ctx: &TenantContext, token: Option<&str>) -> Result<String, BrokerError> {
        let Some(t) = token else { return Err(unauthenticated()) };
        if t.is_empty() || !t.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(unauthenticated());
        }
        let v = ctx
            .broker
            .db
            .sql_root(&format!("SELECT VALUE jwt FROM {SESSION_TABLE} WHERE token = '{t}' LIMIT 1;"))?;
        v.as_array()
            .and_then(|a| a.first())
            .and_then(|j| j.as_str())
            .map(str::to_string)
            .ok_or_else(unauthenticated)
    }

    /// Resolve a bearer token to the acting principal. Both permission
    /// halves flow from here: the role (field envelope) was read from the DB
    /// at login; the row half is the DB itself, via the user's own JWT.
    /// `t` is the token's **secret half** — the router has already split off
    /// and validated the tenant prefix, and `ctx` is the tenant it named.
    fn session_caller(&self, ctx: &TenantContext, t: &str) -> Result<Caller, BrokerError> {
        if t.is_empty() || !t.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(unauthenticated());
        }
        let db = &ctx.broker.db;
        let tenant = db.tenant_id().to_string();

        // The cached path. Valid only while the generation matches
        // (logout bumps it) AND the entry is inside its TTL. The JWT is
        // re-seeded on every hit because `Db`'s token cache is what actually
        // authorises the row half — a hit must leave the caller exactly as a
        // miss would, or the cache would change behaviour rather than just
        // timing.
        //
        // the generation is THIS TENANT'S, read from the
        // handle the broker resolved at construction — one atomic load, and
        // another tenant's logout cannot move it.
        let gen = ctx.broker.gens.session.load(std::sync::atomic::Ordering::Acquire);
        let key = (tenant, t.to_string());
        if let Some((cached_gen, at, caller, jwt)) = session_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
        {
            if *cached_gen == gen && at.elapsed() < SESSION_TTL {
                db.seed_token(&caller.user, jwt);
                return Ok(caller.clone());
            }
        }

        // raw: a missing session table is "no session", not a 500
        let stmts = db.sql_root_raw(&format!(
            "SELECT user, role, jwt FROM {SESSION_TABLE} WHERE token = '{t}' LIMIT 1;"
        ))?;
        let row = stmts
            .first()
            .filter(|s| s.get("status").and_then(|x| x.as_str()) == Some("OK"))
            .and_then(|s| s.get("result"))
            .and_then(|r| r.as_array())
            .and_then(|a| a.first())
            .cloned()
            .ok_or_else(unauthenticated)?;
        let user = row.get("user").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let role = row.get("role").and_then(|v| v.as_str()).unwrap_or("guest").to_string();
        let jwt = row.get("jwt").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        if !jwt.is_empty() {
            db.seed_token(&user, &jwt);
        }
        // pass stays empty: the seeded JWT serves the record session
        let caller = Caller { user, pass: String::new(), role };
        session_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, (gen, std::time::Instant::now(), caller.clone(), jwt));
        Ok(caller)
    }
}

fn unauthenticated() -> BrokerError {
    BrokerError::PermissionDenied { detail: "E_UNAUTHENTICATED".into() }
}

/// The plan, as an operator sees it. Destructive statements are surfaced at
/// the TOP LEVEL rather than buried in the schema half: the whole point of a
/// preview is that the dangerous part is the part you cannot miss.
fn app_plan_json(plan: &crate::app::InstallPlan) -> serde_json::Value {
    let destructive = plan.destructive();
    serde_json::json!({
        "app": plan.app,
        "version": plan.version,
        "destructive": destructive,
        "needs_acknowledgement": !destructive.is_empty(),
        "planned": plan.schema.planned.iter().map(|p| serde_json::json!({
            "resource": p.name,
            "statements": p.statements,
            "destructive": p.destructive,
        })).collect::<Vec<_>>(),
        "applied": plan.schema.applied.len(),
        "errors": plan.schema.errors.iter().map(|e| format!("{e:?}")).collect::<Vec<_>>(),
        "client_scripts": plan.client_scripts,
        "server_scripts": plan.server_scripts,
        "routes": plan.routes,
        "workflows": plan.workflows,
    })
}

impl Rest {
    /// Dispatch with an explicit caller, bypassing the session store.
    ///
    /// A test seam, and deliberately a narrow one: it skips only the
    /// tokenâ†’Caller lookup, so every route below it â€” including
    /// `require_manager` â€” runs exactly as it does for a real request. That is
    /// what lets `the_app_surface_is_manager_only` be a real proof rather than
    /// a test of a test.
    pub fn route_for_test(
        &self,
        path: &str,
        body: &serde_json::Value,
        caller: &Caller,
    ) -> Result<serde_json::Value, BrokerError> {
        self.route_with_caller(path, body, caller)
    }

    fn manifest_from(&self, json: &serde_json::Value) -> Result<crate::app::Manifest, BrokerError> {
        let raw = json.get("manifest").unwrap_or(json);
        let text = match raw {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        crate::app::Manifest::parse(&text)
    }

    /// Serve `/app/{app}/{path}` through the shipped dispatch path.
    ///
    /// Order matters and is the criterion: **throttle â†’ enabled â†’ resolve â†’
    /// run**. The throttle is admitted HERE, at dispatch, in addition to the
    /// broker door the handler's own verbs pass through. Two reasons:
    ///
    /// 1. Running a wasm handler is real compute even if it never reads. A
    ///    route that did no db work would otherwise be free, and "free" is how
    ///    a budget gets bypassed.
    /// 2. The guest picks its own status code. Left to the handler, a
    ///    throttled call would surface as whatever the plugin felt like â€”
    ///    `E_TENANT_THROTTLED` has to be the KERNEL's answer (429), not a
    ///    plugin's opinion about one.
    fn serve_app_route(
        &self,
        ctx: &TenantContext,
        caller: &Caller,
        app: &str,
        path: &str,
        json: &serde_json::Value,
    ) -> Result<serde_json::Value, BrokerError> {
        ident(app)?;
        ident(path)?;
        let _tenant = crate::telemetry::enter_tenant(ctx.broker.db.tenant_id());
        // a plugin route is not a budget bypass
        crate::fairness::admit_verb(ctx.broker.db.tenant_id())?;

        let Some(row) = self.installed(ctx, app)? else {
            return Err(BrokerError::UnknownDoctype { name: format!("app {app}") });
        };
        // PM ruling: a disabled app **does not exist as a surface**. It answers
        // the same 404 as an unknown route, and is deliberately
        // INDISTINGUISHABLE from one â€” a 403 would confirm to an outsider that
        // the route is real-but-forbidden, which is the leak. This is the HTTP
        // echo of "metadata detached".
        //
        // The reason is not lost, only moved: an operator wondering why their
        // route 404s finds `route_refused_app_disabled` in the log. Information
        // the server keeps and the response withholds.
        if !row.get("enabled").and_then(serde_json::Value::as_bool).unwrap_or(false) {
            crate::telemetry::emit(
                crate::telemetry::Level::Info,
                "route_refused_app_disabled",
                &[("app", serde_json::json!(app)), ("path", serde_json::json!(path))],
            );
            return Err(BrokerError::UnknownDoctype {
                name: format!("route /app/{app}/{path}"),
            });
        }
        let manifest =
            crate::app::Manifest::parse(row.get("manifest").and_then(|m| m.as_str()).unwrap_or("{}"))?;
        let Some(decl) = manifest.routes.iter().find(|r| r.path == path) else {
            return Err(BrokerError::UnknownDoctype { name: format!("route /app/{app}/{path}") });
        };

        let host = crate::routes::host_for(&decl.component, Arc::clone(&ctx.broker))?;
        let query = json.get("query").and_then(|q| q.as_str()).unwrap_or("");
        let body = json.get("body").and_then(|b| b.as_str()).unwrap_or("");
        let (status, out) = host.handle(app, caller, path, query, body)?;
        Ok(serde_json::json!({ "app": app, "path": path, "status": status, "body": out }))
    }

    /// **Uninstall, stated honestly.** (REQ-6.6.3.)
    ///
    /// Metadata detaches. Data remains. The manifest that described the
    /// metadata is itself removed â€” which makes uninstall **the one operation
    /// that genuinely forgets something**, and the reason it is not reversible
    /// the way disable is.
    ///
    /// What it removes: the DocType metadata records the app owns, its client
    /// scripts (they live on those records), its route declarations (they live
    /// in the manifest), and the registry row.
    ///
    /// What it does NOT remove: **the tables and every row in them.** An app's
    /// data outlives the app, deliberately. REQ-6.6.3 draws the line â€” schema
    /// revert is not data recovery â€” and the same line holds here: this is not
    /// a delete button wearing a lifecycle costume. Dropping tables is a
    /// separate, destructive, explicitly-acknowledged act, and Frappe's
    /// `bench remove-app` taking data with it is precisely the horror story
    /// this answers.
    ///
    /// The returned payload SAYS all of this, per-table, so an operator learns
    /// what survived at the moment they act rather than from documentation
    /// they did not read.
    fn uninstall_app(&self, ctx: &TenantContext, name: &str) -> Result<serde_json::Value, BrokerError> {
        let Some(row) = self.installed(ctx, name)? else {
            return Err(BrokerError::UnknownDoctype { name: format!("app {name}") });
        };
        let manifest =
            crate::app::Manifest::parse(row.get("manifest").and_then(|m| m.as_str()).unwrap_or("{}"))?;

        let mut retained = Vec::new();
        for dt in &manifest.doctypes {
            ident(&dt.name)?;
            let n = surql::escape_str(&dt.name);
            // count what we are LEAVING BEHIND, so the answer is specific
            let rows = ctx
                .broker
                .db
                .sql_root(&format!("SELECT count() AS n FROM {n} GROUP ALL;"))
                .ok()
                .and_then(|v| v.as_array().and_then(|a| a.first()).and_then(|r| r["n"].as_u64()))
                .unwrap_or(0);
            retained.push(serde_json::json!({ "table": dt.name, "rows_retained": rows }));
            // metadata goes; the table does not
            ctx.broker.db.sql_root(&format!("DELETE doctype WHERE name = '{n}';"))?;
            crate::broker::invalidate_meta(ctx.broker.db.tenant_id()); // uninstall detaches metadata
        }

        // the app's workflows are its metadata too
        for w in &manifest.workflows {
            ident(&w.name)?;
            let wn = surql::escape_str(&w.name);
            ctx.broker.db.sql_root(&format!("DELETE workflow WHERE name = '{wn}';"))?;
        }

        // And its contributions to OTHER apps' DocTypes. Those rows
        // are not this app's to delete, so the fields and chain link it added
        // are removed surgically and the owner's DocType is left standing.
        self.detach_extensions(ctx, name)?;

        let n = surql::escape_str(name);
        ctx.broker
            .db
            .sql_root(&format!("DELETE {} WHERE name = '{n}';", crate::meta::APP_TABLE))?;

        Ok(serde_json::json!({
            "app": name,
            "action": "uninstalled",
            "metadata_removed": manifest.doctypes.iter().map(|d| &d.name).collect::<Vec<_>>(),
            "routes_removed": manifest.routes.len(),
            "data_retained": retained,
            "note": "Metadata detached and the manifest removed. Tables and rows remain â€” \
                     dropping them is a separate, explicitly acknowledged act.",
        }))
    }

    /// Writes the bundle's workflow records. A workflow is metadata,
    /// like a DocType, so it installs the same way and takes effect on the
    /// next transition — no restart.
    fn attach_workflows(&self, ctx: &TenantContext, manifest: &crate::app::Manifest) -> Result<(), BrokerError> {
        for w in &manifest.workflows {
            ident(&w.name)?;
            ident(&w.doctype)?;
            let content = surql::render_value(&infer_value(
                &serde_json::to_value(w).unwrap_or_default(),
            ))?;
            let n = surql::escape_str(&w.name);
            ctx.broker.db.sql_root(&format!("UPSERT workflow:{n} CONTENT {content};"))?;
        }
        Ok(())
    }

    fn installed(&self, ctx: &TenantContext, name: &str) -> Result<Option<serde_json::Value>, BrokerError> {
        let n = surql::escape_str(name);
        let rows = ctx.broker.db.sql_root(&format!(
            "SELECT * FROM {} WHERE name = '{n}' LIMIT 1;",
            crate::meta::APP_TABLE
        ))?;
        Ok(rows.as_array().and_then(|a| a.first()).cloned())
    }

    /// Install or update: **validate â†’ plan â†’ gate â†’ apply â†’ record**, in that
    /// order, and no step runs if an earlier one refused.
    ///
    /// The gate is REQ-6.6.2 doing bundle duty: a destructive change is refused
    /// without an explicit acknowledgement, and the refusal *names what it
    /// would have destroyed* â€” a gate that only says "no" teaches operators to
    /// pass the flag reflexively.
    fn install_app(
        &self,
        ctx: &TenantContext,
        manifest: &crate::app::Manifest,
        acknowledge: bool,
        is_update: bool,
    ) -> Result<serde_json::Value, BrokerError> {
        let errs = manifest.validate();
        if !errs.is_empty() {
            return Err(BrokerError::InvalidValue {
                detail: format!("bundle is not installable:\n- {}", errs.join("\n- ")),
            });
        }

        let existing = self.installed(ctx, &manifest.name)?;
        match (&existing, is_update) {
            (Some(row), false) => {
                let v = row.get("version").and_then(|v| v.as_str()).unwrap_or("?");
                return Err(BrokerError::InvalidValue {
                    detail: format!(
                        "app '{}' is already installed at {v}; use update",
                        manifest.name
                    ),
                });
            }
            (None, true) => {
                return Err(BrokerError::UnknownDoctype {
                    name: format!("app {}", manifest.name),
                });
            }
            (Some(row), true) => {
                let from = row.get("version").and_then(|v| v.as_str()).unwrap_or("0.0.0");
                match crate::app::version_gt(&manifest.version, from) {
                    Some(true) => {}
                    Some(false) => {
                        return Err(BrokerError::InvalidValue {
                            detail: format!(
                                "update to {} does not advance the installed version {from}",
                                manifest.version
                            ),
                        })
                    }
                    None => {
                        return Err(BrokerError::InvalidValue {
                            detail: format!("version '{from}' or '{}' is malformed", manifest.version),
                        })
                    }
                }
            }
            (None, false) => {}
        }

        // PLAN FIRST â€” the same dry run the operator was shown.
        let plan = manifest.plan(ctx.broker.db.target(), &ctx.broker.db)?;
        let destructive = plan.destructive();
        if !destructive.is_empty() && !acknowledge {
            return Err(BrokerError::InvalidValue {
                detail: format!(
                    "refused: this bundle performs destructive changes and was not acknowledged:\n- {}",
                    destructive.join("\n- ")
                ),
            });
        }

        // APPLY â€” same call shape, different options.
        // ---- OWNER EVOLUTION ----
        //
        // This is the hard question, and the one that decides whether this beats
        // Frappe or merely adds a customs post: install-time gating catches the
        // install and says nothing about the owner's NEXT release. An extension
        // reads `Y.z`; owner v2 removes it; both apps' CI is green; the
        // extension dies quietly. A silently-disabled extension is P-2.2 reborn,
        // so the refusal names the casualty and its app.
        //
        // This seam already exists with no new storage: every
        // app's manifest is on record verbatim. So it is one call, not a
        // subsystem.
        let casualties = self.extension_casualties(ctx, manifest, &destructive)?;
        if !casualties.is_empty() && !acknowledge {
            return Err(BrokerError::InvalidValue {
                detail: format!(
                    "FRUST:E_BREAKS_EXTENSION: refused - this update breaks what other apps \
                     declared against it:\n- {}\n\
                     Acknowledge to proceed (their extensions stop working), or ship a version \
                     that keeps the surface.",
                    casualties.join("\n- ")
                ),
            });
        }

        let mut opts = frust_orm::MigrationOptions::default_for_dev();
        opts.allow_destructive = acknowledge;
        opts.acknowledge = acknowledge;
        let applied = manifest.plan_unchecked(ctx.broker.db.target(), &ctx.broker.db, opts)?;
        if !applied.schema.errors.is_empty() {
            return Err(BrokerError::Db {
                detail: format!("bundle apply errors: {:?}", applied.schema.errors),
            });
        }

        self.attach_metadata(ctx, manifest)?;
        self.attach_workflows(ctx, manifest)?;

        let content = surql::render_value(&Value::Text(
            serde_json::to_string(manifest).unwrap_or_default(),
        ))?;
        let name = surql::escape_str(&manifest.name);
        let version = surql::escape_str(&manifest.version);
        if existing.is_some() {
            ctx.broker.db.sql_root(&format!(
                "UPDATE {} SET version = '{version}', manifest = {content}, enabled = true, \
                 updated_at = time::now() WHERE name = '{name}';",
                crate::meta::APP_TABLE
            ))?;
        } else {
            ctx.broker.db.sql_root(&format!(
                "CREATE {}:{name} SET name = '{name}', version = '{version}', enabled = true, \
                 manifest = {content}, installed_at = time::now();",
                crate::meta::APP_TABLE
            ))?;
        }

        Ok(serde_json::json!({
            "app": manifest.name,
            "version": manifest.version,
            "action": if is_update { "updated" } else { "installed" },
            "applied": applied.schema.applied.len(),
            "destructive": destructive,
        }))
    }

    /// Writes the bundle's DocType records and attaches its client scripts.
    /// Schema already exists by now; this is the metadata half.
    fn attach_metadata(&self, ctx: &TenantContext, manifest: &crate::app::Manifest) -> Result<(), BrokerError> {
        for dt in &manifest.doctypes {
            ident(&dt.name)?;
            let mut value = serde_json::to_value(dt).unwrap_or_default();
            value["app"] = serde_json::json!(manifest.name);
            if let Some(s) = manifest.client_scripts.iter().find(|s| s.doctype == dt.name) {
                value["client_script"] = serde_json::json!(s.script);
            }
            // criterion 6: the server script lives on the DocType record, which
            // is where hook dispatch reads it from per call
            // the LIST shape: the owner's entry carries its own app name
            // so the dispatcher can put it first and keep it there.
            // every declared script, each carrying its hook class.
            // `find` used to take the FIRST match and drop the rest, which was
            // invisible while only one class existed and would have silently
            // eaten a second subscription the moment one could be written.
            let mine: Vec<serde_json::Value> = manifest
                .server_scripts
                .iter()
                .filter(|s| s.doctype == dt.name)
                .map(|s| serde_json::json!({
                    "app": manifest.name, "script": s.script, "hook": s.hook
                }))
                .collect();
            if !mine.is_empty() {
                value["server_script"] = serde_json::json!(mine);
            }
            let n = surql::escape_str(&dt.name);

            // **Preserve other apps' contributions across an owner
            // update.** This is a whole-record `CONTENT` upsert built from the
            // OWNER'S manifest, which knows nothing about extensions — so
            // without this, publishing owner v2 would silently delete every
            // extension's fields and every extension's link in the validate
            // chain. Silent loss of another app's declared surface is the
            // P-2.2 shape again, one path over.
            let prior = ctx.broker.db.sql_root(&format!(
                "SELECT fields, server_script FROM doctype WHERE name = '{n}' LIMIT 1;"
            ))?;
            let prior = prior.as_array().and_then(|a| a.first()).cloned().unwrap_or_default();
            let ext_fields: Vec<serde_json::Value> = prior["fields"]
                .as_array()
                .map(|a| a.iter().filter(|f| !f["ext_app"].is_null()).cloned().collect())
                .unwrap_or_default();
            let ext_scripts: Vec<serde_json::Value> = prior["server_script"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|s| {
                            s["app"].as_str().is_some_and(|app| app != manifest.name)
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();

            let content = surql::render_value(&infer_value(&value))?;
            ctx.broker
                .db
                .sql_root(&format!("UPSERT doctype:{n} CONTENT {content};"))?;

            // put the extensions back, behind the owner's own entry
            for f in &ext_fields {
                let fv = surql::render_value(&infer_value(f))?;
                ctx.broker.db.sql_root(&format!("UPDATE doctype:{n} SET fields += {fv};"))?;
            }
            for s in &ext_scripts {
                let sv = surql::render_value(&infer_value(s))?;
                ctx.broker
                    .db
                    .sql_root(&format!("UPDATE doctype:{n} SET server_script += {sv};"))?;
            }
            crate::broker::invalidate_meta(ctx.broker.db.tenant_id()); // app install/update changed metadata
        }
        self.attach_extensions(ctx, manifest)?;
        Ok(())
    }

    /// **Apply this app's extensions to DocTypes it does not own.**
    ///
    /// Additive by construction — fields are appended and the script joins the
    /// chain *behind* the owner's. Nothing here can remove or reorder what the
    /// owner declared; owner-first is enforced independently in the dispatcher,
    /// so this path could not displace an invariant even if it tried.
    ///
    /// Re-applied idempotently on update: this app's previous entries are
    /// stripped first, so an update revises rather than duplicates.
    fn attach_extensions(
        &self,
        ctx: &TenantContext,
        manifest: &crate::app::Manifest,
    ) -> Result<(), BrokerError> {
        let me = surql::escape_str(&manifest.name);
        for e in &manifest.extends {
            ident(&e.doctype)?;
            let target = surql::escape_str(&e.doctype);
            let rows = ctx.broker.db.sql_root(&format!(
                "SELECT name, app, fields FROM doctype WHERE name = '{target}' LIMIT 1;"
            ))?;
            let Some(row) = rows.as_array().and_then(|a| a.first()).cloned() else {
                return Err(BrokerError::InvalidValue {
                    detail: format!(
                        "app '{}' extends '{}', which does not exist — install the owner first",
                        manifest.name, e.doctype
                    ),
                });
            };

            // ── refuse-ambiguity: a collision names BOTH apps ──
            let existing: Vec<(String, Option<String>)> = row["fields"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|f| {
                            Some((
                                f.get("fieldname")?.as_str()?.to_string(),
                                f.get("ext_app").and_then(|x| x.as_str()).map(str::to_string),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            for f in &e.fields {
                if let Some((_, held_by)) = existing.iter().find(|(n, _)| n == &f.fieldname) {
                    let holder = held_by
                        .clone()
                        .unwrap_or_else(|| row["app"].as_str().unwrap_or("the owner").to_string());
                    if holder != manifest.name {
                        return Err(BrokerError::InvalidValue {
                            detail: format!(
                                "FRUST:E_EXTENSION_CONFLICT: app '{}' cannot add field '{}' to \
                                 '{}' — '{}' already declares it. Two apps may not own one field; \
                                 rename yours (it is namespaced under your app anyway).",
                                manifest.name, f.fieldname, e.doctype, holder
                            ),
                        });
                    }
                }
            }

            // strip this app's previous contribution, then re-add (update-safe)
            ctx.broker.db.sql_root(&format!(
                "UPDATE doctype:{target} SET \
                   fields = (fields ?? []).filter(|$f| $f.ext_app != '{me}'), \
                   server_script = (server_script ?? []).filter(|$s| $s.app != '{me}');"
            ))?;

            for f in &e.fields {
                let mut fv = serde_json::to_value(f).unwrap_or_default();
                // Stamp the contributing app ONTO the field. This is what makes
                // uninstall able to remove exactly its own additions and nothing
                // else — without it, "which of these fields were yours" is
                // archaeology.
                fv["ext_app"] = serde_json::json!(manifest.name);
                let content = surql::render_value(&infer_value(&fv))?;
                ctx.broker
                    .db
                    .sql_root(&format!("UPDATE doctype:{target} SET fields += {content};"))?;
            }
            if !e.script.trim().is_empty() {
                let sv = surql::render_value(&infer_value(&serde_json::json!({
                    "app": manifest.name, "script": e.script, "hook": e.hook
                })))?;
                ctx.broker
                    .db
                    .sql_root(&format!("UPDATE doctype:{target} SET server_script += {sv};"))?;
            }
            crate::broker::invalidate_meta(ctx.broker.db.tenant_id());
        }

        // **The extension's fields must become real columns.** The install path
        // syncs schema *before* metadata is attached, so extension fields added
        // here would otherwise exist in the DocType record and nowhere in the
        // database — and the first write naming one fails with SurrealDB's
        // "no such field exists for table", which reads like a bug in the
        // extension rather than a missing step in the installer.
        if !manifest.extends.is_empty() {
            if let Some(sync) = &ctx.sync {
                sync.sync(&ctx.broker.db)?;
            }
        }
        Ok(())
    }

    /// **Which other apps' extensions would this update break?**
    ///
    /// The whole mechanism, wired: every installed app's manifest
    /// is already stored verbatim in the registry and already re-parsed live for
    /// route dispatch, so the declared dependency surface of every *other* app
    /// is readable at plan time with no new storage and no new bookkeeping.
    ///
    /// Cross-references those surfaces against this bundle's own destructive
    /// list (the migration engine's `REMOVE FIELD memo` strings that the install
    /// refusal already names) and returns a casualty per broken declaration.
    /// Naming both the field and the app is the point: "your update breaks
    /// something" is not actionable, "it breaks `crm_note` on `sales_invoice`,
    /// declared by app 'crm'" is.
    fn extension_casualties(
        &self,
        ctx: &TenantContext,
        updating: &crate::app::Manifest,
        destructive: &[String],
    ) -> Result<Vec<String>, BrokerError> {
        if destructive.is_empty() {
            return Ok(Vec::new());
        }
        let rows = ctx.broker.db.sql_root(&format!(
            "SELECT name, manifest FROM {} ORDER BY name;",
            crate::meta::APP_TABLE
        ))?;
        let mut out = Vec::new();
        for r in rows.as_array().cloned().unwrap_or_default() {
            let other = r["name"].as_str().unwrap_or_default().to_string();
            if other == updating.name {
                continue; // an app may break its own surface freely
            }
            let Ok(m) = crate::app::Manifest::parse(r["manifest"].as_str().unwrap_or("{}")) else {
                continue;
            };
            for e in &m.extends {
                // does this update touch a DocType that app extends?
                if !updating.doctypes.iter().any(|d| d.name == e.doctype) {
                    continue;
                }
                for f in &e.fields {
                    // the extension's own field must still be declarable — if
                    // the owner's new version drops the doctype or the
                    // migration removes a field the extension reads, say so
                    if destructive.iter().any(|d| {
                        let d = d.to_ascii_lowercase();
                        d.contains(&e.doctype.to_ascii_lowercase())
                            && d.contains(&f.fieldname.to_ascii_lowercase())
                    }) {
                        out.push(format!(
                            "extension of app '{}' on '{}' declares field '{}', which this update \
                             removes",
                            other, e.doctype, f.fieldname
                        ));
                    }
                }
                // a hook against a doctype this update drops entirely
                if !e.script.trim().is_empty()
                    && destructive.iter().any(|d| {
                        let d = d.to_ascii_lowercase();
                        d.contains("remove table") && d.contains(&e.doctype.to_ascii_lowercase())
                    })
                {
                    out.push(format!(
                        "extension of app '{other}' hooks '{}', which this update removes",
                        e.doctype
                    ));
                }
            }
        }
        Ok(out)
    }

    /// **Honest uninstall, extended cross-app.**
    ///
    /// Removes exactly this app's contributions from DocTypes it extended — its
    /// namespaced fields and its link in the validate chain — and nothing else.
    /// The owner's DocType, the owner's script and every row of data survive,
    /// which is the honest-uninstall promise applied to a second
    /// contributor. `WHERE app != me` keeps it off the app's own doctypes,
    /// which the ordinary uninstall path already handles.
    fn detach_extensions(&self, ctx: &TenantContext, app: &str) -> Result<(), BrokerError> {
        let me = surql::escape_str(app);
        ctx.broker.db.sql_root(&format!(
            "UPDATE doctype SET \
               fields = (fields ?? []).filter(|$f| $f.ext_app != '{me}'), \
               server_script = (server_script ?? []).filter(|$s| $s.app != '{me}') \
             WHERE app != '{me}';"
        ))?;
        crate::broker::invalidate_meta(ctx.broker.db.tenant_id());
        Ok(())
    }

    /// Enable / disable â€” **detach without data loss**.
    ///
    /// Disable clears the app's client scripts and flips the registry flag, so
    /// hooks, routes and scripts stop acting. It does NOT drop tables, delete
    /// rows, or remove DocType metadata: the data stays readable and the
    /// script text survives in the stored manifest, which is what makes enable
    /// a restoration rather than a reconstruction.
    ///
    /// This is the same detach machinery uninstall uses (criterion 5), and the
    /// honest answer written there describes exactly this: metadata detaches,
    /// data remains.
    fn set_app_enabled(&self, ctx: &TenantContext, name: &str, enabled: bool) -> Result<serde_json::Value, BrokerError> {
        let Some(row) = self.installed(ctx, name)? else {
            return Err(BrokerError::UnknownDoctype { name: format!("app {name}") });
        };
        let manifest_text = row.get("manifest").and_then(|m| m.as_str()).unwrap_or("{}");
        let manifest = crate::app::Manifest::parse(manifest_text)?;

        for dt in &manifest.doctypes {
            ident(&dt.name)?;
            let n = surql::escape_str(&dt.name);
            if enabled {
                match manifest.client_scripts.iter().find(|s| s.doctype == dt.name) {
                    Some(s) => {
                        let script = surql::render_value(&Value::Text(s.script.clone()))?;
                        ctx.broker.db.sql_root(&format!(
                            "UPDATE doctype SET client_script = {script} WHERE name = '{n}';"
                        ))?;
                    }
                    None => {}
                }
            } else {
                ctx.broker
                    .db
                    .sql_root(&format!("UPDATE doctype SET client_script = NONE WHERE name = '{n}';"))?;
            }
        }

        let n = surql::escape_str(name);
        ctx.broker.db.sql_root(&format!(
            "UPDATE {} SET enabled = {enabled}, updated_at = time::now() WHERE name = '{n}';",
            crate::meta::APP_TABLE
        ))?;

        Ok(serde_json::json!({
            "app": name,
            "enabled": enabled,
            "doctypes_detached": if enabled { 0 } else { manifest.doctypes.len() },
            "data_removed": false,
        }))
    }
}

/// Refuse any top-level key the route does not understand.
///
/// A silently-ignored field is a lie the client cannot see: it asked for
/// something and got told nothing. Naming the unknown key AND the accepted
/// ones makes the refusal actionable in one round trip.
fn known_keys(json: &serde_json::Value, allowed: &[&str]) -> Result<(), BrokerError> {
    let Some(obj) = json.as_object() else { return Ok(()) };
    if let Some(bad) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(BrokerError::InvalidValue {
            detail: format!(
                "FRUST:E_UNKNOWN_FIELD: '{bad}' is not a field this route accepts                  (it takes {}). It was previously ignored in silence, which is why                  it is refused now.",
                allowed.iter().map(|a| format!("'{a}'")).collect::<Vec<_>>().join(", ")
            ),
        });
    }
    Ok(())
}

fn require_manager(caller: &Caller) -> Result<(), BrokerError> {
    if caller.role == "manager" {
        Ok(())
    } else {
        Err(BrokerError::PermissionDenied { detail: "manager role required".into() })
    }
}

fn ident(s: &str) -> Result<(), BrokerError> {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Ok(())
    } else {
        Err(BrokerError::InvalidValue { detail: format!("bad identifier: {s}") })
    }
}

fn header(req: &tiny_http::Request, name: &str) -> String {
    req.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default()
}

fn no_store_header() -> tiny_http::Header {
    tiny_http::Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap()
}

fn status_for(e: &BrokerError) -> u16 {
    match e {
        BrokerError::PermissionDenied { detail } if detail == "E_UNAUTHENTICATED" => 401,
        // a rejected login is the caller's credentials, not a
        // server fault - 401, and never carrying the transport's own words
        BrokerError::PermissionDenied { detail } if detail == crate::db::AUTH_REJECTED => 401,
        // live budget: not an error, a capacity answer â€” poll instead
        BrokerError::Db { detail } if detail.contains("FRUST:E_SUB_BUDGET") => 429,
        // tenant door budget: the standard "slow down" answer, with a hint
        BrokerError::TenantThrottled { .. } => 429,
        BrokerError::PermissionDenied { .. } | BrokerError::FieldNotReadable { .. } => 403,
        BrokerError::IdentityUnresolved => 403,
        BrokerError::UnknownDoctype { .. } => 404,
        // a refused transition is a rule violation, not a bad request
        BrokerError::WorkflowDenied { .. } => 422,
        BrokerError::HookRejected { .. } | BrokerError::HookCycle { .. } | BrokerError::HookDepthExceeded { .. } => 422,
        BrokerError::WriteConflictExhausted { .. } | BrokerError::Db { .. } => 500,
        _ => 400,
    }
}

// â”€â”€ JSON -> contract (the wire form of the structured filter contract) â”€â”€â”€â”€â”€â”€

fn parse_value(v: &serde_json::Value) -> Result<Value, BrokerError> {
    // typed form: { "kind": "decimal", "v": "19.99" } preserves decimal etc.
    if let Some(obj) = v.as_object() {
        if let (Some(kind), Some(val)) = (obj.get("kind").and_then(|k| k.as_str()), obj.get("v")) {
            return typed_value(kind, val);
        }
    }
    Ok(infer_value(v))
}

fn typed_value(kind: &str, v: &serde_json::Value) -> Result<Value, BrokerError> {
    let s = || v.as_str().unwrap_or_default().to_string();
    Ok(match kind {
        "null" => Value::Null,
        "bool" => Value::Bool(v.as_bool().unwrap_or(false)),
        "int" => Value::Int(v.as_i64().unwrap_or(0)),
        "float" => Value::Float(v.as_f64().unwrap_or(0.0)),
        "decimal" => Value::Decimal(if v.is_string() { s() } else { v.to_string() }),
        "text" => Value::Text(s()),
        "datetime" => Value::Datetime(s()),
        "duration" => Value::Duration(s()),
        "record" => Value::RecordId(s()),
        other => return Err(BrokerError::InvalidValue { detail: format!("unknown value kind: {other}") }),
    })
}

fn infer_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            n.as_i64().map(Value::Int).unwrap_or_else(|| Value::Float(n.as_f64().unwrap_or(0.0)))
        }
        serde_json::Value::String(s) => Value::Text(s.clone()),
        // recurse through `parse_value`, not `infer_value`, so a
        // typed envelope decodes at ANY depth. Embedded child rows are
        // arrays of objects whose money fields are `{kind:"decimal"}` â€” with
        // a plain inference here they would land in the DB as literal
        // envelope objects instead of decimals.
        serde_json::Value::Array(a) => {
            Value::List(a.iter().map(|x| parse_value(x).unwrap_or_else(|_| infer_value(x))).collect())
        }
        serde_json::Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), parse_value(v).unwrap_or_else(|_| infer_value(v))))
                .collect(),
        ),
    }
}

fn parse_doc(v: Option<&serde_json::Value>) -> Result<Vec<(String, Value)>, BrokerError> {
    let Some(serde_json::Value::Object(o)) = v else {
        return Err(BrokerError::InvalidValue { detail: "doc/payload must be an object".into() });
    };
    o.iter().map(|(k, v)| Ok((k.clone(), parse_value(v)?))).collect()
}

fn parse_path(v: &serde_json::Value) -> Result<Vec<PathSegment>, BrokerError> {
    // ["field", "link.hop"] -> segments; a plain string is a single field
    match v {
        serde_json::Value::String(s) => {
            Ok(s.split('.').map(|p| PathSegment::Field(p.to_string())).collect())
        }
        serde_json::Value::Array(a) => a
            .iter()
            .map(|seg| {
                seg.as_str()
                    .map(|s| PathSegment::Field(s.to_string()))
                    .ok_or_else(|| BrokerError::InvalidValue { detail: "path segment must be a string".into() })
            })
            .collect(),
        _ => Err(BrokerError::InvalidValue { detail: "path must be string or array".into() }),
    }
}

fn parse_paths(v: Option<&serde_json::Value>) -> Result<Vec<Vec<PathSegment>>, BrokerError> {
    match v {
        None | Some(serde_json::Value::Null) => Ok(vec![]),
        Some(serde_json::Value::Array(a)) => a.iter().map(parse_path).collect(),
        _ => Err(BrokerError::InvalidValue { detail: "expected array of paths".into() }),
    }
}

fn parse_fields(v: Option<&serde_json::Value>) -> Result<Vec<Vec<PathSegment>>, BrokerError> {
    parse_paths(v)
}

fn parse_filter(v: Option<&serde_json::Value>) -> Result<Option<Filter>, BrokerError> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(None) };
    Ok(Some(parse_filter_inner(v)?))
}

fn parse_filter_inner(v: &serde_json::Value) -> Result<Filter, BrokerError> {
    let obj = v.as_object().ok_or_else(|| BrokerError::InvalidValue { detail: "filter must be object".into() })?;
    if let Some(arr) = obj.get("and").and_then(|a| a.as_array()) {
        return Ok(Filter::And(arr.iter().map(parse_filter_inner).collect::<Result<_, _>>()?));
    }
    if let Some(arr) = obj.get("or").and_then(|a| a.as_array()) {
        return Ok(Filter::Or(arr.iter().map(parse_filter_inner).collect::<Result<_, _>>()?));
    }
    if let Some(inner) = obj.get("not") {
        return Ok(Filter::Not(Box::new(parse_filter_inner(inner)?)));
    }
    // leaf: { path, op, value }
    let path = parse_path(obj.get("path").ok_or_else(|| BrokerError::InvalidValue { detail: "filter leaf needs path".into() })?)?;
    let op = match obj.get("op").and_then(|o| o.as_str()).unwrap_or("eq") {
        "eq" => CmpOp::Eq,
        "ne" => CmpOp::Ne,
        "gt" => CmpOp::Gt,
        "gte" => CmpOp::Gte,
        "lt" => CmpOp::Lt,
        "lte" => CmpOp::Lte,
        "inside" => CmpOp::Inside,
        "contains" => CmpOp::Contains,
        other => return Err(BrokerError::InvalidValue { detail: format!("unknown op: {other}") }),
    };
    let value = parse_value(obj.get("value").unwrap_or(&serde_json::Value::Null))?;
    Ok(Filter::Cmp { path, op, value })
}

fn parse_opts(json: &serde_json::Value) -> Result<ReadOpts, BrokerError> {
    let order_by = match json.get("order") {
        Some(serde_json::Value::Object(o)) => {
            let path = parse_path(o.get("path").ok_or_else(|| BrokerError::InvalidValue { detail: "order needs path".into() })?)?;
            let dir = if o.get("dir").and_then(|d| d.as_str()) == Some("desc") { SortDir::Desc } else { SortDir::Asc };
            Some((path, dir))
        }
        _ => None,
    };
    Ok(ReadOpts {
        order_by,
        limit: json.get("limit").and_then(serde_json::Value::as_u64),
        start: json.get("start").and_then(serde_json::Value::as_u64),
    })
}

fn parse_metrics(v: Option<&serde_json::Value>) -> Result<Vec<(Metric, Vec<PathSegment>)>, BrokerError> {
    let Some(arr) = v.and_then(|v| v.as_array()) else {
        return Err(BrokerError::InvalidValue { detail: "metrics must be an array".into() });
    };
    arr.iter()
        .map(|m| {
            let o = m.as_object().ok_or_else(|| BrokerError::InvalidValue { detail: "metric must be object".into() })?;
            let metric = match o.get("metric").and_then(|s| s.as_str()).unwrap_or("count") {
                "count" => Metric::Count,
                "sum" => Metric::Sum,
                "min" => Metric::Min,
                "max" => Metric::Max,
                "avg" => Metric::Avg,
                other => return Err(BrokerError::InvalidValue { detail: format!("unknown metric: {other}") }),
            };
            let path = match o.get("path") {
                Some(p) if !p.is_null() => parse_path(p)?,
                _ => vec![],
            };
            Ok((metric, path))
        })
        .collect()
}





