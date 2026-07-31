//! `framework-orm-adapter` — a native, production-grade SurrealDB migration
//! engine driven by `framework-core` Resources.
//!
//! ## What it does
//!
//! Each Resource emits a `surreal_schema()` block (the derive generates it with
//! `OVERWRITE`). The engine:
//!
//! 1. **Diffs** the desired schema against the last-applied [`SchemaSnapshot`]
//!    stored per-resource in a `_framework_migration` history table — so a
//!    removed struct field becomes an explicit `REMOVE FIELD` instead of silent
//!    drift, and unchanged schemas are a true no-op.
//! 2. **Classifies** changes: additive (safe), altered (warning), removed
//!    fields (destructive — blocked unless `allow_destructive`).
//! 3. **Orders** resources by record-link dependency (toposort) so a
//!    `record<other>` field is defined after its target.
//! 4. **Applies** each resource's diff inside a SurrealDB **transaction**
//!    (`BEGIN/COMMIT`) together with its history row — atomic. (Verified live
//!    by `failed_transaction_rolls_back_ddl_and_history`; if SurrealDB ever
//!    stops rolling back DDL, that test fails rather than the guarantee
//!    silently degrading.)
//! 5. **Locks** the run with a single-runner advisory lock so concurrent nodes
//!    and tenant fan-out don't race (with stale-lock takeover; release is
//!    holder-scoped so a stolen lock can't cascade).
//! 6. Supports a **dry-run** [`MigrationOptions::dry_run`] that returns the
//!    planned statements without touching schema.
//!
//! ## Scopes
//!
//! - [`ResourceMigrator::migrate_platform`] — platform-scoped Resources into the
//!   platform namespace.
//! - [`ResourceMigrator::migrate_tenant`] — tenant-scoped Resources into a
//!   tenant's database.
//! - [`ResourceMigrator::plan_tenant`] — dry-run plan for a tenant.
//!
//! ## Down-migrations (revert)
//!
//! [`ResourceMigrator::revert_platform`] / [`revert_tenant`] roll a scope back by
//! restoring a previously-recorded [`SchemaSnapshot`]. Because the history table
//! stores the full snapshot per version, a revert is just [`schema::diff`] run in
//! reverse — `diff(current, target)` — so re-adding a dropped field, dropping an
//! added field (destructive — gated like a forward drop), and reversing an alter
//! all fall out of the existing engine. History is an append-only **event log**:
//! a revert appends a `direction = 'down'` row whose snapshot is the target, so
//! the latest-version-wins [`load_history`] sees the rolled-back state and a
//! subsequent forward migrate diffs against it correctly.
//!
//! Semantics: with no target, revert undoes the **most recent event** (restores
//! the prior recorded snapshot — one step back). [`RevertOptions::to_version`]
//! reverts straight to the schema recorded at a specific event-version.
//!
//! [`revert_tenant`]: ResourceMigrator::revert_tenant
//!
//! ## Reviewable `.surql` artifacts
//!
//! [`write_artifacts`] dumps a [`MigrationReport`] (plan or applied run) to
//! on-disk `migrations/<scope>/v####__<resource>.surql` files — directly
//! runnable, reviewable in a PR, and an audit trail of what was applied.
//!
//! ## Drift reconciliation
//!
//! [`ResourceMigrator::detect_drift_platform`] / [`detect_drift_tenant`] read the
//! live schema via `INFO FOR DB` + `INFO FOR TABLE` and compare object presence
//! against the recorded snapshot — surfacing out-of-band DDL (objects added or
//! removed outside the engine). It is presence-based: it catches added/removed
//! tables, fields, and indexes. *Definitional* drift (a field whose type was
//! altered by hand) is out of scope because SurrealDB canonicalises `DEFINE`
//! echoes, so a textual compare would be all false-positives — that needs
//! snapshot normalisation, tracked separately.
//!
//! ## Resumable tenant-fleet fan-out
//!
//! [`ResourceMigrator::migrate_fleet`] migrates every (non-suspended) tenant,
//! checkpointing per-tenant success in a platform `_framework_fleet_checkpoint`
//! table keyed by a stable hash of the tenant-scoped resource schemas. A crash
//! mid-fan-out resumes cleanly: re-running with the same schemas computes the
//! same `run_id`, skips already-`done` tenants, and continues.
//!
//! [`detect_drift_tenant`]: ResourceMigrator::detect_drift_tenant
//!
//! ## Not yet
//!
//! Definitional drift detection (needs snapshot normalisation) and reverse
//! dependency ordering for the rare table-drop revert. Reverting a dropped field
//! restores the *schema* but not the lost row data — inherent to any rollback.

#![forbid(unsafe_code)]

mod artifacts;
mod classify;
mod decision;
mod drift;
mod field;
mod fleet;
mod schema;

pub use artifacts::write_artifacts;
pub use classify::{classify_diff, ClassSummary, FieldChangeEntry, FieldChangeReport};
pub use decision::{
    boot_action, decide, BootAction, BootDecision, DecisionNote, DecisionOptions, Environment,
};
pub use drift::{DriftReport, ResourceDrift};
pub use field::{
    classify_field_change, FieldChange, FieldChangeClassification, FieldClass, FieldSnapshot,
    SurrealType,
};
pub use fleet::{FleetFailure, FleetReport};
pub use schema::{DroppedObject, ObjectKind, SchemaDiff, SchemaObject, SchemaSnapshot};

pub mod resource;
#[cfg(test)]
pub mod testkit;
pub use resource::{Conn, ConnFactory, EngineCtx, ResourceSpec, StorageLocation, Tenancy};

use std::collections::{HashMap, HashSet, VecDeque};

use resource::{duration_secs, escape_str, exec};
use serde::{Deserialize, Serialize};

macro_rules! info {
    ($($t:tt)*) => { eprintln!("[frust-orm] {}", format!($($t)*)) };
}
macro_rules! warn_log {
    ($($t:tt)*) => { eprintln!("[frust-orm][warn] {}", format!($($t)*)) };
}
pub(crate) use {info, warn_log};

/// Per-scope migration-history table. One row per resource per applied version.
const MIGRATION_TABLE: &str = "_framework_migration";
/// Single-runner advisory-lock table (one record: `:main`).
const LOCK_TABLE: &str = "_framework_migration_lock";
/// A held lock older than this (seconds) is considered stale and may be stolen
/// — covers a crashed runner that never released.
const LOCK_STALE_SECS: i64 = 300;
/// Resources with this `app` are platform-scoped (live in the platform NS).
const PLATFORM_APP: &str = "platform";

// ────────────────────────────────────────────────────────────────────────────
// Options + report
// ────────────────────────────────────────────────────────────────────────────

/// Knobs for a migration run.
#[derive(Debug, Clone, Default)]
pub struct MigrationOptions {
    /// Compute and return the plan without applying anything (and without
    /// taking the lock). Schema is untouched.
    pub dry_run: bool,
    /// Permit destructive operations (currently: dropping fields → data loss).
    /// Without this, a destructive diff is reported as an error and skipped.
    pub allow_destructive: bool,
    /// Deployment environment governing the Phase-3 classification gate.
    /// Defaults (via `Environment::default()`) to `Prod` — **fail-closed**.
    pub environment: Environment,
    /// Operator acknowledgment of the non-drop Blocked / NeedsBackfill tier. In a
    /// strict (prod) environment those changes are refused unless this is set.
    /// (Drops are governed separately by `allow_destructive`.)
    pub acknowledge: bool,
}

impl MigrationOptions {
    /// Options for an explicit environment (everything else default). Use the named
    /// helpers at call sites so the environment is a *visible* choice, never an
    /// implicit default.
    pub fn default_for(environment: Environment) -> Self {
        Self { environment, ..Default::default() }
    }
    pub fn default_for_dev() -> Self {
        Self::default_for(Environment::Dev)
    }
    pub fn default_for_prod() -> Self {
        Self::default_for(Environment::Prod)
    }
    /// A dry-run plan for an explicit environment.
    pub fn dry_run_for(environment: Environment) -> Self {
        Self { dry_run: true, environment, ..Default::default() }
    }
}

/// Knobs for a revert (down-migration) run.
#[derive(Debug, Clone, Default)]
pub struct RevertOptions {
    /// Compute and return the plan without applying anything (and without
    /// taking the lock).
    pub dry_run: bool,
    /// Permit destructive operations. Reverting an *added* field drops it →
    /// data loss, so it's gated exactly like a forward `REMOVE FIELD`.
    pub allow_destructive: bool,
    /// Event-version to revert each resource down to. `None` = step back one
    /// recorded event (undo the latest migration step). A target ≥ the
    /// resource's current version is skipped (revert never moves forward).
    pub to_version: Option<i64>,
    /// Deployment environment governing the Phase-3 classification gate
    /// (fail-closed `Prod` by default).
    pub environment: Environment,
    /// Operator acknowledgment of the non-drop Blocked / NeedsBackfill tier.
    pub acknowledge: bool,
}

impl RevertOptions {
    pub fn default_for(environment: Environment) -> Self {
        Self { environment, ..Default::default() }
    }
    pub fn default_for_dev() -> Self {
        Self::default_for(Environment::Dev)
    }
    pub fn default_for_prod() -> Self {
        Self::default_for(Environment::Prod)
    }
}

/// Phase-3b classification gate. Refuses the **non-drop** Blocked / NeedsBackfill tier
/// per policy (`decide`), in a strict environment without acknowledgment. Drops are
/// **excluded** — they remain governed solely by `allow_destructive` — so this never
/// double-gates a drop. Returns the refusal message when refused, else `None`.
///
/// Pure: no DB, no `is_ok` dependency. Both the forward (`run`) and revert
/// (`run_revert`) paths call it after their existing destructive gate.
fn classification_refusal(
    environment: Environment,
    acknowledge: bool,
    report: &FieldChangeReport,
) -> Option<String> {
    match decide(environment, &report.non_drop_summary(), &DecisionOptions { acknowledge }) {
        BootDecision::Refuse(notes) => {
            let detail = notes.iter().map(|n| n.message.as_str()).collect::<Vec<_>>().join("; ");
            Some(format!(
                "{environment:?} policy refuses: {detail} — acknowledge to apply"
            ))
        }
        BootDecision::Allow | BootDecision::Warn(_) => None,
    }
}

/// Aggregate a report's per-resource **non-drop** classifications and compute the
/// apply policy decision (`decide`). The single place the aggregation rules live, so
/// the CLI doesn't duplicate them. Pure. Drops are excluded (they remain an
/// `allow_destructive` concern); this is an *apply* decision, not a boot decision.
pub fn apply_decision_for_report(
    report: &MigrationReport,
    environment: Environment,
    opts: &DecisionOptions,
) -> BootDecision {
    let mut summary = ClassSummary::default();
    for fcr in report
        .planned
        .iter()
        .map(|p| &p.classifications)
        .chain(report.applied.iter().map(|a| &a.classifications))
    {
        let nd = fcr.non_drop_summary();
        summary.safe += nd.safe;
        summary.warn += nd.warn;
        summary.needs_backfill += nd.needs_backfill;
        summary.blocked += nd.blocked;
    }
    decide(environment, &summary, opts)
}

/// Outcome of an opt-in startup [`ResourceMigrator::boot_check`].
#[derive(Debug, Clone)]
pub struct BootCheck {
    /// What boot decided / did.
    pub action: BootAction,
    /// Resources auto-applied (on `Converge`).
    pub converged: Vec<String>,
    /// Resources left pending (on `Refuse`).
    pub pending: Vec<String>,
    /// The explicit command to run when boot refuses (e.g.
    /// `bench migrate apply --env prod`).
    pub command_hint: Option<String>,
}

impl BootCheck {
    /// Whether boot may proceed (anything except `Refuse`).
    pub fn proceed(&self) -> bool {
        !matches!(self.action, BootAction::Refuse)
    }
}

/// Boot action for a (dry-run) platform report: aggregate the **non-drop** worst
/// severity + whether any destructive drop is pending, then [`boot_action`]. Pure —
/// testable with a crafted report, no DB.
pub fn boot_action_for_report(report: &MigrationReport, environment: Environment) -> BootAction {
    let mut summary = ClassSummary::default();
    let mut has_drops = false;
    for p in &report.planned {
        let nd = p.classifications.non_drop_summary();
        summary.safe += nd.safe;
        summary.warn += nd.warn;
        summary.needs_backfill += nd.needs_backfill;
        summary.blocked += nd.blocked;
        if !p.destructive.is_empty() {
            has_drops = true;
        }
    }
    boot_action(environment, &summary, has_drops)
}

/// Direction recorded on a history row. Forward migrations are `Up`; reverts
/// append a `Down` row whose snapshot is the rolled-back-to target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Up,
    Down,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Direction::Up => "up",
            Direction::Down => "down",
        }
    }
}

/// Aggregate result of migrating one scope.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MigrationReport {
    pub scope: String,
    pub namespace: String,
    pub database: String,
    /// Resources considered (after scope filtering).
    pub considered: usize,
    /// Resources whose diff was empty — nothing to do.
    pub skipped: usize,
    /// Resources migrated this run.
    pub applied: Vec<AppliedMigration>,
    /// What a `dry_run` *would* do (empty for real runs).
    pub planned: Vec<PlannedMigration>,
    /// Per-resource failures (incl. refused destructive changes). The run does
    /// not stop on the first error — operators see every problem at once.
    pub errors: Vec<MigrationError>,
}

impl MigrationReport {
    /// True if every considered resource succeeded or was skipped.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AppliedMigration {
    pub name: String,
    pub version: i64,
    pub creates: usize,
    pub alters: usize,
    pub drops: usize,
    /// The exact DDL statements applied this step — retained so a reviewable
    /// `.surql` artifact can be emitted for an applied run, not just a plan.
    pub statements: Vec<String>,
    /// Field-level change classifications (Phase 2a — **observable, not
    /// authoritative**: does not affect what was applied or `is_ok()`).
    pub classifications: FieldChangeReport,
}

impl AppliedMigration {
    /// The single construction point for an applied migration — populates
    /// `classifications` from the two snapshots so it can't be skipped. `statements`
    /// are the (already-applied) statements, threaded in to avoid recomputation.
    fn from_diff(
        name: String,
        version: i64,
        d: &SchemaDiff,
        old: &SchemaSnapshot,
        new: &SchemaSnapshot,
        statements: Vec<String>,
    ) -> Self {
        Self {
            name,
            version,
            creates: d.creates.len(),
            alters: d.alters.len(),
            drops: d.drops.len(),
            statements,
            classifications: classify::classify_diff(old, new),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedMigration {
    pub name: String,
    pub next_version: i64,
    pub statements: Vec<String>,
    pub destructive: Vec<String>,
    pub warnings: Vec<String>,
    /// Field-level change classifications (Phase 2a — **observable, not
    /// authoritative**: does not affect the plan, refusal, or `is_ok()`).
    pub classifications: FieldChangeReport,
}

impl PlannedMigration {
    /// The single construction point for a planned migration — derives every field
    /// (incl. `classifications`) from the diff + snapshots so population can't be
    /// skipped.
    fn from_diff(
        name: String,
        next_version: i64,
        d: &SchemaDiff,
        old: &SchemaSnapshot,
        new: &SchemaSnapshot,
        table: &str,
    ) -> Self {
        Self {
            name,
            next_version,
            statements: d.statements(table),
            destructive: d.destructive(),
            warnings: d.warnings(),
            classifications: classify::classify_diff(old, new),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MigrationError {
    pub name: String,
    pub message: String,
}

// ────────────────────────────────────────────────────────────────────────────
// History + lock records
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MigrationRecord {
    name: String,
    version: i64,
    /// JSON-encoded [`SchemaSnapshot`] of the schema as-applied.
    snapshot: String,
    #[allow(dead_code)]
    applied_at: String,
}



/// Idempotent DDL for the per-scope history table (SCHEMALESS — it stores a
/// JSON snapshot blob plus bookkeeping columns).
fn history_table_schema() -> String {
    format!("DEFINE TABLE OVERWRITE {MIGRATION_TABLE} SCHEMALESS;")
}

// ────────────────────────────────────────────────────────────────────────────
// The migrator
// ────────────────────────────────────────────────────────────────────────────

/// Drives schema migrations for a deployment. Cheap to construct; holds only an
/// identifier used to tag the advisory lock.
#[derive(Debug, Clone)]
pub struct ResourceMigrator {
    holder: String,
}

impl Default for ResourceMigrator {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceMigrator {
    pub fn new() -> Self {
        Self {
            holder: format!("pid-{}", std::process::id()),
        }
    }

    /// Tag the advisory lock with a custom holder id (e.g. a node name).
    pub fn with_holder(holder: impl Into<String>) -> Self {
        Self {
            holder: holder.into(),
        }
    }

    /// Migrate platform-scoped Resources into the platform namespace.
    pub fn migrate_platform(&self, ctx: &EngineCtx, specs: &[ResourceSpec]) -> anyhow::Result<MigrationReport> {
        self.migrate_platform_with(ctx, specs, MigrationOptions::default())
    }

    /// Migrate the platform scope with explicit options (dry-run / allow-destructive).
    pub fn migrate_platform_with(
        &self,
        ctx: &EngineCtx,
        specs: &[ResourceSpec],
        opts: MigrationOptions,
    ) -> anyhow::Result<MigrationReport> {
        let regs: Vec<ResourceSpec> =
            specs.iter().filter(|r| r.app == PLATFORM_APP).cloned().collect();
        self.migrate_scope(ctx, ctx.tenancy.platform_scope(), regs, opts, "platform".to_string())
    }

    /// Opt-in startup gate for the **platform** scope. Computes the pending plan
    /// (dry-run), decides via [`boot_action_for_report`], and — only on `Converge` —
    /// auto-applies through the *existing* apply path. Never auto-applies drops /
    /// NeedsBackfill / Blocked. A library call: it returns the outcome and never exits
    /// the process; the caller decides whether to abort boot. **Not** auto-wired into
    /// `framework-core::build()`.
    pub fn boot_check(
        &self,
        ctx: &EngineCtx,
        specs: &[ResourceSpec],
        environment: Environment,
    ) -> anyhow::Result<BootCheck> {
        let plan = self
            .migrate_platform_with(ctx, specs, MigrationOptions::dry_run_for(environment))?;
        let hint = || Some(format!("bench migrate apply --env {}", environment.as_str()));

        match boot_action_for_report(&plan, environment) {
            BootAction::Proceed => Ok(BootCheck {
                action: BootAction::Proceed,
                converged: Vec::new(),
                pending: Vec::new(),
                command_hint: None,
            }),
            BootAction::Converge => {
                // Reuse the existing apply path. `boot_action` guaranteed worst ≤ Warn
                // and no drops, so the Phase-3b gate refuses nothing here.
                let applied = self
                    .migrate_platform_with(ctx, specs, MigrationOptions::default_for(environment))?;
                let converged = applied.applied.iter().map(|a| a.name.clone()).collect();
                if applied.is_ok() {
                    Ok(BootCheck {
                        action: BootAction::Converge,
                        converged,
                        pending: Vec::new(),
                        command_hint: None,
                    })
                } else {
                    // Unexpected: boot_action cleared it, yet apply errored. Surface
                    // as a refusal rather than a silent partial boot.
                    Ok(BootCheck {
                        action: BootAction::Refuse,
                        converged,
                        pending: applied.errors.iter().map(|e| e.name.clone()).collect(),
                        command_hint: hint(),
                    })
                }
            }
            BootAction::Refuse => Ok(BootCheck {
                action: BootAction::Refuse,
                converged: Vec::new(),
                pending: plan.planned.iter().map(|p| p.name.clone()).collect(),
                command_hint: hint(),
            }),
        }
    }

    /// Migrate tenant-scoped Resources into `tenant_id`'s database.
    pub fn migrate_tenant(
        &self,
        ctx: &EngineCtx,
        specs: &[ResourceSpec],
        tenant_id: &str,
    ) -> anyhow::Result<MigrationReport> {
        self.migrate_tenant_with(ctx, specs, tenant_id, MigrationOptions::default())
    }

    /// Migrate a tenant with explicit options (dry-run / allow-destructive).
    pub fn migrate_tenant_with(
        &self,
        ctx: &EngineCtx,
        specs: &[ResourceSpec],
        tenant_id: &str,
        opts: MigrationOptions,
    ) -> anyhow::Result<MigrationReport> {
        let regs: Vec<ResourceSpec> =
            specs.iter().filter(|r| r.app != PLATFORM_APP).cloned().collect();
        self.migrate_scope(ctx, ctx.tenancy.locate(tenant_id), regs, opts, format!("tenant:{tenant_id}"))
    }

    /// Dry-run: what migrating this tenant *would* do, without applying.
    pub fn plan_tenant(
        &self,
        ctx: &EngineCtx,
        specs: &[ResourceSpec],
        tenant_id: &str,
    ) -> anyhow::Result<MigrationReport> {
        self.migrate_tenant_with(
            ctx,
            specs,
            tenant_id,
            MigrationOptions {
                dry_run: true,
                ..Default::default()
            },
        )
    }

    /// Revert the platform scope (down-migration). See [`RevertOptions`].
    pub fn revert_platform(
        &self,
        ctx: &EngineCtx,
        opts: RevertOptions,
    ) -> anyhow::Result<MigrationReport> {
        self.revert_scope(ctx, ctx.tenancy.platform_scope(), opts, "platform".to_string())
    }

    /// Revert a tenant's scope (down-migration). See [`RevertOptions`].
    pub fn revert_tenant(
        &self,
        ctx: &EngineCtx,
        tenant_id: &str,
        opts: RevertOptions,
    ) -> anyhow::Result<MigrationReport> {
        self.revert_scope(ctx, ctx.tenancy.locate(tenant_id), opts, format!("tenant:{tenant_id}"))
    }

    fn migrate_scope(
        &self,
        ctx: &EngineCtx,
        location: StorageLocation,
        regs: Vec<ResourceSpec>,
        opts: MigrationOptions,
        scope: String,
    ) -> anyhow::Result<MigrationReport> {
        let mut report = MigrationReport {
            scope,
            namespace: location.namespace.clone(),
            database: location.database.clone(),
            ..Default::default()
        };

        let conn_box = ctx.conns.acquire(&location)?;
        let db = conn_box.as_ref();

        // Framework bookkeeping (idempotent): analyzers + history table.
        // Skip the analyzer mutation on a pure dry-run.
        if !opts.dry_run {
            if let Some(bootstrap) = &ctx.bootstrap_sql {
                exec(db, bootstrap)
                    .map_err(|e| anyhow::anyhow!("bootstrap install in {}: {e}", report.database))?;
            }
        }
        exec(db, &history_table_schema())
            .map_err(|e| anyhow::anyhow!("history table provision in {}: {e}", report.database))?;

        let locked = if opts.dry_run {
            false
        } else {
            lock_acquire(db, &self.holder)?;
            true
        };

        let inner = self.run(db, regs, &opts, &mut report);

        if locked {
            lock_release(db, &self.holder);
        }
        inner?;

        info!(
            "migration scope complete: scope={} db={} considered={} applied={} planned={} skipped={} errors={}",
            report.scope, report.database, report.considered,
            report.applied.len(), report.planned.len(), report.skipped, report.errors.len()
        );
        Ok(report)
    }

    fn run(
        &self,
        db: &dyn Conn,
        regs: Vec<ResourceSpec>,
        opts: &MigrationOptions,
        report: &mut MigrationReport,
    ) -> anyhow::Result<()> {
        let history = load_history(db)?;
        let ordered = order_resources(regs);

        for reg in ordered {
            report.considered += 1;
            let name = format!("{}__{}", reg.app, reg.name);
            let table = reg.name.as_str();

            let new_snap = SchemaSnapshot::from_sql(table, &reg.schema);
            let (prev_version, old_snap) = history
                .get(&name)
                .cloned()
                .unwrap_or((0, SchemaSnapshot::default()));

            let d = schema::diff(&old_snap, &new_snap);
            if d.is_empty() {
                report.skipped += 1;
                continue;
            }

            let next_version = prev_version + 1;

            if opts.dry_run {
                report.planned.push(PlannedMigration::from_diff(
                    name,
                    next_version,
                    &d,
                    &old_snap,
                    &new_snap,
                    table,
                ));
                continue;
            }

            let destructive = d.destructive();
            let warnings = d.warnings();
            let statements = d.statements(table);

            if !destructive.is_empty() && !opts.allow_destructive {
                report.errors.push(MigrationError {
                    name: name.clone(),
                    message: format!(
                        "refusing destructive change(s) {destructive:?} — re-run with allow_destructive to apply"
                    ),
                });
                continue;
            }

            // Phase-3b classification gate (drops excluded — handled above).
            if let Some(message) = classification_refusal(
                opts.environment,
                opts.acknowledge,
                &classify::classify_diff(&old_snap, &new_snap),
            ) {
                report.errors.push(MigrationError { name: name.clone(), message });
                continue;
            }

            match apply_resource(db, &name, &new_snap, next_version, &statements, Direction::Up) {
                Ok(()) => {
                    if !warnings.is_empty() {
                        warn_log!("migration applied with warnings: {name} {warnings:?}");
                    }
                    report.applied.push(AppliedMigration::from_diff(
                        name,
                        next_version,
                        &d,
                        &old_snap,
                        &new_snap,
                        statements,
                    ));
                }
                Err(e) => report.errors.push(MigrationError {
                    name,
                    message: format!("{e}"),
                }),
            }
        }
        Ok(())
    }

    fn revert_scope(
        &self,
        ctx: &EngineCtx,
        location: StorageLocation,
        opts: RevertOptions,
        scope: String,
    ) -> anyhow::Result<MigrationReport> {
        let mut report = MigrationReport {
            scope,
            namespace: location.namespace.clone(),
            database: location.database.clone(),
            ..Default::default()
        };

        let conn_box = ctx.conns.acquire(&location)?;
        let db = conn_box.as_ref();

        // History table must exist to revert against. Idempotent; no analyzer
        // mutation needed on the down path.
        exec(db, &history_table_schema())
            .map_err(|e| anyhow::anyhow!("history table provision in {}: {e}", report.database))?;

        let locked = if opts.dry_run {
            false
        } else {
            lock_acquire(db, &self.holder)?;
            true
        };

        let inner = self.run_revert(db, &opts, &mut report);

        if locked {
            lock_release(db, &self.holder);
        }
        inner?;

        info!(
            "revert scope complete: scope={} db={} considered={} reverted={} planned={} skipped={} errors={}",
            report.scope, report.database, report.considered,
            report.applied.len(), report.planned.len(), report.skipped, report.errors.len()
        );
        Ok(report)
    }

    fn run_revert(
        &self,
        db: &dyn Conn,
        opts: &RevertOptions,
        report: &mut MigrationReport,
    ) -> anyhow::Result<()> {
        let history = load_full_history(db)?;

        // Deterministic order. Field-level reverts are order-independent (we
        // never auto-drop tables, so there are no cross-resource record-link
        // constraints to respect on the way down).
        let mut names: Vec<&String> = history.keys().collect();
        names.sort();

        for name in names {
            report.considered += 1;
            let versions = &history[name];
            // `versions` is sorted version-DESC, so [0] is the current state.
            let (cur_version, cur_snap) = &versions[0];

            // Pick the snapshot to revert *to*.
            let target = match opts.to_version {
                // Step back one event: the snapshot recorded before the current one.
                None => versions.get(1).map(|(_, s)| s),
                // Jump to a specific past event-version (must be strictly older).
                Some(v) if v < *cur_version => {
                    versions.iter().find(|(ver, _)| *ver == v).map(|(_, s)| s)
                }
                Some(_) => None,
            };

            let Some(target_snap) = target else {
                // Nothing older to revert to (at base), or target not found /
                // not strictly older — leave this resource untouched.
                report.skipped += 1;
                continue;
            };

            let d = schema::diff(cur_snap, target_snap);
            if d.is_empty() {
                report.skipped += 1;
                continue;
            }

            let next_version = cur_version + 1;

            if opts.dry_run {
                report.planned.push(PlannedMigration::from_diff(
                    name.clone(),
                    next_version,
                    &d,
                    cur_snap,
                    target_snap,
                    &cur_snap.table,
                ));
                continue;
            }

            let destructive = d.destructive();
            let warnings = d.warnings();
            let statements = d.statements(&cur_snap.table);

            if !destructive.is_empty() && !opts.allow_destructive {
                report.errors.push(MigrationError {
                    name: name.clone(),
                    message: format!(
                        "refusing destructive revert {destructive:?} — re-run with allow_destructive to apply"
                    ),
                });
                continue;
            }

            // Phase-3b classification gate (drops excluded — handled above).
            if let Some(message) = classification_refusal(
                opts.environment,
                opts.acknowledge,
                &classify::classify_diff(cur_snap, target_snap),
            ) {
                report.errors.push(MigrationError { name: name.clone(), message });
                continue;
            }

            match apply_resource(db, name, target_snap, next_version, &statements, Direction::Down) {
                Ok(()) => {
                    if !warnings.is_empty() {
                        warn_log!("revert applied with warnings: {name} {warnings:?}");
                    }
                    report.applied.push(AppliedMigration::from_diff(
                        name.clone(),
                        next_version,
                        &d,
                        cur_snap,
                        target_snap,
                        statements,
                    ));
                }
                Err(e) => report.errors.push(MigrationError {
                    name: name.clone(),
                    message: format!("{e}"),
                }),
            }
        }
        Ok(())
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Apply + history
// ────────────────────────────────────────────────────────────────────────────

/// Apply one resource's migration statements + its history row atomically.
/// `snapshot` is the schema state *after* this step (the new current snapshot);
/// `direction` tags the history row `up` (forward) or `down` (revert).
fn apply_resource(
    db: &dyn Conn,
    name: &str,
    snapshot: &SchemaSnapshot,
    version: i64,
    statements: &[String],
    direction: Direction,
) -> anyhow::Result<()> {
    let snapshot_json = serde_json::to_string(snapshot)?;

    let mut txn = String::from("BEGIN TRANSACTION;\n");
    for s in statements {
        txn.push_str(s);
        txn.push_str(";\n");
    }
    txn.push_str(&format!(
        "CREATE {MIGRATION_TABLE} SET name = '{}', version = {version}, snapshot = '{}', direction = '{}', applied_at = <string> time::now();
",
        escape_str(name), escape_str(&snapshot_json), direction.as_str()
    ));
    txn.push_str("COMMIT TRANSACTION;");

    exec(db, &txn).map_err(|e| anyhow::anyhow!("migration transaction failed for {name}: {e}"))?;
    Ok(())
}

/// Load the latest applied snapshot per resource name.
fn load_history(
    db: &dyn Conn,
) -> anyhow::Result<HashMap<String, (i64, SchemaSnapshot)>> {
    let rows = history_rows(db).map_err(|e| anyhow::anyhow!("load migration history: {e}"))?;

    let mut map: HashMap<String, (i64, SchemaSnapshot)> = HashMap::new();
    for row in rows {
        // DESC order → first row seen per name is the latest version.
        // (Superseded rows are skipped before decoding: only the *latest*
        // snapshot per resource must parse for a forward migrate.)
        if map.contains_key(&row.name) {
            continue;
        }
        let snap = decode_snapshot(&row)?;
        map.insert(row.name, (row.version, snap));
    }
    Ok(map)
}

/// Decode a history row's snapshot, failing loudly on corruption. A snapshot
/// that won't parse must never degrade to an empty default: forward migrate
/// would re-classify the whole table as newly created (required adds silently
/// become "Safe"), and revert would diff toward an empty schema — i.e. drop
/// every field.
fn decode_snapshot(row: &MigrationRecord) -> anyhow::Result<SchemaSnapshot> {
    serde_json::from_str(&row.snapshot).map_err(|e| {
        anyhow::anyhow!(
            "corrupt schema snapshot in {MIGRATION_TABLE} for '{}' v{}: {e} — refusing to \
             migrate against unknown state; inspect/repair the history row first",
            row.name,
            row.version
        )
    })
}

/// Load the *full* per-resource history (every recorded event), each list
/// sorted version-DESC so `[0]` is the current state. Used by revert to reach
/// past snapshots; forward migrate only needs the latest, via [`load_history`].
fn load_full_history(
    db: &dyn Conn,
) -> anyhow::Result<HashMap<String, Vec<(i64, SchemaSnapshot)>>> {
    let rows = history_rows(db).map_err(|e| anyhow::anyhow!("load full migration history: {e}"))?;

    let mut map: HashMap<String, Vec<(i64, SchemaSnapshot)>> = HashMap::new();
    for row in rows {
        // Every row is a potential revert target, so all of them must parse.
        let snap = decode_snapshot(&row)?;
        map.entry(row.name).or_default().push((row.version, snap));
    }
    // Query is globally version-DESC, so per-name lists already descend; sort
    // defensively in case the storage layer ever reorders.
    for versions in map.values_mut() {
        versions.sort_by(|a, b| b.0.cmp(&a.0));
    }
    Ok(map)
}

// ────────────────────────────────────────────────────────────────────────────
// Advisory lock
// ────────────────────────────────────────────────────────────────────────────

fn lock_acquire(db: &dyn Conn, holder: &str) -> anyhow::Result<()> {
    exec(db, &format!("DEFINE TABLE IF NOT EXISTS {LOCK_TABLE} SCHEMALESS;"))
        .map_err(|e| anyhow::anyhow!("define lock table: {e}"))?;

    if lock_try_create(db, holder)? {
        return Ok(());
    }
    if lock_is_stale(db)? {
        warn_log!("migration lock is stale; stealing it");
        let _ = db.query(&format!("DELETE {LOCK_TABLE}:main;"));
        if lock_try_create(db, holder)? {
            return Ok(());
        }
    }
    anyhow::bail!("another migration run holds {LOCK_TABLE}:main");
}

/// `true` if the lock record was created by us (i.e. we hold the lock).
/// A pre-existing record makes the `CREATE` statement error, which surfaces
/// as a failed statement rather than a transport error.
fn lock_try_create(db: &dyn Conn, holder: &str) -> anyhow::Result<bool> {
    let stmts = db.query(&format!(
        "CREATE {LOCK_TABLE}:main SET holder = '{}', at = time::now();",
        escape_str(holder)
    ))?;
    Ok(stmts.iter().all(|s| s.ok))
}

fn lock_is_stale(db: &dyn Conn) -> anyhow::Result<bool> {
    let v = exec(db, &format!(
        "SELECT <string> (time::now() - at) AS age FROM {LOCK_TABLE}:main WHERE at != NONE;"
    ))?;
    let age = v
        .as_array()
        .and_then(|a| a.first())
        .and_then(|r| r.get("age"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    if age.is_empty() {
        return Ok(true); // no/unreadable lock row: treat as stale
    }
    Ok(duration_secs(&age) > LOCK_STALE_SECS as f64)
}

/// Release the lock only if we still hold it. After a stale-lock steal the
/// original (slow) runner must not delete the thief's lock — an unconditional
/// delete would let a third runner acquire while the thief still migrates,
/// cascading one overlap into an unbounded chain.
fn lock_release(db: &dyn Conn, holder: &str) {
    let _ = db.query(&format!(
        "DELETE {LOCK_TABLE}:main WHERE holder = '{}';",
        escape_str(holder)
    ));
}

/// Fetch raw history rows (shared by both loaders).
fn history_rows(db: &dyn Conn) -> anyhow::Result<Vec<MigrationRecord>> {
    let v = exec(db, &format!(
        "SELECT name, version, snapshot, <string> applied_at AS applied_at FROM {MIGRATION_TABLE} ORDER BY version DESC;"
    ))?;
    Ok(serde_json::from_value(v)?)
}

// ────────────────────────────────────────────────────────────────────────────
// Dependency ordering
// ────────────────────────────────────────────────────────────────────────────

/// Record-link targets of a resource (precomputed by the caller from
/// DocType link fields), self-links excluded.
fn record_deps(reg: &ResourceSpec) -> Vec<String> {
    reg.deps.iter().filter(|d| **d != reg.name).cloned().collect()
}

/// Order resources so each is applied after the resources it record-links to.
fn order_resources(regs: Vec<ResourceSpec>) -> Vec<ResourceSpec> {
    let nodes: Vec<(String, Vec<String>)> =
        regs.iter().map(|r| (r.name.clone(), record_deps(r))).collect();
    let node_refs: Vec<(&str, Vec<&str>)> = nodes
        .iter()
        .map(|(n, d)| (n.as_str(), d.iter().map(String::as_str).collect()))
        .collect();
    let order: Vec<String> = toposort_keys(&node_refs).into_iter().map(str::to_string).collect();
    let mut by_name: std::collections::HashMap<String, ResourceSpec> =
        regs.into_iter().map(|r| (r.name.clone(), r)).collect();
    order.iter().filter_map(|n| by_name.remove(n)).collect()
}

/// Kahn's algorithm over `(name, deps)` nodes. Deps not in the node set are
/// ignored. Cycles fall back to input order for the unresolved remainder.
fn toposort_keys<'a>(nodes: &[(&'a str, Vec<&'a str>)]) -> Vec<&'a str> {
    let names: HashSet<&str> = nodes.iter().map(|(n, _)| *n).collect();
    let mut indeg: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

    for (n, deps) in nodes {
        let n: &str = *n;
        let real: Vec<&str> = deps
            .iter()
            .copied()
            .filter(|&d| d != n && names.contains(d))
            .collect();
        indeg.insert(n, real.len());
        for d in real {
            dependents.entry(d).or_default().push(n);
        }
    }

    let mut queue: VecDeque<&str> = nodes
        .iter()
        .map(|(n, _)| *n)
        .filter(|&n| indeg.get(n).copied().unwrap_or(0) == 0)
        .collect();

    let mut order: Vec<&str> = Vec::with_capacity(nodes.len());
    while let Some(n) = queue.pop_front() {
        order.push(n);
        if let Some(deps) = dependents.get(n) {
            for &m in deps {
                if let Some(e) = indeg.get_mut(m) {
                    *e -= 1;
                    if *e == 0 {
                        queue.push_back(m);
                    }
                }
            }
        }
    }

    if order.len() < nodes.len() {
        for (n, _) in nodes {
            if !order.contains(n) {
                order.push(*n);
            }
        }
    }
    order
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase 2a invariant: classifications are **observable, not authoritative**. A
    /// report carrying a `Blocked` field classification but no `errors` is still
    /// `is_ok()`. When Phase 3 makes classifications authoritative it will edit *this*
    /// test on purpose — a tripwire, not a silent behavior change.
    #[test]
    fn is_ok_ignores_classifications() {
        let blocked = FieldChangeReport {
            entries: vec![FieldChangeEntry {
                field: "body".into(),
                change: FieldChange::Dropped,
                class: FieldClass::Blocked,
                from_type: Some("string".into()),
                to_type: None,
                reason: "field dropped — data loss".into(),
            }],
            summary: ClassSummary { safe: 0, warn: 0, needs_backfill: 0, blocked: 1 },
        };
        assert_eq!(blocked.summary.worst(), Some(FieldClass::Blocked));

        let report = MigrationReport {
            planned: vec![PlannedMigration {
                name: "crm__note".into(),
                next_version: 2,
                statements: vec!["REMOVE FIELD IF EXISTS body ON TABLE note".into()],
                destructive: vec!["REMOVE FIELD body".into()],
                warnings: vec![],
                classifications: blocked,
            }],
            ..Default::default()
        };

        assert!(report.is_ok(), "classifications must not affect is_ok() in Phase 2a");
        assert!(report.errors.is_empty());
    }

    // ---- Phase 3b: the classification gate (pure; no DB) ----

    fn snap(table: &str, fields: &str) -> SchemaSnapshot {
        SchemaSnapshot::from_sql(table, &format!("DEFINE TABLE OVERWRITE {table} SCHEMAFULL{fields}"))
    }

    #[test]
    fn gate_refuses_non_drop_blocked_in_prod_without_ack() {
        // `a`: string -> int = a narrowing (Blocked, non-drop).
        let old = snap("t", "; DEFINE FIELD OVERWRITE a ON TABLE t TYPE string");
        let new = snap("t", "; DEFINE FIELD OVERWRITE a ON TABLE t TYPE int");
        let fcr = classify_diff(&old, &new);

        assert!(classification_refusal(Environment::Prod, false, &fcr).is_some(), "prod refuses narrowing");
        assert!(classification_refusal(Environment::Prod, true, &fcr).is_none(), "acknowledge applies it");
        assert!(classification_refusal(Environment::Dev, false, &fcr).is_none(), "dev is permissive");
    }

    #[test]
    fn gate_refuses_needs_backfill_in_prod_only() {
        // adding a required field with no default = NeedsBackfill (non-drop).
        let old = snap("t", "");
        let new = snap("t", "; DEFINE FIELD OVERWRITE a ON TABLE t TYPE string");
        let fcr = classify_diff(&old, &new);

        assert!(classification_refusal(Environment::Prod, false, &fcr).is_some());
        assert!(classification_refusal(Environment::Test, false, &fcr).is_none());
        assert!(classification_refusal(Environment::Dev, false, &fcr).is_none());
    }

    #[test]
    fn gate_excludes_drops_even_in_prod() {
        // `a` dropped → non-drop summary empty → the classification gate does NOT fire.
        // The drop remains governed solely by `allow_destructive`.
        let old = snap("t", "; DEFINE FIELD OVERWRITE a ON TABLE t TYPE string");
        let new = snap("t", "");
        let fcr = classify_diff(&old, &new);

        assert!(
            classification_refusal(Environment::Prod, false, &fcr).is_none(),
            "drops are not the classification gate's concern"
        );
    }

    #[test]
    fn gate_allows_safe_everywhere() {
        // adding an optional field = Safe.
        let old = snap("t", "");
        let new = snap("t", "; DEFINE FIELD OVERWRITE a ON TABLE t TYPE option<string>");
        let fcr = classify_diff(&old, &new);

        for env in [Environment::Dev, Environment::Test, Environment::Prod] {
            assert!(classification_refusal(env, false, &fcr).is_none(), "safe applies in {env:?}");
        }
    }

    // ---- Phase 3d: boot_action_for_report (pure; report → action) ----

    fn planned_with(class: FieldClass, change: FieldChange, destructive: Vec<&str>) -> PlannedMigration {
        let mut summary = ClassSummary::default();
        match class {
            FieldClass::Safe => summary.safe += 1,
            FieldClass::Warn => summary.warn += 1,
            FieldClass::NeedsBackfill => summary.needs_backfill += 1,
            FieldClass::Blocked => summary.blocked += 1,
        }
        // A drop is classified Blocked-dropped but excluded from non_drop_summary; model
        // that by carrying the entry as Dropped when destructive is set.
        let entry = FieldChangeEntry {
            field: "f".into(),
            change,
            class,
            from_type: Some("string".into()),
            to_type: Some("int".into()),
            reason: "r".into(),
        };
        PlannedMigration {
            name: "crm__note".into(),
            next_version: 1,
            statements: vec![],
            destructive: destructive.into_iter().map(String::from).collect(),
            warnings: vec![],
            classifications: FieldChangeReport { entries: vec![entry], summary },
        }
    }

    fn report_planned(p: PlannedMigration) -> MigrationReport {
        MigrationReport { planned: vec![p], ..Default::default() }
    }

    #[test]
    fn boot_action_for_report_empty_proceeds() {
        let r = MigrationReport::default();
        assert_eq!(boot_action_for_report(&r, Environment::Prod), BootAction::Proceed);
        assert_eq!(boot_action_for_report(&r, Environment::Dev), BootAction::Proceed);
    }

    #[test]
    fn boot_action_for_report_dev_safe_converges_prod_refuses() {
        let r = report_planned(planned_with(FieldClass::Safe, FieldChange::Added, vec![]));
        assert_eq!(boot_action_for_report(&r, Environment::Dev), BootAction::Converge);
        assert_eq!(boot_action_for_report(&r, Environment::Test), BootAction::Converge);
        assert_eq!(boot_action_for_report(&r, Environment::Prod), BootAction::Refuse);
    }

    #[test]
    fn boot_action_for_report_blocked_refuses_everywhere() {
        let r = report_planned(planned_with(FieldClass::Blocked, FieldChange::TypeNarrowed, vec![]));
        for env in [Environment::Dev, Environment::Test, Environment::Prod] {
            assert_eq!(boot_action_for_report(&r, env), BootAction::Refuse, "blocked {env:?}");
        }
    }

    #[test]
    fn boot_action_for_report_drops_refuse_in_dev() {
        // A dropped field: classified Dropped (excluded from non_drop_summary), but the
        // destructive marker drives has_drops → Refuse even in dev.
        let r = report_planned(planned_with(FieldClass::Blocked, FieldChange::Dropped, vec!["REMOVE FIELD f"]));
        assert_eq!(boot_action_for_report(&r, Environment::Dev), BootAction::Refuse);
    }

    const NOTE_V1: &str = "\
DEFINE TABLE OVERWRITE note SCHEMAFULL;
DEFINE FIELD OVERWRITE title ON TABLE note TYPE string;
DEFINE FIELD OVERWRITE body ON TABLE note TYPE string;
DEFINE INDEX OVERWRITE note_title_idx ON TABLE note FIELDS title;
";

    use crate::testkit::{all_ok, mem_db};

    /// `true` if a `CREATE note ...` statement succeeds (no statement error).
    fn create_ok(db: &crate::testkit::TestDb, set: &str) -> bool {
        all_ok(db, &format!("CREATE note SET {set};"))
    }

    /// Apply a forward step from `prev` → `next` for `note`, recording version.
    fn apply_up(db: &dyn Conn, prev: &SchemaSnapshot, next: &SchemaSnapshot, version: i64) {
        let d = schema::diff(prev, next);
        apply_resource(db, "crm__note", next, version, &d.statements("note"), Direction::Up)
            .unwrap();
    }

    #[test]
    fn apply_creates_table_then_reapply_is_noop() {
        let db = mem_db();
        exec(&db, &history_table_schema()).unwrap();

        let v1 = SchemaSnapshot::from_sql("note", NOTE_V1);
        let d = schema::diff(&SchemaSnapshot::default(), &v1);
        apply_resource(&db, "crm__note", &v1, 1, &d.statements("note"), Direction::Up)
            .unwrap();

        // History recorded version 1 with the snapshot.
        let hist = load_history(&db).unwrap();
        let (ver, stored) = hist.get("crm__note").expect("history row");
        assert_eq!(*ver, 1);

        // Re-running the same schema diffs to nothing.
        assert!(schema::diff(stored, &v1).is_empty(), "re-apply must be a no-op");

        // The SCHEMAFULL table actually enforces the defined fields.
        assert!(create_ok(&db, "title = 'a', body = 'b'"));
    }

    #[test]
    fn add_then_remove_field_round_trip() {
        let db = mem_db();
        exec(&db, &history_table_schema()).unwrap();

        // v1
        let v1 = SchemaSnapshot::from_sql("note", NOTE_V1);
        apply_resource(&db, "crm__note", &v1, 1, &schema::diff(&SchemaSnapshot::default(), &v1).statements("note"), Direction::Up)
            .unwrap();
        assert!(create_ok(&db, "title = 'a', body = 'b'"));

        // v2: drop `body` (destructive). The diff produces a REMOVE FIELD.
        let v2_sql = "\
DEFINE TABLE OVERWRITE note SCHEMAFULL;
DEFINE FIELD OVERWRITE title ON TABLE note TYPE string;
DEFINE INDEX OVERWRITE note_title_idx ON TABLE note FIELDS title;
";
        let v2 = SchemaSnapshot::from_sql("note", v2_sql);
        let d = schema::diff(&v1, &v2);
        assert_eq!(d.destructive(), vec!["REMOVE FIELD body"]);
        apply_resource(&db, "crm__note", &v2, 2, &d.statements("note"), Direction::Up)
            .unwrap();

        // body is gone → SCHEMAFULL now rejects a row that sets it.
        assert!(
            !create_ok(&db, "title = 'c', body = 'd'"),
            "body should be undefined after REMOVE FIELD"
        );
        assert!(create_ok(&db, "title = 'c'"));

        let hist = load_history(&db).unwrap();
        assert_eq!(hist.get("crm__note").unwrap().0, 2, "version bumped to 2");
    }

    // ---- Runtime Metadata v1, Stage 0 spike: FLEXIBLE custom-field bag ----
    // (claudedocs/design_runtime-metadata-v1.md — D1/Stage 0)

    /// SurrealDB must accept the FLEXIBLE bag DDL verbatim through the engine's
    /// apply path, keep nested keys under `custom` on a SCHEMAFULL table, and
    /// still reject undeclared top-level fields.
    #[test]
    fn flexible_custom_bag_applies_and_roundtrips() {
        let db = mem_db();
        exec(&db, &history_table_schema()).unwrap();

        // v1: contact without the bag.
        let v1_sql = "\
DEFINE TABLE OVERWRITE contact SCHEMAFULL;
DEFINE FIELD OVERWRITE name ON TABLE contact TYPE string;
";
        let v1 = SchemaSnapshot::from_sql("contact", v1_sql);
        apply_resource(&db, "crm__contact", &v1, 1, &schema::diff(&SchemaSnapshot::default(), &v1).statements("contact"), Direction::Up)
            .unwrap();

        // v2 adds the FLEXIBLE bag — one plain create, applied through the engine.
        let v2_sql = "\
DEFINE TABLE OVERWRITE contact SCHEMAFULL;
DEFINE FIELD OVERWRITE name ON TABLE contact TYPE string;
DEFINE FIELD OVERWRITE custom ON TABLE contact TYPE option<object> FLEXIBLE;
";
        let v2 = SchemaSnapshot::from_sql("contact", v2_sql);
        let d = schema::diff(&v1, &v2);
        assert_eq!(d.creates.len(), 1);
        assert!(d.destructive().is_empty() && d.warnings().is_empty());
        apply_resource(&db, "crm__contact", &v2, 2, &d.statements("contact"), Direction::Up)
            .expect("SurrealDB must accept the FLEXIBLE DDL verbatim");

        // Nested keys under `custom` survive on the SCHEMAFULL table.
        assert!(
            all_ok(&db, "CREATE contact SET name = 'a', custom = { vat_number: 'DE123', nested: { a: 1, tags: ['x', 'y'] } };"),
            "create with nested custom data must succeed"
        );

        let bags_v = exec(&db, "SELECT VALUE custom FROM contact;").unwrap();
        let bags: Vec<serde_json::Value> = bags_v.as_array().cloned().unwrap_or_default();
        assert_eq!(bags.len(), 1);
        let bag = &bags[0];
        assert_eq!(bag["vat_number"], "DE123", "undeclared nested key must persist (FLEXIBLE)");
        assert_eq!(bag["nested"]["a"], 1);
        assert_eq!(bag["nested"]["tags"][0], "x");

        // The bag is optional — a record without it is fine.
        assert!(all_ok(&db, "CREATE contact SET name = 'b';"), "bag must be optional");

        // SCHEMAFULL still rejects undeclared *top-level* fields.
        let rejected = !all_ok(&db, "CREATE contact SET name = 'c', bogus = 1;");
        assert!(rejected, "undeclared top-level field must still be rejected on a SCHEMAFULL table");
    }

    #[test]
    fn lock_excludes_a_second_runner() {
        let db = mem_db();
        lock_acquire(&db, "runner-a").unwrap();
        assert!(
            lock_acquire(&db, "runner-b").is_err(),
            "second acquire must fail while held"
        );
        lock_release(&db, "runner-a");
        lock_acquire(&db, "runner-c").unwrap(); // free again
    }

    #[test]
    fn lock_release_by_a_non_holder_is_a_noop() {
        // The stolen-lock cascade: a slow runner finishing after its lock was
        // stolen must not delete the current holder's lock.
        let db = mem_db();
        lock_acquire(&db, "thief").unwrap();

        lock_release(&db, "slow-original"); // not the holder — must not release
        assert!(
            lock_acquire(&db, "third").is_err(),
            "lock must still be held by 'thief' after a non-holder release"
        );

        lock_release(&db, "thief"); // the real holder releases
        lock_acquire(&db, "third").unwrap();
    }

    /// The crate's atomicity claim, verified live: when a statement inside the
    /// migration transaction fails, neither the earlier DDL nor the history row
    /// may survive. If this test ever fails, SurrealDB does not roll back DDL in
    /// transactions and the engine's "atomic" guarantee (and its docs) must be
    /// re-designed — do not weaken this assertion.
    #[test]
    fn failed_transaction_rolls_back_ddl_and_history() {
        let db = mem_db();
        exec(&db, &history_table_schema()).unwrap();

        let snap = SchemaSnapshot::from_sql("note", NOTE_V1);
        let mut statements = schema::diff(&SchemaSnapshot::default(), &snap).statements("note");
        statements.push("THROW 'boom'".to_string());

        let res = apply_resource(&db, "crm__note", &snap, 1, &statements, Direction::Up);
        assert!(res.is_err(), "a THROW inside the transaction must fail the apply");

        // The history row was in the same transaction — it must not survive.
        assert!(
            load_history(&db).unwrap().is_empty(),
            "history row must roll back with the failed transaction"
        );

        // The DDL must roll back too: `note` absent from the live schema.
        let info: Option<serde_json::Value> = Some(exec(&db, "INFO FOR DB;").unwrap());
        let has_note = info
            .as_ref()
            .and_then(|v| v.get("tables"))
            .and_then(|t| t.get("note"))
            .is_some();
        assert!(!has_note, "DEFINE TABLE must roll back with the failed transaction");
    }

    #[test]
    fn corrupt_history_snapshot_is_a_hard_error() {
        let db = mem_db();
        exec(&db, &history_table_schema()).unwrap();

        // A history row whose snapshot is not valid SchemaSnapshot JSON.
        exec(&db, &format!(
            "CREATE {MIGRATION_TABLE} SET name = 'crm__note', version = 1, \
             snapshot = 'not json {{', direction = 'up', applied_at = time::now();"
        ))
        .unwrap();

        let err = load_history(&db).expect_err("corrupt snapshot must not degrade to default");
        let msg = err.to_string();
        assert!(msg.contains("crm__note") && msg.contains("v1"), "error names the row: {msg}");

        let err = load_full_history(&db).expect_err("full history must also refuse");
        assert!(err.to_string().contains("corrupt schema snapshot"));
    }

    #[test]
    fn corrupt_superseded_snapshot_does_not_block_forward_migrate() {
        // Only the *latest* row per resource must parse for load_history; an old
        // corrupt row is unreachable by forward migrate and must not brick it.
        // (load_full_history — the revert path — still refuses, by design.)
        let db = mem_db();
        exec(&db, &history_table_schema()).unwrap();

        exec(&db, &format!(
            "CREATE {MIGRATION_TABLE} SET name = 'crm__note', version = 1, \
             snapshot = 'not json {{', direction = 'up', applied_at = time::now();"
        ))
        .unwrap();
        let good = serde_json::to_string(&SchemaSnapshot::from_sql("note", NOTE_V1)).unwrap();
        exec(&db, &format!(
            "CREATE {MIGRATION_TABLE} SET name = 'crm__note', version = 2, \
             snapshot = '{}', direction = 'up', applied_at = time::now();",
            crate::resource::escape_str(&good)
        ))
        .unwrap();

        let hist = load_history(&db).expect("latest row parses — history loads");
        assert_eq!(hist.get("crm__note").unwrap().0, 2);
        assert!(load_full_history(&db).is_err(), "revert path still refuses");
    }

    // ── revert (down-migration) ──────────────────────────────────────────────

    const NOTE_V2_PINNED: &str = "\
DEFINE TABLE OVERWRITE note SCHEMAFULL;
DEFINE FIELD OVERWRITE title ON TABLE note TYPE string;
DEFINE FIELD OVERWRITE body ON TABLE note TYPE string;
DEFINE FIELD OVERWRITE pinned ON TABLE note TYPE bool;
DEFINE INDEX OVERWRITE note_title_idx ON TABLE note FIELDS title;
";

    #[test]
    fn revert_of_an_added_field_is_destructive_and_gated() {
        let db = mem_db();
        exec(&db, &history_table_schema()).unwrap();

        let v1 = SchemaSnapshot::from_sql("note", NOTE_V1);
        let v2 = SchemaSnapshot::from_sql("note", NOTE_V2_PINNED);
        apply_up(&db, &SchemaSnapshot::default(), &v1, 1);
        apply_up(&db, &v1, &v2, 2);
        assert!(create_ok(&db, "title='a', body='b', pinned=true"));

        let m = ResourceMigrator::new();

        // Default revert (one step back, no destructive): re-removing `pinned`
        // drops a column → data loss → refused.
        let mut report = MigrationReport::default();
        m.run_revert(&db, &RevertOptions::default(), &mut report).unwrap();
        assert_eq!(report.applied.len(), 0);
        assert_eq!(report.errors.len(), 1, "destructive revert must be gated");

        // With allow_destructive it applies: a new down row at version 3.
        let mut report = MigrationReport::default();
        m.run_revert(
            &db,
            &RevertOptions { allow_destructive: true, ..Default::default() },
            &mut report,
        )
        .unwrap();
        assert_eq!(report.applied.len(), 1);
        assert_eq!(report.applied[0].version, 3);
        assert_eq!(report.applied[0].drops, 1);

        // `pinned` is gone; the current snapshot is back to v1.
        assert!(!create_ok(&db, "title='c', body='d', pinned=true"));
        assert!(create_ok(&db, "title='c', body='d'"));
        let hist = load_history(&db).unwrap();
        let (ver, snap) = hist.get("crm__note").unwrap();
        assert_eq!(*ver, 3);
        assert_eq!(snap, &v1, "current snapshot rolled back to v1");
    }

    #[test]
    fn revert_of_a_dropped_field_re_adds_it_and_is_safe() {
        let db = mem_db();
        exec(&db, &history_table_schema()).unwrap();

        // v1 has body; v2 drops it (a forward destructive change).
        let v1 = SchemaSnapshot::from_sql("note", NOTE_V1);
        let v2_sql = "\
DEFINE TABLE OVERWRITE note SCHEMAFULL;
DEFINE FIELD OVERWRITE title ON TABLE note TYPE string;
DEFINE INDEX OVERWRITE note_title_idx ON TABLE note FIELDS title;
";
        let v2 = SchemaSnapshot::from_sql("note", v2_sql);
        apply_up(&db, &SchemaSnapshot::default(), &v1, 1);
        apply_up(&db, &v1, &v2, 2);
        assert!(!create_ok(&db, "title='a', body='b'"), "body dropped");

        // Reverting a drop re-adds `body` (required, no default) → NeedsBackfill, not
        // destructive. Under Dev policy it applies (with a warning); Prod would refuse
        // it without acknowledgment. We assert it applies, so run under Dev explicitly.
        let m = ResourceMigrator::new();
        let mut report = MigrationReport::default();
        m.run_revert(&db, &RevertOptions::default_for_dev(), &mut report).unwrap();
        assert_eq!(report.errors.len(), 0);
        assert_eq!(report.applied.len(), 1);
        assert_eq!(report.applied[0].creates, 1, "body re-added");
        assert!(create_ok(&db, "title='c', body='d'"), "body accepted again");
    }

    #[test]
    fn revert_to_version_jumps_directly() {
        let db = mem_db();
        exec(&db, &history_table_schema()).unwrap();

        let v1 = SchemaSnapshot::from_sql("note", NOTE_V1);
        let v2 = SchemaSnapshot::from_sql("note", NOTE_V2_PINNED);
        let v3_sql = format!("{NOTE_V2_PINNED}DEFINE FIELD OVERWRITE archived ON TABLE note TYPE bool;\n");
        let v3 = SchemaSnapshot::from_sql("note", &v3_sql);
        apply_up(&db, &SchemaSnapshot::default(), &v1, 1);
        apply_up(&db, &v1, &v2, 2);
        apply_up(&db, &v2, &v3, 3);

        // Jump straight back to the v1 schema in one revert.
        let m = ResourceMigrator::new();
        let mut report = MigrationReport::default();
        m.run_revert(
            &db,
            &RevertOptions { to_version: Some(1), allow_destructive: true, ..Default::default() },
            &mut report,
        )
        .unwrap();
        assert_eq!(report.applied.len(), 1);
        assert_eq!(report.applied[0].version, 4);
        assert_eq!(report.applied[0].drops, 2, "pinned + archived dropped at once");

        let hist = load_history(&db).unwrap();
        let (ver, snap) = hist.get("crm__note").unwrap();
        assert_eq!(*ver, 4);
        assert_eq!(snap, &v1);
    }

    #[test]
    fn revert_at_base_version_is_a_noop() {
        let db = mem_db();
        exec(&db, &history_table_schema()).unwrap();

        let v1 = SchemaSnapshot::from_sql("note", NOTE_V1);
        apply_up(&db, &SchemaSnapshot::default(), &v1, 1);

        // Only the initial version exists — nothing older to revert to.
        let m = ResourceMigrator::new();
        let mut report = MigrationReport::default();
        m.run_revert(&db, &RevertOptions::default(), &mut report).unwrap();
        assert_eq!(report.considered, 1);
        assert_eq!(report.skipped, 1);
        assert!(report.applied.is_empty() && report.errors.is_empty());
    }

    #[test]
    fn dry_run_revert_plans_without_mutating() {
        let db = mem_db();
        exec(&db, &history_table_schema()).unwrap();

        let v1 = SchemaSnapshot::from_sql("note", NOTE_V1);
        let v2 = SchemaSnapshot::from_sql("note", NOTE_V2_PINNED);
        apply_up(&db, &SchemaSnapshot::default(), &v1, 1);
        apply_up(&db, &v1, &v2, 2);

        let m = ResourceMigrator::new();
        let mut report = MigrationReport::default();
        m.run_revert(
            &db,
            &RevertOptions { dry_run: true, ..Default::default() },
            &mut report,
        )
        .unwrap();

        assert_eq!(report.planned.len(), 1, "plan produced");
        assert_eq!(report.applied.len(), 0, "nothing applied on dry-run");
        // Schema untouched: `pinned` still accepted, history still at version 2.
        assert!(create_ok(&db, "title='a', body='b', pinned=true"));
        assert_eq!(load_history(&db).unwrap().get("crm__note").unwrap().0, 2);
    }

    #[test]
    fn toposort_orders_dependencies_first() {
        // comment → post → user
        let nodes = vec![
            ("comment", vec!["post"]),
            ("post", vec!["user"]),
            ("user", vec![]),
        ];
        let order = toposort_keys(&nodes);
        let pos = |n: &str| order.iter().position(|x| *x == n).unwrap();
        assert!(pos("user") < pos("post"));
        assert!(pos("post") < pos("comment"));
    }

    #[test]
    fn toposort_tolerates_cycles_and_unknown_deps() {
        // a ↔ b cycle, plus a dep on a non-existent node.
        let nodes = vec![("a", vec!["b", "ghost"]), ("b", vec!["a"])];
        let order = toposort_keys(&nodes);
        assert_eq!(order.len(), 2, "every node still appears exactly once");
    }
}
