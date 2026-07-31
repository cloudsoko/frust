//! WO-005 module 3: runtime DocType metadata -> ResourceSpec -> the ported
//! migration engine. THE WO-002 SLIVER DIES HERE: schema DDL emission lives
//! in the kernel, and application goes through frust-orm's diff/gate/apply
//! pipeline with history, classification, and locking â€” not blind OVERWRITE.

use frust_orm::resource::{Conn, ConnFactory, EngineCtx, ResourceSpec, StmtResult, StorageLocation, Tenancy};
use frust_orm::{MigrationOptions, ResourceMigrator};
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
    /// Submittable doctypes get `docstatus` + the lattice EVENT (ADR-009:
    /// the DB tier's one resident).
    #[serde(default)]
    pub submittable: bool,
    pub fields: Vec<FieldDef>,
    /// ADR-010 materialized aggregates declared on the SOURCE doctype.
    #[serde(default)]
    pub aggregates: Vec<AggregateDef>,
}

/// One ADR-010 aggregate declaration. `kind: counter` (Tier 1) compiles to a
/// DEFINE EVENT maintaining the rollup inside the write transaction;
/// `kind: worker` (Tier 2) is maintained by a kernel `RollupWorker` (see
/// `aggregates`) â€” here it only marks the rollup table write-closed and puts
/// INCLUDE ORIGINAL on the source changefeed. The rollup itself must be
/// declared as its own DocType: rollups are records, readable through the
/// contract like anything else (WO-007 criterion 5).
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
    /// ADR-008: children are embedded by default; the flag is IMMUTABLE after
    /// first sync and "related" is not implemented until promotion tooling
    /// exists (its own WO).
    #[serde(default)]
    pub child_storage: Option<String>,
    /// WO-014 client-behaviour rules (ADR-001 Tier-1: metadata, not code).
    /// The kernel stores and serves them; the Desk compiles them into
    /// per-field signals. They are display/validation shaping ONLY â€” the
    /// schema ASSERTs and hooks remain the enforcement floor (REQ-1.2.2).
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

/// The docstatus lattice EVENT (ADR-009, WO-004-verified semantics):
/// 0 -> 1 -> 2 only; no edits at 2; machine codes survive to clients.
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

/// The Tier-1 counter EVENT (ADR-010). One algebra: a doc contributes its
/// metrics to rollup[key] iff it counts (submittable: docstatus = 1; else:
/// exists). Any mutation subtracts the before-contribution and adds the
/// after-contribution â€” create, edit, key-move, delete, and the ERP-critical
/// cancel path (docstatus 1 -> 2 = pure reversal) all fall out of the same
/// two UPSERTs. EVENT-body writes bypass table permissions (probed on
/// v3.2.0), so the rollup stays write-closed to record users.
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
/// blindly. `rollup_targets` lists tables maintained by ADR-010 aggregates:
/// those compile write-closed (EVENT/worker writes bypass permissions; record
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
    // WO-020: the row-WRITE policy (Finding B), ADR-decided as option 2.
    //
    // An owner may update their OWN row while it is a DRAFT; a manager may
    // update anytime. The `docstatus = 0` clause is what makes a submitted
    // document immutable to its owner (closing the P-3.2 hole option 1 would
    // have reopened) — and v3.2.0 evaluates update permissions against the
    // AFTER-state (probed, WO-020), so this also means an owner cannot ADVANCE
    // docstatus: only a manager, or the lattice, moves the lattice. That is
    // the invariant split, not duplicated enforcement — the row permission
    // gates WHO may write; the lattice EVENT gates WHICH docstatus moves are
    // legal at all.
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
            // WO-008 criterion 3: the owner clause is null-safe â€” NONE = NONE
            // can never grant. A NULL-owner row (root/system-written) is
            // invisible to record principals except via the manager role.
            // Delete stays MANAGER-ONLY (WO-020 ruled update; delete is
            // destructive and unruled — conservative default, revisit on
            // evidence).
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
        // WO-008 criterion 2: a record session ($auth set) whose owner stamp
        // resolved NULL means identity resolution failed quiet â€” refuse the
        // write with a machine code instead of storing the hole.
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
                             (ADR-008: flag is immutable, embedded-only in v0)"
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
            // WO-016 / REQ-6.2.1: money is DECIMAL in the schema, not float.
            // This was the root of the rollup finding â€” every Currency field
            // was float-typed, so money became a float the moment it landed,
            // rollups included (a rollup metric is a Currency field too).
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
/// WO-018. Lives here for the same reason `load_server_script` does: this is
/// metadata loading, and `workflow.rs` holds the judgement, not query text.
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
/// guard caught this during WO-019 criterion 6, and moving the query was the
/// correct answer rather than widening the allowlist. The doctype name is
/// escaped on the way in like every other value in this module.
pub fn load_server_script(db: &Db, doctype: &str) -> Result<Option<String>, BrokerError> {
    let n = crate::surql::escape_str(doctype);
    let rows =
        db.sql_root(&format!("SELECT server_script FROM doctype WHERE name = '{n}' LIMIT 1;"))?;
    let text = rows
        .as_array()
        .and_then(|a| a.first())
        .and_then(|r| r.get("server_script"))
        .and_then(|s| s.as_str())
        .unwrap_or("");
    Ok(if text.trim().is_empty() { None } else { Some(text.to_string()) })
}

/// WO-043: the notification rules watching one DocType.
///
/// Lives here for the same reason `load_workflow` and `load_server_script` do —
/// this is metadata loading, and `mail.rs` stays query-free so `surql_monopoly`
/// covers it without the allowlist growing. A rule whose stored shape no longer
/// deserialises is a HARD error, not a skip: silently dropping a notification
/// that an operator can see in the table is precisely the "it never sent and
/// nobody said why" failure this WO exists to design against.
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
        // operators to ignore errors (the WO-033 discipline). Same shape as the
        // WO-040 `bodies()` precedent: map the one known condition, never
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

impl MetadataSync {
    /// Sync every user DocType through the ported engine. Returns the number
    /// of resources applied. Destructive changes stay gated (engine default:
    /// allow_destructive = false); classification runs under Dev posture at
    /// boot â€” the strict prod apply path arrives with the module-6 surface.
    pub fn sync(&self, db: &Db) -> Result<usize, BrokerError> {
        // WO-026: a schema sync can change any doctype — drop cached metadata.
        crate::broker::invalidate_meta(self.base.tenant_id().as_str());
        let doctypes = load_doctypes(db)?;
        // any doctype targeted by an aggregate compiles write-closed
        let rollup_targets: Vec<String> = doctypes
            .iter()
            .flat_map(|dt| dt.aggregates.iter().map(|a| a.rollup.clone()))
            .collect();
        let mut specs = Vec::new();
        for dt in &doctypes {
            specs.push(doctype_spec_in(dt, &rollup_targets)?);
        }
        let conns = KernelConns { base: self.base.clone() };
        let tenancy = StrategyTenancy { target: self.base.clone() };
        let ctx = EngineCtx { conns: &conns, tenancy: &tenancy, bootstrap_sql: None };
        let migrator = ResourceMigrator::with_holder(format!("kernel-{}", std::process::id()));
        let report = migrator
            .migrate_tenant_with(&ctx, &specs, "default", MigrationOptions::default_for_dev())
            .map_err(|e| BrokerError::Db { detail: format!("metadata sync: {e}") })?;
        if !report.is_ok() {
            return Err(BrokerError::Db {
                detail: format!("metadata sync errors: {:?}", report.errors),
            });
        }
        Ok(report.applied.len())
    }
}

impl SchemaSync for MetadataSync {
    fn sync_user_doctypes(&self, db: &Db) -> Result<usize, BrokerError> {
        self.sync(db)
    }
}


