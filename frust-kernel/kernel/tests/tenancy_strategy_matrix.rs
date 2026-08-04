//! **The tenancy implementation matrix.**
//!
//! The seam already exists; this proves it has *two real sides*. The
//! same assertions run against `single` and `database-per-tenant`, and the
//! table below is the contract each one signs:
//!
//! | strategy | two tenants share a database? | restore-one? |
//! |---|---|---|
//! | `single` | **yes** — tenancy is a label, not a boundary | no |
//! | `database-per-tenant` | no | **yes** |
//!
//! The `single` row is the one worth reading twice. It is not a weaker
//! version of the other — it is a topology that **does not isolate tenants at
//! all**, and the test says so out loud rather than skipping the assertion.
//! An operator who believes `single` separates their customers has a much
//! worse problem than one who knows it does not, so the suite states the
//! truth in both directions.
//!
//! What holds under *both*: **authorization**. Row permissions are enforced
//! by the database under the caller's own session (the one-door property), so
//! a clerk sees only their own rows no matter which topology is running. That
//! is the guarantee tenancy must never be confused with.
//!
//! Requires the dev surreal on :8899.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookDispatch};
use frust_kernel::contract::{BrokerError, ReadOpts, Value};
use frust_kernel::db::{scoped_db, ConnConfig};
use frust_kernel::meta::{access_ddl, identity_ddl};
use frust_kernel::sync::MetadataSync;
use frust_kernel::tenancy::{ResolvedTenant, Tenancy, TenancyConfig};

struct PassHooks;
impl HookDispatch for PassHooks {
    fn validate(&self, _d: &str, doc: &[(String, Value)]) -> Result<Vec<(String, Value)>, BrokerError> {
        Ok(doc.to_vec())
    }
}

/// What a strategy promises. Adding `namespace-per-tenant` later is
/// adding a row here, not writing a new test.
struct Expect {
    strategy: &'static str,
    namespace: Option<&'static str>,
    database: Option<&'static str>,
    environment: Option<&'static str>,
    tenants: [&'static str; 2],
    /// Do two distinct tenants land in the same database?
    tenants_share_a_database: bool,
    /// Do they at least land in separate NAMESPACES?
    tenants_share_a_namespace: bool,
    /// Can one tenant be restored without touching another?
    restore_is_tenant_isolated: bool,
}

const MATRIX: &[Expect] = &[
    Expect {
        strategy: "single",
        namespace: None,
        database: Some("wo040b_mx_shared"),
        environment: None,
        tenants: ["wo040b_mx_one", "wo040b_mx_two"],
        tenants_share_a_database: true,
        tenants_share_a_namespace: true,
        restore_is_tenant_isolated: false,
    },
    Expect {
        strategy: "database-per-tenant",
        namespace: None,
        database: None,
        environment: None,
        tenants: ["wo040b_mx_split_a", "wo040b_mx_split_b"],
        tenants_share_a_database: false,
        tenants_share_a_namespace: true,
        restore_is_tenant_isolated: true,
    },
    // ── the namespace topologies ──
    Expect {
        strategy: "namespace-per-tenant",
        namespace: None,
        database: Some("app"),
        environment: None,
        tenants: ["wo040d_ns_a", "wo040d_ns_b"],
        // NOTE: the database NAME is the same in both — `app` — but they are
        // different databases, because they live in different namespaces.
        // The matrix asserts placement by (namespace, database), never by the
        // database name alone, which is exactly the confusion this row exists
        // to catch.
        tenants_share_a_database: false,
        tenants_share_a_namespace: false,
        restore_is_tenant_isolated: true,
    },
    Expect {
        strategy: "namespace-per-tenant-env",
        namespace: None,
        database: None,
        environment: Some("sandbox"),
        tenants: ["wo040d_env_a", "wo040d_env_b"],
        tenants_share_a_database: false,
        tenants_share_a_namespace: false,
        restore_is_tenant_isolated: true,
    },
];

fn tenancy(e: &Expect) -> Tenancy {
    Tenancy::from_config(
        ConnConfig::default(),
        TenancyConfig {
            strategy: e.strategy,
            namespace: e.namespace,
            database: e.database,
            environment: e.environment,
            tenants: &e.tenants,
        },
    )
    .unwrap_or_else(|err| panic!("{}: {err:?}", e.strategy))
}

/// Where a target actually lives. **Placement is the (namespace, database)
/// pair, never the database name alone** — under `namespace-per-tenant` every
/// tenant's database is called `app`, and comparing names would call two
/// perfectly separated tenants "the same database".
fn placement(t: &ResolvedTenant) -> (String, String) {
    (t.namespace().as_str().to_string(), t.database().as_str().to_string())
}

/// Build the fixture in whatever namespace+database `target` resolves to.
/// Rows are titled with the **tenant id**, so an assertion about provenance is
/// an assertion about whose data came back — which is the only thing that
/// stays meaningful across topologies that reuse database names.
fn seed(target: &ResolvedTenant) {
    // the strategy's own provisioning plan builds the namespace and database
    frust_kernel::provision::provision(target).expect("provision");
    let db = scoped_db(target);
    let name = target.tenant_id().as_str();
    let dbname = target.database().as_str();
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {dbname}; DEFINE DATABASE {dbname};"))
        .expect("reset database");
    db.sql_root(&format!(
        "{}; {}; DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE; \
         CREATE app_user:c1 SET name = 'c1', role = 'clerk', pass = crypto::argon2::generate('pw-c1'); \
         CREATE app_user:c2 SET name = 'c2', role = 'clerk', pass = crypto::argon2::generate('pw-c2'); \
         CREATE doctype CONTENT {{ name: 'note', label: 'Note', fields: [ \
            {{ fieldname: 'title', fieldtype: 'Data' }} ] }};",
        identity_ddl(),
        access_ddl(),
    ))
    .expect("seed metadata");
    MetadataSync { base: target.clone() }.sync(&db).expect("sync note");
    db.sql_root(&format!(
        "CREATE note SET title = '{name}-c1', owner = app_user:c1, status = 'Draft'; \
         CREATE note SET title = '{name}-c2', owner = app_user:c2, status = 'Draft';"
    ))
    .expect("seed rows");
}

fn titles(broker: &Broker, user: &str) -> Vec<String> {
    let caller =
        Caller { user: user.into(), pass: format!("pw-{user}"), role: "clerk".into() };
    let mut out: Vec<String> = broker
        .db_read(&caller, "note", None, &[], &ReadOpts::default())
        .expect("read")
        .iter()
        .map(|r| r["title"].as_str().unwrap_or("<missing>").to_string())
        .collect();
    out.sort();
    out
}

#[test]
fn every_strategy_keeps_its_promise_and_all_of_them_keep_authorization() {
    // ONE shared engine across every strategy in the matrix — the same
    // criterion-2 `Arc` the isolation test runs on.
    let hooks: Arc<dyn HookDispatch> = Arc::from(Box::new(PassHooks) as Box<dyn HookDispatch>);

    for e in MATRIX {
        let t = tenancy(e);
        let one = t.resolve(e.tenants[0]).expect("resolve tenant 0");
        let two = t.resolve(e.tenants[1]).expect("resolve tenant 1");

        // ── the placement promise, by (namespace, database) ──
        assert_eq!(
            placement(&one) == placement(&two),
            e.tenants_share_a_database,
            "{}: two tenants sharing a database should be {}, got {:?} vs {:?}",
            e.strategy,
            e.tenants_share_a_database,
            placement(&one),
            placement(&two)
        );
        assert_eq!(
            one.namespace() == two.namespace(),
            e.tenants_share_a_namespace,
            "{}: namespace sharing mis-stated ({} vs {})",
            e.strategy,
            one.namespace(),
            two.namespace()
        );
        // the environment axis is present exactly when the topology has one
        assert_eq!(
            one.environment(),
            e.environment,
            "{}: environment mis-reported",
            e.strategy
        );
        // tenant identity stays distinct even where placement does not —
        // a label is not a boundary, and it is not erased by sharing one
        assert_ne!(one.tenant_id(), two.tenant_id(), "{}: tenant ids collapsed", e.strategy);

        // ── the operational promise ──
        assert_eq!(
            t.strategy().backup_plan(&one).is_tenant_isolated(),
            e.restore_is_tenant_isolated,
            "{}: restore isolation mis-stated — this is what an operator plans DR against",
            e.strategy
        );

        // ── seed whatever databases this topology actually uses ──
        seed(&one);
        if !e.tenants_share_a_database {
            seed(&two);
        }

        let b_one = Broker::with_shared_hooks(scoped_db(&one), Arc::clone(&hooks));
        let b_two = Broker::with_shared_hooks(scoped_db(&two), Arc::clone(&hooks));

        // ── AUTHORIZATION HOLDS UNDER BOTH ──
        // The one-door property is topology-independent: the DB filters rows
        // under the caller's own session, so a clerk sees only their own.
        let c1_via_one = titles(&b_one, "c1");
        assert_eq!(
            c1_via_one,
            vec![format!("{}-c1", one.tenant_id())],
            "{}: clerk c1 saw rows that are not c1's — the permission half broke",
            e.strategy
        );
        assert!(
            !c1_via_one.iter().any(|x| x.ends_with("-c2")),
            "{}: c2's row leaked to c1: {c1_via_one:?}",
            e.strategy
        );

        // ── DATA MATCHES THE PLACEMENT PROMISE ──
        let c1_via_two = titles(&b_two, "c1");
        if e.tenants_share_a_database {
            assert_eq!(
                c1_via_one, c1_via_two,
                "{}: both tenants resolve to one database, so both MUST return the same rows — \
                 this strategy does not isolate tenants and the test refuses to imply it does",
                e.strategy
            );
        } else {
            assert_ne!(
                c1_via_one, c1_via_two,
                "{}: separate databases returned identical rows — routing is not separating them",
                e.strategy
            );
            for title in &c1_via_two {
                assert!(
                    title.starts_with(two.tenant_id().as_str()),
                    "{}: tenant two received a row from elsewhere: {title}",
                    e.strategy
                );
            }
        }

        println!(
            "PASS {:<26} ns_shared={} db_shared={} restore_isolated={} one={:?} two={:?}",
            e.strategy, e.tenants_share_a_namespace, e.tenants_share_a_database, e.restore_is_tenant_isolated, placement(&one), placement(&two)
        );
    }

    // the matrix must cover every strategy this binary will build
    assert_eq!(MATRIX.len(), 4, "a strategy was added without a matrix row");
}
