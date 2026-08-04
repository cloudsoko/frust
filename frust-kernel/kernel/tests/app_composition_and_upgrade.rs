//! The last two v2.0-gate assumptions.
//!
//! **A1 — multi-app hook composition.** The scorecard called it "lightly
//! exercised"; the gate found it untested. This drives the actual question:
//! two installed apps, both declaring a server script on the SAME DocType, one
//! write. Do both hooks run?
//!
//! **A2 — major-upgrade survival.** `accept_meta_migrations_two_step` proves the
//! meta gate with `NoUserSync` and NO app installed. This installs a real app
//! (DocType + hook + rollup), drives the two-step meta upgrade, and asserts
//! the app is still *functional* afterwards — the founding upgrade-survival pain.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::boot::{boot, BootError, BootOptions, NoUserSync};
use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::*;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{meta_ddl, META_SCHEMA_VERSION, META_TABLE};
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

fn setup(dbname: &str) -> (Rest, Arc<Broker>, ResolvedTenant, Db) {
    let cfg = single_tenant(&dbname.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {dbname}; DEFINE DATABASE {dbname};")).unwrap();
    db.sql_root(&meta_ddl()).unwrap();
    db.sql_root(
        "CREATE app_user:mgr SET name='mgr', role='manager', pass=crypto::argon2::generate('pw-mgr');",
    )
    .unwrap();
    MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");
    let broker = Arc::new(Broker::new(scoped_db(&cfg), Box::new(WasmHooks::load(artifacts()).unwrap())));
    let rest = Rest::single(Arc::clone(&broker), "127.0.0.1:0".into(), Some(Arc::new(MetadataSync { base: cfg.clone() })), None);
    (rest, broker, cfg, db)
}

fn mgr() -> Caller {
    Caller { user: "mgr".into(), pass: "pw-mgr".into(), role: "manager".into() }
}

/// A broker that sees installed server scripts (the live-serve wiring).
fn scripted_broker(cfg: &ResolvedTenant) -> Arc<Broker> {
    let hooks = WasmHooks::load(artifacts()).unwrap().with_script_source(scoped_db(&cfg));
    Arc::new(Broker::new(scoped_db(&cfg), Box::new(hooks)))
}

// ── A1: multi-app hook composition ──────────────────────────────────────────

/// **The A1 question, driven.** Two apps, each declaring a server script on the
/// same DocType, then one write.
///
/// This test asserts *what actually happens* and names it, rather than assuming
/// composition works. `sync::load_server_script` reads a single
/// `doctype.server_script` field (`LIMIT 1`), so the structural prediction is
/// "the second install overwrites the first" — the test exists to confirm or
/// refute that on the real install path.
#[test]
fn two_apps_declaring_hooks_on_one_doctype() {
    let (rest, _b, cfg, _db) = setup("ac_compose");

    // app ALPHA owns the DocType and stamps `alpha`
    let alpha = serde_json::json!({
        "manifest_version": 1, "name": "alpha", "version": "1.0.0", "label": "Alpha",
        "doctypes": [ { "name": "shared_doc", "fields": [
            { "fieldname": "title", "fieldtype": "Data" },
            { "fieldname": "alpha",  "fieldtype": "Data" },
            { "fieldname": "beta",   "fieldtype": "Data" } ] } ],
        "server_scripts": [ { "doctype": "shared_doc", "hook": "validate",
            "script": "doc.alpha = 'alpha-ran';" } ]
    });
    rest.route_for_test("/app/install", &alpha, &mgr()).expect("install alpha");

    // app BETA declares a hook on the SAME DocType (it does not redeclare it)
    let beta = serde_json::json!({
        "manifest_version": 1, "name": "beta", "version": "1.0.0", "label": "Beta",
        "doctypes": [],
        "server_scripts": [ { "doctype": "shared_doc", "hook": "validate",
            "script": "doc.beta = 'beta-ran';" } ]
    });
    let beta_install = rest.route_for_test("/app/install", &beta, &mgr());
    println!("beta install -> {beta_install:?}");

    let b = scripted_broker(&cfg);
    let out = b
        .db_write(&mgr(), &HookChain::default(), WriteOp::Create, "shared_doc", None, &[
            ("title".to_string(), Value::Text("t".into())),
        ])
        .expect("the write itself must succeed");
    println!("composed doc -> {out}");

    let alpha_ran = out.get("alpha").and_then(|v| v.as_str()) == Some("alpha-ran");
    let beta_ran = out.get("beta").and_then(|v| v.as_str()) == Some("beta-ran");

    // THE FINDING, asserted either way — this test records the real behaviour.
    if beta_install.is_err() {
        // acceptable outcome: the platform REFUSES the second hook loudly
        println!("A1 RESULT: second app's hook on an existing DocType is REFUSED — loud, not silent");
        assert!(alpha_ran, "alpha's hook must still run after beta was refused");
    } else if alpha_ran && beta_ran {
        println!("A1 RESULT: both hooks ran — composition works");
    } else {
        // the structurally-predicted outcome: one field is a single script slot
        panic!(
            "A1 FINDING — multi-app hook composition does NOT compose: alpha_ran={alpha_ran} \
             beta_ran={beta_ran}. `doctype.server_script` is a single slot (sync.rs LIMIT 1), so \
             installing a second app's hook on the same DocType SILENTLY replaces the first. \
             Two apps cannot both hook one DocType. Doc: {out}"
        );
    }
}

/// A1's other half — the composition that IS possible, and the one the hook-composition concern
/// actually describes: **two apps installed together, each hooking its own
/// DocType**, with no cross-talk.
///
/// The pooled `ScriptSource` is keyed `(tenant, doctype)`, so the structural
/// expectation is isolation — but that keying was never exercised with two
/// *apps* live at once, which is precisely the gap the gate found. A hook
/// leaking across apps would be the worst kind of multi-tenant-adjacent bug.
#[test]
fn two_installed_apps_each_hook_their_own_doctype_without_crosstalk() {
    let (rest, _b, cfg, _db) = setup("ac_two_apps");

    for (app, dt, stamp) in [("one", "one_doc", "one-ran"), ("two", "two_doc", "two-ran")] {
        let bundle = serde_json::json!({
            "manifest_version": 1, "name": app, "version": "1.0.0", "label": app,
            "doctypes": [ { "name": dt, "fields": [
                { "fieldname": "title", "fieldtype": "Data" },
                { "fieldname": "stamp", "fieldtype": "Data" } ] } ],
            "server_scripts": [ { "doctype": dt, "hook": "validate",
                "script": format!("doc.stamp = '{stamp}';") } ]
        });
        rest.route_for_test("/app/install", &bundle, &mgr()).unwrap_or_else(|e| panic!("install {app}: {e:?}"));
    }

    // BOTH apps are now installed and enabled at the same time.
    let b = scripted_broker(&cfg);
    for (dt, expected, other) in [("one_doc", "one-ran", "two-ran"), ("two_doc", "two-ran", "one-ran")] {
        let out = b
            .db_write(&mgr(), &HookChain::default(), WriteOp::Create, dt, None, &[
                ("title".to_string(), Value::Text("t".into())),
            ])
            .unwrap_or_else(|e| panic!("write {dt}: {e:?}"));
        let got = out.get("stamp").and_then(|v| v.as_str()).unwrap_or("");
        assert_eq!(got, expected, "{dt} must run ITS OWN app's hook: {out}");
        assert_ne!(got, other, "{dt} must not run the other app's hook (no cross-talk)");
    }
}

// ── A2: major-upgrade survival ──────────────────────────────────────────────

/// **The founding pain (app-survives-upgrade), driven.** Install a real app, upgrade the
/// meta-schema the two-step way, and prove the app is still FUNCTIONAL
/// — not merely that its rows survived.
///
/// "Functional" is the load-bearing word: a test that only counted rows would
/// pass over an app whose hooks had stopped firing — a
/// verification that only checks what it expects cannot catch what was silently
/// destroyed.
#[test]
fn an_installed_app_survives_a_major_meta_upgrade() {
    let name = "ac_upgrade";
    let (rest, _b, cfg, db) = setup(name);

    let app = serde_json::json!({
        "manifest_version": 1, "name": "acct2", "version": "1.0.0", "label": "Acct",
        "doctypes": [ { "name": "up_invoice", "submittable": true, "fields": [
            { "fieldname": "customer", "fieldtype": "Data", "required": true },
            { "fieldname": "total",    "fieldtype": "Currency" },
            { "fieldname": "stamp",    "fieldtype": "Data" } ],
            "aggregates": [ { "kind": "counter", "rollup": "up_ar", "key": "customer",
                "metrics": [ { "name": "charged", "field": "total" } ] } ] },
            { "name": "up_ar", "fields": [
                { "fieldname": "k", "fieldtype": "Data" },
                { "fieldname": "charged", "fieldtype": "Currency" } ] } ],
        "server_scripts": [ { "doctype": "up_invoice", "hook": "validate",
            "script": "doc.stamp = 'hook-ran';" } ]
    });
    rest.route_for_test("/app/install", &app, &mgr()).expect("install the app");

    // ── before the upgrade: the app works ──
    let b = scripted_broker(&cfg);
    let pre = b
        .db_write(&mgr(), &HookChain::default(), WriteOp::Create, "up_invoice", None, &[
            ("customer".to_string(), Value::Text("acme".into())),
            ("total".to_string(), Value::Decimal("10.00".into())),
        ])
        .expect("pre-upgrade write");
    assert_eq!(pre.get("stamp").and_then(|v| v.as_str()), Some("hook-ran"), "hook fires BEFORE upgrade");

    // ── the major upgrade, two-step ──
    // simulate a database written by an older binary
    db.sql_root(&format!("UPSERT {META_TABLE}:schema SET version = 0;")).unwrap();

    let refused = boot(&db, &BootOptions { holder: "n1".into(), accept_meta_migrations: false }, &NoUserSync)
        .expect_err("an un-acked migration must refuse");
    assert!(
        matches!(refused, BootError::MetaMigrationPending { db_version: 0, .. }),
        "refused for the right reason: {refused:?}"
    );

    let report = boot(&db, &BootOptions { holder: "n1".into(), accept_meta_migrations: true }, &NoUserSync)
        .expect("acknowledged upgrade must apply");
    assert!(report.applied_meta, "the upgrade actually ran");
    assert_eq!(report.meta_version, META_SCHEMA_VERSION);

    // ── after the upgrade: the app must still be FUNCTIONAL, not just present ──
    let rows = db.sql_root("SELECT name FROM doctype WHERE name = 'up_invoice';").unwrap();
    assert!(
        rows.as_array().map_or(false, |a| !a.is_empty()),
        "the app's DocType metadata survived the upgrade"
    );
    let kept = db.sql_root("SELECT count() AS n FROM up_invoice GROUP ALL;").unwrap();
    let n = kept.as_array().and_then(|a| a.first()).and_then(|r| r["n"].as_u64()).unwrap_or(0);
    assert_eq!(n, 1, "the app's DATA survived the upgrade");

    let reg = db.sql_root("SELECT name, version, enabled FROM installed_app WHERE name = 'acct2';").unwrap();
    assert!(
        reg.as_array().and_then(|a| a.first()).map_or(false, |r| r["enabled"] == serde_json::json!(true)),
        "the app is still registered and enabled after the upgrade: {reg}"
    );

    // THE LOAD-BEARING HALF: a write still fires the app's hook post-upgrade.
    let b2 = scripted_broker(&cfg);
    let post = b2
        .db_write(&mgr(), &HookChain::default(), WriteOp::Create, "up_invoice", None, &[
            ("customer".to_string(), Value::Text("acme".into())),
            ("total".to_string(), Value::Decimal("5.00".into())),
        ])
        .expect("post-upgrade write must succeed");
    assert_eq!(
        post.get("stamp").and_then(|v| v.as_str()),
        Some("hook-ran"),
        "THE APP'S HOOK STILL FIRES after a major meta upgrade (the founding pain): {post}"
    );
}
