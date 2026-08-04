//! Runtime DocType metadata -> ResourceSpec -> the ported migration engine.
//! Schema DDL emission lives in the kernel, and application goes through
//! frust-orm's diff/gate/apply pipeline with history, classification, and
//! locking — never a blind OVERWRITE.

use frust_orm::resource::{Conn, ConnFactory, EngineCtx, ResourceSpec, StmtResult, StorageLocation, Tenancy};
use frust_orm::{MigrationOptions, ObjectKind, ResourceMigrator, SchemaSnapshot};
use std::collections::{BTreeMap, HashMap};
use serde::{Deserialize, Serialize};

use crate::boot::SchemaSync;
use crate::contract::BrokerError;
use crate::db::Db;
use crate::tenancy::ResolvedTenant;

// â”€â”€ DocType metadata (superset of the broker's view) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct DocTypeDef {
    pub name: String,
    #[serde(default)]
    pub app: Option<String>,
    /// Submittable doctypes get `docstatus` + the lattice EVENT (the DB
    /// tier's one resident).
    #[serde(default)]
    pub submittable: bool,
    pub fields: Vec<FieldDef>,
    /// Materialized aggregates declared on the SOURCE doctype.
    #[serde(default)]
    pub aggregates: Vec<AggregateDef>,
}

/// One aggregate declaration. `kind: counter` (Tier 1) compiles to a
/// DEFINE EVENT maintaining the rollup inside the write transaction;
/// `kind: worker` (Tier 2) is maintained by a kernel `RollupWorker` (see
/// `aggregates`) — here it only marks the rollup table write-closed and puts
/// INCLUDE ORIGINAL on the source changefeed. The rollup itself must be
/// declared as its own DocType: rollups are records, readable through the
/// contract like anything else.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct AggregateDef {
    pub kind: String,
    pub rollup: String,
    /// counter: the key field on the source doc (string or record link).
    #[serde(default)]
    pub key: String,
    /// Sums to maintain besides the always-present doc count `n`.
    #[serde(default)]
    pub metrics: Vec<MetricSpec>,
    /// worker: which registered kernel handler maintains it.
    #[serde(default)]
    pub handler: Option<String>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct MetricSpec {
    pub name: String,
    pub field: String,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct FieldDef {
    pub fieldname: String,
    pub fieldtype: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub options: Vec<String>,
    /// Children are embedded by default; the flag is IMMUTABLE after first
    /// sync and "related" is not implemented until promotion tooling exists.
    #[serde(default)]
    pub child_storage: Option<String>,
    /// Client-behaviour rules (Tier-1: metadata, not code). The kernel stores
    /// and serves them; the Desk compiles them into per-field signals. They
    /// are display/validation shaping ONLY — the schema ASSERTs and hooks
    /// remain the enforcement floor.
    #[serde(default)]
    pub depends_on: Option<Rule>,
    #[serde(default)]
    pub read_only_when: Option<Rule>,
    #[serde(default)]
    pub required_when: Option<Rule>,
    /// Client-side validation message shown when the rule holds.
    #[serde(default)]
    pub invalid_when: Option<Rule>,
    /// `call-server`: fetch this field's value from a kernel read when the
    /// source field changes (the one verb that costs a round-trip).
    #[serde(default)]
    pub fetch_from: Option<FetchFrom>,
}

/// A declarative client rule: `field <op> value`.
///
/// Deliberately NOT an expression DSL. The vocabulary is what the runtime's
/// `$()` language can express against a field signal â€” comparisons, nothing
/// else. `and`/`or` are absent because the expression language lacks `&&`/
/// `||` (a recorded vendor follow-up); a rule that needs them belongs on the
/// server, which the metadata docs state rather than the renderer guessing.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rule {
    pub field: String,
    /// eq | ne | gt | lt | ge | le | empty | not_empty
    pub op: String,
    #[serde(default)]
    pub value: Option<String>,
    /// Message for `invalid_when`; ignored by the other rule kinds.
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FetchFrom {
    /// The field holding the source record id (a Link field).
    pub source: String,
    /// The doctype to read.
    pub doctype: String,
    /// Which field of the fetched record to copy in.
    pub field: String,
}

/// The docstatus lattice EVENT: 0 -> 1 -> 2 only; no edits at 2; machine
/// codes survive to clients.
fn lattice_event(table: &str) -> String {
    format!(
        "DEFINE EVENT OVERWRITE docstatus_lattice ON TABLE {table} WHEN $event = 'UPDATE' THEN {{ \
         IF $before.docstatus = 2 {{ THROW 'FRUST:E_DOCSTATUS:RESURRECTION'; }}; \
         IF $before.docstatus = 1 AND $after.docstatus = 0 {{ THROW 'FRUST:E_DOCSTATUS:ILLEGAL_TRANSITION_1_0'; }}; \
         IF $before.docstatus = 0 AND $after.docstatus = 2 {{ THROW 'FRUST:E_DOCSTATUS:ILLEGAL_TRANSITION_0_2'; }}; \
         }}"
    )
}

fn ident_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The Tier-1 counter EVENT. One algebra: a doc contributes its metrics to
/// rollup[key] iff it counts (submittable: docstatus = 1; else: exists). Any
/// mutation subtracts the before-contribution and adds the after-contribution
/// — create, edit, key-move, delete, and the ERP-critical cancel path
/// (docstatus 1 -> 2 = pure reversal) all fall out of the same two UPSERTs.
/// EVENT-body writes bypass table permissions (probed on v3.2.0), so the
/// rollup stays write-closed to record users.
pub fn counter_event_ddl(dt: &DocTypeDef, agg: &AggregateDef) -> Result<String, BrokerError> {
    validate_agg(agg)?;
    let (t, r, k) = (&dt.name, &agg.rollup, &agg.key);
    let counts = |side: &str| {
        if dt.submittable {
            format!("${side}.docstatus = 1")
        } else {
            format!("${side}.id != NONE")
        }
    };
    let mut dec = vec!["n = (n ?? 0) - 1".to_string()];
    let mut inc = vec!["n = (n ?? 0) + 1".to_string()];
    for m in &agg.metrics {
        dec.push(format!("{0} = ({0} ?? 0) - ($before.{1} ?? 0)", m.name, m.field));
        inc.push(format!("{0} = ({0} ?? 0) + ($after.{1} ?? 0)", m.name, m.field));
    }
    Ok(format!(
        "DEFINE EVENT OVERWRITE agg_{r} ON TABLE {t} WHEN $event != 'SELECT' THEN {{ \
         LET $bk = IF {bc} {{ $before.{k} }} ELSE {{ NONE }}; \
         LET $ak = IF {ac} {{ $after.{k} }} ELSE {{ NONE }}; \
         IF $bk != NONE {{ UPSERT type::record('{r}', <string>$bk) SET k = <string>$bk, {dec}; }}; \
         IF $ak != NONE {{ UPSERT type::record('{r}', <string>$ak) SET k = <string>$ak, {inc}; }}; \
         }}",
        bc = counts("before"),
        ac = counts("after"),
        dec = dec.join(", "),
        inc = inc.join(", "),
    ))
}

fn validate_agg(agg: &AggregateDef) -> Result<(), BrokerError> {
    let bad = |d: String| Err(BrokerError::InvalidValue { detail: d });
    if !ident_ok(&agg.rollup) {
        return bad(format!("bad rollup name: {}", agg.rollup));
    }
    if agg.kind == "counter" && !ident_ok(&agg.key) {
        return bad(format!("counter on {} needs a valid key field", agg.rollup));
    }
    for m in &agg.metrics {
        if !ident_ok(&m.name) || !ident_ok(&m.field) || ["k", "n", "id"].contains(&m.name.as_str()) {
            return bad(format!("bad metric {}:{}", m.name, m.field));
        }
    }
    Ok(())
}

/// One-shot recompute of a counter rollup from the live table (root session).
/// DELETE + regroup + re-upsert in a single transaction: a concurrent submit
/// either commits before us (the scan sees it) or conflicts and retries after
/// us (its EVENT applies on top) â€” consistent either way.
/// (v3.2.0 quirk: `FOR $g IN (SELECT ...)` fails to iterate; LET-then-FOR works.)
pub fn backfill_counter(db: &Db, dt: &DocTypeDef, agg: &AggregateDef) -> Result<u64, BrokerError> {
    validate_agg(agg)?;
    if !ident_ok(&dt.name) || !ident_ok(&agg.key) {
        return Err(BrokerError::InvalidValue { detail: "bad backfill source/key".into() });
    }
    let (t, r, k) = (&dt.name, &agg.rollup, &agg.key);
    let mut filter = format!("{k} != NONE");
    if dt.submittable {
        filter.push_str(" AND docstatus = 1");
    }
    let mut sel = vec![format!("{k} AS k"), "count() AS n".to_string()];
    let mut set = vec!["k = <string>$g.k".to_string(), "n = $g.n".to_string()];
    for m in &agg.metrics {
        sel.push(format!("math::sum({}) AS {}", m.field, m.name));
        set.push(format!("{0} = $g.{0}", m.name));
    }
    db.sql_root(&format!(
        "BEGIN; DELETE {r}; \
         LET $rows = SELECT {sel} FROM {t} WHERE {filter} GROUP BY k; \
         FOR $g IN $rows {{ UPSERT type::record('{r}', <string>$g.k) SET {set}; }}; \
         COMMIT;",
        sel = sel.join(", "),
        set = set.join(", "),
    ))?;
    let v = db.sql_root(&format!("SELECT count() FROM {r} GROUP ALL;"))?;
    Ok(v.as_array().and_then(|a| a.first()).and_then(|r| r["count"].as_u64()).unwrap_or(0))
}

/// DocType metadata -> the resource's desired DDL block. The kernel's
/// derive-equivalent; consumed by the engine's snapshot/diff, never applied
/// blindly. `rollup_targets` lists tables maintained by aggregates: those
/// compile write-closed (EVENT/worker writes bypass permissions; record
/// users can read via role, never tamper).
pub fn doctype_ddl(dt: &DocTypeDef) -> Result<String, BrokerError> {
    doctype_ddl_in(dt, &[])
}

pub fn doctype_ddl_in(dt: &DocTypeDef, rollup_targets: &[String]) -> Result<String, BrokerError> {
    let t = &dt.name;
    if !ident_ok(t) {
        return Err(BrokerError::InvalidValue { detail: format!("bad doctype name: {t}") });
    }
    if rollup_targets.contains(t) {
        // A rollup DocType: schemaless (values come from the maintaining
        // EVENT/worker), manager-readable, closed to record writes.
        return Ok(format!(
            "DEFINE TABLE OVERWRITE {t} SCHEMALESS \
             PERMISSIONS \
               FOR select WHERE $auth.role = 'manager' \
               FOR create, update, delete NONE \
             CHANGEFEED 7d"
        ));
    }
    // Tier-2 sources need the feed's undo-patches (worker reconstructs the
    // before-doc); counter-only tables keep the plain feed.
    let feed = if dt.aggregates.iter().any(|a| a.kind == "worker") {
        "CHANGEFEED 7d INCLUDE ORIGINAL"
    } else {
        "CHANGEFEED 7d"
    };
    // The row-WRITE policy.
    //
    // An owner may update their OWN row while it is a DRAFT; a manager may
    // update anytime. The `docstatus = 0` clause is what makes a submitted
    // document immutable to its owner — and v3.2.0 evaluates update
    // permissions against the AFTER-state, so this also means an owner cannot
    // ADVANCE docstatus: only a manager, or the lattice, moves the lattice.
    // That is the invariant split, not duplicated enforcement — the row
    // permission gates WHO may write; the lattice EVENT gates WHICH docstatus
    // moves are legal at all.
    //
    // The clause is conditional on `docstatus` EXISTING: a non-submittable
    // doctype has no docstatus field, so gating on it would evaluate `NONE = 0`
    // (false) and lock owners out. There, plain ownership is the whole rule.
    let update_clause = if dt.submittable {
        "(owner != NONE AND owner = $auth.id AND docstatus = 0) OR $auth.role = 'manager'"
    } else {
        "(owner != NONE AND owner = $auth.id) OR $auth.role = 'manager'"
    };
    let mut stmts = vec![
        format!(
            // The owner clause is null-safe — NONE = NONE can never grant. A
            // NULL-owner row (root/system-written) is invisible to record
            // principals except via the manager role. Delete stays
            // MANAGER-ONLY: it is destructive and unruled — a conservative
            // default, revisit on evidence.
            "DEFINE TABLE OVERWRITE {t} SCHEMAFULL \
             PERMISSIONS \
               FOR select WHERE (owner != NONE AND owner = $auth.id) OR $auth.role = 'manager' \
               FOR create WHERE $auth.id != NONE \
               FOR update WHERE {update_clause} \
               FOR delete WHERE $auth.role = 'manager' \
             {feed}"
        ),
        // option<...>: root/system sessions have no $auth, so owner may be
        // NONE; record users still get stamped by the DEFAULT
        format!("DEFINE FIELD OVERWRITE owner ON {t} TYPE option<record<app_user>> DEFAULT $auth.id READONLY"),
        format!("DEFINE FIELD OVERWRITE status ON {t} TYPE string DEFAULT 'Draft'"),
        // A record session ($auth set) whose owner stamp resolved NULL means
        // identity resolution failed quiet — refuse the write with a machine
        // code instead of storing the hole.
        format!(
            "DEFINE EVENT OVERWRITE identity_guard ON TABLE {t} WHEN $event = 'CREATE' THEN {{ \
             IF $auth != NONE AND $after.owner = NONE {{ THROW 'FRUST:E_IDENTITY_UNRESOLVED'; }}; \
             }}"
        ),
    ];
    if dt.submittable {
        stmts.push(format!("DEFINE FIELD OVERWRITE docstatus ON {t} TYPE int DEFAULT 0"));
        stmts.push(lattice_event(t));
    }
    for f in &dt.fields {
        let name = &f.fieldname;
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || name.is_empty() {
            return Err(BrokerError::InvalidValue { detail: format!("bad field name: {name}") });
        }
        if f.fieldtype == "Table" {
            match f.child_storage.as_deref() {
                None | Some("embedded") => {
                    // v3.2.0 finding: FLEXIBLE on array<object> does not reach
                    // the elements â€” the element field needs its own define.
                    stmts.push(format!("DEFINE FIELD OVERWRITE {name} ON {t} TYPE option<array<object>>"));
                    stmts.push(format!("DEFINE FIELD OVERWRITE {name}.* ON {t} TYPE object FLEXIBLE"));
                    continue;
                }
                Some("related") => {
                    return Err(BrokerError::InvalidValue {
                        detail: format!(
                            "field {name}: child_storage 'related' requires promotion tooling \
                             (the flag is immutable, embedded-only in v0)"
                        ),
                    })
                }
                Some(other) => {
                    return Err(BrokerError::InvalidValue {
                        detail: format!("field {name}: unknown child_storage '{other}'"),
                    })
                }
            }
        }
        let def = match f.fieldtype.as_str() {
            "Link" => {
                let target = f.options.first().cloned().unwrap_or_default();
                if !target.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || target.is_empty() {
                    return Err(BrokerError::InvalidValue {
                        detail: format!("field {name}: Link needs a valid target doctype"),
                    });
                }
                format!("TYPE option<record<{target}>>")
            }
            // Money is DECIMAL in the schema, not float. If a Currency field
            // were float-typed, money would become a float the moment it
            // landed, rollups included (a rollup metric is a Currency field
            // too).
            "Currency" if f.required => "TYPE decimal ASSERT $value >= 0dec".to_string(),
            "Currency" => "TYPE option<decimal> ASSERT $value = NONE OR $value >= 0dec".to_string(),
            "Select" if f.required && !f.options.is_empty() => {
                let opts = f.options.iter().map(|o| format!("'{o}'")).collect::<Vec<_>>().join(", ");
                format!("TYPE string ASSERT $value INSIDE [{opts}]")
            }
            _ if f.required => "TYPE string ASSERT string::len($value) > 0".to_string(),
            _ => "TYPE option<string>".to_string(),
        };
        stmts.push(format!("DEFINE FIELD OVERWRITE {name} ON {t} {def}"));
    }
    for agg in &dt.aggregates {
        match agg.kind.as_str() {
            "counter" => stmts.push(counter_event_ddl(dt, agg)?),
            // worker rollups are kernel code (aggregates::RollupWorker); the
            // declaration only shaped the changefeed + rollup permissions
            "worker" => validate_agg(agg)?,
            other => {
                return Err(BrokerError::InvalidValue {
                    detail: format!("unknown aggregate kind '{other}' on {t}"),
                })
            }
        }
    }
    Ok(stmts.join(";\n"))
}

/// DocType -> ResourceSpec (deps from Link fields + aggregate rollups, for
/// toposort â€” the rollup table must exist before the source's EVENT can
/// UPSERT into it, else auto-creation would leave it permissionless).
pub fn doctype_spec(dt: &DocTypeDef) -> Result<ResourceSpec, BrokerError> {
    doctype_spec_in(dt, &[])
}

pub fn doctype_spec_in(dt: &DocTypeDef, rollup_targets: &[String]) -> Result<ResourceSpec, BrokerError> {
    let mut deps: Vec<String> = dt
        .fields
        .iter()
        .filter(|f| f.fieldtype == "Link")
        .filter_map(|f| f.options.first().cloned())
        .collect();
    deps.extend(dt.aggregates.iter().map(|a| a.rollup.clone()));
    Ok(ResourceSpec {
        app: dt.app.clone().unwrap_or_else(|| "app".to_string()),
        name: dt.name.clone(),
        schema: doctype_ddl_in(dt, rollup_targets)?,
        deps,
    })
}

// â”€â”€ kernel-side implementations of the engine's interface â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

struct KernelConn {
    db: Db,
}

impl Conn for KernelConn {
    fn query(&self, sql: &str) -> anyhow::Result<Vec<StmtResult>> {
        let stmts = self.db.sql_root_raw(sql).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(stmts
            .into_iter()
            .map(|stmt| StmtResult {
                ok: stmt.get("status").and_then(|s| s.as_str()) == Some("OK"),
                result: stmt.get("result").cloned().unwrap_or(serde_json::Value::Null),
            })
            .collect())
    }
}

pub struct KernelConns {
    pub base: ResolvedTenant,
}

impl ConnFactory for KernelConns {
    /// **Checks the location, never chooses it.**
    ///
    /// This used to build a fresh `DbConfig` from whatever `StorageLocation`
    /// the migrator handed over — a direct ns/db selection sitting outside the
    /// seam. Now the only location this factory can serve is the target it was
    /// constructed with, and anything else is a refusal rather than a quiet
    /// connection to a database nobody resolved.
    fn acquire(&self, loc: &StorageLocation) -> anyhow::Result<Box<dyn Conn>> {
        let want = scope_of(&self.base);
        if *loc != want {
            anyhow::bail!(
                "tenancy seam: migrator asked for {}/{}, this connection is scoped to {}/{}",
                loc.namespace,
                loc.database,
                want.namespace,
                want.database
            );
        }
        Ok(Box::new(KernelConn { db: crate::db::scoped_db(&self.base) }))
    }
}

fn scope_of(target: &ResolvedTenant) -> StorageLocation {
    StorageLocation {
        namespace: target.namespace().to_string(),
        database: target.database().to_string(),
    }
}

/// The migration engine's tenancy, **derived from the kernel's strategy**.
///
/// `frust_orm::Tenancy` is the migrator's older, narrower view of the same
/// question. Rather than answer it twice — the `SingleDbTenancy { ns, db }`
/// this replaces read `cfg.ns`/`cfg.db` straight out of the connection config
/// — it is now a projection of the one [`crate::tenancy::TenancyStrategy`], so
/// a new topology cannot migrate schema to a different place than it serves
/// requests from.
pub struct StrategyTenancy {
    pub target: ResolvedTenant,
}

impl Tenancy for StrategyTenancy {
    fn platform_scope(&self) -> StorageLocation {
        scope_of(&self.target)
    }
    fn locate(&self, _tenant_id: &str) -> StorageLocation {
        self.platform_scope()
    }
    fn requires_per_tenant_schema_deploy(&self) -> bool {
        self.target.strategy().per_tenant_schema()
    }
    fn strategy_name(&self) -> &'static str {
        self.target.strategy().name()
    }
}

// â”€â”€ the SchemaSync seam, filled â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

pub struct MetadataSync {
    pub base: ResolvedTenant,
}

/// The workflow governing a DocType, or `None` when it is unmanaged.
///
/// Lives here for the same reason `load_server_script` does: this is metadata
/// loading, and `workflow.rs` holds the judgement, not query text.
pub fn load_workflow(
    db: &Db,
    doctype: &str,
) -> Result<Option<crate::workflow::WorkflowDef>, BrokerError> {
    let n = crate::surql::escape_str(doctype);
    let rows = db.sql_root(&format!("SELECT * FROM workflow WHERE doctype = '{n}' LIMIT 1;"))?;
    let Some(rec) = rows.as_array().and_then(|a| a.first()) else { return Ok(None) };
    serde_json::from_value(rec.clone())
        .map(Some)
        .map_err(|e| BrokerError::Db { detail: format!("bad workflow metadata: {e}") })
}

/// One DocType's Tier-2 server script, or `None` when it declares none.
///
/// Lives here rather than in `hooks.rs` because this is metadata loading, and
/// `surql_monopoly` is right to refuse query text in the hook dispatcher — the
/// guard flagged this, and moving the query here was the correct answer rather
/// than widening the allowlist. The doctype name is escaped on the way in like
/// every other value in this module.
/// A DocType's whole validate chain plus the field envelope it may write into.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HookPlan {
    /// Owner first, then extensions in install order.
    pub entries: Vec<ScriptEntry>,
    /// Field names the DocType declares. Carried alongside so an app writing a
    /// field it never declared can be refused BY NAME rather than silently
    /// stripped — the silent-strip failure this guards against.
    pub declared: Vec<String>,
}

/// One app's contribution to a DocType's validate chain.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptEntry {
    /// The contributing app. `None` = a kernel-created doctype's own script
    /// (no owning bundle), which is still "the owner" for ordering purposes.
    pub app: Option<String>,
    pub script: String,
    /// Which lifecycle event this entry subscribes to. Stored (meta v8), never
    /// inferred — an entry whose hook was lost must not silently become a
    /// `validate`, so boot backfills it and the reader refuses what it cannot
    /// name.
    pub hook: crate::contract::HookClass,
    /// Is this the DocType's OWNER — the app named on the doctype record?
    ///
    /// Carried rather than recomputed downstream, because "who is the owner"
    /// must be decided in exactly one place: the moment the list is read.
    pub is_owner: bool,
}

/// **Every app's validate script for this DocType, OWNER FIRST.**
///
/// The ordering is the guarantee, not a convenience. With a single-slot shape,
/// allowing a second app in meant the extension *replaced* the owner's hook
/// silently, `owner_ran = null`. So the owner's entry is placed first here, at
/// the read, and the dispatcher runs the list in order — an extension can add
/// to the chain and can reject a write, but it can never displace the invariant
/// the DocType's own app declared.
///
/// Extensions follow in stored order, which is install order: deterministic,
/// recorded, boring. Two extensions that disagree are refused at install
/// (refuse-ambiguity), so ordering never has to arbitrate a conflict.
pub fn load_server_scripts(db: &Db, doctype: &str) -> Result<HookPlan, BrokerError> {
    let n = crate::surql::escape_str(doctype);
    let rows = db.sql_root(&format!(
        "SELECT app, server_script, fields FROM doctype WHERE name = '{n}' LIMIT 1;"
    ))?;
    let Some(row) = rows.as_array().and_then(|a| a.first()) else { return Ok(HookPlan::default()) };
    let owner = row.get("app").and_then(|a| a.as_str()).map(str::to_string);

    let raw = row.get("server_script");
    let mut entries: Vec<ScriptEntry> = match raw {
        // the v7 shape
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|it| {
                let script = it.get("script")?.as_str()?.to_string();
                if script.trim().is_empty() {
                    return None;
                }
                let app = it.get("app").and_then(|a| a.as_str()).map(str::to_string);
                let is_owner = app == owner;
                // A pre-v8 entry has no `hook`; boot's migration backfills it,
                // and a doctype CREATED between boot and now can still lack it
                // — the same tolerate-the-old-shape-on-read rule v7 set.
                let hook = it
                    .get("hook")
                    .and_then(|h| h.as_str())
                    .map_or(Some(crate::contract::HookClass::Validate), crate::contract::HookClass::from_wire)?;
                Some(ScriptEntry { app, script, is_owner, hook })
            })
            .collect(),
        // A pre-v7 scalar. Boot migrates these, but a doctype CREATED between
        // boot and now (POST /doctype writes whatever it was given) can still
        // be a string — so read it rather than refuse it. Tolerating the old
        // shape on the read path is what makes the migration a non-event.
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => {
            vec![ScriptEntry {
                app: owner.clone(),
                script: s.clone(),
                is_owner: true,
                hook: crate::contract::HookClass::Validate,
            }]
        }
        _ => Vec::new(),
    };

    // owner first; extensions keep their stored (install) order
    entries.sort_by_key(|e| !e.is_owner);

    // The declared field names travel WITH the scripts, from the same row and
    // the same query — so the loudness check in the dispatcher costs nothing
    // extra and can never disagree with the metadata the scripts were read from.
    let declared = row
        .get("fields")
        .and_then(|f| f.as_array())
        .map(|a| {
            a.iter().filter_map(|f| f.get("fieldname")?.as_str().map(str::to_string)).collect()
        })
        .unwrap_or_default();

    Ok(HookPlan { entries, declared })
}

/// The notification rules watching one DocType.
///
/// Lives here for the same reason `load_workflow` and `load_server_script` do —
/// this is metadata loading, and `mail.rs` stays query-free so `surql_monopoly`
/// covers it without the allowlist growing. A rule whose stored shape no longer
/// deserialises is a HARD error, not a skip: silently dropping a notification
/// that an operator can see in the table is precisely the "it never sent and
/// nobody said why" failure this path is designed against.
pub fn load_notifications(
    db: &Db,
    doctype: &str,
) -> Result<Vec<crate::mail::NotificationDef>, BrokerError> {
    let n = crate::surql::escape_str(doctype);
    let rows = match db.sql_root(&format!(
        "SELECT * FROM {} WHERE doctype = '{n}' ORDER BY name;",
        crate::mail::NOTIFICATION_TABLE
    )) {
        Ok(rows) => rows,
        // A database predating meta v5 has no notification table, and SurrealDB
        // makes that a query ERROR rather than an empty result. "No table" and
        // "no rules" mean the same thing here, so mapping THIS ONE error keeps
        // pre-v5 databases working — while every other failure stays loud,
        // because a write path that logged an error on every save would train
        // operators to ignore errors. Map the one known condition, never
        // swallow the class.
        Err(BrokerError::Db { detail })
            if detail.contains(&format!("The table '{}' does not exist", crate::mail::NOTIFICATION_TABLE)) =>
        {
            return Ok(Vec::new())
        }
        Err(e) => return Err(e),
    };
    serde_json::from_value(rows)
        .map_err(|e| BrokerError::Db { detail: format!("bad notification metadata: {e}") })
}

/// Addresses of every active user holding `role` (the `role:` recipient).
///
/// Users with no email are simply absent — which, if that leaves the list empty,
/// makes the notification dead-letter as `no_recipients` rather than send to
/// nobody quietly.
pub fn role_addresses(db: &Db, role: &str) -> Result<Vec<String>, BrokerError> {
    let r = crate::surql::escape_str(role);
    let rows = db.sql_root(&format!(
        "SELECT email FROM app_user WHERE role = '{r}' AND (status = NONE OR status = 'active') \
         AND email != NONE ORDER BY email;"
    ))?;
    Ok(rows
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| r.get("email").and_then(|e| e.as_str()))
                .filter(|e| crate::mail::looks_like_address(e))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default())
}

/// All DocType metadata records (also used by `frust serve` to wire Tier-2
/// rollup workers from `aggregates` declarations).
pub fn load_doctypes(db: &Db) -> Result<Vec<DocTypeDef>, BrokerError> {
    let v = db.sql_root("SELECT * FROM doctype ORDER BY name;")?;
    serde_json::from_value(v).map_err(|e| BrokerError::Db { detail: format!("bad doctype metadata: {e}") })
}

/// What a user-DocType sync did, and what it deliberately left alone.
#[derive(Debug, Clone, Default)]
pub struct SyncOutcome {
    pub applied: usize,
    /// `"<doctype>.<field>"` for each column the schema holds that no DocType
    /// declares. Named so an operator can list them; see `BootReport`.
    pub orphans: Vec<String>,
}

/// The last-applied schema objects per resource, read from the engine's own
/// history table.
///
/// The migrator diffs metadata against **history**, not against the live
/// schema, so history is where an orphan actually lives: it is a field the
/// engine once applied and the metadata has since stopped declaring.
fn history_fields(db: &Db) -> Result<HashMap<String, BTreeMap<String, String>>, BrokerError> {
    const Q: &str =
        "SELECT name, version, snapshot FROM _framework_migration ORDER BY version DESC;";
    // Before the engine has ever run in this database the history table does
    // not exist, and SurrealDB makes that an error rather than an empty set.
    // No history means no orphans — matched on that ONE condition so a
    // genuinely broken read still fails loudly.
    let rows = match db.sql_root(Q) {
        Ok(v) => v,
        Err(e) if format!("{e:?}").contains("'_framework_migration' does not exist") => {
            return Ok(HashMap::new())
        }
        Err(e) => return Err(e),
    };
    let mut out: HashMap<String, BTreeMap<String, String>> = HashMap::new();
    for row in rows.as_array().map(Vec::as_slice).unwrap_or_default() {
        let Some(name) = row["name"].as_str() else { continue };
        // version-DESC: the first row seen for a resource is its current state
        if out.contains_key(name) {
            continue;
        }
        let Some(raw) = row["snapshot"].as_str() else { continue };
        let snap: SchemaSnapshot = serde_json::from_str(raw).map_err(|e| BrokerError::Db {
            detail: format!(
                "corrupt schema snapshot for '{name}': {e} — refusing to guess what the \
                 schema used to be"
            ),
        })?;
        out.insert(
            name.to_string(),
            snap.objects
                .values()
                .filter(|o| o.kind == ObjectKind::Field)
                .map(|o| (o.name.clone(), o.define_sql.clone()))
                .collect(),
        );
    }
    Ok(out)
}

/// **An orphan column is CARRIED, not merely tolerated.**
///
/// Appends every field the history holds and the metadata no longer declares
/// back onto the resource's desired schema, verbatim. Returns the orphaned
/// field names.
///
/// Tolerating the refusal instead — classifying it as drift and moving on —
/// boots the kernel, but the migrator abandons the **whole resource** on a
/// refused diff (`continue`), so that DocType could never take another schema
/// change again: the owner's next field add would be skipped with nothing but
/// an "orphan" line to show for it. Silent, and exactly the class this project
/// exists to refuse. Carrying the column makes the diff genuinely empty, so
/// the DocType keeps evolving around its orphan.
///
/// `reclaiming` is the one field deliberately NOT carried — see
/// [`MetadataSync::reclaim`].
fn carry_orphans(
    spec: &mut ResourceSpec,
    history: &HashMap<String, BTreeMap<String, String>>,
    reclaiming: Option<&str>,
) -> Vec<String> {
    let Some(fields) = history.get(&format!("{}__{}", spec.app, spec.name)) else {
        return Vec::new();
    };
    let declared = SchemaSnapshot::from_sql(&spec.name, &spec.schema);
    // an unterminated last statement would silently swallow the first carried
    // one — the two DEFINEs would concatenate into one unparseable statement
    if !spec.schema.trim_end().ends_with(';') {
        spec.schema.push(';');
    }
    let mut orphans = Vec::new();
    for (field, define_sql) in fields {
        // `lines.*` belongs to `lines`: a child-object field is declared iff
        // its parent is, or a re-declared table would orphan its own children.
        let base = field.split('.').next().unwrap_or(field);
        if declared.objects.contains_key(&format!("field:{base}")) {
            continue;
        }
        if reclaiming == Some(base) {
            continue;
        }
        spec.schema.push_str(define_sql);
        spec.schema.push(';');
        if base == field {
            orphans.push(field.clone());
        }
    }
    orphans
}

/// Is this migration error the destructive-DROP refusal (i.e. drift), rather
/// than a real failure?
///
/// Matched on the refusal's own wording, deliberately narrowly: anything this
/// does not recognise stays fatal. A broad match here would turn real schema
/// failures into "orphans" and boot straight past them.
fn is_orphan_refusal(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("refusing destructive") && m.contains("remove field")
}

/// The field names inside a `REMOVE FIELD x` refusal message.
fn orphan_fields(message: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = message;
    while let Some(i) = rest.to_ascii_uppercase().find("REMOVE FIELD ") {
        let after = &rest[i + "REMOVE FIELD ".len()..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        if end > 0 {
            out.push(after[..end].to_string());
        }
        rest = &after[end..];
    }
    out
}

/// Resource names are `app__doctype` (or bare `doctype`); the orphan should be
/// reported against the DocType an operator would recognise.
fn resource_doctype(name: &str) -> &str {
    name.rsplit("__").next().unwrap_or(name)
}

impl MetadataSync {
    /// Sync every user DocType through the ported engine. Returns the number
    /// of resources applied. Destructive changes stay gated (engine default:
    /// allow_destructive = false); classification runs under Dev posture at
    /// boot â€” the strict prod apply path arrives with the module-6 surface.
    pub fn sync(&self, db: &Db) -> Result<SyncOutcome, BrokerError> {
        self.sync_inner(db, None)
    }

    /// **Reclaim one orphan column: the acknowledged, online destructive act.**
    ///
    /// The one place `allow_destructive` is ever set, and it is scoped three
    /// ways so it cannot become a blanket drop: only the target DocType's
    /// resource is migrated, every *other* orphan on it is still carried, and
    /// the migrator's own drop path does the work — so the history snapshot
    /// converges with the schema. A hand-issued `REMOVE FIELD` would drop the
    /// column and leave history still claiming it, and the next boot would
    /// report a phantom orphan for a column that no longer exists.
    pub fn reclaim(&self, db: &Db, doctype: &str, column: &str) -> Result<(), BrokerError> {
        let out = self.sync_inner(db, Some((doctype, column)))?;
        if out.applied == 0 {
            return Err(BrokerError::Db {
                detail: format!(
                    "reclaim of '{doctype}.{column}' applied nothing — the column is not in \
                     this DocType's applied schema"
                ),
            });
        }
        Ok(())
    }

    fn sync_inner(
        &self,
        db: &Db,
        reclaim: Option<(&str, &str)>,
    ) -> Result<SyncOutcome, BrokerError> {
        // A schema sync can change any doctype — drop cached metadata.
        crate::broker::invalidate_meta(self.base.tenant_id().as_str());
        let doctypes = load_doctypes(db)?;
        // any doctype targeted by an aggregate compiles write-closed
        let rollup_targets: Vec<String> = doctypes
            .iter()
            .flat_map(|dt| dt.aggregates.iter().map(|a| a.rollup.clone()))
            .collect();
        let history = history_fields(db)?;
        let mut orphans = Vec::new();
        let mut specs = Vec::new();
        for dt in &doctypes {
            if let Some((target, _)) = reclaim {
                if dt.name != target {
                    continue;
                }
            }
            let mut spec = doctype_spec_in(dt, &rollup_targets)?;
            let reclaiming = reclaim.and_then(|(t, c)| (dt.name == t).then_some(c));
            orphans.extend(
                carry_orphans(&mut spec, &history, reclaiming)
                    .into_iter()
                    .map(|f| format!("{}.{f}", dt.name)),
            );
            specs.push(spec);
        }
        if reclaim.is_some() && specs.is_empty() {
            return Err(BrokerError::InvalidValue {
                detail: format!("unknown doctype '{}'", reclaim.unwrap_or_default().0),
            });
        }
        let conns = KernelConns { base: self.base.clone() };
        let tenancy = StrategyTenancy { target: self.base.clone() };
        let ctx = EngineCtx { conns: &conns, tenancy: &tenancy, bootstrap_sql: None };
        let migrator = ResourceMigrator::with_holder(format!("kernel-{}", std::process::id()));
        let opts = MigrationOptions {
            // the ONLY place this is ever true, and only for the single
            // acknowledged column above
            allow_destructive: reclaim.is_some(),
            ..MigrationOptions::default_for_dev()
        };
        let report = migrator
            .migrate_tenant_with(&ctx, &specs, "default", opts)
            .map_err(|e| BrokerError::Db { detail: format!("metadata sync: {e}") })?;
        // ── orphan columns are drift, not a boot failure ──
        //
        // A column present in the schema that no DocType declares is a
        // *legitimate* outcome: an extension uninstall detaches its field from
        // metadata and deliberately leaves the column and its data behind
        // ("metadata detaches, data remains", including cross-app extensions).
        // The migrator correctly classifies removing it as destructive and
        // correctly refuses to apply that without acknowledgement.
        //
        // What was wrong was the CONSEQUENCE. Boot treated the refusal as a
        // fatal error, so a data-safety guard inverted into an availability
        // outage: the kernel would not start, and `BootOptions` had no remedy.
        //
        // So: a refusal to DROP is separated from every other error. It is
        // reported as a named orphan and applied to nothing. Every other error
        // still fails, unchanged — this narrows what is fatal, it does not
        // weaken what acknowledgement a destructive APPLY requires.
        let (drift, fatal): (Vec<_>, Vec<_>) =
            report.errors.iter().partition(|e| is_orphan_refusal(&e.message));
        if !fatal.is_empty() {
            return Err(BrokerError::Db {
                detail: format!("metadata sync errors: {fatal:?}"),
            });
        }
        orphans.extend(drift.iter().flat_map(|e| {
            orphan_fields(&e.message)
                .into_iter()
                .map(move |f| format!("{}.{f}", resource_doctype(&e.name)))
        }));
        orphans.sort();
        orphans.dedup();
        Ok(SyncOutcome { applied: report.applied.len(), orphans })
    }
}

impl SchemaSync for MetadataSync {
    fn sync_user_doctypes(&self, db: &Db) -> Result<SyncOutcome, BrokerError> {
        self.sync(db)
    }
}


