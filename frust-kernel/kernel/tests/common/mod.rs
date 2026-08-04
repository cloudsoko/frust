//! Shared self-seeding fixture.
//!
//! `permission_proof` and `rest_surface` were the only two binaries that
//! depended on a hand-seeded ambient `skeleton` database. That dependency was
//! a landmine — repeatedly tripped by store rebuilds, decimal migrations, and
//! perf-store juggling across separate sessions.
//!
//! This builds the **exact** base dataset (the real pinned rows, not a
//! reconstruction) into a dedicated database per caller. No test touches
//! ambient dev state again.

use std::sync::Arc;

use frust_kernel::broker::Broker;
use frust_kernel::db::scoped_db;
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{access_ddl, identity_ddl};
use frust_kernel::sync::MetadataSync;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

/// A broker over a freshly-seeded `purchase_order` fixture in database `name`.
///
/// The dataset: clerk1/clerk2/manager, and six `purchase_order` rows split
/// three/three between the clerks — the canonical fixture the permission
/// proofs assert against (partition counts, `title INSIDE [...]`, the
/// `total >= 100 AND status != 'Paid'` filter, sort order).
pub fn seeded_broker(prefix: &str) -> (Arc<Broker>, ResolvedTenant) {
    // A unique database per call — `rest_surface`'s tests run in PARALLEL, so
    // a shared name would have them REMOVE/DEFINE the same database
    // concurrently and race. The counter makes every fixture independent.
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let name = format!("{prefix}_{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed));
    let cfg = single_tenant(&name.clone()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&format!(
        "{}; {}; DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE; \
         CREATE app_user:clerk1 SET name = 'clerk1', role = 'clerk', pass = crypto::argon2::generate('pw-clerk1'); \
         CREATE app_user:clerk2 SET name = 'clerk2', role = 'clerk', pass = crypto::argon2::generate('pw-clerk2'); \
         CREATE app_user:manager SET name = 'manager', role = 'manager', pass = crypto::argon2::generate('pw-manager'); \
         CREATE doctype CONTENT {{ name: 'purchase_order', label: 'purchase order', fields: [ \
            {{ fieldname: 'title', fieldtype: 'Data', required: true }}, \
            {{ fieldname: 'total', fieldtype: 'Currency', required: true }}, \
            {{ fieldname: 'notes', fieldtype: 'Text' }} ] }};",
        identity_ddl(),
        access_ddl(),
    ))
    .unwrap();
    // build the table (permissions, owner field, changefeed) from the metadata
    MetadataSync { base: cfg.clone() }.sync(&db).expect("sync purchase_order");

    // the six canonical rows, owners set explicitly (root write). Decimals as
    // `dec` literals so `total` lands as decimal, matching production writes.
    db.sql_root(
        "CREATE purchase_order SET title = 'Alpha order', owner = app_user:clerk1, status = 'Draft', total = 230.00dec; \
         CREATE purchase_order SET title = 'Beta order',  owner = app_user:clerk2, status = 'Paid',  total = 345.00dec; \
         CREATE purchase_order SET title = 'Big draft',   owner = app_user:clerk1, status = 'Needs Approval', total = 28750.00dec; \
         CREATE purchase_order SET title = 'Final check', owner = app_user:clerk2, status = 'Draft', total = 575.00dec; \
         CREATE purchase_order SET title = 'Warm check',  owner = app_user:clerk2, status = 'Draft', total = 690.00dec; \
         CREATE purchase_order SET title = 'authprobe',   owner = app_user:clerk1, status = 'Draft', total = 1.00dec;",
    )
    .expect("seed purchase_order rows");

    let broker = Arc::new(Broker::new(scoped_db(&cfg), Box::new(WasmHooks::load(artifacts()).unwrap())));
    (broker, cfg)
}
