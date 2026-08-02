//! PM note 2 on module 2: the conflict-detection canary. `is_conflict`
//! substring-matches the pinned SurrealDB version's wording; if an upgrade
//! rewords it, retry silently stops and every legitimate conflict becomes an
//! immediate hard failure. This test provokes REAL conflicts and asserts
//! both halves: writes all succeed (retry worked) and retries were observed
//! (detection fired). A SurrealDB bump that changes the wording fails here,
//! loudly, in CI.

use std::sync::Arc;

use frust_kernel::db::scoped_db;
use frust_kernel::tenancy::single_tenant;

#[test]
fn conflict_detection_canary() {
    let name = "conflict_canary";
    let provision = scoped_db(&single_tenant(&name.to_string()).expect("tenancy"));
    provision
        .sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};"))
        .unwrap();
    provision.sql_root("DEFINE TABLE canary SCHEMALESS; UPSERT canary:one SET n = 0;").unwrap();

    // one shared Db so the retry counter aggregates across threads
    let db = Arc::new(scoped_db(&single_tenant(&name.to_string()).expect("tenancy")));

    const THREADS: usize = 4;
    const OPS: usize = 40;
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let db = Arc::clone(&db);
            std::thread::spawn(move || {
                for _ in 0..OPS {
                    // Use the production SurrealKV engine's atomic numeric
                    // update inside an explicit transaction. The in-memory
                    // engine has acknowledged lost increments under this exact
                    // workload, which is why the hermetic lane pins SurrealKV.
                    // The row already exists, so UPDATE keeps this a conflict
                    // probe rather than exercising UPSERT semantics.
                    db.sql_root(
                        "BEGIN TRANSACTION; \
                         UPDATE canary:one SET n += 1; \
                         COMMIT TRANSACTION;",
                    )
                    .expect("write must succeed via retry");
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    let n = db
        .sql_root("SELECT VALUE n FROM ONLY canary:one;")
        .unwrap()
        .as_i64()
        .unwrap_or(-1);
    let retries = db.conflict_retries();
    println!("canary: final n = {n} (expected {}), conflict retries observed = {retries}", THREADS * OPS);

    assert_eq!(n as usize, THREADS * OPS, "every increment must land exactly once");
    assert!(
        retries > 0,
        "no conflicts detected under {THREADS}x{OPS} contention — either SurrealDB \
         changed its conflict wording (update Db::is_conflict) or the workload no \
         longer contends; both need a human"
    );
}
