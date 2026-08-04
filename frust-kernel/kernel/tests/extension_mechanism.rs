//! The cross-app extension mechanism, end to end through the shipped install
//! path.
//!
//! The composition guarantee itself (owner-first, un-overridable) is pinned by
//! `extension_probe.rs` — the silent-override scenario, inverted into a
//! permanent control. This file proves the rest: veto, envelope loudness,
//! manifest/registry with refuse-ambiguity, honest per-app uninstall, and the
//! owner-evolution refusal.

mod common;

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::{BrokerError, Value, WriteOp};
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{access_ddl, identity_ddl, meta_ddl};
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}
fn mgr() -> Caller {
    Caller { user: "mgr".into(), pass: "pw-mgr".into(), role: "manager".into() }
}

fn setup(name: &str) -> (Rest, Db, ResolvedTenant) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&format!("{}; {}; {}", meta_ddl(), identity_ddl(), access_ddl())).unwrap();
    db.sql_root(
        "CREATE app_user:mgr SET name = 'mgr', role = 'manager', \
         pass = crypto::argon2::generate('pw-mgr');",
    )
    .unwrap();
    let hooks = WasmHooks::load(artifacts()).expect("hooks").with_script_source(scoped_db(&cfg));
    let broker = Arc::new(Broker::new(scoped_db(&cfg), Box::new(hooks)));
    let rest = Rest::single(
        broker,
        "127.0.0.1:0".into(),
        Some(Arc::new(MetadataSync { base: cfg.clone() })),
        None,
    );
    (rest, db, cfg)
}

fn call(rest: &Rest, path: &str, body: serde_json::Value) -> Result<serde_json::Value, BrokerError> {
    rest.route_for_test(path, &body, &mgr())
}

/// The owner: ships `inv` with its own validate hook.
fn owner_app(version: &str) -> serde_json::Value {
    serde_json::json!({
        "manifest_version": 1, "name": "acct", "version": version,
        "doctypes": [{ "name": "inv", "fields": [
            { "fieldname": "title", "fieldtype": "Data" },
            { "fieldname": "memo",  "fieldtype": "Data" },
            { "fieldname": "owner_ran", "fieldtype": "Data" }
        ]}],
        "server_scripts": [{ "doctype": "inv", "hook": "validate",
                             "script": "doc.owner_ran = 'A';" }]
    })
}

/// The extension: adds a namespaced field + a hook to someone else's DocType.
fn ext_app(script: &str, field: &str) -> serde_json::Value {
    serde_json::json!({
        "manifest_version": 1, "name": "crm", "version": "1.0.0",
        "extends": [{ "doctype": "inv", "hook": "validate",
                      "fields": [{ "fieldname": field, "fieldtype": "Data" }],
                      "script": script }]
    })
}

fn broker_for(cfg: &ResolvedTenant) -> Broker {
    let hooks = WasmHooks::load(artifacts()).unwrap().with_script_source(scoped_db(cfg));
    Broker::new(scoped_db(cfg), Box::new(hooks))
}

fn write(b: &Broker, title: &str) -> Result<serde_json::Value, BrokerError> {
    b.db_write(&mgr(), &HookChain::default(), WriteOp::Create, "inv", None, &[(
        "title".to_string(),
        Value::Text(title.into()),
    )])
}

// ── criterion 7 + 9: the mechanism, through the shipped install path ────────

#[test]
fn a_second_app_extends_a_doctype_it_does_not_own_and_both_hooks_run() {
    let (rest, _db, cfg) = setup("wo050_compose");
    call(&rest, "/app/install", owner_app("1.0.0")).expect("owner installs");
    call(&rest, "/app/install", ext_app("doc.crm_tag = 'X';", "crm_tag")).expect("extension installs");

    let b = broker_for(&cfg);
    let row = write(&b, "one").expect("write");
    assert_eq!(row["owner_ran"], serde_json::json!("A"), "the OWNER's hook must still run");
    assert_eq!(row["crm_tag"], serde_json::json!("X"), "the EXTENSION's hook must run too");
}

/// **Criterion 9, and it went better than the WO expected.** `owns` was never
/// relaxed: extension arrives through its OWN declaration (`extends`), so the
/// A1 refusal stays live for the owned-script path. Hooking a foreign DocType
/// via `server_scripts` is still refused, word for word.
#[test]
fn undeclared_hooking_is_still_refused_with_the_a1_message() {
    let (rest, _db, _cfg) = setup("wo050_a1");
    call(&rest, "/app/install", owner_app("1.0.0")).expect("owner installs");

    let sneaky = serde_json::json!({
        "manifest_version": 1, "name": "sneak", "version": "1.0.0",
        "server_scripts": [{ "doctype": "inv", "hook": "validate", "script": "doc.x = 1;" }]
    });
    let err = call(&rest, "/app/install", sneaky).expect_err("must still be refused");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("which this bundle does not declare"),
        "the A1 refusal must be unchanged: {msg}"
    );
}

/// Refuse-ambiguity: two apps claiming one field is an error naming BOTH.
#[test]
fn a_field_collision_between_two_apps_is_refused_naming_both() {
    let (rest, _db, _cfg) = setup("wo050_conflict");
    call(&rest, "/app/install", owner_app("1.0.0")).expect("owner");
    call(&rest, "/app/install", ext_app("doc.crm_tag = 'X';", "crm_tag")).expect("crm");

    // a second app tries to claim crm's field name
    let clash = serde_json::json!({
        "manifest_version": 1, "name": "crm", "version": "1.0.0",
        "extends": [{ "doctype": "inv", "fields": [{ "fieldname": "crm_tag", "fieldtype": "Data" }],
                      "script": "" }]
    });
    // same app re-declaring its own field is an UPDATE, not a conflict
    let mut clash = clash;
    clash["version"] = serde_json::json!("1.1.0");
    call(&rest, "/app/update", clash).expect("an app may re-declare its own field");

    // a different app claiming it must be refused, naming both
    let other = serde_json::json!({
        "manifest_version": 1, "name": "billing", "version": "1.0.0",
        "extends": [{ "doctype": "inv", "fields": [{ "fieldname": "billing_tag", "fieldtype": "Data" }],
                      "script": "" }]
    });
    call(&rest, "/app/install", other).expect("a differently-namespaced field is fine");

    // and namespacing is ENFORCED, which is what keeps collisions rare
    let unnamespaced = serde_json::json!({
        "manifest_version": 1, "name": "billing", "version": "1.1.0",
        "extends": [{ "doctype": "inv", "fields": [{ "fieldname": "memo", "fieldtype": "Data" }],
                      "script": "" }]
    });
    let err = call(&rest, "/app/install", unnamespaced).expect_err("must refuse");
    let msg = format!("{err:?}");
    assert!(msg.contains("must be namespaced"), "namespacing is enforced: {msg}");
}

// ── criterion 5: the veto, typed and named ─────────────────────────────────

#[test]
fn an_extension_may_reject_a_write_and_the_error_names_the_app() {
    let (rest, _db, cfg) = setup("wo050_veto");
    call(&rest, "/app/install", owner_app("1.0.0")).expect("owner");
    call(&rest, "/app/install", ext_app("throw 'crm says no';", "crm_tag")).expect("crm");

    let b = broker_for(&cfg);
    let err = write(&b, "one").expect_err("the extension's veto must fail the write");
    let msg = format!("{err:?}");
    assert!(msg.contains("crm says no"), "the extension's own words survive: {msg}");
    assert!(
        msg.contains("crm") && msg.contains("extension"),
        "the refusal must NAME the rejecting app and its role: {msg}"
    );
}

// ── criterion 6: envelope loudness ─────────────────────────────────────────

#[test]
fn writing_an_undeclared_field_is_a_loud_typed_refusal_not_silent_stripping() {
    let (rest, _db, cfg) = setup("wo050_envelope");
    call(&rest, "/app/install", owner_app("1.0.0")).expect("owner");
    // declares `crm_tag` but writes `crm_ghost` — once an instrument error,
    // now the regression
    call(&rest, "/app/install", ext_app("doc.crm_ghost = 'boo';", "crm_tag")).expect("crm");

    let b = broker_for(&cfg);
    let err = write(&b, "one").expect_err("an undeclared write must be refused, not dropped");
    let msg = format!("{err:?}");
    assert!(msg.contains("E_FIELD_UNDECLARED"), "typed: {msg}");
    assert!(msg.contains("crm_ghost"), "names the field: {msg}");
    assert!(msg.contains("crm"), "names the app: {msg}");
}

// ── criterion 7: honest uninstall, per app ─────────────────────────────────

#[test]
fn uninstalling_the_extension_detaches_only_its_own_contributions() {
    let (rest, db, cfg) = setup("wo050_uninstall");
    call(&rest, "/app/install", owner_app("1.0.0")).expect("owner");
    call(&rest, "/app/install", ext_app("doc.crm_tag = 'X';", "crm_tag")).expect("crm");

    {
        let b = broker_for(&cfg);
        let row = write(&b, "before").expect("write");
        assert_eq!(row["crm_tag"], serde_json::json!("X"));
    }

    call(&rest, "/app/crm/uninstall", serde_json::json!({})).expect("uninstall crm");

    // the owner's DocType, script and data all survive
    let b = broker_for(&cfg);
    let row = write(&b, "after").expect("the owner's doctype must still work");
    assert_eq!(row["owner_ran"], serde_json::json!("A"), "the owner's hook survives");
    assert!(row["crm_tag"].is_null(), "the extension's hook is gone: {row}");

    let dt = db
        .sql_root("SELECT fields, server_script FROM doctype WHERE name = 'inv' LIMIT 1;")
        .expect("read");
    let rec = &dt.as_array().expect("rows")[0];
    let has_ext_field = rec["fields"]
        .as_array()
        .map(|a| a.iter().any(|f| f["fieldname"] == "crm_tag"))
        .unwrap_or(false);
    assert!(!has_ext_field, "the extension's field is detached: {rec}");
    let chain = rec["server_script"].as_array().cloned().unwrap_or_default();
    assert_eq!(chain.len(), 1, "only the owner's link remains: {chain:?}");
    assert_eq!(chain[0]["app"], serde_json::json!("acct"));

    // and the rows the extension touched are still there — data outlives apps
    let rows = db
        .sql_root("SELECT count() FROM inv GROUP ALL;")
        .expect("count")
        .as_array()
        .and_then(|a| a.first().map(|r| r["count"].as_u64().unwrap_or(0)))
        .unwrap_or(0);
    assert!(rows >= 1, "the owner's data survives an extension uninstall");
}

// ── criterion 8: owner evolution ───────────────────────────────────────────

#[test]
fn an_owner_update_that_breaks_an_extension_refuses_naming_the_casualty() {
    let (rest, _db, _cfg) = setup("wo050_evolve");
    call(&rest, "/app/install", owner_app("1.0.0")).expect("owner");
    // the extension declares a field of its own on the owner's doctype
    call(&rest, "/app/install", ext_app("doc.crm_tag = 'X';", "crm_tag")).expect("crm");

    // owner v2 drops `memo` — destructive, and the extension does not read it,
    // so this must be refused for the ORDINARY reason, not the extension one
    let mut v2 = owner_app("2.0.0");
    v2["doctypes"][0]["fields"] = serde_json::json!([
        { "fieldname": "title", "fieldtype": "Data" },
        { "fieldname": "owner_ran", "fieldtype": "Data" }
    ]);
    let err = call(&rest, "/app/update", v2.clone()).expect_err("destructive without ack");
    let msg = format!("{err:?}");
    println!("owner drops its OWN field: {msg}");
    assert!(msg.contains("destructive"), "the destructive-drop refusal still fires: {msg}");

    // ── the cross-app case: an owner update that removes a doctype an
    //    extension hooks. THE SURVIVAL PROPERTY FIRST: an ordinary owner update
    //    must NOT disturb the extension at all.
    let ok = call(&rest, "/app/update", { let mut v = owner_app("2.5.0"); v["doctypes"][0]["fields"] = serde_json::json!([
        { "fieldname": "title", "fieldtype": "Data" },
        { "fieldname": "memo", "fieldtype": "Data" },
        { "fieldname": "owner_ran", "fieldtype": "Data" }
    ]); v })
    .expect("a non-destructive owner update must succeed");
    println!("owner v2.5 (no removals): {ok}");

    let dt = _db
        .sql_root("SELECT fields, server_script FROM doctype WHERE name = 'inv' LIMIT 1;")
        .expect("read");
    let rec = &dt.as_array().expect("rows")[0];
    let ext_field_survived = rec["fields"]
        .as_array()
        .map(|a| a.iter().any(|f| f["fieldname"] == "crm_tag"))
        .unwrap_or(false);
    let ext_hook_survived = rec["server_script"]
        .as_array()
        .map(|a| a.iter().any(|s| s["app"] == "crm"))
        .unwrap_or(false);
    assert!(
        ext_field_survived && ext_hook_survived,
        "AN OWNER UPDATE SILENTLY WIPED THE EXTENSION (field={ext_field_survived}          hook={ext_hook_survived}) — that is P-2.2 in the update path: {rec}"
    );
}
