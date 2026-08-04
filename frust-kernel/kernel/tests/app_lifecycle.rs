//! Install / enable / disable / update, through the kernel's manager surface.
//!
//! The load-bearing claim under test is **detach without data loss**: disable
//! stops an app acting without touching a row, and enable restores what was
//! there rather than reconstructing it. The honest-uninstall answer has to
//! describe what these tests actually prove, so they assert the data survives
//! explicitly rather than by omission.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::*;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::single_tenant;
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{meta_ddl, APP_TABLE};
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

fn rest_for(name: &str) -> (Rest, Db) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&meta_ddl()).unwrap();
    db.sql_root(
        "CREATE app_user:mgr SET name = 'mgr', role = 'manager', pass = crypto::argon2::generate('pw-mgr'); \
         CREATE app_user:clerk SET name = 'clerk', role = 'clerk', pass = crypto::argon2::generate('pw-clerk');",
    )
    .unwrap();
    let broker = Arc::new(Broker::new(
        scoped_db(&cfg),
        Box::new(WasmHooks::load(artifacts()).expect("hooks")),
    ));
    let rest = Rest::single(broker, "127.0.0.1:0".into(), Some(Arc::new(MetadataSync { base: cfg.clone() })), None);
    (rest, db)
}

fn mgr() -> Caller {
    Caller { user: "mgr".into(), pass: "pw-mgr".into(), role: "manager".into() }
}

fn bundle(version: &str, extra_field: bool) -> serde_json::Value {
    let mut fields = vec![serde_json::json!({ "fieldname": "customer", "fieldtype": "Data" })];
    if extra_field {
        fields.push(serde_json::json!({ "fieldname": "memo", "fieldtype": "Data" }));
    }
    serde_json::json!({
        "manifest_version": 1,
        "name": "acct",
        "version": version,
        "doctypes": [{ "name": "acct_bill", "fields": fields }],
        "client_scripts": [{ "doctype": "acct_bill", "script": "doc.customer = doc.customer;" }]
    })
}

fn call(rest: &Rest, path: &str, body: serde_json::Value) -> Result<serde_json::Value, BrokerError> {
    rest.route_for_test(path, &body, &mgr())
}

/// Install: validate → plan → gate → apply → registry record.
#[test]
fn install_applies_schema_attaches_metadata_and_records_the_app() {
    let (rest, db) = rest_for("app_install");

    let plan = call(&rest, "/app/plan", bundle("1.0.0", false)).expect("plan");
    println!("plan: {plan}");
    assert_eq!(plan["needs_acknowledgement"], serde_json::json!(false));
    assert!(!plan["planned"].as_array().unwrap().is_empty());

    let out = call(&rest, "/app/install", bundle("1.0.0", false)).expect("install");
    println!("install: {out}");
    assert_eq!(out["action"], serde_json::json!("installed"));

    // schema exists
    let info = db.sql_root("INFO FOR DB;").unwrap();
    assert!(info.to_string().contains("acct_bill"), "table created");

    // metadata attached, owned by the app, with its client script
    let dt = db.sql_root("SELECT app, client_script FROM doctype WHERE name = 'acct_bill';").unwrap();
    let dt = &dt.as_array().unwrap()[0];
    assert_eq!(dt["app"], serde_json::json!("acct"), "doctype is owned by the app");
    assert!(dt["client_script"].as_str().unwrap_or("").contains("doc.customer"), "script attached");

    // registry record
    let apps = call(&rest, "/app", serde_json::json!({})).expect("list");
    assert_eq!(apps["apps"][0]["name"], serde_json::json!("acct"));
    assert_eq!(apps["apps"][0]["enabled"], serde_json::json!(true));
}

/// Installing twice is refused by name — an app is a versioned thing, and
/// "install over the top" is how Frappe app state becomes unknowable.
#[test]
fn installing_an_installed_app_is_refused_and_points_at_update() {
    let (rest, _db) = rest_for("app_twice");
    call(&rest, "/app/install", bundle("1.0.0", false)).expect("first install");
    let err = call(&rest, "/app/install", bundle("1.0.0", false)).expect_err("second refused");
    let msg = format!("{err:?}");
    println!("refused: {msg}");
    assert!(msg.contains("already installed"), "{msg}");
    assert!(msg.contains("use update"), "the refusal says what to do instead: {msg}");
}

/// **Detach without data loss** — the load-bearing claim, and the sentence the
/// honest-uninstall answer will have to stand behind.
#[test]
fn disable_detaches_without_touching_data_and_enable_restores() {
    let (rest, db) = rest_for("app_disable");
    call(&rest, "/app/install", bundle("1.0.0", false)).expect("install");

    // put real data in the app's table, through the broker
    let b = Arc::new(Broker::new(
        scoped_db(&single_tenant(&"app_disable".to_string()).expect("tenancy")),
        Box::new(WasmHooks::load(artifacts()).expect("hooks")),
    ));
    b.db_write(
        &mgr(),
        &HookChain::default(),
        WriteOp::Create,
        "acct_bill",
        None,
        &[("customer".to_string(), Value::Text("Acme".into()))],
    )
    .expect("write a row");

    let out = call(&rest, "/app/acct/disable", serde_json::json!({})).expect("disable");
    println!("disable: {out}");
    assert_eq!(out["enabled"], serde_json::json!(false));
    assert_eq!(out["data_removed"], serde_json::json!(false));

    // the script is detached...
    let dt = db.sql_root("SELECT client_script FROM doctype WHERE name = 'acct_bill';").unwrap();
    assert!(
        dt.as_array().unwrap()[0]["client_script"].is_null(),
        "client script detached on disable"
    );
    // ...the TABLE and its ROW are untouched
    let rows = db.sql_root("SELECT customer FROM acct_bill;").unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1, "data survives disable");
    assert_eq!(rows.as_array().unwrap()[0]["customer"], serde_json::json!("Acme"));

    // enable RESTORES from the stored manifest, not from a reconstruction
    call(&rest, "/app/acct/enable", serde_json::json!({})).expect("enable");
    let dt = db.sql_root("SELECT client_script FROM doctype WHERE name = 'acct_bill';").unwrap();
    assert!(
        dt.as_array().unwrap()[0]["client_script"].as_str().unwrap_or("").contains("doc.customer"),
        "the exact script comes back"
    );
    let rows = db.sql_root("SELECT customer FROM acct_bill;").unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1, "still one row, never re-created");
}

/// Update: version must advance, and a field addition applies without a restart.
#[test]
fn update_advances_the_version_and_adds_the_field() {
    let (rest, db) = rest_for("app_update");
    call(&rest, "/app/install", bundle("1.0.0", false)).expect("install");

    let stale = call(&rest, "/app/update", bundle("1.0.0", true)).expect_err("same version refused");
    assert!(format!("{stale:?}").contains("does not advance"), "{stale:?}");

    let out = call(&rest, "/app/update", bundle("1.1.0", true)).expect("update");
    println!("update: {out}");
    assert_eq!(out["action"], serde_json::json!("updated"));

    let info = db.sql_root("INFO FOR TABLE acct_bill;").unwrap();
    assert!(info.to_string().contains("memo"), "the new field exists: {info}");

    let apps = call(&rest, "/app", serde_json::json!({})).expect("list");
    assert_eq!(apps["apps"][0]["version"], serde_json::json!("1.1.0"));
}

/// The prod-strictness gate doing bundle duty: a destructive update is refused without an
/// explicit acknowledgement, and the refusal NAMES what it would destroy.
#[test]
fn a_destructive_update_is_refused_until_acknowledged() {
    let (rest, db) = rest_for("app_destructive");
    call(&rest, "/app/install", bundle("1.0.0", true)).expect("install with memo");

    // v2 drops `memo`
    let err = call(&rest, "/app/update", bundle("2.0.0", false)).expect_err("must refuse");
    let msg = format!("{err:?}");
    println!("refused: {msg}");
    assert!(msg.contains("destructive"), "{msg}");
    assert!(msg.contains("memo"), "the refusal names the casualty, not just 'no': {msg}");

    // and nothing was dropped by the refused attempt
    let info = db.sql_root("INFO FOR TABLE acct_bill;").unwrap();
    assert!(info.to_string().contains("memo"), "refusal left the field alone");

    let mut ack = bundle("2.0.0", false);
    ack["acknowledge"] = serde_json::json!(true);
    let out = call(&rest, "/app/update", ack).expect("acknowledged update");
    println!("acknowledged: {out}");
    assert!(!out["destructive"].as_array().unwrap().is_empty(), "it reports what it did");
}

/// Every lifecycle action is a write to a CHANGEFEED table, so the audit trail
/// is a property of the storage rather than of remembering to log.
#[test]
fn every_lifecycle_action_lands_in_the_changefeed() {
    let (rest, db) = rest_for("app_audit");
    call(&rest, "/app/install", bundle("1.0.0", false)).expect("install");
    call(&rest, "/app/acct/disable", serde_json::json!({})).expect("disable");
    call(&rest, "/app/acct/enable", serde_json::json!({})).expect("enable");
    call(&rest, "/app/update", bundle("1.1.0", true)).expect("update");

    let feed = db
        .sql_root(&format!("SHOW CHANGES FOR TABLE {APP_TABLE} SINCE 1 LIMIT 100;"))
        .expect("changefeed");
    let text = feed.to_string();
    println!("changefeed entries: {}", feed.as_array().map(|a| a.len()).unwrap_or(0));
    assert!(feed.as_array().map(|a| a.len()).unwrap_or(0) >= 4, "one entry per action: {text}");
    assert!(text.contains("1.1.0"), "the update is visible in the feed");
}

/// A clerk cannot install, disable, or even list apps.
#[test]
fn the_app_surface_is_manager_only() {
    let (rest, _db) = rest_for("app_perm");
    let clerk = Caller { user: "clerk".into(), pass: "pw-clerk".into(), role: "clerk".into() };
    for path in ["/app", "/app/plan", "/app/install", "/app/acct/disable"] {
        let err = rest
            .route_for_test(path, &bundle("1.0.0", false), &clerk)
            .expect_err(&format!("{path} must refuse a clerk"));
        assert!(
            matches!(err, BrokerError::PermissionDenied { .. }),
            "{path}: expected permission denied, got {err:?}"
        );
    }
}
