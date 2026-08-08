//! A CREATE that persists nothing must not report success.
//!
//! This was already closed for UPDATE (`E_WRITE_NO_ROWS`): a write the database
//! refuses comes back as an EMPTY RESULT SET, not an error, so returning `Ok`
//! made a refused write look like a completed one. The guard was written as
//! `(WriteOp::Update, None)`, and CREATE fell through the catch-all —
//! `POST /write/{write-closed rollup}` answered
//! `200 {"action":"created","created":null,"record":null}` while creating no
//! row.
//!
//! The database's refusal is correct and unchanged here. What changes is the
//! sentence the kernel says about it.

mod common;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::{BrokerError, Value, WriteOp};
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{access_ddl, identity_ddl, meta_ddl};
use frust_kernel::sync::MetadataSync;
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}
fn mgr() -> Caller {
    Caller { user: "mgr".into(), pass: "pw-mgr".into(), role: "manager".into() }
}

/// A source DocType with a Tier-1 aggregate, and the rollup it feeds. Declaring
/// the aggregate is what compiles the rollup **write-closed**: the EVENT writes
/// it, record users never can.
fn setup(name: &str) -> (Broker, Db, ResolvedTenant) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&format!("{}; {}; {}", meta_ddl(), identity_ddl(), access_ddl())).unwrap();
    db.sql_root(
        "CREATE app_user:mgr SET name = 'mgr', role = 'manager', \
         pass = crypto::argon2::generate('pw-mgr');",
    )
    .unwrap();
    db.sql_root(
        "CREATE doctype CONTENT { name: 'ledger_entry', fields: [\
           { fieldname: 'party', fieldtype: 'Data' }, \
           { fieldname: 'amount', fieldtype: 'Currency' }], \
         aggregates: [{ kind: 'counter', rollup: 'party_total', key: 'party', \
           metrics: [{ name: 'charged', field: 'amount' }] }] };",
    )
    .unwrap();
    db.sql_root(
        "CREATE doctype CONTENT { name: 'party_total', fields: [\
           { fieldname: 'k', fieldtype: 'Data' }, \
           { fieldname: 'charged', fieldtype: 'Currency' }] };",
    )
    .unwrap();
    MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");
    let hooks = WasmHooks::load(artifacts()).expect("hooks").with_script_source();
    (Broker::new(scoped_db(&cfg), Box::new(hooks)), db, cfg)
}

fn count_of(db: &Db, table: &str) -> usize {
    db.sql_root(&format!("SELECT count() FROM {table} GROUP ALL;"))
        .ok()
        .and_then(|v| {
            v.as_array()
                .and_then(|a| a.first())
                .and_then(|r| r["count"].as_u64())
        })
        .unwrap_or(0) as usize
}

/// A create the database refuses must come back as a typed refusal — never as
/// a success carrying nothing.
#[test]
fn a_create_that_persists_nothing_is_refused_not_reported_as_created() {
    let (broker, db, _cfg) = setup("wo057_closed");

    let before = count_of(&db, "party_total");
    let out = broker.db_write(
        &mgr(),
        &HookChain::default(),
        WriteOp::Create,
        "party_total",
        None,
        &[("k".to_string(), Value::Text("Probe Co".into())),
          ("charged".to_string(), Value::Decimal("99.00".into()))],
    );
    println!("create into a write-closed rollup -> {out:?}");

    let err = out.expect_err(
        "a CREATE that persisted no row must be a typed refusal — reporting success for a \
         write that did not happen is the silent-wrong class",
    );
    let msg = format!("{err:?}");
    assert!(msg.contains("E_WRITE_NO_ROWS"), "typed, same family as the UPDATE guard: {msg}");
    assert!(msg.contains("party_total"), "names the table: {msg}");

    // the DB's own behaviour is unchanged — still zero rows, still write-closed
    assert_eq!(
        count_of(&db, "party_total"),
        before,
        "the rollup must still be write-closed; the fix changes the RESPONSE, not the refusal"
    );
}

/// **The other side.** A legitimate create still returns its record — or the
/// fix has traded a false success for a false failure.
#[test]
fn a_legitimate_create_still_returns_its_record() {
    let (broker, db, _cfg) = setup("wo057_open");

    let out = broker
        .db_write(
            &mgr(),
            &HookChain::default(),
            WriteOp::Create,
            "ledger_entry",
            None,
            &[("party".to_string(), Value::Text("Meridian".into())),
              ("amount".to_string(), Value::Decimal("300.00".into()))],
        )
        .expect("a create the database accepts must still succeed");
    println!("legitimate create -> {out}");
    assert!(
        out.get("id").and_then(|i| i.as_str()).is_some_and(|s| s.starts_with("ledger_entry:")),
        "the created record comes back: {out}"
    );
    assert_eq!(count_of(&db, "ledger_entry"), 1, "and it really is stored");

    // and the write-closed rollup was still maintained BY THE EVENT — the
    // refusal above is about who may write it directly, never about whether
    // the ladder works
    let rolled = db
        .sql_root("SELECT k, charged FROM party_total;")
        .expect("read rollup");
    println!("rollup maintained by the EVENT: {rolled}");
    assert_eq!(
        rolled.as_array().map(Vec::len),
        Some(1),
        "the aggregate still maintains the rollup a record user may not write: {rolled}"
    );
}

/// The UPDATE half is unchanged — this adds a sibling arm, it does not rewrite
/// the guard.
#[test]
fn the_update_guard_is_unchanged() {
    let (broker, _db, _cfg) = setup("wo057_update");
    let err = broker
        .db_write(
            &mgr(),
            &HookChain::default(),
            WriteOp::Update,
            "ledger_entry",
            Some("nosuchrecord"),
            &[("party".to_string(), Value::Text("x".into()))],
        )
        .expect_err("updating a record that does not exist is still refused");
    let msg = format!("{err:?}");
    println!("update of a missing record -> {msg}");
    assert!(msg.contains("E_WRITE_NO_ROWS"), "{msg}");
    assert!(matches!(err, BrokerError::PermissionDenied { .. }), "{err:?}");
}
