//! The lifecycle hook set, and each event's contract **stated**.
//!
//! The founding MUST names four subscription points. Until this WO exactly one
//! of them reached a script. The other three are here — and the one that
//! decides whether the vocabulary is safe is `on_submit`'s rejection, because
//! it is the only event whose failure has to *prevent* something rather than
//! report it.

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

fn broker_for(cfg: &ResolvedTenant) -> Broker {
    let hooks = WasmHooks::load(artifacts()).unwrap().with_script_source(scoped_db(cfg));
    Broker::new(scoped_db(cfg), Box::new(hooks))
}

/// An app whose DocType subscribes to several classes at once — which is
/// itself a thing that could not be expressed before this WO.
fn app_with(scripts: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "manifest_version": 1, "name": "acct", "version": "1.0.0",
        "doctypes": [{ "name": "inv", "submittable": true, "fields": [
            { "fieldname": "title", "fieldtype": "Data" },
            { "fieldname": "stamp", "fieldtype": "Data" },
            { "fieldname": "ran", "fieldtype": "Data" }
        ]}],
        "server_scripts": scripts
    })
}

fn create(cfg: &ResolvedTenant, title: &str) -> Result<serde_json::Value, BrokerError> {
    broker_for(cfg).db_write(
        &mgr(),
        &HookChain::default(),
        WriteOp::Create,
        "inv",
        None,
        &[("title".to_string(), Value::Text(title.into()))],
    )
}

fn set_docstatus(cfg: &ResolvedTenant, key: &str, to: i64) -> Result<serde_json::Value, BrokerError> {
    broker_for(cfg).db_write(
        &mgr(),
        &HookChain::default(),
        WriteOp::Update,
        "inv",
        Some(key),
        &[("docstatus".to_string(), Value::Int(to))],
    )
}

fn key_of(row: &serde_json::Value) -> String {
    row["id"].as_str().unwrap_or_default().split(':').next_back().unwrap_or_default().to_string()
}

// ── before_insert ──────────────────────────────────────────────────────────

/// Fires on a CREATE, may mutate, and does **not** fire on an update — there
/// is no insert to be before.
#[test]
fn before_insert_stamps_a_create_and_skips_updates() {
    let (rest, _db, cfg) = setup("wo053_bi");
    call(
        &rest,
        "/app/install",
        app_with(serde_json::json!([
            { "doctype": "inv", "hook": "before_insert", "script": "doc.stamp = 'BI';" }
        ])),
    )
    .expect("install");

    let row = create(&cfg, "one").expect("create");
    println!("created: {row}");
    assert_eq!(row["stamp"], serde_json::json!("BI"), "before_insert must mutate the create");

    // an update must NOT re-run it: clear the stamp and write something else
    let k = key_of(&row);
    let updated = broker_for(&cfg)
        .db_write(
            &mgr(),
            &HookChain::default(),
            WriteOp::Update,
            "inv",
            Some(&k),
            &[("title".to_string(), Value::Text("two".into())), ("stamp".to_string(), Value::Text("cleared".into()))],
        )
        .expect("update");
    println!("updated: {updated}");
    assert_eq!(
        updated["stamp"],
        serde_json::json!("cleared"),
        "before_insert must NOT fire on an update — it re-stamped one"
    );
}

// ── the docstatus edges, and the contract that matters ─────────────────────

/// **THE LOAD-BEARING TEST.** A rejecting `on_submit` blocks the transition:
/// the document stays at docstatus 0, and it does so because the write never
/// happened — the lattice EVENT is not involved,
/// consulted, or modified.
#[test]
fn a_rejected_on_submit_leaves_docstatus_0_and_the_lattice_untouched() {
    let (rest, db, cfg) = setup("wo053_reject");
    call(
        &rest,
        "/app/install",
        app_with(serde_json::json!([
            { "doctype": "inv", "hook": "on_submit",
              "script": "throw new Error('books are closed for the period');" }
        ])),
    )
    .expect("install");

    let row = create(&cfg, "blocked").expect("create is unaffected");
    let k = key_of(&row);
    assert_eq!(row["docstatus"], serde_json::json!(0));

    let err = set_docstatus(&cfg, &k, 1).expect_err("a rejecting on_submit must block the submit");
    let msg = format!("{err:?}");
    println!("on_submit rejection: {msg}");
    assert!(msg.contains("books are closed"), "the script's own message survives: {msg}");

    // the state, asserted from the DATABASE rather than from the error
    let after = db
        .sql_root(&format!("SELECT docstatus FROM inv:{k};"))
        .expect("read back");
    println!("docstatus after the refused submit: {after}");
    assert_eq!(
        after.as_array().and_then(|a| a.first()).map(|r| r["docstatus"].clone()),
        Some(serde_json::json!(0)),
        "a rejected submit must leave the document at 0 — anything else means the write \
         partially happened"
    );

    // and the lattice is intact: 0→1 still works once the hook stops refusing
    call(
        &rest,
        "/app/update",
        {
            let mut m = app_with(serde_json::json!([
                { "doctype": "inv", "hook": "on_submit", "script": "doc.ran = 'S';" }
            ]));
            m["version"] = serde_json::json!("1.1.0");
            m
        },
    )
    .expect("update the app");
    let ok = set_docstatus(&cfg, &k, 1).expect("with the hook allowing it, 0->1 must work");
    assert_eq!(ok["docstatus"], serde_json::json!(1), "the lattice still permits 0->1: {ok}");
}

/// A non-mutating class cannot smuggle a change through. The host discards
/// what `on_submit` hands back, so the contract holds even for a script that
/// ignores it.
#[test]
fn on_submit_may_not_mutate_the_document() {
    let (rest, _db, cfg) = setup("wo053_nomut");
    call(
        &rest,
        "/app/install",
        app_with(serde_json::json!([
            { "doctype": "inv", "hook": "on_submit", "script": "doc.stamp = 'SMUGGLED';" }
        ])),
    )
    .expect("install");

    let row = create(&cfg, "x").expect("create");
    let k = key_of(&row);
    let out = set_docstatus(&cfg, &k, 1).expect("submit");
    println!("after submit: {out}");
    assert_eq!(out["docstatus"], serde_json::json!(1), "the submit happened");
    assert_ne!(
        out["stamp"],
        serde_json::json!("SMUGGLED"),
        "on_submit fires on a docstatus edge where the lattice owns the value — it may \
         refuse the transition, never rewrite the document"
    );

    // -- THE CONTROL, without which the assertion above is vacuous --
    //
    // "the stamp is absent" is equally true of a host that discards the
    // mutation and of a hook that never ran at all. So: the SAME SCRIPT TEXT,
    // moved to a class that MAY mutate. If it stamps there, it runs, and the
    // absence above is the contract being enforced rather than the hook being
    // missing.
    let mut v2 = app_with(serde_json::json!([
        { "doctype": "inv", "hook": "validate", "script": "doc.stamp = 'SMUGGLED';" }
    ]));
    v2["version"] = serde_json::json!("1.1.0");
    call(&rest, "/app/update", v2).expect("re-publish the same script under validate");

    let again = create(&cfg, "control").expect("create");
    println!("same script under a mutating class: {again}");
    assert_eq!(
        again["stamp"],
        serde_json::json!("SMUGGLED"),
        "the control failed: this script does not mutate even where it is allowed to, so          the on_submit assertion above proved nothing"
    );
}

/// The cancel edge, same contract.
#[test]
fn on_cancel_fires_on_the_edge_into_2() {
    let (rest, _db, cfg) = setup("wo053_cancel");
    call(
        &rest,
        "/app/install",
        app_with(serde_json::json!([
            { "doctype": "inv", "hook": "on_cancel", "script": "throw new Error('cancel refused');" }
        ])),
    )
    .expect("install");

    let row = create(&cfg, "c").expect("create");
    let k = key_of(&row);
    // 0 -> 1 is untouched by an on_cancel subscription
    set_docstatus(&cfg, &k, 1).expect("submit is not a cancel");
    let err = set_docstatus(&cfg, &k, 2).expect_err("on_cancel must be able to refuse");
    println!("on_cancel rejection: {err:?}");
    assert!(format!("{err:?}").contains("cancel refused"));
}

// ── composition per class ─────────────────────────────────────────────────

/// The silent-override scenario, **re-run per class**. Two apps subscribe
/// to the same event on the same DocType; both must run, owner first, neither
/// silently replacing the other.
#[test]
fn two_apps_on_the_same_class_both_run_owner_first() {
    let (rest, _db, cfg) = setup("wo053_compose");
    call(
        &rest,
        "/app/install",
        app_with(serde_json::json!([
            { "doctype": "inv", "hook": "before_insert", "script": "doc.ran = 'A';" }
        ])),
    )
    .expect("owner");
    call(
        &rest,
        "/app/install",
        serde_json::json!({
            "manifest_version": 1, "name": "crm", "version": "1.0.0",
            "extends": [{ "doctype": "inv", "hook": "before_insert",
                          "fields": [{ "fieldname": "crm_note", "fieldtype": "Data" }],
                          "script": "doc.ran = doc.ran + 'B'; doc.crm_note = 'ext';" }]
        }),
    )
    .expect("extension");

    let row = create(&cfg, "composed").expect("create");
    println!("composed: {row}");
    assert_eq!(
        row["ran"],
        serde_json::json!("AB"),
        "OWNER FIRST, THEN THE EXTENSION, BOTH — 'A' alone means the extension never ran; \
         'B' alone is P-2.2: the extension silently replaced the owner: {row}"
    );
    assert_eq!(row["crm_note"], serde_json::json!("ext"), "the extension's own field landed");
}

/// Subscriptions do not leak across classes: an app on `before_insert` is not
/// run for `on_submit`.
#[test]
fn a_subscription_is_confined_to_its_class() {
    let (rest, _db, cfg) = setup("wo053_confined");
    call(
        &rest,
        "/app/install",
        app_with(serde_json::json!([
            { "doctype": "inv", "hook": "on_submit", "script": "throw new Error('SUBMIT ONLY');" }
        ])),
    )
    .expect("install");
    // a create must be unaffected by an on_submit subscription
    let row = create(&cfg, "free").expect("an on_submit hook must not fire on a create");
    println!("create with an on_submit subscriber present: {row}");
    assert_eq!(row["docstatus"], serde_json::json!(0));
}

// ── the door ──────────────────────────────────────────────────────────────

/// A hook point that does not exist is refused AT INSTALL, naming the ones
/// that do — never accepted and then silently never fired.
#[test]
fn an_unknown_hook_point_is_refused_at_the_door() {
    let (rest, _db, _cfg) = setup("wo053_door");
    let err = call(
        &rest,
        "/app/install",
        app_with(serde_json::json!([
            { "doctype": "inv", "hook": "on_sumbit", "script": "doc.ran = 'x';" }
        ])),
    )
    .expect_err("a typo'd hook point must be a 400, not a silent no-op");
    let msg = format!("{err:?}");
    println!("door refusal: {msg}");
    assert!(msg.contains("on_sumbit"), "names the typo: {msg}");
    assert!(msg.contains("on_submit"), "and names what does exist: {msg}");
}
