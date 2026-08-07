use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller};
use frust_kernel::contract::BrokerError;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{access_ddl, identity_ddl, meta_ddl};
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    GUARD
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn mgr() -> Caller {
    Caller {
        user: "mgr".into(),
        pass: "pw-mgr".into(),
        role: "manager".into(),
    }
}

fn setup(name: &str) -> (Rest, Db, ResolvedTenant) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!(
        "REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};"
    ))
    .unwrap();
    db.sql_root(&format!(
        "{}; {}; {}",
        meta_ddl(),
        identity_ddl(),
        access_ddl()
    ))
    .unwrap();
    db.sql_root(
        "CREATE app_user:mgr SET name = 'mgr', role = 'manager', \
         pass = crypto::argon2::generate('pw-mgr');",
    )
    .unwrap();
    let hooks = WasmHooks::load(artifacts())
        .expect("hooks")
        .with_script_source(scoped_db(&cfg));
    let broker = Arc::new(Broker::new(scoped_db(&cfg), Box::new(hooks)));
    let rest = Rest::single(
        broker,
        "127.0.0.1:0".into(),
        Some(Arc::new(MetadataSync { base: cfg.clone() })),
        None,
    );
    (rest, db, cfg)
}

fn call(
    rest: &Rest,
    path: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, BrokerError> {
    rest.route_for_test(path, &body, &mgr())
}

fn settings_meta() -> serde_json::Value {
    serde_json::json!({
        "name": "settings",
        "label": "Settings",
        "issingle": true,
        "fields": [
            { "fieldname": "title", "fieldtype": "Data", "required": true },
            { "fieldname": "budget", "fieldtype": "Currency", "required": true },
            { "fieldname": "notes", "fieldtype": "Text" }
        ]
    })
}

fn settings_meta_named(name: &str, issingle: bool) -> serde_json::Value {
    let mut meta = settings_meta();
    meta["name"] = serde_json::json!(name);
    meta["label"] = serde_json::json!(name.replace('_', " "));
    meta["issingle"] = serde_json::json!(issingle);
    meta["fields"] = serde_json::json!([
        { "fieldname": "title", "fieldtype": "Data", "required": true },
        { "fieldname": "budget", "fieldtype": "Currency", "required": true },
        { "fieldname": "manager_note", "fieldtype": "Text", "perm_role": "manager" }
    ]);
    meta
}

fn live_table_definition(db: &Db, table: &str) -> String {
    db.sql_root("INFO FOR DB;")
        .expect("db info")
        .get("tables")
        .and_then(|tables| tables.get(table))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("missing live table definition for {table}"))
        .to_string()
}

fn compiled_permission_clause(definition: &str, table: &str) -> String {
    let start = definition.find("PERMISSIONS").expect("permissions clause");
    let end = definition[start..]
        .find("CHANGEFEED")
        .map(|offset| start + offset)
        .unwrap_or(definition.len());
    definition[start..end].replace(table, "__DOCTYPE__")
}

#[test]
fn required_single_has_seed_row_and_first_save_completes_it() {
    let _g = lock();
    let (rest, db, _cfg) = setup("single_required");
    call(
        &rest,
        "/doctype",
        serde_json::json!({ "meta": settings_meta() }),
    )
    .expect("doctype");

    let seeded = db.sql_root("SELECT * FROM settings;").expect("seeded row");
    let rows = seeded.as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 1, "a Single is one real row: {seeded}");
    assert_eq!(rows[0]["id"], serde_json::json!("settings:settings"));
    assert!(
        rows[0].get("title").is_none(),
        "seed is intentionally incomplete: {seeded}"
    );

    let single = call(&rest, "/single/settings", serde_json::json!({})).expect("single read");
    assert_eq!(single["record"], serde_json::json!("settings:settings"));
    assert_eq!(single["row"]["id"], serde_json::json!("settings:settings"));

    let partial = call(
        &rest,
        "/single/settings/write",
        serde_json::json!({ "doc": { "title": "Acme" } }),
    )
    .expect_err("first save must complete all required fields");
    let msg = format!("{partial:?}");
    assert!(msg.contains("FRUST:E_SINGLE_REQUIRED:budget"), "{msg}");

    let saved = call(
        &rest,
        "/single/settings/write",
        serde_json::json!({
            "doc": {
                "title": "Acme",
                "budget": { "kind": "decimal", "v": "25.00" }
            }
        }),
    )
    .expect("complete first save");
    assert_eq!(saved["action"], serde_json::json!("updated"));
    assert_eq!(saved["record"], serde_json::json!("settings:settings"));

    let edited = call(
        &rest,
        "/write/settings",
        serde_json::json!({ "doc": { "notes": "persisted through generic write" } }),
    )
    .expect("generic write updates the fixed row");
    assert_eq!(edited["action"], serde_json::json!("updated"));
    assert_eq!(edited["record"], serde_json::json!("settings:settings"));

    let after = db
        .sql_root("SELECT title, budget, notes FROM settings;")
        .expect("stored");
    let rows = after.as_array().cloned().unwrap_or_default();
    assert_eq!(
        rows.len(),
        1,
        "REST must not create extra Single rows: {after}"
    );
    assert_eq!(rows[0]["title"], serde_json::json!("Acme"));
    assert_eq!(rows[0]["budget"], serde_json::json!("25"));
    assert_eq!(
        rows[0]["notes"],
        serde_json::json!("persisted through generic write")
    );
}

#[test]
fn single_rejects_non_fixed_record_keys() {
    let _g = lock();
    let (rest, _db, _cfg) = setup("single_fixed_key");
    call(
        &rest,
        "/doctype",
        serde_json::json!({ "meta": settings_meta() }),
    )
    .expect("doctype");

    let err = call(
        &rest,
        "/write/settings",
        serde_json::json!({
            "record": "other",
            "doc": {
                "title": "Acme",
                "budget": { "kind": "decimal", "v": "25.00" }
            }
        }),
    )
    .expect_err("a Single has only its fixed row");
    let msg = format!("{err:?}");
    assert!(msg.contains("only accepts record 'settings'"), "{msg}");
}

#[test]
fn single_keeps_live_compiled_permissions_byte_identical() {
    let _g = lock();
    let (rest, db, _cfg) = setup("single_permissions");
    call(
        &rest,
        "/doctype",
        serde_json::json!({ "meta": settings_meta_named("normal_settings", false) }),
    )
    .expect("normal doctype");
    call(
        &rest,
        "/doctype",
        serde_json::json!({ "meta": settings_meta_named("single_settings", true) }),
    )
    .expect("single doctype");

    let normal = compiled_permission_clause(
        &live_table_definition(&db, "normal_settings"),
        "normal_settings",
    );
    let single = compiled_permission_clause(
        &live_table_definition(&db, "single_settings"),
        "single_settings",
    );

    assert_eq!(
        normal, single,
        "Single must not fork the compiled permission clauses consumed by broker/REST"
    );
}

#[test]
fn single_refuses_delete_of_the_fixed_row() {
    let _g = lock();
    let (rest, db, _cfg) = setup("single_no_delete");
    call(
        &rest,
        "/doctype",
        serde_json::json!({ "meta": settings_meta() }),
    )
    .expect("doctype");

    // The fixed row is seeded.
    let before = db.sql_root("SELECT id FROM settings;").expect("seed");
    assert_eq!(
        before.as_array().map(Vec::len),
        Some(1),
        "the fixed Single row must exist before the delete attempt: {before}"
    );

    // Even a root delete — which bypasses table PERMISSIONS entirely — is
    // refused by the EVENT guard. This is exactly why the guard is an EVENT and
    // not a `FOR delete NONE` clause: a permission clause could never stop root,
    // and adding one would fork the compiled permissions the byte-identity test
    // pins. A manager (who passes `FOR delete WHERE role = 'manager'`) is
    // likewise stopped, since the EVENT fires for every writer.
    let err = db
        .sql_root("DELETE settings:settings;")
        .expect_err("the fixed Single row must not be deletable");
    let msg = format!("{err:?}");
    assert!(msg.contains("FRUST:E_SINGLE_NO_DELETE"), "{msg}");

    // The row survives the refused delete.
    let after = db.sql_root("SELECT id FROM settings;").expect("post-delete read");
    let rows = after.as_array().cloned().unwrap_or_default();
    assert_eq!(
        rows.len(),
        1,
        "the fixed row must survive a refused delete: {after}"
    );
    assert_eq!(rows[0]["id"], serde_json::json!("settings:settings"));
}
