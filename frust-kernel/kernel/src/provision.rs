//! **Tenant provisioning** — WO-040 Chunk C. Turns a
//! [`crate::tenancy::ProvisioningPlan`] into the DDL that creates a tenant's
//! home, and runs it.
//!
//! ## Why this module is on the `surql_monopoly` allowlist
//!
//! A deliberate ruling, not a convenience. Every value this module
//! interpolates is a typed [`NamespaceName`] or [`DatabaseName`], and those
//! types have module-private constructors in `tenancy` that accept nothing but
//! a plain identifier. There is no path by which a client string reaches this
//! text — not because a caller remembers to escape, but because the parameters
//! cannot hold anything else.
//!
//! ## What it does NOT do
//!
//! It does not mint signing keys. `DEFINE ACCESS` here is `access_ddl()` — the
//! same `IF NOT EXISTS` form the meta boot uses, so provisioning a tenant
//! twice never rotates its key and never signs everyone out (ADR-013's
//! reasoning, unchanged).

use crate::contract::BrokerError;
use crate::tenancy::{AccessPlacement, ProvisioningPlan, ProvisioningStep, ResolvedTenant};

/// The statements a plan becomes, in order.
pub fn ddl(plan: &ProvisioningPlan) -> Result<Vec<String>, BrokerError> {
    plan.steps.iter().map(step_ddl).collect()
}

fn step_ddl(step: &ProvisioningStep) -> Result<String, BrokerError> {
    Ok(match step {
        ProvisioningStep::CreateNamespace(ns) => {
            format!("DEFINE NAMESPACE IF NOT EXISTS {ns}")
        }
        ProvisioningStep::CreateDatabase { database, .. } => {
            format!("DEFINE DATABASE IF NOT EXISTS {database}")
        }
        ProvisioningStep::DefineAccess { placement } => match placement {
            AccessPlacement::Database => crate::meta::access_ddl(),
            // Unreachable today and refused loudly rather than rendered into
            // something SurrealDB would reject with a shrug: the Chunk C probe
            // proved `DEFINE ACCESS ... ON NAMESPACE TYPE RECORD` is a 400 on
            // 3.2.0. If a topology ever answers `Namespace`, this is the line
            // that has to be designed, not guessed.
            AccessPlacement::Namespace => {
                return Err(BrokerError::InvalidValue {
                    detail: "namespace-level RECORD access is not supported by SurrealDB 3.2.0 \
                             (probed: HTTP 400); this topology must place its access ON DATABASE"
                        .into(),
                })
            }
        },
    })
}

/// Create this tenant's home, idempotently.
///
/// Ordered deliberately: the namespace, then the database inside it, then the
/// access inside that. Each step runs at the scope it needs — `DEFINE
/// NAMESPACE` cannot run with a database selected that does not exist yet,
/// which is the shape `Db::sql_root_ns` exists for.
pub fn provision(target: &ResolvedTenant) -> Result<(), BrokerError> {
    let db = crate::db::scoped_db(target);
    for step in &target.strategy().provisioning_plan(target).steps {
        let sql = format!("{};", step_ddl(step)?);
        match step {
            // namespace and database creation are namespace-scoped: the
            // database header cannot name something that is not there yet
            ProvisioningStep::CreateNamespace(_) | ProvisioningStep::CreateDatabase { .. } => {
                db.sql_root_ns(&sql)?;
            }
            ProvisioningStep::DefineAccess { .. } => {
                db.sql_root(&sql)?;
            }
        }
    }
    Ok(())
}

/// Every tenant this process serves, provisioned. The whole-instance shape:
/// **enumerate the registry**, which is the same list the router is built
/// from, so nothing can be provisioned that is not served and vice versa.
pub fn provision_all(tenancy: &crate::tenancy::Tenancy) -> Result<usize, BrokerError> {
    let targets = tenancy.resolve_all()?;
    for target in &targets {
        provision(target)?;
    }
    Ok(targets.len())
}

/// A tenant's backup unit, rendered for `surreal export`.
///
/// Returns the `--ns` / `--db` pair an operator (or a script) needs, plus
/// whether restoring it touches anyone else. **The isolation flag is the
/// answer to the DR question**, and it comes from the strategy rather than
/// from an assumption about the shape.
pub fn export_target(target: &ResolvedTenant) -> (String, Option<String>, bool) {
    let plan = target.strategy().backup_plan(target);
    (
        plan.namespace.as_str().to_string(),
        plan.database.as_ref().map(|d| d.as_str().to_string()),
        plan.is_tenant_isolated(),
    )
}

/// Empty and recreate the database selected by the tenancy strategy before a
/// restore. `surreal import` is additive, so importing over the live database
/// is not a restore and can fail after applying only part of a dump.
///
/// The database name is not accepted as a string: it comes from the same
/// private-constructor target used by every request and backup plan. Recovery
/// performs its confirmation and safety-backup checks before reaching here.
pub(crate) fn reset_database_for_restore(target: &ResolvedTenant) -> Result<(), BrokerError> {
    let plan = target.strategy().backup_plan(target);
    if !plan.is_tenant_isolated() {
        return Err(BrokerError::InvalidValue {
            detail: "tenant restore refused: the strategy's database contains shared tenant data"
                .into(),
        });
    }
    let database = plan.database.ok_or_else(|| BrokerError::InvalidValue {
        detail: "restore requires a database-scoped backup plan".into(),
    })?;
    crate::db::scoped_db(target).sql_root_ns(&format!(
        "REMOVE DATABASE {database}; DEFINE DATABASE {database};"
    ))?;
    Ok(())
}

/// Replace SurrealDB's exported `[REDACTED]` access key immediately after an
/// import. The random value is produced inside the database, never printed,
/// and the access replacement is one DDL statement.
pub(crate) fn rotate_restored_access(target: &ResolvedTenant) -> Result<(), BrokerError> {
    let db = crate::db::scoped_db(target);
    let secret = db.sql_root("RETURN rand::string(64);")?;
    let secret = secret.as_str().ok_or_else(|| BrokerError::Db {
        detail: "database did not return a signing-key string".into(),
    })?;
    if secret.len() != 64 || !secret.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(BrokerError::Db {
            detail: "database returned an invalid signing-key value".into(),
        });
    }

    // `access` is configuration rather than request data, but it still passes
    // through the compiler's identifier validator before entering DDL.
    let access =
        crate::surql::render_path(&[crate::contract::PathSegment::Field(db.access().to_string())])?;
    let key = crate::surql::escape_str(secret);
    db.sql_root(&format!(
        "DEFINE ACCESS OVERWRITE {access} ON DATABASE TYPE RECORD \
         SIGNIN (SELECT * FROM app_user WHERE name = $name AND \
         crypto::argon2::compare(pass, $pass)) WITH JWT ALGORITHM HS512 KEY '{key}' \
         DURATION FOR TOKEN 1h, FOR SESSION 12h;"
    ))?;
    Ok(())
}
