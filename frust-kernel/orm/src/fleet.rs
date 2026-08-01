//! Resumable tenant-fleet fan-out.
//!
//! [`ResourceMigrator::migrate_fleet`] migrates every non-suspended tenant,
//! recording per-tenant success in a platform `_framework_fleet_checkpoint`
//! table keyed by a stable hash of the tenant-scoped resource schemas (the
//! `run_id`). A crash mid-fan-out resumes cleanly: re-running with the same
//! schemas yields the same `run_id`, the already-`done` tenants are skipped, and
//! the run continues from where it stopped.
//!
//! [`ResourceMigrator::migrate_fleet_rollout`] adds an explicit canary and
//! promotion gate: rollout is refused before tenant DDL runs unless the stable
//! canary cohort is checkpointed for the exact target resource set.

use std::collections::HashSet;
use std::num::NonZeroUsize;

use serde::Serialize;

use crate::resource::{escape_str, exec, Conn, EngineCtx, ResourceSpec};
use crate::{info, warn_log};

use crate::{MigrationOptions, ResourceMigrator, PLATFORM_APP};

/// Platform-scope checkpoint table: one row per (run_id, tenant).
const FLEET_CHECKPOINT_TABLE: &str = "_framework_fleet_checkpoint";

/// An explicit phase in a guarded fleet rollout.
///
/// `Canary` touches only the deterministic canary cohort. `Rollout` touches the
/// whole fleet, but only after every member of that same cohort has a durable
/// `done` checkpoint for the current schema [`FleetReport::run_id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetStage {
    Canary,
    Rollout,
}

/// Controls a guarded two-stage tenant rollout.
///
/// Construct this with [`FleetRolloutOptions::canary`] for the first pass, then
/// rerun the exact same schemas with [`FleetRolloutOptions::rollout`]. Promotion
/// requires durable checkpoints for the requested leading canary cohort.
#[derive(Debug, Clone)]
pub struct FleetRolloutOptions {
    pub stage: FleetStage,
    pub canary_size: NonZeroUsize,
    pub migration: MigrationOptions,
    /// Stop scheduling new tenants after the first migration failure.
    pub halt_on_failure: bool,
}

impl FleetRolloutOptions {
    /// Run only the first `canary_size` tenants in stable tenant-id order.
    pub fn canary(canary_size: NonZeroUsize, migration: MigrationOptions) -> Self {
        Self {
            stage: FleetStage::Canary,
            canary_size,
            migration,
            halt_on_failure: true,
        }
    }

    /// Promote after at least this stable leading canary cohort is checkpointed.
    pub fn rollout(canary_size: NonZeroUsize, migration: MigrationOptions) -> Self {
        Self {
            stage: FleetStage::Rollout,
            canary_size,
            migration,
            halt_on_failure: true,
        }
    }
}

/// Immutable scheduling decision captured before a guarded rollout mutates a
/// tenant. It makes cohort selection and resume state auditable in the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FleetRolloutPlan {
    pub stage: FleetStage,
    pub canary_size: usize,
    /// Stable identity of the exact canary tenant set.
    pub cohort_id: String,
    /// All active tenants at planning time.
    pub active: Vec<String>,
    /// Tenants eligible to run in this stage (canary cohort or whole fleet).
    pub targets: Vec<String>,
    /// Target tenants already checkpointed for this exact schema run.
    pub completed: Vec<String>,
    /// Target tenants that still need to run.
    pub pending: Vec<String>,
}

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
    /// Tenants migrated successfully but not durably checkpointed. A resume
    /// safely revisits them as a no-op, but guarded promotion will remain closed.
    pub checkpoint_failed: Vec<FleetFailure>,
    /// Guarded rollout plan, when `migrate_fleet_rollout` was used.
    pub rollout: Option<FleetRolloutPlan>,
    /// True when the guarded failure-containment policy triggered.
    pub halted: bool,
}

impl FleetReport {
    /// True when no tenant migration failed. Use [`Self::is_promotable`] before
    /// advancing a guarded rollout.
    pub fn is_ok(&self) -> bool {
        self.failed.is_empty()
    }

    /// True when the stage completed without migration or checkpoint failures.
    pub fn is_promotable(&self) -> bool {
        self.failed.is_empty() && self.checkpoint_failed.is_empty() && !self.halted
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
    pub fn migrate_fleet(
        &self,
        ctx: &EngineCtx,
        specs: &[ResourceSpec],
    ) -> anyhow::Result<FleetReport> {
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
        self.migrate_fleet_inner(ctx, specs, opts, None)
    }

    /// Run a resumable two-stage fleet rollout.
    ///
    /// A canary call deterministically selects the first `canary_size` active
    /// tenant IDs. A rollout call validates that every tenant in that cohort has
    /// a durable success checkpoint for the exact same resource target before it
    /// invokes any tenant migration. Schema changes produce a new run id, so old
    /// canary evidence cannot authorize a different deployment.
    pub fn migrate_fleet_rollout(
        &self,
        ctx: &EngineCtx,
        specs: &[ResourceSpec],
        rollout: FleetRolloutOptions,
    ) -> anyhow::Result<FleetReport> {
        let opts = rollout.migration.clone();
        self.migrate_fleet_inner(ctx, specs, opts, Some(rollout))
    }

    fn migrate_fleet_inner(
        &self,
        ctx: &EngineCtx,
        specs: &[ResourceSpec],
        opts: MigrationOptions,
        rollout: Option<FleetRolloutOptions>,
    ) -> anyhow::Result<FleetReport> {
        if rollout.is_some() && opts.dry_run {
            anyhow::bail!(
                "guarded fleet rollout refused: dry-run cannot produce durable canary evidence"
            );
        }
        if !ctx.tenancy.requires_per_tenant_schema_deploy() {
            warn_log!(
                "migrate_fleet is a no-op for tenancy strategy {} (schemas deploy once at platform install)",
                ctx.tenancy.strategy_name()
            );
            return Ok(FleetReport::default());
        }

        // Include resource identity and dependencies, not just a sorted bag of
        // schema strings. Moving the same DDL to another resource must create a
        // new run rather than inherit unrelated checkpoints.
        let run_id = fleet_run_id(specs);

        let platform = ctx.tenancy.platform_scope();
        let pdb_box = ctx.conns.acquire(&platform)?;
        let pdb = pdb_box.as_ref();
        ensure_checkpoint_table(pdb)?;

        let done = load_done_set(pdb, &run_id)?;
        let canary_evidence = load_canary_evidence(pdb, &run_id)?;
        let tenants = list_active_tenants(pdb)?;

        let guarded_plan = rollout
            .as_ref()
            .map(|r| {
                plan_rollout(
                    r.stage,
                    r.canary_size,
                    tenants.clone(),
                    &done,
                    &canary_evidence,
                )
            })
            .transpose()?;
        let targets = guarded_plan
            .as_ref()
            .map(|plan| plan.targets.clone())
            .unwrap_or_else(|| tenants.clone());
        let halt_on_failure = rollout.as_ref().is_some_and(|r| r.halt_on_failure);

        let mut report = FleetReport {
            run_id: run_id.clone(),
            total: tenants.len(),
            rollout: guarded_plan,
            ..Default::default()
        };

        for tenant in targets {
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
                        report.checkpoint_failed.push(FleetFailure {
                            tenant: tenant.clone(),
                            message: e.to_string(),
                        });
                    }
                    report.migrated.push(tenant);
                }
                Ok(r) => {
                    let _ = record_checkpoint(pdb, &run_id, &tenant, "failed");
                    report.failed.push(FleetFailure {
                        tenant,
                        message: format!("{} migration error(s): {:?}", r.errors.len(), r.errors),
                    });
                    if halt_on_failure {
                        report.halted = true;
                        break;
                    }
                }
                Err(e) => {
                    let _ = record_checkpoint(pdb, &run_id, &tenant, "failed");
                    report.failed.push(FleetFailure {
                        tenant,
                        message: e.to_string(),
                    });
                    if halt_on_failure {
                        report.halted = true;
                        break;
                    }
                }
            }
        }

        let completed_canary = report
            .rollout
            .as_ref()
            .filter(|plan| plan.stage == FleetStage::Canary && report.is_promotable())
            .map(|plan| plan.cohort_id.clone());
        if let Some(cohort_id) = completed_canary {
            if let Err(e) = record_checkpoint(pdb, &run_id, &cohort_id, "canary_done") {
                report.checkpoint_failed.push(FleetFailure {
                    tenant: "canary-stage".to_string(),
                    message: format!("record canary cohort: {e}"),
                });
            }
        }

        info!(
            "fleet migration complete: run_id={} total={} migrated={} already_done={} failed={}",
            report.run_id,
            report.total,
            report.migrated.len(),
            report.already_done.len(),
            report.failed.len()
        );
        Ok(report)
    }
}

/// Build a stable run id from all semantics that identify a tenant resource.
fn fleet_run_id(specs: &[ResourceSpec]) -> String {
    let mut resources: Vec<&ResourceSpec> = specs
        .iter()
        .filter(|resource| resource.app != PLATFORM_APP)
        .collect();
    resources.sort_by(|a, b| (&a.app, &a.name).cmp(&(&b.app, &b.name)));

    let mut parts = vec!["frust-fleet-v2".to_string()];
    for resource in resources {
        let mut deps = resource.deps.clone();
        deps.sort();
        parts.push(resource.app.clone());
        parts.push(resource.name.clone());
        parts.push(deps.join("\u{1f}"));
        parts.push(resource.schema.clone());
    }
    fnv1a_hex(&parts)
}

/// Pure, deterministic stage planner. Promotion validation happens here before
/// the execution loop gets a chance to mutate any tenant schema.
fn plan_rollout(
    stage: FleetStage,
    canary_size: NonZeroUsize,
    mut active: Vec<String>,
    done: &HashSet<String>,
    canary_evidence: &HashSet<String>,
) -> anyhow::Result<FleetRolloutPlan> {
    active.sort();
    if active.windows(2).any(|pair| pair[0] == pair[1]) {
        anyhow::bail!("fleet rollout refused: duplicate active tenant id");
    }
    if canary_size.get() > active.len() {
        anyhow::bail!(
            "fleet rollout refused: canary size {} exceeds {} active tenants",
            canary_size,
            active.len()
        );
    }

    let cohort = &active[..canary_size.get()];
    let cohort_id = fnv1a_hex(cohort);
    if stage == FleetStage::Rollout {
        if !canary_evidence.contains(&cohort_id) {
            anyhow::bail!(
                "fleet rollout refused: durable canary evidence absent for cohort {cohort_id}"
            );
        }
        let missing: Vec<&str> = cohort
            .iter()
            .filter(|tenant| !done.contains(*tenant))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            anyhow::bail!(
                "fleet rollout refused: canary checkpoint incomplete for tenant(s): {}",
                missing.join(", ")
            );
        }
    }

    let targets = match stage {
        FleetStage::Canary => cohort.to_vec(),
        FleetStage::Rollout => active.clone(),
    };
    let completed = targets
        .iter()
        .filter(|tenant| done.contains(*tenant))
        .cloned()
        .collect();
    let pending = targets
        .iter()
        .filter(|tenant| !done.contains(*tenant))
        .cloned()
        .collect();

    Ok(FleetRolloutPlan {
        stage,
        canary_size: canary_size.get(),
        cohort_id,
        active,
        targets,
        completed,
        pending,
    })
}

/// FNV-1a 64-bit over the joined parts. A separator byte avoids
/// `["ab", "c"]` colliding with `["a", "bc"]`.
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
    exec(
        db,
        &format!("DEFINE TABLE OVERWRITE {FLEET_CHECKPOINT_TABLE} SCHEMALESS;"),
    )
    .map_err(|e| anyhow::anyhow!("define fleet checkpoint table: {e}"))?;
    Ok(())
}

fn load_done_set(db: &dyn Conn, run_id: &str) -> anyhow::Result<HashSet<String>> {
    #[derive(serde::Deserialize)]
    struct Row {
        tenant: String,
    }
    let v = exec(
        db,
        &format!(
            "SELECT tenant FROM {FLEET_CHECKPOINT_TABLE} WHERE run_id = '{}' AND status = 'done';",
            escape_str(run_id)
        ),
    )
    .map_err(|e| anyhow::anyhow!("load fleet checkpoints: {e}"))?;
    let rows: Vec<Row> =
        serde_json::from_value(v).map_err(|e| anyhow::anyhow!("decode fleet checkpoints: {e}"))?;
    Ok(rows.into_iter().map(|r| r.tenant).collect())
}

fn load_canary_evidence(db: &dyn Conn, run_id: &str) -> anyhow::Result<HashSet<String>> {
    #[derive(serde::Deserialize)]
    struct Row {
        tenant: String,
    }
    let v = exec(db, &format!(
        "SELECT tenant FROM {FLEET_CHECKPOINT_TABLE} WHERE run_id = '{}' AND status = 'canary_done';",
        escape_str(run_id)
    ))
    .map_err(|e| anyhow::anyhow!("load canary evidence: {e}"))?;
    let rows: Vec<Row> =
        serde_json::from_value(v).map_err(|e| anyhow::anyhow!("decode canary evidence: {e}"))?;
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
    let v = exec(
        db,
        "SELECT short_id FROM tenant \
         WHERE status NOT IN ['suspended', 'deleting', 'deleted'] ORDER BY short_id;",
    )
    .map_err(|e| anyhow::anyhow!("list tenants: {e}"))?;
    let rows: Vec<Row> =
        serde_json::from_value(v).map_err(|e| anyhow::anyhow!("decode tenants: {e}"))?;
    Ok(rows.into_iter().map(|r| r.short_id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::resource::exec;
    use crate::testkit::mem_db;

    use crate::resource::{
        Conn, ConnFactory, EngineCtx, ResourceSpec, StmtResult, StorageLocation, Tenancy,
    };
    use crate::{MigrationOptions, ResourceMigrator};

    fn nz(value: usize) -> NonZeroUsize {
        NonZeroUsize::new(value).unwrap()
    }

    fn tenants(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn canary_plan_is_stable_and_resumable() {
        let done = HashSet::from(["acme".to_string()]);
        let plan = plan_rollout(
            FleetStage::Canary,
            nz(2),
            tenants(&["zen", "beta", "acme"]),
            &done,
            &HashSet::new(),
        )
        .unwrap();

        assert_eq!(plan.active, tenants(&["acme", "beta", "zen"]));
        assert_eq!(plan.targets, tenants(&["acme", "beta"]));
        assert_eq!(plan.completed, tenants(&["acme"]));
        assert_eq!(plan.pending, tenants(&["beta"]));
    }

    #[test]
    fn rollout_refuses_before_scheduling_when_canary_is_incomplete() {
        let done = HashSet::from(["acme".to_string()]);
        let evidence = HashSet::from([fnv1a_hex(&tenants(&["acme", "beta"]))]);
        let error = plan_rollout(
            FleetStage::Rollout,
            nz(2),
            tenants(&["zen", "beta", "acme"]),
            &done,
            &evidence,
        )
        .unwrap_err();

        assert!(error.to_string().contains("canary checkpoint incomplete"));
        assert!(error.to_string().contains("beta"));
    }

    #[test]
    fn rollout_promotes_only_after_exact_cohort_is_done() {
        let done = HashSet::from(["acme".to_string(), "beta".to_string()]);
        let evidence = HashSet::from([fnv1a_hex(&tenants(&["acme", "beta"]))]);
        let plan = plan_rollout(
            FleetStage::Rollout,
            nz(2),
            tenants(&["zen", "beta", "acme"]),
            &done,
            &evidence,
        )
        .unwrap();

        assert_eq!(plan.targets, tenants(&["acme", "beta", "zen"]));
        assert_eq!(plan.completed, tenants(&["acme", "beta"]));
        assert_eq!(plan.pending, tenants(&["zen"]));
    }

    #[test]
    fn rollout_refuses_when_active_membership_changes_the_canary_cohort() {
        let done = HashSet::from(["acme".to_string(), "beta".to_string(), "gamma".to_string()]);
        let old_evidence = HashSet::from([fnv1a_hex(&tenants(&["acme", "beta"]))]);
        let error = plan_rollout(
            FleetStage::Rollout,
            nz(2),
            tenants(&["acme", "gamma", "zen"]),
            &done,
            &old_evidence,
        )
        .unwrap_err();

        assert!(error.to_string().contains("durable canary evidence absent"));
    }

    #[test]
    fn rollout_plan_rejects_invalid_fleet_inputs() {
        let done = HashSet::new();
        let oversized = plan_rollout(
            FleetStage::Canary,
            nz(3),
            tenants(&["acme", "beta"]),
            &done,
            &HashSet::new(),
        )
        .unwrap_err();
        assert!(oversized.to_string().contains("exceeds 2 active tenants"));

        let duplicate = plan_rollout(
            FleetStage::Canary,
            nz(1),
            tenants(&["acme", "acme"]),
            &done,
            &HashSet::new(),
        )
        .unwrap_err();
        assert!(duplicate.to_string().contains("duplicate active tenant id"));
    }

    #[test]
    fn run_id_covers_resource_identity_and_is_order_independent() {
        let note = ResourceSpec {
            app: "crm".into(),
            name: "note".into(),
            schema: "DEFINE TABLE note".into(),
            deps: vec!["contact".into()],
        };
        let contact = ResourceSpec {
            app: "crm".into(),
            name: "contact".into(),
            schema: "DEFINE TABLE contact".into(),
            deps: vec![],
        };
        assert_eq!(
            fleet_run_id(&[note.clone(), contact.clone()]),
            fleet_run_id(&[contact, note.clone()])
        );

        let renamed = ResourceSpec {
            name: "memo".into(),
            ..note
        };
        assert_ne!(
            fleet_run_id(&[renamed]),
            fleet_run_id(&[ResourceSpec {
                app: "crm".into(),
                name: "note".into(),
                schema: "DEFINE TABLE note".into(),
                deps: vec!["contact".into()],
            }])
        );
    }

    #[test]
    fn guarded_rollout_rejects_dry_run_before_database_access() {
        let factory = TestFactory {
            platform_db: "unused-platform".into(),
            fail_checkpoints: false,
        };
        let tenancy = TestTenancy {
            platform_db: "unused-platform".into(),
            tenant_db: "unused-tenant".into(),
        };
        let ctx = EngineCtx {
            conns: &factory,
            tenancy: &tenancy,
            bootstrap_sql: None,
        };
        let error = ResourceMigrator::new()
            .migrate_fleet_rollout(
                &ctx,
                &[],
                FleetRolloutOptions::canary(
                    nz(1),
                    MigrationOptions {
                        dry_run: true,
                        ..MigrationOptions::default_for_dev()
                    },
                ),
            )
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("dry-run cannot produce durable canary evidence"));
    }

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
            StorageLocation {
                namespace: "frust".into(),
                database: self.platform_db.clone(),
            }
        }
        fn locate(&self, _tenant_id: &str) -> StorageLocation {
            StorageLocation {
                namespace: "frust".into(),
                database: self.tenant_db.clone(),
            }
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
        exec(
            &pdb,
            "CREATE tenant SET short_id = 't1', status = 'active';",
        )
        .unwrap();

        let specs = vec![ResourceSpec {
            app: "crm".into(),
            name: "note".into(),
            schema: "DEFINE TABLE OVERWRITE note SCHEMAFULL;\nDEFINE FIELD OVERWRITE title ON TABLE note TYPE option<string>;".into(),
            deps: vec![],
        }];
        let tenancy = TestTenancy {
            platform_db: pdb.db_name.clone(),
            tenant_db: tdb.db_name.clone(),
        };
        let m = ResourceMigrator::with_holder("fleet-test");

        // run 1: tenant migrates, checkpoint write injected to fail —
        // the fan-out must NOT abort (the fix's contract)
        let flaky = TestFactory {
            platform_db: pdb.db_name.clone(),
            fail_checkpoints: true,
        };
        let ctx = EngineCtx {
            conns: &flaky,
            tenancy: &tenancy,
            bootstrap_sql: None,
        };
        let r1 = m
            .migrate_fleet_with(&ctx, &specs, MigrationOptions::default_for_dev())
            .unwrap();
        assert!(
            r1.is_ok(),
            "checkpoint failure must not fail the fleet run: {:?}",
            r1.failed
        );
        assert_eq!(
            r1.migrated,
            vec!["t1".to_string()],
            "tenant migrated despite checkpoint failure"
        );
        assert!(
            load_done_set(&pdb, &r1.run_id).unwrap().is_empty(),
            "checkpoint really did fail"
        );

        // run 2 (resume, healthy): tenant re-visited as a no-op diff, and
        // this time the checkpoint lands
        let healthy = TestFactory {
            platform_db: pdb.db_name.clone(),
            fail_checkpoints: false,
        };
        let ctx = EngineCtx {
            conns: &healthy,
            tenancy: &tenancy,
            bootstrap_sql: None,
        };
        let r2 = m
            .migrate_fleet_with(&ctx, &specs, MigrationOptions::default_for_dev())
            .unwrap();
        assert!(r2.is_ok());
        assert_eq!(
            r2.run_id, r1.run_id,
            "same schemas => same resumable run id"
        );
        assert!(
            load_done_set(&pdb, &r2.run_id).unwrap().contains("t1"),
            "resume checkpointed"
        );
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
        exec(
            &db,
            "CREATE tenant SET short_id = 'acme', status = 'active'; \
             CREATE tenant SET short_id = 'beta', status = 'pending'; \
             CREATE tenant SET short_id = 'gone', status = 'suspended';",
        )
        .unwrap();

        let active = list_active_tenants(&db).unwrap();
        assert_eq!(active, vec!["acme".to_string(), "beta".to_string()]);
    }
}
