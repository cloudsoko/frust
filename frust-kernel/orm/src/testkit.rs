//! Live-server test harness (replaces the SDK's kv-mem `mem_db`): every call
//! provisions a fresh, uniquely-named database on the local surreal.exe, so
//! tests parallelize without sharing state.
//!
//! Requires: `FRUST_DB_ENDPOINT` pointing at SurrealDB (root/root), ns `frust`.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};

use crate::resource::{Conn, StmtResult};

const NS: &str = "frust";
const AUTH: &str = "Basic cm9vdDpyb290"; // root:root

fn endpoint() -> anyhow::Result<String> {
    std::env::var("FRUST_DB_ENDPOINT")
        .map_err(|_| anyhow::anyhow!("FRUST_DB_ENDPOINT is required for live ORM tests"))
}

// Namespace-level database DDL conflicts even when two tests create different
// database names. Keep provisioning and teardown serial within this test binary.
static DB_LIFECYCLE: Mutex<()> = Mutex::new(());

pub struct TestDb {
    pub db_name: String,
    /// Only the handle that provisioned the database removes it. `attach()`
    /// hands out extra connections to the SAME database, and those must not
    /// drop it out from under the owner when they go out of scope.
    owned: bool,
}

/// Scratch databases are removed on teardown, not merely before the next run.
///
/// Provisioning used to `REMOVE ... IF EXISTS` on the way *in*, which cleans
/// up the previous run but leaves the current one resident forever — and since
/// the name carries the PID, every invocation left a fresh set behind. They
/// accumulated across runs and measurably degraded the instance, which then
/// had to be cleaned by hand before every perf run.
///
/// Best-effort by construction: a failed cleanup must never turn a passing
/// test red, and must never panic while unwinding from one that already failed.
impl Drop for TestDb {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        let _guard = DB_LIFECYCLE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = self.post(&format!("REMOVE DATABASE IF EXISTS {};", self.db_name), false);
    }
}

impl TestDb {
    fn post(&self, query: &str, with_db: bool) -> anyhow::Result<Vec<StmtResult>> {
        let mut req = ureq::post(format!("{}/sql", endpoint()?))
            .header("Authorization", AUTH)
            .header("Accept", "application/json")
            .header("surreal-ns", NS);
        if with_db {
            req = req.header("surreal-db", &self.db_name);
        }
        let mut res = req.send(query)?;
        let text = res.body_mut().read_to_string()?;
        let parsed: serde_json::Value = serde_json::from_str(&text)?;
        Ok(parsed
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|stmt| StmtResult {
                ok: stmt.get("status").and_then(|s| s.as_str()) == Some("OK"),
                result: stmt.get("result").cloned().unwrap_or(serde_json::Value::Null),
            })
            .collect())
    }
}

impl Conn for TestDb {
    fn query(&self, sql: &str) -> anyhow::Result<Vec<StmtResult>> {
        self.post(sql, true)
    }
}

/// Fresh uniquely-named database per call — the mem_db() replacement.
pub fn mem_db() -> TestDb {
    static N: AtomicU64 = AtomicU64::new(0);
    let db_name = format!("orm_t_{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed));
    let db = TestDb { db_name: db_name.clone(), owned: true };
    let guard = DB_LIFECYCLE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let statements = db
        .post(
            &format!("REMOVE DATABASE IF EXISTS {db_name}; DEFINE DATABASE {db_name};"),
            false,
        )
        .expect("provision test database at FRUST_DB_ENDPOINT");
    let failure = statements
        .iter()
        .find(|statement| !statement.ok)
        .map(|statement| statement.result.clone());
    drop(guard);
    if let Some(failed) = failure {
        panic!("provision test database {db_name}: {failed}");
    }
    db
}

/// `true` when every statement in `sql` succeeded.
pub fn all_ok(db: &TestDb, sql: &str) -> bool {
    db.query(sql).map(|s| s.iter().all(|x| x.ok)).unwrap_or(false)
}

/// Attach to an existing test database by name (no provisioning) — lets
/// ConnFactory implementations hand out fresh connections per acquire.
pub fn attach(db_name: &str) -> TestDb {
    TestDb { db_name: db_name.to_string(), owned: false }
}
