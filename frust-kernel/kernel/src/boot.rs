//! Boot discipline: the boot sequence, exactly —
//! advisory lock -> meta sync from binary -> re-read from DB ->
//! user-DocType sync -> boot-check verdict.
//!
//! Fail-closed rules:
//! - DB meta NEWER than binary  -> refuse (`MetaNewerThanBinary`) — the
//!   database does not get to gaslight the engine.
//! - DB meta OLDER than binary  -> pending meta migration; refuse unless
//!   `--accept-meta-migrations` (the two-step ack). A fresh DB (no version
//!   record) is first boot and applies without ack — nothing exists to lose.

use std::sync::{Mutex, OnceLock};

use crate::contract::BrokerError;
use crate::db::Db;
use crate::meta::{identity_ddl, meta_ddl, BOOT_LOCK_TABLE, META_SCHEMA_VERSION, META_TABLE};
use crate::sync::{DocTypeDef, FieldDef};

pub const LOCK_STALE_SECS: f64 = 60.0;
const LOCK_WAIT_MS: u64 = 100;
const LOCK_WAIT_TRIES: u32 = 100; // ~10s: boot contention is short-lived

#[derive(Debug, Clone)]
pub struct BootOptions {
    pub holder: String,
    pub accept_meta_migrations: bool,
}

impl Default for BootOptions {
    fn default() -> Self {
        Self { holder: format!("pid-{}", std::process::id()), accept_meta_migrations: false }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BootError {
    /// Named error for exit criterion 4: the DB's meta-schema is from a
    /// newer binary. Upgrade the binary; never downgrade the schema.
    MetaNewerThanBinary { db_version: i64, binary_version: i64 },
    /// Pending meta migration in an existing DB; re-run with
    /// `--accept-meta-migrations` to apply (two-step ack).
    MetaMigrationPending { db_version: i64, binary_version: i64 },
    LockHeld { holder: String },
    Db(String),
}

impl std::fmt::Display for BootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MetaNewerThanBinary { db_version, binary_version } => write!(
                f,
                "E_META_NEWER_THAN_BINARY: database meta-schema v{db_version} is newer than this binary's v{binary_version}; upgrade the binary"
            ),
            Self::MetaMigrationPending { db_version, binary_version } => write!(
                f,
                "E_META_MIGRATION_PENDING: database meta-schema v{db_version} -> v{binary_version} requires --accept-meta-migrations"
            ),
            Self::LockHeld { holder } => write!(f, "E_BOOT_LOCK_HELD: boot lock held by {holder}"),
            Self::Db(d) => write!(f, "E_BOOT_DB: {d}"),
        }
    }
}

impl std::error::Error for BootError {}

impl From<BrokerError> for BootError {
    fn from(e: BrokerError) -> Self {
        BootError::Db(e.to_string())
    }
}

#[derive(Debug, Clone, Default)]
pub struct BootReport {
    /// Meta DDL applied this boot (false = already current: the no-op path).
    pub applied_meta: bool,
    pub meta_version: i64,
    /// DocTypes re-read from the DB after meta sync (self-hosting honesty).
    pub doctypes: usize,
    /// Columns present in the schema that no DocType declares.
    ///
    /// Named, never merely tolerated. These arise legitimately — an extension
    /// uninstall detaches its field from metadata and deliberately leaves the
    /// column and its data behind (metadata detaches, data remains) — and an
    /// orphan nobody can enumerate is the silent-wrong shape wearing ops
    /// clothing.
    ///
    /// Each entry is `"<doctype>.<field>"`.
    pub orphan_columns: Vec<String>,
}

/// The module-3 seam: user-DocType schema sync. The ported adapter fills
/// this; until then boot performs no user sync and reports it.
pub trait SchemaSync: Send + Sync {
    fn sync_user_doctypes(&self, db: &Db) -> Result<crate::sync::SyncOutcome, BrokerError>;
}

/// Placeholder until module 3 lands the ported engine.
pub struct NoUserSync;
impl SchemaSync for NoUserSync {
    fn sync_user_doctypes(&self, _db: &Db) -> Result<crate::sync::SyncOutcome, BrokerError> {
        Ok(crate::sync::SyncOutcome::default())
    }
}

/// Publish the orphan gauges, so `/metrics` answers "what is orphaned here?"
/// without anyone reading a log line.
///
/// `cleared` is a column that has just STOPPED being an orphan. A gauge map
/// never forgets a key, so a reclaimed column has to be zeroed by name — a
/// metric that keeps reporting a column nobody can find is worse than no
/// metric at all.
pub fn publish_orphans(tenant: &str, orphans: &[String], cleared: Option<&str>) {
    crate::telemetry::gauge("frust_orphan_columns", &[("tenant", tenant)], orphans.len() as f64);
    for o in orphans {
        crate::telemetry::gauge(
            "frust_orphan_column",
            &[("tenant", tenant), ("column", o.as_str())],
            1.0,
        );
    }
    if let Some(c) = cleared {
        crate::telemetry::gauge("frust_orphan_column", &[("tenant", tenant), ("column", c)], 0.0);
    }
}

pub fn boot(db: &Db, opts: &BootOptions, sync: &dyn SchemaSync) -> Result<BootReport, BootError> {
    // The meta boot asks the STRATEGY where its record access belongs instead
    // of assuming. Every topology answers `Database` today (SurrealDB 3.2.0
    // refuses a namespace-level RECORD access with a 400), and the bootstrap
    // DDL is written for that.
    //
    // The refusal matters more than the branch: if a future topology answered
    // `Namespace`, boot would emit database-scoped DDL that lands in the wrong
    // place, and the keyguard would then probe a location no key lives at —
    // refusing a forged token for the WRONG REASON and reporting a vulnerable
    // store SAFE. That is the exact fail-open latent in `keyguard.rs`, and it
    // stays closed by refusing here rather than by anyone remembering.
    if db.target().strategy().access_placement() != crate::tenancy::AccessPlacement::Database {
        return Err(BootError::Db(format!(
            "tenancy strategy {:?} wants its access somewhere the meta boot cannot put it; \
             the keyguard would probe the wrong location and could report a compromised store \
             as safe",
            db.target().strategy().name()
        )));
    }
    // A7 step 1: advisory lock (short wait: boot contention resolves fast)
    lock_acquire(db, &opts.holder)?;
    let result = boot_locked(db, opts, sync);
    lock_release(db, &opts.holder);
    result
}

fn boot_locked(db: &Db, opts: &BootOptions, sync: &dyn SchemaSync) -> Result<BootReport, BootError> {
    // A7 step 2: version check, fail-closed both directions
    let db_version = read_meta_version(db)?;
    let mut applied = false;
    match db_version {
        Some(v) if v > META_SCHEMA_VERSION => {
            return Err(BootError::MetaNewerThanBinary { db_version: v, binary_version: META_SCHEMA_VERSION });
        }
        Some(v) if v < META_SCHEMA_VERSION => {
            if !opts.accept_meta_migrations {
                return Err(BootError::MetaMigrationPending { db_version: v, binary_version: META_SCHEMA_VERSION });
            }
            apply_meta(db, &opts.holder, v)?;
            applied = true;
        }
        Some(_) => {} // current: no-op
        None => {
            // fresh database: first boot applies without ack
            apply_meta(db, &opts.holder, 0)?;
            applied = true;
        }
    }

    // Identity posture repaired on EVERY boot, not just migrations — operator
    // drift on app_user (the $auth sharp edge) has no standing. Idempotent
    // OVERWRITE; records survive.
    db.sql_root(&format!("{};", identity_ddl()))?;

    // Key integrity, the same fail-closed lineage as the meta version above. A
    // `surreal import` of an exported dump restores the JWT signing key as the
    // literal redaction placeholder, so ANY party can forge a session for ANY
    // user — and the instance looks perfectly healthy. Checked AFTER
    // identity_ddl so the access exists to test, and BEFORE anything is served.
    // There is deliberately no acknowledgement flag: proceeding here is
    // serving-compromised, not an operator judgement call.
    crate::keyguard::assert_access_key_is_not_redacted(db)?;

    // Base product records use the same metadata and schema compiler as every
    // app or runtime-created DocType. Only their initial metadata is seeded;
    // their record tables are not part of the kernel meta-schema.
    seed_base_doctypes(db)?;

    // A7 step 3: re-read meta from the DB — the kernel now trusts only what
    // the database actually holds
    let now = read_meta_version(db)?.ok_or_else(|| BootError::Db("meta version missing after sync".into()))?;
    if now != META_SCHEMA_VERSION {
        return Err(BootError::Db(format!("meta version {now} after sync, expected {META_SCHEMA_VERSION}")));
    }
    let doctypes = db
        .sql_root("SELECT count() FROM doctype GROUP ALL;")?
        .as_array()
        .and_then(|a| a.first())
        .and_then(|r| r.get("count"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as usize;

    // A7 step 4: user-DocType sync (module-3 seam)
    let synced = sync.sync_user_doctypes(db)?;

    // Tenant policy is kernel-owned DDL, re-asserted every boot, and loaded
    // into the door buckets. No rows = unlimited = unchanged posture.
    db.sql_root(&format!("{};", crate::meta::tenant_policy_ddl()))?;
    let policies = load_tenant_policies(db)?;

    // observability: boot/meta info visible on /metrics
    crate::telemetry::gauge("frust_meta_version", &[("tenant", db.tenant_id())], now as f64);
    crate::telemetry::gauge("frust_tenant_policies", &[("tenant", db.tenant_id())], policies as f64);
    crate::telemetry::emit(
        crate::telemetry::Level::Info,
        "boot_complete",
        &[
            ("meta_version", serde_json::json!(now)),
            ("meta_applied", serde_json::json!(applied)),
            ("doctypes", serde_json::json!(doctypes)),
            // Orphans are NAMED at boot. An orphan nobody can list is the
            // silent-wrong shape in ops clothing.
            ("orphan_columns", serde_json::json!(synced.orphans)),
        ],
    );
    publish_orphans(db.tenant_id(), &synced.orphans, None);

    // A7 step 5: verdict
    let report =
        BootReport { applied_meta: applied, meta_version: now, doctypes, orphan_columns: synced.orphans };
    mark_ready(db.tenant_id(), &report);
    Ok(report)
}

/// DocTypes that every tenant can use before installing an app.
///
/// `workspace_item` is the embedded row shape used by `workspace.items`.
/// Both are ordinary user DocTypes and deliberately carry no owning app.
pub fn base_doctypes() -> Vec<DocTypeDef> {
    let field = |fieldname: &str, fieldtype: &str, required: bool, options: &[&str]| FieldDef {
        fieldname: fieldname.into(),
        fieldtype: fieldtype.into(),
        required,
        options: options.iter().map(|s| (*s).to_string()).collect(),
        child_storage: None,
        depends_on: None,
        read_only_when: None,
        required_when: None,
        invalid_when: None,
        fetch_from: None,
    };
    vec![
        DocTypeDef {
            name: "workspace_item".into(),
            app: None,
            issingle: false,
            submittable: false,
            fields: vec![
                field("label", "Data", false, &[]),
                field("kind", "Select", true, &["doctype", "report"]),
                field("target", "Data", true, &[]),
            ],
            aggregates: Vec::new(),
        },
        DocTypeDef {
            name: "workspace".into(),
            app: None,
            issingle: false,
            submittable: false,
            fields: vec![
                field("label", "Data", true, &[]),
                field("items", "Table", false, &["workspace_item"]),
                field("module", "Data", false, &[]),
            ],
            aggregates: Vec::new(),
        },
    ]
}

/// Seed missing base metadata without replacing a tenant's live metadata.
pub fn seed_base_doctypes(db: &Db) -> Result<usize, BrokerError> {
    let mut seeded = 0;
    for dt in base_doctypes() {
        let exists = db
            .sql_root(&format!("SELECT name FROM doctype WHERE name = '{}' LIMIT 1;", dt.name))?
            .as_array()
            .is_some_and(|rows| !rows.is_empty());
        if exists {
            continue;
        }
        let content = serde_json::to_string(&dt).map_err(|e| BrokerError::Db {
            detail: format!("serialize base doctype '{}': {e}", dt.name),
        })?;
        db.sql_root(&format!("CREATE doctype:{} CONTENT {content};", dt.name))?;
        seeded += 1;
    }
    if seeded > 0 {
        crate::broker::invalidate_meta(db.tenant_id());
    }
    Ok(seeded)
}

// ── readiness, said out loud ────────────────────────────────────────────────

/// What has actually finished booting, per tenant.
///
/// Process-scoped on purpose: readiness is a property of the PROCESS, and with
/// N tenants in one process "ready" means every tenant this process serves got
/// through boot. Only `boot()` writes here, on its success path, so the flag
/// cannot be true for a tenant that did not boot.
static READY: OnceLock<Mutex<Vec<serde_json::Value>>> = OnceLock::new();

fn ready_slots() -> &'static Mutex<Vec<serde_json::Value>> {
    READY.get_or_init(|| Mutex::new(Vec::new()))
}

fn mark_ready(tenant: &str, report: &BootReport) {
    let mut slots = ready_slots().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    slots.retain(|s| s.get("tenant").and_then(serde_json::Value::as_str) != Some(tenant));
    slots.push(serde_json::json!({
        "tenant": tenant,
        "meta_version": report.meta_version,
        "doctypes": report.doctypes,
        "orphan_columns": report.orphan_columns,
    }));
}

/// The `/ready` answer.
///
/// **What this can and cannot tell you, stated because the difference matters
/// operationally:** the kernel does not accept connections until boot
/// finishes, so over HTTP this has never been observed `false` — during the
/// ~25 s accepting-boot window the honest signal is a refused
/// connection, and a health check must budget for it or it will kill a kernel
/// that is working. What this endpoint adds is the *positive* signal and the
/// boot facts to assert against, separated from `/health`, which answers for
/// the process and knows nothing about any tenant.
pub fn readiness() -> serde_json::Value {
    let slots = ready_slots().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    serde_json::json!({ "ready": !slots.is_empty(), "tenants": slots.clone() })
}

/// Reads `_tenant_policy` into the door buckets. Boot owns the read because
/// boot owns meta bootstrapping; `fairness` stays query-text-free.
fn load_tenant_policies(db: &Db) -> Result<usize, BootError> {
    use crate::fairness::{Policy, POLICY_TABLE, set_policy};
    let rows = db.sql_root(&format!("SELECT * FROM {POLICY_TABLE};"))?;
    let rows = rows.as_array().cloned().unwrap_or_default();
    let mut n = 0;
    for row in rows {
        let Some(tenant) = row.get("tenant").and_then(|t| t.as_str()) else { continue };
        set_policy(
            tenant,
            Policy {
                verbs_per_sec: row.get("verbs_per_sec").and_then(|v| v.as_f64()).unwrap_or(f64::INFINITY),
                verb_burst: row.get("verb_burst").and_then(|v| v.as_f64()).unwrap_or(f64::INFINITY),
                jobs_per_round: row
                    .get("jobs_per_round")
                    .and_then(|v| v.as_u64())
                    .map_or(usize::MAX, |v| v as usize),
            },
        );
        n += 1;
    }
    Ok(n)
}

fn read_meta_version(db: &Db) -> Result<Option<i64>, BootError> {
    // fresh db: the meta table does not exist yet and the SELECT errors —
    // that IS the "no version" answer, so use raw statement results
    let stmts = db.sql_root_raw(&format!("SELECT version FROM {META_TABLE}:schema;"))?;
    let Some(stmt) = stmts.first() else { return Ok(None) };
    if stmt.get("status").and_then(|s| s.as_str()) != Some("OK") {
        let detail = stmt.get("result").map(std::string::ToString::to_string).unwrap_or_default();
        if detail.contains("does not exist") {
            return Ok(None);
        }
        return Err(BootError::Db(detail));
    }
    Ok(stmt
        .get("result")
        .and_then(|r| r.as_array())
        .and_then(|a| a.first())
        .and_then(|r| r.get("version"))
        .and_then(serde_json::Value::as_i64))
}

/// Apply meta DDL + version record + application log IN ONE TRANSACTION —
/// the racing-nodes test counts `_frust_meta` log rows to prove single-apply.
fn apply_meta(db: &Db, holder: &str, from_version: i64) -> Result<(), BootError> {
    let ddl = meta_ddl();
    // Record migrations ride the SAME transaction as the version bump, so a
    // bumped version can never outrun the data it promises.
    let data = crate::meta::meta_data_migrations();
    let holder = crate::surql::escape_str(holder);
    let txn = format!(
        "BEGIN TRANSACTION;\n{ddl};\n{data};\n\
         UPSERT {META_TABLE}:schema SET version = {META_SCHEMA_VERSION};\n\
         CREATE {META_TABLE} SET kind = 'meta_apply_log', from_version = {from_version}, \
         to_version = {META_SCHEMA_VERSION}, applied_by = '{holder}', at = time::now();\n\
         COMMIT TRANSACTION;"
    );
    db.sql_root(&txn)?;
    Ok(())
}

// ── advisory lock (holder-scoped release; stale takeover) ───────────────────

fn lock_acquire(db: &Db, holder: &str) -> Result<(), BootError> {
    db.sql_root(&format!("DEFINE TABLE IF NOT EXISTS {BOOT_LOCK_TABLE} SCHEMALESS PERMISSIONS NONE;"))?;
    for _ in 0..LOCK_WAIT_TRIES {
        if lock_try_create(db, holder)? {
            return Ok(());
        }
        if lock_is_stale(db)? {
            let _ = db.sql_root_raw(&format!("DELETE {BOOT_LOCK_TABLE}:main;"));
            continue;
        }
        std::thread::sleep(std::time::Duration::from_millis(LOCK_WAIT_MS));
    }
    let holder_now = db
        .sql_root(&format!("SELECT holder FROM {BOOT_LOCK_TABLE}:main;"))?
        .as_array()
        .and_then(|a| a.first())
        .and_then(|r| r.get("holder"))
        .and_then(|h| h.as_str())
        .unwrap_or("?")
        .to_string();
    Err(BootError::LockHeld { holder: holder_now })
}

fn lock_try_create(db: &Db, holder: &str) -> Result<bool, BootError> {
    let holder = crate::surql::escape_str(holder);
    let stmts = db.sql_root_raw(&format!(
        "CREATE {BOOT_LOCK_TABLE}:main SET holder = '{holder}', at = time::now();"
    ))?;
    Ok(stmts.iter().all(|s| s.get("status").and_then(|x| x.as_str()) == Some("OK")))
}

fn lock_is_stale(db: &Db) -> Result<bool, BootError> {
    let rows = db.sql_root(&format!(
        "SELECT time::now() - at AS age FROM {BOOT_LOCK_TABLE}:main WHERE at != NONE;"
    ))?;
    // age renders as a duration string (e.g. "1m30s"); anything with an
    // hour/day unit or a minutes value over the threshold counts as stale
    let age = rows
        .as_array()
        .and_then(|a| a.first())
        .and_then(|r| r.get("age"))
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();
    if age.is_empty() {
        return Ok(true); // no/unreadable lock row: treat as stale
    }
    Ok(duration_secs(&age) > LOCK_STALE_SECS)
}

fn duration_secs(s: &str) -> f64 {
    let mut total = 0.0;
    let mut num = String::new();
    let mut unit = String::new();
    let flush = |num: &mut String, unit: &mut String, total: &mut f64| {
        if let Ok(v) = num.parse::<f64>() {
            *total += v
                * match unit.as_str() {
                    "ns" => 1e-9,
                    "us" | "µs" => 1e-6,
                    "ms" => 1e-3,
                    "s" | "" => 1.0,
                    "m" => 60.0,
                    "h" => 3600.0,
                    "d" => 86400.0,
                    "w" => 604800.0,
                    _ => 0.0,
                };
        }
        num.clear();
        unit.clear();
    };
    for c in s.chars() {
        if c.is_ascii_digit() || c == '.' {
            if !unit.is_empty() {
                flush(&mut num, &mut unit, &mut total);
            }
            num.push(c);
        } else {
            unit.push(c);
        }
    }
    flush(&mut num, &mut unit, &mut total);
    total
}

fn lock_release(db: &Db, holder: &str) {
    // holder-scoped: a stale-stolen lock must not cascade (the orm-adapter
    // lesson, applied on day one here)
    let holder = crate::surql::escape_str(holder);
    let _ = db.sql_root_raw(&format!("DELETE {BOOT_LOCK_TABLE}:main WHERE holder = '{holder}';"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_parsing() {
        assert!((duration_secs("1m30s") - 90.0).abs() < 1e-9);
        assert!((duration_secs("250ms") - 0.25).abs() < 1e-9);
        assert!(duration_secs("2h") > LOCK_STALE_SECS);
        assert!(duration_secs("59s") < LOCK_STALE_SECS);
    }

    #[test]
    fn workspace_is_an_ordinary_base_doctype() {
        let base = base_doctypes();
        let workspace = base.iter().find(|dt| dt.name == "workspace").unwrap();
        assert_eq!(workspace.app, None);
        assert!(!workspace.issingle);
        assert!(!workspace.submittable);
        assert_eq!(
            workspace.fields.iter().map(|f| (f.fieldname.as_str(), f.fieldtype.as_str())).collect::<Vec<_>>(),
            vec![("label", "Data"), ("items", "Table"), ("module", "Data")]
        );
        assert_eq!(workspace.fields[1].options, ["workspace_item"]);

        let ddl = crate::sync::doctype_ddl(workspace).unwrap();
        assert!(ddl.contains("DEFINE TABLE OVERWRITE workspace SCHEMAFULL"));
        assert!(ddl.contains("FOR select WHERE $auth.id != NONE"));
        assert!(ddl.contains("FOR create WHERE $auth.id != NONE"));
        assert!(!ddl.contains("PERMISSIONS NONE"));
    }
}
