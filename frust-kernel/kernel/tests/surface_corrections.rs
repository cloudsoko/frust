//! WO-055: the WO-054 gaps, fixed — G3, G4, G5.
//! (G1 has its own file, `login_errors.rs`, because its both-sides
//! requirement is the whole point of that test.)

mod common;

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

fn a_doctype() -> serde_json::Value {
    serde_json::json!({
        "name": "widget",
        "fields": [{ "fieldname": "title", "fieldtype": "Data" }]
    })
}

// ── G4: an unknown key is refused, not discarded ───────────────────────────

/// The write-path silent-wrong: `op` was accepted and dropped, so a client
/// asking for a create *while also* sending `record` got an update in silence.
#[test]
fn an_unknown_write_field_is_refused_by_name() {
    let (rest, _db, _cfg) = setup("wo055_unknown");
    call(&rest, "/doctype", a_doctype()).expect("doctype");

    let err = call(
        &rest,
        "/write/widget",
        serde_json::json!({ "op": "create", "doc": { "title": "x" } }),
    )
    .expect_err("an ignored field must become a refusal");
    let msg = format!("{err:?}");
    println!("unknown field: {msg}");
    assert!(msg.contains("E_UNKNOWN_FIELD"), "typed: {msg}");
    assert!(msg.contains("'op'"), "names the offending key: {msg}");
    assert!(msg.contains("'doc'") && msg.contains("'record'"), "names what IS accepted: {msg}");

    // and the write it was attached to did NOT happen — a refusal that still
    // wrote would be worse than the silent ignore it replaced
    let rows = call(&rest, "/read/widget", serde_json::json!({})).expect("read");
    assert_eq!(
        rows["rows"].as_array().map(Vec::len),
        Some(0),
        "the refused request must not have written: {rows}"
    );

    // the control: the same write without the stray key succeeds
    call(&rest, "/write/widget", serde_json::json!({ "doc": { "title": "x" } }))
        .expect("a well-formed write is unaffected");
}

// ── G3: the response says which happened ───────────────────────────────────

/// `created` always carried the ROW, on updates too. Additive fix: `action`
/// says which, `record` gives the id; `created` is untouched so no existing
/// reader breaks — the shape the evolution policy requires.
#[test]
fn a_write_response_says_whether_it_created_or_updated() {
    let (rest, _db, _cfg) = setup("wo055_action");
    call(&rest, "/doctype", a_doctype()).expect("doctype");

    let made = call(&rest, "/write/widget", serde_json::json!({ "doc": { "title": "one" } }))
        .expect("create");
    println!("create -> {made}");
    assert_eq!(made["action"], serde_json::json!("created"));
    assert!(made["record"].as_str().is_some_and(|r| r.starts_with("widget:")), "{made}");
    assert!(made["created"].is_object(), "`created` still carries the row, unchanged: {made}");

    let key = made["record"].as_str().unwrap().split(':').next_back().unwrap().to_string();
    let edited = call(
        &rest,
        "/write/widget",
        serde_json::json!({ "record": key, "doc": { "title": "two" } }),
    )
    .expect("update");
    println!("update -> {edited}");
    assert_eq!(
        edited["action"],
        serde_json::json!("updated"),
        "an update must not describe itself as a creation: {edited}"
    );
    assert_eq!(edited["record"], made["record"], "same record: {edited}");
}

// ── G5: readiness is explicit ──────────────────────────────────────────────

/// **The control that stops `/ready` being decorative.** This `Rest` was built
/// without a `boot()`, so nothing marked the process ready — and the endpoint
/// says so. If it were hardcoded true, this reads true here.
#[test]
fn ready_reports_what_actually_booted() {
    let before = frust_kernel::boot::readiness();
    println!("readiness with no boot in this process: {before}");
    assert_eq!(
        before["ready"],
        serde_json::json!(false),
        "an unbooted process must not claim readiness — otherwise the endpoint is a \
         constant wearing a probe's name: {before}"
    );

    // now boot for real, and it flips with the facts attached
    let cfg = single_tenant(&"wo055_ready".to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns("REMOVE DATABASE IF EXISTS wo055_ready; DEFINE DATABASE wo055_ready;").unwrap();
    let sync = MetadataSync { base: cfg.clone() };
    let report = frust_kernel::boot::boot(&db, &frust_kernel::boot::BootOptions::default(), &sync)
        .expect("boot");

    let after = frust_kernel::boot::readiness();
    println!("readiness after boot: {after}");
    assert_eq!(after["ready"], serde_json::json!(true));
    let mine = after["tenants"]
        .as_array()
        .and_then(|a| a.iter().find(|t| t["tenant"] == "wo055_ready"))
        .expect("the booted tenant is named");
    assert_eq!(
        mine["meta_version"],
        serde_json::json!(report.meta_version),
        "readiness reports the meta version boot actually applied, not a guess: {mine}"
    );
}
