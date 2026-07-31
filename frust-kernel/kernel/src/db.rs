//! SurrealDB wire transport (WO-005 contract line: HTTP `/sql` + WS RPC,
//! no SDK crate unless typed responses earn the build cost).
//!
//! Blocking ureq; async callers wrap in spawn_blocking. Root and record-user
//! sessions; token cache per user.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::contract::BrokerError;
use crate::tenancy::ResolvedTenant;

/// **How to reach the store — never *which* store.**
///
/// WO-040 Chunk A split `ns`/`db` out of this struct and into
/// [`ResolvedTenant`]. That split is the seam: with placement gone from the
/// connection config, no module can pick a namespace or database by filling
/// in a field, because there is no longer a field to fill in.
#[derive(Debug, Clone)]
pub struct ConnConfig {
    pub endpoint: String,
    pub root_user: String,
    pub root_pass: String,
    pub access: String,
}

impl Default for ConnConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:8899".into(),
            root_user: "root".into(),
            root_pass: "root".into(),
            access: "account".into(),
        }
    }
}

/// **WO-041: one HTTP agent per ENDPOINT**, shared by every `Db` that talks
/// to it.
///
/// Until now `db.rs` called `ureq::post(..)` — the bare free function — which
/// opens a fresh TCP connection per call and drops it. WO-026 measured that
/// model and exonerated it *for throughput* (`bare ≈ fresh ≈ pooled within
/// 10%`), which was true and still is. The dimension it never measured is
/// **resource exhaustion**, and WO-040 Chunk B's load leg found it: TIME_WAIT
/// climbing at request rate (118 → 1252 → 4274 → 6150 across three benches).
/// On Windows's ~14k ephemeral ports with a ~4-minute TIME_WAIT, sustained
/// ~50 req/s reaches port exhaustion in minutes.
///
/// **The A/B that settled it** (`examples/connprobe.rs`, 200 sequential
/// queries against the dev store):
///
/// | model | TIME_WAIT left behind | rate |
/// |---|---|---|
/// | bare `ureq::post` | **200** | 54.8 req/s |
/// | one persistent `Agent` | **1** | 63.7 req/s |
///
/// Not a trade: reuse removes a TCP handshake per query, so it is faster *and*
/// bounded. SurrealDB advertises no `Connection: close`, so the connection was
/// always reusable — nothing server-side had to change.
///
/// **Per endpoint, not per tenant.** Every tenant's queries go to the same
/// SurrealDB, so pool size follows query *rate*, not tenant *count* — N
/// tenants add no sockets. The registry lock is taken once per `Db`
/// construction and never on the query path, the same shape `tenant_gen` uses
/// for its generation handles: don't trade a resource win for a lock
/// (WO-024/025's lesson).
pub(crate) fn agent_for(endpoint: &str) -> ureq::Agent {
    static AGENTS: OnceLock<Mutex<HashMap<String, ureq::Agent>>> = OnceLock::new();
    let mut map = AGENTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.entry(endpoint.to_string())
        .or_insert_with(|| {
            // Sized to the REST worker pool (`Rest::serve` clamps to 2..=16)
            // plus headroom for the resident worker, rollup drains and boot,
            // so 16 workers in flight never evict each other's connection and
            // fall back to a handshake apiece.
            let idle = std::thread::available_parallelism()
                .map(std::num::NonZeroUsize::get)
                .unwrap_or(4)
                .clamp(2, 16)
                + 8;
            ureq::Agent::config_builder()
                .max_idle_connections_per_host(idle)
                .max_idle_connections(idle)
                .build()
                .into()
        })
        .clone()
}

// ── WO-044: the root token ───────────────────────────────────────────────────

/// A minted root JWT and when it stops being trustworthy.
struct CachedRoot {
    token: String,
    /// Absolute instant after which we re-mint. Derived from the token's own
    /// `exp` claim, not from a constant: SurrealDB currently issues root tokens
    /// with a 1-hour life, and a hardcoded assumption about that would silently
    /// become wrong the day it changes.
    refresh_at: std::time::Instant,
}

/// Per-endpoint root credential state.
///
/// `Basic` is the pre-WO-044 behaviour, kept as a **loud** fallback: if
/// `/signin` will not mint a root token (an older store, a differently
/// configured root user), the kernel keeps working exactly as it did before and
/// says so at error level with a `frust_root_auth{mode="basic"}` gauge. Silently
/// paying a 21× tax would be the failure this WO exists to remove.
#[derive(Default)]
struct RootAuth {
    cached: Option<CachedRoot>,
    /// Set once `/signin` has been proven unavailable, so the kernel stops
    /// re-attempting it on every query.
    basic_only: bool,
    /// Test-only: pin this handle to the JWT path regardless of
    /// `FRUST_ROOT_AUTH`. A test that asserts JWT-path behaviour has to OWN its
    /// arm — WO-047 found three of WO-044's tests floating on the process env
    /// instead, so they failed under `FRUST_ROOT_AUTH=basic` for the entirely
    /// correct reason that there was no token to assert about. The Basic arm was
    /// already pinned (`force_basic_for_test`); this is the missing half.
    force_jwt: bool,
}

/// `FRUST_ROOT_AUTH=jwt|basic` — default `jwt`.
///
/// Exists for two reasons, and the second is why it is not just a test hook:
/// the WO's own A/B needs both paths measurable **in one process**, and an
/// operator who hits an unforeseen incompatibility deserves a documented way
/// back to the previous behaviour rather than a downgrade. An unrecognised
/// value refuses rather than guessing, the same posture as `FRUST_MAIL`.
///
/// Read once per process: this must never touch the query path.
fn root_auth_mode() -> &'static str {
    static MODE: OnceLock<&'static str> = OnceLock::new();
    MODE.get_or_init(|| match std::env::var("FRUST_ROOT_AUTH").as_deref() {
        Ok("basic") => "basic",
        Ok("jwt") | Err(_) => "jwt",
        Ok(other) => {
            crate::telemetry::emit(
                crate::telemetry::Level::Error,
                "root_auth_mode_refused",
                &[(
                    "error",
                    serde_json::json!(format!(
                        "FRUST_ROOT_AUTH must be 'jwt' or 'basic', got {other:?}"
                    )),
                )],
            );
            std::process::exit(1);
        }
    })
}

/// The registry, keyed by `endpoint|user` — the same shape as [`agent_for`], and
/// for the same reason: **the registry lock is taken once per `Db`
/// construction and never on the query path** (WO-024/025). What the query path
/// touches is the per-endpoint handle below, an uncontended `RwLock` read.
///
/// Keyed by endpoint AND user because a root token is scope-free (`ID: root`,
/// no `ns`/`db` claims — probed), so every tenant sharing a store shares one
/// token. N tenants mint one token, not N.
fn root_auth_for(endpoint: &str, user: &str) -> std::sync::Arc<std::sync::RwLock<RootAuth>> {
    static ROOTS: OnceLock<Mutex<HashMap<String, std::sync::Arc<std::sync::RwLock<RootAuth>>>>> =
        OnceLock::new();
    let mut map = ROOTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.entry(format!("{endpoint}|{user}"))
        .or_insert_with(|| std::sync::Arc::new(std::sync::RwLock::new(RootAuth::default())))
        .clone()
}

/// Seconds of life left at which a token is re-minted, as a fraction. Refreshing
/// at 75% means a 1-hour token is replaced with 15 minutes to spare — long
/// enough that the reactive 401 path below stays a backstop rather than the
/// normal case.
const REFRESH_AT_FRACTION: f64 = 0.75;
/// Used when a token carries no readable `exp`. Deliberately short: guessing
/// long would risk serving an expired token, and the reactive retry makes a
/// wrong guess cheap in only one direction.
const REFRESH_FALLBACK_SECS: u64 = 300;

/// Read a JWT's `exp`/`iat` and return how long to trust it.
///
/// Best-effort by design: an unparseable token still WORKS (the store is the
/// authority on validity), it just gets re-minted sooner.
fn token_lifetime(token: &str) -> std::time::Duration {
    use base64::Engine as _;
    let fallback = std::time::Duration::from_secs(REFRESH_FALLBACK_SECS);
    let Some(payload) = token.split('.').nth(1) else { return fallback };
    let trimmed = payload.trim_end_matches('=');
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(trimmed) else {
        return fallback;
    };
    let Ok(claims) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return fallback };
    let (Some(exp), Some(iat)) =
        (claims.get("exp").and_then(|v| v.as_i64()), claims.get("iat").and_then(|v| v.as_i64()))
    else {
        return fallback;
    };
    let life = exp.saturating_sub(iat);
    if life <= 0 {
        return fallback;
    }
    #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    std::time::Duration::from_secs(((life as f64) * REFRESH_AT_FRACTION) as u64)
}

/// Is this error SurrealDB refusing the credential (rather than the query)?
fn is_auth_failure(e: &BrokerError) -> bool {
    match e {
        BrokerError::PermissionDenied { .. } => true,
        BrokerError::Db { detail } => {
            detail.contains("401") || detail.to_lowercase().contains("unauthor")
        }
        _ => false,
    }
}

pub struct Db {
    target: ResolvedTenant,
    /// The shared, connection-reusing HTTP client for this endpoint (WO-041).
    /// Resolved once at construction so the query path never touches the
    /// registry.
    agent: ureq::Agent,
    /// **WO-044:** this endpoint's root credential, resolved once at
    /// construction like the agent above. The query path reads it under a
    /// shared `RwLock`; it never takes the registry lock and never runs argon2.
    root: std::sync::Arc<std::sync::RwLock<RootAuth>>,
    tokens: OnceLock<Mutex<HashMap<String, String>>>,
    /// Conflict retries observed (criterion-6 telemetry + the canary test).
    conflict_retries: std::sync::atomic::AtomicU64,
    /// WO-044: signins performed by THIS handle — the per-handle counterpart of
    /// `frust_root_signin_total`, so a test can assert "argon2 ran once" exactly
    /// rather than racing a process-global counter.
    root_signins: std::sync::atomic::AtomicU64,
}

/// **The one door to a database handle.**
///
/// Every `Db` in the kernel comes from here, and a [`ResolvedTenant`] can only
/// be minted by [`crate::tenancy`]. There is deliberately **no** `Db::new`,
/// `Db::for_database`, or any other name-taking overload beside it: a
/// fallback that accepted a `&str` would recreate the exact bypass this seam
/// closes, and would do it while looking like a convenience.
pub fn scoped_db(target: &ResolvedTenant) -> Db {
    Db {
        agent: agent_for(target.endpoint()),
        root: root_auth_for(target.endpoint(), &target.conn().root_user),
        target: target.clone(),
        tokens: OnceLock::new(),
        conflict_retries: std::sync::atomic::AtomicU64::new(0),
        root_signins: std::sync::atomic::AtomicU64::new(0),
    }
}

impl Db {
    /// Who this handle serves and where they live. Read-only: a handle's
    /// target is fixed at construction and never mutated, which is what makes
    /// two concurrent requests unable to move each other's target.
    pub fn target(&self) -> &ResolvedTenant {
        &self.target
    }

    /// The tenant's canonical id — the telemetry, fairness and cache label.
    ///
    /// Note this is deliberately **not** the database name any more. Under
    /// database-per-tenant they coincide; under a namespace topology they do
    /// not, and every label that means "tenant" must keep meaning tenant.
    pub fn tenant_id(&self) -> &str {
        self.target.tenant_id().as_str()
    }

    /// The record access method name (WO-027 key guard).
    pub fn access(&self) -> &str {
        &self.target.conn().access
    }

    /// Total optimistic-concurrency retries this instance has performed.
    pub fn conflict_retries(&self) -> u64 {
        self.conflict_retries.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn note_retry(&self) {
        self.conflict_retries.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        crate::telemetry::inc("frust_conflict_retries_total", &[("tenant", self.tenant_id())], 1);
    }

    fn read_json(res: &mut ureq::http::Response<ureq::Body>) -> Result<serde_json::Value, BrokerError> {
        let text = res
            .body_mut()
            .read_to_string()
            .map_err(|e| BrokerError::Db { detail: format!("read body: {e}") })?;
        serde_json::from_str(&text).map_err(|e| BrokerError::Db { detail: format!("parse: {e}") })
    }

    /// SurrealDB uses optimistic concurrency: concurrent writers can get
    /// "Transaction write conflict. This transaction can be retried" â€” the
    /// transaction did NOT commit, so retrying is safe and required. This is
    /// the one place the kernel honors that contract.
    const CONFLICT_RETRIES: u32 = 5;

    fn is_conflict(detail: &str) -> bool {
        detail.contains("Transaction conflict") || detail.contains("can be retried")
    }

    pub fn sql_with_auth(&self, auth: &str, query: &str) -> Result<serde_json::Value, BrokerError> {
        self.sql_with_auth_at(auth, query, "session", crate::telemetry::Level::Error)
    }

    /// The kernel's own root call — labelled explicitly rather than sniffed from
    /// the header, because WO-044 made root a Bearer token too.
    fn sql_as_root(&self, auth: &str, query: &str) -> Result<serde_json::Value, BrokerError> {
        self.sql_with_auth_at(auth, query, "root", crate::telemetry::Level::Error)
    }

    /// WO-033 item 2: `sql_with_auth`, but a failure logs at **Debug**.
    ///
    /// One caller only — the keyguard's self-forge probe (ADR-013), whose
    /// refusal is the HEALTHY answer. Without this a clean boot emits an
    /// `lvl:error / E_DB` line every time, which trains operators to ignore
    /// boot errors: exactly the "errors are real" discipline WO-010 built.
    ///
    /// **Only the log level changes. The `Result` is returned untouched**, so
    /// the keyguard still distinguishes an auth refusal (`Ok(false)`, healthy)
    /// from a transport failure (`Err`, refuse to boot). Suppressing the
    /// *outcome* rather than the *log* would turn "I could not verify" into
    /// "verified safe" — the one thing a fail-closed guard must never do.
    pub fn sql_with_auth_quiet(&self, auth: &str, query: &str) -> Result<serde_json::Value, BrokerError> {
        self.sql_with_auth_at(auth, query, "session", crate::telemetry::Level::Debug)
    }

    fn sql_with_auth_at(
        &self,
        auth: &str,
        query: &str,
        kind: &'static str,
        fail_level: crate::telemetry::Level,
    ) -> Result<serde_json::Value, BrokerError> {
        let _tenant = crate::telemetry::enter_tenant(self.tenant_id());
        // WO-044: label by CREDENTIAL CLASS, not by scheme. Root is now a Bearer
        // token too, so the old `starts_with("Basic")` test would have silently
        // relabelled every root call as a session call the moment the JWT landed
        // — a metric quietly measuring the wrong thing, which is worse than no
        // metric. The kernel's own root calls are tagged explicitly.
        crate::telemetry::inc(
            "frust_db_calls_total",
            &[("kind", kind), ("tenant", self.tenant_id())],
            1,
        );
        let span = crate::telemetry::Span::begin("db_call")
            .field("kind", kind)
            .field("stmts", query.matches(';').count().max(1) as u64);
        let result = self.sql_with_auth_inner(auth, query);
        match &result {
            // success at Debug: the hot path stays cheap at the Info default;
            // failures ALWAYS emit â€” silent is the enemy (level is Error for
            // every caller but the keyguard probe)
            Ok(_) => span.ok_at(crate::telemetry::Level::Debug),
            Err(e) => span.err_at(e, fail_level),
        }
        result
    }

    fn sql_with_auth_inner(&self, auth: &str, query: &str) -> Result<serde_json::Value, BrokerError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            crate::telemetry::inc(
                "frust_db_calls_total",
                &[("kind", "root"), ("tenant", self.tenant_id())],
                1,
            );
            let mut res = self.agent.post(format!("{}/sql", self.target.endpoint()))
                .header("Authorization", auth)
                .header("Accept", "application/json")
                .header("surreal-ns", self.target.namespace().as_str())
                .header("surreal-db", self.target.database().as_str())
                .send(query)
                .map_err(|e| BrokerError::Db { detail: format!("transport: {e}") })?;
            let parsed = Self::read_json(&mut res)?;
            let arr = parsed
                .as_array()
                .ok_or_else(|| BrokerError::Db { detail: format!("unexpected response: {parsed}") })?
                .clone();

            // Collect ALL statement errors first: in a BEGIN/COMMIT batch the
            // conflicting statement carries the conflict wording while its
            // companions only say "not executed due to a failed transaction" â€”
            // deciding on the first non-OK statement misclassifies conflicts.
            let errors: Vec<String> = arr
                .iter()
                .filter(|stmt| stmt.get("status").and_then(|s| s.as_str()) != Some("OK"))
                .map(|stmt| stmt.get("result").map(std::string::ToString::to_string).unwrap_or_default())
                .collect();
            if errors.iter().any(|e| Self::is_conflict(e)) {
                if attempt <= Self::CONFLICT_RETRIES {
                    self.note_retry();
                    std::thread::sleep(std::time::Duration::from_millis(30 * u64::from(attempt)));
                    continue;
                }
                // typed exhaustion: raw wording never reaches callers
                return Err(BrokerError::WriteConflictExhausted { attempts: attempt });
            }
            if let Some(raw) = errors
                .iter()
                .find(|e| !e.contains("not executed due to"))
                .or_else(|| errors.first())
            {
                // permission failures surface as statement errors under a
                // record session; classify the obvious ones
                if raw.contains("Not enough permissions") {
                    return Err(BrokerError::PermissionDenied { detail: raw.clone() });
                }
                // the identity_guard EVENT's machine code (WO-008)
                if raw.contains("FRUST:E_IDENTITY_UNRESOLVED") {
                    return Err(BrokerError::IdentityUnresolved);
                }
                return Err(BrokerError::Db { detail: raw.clone() });
            }
            return Ok(arr.last().and_then(|s| s.get("result")).cloned().unwrap_or(serde_json::Value::Null));
        }
    }

    /// **WO-044: the root credential, cached as a JWT.**
    ///
    /// SurrealDB argon2-verifies a Basic root password on **every request** —
    /// measured at 16.56 ms against 0.79 ms for a Bearer token running the
    /// identical query in the same process, a 21× tax the kernel was paying on
    /// every metadata read, job claim, rollup drain and DDL statement. Signing
    /// in once moves that argon2 to a single call and leaves a signature check
    /// on the hot path.
    ///
    /// The hot path here is: one uncontended `RwLock` read and a `String`
    /// clone. No registry lock (resolved at construction), no argon2, no
    /// network — until the token approaches its own expiry.
    fn root_auth(&self) -> Result<String, BrokerError> {
        let forced_jwt =
            self.root.read().unwrap_or_else(std::sync::PoisonError::into_inner).force_jwt;
        if !forced_jwt && root_auth_mode() == "basic" {
            // The gauge is published on EVERY path, including this one. It was
            // originally only set where a token got minted, so
            // `FRUST_ROOT_AUTH=basic` produced no `frust_root_auth` series at
            // all — leaving an operator unable to tell "running on Basic" from
            // "this kernel predates the metric". An absent series is not a
            // reading. (`gauge` is a cheap map write and this branch is only
            // taken when the operator has already opted into the slow path.)
            crate::telemetry::gauge("frust_root_auth", &[("mode", "basic")], 1.0);
            return Ok(self.root_basic());
        }
        // ── fast path: a token we still trust ──
        {
            let guard = self.root.read().unwrap_or_else(std::sync::PoisonError::into_inner);
            if guard.basic_only {
                return Ok(self.root_basic());
            }
            if let Some(c) = &guard.cached {
                if std::time::Instant::now() < c.refresh_at {
                    return Ok(format!("Bearer {}", c.token));
                }
            }
        }
        // ── slow path: mint. Double-checked, because two threads can arrive
        //    here together and only one should pay for the signin. ──
        let mut guard = self.root.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.basic_only {
            return Ok(self.root_basic());
        }
        if let Some(c) = &guard.cached {
            if std::time::Instant::now() < c.refresh_at {
                return Ok(format!("Bearer {}", c.token));
            }
        }
        match self.root_signin() {
            Ok(token) => {
                let refresh_at = std::time::Instant::now() + token_lifetime(&token);
                let auth = format!("Bearer {token}");
                guard.cached = Some(CachedRoot { token, refresh_at });
                crate::telemetry::gauge("frust_root_auth", &[("mode", "jwt")], 1.0);
                crate::telemetry::inc("frust_root_signin_total", &[("endpoint", self.target.endpoint())], 1);
                Ok(auth)
            }
            // **Loud** fallback, never silent. The kernel keeps working exactly
            // as it did before WO-044 — but an operator paying 16 ms a query
            // deserves to be told, not left to discover it in a profile.
            Err(e) => {
                guard.basic_only = true;
                crate::telemetry::gauge("frust_root_auth", &[("mode", "basic")], 1.0);
                crate::telemetry::emit(
                    crate::telemetry::Level::Error,
                    "root_jwt_unavailable",
                    &[
                        ("endpoint", serde_json::json!(self.target.endpoint())),
                        ("error", serde_json::json!(e.to_string())),
                        (
                            "impact",
                            serde_json::json!(
                                "falling back to Basic root auth — correct, but SurrealDB will \
                                 argon2-verify the password on every request (~16 ms/query)"
                            ),
                        ),
                    ],
                );
                Ok(self.root_basic())
            }
        }
    }

    /// Mint a root JWT. The root signin body is `{user, pass}` — no `ns`/`db`/
    /// `ac`, unlike the record-user form in `signin_inner`; the resulting token
    /// carries `ID: root` and no scope claims, which is why one token serves
    /// every tenant on the endpoint.
    fn root_signin(&self) -> Result<String, BrokerError> {
        let body = serde_json::json!({
            "user": self.target.conn().root_user,
            "pass": self.target.conn().root_pass,
        })
        .to_string();
        let mut res = self
            .agent
            .post(format!("{}/signin", self.target.endpoint()))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(|e| BrokerError::Db { detail: format!("root signin transport: {e}") })?;
        let parsed = Self::read_json(&mut res)?;
        self.root_signins.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        parsed
            .get("token")
            .and_then(|t| t.as_str())
            .map(str::to_string)
            .ok_or_else(|| BrokerError::PermissionDenied {
                detail: format!("root signin returned no token: {parsed}"),
            })
    }

    /// Drop the cached token so the next call re-mints — the reactive half of
    /// refresh, used when the store rejects a token we thought was good.
    fn invalidate_root_token(&self) {
        self.root.write().unwrap_or_else(std::sync::PoisonError::into_inner).cached = None;
    }

    /// **Test support: plant a root token the store will reject.**
    ///
    /// The reactive-refresh path only runs when a cached token stops working,
    /// and the honest way to reach that state otherwise is to wait out a
    /// one-hour expiry. Planting a bad token reaches the same branch in a
    /// second, so the retry is proven rather than assumed.
    ///
    /// Safe by construction: the worst a wrong value can do is provoke the very
    /// 401-and-re-mint this exists to exercise.
    #[doc(hidden)]
    pub fn force_root_token_for_test(&mut self, token: &str) {
        self.detach_root_for_test();
        let mut g = self.root.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.basic_only = false;
        // planting a token only means anything on the JWT path, so pin it —
        // otherwise under `FRUST_ROOT_AUTH=basic` the planted token is never
        // sent, no 401 comes back, and the retry this test exists to prove is
        // never exercised (it would pass by not running).
        g.force_jwt = true;
        g.cached = Some(CachedRoot {
            token: token.to_string(),
            refresh_at: std::time::Instant::now() + std::time::Duration::from_secs(3600),
        });
    }

    /// Whether this handle currently holds a minted root JWT — lets a test
    /// assert the CACHE, not merely that queries worked.
    #[doc(hidden)]
    pub fn has_cached_root_token(&self) -> bool {
        self.root.read().unwrap_or_else(std::sync::PoisonError::into_inner).cached.is_some()
    }

    /// Give this handle its OWN root-credential state instead of the endpoint's
    /// shared one.
    ///
    /// Necessary because the cache is deliberately per-endpoint (one token
    /// serves every tenant), which means two test threads manipulating it would
    /// poison each other — and a test that fails depending on what ran beside it
    /// is worse than no test. `&mut self` is what makes this safe: a detached
    /// handle cannot be shared.
    #[doc(hidden)]
    pub fn detach_root_for_test(&mut self) {
        self.root = std::sync::Arc::new(std::sync::RwLock::new(RootAuth::default()));
        self.root_signins.store(0, std::sync::atomic::Ordering::Relaxed);
    }

    /// Pin this handle to the pre-WO-044 Basic credential — the per-handle
    /// equivalent of `FRUST_ROOT_AUTH=basic`, which is what makes the A/B
    /// measurable inside one process.
    #[doc(hidden)]
    pub fn force_basic_for_test(&mut self) {
        self.detach_root_for_test();
        let mut g = self.root.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.basic_only = true;
    }

    /// Pin this handle to the JWT path whatever `FRUST_ROOT_AUTH` says — the
    /// counterpart of `force_basic_for_test`, so a test that asserts JWT
    /// behaviour asserts it in both process modes.
    #[doc(hidden)]
    pub fn force_jwt_for_test(&mut self) {
        self.detach_root_for_test();
        let mut g = self.root.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.force_jwt = true;
        g.basic_only = false;
    }

    /// How many times THIS handle has signed in. Per-handle, so the assertion
    /// "argon2 ran once" cannot be perturbed by another test in the binary.
    #[doc(hidden)]
    pub fn root_signins(&self) -> u64 {
        self.root_signins.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[doc(hidden)]
    pub fn root_token_for_test(&self) -> Option<String> {
        self.root
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cached
            .as_ref()
            .map(|c| c.token.clone())
    }

    /// How long until the cached token is re-minted — so a test can assert the
    /// refresh lands before expiry instead of trusting the arithmetic.
    #[doc(hidden)]
    pub fn root_refresh_in_for_test(&self) -> Option<std::time::Duration> {
        self.root
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cached
            .as_ref()
            .map(|c| c.refresh_at.saturating_duration_since(std::time::Instant::now()))
    }

    /// Run a root query, re-minting **once** if the store rejects the token.
    ///
    /// **This retry is deliberately root-only and lives here, not in
    /// `sql_with_auth`.** ADR-013's keyguard probes with a *deliberately
    /// forged* Bearer and reads the 401 as its healthy answer; if the shared
    /// path retried auth failures by signing in as root, the forged probe would
    /// succeed on retry and the guard would report every healthy store
    /// compromised. The blast radius of that mistake is every boot, so the
    /// retry is scoped to calls the kernel makes as itself.
    fn with_root_retry<T>(
        &self,
        run: impl Fn(&str) -> Result<T, BrokerError>,
    ) -> Result<T, BrokerError> {
        let auth = self.root_auth()?;
        let first = run(&auth);
        match first {
            Err(ref e) if is_auth_failure(e) && auth.starts_with("Bearer ") => {
                // the token expired or was revoked mid-flight; mint and retry ONCE
                crate::telemetry::inc(
                    "frust_root_token_refresh_total",
                    &[("reason", "rejected")],
                    1,
                );
                self.invalidate_root_token();
                let fresh = self.root_auth()?;
                run(&fresh)
            }
            other => other,
        }
    }

    fn root_basic(&self) -> String {
        use std::fmt::Write as _;
        let raw = format!("{}:{}", self.target.conn().root_user, self.target.conn().root_pass);
        // minimal base64 (no dep): standard alphabet, padded
        const AL: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let b = raw.as_bytes();
        let mut out = String::new();
        for chunk in b.chunks(3) {
            let n = (u32::from(chunk[0]) << 16)
                | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                | u32::from(*chunk.get(2).unwrap_or(&0));
            let _ = write!(out, "{}", AL[(n >> 18) as usize & 63] as char);
            let _ = write!(out, "{}", AL[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 { AL[(n >> 6) as usize & 63] as char } else { '=' });
            out.push(if chunk.len() > 2 { AL[n as usize & 63] as char } else { '=' });
        }
        format!("Basic {out}")
    }

    /// Root session: kernel-internal reads (metadata, changefeed, sync).
    pub fn sql_root(&self, query: &str) -> Result<serde_json::Value, BrokerError> {
        self.with_root_retry(|auth| self.sql_as_root(auth, query))
    }

    /// Root session scoped to the namespace only â€” for DEFINE/REMOVE DATABASE,
    /// which cannot run with a database header targeting a db that may not
    /// exist yet.
    pub fn sql_root_ns(&self, query: &str) -> Result<serde_json::Value, BrokerError> {
        self.with_root_retry(|auth| {
            crate::telemetry::inc(
                "frust_db_calls_total",
                &[("kind", "root"), ("tenant", self.tenant_id())],
                1,
            );
            let mut res = self.agent.post(format!("{}/sql", self.target.endpoint()))
                .header("Authorization", auth)
                .header("Accept", "application/json")
                .header("surreal-ns", self.target.namespace().as_str())
                .send(query)
                .map_err(|e| BrokerError::Db { detail: format!("transport: {e}") })?;
            let parsed = Self::read_json(&mut res)?;
            let arr = parsed
                .as_array()
                .ok_or_else(|| BrokerError::Db { detail: format!("unexpected response: {parsed}") })?;
            for stmt in arr {
                if stmt.get("status").and_then(|s| s.as_str()) != Some("OK") {
                    return Err(BrokerError::Db {
                        detail: stmt.get("result").map(std::string::ToString::to_string).unwrap_or_default(),
                    });
                }
            }
            Ok(serde_json::Value::Null)
        })
    }

    /// Root session, raw per-statement results â€” for try-create semantics
    /// (advisory locks) where a statement error is an answer, not a failure.
    /// Write conflicts are still retried: a conflict is not an answer.
    pub fn sql_root_raw(&self, query: &str) -> Result<Vec<serde_json::Value>, BrokerError> {
        self.with_root_retry(|auth| self.sql_root_raw_inner(auth, query))
    }

    fn sql_root_raw_inner(
        &self,
        auth: &str,
        query: &str,
    ) -> Result<Vec<serde_json::Value>, BrokerError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            crate::telemetry::inc(
                "frust_db_calls_total",
                &[("kind", "root"), ("tenant", self.tenant_id())],
                1,
            );
            let mut res = self.agent.post(format!("{}/sql", self.target.endpoint()))
                .header("Authorization", auth)
                .header("Accept", "application/json")
                .header("surreal-ns", self.target.namespace().as_str())
                .header("surreal-db", self.target.database().as_str())
                .send(query)
                .map_err(|e| BrokerError::Db { detail: format!("transport: {e}") })?;
            let parsed = Self::read_json(&mut res)?;
            let arr = parsed.as_array().cloned().unwrap_or_default();
            let conflicted = arr.iter().any(|stmt| {
                stmt.get("status").and_then(|s| s.as_str()) != Some("OK")
                    && Self::is_conflict(
                        &stmt.get("result").map(std::string::ToString::to_string).unwrap_or_default(),
                    )
            });
            if conflicted {
                if attempt <= Self::CONFLICT_RETRIES {
                    self.note_retry();
                    std::thread::sleep(std::time::Duration::from_millis(30 * u64::from(attempt)));
                    continue;
                }
                return Err(BrokerError::WriteConflictExhausted { attempts: attempt });
            }
            return Ok(arr);
        }
    }

    /// Record-user session: the caller's own authority. SurrealDB enforces
    /// PERMISSIONS; the kernel adds field-envelope filtering on top.
    pub fn sql_as(&self, user: &str, pass: &str, query: &str) -> Result<serde_json::Value, BrokerError> {
        let token = self.token_for(user, pass)?;
        self.sql_with_auth(&format!("Bearer {token}"), query)
    }

    /// Seed the per-user token cache with an externally-obtained JWT (the
    /// session layer re-hydrates sessions across kernel restarts this way).
    pub fn seed_token(&self, user: &str, token: &str) {
        let cache = self.tokens.get_or_init(|| Mutex::new(HashMap::new()));
        cache.lock().unwrap().insert(user.to_string(), token.to_string());
    }

    /// Prove credentials against the record access and return the SurrealDB
    /// JWT (also cached). Bad credentials -> PermissionDenied.
    pub fn signin(&self, user: &str, pass: &str) -> Result<String, BrokerError> {
        self.signin_inner(user, pass)
    }

    fn token_for(&self, user: &str, pass: &str) -> Result<String, BrokerError> {
        let cache = self.tokens.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(t) = cache.lock().unwrap().get(user) {
            return Ok(t.clone());
        }
        self.signin_inner(user, pass)
    }

    fn signin_inner(&self, user: &str, pass: &str) -> Result<String, BrokerError> {
        let cache = self.tokens.get_or_init(|| Mutex::new(HashMap::new()));
        let body = serde_json::json!({
            "ns": self.target.namespace().as_str(),
            "db": self.target.database().as_str(),
            "ac": self.access(),
            "name": user, "pass": pass,
        })
        .to_string();
        let mut res = self.agent.post(format!("{}/signin", self.target.endpoint()))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(|e| BrokerError::Db { detail: format!("signin transport: {e}") })?;
        let parsed = Self::read_json(&mut res)?;
        let token = parsed
            .get("token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| BrokerError::PermissionDenied { detail: format!("signin failed: {parsed}") })?
            .to_string();
        cache.lock().unwrap().insert(user.to_string(), token.clone());
        Ok(token)
    }
}

