//! Uninstall honesty, and per-DocType server-script delivery.
//!
//! **Uninstall is honest.** Metadata detaches, data remains, and the
//! manifest that described the metadata is itself removed. That last clause
//! is what makes uninstall the one operation that genuinely forgets
//! something, and why it is not reversible the way disable is.
//!
//! **Server scripts are delivered per DocType**, from metadata, via a
//! seam rather than env inheritance. Before this, every server-side write ran
//! the engine's built-in default. The principle that scripts are data and
//! live-mutable becomes true server-side here.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::*;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{meta_ddl, APP_TABLE};
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

fn setup(name: &str) -> (Rest, Db, ResolvedTenant) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&meta_ddl()).unwrap();
    db.sql_root(
        "CREATE app_user:mgr SET name = 'mgr', role = 'manager', pass = crypto::argon2::generate('pw-mgr');",
    )
    .unwrap();
    let hooks = WasmHooks::load(artifacts()).expect("hooks").with_script_source();
    let broker = Arc::new(Broker::new(scoped_db(&cfg), Box::new(hooks)));
    let rest = Rest::single(broker, "127.0.0.1:0".into(), Some(Arc::new(MetadataSync { base: cfg.clone() })), None);
    (rest, db, cfg)
}

fn mgr() -> Caller {
    Caller { user: "mgr".into(), pass: "pw-mgr".into(), role: "manager".into() }
}

fn bundle(server_script: Option<&str>) -> serde_json::Value {
    let mut m = serde_json::json!({
        "manifest_version": 1,
        "name": "acct",
        "version": "1.0.0",
        "doctypes": [{
            "name": "acct_note",
            "fields": [
                { "fieldname": "title", "fieldtype": "Data" },
                { "fieldname": "flag", "fieldtype": "Data" }
            ]
        }]
    });
    if let Some(s) = server_script {
        m["server_scripts"] =
            serde_json::json!([{ "doctype": "acct_note", "hook": "validate", "script": s }]);
    }
    m
}

fn call(rest: &Rest, path: &str, body: serde_json::Value) -> Result<serde_json::Value, BrokerError> {
    rest.route_for_test(path, &body, &mgr())
}

// â”€â”€ Criterion 5 â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// The honest answer, asserted: metadata goes, data stays, manifest forgotten.
#[test]
fn uninstall_detaches_metadata_and_keeps_every_row() {
    let (rest, db, cfg) = setup("app_uninstall");
    call(&rest, "/app/install", bundle(None)).expect("install");

    let b = Arc::new(Broker::new(
        scoped_db(&cfg),
        Box::new(WasmHooks::load(artifacts()).expect("hooks")),
    ));
    for t in ["one", "two", "three"] {
        b.db_write(&mgr(), &HookChain::default(), WriteOp::Create, "acct_note", None,
            &[("title".to_string(), Value::Text(t.into()))])
            .expect("seed");
    }

    let out = call(&rest, "/app/acct/uninstall", serde_json::json!({})).expect("uninstall");
    println!("uninstall: {out}");
    assert_eq!(out["action"], serde_json::json!("uninstalled"));
    assert_eq!(out["data_retained"][0]["table"], serde_json::json!("acct_note"));
    assert_eq!(out["data_retained"][0]["rows_retained"], serde_json::json!(3),
        "the answer STATES what survived, per table");

    // metadata gone
    let dt = db.sql_root("SELECT name FROM doctype WHERE name = 'acct_note';").unwrap();
    assert!(dt.as_array().map(|a| a.is_empty()).unwrap_or(true), "doctype metadata detached");
    // registry row gone â€” the manifest is genuinely forgotten
    let reg = db.sql_root(&format!("SELECT name FROM {APP_TABLE};")).unwrap();
    assert!(reg.as_array().map(|a| a.is_empty()).unwrap_or(true), "registry row removed");
    // DATA REMAINS â€” the whole point
    let rows = db.sql_root("SELECT title FROM acct_note;").unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 3, "every row survives uninstall");
}

/// Uninstall is not reversible the way disable is: with the manifest gone,
/// enable has nothing to restore from and says so rather than half-working.
#[test]
fn uninstall_is_not_reversible_and_says_so() {
    let (rest, _db, _cfg) = setup("app_unrev");
    call(&rest, "/app/install", bundle(None)).expect("install");
    call(&rest, "/app/acct/uninstall", serde_json::json!({})).expect("uninstall");

    let err = call(&rest, "/app/acct/enable", serde_json::json!({})).expect_err("nothing to enable");
    println!("enable after uninstall: {err:?}");
    assert!(matches!(err, BrokerError::UnknownDoctype { .. }));
}

/// PM ruling: a disabled app's route is INDISTINGUISHABLE from an unknown one.
/// 403 would confirm the route is real-but-forbidden; that is the leak.
#[test]
fn a_disabled_route_is_indistinguishable_from_an_unknown_one() {
    let (rest, _db, _cfg) = setup("app_404");
    let mut b = bundle(None);
    b["routes"] = serde_json::json!([{ "path": "ledger", "component": "plugin_demo.wasm" }]);
    b["components"] = serde_json::json!(["plugin_demo.wasm"]);
    call(&rest, "/app/install", b).expect("install");
    call(&rest, "/app/acct/disable", serde_json::json!({})).expect("disable");

    let disabled = call(&rest, "/app/acct/ledger", serde_json::json!({})).expect_err("disabled");
    let unknown = call(&rest, "/app/acct/nosuch", serde_json::json!({})).expect_err("unknown");
    println!("disabled: {disabled:?}\nunknown:  {unknown:?}");
    assert!(matches!(disabled, BrokerError::UnknownDoctype { .. }), "404, not 403");
    assert_eq!(
        std::mem::discriminant(&disabled),
        std::mem::discriminant(&unknown),
        "the two must be the same answer â€” a distinguishable refusal IS the leak"
    );
}

// â”€â”€ Criterion 6 â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

/// The finding closed: a DocType's own server script runs on a server-side
/// write, delivered from metadata.
#[test]
fn a_bundled_server_script_actually_runs_on_a_write() {
    let (rest, _db, cfg) = setup("app_srvscript");
    call(&rest, "/app/install", bundle(Some("doc.flag = \"server-script-ran\";")))
        .expect("install");

    let hooks = WasmHooks::load(artifacts()).unwrap().with_script_source();
    let b = Broker::new(scoped_db(&cfg), Box::new(hooks));
    let created = b
        .db_write(&mgr(), &HookChain::default(), WriteOp::Create, "acct_note", None,
            &[("title".to_string(), Value::Text("hello".into()))])
        .expect("write");
    println!("created: {created}");
    assert_eq!(
        created["flag"], serde_json::json!("server-script-ran"),
        "the DocType's OWN script ran server-side"
    );
}

/// A script's `throw` is the reject path, server-side, typed.
#[test]
fn a_server_script_can_reject_a_write() {
    let (rest, _db, cfg) = setup("app_srvreject");
    call(&rest, "/app/install", bundle(Some("throw new Error(\"notes must be approved first\");")))
        .expect("install");

    let hooks = WasmHooks::load(artifacts()).unwrap().with_script_source();
    let b = Broker::new(scoped_db(&cfg), Box::new(hooks));
    let err = b
        .db_write(&mgr(), &HookChain::default(), WriteOp::Create, "acct_note", None,
            &[("title".to_string(), Value::Text("x".into()))])
        .expect_err("must reject");
    println!("reject: {err:?}");
    assert!(matches!(err, BrokerError::HookRejected { .. }));
    assert!(format!("{err:?}").contains("approved first"), "the script's own words: {err:?}");
}

/// **Scripts are data, live-mutable.** Revising a script takes effect on the
/// NEXT WRITE, with no restart — the pool compares text, it does not merely
/// cache by key.
///
/// **This test revises the script through the real delivery path, and that is
/// the point.** It used to rewrite `doctype.server_script` with a direct
/// `sql_root`, which worked only because nothing cached the script. With the
/// script now generation-cached, a direct DB write is an OUT-OF-BAND edit — the
/// standing DocType-meta-cache caveat — and the cache correctly does not see
/// it. So the test now revises the script the way the product actually does,
/// through an app update, which bumps the generation.
///
/// That is a narrowing of what this test proves, stated rather than hidden:
/// it proves the DELIVERED path is live-mutable. The out-of-band case is pinned
/// by its own test below so the boundary is documented, not merely true.
#[test]
fn editing_a_server_script_takes_effect_without_a_restart() {
    let (rest, _db, cfg) = setup("app_srvlive");
    call(&rest, "/app/install", bundle(Some("doc.flag = \"v1\";"))).expect("install");

    let hooks = WasmHooks::load(artifacts()).unwrap().with_script_source();
    let b = Broker::new(scoped_db(&cfg), Box::new(hooks));
    let write = |b: &Broker| {
        b.db_write(&mgr(), &HookChain::default(), WriteOp::Create, "acct_note", None,
            &[("title".to_string(), Value::Text("t".into()))])
            .expect("write")
    };

    assert_eq!(write(&b)["flag"], serde_json::json!("v1"));

    // revise the script through the REAL path — same process, same broker,
    // no restart
    let mut v2 = bundle(Some("doc.flag = \"v2\";"));
    v2["version"] = serde_json::json!("2.0.0");
    call(&rest, "/app/update", v2).expect("update");

    assert_eq!(
        write(&b)["flag"], serde_json::json!("v2"),
        "a revised script takes effect on the next write — scripts are live-mutable, server-side"
    );
}

/// **The out-of-band caveat, pinned.**
///
/// Editing the `doctype` table directly — outside the kernel — does not bump the
/// metadata generation, so the cached script keeps serving until something does.
/// This is the same caveat the DocType meta cache has carried, and
/// it is asserted here rather than left as a comment: the boundary of a cache's
/// correctness claim should fail loudly if someone widens it by accident.
///
/// It also documents the remedy — any kernel-mediated metadata change (here, an
/// app update) republishes the truth.
#[test]
fn an_out_of_band_script_edit_is_not_seen_until_the_generation_bumps() {
    let (rest, db, cfg) = setup("app_srvoob");
    call(&rest, "/app/install", bundle(Some("doc.flag = \"v1\";"))).expect("install");

    let hooks = WasmHooks::load(artifacts()).unwrap().with_script_source();
    let b = Broker::new(scoped_db(&cfg), Box::new(hooks));
    let write = |b: &Broker| {
        b.db_write(&mgr(), &HookChain::default(), WriteOp::Create, "acct_note", None,
            &[("title".to_string(), Value::Text("t".into()))])
            .expect("write")
    };
    assert_eq!(write(&b)["flag"], serde_json::json!("v1"), "the cache must be warm first");

    // straight to the table, bypassing every kernel door
    db.sql_root("UPDATE doctype SET server_script = 'doc.flag = \"oob\";' WHERE name = 'acct_note';")
        .unwrap();
    assert_eq!(
        write(&b)["flag"], serde_json::json!("v1"),
        "an out-of-band edit is NOT picked up — this is the documented cache boundary, \
         not a bug; if this ever returns \"oob\" the cache has silently grown a TTL"
    );

    // and the remedy: any kernel-mediated metadata change republishes the truth
    let mut v2 = bundle(Some("doc.flag = \"v2\";"));
    v2["version"] = serde_json::json!("2.0.0");
    call(&rest, "/app/update", v2).expect("update");
    assert_eq!(write(&b)["flag"], serde_json::json!("v2"), "a real save is seen immediately");
}

/// A DocType with no script runs NO script â€” it must not silently inherit the
/// engine's built-in default.
#[test]
fn a_doctype_without_a_script_runs_nothing() {
    let (rest, _db, cfg) = setup("app_srvnone");
    call(&rest, "/app/install", bundle(None)).expect("install");

    let hooks = WasmHooks::load(artifacts()).unwrap().with_script_source();
    let b = Broker::new(scoped_db(&cfg), Box::new(hooks));
    let created = b
        .db_write(&mgr(), &HookChain::default(), WriteOp::Create, "acct_note", None,
            &[("title".to_string(), Value::Text("plain".into()))])
        .expect("write");
    println!("created: {created}");
    assert!(created["flag"].is_null(), "no script declared means no script ran: {created}");
    assert_eq!(created["title"], serde_json::json!("plain"));
}
