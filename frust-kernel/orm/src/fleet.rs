//! Resumable tenant-fleet fan-out.
//!
//! [`ResourceMigrator::migrate_fleet`] migrates every non-suspended tenant,
//! recording per-tenant success in a platform `_framework_fleet_checkpoint`
//! table keyed by a stable hash of the tenant-scoped resource schemas (the
//! `run_id`). A crash mid-fan-out resumes cleanly: re-running with the same
//! schemas yields the same `run_id`, the already-`done` tenants are skipped, and
//! the run continues from where it stopped.

use std::collections::HashSet;

use serde::Serialize;

use crate::resource::{escape_str, exec, Conn, EngineCtx, ResourceSpec};
use crate::{info, warn_log};

use crate::{MigrationOptions, ResourceMigrator, PLATFORM_APP};

/// Platform-scope checkpoint table: one row per (run_id, tenant).
const FLEET_CHECKPOINT_TABLE: &str = "_framework_fleet_checkpoint";

/// Outcome of a fleet fan-out.
#[derive(Debug, Clone, Serialize, Default)]
pub struct FleetReport {
    /// Stable id of this fan-out, derived from the tenant-scoped schemas.
    pub run_id: String,
    /// Tenants considered (non-suspended) this invocation.
    pub total: usize,
    /// Tenants migrated this invocation.
    pub migrated: Vec<String>,
    /// Tenants skipped because a prior run already completed them (same run_id).
    pub already_done: Vec<String>,
    /// Tenants whose migration errored — recorded so a resume retries them.
    pub failed: Vec<FleetFailure>,
}

impl FleetReport {
    /// True when no tenant failed.
    pub fn is_ok(&self) -> bool {
        self.failed.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FleetFailure {
    pub tenant: String,
    pub message: String,
}

impl ResourceMigrator {
    /// Migrate every non-suspended tenant, resumably, under fail-closed default
    /// (`Prod`) options. See [`migrate_fleet_with`](Self::migrate_fleet_with) to pass
    /// explicit policy options.
    pub fn migrate_fleet(&self, ctx: &EngineCtx, specs: &[ResourceSpec]) -> anyhow::Result<FleetReport> {
        self.migrate_fleet_with(ctx, specs, MigrationOptions::default())
    }

    /// Migrate every non-suspended tenant, resumably, threading the full migration
    /// `opts` (environment + acknowledge + allow_destructive) into each per-tenant
    /// migration so the Phase-3 classification gate applies uniformly across the
    /// fleet. No-op for tenancy strategies that don't deploy schemas per tenant
    /// (Strategy C / row-level).
    pub fn migrate_fleet_with(
        &self,
        ctx: &EngineCtx,
        specs: &[ResourceSpec],
        opts: MigrationOptions,
    ) -> anyhow::Result<FleetReport> {
        if !ctx.tenancy.requires_per_tenant_schema_deploy() {
            warn_log!(
                "migrate_fleet is a no-op for tenancy strategy {} (schemas deploy once at platform install)",
                ctx.tenancy.strategy_name()
            );
            return Ok(FleetReport::default());
        }

        // run_id = stable hash of the tenant-scoped resource schemas, so a
        // resumed run identifies completed tenants for the *same* target schema.
        let mut schemas: Vec<String> = specs
            .iter()
            .filter(|r| r.app != PLATFORM_APP)
            .map(|r| r.schema.clone())
            .collect();
        schemas.sort();
        let run_id = fnv1a_hex(&schemas);

        let platform = ctx.tenancy.platform_scope();
        let pdb_box = ctx.conns.acquire(&platform)?;
        let pdb = pdb_box.as_ref();
        ensure_checkpoint_table(pdb)?;

        let done = load_done_set(pdb, &run_id)?;
        let tenants = list_active_tenants(pdb)?;

        let mut report = FleetReport {
            run_id: run_id.clone(),
            total: tenants.len(),
            ..Default::default()
        };

        for tenant in tenants {
            if done.contains(&tenant) {
                report.already_done.push(tenant);
                continue;
            }
            match self.migrate_tenant_with(ctx, specs, &tenant, opts.clone()) {
                Ok(r) if r.is_ok() => {
                    // A failed checkpoint write must not abort the fan-out: the
                    // tenant *is* migrated, and a resume re-runs it as a no-op diff.
                    if let Err(e) = record_checkpoint(pdb, &run_id, &tenant, "done") {
                        warn_log!("tenant {tenant} migrated but checkpoint write failed ({e}); a resume will re-visit it (no-op)");
                    }
                    report.migrated.push(tenant);
                }
                Ok(r) => {
                    let _ = record_checkpoint(pdb, &run_id, &tenant, "failed");
                    report.failed.push(FleetFailure {
                        tenant,
                        message: format!("{} migration error(s): {:?}", r.errors.len(), r.errors),
                    });
                }
                Err(e) => {
                    let _ = record_checkpoint(pdb, &run_id, &tenant, "failed");
                    report.failed.push(FleetFailure { tenant, message: e.to_string() });
                }
            }
        }

        info!(
            "fleet migration complete: run_id={} total={} migrated={} already_done={} failed={}",
            report.run_id, report.total, report.migrated.len(),
            report.already_done.len(), report.failed.len()
        );
        Ok(report)
    }
}

/// FNV-1a 64-bit over the joined parts — deterministic across process restarts
/// (no RNG, unlike `DefaultHasher`'s `RandomState`), so a resumed run computes
/// the same id. A separator byte avoids `["ab","c"]` colliding with `["a","bc"]`.
fn fnv1a_hex(parts: &[String]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn ensure_checkpoint_table(db: &dyn Conn) -> anyhow::Result<()> {
    exec(db, &format!("DEFINE TABLE OVERWRITE {FLEET_CHECKPOINT_TABLE} SCHEMALESS;"))
        .map_err(|e| anyhow::anyhow!("define fleet checkpoint table: {e}"))?;
    Ok(())
}

fn load_done_set(db: &dyn Conn, run_id: &str) -> anyhow::Result<HashSet<String>> {
    #[derive(serde::Deserialize)]
    struct Row {
        tenant: String,
    }
    let v = exec(db, &format!(
        "SELECT tenant FROM {FLEET_CHECKPOINT_TABLE} WHERE run_id = '{}' AND status = 'done';",
        escape_str(run_id)
    ))
    .map_err(|e| anyhow::anyhow!("load fleet checkpoints: {e}"))?;
    let rows: Vec<Row> = serde_json::from_value(v)
        .map_err(|e| anyhow::anyhow!("decode fleet checkpoints: {e}"))?;
    Ok(rows.into_iter().map(|r| r.tenant).collect())
}

fn record_checkpoint(
    db: &dyn Conn,
    run_id: &str,
    tenant: &str,
    status: &str,
) -> anyhow::Result<()> {
    // Idempotent delete-then-create: a retry of the same (run_id, tenant)
    // replaces the row rather than duplicating it. (Avoids record-id builder
    // functions, whose names churn across SurrealDB versions.)
    let (r, t, s) = (escape_str(run_id), escape_str(tenant), escape_str(status));
    exec(db, &format!(
        "DELETE {FLEET_CHECKPOINT_TABLE} WHERE run_id = '{r}' AND tenant = '{t}';\n\
         CREATE {FLEET_CHECKPOINT_TABLE} SET run_id = '{r}', tenant = '{t}', status = '{s}', at = time::now();"
    ))
    .map_err(|e| anyhow::anyhow!("record fleet checkpoint: {e}"))?;
    Ok(())
}

fn list_active_tenants(db: &dyn Conn) -> anyhow::Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct Row {
        short_id: String,
    }
    let v = exec(db,
        "SELECT short_id FROM tenant \
         WHERE status NOT IN ['suspended', 'deleting', 'deleted'] ORDER BY short_id;",
    )
    .map_err(|e| anyhow::anyhow!("list tenants: {e}"))?;
    let rows: Vec<Row> = serde_json::from_value(v)
        .map_err(|e| anyhow::anyhow!("decode tenants: {e}"))?;
    Ok(rows.into_iter().map(|r| r.short_id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testkit::mem_db;
    use crate::resource::exec;

    use crate::resource::{Conn, ConnFactory, EngineCtx, ResourceSpec, StorageLocation, StmtResult, Tenancy};
    use crate::{MigrationOptions, ResourceMigrator};

    /// Fix #5's dedicated test (the last unfalsified static fix): a checkpoint
    /// write that fails AFTER a successful tenant migration must warn and
    /// continue — never abort the fan-out — and a resume re-visits the tenant
    /// as a no-op diff, then checkpoints it.
    struct FlakyCheckpoint {
        inner: crate::testkit::TestDb,
        fail_checkpoints: bool,
    }
    impl Conn for FlakyCheckpoint {
        fn query(&self, sql: &str) -> anyhow::Result<Vec<StmtResult>> {
            if self.fail_checkpoints && sql.contains(&format!("CREATE {FLEET_CHECKPOINT_TABLE}")) {
                anyhow::bail!("injected checkpoint write failure");
            }
            self.inner.query(sql)
        }
    }

    struct TestFactory {
        platform_db: String,
        fail_checkpoints: bool,
    }
    impl ConnFactory for TestFactory {
        fn acquire(&self, loc: &StorageLocation) -> anyhow::Result<Box<dyn Conn>> {
            if loc.database == self.platform_db {
                Ok(Box::new(FlakyCheckpoint {
                    inner: crate::testkit::attach(&loc.database),
                    fail_checkpoints: self.fail_checkpoints,
                }))
            } else {
                Ok(Box::new(crate::testkit::attach(&loc.database)))
            }
        }
    }

    struct TestTenancy {
        platform_db: String,
        tenant_db: String,
    }
    impl Tenancy for TestTenancy {
        fn platform_scope(&self) -> StorageLocation {
            StorageLocation { namespace: "frust".into(), database: self.platform_db.clone() }
        }
        fn locate(&self, _tenant_id: &str) -> StorageLocation {
            StorageLocation { namespace: "frust".into(), database: self.tenant_db.clone() }
        }
        fn requires_per_tenant_schema_deploy(&self) -> bool {
            true
        }
        fn strategy_name(&self) -> &'static str {
            "test-db-per-tenant"
        }
    }

    #[test]
    fn failed_checkpoint_warns_and_continues_then_resume_checkpoints() {
        let pdb = crate::testkit::mem_db();
        let tdb = crate::testkit::mem_db();
        exec(&pdb, "CREATE tenant SET short_id = 't1', status = 'active';").unwrap();

        let specs = vec![ResourceSpec {
            app: "crm".into(),
            name: "note".into(),
            schema: "DEFINE TABLE OVERWRITE note SCHEMAFULL;\nDEFINE FIELD OVERWRITE title ON TABLE note TYPE option<string>;".into(),
            deps: vec![],
        }];
        let tenancy = TestTenancy { platform_db: pdb.db_name.clone(), tenant_db: tdb.db_name.clone() };
        let m = ResourceMigrator::with_holder("fleet-test");

        // run 1: tenant migrates, checkpoint write injected to fail —
        // the fan-out must NOT abort (the fix's contract)
        let flaky = TestFactory { platform_db: pdb.db_name.clone(), fail_checkpoints: true };
        let ctx = EngineCtx { conns: &flaky, tenancy: &tenancy, bootstrap_sql: None };
        let r1 = m.migrate_fleet_with(&ctx, &specs, MigrationOptions::default_for_dev()).unwrap();
        assert!(r1.is_ok(), "checkpoint failure must not fail the fleet run: {:?}", r1.failed);
        assert_eq!(r1.migrated, vec!["t1".to_string()], "tenant migrated despite checkpoint failure");
        assert!(load_done_set(&pdb, &r1.run_id).unwrap().is_empty(), "checkpoint really did fail");

        // run 2 (resume, healthy): tenant re-visited as a no-op diff, and
        // this time the checkpoint lands
        let healthy = TestFactory { platform_db: pdb.db_name.clone(), fail_checkpoints: false };
        let ctx = EngineCtx { conns: &healthy, tenancy: &tenancy, bootstrap_sql: None };
        let r2 = m.migrate_fleet_with(&ctx, &specs, MigrationOptions::default_for_dev()).unwrap();
        assert!(r2.is_ok());
        assert_eq!(r2.run_id, r1.run_id, "same schemas => same resumable run id");
        assert!(load_done_set(&pdb, &r2.run_id).unwrap().contains("t1"), "resume checkpointed");
    }

    #[test]
    fn fnv1a_is_stable_and_sensitive() {
        let a = vec!["DEFINE TABLE a".to_string(), "DEFINE TABLE b".to_string()];
        // Deterministic: same input → same id (this is what makes resume work).
        assert_eq!(fnv1a_hex(&a), fnv1a_hex(&a));
        // 16 hex chars (64-bit).
        assert_eq!(fnv1a_hex(&a).len(), 16);
        // Sensitive: a schema change flips the id (→ a fresh run).
        let b = vec!["DEFINE TABLE a".to_string(), "DEFINE TABLE c".to_string()];
        assert_ne!(fnv1a_hex(&a), fnv1a_hex(&b));
        // Separator matters: ["ab","c"] != ["a","bc"].
        assert_ne!(
            fnv1a_hex(&["ab".to_string(), "c".to_string()]),
            fnv1a_hex(&["a".to_string(), "bc".to_string()])
        );
    }

    #[test]
    fn checkpoints_are_scoped_by_run_id_and_idempotent() {
        let db = mem_db();
        ensure_checkpoint_table(&db).unwrap();

        record_checkpoint(&db, "run1", "t1", "done").unwrap();
        record_checkpoint(&db, "run1", "t2", "failed").unwrap();

        // Only `done` tenants for this run_id come back.
        let done = load_done_set(&db, "run1").unwrap();
        assert!(done.contains("t1"));
        assert!(!done.contains("t2"), "failed tenants are not 'done'");

        // A different run_id shares no checkpoints.
        assert!(load_done_set(&db, "run2").unwrap().is_empty());

        // Re-recording the same (run, tenant) overwrites — no duplicate row.
        record_checkpoint(&db, "run1", "t1", "done").unwrap();
        assert_eq!(load_done_set(&db, "run1").unwrap().len(), 1);
    }

    #[test]
    fn list_active_tenants_excludes_suspended() {
        let db = mem_db();
        exec(&db,
            "CREATE tenant SET short_id = 'acme', status = 'active'; \
             CREATE tenant SET short_id = 'beta', status = 'pending'; \
             CREATE tenant SET short_id = 'gone', status = 'suspended';",
        )
        .unwrap();

        let active = list_active_tenants(&db).unwrap();
        assert_eq!(active, vec!["acme".to_string(), "beta".to_string()]);
    }
}
