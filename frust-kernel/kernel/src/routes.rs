//! WO-019 criterion 1: plugin-declared REST routes (REQ-2.2.2).
//!
//! **This module is deliberately absent from `surql_monopoly`'s allowlist.**
//! That is the point: a plugin route reaches data only by calling broker
//! verbs, so no query text can legitimately appear here, and CI fails the
//! build if any ever does. The door is proven by a test, not by a promise.
//!
//! Routes bind to the `route-plugin` world, separate from `plugin`: declaring
//! a route is a distinct capability claim, and a hook-only component (the
//! script engine, every plain hook plugin) must not have to name the door to
//! remain loadable.
//!
//! Ownership stays acyclic — `RouteHost -> Broker -> WasmHooks` — because
//! routes are a separate door owned by the REST layer, which already holds the
//! broker.

use std::sync::{Arc, Mutex};

use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::broker::{Broker, Caller};
use crate::contract::BrokerError;

wasmtime::component::bindgen!({ path: "../wit", world: "route-plugin" });

/// Same wall-clock and memory posture as a hook (ADR-005): a route handler is
/// untrusted guest code reached from the network, so it gets the stricter of
/// the two treatments, never a relaxed one.
const CALL_DEADLINE_TICKS: u64 = 50;
const EPOCH_TICK_MS: u64 = 10;
const CALL_FUEL: u64 = 2_000_000_000;
const MEM_CAP: usize = 128 << 20;

struct RouteState {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    /// THE DOOR, bound by the HOST for the duration of one call and torn down
    /// with the store. The guest cannot set, read or influence it: it is not
    /// in the guest's world. There is no caller parameter to forge.
    door: Option<(Arc<Broker>, Caller)>,
}

impl WasiView for RouteState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

impl frust::plugin::host_api::Host for RouteState {
    fn log(&mut self, msg: String) {
        crate::telemetry::emit(
            crate::telemetry::Level::Info,
            "route_log",
            &[("msg", serde_json::json!(msg))],
        );
    }
}

/// The host side of the door — where every escape has to fail STRUCTURALLY,
/// not by inspecting what was asked for:
///
/// - **cannot be someone else** — the `Caller` comes from `self.door`, which
///   the host bound from the request it is serving.
/// - **cannot write a query** — `doctype` is identifier-checked by the broker
///   and `filter` is parsed into the typed ADR-006 `Filter` tree. SurrealQL in
///   either is a parse error, never text that reaches the database.
/// - **cannot get a handle** — `db-read` is the whole surface; `Db` is not
///   reachable from the guest's world at all.
/// - **is not a second permission compiler** — this calls `Broker::db_read`,
///   the same one the Desk and REST use, so row rules are enforced by the DB
///   under the caller's own session (ADR-003).
impl frust::plugin::db_api::Host for RouteState {
    fn db_read(
        &mut self,
        doctype: String,
        filter: String,
        fields: Vec<String>,
    ) -> Result<String, String> {
        let Some((broker, caller)) = self.door.as_ref() else {
            return Err("FRUST:E_NO_DOOR: db-read is not available in this context".into());
        };
        let filter: Option<crate::contract::Filter> = match filter.trim() {
            "" | "null" => None,
            raw => Some(serde_json::from_str(raw).map_err(|e| {
                format!(
                    "FRUST:E_BAD_FILTER: filter must be a structured ADR-006 filter, not text ({e})"
                )
            })?),
        };
        let paths: Vec<Vec<crate::contract::PathSegment>> = fields
            .iter()
            .map(|f| vec![crate::contract::PathSegment::Field(f.clone())])
            .collect();
        broker
            .db_read(
                caller,
                &doctype,
                filter.as_ref(),
                &paths,
                &crate::contract::ReadOpts::default(),
            )
            .map(|rows| serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into()))
            .map_err(|e| format!("{}: {e:?}", crate::telemetry::error_code(&e)))
    }
}

/// Where shipped `.wasm` components live. Same seam `main.rs` uses, so a
/// deployment configures one path, not two.
pub fn artifacts_dir() -> String {
    std::env::var("FRUST_ARTIFACTS")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts").to_string())
}

/// Compiled components, cached per process.
///
/// Compiling a component is expensive enough that doing it per request would
/// make a plugin route's latency a compilation benchmark. Keyed by filename:
/// two apps shipping the same component share one compiled module, which is
/// safe because a `RouteHost` holds no per-app state — the app identity and
/// the caller both arrive per call.
pub fn host_for(component: &str, broker: Arc<Broker>) -> Result<Arc<RouteHost>, BrokerError> {
    static HOSTS: std::sync::OnceLock<Mutex<std::collections::HashMap<String, Arc<RouteHost>>>> =
        std::sync::OnceLock::new();
    let cache = HOSTS.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(h) = guard.get(component) {
        return Ok(Arc::clone(h));
    }
    let host = Arc::new(RouteHost::load(&artifacts_dir(), component, broker)?);
    guard.insert(component.to_string(), Arc::clone(&host));
    Ok(host)
}

pub struct RouteHost {
    engine: Engine,
    pre: RoutePluginPre<RouteState>,
    broker: Arc<Broker>,
    /// One instance, serialized. A route handler is guest code with mutable
    /// linear memory; sharing it across concurrent requests would leak one
    /// caller's state into another's handler.
    lock: Mutex<()>,
}

impl RouteHost {
    pub fn load(
        artifacts_dir: &str,
        component: &str,
        broker: Arc<Broker>,
    ) -> Result<Self, BrokerError> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        config.consume_fuel(true);
        let engine = Engine::new(&config)
            .map_err(|e| BrokerError::Db { detail: format!("route engine: {e}") })?;
        {
            let engine = engine.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(EPOCH_TICK_MS));
                engine.increment_epoch();
            });
        }
        let mut linker: Linker<RouteState> = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|e| BrokerError::Db { detail: format!("wasi link: {e}") })?;
        RoutePlugin::add_to_linker::<RouteState, wasmtime::component::HasSelf<RouteState>>(
            &mut linker,
            |s| s,
        )
        .map_err(|e| BrokerError::Db { detail: format!("host link: {e}") })?;

        let path = format!("{artifacts_dir}/{component}");
        let component = Component::from_file(&engine, &path)
            .map_err(|e| BrokerError::Db { detail: format!("load {path}: {e}") })?;
        let pre = linker
            .instantiate_pre(&component)
            .map_err(|e| BrokerError::Db { detail: format!("pre {path}: {e}") })?;
        let pre = RoutePluginPre::new(pre)
            .map_err(|e| BrokerError::Db { detail: format!("bind {path}: {e}") })?;

        Ok(Self { engine, pre, broker, lock: Mutex::new(()) })
    }

    /// Serve one request through the plugin's route handler.
    ///
    /// The `Caller` is bound here, by the host, from the session that
    /// authenticated the HTTP request — and torn down with the store when the
    /// call returns. A handler cannot outlive, cache, or widen the authority
    /// it ran under.
    pub fn handle(
        &self,
        app: &str,
        caller: &Caller,
        path: &str,
        query: &str,
        body: &str,
    ) -> Result<(u16, String), BrokerError> {
        let _serial = self.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // WO-010's reconstruction property extends to third-party surface on
        // day one: `app` is a span field, so a trace through a plugin route
        // names WHOSE code ran, not just that guest code ran.
        let span = crate::telemetry::Span::begin("route_dispatch")
            .field("app", app)
            .field("path", path);

        // A FRESH store per call: the door is installed into this store only,
        // so authority cannot persist between requests even by accident.
        let mut store = Store::new(
            &self.engine,
            RouteState {
                wasi: WasiCtxBuilder::new().build(),
                table: ResourceTable::new(),
                limits: StoreLimitsBuilder::new().memory_size(MEM_CAP).build(),
                door: Some((Arc::clone(&self.broker), caller.clone())),
            },
        );
        store.limiter(|s| &mut s.limits);
        store.set_epoch_deadline(CALL_DEADLINE_TICKS);
        store
            .set_fuel(CALL_FUEL)
            .map_err(|e| BrokerError::Db { detail: format!("fuel: {e}") })?;

        let plugin = self.pre.instantiate(&mut store).map_err(|e| BrokerError::HookRejected {
            stage: "route-instantiate".into(),
            message: e.to_string(),
        })?;

        let req = exports::frust::plugin::routes::Request {
            path: path.to_string(),
            query: query.to_string(),
            body: body.to_string(),
        };
        let out = plugin
            .frust_plugin_routes()
            .call_handle(&mut store, &req)
            .map_err(|trap| BrokerError::HookRejected {
                stage: "route".into(),
                message: trap.to_string(),
            })?;

        crate::telemetry::observe_ms(
            "frust_route_duration_ms",
            &[("app", app), ("path", path), ("tenant", &crate::telemetry::current_tenant())],
            span.elapsed_ms(),
        );
        span.ok();
        Ok((out.status, out.body))
    }
}
