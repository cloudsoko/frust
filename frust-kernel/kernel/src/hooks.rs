//! Module 4: the in-process hook dispatcher. Folds the WO-002 hook-runner
//! process into the kernel â€” same wasmtime host design (pooled instance per
//! component, engine-global epoch ticker, per-call deadline, self-healing on
//! trap), now behind the broker's `HookDispatch` trait so `db_write` is
//! unchanged. Three processes become two.
//!
//! Doc travels as ADR-006's dynamic envelope; the toy `{id,status,total}`
//! shape is gone. Both hook classes (compiled plugin, Tier-2 script) run on
//! one validate, chained.

use std::sync::Mutex;
use std::time::Duration;

use wasmtime::component::{Component, Linker, ResourceTable};
use std::collections::HashMap;
use crate::db::Db;
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::broker::HookDispatch;
use crate::contract::{BrokerError, Value};

wasmtime::component::bindgen!({ path: "../wit", world: "plugin" });

use exports::frust::plugin::hooks::{Entry as WitEntry, Value as WitValue};

const EPOCH_TICK_MS: u64 = 10;
/// 500 ms wall-clock budget per hook call (50 ticks).
const CALL_DEADLINE_TICKS: u64 = 50;
/// Guest memory cap per hook instance.
const MEM_CAP: usize = 128 << 20;

/// WO-013 phase 2: fuel is the ACCOUNTING truth for hook compute â€” wall-time
/// conflates a slow guest with a compute-heavy one, fuel does not. The epoch
/// deadline above stays as the wall-clock backstop (ADR-005/ADR-007: the
/// allocation bomb ground for 10.4 s before the memory cap caught it, so
/// wall-clock bounding is not optional).
///
/// Per-call allowance, refilled before every call. Generous: this bounds a
/// runaway, it does not shape normal work (the spike's warm call is ~56 Âµs;
/// this is millions of instructions).
const CALL_FUEL: u64 = 2_000_000_000;

struct State {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

/// The hook context has NO DOOR, and says so.
///
/// A hook already receives the document it is meant to act on; it does not
/// need to read back. Refusing explicitly beats two worse alternatives:
/// silently returning empty rows (which teaches plugin authors that reads
/// "sometimes don't work"), and quietly binding some ambient authority
/// (which is how a second permission path gets born).
impl frust::plugin::db_api::Host for State {
    fn db_read(
        &mut self,
        _doctype: String,
        _filter: String,
        _fields: Vec<String>,
    ) -> Result<String, String> {
        Err("FRUST:E_NO_DOOR: db-read is not available from a hook".into())
    }
}

impl frust::plugin::host_api::Host for State {
    fn log(&mut self, msg: String) {
        // the guest's log verb: a structured line under the CURRENT TRACE â€”
        // plugin/script output is part of the request's story (REQ-6.4.1)
        crate::telemetry::emit(
            crate::telemetry::Level::Info,
            "hook_log",
            &[("msg", serde_json::json!(msg))],
        );
    }
}

// â”€â”€ contract Value <-> WIT value â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn to_wit(v: &Value) -> WitValue {
    match v {
        Value::Null => WitValue::NullV,
        Value::Bool(b) => WitValue::BoolV(*b),
        Value::Int(i) => WitValue::IntV(*i),
        Value::Float(x) => WitValue::FloatV(*x),
        Value::Decimal(s) => WitValue::DecimalV(s.clone()),
        Value::Text(s) => WitValue::TextV(s.clone()),
        Value::Datetime(s) => WitValue::DatetimeV(s.clone()),
        Value::Duration(s) => WitValue::DurationV(s.clone()),
        Value::RecordId(s) => WitValue::RecordIdV(s.clone()),
        // WIT has no recursive types: nested list/object as JSON text
        Value::List(_) | Value::Object(_) => {
            WitValue::CompoundV(serde_json::to_string(&contract_to_json(v)).unwrap_or_default())
        }
    }
}

fn from_wit(v: &WitValue) -> Value {
    match v {
        WitValue::NullV => Value::Null,
        WitValue::BoolV(b) => Value::Bool(*b),
        WitValue::IntV(i) => Value::Int(*i),
        WitValue::FloatV(x) => Value::Float(*x),
        WitValue::DecimalV(s) => Value::Decimal(s.clone()),
        WitValue::TextV(s) => Value::Text(s.clone()),
        WitValue::DatetimeV(s) => Value::Datetime(s.clone()),
        WitValue::DurationV(s) => Value::Duration(s.clone()),
        WitValue::RecordIdV(s) => Value::RecordId(s.clone()),
        WitValue::CompoundV(raw) => json_to_contract(
            &serde_json::from_str(raw).unwrap_or(serde_json::Value::Null),
        ),
    }
}

fn contract_to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::json!(b),
        Value::Int(i) => serde_json::json!(i),
        Value::Float(x) => serde_json::json!(x),
        // decimal/datetime/etc keep their string form inside compounds
        Value::Decimal(s) | Value::Text(s) | Value::Datetime(s) | Value::Duration(s)
        | Value::RecordId(s) => serde_json::json!(s),
        Value::List(items) => serde_json::Value::Array(items.iter().map(contract_to_json).collect()),
        Value::Object(entries) => serde_json::Value::Object(
            entries.iter().map(|(k, v)| (k.clone(), contract_to_json(v))).collect(),
        ),
    }
}

fn json_to_contract(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            n.as_i64().map(Value::Int).unwrap_or_else(|| Value::Float(n.as_f64().unwrap_or(0.0)))
        }
        serde_json::Value::String(s) => Value::Text(s.clone()),
        serde_json::Value::Array(a) => Value::List(a.iter().map(json_to_contract).collect()),
        serde_json::Value::Object(o) => {
            Value::Object(o.iter().map(|(k, v)| (k.clone(), json_to_contract(v))).collect())
        }
    }
}

fn doc_to_wit(doc: &[(String, Value)]) -> Vec<WitEntry> {
    doc.iter().map(|(k, v)| WitEntry { key: k.clone(), val: to_wit(v) }).collect()
}

fn doc_from_wit(entries: Vec<WitEntry>) -> Vec<(String, Value)> {
    entries.into_iter().map(|e| (e.key, from_wit(&e.val))).collect()
}

// â”€â”€ a pooled, self-healing hook instance â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

struct HookInstance {
    engine: Engine,
    pre: PluginPre<State>,
    live: Option<(Store<State>, Plugin)>,
    /// The Tier-2 script this instance runs, if any. `None` means the engine
    /// falls back to its built-in default.
    script: Option<String>,
}

impl HookInstance {
    fn new(engine: &Engine, pre: PluginPre<State>) -> Self {
        Self { engine: engine.clone(), pre, live: None, script: None }
    }

    fn ensure(&mut self) -> Result<(), BrokerError> {
        if self.live.is_none() {
            let mut store = new_store(&self.engine, self.script.as_deref());
            store.set_epoch_deadline(CALL_DEADLINE_TICKS);
            store.set_fuel(CALL_FUEL).map_err(|e| BrokerError::HookRejected {
                stage: "instantiate".into(),
                message: format!("fuel: {e}"),
            })?;
            let plugin = self
                .pre
                .instantiate(&mut store)
                .map_err(|e| BrokerError::HookRejected { stage: "instantiate".into(), message: e.to_string() })?;
            self.live = Some((store, plugin));
        }
        Ok(())
    }

    /// Runs one hook call and reports the FUEL it burned (the accounting
    /// truth) alongside the result.
    fn validate(&mut self, doc: &[(String, Value)]) -> Result<(Vec<(String, Value)>, u64), BrokerError> {
        self.ensure()?;
        let (store, plugin) = self.live.as_mut().unwrap();
        store.set_epoch_deadline(CALL_DEADLINE_TICKS);
        // refill to a known mark so consumption is this call's, not history's
        let _ = store.set_fuel(CALL_FUEL);
        let wit_doc = doc_to_wit(doc);
        let result = plugin.frust_plugin_hooks().call_validate(&mut *store, &wit_doc);
        let burned = store.get_fuel().map_or(0, |left| CALL_FUEL.saturating_sub(left));
        match result {
            Ok(Ok(out)) => Ok((doc_from_wit(out), burned)),
            Ok(Err(reject)) => Err(BrokerError::HookRejected { stage: "hook".into(), message: reject }),
            Err(trap) => {
                // deadline/memory/fuel/panic: drop the poisoned instance so
                // the next call rebuilds â€” the kernel never dies with a guest
                self.live = None;
                Err(BrokerError::HookRejected { stage: "trap".into(), message: trap.to_string() })
            }
        }
    }
}

/// Builds the guest's world. The environment is **constructed, never
/// inherited**: `WasiCtxBuilder::new()` starts empty, and at most the single
/// `FRUST_SCRIPT` variable is added. Inheriting the kernel's environment would
/// hand a sandboxed guest every secret the process holds â€” the emptiness here
/// is the ADR-005 capability posture, not an oversight.
fn new_store(engine: &Engine, script: Option<&str>) -> Store<State> {
    let mut wasi = WasiCtxBuilder::new();
    if let Some(src) = script {
        wasi.env("FRUST_SCRIPT", src);
    }
    let mut store = Store::new(
        engine,
        State {
            wasi: wasi.build(),
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new().memory_size(MEM_CAP).build(),
        },
    );
    store.limiter(|s| &mut s.limits);
    store
}

/// The in-process dispatcher: compiled plugin then Tier-2 script, chained,
/// on one validate. Replaces `ExternalHookRunner` with zero broker changes.
pub struct WasmHooks {
    plugin: Mutex<HookInstance>,
    script: Mutex<HookInstance>,
    /// Set by `with_script_source`; `None` keeps the pre-WO-019 behaviour of
    /// running the engine's built-in default.
    scripts: Option<ScriptSource>,
    /// Kept so pooled per-DocType instances can be built lazily.
    engine: Engine,
    script_pre: PluginPre<State>,
}

impl WasmHooks {
    /// Load both components from the artifacts dir. One engine, one epoch
    /// ticker for the process (ADR-005 finding: deadlines are engine-global).
    pub fn load(artifacts_dir: &str) -> Result<Self, BrokerError> {
        Self::load_inner(artifacts_dir, None)
    }

    fn load_inner(artifacts_dir: &str, script: Option<String>) -> Result<Self, BrokerError> {
        let mut config = Config::new();
        config.epoch_interruption(true);
        // fuel metering: the per-tenant compute accounting (WO-013 phase 2)
        config.consume_fuel(true);
        let engine = Engine::new(&config)
            .map_err(|e| BrokerError::Db { detail: format!("wasm engine: {e}") })?;

        {
            let engine = engine.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(EPOCH_TICK_MS));
                engine.increment_epoch();
            });
        }

        let mut linker: Linker<State> = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|e| BrokerError::Db { detail: format!("wasi link: {e}") })?;
        Plugin::add_to_linker::<State, wasmtime::component::HasSelf<State>>(&mut linker, |s| s)
            .map_err(|e| BrokerError::Db { detail: format!("host link: {e}") })?;

        let load = |name: &str| -> Result<PluginPre<State>, BrokerError> {
            let path = format!("{artifacts_dir}/{name}");
            let component = Component::from_file(&engine, &path)
                .map_err(|e| BrokerError::Db { detail: format!("load {path}: {e}") })?;
            let pre = linker
                .instantiate_pre(&component)
                .map_err(|e| BrokerError::Db { detail: format!("pre {path}: {e}") })?;
            PluginPre::new(pre).map_err(|e| BrokerError::Db { detail: format!("bind {path}: {e}") })
        };

        let script_pre = load("script_engine.wasm")?;
        let mut script_instance = HookInstance::new(&engine, script_pre.clone());
        script_instance.script = script;

        Ok(Self {
            plugin: Mutex::new(HookInstance::new(&engine, load("plugin_demo.wasm")?)),
            script: Mutex::new(script_instance),
            scripts: None,
            engine: engine.clone(),
            script_pre,
        })
    }

    /// Load with an explicit Tier-2 script instead of the engine's built-in
    /// default. The kernel-side counterpart of the browser host's `_setEnv`:
    /// one variable, into an otherwise empty world.
    pub fn load_with_script(artifacts_dir: &str, script: &str) -> Result<Self, BrokerError> {
        Self::load_inner(artifacts_dir, Some(script.to_string()))
    }

    /// WO-019 criterion 6: **per-DocType server scripts, delivered.**
    ///
    /// Attach the source of truth for server scripts. Until this exists, every
    /// server-side write runs the engine's built-in default â€” the WO-017
    /// item-3 finding, and the hole this closes: ADR-007's "scripts are data,
    /// live-mutable" becomes true server-side, not merely proven in principle.
    ///
    /// **Delivery is by seam, never by env inheritance.** The guest's world
    /// stays empty apart from the single `FRUST_SCRIPT` variable the host
    /// chooses to put there. Inheriting the kernel's environment would hand a
    /// sandboxed guest every secret the process holds; that posture is the
    /// point, and this does not weaken it.
    pub fn with_script_source(mut self, db: Db) -> Self {
        // The generation handle is resolved ONCE here, never on the write path
        // — the same rule `Db` follows for its agent and its root credential
        // (WO-041/WO-044): a registry lock per query is not a cache, it is a
        // different bottleneck.
        let gens = crate::tenant_gen::for_tenant(db.tenant_id());
        self.scripts = Some(ScriptSource {
            db,
            pool: Mutex::new(HashMap::new()),
            script_cache: Mutex::new(HashMap::new()),
            gens,
        });
        self
    }
}

/// Per-DocType script instances, pooled per ADR-007's `(tenant, script-set)`.
///
/// Pooling matters because building a Boa context is not free and a hot write
/// path would otherwise rebuild one per document. Keyed by
/// `(tenant, doctype)`, and the CACHED SCRIPT TEXT IS COMPARED on every call â€”
/// so editing a script through the Desk takes effect on the next write, with
/// no restart. A pool that ignored the text would make scripts data in name
/// and configuration in practice.
struct ScriptSource {
    db: Db,
    /// **WO-050: keyed `(tenant, doctype, app)`.** It was `(tenant, doctype)`,
    /// which held exactly one instance per DocType — the shape WO-049 proved
    /// turns a second contributor into a silent replacement of the first. The
    /// app in the key is what lets the owner's instance and an extension's
    /// coexist instead of evicting each other.
    pool: Mutex<HashMap<(String, String, String, crate::contract::HookClass), (String, HookInstance)>>,
    /// **WO-048: the script text, generation-cached.**
    ///
    /// WO-047's trace census found this module's `load_server_script` to be the
    /// ONE root round trip left on the steady-state write path — issued on
    /// every validate, including for doctypes that declare no script at all,
    /// because the query is how you find that out. `None` is cached as
    /// deliberately as `Some`: the scriptless case is the common one and it was
    /// paying full price.
    ///
    /// **The generation is the meta generation** — a server script is a field
    /// on the `doctype` record, so every site that already invalidates DocType
    /// metadata invalidates this too, and no new bump site is introduced. That
    /// is what keeps ADR-007's live-mutability true: the invalidation this
    /// cache relies on is the invalidation the doctype cache already relies on,
    /// so the two cannot drift apart.
    script_cache: Mutex<HashMap<(String, String), (u64, crate::sync::HookPlan)>>,
    /// This tenant's generation counters, resolved once at construction.
    gens: crate::tenant_gen::TenantGenerations,
}

impl ScriptSource {
    /// Every app's script for `doctype`, owner first, from cache when the
    /// generation still matches (WO-048's machinery, now carrying a list).
    fn scripts_for(&self, doctype: &str) -> Result<crate::sync::HookPlan, BrokerError> {
        let gen = self.gens.meta.load(std::sync::atomic::Ordering::Acquire);
        let key = (self.db.tenant_id().to_string(), doctype.to_string());
        if let Some((cached_gen, plan)) = self
            .script_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
        {
            if *cached_gen == gen {
                return Ok(plan.clone());
            }
        }
        let plan = crate::sync::load_server_scripts(&self.db, doctype)?;
        self.script_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, (gen, plan.clone()));
        Ok(plan)
    }
}

impl WasmHooks {
    fn dispatch_one(
        &self,
        runtime: &'static str,
        instance: &Mutex<HookInstance>,
        doc: &[(String, Value)],
    ) -> Result<Vec<(String, Value)>, BrokerError> {
        let span = crate::telemetry::Span::begin("hook_dispatch").field("runtime", runtime);
        let result = instance.lock().unwrap().validate(doc);
        let tenant = crate::telemetry::current_tenant();
        crate::telemetry::observe_ms(
            "frust_hook_duration_ms",
            &[("runtime", runtime), ("tenant", &tenant)],
            span.elapsed_ms(),
        );
        // fuel is the per-tenant compute truth (WO-013 phase 2): wall-time
        // says "slow", fuel says "expensive" â€” quotas need the second one
        if let Ok((_, fuel)) = &result {
            crate::telemetry::inc(
                "frust_hook_fuel_total",
                &[("runtime", runtime), ("tenant", &tenant)],
                *fuel,
            );
        }
        match &result {
            Ok(_) => span.ok(),
            Err(e) => span.err(e),
        }
        result.map(|(doc, _)| doc)
    }
}

impl WasmHooks {
    /// The script for this DocType, from metadata, pooled.
    ///
    /// Returns `Ok(None)` when the DocType declares no server script â€” which
    /// must mean *no script runs*, not *the default runs*. A DocType silently
    /// inheriting someone else's validation is the WO-017 finding in a new
    /// costume.
    /// **WO-050: the owner-first chain.**
    ///
    /// Every app that contributes a `validate` to this DocType runs, in order,
    /// each seeing the previous one's output. The owner is first and cannot be
    /// displaced — WO-049 measured the alternative: with one slot, a second app
    /// silently *replaced* the owner's hook and its invariant stopped running
    /// with no error and no trace. That is P-2.2, and the ordering here plus the
    /// per-app pool key is what makes it unreachable rather than merely refused.
    ///
    /// An extension may REJECT (ADR-015's ruled veto): its `Err` fails the write
    /// and names the rejecting app. It may not skip, reorder or replace anyone.
    fn dispatch_doctype_script(
        &self,
        class: crate::contract::HookClass,
        doctype: &str,
        doc: &[(String, Value)],
    ) -> Result<Option<Vec<(String, Value)>>, BrokerError> {
        let Some(src) = &self.scripts else { return Ok(None) };
        let plan = src.scripts_for(doctype)?;
        // **WO-053: the class selects who runs.** A script subscribes to one
        // event; the others are not its business. Filtering here rather than
        // inside the loop keeps the empty case cheap — the overwhelmingly
        // common shape is a doctype with one `validate` and nothing else, and
        // it must not pay for a vocabulary it does not use.
        let entries: Vec<&crate::sync::ScriptEntry> = plan.entries.iter().filter(|e| e.hook == class).collect();
        if entries.is_empty() {
            return Ok(None);
        }

        let tenant = src.db.tenant_id().to_string();
        let mut current = doc.to_vec();
        for entry in entries {
            // `app` is part of the KEY, so the owner's instance and each
            // extension's are separate pooled contexts — no eviction, no shared
            // globals between apps.
            let app_key = entry.app.clone().unwrap_or_default();
            // the CLASS is part of the key: one app may subscribe to several
            // events on one doctype, and each is different script text
            let key = (tenant.clone(), doctype.to_string(), app_key.clone(), class);
            let out = {
                let mut pool = src.pool.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let slot = pool.entry(key).or_insert_with(|| {
                    let mut inst = HookInstance::new(&self.engine, self.script_pre.clone());
                    inst.script = Some(entry.script.clone());
                    (entry.script.clone(), inst)
                });
                // live-mutable: an edited script replaces its pooled instance
                if slot.0 != entry.script {
                    let mut inst = HookInstance::new(&self.engine, self.script_pre.clone());
                    inst.script = Some(entry.script.clone());
                    *slot = (entry.script.clone(), inst);
                }

                // **WO-050 criterion 4: attribution.** "Which app changed this
                // behaviour" is a log field, not an archaeology exercise —
                // P-2.2's complaint answered where it is checkable.
                let span = crate::telemetry::Span::begin("hook_dispatch")
                    .field("runtime", "server_script")
                    .field("doctype", doctype)
                    .field("hook", class.wire())
                    .field("app", if app_key.is_empty() { "-".to_string() } else { app_key.clone() })
                    .field("owner", entry.is_owner);
                let result = slot.1.validate(&current);
                crate::telemetry::observe_ms(
                    "frust_hook_duration_ms",
                    &[("runtime", "server_script"), ("tenant", &crate::telemetry::current_tenant())],
                    span.elapsed_ms(),
                );
                match &result {
                    Ok(_) => span.ok(),
                    Err(e) => span.err(e),
                }
                result
            };

            match out {
                Ok((next, _fuel)) => {
                    // **Criterion 6: undeclared writes are LOUD.** WO-049 lost
                    // an hour to this: an app wrote a field its manifest never
                    // declared, WO-009's envelope filter dropped it in silence,
                    // and the run looked exactly like the hook never executing.
                    // Declare-or-lose-your-data with no error is the silent-wrong
                    // class; refusing by name is the house answer.
                    //
                    // Checked per app, right after its own hook returns, because
                    // that is the only moment the culprit is still known.
                    if !plan.declared.is_empty() {
                        if let Some((bad, _)) = next.iter().find(|(k, _)| {
                            !plan.declared.iter().any(|d| d == k)
                                && k != "id"
                                && k != "docstatus"
                                && !current.iter().any(|(ck, _)| ck == k)
                        }) {
                            let who = entry.app.clone().unwrap_or_else(|| "the doctype's own script".into());
                            return Err(BrokerError::HookRejected {
                                stage: "envelope".into(),
                                message: format!(
                                    "FRUST:E_FIELD_UNDECLARED: '{who}' wrote field '{bad}' on                                      '{doctype}', which no app declares. Declare it in the                                      manifest (extensions must namespace their fields) — it                                      would otherwise be dropped without a word."
                                ),
                            });
                        }
                    }
                    current = next;
                }
                // **Criterion 5: the veto, typed and NAMED.** A rejection that
                // did not say which app rejected would be the archaeology this
                // whole mechanism exists to end.
                Err(BrokerError::HookRejected { stage, message }) => {
                    let who = entry
                        .app
                        .clone()
                        .map_or_else(|| "the doctype's own script".to_string(), |a| format!("app '{a}'"));
                    let role = if entry.is_owner { "owner" } else { "extension" };
                    return Err(BrokerError::HookRejected {
                        stage,
                        message: format!("{message} (rejected by the {role}, {who})"),
                    });
                }
                Err(e) => return Err(e),
            }
        }
        Ok(Some(current))
    }
}

impl HookDispatch for WasmHooks {
    fn validate(&self, doctype: &str, doc: &[(String, Value)]) -> Result<Vec<(String, Value)>, BrokerError> {
        let after_plugin = self.dispatch_one("plugin", &self.plugin, doc)?;
        // A DocType's own server script REPLACES the built-in default rather
        // than running after it â€” two scripts silently chained is a debugging
        // nightmare nobody asked for. With a script source attached and no
        // script declared, nothing runs.
        match self.dispatch_doctype_script(crate::contract::HookClass::Validate, doctype, &after_plugin)? {
            Some(out) => return Ok(out),
            None if self.scripts.is_some() => return Ok(after_plugin),
            None => {}
        }
        self.dispatch_one("script", &self.script, &after_plugin)
    }

    /// **WO-053: the rest of REQ-2.2.1.**
    ///
    /// The resident plugin is deliberately NOT consulted here. It is built
    /// against `world plugin`, which exports `hooks` and nothing else — and
    /// ADR-006 edge-1, measured in this WO, says a component receives exactly
    /// the events it exports. Handing it a lifecycle event it never declared
    /// would be the host inventing a subscription on the guest's behalf.
    ///
    /// Server scripts need no such thing: the engine is a *text runner*, so
    /// which script runs for which event is a host-side routing decision, and
    /// the vocabulary reaches scripts without the engine changing world at all.
    fn fire(
        &self,
        class: crate::contract::HookClass,
        doctype: &str,
        doc: &[(String, Value)],
    ) -> Result<Vec<(String, Value)>, BrokerError> {
        let out = self.dispatch_doctype_script(class, doctype, doc)?;
        match (out, class.may_mutate()) {
            // a mutating class takes the document back
            (Some(next), true) => Ok(next),
            // **a non-mutating class does not.** on_submit/on_cancel fire on a
            // docstatus edge where ADR-009's lattice owns the value; they may
            // refuse (their Err propagates and blocks the write) but what they
            // hand back is discarded HOST-side, so the contract holds even for
            // a script that ignores it.
            (Some(_), false) | (None, _) => Ok(doc.to_vec()),
        }
    }
}

