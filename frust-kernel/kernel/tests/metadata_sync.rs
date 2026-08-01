//! WO-005 module 3 close-out: the sliver's replacement, proven end-to-end.
//! DocType metadata -> ResourceSpec -> the ported engine (diff, history,
//! lock, gate) -> live schema — plus the ADR-008/009 additions and the
//! WO-002 Finding-A assertion.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};
use frust_kernel::sync::{doctype_ddl, doctype_spec, DocTypeDef, MetadataSync};

fn fresh(name: &str) -> (Db, ResolvedTenant) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};"))
        .expect("provision test database");
    (db, cfg)
}

fn seed_doctype(db: &Db, json: &str) {
    db.sql_root(&format!("DEFINE TABLE IF NOT EXISTS doctype SCHEMALESS PERMISSIONS NONE;"))
        .unwrap();
    db.sql_root(&format!("CREATE doctype CONTENT {json};")).unwrap();
}

const INVOICE_META: &str = r#"{
    name: 'sinv',
    submittable: true,
    fields: [
        { fieldname: 'title', fieldtype: 'Data', required: true },
        { fieldname: 'total', fieldtype: 'Currency' },
        { fieldname: 'supplier', fieldtype: 'Link', options: ['supplier2'] },
        { fieldname: 'lines', fieldtype: 'Table' }
    ]
}"#;

const SUPPLIER_META: &str = r#"{
    name: 'supplier2',
    fields: [ { fieldname: 'sname', fieldtype: 'Data', required: true } ]
}"#;

/// The whole path: metadata -> engine -> live schema, with dependency
/// ordering (sinv links supplier2, so supplier2 must sync first), the
/// lattice EVENT on the submittable doctype, and an embedded child field.
#[test]
fn metadata_syncs_through_the_engine() {
    let (db, cfg) = fresh("msync_e2e");
    seed_doctype(&db, INVOICE_META);
    seed_doctype(&db, SUPPLIER_META);

    let sync = MetadataSync { base: cfg };
    let applied = sync.sync(&db).expect("sync succeeds").applied;
    assert_eq!(applied, 2, "both doctypes applied");

    // engine bookkeeping exists: one history row per resource
    let hist = db
        .sql_root("SELECT count() FROM _framework_migration WHERE version = 1 GROUP ALL;")
        .unwrap();
    let rows = hist.as_array().and_then(|a| a.first()).and_then(|r| r["count"].as_u64()).unwrap_or(0);
    assert_eq!(rows, 2, "engine history recorded both resources");

    // second sync is a true no-op (diff-based, not blind OVERWRITE)
    let applied2 = sync.sync(&db).expect("re-sync succeeds").applied;
    assert_eq!(applied2, 0, "unchanged metadata -> empty diff -> no-op");

    // the lattice EVENT is live and enforces (WO-004 semantics, machine code)
    db.sql_root("CREATE sinv:1 SET title = 'x', docstatus = 1, status = 'Draft', owner = app_user:nobody;")
        .ok(); // owner type may refuse under root without app_user table; create minimally instead
    db.sql_root("DEFINE TABLE IF NOT EXISTS app_user SCHEMALESS;").unwrap();
    db.sql_root("CREATE sinv:2 SET title = 'y', docstatus = 1;").unwrap();
    let raw = db.sql_root_raw("UPDATE sinv:2 SET docstatus = 0;").unwrap();
    let err = raw
        .iter()
        .find(|s| s.get("status").and_then(|x| x.as_str()) != Some("OK"))
        .map(|s| s.get("result").map(std::string::ToString::to_string).unwrap_or_default())
        .unwrap_or_default();
    assert!(
        err.contains("FRUST:E_DOCSTATUS:ILLEGAL_TRANSITION_1_0"),
        "lattice EVENT enforces with machine code, got: {err}"
    );

    // the embedded child field roundtrips nested data (ADR-008 embedded default)
    db.sql_root("CREATE sinv:3 SET title = 'z', lines = [{ item: 'a', qty: 2 }, { item: 'b', qty: 1 }];")
        .unwrap();
    let lines = db.sql_root("SELECT VALUE lines FROM sinv:3;").unwrap();
    assert_eq!(lines.as_array().and_then(|a| a.first()).and_then(|l| l.as_array()).map(|l| l.len()), Some(2));
}

/// WO-002 Finding A, asserted in the engine's new home: repeated engine
/// syncs (DEFINE ... OVERWRITE) preserve changefeed history.
#[test]
fn resync_preserves_changefeed_history() {
    let (db, cfg) = fresh("msync_feed");
    seed_doctype(&db, SUPPLIER_META);
    let sync = MetadataSync { base: cfg };
    sync.sync(&db).unwrap();

    db.sql_root("CREATE supplier2:a SET sname = 'one';").unwrap();
    db.sql_root("UPDATE supplier2:a SET sname = 'two';").unwrap();
    let before = feed_len(&db);
    assert!(before >= 2, "feed has the mutations: {before}");

    // metadata change: add an optional field -> engine applies a real diff
    db.sql_root("UPDATE doctype SET fields += { fieldname: 'notes', fieldtype: 'Text' } WHERE name = 'supplier2';")
        .unwrap();
    let applied = sync.sync(&db).unwrap().applied;
    assert_eq!(applied, 1, "field addition applied through the engine");

    let after = feed_len(&db);
    assert!(after >= before, "changefeed history survived the re-sync: {before} -> {after}");
}

fn feed_len(db: &Db) -> usize {
    db.sql_root("SHOW CHANGES FOR TABLE supplier2 SINCE 1 LIMIT 1000;")
        .unwrap()
        .as_array()
        .map(std::vec::Vec::len)
        .unwrap_or(0)
}

/// ADR-008: the child-storage flag is immutable/embedded-only in v0 —
/// 'related' refuses loudly, naming the missing tooling.
#[test]
fn related_children_refused_in_v0() {
    let dt: DocTypeDef = serde_json::from_value(serde_json::json!({
        "name": "po",
        "fields": [ { "fieldname": "items", "fieldtype": "Table", "child_storage": "related" } ]
    }))
    .unwrap();
    let err = doctype_ddl(&dt).unwrap_err();
    assert!(err.to_string().contains("promotion tooling"), "{err}");
}

/// Link fields become deps for the engine's toposort.
#[test]
fn link_fields_become_toposort_deps() {
    let dt: DocTypeDef = serde_json::from_value(serde_json::json!({
        "name": "sinv",
        "fields": [ { "fieldname": "supplier", "fieldtype": "Link", "options": ["supplier2"] } ]
    }))
    .unwrap();
    let spec = doctype_spec(&dt).unwrap();
    assert_eq!(spec.deps, vec!["supplier2".to_string()]);
    assert!(spec.schema.contains("TYPE option<record<supplier2>>"));
}
