//! Money is decimal end-to-end through the aggregates layer.
//!
//! The test values are chosen to EXPOSE float error, not to avoid it: sums
//! of the 0.1 family that `f64` provably cannot represent. A rollup that
//! equals the exact decimal sum is the requirement; "close enough" is the
//! defect (REQ-6.2.1).
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::aggregates::{ItemSales, RollupWorker};
use frust_kernel::broker::{Broker, Caller, HookChain, HookDispatch};
use frust_kernel::contract::*;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};
use frust_kernel::meta::{access_ddl, identity_ddl};
use frust_kernel::sync::MetadataSync;

struct PassHooks;
impl HookDispatch for PassHooks {
    fn validate(&self, doctype: &str, doc: &[(String, Value)]) -> Result<Vec<(String, Value)>, BrokerError> {
        Ok(doc.to_vec())
    }
}

fn setup(name: &str) -> (Db, ResolvedTenant) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&format!(
        "{}; {}; \
         CREATE app_user:u SET name = 'u', role = 'manager', pass = crypto::argon2::generate('pw-u'); \
         DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE; \
         CREATE doctype CONTENT {{ name: 'dsrc', fields: [ \
            {{ fieldname: 'lines', fieldtype: 'Table', options: ['dline'] }}, \
            {{ fieldname: 'total', fieldtype: 'Currency' }} ], \
            aggregates: [ {{ kind: 'worker', rollup: 'droll', handler: 'item_sales' }} ] }}; \
         CREATE doctype CONTENT {{ name: 'dline', fields: [ \
            {{ fieldname: 'item', fieldtype: 'Data' }}, {{ fieldname: 'amount', fieldtype: 'Currency' }} ] }}; \
         CREATE doctype CONTENT {{ name: 'droll', fields: [ \
            {{ fieldname: 'k', fieldtype: 'Data' }}, {{ fieldname: 'qty', fieldtype: 'Data' }}, \
            {{ fieldname: 'amount', fieldtype: 'Currency' }} ] }};",
        identity_ddl(),
        access_ddl(),
    ))
    .unwrap();
    MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");
    (db, cfg)
}

fn caller() -> Caller {
    Caller { user: "u".into(), pass: "pw-u".into(), role: "manager".into() }
}

/// Criterion 1: 100 lines of 0.10 must roll up to EXACTLY 10.00 â€” the sum
/// f64 gets wrong. Asserted as an exact decimal string, never an epsilon.
#[test]
fn tenths_at_volume_roll_up_exactly() {
    let (db, cfg) = setup("dec_roll");
    let b = Arc::new(Broker::new(scoped_db(&cfg), Box::new(PassHooks)));

    // the float control: this is the sum that f64 cannot represent
    let mut f = 0.0f64;
    for _ in 0..100 {
        f += 0.1;
    }
    assert!(f != 10.0, "f64 control must miss 10.0, else the test proves nothing (got {f})");

    // 100 documents, one 0.10 line each, all on the same item
    for i in 0..100 {
        let line = Value::Object(vec![
            ("item".to_string(), Value::Text("penny".into())),
            ("amount".to_string(), Value::Decimal("0.10".into())),
        ]);
        let doc = vec![
            ("lines".to_string(), Value::List(vec![line])),
            ("total".to_string(), Value::Decimal("0.10".into())),
        ];
        b.db_write(&caller(), &HookChain::default(), WriteOp::Create, "dsrc", None, &doc)
            .unwrap_or_else(|e| panic!("write {i}: {e:?}"));
    }

    let worker = RollupWorker::new(&db, Box::new(ItemSales::new("dsrc", "droll"))).unwrap();
    worker.drain().expect("drain the feed");

    // DECIMAL-equal, not string-equal: SurrealDB normalises trailing zeros,
    // so `10` and `10.00` are the same exact value. Ask the DB to compare.
    let rows = db
        .sql_root("SELECT <string>amount AS shown, amount = 10.00dec AS exact FROM droll;")
        .unwrap();
    let rows = rows.as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 1, "one item bucket: {rows:?}");
    let shown = rows[0]["shown"].as_str().unwrap_or_default();
    assert_eq!(
        rows[0]["exact"],
        serde_json::json!(true),
        "100 x 0.10 must be EXACTLY 10.00 (shown as {shown})"
    );

    // and the column really is decimal, not a float that printed nicely
    let kind = db
        .sql_root("SELECT type::is_decimal(amount) AS d, type::is_float(amount) AS f FROM droll;")
        .unwrap();
    let kind = &kind.as_array().unwrap()[0];
    assert_eq!(kind["d"], serde_json::json!(true), "rollup column is decimal");
    assert_eq!(kind["f"], serde_json::json!(false), "rollup column is NOT float");
}

/// Criterion 1 (reversal): the signed fold must cancel EXACTLY. Editing a
/// line down and back up leaves the rollup where it started â€” no drift.
#[test]
fn signed_fold_cancels_exactly() {
    let (db, cfg) = setup("dec_fold");
    let b = Arc::new(Broker::new(scoped_db(&cfg), Box::new(PassHooks)));
    let line = |amt: &str| {
        Value::List(vec![Value::Object(vec![
            ("item".to_string(), Value::Text("widget".into())),
            ("amount".to_string(), Value::Decimal(amt.into())),
        ])])
    };
    let created = b
        .db_write(
            &caller(),
            &HookChain::default(),
            WriteOp::Create,
            "dsrc",
            None,
            &[("lines".to_string(), line("0.30"))],
        )
        .unwrap();
    let key = created["id"].as_str().unwrap().split_once(':').unwrap().1.to_string();

    let worker = RollupWorker::new(&db, Box::new(ItemSales::new("dsrc", "droll"))).unwrap();
    worker.drain().unwrap();

    // 0.30 -> 0.10 -> 0.30 : three float ops that would drift; decimal must not
    for amt in ["0.10", "0.30"] {
        b.db_write(
            &caller(),
            &HookChain::default(),
            WriteOp::Update,
            "dsrc",
            Some(&key),
            &[("lines".to_string(), line(amt))],
        )
        .unwrap();
        worker.drain().unwrap();
    }

    let rows = db
        .sql_root("SELECT <string>amount AS shown, amount = 0.30dec AS exact FROM droll;")
        .unwrap();
    let row = &rows.as_array().unwrap()[0];
    assert_eq!(
        row["exact"],
        serde_json::json!(true),
        "edit down and back up must return EXACTLY 0.30 (shown as {})",
        row["shown"].as_str().unwrap_or_default()
    );
}

/// Criterion 4: the migration path. A rollup holding pre-conversion FLOAT
/// values is upgraded by recomputing from source â€” never by converting the
/// stored float, which would launder the error into a decimal that merely
/// looks exact.
#[test]
fn recompute_from_source_replaces_legacy_float_state() {
    let (db, cfg) = setup("dec_migrate");
    let b = Arc::new(Broker::new(scoped_db(&cfg), Box::new(PassHooks)));
    for amt in ["0.10", "0.20"] {
        let doc = vec![(
            "lines".to_string(),
            Value::List(vec![Value::Object(vec![
                ("item".to_string(), Value::Text("legacy".into())),
                ("amount".to_string(), Value::Decimal(amt.into())),
            ])]),
        )];
        b.db_write(&caller(), &HookChain::default(), WriteOp::Create, "dsrc", None, &doc).unwrap();
    }

    // simulate the legacy pre-decimal world: a rollup doc holding the float sum that
    // f64 would have produced, and a cursor already past the feed
    db.sql_root(
        "UPSERT droll:legacy SET k = 'legacy', qty = 0, amount = 0.30000000000000004f; \
         UPSERT agg_cursor:droll SET vs = 999999999999999, updated_at = time::now();",
    )
    .unwrap();

    let worker = RollupWorker::new(&db, Box::new(ItemSales::new("dsrc", "droll"))).unwrap();
    worker.recompute_from_source().expect("recompute");

    let rows = db
        .sql_root("SELECT <string>amount AS shown, amount = 0.30dec AS exact, type::is_decimal(amount) AS d FROM droll;")
        .unwrap();
    let rows = rows.as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 1, "one bucket after recompute: {rows:?}");
    assert_eq!(rows[0]["exact"], serde_json::json!(true), "recomputed to EXACTLY 0.30");
    assert_eq!(rows[0]["d"], serde_json::json!(true), "and it is a decimal now");
    assert_ne!(
        rows[0]["shown"].as_str().unwrap_or_default(),
        "0.30000000000000004",
        "the laundered float must be gone, not converted"
    );
}

/// Criterion 3: full-scan reconciliation, decimal-equal (not epsilon-equal).
#[test]
fn reconciliation_is_decimal_exact() {
    let (db, cfg) = setup("dec_recon");
    let b = Arc::new(Broker::new(scoped_db(&cfg), Box::new(PassHooks)));
    let amounts = ["0.10", "0.20", "1.005", "99.99", "0.01"];
    for a in amounts {
        let doc = vec![(
            "lines".to_string(),
            Value::List(vec![Value::Object(vec![
                ("item".to_string(), Value::Text("mix".into())),
                ("amount".to_string(), Value::Decimal(a.into())),
            ])]),
        )];
        b.db_write(&caller(), &HookChain::default(), WriteOp::Create, "dsrc", None, &doc).unwrap();
    }
    let worker = RollupWorker::new(&db, Box::new(ItemSales::new("dsrc", "droll"))).unwrap();
    worker.drain().unwrap();

    // live aggregate over the source, computed by the DB in decimal
    let live = db
        .sql_root(
            "SELECT VALUE <string>math::sum(amount) FROM (SELECT VALUE lines FROM dsrc).flatten() GROUP ALL;",
        )
        .unwrap();
    let live = live.as_array().and_then(|a| a.first()).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let rolled = db.sql_root("SELECT VALUE <string>amount FROM droll;").unwrap();
    let rolled = rolled.as_array().and_then(|a| a.first()).and_then(|v| v.as_str()).unwrap_or_default().to_string();

    println!("live={live} rolled={rolled}");
    assert_eq!(rolled, live, "rollup must equal the live aggregate EXACTLY");
    assert_eq!(rolled, "101.305", "and equal the hand-computed exact sum");
}

