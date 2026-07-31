//! WO-005 first report: the broker serving db-read with the permission
//! compiler producing the role-split row proof through the kernel path â€”
//! byte-compared against direct DB sessions, proving the kernel adds
//! exactly the field envelope and nothing else.
//!
//! Requires the WO-002 environment: surreal.exe on :8899 with ns frust /
//! db skeleton (app_user roles + purchase_order data), and â€” for the
//! acceptance-flow leg â€” the composition hook-runner on :8787.

use frust_kernel::broker::{Broker, Caller};
use frust_kernel::contract::*;
use frust_kernel::db::Db;

/// These tests share one live dataset; serialize them.
fn lock() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    GUARD
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

mod common;

/// WO-020 criterion 5: SELF-SEEDING. Was `DbConfig::default()` over the
/// ambient `skeleton` dataset â€” the landmine three sessions tripped. Now each
/// call rebuilds the exact fixture in a dedicated database (the tests are
/// serialized by `lock()`, so one name is safe and each gets fresh data). The
/// permission assertions below are unchanged; only where the data comes from
/// changed.
fn broker() -> std::sync::Arc<Broker> {
    let (b, _cfg) = common::seeded_broker("pp_fixture");
    b
}

fn caller(user: &str, role: &str) -> Caller {
    Caller { user: user.into(), pass: format!("pw-{user}"), role: role.into() }
}

/// Reads a numeric field that may arrive in either shape.
///
/// Since WO-016 `Currency` is `decimal`, and SurrealDB serialises decimals as
/// JSON *strings* â€” so `as_f64()` returns `None` on precisely the fields that
/// matter most, and `.unwrap()` on it turns a representation change into a
/// panic. Rows written before the migration are still floats, so both shapes
/// are live in one table at once.
///
/// f64 is the right tool *here*: these assertions are about row visibility and
/// sort order, not monetary exactness. Money exactness is asserted decimally,
/// against the DB, in `decimal_rollups.rs` (REQ-6.2.1).
fn num(v: &serde_json::Value) -> f64 {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or_else(|| panic!("not a number or decimal string: {v}"))
}

/// One permission compiler: same request, three principals, DB-enforced
/// row counts â€” through the kernel's single db-read implementation.
#[test]
fn role_split_row_proof_through_kernel() {
    let _g = lock();
    let b = broker();
    let opts = ReadOpts::default();

    let clerk1 = b.db_read(&caller("clerk1", "clerk"), "purchase_order", None, &[], &opts).unwrap();
    let clerk2 = b.db_read(&caller("clerk2", "clerk"), "purchase_order", None, &[], &opts).unwrap();
    let manager = b.db_read(&caller("manager", "manager"), "purchase_order", None, &[], &opts).unwrap();

    println!(
        "kernel db-read rows: clerk1={} clerk2={} manager={}",
        clerk1.len(), clerk2.len(), manager.len()
    );
    // row-level: each clerk sees only their own; manager sees everything
    assert!(clerk1.len() < manager.len() && clerk2.len() < manager.len());
    assert_eq!(clerk1.len() + clerk2.len(), manager.len(), "clerks partition the table");
    for row in clerk1.iter() {
        assert_eq!(row["owner"].as_str().unwrap(), "app_user:clerk1");
    }
    for row in clerk2.iter() {
        assert_eq!(row["owner"].as_str().unwrap(), "app_user:clerk2");
    }
}

/// The kernel path returns byte-identical rows to a direct DB session under
/// the same principal â€” the broker adds the envelope filter and nothing else.
#[test]
fn kernel_read_matches_direct_session_bytes() {
    let _g = lock();
    let b = broker();
    // pin to the two stable WO-002 seed rows so the concurrently-running
    // write test can't race the two snapshots
    let filter = Filter::Cmp {
        path: vec![PathSegment::Field("title".into())],
        op: CmpOp::Inside,
        value: Value::List(vec![
            Value::Text("Alpha order".into()),
            Value::Text("Big draft".into()),
        ]),
    };
    let via_kernel = b
        .db_read(&caller("clerk1", "clerk"), "purchase_order", Some(&filter), &[], &ReadOpts::default())
        .unwrap();
    let direct = b
        .db
        .sql_as(
            "clerk1",
            "pw-clerk1",
            "SELECT * FROM purchase_order WHERE title INSIDE ['Alpha order', 'Big draft'] TIMEOUT 30s;",
        )
        .unwrap();
    let direct = direct.as_array().cloned().unwrap_or_default();
    assert_eq!(via_kernel.len(), 2);
    assert_eq!(
        serde_json::to_string(&via_kernel).unwrap(),
        serde_json::to_string(&direct).unwrap(),
        "kernel path must equal the raw session (no fields restricted on this doctype)"
    );
}

/// Structured filters compile and execute under the caller's authority.
#[test]
fn filtered_read_with_index_policy() {
    let _g = lock();
    let b = broker();
    let filter = Filter::And(vec![
        Filter::Cmp {
            path: vec![PathSegment::Field("total".into())],
            op: CmpOp::Gte,
            value: Value::Float(100.0),
        },
        Filter::Cmp {
            path: vec![PathSegment::Field("status".into())],
            op: CmpOp::Ne,
            value: Value::Text("Paid".into()),
        },
    ]);
    let opts = ReadOpts {
        order_by: Some((vec![PathSegment::Field("total".into())], SortDir::Desc)),
        limit: Some(10),
        start: None,
    };
    let rows = b
        .db_read(&caller("manager", "manager"), "purchase_order", Some(&filter), &[], &opts)
        .unwrap();
    println!("filtered rows: {}", rows.len());
    let mut last = f64::MAX;
    for r in &rows {
        let t = num(&r["total"]);
        assert!(t >= 100.0 && t <= last);
        last = t;
        assert_ne!(r["status"].as_str().unwrap(), "Paid");
    }
}

/// Field-level half of the permission compiler: a projection outside the
/// caller's readable set is refused before any query runs.
#[test]
fn field_envelope_enforced() {
    let _g = lock();
    let b = broker();
    let err = b
        .db_read(
            &caller("clerk1", "clerk"),
            "purchase_order",
            None,
            &[vec![PathSegment::Field("no_such_field".into())]],
            &ReadOpts::default(),
        )
        .unwrap_err();
    assert!(matches!(err, BrokerError::FieldNotReadable { .. }));
}

/// db-aggregate under the caller's authority: clerks aggregate only their
/// own rows â€” the DB's row filter applies to aggregation too.
#[test]
fn aggregate_respects_row_permissions() {
    let _g = lock();
    let b = broker();
    let metrics = vec![(Metric::Count, vec![]), (Metric::Sum, vec![PathSegment::Field("total".into())])];
    let clerk = b.db_aggregate(&caller("clerk1", "clerk"), "purchase_order", None, &[], &metrics).unwrap();
    let mgr = b.db_aggregate(&caller("manager", "manager"), "purchase_order", None, &[], &metrics).unwrap();
    let count = |rows: &Vec<serde_json::Value>| rows.first().and_then(|r| r["m0"].as_u64()).unwrap_or(0);
    println!("aggregate counts: clerk1={} manager={}", count(&clerk), count(&mgr));
    assert!(count(&clerk) < count(&mgr));
}

/// WO-002 acceptance flow through the kernel: db-write fires both external
/// hook classes (module-4 fold-in pending), the write lands under the
/// caller's session, and the mutation is hook-shaped.
#[test]
fn acceptance_write_through_hooks() {
    let _g = lock();
    let b = broker();
    let chain = frust_kernel::broker::HookChain::default();
    let doc = vec![
        ("title".to_string(), Value::Text("Kernel write".into())),
        ("total".to_string(), Value::Float(20000.0)),
        ("notes".to_string(), Value::Text("via broker".into())),
        ("status".to_string(), Value::Text("Draft".into())),
    ];
    let created = b
        .db_write(&caller("clerk1", "clerk"), &chain, WriteOp::Create, "purchase_order", None, &doc)
        .unwrap();
    // plugin flags >10k drafts, script applies 15% tax: 20000 -> 23000
    assert_eq!(created["status"].as_str().unwrap(), "Needs Approval");
    assert!((num(&created["total"]) - 23000.0).abs() < 0.01);
    assert_eq!(created["owner"].as_str().unwrap(), "app_user:clerk1");

    // reject path: typed error from the hook stage
    let bad = vec![("total".to_string(), Value::Float(-5.0)), ("title".to_string(), Value::Text("x".into()))];
    let err = b
        .db_write(&caller("clerk1", "clerk"), &chain, WriteOp::Create, "purchase_order", None, &bad)
        .unwrap_err();
    assert!(matches!(err, BrokerError::HookRejected { .. }));

    // cleanup: keep the shared WO-002 dataset stable across runs
    b.db
        .sql_root("DELETE purchase_order WHERE title = 'Kernel write';")
        .unwrap();
}

