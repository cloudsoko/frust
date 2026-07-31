//! WO-005 module 5: the worker loop. Exit criteria 5, 6, 7 EXECUTED.
//! Requires surreal.exe on :8899 + hook artifacts (via WasmHooks).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller};
use frust_kernel::contract::Value;
use frust_kernel::db::scoped_db;
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::worker::{AuthorityResolver, JobOutcome, Worker};

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

/// Provision a fresh db with the queue table + a doctype for job effects.
fn setup(name: &str) -> ResolvedTenant {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    // queue table with a short changefeed for the retention test
    db.sql_root("DEFINE TABLE job SCHEMALESS CHANGEFEED 1h;").unwrap();
    // record auth so job effects run under a real (re-derived) principal,
    // exactly as production would — not root
    db.sql_root(
        "DEFINE TABLE app_user SCHEMAFULL PERMISSIONS NONE; \
         DEFINE FIELD name ON app_user TYPE string; \
         DEFINE FIELD role ON app_user TYPE string; \
         DEFINE FIELD pass ON app_user TYPE string; \
         DEFINE ACCESS account ON DATABASE TYPE RECORD \
           SIGNIN (SELECT * FROM app_user WHERE name = $name AND crypto::argon2::compare(pass, $pass)) \
           DURATION FOR TOKEN 1h, FOR SESSION 12h;",
    )
    .unwrap();
    db.sql_root("CREATE app_user:worker SET name = 'worker', role = 'manager', pass = crypto::argon2::generate('pw');").unwrap();
    // a doctype for create_doc job effects, permissive to any record user.
    // SCHEMALESS so hook-added fields (total, etc.) are accepted freely.
    db.sql_root(
        "DEFINE TABLE thing SCHEMALESS PERMISSIONS FULL; \
         DEFINE FIELD owner ON thing TYPE option<record<app_user>> DEFAULT $auth.id;",
    )
    .unwrap();
    // metadata the broker needs to resolve the doctype
    db.sql_root("DEFINE TABLE doctype SCHEMALESS; CREATE doctype CONTENT { name: 'thing', label: 'Thing', fields: [{ fieldname: 'title', fieldtype: 'Data' }] };").unwrap();
    cfg
}

fn broker(cfg: &ResolvedTenant) -> Broker {
    Broker::new(scoped_db(&cfg), Box::new(WasmHooks::load(artifacts()).unwrap()))
}

/// Resolver that maps any requested_by to a root-ish caller, EXCEPT a
/// revoked set that returns None (authority revoked -> non-retryable deny).
struct TestResolver {
    revoked: Vec<String>,
}
impl AuthorityResolver for TestResolver {
    fn caller_for(&self, requested_by: &str) -> Option<Caller> {
        if self.revoked.iter().any(|r| r == requested_by) {
            return None;
        }
        // re-derive a live record-user session (the seeded 'worker')
        Some(Caller { user: "worker".into(), pass: "pw".into(), role: "manager".into() })
    }
}

/// EXIT CRITERION 6: contested atomic claim under burst — N workers race for
/// the same jobs, EXACTLY ONE winner per job, measured. Plus rider (2): the
/// conflict-retry counter reports attempts-per-claim under contention.
#[test]
fn criterion6_exactly_one_winner_per_job_under_burst() {
    let cfg = setup("wq_claim");
    let enq = scoped_db(&cfg);
    // enqueue 200 jobs directly (no effect needed for the claim race)
    let mut ids = Vec::new();
    for i in 0..200 {
        let out = enq
            .sql_root(&format!(
                "CREATE job SET kind = 'noop', payload = {{}}, status = 'queued', \
                 requested_by = 'u', enqueued_at = time::now(), n = {i};"
            ))
            .unwrap();
        ids.push(out.as_array().unwrap()[0]["id"].as_str().unwrap().to_string());
    }

    const WORKERS: usize = 6;
    let claims: Arc<Vec<AtomicU64>> = Arc::new((0..200).map(|_| AtomicU64::new(0)).collect());
    let ids = Arc::new(ids);
    let cfg = Arc::new(cfg);

    let handles: Vec<_> = (0..WORKERS)
        .map(|w| {
            let claims = Arc::clone(&claims);
            let ids = Arc::clone(&ids);
            let cfg = Arc::clone(&cfg);
            std::thread::spawn(move || {
                let db = scoped_db(&(*cfg).clone());
                let wk = Worker::claim_only(&db, format!("w{w}"));
                // each worker attempts EVERY job id (maximal contention)
                for (idx, id) in ids.iter().enumerate() {
                    if wk.try_claim(id).unwrap().is_some() {
                        claims[idx].fetch_add(1, Ordering::SeqCst);
                    }
                }
                db.conflict_retries()
            })
        })
        .collect();
    let total_retries: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();

    let winners: Vec<u64> = claims.iter().map(|c| c.load(Ordering::SeqCst)).collect();
    let claimed = winners.iter().filter(|&&c| c == 1).count();
    let double = winners.iter().filter(|&&c| c > 1).count();
    println!(
        "criterion 6: {WORKERS} workers x 200 jobs | exactly-once={claimed} double-claimed={double} \
         conflict-retries={total_retries} (attempts-per-claim ~ {:.2})",
        1.0 + total_retries as f64 / 200.0
    );
    assert_eq!(double, 0, "NO job may be claimed twice — the serialization point holds");
    assert_eq!(claimed, 200, "every job claimed exactly once");
}

/// EXIT CRITERION 7: retention cold-start. A worker that comes up with an
/// empty cursor recovers pending work by rescanning status='queued' —
/// jobs are records, recovery is a query (ADR-009 ruling #2). Missing
/// nothing is proven by draining to empty.
#[test]
fn criterion7_coldstart_rescan_drains_queue() {
    let cfg = setup("wq_rescan");
    let b = broker(&cfg);
    let db = scoped_db(&cfg);
    // enqueue 10 create_doc jobs
    let caller = Caller { user: "root".into(), pass: "root".into(), role: "manager".into() };
    for i in 0..10 {
        b.enqueue(
            &caller,
            "create_doc",
            &[
                ("doctype".into(), Value::Text("thing".into())),
                (
                    "doc".into(),
                    Value::Object(vec![("title".into(), Value::Text(format!("job {i}")))]),
                ),
            ],
        )
        .unwrap();
    }
    // a FRESH worker (no cursor, no LIVE history) drains purely by rescan
    let wk = Worker::new(&db, &b, "cold-worker");
    let resolver = TestResolver { revoked: vec![] };
    let mut ran = 0;
    while let Some(job) = wk.claim_next().unwrap() {
        assert_eq!(wk.run(&job, &resolver), JobOutcome::Done);
        ran += 1;
    }
    assert_eq!(ran, 10, "cold-start rescan recovered every queued job");
    // queue drained: nothing left queued, and the effects landed
    let left = db.sql_root("SELECT VALUE id FROM job WHERE status = 'queued';").unwrap();
    assert_eq!(left.as_array().map(std::vec::Vec::len).unwrap_or(0), 0, "queue empty");
    let things = db.sql_root("SELECT count() FROM thing GROUP ALL;").unwrap();
    assert_eq!(things.as_array().unwrap()[0]["count"].as_u64(), Some(10), "all effects applied");
}

/// EXIT CRITERION 5 (hardest clause): identity captured, authority re-derived
/// — deny-after-revocation is a typed NON-RETRYABLE failure (ADR-006 edge 4).
#[test]
fn criterion5_revoked_authority_is_nonretryable_deny() {
    let cfg = setup("wq_revoke");
    let b = broker(&cfg);
    let db = scoped_db(&cfg);
    let caller = Caller { user: "ghost".into(), pass: "x".into(), role: "clerk".into() };
    b.enqueue(
        &caller,
        "create_doc",
        &[
            ("doctype".into(), Value::Text("thing".into())),
            ("doc".into(), Value::Object(vec![("title".into(), Value::Text("revoked".into()))])),
        ],
    )
    .unwrap();

    let wk = Worker::new(&db, &b, "w");
    // 'ghost' has been revoked between enqueue and run
    let resolver = TestResolver { revoked: vec!["ghost".into()] };
    let job = wk.claim_next().unwrap().expect("a job to run");
    let outcome = wk.run(&job, &resolver);

    assert!(matches!(outcome, JobOutcome::Denied(_)), "revoked authority -> denied, got {outcome:?}");
    // and the job is terminal 'denied', NOT requeued (non-retryable)
    let status = db
        .sql_root("SELECT VALUE status FROM job LIMIT 1;")
        .unwrap()
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    assert_eq!(status, "denied", "denied job is terminal, never requeued");
}

/// A job handler writing through the broker fires hooks end-to-end (the
/// re-entrant-write carry-forward): the large-draft job comes out
/// 'Needs Approval', proving hooks ran inside the job effect.
#[test]
fn job_effect_fires_hooks() {
    let cfg = setup("wq_hooks");
    let b = broker(&cfg);
    let db = scoped_db(&cfg);
    let caller = Caller { user: "root".into(), pass: "root".into(), role: "manager".into() };
    b.enqueue(
        &caller,
        "create_doc",
        &[
            ("doctype".into(), Value::Text("thing".into())),
            (
                "doc".into(),
                Value::Object(vec![
                    ("title".into(), Value::Text("big".into())),
                    ("status".into(), Value::Text("Draft".into())),
                    ("total".into(), Value::Float(20000.0)),
                ]),
            ),
        ],
    )
    .unwrap();
    let wk = Worker::new(&db, &b, "w");
    let job = wk.claim_next().unwrap().unwrap();
    assert_eq!(wk.run(&job, &TestResolver { revoked: vec![] }), JobOutcome::Done);
    // the plugin hook flagged the large draft inside the job's write
    let status = db.sql_root("SELECT VALUE status FROM thing LIMIT 1;").unwrap();
    assert_eq!(status.as_array().unwrap()[0].as_str(), Some("Needs Approval"), "hooks fired inside the job effect");
}
