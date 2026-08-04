//! Boot discipline evidence. Each test gets its own database so they
//! parallelize safely.

use frust_kernel::boot::{boot, BootError, BootOptions, NoUserSync};
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::single_tenant;
use frust_kernel::meta::{META_SCHEMA_VERSION, META_TABLE};

fn fresh_db(name: &str) -> Db {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};"))
        .expect("provision test database");
    db
}

fn opts(holder: &str, accept: bool) -> BootOptions {
    BootOptions { holder: holder.to_string(), accept_meta_migrations: accept }
}

/// Fresh DB: first boot applies meta v1 without ack; second boot is a no-op.
#[test]
fn fresh_boot_applies_then_noops() {
    let db = fresh_db("boot_fresh");
    let r1 = boot(&db, &opts("n1", false), &NoUserSync).unwrap();
    assert!(r1.applied_meta);
    assert_eq!(r1.meta_version, META_SCHEMA_VERSION);

    let r2 = boot(&db, &opts("n1", false), &NoUserSync).unwrap();
    assert!(!r2.applied_meta, "second boot must be a no-op");
}

/// Exit criterion 4a: a DB whose meta-schema is newer than the binary
/// refuses boot with the named error — and the lock is released so a fixed
/// binary can boot afterwards.
#[test]
fn newer_db_meta_refuses_boot_named() {
    let db = fresh_db("boot_newer");
    boot(&db, &opts("n1", false), &NoUserSync).unwrap();
    db.sql_root(&format!(
        "UPSERT {META_TABLE}:schema SET version = {};",
        META_SCHEMA_VERSION + 100
    ))
    .unwrap();

    let err = boot(&db, &opts("n1", false), &NoUserSync).unwrap_err();
    assert!(
        matches!(err, BootError::MetaNewerThanBinary { db_version, binary_version }
            if db_version == META_SCHEMA_VERSION + 100 && binary_version == META_SCHEMA_VERSION)
    );
    assert!(err.to_string().starts_with("E_META_NEWER_THAN_BINARY"), "named error: {err}");

    // lock must have been released by the refusing boot
    db.sql_root(&format!("UPSERT {META_TABLE}:schema SET version = {META_SCHEMA_VERSION};")).unwrap();
    boot(&db, &opts("n2", false), &NoUserSync).expect("lock released after refusal");
}

/// Exit criterion 4b: the --accept-meta-migrations two-step. An existing DB
/// with an older meta version refuses without the flag (named error), then
/// applies with it.
#[test]
fn accept_meta_migrations_two_step() {
    let db = fresh_db("boot_twostep");
    boot(&db, &opts("n1", false), &NoUserSync).unwrap();
    // simulate a database created by an older binary
    db.sql_root(&format!("UPSERT {META_TABLE}:schema SET version = 0;")).unwrap();

    // step 1: refused, named
    let err = boot(&db, &opts("n1", false), &NoUserSync).unwrap_err();
    assert!(matches!(err, BootError::MetaMigrationPending { db_version: 0, .. }));
    assert!(err.to_string().starts_with("E_META_MIGRATION_PENDING"), "named error: {err}");

    // step 2: acknowledged, applied
    let r = boot(&db, &opts("n1", true), &NoUserSync).unwrap();
    assert!(r.applied_meta);
    assert_eq!(r.meta_version, META_SCHEMA_VERSION);
}

/// Exit criterion 4c (the miserable-to-retrofit one, built alongside):
/// two nodes booting a fresh database concurrently cannot double-apply the
/// meta sync. Evidence: both boots succeed, exactly ONE apply-log row.
#[test]
fn racing_nodes_single_apply() {
    let db_name = "boot_race";
    {
        fresh_db(db_name); // provision once
    }
    let handles: Vec<_> = (0..2)
        .map(|i| {
            let name = db_name.to_string();
            std::thread::spawn(move || {
                let cfg = single_tenant(&name).expect("tenancy");
                let db = scoped_db(&cfg);
                boot(&db, &opts(&format!("racer-{i}"), false), &NoUserSync)
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    // both nodes must come up: one applies, the loser waits on the lock,
    // re-reads the now-current version, and no-ops
    let mut applied = 0;
    for r in &results {
        let report = r.as_ref().expect("both racing boots succeed");
        if report.applied_meta {
            applied += 1;
        }
    }
    assert_eq!(applied, 1, "exactly one node applies");

    // and the database agrees: exactly one apply-log row
    let db = scoped_db(&single_tenant(&db_name.to_string()).expect("tenancy"));
    let logs = db
        .sql_root(&format!(
            "SELECT count() FROM {META_TABLE} WHERE kind = 'meta_apply_log' GROUP ALL;"
        ))
        .unwrap();
    let count = logs
        .as_array()
        .and_then(|a| a.first())
        .and_then(|r| r.get("count"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    assert_eq!(count, 1, "exactly one meta apply-log row across racing nodes");
}
