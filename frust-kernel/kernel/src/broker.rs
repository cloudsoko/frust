//! The broker (WO-005 module 1): one implementation of the ADR-006 verbs
//! serving Desk, REST, and plugins. Everything else consumes this.

use serde::Deserialize;

use crate::contract::*;
use crate::db::Db;
use crate::surql;

/// The acting principal. Identity is captured here; authority is always
/// re-derived by executing under this principal's DB session (ADR-006 edge 4).
#[derive(Debug, Clone)]
pub struct Caller {
    pub user: String,
    pub pass: String,
    pub role: String,
}

/// Hook chain for the cycle trap (ADR-006 edge 6): every db-write from
/// inside a hook carries it. `(record-id, hook-class)` re-entry traps
/// immediately; depth 8 is the fan-out backstop.
#[derive(Debug, Clone, Default)]
pub struct HookChain(Vec<(String, HookClass)>);

pub const MAX_HOOK_DEPTH: usize = 8;

impl HookChain {
    pub fn push(&self, record: &str, class: HookClass) -> Result<Self, BrokerError> {
        if self.0.iter().any(|(r, c)| r == record && *c == class) {
            return Err(BrokerError::HookCycle {
                record: record.to_string(),
                hook: format!("{class:?}"),
            });
        }
        if self.0.len() >= MAX_HOOK_DEPTH {
            return Err(BrokerError::HookDepthExceeded { max: MAX_HOOK_DEPTH });
        }
        let mut next = self.clone();
        next.0.push((record.to_string(), class));
        Ok(next)
    }

    pub fn depth(&self) -> usize {
        self.0.len()
    }
}

/// The fold-in seam: hooks fire through this trait over ADR-006's dynamic
/// doc envelope. Module 1 wired the external hook-runner; module 4 swaps in
/// in-process wasmtime (`hooks::WasmHooks`) with the broker unchanged.
pub trait HookDispatch: Send + Sync {
    fn validate(&self, doctype: &str, doc: &[(String, Value)]) -> Result<Vec<(String, Value)>, BrokerError>;

    /// WO-053: fire one lifecycle class. Returns the (possibly mutated)
    /// document for a mutating class, the input unchanged otherwise; `Err`
    /// rejects and blocks the write.
    ///
    /// Defaulted to a no-op so the many test doubles that implement only
    /// `validate` keep compiling and simply deliver nothing — the honest
    /// behaviour for a dispatcher that predates the vocabulary.
    fn fire(
        &self,
        _class: crate::contract::HookClass,
        _doctype: &str,
        doc: &[(String, Value)],
    ) -> Result<Vec<(String, Value)>, BrokerError> {
        Ok(doc.to_vec())
    }
}

/// External hook-runner (the WO-002 composition process on :8787). Retained
/// so the composition still runs; it speaks the toy `{id,status,total}` JSON,
/// so this adapter marshals the dynamic doc down to it and back â€” the toy
/// shape now lives ONLY at this legacy boundary, not in the broker.
pub struct ExternalHookRunner {
    pub endpoint: String,
}

impl HookDispatch for ExternalHookRunner {
    fn validate(&self, _doctype: &str, doc: &[(String, Value)]) -> Result<Vec<(String, Value)>, BrokerError> {
        let get = |k: &str| doc.iter().find(|(n, _)| n == k).map(|(_, v)| v);
        let num = |v: Option<&Value>| match v {
            Some(Value::Float(x)) => *x,
            Some(Value::Int(i)) => *i as f64,
            Some(Value::Decimal(s)) => s.parse().unwrap_or(0.0),
            _ => 0.0,
        };
        let text = |v: Option<&Value>| match v {
            Some(Value::Text(s) | Value::RecordId(s)) => s.clone(),
            _ => String::new(),
        };
        let hook_doc = serde_json::json!({
            "id": text(get("id")),
            "status": if text(get("status")).is_empty() { "Draft".to_string() } else { text(get("status")) },
            "total": num(get("total")),
        });
        // WO-041: through the shared per-endpoint agent, like every other
        // HTTP caller in the kernel. This is the legacy WO-002 hook runner and
        // not on the hot path, but "one agent per endpoint" is a process-wide
        // rule — an exception here would be a port leak waiting for the day
        // someone puts it on a hot path.
        let mut res = crate::db::agent_for(&self.endpoint)
            .post(format!("{}/validate", self.endpoint))
            .send(hook_doc.to_string())
            .map_err(|e| BrokerError::Db { detail: format!("hook-runner transport: {e}") })?;
        let body = res
            .body_mut()
            .read_to_string()
            .map_err(|e| BrokerError::Db { detail: format!("hook-runner read: {e}") })?;
        let verdict: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| BrokerError::Db { detail: format!("hook-runner parse: {e}") })?;
        if let Some(err) = verdict.get("err") {
            return Err(BrokerError::HookRejected {
                stage: verdict.get("stage").and_then(|s| s.as_str()).unwrap_or("?").to_string(),
                message: err.to_string(),
            });
        }
        // carry the toy verdict's mutations back onto the full doc
        let mut out = doc.to_vec();
        let ok = &verdict["ok"];
        if let Some(s) = ok.get("status").and_then(|v| v.as_str()) {
            out.retain(|(k, _)| k != "status");
            out.push(("status".into(), Value::Text(s.to_string())));
        }
        if let Some(t) = ok.get("total").and_then(serde_json::Value::as_f64) {
            if get("total").is_some() {
                out.retain(|(k, _)| k != "total");
                out.push(("total".into(), Value::Float(t)));
            }
        }
        Ok(out)
    }
}

// â”€â”€ DocType metadata (runtime) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[derive(Debug, Clone, Deserialize)]
pub struct DocTypeMeta {
    pub name: String,
    // display-only; absent for programmatically-created doctypes
    #[serde(default)]
    #[allow(dead_code)]
    pub label: String,
    pub fields: Vec<FieldMeta>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FieldMeta {
    pub fieldname: String,
    /// WO-028: the broker needs the declared type to re-type a record read
    /// back for a hook — SurrealDB returns a decimal as a JSON string, so a
    /// `Currency` field would otherwise re-enter the hook as text.
    #[serde(default)]
    pub fieldtype: String,
    #[serde(default)]
    pub perm_role: Option<String>,
}

/// The docstatus carried by a document envelope, if it names one.
fn docstatus_of(doc: &[(String, Value)]) -> Option<i64> {
    doc.iter().find(|(k, _)| k == "docstatus").and_then(|(_, v)| match v {
        Value::Int(i) => Some(*i),
        _ => None,
    })
}

/// Engine-level fields present on every synced table. `docstatus` is the
/// lifecycle floor â€” readable everywhere it exists (WO-009: the Desk renders
/// state it cannot see otherwise).
const ENGINE_FIELDS: [&str; 4] = ["id", "status", "owner", "docstatus"];

// â”€â”€ The broker â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// A stored JSON value back into the contract's `Value`. Deliberately
/// conservative: strings stay text unless the DocType says otherwise (see
/// `record_for_hook`), because guessing a decimal from a numeric-looking string
/// would retype ordinary `Data` fields.
fn json_to_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(Value::Int)
            .unwrap_or_else(|| Value::Float(n.as_f64().unwrap_or(0.0))),
        serde_json::Value::String(s) => Value::Text(s.clone()),
        serde_json::Value::Array(a) => Value::List(a.iter().map(json_to_value).collect()),
        serde_json::Value::Object(o) => {
            Value::Object(o.iter().map(|(k, v)| (k.clone(), json_to_value(v))).collect())
        }
    }
}

/// Are two field values the SAME VALUE (not merely the same representation)?
///
/// WO-028's subtle half. A hook that reads a field and hands it straight back
/// must not register as a change, or every hooked update rewrites the whole
/// document and the lost-update race reopens. The trap is decimals: money
/// crosses the hook boundary as a string (REQ-6.2.1), and SurrealDB normalises
/// trailing zeros — so `50.98` read back can differ *textually* from `50.980`
/// while being the identical amount. That is WO-016's
/// representation-versus-value lesson landing in a new place, and comparing
/// representations here would silently widen every write.
fn same_value(a: &Value, b: &Value) -> bool {
    use crate::decimal::Decimal;
    let as_num = |v: &Value| -> Option<Decimal> {
        match v {
            Value::Decimal(s) | Value::Text(s) => Decimal::parse(s),
            Value::Int(i) => Decimal::parse(&i.to_string()),
            Value::Float(f) => Decimal::parse(&f.to_string()),
            _ => None,
        }
    };
    match (as_num(a), as_num(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Invalidate **one tenant's** cached DocTypes. Call this from any path that
/// changes DocType metadata — `POST /doctype`, app install/update/uninstall,
/// and the schema sync. Cheap; forgetting it is the bug this design is shaped
/// to make loud rather than silent, because the next read after a bump always
/// goes to the database.
///
/// **WO-040 Chunk B: the generation is per tenant.** It was one process-global
/// counter, so a schema sync in any tenant cold-started every other tenant's
/// metadata cache — correct, but a noisy-neighbour vector on a WO-026 cache.
pub fn invalidate_meta(tenant: &str) {
    crate::tenant_gen::invalidate_meta(tenant);
}

pub struct Broker {
    pub db: Db,
    /// **WO-040 criterion 2:** shared, not owned. One wasmtime engine (and its
    /// one epoch ticker) serves EVERY tenant — the WO-039 finding that an owned
    /// `Box` would have made N tenants cost N engines + N threads, multiplying
    /// the 60 MB idle footprint (P-1.4) by tenant count. Safe because the script
    /// pool inside `WasmHooks` is already keyed `(tenant, doctype)`.
    pub hooks: std::sync::Arc<dyn HookDispatch>,
    /// **WO-040 Chunk B:** this tenant's cache generations, resolved ONCE
    /// here so the per-request path stays a single atomic load rather than a
    /// registry lookup — trading a cross-tenant coupling for a cross-thread
    /// one would be the worse deal at 16 workers.
    pub gens: crate::tenant_gen::TenantGenerations,
    /// (generation, meta) per doctype — see `load_doctype`.
    meta_cache: std::sync::Mutex<std::collections::HashMap<String, (u64, DocTypeMeta)>>,
    /// **WO-043:** (generation, rules) per doctype, on the SAME generation
    /// counter as the metadata cache — a notification is metadata, and the
    /// route that writes one bumps the counter, so a new rule fires on the very
    /// next write with no restart (criterion 1) while a write to a doctype with
    /// no rules stays a hashmap hit rather than a query. Uncached, this would
    /// have added a fourth DB round-trip to every write and re-created P-1.2 in
    /// a new place.
    notif_cache: std::sync::Mutex<std::collections::HashMap<String, (u64, std::sync::Arc<Vec<crate::mail::NotificationDef>>)>>,
    /// Every broker query carries a server-side timeout (benchmark policy).
    pub query_timeout: &'static str,
}

impl Broker {
    /// Single-tenant construction — unchanged for every existing caller; the
    /// `Box` is simply promoted to a shared handle.
    pub fn new(db: Db, hooks: Box<dyn HookDispatch>) -> Self {
        Self::with_shared_hooks(db, std::sync::Arc::from(hooks))
    }

    /// **The multi-tenant constructor (WO-040).** N tenant brokers built from
    /// ONE `Arc<WasmHooks>`: one engine, one ticker, one component load, for the
    /// whole process. This is what keeps per-tenant cost at a `Db` plus a
    /// metadata cache — kilobytes — instead of a wasm engine apiece.
    pub fn with_shared_hooks(db: Db, hooks: std::sync::Arc<dyn HookDispatch>) -> Self {
        let gens = crate::tenant_gen::for_tenant(db.tenant_id());
        Self {
            db,
            hooks,
            gens,
            query_timeout: "30s",
            meta_cache: std::sync::Mutex::new(std::collections::HashMap::new()),
            notif_cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// WO-026: the DocType metadata cache, invalidated by GENERATION.
    ///
    /// `load_doctype` ran a `SELECT` on **every** read, write and aggregate —
    /// which the WO-026 probe measured as one of three DB round trips per
    /// request, i.e. one third of the concurrency write ceiling. It is also
    /// literally P-1.2 ("metadata loaded per request"), the Frappe pain point
    /// this project criticized and then reproduced in mechanism.
    ///
    /// **Invalidation is the work, not the caching.** A cache that serves stale
    /// metadata after a sync is the silent-wrong class this project exists to
    /// refuse — and it would do it in a path that decides field permissions.
    /// So this is NOT a TTL: a global generation counter is bumped by every
    /// kernel-mediated metadata mutation, and an entry is only reused while its
    /// generation still matches. A stale read is structurally impossible rather
    /// than improbable.
    ///
    /// Out-of-band DDL (someone editing the `doctype` table directly, outside
    /// the kernel) does not bump the counter — the same caveat the boot-time
    /// meta discipline already carries, and it is why the bump lives at every
    /// kernel write site rather than being inferred.
    fn load_doctype(&self, name: &str) -> Result<DocTypeMeta, BrokerError> {
        let gen = self.gens.meta.load(std::sync::atomic::Ordering::Acquire);
        if let Some((cached_gen, meta)) = self
            .meta_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
        {
            if *cached_gen == gen {
                return Ok(meta.clone());
            }
        }
        let meta = self.load_doctype_uncached(name)?;
        self.meta_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.to_string(), (gen, meta.clone()));
        Ok(meta)
    }

    /// The current record, as the hook must see it (WO-028).
    ///
    /// Read under the CALLER'S session, deliberately — this must not become a
    /// door to rows the caller cannot see. A caller who cannot read the record
    /// gets an empty baseline, and the write that follows is refused by the
    /// same row permission with `E_WRITE_NO_ROWS` (ADR-012 Finding A), which is
    /// the answer they would have got anyway.
    fn record_for_hook(
        &self,
        caller: &Caller,
        meta: &DocTypeMeta,
        key: &str,
    ) -> Result<Vec<(String, Value)>, BrokerError> {
        let rid = surql::render_value(&Value::RecordId(format!("{}:{key}", meta.name)))?;
        let out = self.db.sql_as(&caller.user, &caller.pass, &format!("SELECT * FROM {rid};"))?;
        let Some(serde_json::Value::Object(map)) = out.as_array().and_then(|a| a.first()).cloned()
        else {
            return Ok(Vec::new());
        };
        Ok(map
            .into_iter()
            .filter(|(k, _)| k != "id")
            .map(|(k, v)| {
                // Type from the DocType, not from the JSON shape: SurrealDB
                // returns a decimal as a STRING, so a `Currency` field would
                // otherwise re-enter the hook as text and leave it as text —
                // reopening the float/representation door WO-016 closed.
                let ft = meta.fields.iter().find(|f| f.fieldname == k).map(|f| f.fieldtype.as_str());
                let val = match (ft, &v) {
                    (Some("Currency"), serde_json::Value::String(s)) => Value::Decimal(s.clone()),
                    _ => json_to_value(&v),
                };
                (k, val)
            })
            .collect())
    }

    fn load_doctype_uncached(&self, name: &str) -> Result<DocTypeMeta, BrokerError> {
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || name.is_empty() {
            return Err(BrokerError::UnknownDoctype { name: name.to_string() });
        }
        let v = self.db.sql_root(&format!("SELECT * FROM doctype WHERE name = '{name}' LIMIT 1;"))?;
        let rec = v
            .as_array()
            .and_then(|a| a.first())
            .ok_or_else(|| BrokerError::UnknownDoctype { name: name.to_string() })?;
        serde_json::from_value(rec.clone())
            .map_err(|e| BrokerError::Db { detail: format!("bad doctype meta: {e}") })
    }

    /// The field half of the permission compiler: which fields may this role
    /// see? (The row half is the DB's PERMISSIONS clause, enforced by
    /// executing under the caller's session â€” the kernel cannot bypass it.)
    fn readable_fields(meta: &DocTypeMeta, role: &str) -> Vec<String> {
        let mut allowed: Vec<String> = ENGINE_FIELDS.iter().map(|s| (*s).to_string()).collect();
        for f in &meta.fields {
            if f.perm_role.as_deref().is_none_or(|r| r == role) {
                allowed.push(f.fieldname.clone());
            }
        }
        allowed
    }

    /// Instruments one verb call: tenant scope, span, latency histogram.
    fn instrument<T>(
        &self,
        verb: &'static str,
        doctype: &str,
        run: impl FnOnce() -> Result<T, BrokerError>,
    ) -> Result<T, BrokerError> {
        let _tenant = crate::telemetry::enter_tenant(self.db.tenant_id());
        let span = crate::telemetry::Span::begin("broker_verb")
            .field("verb", verb)
            .field("doctype", doctype);
        // WO-013: the broker door admits before the verb runs. Absent policy
        // is unlimited, so this is a compare-and-return on the hot path.
        let result = match crate::fairness::admit_verb(self.db.tenant_id()) {
            Ok(()) => run(),
            Err(e) => Err(e),
        };
        let ms = span.elapsed_ms();
        crate::telemetry::observe_ms(
            "frust_verb_duration_ms",
            &[("verb", verb), ("tenant", self.db.tenant_id())],
            ms,
        );
        match &result {
            Ok(_) => span.ok(),
            Err(e) => span.err(e),
        }
        result
    }

    /// db-read: structured filters in, permission-filtered envelope out.
    pub fn db_read(
        &self,
        caller: &Caller,
        doctype: &str,
        filter: Option<&Filter>,
        fields: &[Vec<PathSegment>],
        opts: &ReadOpts,
    ) -> Result<Vec<serde_json::Value>, BrokerError> {
        self.instrument("db_read", doctype, || self.db_read_inner(caller, doctype, filter, fields, opts))
    }

    fn db_read_inner(
        &self,
        caller: &Caller,
        doctype: &str,
        filter: Option<&Filter>,
        fields: &[Vec<PathSegment>],
        opts: &ReadOpts,
    ) -> Result<Vec<serde_json::Value>, BrokerError> {
        let meta = self.load_doctype(doctype)?;
        let allowed = Self::readable_fields(&meta, &caller.role);

        // requested projection must be within the caller's readable set
        for path in fields {
            if let Some(PathSegment::Field(root)) = path.first() {
                if !allowed.iter().any(|a| a == root) {
                    return Err(BrokerError::FieldNotReadable { field: root.clone() });
                }
            }
            surql::render_path(path)?; // validates + depth-caps
        }

        let mut q = format!("SELECT * FROM {}", meta.name);
        if let Some(f) = filter {
            q.push_str(&format!(" WHERE {}", surql::render_filter(f)?));
        }
        // benchmark policy (#7432): range filter + explicit order => the
        // planner's compound-index path regresses; force the scan.
        let range = filter.map(surql::filter_has_range).unwrap_or(false);
        if let Some((path, dir)) = &opts.order_by {
            q.push_str(&format!(
                " ORDER BY {} {}",
                surql::render_path(path)?,
                match dir { SortDir::Asc => "ASC", SortDir::Desc => "DESC" }
            ));
        }
        if let Some(l) = opts.limit {
            q.push_str(&format!(" LIMIT {l}"));
        }
        if let Some(s) = opts.start {
            q.push_str(&format!(" START {s}"));
        }
        if range && opts.order_by.is_some() {
            // WITH NOINDEX sits after FROM in SurrealQL; rebuild head
            q = q.replacen(
                &format!("FROM {}", meta.name),
                &format!("FROM {} WITH NOINDEX", meta.name),
                1,
            );
        }
        q.push_str(&format!(" TIMEOUT {};", self.query_timeout));

        // row-level: the DB enforces PERMISSIONS because this runs under the
        // caller's own session
        let rows = self.db.sql_as(&caller.user, &caller.pass, &q)?;
        let rows = rows.as_array().cloned().unwrap_or_default();

        // field-level: filter the envelope down to the readable set (and to
        // the requested projection when one was given)
        let projection: Option<Vec<&str>> = if fields.is_empty() {
            None
        } else {
            Some(
                fields
                    .iter()
                    .filter_map(|p| match p.first() {
                        Some(PathSegment::Field(f)) => Some(f.as_str()),
                        _ => None,
                    })
                    .collect(),
            )
        };
        let filtered = rows
            .into_iter()
            .map(|row| {
                let serde_json::Value::Object(map) = row else { return row };
                let kept: serde_json::Map<String, serde_json::Value> = map
                    .into_iter()
                    .filter(|(k, _)| allowed.iter().any(|a| a == k))
                    .filter(|(k, _)| projection.as_ref().is_none_or(|p| p.contains(&k.as_str()) || k == "id"))
                    .collect();
                serde_json::Value::Object(kept)
            })
            .collect();
        Ok(filtered)
    }

    /// db-aggregate: closed metric surface (ADR-006 edge 5).
    pub fn db_aggregate(
        &self,
        caller: &Caller,
        doctype: &str,
        filter: Option<&Filter>,
        group_by: &[Vec<PathSegment>],
        metrics: &[(Metric, Vec<PathSegment>)],
    ) -> Result<Vec<serde_json::Value>, BrokerError> {
        self.instrument("db_aggregate", doctype, || {
            self.db_aggregate_inner(caller, doctype, filter, group_by, metrics)
        })
    }

    fn db_aggregate_inner(
        &self,
        caller: &Caller,
        doctype: &str,
        filter: Option<&Filter>,
        group_by: &[Vec<PathSegment>],
        metrics: &[(Metric, Vec<PathSegment>)],
    ) -> Result<Vec<serde_json::Value>, BrokerError> {
        let meta = self.load_doctype(doctype)?;
        let allowed = Self::readable_fields(&meta, &caller.role);
        let check = |path: &Vec<PathSegment>| -> Result<String, BrokerError> {
            if let Some(PathSegment::Field(root)) = path.first() {
                if !allowed.iter().any(|a| a == root) {
                    return Err(BrokerError::FieldNotReadable { field: root.clone() });
                }
            }
            surql::render_path(path)
        };
        let mut sel: Vec<String> = Vec::new();
        for (i, g) in group_by.iter().enumerate() {
            sel.push(format!("{} AS g{i}", check(g)?));
        }
        for (i, (m, path)) in metrics.iter().enumerate() {
            let expr = match m {
                Metric::Count => "count()".to_string(),
                Metric::Sum => format!("math::sum({})", check(path)?),
                Metric::Min => format!("math::min({})", check(path)?),
                Metric::Max => format!("math::max({})", check(path)?),
                Metric::Avg => format!("math::mean({})", check(path)?),
            };
            sel.push(format!("{expr} AS m{i}"));
        }
        let mut q = format!("SELECT {} FROM {}", sel.join(", "), meta.name);
        if filter.map(surql::filter_has_range).unwrap_or(false) {
            q.push_str(" WITH NOINDEX");
        }
        if let Some(f) = filter {
            q.push_str(&format!(" WHERE {}", surql::render_filter(f)?));
        }
        if !group_by.is_empty() {
            let gs: Vec<String> = (0..group_by.len()).map(|i| format!("g{i}")).collect();
            q.push_str(&format!(" GROUP BY {}", gs.join(", ")));
        } else {
            q.push_str(" GROUP ALL");
        }
        q.push_str(&format!(" TIMEOUT {};", self.query_timeout));
        let rows = self.db.sql_as(&caller.user, &caller.pass, &q)?;
        Ok(rows.as_array().cloned().unwrap_or_default())
    }

    /// db-write: hooks ALWAYS fire (no hook-free writes, ADR-006 edge 6);
    /// the write itself executes under the caller's session.
    pub fn db_write(
        &self,
        caller: &Caller,
        chain: &HookChain,
        op: WriteOp,
        doctype: &str,
        record: Option<&str>,
        doc: &[(String, Value)],
    ) -> Result<serde_json::Value, BrokerError> {
        self.instrument("db_write", doctype, || {
            self.db_write_inner(caller, chain, op, doctype, record, doc)
        })
    }

    /// WO-018: take a workflow action on a document (REQ-4.1.2).
    ///
    /// **The kernel judges, then writes.** The judgement is ordinary Rust in
    /// `workflow.rs`, evaluated here *before* any state or docstatus value is
    /// assembled — ADR-009 A2's "kernel logic evaluated before the kernel
    /// attempts the transition", literally.
    ///
    /// The write itself is a plain `db_write`, deliberately: hooks fire on it
    /// (both runtimes), the changefeed records it, and the trace carries it.
    /// A transition is not a special kind of mutation with its own audit
    /// story — it is a mutation, and it inherits every property writes already
    /// have.
    ///
    /// The lattice EVENT is downstream of this and does not trust it. A
    /// workflow declaring a docstatus-2 state reachable from 0 is judged
    /// *allowed* here and then refused by `FRUST:E_DOCSTATUS` — which is the
    /// backstop working, not a contradiction.
    pub fn db_transition(
        &self,
        caller: &Caller,
        chain: &HookChain,
        doctype: &str,
        record: &str,
        action: &str,
    ) -> Result<serde_json::Value, BrokerError> {
        self.instrument("db_transition", doctype, || {
            let Some(wf) = crate::sync::load_workflow(&self.db, doctype)? else {
                return Err(BrokerError::WorkflowDenied {
                    code: "FRUST:E_WORKFLOW:UNMANAGED".into(),
                    detail: format!("'{doctype}' has no workflow"),
                });
            };

            // current state, read under the CALLER's own authority — a user
            // who cannot see the document cannot transition it, and that falls
            // out of the read rather than needing its own check
            let key = format!("{doctype}:{record}");
            let rows = self.db_read(
                caller,
                doctype,
                Some(&Filter::Cmp {
                    path: vec![PathSegment::Field("id".into())],
                    op: CmpOp::Eq,
                    value: Value::RecordId(key.clone()),
                }),
                &[],
                &ReadOpts::default(),
            )?;
            let Some(doc) = rows.first() else {
                return Err(BrokerError::UnknownDoctype { name: key });
            };
            // WO-031: empty state == not-yet-entered == initial (a Desk-created
            // doc carries an empty workflow_state).
            let current = wf
                .state_or_initial(doc.get(crate::workflow::STATE_FIELD).and_then(|v| v.as_str()))
                .to_string();

            // ── the judgement, before anything is written ──
            let judged = wf.judge(&current, action, &caller.role)?;

            let span = crate::telemetry::Span::begin("workflow_transition")
                .field("workflow", wf.name.clone())
                .field("doctype", doctype)
                .field("action", judged.action.clone())
                .field("from", judged.from.clone())
                .field("to", judged.to.clone());

            // ── and now an ordinary write, with everything writes get ──
            let out = self.db_write_inner(
                caller,
                chain,
                WriteOp::Update,
                doctype,
                Some(record),
                &[
                    (crate::workflow::STATE_FIELD.to_string(), Value::Text(judged.to.clone())),
                    ("docstatus".to_string(), Value::Int(judged.docstatus)),
                ],
            );
            match &out {
                Ok(_) => span.ok(),
                Err(e) => span.err(e),
            }
            // WO-043: the transition event. The inner write above has already
            // fired `on_update` (and `on_submit`, if this action crossed
            // docstatus into 1) — because a transition IS an update, and a rule
            // watching updates would be wrong to miss one. `on_transition` is
            // the additional, workflow-aware event, and it is the only one that
            // knows the ACTION's name.
            if let Ok(stored) = &out {
                self.fire_notifications(
                    doctype,
                    stored,
                    &[crate::mail::Trigger::Transition {
                        action: &judged.action,
                        to_state: &judged.to,
                    }],
                );
            }
            out
        })
    }

    /// The transitions this caller may take on this document right now — the
    /// same data `judge` uses, so a rendered button is an allowed action.
    pub fn workflow_actions(
        &self,
        caller: &Caller,
        doctype: &str,
        record: &str,
    ) -> Result<serde_json::Value, BrokerError> {
        let Some(wf) = crate::sync::load_workflow(&self.db, doctype)? else {
            return Ok(serde_json::json!({ "workflow": null, "state": null, "actions": [] }));
        };
        let key = format!("{doctype}:{record}");
        let rows = self.db_read(
            caller,
            doctype,
            Some(&Filter::Cmp {
                path: vec![PathSegment::Field("id".into())],
                op: CmpOp::Eq,
                value: Value::RecordId(key.clone()),
            }),
            &[],
            &ReadOpts::default(),
        )?;
        let Some(doc) = rows.first() else {
            return Err(BrokerError::UnknownDoctype { name: key });
        };
        // WO-031: empty state == not-yet-entered == initial (see db_transition).
        let state = wf
            .state_or_initial(doc.get(crate::workflow::STATE_FIELD).and_then(|v| v.as_str()))
            .to_string();
        let actions: Vec<serde_json::Value> = wf
            .available(&state, &caller.role)
            .iter()
            .map(|t| serde_json::json!({ "action": t.action, "to": t.to }))
            .collect();
        Ok(serde_json::json!({ "workflow": wf.name, "state": state, "actions": actions }))
    }

    fn db_write_inner(
        &self,
        caller: &Caller,
        chain: &HookChain,
        op: WriteOp,
        doctype: &str,
        record: Option<&str>,
        doc: &[(String, Value)],
    ) -> Result<serde_json::Value, BrokerError> {
        let meta = self.load_doctype(doctype)?;
        let record_key = record.unwrap_or("new");
        let _chain = chain.push(&format!("{}:{record_key}", meta.name), HookClass::Validate)?;
        // _chain is carried into nested writes once hooks gain db-write.

        // The FULL document goes to the hooks now (ADR-006 edge 3). We inject
        // the record id so hooks that key on it still work, run validate
        // (both classes, chained by the dispatcher), then take the mutated
        // doc back verbatim â€” no field-by-field cherry-picking.
        // WO-028: make the contract above TRUE. It was not: `doc` is the
        // caller's PARTIAL payload, so on an update a hook saw only the fields
        // being written. The accounting seed's reconciliation script then did
        // `doc.lines || []` on an absent field, wrote `[]` back, and MERGE
        // faithfully destroyed the stored child rows. Any hook that derives or
        // echoes a field had the same power.
        //
        // So on an update we read the current record UNDER THE CALLER'S OWN
        // SESSION (never root — reading a record to hand to a hook must not
        // become a permission bypass; a caller who cannot see the row cannot
        // update it either, and gets the same refusal) and merge the delta
        // over it. Creates need no read: there is no prior document.
        let baseline: Vec<(String, Value)> = match (op, record) {
            (WriteOp::Update, Some(key)) => self.record_for_hook(caller, &meta, key)?,
            _ => Vec::new(),
        };
        let mut hook_doc = baseline.clone();
        for (k, v) in doc {
            match hook_doc.iter_mut().find(|(hk, _)| hk == k) {
                Some(slot) => slot.1 = v.clone(),
                None => hook_doc.push((k.clone(), v.clone())),
            }
        }
        if !hook_doc.iter().any(|(k, _)| k == "id") {
            hook_doc.push(("id".into(), Value::Text(format!("{}:{record_key}", meta.name))));
        }
        // ── WO-053: before_insert, ahead of validate and creates only ──
        //
        // Stated, not inherited: it fires only for a CREATE (there is no
        // "insert" to be before on an update), it is shown the same full
        // envelope validate gets, and it MAY MUTATE — so its output is the
        // document validate then sees. Ordering is the point: an app that
        // stamps a default in before_insert must have that default available
        // to every validate, including other apps'.
        if op == WriteOp::Create {
            let chain = chain.push(&format!("{}:{record_key}", meta.name), HookClass::BeforeInsert)?;
            let _ = &chain;
            hook_doc = self.hooks.fire(HookClass::BeforeInsert, doctype, &hook_doc)?;
        }

        // ── WO-053: the docstatus edges, BEFORE the write ──
        //
        // The edge is computed from the baseline against the document about to
        // be written — the pre-commit twin of the post-commit derivation the
        // notification path does further down. It has to be pre-commit for one
        // reason that decides the whole contract: **these hooks may REJECT**,
        // and a rejection has to prevent the transition rather than annotate it
        // after the fact.
        //
        // What that buys, and it is exactly what ADR-009 requires: a rejected
        // 0→1 leaves docstatus 0 and the lattice EVENT never runs, because the
        // WRITE never happens. The lattice stays the floor; no hook is anywhere
        // near it. Nothing here touches the EVENT — the escalation clause this
        // WO carried is not reached.
        let edge_before = docstatus_of(&baseline).unwrap_or(0);
        let edge_after = docstatus_of(&hook_doc).unwrap_or(edge_before);
        for (from_to, class) in
            [(1, HookClass::OnSubmit), (2, HookClass::OnCancel)]
        {
            if edge_before != from_to && edge_after == from_to {
                let chain = chain.push(&format!("{}:{record_key}", meta.name), class)?;
                let _ = &chain;
                // the return is DISCARDED by the dispatcher for a non-mutating
                // class; only the Err matters here
                self.hooks.fire(class, doctype, &hook_doc)?;
            }
        }

        let mut entries = self.hooks.validate(doctype, &hook_doc)?;
        // the injected id is a hook input, never a stored column
        entries.retain(|(k, _)| k != "id");
        // WO-009: hook output is bounded by the metadata envelope â€” a hook
        // cannot invent fields the doctype doesn't declare (SCHEMAFULL would
        // reject them; SCHEMALESS gets the same discipline here)
        entries.retain(|(k, _)| {
            meta.fields.iter().any(|f| &f.fieldname == k)
                || ENGINE_FIELDS.contains(&k.as_str())
                || k == "docstatus"
        });

        // WO-028, the half that matters: persist ONLY what actually changed.
        //
        // Handing hooks the full document (above) means their output is now a
        // full document too. Writing all of it back would turn every hooked
        // partial update into a whole-record write — and under the concurrent
        // loop WO-025 shipped, two disjoint partial updates to the same record
        // would then clobber each other (both read version A, both write their
        // own full merge, last writer wins, one delta silently lost). That
        // would trade one silent-data-loss for another.
        //
        // MERGE's atomic partial-write property is what made the old path safe;
        // the bug was letting the hook WIDEN the field set, not the merge. So a
        // field is persisted iff the caller explicitly wrote it, or the hook
        // actually changed it against the document it was shown. Unchanged
        // fields are never rewritten, and disjoint updates stay disjoint.
        if op == WriteOp::Update {
            entries.retain(|(k, v)| {
                doc.iter().any(|(dk, _)| dk == k)
                    || !hook_doc.iter().any(|(bk, bv)| bk == k && same_value(bv, v))
            });
        }

        let content = surql::render_value(&Value::Object(entries))?;
        let q = match (op, record) {
            (WriteOp::Create, _) => format!("CREATE {} CONTENT {content};", meta.name),
            (WriteOp::Update, Some(key)) => {
                let rid = surql::render_value(&Value::RecordId(format!("{}:{key}", meta.name)))?;
                format!("UPDATE {rid} MERGE {content};")
            }
            (WriteOp::Update, None) => {
                return Err(BrokerError::InvalidValue { detail: "update needs a record".into() })
            }
        };
        let out = self.db.sql_as(&caller.user, &caller.pass, &q)?;
        let row = out.as_array().and_then(|a| a.first()).cloned();

        // WO-018 FINDING: an UPDATE the DB refuses returns an EMPTY RESULT
        // SET, not an error. Returning `Null` here made a permission-denied
        // write look like a successful one — the caller got `Ok`, nothing
        // changed, and nobody was told. That is silent misbehaviour on the
        // write path, which is the failure mode this project exists to refuse.
        //
        // Empty means one of two things and we cannot tell which from here:
        // the row does not exist, or the caller may not write it. The message
        // says both rather than guessing, because guessing wrong sends an
        // operator to the wrong place.
        match (op, &row) {
            (WriteOp::Update, None) => Err(BrokerError::PermissionDenied {
                detail: format!(
                    "E_WRITE_NO_ROWS: update of {}:{} changed nothing — the record does not exist, \
                     or your role may not write it",
                    meta.name,
                    record.unwrap_or("?")
                ),
            }),
            _ => {
                let stored = row.unwrap_or(serde_json::Value::Null);
                // ── WO-043: the lifecycle events, AFTER the write committed ──
                //
                // Docstatus edges are derived by comparing the baseline the hook
                // was shown with what the DB actually stored, rather than by
                // trusting the caller's payload — an `on_submit` notification
                // must fire on the transition that really moved docstatus to 1,
                // not on any write that happens to mention the field.
                let before = baseline
                    .iter()
                    .find(|(k, _)| k == "docstatus")
                    .and_then(|(_, v)| match v {
                        Value::Int(i) => Some(*i),
                        _ => None,
                    })
                    .unwrap_or(0);
                let after = stored.get("docstatus").and_then(|v| v.as_i64()).unwrap_or(before);
                let mut triggers = match op {
                    WriteOp::Create => vec![crate::mail::Trigger::AfterInsert],
                    WriteOp::Update => vec![crate::mail::Trigger::Update],
                };
                if before != 1 && after == 1 {
                    triggers.push(crate::mail::Trigger::Submit);
                }
                if before != 2 && after == 2 {
                    triggers.push(crate::mail::Trigger::Cancel);
                }
                self.fire_notifications(doctype, &stored, &triggers);
                Ok(stored)
            }
        }
    }

    // ── WO-043: notifications ───────────────────────────────────────────────

    /// This DocType's notification rules, generation-cached (see `notif_cache`).
    fn notification_rules(
        &self,
        doctype: &str,
    ) -> Result<std::sync::Arc<Vec<crate::mail::NotificationDef>>, BrokerError> {
        let gen = self.gens.meta.load(std::sync::atomic::Ordering::Acquire);
        if let Some((cached_gen, rules)) = self
            .notif_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(doctype)
        {
            if *cached_gen == gen {
                return Ok(std::sync::Arc::clone(rules));
            }
        }
        let rules = std::sync::Arc::new(crate::sync::load_notifications(&self.db, doctype)?);
        self.notif_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(doctype.to_string(), (gen, std::sync::Arc::clone(&rules)));
        Ok(rules)
    }

    /// Evaluate every rule for this DocType against what just happened, and
    /// enqueue the matches.
    ///
    /// **This must never fail a write.** By the time it runs, the document is
    /// already committed — returning an error here would tell the caller their
    /// save failed when it did not, which is a worse lie than a missing email.
    /// So the failure path is: log at ERROR with the reason, count it in
    /// `/metrics`, and return. That is the honest shape of "the mutation
    /// survived, the notification did not" (criterion 4).
    ///
    /// `stored` is the record the DB returned from the write — not the caller's
    /// payload and not the hook's view. That matters for money: a Currency field
    /// comes back as SurrealDB stored it, so a template renders the stored
    /// decimal string and nothing recomputes it (ADR-007).
    fn fire_notifications(
        &self,
        doctype: &str,
        stored: &serde_json::Value,
        triggers: &[crate::mail::Trigger],
    ) {
        let rules = match self.notification_rules(doctype) {
            Ok(r) => r,
            Err(e) => {
                self.note_mail_failure("notification_rules_unreadable", doctype, &e.to_string());
                return;
            }
        };
        if rules.is_empty() {
            return;
        }
        for rule in rules.iter() {
            if !triggers.iter().any(|t| rule.matches(t, stored)) {
                continue;
            }
            // `role:` resolution reads app_user under ROOT deliberately: the
            // notification is the kernel's own side-effect, not the caller's
            // query, and a clerk must not need permission to see the manager's
            // address in order for the manager to be told. The rule is
            // manager-authored metadata, so the authority is the author's.
            let resolve = |role: &str| -> Vec<String> {
                match crate::sync::role_addresses(&self.db, role) {
                    Ok(a) => a,
                    Err(e) => {
                        self.note_mail_failure("mail_role_lookup_failed", &rule.name, &e.to_string());
                        Vec::new()
                    }
                }
            };
            match crate::mail::render(rule, stored, &resolve) {
                Ok(out) => {
                    if let Err(e) = self.enqueue_mail(rule, stored, &out) {
                        self.note_mail_failure("mail_enqueue_failed", &rule.name, &e.to_string());
                    }
                }
                // Unroutable is not an exception — it is a delivery outcome, and
                // it lands in the outbox as a dead row so `/metrics` and the
                // table both show it. A notification that cannot be addressed is
                // exactly the "silently never sends" case, made loud.
                Err(unroutable) => {
                    let reason = unroutable.to_string();
                    if let Err(e) = self.record_dead_mail(rule, stored, &reason) {
                        self.note_mail_failure("mail_deadletter_failed", &rule.name, &e.to_string());
                    }
                    crate::telemetry::inc(
                        "frust_mail_dead_total",
                        &[("tenant", self.db.tenant_id()), ("reason", unroutable.reason_label())],
                        1,
                    );
                    crate::telemetry::emit(
                        crate::telemetry::Level::Error,
                        "mail_unroutable",
                        &[
                            ("notification", serde_json::json!(rule.name)),
                            ("doctype", serde_json::json!(doctype)),
                            ("reason", serde_json::json!(reason)),
                        ],
                    );
                }
            }
        }
    }

    /// The one shape every notification-side failure takes: loud in the log,
    /// counted in `/metrics`, fatal to nothing.
    fn note_mail_failure(&self, event: &'static str, subject: &str, detail: &str) {
        crate::telemetry::inc(
            "frust_mail_errors_total",
            &[("tenant", self.db.tenant_id()), ("stage", event)],
            1,
        );
        crate::telemetry::emit(
            crate::telemetry::Level::Error,
            event,
            &[("subject", serde_json::json!(subject)), ("error", serde_json::json!(detail))],
        );
    }

    fn record_id_of(stored: &serde_json::Value) -> String {
        stored.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_string()
    }

    /// Enqueue one rendered message. A row in the outbox, nothing more — the
    /// request path never opens a socket (criterion 2).
    fn enqueue_mail(
        &self,
        rule: &crate::mail::NotificationDef,
        stored: &serde_json::Value,
        out: &crate::mail::Outbound,
    ) -> Result<(), BrokerError> {
        let content = surql::render_value(&Value::Object(vec![
            ("notification".into(), Value::Text(rule.name.clone())),
            ("doctype".into(), Value::Text(rule.doctype.clone())),
            ("record".into(), Value::Text(Self::record_id_of(stored))),
            (
                "to".into(),
                Value::List(out.to.iter().map(|a| Value::Text(a.clone())).collect()),
            ),
            ("subject".into(), Value::Text(out.subject.clone())),
            ("body".into(), Value::Text(out.body.clone())),
            ("status".into(), Value::Text("queued".into())),
            ("attempts".into(), Value::Int(0)),
            ("tenant".into(), Value::Text(self.db.tenant_id().to_string())),
            (
                "trace".into(),
                Value::Text(crate::telemetry::current_trace().as_str().to_string()),
            ),
        ]))?;
        // `enqueued_at` defaults in the DDL: on v3.2.0 `CONTENT {…} SET …` does
        // not parse, and the column defaulting itself is the better answer anyway.
        self.db.sql_root(&format!(
            "CREATE {} CONTENT {content};",
            crate::mail::OUTBOX_TABLE
        ))?;
        crate::telemetry::inc(
            "frust_mail_enqueued_total",
            &[("tenant", self.db.tenant_id()), ("notification", &rule.name)],
            1,
        );
        Ok(())
    }

    /// A message that could not even be addressed still gets an outbox row —
    /// `status = 'dead'`, with the reason. "Nothing was written" and "it failed"
    /// must not look the same to whoever goes looking.
    fn record_dead_mail(
        &self,
        rule: &crate::mail::NotificationDef,
        stored: &serde_json::Value,
        reason: &str,
    ) -> Result<(), BrokerError> {
        let content = surql::render_value(&Value::Object(vec![
            ("notification".into(), Value::Text(rule.name.clone())),
            ("doctype".into(), Value::Text(rule.doctype.clone())),
            ("record".into(), Value::Text(Self::record_id_of(stored))),
            ("to".into(), Value::List(Vec::new())),
            ("subject".into(), Value::Text(rule.subject.clone())),
            ("body".into(), Value::Text(String::new())),
            ("status".into(), Value::Text("dead".into())),
            ("attempts".into(), Value::Int(0)),
            ("last_error".into(), Value::Text(reason.to_string())),
            ("tenant".into(), Value::Text(self.db.tenant_id().to_string())),
        ]))?;
        self.db.sql_root(&format!(
            "CREATE {} CONTENT {content};",
            crate::mail::OUTBOX_TABLE
        ))?;
        Ok(())
    }

    /// enqueue: identity captured, authority re-derived at run (ADR-006 #4).
    pub fn enqueue(
        &self,
        caller: &Caller,
        job_kind: &str,
        payload: &[(String, Value)],
    ) -> Result<String, BrokerError> {
        self.instrument("enqueue", job_kind, || self.enqueue_inner(caller, job_kind, payload))
    }

    fn enqueue_inner(
        &self,
        caller: &Caller,
        job_kind: &str,
        payload: &[(String, Value)],
    ) -> Result<String, BrokerError> {
        if !job_kind.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(BrokerError::InvalidValue { detail: "bad job kind".into() });
        }
        let payload_s = surql::render_value(&Value::Object(payload.to_vec()))?;
        // identity, not authority: who asked â€” permissions re-evaluate at run.
        // The job record carries the ORIGINATING TRACE (REQ-6.4.1): the worker
        // adopts it at run, so one trace spans enqueue -> claim -> effect.
        // the job carries its TENANT (worker-door fairness, WO-013) and its
        // originating TRACE (REQ-6.4.1) â€” both are accounting the queue owes
        let q = format!(
            "CREATE job SET kind = '{job_kind}', payload = {payload_s}, status = 'queued', \
             requested_by = '{}', tenant = '{}', trace = '{}', enqueued_at = time::now();",
            surql::escape_str(&caller.user),
            surql::escape_str(self.db.tenant_id()),
            surql::escape_str(crate::telemetry::current_trace().as_str())
        );
        let out = self.db.sql_root(&q)?;
        Ok(out
            .as_array()
            .and_then(|a| a.first())
            .and_then(|r| r.get("id"))
            .and_then(|i| i.as_str())
            .unwrap_or_default()
            .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_trap_catches_reentry() {
        let chain = HookChain::default();
        let c1 = chain.push("invoice:a", HookClass::Validate).unwrap();
        // fan-out to a different record is legitimate
        let c2 = c1.push("invoice:b", HookClass::Validate).unwrap();
        // re-entering A's validate anywhere on the chain traps
        let err = c2.push("invoice:a", HookClass::Validate).unwrap_err();
        assert!(matches!(err, BrokerError::HookCycle { .. }));
        // same record, different hook class: allowed
        assert!(c2.push("invoice:a", HookClass::OnWrite).is_ok());
    }

    #[test]
    fn depth_cap_backstops_long_chains() {
        let mut chain = HookChain::default();
        for i in 0..MAX_HOOK_DEPTH {
            chain = chain.push(&format!("t:{i}"), HookClass::Validate).unwrap();
        }
        let err = chain.push("t:final", HookClass::Validate).unwrap_err();
        assert!(matches!(err, BrokerError::HookDepthExceeded { max: 8 }));
    }
}



