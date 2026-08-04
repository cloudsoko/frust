//! The EVENT-bypass canary, pinned.
//!
//! v3.2.0 behavior (probed, now load-bearing): writes made inside a
//! DEFINE EVENT body BYPASS table permissions, while the same session's
//! direct writes are permission-filtered. Tier-1 counters depend on
//! exactly this — write-closed rollup tables that the submit transaction can
//! still maintain. If a SurrealDB version bump changes it, THIS test fails
//! CI loudly (same posture as the conflict-string canary); re-evaluate
//! Tier-1's write path before trusting any rollup.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use frust_kernel::db::scoped_db;
use frust_kernel::tenancy::single_tenant;
use frust_kernel::meta::{access_ddl, identity_ddl};

#[test]
fn event_writes_bypass_permissions_direct_writes_do_not() {
    let cfg = single_tenant(&"evt_canary".to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns("REMOVE DATABASE IF EXISTS evt_canary; DEFINE DATABASE evt_canary;").unwrap();
    db.sql_root(&format!(
        "{}; {}; \
         CREATE app_user:u SET name = 'u', role = 'user', pass = crypto::argon2::generate('pw-u'); \
         DEFINE TABLE src SCHEMALESS PERMISSIONS FOR select, create WHERE $auth.id != NONE; \
         DEFINE TABLE locked SCHEMALESS PERMISSIONS FOR select WHERE $auth.role = 'manager' FOR create, update, delete NONE; \
         DEFINE EVENT bump ON TABLE src WHEN $event = 'CREATE' THEN {{ \
           UPSERT locked:tally SET n = (n ?? 0) + 1; \
         }};",
        identity_ddl(),
        access_ddl(),
    ))
    .unwrap();

    // the record user's CREATE fires the EVENT; the EVENT writes the
    // write-closed table
    db.sql_as("u", "pw-u", "CREATE src SET v = 1;").unwrap();
    let tally = db.sql_root("SELECT n FROM locked:tally;").unwrap();
    let n = tally.as_array().and_then(|a| a.first()).and_then(|r| r["n"].as_i64());
    assert_eq!(
        n,
        Some(1),
        "PINNED v3.2.0 BEHAVIOR CHANGED: EVENT-body writes no longer bypass table \
         permissions — Tier-1 write-closed rollups depend on this; do not \
         bump the SurrealDB pin without re-ruling Tier-1's write path"
    );

    // the SAME session writing the SAME table directly is filtered to nothing
    db.sql_as("u", "pw-u", "UPSERT locked:tamper SET n = 999;").unwrap();
    let direct = db.sql_root("SELECT count() FROM locked GROUP ALL;").unwrap();
    let rows = direct.as_array().and_then(|a| a.first()).and_then(|r| r["count"].as_i64());
    assert_eq!(
        rows,
        Some(1),
        "PINNED v3.2.0 BEHAVIOR CHANGED: a record user's direct write reached a \
         write-closed table — rollup tamper-resistance is broken"
    );
}
