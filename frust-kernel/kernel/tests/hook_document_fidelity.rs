//! WO-028 — hooks see the FULL document (ADR-006 edge 3), without opening a
//! lost-update race under the concurrent loop WO-025 shipped.
//!
//! The bug: `db_write_inner` handed hooks the caller's PARTIAL payload while
//! its own comment promised the full document. The accounting seed's
//! reconciliation script did `doc.lines || []` on an absent field and wrote
//! `[]` back; `MERGE` faithfully destroyed the stored child rows. Any hook
//! that derives or echoes a field had the same power, on every partial update.
//!
//! The fix has two halves and BOTH are load-bearing:
//!   1. read the record (under the caller's session) and merge the delta, so
//!      the hook sees truth;
//!   2. persist only what actually CHANGED, so a hooked update stays a partial
//!      write and two disjoint updates cannot clobber each other.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain, HookDispatch};
use frust_kernel::contract::*;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::single_tenant;
use frust_kernel::meta::{access_ddl, identity_ddl};
use frust_kernel::sync::MetadataSync;

/// A hook that ECHOES the whole document back — the shape that caused the bug.
/// It also derives `memo` from `lines`, so it must be able to SEE `lines`.
struct EchoHooks;
impl HookDispatch for EchoHooks {
    fn validate(&self, _dt: &str, doc: &[(String, Value)]) -> Result<Vec<(String, Value)>, BrokerError> {
        let n = doc
            .iter()
            .find(|(k, _)| k == "lines")
            .map(|(_, v)| match v {
                Value::List(items) => items.len(),
                _ => 0,
            })
            .unwrap_or(0);
        let mut out = doc.to_vec();
        out.retain(|(k, _)| k != "memo");
        out.push(("memo".into(), Value::Text(format!("lines={n}"))));
        Ok(out)
    }
}

fn setup(name: &str) -> (Arc<Broker>, Db) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&format!(
        "{}; {}; DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE; \
         CREATE app_user:u SET name = 'u', role = 'manager', pass = crypto::argon2::generate('pw-u'); \
         CREATE doctype CONTENT {{ name: 'inv', fields: [ \
            {{ fieldname: 'customer', fieldtype: 'Data' }}, \
            {{ fieldname: 'memo', fieldtype: 'Data' }}, \
            {{ fieldname: 'total', fieldtype: 'Currency' }}, \
            {{ fieldname: 'lines', fieldtype: 'Table', options: ['inv_line'] }} ] }}; \
         CREATE doctype CONTENT {{ name: 'inv_line', fields: [ \
            {{ fieldname: 'item', fieldtype: 'Data' }} ] }};",
        identity_ddl(),
        access_ddl(),
    ))
    .unwrap();
    MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");
    (Arc::new(Broker::new(scoped_db(&cfg), Box::new(EchoHooks))), db)
}

fn u() -> Caller {
    Caller { user: "u".into(), pass: "pw-u".into(), role: "manager".into() }
}

fn lines2() -> Value {
    Value::List(vec![
        Value::Object(vec![("item".into(), Value::Text("a".into()))]),
        Value::Object(vec![("item".into(), Value::Text("b".into()))]),
    ])
}

fn create(b: &Broker) -> String {
    let out = b
        .db_write(&u(), &HookChain::default(), WriteOp::Create, "inv", None, &[
            ("customer".to_string(), Value::Text("acme".into())),
            ("total".to_string(), Value::Decimal("50.98".into())),
            ("lines".to_string(), lines2()),
        ])
        .expect("create");
    out["id"].as_str().unwrap().split_once(':').unwrap().1.to_string()
}

fn field(db: &Db, key: &str, f: &str) -> serde_json::Value {
    let v = db.sql_root(&format!("SELECT VALUE {f} FROM inv:{key};")).unwrap();
    v.as_array().and_then(|a| a.first()).cloned().unwrap_or(serde_json::Value::Null)
}

/// THE REGRESSION. A partial update must not destroy fields it never mentioned
/// — the failure that shipped through the whole platform milestone.
#[test]
fn a_partial_update_does_not_destroy_unmentioned_child_rows() {
    let (b, db) = setup("hf_regress");
    let key = create(&b);
    assert_eq!(field(&db, &key, "array::len(lines)"), serde_json::json!(2), "created with 2 lines");

    // touch ONE unrelated field
    b.db_write(&u(), &HookChain::default(), WriteOp::Update, "inv", Some(&key),
        &[("customer".to_string(), Value::Text("changed".into()))])
        .expect("partial update");

    assert_eq!(
        field(&db, &key, "array::len(lines)"),
        serde_json::json!(2),
        "the embedded lines must SURVIVE an update that never mentioned them"
    );
    assert_eq!(field(&db, &key, "customer"), serde_json::json!("changed"), "and the delta applied");
}

/// The contract, directly: a hook must SEE the fields it was not sent. The
/// echo hook derives `memo` from `lines`, so `memo` proves what it saw.
#[test]
fn a_hook_sees_the_full_document_on_a_partial_update() {
    let (b, db) = setup("hf_sees");
    let key = create(&b);
    assert_eq!(field(&db, &key, "memo"), serde_json::json!("lines=2"), "hook saw 2 lines at create");

    b.db_write(&u(), &HookChain::default(), WriteOp::Update, "inv", Some(&key),
        &[("customer".to_string(), Value::Text("x".into()))])
        .expect("partial update");

    assert_eq!(
        field(&db, &key, "memo"),
        serde_json::json!("lines=2"),
        "on a partial update the hook must still SEE the stored lines (not an empty doc)"
    );
}

/// THE CONCURRENCY PROOF. Two writers, disjoint fields, same record — both
/// deltas must survive. A read-modify-write that persisted the whole document
/// would lose one of them under WO-025's parallel loop.
#[test]
fn concurrent_disjoint_updates_both_survive() {
    let (b, db) = setup("hf_race");
    let key = create(&b);

    let mut handles = Vec::new();
    for (f, v) in [("customer", "writer-A"), ("memo", "writer-B")] {
        let b = Arc::clone(&b);
        let key = key.clone();
        handles.push(std::thread::spawn(move || {
            b.db_write(&u(), &HookChain::default(), WriteOp::Update, "inv", Some(&key),
                &[(f.to_string(), Value::Text(v.into()))])
        }));
    }
    for h in handles {
        let _ = h.join().expect("thread");
    }

    // `memo` is also hook-derived, so writer-B's explicit value may be
    // overwritten by the hook — what must NOT happen is losing `customer`
    // or the lines.
    assert_eq!(
        field(&db, &key, "customer"),
        serde_json::json!("writer-A"),
        "writer A's delta must survive a concurrent write to a different field"
    );
    assert_eq!(
        field(&db, &key, "array::len(lines)"),
        serde_json::json!(2),
        "and neither writer may destroy the untouched child rows"
    );
}

/// The representation trap (WO-016's lesson, new location): a hook echoing a
/// decimal back unchanged must NOT count as a change. If it did, every hooked
/// update would rewrite the whole document and reopen the race above.
#[test]
fn echoing_an_unchanged_decimal_is_not_a_change() {
    let (b, db) = setup("hf_decimal");
    let key = create(&b);

    // update an unrelated field; the hook echoes `total` back verbatim
    b.db_write(&u(), &HookChain::default(), WriteOp::Update, "inv", Some(&key),
        &[("customer".to_string(), Value::Text("y".into()))])
        .expect("update");

    // the stored decimal is untouched and still EXACT
    let exact = db.sql_root(&format!("SELECT VALUE total = 50.98dec FROM inv:{key};")).unwrap();
    assert_eq!(
        exact.as_array().and_then(|a| a.first()).cloned().unwrap_or(serde_json::Value::Null),
        serde_json::json!(true),
        "an echoed decimal must round-trip exactly, not drift or re-type"
    );
}
