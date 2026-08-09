//! Live app metadata exported from one tenant must install equivalently in another.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::{BrokerError, Value, WriteOp};
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::meta_ddl;
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

fn setup(name: &str) -> (Rest, Db, ResolvedTenant) {
    let target = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&target);
    db.sql_root_ns(&format!(
        "REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};"
    ))
    .expect("fresh database");
    db.sql_root(&meta_ddl()).expect("meta schema");
    db.sql_root(
        "CREATE app_user:mgr SET name = 'mgr', role = 'manager', \
         pass = crypto::argon2::generate('pw-mgr');",
    )
    .expect("manager");
    let hooks = WasmHooks::load(artifacts()).expect("hooks").with_script_source();
    let broker = Arc::new(Broker::new(scoped_db(&target), Box::new(hooks)));
    let rest = Rest::single(
        broker,
        "127.0.0.1:0".into(),
        Some(Arc::new(MetadataSync {
            base: target.clone(),
        })),
        None,
    );
    (rest, db, target)
}

fn manager() -> Caller {
    Caller {
        user: "mgr".into(),
        pass: "pw-mgr".into(),
        role: "manager".into(),
    }
}

fn call(
    rest: &Rest,
    path: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, BrokerError> {
    rest.route_for_test(path, &body, &manager())
}

fn bundle() -> serde_json::Value {
    serde_json::json!({
        "manifest_version": 1,
        "name": "ledger",
        "version": "1.0.0",
        "label": "Ledger",
        "doctypes": [{
            "name": "ledger_entry",
            "fields": [
                { "fieldname": "title", "fieldtype": "Data", "required": true },
                { "fieldname": "flag", "fieldtype": "Data" }
            ]
        }],
        "client_scripts": [{
            "doctype": "ledger_entry",
            "hook": "validate",
            "script": "doc.title = doc.title;"
        }],
        "server_scripts": [{
            "doctype": "ledger_entry",
            "hook": "validate",
            "script": "doc.flag = \"exported-hook\";"
        }],
        "workflows": [{
            "name": "ledger_review",
            "doctype": "ledger_entry",
            "states": [
                { "name": "Draft", "docstatus": 0 },
                { "name": "Reviewed", "docstatus": 0 }
            ],
            "transitions": [{
                "from": "Draft", "to": "Reviewed", "role": "manager", "action": "Review"
            }]
        }],
        "notifications": [{
            "name": "ledger_created",
            "doctype": "ledger_entry",
            "event": "after_insert",
            "recipients": ["ops@example.com"],
            "subject": "Ledger entry created",
            "body": "A ledger entry was created",
            "enabled": true
        }],
        "fixtures": [{
            "doctype": "ledger_entry",
            "key": "opening",
            "values": { "title": "Opening balance", "flag": "fixture" }
        }]
    })
}

fn doctype_value(db: &Db) -> serde_json::Value {
    db.sql_root(
        "SELECT name, app, issingle, submittable, fields, aggregates, client_script, server_script \
         FROM doctype WHERE name = 'ledger_entry';",
    )
    .expect("doctype metadata")
}

fn workflow_value(db: &Db) -> serde_json::Value {
    db.sql_root(
        "SELECT name, doctype, states, transitions, state_rules FROM workflow \
         WHERE name = 'ledger_review';",
    )
    .expect("workflow metadata")
}

fn notification_value(db: &Db) -> serde_json::Value {
    db.sql_root(
        "SELECT name, doctype, event, action, condition, recipients, subject, body, enabled \
         FROM notification WHERE name = 'ledger_created';",
    )
    .expect("notification metadata")
}

fn fixture_values(db: &Db) -> (serde_json::Value, serde_json::Value) {
    let row = db
        .sql_root("SELECT title, flag FROM ledger_entry:opening;")
        .expect("fixture row");
    let provenance = db
        .sql_root(
            "SELECT app, doctype, record_key, shipped, active FROM app_fixture \
             WHERE doctype = 'ledger_entry' AND record_key = 'opening';",
        )
        .expect("fixture provenance");
    (row, provenance)
}

fn write_and_observe_hook(target: &ResolvedTenant, title: &str) -> serde_json::Value {
    let hooks = WasmHooks::load(artifacts()).expect("hooks").with_script_source();
    let broker = Broker::new(scoped_db(target), Box::new(hooks));
    broker
        .db_write(
            &manager(),
            &HookChain::default(),
            WriteOp::Create,
            "ledger_entry",
            None,
            &[("title".into(), Value::Text(title.into()))],
        )
        .expect("hooked write")
}

#[test]
fn live_export_round_trips_by_value_and_same_app_install_refuses_cleanly() {
    let suffix = std::process::id();
    let source_name = format!("export_source_{suffix}");
    let target_name = format!("export_target_{suffix}");
    let (source_rest, source_db, source_target) = setup(&source_name);
    call(&source_rest, "/app/install", bundle()).expect("install source app");

    source_db
        .sql_root(
            "UPDATE doctype:ledger_entry SET fields += { \
             fieldname: 'runtime_note', fieldtype: 'Data', required: false, options: [] };",
        )
        .expect("runtime metadata edit");
    MetadataSync {
        base: source_target.clone(),
    }
    .sync(&source_db)
    .expect("apply runtime metadata edit");

    call(
        &source_rest,
        "/doctype",
        serde_json::json!({
            "name": "desk_scratch",
            "fields": [{ "fieldname": "note", "fieldtype": "Data" }]
        }),
    )
    .expect("create unowned metadata");
    call(
        &source_rest,
        "/notification",
        serde_json::json!({
            "name": "desk_ledger_notice",
            "doctype": "ledger_entry",
            "event": "on_update",
            "recipients": ["desk@example.com"],
            "subject": "Desk-authored notice",
            "body": "This notification is not claimed by the app",
            "enabled": true
        }),
    )
    .expect("create unowned notification");
    source_db
        .sql_root(
            "CREATE workflow:desk_ledger_review CONTENT { \
             name: 'desk_ledger_review', doctype: 'ledger_entry', \
             states: [{ name: 'Draft', docstatus: 0 }], transitions: [], state_rules: [] \
             };",
        )
        .expect("create unowned workflow");

    let stored = source_db
        .sql_root("SELECT manifest FROM installed_app WHERE name = 'ledger';")
        .expect("stored manifest");
    let stored_manifest = stored.as_array().unwrap()[0]["manifest"].as_str().unwrap();
    assert!(
        !stored_manifest.contains("runtime_note"),
        "the registry remains the installed input, so the export must read live metadata"
    );

    let exported = call(
        &source_rest,
        "/app/ledger/export",
        serde_json::json!({}),
    )
    .expect("export live app");
    let manifest = exported.clone();
    assert!(
        manifest["doctypes"][0]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field["fieldname"] == "runtime_note"),
        "runtime metadata edit is in the exported definition"
    );
    assert!(
        manifest["doctypes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|doctype| doctype["name"] != "desk_scratch"),
        "unowned metadata is excluded by default"
    );
    assert_eq!(manifest["workflows"][0]["name"], "ledger_review");
    assert_eq!(manifest["notifications"][0]["name"], "ledger_created");
    assert!(manifest["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .all(|workflow| workflow["name"] != "desk_ledger_review"));
    assert!(manifest["notifications"]
        .as_array()
        .unwrap()
        .iter()
        .all(|notification| notification["name"] != "desk_ledger_notice"));
    assert_eq!(manifest["fixtures"][0]["values"]["title"], "Opening balance");

    let with_unowned = call(
        &source_rest,
        "/app/ledger/export?include_unowned=true",
        serde_json::json!({}),
    )
    .expect("explicitly include unowned metadata");
    assert!(with_unowned["doctypes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|doctype| doctype["name"] == "desk_scratch"));
    assert!(with_unowned["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|workflow| workflow["name"] == "desk_ledger_review"));
    assert!(with_unowned["notifications"]
        .as_array()
        .unwrap()
        .iter()
        .any(|notification| notification["name"] == "desk_ledger_notice"));

    let source_hooked = write_and_observe_hook(&source_target, "source proof");
    assert_eq!(source_hooked["flag"], "exported-hook");

    let (target_rest, target_db, target_target) = setup(&target_name);
    call(&target_rest, "/app/install", manifest.clone()).expect("install exported manifest");

    assert_eq!(doctype_value(&target_db), doctype_value(&source_db));
    assert_eq!(workflow_value(&target_db), workflow_value(&source_db));
    assert_eq!(notification_value(&target_db), notification_value(&source_db));
    assert_eq!(fixture_values(&target_db), fixture_values(&source_db));

    let target_hooked = write_and_observe_hook(&target_target, "target proof");
    assert_eq!(target_hooked["flag"], source_hooked["flag"]);

    let refused = call(&target_rest, "/app/install", manifest)
        .expect_err("installing the exported app over itself must refuse");
    let refusal = format!("{refused:?}");
    assert!(refusal.contains("already installed"), "{refusal}");
    assert!(refusal.contains("use update"), "{refusal}");

    call(
        &source_rest,
        "/app/ledger/disable",
        serde_json::json!({}),
    )
    .expect("disable source app");
    let disabled = call(
        &source_rest,
        "/app/ledger/export",
        serde_json::json!({}),
    )
    .expect_err("disabled live metadata must not export incompletely");
    assert!(format!("{disabled:?}").contains("enable it before exporting"));
}

#[test]
fn bundled_notifications_leave_no_live_metadata_after_update_or_uninstall() {
    let suffix = std::process::id();
    let update_name = format!("export_cleanup_update_{suffix}");
    let (update_rest, update_db, _) = setup(&update_name);
    call(&update_rest, "/app/install", bundle()).expect("install app before update");

    let mut updated = bundle();
    updated["version"] = serde_json::json!("2.0.0");
    updated["workflows"] = serde_json::json!([]);
    updated["notifications"] = serde_json::json!([]);
    call(&update_rest, "/app/update", updated).expect("remove bundled live metadata");
    assert!(workflow_value(&update_db).as_array().unwrap().is_empty());
    assert!(notification_value(&update_db).as_array().unwrap().is_empty());

    let uninstall_name = format!("export_cleanup_uninstall_{suffix}");
    let (uninstall_rest, uninstall_db, _) = setup(&uninstall_name);
    call(&uninstall_rest, "/app/install", bundle()).expect("install app before uninstall");
    call(
        &uninstall_rest,
        "/app/ledger/uninstall",
        serde_json::json!({}),
    )
    .expect("uninstall app");
    assert!(workflow_value(&uninstall_db).as_array().unwrap().is_empty());
    assert!(notification_value(&uninstall_db)
        .as_array()
        .unwrap()
        .is_empty());
}
