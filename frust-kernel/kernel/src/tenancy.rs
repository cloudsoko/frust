//! **The tenancy seam** — WO-040 Chunk A, under [[ADR-003 Tenancy Model]] as
//! amended.
//!
//! The framing this module exists to make true in code:
//!
//! > Tenancy is not a feature. **Tenancy topology is a strategy**, and
//! > *resolution* is one operation that strategy performs.
//!
//! One binary serves every topology; which one is chosen at startup by
//! validated config (`FRUST_TENANCY`). Chunk A builds the seam and migrates
//! today's database-per-tenant behaviour onto it — **no new topology yet**.
//!
//! ## The monopoly
//!
//! Nothing outside this module may decide which namespace or database a query
//! lands in. That is enforced three ways, and `tests/tenancy_monopoly.rs`
//! fails the build when a new bypass appears:
//!
//! 1. **Private constructors.** [`TenantId`], [`NamespaceName`],
//!    [`DatabaseName`] and [`ResolvedTenant`] all have module-private
//!    constructors. A `String` cannot become a target outside this file.
//! 2. **One door.** [`crate::db::scoped_db`] takes a `&ResolvedTenant` and
//!    there is no name-taking overload beside it.
//! 3. **Registry-validated identity.** Untrusted input reaches a target only
//!    through [`Tenancy::resolve`], which looks the caller's slug up in the
//!    registry and hands the *canonical* [`TenantId`] to the strategy. A raw
//!    client string is never substituted into a database name.
//!
//! Point 3 is also the runway for the earlier opaque-token ruling: because
//! nothing outside the registry constructs a `TenantId`, swapping today's
//! "canonical id == slug" for an opaque, non-enumerable id is a change to
//! `Tenancy::register` alone — no caller moves.

use std::collections::HashMap;
use std::sync::Arc;

use crate::contract::BrokerError;
use crate::db::ConnConfig;

/// Long enough for a readable slug, short enough that a name cannot be used
/// as a smuggling channel.
const MAX_NAME: usize = 64;

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME
        && s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A name that is checked once, here, and typed thereafter.
///
/// `String` is what let a raw request field become a database name; these
/// types are what stop it. The constructor is module-private on purpose.
macro_rules! typed_name {
    ($(#[$doc:meta])* $t:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $t(String);

        impl $t {
            fn parse(raw: &str) -> Result<Self, BrokerError> {
                if !valid_name(raw) {
                    return Err(BrokerError::InvalidValue {
                        detail: format!("{}: not a valid name: {raw:?}", stringify!($t)),
                    });
                }
                Ok(Self(raw.to_string()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $t {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

typed_name! {
    /// A tenant's **canonical** identity, minted only by the registry.
    ///
    /// Deliberately not the client's slug by contract — today they happen to
    /// coincide, and the type is what lets that stop being true without
    /// touching a single caller.
    TenantId
}
typed_name! {
    /// A SurrealDB namespace. Chosen by a strategy, never by a request.
    NamespaceName
}
typed_name! {
    /// A SurrealDB database. Chosen by a strategy, never by a request.
    DatabaseName
}

/// Where a topology puts a tenant. Plain data: the strategy computes it,
/// [`Tenancy`] mints the [`ResolvedTenant`] around it.
#[derive(Debug, Clone)]
pub struct TenantPlacement {
    pub namespace: NamespaceName,
    pub database: DatabaseName,
    pub environment: Option<String>,
}

/// **A request's target: who, and where.**
///
/// The only thing [`crate::db::scoped_db`] accepts, and constructible only by
/// this module. Cloning is cheap (the connection config and strategy are
/// shared) and — the invariant that matters — a clone is *independent*: there
/// is no mutable "current tenant" anywhere for one request to move under
/// another. `tests/tenant_isolation_concurrent.rs` is that claim under load.
#[derive(Clone)]
pub struct ResolvedTenant {
    tenant_id: TenantId,
    namespace: NamespaceName,
    database: DatabaseName,
    environment: Option<String>,
    conn: Arc<ConnConfig>,
    strategy: Arc<dyn TenancyStrategy>,
}

impl ResolvedTenant {
    pub fn tenant_id(&self) -> &TenantId {
        &self.tenant_id
    }

    pub fn namespace(&self) -> &NamespaceName {
        &self.namespace
    }

    pub fn database(&self) -> &DatabaseName {
        &self.database
    }

    /// The deployment environment this tenant's data sits in, when the
    /// topology separates them (Chunk C's `namespace-per-tenant-env`).
    pub fn environment(&self) -> Option<&str> {
        self.environment.as_deref()
    }

    /// The SurrealDB endpoint. Transport, not placement — safe to read.
    pub fn endpoint(&self) -> &str {
        &self.conn.endpoint
    }

    pub fn strategy(&self) -> &Arc<dyn TenancyStrategy> {
        &self.strategy
    }

    pub(crate) fn conn(&self) -> &ConnConfig {
        &self.conn
    }
}

impl std::fmt::Debug for ResolvedTenant {
    /// Never prints `conn`: it carries root credentials.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedTenant")
            .field("tenant_id", &self.tenant_id)
            .field("namespace", &self.namespace)
            .field("database", &self.database)
            .field("environment", &self.environment)
            .field("strategy", &self.strategy.name())
            .finish()
    }
}

/// **A deployment environment, as a closed set.**
///
/// The invariant-3 discipline extended to the environment axis: a raw string
/// never becomes a database name. `FRUST_ENVIRONMENT=prod-2` is refused at
/// boot rather than silently creating a database called `prod-2` that nobody
/// provisioned and no backup plan knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentEnvironment {
    Production,
    Sandbox,
    Test,
}

impl DeploymentEnvironment {
    fn parse(raw: &str) -> Result<Self, BrokerError> {
        match raw {
            "production" => Ok(Self::Production),
            "sandbox" => Ok(Self::Sandbox),
            "test" => Ok(Self::Test),
            other => Err(BrokerError::InvalidValue {
                detail: format!(
                    "FRUST_ENVIRONMENT={other}: not a deployment environment \
                     (production | sandbox | test)"
                ),
            }),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Sandbox => "sandbox",
            Self::Test => "test",
        }
    }

    /// Every environment, for ops paths that must cover a tenant whole.
    pub fn all() -> [Self; 3] {
        [Self::Production, Self::Sandbox, Self::Test]
    }

    fn database(&self) -> DatabaseName {
        DatabaseName(self.as_str().to_string())
    }
}

impl std::fmt::Display for DeploymentEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a topology defines its record-access method (and therefore where the
/// signing key lives — ADR-013's keyguard follows this).
///
/// ## Probe result (WO-040 Chunk C, SurrealDB 3.2.0)
///
/// **`DEFINE ACCESS ... ON NAMESPACE TYPE RECORD` is refused (HTTP 400).**
/// `TYPE JWT` at namespace level is accepted, and `TYPE RECORD ON DATABASE`
/// is accepted (the control). So RECORD access — which is what the kernel's
/// whole auth model is, `SIGNIN (SELECT * FROM app_user ...)` against a table,
/// and tables live in databases — **cannot** be hosted at namespace level.
///
/// Every strategy therefore answers [`AccessPlacement::Database`] today, and
/// the namespace topologies get a **separate signing key per database** as a
/// consequence. Under `namespace-per-tenant-env` that is a security *gain*: a
/// sandbox token cannot authenticate against production even though both live
/// in the tenant's namespace.
///
/// [`AccessPlacement::Namespace`] is kept because the question is real and
/// namespace-level **JWT** access does work — a future design where the kernel
/// issues its own tokens could use it. Nothing returns it today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessPlacement {
    /// One access per database: a signing key per database.
    Database,
    /// One access per namespace, shared by the databases under it. Not
    /// reachable with RECORD access on SurrealDB 3.2.0 — see above.
    Namespace,
}

/// What a restore of *this tenant alone* has to cover.
///
/// WO-027 measured the cost of getting this wrong (restore-one = restore-all)
/// and WO-039 proved per-database export works; this is that knowledge given
/// a type, so Chunk D's ops path reads it off the strategy instead of
/// assuming a shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupPlan {
    pub namespace: NamespaceName,
    /// `None` means the whole namespace is the unit.
    pub database: Option<DatabaseName>,
    /// **Does restoring this unit touch only this tenant?**
    ///
    /// Deliberately a field the strategy sets, not something inferred from
    /// the shape above: under `single` the unit *is* a database and the
    /// answer is still **no**, because that database holds every tenant.
    /// Inferring isolation from "a database is named" would report
    /// restore-one where WO-027 measured restore-all.
    pub tenant_isolated: bool,
}

impl BackupPlan {
    /// True when this tenant can be restored without touching another.
    pub fn is_tenant_isolated(&self) -> bool {
        self.tenant_isolated
    }
}

/// Provisioning as *data*, not query text — which is also why this module
/// stays outside `surql_monopoly`'s allowlist. The executor renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvisioningStep {
    CreateNamespace(NamespaceName),
    CreateDatabase { namespace: NamespaceName, database: DatabaseName },
    DefineAccess { placement: AccessPlacement },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisioningPlan {
    pub steps: Vec<ProvisioningStep>,
}

/// **The seam.** One trait, one implementation per topology.
///
/// Every question the kernel used to answer by reading `cfg.db` is a method
/// here, so adding a topology is adding an impl — not hunting for the places
/// that assumed the old one.
///
/// **Two deviations from the shape sketched in ADR-003, both deliberate and
/// both flagged rather than quietly taken:**
///
/// 1. `resolve` takes a canonical [`TenantId`] and returns a
///    [`TenantPlacement`], not a `TenantRequest` → `ResolvedTenant`.
///    [`Tenancy`] does the registry lookup and mints the target. This is
///    *stronger* on invariants 3 and 4: the strategy never sees untrusted
///    input, and `ResolvedTenant`'s constructor stays private to this module
///    rather than merely crate-private, so no other kernel module can mint a
///    target even by accident.
/// 2. `backup_plan` / `provisioning_plan` take a `&ResolvedTenant` and cannot
///    fail. Both need placement, so taking a bare id would mean re-resolving
///    inside — and a plan for a tenant nobody resolved is not a question worth
///    being able to ask.
pub trait TenancyStrategy: Send + Sync {
    /// The configured name, echoed in boot telemetry so an operator can see
    /// which topology a process actually chose.
    fn name(&self) -> &'static str;

    /// Where this canonical tenant's data lives under this topology.
    fn resolve(&self, id: &TenantId) -> Result<TenantPlacement, BrokerError>;

    fn access_placement(&self) -> AccessPlacement;

    fn backup_plan(&self, target: &ResolvedTenant) -> BackupPlan;

    fn provisioning_plan(&self, target: &ResolvedTenant) -> ProvisioningPlan;

    /// Whether schema must be applied per tenant (the migration engine's
    /// question — `frust_orm::Tenancy::requires_per_tenant_schema_deploy`).
    fn per_tenant_schema(&self) -> bool;
}

/// **Today's topology**: one SurrealDB database per tenant, all under one
/// namespace. This is what WO-039 found the kernel had already been modelling
/// (`Db::tenant()` returned `cfg.db`); Chunk A moves it behind the seam
/// without changing a single thing it does.
struct DatabasePerTenant {
    namespace: NamespaceName,
}

impl TenancyStrategy for DatabasePerTenant {
    fn name(&self) -> &'static str {
        "database-per-tenant"
    }

    fn resolve(&self, id: &TenantId) -> Result<TenantPlacement, BrokerError> {
        // The database name is the CANONICAL id, which only the registry
        // mints — not a slug that arrived on a request.
        Ok(TenantPlacement {
            namespace: self.namespace.clone(),
            database: DatabaseName::parse(id.as_str())?,
            environment: None,
        })
    }

    fn access_placement(&self) -> AccessPlacement {
        // Per-tenant database => per-tenant signing key. WO-039 proved this
        // is what makes isolation DB-enforced rather than kernel-enforced.
        AccessPlacement::Database
    }

    fn backup_plan(&self, target: &ResolvedTenant) -> BackupPlan {
        BackupPlan {
            namespace: target.namespace.clone(),
            database: Some(target.database.clone()),
            // WO-039 measured this: `surreal export --db <tenant>` restores
            // one tenant and leaves the others, including writes made after
            // the export. This is the P-8.1 unlock, as data.
            tenant_isolated: true,
        }
    }

    fn provisioning_plan(&self, target: &ResolvedTenant) -> ProvisioningPlan {
        ProvisioningPlan {
            steps: vec![
                ProvisioningStep::CreateDatabase {
                    namespace: target.namespace.clone(),
                    database: target.database.clone(),
                },
                ProvisioningStep::DefineAccess { placement: self.access_placement() },
            ],
        }
    }

    fn per_tenant_schema(&self) -> bool {
        // Model-correct: a database per tenant means schema lands in each.
        // Consumed only by `frust_orm`'s `migrate_fleet_with`, which the
        // kernel does not call today (it migrates one tenant scope per
        // process), so this is a truthful answer that is currently inert —
        // stated rather than left false to avoid thinking about it.
        true
    }
}

/// **The v0 topology, kept as a first-class strategy**: one namespace, one
/// database, every tenant inside it. Tenancy is logical — a column, not a
/// boundary.
///
/// This is what Frust actually shipped before Milestone 4, and it exists here
/// so "single tenant" is *just another resolver*. Nothing downstream may ask
/// which topology it is running under; `tenancy_monopoly` fails the build if
/// a strategy name appears outside this module.
///
/// Its defining behaviour is the one thing `DatabasePerTenant` will never do:
/// **two different tenants resolve to the same database.**
struct SingleTenant {
    namespace: NamespaceName,
    database: DatabaseName,
}

impl TenancyStrategy for SingleTenant {
    fn name(&self) -> &'static str {
        "single"
    }

    fn resolve(&self, _id: &TenantId) -> Result<TenantPlacement, BrokerError> {
        // the id does not participate: that IS the topology
        Ok(TenantPlacement {
            namespace: self.namespace.clone(),
            database: self.database.clone(),
            environment: None,
        })
    }

    fn access_placement(&self) -> AccessPlacement {
        AccessPlacement::Database
    }

    fn backup_plan(&self, target: &ResolvedTenant) -> BackupPlan {
        BackupPlan {
            namespace: target.namespace.clone(),
            database: Some(target.database.clone()),
            // **restore-one is restore-all** — WO-027's finding, which is the
            // whole reason database-per-tenant was built. An operator asking
            // this strategy for a single-tenant restore must be told no.
            tenant_isolated: false,
        }
    }

    fn provisioning_plan(&self, target: &ResolvedTenant) -> ProvisioningPlan {
        ProvisioningPlan {
            steps: vec![
                ProvisioningStep::CreateDatabase {
                    namespace: target.namespace.clone(),
                    database: target.database.clone(),
                },
                ProvisioningStep::DefineAccess { placement: self.access_placement() },
            ],
        }
    }

    fn per_tenant_schema(&self) -> bool {
        // one database, so schema deploys once
        false
    }
}

/// Startup tenancy settings, as an operator supplies them.
///
/// A parameter object rather than four positional `&str`s: `strategy`,
/// `namespace` and `database` are all strings, and getting two of them the
/// wrong way round would be a silent misconfiguration of exactly the kind
/// [`Tenancy::from_config`] exists to refuse.
pub struct TenancyConfig<'a> {
    pub strategy: &'a str,
    /// The namespace every tenant shares. Required by the database-scoped
    /// topologies; **rejected** by the namespace topologies, where the
    /// namespace *is* the tenant and an operator supplying one believes
    /// something untrue about their own layout. `None` means "unset", which
    /// the database-scoped topologies fill with the default.
    pub namespace: Option<&'a str>,
    /// The database name. Required by `single` (the shared database) and by
    /// `namespace-per-tenant` (the one database inside each tenant's
    /// namespace); **rejected** by the other two, which derive it.
    pub database: Option<&'a str>,
    /// Required by `namespace-per-tenant-env`; **rejected** by the rest.
    pub environment: Option<&'a str>,
    /// The tenants this process may serve. Registration is provisioning —
    /// the caller is startup config, never a request.
    pub tenants: &'a [&'a str],
}

/// **A namespace per tenant**, one database inside it.
///
/// The tenant owns a whole SurrealDB namespace, which is a wider boundary than
/// a database: a per-tenant backup is "this namespace", and nothing of another
/// tenant's is inside it. The record access still lives ON DATABASE — see
/// [`AccessPlacement`] for the probe that settled that.
struct NamespacePerTenant {
    database: DatabaseName,
}

impl TenancyStrategy for NamespacePerTenant {
    fn name(&self) -> &'static str {
        "namespace-per-tenant"
    }

    fn resolve(&self, id: &TenantId) -> Result<TenantPlacement, BrokerError> {
        // the NAMESPACE is the canonical tenant id; the database is fixed
        Ok(TenantPlacement {
            namespace: NamespaceName::parse(id.as_str())?,
            database: self.database.clone(),
            environment: None,
        })
    }

    fn access_placement(&self) -> AccessPlacement {
        AccessPlacement::Database
    }

    fn backup_plan(&self, target: &ResolvedTenant) -> BackupPlan {
        BackupPlan {
            namespace: target.namespace.clone(),
            database: Some(target.database.clone()),
            // `surreal export` is per-database, and this tenant's namespace
            // contains exactly one — so restoring it touches nobody else.
            tenant_isolated: true,
        }
    }

    fn provisioning_plan(&self, target: &ResolvedTenant) -> ProvisioningPlan {
        ProvisioningPlan {
            steps: vec![
                ProvisioningStep::CreateNamespace(target.namespace.clone()),
                ProvisioningStep::CreateDatabase {
                    namespace: target.namespace.clone(),
                    database: target.database.clone(),
                },
                ProvisioningStep::DefineAccess { placement: self.access_placement() },
            ],
        }
    }

    fn per_tenant_schema(&self) -> bool {
        true
    }
}

/// **A namespace per tenant, a database per environment inside it.**
///
/// The SaaS shape: `acme` owns a namespace holding `production`, `sandbox` and
/// `test`. A process is pinned to ONE environment by config and can address no
/// other — which is the point. Serving prod and sandbox from one process would
/// make "a routing bug wrote prod data from a sandbox request" a reachable
/// state; separate processes make it unreachable, the same no-surface
/// reasoning that shaped the rest of this seam.
struct NamespacePerTenantEnvironment {
    environment: DeploymentEnvironment,
}

impl TenancyStrategy for NamespacePerTenantEnvironment {
    fn name(&self) -> &'static str {
        "namespace-per-tenant-env"
    }

    fn resolve(&self, id: &TenantId) -> Result<TenantPlacement, BrokerError> {
        Ok(TenantPlacement {
            namespace: NamespaceName::parse(id.as_str())?,
            // from the ENUM, never from a string an operator typed
            database: self.environment.database(),
            environment: Some(self.environment.as_str().to_string()),
        })
    }

    fn access_placement(&self) -> AccessPlacement {
        // per-database => a separate signing key per environment. A sandbox
        // token cannot authenticate against production.
        AccessPlacement::Database
    }

    fn backup_plan(&self, target: &ResolvedTenant) -> BackupPlan {
        BackupPlan {
            namespace: target.namespace.clone(),
            database: Some(target.database.clone()),
            // one environment of one tenant, restorable alone — which is
            // finer-grained than "the tenant", and still touches nobody else
            tenant_isolated: true,
        }
    }

    fn provisioning_plan(&self, target: &ResolvedTenant) -> ProvisioningPlan {
        ProvisioningPlan {
            steps: vec![
                ProvisioningStep::CreateNamespace(target.namespace.clone()),
                ProvisioningStep::CreateDatabase {
                    namespace: target.namespace.clone(),
                    database: target.database.clone(),
                },
                ProvisioningStep::DefineAccess { placement: self.access_placement() },
            ],
        }
    }

    fn per_tenant_schema(&self) -> bool {
        true
    }
}

/// The process's tenancy: a chosen strategy plus the registry of tenants it
/// is allowed to serve.
pub struct Tenancy {
    strategy: Arc<dyn TenancyStrategy>,
    conn: Arc<ConnConfig>,
    /// Client-facing slug → canonical id. **The only door from untrusted
    /// input to a target.**
    registry: HashMap<String, TenantId>,
}

impl std::fmt::Debug for Tenancy {
    /// Prints the topology and how many tenants are admitted — never the
    /// connection config (root credentials) and never the roster (a tenant
    /// list in a log line is a customer list).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tenancy")
            .field("strategy", &self.strategy.name())
            .field("registered", &self.registry.len())
            .finish()
    }
}

/// The default namespace and tenant. **The only place in the kernel that
/// names either** — every other module reads them off a `ResolvedTenant`, and
/// `tests/tenancy_monopoly.rs` fails the build if a literal reappears
/// elsewhere.
const DEFAULT_NAMESPACE: &str = "frust";
pub const DEFAULT_TENANT: &str = "skeleton";

impl Tenancy {
    /// Build from **validated** startup config.
    ///
    /// Fail-closed twice over: an unknown topology name refuses, and a
    /// topology that is *named but not built into this binary* refuses too.
    /// Falling back to database-per-tenant for a config that asked for
    /// something else would be exactly the silent misbehaviour the house
    /// rules forbid — an operator would believe they had isolation they do
    /// not have.
    pub fn from_config(conn: ConnConfig, cfg: TenancyConfig<'_>) -> Result<Self, BrokerError> {
        // The roster is validated BEFORE the topology, so "three tenants
        // under `single`" is reported as the contradiction it is rather than
        // as a resolution failure later.
        let mut registry = HashMap::new();
        for slug in cfg.tenants {
            let id = TenantId::parse(slug)?;
            if registry.insert((*slug).to_string(), id).is_some() {
                return Err(BrokerError::InvalidValue {
                    detail: format!("tenant {slug:?} is listed twice; a duplicate is a typo, and \
                                     silently de-duplicating it would hide the mistake"),
                });
            }
        }
        let refuse = |detail: String| Err(BrokerError::InvalidValue { detail });
        if registry.is_empty() {
            return refuse(format!("{}: no tenants configured", cfg.strategy));
        }

        // Every setting a topology does not use is a CONTRADICTION, not a
        // spare part. An operator who sets `FRUST_NS` under a namespace
        // topology, or `FRUST_DATABASE` under database-per-tenant, believes
        // something untrue about their own layout — and ignoring the setting
        // would leave them believing it.
        let reject = |name: &str, value: Option<&str>, why: &str| -> Result<(), BrokerError> {
            match value {
                Some(v) => Err(BrokerError::InvalidValue {
                    detail: format!(
                        "{} does not take {name} (got {v:?}): {why} — this config contradicts \
                         itself",
                        cfg.strategy
                    ),
                }),
                None => Ok(()),
            }
        };
        let shared_namespace = || -> Result<NamespaceName, BrokerError> {
            NamespaceName::parse(cfg.namespace.unwrap_or(DEFAULT_NAMESPACE))
        };

        let strategy: Arc<dyn TenancyStrategy> = match cfg.strategy {
            "database-per-tenant" => {
                reject("a shared database", cfg.database, "each tenant's database is its own")?;
                reject("an environment", cfg.environment, "this topology has no environment axis")?;
                Arc::new(DatabasePerTenant { namespace: shared_namespace()? })
            }

            "single" => {
                reject("an environment", cfg.environment, "this topology has no environment axis")?;
                let Some(db) = cfg.database else {
                    return refuse(
                        "single: a shared-database topology with no database named is incomplete \
                         (set FRUST_DATABASE)"
                            .into(),
                    );
                };
                Arc::new(SingleTenant {
                    namespace: shared_namespace()?,
                    database: DatabaseName::parse(db)?,
                })
            }

            "namespace-per-tenant" => {
                reject("a namespace", cfg.namespace, "the namespace IS the tenant")?;
                reject("an environment", cfg.environment, "this topology has no environment axis")?;
                let Some(db) = cfg.database else {
                    return refuse(
                        "namespace-per-tenant: no database named for the one database inside each \
                         tenant's namespace (set FRUST_DATABASE)"
                            .into(),
                    );
                };
                Arc::new(NamespacePerTenant { database: DatabaseName::parse(db)? })
            }

            "namespace-per-tenant-env" => {
                reject("a namespace", cfg.namespace, "the namespace IS the tenant")?;
                reject("a database", cfg.database, "the database IS the environment")?;
                let Some(env) = cfg.environment else {
                    return refuse(
                        "namespace-per-tenant-env: no environment named, so this process does not \
                         know whether it is production (set FRUST_ENVIRONMENT)"
                            .into(),
                    );
                };
                Arc::new(NamespacePerTenantEnvironment {
                    environment: DeploymentEnvironment::parse(env)?,
                })
            }

            other => return refuse(format!("FRUST_TENANCY={other}: unknown tenancy topology")),
        };

        Ok(Self { strategy, conn: Arc::new(conn), registry })
    }

    /// Startup config from the environment: `FRUST_TENANCY`, `FRUST_NS`,
    /// `FRUST_DATABASE`.
    ///
    /// `FRUST_DATABASE` is passed through **unset when it is unset**, so
    /// `FRUST_TENANCY=single` with no database named refuses to boot instead
    /// of quietly picking one.
    pub fn from_env(conn: ConnConfig, tenants: &[&str]) -> Result<Self, BrokerError> {
        let strategy =
            std::env::var("FRUST_TENANCY").unwrap_or_else(|_| "database-per-tenant".to_string());
        // each passed through UNSET when it is unset, so a topology that does
        // not take a setting can refuse a config that supplies one
        let ns = std::env::var("FRUST_NS").ok();
        let database = std::env::var("FRUST_DATABASE").ok();
        let environment = std::env::var("FRUST_ENVIRONMENT").ok();
        Self::from_config(
            conn,
            TenancyConfig {
                strategy: &strategy,
                namespace: ns.as_deref(),
                database: database.as_deref(),
                environment: environment.as_deref(),
                tenants,
            },
        )
    }

    pub fn strategy(&self) -> &Arc<dyn TenancyStrategy> {
        &self.strategy
    }

    pub fn registered(&self) -> usize {
        self.registry.len()
    }

    /// **Request-time resolution.** Untrusted slug → registry → canonical id
    /// → strategy. An unregistered slug is refused with one indistinguishable
    /// error, so this cannot be used to enumerate tenants.
    pub fn resolve(&self, slug: &str) -> Result<ResolvedTenant, BrokerError> {
        let id = self
            .registry
            .get(slug)
            .ok_or_else(|| BrokerError::PermissionDenied { detail: "E_UNKNOWN_TENANT".into() })?;
        self.target(id)
    }

    /// The one registered tenant.
    ///
    /// **The standing limit, and it is not a chunk boundary — it is a gap.**
    /// The resident kernel serves exactly one tenant per process, so `frust
    /// serve` resolves once at boot rather than per request. Chunk A named
    /// this; Chunk B removed the *cache* coupling but not this one, and the
    /// rescoped Chunk C is namespace topologies. **No chunk currently owns
    /// per-request routing.** `main` refuses a roster larger than one instead
    /// of serving a subset, so the limit is enforced rather than assumed —
    /// but it is still a limit, and it wants sequencing.
    pub fn sole(&self) -> Result<ResolvedTenant, BrokerError> {
        let mut it = self.registry.values();
        match (it.next(), it.next()) {
            (Some(id), None) => self.target(id),
            _ => Err(BrokerError::InvalidValue {
                detail: format!("sole(): {} tenants registered, expected exactly 1", self.registry.len()),
            }),
        }
    }

    /// Every registered tenant, resolved. **The router is built from this**,
    /// which is what makes "the request path found a context" mean "the
    /// strategy resolved this tenant" — by construction rather than by
    /// discipline.
    pub fn resolve_all(&self) -> Result<Vec<ResolvedTenant>, BrokerError> {
        self.registry.values().map(|id| self.target(id)).collect()
    }

    fn target(&self, id: &TenantId) -> Result<ResolvedTenant, BrokerError> {
        let placement = self.strategy.resolve(id)?;
        Ok(ResolvedTenant {
            tenant_id: id.clone(),
            namespace: placement.namespace,
            database: placement.database,
            environment: placement.environment,
            conn: Arc::clone(&self.conn),
            strategy: Arc::clone(&self.strategy),
        })
    }
}

/// The single-tenant startup path in one call — `frust serve`'s shape today,
/// and every test's. Registration is provisioning; the slug comes from
/// config, not from a request.
/// A dedicated database for one tenant.
///
/// Explicitly `database-per-tenant`, **not** `from_env`: this helper means
/// "give me my own database", so it must not change topology because of an
/// environment variable set for something else.
pub fn single_tenant(slug: &str) -> Result<ResolvedTenant, BrokerError> {
    Tenancy::from_config(
        ConnConfig::default(),
        TenancyConfig {
            strategy: "database-per-tenant",
            namespace: Some(DEFAULT_NAMESPACE),
            database: None,
            environment: None,
            tenants: &[slug],
        },
    )?
    .sole()
}

#[cfg(test)]
pub(crate) fn test_tenancy(
    strategy: &str,
    database: Option<&str>,
) -> Result<Tenancy, BrokerError> {
    Tenancy::from_config(
        ConnConfig::default(),
        TenancyConfig {
            strategy,
            namespace: None,
            database,
            environment: None,
            tenants: &[DEFAULT_TENANT],
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg<'a>(
        strategy: &'a str,
        database: Option<&'a str>,
        tenants: &'a [&'a str],
    ) -> TenancyConfig<'a> {
        TenancyConfig { strategy, namespace: None, database, environment: None, tenants }
    }

    /// A namespace topology: no namespace and no database of our choosing.
    fn ns_cfg<'a>(
        strategy: &'a str,
        database: Option<&'a str>,
        environment: Option<&'a str>,
        tenants: &'a [&'a str],
    ) -> TenancyConfig<'a> {
        TenancyConfig { strategy, namespace: None, database, environment, tenants }
    }

    fn build(c: TenancyConfig<'_>) -> Result<Tenancy, BrokerError> {
        Tenancy::from_config(ConnConfig::default(), c)
    }

    fn tenancy(tenants: &[&str]) -> Result<Tenancy, BrokerError> {
        build(cfg("database-per-tenant", None, tenants))
    }

    fn err(c: TenancyConfig<'_>) -> String {
        format!("{:?}", build(c).unwrap_err())
    }

    /// Fail-closed on the topology name, both ways round. The second case is
    /// the one that matters: `namespace-per-tenant` is a REAL topology name,
    /// so quietly serving something else would leave an operator believing
    /// they had an isolation posture they do not have.
    /// All four topologies build; only a name that is not one refuses.
    #[test]
    fn every_declared_topology_is_now_built() {
        assert!(tenancy(&["a"]).is_ok());
        assert!(build(cfg("single", Some("shared"), &["a"])).is_ok());
        assert!(build(ns_cfg("namespace-per-tenant", Some("app"), None, &["a"])).is_ok());
        assert!(build(ns_cfg("namespace-per-tenant-env", None, Some("sandbox"), &["a"])).is_ok());

        assert!(err(cfg("elastic", None, &["a"])).contains("unknown tenancy topology"));
    }

    /// **The environment axis is a closed set.** A raw string never becomes a
    /// database name — `FRUST_ENVIRONMENT=prod-2` would otherwise create a
    /// database nobody provisioned and no backup plan knows about.
    #[test]
    fn only_a_known_environment_is_accepted() {
        for good in ["production", "sandbox", "test"] {
            let t = build(ns_cfg("namespace-per-tenant-env", None, Some(good), &["acme"]))
                .unwrap_or_else(|e| panic!("{good}: {e:?}"));
            let acme = t.resolve("acme").unwrap();
            assert_eq!(acme.database().as_str(), good, "the database IS the environment");
            assert_eq!(acme.environment(), Some(good));
            assert_eq!(acme.namespace().as_str(), "acme", "the namespace IS the tenant");
        }
        for bad in ["prod-2", "PRODUCTION", "", "staging"] {
            let msg = err(ns_cfg("namespace-per-tenant-env", None, Some(bad), &["acme"]));
            assert!(msg.contains("not a deployment environment"), "{bad}: {msg}");
        }
    }

    /// **Every setting a topology does not use is a contradiction.** Each of
    /// these is a config an operator could write while believing something
    /// untrue about their own layout; ignoring the setting would leave them
    /// believing it.
    #[test]
    fn a_setting_a_topology_does_not_use_refuses() {
        fn ns_supplied(s: &str) -> TenancyConfig<'_> {
            TenancyConfig {
                strategy: s,
                namespace: Some("frust"),
                database: Some("app"),
                environment: None,
                tenants: &["a"],
            }
        }

        // the namespace topologies do not take a namespace: it IS the tenant
        for s in ["namespace-per-tenant", "namespace-per-tenant-env"] {
            assert!(err(ns_supplied(s)).contains("the namespace IS the tenant"), "{s}");
        }
        // namespace-per-tenant-env does not take a database: it IS the env
        assert!(err(ns_cfg("namespace-per-tenant-env", Some("app"), Some("sandbox"), &["a"]))
            .contains("the database IS the environment"));
        // the database-scoped topologies have no environment axis. `single`
        // is given its required database so the environment is the ONLY thing
        // wrong — otherwise the first rejection fires and the test would pass
        // while proving something else.
        assert!(err(ns_cfg("single", Some("shared"), Some("sandbox"), &["a"]))
            .contains("no environment axis"));
        assert!(err(ns_cfg("database-per-tenant", None, Some("sandbox"), &["a"]))
            .contains("no environment axis"));
        // ...and each namespace topology still needs the one setting it does use
        assert!(err(ns_cfg("namespace-per-tenant", None, None, &["a"])).contains("no database named"));
        assert!(err(ns_cfg("namespace-per-tenant-env", None, None, &["a"]))
            .contains("does not know whether it is production"));
    }

    /// Namespace topologies separate tenants by NAMESPACE while reusing one
    /// database name — the case that would fool any check comparing database
    /// names alone.
    #[test]
    fn namespace_topologies_separate_by_namespace_not_by_database_name() {
        let t = build(ns_cfg("namespace-per-tenant", Some("app"), None, &["acme", "globex"]))
            .unwrap();
        let a = t.resolve("acme").unwrap();
        let b = t.resolve("globex").unwrap();
        assert_eq!(a.database(), b.database(), "both are called `app`...");
        assert_ne!(a.namespace(), b.namespace(), "...in different namespaces");
        assert!(t.strategy().backup_plan(&a).is_tenant_isolated());
    }

    /// The probe that shaped Chunk C, held as an assertion: no topology may
    /// ask for an access placement SurrealDB 3.2.0 cannot host.
    #[test]
    fn no_topology_asks_for_an_unsupported_access_placement() {
        let all = [
            build(cfg("database-per-tenant", None, &["a"])).unwrap(),
            build(cfg("single", Some("shared"), &["a"])).unwrap(),
            build(ns_cfg("namespace-per-tenant", Some("app"), None, &["a"])).unwrap(),
            build(ns_cfg("namespace-per-tenant-env", None, Some("test"), &["a"])).unwrap(),
        ];
        for t in &all {
            assert_eq!(
                t.strategy().access_placement(),
                AccessPlacement::Database,
                "{}: RECORD access is database-scoped on SurrealDB 3.2.0 (probed: 400 at \
                 namespace level), and the meta boot refuses anything else",
                t.strategy().name()
            );
        }
    }

    /// **Boot-time rejection of incomplete or contradictory settings.**
    ///
    /// Each case is a configuration an operator could plausibly write while
    /// believing something untrue about their own isolation. Refusing is the
    /// only answer that does not confirm the belief.
    #[test]
    fn an_incomplete_or_contradictory_config_refuses_to_boot() {
        // incomplete: a shared-database topology with no database named
        assert!(err(cfg("single", None, &["a"])).contains("incomplete"));

        // contradictory: a shared database named under a topology that gives
        // every tenant its own
        assert!(err(cfg("database-per-tenant", Some("shared"), &["a"]))
            .contains("contradicts itself"));

        // incomplete: a topology with nothing to serve
        assert!(err(cfg("database-per-tenant", None, &[])).contains("no tenants configured"));
        assert!(err(cfg("single", Some("shared"), &[])).contains("no tenants configured"));

        // a duplicate is a typo — de-duplicating it silently would hide it
        assert!(err(cfg("database-per-tenant", None, &["a", "a"])).contains("listed twice"));
    }

    /// **The abstraction is genuine, not a rename.** The two strategies
    /// disagree about the one thing a tenancy topology is *for*.
    #[test]
    fn the_two_strategies_place_tenants_differently() {
        let shared = build(cfg("single", Some("everyone"), &["acme", "globex"])).unwrap();
        let a = shared.resolve("acme").unwrap();
        let b = shared.resolve("globex").unwrap();
        assert_eq!(a.database(), b.database(), "single must put both tenants in ONE database");
        assert_eq!(a.database().as_str(), "everyone");
        // ...and the tenant identities stay distinct even though the
        // placement does not — a label is not a boundary
        assert_ne!(a.tenant_id(), b.tenant_id());

        let split = tenancy(&["acme", "globex"]).unwrap();
        let a = split.resolve("acme").unwrap();
        let b = split.resolve("globex").unwrap();
        assert_ne!(a.database(), b.database(), "database-per-tenant must separate them");
        assert_eq!(a.database().as_str(), "acme");
    }

    /// The operational consequence of that difference, which is the reason
    /// the difference matters: WO-027 measured restore-one = restore-all
    /// under one shared database, and WO-039 measured restore-one under
    /// database-per-tenant. The strategies must say so.
    #[test]
    fn only_database_per_tenant_promises_a_tenant_isolated_restore() {
        let shared = build(cfg("single", Some("everyone"), &["acme"])).unwrap();
        let acme = shared.resolve("acme").unwrap();
        assert!(
            !shared.strategy().backup_plan(&acme).is_tenant_isolated(),
            "single claimed a tenant-isolated restore; its database holds every tenant"
        );

        let split = tenancy(&["acme"]).unwrap();
        let acme = split.resolve("acme").unwrap();
        assert!(split.strategy().backup_plan(&acme).is_tenant_isolated());
    }

    /// Invariant 3, at its narrowest point: a name that is not a plain
    /// identifier never becomes a database. This is the check that stands
    /// between a request field and a `USE DB` clause.
    #[test]
    fn a_name_that_is_not_an_identifier_cannot_become_a_target() {
        for bad in ["", "has space", "semi;colon", "../etc", "9leading", "a'b", &"x".repeat(65)] {
            assert!(tenancy(&[bad]).is_err(), "accepted {bad:?} as a tenant");
        }
    }

    #[test]
    fn only_a_registered_slug_resolves() {
        let t = tenancy(&["acme"]).unwrap();
        let acme = t.resolve("acme").unwrap();
        assert_eq!(acme.tenant_id().as_str(), "acme");
        assert_eq!(acme.namespace().as_str(), "frust");
        assert_eq!(acme.database().as_str(), "acme");
        assert!(acme.environment().is_none());

        assert!(t.resolve("globex").is_err(), "an unregistered slug resolved");
        // one indistinguishable refusal — resolution is not a tenant oracle
        assert_eq!(
            format!("{:?}", t.resolve("globex").unwrap_err()),
            format!("{:?}", t.resolve("not-even-a-valid-name").unwrap_err())
        );
    }

    #[test]
    fn sole_refuses_when_the_process_serves_more_than_one() {
        assert!(tenancy(&["a"]).unwrap().sole().is_ok());
        assert!(tenancy(&["a", "b"]).unwrap().sole().is_err());
    }

    /// The operational half of the seam. WO-039 proved per-database export
    /// gives restore-one; this is that fact readable off the strategy so
    /// Chunk D's ops path does not have to assume a shape.
    #[test]
    fn database_per_tenant_plans_a_tenant_isolated_restore() {
        let t = tenancy(&["acme"]).unwrap();
        let acme = t.resolve("acme").unwrap();
        let s = t.strategy();

        assert_eq!(s.name(), "database-per-tenant");
        assert_eq!(s.access_placement(), AccessPlacement::Database);

        let backup = s.backup_plan(&acme);
        assert!(backup.is_tenant_isolated(), "restore-one must not be restore-all");
        assert_eq!(backup.database.as_ref().unwrap().as_str(), "acme");

        assert_eq!(
            s.provisioning_plan(&acme).steps,
            vec![
                ProvisioningStep::CreateDatabase {
                    namespace: acme.namespace().clone(),
                    database: acme.database().clone(),
                },
                ProvisioningStep::DefineAccess { placement: AccessPlacement::Database },
            ]
        );
    }
}
