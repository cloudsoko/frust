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

fn fixture_bundle(app: &str, version: &str, label: &str) -> serde_json::Value {
    let doctypes = if app == "acct" {
        serde_json::json!([{
            "name": "acct_country",
            "fields": [
                { "fieldname": "code", "fieldtype": "Data", "required": true },
                { "fieldname": "label", "fieldtype": "Data" }
            ]
        }])
    } else {
        serde_json::json!([])
    };
    serde_json::json!({
        "manifest_version": 1,
        "name": app,
        "version": version,
        "doctypes": doctypes,
        "fixtures": [{
            "doctype": "acct_country",
            "key": "kenya",
            "values": { "code": "KE", "label": label }
        }]
    })
}

fn fixture_row(db: &Db) -> serde_json::Value {
    db.sql_root("SELECT code, label FROM acct_country:kenya;")
        .expect("read fixture")
        .as_array()
        .and_then(|rows| rows.first())
        .cloned()
        .expect("fixture row exists")
}

fn fixture_provenance(db: &Db) -> serde_json::Value {
    db.sql_root(
        "SELECT app, doctype, record_key, shipped, active, orphaned_at \
         FROM app_fixture WHERE doctype = 'acct_country' AND record_key = 'kenya';",
    )
    .expect("read provenance")
    .as_array()
    .and_then(|rows| rows.first())
    .cloned()
    .expect("fixture provenance exists")
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

/// The complete fixture lifecycle. Every stage is asserted by record value
/// and durable provenance rather than by an operation's success response.
#[test]
fn fixture_records_follow_the_app_lifecycle_without_silent_overwrite_or_delete() {
    let (rest, db) = rest_for("app_fixtures");

    let v1 = fixture_bundle("acct", "1.0.0", "Kenya");
    let plan = call(&rest, "/app/plan", v1.clone()).expect("fixture plan");
    assert_eq!(plan["fixtures"][0]["doctype"], serde_json::json!("acct_country"));
    assert_eq!(plan["fixtures"][0]["key"], serde_json::json!("kenya"));
    assert_eq!(plan["fixtures"][0]["action"], serde_json::json!("create"));

    call(&rest, "/app/install", v1).expect("install fixture bundle");
    assert_eq!(fixture_row(&db)["code"], serde_json::json!("KE"));
    assert_eq!(fixture_row(&db)["label"], serde_json::json!("Kenya"));
    let installed_provenance = fixture_provenance(&db);
    assert_eq!(installed_provenance["app"], serde_json::json!("acct"));
    assert_eq!(installed_provenance["active"], serde_json::json!(true));
    let shipped: serde_json::Value =
        serde_json::from_str(installed_provenance["shipped"].as_str().unwrap()).unwrap();
    assert_eq!(shipped["label"], serde_json::json!("Kenya"));

    let registry = db
        .sql_root("SELECT manifest FROM installed_app WHERE name = 'acct';")
        .expect("registry");
    let stored: serde_json::Value = serde_json::from_str(
        registry.as_array().unwrap()[0]["manifest"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(stored["fixtures"][0]["key"], serde_json::json!("kenya"));

    call(
        &rest,
        "/app/update",
        fixture_bundle("acct", "1.1.0", "Republic of Kenya"),
    )
    .expect("unchanged shipped row takes the app update");
    assert_eq!(fixture_row(&db)["label"], serde_json::json!("Republic of Kenya"));
    let shipped: serde_json::Value =
        serde_json::from_str(fixture_provenance(&db)["shipped"].as_str().unwrap()).unwrap();
    assert_eq!(shipped["label"], serde_json::json!("Republic of Kenya"));

    db.sql_root("UPDATE acct_country:kenya SET label = 'Local Name';")
        .expect("user edits fixture");
    let refused = call(
        &rest,
        "/app/update",
        fixture_bundle("acct", "1.2.0", "Kenya v2"),
    )
    .expect_err("user-modified fixture must refuse");
    match refused {
        BrokerError::FixtureRefused { code, doctype, key, apps, detail } => {
            assert_eq!(code, "FRUST:E_FIXTURE:USER_MODIFIED");
            assert_eq!(doctype, "acct_country");
            assert_eq!(key, "kenya");
            assert_eq!(apps, vec!["acct".to_string()]);
            assert!(detail.contains("acct") && detail.contains("acct_country:kenya"));
            assert!(detail.contains("acknowledge"), "refusal gives the overwrite path: {detail}");
        }
        other => panic!("expected typed fixture refusal, got {other:?}"),
    }
    assert_eq!(fixture_row(&db)["label"], serde_json::json!("Local Name"));

    let mut acknowledged = fixture_bundle("acct", "1.2.0", "Kenya v2");
    acknowledged["acknowledge"] = serde_json::json!(true);
    call(&rest, "/app/update", acknowledged).expect("acknowledged overwrite");
    assert_eq!(fixture_row(&db)["label"], serde_json::json!("Kenya v2"));

    call(&rest, "/app/acct/uninstall", serde_json::json!({})).expect("uninstall");
    assert_eq!(fixture_row(&db)["label"], serde_json::json!("Kenya v2"));
    let orphaned = fixture_provenance(&db);
    assert_eq!(orphaned["app"], serde_json::json!("acct"));
    assert_eq!(orphaned["active"], serde_json::json!(false));
    assert!(!orphaned["orphaned_at"].is_null());

    call(
        &rest,
        "/app/install",
        fixture_bundle("acct", "2.0.0", "Kenya after reinstall"),
    )
    .expect("re-install re-adopts the row");
    assert_eq!(
        fixture_row(&db)["label"],
        serde_json::json!("Kenya after reinstall"),
        "re-adoption is verified by the stored value"
    );
    assert_eq!(fixture_provenance(&db)["active"], serde_json::json!(true));

    let ambiguous = call(
        &rest,
        "/app/install",
        fixture_bundle("regional", "1.0.0", "Other Kenya"),
    )
    .expect_err("two apps cannot own one fixture row");
    match ambiguous {
        BrokerError::FixtureRefused { code, doctype, key, apps, detail } => {
            assert_eq!(code, "FRUST:E_FIXTURE:AMBIGUOUS_OWNER");
            assert_eq!(doctype, "acct_country");
            assert_eq!(key, "kenya");
            assert!(apps.contains(&"acct".to_string()) && apps.contains(&"regional".to_string()));
            assert!(detail.contains("acct") && detail.contains("regional"));
        }
        other => panic!("expected typed ambiguity refusal, got {other:?}"),
    }
}
