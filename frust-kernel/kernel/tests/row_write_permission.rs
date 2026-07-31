//! WO-020: the row-write permission (Finding B), ADR option 2.
//!
//! The four-way enforcement proof, each asserted **through the broker under
//! the caller's own session** — "the DB is the enforcer" is only true if the
//! test writes as the actual principal, never as manager standing in.
//!
//!   clerk edits own DRAFT      → OK
//!   clerk edits own SUBMITTED  → refused typed (not swallowed)
//!   clerk edits ANOTHER's row  → refused
//!   manager edits anything     → OK
//!
//! And Finding A general: a write that changes zero rows returns a typed
//! error, never `Ok`.
//!
//! **Self-seeding** (criterion 5): this file builds its own users and rows
//! under known ownership and docstatus. It does NOT touch the ambient
//! `skeleton` dataset — the landmine that has bitten three prior sessions.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::*;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::single_tenant;
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{access_ddl, identity_ddl};
use frust_kernel::sync::MetadataSync;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

/// A fully self-seeded world: identity, two clerks + a manager, and a
/// submittable `claim` doctype. Nothing ambient.
fn setup(name: &str) -> (Arc<Broker>, Db) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&format!(
        "{}; {}; DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE; \
         CREATE app_user:alice SET name = 'alice', role = 'clerk', pass = crypto::argon2::generate('pw-alice'); \
         CREATE app_user:bob   SET name = 'bob',   role = 'clerk', pass = crypto::argon2::generate('pw-bob'); \
         CREATE app_user:mgr   SET name = 'mgr',   role = 'manager', pass = crypto::argon2::generate('pw-mgr'); \
         CREATE doctype CONTENT {{ name: 'claim', submittable: true, fields: [ \
            {{ fieldname: 'title', fieldtype: 'Data' }}, \
            {{ fieldname: 'note', fieldtype: 'Data' }} ] }}; \
         CREATE doctype CONTENT {{ name: 'memo', submittable: false, fields: [ \
            {{ fieldname: 'body', fieldtype: 'Data' }} ] }};",
        identity_ddl(),
        access_ddl(),
    ))
    .unwrap();
    MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");
    let b = Arc::new(Broker::new(scoped_db(&cfg), Box::new(WasmHooks::load(artifacts()).unwrap())));
    (b, db)
}

fn alice() -> Caller { Caller { user: "alice".into(), pass: "pw-alice".into(), role: "clerk".into() } }
fn bob() -> Caller { Caller { user: "bob".into(), pass: "pw-bob".into(), role: "clerk".into() } }
fn mgr() -> Caller { Caller { user: "mgr".into(), pass: "pw-mgr".into(), role: "manager".into() } }

fn create(b: &Broker, who: &Caller, dt: &str, title: &str) -> String {
    let doc = b
        .db_write(who, &HookChain::default(), WriteOp::Create, dt, None,
            &[("title".to_string(), Value::Text(title.into())),
              ("body".to_string(), Value::Text(title.into()))])
        .expect("create");
    doc["id"].as_str().unwrap().split_once(':').unwrap().1.to_string()
}

fn edit(b: &Broker, who: &Caller, dt: &str, key: &str, note: &str) -> Result<serde_json::Value, BrokerError> {
    b.db_write(who, &HookChain::default(), WriteOp::Update, dt, Some(key),
        &[("note".to_string(), Value::Text(note.into())),
          ("body".to_string(), Value::Text(note.into()))])
}

/// ① Owner edits own draft → OK.
#[test]
fn owner_edits_own_draft() {
    let (b, db) = setup("rw_own_draft");
    let key = create(&b, &alice(), "claim", "expenses");
    edit(&b, &alice(), "claim", &key, "updated by owner").expect("owner may edit own draft");
    let row = db.sql_root(&format!("SELECT note FROM claim:{key};")).unwrap();
    assert_eq!(row.as_array().unwrap()[0]["note"], serde_json::json!("updated by owner"),
        "the edit actually landed");
}

/// ② Owner edits own SUBMITTED doc → refused typed (Finding A: not swallowed).
#[test]
fn owner_cannot_edit_own_submitted_doc() {
    let (b, db) = setup("rw_own_submitted");
    let key = create(&b, &alice(), "claim", "expenses");
    // a manager advances it to docstatus 1 (owners cannot — that is the point)
    db.sql_root(&format!("UPDATE claim:{key} SET docstatus = 1;")).unwrap();

    let e = edit(&b, &alice(), "claim", &key, "sneaky post-submit edit")
        .expect_err("a submitted doc is immutable to its owner");
    println!("refused: {e:?}");
    assert!(matches!(e, BrokerError::PermissionDenied { .. }), "typed, not swallowed: {e:?}");
    assert!(format!("{e:?}").contains("E_WRITE_NO_ROWS"), "{e:?}");

    let row = db.sql_root(&format!("SELECT note FROM claim:{key};")).unwrap();
    assert_ne!(row.as_array().unwrap()[0]["note"], serde_json::json!("sneaky post-submit edit"),
        "and nothing was written");
}

/// ③ Owner cannot advance docstatus themselves (the after-state consequence).
#[test]
fn owner_cannot_advance_docstatus() {
    let (b, db) = setup("rw_advance");
    let key = create(&b, &alice(), "claim", "expenses");
    let e = b.db_write(&alice(), &HookChain::default(), WriteOp::Update, "claim", Some(&key),
        &[("docstatus".to_string(), Value::Int(1))])
        .expect_err("an owner may not move the lattice");
    println!("refused: {e:?}");
    assert!(matches!(e, BrokerError::PermissionDenied { .. }), "{e:?}");
    let row = db.sql_root(&format!("SELECT docstatus FROM claim:{key};")).unwrap();
    assert_eq!(row.as_array().unwrap()[0]["docstatus"], serde_json::json!(0), "still a draft");
}

/// ④ Clerk edits ANOTHER clerk's draft → refused.
#[test]
fn a_clerk_cannot_edit_anothers_draft() {
    let (b, db) = setup("rw_cross");
    let key = create(&b, &alice(), "claim", "alice's claim");
    let e = edit(&b, &bob(), "claim", &key, "bob was here")
        .expect_err("bob does not own alice's claim");
    println!("refused: {e:?}");
    assert!(matches!(e, BrokerError::PermissionDenied { .. }), "{e:?}");
    // bob cannot even see it, let alone write it
    let row = db.sql_root(&format!("SELECT note FROM claim:{key};")).unwrap();
    assert_ne!(row.as_array().unwrap()[0]["note"], serde_json::json!("bob was here"));
}

/// ⑤ Manager edits anything — own or others', draft or submitted.
#[test]
fn a_manager_edits_anything() {
    let (b, db) = setup("rw_manager");
    let key = create(&b, &alice(), "claim", "alice's claim");
    edit(&b, &mgr(), "claim", &key, "manager touched a draft").expect("manager edits a draft");

    db.sql_root(&format!("UPDATE claim:{key} SET docstatus = 1;")).unwrap();
    edit(&b, &mgr(), "claim", &key, "manager touched a submitted doc")
        .expect("manager edits a submitted doc too");
    let row = db.sql_root(&format!("SELECT note FROM claim:{key};")).unwrap();
    assert_eq!(row.as_array().unwrap()[0]["note"], serde_json::json!("manager touched a submitted doc"));
}

/// A NON-submittable doctype has no docstatus, so ownership alone governs —
/// the owner edits freely (the conditional clause, proven).
#[test]
fn owner_edits_a_non_submittable_doc_freely() {
    let (b, db) = setup("rw_nonsub");
    let key = create(&b, &alice(), "memo", "a memo");
    edit(&b, &alice(), "memo", &key, "owner edits a plain doc").expect("no docstatus, no draft gate");
    let row = db.sql_root(&format!("SELECT body FROM memo:{key};")).unwrap();
    assert_eq!(row.as_array().unwrap()[0]["body"], serde_json::json!("owner edits a plain doc"));

    // still owner-scoped: bob cannot touch it
    edit(&b, &bob(), "memo", &key, "bob").expect_err("non-submittable is still owner-gated");
}

/// Finding A general: a write to a NON-EXISTENT record is a typed error too,
/// not a silent `Ok` — the same swallow, the same fix, any doctype.
#[test]
fn a_write_to_a_missing_record_is_typed_not_swallowed() {
    let (b, _db) = setup("rw_missing");
    let e = edit(&b, &mgr(), "claim", "does_not_exist", "into the void")
        .expect_err("a manager write to a ghost still changes nothing");
    println!("refused: {e:?}");
    assert!(format!("{e:?}").contains("E_WRITE_NO_ROWS"), "even a manager gets told: {e:?}");
}
